#[path = "local_observation.rs"]
mod local_observation;
#[path = "parent_guard.rs"]
mod parent_guard;
use crate::sandbox::{self, PreparedLaunch};
use crate::{CODEX_ACP_VERSION, ProviderError, ProviderRemediation};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::future::Future;
use std::io::{self, Read, Write};
use std::net::{
    IpAddr, Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs,
};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::ExitStatus;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::task::{Context, Poll};
use std::thread::{self, JoinHandle as ThreadJoinHandle};
use std::time::Duration;
#[cfg(test)]
use tokio::io::AsyncWriteExt;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, BufReader};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout};
use tokio::sync::{Mutex, Notify, mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::timeout;

const MAX_PROTOCOL_FRAME: usize = 16 * 1024 * 1024;
const MAX_PROMPT_BYTES: usize = 10 * 1024 * 1024;
const MAX_RESPONSE_BYTES: usize = 256 * 1024;
const MAX_PROMPT_EVENTS: usize = 4096;
const PROMPT_EVENT_QUEUE: usize = 128;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const WRITE_TIMEOUT: Duration = Duration::from_secs(30);
const CANCEL_GRACE: Duration = Duration::from_secs(2);
const MAX_CATALOG_MODELS: usize = 2000;
const MAX_AUTH_OPEN_REQUEST_URL_BYTES: usize = 8192;
const AUTH_OPEN_REQUEST_PREFIX: &[u8] = b"MAGI_CODEX_AUTH_OPEN_REQUEST=";
const MAX_AUTH_PROGRESS_MARKER_BYTES: usize =
    AUTH_OPEN_REQUEST_PREFIX.len() + MAX_AUTH_OPEN_REQUEST_URL_BYTES;
const MAX_PROXY_CONNECTIONS: usize = 32;
const MAX_PROXY_HEADER_BYTES: usize = 8192;
const PROXY_HEADER_TIMEOUT: Duration = Duration::from_secs(5);
const PROXY_CONNECT_TIMEOUT: Duration = Duration::from_secs(12);
const MAX_UPSTREAM_ADDRESSES: usize = 16;
const DNS_LOOKUP_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_DNS_LOOKUP_WORKERS: usize = 4;
const PROXY_HOSTS: [&str; 2] = ["auth.openai.com", "chatgpt.com"];
const HOME_OPERATION_LOCK_FILE: &str = ".magi-provider-operation.lock";
const APP_SERVER_HOME_LOCK_FD: i32 = 3;
const APP_SERVER_HOME_LOCK_ENV: &str = "MAGI_PROVIDER_HOME_LOCK_FD";
static ACTIVE_DNS_LOOKUPS: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug, Clone, Copy)]
pub struct ClientFileReadError;

pub trait ClientFileReader: Send + Sync + 'static {
    fn read_text_file(
        &self,
        path: &Path,
        line: Option<u32>,
        limit: Option<u32>,
    ) -> Result<String, ClientFileReadError>;
}

struct HomeOperationLock(File);

enum CertificatePolicy {
    Configured,
    #[cfg(test)]
    PlatformRootsDiagnostic,
}

impl CertificatePolicy {
    fn apply(self, _command: &mut tokio::process::Command) {
        match self {
            Self::Configured => {}
            #[cfg(test)]
            Self::PlatformRootsDiagnostic => {
                _command.env_remove("CODEX_CA_CERTIFICATE");
            }
        }
    }
}

impl HomeOperationLock {
    fn acquire(profile_home: &Path, runtime_home_id: &str) -> Result<Self, ProviderError> {
        let home_metadata = fs::symlink_metadata(profile_home)
            .map_err(|_| ProviderError::ProfileHomeUnavailable)?;
        let canonical_home = profile_home
            .canonicalize()
            .map_err(|_| ProviderError::ProfileHomeUnavailable)?;
        if !home_metadata.file_type().is_dir()
            || home_metadata.uid() != unsafe { libc::geteuid() }
            || home_metadata.mode() & 0o077 != 0
            || canonical_home != profile_home
            || canonical_home.file_name().and_then(|name| name.to_str()) != Some(runtime_home_id)
        {
            return Err(ProviderError::ProfileHomeUnavailable);
        }

        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .mode(0o600)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .open(canonical_home.join(HOME_OPERATION_LOCK_FILE))
            .map_err(|_| ProviderError::ProfileHomeUnavailable)?;
        let lock_metadata = lock
            .metadata()
            .map_err(|_| ProviderError::ProfileHomeUnavailable)?;
        if !lock_metadata.file_type().is_file()
            || lock_metadata.uid() != unsafe { libc::geteuid() }
            || lock_metadata.mode() & 0o777 != 0o600
            || lock_metadata.nlink() != 1
            || unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0
        {
            return Err(ProviderError::ProfileHomeUnavailable);
        }
        let stable_fd = unsafe {
            libc::fcntl(
                lock.as_raw_fd(),
                libc::F_DUPFD_CLOEXEC,
                APP_SERVER_HOME_LOCK_FD,
            )
        };
        if stable_fd < APP_SERVER_HOME_LOCK_FD {
            return Err(ProviderError::ProfileHomeUnavailable);
        }
        let stable_lock = unsafe { File::from_raw_fd(stable_fd) };
        drop(lock);
        Ok(Self(stable_lock))
    }

    fn install_on(&self, command: &mut tokio::process::Command) {
        let lock_fd = self.0.as_raw_fd();
        command.env(
            APP_SERVER_HOME_LOCK_ENV,
            APP_SERVER_HOME_LOCK_FD.to_string(),
        );
        unsafe {
            command.as_std_mut().pre_exec(move || {
                if libc::dup2(lock_fd, APP_SERVER_HOME_LOCK_FD) == -1 {
                    return Err(io::Error::last_os_error());
                }
                let descriptor_flags = libc::fcntl(APP_SERVER_HOME_LOCK_FD, libc::F_GETFD);
                if descriptor_flags == -1
                    || libc::fcntl(
                        APP_SERVER_HOME_LOCK_FD,
                        libc::F_SETFD,
                        descriptor_flags & !libc::FD_CLOEXEC,
                    ) == -1
                {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMethod {
    ChatGpt,
}

impl AuthMethod {
    pub fn acp_id(self) -> &'static str {
        match self {
            Self::ChatGpt => "chat-gpt",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum AuthenticationStatus {
    Unauthenticated,
    Authenticated { method: AuthMethod },
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdapterInfo {
    pub name: Option<String>,
    pub title: Option<String>,
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HomeBindingProof {
    pub provider_profile_id: String,
    pub profile_revision: u64,
    pub explicit_codex_home_environment: bool,
    pub operating_system_sandbox_applied: bool,
    pub adapter_reported_home: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    pub protocol_version: u64,
    pub adapter: AdapterInfo,
    pub authentication_methods: Vec<AuthMethod>,
    pub home_binding: HomeBindingProof,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AvailableModel {
    pub model_id: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub context_window_tokens: Option<u64>,
    pub max_output_tokens: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AvailableMode {
    pub mode_id: String,
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub session_id: String,
    pub current_model_id: String,
    pub available_models: Vec<AvailableModel>,
    pub current_mode_id: Option<String>,
    pub available_modes: Vec<AvailableMode>,
    pub context_window_tokens: Option<u64>,
    pub max_output_tokens: Option<u64>,
    pub selected_model_available: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "text", rename_all = "snake_case")]
pub enum PromptEvent {
    TextDelta(String),
    SecurityViolation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum CancelOutcome {
    NoPromptInFlight,
    ProviderTerminal { stop_reason: String },
    LocalProcessTerminatedWithoutConfirmation,
}

#[derive(Debug, Clone)]
enum PromptTerminalOutcome {
    Pending,
    Response { stop_reason: String },
    Error(ProviderError),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderUsage {
    pub total_tokens: Option<u64>,
    pub input_tokens: Option<u64>,
    pub cached_read_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub thought_tokens: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptResult {
    pub final_text: String,
    pub stop_reason: String,
    pub usage: Option<ProviderUsage>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub buffered_events: Vec<PromptEvent>,
}

pub struct PromptHandle {
    pub events: mpsc::Receiver<PromptEvent>,
    completion: oneshot::Receiver<Result<PromptResult, ProviderError>>,
    stream: PromptStreamDiagnostic,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptStreamFailure {
    #[default]
    None,
    ByteLimit,
    EventCeiling,
    NoticeCeiling,
    ConsumerClosed,
    Cancelled,
    DeliveryTimeout,
    Protocol,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PromptStreamSnapshot {
    pub raw_events: usize,
    pub accepted_bytes: usize,
    pub pending_buffer_bytes: usize,
    pub in_flight_bytes: usize,
    pub queue_capacity: usize,
    pub queue_occupancy: usize,
    pub queue_pressure: usize,
    pub delivered_chunks: usize,
    pub coalesced_updates: usize,
    pub failure: PromptStreamFailure,
}

#[derive(Clone)]
pub struct PromptStreamDiagnostic(Arc<StdMutex<PromptStreamSnapshot>>);

impl PromptStreamDiagnostic {
    pub fn snapshot(&self) -> PromptStreamSnapshot {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

#[derive(Clone)]
struct SessionAuthority {
    info: SessionInfo,
    requested_model_id: Option<String>,
    model_pending: bool,
    model_confirmed: bool,
    mode_pending: bool,
    mode_confirmed: bool,
}

impl SessionAuthority {
    fn new(info: SessionInfo) -> Self {
        let mode_confirmed = info.available_modes.is_empty() && info.current_mode_id.is_none();
        Self {
            info,
            requested_model_id: None,
            model_pending: false,
            model_confirmed: false,
            mode_pending: false,
            mode_confirmed,
        }
    }
    fn confirmed_info(&self, session_id: &str) -> Result<SessionInfo, ProviderError> {
        if self.info.session_id != session_id
            || !self.model_confirmed
            || !self.mode_confirmed
            || self.model_pending
            || self.mode_pending
        {
            return Err(ProviderError::SessionUnavailable);
        }
        Ok(self.info.clone())
    }
    fn begin_model(&mut self, session_id: &str, model_id: &str) -> Result<(), ProviderError> {
        if self.info.session_id != session_id {
            return Err(ProviderError::SessionUnavailable);
        }
        if self.model_pending || self.mode_pending {
            return Err(ProviderError::Protocol);
        }
        if !self
            .info
            .available_models
            .iter()
            .any(|model| model.model_id == model_id)
        {
            return Err(ProviderError::ModelUnavailable);
        }
        self.requested_model_id = Some(model_id.to_owned());
        self.model_pending = true;
        self.model_confirmed = false;
        Ok(())
    }
    fn begin_mode(&mut self, session_id: &str, mode_id: &str) -> Result<(), ProviderError> {
        if self.info.session_id != session_id
            || !self
                .info
                .available_modes
                .iter()
                .any(|mode| mode.mode_id == mode_id)
        {
            return Err(ProviderError::InvalidResponse);
        }
        if self.model_pending || self.mode_pending {
            return Err(ProviderError::Protocol);
        }
        self.mode_pending = true;
        self.mode_confirmed = false;
        Ok(())
    }
    fn confirm_mode(
        &mut self,
        session_id: &str,
        mode_id: &str,
        response: &Value,
    ) -> Result<(), ProviderError> {
        if self.info.session_id != session_id || !self.mode_pending {
            return Err(ProviderError::SessionUnavailable);
        }
        if !response.is_object()
            || response
                .get("currentModeId")
                .is_some_and(|actual| actual.as_str() != Some(mode_id))
        {
            return Err(ProviderError::InvalidResponse);
        }
        self.info.current_mode_id = Some(mode_id.to_owned());
        self.mode_pending = false;
        self.mode_confirmed = true;
        Ok(())
    }
    fn confirm_model(&mut self, session_id: &str, model_id: &str) -> Result<(), ProviderError> {
        if self.info.session_id != session_id
            || !self.model_pending
            || self.requested_model_id.as_deref() != Some(model_id)
        {
            return Err(ProviderError::SessionUnavailable);
        }
        let model = self
            .info
            .available_models
            .iter()
            .find(|model| model.model_id == model_id)
            .ok_or(ProviderError::ModelUnavailable)?;
        self.info.current_model_id = model_id.to_owned();
        self.info.context_window_tokens = model.context_window_tokens;
        self.info.max_output_tokens = model.max_output_tokens;
        self.model_pending = false;
        self.model_confirmed = true;
        Ok(())
    }
}

fn validate_model_ack(response: &Value, model_id: &str) -> Result<(), ProviderError> {
    let base = split_model_id(model_id).map_or(model_id, |(base, _)| base);
    let config = config_value(response, "model");
    let current = response
        .pointer("/models/currentModelId")
        .and_then(Value::as_str);
    if (config.is_none() && current.is_none())
        || config.is_some_and(|actual| actual != base)
        || current.is_some_and(|actual| actual != model_id)
    {
        return Err(ProviderError::InvalidResponse);
    }
    Ok(())
}

pub struct PreparedSession {
    model_id: Option<String>,
    cwd: String,
}

pub struct PendingSession {
    prepared: PreparedSession,
    request: PendingRpc,
    _opening: SessionOpeningGuard,
}

pub struct PendingModelConfiguration {
    model_id: String,
    session_id: String,
    reasoning_effort: Option<String>,
    request: PendingRpc,
}

pub struct PendingCancel {
    immediate: Option<CancelOutcome>,
    terminal_outcome: Option<watch::Receiver<PromptTerminalOutcome>>,
}

struct SessionOpeningGuard(Arc<AtomicBool>);

impl Drop for SessionOpeningGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

impl PromptHandle {
    pub fn stream_diagnostic(&self) -> PromptStreamDiagnostic {
        self.stream.clone()
    }

    pub async fn finish(mut self) -> Result<PromptResult, ProviderError> {
        let mut buffered_events = Vec::new();
        let mut events_open = true;
        let completion = loop {
            tokio::select! {
                completion = &mut self.completion => break completion.map_err(|_| ProviderError::ProcessClosed)?,
                event = self.events.recv(), if events_open => match event {
                    Some(event) => buffered_events.push(event),
                    None => events_open = false,
                }
            }
        };
        while let Ok(event) = self.events.try_recv() {
            buffered_events.push(event);
        }
        match completion {
            Ok(mut result) => {
                result.buffered_events.extend(buffered_events);
                Ok(result)
            }
            Err(error) if buffered_events.is_empty() => Err(error),
            Err(source) => Err(ProviderError::PromptFailedWithEvents {
                source: Box::new(source),
                buffered_events,
            }),
        }
    }
}

struct PendingPrompt {
    events: mpsc::Sender<PromptEvent>,
    text: Mutex<String>,
    pending_text: Mutex<String>,
    delivery_wake: Notify,
    delivery_finished: AtomicBool,
    queue_pressure: AtomicUsize,
    delivered_chunks: AtomicUsize,
    coalesced_updates: AtomicUsize,
    consumer_closed: AtomicBool,
    in_flight_bytes: AtomicUsize,
    diagnostic: PromptStreamDiagnostic,
    tool_denied: AtomicBool,
    output_limited: AtomicBool,
    event_limited: AtomicBool,
    event_count: AtomicUsize,
    terminal_outcome: watch::Receiver<PromptTerminalOutcome>,
}

type PromptReservations = Arc<Mutex<HashMap<String, Arc<PendingPrompt>>>>;

struct PromptReservation {
    prompts: PromptReservations,
    session_id: String,
    state: Arc<PendingPrompt>,
    inner: Option<Arc<Inner>>,
    armed: bool,
}

impl PromptReservation {
    async fn release(&mut self) {
        remove_prompt_reservation(&self.prompts, &self.session_id, &self.state).await;
        self.armed = false;
    }
}

async fn publish_prompt_terminal(
    reservation: &mut PromptReservation,
    completion: oneshot::Sender<Result<PromptResult, ProviderError>>,
    terminal: watch::Sender<PromptTerminalOutcome>,
    result: Result<PromptResult, ProviderError>,
    outcome: PromptTerminalOutcome,
) {
    reservation.release().await;
    let _ = completion.send(result);
    let _ = terminal.send(outcome);
}

impl Drop for PromptReservation {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        // An interrupted write may already have reached the provider; never reuse that session.
        if let Some(inner) = &self.inner {
            inner.closed.store(true, Ordering::Release);
            inner.process.signal_group(libc::SIGKILL);
        }
        let prompts = self.prompts.clone();
        let session_id = self.session_id.clone();
        let state = self.state.clone();
        let inner = self.inner.clone();
        tokio::spawn(async move {
            if let Some(inner) = inner {
                close_transport(&inner, ProviderError::ProcessClosed).await;
            }
            remove_prompt_reservation(&prompts, &session_id, &state).await;
        });
    }
}

async fn remove_prompt_reservation(
    prompts: &PromptReservations,
    session_id: &str,
    state: &Arc<PendingPrompt>,
) {
    let mut prompts = prompts.lock().await;
    if prompts
        .get(session_id)
        .is_some_and(|current| Arc::ptr_eq(current, state))
    {
        prompts.remove(session_id);
    }
}

async fn reserve_prompt_authority(
    session_state: &Mutex<Option<SessionAuthority>>,
    prompts: &PromptReservations,
    session_id: &str,
    state: Arc<PendingPrompt>,
) -> Result<(), ProviderError> {
    let authority = session_state.lock().await;
    authority
        .as_ref()
        .ok_or(ProviderError::SessionUnavailable)?
        .confirmed_info(session_id)?;
    let mut prompts = prompts.lock().await;
    if prompts.contains_key(session_id) {
        return Err(ProviderError::PromptInProgress);
    }
    prompts.insert(session_id.to_owned(), state);
    Ok(())
}

async fn begin_configuration_authority(
    session_state: &Mutex<Option<SessionAuthority>>,
    prompts: &PromptReservations,
    session_id: &str,
    value: &str,
    mode: bool,
) -> Result<(), ProviderError> {
    let mut authority = session_state.lock().await;
    let state = authority
        .as_mut()
        .ok_or(ProviderError::SessionUnavailable)?;
    let prompts = prompts.lock().await;
    if prompts.contains_key(session_id) {
        return Err(ProviderError::PromptInProgress);
    }
    if mode {
        state.begin_mode(session_id, value)
    } else {
        state.begin_model(session_id, value)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct ProviderPhaseSnapshot {
    deadline: Option<std::time::Instant>,
    generation: u64,
    model_started: bool,
}
struct ProviderPhaseClock {
    state: StdMutex<ProviderPhaseSnapshot>,
    updates: watch::Sender<u64>,
}
impl ProviderPhaseClock {
    fn new(deadline: Option<std::time::Instant>) -> Self {
        Self {
            state: StdMutex::new(ProviderPhaseSnapshot {
                deadline,
                generation: 0,
                model_started: false,
            }),
            updates: watch::channel(0).0,
        }
    }
    fn snapshot(&self) -> Result<ProviderPhaseSnapshot, ProviderError> {
        self.state
            .lock()
            .map(|state| *state)
            .map_err(|_| ProviderError::ProcessClosed)
    }
    fn deadline(&self) -> Option<std::time::Instant> {
        self.snapshot()
            .map(|phase| phase.deadline)
            .unwrap_or(Some(std::time::Instant::now()))
    }
    fn transition(
        &self,
        deadline: std::time::Instant,
        closed: &AtomicBool,
    ) -> Result<(), ProviderError> {
        let mut phase = self
            .state
            .lock()
            .map_err(|_| ProviderError::ProcessClosed)?;
        let now = std::time::Instant::now();
        if closed.load(Ordering::Acquire)
            || phase.model_started
            || phase.deadline.is_none_or(|old| now >= old)
            || deadline <= now
            || deadline.duration_since(now) > Duration::from_secs(600)
        {
            return Err(ProviderError::Cancelled);
        }
        phase.deadline = Some(deadline);
        phase.generation += 1;
        phase.model_started = true;
        self.updates.send_replace(phase.generation);
        Ok(())
    }
    fn close_expired_snapshot(&self, snapshot: ProviderPhaseSnapshot, closed: &AtomicBool) -> bool {
        let Ok(phase) = self.state.lock() else {
            closed.store(true, Ordering::Release);
            return true;
        };
        if *phase != snapshot
            || phase
                .deadline
                .is_none_or(|deadline| std::time::Instant::now() < deadline)
        {
            return false;
        }
        closed.store(true, Ordering::Release);
        true
    }
}

struct Inner {
    request_deadline: Arc<ProviderPhaseClock>,
    effect_authority: Option<crate::verification::EffectAuthority>,
    writer: Mutex<ChildStdin>,
    pending: Mutex<HashMap<u64, PendingRequest>>,
    prompts: Arc<Mutex<HashMap<String, Arc<PendingPrompt>>>>,
    active_session_id: Mutex<Option<String>>,
    source_reader: Option<Arc<dyn ClientFileReader>>,
    subscription: Mutex<Option<Arc<crate::subscription::SubscriptionBroker>>>,
    subscription_authenticated: AtomicBool,
    rpc_failure: Mutex<Option<RpcFailureDiagnostic>>,
    provider_notices: Mutex<Vec<ProviderNoticeDiagnostic>>,
    event_gate: Mutex<()>,
    next_request_id: AtomicU64,
    closed: AtomicBool,
    process: Arc<ProcessControl>,
}

enum FramePurpose {
    Effect,
    CancelOwnedPrompt,
}

async fn lock_effect_writer<'a, W>(
    writer: &'a Mutex<W>,
    authority: Option<&crate::verification::EffectAuthority>,
    remediation: ProviderRemediation,
    deadline: Option<std::time::Instant>,
) -> Result<tokio::sync::MutexGuard<'a, W>, ProviderError> {
    let remaining = deadline.map_or(WRITE_TIMEOUT, |deadline| {
        deadline
            .saturating_duration_since(std::time::Instant::now())
            .min(WRITE_TIMEOUT)
    });
    let lock = timeout(remaining, writer.lock());
    let result = match authority {
        Some(authority) => tokio::select! {
            biased;
            _ = authority.cancelled() => return Err(ProviderError::Cancelled),
            result = lock => result,
        },
        None => lock.await,
    };
    result.map_err(|_| ProviderError::RpcTimeout { remediation })
}

#[cfg(test)]
async fn write_effect_frame<W: AsyncWrite + Unpin>(
    writer: &mut W,
    bytes: &[u8],
    authority: Option<&crate::verification::EffectAuthority>,
    boundary: Option<(std::time::Instant, &AtomicBool)>,
) -> Result<(), ProviderError> {
    write_authorized_effect_frame(writer, bytes, authority, boundary, None).await
}

async fn write_authorized_effect_frame<W: AsyncWrite + Unpin>(
    writer: &mut W,
    bytes: &[u8],
    authority: Option<&crate::verification::EffectAuthority>,
    boundary: Option<(std::time::Instant, &AtomicBool)>,
    authorization: Option<&crate::verification::PromptPublicationAuthorization>,
) -> Result<(), ProviderError> {
    if authorization.is_some() && authority.is_none() {
        return Err(ProviderError::Cancelled);
    }
    let remaining = || {
        boundary.map_or(WRITE_TIMEOUT, |(deadline, _)| {
            deadline
                .saturating_duration_since(std::time::Instant::now())
                .min(WRITE_TIMEOUT)
        })
    };
    let check = || {
        if boundary.is_some_and(|(deadline, closed)| {
            closed.load(Ordering::Acquire) || std::time::Instant::now() >= deadline
        }) {
            Err(ProviderError::ProcessClosed)
        } else {
            Ok(())
        }
    };
    let mut written = 0;
    while written < bytes.len() {
        let next = std::future::poll_fn(|context| {
            let mut poll = || {
                if let Err(error) = authorization
                    .map_or(Ok(()), |a| a.check())
                    .and_then(|()| check())
                {
                    return Poll::Ready(Err(error));
                }
                Pin::new(&mut *writer)
                    .poll_write(context, &bytes[written..])
                    .map_err(|_| ProviderError::ProcessClosed)
            };
            match authority {
                Some(authority) => match authority.publish(poll) {
                    Ok(value) => value.map_err(|_| ProviderError::ProcessClosed),
                    Err(error) => Poll::Ready(Err(error)),
                },
                None => poll().map_err(|_| ProviderError::ProcessClosed),
            }
        });
        let result = match authority {
            Some(authority) => tokio::select! {
                biased;
                _ = authority.cancelled() => Err(ProviderError::Cancelled),
                result = timeout(remaining(), next) => result.unwrap_or(Err(ProviderError::ProcessClosed)),
            },
            None => timeout(remaining(), next)
                .await
                .unwrap_or(Err(ProviderError::ProcessClosed)),
        };
        match result {
            Ok(0) => return Err(ProviderError::ProcessClosed),
            Ok(count) => written += count,
            Err(ProviderError::Cancelled) if written == 0 => return Err(ProviderError::Cancelled),
            Err(_) => return Err(ProviderError::ProcessClosed),
        }
    }
    let flush = std::future::poll_fn(|context| {
        let mut poll = || {
            if let Err(error) = authorization
                .map_or(Ok(()), |a| a.check())
                .and_then(|()| check())
            {
                return Poll::Ready(Err(error));
            }
            Pin::new(&mut *writer)
                .poll_flush(context)
                .map_err(|_| ProviderError::ProcessClosed)
        };
        match authority {
            Some(authority) => match authority.publish(poll) {
                Ok(value) => value.map_err(|_| ProviderError::ProcessClosed),
                Err(_) => Poll::Ready(Err(ProviderError::ProcessClosed)),
            },
            None => poll().map_err(|_| ProviderError::ProcessClosed),
        }
    });
    match authority {
        Some(authority) => tokio::select! {
            biased;
            _ = authority.cancelled() => Err(ProviderError::ProcessClosed),
            result = timeout(remaining(), flush) => result.unwrap_or(Err(ProviderError::ProcessClosed)),
        },
        None => timeout(remaining(), flush)
            .await
            .unwrap_or(Err(ProviderError::ProcessClosed)),
    }
}

struct PendingRequest {
    method: String,
    sender: oneshot::Sender<Result<Value, ProviderError>>,
}

struct PendingRpc {
    deadline: Option<std::time::Instant>,
    id: u64,
    method: String,
    response: oneshot::Receiver<Result<Value, ProviderError>>,
}

impl PendingRpc {
    async fn wait(self, inner: &Inner) -> Result<Value, ProviderError> {
        let remaining = self
            .deadline
            .map(|deadline| deadline.saturating_duration_since(std::time::Instant::now()))
            .unwrap_or(REQUEST_TIMEOUT)
            .min(REQUEST_TIMEOUT);
        let response = match timeout(remaining, self.response).await {
            Err(_) => {
                inner.pending.lock().await.remove(&self.id);
                inner.poison().await;
                return Err(ProviderError::RpcTimeout {
                    remediation: rpc_remediation(&self.method),
                });
            }
            Ok(Err(_)) => Err(ProviderError::ProcessClosed),
            Ok(Ok(response)) => response,
        };
        match response {
            Err(error @ ProviderError::RpcTimeout { .. })
            | Err(error @ ProviderError::ProcessClosed) => {
                inner.pending.lock().await.remove(&self.id);
                inner.poison().await;
                Err(error)
            }
            Err(error) => {
                inner.pending.lock().await.remove(&self.id);
                Err(error)
            }
            Ok(result) => {
                if self
                    .deadline
                    .is_some_and(|deadline| std::time::Instant::now() >= deadline)
                {
                    inner.poison().await;
                    Err(ProviderError::RpcTimeout {
                        remediation: rpc_remediation(&self.method),
                    })
                } else {
                    Ok(result)
                }
            }
        }
    }
}

struct ProcessControl {
    process_group_id: i32,
    exit_status: watch::Receiver<Option<i32>>,
    observation: StdMutex<local_observation::LocalObservation>,
}

impl ProcessControl {
    fn observation_is_fresh(&self) -> bool {
        self.observation
            .lock()
            .map(|last| last.elapsed() < Duration::from_secs(15))
            .unwrap_or(false)
    }

    fn observe_owned_process(&self) -> bool {
        if self.exit_status.borrow().is_some() || !self.observation_is_fresh() {
            return false;
        }
        if unsafe { libc::kill(self.process_group_id, 0) } != 0 {
            return false;
        }
        match self.observation.lock() {
            Ok(mut last) if last.elapsed() < Duration::from_secs(15) => {
                *last = local_observation::LocalObservation::now();
                true
            }
            _ => false,
        }
    }

    fn signal_group(&self, signal: i32) {
        signal_process_group(self.process_group_id, signal);
    }

    async fn wait_for_exit(&self, wait: Duration) -> bool {
        let mut status = self.exit_status.clone();
        if status.borrow().is_some() {
            return true;
        }
        matches!(timeout(wait, status.changed()).await, Ok(Ok(()))) && status.borrow().is_some()
    }

    async fn terminate_and_reap(&self) {
        if self.exit_status.borrow().is_some() {
            return;
        }
        self.signal_group(libc::SIGTERM);
        if self.wait_for_exit(CANCEL_GRACE).await {
            return;
        }
        self.signal_group(libc::SIGKILL);
        let mut status = self.exit_status.clone();
        loop {
            if status.borrow().is_some() || status.changed().await.is_err() {
                return;
            }
        }
    }
}

struct StartupProcessGuard {
    activity: Option<crate::verification::ActivityLease>,
    child: Arc<StdMutex<Option<Child>>>,
    home_operation_lock: Option<HomeOperationLock>,
    process: Option<Arc<ProcessControl>>,
    exit_sender: Option<watch::Sender<Option<i32>>>,
    reader_task: Option<JoinHandle<()>>,
    supervisor_task: Option<JoinHandle<()>>,
    armed: bool,
}

impl StartupProcessGuard {
    fn new(
        child: Child,
        home_operation_lock: HomeOperationLock,
        activity: Option<crate::verification::ActivityLease>,
    ) -> Self {
        Self {
            activity,
            child: Arc::new(StdMutex::new(Some(child))),
            home_operation_lock: Some(home_operation_lock),
            process: None,
            exit_sender: None,
            reader_task: None,
            supervisor_task: None,
            armed: true,
        }
    }

    fn child_id(&self) -> Option<u32> {
        self.child
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .and_then(Child::id)
    }

    async fn cleanup(&mut self) {
        if let Some(reader_task) = self.reader_task.take() {
            reader_task.abort();
            let _ = reader_task.await;
        }

        let child = {
            let mut slot = self
                .child
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if self.supervisor_task.is_none() {
                slot.take()
            } else {
                if let Some(child) = slot.as_mut() {
                    let _ = child.start_kill();
                }
                None
            }
        };
        if let Some(mut child) = child {
            let process_group_id = self
                .process
                .as_ref()
                .map(|process| process.process_group_id)
                .or_else(|| child.id().map(|id| id as i32));
            let exit_code = terminate_startup_child(&mut child, process_group_id).await;
            if let Some(exit_code) = exit_code {
                if let Some(exit_sender) = &self.exit_sender {
                    let _ = exit_sender.send(Some(exit_code));
                }
                if let Some(group) = process_group_id
                    && settle_owned_process_group(group).await
                    && let Some(activity) = self.activity.take()
                {
                    activity.settle();
                }
            }
        } else if let Some(process) = &self.process {
            process.terminate_and_reap().await;
        }

        if let Some(supervisor_task) = self.supervisor_task.take() {
            let _ = supervisor_task.await;
        }
        self.armed = false;
    }
}

impl Drop for StartupProcessGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let process_group_id = self.process.as_ref().map_or_else(
            || self.child_id().map(|process_id| process_id as i32),
            |process| Some(process.process_group_id),
        );
        if let Some(process_group_id) = process_group_id {
            signal_process_group(process_group_id, libc::SIGTERM);
            signal_process_group(process_group_id, libc::SIGKILL);
        }
        let child = {
            let mut slot = self
                .child
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if self.supervisor_task.is_none() {
                slot.take()
            } else {
                if let Some(child) = slot.as_mut() {
                    let _ = child.start_kill();
                }
                None
            }
        };
        let activity = self.activity.take();
        if let Some(mut child) = child {
            let _ = child.start_kill();
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(async move {
                    if terminate_startup_child(&mut child, process_group_id)
                        .await
                        .is_some()
                        && let Some(group) = process_group_id
                        && settle_owned_process_group(group).await
                        && let Some(activity) = activity
                    {
                        activity.settle();
                    }
                });
            }
        }
        if let Some(reader_task) = &self.reader_task {
            reader_task.abort();
        }
    }
}

struct CatchUnwindFuture<F: Future> {
    future: Pin<Box<F>>,
}

impl<F: Future> CatchUnwindFuture<F> {
    fn new(future: F) -> Self {
        Self {
            future: Box::pin(future),
        }
    }
}

impl<F: Future> Future for CatchUnwindFuture<F> {
    type Output = Result<F::Output, Box<dyn std::any::Any + Send>>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            this.future.as_mut().poll(context)
        })) {
            Ok(Poll::Pending) => Poll::Pending,
            Ok(Poll::Ready(output)) => Poll::Ready(Ok(output)),
            Err(payload) => Poll::Ready(Err(payload)),
        }
    }
}

fn signal_process_group(process_group_id: i32, signal: i32) {
    unsafe {
        libc::kill(-process_group_id, signal);
    }
}

async fn terminate_startup_child(child: &mut Child, process_group_id: Option<i32>) -> Option<i32> {
    if let Some(process_group_id) = process_group_id {
        signal_process_group(process_group_id, libc::SIGTERM);
    } else if let Some(process_id) = child.id() {
        signal_process_group(process_id as i32, libc::SIGTERM);
    }
    if let Ok(Ok(status)) = timeout(CANCEL_GRACE, child.wait()).await {
        return Some(status.code().unwrap_or(-1));
    }

    if let Some(process_group_id) = process_group_id {
        signal_process_group(process_group_id, libc::SIGKILL);
    }
    let _ = child.start_kill();
    match timeout(CANCEL_GRACE, CatchUnwindFuture::new(child.wait())).await {
        Ok(Ok(Ok(status))) => Some(status.code().unwrap_or(-1)),
        _ => None,
    }
}

async fn settle_owned_process_group(group: i32) -> bool {
    if group <= 0 {
        return false;
    }
    let deadline = tokio::time::Instant::now() + CANCEL_GRACE;
    loop {
        if unsafe { libc::kill(-group, 0) } == -1
            && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
        {
            return true;
        }
        signal_process_group(group, libc::SIGKILL);
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

struct ActiveProxySockets {
    client: TcpStream,
    upstream: Option<TcpStream>,
}

struct ProxyState {
    closed_network: bool,
    stopping: AtomicBool,
    next_connection_id: AtomicU64,
    expected_authorization: String,
    rejected_api_openai: AtomicU64,
    rejected_other: AtomicU64,
    upstream_connect_failures: AtomicU64,
    tunnels_opened: AtomicU64,
    auth_openai_tunnels: AtomicU64,
    chatgpt_tunnels: AtomicU64,
    uploaded_bytes: AtomicU64,
    downloaded_bytes: AtomicU64,
    tunnel_reset_errors: AtomicU64,
    tunnel_timeout_errors: AtomicU64,
    upload_timeout_errors: AtomicU64,
    download_timeout_errors: AtomicU64,
    tunnel_other_errors: AtomicU64,
    active: StdMutex<HashMap<u64, ActiveProxySockets>>,
    workers: StdMutex<Vec<ThreadJoinHandle<()>>>,
}

struct DnsLookupPermit;

impl Drop for DnsLookupPermit {
    fn drop(&mut self) {
        ACTIVE_DNS_LOOKUPS.fetch_sub(1, Ordering::AcqRel);
    }
}

struct ProviderNetworkProxy {
    address: SocketAddr,
    proxy_url: String,
    state: Arc<ProxyState>,
    accept_task: Option<ThreadJoinHandle<()>>,
    _activity: StdMutex<Option<crate::verification::ActivityLease>>,
}

impl ProviderNetworkProxy {
    #[cfg(test)]
    fn start() -> Result<Self, ProviderError> {
        Self::start_tracked(None)
    }

    fn start_tracked(
        activity: Option<crate::verification::ActivityLease>,
    ) -> Result<Self, ProviderError> {
        Self::start_with_policy(activity, false)
    }
    fn start_with_policy(
        activity: Option<crate::verification::ActivityLease>,
        closed_network: bool,
    ) -> Result<Self, ProviderError> {
        let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
            .map_err(|_| ProviderError::ProxyUnavailable)?;
        listener
            .set_nonblocking(true)
            .map_err(|_| ProviderError::ProxyUnavailable)?;
        let address = listener
            .local_addr()
            .map_err(|_| ProviderError::ProxyUnavailable)?;
        if !matches!(address.ip(), IpAddr::V4(ip) if ip == Ipv4Addr::LOCALHOST)
            || address.port() == 0
        {
            return Err(ProviderError::ProxyUnavailable);
        }

        let token = random_proxy_token()?;
        let encoded_credentials = base64_encode(format!("magi:{token}").as_bytes());
        let state = Arc::new(ProxyState {
            closed_network,
            stopping: AtomicBool::new(false),
            next_connection_id: AtomicU64::new(1),
            expected_authorization: format!("Basic {encoded_credentials}"),
            rejected_api_openai: AtomicU64::new(0),
            rejected_other: AtomicU64::new(0),
            upstream_connect_failures: AtomicU64::new(0),
            tunnels_opened: AtomicU64::new(0),
            auth_openai_tunnels: AtomicU64::new(0),
            chatgpt_tunnels: AtomicU64::new(0),
            uploaded_bytes: AtomicU64::new(0),
            downloaded_bytes: AtomicU64::new(0),
            tunnel_reset_errors: AtomicU64::new(0),
            tunnel_timeout_errors: AtomicU64::new(0),
            upload_timeout_errors: AtomicU64::new(0),
            download_timeout_errors: AtomicU64::new(0),
            tunnel_other_errors: AtomicU64::new(0),
            active: StdMutex::new(HashMap::new()),
            workers: StdMutex::new(Vec::new()),
        });
        let accept_state = state.clone();
        let accept_task = thread::Builder::new()
            .name("codex-provider-proxy".into())
            .spawn(move || accept_proxy_connections(listener, accept_state))
            .map_err(|_| ProviderError::ProxyUnavailable)?;

        Ok(Self {
            address,
            proxy_url: format!("http://magi:{token}@127.0.0.1:{}", address.port()),
            state,
            accept_task: Some(accept_task),
            _activity: StdMutex::new(activity),
        })
    }

    fn local_addr(&self) -> SocketAddr {
        self.address
    }

    fn proxy_url(&self) -> &str {
        &self.proxy_url
    }
}

impl ProviderNetworkProxy {
    async fn stop_and_observe(&self, deadline: std::time::Instant) -> bool {
        self.state.stopping.store(true, Ordering::Release);
        if let Ok(active) = self.state.active.lock() {
            for sockets in active.values() {
                let _ = sockets.client.shutdown(Shutdown::Both);
                if let Some(upstream) = &sockets.upstream {
                    let _ = upstream.shutdown(Shutdown::Both);
                }
            }
        } else {
            return false;
        }
        loop {
            let accept_done = self
                .accept_task
                .as_ref()
                .is_none_or(ThreadJoinHandle::is_finished);
            let workers_done = self
                .state
                .workers
                .lock()
                .map(|workers| workers.iter().all(ThreadJoinHandle::is_finished))
                .unwrap_or(false);
            let sockets_closed = self
                .state
                .active
                .lock()
                .map(|active| active.is_empty())
                .unwrap_or(false);
            if accept_done && workers_done && sockets_closed {
                let Ok(mut activity) = self._activity.lock() else {
                    return false;
                };
                activity.take();
                return true;
            }
            if std::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}
fn owned_process_exit_confirmed(process: &ProcessControl) -> bool {
    process.process_group_id > 0
        && process.exit_status.borrow().is_some()
        && unsafe { libc::kill(-process.process_group_id, 0) } == -1
        && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
}

impl Drop for ProviderNetworkProxy {
    fn drop(&mut self) {
        self.state.stopping.store(true, Ordering::Release);
        if let Some(task) = self.accept_task.take() {
            let _ = task.join();
        }
        if let Ok(active) = self.state.active.lock() {
            for sockets in active.values() {
                let _ = sockets.client.shutdown(Shutdown::Both);
                if let Some(upstream) = sockets.upstream.as_ref() {
                    let _ = upstream.shutdown(Shutdown::Both);
                }
            }
        }
        if let Ok(mut workers) = self.state.workers.lock() {
            for worker in workers.drain(..) {
                let _ = worker.join();
            }
        }
    }
}

fn accept_proxy_connections(listener: TcpListener, state: Arc<ProxyState>) {
    while !state.stopping.load(Ordering::Acquire) {
        prune_proxy_workers(&state);
        match listener.accept() {
            Ok((client, _)) => {
                // macOS accepts inherit the listener mode; worker deadlines require blocking I/O.
                if client.set_nonblocking(false).is_err() {
                    write_proxy_response(&client, 502, "Bad Gateway");
                    continue;
                }
                if state.closed_network {
                    write_proxy_response(&client, 403, "Forbidden");
                    continue;
                }
                let connection_id = state.next_connection_id.fetch_add(1, Ordering::Relaxed);
                let tracked_client = match client.try_clone() {
                    Ok(stream) => stream,
                    Err(_) => {
                        write_proxy_response(&client, 502, "Bad Gateway");
                        continue;
                    }
                };
                let can_accept = state
                    .active
                    .lock()
                    .map(|mut active| {
                        if active.len() >= MAX_PROXY_CONNECTIONS {
                            false
                        } else {
                            active.insert(
                                connection_id,
                                ActiveProxySockets {
                                    client: tracked_client,
                                    upstream: None,
                                },
                            );
                            true
                        }
                    })
                    .unwrap_or(false);
                if !can_accept {
                    write_proxy_response(&client, 503, "Service Unavailable");
                    continue;
                }
                let connection_state = state.clone();
                let spawn_error_client = client.try_clone().ok();
                match thread::Builder::new()
                    .name("codex-provider-tunnel".into())
                    .spawn(move || serve_proxy_connection(client, connection_id, connection_state))
                {
                    Ok(worker) => {
                        if let Ok(mut workers) = state.workers.lock() {
                            workers.push(worker);
                        }
                    }
                    Err(_) => {
                        if let Some(client) = spawn_error_client.as_ref() {
                            write_proxy_response(client, 503, "Service Unavailable");
                        }
                        if let Ok(mut active) = state.active.lock() {
                            active.remove(&connection_id);
                        }
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(20));
            }
            Err(_) => thread::sleep(Duration::from_millis(50)),
        }
    }
}

fn prune_proxy_workers(state: &ProxyState) {
    if let Ok(mut workers) = state.workers.lock() {
        workers.retain(|worker| !worker.is_finished());
    }
}

fn serve_proxy_connection(client: TcpStream, connection_id: u64, state: Arc<ProxyState>) {
    let _ = serve_authenticated_tunnel(&client, connection_id, &state);
    if let Ok(mut active) = state.active.lock() {
        active.remove(&connection_id);
    }
}

fn serve_authenticated_tunnel(
    client: &TcpStream,
    connection_id: u64,
    state: &Arc<ProxyState>,
) -> Result<(), ProviderError> {
    client
        .set_read_timeout(Some(PROXY_HEADER_TIMEOUT))
        .map_err(|_| ProviderError::ProxyUnavailable)?;
    client
        .set_write_timeout(Some(PROXY_HEADER_TIMEOUT))
        .map_err(|_| ProviderError::ProxyUnavailable)?;
    let host = match read_connect_request(client, state) {
        Ok(Some(host)) => host,
        Ok(None) => {
            write_proxy_response(client, 403, "Forbidden");
            return Ok(());
        }
        Err(_) => {
            write_proxy_response(client, 400, "Bad Request");
            return Ok(());
        }
    };

    let upstream = match connect_allowlisted_host(host) {
        Ok(upstream) => upstream,
        Err(_) => {
            state
                .upstream_connect_failures
                .fetch_add(1, Ordering::Relaxed);
            write_proxy_response(client, 502, "Bad Gateway");
            return Ok(());
        }
    };
    let tracked_upstream = upstream
        .try_clone()
        .map_err(|_| ProviderError::ProxyUnavailable)?;
    if let Ok(mut active) = state.active.lock() {
        if let Some(sockets) = active.get_mut(&connection_id) {
            sockets.upstream = Some(tracked_upstream);
        } else {
            let _ = upstream.shutdown(Shutdown::Both);
            return Err(ProviderError::ProxyUnavailable);
        }
    } else {
        let _ = upstream.shutdown(Shutdown::Both);
        return Err(ProviderError::ProxyUnavailable);
    }

    client
        .set_read_timeout(None)
        .map_err(|_| ProviderError::ProxyUnavailable)?;
    client
        .set_write_timeout(None)
        .map_err(|_| ProviderError::ProxyUnavailable)?;
    let mut client = client;
    client
        .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
        .map_err(|_| ProviderError::ProxyUnavailable)?;
    state.tunnels_opened.fetch_add(1, Ordering::Relaxed);
    match host {
        "auth.openai.com" => {
            state.auth_openai_tunnels.fetch_add(1, Ordering::Relaxed);
        }
        "chatgpt.com" => {
            state.chatgpt_tunnels.fetch_add(1, Ordering::Relaxed);
        }
        _ => {}
    }
    forward_established_tunnel(
        client
            .try_clone()
            .map_err(|_| ProviderError::ProxyUnavailable)?,
        upstream,
        state,
    )
}

fn forward_established_tunnel(
    mut client: TcpStream,
    upstream: TcpStream,
    state: &Arc<ProxyState>,
) -> Result<(), ProviderError> {
    let mut client_upload = client
        .try_clone()
        .map_err(|_| ProviderError::ProxyUnavailable)?;
    let mut upstream_download = upstream
        .try_clone()
        .map_err(|_| ProviderError::ProxyUnavailable)?;
    let upload_state = state.clone();
    let upload_task = thread::spawn(move || {
        let mut upstream = upstream;
        let result = copy_tunnel_bytes(
            &mut client_upload,
            &mut upstream,
            &upload_state.uploaded_bytes,
        );
        let _ = upstream.shutdown(if result.1.is_some() {
            Shutdown::Both
        } else {
            Shutdown::Write
        });
        result
    });
    let download = copy_tunnel_bytes(&mut upstream_download, &mut client, &state.downloaded_bytes);
    record_tunnel_error(state, download.1, false);
    if download.1.is_some() {
        let _ = upstream_download.shutdown(Shutdown::Both);
        let _ = client.shutdown(Shutdown::Both);
    } else {
        let _ = client.shutdown(Shutdown::Write);
    }
    let upload = upload_task
        .join()
        .map_err(|_| ProviderError::ProxyUnavailable)?;
    record_tunnel_error(state, upload.1, true);
    if download.1.is_some() || upload.1.is_some() {
        Err(ProviderError::ProxyUnavailable)
    } else {
        Ok(())
    }
}

fn copy_tunnel_bytes(
    reader: &mut impl Read,
    writer: &mut impl Write,
    counter: &AtomicU64,
) -> (u64, Option<io::ErrorKind>) {
    let mut buffer = [0u8; 8192];
    let mut transferred = 0;
    loop {
        let count = match reader.read(&mut buffer) {
            Ok(0) => return (transferred, None),
            Ok(count) => count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return (transferred, Some(error.kind())),
        };
        let mut written = 0;
        while written < count {
            match writer.write(&buffer[written..count]) {
                Ok(0) => return (transferred, Some(io::ErrorKind::WriteZero)),
                Ok(count) => {
                    written += count;
                    transferred += count as u64;
                    counter.fetch_add(count as u64, Ordering::Relaxed);
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return (transferred, Some(error.kind())),
            }
        }
    }
}

fn record_tunnel_error(state: &ProxyState, error: Option<io::ErrorKind>, upload: bool) {
    if matches!(
        error,
        Some(io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock)
    ) {
        let directional = if upload {
            &state.upload_timeout_errors
        } else {
            &state.download_timeout_errors
        };
        directional.fetch_add(1, Ordering::Relaxed);
    }
    let counter = match error {
        None => return,
        Some(
            io::ErrorKind::ConnectionReset
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::BrokenPipe,
        ) => &state.tunnel_reset_errors,
        Some(io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock) => &state.tunnel_timeout_errors,
        Some(_) => &state.tunnel_other_errors,
    };
    counter.fetch_add(1, Ordering::Relaxed);
}

fn read_connect_request(
    client: &TcpStream,
    state: &ProxyState,
) -> Result<Option<&'static str>, ProviderError> {
    let mut bytes = Vec::with_capacity(1024);
    let mut byte = [0u8; 1];
    while bytes.len() < MAX_PROXY_HEADER_BYTES {
        let count = (&*client)
            .read(&mut byte)
            .map_err(|_| ProviderError::ProxyUnavailable)?;
        if count == 0 {
            return Err(ProviderError::ProxyUnavailable);
        }
        bytes.push(byte[0]);
        if bytes.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    if !bytes.ends_with(b"\r\n\r\n") {
        return Err(ProviderError::ProxyUnavailable);
    }
    let request = std::str::from_utf8(&bytes).map_err(|_| ProviderError::ProxyUnavailable)?;
    let mut lines = request[..request.len() - 4].split("\r\n");
    let request_line = lines.next().ok_or(ProviderError::ProxyUnavailable)?;
    let mut parts = request_line.split_ascii_whitespace();
    if parts.next() != Some("CONNECT") {
        return Ok(None);
    }
    let authority = parts.next().ok_or(ProviderError::ProxyUnavailable)?;
    if parts.next() != Some("HTTP/1.1") || parts.next().is_some() {
        return Err(ProviderError::ProxyUnavailable);
    }
    let Some((host, "443")) = allowed_proxy_authority(authority) else {
        if authority.eq_ignore_ascii_case("api.openai.com:443") {
            state.rejected_api_openai.fetch_add(1, Ordering::Relaxed);
        } else {
            state.rejected_other.fetch_add(1, Ordering::Relaxed);
        }
        return Ok(None);
    };
    let mut authorization = None;
    let mut host_header = None;
    for line in lines {
        if line
            .as_bytes()
            .first()
            .is_some_and(|byte| matches!(byte, b' ' | b'\t'))
        {
            return Err(ProviderError::ProxyUnavailable);
        }
        let Some((name, value)) = line.split_once(':') else {
            return Err(ProviderError::ProxyUnavailable);
        };
        let value = value.trim_matches([' ', '\t']);
        if name.eq_ignore_ascii_case("proxy-authorization") {
            if authorization.replace(value).is_some() {
                return Ok(None);
            }
        } else if name.eq_ignore_ascii_case("host") && host_header.replace(value).is_some() {
            return Ok(None);
        }
    }
    if let Some(header) = host_header
        && allowed_proxy_authority(header).map(|(header_host, _)| header_host) != Some(host)
    {
        return Ok(None);
    }
    let Some(authorization) = authorization else {
        return Ok(None);
    };
    if !constant_time_eq(
        authorization.as_bytes(),
        state.expected_authorization.as_bytes(),
    ) {
        return Ok(None);
    }
    Ok(Some(host))
}

fn allowed_proxy_authority(authority: &str) -> Option<(&'static str, &'static str)> {
    let (host, port) = authority.rsplit_once(':')?;
    if port != "443"
        || host.is_empty()
        || host
            .bytes()
            .any(|byte| matches!(byte, b'@' | b'[' | b']' | b'%' | b'/' | b'\\'))
    {
        return None;
    }
    PROXY_HOSTS
        .iter()
        .copied()
        .find(|allowed_host| host.eq_ignore_ascii_case(allowed_host))
        .map(|host| (host, "443"))
}

fn connect_allowlisted_host(host: &'static str) -> Result<TcpStream, ProviderError> {
    let permit = reserve_dns_lookup()?;
    let (address_sender, address_receiver) = std::sync::mpsc::sync_channel(1);
    if thread::Builder::new()
        .name("codex-provider-dns".into())
        .spawn(move || {
            let _permit = permit;
            let addresses = (host, 443)
                .to_socket_addrs()
                .map(|addresses| addresses.collect::<Vec<_>>());
            let _ = address_sender.send(addresses);
        })
        .is_err()
    {
        return Err(ProviderError::ProxyUnavailable);
    }
    let addresses = address_receiver
        .recv_timeout(DNS_LOOKUP_TIMEOUT)
        .map_err(|_| ProviderError::ProxyUnavailable)?
        .map_err(|_| ProviderError::ProxyUnavailable)?;
    let addresses = addresses
        .into_iter()
        .filter(|address| is_public_provider_address(address.ip()))
        .take(MAX_UPSTREAM_ADDRESSES)
        .collect::<Vec<_>>();
    if addresses.is_empty() {
        return Err(ProviderError::ProxyUnavailable);
    }

    let deadline = std::time::Instant::now() + PROXY_CONNECT_TIMEOUT;
    let attempts = addresses.len() as u32;
    for address in addresses {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        let attempt_timeout = remaining.min(PROXY_CONNECT_TIMEOUT / attempts);
        if let Ok(stream) = TcpStream::connect_timeout(&address, attempt_timeout) {
            return Ok(stream);
        }
    }
    Err(ProviderError::ProxyUnavailable)
}

fn is_public_provider_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => {
            // Reject IANA special-purpose ranges before the unsandboxed proxy dials DNS answers.
            ![
                (Ipv4Addr::new(0, 0, 0, 0), 8),
                (Ipv4Addr::new(10, 0, 0, 0), 8),
                (Ipv4Addr::new(100, 64, 0, 0), 10),
                (Ipv4Addr::new(127, 0, 0, 0), 8),
                (Ipv4Addr::new(169, 254, 0, 0), 16),
                (Ipv4Addr::new(172, 16, 0, 0), 12),
                (Ipv4Addr::new(192, 0, 0, 0), 24),
                (Ipv4Addr::new(192, 0, 2, 0), 24),
                (Ipv4Addr::new(192, 31, 196, 0), 24),
                (Ipv4Addr::new(192, 52, 193, 0), 24),
                (Ipv4Addr::new(192, 88, 99, 0), 24),
                (Ipv4Addr::new(192, 168, 0, 0), 16),
                (Ipv4Addr::new(192, 175, 48, 0), 24),
                (Ipv4Addr::new(198, 18, 0, 0), 15),
                (Ipv4Addr::new(198, 51, 100, 0), 24),
                (Ipv4Addr::new(203, 0, 113, 0), 24),
                (Ipv4Addr::new(224, 0, 0, 0), 4),
                (Ipv4Addr::new(240, 0, 0, 0), 4),
            ]
            .into_iter()
            .any(|(network, prefix)| ipv4_in_prefix(address, network, prefix))
        }
        IpAddr::V6(address) => {
            ipv6_in_prefix(address, Ipv6Addr::new(0x2000, 0, 0, 0, 0, 0, 0, 0), 3)
                && ![
                    (Ipv6Addr::new(0x2001, 0, 0, 0, 0, 0, 0, 0), 23),
                    (Ipv6Addr::new(0x2001, 0x0db8, 0, 0, 0, 0, 0, 0), 32),
                    (Ipv6Addr::new(0x2002, 0, 0, 0, 0, 0, 0, 0), 16),
                    (Ipv6Addr::new(0x2620, 0x004f, 0x8000, 0, 0, 0, 0, 0), 48),
                    (Ipv6Addr::new(0x3fff, 0, 0, 0, 0, 0, 0, 0), 20),
                ]
                .into_iter()
                .any(|(network, prefix)| ipv6_in_prefix(address, network, prefix))
        }
    }
}

fn ipv4_in_prefix(address: Ipv4Addr, network: Ipv4Addr, prefix: u32) -> bool {
    let mask = u32::MAX << (32 - prefix);
    u32::from(address) & mask == u32::from(network) & mask
}

fn ipv6_in_prefix(address: Ipv6Addr, network: Ipv6Addr, prefix: u32) -> bool {
    let mask = u128::MAX << (128 - prefix);
    u128::from(address) & mask == u128::from(network) & mask
}

fn reserve_dns_lookup() -> Result<DnsLookupPermit, ProviderError> {
    loop {
        let active = ACTIVE_DNS_LOOKUPS.load(Ordering::Acquire);
        if active >= MAX_DNS_LOOKUP_WORKERS {
            return Err(ProviderError::ProxyUnavailable);
        }
        if ACTIVE_DNS_LOOKUPS
            .compare_exchange(active, active + 1, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            return Ok(DnsLookupPermit);
        }
    }
}

fn write_proxy_response(client: &TcpStream, status: u16, reason: &str) {
    let response =
        format!("HTTP/1.1 {status} {reason}\r\nConnection: close\r\nContent-Length: 0\r\n\r\n");
    let _ = (&*client).write_all(response.as_bytes());
}

fn random_proxy_token() -> Result<String, ProviderError> {
    let mut bytes = [0u8; 32];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut bytes))
        .map_err(|_| ProviderError::ProxyUnavailable)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let first = chunk[0];
        let second = chunk.get(1).copied().unwrap_or_default();
        let third = chunk.get(2).copied().unwrap_or_default();
        encoded.push(ALPHABET[(first >> 2) as usize] as char);
        encoded.push(ALPHABET[(((first & 0x03) << 4) | (second >> 4)) as usize] as char);
        if chunk.len() > 1 {
            encoded.push(ALPHABET[(((second & 0x0f) << 2) | (third >> 6)) as usize] as char);
        } else {
            encoded.push('=');
        }
        if chunk.len() > 2 {
            encoded.push(ALPHABET[(third & 0x3f) as usize] as char);
        } else {
            encoded.push('=');
        }
    }
    encoded
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    let mut difference = left.len() ^ right.len();
    for index in 0..left.len().max(right.len()) {
        difference |= usize::from(
            left.get(index).copied().unwrap_or_default()
                ^ right.get(index).copied().unwrap_or_default(),
        );
    }
    difference == 0
}

pub struct CodexAcpClient {
    inner: Arc<Inner>,
    prepared: PreparedLaunch,
    verified_runtime: Option<Arc<crate::VerifiedRuntimeArtifact>>,
    resource_custody: Mutex<Option<crate::verification::ArtifactClientCustody>>,
    activity_request: Option<crate::VerificationRequest>,
    _home_operation_lock: HomeOperationLock,
    _parent_lease: parent_guard::ParentLease,
    network_proxy: ProviderNetworkProxy,
    auth_methods: Mutex<Vec<AuthMethod>>,
    session_state: Mutex<Option<SessionAuthority>>,
    initializing: AtomicBool,
    initialized: AtomicBool,
    image_prompt_supported: AtomicBool,
    session_opening: Arc<AtomicBool>,
    catalog_listing: AtomicBool,
    auth_progress: watch::Receiver<AuthProgressStage>,
    auth_browser_requests: Mutex<mpsc::Receiver<Result<String, ()>>>,
    auth_progress_task: Mutex<Option<JoinHandle<()>>>,
    reader_task: Mutex<Option<JoinHandle<()>>>,
    supervisor_task: Mutex<Option<JoinHandle<()>>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthProgressStage {
    Opening,
    CheckingStatus,
    StartingLogin,
    BrowserOpening,
    LaunchRequested,
    LauncherAccepted,
    CallbackComplete,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoticeSeverity {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoticeCategory {
    CodeModeUnavailable,
    WebSocketCertificateFailure,
    WebSocketFallback,
    Configuration,
    Deprecation,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderNoticeDiagnostic {
    pub severity: NoticeSeverity,
    pub category: NoticeCategory,
}

fn parse_provider_notice(update: &Value) -> Result<ProviderNoticeDiagnostic, ProviderError> {
    let severity = match update.get("severity").and_then(Value::as_str) {
        Some("info") => NoticeSeverity::Info,
        Some("warning") => NoticeSeverity::Warning,
        Some("error") => NoticeSeverity::Error,
        _ => return Err(ProviderError::InvalidResponse),
    };
    let title = update
        .get("title")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty() && value.len() <= 4096)
        .ok_or(ProviderError::InvalidResponse)?;
    if let Some(description) = update.get("description")
        && !description.is_null()
        && description.as_str().is_none_or(|value| value.len() > 4096)
    {
        return Err(ProviderError::InvalidResponse);
    }
    let category = if title.contains("Code Mode is unavailable") {
        NoticeCategory::CodeModeUnavailable
    } else if title.contains("WebSockets")
        && (title.contains("UnknownIssuer")
            || update
                .get("description")
                .and_then(Value::as_str)
                .is_some_and(|description| description.contains("UnknownIssuer")))
    {
        NoticeCategory::WebSocketCertificateFailure
    } else if title.contains("Falling back from WebSockets to HTTPS transport") {
        NoticeCategory::WebSocketFallback
    } else if title.contains("Configuration") || title.contains("configuration") {
        NoticeCategory::Configuration
    } else if title.contains("Deprecated") || title.contains("deprecated") {
        NoticeCategory::Deprecation
    } else {
        NoticeCategory::Other
    };
    Ok(ProviderNoticeDiagnostic { severity, category })
}

enum SessionUpdatePayload<'a> {
    AgentText(&'a str),
    Notice(ProviderNoticeDiagnostic),
    ToolUse,
    Other,
}

fn session_update_payload(update: &Value) -> Result<SessionUpdatePayload<'_>, ProviderError> {
    match update
        .get("sessionUpdate")
        .and_then(Value::as_str)
        .unwrap_or_default()
    {
        "notice" => Ok(SessionUpdatePayload::Notice(parse_provider_notice(update)?)),
        "agent_message_chunk" => {
            let content = update.get("content").unwrap_or(&Value::Null);
            if content.get("type").and_then(Value::as_str) == Some("text")
                && let Some(text) = content.get("text").and_then(Value::as_str)
            {
                return Ok(SessionUpdatePayload::AgentText(text));
            }
            Ok(SessionUpdatePayload::Other)
        }
        "tool_call" | "tool_call_update" => Ok(SessionUpdatePayload::ToolUse),
        _ => Ok(SessionUpdatePayload::Other),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RpcFailureMethod {
    AuthenticationStatus,
    AuthenticationConnect,
    Catalog,
    Session,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RpcFailureCategory {
    ConfigRead,
    AccountRead,
    ProcessExit,
    WorkspaceUnauthorized,
    WorkspaceDiscovery,
    WorkspaceDiscoveryFailed,
    WorkspaceDiscoveryTimedOut,
    WorkspaceBackendMissing,
    WorkspaceSelectionMissing,
    RequestTransport,
    NetworkTimeout,
    ConnectionReset,
    TlsCertificate,
    TlsHandshake,
    DnsResolution,
    WorkspaceRequirements,
    Unclassified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RpcFailureDiagnostic {
    pub method: RpcFailureMethod,
    pub code: Option<i32>,
    pub category: RpcFailureCategory,
    pub rejected_api_openai: u64,
    pub rejected_other: u64,
    pub upstream_connect_failures: u64,
    pub tunnels_opened: u64,
    pub auth_openai_tunnels: u64,
    pub chatgpt_tunnels: u64,
    pub uploaded_bytes: u64,
    pub downloaded_bytes: u64,
    pub tunnel_reset_errors: u64,
    pub tunnel_timeout_errors: u64,
    pub upload_timeout_errors: u64,
    pub download_timeout_errors: u64,
    pub tunnel_other_errors: u64,
    pub http_status: Option<u16>,
    pub network_category: RpcFailureCategory,
}

fn rpc_failure_diagnostic(method: &str, error: &Value) -> RpcFailureDiagnostic {
    let method = match method {
        "authentication/status" => RpcFailureMethod::AuthenticationStatus,
        "_magi/auth/connect" => RpcFailureMethod::AuthenticationConnect,
        "model/list" => RpcFailureMethod::Catalog,
        "session/new" | "session/prompt" => RpcFailureMethod::Session,
        _ => RpcFailureMethod::Other,
    };
    let text = error.get("message").and_then(Value::as_str).unwrap_or("");
    let details = error
        .pointer("/data/details")
        .and_then(Value::as_str)
        .unwrap_or("");
    let code_tag = error
        .pointer("/data/code")
        .and_then(Value::as_str)
        .unwrap_or("");
    let category = if code_tag == "codex_app_server_exited" {
        RpcFailureCategory::ProcessExit
    } else if text.contains("workspace routing discovery unauthorized (401)")
        || details.contains("workspace routing discovery unauthorized (401)")
    {
        RpcFailureCategory::WorkspaceUnauthorized
    } else if text.contains("failed to load workspace requirements")
        || details.contains("failed to load workspace requirements")
    {
        RpcFailureCategory::WorkspaceRequirements
    } else if text.contains("workspace routing discovery timed out")
        || details.contains("workspace routing discovery timed out")
    {
        RpcFailureCategory::WorkspaceDiscoveryTimedOut
    } else if text.contains("workspace routing discovery missing backend origin")
        || details.contains("workspace routing discovery missing backend origin")
    {
        RpcFailureCategory::WorkspaceBackendMissing
    } else if text.contains("selected workspace missing from routing discovery")
        || details.contains("selected workspace missing from routing discovery")
    {
        RpcFailureCategory::WorkspaceSelectionMissing
    } else if text.contains("workspace routing discovery failed")
        || details.contains("workspace routing discovery failed")
    {
        RpcFailureCategory::WorkspaceDiscoveryFailed
    } else if text.contains("workspace routing discovery")
        || details.contains("workspace routing discovery")
    {
        RpcFailureCategory::WorkspaceDiscovery
    } else if text.contains("config/read") || details.contains("config/read") {
        RpcFailureCategory::ConfigRead
    } else if text.contains("account/read") || details.contains("account/read") {
        RpcFailureCategory::AccountRead
    } else {
        RpcFailureCategory::Unclassified
    };
    let fragments = [text, details];
    let contains = |pattern: &str| fragments.iter().any(|fragment| fragment.contains(pattern));
    let http_status = [401u16, 403, 404, 408, 429, 500, 502, 503, 504]
        .into_iter()
        .find(|status| {
            [
                format!("status code: {status}"),
                format!("HTTP status {status}"),
                format!("HTTP {status}"),
                format!("({status})"),
                format!("\"status\":{status}"),
                format!("\"status\": {status}"),
            ]
            .iter()
            .any(|pattern| contains(pattern))
        });
    let network_category = if contains("certificate verify failed")
        || contains("invalid peer certificate")
        || contains("invalid certificate")
    {
        RpcFailureCategory::TlsCertificate
    } else if contains("TLS handshake") || contains("tls handshake") {
        RpcFailureCategory::TlsHandshake
    } else if contains("connection reset") || contains("Connection reset") {
        RpcFailureCategory::ConnectionReset
    } else if contains("dns error")
        || contains("DNS resolution")
        || contains("failed to lookup address")
    {
        RpcFailureCategory::DnsResolution
    } else if contains("timed out") || contains("timeout") {
        RpcFailureCategory::NetworkTimeout
    } else if contains("error sending request") || contains("request transport") {
        RpcFailureCategory::RequestTransport
    } else {
        RpcFailureCategory::Unclassified
    };
    RpcFailureDiagnostic {
        method,
        code: error
            .get("code")
            .and_then(Value::as_i64)
            .and_then(|code| i32::try_from(code).ok()),
        category,
        rejected_api_openai: 0,
        rejected_other: 0,
        upstream_connect_failures: 0,
        tunnels_opened: 0,
        auth_openai_tunnels: 0,
        chatgpt_tunnels: 0,
        uploaded_bytes: 0,
        downloaded_bytes: 0,
        tunnel_reset_errors: 0,
        tunnel_timeout_errors: 0,
        upload_timeout_errors: 0,
        download_timeout_errors: 0,
        tunnel_other_errors: 0,
        http_status,
        network_category,
    }
}

impl CodexAcpClient {
    pub fn artifact_identity(&self) -> &crate::RuntimeArtifactIdentity {
        &self.prepared.artifact_identity
    }

    pub async fn provider_notices(&self) -> Vec<ProviderNoticeDiagnostic> {
        self.inner.provider_notices.lock().await.clone()
    }

    pub async fn rpc_failure_diagnostic(&self) -> Option<RpcFailureDiagnostic> {
        self.inner.rpc_failure.lock().await.map(|mut record| {
            record.rejected_api_openai = self
                .network_proxy
                .state
                .rejected_api_openai
                .load(Ordering::Relaxed);
            record.rejected_other = self
                .network_proxy
                .state
                .rejected_other
                .load(Ordering::Relaxed);
            record.upstream_connect_failures = self
                .network_proxy
                .state
                .upstream_connect_failures
                .load(Ordering::Relaxed);
            record.tunnels_opened = self
                .network_proxy
                .state
                .tunnels_opened
                .load(Ordering::Relaxed);
            record.auth_openai_tunnels = self
                .network_proxy
                .state
                .auth_openai_tunnels
                .load(Ordering::Relaxed);
            record.uploaded_bytes = self
                .network_proxy
                .state
                .uploaded_bytes
                .load(Ordering::Relaxed);
            record.downloaded_bytes = self
                .network_proxy
                .state
                .downloaded_bytes
                .load(Ordering::Relaxed);
            record.tunnel_reset_errors = self
                .network_proxy
                .state
                .tunnel_reset_errors
                .load(Ordering::Relaxed);
            record.upload_timeout_errors = self
                .network_proxy
                .state
                .upload_timeout_errors
                .load(Ordering::Relaxed);
            record.download_timeout_errors = self
                .network_proxy
                .state
                .download_timeout_errors
                .load(Ordering::Relaxed);
            record.tunnel_timeout_errors = self
                .network_proxy
                .state
                .tunnel_timeout_errors
                .load(Ordering::Relaxed);
            record.tunnel_other_errors = self
                .network_proxy
                .state
                .tunnel_other_errors
                .load(Ordering::Relaxed);
            record.chatgpt_tunnels = self
                .network_proxy
                .state
                .chatgpt_tunnels
                .load(Ordering::Relaxed);
            record
        })
    }

    #[cfg(test)]
    pub(crate) async fn spawn_controlled_fixture(
        launch: crate::CodexAcpLaunch,
    ) -> Result<Self, ProviderError> {
        Self::spawn_inner(
            launch,
            None,
            CertificatePolicy::Configured,
            None,
            true,
            false,
        )
        .await
    }
    pub async fn spawn(launch: crate::CodexAcpLaunch) -> Result<Self, ProviderError> {
        Self::spawn_inner(
            launch,
            None,
            CertificatePolicy::Configured,
            None,
            false,
            false,
        )
        .await
    }

    pub async fn spawn_with_client_file_reader(
        launch: crate::CodexAcpLaunch,
        source_reader: Arc<dyn ClientFileReader>,
    ) -> Result<Self, ProviderError> {
        Self::spawn_inner(
            launch,
            Some(source_reader),
            CertificatePolicy::Configured,
            None,
            false,
            false,
        )
        .await
    }

    #[cfg(test)]
    pub(crate) async fn spawn_platform_roots_diagnostic(
        launch: crate::CodexAcpLaunch,
        source_reader: Arc<dyn ClientFileReader>,
    ) -> Result<Self, ProviderError> {
        Self::spawn_inner(
            launch,
            Some(source_reader),
            CertificatePolicy::PlatformRootsDiagnostic,
            None,
            true,
            false,
        )
        .await
    }

    pub async fn spawn_with_verified_artifact(
        launch: crate::CodexAcpLaunch,
        source_reader: Arc<dyn ClientFileReader>,
        artifact: Arc<crate::VerifiedRuntimeArtifact>,
        request: crate::VerificationRequest,
    ) -> Result<Self, ProviderError> {
        Self::spawn_inner(
            launch,
            Some(source_reader),
            CertificatePolicy::Configured,
            Some((artifact, request)),
            false,
            false,
        )
        .await
    }

    #[cfg(test)]
    pub(crate) fn fixture_closed_network_counters(&self) -> (bool, u64, u64, u64) {
        (
            self.network_proxy.state.closed_network,
            self.network_proxy
                .state
                .tunnels_opened
                .load(Ordering::SeqCst),
            self.network_proxy
                .state
                .upstream_connect_failures
                .load(Ordering::SeqCst),
            self.inner.next_request_id.load(Ordering::SeqCst),
        )
    }
    #[cfg(test)]
    pub(crate) async fn spawn_verified_network_closed_fixture(
        launch: crate::CodexAcpLaunch,
        reader: Arc<dyn ClientFileReader>,
        artifact: Arc<crate::VerifiedRuntimeArtifact>,
        request: crate::VerificationRequest,
    ) -> Result<Self, ProviderError> {
        Self::spawn_inner(
            launch,
            Some(reader),
            CertificatePolicy::Configured,
            Some((artifact, request)),
            false,
            true,
        )
        .await
    }
    pub fn subscribe_auth_progress(&self) -> watch::Receiver<AuthProgressStage> {
        self.auth_progress.clone()
    }

    async fn spawn_inner(
        launch: crate::CodexAcpLaunch,
        source_reader: Option<Arc<dyn ClientFileReader>>,
        certificate_policy: CertificatePolicy,
        verified: Option<(
            Arc<crate::VerifiedRuntimeArtifact>,
            crate::VerificationRequest,
        )>,
        controlled_fixture: bool,
        closed_network: bool,
    ) -> Result<Self, ProviderError> {
        let resource_custody = match &verified {
            Some((artifact, request)) => artifact.client_custody(request.clone())?,
            None => {
                #[cfg(not(test))]
                {
                    let _ = controlled_fixture;
                    return Err(ProviderError::ArtifactVerification);
                }
                #[cfg(test)]
                {
                    if !controlled_fixture {
                        return Err(ProviderError::ArtifactVerification);
                    }
                    None
                }
            }
        };
        let home_operation_lock =
            HomeOperationLock::acquire(&launch.profile_home, &launch.runtime_home_id)?;
        let prepared = match &verified {
            Some((artifact, request)) => sandbox::prepare_verified(&launch, artifact, request)?,
            None => sandbox::prepare(&launch)?,
        };
        let activity = || {
            verified
                .as_ref()
                .map(|(_, request)| request.provider_activity())
                .transpose()
        };
        let network_proxy = if closed_network {
            #[cfg(test)]
            {
                ProviderNetworkProxy::start_with_policy(activity()?, true)?
            }
            #[cfg(not(test))]
            {
                return Err(ProviderError::InvalidLaunch);
            }
        } else {
            ProviderNetworkProxy::start_tracked(activity()?)?
        };
        let mut process_activity = activity()?;
        let reader_activity = activity()?;
        let auth_activity = activity()?;
        let mut command = sandbox::isolated_command(
            &prepared,
            network_proxy.local_addr(),
            network_proxy.proxy_url(),
        )?;
        certificate_policy.apply(&mut command);
        home_operation_lock.install_on(&mut command);
        command.kill_on_drop(true);
        let parent_lease =
            parent_guard::install(&mut command).map_err(|_| ProviderError::ProcessStart)?;
        if let Some((artifact, request)) = &verified {
            artifact.check(request)?;
        }
        let effect_authority = verified
            .as_ref()
            .map(|(_, request)| request.effect_authority());
        let child = match &verified {
            Some((_, request)) => request.publish(|| command.spawn())?,
            None => command.spawn(),
        }
        .map_err(|_| ProviderError::ProcessStart)?;
        if let Some(activity) = &mut process_activity {
            activity.mark_owned_process();
        }
        let mut startup = StartupProcessGuard::new(child, home_operation_lock, process_activity);
        let mut prepared = Some(prepared);
        let mut network_proxy = Some(network_proxy);
        let result = CatchUnwindFuture::new(async {
            let process_group_id = startup.child_id().ok_or(ProviderError::ProcessStart)? as i32;
            let (exit_sender, exit_status) = watch::channel(None);
            let process = Arc::new(ProcessControl {
                process_group_id,
                exit_status,
                observation: StdMutex::new(local_observation::LocalObservation::now()),
            });
            startup.process = Some(process.clone());
            startup.exit_sender = Some(exit_sender.clone());

            let (stdin, stdout, stderr) = {
                let mut child_slot = startup
                    .child
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let child = child_slot.as_mut().ok_or(ProviderError::ProcessStart)?;
                match (child.stdin.take(), child.stdout.take(), child.stderr.take()) {
                    (Some(stdin), Some(stdout), Some(stderr)) => Ok((stdin, stdout, stderr)),
                    _ => Err(ProviderError::ProcessStart),
                }
            }?;
            let (auth_progress_sender, auth_progress) = watch::channel(AuthProgressStage::Opening);
            let (auth_browser_request_sender, auth_browser_requests) = mpsc::channel(1);
            let inner = Arc::new(Inner {
                request_deadline: Arc::new(ProviderPhaseClock::new(verified.as_ref().map(|(_,request)|request.deadline()))),
                effect_authority,
                writer: Mutex::new(stdin),
                pending: Mutex::new(HashMap::new()),
                prompts: Arc::new(Mutex::new(HashMap::new())),
                active_session_id: Mutex::new(None),
                source_reader,
                subscription: Mutex::new(None),
                subscription_authenticated: AtomicBool::new(false),
                rpc_failure: Mutex::new(None),
                provider_notices: Mutex::new(Vec::new()),
                event_gate: Mutex::new(()),
                next_request_id: AtomicU64::new(1),
                closed: AtomicBool::new(false),
                process: process.clone(),
            });

            let observed_inner=Arc::downgrade(&inner);
            let phase_clock=inner.request_deadline.clone();
            let mut phase_updates=phase_clock.updates.subscribe();
            tokio::spawn(async move {
                loop {
                    let Ok(snapshot)=phase_clock.snapshot() else {break;};
                    let deadline_reached=match snapshot.deadline {
                        Some(deadline)=>tokio::select!{biased;
                            changed=phase_updates.changed()=>{if changed.is_err(){break;} continue;},
                            _=tokio::time::sleep_until(tokio::time::Instant::from_std(deadline))=>true,
                            _=tokio::time::sleep(Duration::from_secs(5))=>false,
                        },
                        None=>tokio::select!{changed=phase_updates.changed()=>{if changed.is_err(){break;}continue;},_=tokio::time::sleep(Duration::from_secs(5))=>false,},
                    };
                    let Some(inner)=observed_inner.upgrade() else {break;};
                    if inner.closed.load(Ordering::Acquire){break;}
                    if deadline_reached {
                        if !phase_clock.close_expired_snapshot(snapshot,&inner.closed){continue;}
                        inner.poison().await;break;
                    }
                    if !inner.process.observe_owned_process(){inner.poison().await;break;}
                }
            });
            let reader_inner = inner.clone();
            startup.reader_task = Some(tokio::spawn(async move {
                let _activity = reader_activity;
                if read_protocol(reader_inner.clone(), stdout).await.is_err() {
                    close_transport(&reader_inner, ProviderError::Protocol).await;
                    reader_inner.poison().await;
                }
            }));

            let supervisor_inner = inner.clone();
            let child_slot = startup.child.clone();
            let process_activity = startup.activity.take();
            startup.supervisor_task = Some(tokio::spawn(async move {
                let child = child_slot
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take();
                if let Some(child) = child {
                    supervise_child(supervisor_inner, child, exit_sender, process_activity).await;
                }
            }));

            let prepared = prepared.take().ok_or(ProviderError::ProcessStart)?;
            let network_proxy = network_proxy.take().ok_or(ProviderError::ProcessStart)?;
            let home_operation_lock = startup
                .home_operation_lock
                .take()
                .ok_or(ProviderError::ProcessStart)?;
            let mut client = Self {
                inner,
                prepared,
                verified_runtime: verified.as_ref().map(|(artifact, _)| artifact.clone()),
                resource_custody: Mutex::new(resource_custody),
                activity_request: verified.as_ref().map(|(_, request)| request.clone()),
                _home_operation_lock: home_operation_lock,
                _parent_lease: parent_lease,
                network_proxy,
                auth_methods: Mutex::new(Vec::new()),
                session_state: Mutex::new(None),
                initializing: AtomicBool::new(false),
                initialized: AtomicBool::new(false),
                image_prompt_supported: AtomicBool::new(false),
                session_opening: Arc::new(AtomicBool::new(false)),
                catalog_listing: AtomicBool::new(false),
                auth_progress,
                auth_browser_requests: Mutex::new(auth_browser_requests),
                auth_progress_task: Mutex::new(None),
                reader_task: Mutex::new(None),
                supervisor_task: Mutex::new(None),
            };
            *client.auth_progress_task.get_mut() = Some(tokio::spawn(async move {
                let _activity = auth_activity;
                read_auth_progress(stderr, auth_progress_sender, auth_browser_request_sender).await;
            }));
            *client.reader_task.get_mut() = startup.reader_task.take();
            *client.supervisor_task.get_mut() = startup.supervisor_task.take();
            startup.armed = false;
            Ok(client)
        })
        .await;

        match result {
            Ok(Ok(client)) => Ok(client),
            Ok(Err(error)) => {
                startup.cleanup().await;
                Err(error)
            }
            Err(payload) => {
                startup.cleanup().await;
                std::panic::resume_unwind(payload)
            }
        }
    }

    pub fn check_runtime_authority(
        &self,
        request: &crate::VerificationRequest,
    ) -> Result<(), ProviderError> {
        request.check()?;
        self.verified_runtime
            .as_ref()
            .ok_or(ProviderError::ArtifactVerification)?
            .check(request)
    }

    pub fn home_binding_proof(&self) -> HomeBindingProof {
        HomeBindingProof {
            provider_profile_id: self.prepared.provider_profile_id.clone(),
            profile_revision: self.prepared.profile_revision,
            explicit_codex_home_environment: true,
            operating_system_sandbox_applied: true,
            adapter_reported_home: false,
        }
    }

    pub fn proves_profile_binding(
        &self,
        profile_id: &str,
        revision: u64,
        expected_canonical_managed_home: &Path,
    ) -> bool {
        let Ok(expected_home) = expected_canonical_managed_home.canonicalize() else {
            return false;
        };
        self.prepared.provider_profile_id == profile_id
            && self.prepared.profile_revision == revision
            && self.prepared.runtime_home_id
                == expected_home
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or_default()
            && self.prepared.profile_home == expected_home
            && self.home_binding_proof().explicit_codex_home_environment
            && self.home_binding_proof().operating_system_sandbox_applied
    }

    pub async fn connect_existing_subscription(
        &self,
        broker: Arc<crate::subscription::SubscriptionBroker>,
    ) -> Result<(), ProviderError> {
        self.ensure_initialized()?;
        let parameters = broker
            .connect_parameters()
            .map_err(|_| ProviderError::Unauthenticated)?;
        *self.inner.subscription.lock().await = Some(broker);
        let response = self.inner.call("_magi/auth/connect", parameters).await?;
        if response.get("authenticated").and_then(Value::as_bool) != Some(true) {
            return Err(ProviderError::Unauthenticated);
        }
        self.inner
            .subscription_authenticated
            .store(true, Ordering::Release);
        Ok(())
    }

    /// Enters the model-call phase once after the exact authenticated session is configured.
    /// The native caller supplies the remaining durable Run budget; this is not a renewal API.
    pub async fn begin_authenticated_model_phase(
        &self,
        expected: &SessionInfo,
        deadline: std::time::Instant,
    ) -> Result<(), ProviderError> {
        self.ensure_initialized()?;
        let request = self
            .activity_request
            .as_ref()
            .ok_or(ProviderError::ArtifactVerification)?;
        request.check()?;
        self.verified_runtime
            .as_ref()
            .ok_or(ProviderError::ArtifactVerification)?
            .check(request)?;
        if !self
            .inner
            .subscription_authenticated
            .load(Ordering::Acquire)
        {
            return Err(ProviderError::Unauthenticated);
        }
        let subscription = self.inner.subscription.lock().await;
        subscription
            .as_ref()
            .ok_or(ProviderError::Unauthenticated)?
            .connect_parameters()
            .map_err(|_| ProviderError::Unauthenticated)?;
        let session = self.session_state.lock().await;
        if session
            .as_ref()
            .is_none_or(|current| current.info != *expected)
            || !expected.selected_model_available
        {
            return Err(ProviderError::SessionUnavailable);
        }
        let pending = self.inner.pending.lock().await;
        let prompts = self.inner.prompts.lock().await;
        if !pending.is_empty() || !prompts.is_empty() {
            return Err(ProviderError::PromptInProgress);
        }
        let _writer = self
            .inner
            .writer
            .try_lock()
            .map_err(|_| ProviderError::PromptInProgress)?;
        self.inner
            .effect_authority
            .as_ref()
            .ok_or(ProviderError::ArtifactVerification)?
            .publish(|| {
                self.inner
                    .request_deadline
                    .transition(deadline, &self.inner.closed)
            })??;
        Ok(())
    }

    pub async fn initialize(&self) -> Result<InitializeResult, ProviderError> {
        if self.initialized.load(Ordering::Acquire)
            || self
                .initializing
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        {
            return Err(ProviderError::Protocol);
        }
        let result = self
            .inner
            .call(
                "initialize",
                json!({
                    "protocolVersion": 1,
                    "clientInfo": {
                        "name": "oh-my-magi",
                        "title": "Oh My MAGI",
                        "version": env!("CARGO_PKG_VERSION")
                    },
                    "clientCapabilities": {
                        "session": { "notices": {} },
                        "fs": {
                            "readTextFile": self.inner.source_reader.is_some(),
                            "writeTextFile": false
                        }
                    }
                }),
            )
            .await;
        self.initializing.store(false, Ordering::Release);
        let result = result?;
        let protocol_version = result
            .get("protocolVersion")
            .and_then(Value::as_u64)
            .ok_or(ProviderError::InvalidResponse)?;
        if protocol_version != 1 {
            return Err(ProviderError::InvalidResponse);
        }
        let agent_info = result.get("agentInfo").unwrap_or(&Value::Null);
        let authentication_methods = result
            .get("authMethods")
            .and_then(Value::as_array)
            .map(|methods| {
                methods
                    .iter()
                    .filter_map(|method| method.get("id").and_then(Value::as_str))
                    .filter(|id| *id == AuthMethod::ChatGpt.acp_id())
                    .map(|_| AuthMethod::ChatGpt)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        *self.auth_methods.lock().await = authentication_methods.clone();
        self.image_prompt_supported.store(
            result
                .pointer("/agentCapabilities/promptCapabilities/image")
                .and_then(Value::as_bool)
                == Some(true),
            Ordering::Release,
        );
        self.initialized.store(true, Ordering::Release);
        Ok(InitializeResult {
            protocol_version,
            adapter: AdapterInfo {
                name: agent_info
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                title: agent_info
                    .get("title")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                version: agent_info
                    .get("version")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .or_else(|| Some(CODEX_ACP_VERSION.to_owned())),
            },
            authentication_methods,
            home_binding: self.home_binding_proof(),
        })
    }

    pub async fn authentication_status(&self) -> Result<AuthenticationStatus, ProviderError> {
        self.ensure_initialized()?;
        let result = self.inner.call("authentication/status", json!({})).await?;
        match result.get("type").and_then(Value::as_str) {
            Some("unauthenticated") => Ok(AuthenticationStatus::Unauthenticated),
            Some("chat-gpt") => Ok(AuthenticationStatus::Authenticated {
                method: AuthMethod::ChatGpt,
            }),
            Some(_) => Ok(AuthenticationStatus::Unsupported),
            None => Err(ProviderError::InvalidResponse),
        }
    }

    pub async fn authenticate(&self, method: AuthMethod) -> Result<(), ProviderError> {
        self.authenticate_with_browser_opener(method, |_| async {
            Err(ProviderError::BrowserOpenFailed)
        })
        .await
    }

    pub async fn authenticate_with_browser_opener<F, Fut>(
        &self,
        method: AuthMethod,
        mut open_browser: F,
    ) -> Result<(), ProviderError>
    where
        F: FnMut(String) -> Fut,
        Fut: Future<Output = Result<(), ProviderError>>,
    {
        self.ensure_initialized()?;
        if !self.auth_methods.lock().await.contains(&method) {
            return Err(ProviderError::AuthenticationUnavailable);
        }
        let inner = self.inner.clone();
        let authentication = inner.call("authenticate", json!({ "methodId": method.acp_id() }));
        tokio::pin!(authentication);
        let mut browser_requests = self.auth_browser_requests.lock().await;
        let mut opened_browser = false;
        loop {
            tokio::select! {
                result = &mut authentication => {
                    result?;
                    return Ok(());
                }
                request = browser_requests.recv() => {
                    let Some(request) = request else {
                        return Err(ProviderError::ProcessClosed);
                    };
                    let url = request.map_err(|_| ProviderError::BrowserOpenFailed)?;
                    if opened_browser {
                        return Err(ProviderError::BrowserOpenFailed);
                    }
                    open_browser(url).await?;
                    opened_browser = true;
                }
            }
        }
    }

    pub async fn list_models(&self) -> Result<Vec<AvailableModel>, ProviderError> {
        self.ensure_initialized()?;
        if self
            .catalog_listing
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(ProviderError::CatalogBusy);
        }
        let _listing = FlagReset(&self.catalog_listing);
        if let Some(session) = self.session_state.lock().await.as_ref() {
            return Ok(session.info.available_models.clone());
        }
        Ok(self.discover_session().await?.available_models)
    }

    pub async fn discover_session(&self) -> Result<SessionInfo, ProviderError> {
        let prepared = self.prepare_session(None).await?;
        let pending = self.begin_new_session(prepared).await?;
        self.finish_new_session(pending).await
    }

    pub async fn select_mode(&self, session_id: &str, mode_id: &str) -> Result<(), ProviderError> {
        self.ensure_initialized()?;
        begin_configuration_authority(
            &self.session_state,
            &self.inner.prompts,
            session_id,
            mode_id,
            true,
        )
        .await?;
        let response = self
            .inner
            .call(
                "session/set_mode",
                json!({"sessionId": session_id, "modeId": mode_id}),
            )
            .await?;
        let mut slot = self.session_state.lock().await;
        let state = slot.as_mut().ok_or(ProviderError::SessionUnavailable)?;
        state.confirm_mode(session_id, mode_id, &response)
    }

    pub async fn confirmed_session_info(
        &self,
        session_id: &str,
    ) -> Result<SessionInfo, ProviderError> {
        self.session_state
            .lock()
            .await
            .as_ref()
            .ok_or(ProviderError::SessionUnavailable)?
            .confirmed_info(session_id)
    }

    /// Confirms the requested model. Advertised modes still require an explicit `select_mode` before prompting.
    pub async fn new_session(&self, model_id: &str) -> Result<SessionInfo, ProviderError> {
        let prepared = self.prepare_new_session(model_id).await?;
        let pending = self.begin_new_session(prepared).await?;
        let info = self.finish_new_session(pending).await?;
        if !info.selected_model_available {
            return Err(ProviderError::ModelUnavailable);
        }
        let pending = self
            .begin_model_configuration(&info.session_id, model_id)
            .await?;
        self.finish_model_configuration(pending).await?;
        self.session_state
            .lock()
            .await
            .as_ref()
            .map(|state| state.info.clone())
            .ok_or(ProviderError::SessionUnavailable)
    }

    pub async fn prepare_new_session(
        &self,
        model_id: &str,
    ) -> Result<PreparedSession, ProviderError> {
        if !valid_catalog_id(model_id) {
            return Err(ProviderError::ModelUnavailable);
        }
        self.prepare_session(Some(model_id.to_owned())).await
    }

    async fn prepare_session(
        &self,
        model_id: Option<String>,
    ) -> Result<PreparedSession, ProviderError> {
        self.ensure_initialized()?;
        if self.session_state.lock().await.is_some()
            || self.inner.active_session_id.lock().await.is_some()
        {
            return Err(ProviderError::SessionAlreadyCreated);
        }
        match self.authentication_status().await? {
            AuthenticationStatus::Authenticated {
                method: AuthMethod::ChatGpt,
            } => {}
            AuthenticationStatus::Unauthenticated => return Err(ProviderError::Unauthenticated),
            AuthenticationStatus::Unsupported => {
                return Err(ProviderError::AuthenticationUnavailable);
            }
        }
        let cwd = self
            .prepared
            .role_workdir
            .to_str()
            .ok_or(ProviderError::RoleWorkdirUnavailable)?
            .to_owned();
        Ok(PreparedSession { model_id, cwd })
    }

    pub async fn begin_new_session(
        &self,
        prepared: PreparedSession,
    ) -> Result<PendingSession, ProviderError> {
        self.ensure_initialized()?;
        if self.session_state.lock().await.is_some()
            || self.inner.active_session_id.lock().await.is_some()
            || self
                .session_opening
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        {
            return Err(ProviderError::SessionAlreadyCreated);
        }
        let opening = SessionOpeningGuard(self.session_opening.clone());
        let request = self
            .inner
            .begin_call(
                "session/new",
                json!({
                    "cwd": prepared.cwd.clone(),
                    "mcpServers": []
                }),
            )
            .await?;
        Ok(PendingSession {
            prepared,
            request,
            _opening: opening,
        })
    }

    pub async fn finish_new_session(
        &self,
        pending: PendingSession,
    ) -> Result<SessionInfo, ProviderError> {
        let PendingSession {
            prepared,
            request,
            _opening,
        } = pending;
        let response = request.wait(&self.inner).await?;
        let info = parse_session_info(&response, prepared.model_id.as_deref())?;
        *self.session_state.lock().await = Some(SessionAuthority::new(info.clone()));
        Ok(info)
    }

    pub async fn begin_model_configuration(
        &self,
        session_id: &str,
        model_id: &str,
    ) -> Result<PendingModelConfiguration, ProviderError> {
        self.ensure_initialized()?;
        begin_configuration_authority(
            &self.session_state,
            &self.inner.prompts,
            session_id,
            model_id,
            false,
        )
        .await?;
        let (base_model, reasoning_effort) = match split_model_id(model_id) {
            Some((model, effort)) => (model, Some(effort.to_owned())),
            None => (model_id, None),
        };
        let request = self
            .inner
            .begin_call(
                "session/set_config_option",
                json!({"sessionId":session_id,"configId":"model","value":base_model}),
            )
            .await?;
        Ok(PendingModelConfiguration {
            model_id: model_id.to_owned(),
            session_id: session_id.to_owned(),
            reasoning_effort,
            request,
        })
    }

    pub async fn finish_model_configuration(
        &self,
        pending: PendingModelConfiguration,
    ) -> Result<(), ProviderError> {
        let response = pending.request.wait(&self.inner).await?;
        validate_model_ack(&response, &pending.model_id)?;
        if let Some(effort) = pending.reasoning_effort {
            let response = self.inner.call("session/set_config_option", json!({"sessionId":pending.session_id,"configId":"reasoning_effort","value":effort})).await?;
            if config_value(&response, "reasoning_effort") != Some(effort.as_str()) {
                return Err(ProviderError::InvalidResponse);
            }
        }
        self.session_state
            .lock()
            .await
            .as_mut()
            .ok_or(ProviderError::SessionUnavailable)?
            .confirm_model(&pending.session_id, &pending.model_id)
    }

    pub fn public_text_token_count(model_id: &str, text: &str) -> Result<u64, ProviderError> {
        let slug = match model_id.split_once('[') {
            None => model_id,
            Some((slug, "low]" | "medium]" | "high]" | "xhigh]")) => slug,
            _ => return Err(ProviderError::InvalidResponse),
        };
        if !matches!(slug, "gpt-5.5" | "gpt-5.4") {
            // UTF-8 bytes bound public text tokens without guessing a model encoding.
            return if text.len() <= 8192 {
                Ok(text.len() as u64)
            } else {
                Err(ProviderError::InvalidResponse)
            };
        }
        // The official GPT-5 tokenizer prefix maps to o200k_base. This counts public text only.
        Ok(tiktoken_rs::o200k_base_singleton()
            .encode_ordinary(text)
            .len() as u64)
    }

    pub fn validate_prompt_blocks(blocks: &[Value]) -> Result<(), ProviderError> {
        use base64::Engine;
        if blocks.is_empty() || blocks.len() > 201 {
            return Err(ProviderError::InputLimit);
        }
        for (index, block) in blocks.iter().enumerate() {
            let object = block.as_object().ok_or(ProviderError::InvalidResponse)?;
            match block.get("type").and_then(Value::as_str) {
                Some("text") if index == 0 && object.len() == 2 && object.contains_key("text") => {
                    block
                        .get("text")
                        .and_then(Value::as_str)
                        .ok_or(ProviderError::InvalidResponse)?;
                }
                Some("image")
                    if index > 0
                        && object.len() == 3
                        && object.contains_key("data")
                        && object.contains_key("mimeType") =>
                {
                    let mime = block
                        .get("mimeType")
                        .and_then(Value::as_str)
                        .ok_or(ProviderError::InvalidResponse)?;
                    let data = block
                        .get("data")
                        .and_then(Value::as_str)
                        .ok_or(ProviderError::InvalidResponse)?;
                    if data.len() > MAX_PROMPT_BYTES {
                        return Err(ProviderError::InputLimit);
                    }
                    let bytes = base64::engine::general_purpose::STANDARD
                        .decode(data)
                        .map_err(|_| ProviderError::InvalidResponse)?;
                    let valid = match mime {
                        "image/png" => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
                        "image/jpeg" => bytes.starts_with(&[0xff, 0xd8, 0xff]),
                        _ => false,
                    };
                    if !valid {
                        return Err(ProviderError::InvalidResponse);
                    }
                }
                _ => return Err(ProviderError::InvalidResponse),
            }
        }
        if serde_json::to_vec(blocks)
            .map_err(|_| ProviderError::InvalidResponse)?
            .len()
            > MAX_PROMPT_BYTES
        {
            return Err(ProviderError::InputLimit);
        }
        Ok(())
    }

    pub async fn prompt(
        &self,
        session_id: &str,
        text: String,
    ) -> Result<PromptHandle, ProviderError> {
        self.prompt_blocks(session_id, vec![json!({"type":"text","text":text})])
            .await
    }

    pub async fn prompt_blocks(
        &self,
        session_id: &str,
        blocks: Vec<Value>,
    ) -> Result<PromptHandle, ProviderError> {
        self.prompt_blocks_with_authorization(session_id, blocks, None)
            .await
    }

    pub async fn prompt_blocks_authorized(
        &self,
        session_id: &str,
        blocks: Vec<Value>,
        expires_at_epoch_ms: u64,
        check_current: Arc<dyn Fn() -> Result<(), ProviderError> + Send + Sync>,
    ) -> Result<PromptHandle, ProviderError> {
        let authorization = crate::verification::PromptPublicationAuthorization::new(
            expires_at_epoch_ms,
            check_current,
        )?;
        self.prompt_blocks_with_authorization(session_id, blocks, Some(authorization))
            .await
    }

    async fn prompt_blocks_with_authorization(
        &self,
        session_id: &str,
        blocks: Vec<Value>,
        authorization: Option<crate::verification::PromptPublicationAuthorization>,
    ) -> Result<PromptHandle, ProviderError> {
        self.ensure_initialized()?;
        Self::validate_prompt_blocks(&blocks)?;
        if blocks.len() > 1 && !self.image_prompt_supported.load(Ordering::Acquire) {
            return Err(ProviderError::ToolDenied);
        }
        let (event_sender, events) = mpsc::channel(PROMPT_EVENT_QUEUE);
        let (terminal_sender, terminal_outcome) = watch::channel(PromptTerminalOutcome::Pending);
        let state = Arc::new(PendingPrompt {
            events: event_sender,
            text: Mutex::new(String::new()),
            pending_text: Mutex::new(String::new()),
            delivery_wake: Notify::new(),
            delivery_finished: AtomicBool::new(false),
            queue_pressure: AtomicUsize::new(0),
            delivered_chunks: AtomicUsize::new(0),
            coalesced_updates: AtomicUsize::new(0),
            consumer_closed: AtomicBool::new(false),
            in_flight_bytes: AtomicUsize::new(0),
            diagnostic: PromptStreamDiagnostic(Arc::new(StdMutex::new(
                PromptStreamSnapshot::default(),
            ))),
            tool_denied: AtomicBool::new(false),
            output_limited: AtomicBool::new(false),
            event_limited: AtomicBool::new(false),
            event_count: AtomicUsize::new(0),
            terminal_outcome,
        });
        reserve_prompt_authority(
            &self.session_state,
            &self.inner.prompts,
            session_id,
            state.clone(),
        )
        .await?;
        let mut reservation = PromptReservation {
            prompts: self.inner.prompts.clone(),
            session_id: session_id.to_owned(),
            state: state.clone(),
            inner: Some(self.inner.clone()),
            armed: true,
        };

        let delivery_activity = self
            .activity_request
            .as_ref()
            .map(crate::VerificationRequest::provider_activity)
            .transpose()?;
        let delivery_state = state.clone();
        let delivery_authority = self.inner.effect_authority.clone();
        let delivery_inner = self.inner.clone();
        let delivery_session = session_id.to_owned();
        let delivery = tokio::spawn(async move {
            let _activity = delivery_activity;
            let result = deliver_prompt_text(delivery_state, delivery_authority.as_ref()).await;
            if result.is_err() {
                let _ = delivery_inner
                    .notify("session/cancel", json!({"sessionId": delivery_session}))
                    .await;
            }
            result
        });

        let request_session_id = session_id.to_owned();
        let prompt_blocks = blocks;
        let request = match self
            .inner
            .begin_call_authorized(
                "session/prompt",
                json!({
                    "sessionId": request_session_id,
                    "prompt": prompt_blocks
                }),
                authorization.as_ref(),
            )
            .await
        {
            Ok(request) => request,
            Err(error) => {
                state.delivery_finished.store(true, Ordering::Release);
                state.delivery_wake.notify_one();
                let _ = delivery.await;
                reservation.release().await;
                return Err(error);
            }
        };
        let inner = self.inner.clone();
        let state_for_handle = state.diagnostic.clone();
        let (completion_sender, completion) = oneshot::channel();
        tokio::spawn(async move {
            let response = request.wait(&inner).await;
            let terminal_outcome = match response.as_ref() {
                Ok(response) => match response.get("stopReason").and_then(Value::as_str) {
                    Some(stop_reason) => PromptTerminalOutcome::Response {
                        stop_reason: stop_reason.to_owned(),
                    },
                    None => PromptTerminalOutcome::Error(ProviderError::InvalidResponse),
                },
                Err(error) => PromptTerminalOutcome::Error(error.clone()),
            };
            state.delivery_finished.store(true, Ordering::Release);
            state.delivery_wake.notify_one();
            let delivery_result = delivery.await.unwrap_or(Err(ProviderError::Cancelled));
            let result = match response {
                Err(error) => Err(error),
                Ok(_) if state.tool_denied.load(Ordering::Acquire) => {
                    Err(ProviderError::ToolDenied)
                }
                Ok(_) if state.output_limited.load(Ordering::Acquire) => {
                    Err(ProviderError::OutputLimit)
                }
                Ok(_) if state.event_limited.load(Ordering::Acquire) => {
                    Err(ProviderError::EventLimit)
                }
                Ok(_) if delivery_result.is_err() => {
                    Err(delivery_result.err().unwrap_or(ProviderError::Cancelled))
                }
                Ok(response) => {
                    let stop_reason = response
                        .get("stopReason")
                        .and_then(Value::as_str)
                        .ok_or(ProviderError::InvalidResponse);
                    match stop_reason {
                        Err(error) => Err(error),
                        Ok("cancelled") => Err(ProviderError::Cancelled),
                        Ok(stop_reason) => {
                            let text = state.text.lock().await.clone();
                            if text.len() > MAX_RESPONSE_BYTES {
                                Err(ProviderError::OutputLimit)
                            } else {
                                Ok(PromptResult {
                                    final_text: text,
                                    stop_reason: stop_reason.to_owned(),
                                    usage: parse_usage(response.get("usage")),
                                    buffered_events: Vec::new(),
                                })
                            }
                        }
                    }
                }
            };
            let failure = match &result {
                Err(ProviderError::OutputLimit) => PromptStreamFailure::ByteLimit,
                Err(ProviderError::EventLimit) => PromptStreamFailure::EventCeiling,
                Err(ProviderError::NoticeLimit) => PromptStreamFailure::NoticeCeiling,
                Err(ProviderError::StreamConsumerClosed) => PromptStreamFailure::ConsumerClosed,
                Err(ProviderError::Cancelled) => PromptStreamFailure::Cancelled,
                Err(ProviderError::Timeout) => PromptStreamFailure::DeliveryTimeout,
                Err(_) => PromptStreamFailure::Protocol,
                Ok(_) => PromptStreamFailure::None,
            };
            let diagnostic = PromptStreamSnapshot {
                raw_events: state.event_count.load(Ordering::Acquire),
                accepted_bytes: state.text.lock().await.len(),
                pending_buffer_bytes: state.pending_text.lock().await.len(),
                in_flight_bytes: state.in_flight_bytes.load(Ordering::Acquire),
                queue_capacity: PROMPT_EVENT_QUEUE,
                queue_occupancy: state.events.max_capacity() - state.events.capacity(),
                queue_pressure: state.queue_pressure.load(Ordering::Acquire),
                delivered_chunks: state.delivered_chunks.load(Ordering::Acquire),
                coalesced_updates: state.coalesced_updates.load(Ordering::Acquire),
                failure,
            };
            *state
                .diagnostic
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = diagnostic;
            publish_prompt_terminal(
                &mut reservation,
                completion_sender,
                terminal_sender,
                result,
                terminal_outcome,
            )
            .await;
        });

        Ok(PromptHandle {
            events,
            completion,
            stream: state_for_handle,
        })
    }

    pub async fn cancel(&self, session_id: &str) -> Result<CancelOutcome, ProviderError> {
        let pending = self.begin_cancel(session_id).await?;
        self.finish_cancel(pending).await
    }

    pub async fn begin_cancel(&self, session_id: &str) -> Result<PendingCancel, ProviderError> {
        self.ensure_initialized()?;
        let state = self.inner.prompts.lock().await.get(session_id).cloned();
        let Some(state) = state else {
            return Ok(PendingCancel {
                immediate: Some(CancelOutcome::NoPromptInFlight),
                terminal_outcome: None,
            });
        };
        let terminal_outcome = state.terminal_outcome.clone();
        let current_outcome = terminal_outcome.borrow().clone();
        if !matches!(&current_outcome, PromptTerminalOutcome::Pending) {
            return Ok(PendingCancel {
                immediate: Some(cancel_outcome(current_outcome)?),
                terminal_outcome: None,
            });
        }
        self.inner
            .notify("session/cancel", json!({ "sessionId": session_id }))
            .await?;
        Ok(PendingCancel {
            immediate: None,
            terminal_outcome: Some(terminal_outcome),
        })
    }

    pub async fn finish_cancel(
        &self,
        pending: PendingCancel,
    ) -> Result<CancelOutcome, ProviderError> {
        if let Some(outcome) = pending.immediate {
            return Ok(outcome);
        }
        let mut terminal_outcome = pending.terminal_outcome.ok_or(ProviderError::Protocol)?;
        match timeout(CANCEL_GRACE, terminal_outcome.changed()).await {
            Ok(Ok(())) => cancel_outcome(terminal_outcome.borrow().clone()),
            Ok(Err(_)) | Err(_) => {
                self.terminate().await;
                Ok(CancelOutcome::LocalProcessTerminatedWithoutConfirmation)
            }
        }
    }

    /// Confirms only local owned process, transport and proxy cleanup; no remote completion is inferred.
    pub async fn shutdown_local_stop_confirmed(&self) -> bool {
        self.inner.closed.store(true, Ordering::Release);
        self.inner.process.signal_group(libc::SIGTERM);
        if !self.inner.process.wait_for_exit(CANCEL_GRACE).await {
            self.inner.process.signal_group(libc::SIGKILL);
            if !self
                .inner
                .process
                .wait_for_exit(Duration::from_secs(5))
                .await
            {
                return false;
            }
        }
        if !owned_process_exit_confirmed(&self.inner.process) {
            return false;
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        if !self.network_proxy.stop_and_observe(deadline).await {
            return false;
        }
        close_transport(&self.inner, ProviderError::ProcessExited).await;
        for tasks in [
            &self.auth_progress_task,
            &self.reader_task,
            &self.supervisor_task,
        ] {
            let mut slot = tasks.lock().await;
            if let Some(mut task) = slot.take() {
                let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                if !matches!(timeout(remaining, &mut task).await, Ok(Ok(()))) {
                    *slot = Some(task);
                    return false;
                }
            }
        }
        if self.activity_request.as_ref().is_some_and(|request| {
            let state = request.settlement();
            state.provider_operations != 0 || state.unresolved_cleanup != 0
        }) {
            return false;
        }
        if let Some(custody) = self.resource_custody.lock().await.take() {
            custody.retain_until_settlement();
        }
        owned_process_exit_confirmed(&self.inner.process)
    }

    pub async fn shutdown(&self) {
        self.terminate().await;
        if let Some(task) = self.auth_progress_task.lock().await.take() {
            let _ = timeout(CANCEL_GRACE, task).await;
        }
        if let Some(task) = self.reader_task.lock().await.take() {
            let _ = timeout(CANCEL_GRACE, task).await;
        }
        if let Some(task) = self.supervisor_task.lock().await.take() {
            let _ = timeout(CANCEL_GRACE, task).await;
        }
        if let Some(custody) = self.resource_custody.lock().await.take() {
            custody.retain_until_settlement();
        }
    }

    fn ensure_initialized(&self) -> Result<(), ProviderError> {
        if !self.initialized.load(Ordering::Acquire) {
            return Err(ProviderError::Protocol);
        }
        if self.inner.closed.load(Ordering::Acquire) {
            return Err(ProviderError::ProcessClosed);
        }
        Ok(())
    }

    async fn terminate(&self) {
        self.inner.poison().await;
    }
}

fn cancel_outcome(outcome: PromptTerminalOutcome) -> Result<CancelOutcome, ProviderError> {
    match outcome {
        PromptTerminalOutcome::Response { stop_reason } => {
            Ok(CancelOutcome::ProviderTerminal { stop_reason })
        }
        PromptTerminalOutcome::Error(error) => Err(error),
        PromptTerminalOutcome::Pending => Err(ProviderError::ProcessClosed),
    }
}

impl Drop for CodexAcpClient {
    fn drop(&mut self) {
        self.inner.closed.store(true, Ordering::Release);
        self.inner.process.signal_group(libc::SIGTERM);
        self.inner.process.signal_group(libc::SIGKILL);
    }
}

impl Inner {
    async fn poison(&self) {
        self.closed.store(true, Ordering::Release);
        if self
            .request_deadline
            .deadline()
            .is_some_and(|deadline| std::time::Instant::now() >= deadline)
        {
            self.process.signal_group(libc::SIGKILL);
        }
        if timeout(WRITE_TIMEOUT, self.writer.lock()).await.is_err() {
            self.process.signal_group(libc::SIGKILL);
        }
        let _event_gate = self.event_gate.lock().await;
        self.process.terminate_and_reap().await;
        close_transport(self, ProviderError::ProcessExited).await;
        self.prompts.lock().await.clear();
    }

    async fn call(&self, method: &str, params: Value) -> Result<Value, ProviderError> {
        self.begin_call(method, params).await?.wait(self).await
    }

    async fn begin_call(&self, method: &str, params: Value) -> Result<PendingRpc, ProviderError> {
        self.begin_call_authorized(method, params, None).await
    }

    async fn begin_call_authorized(
        &self,
        method: &str,
        params: Value,
        authorization: Option<&crate::verification::PromptPublicationAuthorization>,
    ) -> Result<PendingRpc, ProviderError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(ProviderError::ProcessClosed);
        }
        let id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = oneshot::channel();
        let mut pending = self.pending.lock().await;
        let deadline = self.request_deadline.deadline();
        pending.insert(
            id,
            PendingRequest {
                method: method.to_owned(),
                sender,
            },
        );
        drop(pending);
        if let Err(error) = self
            .write_message_authorized(
                json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }),
                rpc_remediation(method),
                FramePurpose::Effect,
                authorization,
            )
            .await
        {
            self.pending.lock().await.remove(&id);
            return Err(error);
        }
        Ok(PendingRpc {
            deadline,
            id,
            method: method.to_owned(),
            response: receiver,
        })
    }

    async fn notify(&self, method: &str, params: Value) -> Result<(), ProviderError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(ProviderError::ProcessClosed);
        }

        let purpose = if method == "session/cancel" {
            let session = params
                .get("sessionId")
                .and_then(Value::as_str)
                .ok_or(ProviderError::Protocol)?;
            if !self.prompts.lock().await.contains_key(session) {
                return Err(ProviderError::Protocol);
            }
            FramePurpose::CancelOwnedPrompt
        } else {
            FramePurpose::Effect
        };
        self.write_message(
            json!({ "jsonrpc": "2.0", "method": method, "params": params }),
            rpc_remediation(method),
            purpose,
        )
        .await
    }

    async fn write_message(
        &self,
        message: Value,
        remediation: ProviderRemediation,
        purpose: FramePurpose,
    ) -> Result<(), ProviderError> {
        self.write_message_authorized(message, remediation, purpose, None)
            .await
    }

    async fn write_message_authorized(
        &self,
        message: Value,
        remediation: ProviderRemediation,
        purpose: FramePurpose,
        authorization: Option<&crate::verification::PromptPublicationAuthorization>,
    ) -> Result<(), ProviderError> {
        let frame_deadline = self.request_deadline.deadline();
        if matches!(purpose, FramePurpose::Effect)
            && (!self.process.observation_is_fresh()
                || frame_deadline.is_some_and(|deadline| std::time::Instant::now() >= deadline))
        {
            self.poison().await;
            return Err(ProviderError::ProcessClosed);
        }
        let mut bytes = zeroize::Zeroizing::new(
            serde_json::to_vec(&message).map_err(|_| ProviderError::Protocol)?,
        );
        if bytes.len() > MAX_PROTOCOL_FRAME {
            return Err(ProviderError::OutputLimit);
        }
        bytes.push(b'\n');
        let authority = match purpose {
            FramePurpose::Effect => self.effect_authority.as_ref(),
            FramePurpose::CancelOwnedPrompt => None,
        };
        let mut writer = match lock_effect_writer(
            &self.writer,
            authority,
            remediation,
            if matches!(purpose, FramePurpose::Effect) {
                frame_deadline
            } else {
                None
            },
        )
        .await
        {
            Ok(writer) => writer,
            Err(ProviderError::Cancelled) => return Err(ProviderError::Cancelled),
            Err(error) => {
                self.poison().await;
                return Err(error);
            }
        };
        if self.closed.load(Ordering::Acquire) {
            return Err(ProviderError::ProcessClosed);
        }
        if matches!(purpose, FramePurpose::Effect)
            && (!self.process.observation_is_fresh()
                || frame_deadline.is_some_and(|deadline| std::time::Instant::now() >= deadline))
        {
            drop(writer);
            self.poison().await;
            return Err(ProviderError::ProcessClosed);
        }
        let write = timeout(
            WRITE_TIMEOUT,
            write_authorized_effect_frame(
                &mut *writer,
                &bytes,
                authority,
                if matches!(purpose, FramePurpose::Effect) {
                    frame_deadline.map(|deadline| (deadline, &self.closed))
                } else {
                    None
                },
                authorization,
            ),
        )
        .await;
        match write {
            Ok(Ok(())) => Ok(()),
            Ok(Err(ProviderError::Cancelled)) => Err(ProviderError::Cancelled),
            Ok(Err(_)) => {
                self.closed.store(true, Ordering::Release);
                drop(writer);
                self.poison().await;
                Err(ProviderError::ProcessClosed)
            }
            Err(_) => {
                self.closed.store(true, Ordering::Release);
                drop(writer);
                self.poison().await;
                Err(ProviderError::RpcTimeout { remediation })
            }
        }
    }
}

async fn read_auth_progress(
    mut stderr: ChildStderr,
    progress: watch::Sender<AuthProgressStage>,
    browser_requests: mpsc::Sender<Result<String, ()>>,
) {
    let mut buffer = [0u8; 256];
    let mut line = [0u8; MAX_AUTH_PROGRESS_MARKER_BYTES];
    let mut line_len = 0;
    let mut overflowed = false;
    loop {
        let read = match stderr.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(read) => read,
        };
        for byte in &buffer[..read] {
            if *byte == b'\n' {
                let is_browser_request = line_len >= AUTH_OPEN_REQUEST_PREFIX.len()
                    && line[..AUTH_OPEN_REQUEST_PREFIX.len()] == AUTH_OPEN_REQUEST_PREFIX[..];
                if is_browser_request {
                    let request = if overflowed {
                        Err(())
                    } else {
                        let url = &line[AUTH_OPEN_REQUEST_PREFIX.len()..line_len];
                        if url.is_empty() || url.iter().any(|byte| byte.is_ascii_control()) {
                            Err(())
                        } else {
                            String::from_utf8(url.to_vec()).map_err(|_| ())
                        }
                    };
                    progress.send_replace(if request.is_ok() {
                        AuthProgressStage::LaunchRequested
                    } else {
                        AuthProgressStage::Failed
                    });
                    if browser_requests.send(request).await.is_err() {
                        return;
                    }
                } else if !overflowed {
                    let content_len = if line_len > 0 && line[line_len - 1] == b'\r' {
                        line_len - 1
                    } else {
                        line_len
                    };
                    let stage = match &line[..content_len] {
                        b"MAGI_AUTH_PROGRESS=opening" => Some(AuthProgressStage::Opening),
                        b"MAGI_AUTH_PROGRESS=browser_opening" => {
                            Some(AuthProgressStage::BrowserOpening)
                        }
                        b"MAGI_AUTH_PROGRESS=failed" => Some(AuthProgressStage::Failed),
                        _ => None,
                    };
                    if let Some(stage) = stage {
                        progress.send_replace(stage);
                    }
                }
                line_len = 0;
                overflowed = false;
            } else if line_len < line.len() {
                line[line_len] = *byte;
                line_len += 1;
            } else {
                overflowed = true;
            }
        }
    }
}

async fn read_protocol(inner: Arc<Inner>, stdout: ChildStdout) -> Result<(), ProviderError> {
    let mut reader = BufReader::new(stdout);
    while let Some(frame) = read_frame(&mut reader).await? {
        if frame.is_empty() {
            continue;
        }
        let message: Value = serde_json::from_slice(&frame).map_err(|_| ProviderError::Protocol)?;
        handle_message(&inner, message).await?;
    }
    close_transport(&inner, ProviderError::ProcessExited).await;
    Ok(())
}

async fn read_frame<R>(reader: &mut BufReader<R>) -> Result<Option<Vec<u8>>, ProviderError>
where
    R: AsyncRead + Unpin,
{
    let mut frame = Vec::with_capacity(4096);
    loop {
        let (chunk, consumed, done) = {
            let available = reader
                .fill_buf()
                .await
                .map_err(|_| ProviderError::ProcessClosed)?;
            if available.is_empty() {
                return if frame.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(frame))
                };
            }
            let newline = available.iter().position(|byte| *byte == b'\n');
            let consumed = newline.map_or(available.len(), |position| position + 1);
            (available[..consumed].to_vec(), consumed, newline.is_some())
        };
        if frame.len().saturating_add(chunk.len()) > MAX_PROTOCOL_FRAME {
            return Err(ProviderError::OutputLimit);
        }
        frame.extend_from_slice(&chunk);
        reader.consume(consumed);
        if done {
            if frame.last() == Some(&b'\n') {
                frame.pop();
            }
            if frame.last() == Some(&b'\r') {
                frame.pop();
            }
            return Ok(Some(frame));
        }
    }
}

async fn handle_message(inner: &Arc<Inner>, message: Value) -> Result<(), ProviderError> {
    if inner.closed.load(Ordering::Acquire) {
        return Ok(());
    }
    if let Some(method) = message.get("method").and_then(Value::as_str) {
        if let Some(id) = message.get("id") {
            let response = if method == "session/request_permission" {
                if let Some(session_id) = message
                    .get("params")
                    .and_then(|params| params.get("sessionId"))
                    .and_then(Value::as_str)
                {
                    deny_tool_use(inner, session_id).await;
                }
                json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": { "outcome": { "outcome": "cancelled" } }
                })
            } else if method == "_magi/auth/refresh" {
                let parameters = message.get("params").and_then(Value::as_object);
                let valid = parameters.is_some_and(|parameters| {
                    parameters
                        .keys()
                        .all(|key| matches!(key.as_str(), "reason" | "previousAccountId"))
                        && parameters.get("reason").and_then(Value::as_str) == Some("unauthorized")
                        && parameters
                            .get("previousAccountId")
                            .is_none_or(|account| account.is_null() || account.is_string())
                });
                let broker = inner.subscription.lock().await.clone();
                let result = if valid {
                    broker
                        .ok_or("credential_authority_unknown")
                        .and_then(|broker| {
                            broker.refresh_parameters(
                                parameters
                                    .and_then(|parameters| parameters.get("previousAccountId"))
                                    .and_then(Value::as_str),
                            )
                        })
                } else {
                    Err("credential_refresh_required")
                };
                match result {
                    Ok(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
                    Err(_) => {
                        json!({"jsonrpc":"2.0","id":id,"error":{"code":-32001,"message":"credential_refresh_required"}})
                    }
                }
            } else if method == "fs/read_text_file" {
                let params = message.get("params").unwrap_or(&Value::Null);
                read_text_file_request_response(inner, id, params).await
            } else {
                json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": { "code": -32601, "message": "Unsupported client method" }
                })
            };
            inner
                .write_message(
                    response,
                    ProviderRemediation::ReviewRequest,
                    FramePurpose::Effect,
                )
                .await?;
            return Ok(());
        }
        if method == "session/update" {
            let params = message
                .get("params")
                .ok_or(ProviderError::InvalidResponse)?;
            let session_id = params
                .get("sessionId")
                .and_then(Value::as_str)
                .ok_or(ProviderError::InvalidResponse)?;
            let update = params.get("update").ok_or(ProviderError::InvalidResponse)?;
            match session_update_payload(update)? {
                SessionUpdatePayload::AgentText(text) => push_text(inner, session_id, text).await,
                SessionUpdatePayload::Notice(notice) => {
                    let mut notices = inner.provider_notices.lock().await;
                    if notices.len() >= 64 {
                        return Err(ProviderError::NoticeLimit);
                    }
                    notices.push(notice);
                }
                SessionUpdatePayload::ToolUse => deny_tool_use(inner, session_id).await,
                SessionUpdatePayload::Other => {}
            }
        }
        return Ok(());
    }

    let id = message
        .get("id")
        .and_then(Value::as_u64)
        .ok_or(ProviderError::Protocol)?;
    let pending = inner.pending.lock().await.remove(&id);
    let Some(pending) = pending else {
        return Ok(());
    };
    let result = if let Some(result) = message.get("result") {
        Ok(result.clone())
    } else if let Some(error) = message.get("error") {
        *inner.rpc_failure.lock().await = Some(rpc_failure_diagnostic(&pending.method, error));
        let app_server_exit = (pending.method == "authenticate")
            .then(|| error.get("data"))
            .flatten()
            .filter(|data| {
                data.get("code").and_then(Value::as_str) == Some("codex_app_server_exited")
            })
            .map(|data| {
                let exit_code = data
                    .get("exitCode")
                    .and_then(Value::as_i64)
                    .and_then(|value| i32::try_from(value).ok())
                    .filter(|value| (0..=255).contains(value));
                ProviderError::CodexAppServerExited { exit_code }
            });
        let browser_open_failed = (pending.method == "authenticate")
            .then(|| error.get("data"))
            .flatten()
            .filter(|data| data.get("code").and_then(Value::as_str) == Some("browser_open_failed"))
            .map(|_| ProviderError::BrowserOpenFailed);
        Err(app_server_exit.or(browser_open_failed).unwrap_or_else(|| {
            match pending.method.as_str() {
                "authentication/status" => ProviderError::AuthenticationStatusRpcFailed,
                "authenticate" => ProviderError::AuthenticationRpcFailed,
                _ => ProviderError::RemoteRequestFailed {
                    remediation: rpc_remediation(&pending.method),
                },
            }
        }))
    } else {
        Err(ProviderError::InvalidResponse)
    };
    if pending.method == "session/new"
        && let Some(session_id) = result
            .as_ref()
            .ok()
            .and_then(|response| response.get("sessionId"))
            .and_then(Value::as_str)
            .filter(|session_id| !session_id.is_empty())
    {
        *inner.active_session_id.lock().await = Some(session_id.to_owned());
    }
    let _ = pending.sender.send(result);
    Ok(())
}

fn optional_u32_parameter(params: &Value, name: &str) -> Result<Option<u32>, ()> {
    match params.get(name) {
        None => Ok(None),
        Some(value) => value
            .as_u64()
            .and_then(|value| u32::try_from(value).ok())
            .map(Some)
            .ok_or(()),
    }
}

fn client_read_error(id: &Value, code: i64, message: &'static str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message }
    })
}

async fn read_text_file_request_response(inner: &Arc<Inner>, id: &Value, params: &Value) -> Value {
    let Some(session_id) = params
        .get("sessionId")
        .and_then(Value::as_str)
        .filter(|session_id| !session_id.is_empty() && session_id.len() <= 256)
    else {
        return client_read_error(id, -32602, "Invalid fs/read_text_file parameters");
    };
    let Some(path) = params
        .get("path")
        .and_then(Value::as_str)
        .filter(|path| !path.is_empty() && path.len() <= 4096)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
    else {
        return client_read_error(id, -32602, "Invalid fs/read_text_file parameters");
    };
    let line = match optional_u32_parameter(params, "line") {
        Ok(line) => line,
        Err(()) => return client_read_error(id, -32602, "Invalid fs/read_text_file parameters"),
    };
    let limit = match optional_u32_parameter(params, "limit") {
        Ok(limit) => limit,
        Err(()) => return client_read_error(id, -32602, "Invalid fs/read_text_file parameters"),
    };
    if inner.active_session_id.lock().await.as_deref() != Some(session_id) {
        return client_read_error(
            id,
            -32001,
            "The file is outside the approved read scope or is unavailable.",
        );
    }
    let Some(reader) = inner.source_reader.clone() else {
        return client_read_error(
            id,
            -32001,
            "The file is outside the approved read scope or is unavailable.",
        );
    };
    let content =
        tokio::task::spawn_blocking(move || reader.read_text_file(&path, line, limit)).await;
    match content {
        Ok(Ok(content)) if content.len() <= MAX_RESPONSE_BYTES => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": { "content": content }
        }),
        _ => client_read_error(
            id,
            -32001,
            "The file is outside the approved read scope or is unavailable.",
        ),
    }
}

async fn deliver_prompt_text(
    state: Arc<PendingPrompt>,
    authority: Option<&crate::verification::EffectAuthority>,
) -> Result<(), ProviderError> {
    let deadline = tokio::time::Instant::now() + REQUEST_TIMEOUT;
    loop {
        let notified = state.delivery_wake.notified();
        let text = std::mem::take(&mut *state.pending_text.lock().await);
        if text.is_empty() {
            if state.delivery_finished.load(Ordering::Acquire) {
                return Ok(());
            }
            match authority {
                Some(authority) => tokio::select! {
                    _ = authority.cancelled() => return Err(ProviderError::Cancelled),
                    result = tokio::time::timeout_at(deadline, notified) => {
                        result.map_err(|_| ProviderError::Timeout)?;
                    }
                },
                None => tokio::time::timeout_at(deadline, notified)
                    .await
                    .map_err(|_| ProviderError::Timeout)?,
            }
            continue;
        }
        state.in_flight_bytes.store(text.len(), Ordering::Release);
        let mut remaining = text.as_str();
        while !remaining.is_empty() {
            let mut boundary = remaining.len().min(MAX_RESPONSE_BYTES / 8);
            while !remaining.is_char_boundary(boundary) {
                boundary -= 1;
            }
            let (chunk, rest) = remaining.split_at(boundary);
            remaining = rest;
            let event = PromptEvent::TextDelta(chunk.to_owned());
            if serde_json::to_vec(&event)
                .map_err(|_| ProviderError::InvalidResponse)?
                .len()
                > MAX_RESPONSE_BYTES
            {
                return Err(ProviderError::OutputLimit);
            }
            if state.events.capacity() == 0 {
                state.queue_pressure.fetch_add(1, Ordering::Relaxed);
            }
            let send = tokio::time::timeout_at(deadline, state.events.send(event));
            let result = match authority {
                Some(authority) => tokio::select! {
                    _ = authority.cancelled() => return Err(ProviderError::Cancelled),
                    result = send => result,
                },
                None => send.await,
            };
            result.map_err(|_| ProviderError::Timeout)?.map_err(|_| {
                state.consumer_closed.store(true, Ordering::Release);
                ProviderError::StreamConsumerClosed
            })?;
            state
                .in_flight_bytes
                .store(remaining.len(), Ordering::Release);
            state.delivered_chunks.fetch_add(1, Ordering::Relaxed);
        }
    }
}

async fn append_prompt_text(state: &PendingPrompt, text: &str) -> bool {
    if state.event_count.fetch_add(1, Ordering::Relaxed) >= MAX_PROMPT_EVENTS {
        state.event_limited.store(true, Ordering::Release);
        return true;
    }
    let mut accumulated = state.text.lock().await;
    if accumulated.len().saturating_add(text.len()) > MAX_RESPONSE_BYTES {
        state.output_limited.store(true, Ordering::Release);
        return true;
    }
    accumulated.push_str(text);
    let mut pending = state.pending_text.lock().await;
    if !pending.is_empty() {
        state.coalesced_updates.fetch_add(1, Ordering::Relaxed);
    }
    pending.push_str(text);
    state.delivery_wake.notify_one();
    false
}

async fn push_text(inner: &Arc<Inner>, session_id: &str, text: &str) {
    let cancel = {
        let _event_gate = inner.event_gate.lock().await;
        if inner.closed.load(Ordering::Acquire) {
            return;
        }
        let state = inner.prompts.lock().await.get(session_id).cloned();
        let Some(state) = state else {
            return;
        };
        append_prompt_text(&state, text).await
    };
    if cancel {
        let _ = inner
            .notify("session/cancel", json!({ "sessionId": session_id }))
            .await;
    }
}

async fn deny_tool_use(inner: &Arc<Inner>, session_id: &str) {
    {
        let _event_gate = inner.event_gate.lock().await;
        if inner.closed.load(Ordering::Acquire) {
            return;
        }
        let state = inner.prompts.lock().await.get(session_id).cloned();
        if let Some(state) = state
            && !state.tool_denied.swap(true, Ordering::AcqRel)
        {
            let _ = state.events.try_send(PromptEvent::SecurityViolation);
        }
    }
    let _ = inner
        .notify("session/cancel", json!({ "sessionId": session_id }))
        .await;
}

async fn close_transport(inner: &Inner, error: ProviderError) {
    inner.closed.store(true, Ordering::Release);
    let _ = timeout(WRITE_TIMEOUT, inner.writer.lock()).await;
    let protocol_error = matches!(error, ProviderError::Protocol);
    let mut pending = inner.pending.lock().await;
    for (_, request) in pending.drain() {
        let error = if protocol_error {
            ProviderError::Protocol
        } else {
            ProviderError::ProcessExited
        };
        let _ = request.sender.send(Err(error));
    }
}

fn rpc_remediation(method: &str) -> ProviderRemediation {
    match method {
        "authentication/status" | "authenticate" => ProviderRemediation::Reauthenticate,
        _ => ProviderRemediation::ReviewRequest,
    }
}

async fn supervise_child(
    inner: Arc<Inner>,
    mut child: Child,
    exit_sender: watch::Sender<Option<i32>>,
    activity: Option<crate::verification::ActivityLease>,
) {
    let status = child.wait().await;
    let exit_code = status
        .as_ref()
        .ok()
        .and_then(ExitStatus::code)
        .unwrap_or(-1);
    let _ = exit_sender.send(Some(exit_code));
    close_transport(&inner, ProviderError::ProcessExited).await;
    if status.is_ok()
        && settle_owned_process_group(inner.process.process_group_id).await
        && let Some(activity) = activity
    {
        activity.settle();
    }
}

struct FlagReset<'a>(&'a AtomicBool);

impl Drop for FlagReset<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

fn valid_catalog_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value.trim() == value
        && !value.chars().any(char::is_control)
}

fn required_catalog_id(value: Option<&Value>) -> Result<String, ProviderError> {
    value
        .and_then(Value::as_str)
        .filter(|id| valid_catalog_id(id))
        .map(str::to_owned)
        .ok_or(ProviderError::InvalidResponse)
}

fn collect_config_choices(
    value: &Value,
    depth: u8,
    choices: &mut HashSet<String>,
) -> Result<(), ProviderError> {
    if depth > 3 {
        return Err(ProviderError::InvalidResponse);
    }
    let options = value
        .as_array()
        .filter(|options| !options.is_empty() && options.len() <= MAX_CATALOG_MODELS)
        .ok_or(ProviderError::InvalidResponse)?;
    for option in options {
        if option.get("value").is_some() {
            let value = required_catalog_id(option.get("value"))?;
            if !choices.insert(value) || choices.len() > MAX_CATALOG_MODELS {
                return Err(ProviderError::InvalidResponse);
            }
        } else {
            collect_config_choices(
                option
                    .get("options")
                    .ok_or(ProviderError::InvalidResponse)?,
                depth + 1,
                choices,
            )?;
        }
    }
    Ok(())
}

fn parse_session_info(
    response: &Value,
    selected: Option<&str>,
) -> Result<SessionInfo, ProviderError> {
    let session_id = required_catalog_id(response.get("sessionId"))?;
    let model_state = response
        .get("models")
        .ok_or(ProviderError::InvalidResponse)?;
    let actual_current_model = required_catalog_id(model_state.get("currentModelId"))?;
    let models = model_state
        .get("availableModels")
        .and_then(Value::as_array)
        .filter(|models| !models.is_empty() && models.len() <= MAX_CATALOG_MODELS)
        .ok_or(ProviderError::InvalidResponse)?;
    let mut ids = HashSet::new();
    let available_models = models
        .iter()
        .map(|model| {
            let model_id = required_catalog_id(model.get("modelId"))?;
            if !ids.insert(model_id.clone()) {
                return Err(ProviderError::InvalidResponse);
            }
            let optional_limit = |name| -> Result<Option<u64>, ProviderError> {
                match model.get(name) {
                    None | Some(Value::Null) => Ok(None),
                    Some(value) => value
                        .as_u64()
                        .filter(|n| *n > 0)
                        .map(Some)
                        .ok_or(ProviderError::InvalidResponse),
                }
            };
            Ok(AvailableModel {
                model_id,
                name: model.get("name").and_then(Value::as_str).map(str::to_owned),
                description: model
                    .get("description")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                context_window_tokens: optional_limit("contextWindowTokens")?,
                max_output_tokens: optional_limit("maxOutputTokens")?,
            })
        })
        .collect::<Result<Vec<_>, ProviderError>>()?;
    if !available_models
        .iter()
        .any(|model| model.model_id == actual_current_model)
    {
        return Err(ProviderError::InvalidResponse);
    }
    let (current_mode_id, available_modes) = match response.get("modes") {
        None => (None, Vec::new()),
        Some(state) => {
            let current = required_catalog_id(state.get("currentModeId"))?;
            let modes = state
                .get("availableModes")
                .and_then(Value::as_array)
                .filter(|modes| !modes.is_empty() && modes.len() <= 64)
                .ok_or(ProviderError::InvalidResponse)?;
            let mut ids = HashSet::new();
            let modes = modes
                .iter()
                .map(|mode| {
                    let mode_id = required_catalog_id(mode.get("id"))?;
                    if !ids.insert(mode_id.clone()) {
                        return Err(ProviderError::InvalidResponse);
                    }
                    Ok(AvailableMode {
                        mode_id,
                        name: required_catalog_id(mode.get("name"))?,
                        description: mode
                            .get("description")
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                    })
                })
                .collect::<Result<Vec<_>, ProviderError>>()?;
            if !modes.iter().any(|mode| mode.mode_id == current) {
                return Err(ProviderError::InvalidResponse);
            }
            (Some(current), modes)
        }
    };
    if let Some(options) = response.get("configOptions") {
        let options = options
            .as_array()
            .filter(|options| options.len() <= 128)
            .ok_or(ProviderError::InvalidResponse)?;
        let mut ids = HashSet::new();
        for option in options {
            let id = required_catalog_id(option.get("id"))?;
            if !ids.insert(id.clone()) {
                return Err(ProviderError::InvalidResponse);
            }
            if matches!(id.as_str(), "model" | "reasoning_effort" | "mode") {
                let current = required_catalog_id(option.get("currentValue"))?;
                let mut choices = HashSet::new();
                collect_config_choices(
                    option
                        .get("options")
                        .ok_or(ProviderError::InvalidResponse)?,
                    0,
                    &mut choices,
                )?;
                if !choices.contains(&current) {
                    return Err(ProviderError::InvalidResponse);
                }
                let expected = match id.as_str() {
                    "model" => Some(
                        split_model_id(&actual_current_model)
                            .map_or(actual_current_model.as_str(), |(model, _)| model),
                    ),
                    "reasoning_effort" => {
                        split_model_id(&actual_current_model).map(|(_, effort)| effort)
                    }
                    "mode" => current_mode_id.as_deref(),
                    _ => None,
                };
                if expected.is_some_and(|expected| current != expected) {
                    return Err(ProviderError::InvalidResponse);
                }
                if id == "mode" && current_mode_id.is_none() {
                    return Err(ProviderError::InvalidResponse);
                }
            }
        }
    }
    let desired = selected.unwrap_or(&actual_current_model);
    let selected_model = available_models
        .iter()
        .find(|model| model.model_id == desired);
    let selected_model_available = selected_model.is_some();
    let actual_model = available_models
        .iter()
        .find(|model| model.model_id == actual_current_model);
    let context_window_tokens = actual_model.and_then(|model| model.context_window_tokens);
    let max_output_tokens = actual_model.and_then(|model| model.max_output_tokens);
    let current_model_id = actual_current_model;
    Ok(SessionInfo {
        session_id,
        current_model_id,
        available_models,
        current_mode_id,
        available_modes,
        context_window_tokens,
        max_output_tokens,
        selected_model_available,
    })
}

fn split_model_id(value: &str) -> Option<(&str, &str)> {
    let (model, effort) = value.rsplit_once('[')?;
    let effort = effort.strip_suffix(']')?;
    if model.is_empty() || effort.is_empty() {
        return None;
    }
    Some((model, effort))
}

fn config_value<'a>(response: &'a Value, config_id: &str) -> Option<&'a str> {
    response
        .get("configOptions")?
        .as_array()?
        .iter()
        .find(|option| option.get("id").and_then(Value::as_str) == Some(config_id))?
        .get("currentValue")?
        .as_str()
}

fn parse_usage(value: Option<&Value>) -> Option<ProviderUsage> {
    let usage = value?;
    Some(ProviderUsage {
        total_tokens: usage.get("totalTokens").and_then(Value::as_u64),
        input_tokens: usage.get("inputTokens").and_then(Value::as_u64),
        cached_read_tokens: usage.get("cachedReadTokens").and_then(Value::as_u64),
        output_tokens: usage.get("outputTokens").and_then(Value::as_u64),
        thought_tokens: usage.get("thoughtTokens").and_then(Value::as_u64),
    })
}

#[cfg(test)]
mod rpc_diagnostic_tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::process::Stdio;

    #[tokio::test]
    async fn owned_process_group_and_proxy_must_reap_before_request_settlement() {
        use std::os::unix::fs::PermissionsExt;
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let request = crate::VerificationRequest::until(deadline);
        let name = format!(
            "provider-settlement-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let home = std::env::temp_dir().canonicalize().unwrap().join(&name);
        fs::create_dir(&home).unwrap();
        fs::set_permissions(&home, fs::Permissions::from_mode(0o700)).unwrap();
        let lock = HomeOperationLock::acquire(&home, &name).unwrap();
        let proxy = ProviderNetworkProxy::start_tracked(Some(request.provider_activity().unwrap()))
            .unwrap();
        let connection = TcpStream::connect(proxy.local_addr()).unwrap();
        let wait = deadline;
        while proxy.state.active.lock().unwrap().is_empty() {
            assert!(std::time::Instant::now() < wait);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let mut child = tokio::process::Command::new("/bin/sh")
            .args(["-c", "/bin/sleep 10 & printf '%s\\n' \"$!\"; wait"])
            .env_clear()
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let parent = child.id().unwrap() as i32;
        let mut output = BufReader::new(child.stdout.take().unwrap());
        let mut stderr = child.stderr.take().unwrap();
        let mut activity = request.provider_activity().unwrap();
        activity.mark_owned_process();
        let mut guard = StartupProcessGuard::new(child, lock, Some(activity));
        let mut line = String::new();
        let ready = tokio::time::timeout_at(
            tokio::time::Instant::from_std(deadline),
            output.read_line(&mut line),
        )
        .await;
        if !matches!(ready, Ok(Ok(size)) if size > 0) {
            request.revoke();
            drop(output);
            drop(stderr);
            drop(connection);
            drop(proxy);
            guard.cleanup().await;
            request
                .wait_for_settlement(std::time::Instant::now() + Duration::from_secs(6))
                .await
                .unwrap();
            panic!("Owned subprocess readiness failed after observed cleanup");
        }
        let descendant: i32 = line.trim().parse().unwrap();
        let reader_activity = request.provider_activity().unwrap();
        let reader = tokio::spawn(async move {
            let _activity = reader_activity;
            let mut bytes = Vec::new();
            output.read_to_end(&mut bytes).await.unwrap();
        });
        let auth_activity = request.provider_activity().unwrap();
        let auth_reader = tokio::spawn(async move {
            let _activity = auth_activity;
            let mut bytes = Vec::new();
            stderr.read_to_end(&mut bytes).await.unwrap();
        });
        request.revoke();
        assert_eq!(request.settlement().provider_operations, 4);
        assert!(matches!(
            request
                .wait_for_settlement(std::time::Instant::now() + Duration::from_millis(30))
                .await,
            Err(ProviderError::Timeout)
        ));
        drop(guard);
        drop(proxy);
        drop(connection);
        assert!(
            request
                .wait_for_settlement(std::time::Instant::now() + Duration::from_secs(6))
                .await
                .unwrap()
                .is_settled()
        );
        reader.await.unwrap();
        auth_reader.await.unwrap();
        for process in [parent, descendant] {
            assert_eq!(unsafe { libc::kill(process, 0) }, -1);
            assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
        }
        fs::remove_dir_all(home).unwrap();
    }

    #[tokio::test]
    async fn revoked_queued_writer_never_acquires_first_byte_permission() {
        let request =
            crate::VerificationRequest::until(std::time::Instant::now() + Duration::from_secs(2));
        let authority = request.effect_authority();
        let writer = Arc::new(Mutex::new(Vec::<u8>::new()));
        let held = writer.lock().await;
        let queued_writer = writer.clone();
        let queued = tokio::spawn(async move {
            lock_effect_writer(
                &queued_writer,
                Some(&authority),
                ProviderRemediation::ReviewRequest,
                None,
            )
            .await
            .map(|mut value| value.push(1))
        });
        tokio::task::yield_now().await;
        request.revoke();
        assert!(matches!(
            timeout(Duration::from_secs(1), queued)
                .await
                .unwrap()
                .unwrap(),
            Err(ProviderError::Cancelled)
        ));
        assert!(held.is_empty());
    }

    struct ObservedWriter {
        stream: tokio::io::DuplexStream,
        first_poll: Option<oneshot::Sender<()>>,
        stall_flush: bool,
    }

    impl AsyncWrite for ObservedWriter {
        fn poll_write(
            mut self: Pin<&mut Self>,
            context: &mut Context<'_>,
            bytes: &[u8],
        ) -> Poll<io::Result<usize>> {
            if let Some(sender) = self.first_poll.take() {
                let _ = sender.send(());
            }
            Pin::new(&mut self.stream).poll_write(context, bytes)
        }
        fn poll_flush(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
            if self.stall_flush {
                return Poll::Pending;
            }
            Pin::new(&mut self.stream).poll_flush(context)
        }
        fn poll_shutdown(
            mut self: Pin<&mut Self>,
            context: &mut Context<'_>,
        ) -> Poll<io::Result<()>> {
            Pin::new(&mut self.stream).poll_shutdown(context)
        }
    }

    #[tokio::test]
    async fn effect_write_revocation_before_first_byte_and_after_partial_write_are_distinct() {
        for partial in [false, true] {
            let request = crate::VerificationRequest::until(
                std::time::Instant::now() + Duration::from_secs(2),
            );
            let authority = request.effect_authority();
            let (mut stream, mut reader) = tokio::io::duplex(1);
            if !partial {
                stream.write_all(b"x").await.unwrap();
            }
            let (sender, observed) = oneshot::channel();
            let mut writer = ObservedWriter {
                stream,
                first_poll: Some(sender),
                stall_flush: false,
            };
            let task = tokio::spawn(async move {
                write_effect_frame(&mut writer, b"ab", Some(&authority), None).await
            });
            observed.await.unwrap();
            let mut accepted = [0_u8; 1];
            request.revoke();
            let result = timeout(Duration::from_secs(1), task)
                .await
                .unwrap()
                .unwrap();
            if partial {
                assert!(matches!(result, Err(ProviderError::ProcessClosed)));
                reader.read_exact(&mut accepted).await.unwrap();
                assert_eq!(&accepted, b"a");
            } else {
                assert!(matches!(result, Err(ProviderError::Cancelled)));
            }
            let mut remaining = Vec::new();
            reader.read_to_end(&mut remaining).await.unwrap();
            assert_eq!(remaining, if partial { Vec::new() } else { b"x".to_vec() });
        }
    }

    #[tokio::test]
    async fn expired_proof_deadline_does_not_expire_live_effect_authority() {
        let root = crate::VerificationRequest::until(std::time::Instant::now());
        assert!(matches!(root.check(), Err(ProviderError::Timeout)));
        let phase = root.with_deadline(std::time::Instant::now() + Duration::from_secs(1));
        assert!(phase.check().is_ok());
        let authority = phase.effect_authority();
        let (mut writer, mut reader) = tokio::io::duplex(32);
        write_effect_frame(&mut writer, b"bounded-frame", Some(&authority), None)
            .await
            .unwrap();
        let mut bytes = [0_u8; 13];
        reader.read_exact(&mut bytes).await.unwrap();
        assert_eq!(&bytes, b"bounded-frame");
        root.revoke();
        assert!(matches!(
            write_effect_frame(&mut writer, b"late", Some(&authority), None).await,
            Err(ProviderError::Cancelled)
        ));
    }

    #[tokio::test]
    async fn revocation_during_pending_flush_preserves_full_frame_unknown_outcome() {
        let request =
            crate::VerificationRequest::until(std::time::Instant::now() + Duration::from_secs(2));
        let authority = request.effect_authority();
        let (stream, mut reader) = tokio::io::duplex(32);
        let (sender, observed) = oneshot::channel();
        let mut writer = ObservedWriter {
            stream,
            first_poll: Some(sender),
            stall_flush: true,
        };
        let task = tokio::spawn(async move {
            write_effect_frame(&mut writer, b"frame", Some(&authority), None).await
        });
        observed.await.unwrap();
        request.revoke();
        assert!(matches!(
            timeout(Duration::from_secs(1), task)
                .await
                .unwrap()
                .unwrap(),
            Err(ProviderError::ProcessClosed)
        ));
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await.unwrap();
        assert_eq!(bytes, b"frame");
    }

    #[test]
    fn certificate_backend_diagnostic_changes_only_explicit_ca_environment() {
        for (policy, removed) in [
            (CertificatePolicy::Configured, false),
            (CertificatePolicy::PlatformRootsDiagnostic, true),
        ] {
            let mut command = tokio::process::Command::new("unused-local-control");
            command
                .env_clear()
                .env("CODEX_CA_CERTIFICATE", "public-ca-reference")
                .env("CONTROL", "unchanged");
            policy.apply(&mut command);
            let ca = command
                .as_std()
                .get_envs()
                .find(|(key, _)| *key == "CODEX_CA_CERTIFICATE")
                .and_then(|(_, value)| value);
            assert!(ca.is_none() == removed);
            assert!(
                command
                    .as_std()
                    .get_envs()
                    .any(|(key, value)| key == "CONTROL"
                        && value.is_some_and(|value| value == "unchanged"))
            );
        }
    }

    #[derive(Debug, serde::Deserialize, serde::Serialize)]
    #[serde(rename_all = "camelCase", deny_unknown_fields)]
    struct AccountsDiagnostic {
        phase: AccountsDiagnosticPhase,
        http_status: Option<u16>,
        response_class: AccountsResponseClass,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        certificate_verify_code: Option<u16>,
    }

    #[derive(Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
    #[serde(rename_all = "snake_case")]
    enum AccountsDiagnosticPhase {
        Input,
        Proxy,
        Ca,
        Tls,
        Http,
        Response,
    }

    #[derive(Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
    #[serde(rename_all = "snake_case")]
    enum AccountsResponseClass {
        AccountsArray,
        AccountsMap,
        CaReady,
        InvalidJson,
        InvalidShape,
        BodyLimit,
        StatusError,
        ProxyRejected,
        CertificateFailure,
        CertificateIssuerUnavailable,
        CertificateSignatureUnverifiable,
        CertificateNameMismatch,
        CertificateExpired,
        CertificateNotYetValid,
        CertificateSelfSigned,
        CertificateRevoked,
        TlsFailure,
        Timeout,
        TransportFailure,
    }

    const ACCOUNTS_DIAGNOSTIC_CLIENT: &str = r#"
import base64, hashlib, http.client, json, os, re, socket, ssl, stat, sys

LIMIT = 262144
def pem_context(bundle):
    blocks = re.findall(br"-----BEGIN CERTIFICATE-----.*?-----END CERTIFICATE-----", bundle, re.S)
    if (len(blocks) != 121 or bundle.count(b"-----BEGIN CERTIFICATE-----") != 121
            or bundle.count(b"-----END CERTIFICATE-----") != 121):
        raise ValueError()
    context = ssl.create_default_context(cadata=b"\n".join(blocks).decode("ascii"))
    if (context.cert_store_stats()["x509"] != 121 or not context.check_hostname
            or context.verify_mode != ssl.CERT_REQUIRED):
        raise ValueError()
    return context

def account_fields(value):
    return (isinstance(value, dict)
            and all(value.get(key) is None or isinstance(value[key], str)
                    for key in ("plan_type", "name", "profile_picture_url", "workspace_backend_origin", "account_routing_override"))
            and isinstance(value.get("structure", ""), str))

def classify(status, body):
    if len(body) > LIMIT:
        return "body_limit"
    if not 200 <= status < 300:
        return "status_error"
    try:
        value = json.loads(body)
    except (ValueError, UnicodeError):
        return "invalid_json"
    if not isinstance(value, dict) or "accounts" not in value:
        return "invalid_shape"
    accounts = value.get("accounts", [])
    ordering = value.get("account_ordering", [])
    default = value.get("default_account_id")
    if not isinstance(ordering, list) or any(not isinstance(item, str) for item in ordering):
        return "invalid_shape"
    if default is not None and not isinstance(default, str):
        return "invalid_shape"
    if isinstance(accounts, list):
        valid = all(account_fields(item) and isinstance(item.get("id"), str) for item in accounts)
        category = "accounts_array"
    elif isinstance(accounts, dict):
        valid = all(isinstance(item, dict) and account_fields(item.get("account"))
                    and (item["account"].get("account_id") is None or isinstance(item["account"].get("account_id"), str))
                    for item in accounts.values())
        category = "accounts_map"
    else:
        valid = False
    return category if valid else "invalid_shape"

def emit(status, category, phase="response", verify_code=None):
    value = {"phase":phase,"httpStatus": status, "responseClass": category}
    if verify_code is not None:
        value["certificateVerifyCode"] = verify_code
    print(json.dumps(value, separators=(",", ":")))

def emit_certificate_failure(error, phase, status):
    code = getattr(error, "verify_code", None)
    if type(code) is not int or not 0 <= code <= 1024:
        code = None
    category = {2:"certificate_issuer_unavailable",20:"certificate_issuer_unavailable",
                21:"certificate_signature_unverifiable",62:"certificate_name_mismatch",64:"certificate_name_mismatch",
                10:"certificate_expired",9:"certificate_not_yet_valid",18:"certificate_self_signed",
                19:"certificate_self_signed",23:"certificate_revoked"}.get(code,"certificate_failure")
    emit(status, category, phase, code)

def diagnostic():
    connection = None
    status = None
    phase = "input"
    try:
        authority = json.loads(sys.stdin.buffer.read(65537))
        token = authority.pop("accessToken")
        account = authority.pop("chatgptAccountId")
        if any(not isinstance(value, str) or not value or "\r" in value or "\n" in value for value in (token, account)):
            raise ValueError()
        phase = "proxy"
        host, port = os.environ["PROXY_ADDRESS"].rsplit(":", 1)
        connection = socket.create_connection((host, int(port)), timeout=10)
        capability = os.environ.pop("PROXY_PASSWORD")
        authorization = base64.b64encode(("magi:" + capability).encode())
        connection.sendall(b"CONNECT chatgpt.com:443 HTTP/1.1\r\nHost: chatgpt.com:443\r\nProxy-Authorization: Basic " + authorization + b"\r\n\r\n")
        headers = b""
        while not headers.endswith(b"\r\n\r\n") and len(headers) < 16384:
            byte = connection.recv(1)
            if not byte:
                break
            headers += byte
        if headers.split(b"\r\n", 1)[0] != b"HTTP/1.1 200 Connection Established":
            emit(None, "proxy_rejected", phase)
            return
        phase = "ca"
        descriptor = os.open(os.environ["PUBLIC_CA_FILE"], os.O_RDONLY | os.O_NOFOLLOW)
        with os.fdopen(descriptor, "rb") as ca:
            metadata = os.fstat(ca.fileno())
            if not stat.S_ISREG(metadata.st_mode) or metadata.st_nlink != 1 or metadata.st_mode & 0o222:
                raise ValueError()
            public_ca = ca.read(4194305)
        if len(public_ca) > 4194304 or hashlib.sha256(public_ca).hexdigest() != "a41b5d356aea97a529fe27e0f7316d2f9d946d75927476cf9cf1b90637d00505":
            raise ValueError()
        context = pem_context(public_ca)
        phase = "tls"
        connection = context.wrap_socket(connection, server_hostname="chatgpt.com")
        phase = "http"
        request = ("GET /backend-api/wham/accounts/check HTTP/1.1\r\nHost: chatgpt.com\r\nUser-Agent: codex-cli\r\nAuthorization: Bearer " + token
                   + "\r\nChatGPT-Account-Id: " + account + "\r\nConnection: close\r\n\r\n").encode()
        connection.sendall(request)
        token = account = request = authority = None
        response = http.client.HTTPResponse(connection)
        response.begin()
        status = response.status
        phase = "response"
        body = response.read(LIMIT + 1)
        category = classify(status, body)
        body = None
        emit(status, category, phase)
    except ssl.SSLCertVerificationError as error:
        emit_certificate_failure(error, phase, status)
    except ssl.SSLError:
        emit(status, "tls_failure", phase)
    except (TimeoutError, socket.timeout):
        emit(status, "timeout", phase)
    except Exception:
        emit(status, "transport_failure", phase)
    finally:
        if connection is not None:
            connection.close()
"#;

    async fn accounts_diagnostic_process(
        script: String,
        input: zeroize::Zeroizing<Vec<u8>>,
        variables: &[(&str, &std::ffi::OsStr)],
    ) -> AccountsDiagnostic {
        let python = std::env::var_os("MAGI_TEST_TLS_PYTHON")
            .unwrap_or_else(|| std::ffi::OsString::from("/usr/bin/python3"));
        let mut command = tokio::process::Command::new(python);
        command
            .env_clear()
            .args(["-c", &script])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        for (name, value) in variables {
            command.env(name, value);
        }
        let mut child = command.spawn().expect("diagnostic process unavailable");
        let mut stdin = child.stdin.take().expect("diagnostic input unavailable");
        let mut stdout = child.stdout.take().expect("diagnostic output unavailable");
        let outcome = timeout(Duration::from_secs(45), async {
            stdin.write_all(&input).await?;
            stdin.shutdown().await?;
            drop(stdin);
            drop(input);
            let mut output = Vec::new();
            (&mut stdout).take(4097).read_to_end(&mut output).await?;
            let status = child.wait().await?;
            Ok::<_, io::Error>((status, output))
        })
        .await;
        let (status, output) = match outcome {
            Ok(Ok(value)) => value,
            _ => {
                let _ = child.kill().await;
                let _ = child.wait().await;
                panic!("diagnostic process incomplete");
            }
        };
        assert!(
            status.success() && output.len() <= 4096,
            "diagnostic process failed"
        );
        let result: AccountsDiagnostic = serde_json::from_slice(&output)
            .unwrap_or_else(|_| panic!("closed diagnostic output required"));
        assert!(
            result
                .http_status
                .is_none_or(|status| (100..=599).contains(&status)
                    && result.phase == AccountsDiagnosticPhase::Response),
            "invalid diagnostic status"
        );
        assert!(
            result
                .certificate_verify_code
                .is_none_or(|code| code <= 1024 && result.phase == AccountsDiagnosticPhase::Tls),
            "invalid certificate diagnostic code"
        );
        result
    }

    #[tokio::test]
    async fn accounts_diagnostic_certificate_codes_never_retain_private_exception_text() {
        for (code, expected) in [
            (20, AccountsResponseClass::CertificateIssuerUnavailable),
            (62, AccountsResponseClass::CertificateNameMismatch),
            (10, AccountsResponseClass::CertificateExpired),
        ] {
            let script = format!(
                "{ACCOUNTS_DIAGNOSTIC_CLIENT}\nerror=ssl.SSLCertVerificationError(1,'private-token-canary private-path-canary')\nerror.verify_code={code}\nemit_certificate_failure(error,'tls',None)"
            );
            let result =
                accounts_diagnostic_process(script, zeroize::Zeroizing::new(Vec::new()), &[]).await;
            assert_eq!(result.phase, AccountsDiagnosticPhase::Tls);
            assert_eq!(result.certificate_verify_code, Some(code));
            assert_eq!(result.response_class, expected);
            assert!(
                !serde_json::to_string(&result).unwrap().contains("canary"),
                "private certificate detail retained"
            );
        }
    }

    #[tokio::test]
    async fn accounts_diagnostic_local_responses_preserve_status_and_redact_private_data() {
        let cases = [
            (
                200,
                br#"{"accounts":[{"id":"private-account-canary"}]}"#.as_slice(),
                AccountsResponseClass::AccountsArray,
            ),
            (200, br#"{"accounts":{"private-key-canary":{"account":{"account_id":"private-account-canary"}}},"account_ordering":["private-key-canary"]}"#.as_slice(), AccountsResponseClass::AccountsMap),
            (200, br#"{"accounts":[{"id":"private-account-canary","plan_type":42}]}"#.as_slice(), AccountsResponseClass::InvalidShape),
            (
                200,
                b"private-token-canary private-path-canary".as_slice(),
                AccountsResponseClass::InvalidJson,
            ),
            (
                200,
                br#"{"accounts":false}"#.as_slice(),
                AccountsResponseClass::InvalidShape,
            ),
            (200, b"{}".as_slice(), AccountsResponseClass::InvalidShape),
            (
                401,
                b"private-token-canary".as_slice(),
                AccountsResponseClass::StatusError,
            ),
            (
                403,
                b"private-path-canary".as_slice(),
                AccountsResponseClass::StatusError,
            ),
        ];
        for (status, body, expected) in cases {
            let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
            let address = listener.local_addr().unwrap();
            let body = body.to_vec();
            let server = thread::spawn(move || {
                let (mut connection, _) = listener.accept().unwrap();
                connection
                    .set_read_timeout(Some(Duration::from_secs(10)))
                    .unwrap();
                let mut headers = Vec::new();
                while !headers.ends_with(b"\r\n\r\n") && headers.len() < 16384 {
                    let mut byte = [0];
                    connection.read_exact(&mut byte).unwrap();
                    headers.push(byte[0]);
                }
                write!(
                    connection,
                    "HTTP/1.1 {status} Control\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .unwrap();
                connection.write_all(&body).unwrap();
            });
            let script = format!(
                "{ACCOUNTS_DIAGNOSTIC_CLIENT}\nc=http.client.HTTPConnection('127.0.0.1', {}, timeout=10)\nc.request('GET','/')\nr=c.getresponse()\nemit(r.status,classify(r.status,r.read(LIMIT+1)))\nc.close()",
                address.port()
            );
            let result =
                accounts_diagnostic_process(script, zeroize::Zeroizing::new(Vec::new()), &[]).await;
            server.join().unwrap();
            assert_eq!(result.http_status, Some(status));
            assert_eq!(result.response_class, expected);
            let serialized = serde_json::to_string(&result).unwrap();
            assert!(
                !serialized.contains("canary"),
                "private diagnostic data retained"
            );
        }
        let result = accounts_diagnostic_process(
            format!("{ACCOUNTS_DIAGNOSTIC_CLIENT}\nemit(200,classify(200,b'x'*(LIMIT+1)))"),
            zeroize::Zeroizing::new(Vec::new()),
            &[],
        )
        .await;
        assert_eq!(result.response_class, AccountsResponseClass::BodyLimit);
    }

    #[test]
    fn accounts_diagnostic_output_rejects_private_fields_and_unknown_categories() {
        assert!(
            serde_json::from_value::<AccountsDiagnostic>(json!({
                "phase": "response", "httpStatus": 403, "responseClass": "status_error",
                "body": "private-token-canary"
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<AccountsDiagnostic>(json!({
                "phase": "response", "httpStatus": 200, "responseClass": "private-token-canary"
            }))
            .is_err()
        );
    }

    #[tokio::test]
    async fn accounts_diagnostic_malformed_process_output_never_reports_private_values() {
        for script in [
            "print('{\"phase\":\"response\",\"httpStatus\":200,\"responseClass\":\"private-token-canary\"}')",
            "print('{\"phase\":\"response\",\"httpStatus\":403,\"responseClass\":\"status_error\",\"private-token-canary\":true}')",
            "print('{\"phase\":\"private-token-canary\",\"httpStatus\":null,\"responseClass\":\"transport_failure\"}')",
        ] {
            let result = CatchUnwindFuture::new(accounts_diagnostic_process(
                script.to_owned(),
                zeroize::Zeroizing::new(Vec::new()),
                &[],
            ))
            .await;
            let payload = match result {
                Err(payload) => payload,
                Ok(_) => panic!("malformed diagnostic output accepted"),
            };
            assert!(
                payload.downcast_ref::<&str>() == Some(&"closed diagnostic output required"),
                "diagnostic error retained untrusted output"
            );
        }
    }

    #[tokio::test]
    async fn accounts_diagnostic_rejects_incomplete_certificate_bundles_without_network() {
        for bundle in [
            b"".as_slice(),
            b"-----BEGIN CERTIFICATE-----\nprivate-canary".as_slice(),
            b"-----BEGIN CERTIFICATE-----\ninvalid\n-----END CERTIFICATE-----".as_slice(),
        ] {
            let script = format!(
                "{ACCOUNTS_DIAGNOSTIC_CLIENT}\ntry:\n pem_context(sys.stdin.buffer.read(4194305))\n emit(None,'ca_ready','ca')\nexcept Exception:\n emit(None,'certificate_failure','ca')"
            );
            let result =
                accounts_diagnostic_process(script, zeroize::Zeroizing::new(bundle.to_vec()), &[])
                    .await;
            assert_eq!(result.phase, AccountsDiagnosticPhase::Ca);
            assert_eq!(
                result.response_class,
                AccountsResponseClass::CertificateFailure
            );
            assert!(result.http_status.is_none());
        }
    }

    #[tokio::test]
    #[ignore = "Requires an explicit pinned public CA file; offline context control performs no authentication or network request."]
    async fn accounts_diagnostic_loads_all_pinned_public_certificates_offline() {
        let file = PathBuf::from(
            std::env::var_os("MAGI_TEST_TLS_CA_FILE").expect("explicit public CA required"),
        );
        let bundle = fs::read(file).expect("public CA unavailable");
        assert!(
            format!("{:x}", Sha256::digest(&bundle))
                == "a41b5d356aea97a529fe27e0f7316d2f9d946d75927476cf9cf1b90637d00505",
            "public CA pin mismatch"
        );
        let script = format!(
            "{ACCOUNTS_DIAGNOSTIC_CLIENT}\npem_context(sys.stdin.buffer.read(4194305))\nemit(None,'ca_ready','ca')"
        );
        let result =
            accounts_diagnostic_process(script, zeroize::Zeroizing::new(bundle), &[]).await;
        assert_eq!(result.phase, AccountsDiagnosticPhase::Ca);
        assert_eq!(result.response_class, AccountsResponseClass::CaReady);
        assert!(result.http_status.is_none());
    }

    #[tokio::test]
    #[ignore = "Requires explicit selected subscription and public CA; one GET through a different HTTP client is diagnostic only, not ACP authentication or inference proof."]
    async fn production_proxy_accounts_check_diagnostic_without_inference() {
        use zeroize::Zeroize;
        let selected = PathBuf::from(
            std::env::var_os("MAGI_TEST_CREDENTIAL_HOME")
                .expect("explicit selected authority required"),
        );
        let ca_file = PathBuf::from(
            std::env::var_os("MAGI_TEST_TLS_CA_FILE").expect("explicit public CA required"),
        );
        let metadata = fs::symlink_metadata(&ca_file).expect("public CA unavailable");
        use std::os::unix::fs::MetadataExt;
        assert!(
            metadata.is_file()
                && !metadata.file_type().is_symlink()
                && metadata.nlink() == 1
                && metadata.mode() & 0o222 == 0,
            "public CA authority rejected"
        );
        let mut ca = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&ca_file)
            .expect("public CA authority rejected");
        let opened = ca.metadata().unwrap();
        assert!(
            opened.dev() == metadata.dev()
                && opened.ino() == metadata.ino()
                && opened.len() <= 4 * 1024 * 1024,
            "public CA authority changed"
        );
        let mut bytes = Vec::new();
        ca.read_to_end(&mut bytes).unwrap();
        assert!(
            format!("{:x}", Sha256::digest(&bytes))
                == "a41b5d356aea97a529fe27e0f7316d2f9d946d75927476cf9cf1b90637d00505",
            "public CA pin mismatch"
        );
        let broker = crate::subscription::SubscriptionBroker::open(
            crate::subscription::CredentialHome::inspect(&selected)
                .expect("selected authority rejected"),
        )
        .expect("selected authority rejected");
        let mut parameters = broker
            .connect_parameters()
            .expect("selected authority unavailable");
        let input = zeroize::Zeroizing::new(
            serde_json::to_vec(&parameters).expect("diagnostic input unavailable"),
        );
        for value in parameters.as_object_mut().unwrap().values_mut() {
            if let Value::String(value) = value {
                value.zeroize();
            }
        }
        drop(parameters);
        let proxy = ProviderNetworkProxy::start().unwrap();
        let token = zeroize::Zeroizing::new(
            proxy
                .proxy_url
                .strip_prefix("http://magi:")
                .and_then(|value| value.split_once('@'))
                .unwrap()
                .0
                .to_owned(),
        );
        let address = proxy.local_addr().to_string();
        let result = accounts_diagnostic_process(
            format!("{ACCOUNTS_DIAGNOSTIC_CLIENT}\ndiagnostic()"),
            input,
            &[
                ("PROXY_ADDRESS", std::ffi::OsStr::new(&address)),
                ("PROXY_PASSWORD", std::ffi::OsStr::new(token.as_str())),
                ("PUBLIC_CA_FILE", ca_file.as_os_str()),
            ],
        )
        .await;
        eprintln!(
            "different HTTP client accounts diagnostic: {}",
            serde_json::to_string(&result).unwrap()
        );
        assert_eq!(
            proxy.state.tunnels_opened.load(Ordering::Relaxed),
            1,
            "single diagnostic tunnel required"
        );
    }

    const TLS_CLIENT: &str = r#"
import base64, os, socket, ssl

def verify_tls(address, password, ca_file, hostname="chatgpt.com"):
    connection = None
    try:
        connection = socket.create_connection(address, timeout=10)
        authorization = base64.b64encode(("magi:" + password).encode())
        connection.sendall(b"CONNECT chatgpt.com:443 HTTP/1.1\r\nHost: chatgpt.com:443\r\nProxy-Authorization: Basic " + authorization + b"\r\n\r\n")
        response = b""
        while not response.endswith(b"\r\n\r\n") and len(response) < 16384:
            byte = connection.recv(1)
            if not byte:
                return 11
            response += byte
        if response.split(b"\r\n", 1)[0] != b"HTTP/1.1 200 Connection Established":
            return 11
        context = ssl.create_default_context(cafile=ca_file)
        context.check_hostname = True
        context.verify_mode = ssl.CERT_REQUIRED
        with context.wrap_socket(connection, server_hostname=hostname):
            connection = None
        return 0
    except ssl.SSLCertVerificationError:
        return 12
    except ssl.SSLError:
        return 13
    except (OSError, TimeoutError):
        return 10
    finally:
        if connection is not None:
            connection.close()
"#;

    async fn tls_process(
        script: String,
        variables: &[(&str, &std::ffi::OsStr)],
    ) -> (bool, Option<i32>) {
        let python =
            std::env::var_os("MAGI_TEST_TLS_PYTHON").expect("explicit TLS Python required");
        let mut command = tokio::process::Command::new(python);
        command
            .env_clear()
            .args(["-c", &script])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        for (name, value) in variables {
            command.env(name, value);
        }
        let mut child = command.spawn().expect("TLS control spawn");
        match timeout(Duration::from_secs(45), child.wait()).await {
            Ok(Ok(status)) => (true, status.code()),
            Ok(Err(_)) => (true, None),
            Err(_) => {
                let _ = child.kill().await;
                let _ = child.wait().await;
                (false, None)
            }
        }
    }

    #[tokio::test]
    #[ignore = "Requires explicit TLS tools and fresh fixture directory; verifies local CA, hostname mismatch and proxy capability denial without network egress."]
    async fn tls_control_client_validates_local_certificate_and_rejects_wrong_authority() {
        use std::os::unix::fs::PermissionsExt;
        let root = PathBuf::from(
            std::env::var_os("MAGI_TEST_TLS_FIXTURE_ROOT")
                .expect("explicit fresh TLS fixture root required"),
        );
        assert!(
            root.is_absolute() && !root.exists(),
            "fresh TLS fixture root required"
        );
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let key = root.join("private-key.pem");
        let certificate = root.join("certificate.pem");
        let openssl =
            std::env::var_os("MAGI_TEST_TLS_OPENSSL").expect("explicit certificate tool required");
        let status = std::process::Command::new(openssl)
            .env_clear()
            .args([
                "req",
                "-x509",
                "-newkey",
                "rsa:2048",
                "-noenc",
                "-days",
                "1",
                "-subj",
                "/CN=chatgpt.com",
                "-addext",
                "subjectAltName=DNS:chatgpt.com",
                "-keyout",
            ])
            .arg(&key)
            .arg("-out")
            .arg(&certificate)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "local certificate generation");
        fs::set_permissions(&key, fs::Permissions::from_mode(0o600)).unwrap();
        fs::set_permissions(&certificate, fs::Permissions::from_mode(0o644)).unwrap();
        let script = format!(
            "{TLS_CLIENT}\n{}",
            r#"
import threading

def local_case(hostname, password):
    listener = socket.socket()
    listener.bind(("127.0.0.1", 0))
    listener.listen(1)
    listener.settimeout(3)
    server_context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    server_context.load_cert_chain(os.environ["LOCAL_CERT"], os.environ["LOCAL_KEY"])
    failures = []
    def serve():
        try:
            with listener.accept()[0] as peer:
                peer.settimeout(3)
                header = b""
                while not header.endswith(b"\r\n\r\n") and len(header) < 16384:
                    chunk = peer.recv(1)
                    if not chunk: raise RuntimeError()
                    header += chunk
                expected = b"Proxy-Authorization: Basic " + base64.b64encode(b"magi:public-local-canary")
                if not header.startswith(b"CONNECT chatgpt.com:443 HTTP/1.1\r\n") or expected not in header:
                    peer.sendall(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n")
                    return
                peer.sendall(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                try:
                    with server_context.wrap_socket(peer, server_side=True) as tls:
                        tls.recv(1)
                except ssl.SSLError:
                    if hostname == "chatgpt.com": failures.append(True)
        except Exception:
            failures.append(True)
    worker = threading.Thread(target=serve, daemon=True)
    worker.start()
    result = verify_tls(listener.getsockname(), password, os.environ["LOCAL_CERT"], hostname)
    worker.join(4)
    listener.close()
    if worker.is_alive() or failures: return 20
    return result

results = [local_case("chatgpt.com", "public-local-canary"), local_case("wrong.example", "public-local-canary"), local_case("chatgpt.com", "wrong-capability")]
raise SystemExit(0 if results == [0, 12, 11] else 21)
"#
        );
        let outcome = tls_process(
            script,
            &[
                ("LOCAL_CERT", certificate.as_os_str()),
                ("LOCAL_KEY", key.as_os_str()),
            ],
        )
        .await;
        assert!(
            outcome == (true, Some(0)),
            "local TLS authority controls failed"
        );
    }

    #[tokio::test]
    #[ignore = "Requires explicit Python and public CA file; performs one certificate-verified TLS handshake through the production proxy."]
    async fn production_proxy_completes_certificate_verified_tls_without_subscription() {
        let ca_file =
            std::env::var_os("MAGI_TEST_TLS_CA_FILE").expect("explicit public CA file required");
        let proxy = ProviderNetworkProxy::start().unwrap();
        let token = zeroize::Zeroizing::new(
            proxy
                .proxy_url
                .strip_prefix("http://magi:")
                .and_then(|value| value.split_once('@'))
                .expect("proxy capability")
                .0
                .to_owned(),
        );
        let address = proxy.local_addr().to_string();
        let script = format!(
            "{TLS_CLIENT}\n{}",
            r#"
host, port = os.environ["PROXY_ADDRESS"].rsplit(":", 1)
raise SystemExit(verify_tls((host, int(port)), os.environ.pop("PROXY_PASSWORD"), os.environ["PUBLIC_CA_FILE"]))
"#
        );
        let (completed, code) = tls_process(
            script,
            &[
                ("PROXY_ADDRESS", std::ffi::OsStr::new(&address)),
                ("PROXY_PASSWORD", std::ffi::OsStr::new(token.as_str())),
                ("PUBLIC_CA_FILE", &ca_file),
            ],
        )
        .await;
        let verified = code == Some(0);
        let uploaded = proxy.state.uploaded_bytes.load(Ordering::Relaxed);
        let downloaded = proxy.state.downloaded_bytes.load(Ordering::Relaxed);
        let upload_timeouts = proxy.state.upload_timeout_errors.load(Ordering::Relaxed);
        let download_timeouts = proxy.state.download_timeout_errors.load(Ordering::Relaxed);
        let accepted = proxy.state.tunnels_opened.load(Ordering::Relaxed);
        eprintln!(
            "production proxy TLS control: completed={completed} exit_class={code:?} verified={verified} accepted={accepted} uploaded={uploaded} downloaded={downloaded} upload_timeouts={upload_timeouts} download_timeouts={download_timeouts}"
        );
        assert!(completed && verified, "certificate_verified_tls_failed");
        assert!(
            accepted == 1 && uploaded > 0 && downloaded > 0,
            "admitted_bidirectional_tls_required"
        );
        assert_eq!(proxy.state.chatgpt_tunnels.load(Ordering::Relaxed), 1);
    }

    fn socket_pair() -> (TcpStream, TcpStream) {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let accepted = listener.accept().unwrap().0;
        for stream in [&client, &accepted] {
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(2)))
                .unwrap();
        }
        (client, accepted)
    }

    #[test]
    fn production_proxy_waits_for_fragmented_connect_before_applying_policy() {
        let proxy = ProviderNetworkProxy::start().unwrap();
        let mut client = TcpStream::connect(proxy.local_addr()).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        client.write_all(b"CONNECT ").unwrap();
        thread::sleep(Duration::from_millis(50));
        let _ = client
            .write_all(b"attacker.example:443 HTTP/1.1\r\nHost: attacker.example:443\r\n\r\n");
        let mut response = String::new();
        client.read_to_string(&mut response).unwrap();
        assert!(
            response.starts_with("HTTP/1.1 403"),
            "fragmented complete request reaches destination policy"
        );
        assert_eq!(proxy.state.rejected_other.load(Ordering::Relaxed), 1);
        assert_eq!(proxy.state.tunnels_opened.load(Ordering::Relaxed), 0);
        assert_eq!(
            proxy
                .state
                .upstream_connect_failures
                .load(Ordering::Relaxed),
            0
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_native_accept_inherits_nonblocking_listener_mode() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let _client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        let accepted = loop {
            match listener.accept() {
                Ok((accepted, _)) => break accepted,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "owned listener did not become ready"
                    );
                    thread::sleep(Duration::from_millis(1));
                }
                Err(error) => panic!("owned listener accept failed: {}", error.kind()),
            }
        };
        let flags = unsafe { libc::fcntl(accepted.as_raw_fd(), libc::F_GETFL) };
        assert!(flags >= 0 && flags & libc::O_NONBLOCK != 0);
    }

    #[test]
    fn established_tunnel_preserves_upload_after_upstream_halfclose() {
        let proxy = ProviderNetworkProxy::start().unwrap();
        let state = proxy.state.clone();
        let (mut client, relay_client) = socket_pair();
        let (relay_upstream, mut upstream) = socket_pair();
        let forwarding =
            thread::spawn(move || forward_established_tunnel(relay_client, relay_upstream, &state));
        let response = vec![b'r'; 8193];
        upstream.write_all(&response).unwrap();
        upstream.shutdown(Shutdown::Write).unwrap();
        let mut received = Vec::new();
        client.read_to_end(&mut received).unwrap();
        assert_eq!(received, response);
        let upload = vec![b'u'; 16387];
        let write_result = client.write_all(&upload);
        let _ = client.shutdown(Shutdown::Write);
        let mut uploaded = Vec::new();
        let read_result = upstream.read_to_end(&mut uploaded);
        let forward_result = forwarding.join().unwrap();
        assert!(
            write_result.is_ok(),
            "upload remains writable after reverse EOF"
        );
        assert!(read_result.is_ok(), "upstream receives upload EOF");
        assert!(forward_result.is_ok());
        assert!(uploaded == upload, "remaining upload preserved");
        assert_eq!(
            proxy.state.downloaded_bytes.load(Ordering::Relaxed),
            response.len() as u64
        );
        assert_eq!(
            proxy.state.uploaded_bytes.load(Ordering::Relaxed),
            upload.len() as u64
        );
    }

    #[test]
    fn established_tunnel_preserves_response_after_client_halfclose() {
        let proxy = ProviderNetworkProxy::start().unwrap();
        let state = proxy.state.clone();
        let (mut client, relay_client) = socket_pair();
        let (relay_upstream, mut upstream) = socket_pair();
        let forwarding =
            thread::spawn(move || forward_established_tunnel(relay_client, relay_upstream, &state));
        let peer = thread::spawn(move || {
            let mut received = Vec::new();
            upstream.read_to_end(&mut received).unwrap();
            assert!(
                received == vec![b'u'; 131077],
                "complete upload before halfclose"
            );
            upstream.write_all(&vec![b'r'; 65539]).unwrap();
            upstream.shutdown(Shutdown::Write).unwrap();
        });
        client.write_all(&vec![b'u'; 131077]).unwrap();
        client.shutdown(Shutdown::Write).unwrap();
        let mut response = Vec::new();
        client.read_to_end(&mut response).unwrap();
        assert!(
            response == vec![b'r'; 65539],
            "complete reverse response after upload EOF"
        );
        peer.join().unwrap();
        assert!(forwarding.join().unwrap().is_ok());
        assert_eq!(proxy.state.uploaded_bytes.load(Ordering::Relaxed), 131077);
        assert_eq!(proxy.state.downloaded_bytes.load(Ordering::Relaxed), 65539);
    }

    #[test]
    fn established_tunnel_reports_directional_timeout_and_closes_both_directions() {
        let proxy = ProviderNetworkProxy::start().unwrap();
        let state = proxy.state.clone();
        let (mut client, relay_client) = socket_pair();
        let (relay_upstream, mut upstream) = socket_pair();
        relay_upstream
            .set_read_timeout(Some(Duration::from_millis(50)))
            .unwrap();
        let started = std::time::Instant::now();
        let result = forward_established_tunnel(relay_client, relay_upstream, &state);
        assert!(matches!(result, Err(ProviderError::ProxyUnavailable)));
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(state.download_timeout_errors.load(Ordering::Relaxed), 1);
        assert_eq!(state.upload_timeout_errors.load(Ordering::Relaxed), 0);
        let mut byte = [0];
        assert_eq!(client.read(&mut byte).unwrap(), 0);
        assert_eq!(upstream.read(&mut byte).unwrap(), 0);
        assert_eq!(state.uploaded_bytes.load(Ordering::Relaxed), 0);
        assert_eq!(state.downloaded_bytes.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn tunnel_observation_counts_partial_writes_without_error_payloads() {
        struct PartialWriter(bool);
        impl Write for PartialWriter {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                if self.0 {
                    Err(io::Error::new(
                        io::ErrorKind::ConnectionReset,
                        "private-canary",
                    ))
                } else {
                    self.0 = true;
                    Ok(bytes.len().min(3))
                }
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let counter = AtomicU64::new(0);
        let observation = copy_tunnel_bytes(
            &mut &b"opaque bytes"[..],
            &mut PartialWriter(false),
            &counter,
        );
        assert_eq!(counter.load(Ordering::Relaxed), 3);
        assert_eq!(observation, (3, Some(io::ErrorKind::ConnectionReset)));
        assert!(!format!("{observation:?}").contains("private-canary"));
        let mut destination = Vec::new();
        assert_eq!(
            copy_tunnel_bytes(&mut &b"opaque"[..], &mut destination, &counter),
            (6, None)
        );
    }

    #[test]
    fn diagnostic_retains_only_allowlisted_fields() {
        let secret = "canary-private-token";
        let private_path = "/canary/private/home/auth.json";
        let error = json!({"code": -32603, "message": format!("config/read failed {secret} {private_path}"), "data": {"details": secret, "path": private_path}});
        let diagnostic = rpc_failure_diagnostic("authentication/status", &error);
        assert_eq!(diagnostic.category, RpcFailureCategory::ConfigRead);
        assert_eq!(diagnostic.code, Some(-32603));
        let encoded = serde_json::to_string(&diagnostic).unwrap();
        let debug = format!("{diagnostic:?}");
        for output in [encoded, debug] {
            assert!(!output.contains(secret));
            assert!(!output.contains(private_path));
        }
        let unknown =
            rpc_failure_diagnostic(secret, &json!({"code": i64::MAX, "message": private_path}));
        assert_eq!(unknown.method, RpcFailureMethod::Other);
        assert_eq!(unknown.category, RpcFailureCategory::Unclassified);
        assert_eq!(unknown.code, None);
        let transport = rpc_failure_diagnostic(
            "authentication/status",
            &json!({"message": format!("workspace routing discovery failed: HTTP status 503; TLS handshake; {secret} {private_path}")}),
        );
        assert_eq!(transport.http_status, Some(503));
        assert_eq!(transport.network_category, RpcFailureCategory::TlsHandshake);
        let encoded = serde_json::to_string(&transport).unwrap();
        assert!(!encoded.contains(secret));
        assert!(!encoded.contains(private_path));
    }
    #[test]
    fn malformed_and_unauthorized_connect_never_open_tunnels() {
        let proxy = ProviderNetworkProxy::start().unwrap();
        for request in [
            "GET / HTTP/1.1\r\n\r\n",
            "CONNECT chatgpt.com:443 HTTP/1.1\r\nProxy-Authorization: Basic invalid\r\n\r\n",
            "CONNECT api.openai.com:443 HTTP/1.1\r\n\r\n",
        ] {
            let mut client = TcpStream::connect(proxy.local_addr()).unwrap();
            client
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            client.write_all(request.as_bytes()).unwrap();
            let mut response = [0u8; 256];
            let count = client.read(&mut response).unwrap();
            assert!(
                std::str::from_utf8(&response[..count])
                    .unwrap()
                    .starts_with("HTTP/1.1 403")
            );
        }
        assert_eq!(proxy.state.tunnels_opened.load(Ordering::Relaxed), 0);
        assert_eq!(
            proxy
                .state
                .upstream_connect_failures
                .load(Ordering::Relaxed),
            0
        );
        assert_eq!(proxy.state.rejected_api_openai.load(Ordering::Relaxed), 1);
        assert!(allowed_proxy_authority("api.openai.com:443").is_none());
        assert!(allowed_proxy_authority("chatgpt.com:443").is_some());
    }
}

#[cfg(test)]
mod session_discovery_tests {
    use super::*;

    fn payload() -> Value {
        json!({"sessionId":"actual-session","models":{"currentModelId":"actual-model[high]","availableModels":[{"modelId":"actual-model[high]","name":"Actual model","contextWindowTokens":12000,"maxOutputTokens":2000}]},"modes":{"currentModeId":"review","availableModes":[{"id":"review","name":"Review"}]},"configOptions":[{"id":"model","currentValue":"actual-model","options":[{"value":"actual-model","name":"Actual model"}]},{"id":"reasoning_effort","currentValue":"high","options":[{"value":"high","name":"High"}]}]})
    }

    fn prompt_state() -> Arc<PendingPrompt> {
        let (events, _) = mpsc::channel(1);
        let (_, terminal_outcome) = watch::channel(PromptTerminalOutcome::Pending);
        Arc::new(PendingPrompt {
            events,
            text: Mutex::new(String::new()),
            pending_text: Mutex::new(String::new()),
            delivery_wake: Notify::new(),
            delivery_finished: AtomicBool::new(false),
            queue_pressure: AtomicUsize::new(0),
            delivered_chunks: AtomicUsize::new(0),
            coalesced_updates: AtomicUsize::new(0),
            consumer_closed: AtomicBool::new(false),
            in_flight_bytes: AtomicUsize::new(0),
            diagnostic: PromptStreamDiagnostic(Arc::new(StdMutex::new(
                PromptStreamSnapshot::default(),
            ))),
            tool_denied: AtomicBool::new(false),
            output_limited: AtomicBool::new(false),
            event_limited: AtomicBool::new(false),
            event_count: AtomicUsize::new(0),
            terminal_outcome,
        })
    }

    fn ready_authority() -> SessionAuthority {
        let mut state = SessionAuthority::new(parse_session_info(&payload(), None).unwrap());
        state
            .begin_model("actual-session", "actual-model[high]")
            .unwrap();
        state
            .confirm_model("actual-session", "actual-model[high]")
            .unwrap();
        state.begin_mode("actual-session", "review").unwrap();
        state
            .confirm_mode("actual-session", "review", &json!({}))
            .unwrap();
        state
    }

    #[tokio::test]
    async fn prompt_reservation_atomically_excludes_model_and_mode_setters() {
        for mode in [false, true] {
            let authority = Arc::new(Mutex::new(Some(ready_authority())));
            let prompts = Arc::new(Mutex::new(HashMap::new()));
            let map_barrier = prompts.lock().await;
            let prompt_task = {
                let authority = authority.clone();
                let prompts = prompts.clone();
                tokio::spawn(async move {
                    reserve_prompt_authority(&authority, &prompts, "actual-session", prompt_state())
                        .await
                })
            };
            tokio::time::timeout(Duration::from_secs(1), async {
                while authority.try_lock().is_ok() {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            let setter = {
                let authority = authority.clone();
                let prompts = prompts.clone();
                tokio::spawn(async move {
                    begin_configuration_authority(
                        &authority,
                        &prompts,
                        "actual-session",
                        if mode { "review" } else { "actual-model[high]" },
                        mode,
                    )
                    .await
                })
            };
            drop(map_barrier);
            prompt_task.await.unwrap().unwrap();
            assert!(matches!(
                setter.await.unwrap(),
                Err(ProviderError::PromptInProgress)
            ));
            prompts.lock().await.clear();
            begin_configuration_authority(
                &authority,
                &prompts,
                "actual-session",
                if mode { "review" } else { "actual-model[high]" },
                mode,
            )
            .await
            .unwrap();
            assert!(
                reserve_prompt_authority(&authority, &prompts, "actual-session", prompt_state())
                    .await
                    .is_err()
            );
        }
    }

    #[tokio::test]
    async fn completion_is_not_observable_until_reservation_release_allows_next_operation() {
        for configure in [false, true] {
            let prompts = Arc::new(Mutex::new(HashMap::new()));
            let state = prompt_state();
            prompts
                .lock()
                .await
                .insert("actual-session".into(), state.clone());
            let map_barrier = prompts.lock().await;
            let (completion, mut completed) = oneshot::channel();
            let (terminal, mut terminal_state) = watch::channel(PromptTerminalOutcome::Pending);
            let (started, ready) = oneshot::channel();
            let task = {
                let prompts = prompts.clone();
                tokio::spawn(async move {
                    let mut reservation = PromptReservation {
                        prompts,
                        session_id: "actual-session".into(),
                        state,
                        inner: None,
                        armed: true,
                    };
                    let _ = started.send(());
                    publish_prompt_terminal(
                        &mut reservation,
                        completion,
                        terminal,
                        Err(ProviderError::Cancelled),
                        PromptTerminalOutcome::Error(ProviderError::Cancelled),
                    )
                    .await;
                })
            };
            ready.await.unwrap();
            tokio::task::yield_now().await;
            assert!(matches!(
                completed.try_recv(),
                Err(oneshot::error::TryRecvError::Empty)
            ));
            assert!(matches!(
                *terminal_state.borrow(),
                PromptTerminalOutcome::Pending
            ));
            drop(map_barrier);
            assert!(matches!(
                completed.await.unwrap(),
                Err(ProviderError::Cancelled)
            ));
            terminal_state.changed().await.unwrap();
            let authority = Mutex::new(Some(ready_authority()));
            if configure {
                begin_configuration_authority(
                    &authority,
                    &prompts,
                    "actual-session",
                    "actual-model[high]",
                    false,
                )
                .await
                .unwrap();
            } else {
                reserve_prompt_authority(&authority, &prompts, "actual-session", prompt_state())
                    .await
                    .unwrap();
            }
            task.await.unwrap();
        }
    }

    #[tokio::test]
    async fn aborted_prompt_reservation_cleans_up_without_erasing_replacement() {
        let prompts = Arc::new(Mutex::new(HashMap::new()));
        let state = prompt_state();
        prompts
            .lock()
            .await
            .insert("actual-session".into(), state.clone());
        let (started, ready) = oneshot::channel();
        let task = {
            let prompts = prompts.clone();
            let state = state.clone();
            tokio::spawn(async move {
                let _reservation = PromptReservation {
                    prompts,
                    session_id: "actual-session".into(),
                    state,
                    inner: None,
                    armed: true,
                };
                let _ = started.send(());
                std::future::pending::<()>().await;
            })
        };
        ready.await.unwrap();
        task.abort();
        let _ = task.await;
        tokio::time::timeout(Duration::from_secs(1), async {
            while !prompts.lock().await.is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let replacement = prompt_state();
        prompts
            .lock()
            .await
            .insert("actual-session".into(), replacement.clone());
        remove_prompt_reservation(&prompts, "actual-session", &state).await;
        assert!(Arc::ptr_eq(
            prompts.lock().await.get("actual-session").unwrap(),
            &replacement
        ));
    }

    #[test]
    fn prompts_require_both_requested_model_and_explicit_mode_acknowledgements() {
        let mut source = payload();
        source["models"]["availableModels"]
            .as_array_mut()
            .unwrap()
            .push(json!({"modelId":"other-model","name":"Other"}));
        let info = parse_session_info(&source, Some("other-model")).unwrap();
        assert_eq!(info.current_model_id, "actual-model[high]");
        assert!(info.selected_model_available);
        let mut state = SessionAuthority::new(info);
        assert!(state.confirmed_info("actual-session").is_err());
        state.begin_model("actual-session", "other-model").unwrap();
        assert!(state.confirmed_info("actual-session").is_err());
        for invalid in [
            json!({}),
            json!({"models":{"currentModelId":"wrong"}}),
            json!({"models":{"currentModelId":"other-model"},"configOptions":[{"id":"model","currentValue":"wrong"}]}),
        ] {
            assert!(validate_model_ack(&invalid, "other-model").is_err());
            assert!(state.confirmed_info("actual-session").is_err());
        }
        validate_model_ack(
            &json!({"models":{"currentModelId":"other-model"}}),
            "other-model",
        )
        .unwrap();
        state
            .confirm_model("actual-session", "other-model")
            .unwrap();
        assert!(state.confirmed_info("actual-session").is_err());
        state.begin_mode("actual-session", "review").unwrap();
        assert!(
            state
                .confirm_mode(
                    "actual-session",
                    "review",
                    &json!({"currentModeId":"wrong"})
                )
                .is_err()
        );
        assert!(state.confirmed_info("actual-session").is_err());
        state
            .confirm_mode("actual-session", "review", &json!({}))
            .unwrap();
        let confirmed = state.confirmed_info("actual-session").unwrap();
        assert_eq!(confirmed.current_model_id, "other-model");
        assert_eq!(confirmed.current_mode_id.as_deref(), Some("review"));
        assert!(state.confirmed_info("other-session").is_err());
    }

    #[test]
    fn attested_absence_of_modes_needs_no_invented_mode_acknowledgement() {
        let mut source = payload();
        source.as_object_mut().unwrap().remove("modes");
        let mut state = SessionAuthority::new(parse_session_info(&source, None).unwrap());
        state
            .begin_model("actual-session", "actual-model[high]")
            .unwrap();
        state
            .confirm_model("actual-session", "actual-model[high]")
            .unwrap();
        assert!(state.confirmed_info("actual-session").is_ok());
        assert!(state.begin_mode("actual-session", "invented").is_err());
    }

    #[test]
    fn provider_notices_are_typed_and_never_rewrite_genuine_model_text() {
        let notice = json!({"sessionUpdate":"notice","severity":"warning","title":"Falling back from WebSockets to HTTPS transport CANARY_SECRET","description":"UnknownIssuer /private/CANARY_PATH"});
        let SessionUpdatePayload::Notice(diagnostic) = session_update_payload(&notice).unwrap()
        else {
            panic!("notice channel expected")
        };
        assert_eq!(
            diagnostic.category,
            NoticeCategory::WebSocketCertificateFailure
        );
        let serialized = serde_json::to_string(&diagnostic).unwrap();
        assert!(!serialized.contains("CANARY"));
        assert!(!format!("{diagnostic:?}").contains("CANARY"));
        let text = "Warning: genuine model content CANARY_SECRET";
        let message =
            json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":text}});
        let SessionUpdatePayload::AgentText(actual) = session_update_payload(&message).unwrap()
        else {
            panic!("text channel expected")
        };
        assert_eq!(actual, text);
        assert!(
            parse_provider_notice(&json!({"severity":"info","title":"Notice","description":null}))
                .is_ok()
        );
        for invalid in [
            json!({"severity":"warning"}),
            json!({"severity":{},"title":"Notice"}),
            json!({"severity":"warning","title":"Notice","description":{}}),
            json!({"severity":"warning","title":"x".repeat(4097)}),
        ] {
            assert!(parse_provider_notice(&invalid).is_err());
        }
    }

    #[test]
    fn actual_session_negotiation_preserves_exact_model_and_mode() {
        let info = parse_session_info(&payload(), None).unwrap();
        assert_eq!(info.current_model_id, "actual-model[high]");
        assert_eq!(info.current_mode_id.as_deref(), Some("review"));
        assert_eq!(info.available_modes[0].mode_id, "review");
        assert_eq!(info.context_window_tokens, Some(12000));
        assert!(
            !parse_session_info(&payload(), Some("actual-model"))
                .unwrap()
                .selected_model_available
        );
        let mut absent = payload();
        absent.as_object_mut().unwrap().remove("modes");
        let info = parse_session_info(&absent, None).unwrap();
        assert!(info.current_mode_id.is_none());
        assert!(info.available_modes.is_empty());
    }

    #[test]
    fn malformed_present_modes_models_and_config_fail_closed() {
        for (key, invalid) in [
            ("modes", Value::Null),
            (
                "modes",
                json!({"currentModeId":"invented","availableModes":[{"id":"review","name":"Review"}]}),
            ),
            (
                "modes",
                json!({"currentModeId":"review","availableModes":[]}),
            ),
            (
                "models",
                json!({"currentModelId":"missing","availableModels":[{"modelId":"actual-model"}]}),
            ),
            (
                "models",
                json!({"currentModelId":"actual-model","availableModels":[{"modelId":"actual-model"},{"modelId":"actual-model"}]}),
            ),
            ("configOptions", json!({"id":"model"})),
            (
                "configOptions",
                json!([{"id":"model","currentValue":"invented","options":[{"value":"actual-model"}]}]),
            ),
            (
                "configOptions",
                json!([{"id":"model","currentValue":"actual-model","options":[]}]),
            ),
            (
                "configOptions",
                json!([{"id":"model","currentValue":"actual-model"}]),
            ),
        ] {
            let mut response = payload();
            response[key] = invalid;
            assert!(parse_session_info(&response, None).is_err());
        }
    }
}

#[cfg(test)]
mod bounded_stream_tests {
    use super::*;

    fn state(capacity: usize) -> (Arc<PendingPrompt>, mpsc::Receiver<PromptEvent>) {
        let (events, receiver) = mpsc::channel(capacity);
        let (_, terminal_outcome) = watch::channel(PromptTerminalOutcome::Pending);
        (
            Arc::new(PendingPrompt {
                events,
                text: Mutex::new(String::new()),
                pending_text: Mutex::new(String::new()),
                delivery_wake: Notify::new(),
                delivery_finished: AtomicBool::new(false),
                queue_pressure: AtomicUsize::new(0),
                delivered_chunks: AtomicUsize::new(0),
                coalesced_updates: AtomicUsize::new(0),
                consumer_closed: AtomicBool::new(false),
                in_flight_bytes: AtomicUsize::new(0),
                diagnostic: PromptStreamDiagnostic(Arc::new(StdMutex::new(
                    PromptStreamSnapshot::default(),
                ))),
                tool_denied: AtomicBool::new(false),
                output_limited: AtomicBool::new(false),
                event_limited: AtomicBool::new(false),
                event_count: AtomicUsize::new(0),
                terminal_outcome,
            }),
            receiver,
        )
    }

    #[tokio::test]
    async fn finish_alone_drains_burst_larger_than_queue_losslessly() {
        let (state, events) = state(PROMPT_EVENT_QUEUE);
        let worker_state = state.clone();
        let delivery = tokio::spawn(async move { deliver_prompt_text(worker_state, None).await });
        let expected = "界\\\n".repeat(600);
        for index in 0..600 {
            assert!(!append_prompt_text(&state, "界\\\n").await);
            if index < PROMPT_EVENT_QUEUE {
                timeout(Duration::from_secs(1), async {
                    while state.events.capacity() > PROMPT_EVENT_QUEUE - index - 1 {
                        tokio::task::yield_now().await;
                    }
                })
                .await
                .unwrap();
            }
        }
        assert_eq!(state.events.capacity(), 0);
        assert!(
            !state.pending_text.lock().await.is_empty()
                || state.in_flight_bytes.load(Ordering::Acquire) > 0
        );
        assert!(!state.event_limited.load(Ordering::Acquire));
        let (sender, completion) = oneshot::channel();
        let handle = PromptHandle {
            events,
            completion,
            stream: state.diagnostic.clone(),
        };
        let producer = tokio::spawn(async move {
            state.delivery_finished.store(true, Ordering::Release);
            state.delivery_wake.notify_one();
            delivery.await.unwrap().unwrap();
            sender
                .send(Ok(PromptResult {
                    final_text: state.text.lock().await.clone(),
                    stop_reason: "end_turn".into(),
                    usage: None,
                    buffered_events: vec![],
                }))
                .unwrap();
        });
        let result = timeout(Duration::from_secs(2), handle.finish())
            .await
            .unwrap()
            .unwrap();
        producer.await.unwrap();
        assert_eq!(result.final_text, expected);
        let delivered: String = result
            .buffered_events
            .iter()
            .filter_map(|event| match event {
                PromptEvent::TextDelta(text) => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(delivered, expected);
    }

    #[tokio::test]
    async fn escaped_unicode_frames_and_raw_ceilings_remain_bounded() {
        let (state, mut events) = state(128);
        let expected = "\u{1}界\\".repeat(40000);
        assert!(!append_prompt_text(&state, &expected).await);
        assert!(append_prompt_text(&state, &"x".repeat(MAX_RESPONSE_BYTES)).await);
        assert!(state.output_limited.load(Ordering::Acquire));
        assert_eq!(*state.text.lock().await, expected);
        state.delivery_finished.store(true, Ordering::Release);
        deliver_prompt_text(state.clone(), None).await.unwrap();
        let mut delivered = String::new();
        while let Ok(event) = events.try_recv() {
            assert!(serde_json::to_vec(&event).unwrap().len() <= MAX_RESPONSE_BYTES);
            if let PromptEvent::TextDelta(text) = event {
                delivered.push_str(&text);
            }
        }
        assert_eq!(delivered, expected);
        let (raw, _) = self::state(128);
        for _ in 0..MAX_PROMPT_EVENTS {
            assert!(!append_prompt_text(&raw, "x").await);
        }
        assert!(append_prompt_text(&raw, "y").await);
        assert!(raw.event_limited.load(Ordering::Acquire));
        assert_eq!(raw.text.lock().await.len(), MAX_PROMPT_EVENTS);
    }

    #[test]
    fn settlement_checkpoint_does_not_depend_on_available_blocking_workers() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .max_blocking_threads(1)
            .build()
            .unwrap();
        runtime.block_on(async {
            let (ready, waiting) = std::sync::mpsc::channel();
            let (release, held) = std::sync::mpsc::channel();
            let worker = tokio::task::spawn_blocking(move || {
                ready.send(()).unwrap();
                held.recv().unwrap();
            });
            waiting.recv_timeout(Duration::from_secs(1)).unwrap();
            let root = crate::VerificationRequest::until(
                std::time::Instant::now() + Duration::from_secs(1),
            );
            let started = std::time::Instant::now();
            root.wait_for_stream_checkpoint(started + Duration::from_millis(20))
                .await
                .unwrap();
            assert!(started.elapsed() < Duration::from_millis(250));
            assert!(!worker.is_finished());
            release.send(()).unwrap();
            worker.await.unwrap();
        });
    }

    #[test]
    fn stream_diagnostic_is_closed_numeric_metadata_without_private_fields() {
        let snapshot = PromptStreamSnapshot {
            queue_capacity: PROMPT_EVENT_QUEUE,
            failure: PromptStreamFailure::ConsumerClosed,
            ..Default::default()
        };
        let value = serde_json::to_value(&snapshot).unwrap();
        assert_eq!(value["failure"], "consumer_closed");
        for key in [
            "text",
            "token",
            "accountId",
            "responseBody",
            "privateCanary",
        ] {
            let mut altered = value.clone();
            altered[key] = json!("private-canary");
            assert!(serde_json::from_value::<PromptStreamSnapshot>(altered).is_err());
        }
        let mut altered = value.clone();
        altered["failure"] = json!("private-canary");
        assert!(serde_json::from_value::<PromptStreamSnapshot>(altered).is_err());
        let mut altered = value;
        altered.as_object_mut().unwrap().remove("rawEvents");
        assert!(serde_json::from_value::<PromptStreamSnapshot>(altered).is_err());
    }

    #[tokio::test]
    async fn exact_utf8_byte_budget_is_accepted_and_next_byte_rejected() {
        let (state, mut events) = self::state(PROMPT_EVENT_QUEUE);
        let exact = "x".repeat(MAX_RESPONSE_BYTES - 3) + "界";
        assert_eq!(exact.len(), MAX_RESPONSE_BYTES);
        assert!(!append_prompt_text(&state, &exact).await);
        assert!(append_prompt_text(&state, "x").await);
        assert_eq!(*state.text.lock().await, exact);
        assert_eq!(*state.pending_text.lock().await, exact);
        assert!(state.output_limited.load(Ordering::Acquire));
        assert!(!state.event_limited.load(Ordering::Acquire));
        state.delivery_finished.store(true, Ordering::Release);
        deliver_prompt_text(state.clone(), None).await.unwrap();
        let mut delivered = String::new();
        while let Ok(event) = events.try_recv() {
            assert!(serde_json::to_vec(&event).unwrap().len() <= MAX_RESPONSE_BYTES);
            if let PromptEvent::TextDelta(text) = event {
                delivered.push_str(&text);
            }
        }
        assert_eq!(delivered, exact);
    }

    #[tokio::test]
    async fn blocked_text_delivery_does_not_block_rpc_response_or_permission_dispatch() {
        let root =
            crate::VerificationRequest::until(std::time::Instant::now() + Duration::from_secs(3));
        let (state, _events) = self::state(PROMPT_EVENT_QUEUE);
        let mut command = tokio::process::Command::new("/bin/cat");
        command
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .process_group(0);
        let mut child = command.spawn().unwrap();
        let (_, exit_status) = watch::channel(None);
        let inner = Arc::new(Inner {
            request_deadline: Arc::new(ProviderPhaseClock::new(None)),
            effect_authority: Some(root.effect_authority()),
            writer: Mutex::new(child.stdin.take().unwrap()),
            pending: Mutex::new(HashMap::new()),
            prompts: Arc::new(Mutex::new(HashMap::new())),
            active_session_id: Mutex::new(Some("bounded-session".into())),
            source_reader: None,
            subscription: Mutex::new(None),
            subscription_authenticated: AtomicBool::new(false),
            rpc_failure: Mutex::new(None),
            provider_notices: Mutex::new(vec![]),
            event_gate: Mutex::new(()),
            next_request_id: AtomicU64::new(1),
            closed: AtomicBool::new(false),
            process: Arc::new(ProcessControl {
                process_group_id: child.id().unwrap() as i32,
                exit_status,
                observation: StdMutex::new(local_observation::LocalObservation::now()),
            }),
        });
        inner
            .prompts
            .lock()
            .await
            .insert("bounded-session".into(), state.clone());
        let delivery_state = state.clone();
        let authority = root.effect_authority();
        let lease = root.track_stream_operation().unwrap();
        let delivery = tokio::spawn(async move {
            let _lease = lease;
            deliver_prompt_text(delivery_state, Some(&authority)).await
        });
        for index in 0..PROMPT_EVENT_QUEUE {
            handle_message(&inner, json!({"method":"session/update","params":{"sessionId":"bounded-session",
                "update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"x"}}}})).await.unwrap();
            timeout(Duration::from_secs(1), async {
                while state.events.capacity() > PROMPT_EVENT_QUEUE - index - 1 {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
        }
        assert_eq!(state.events.capacity(), 0);
        let (response, result) = oneshot::channel();
        inner.pending.lock().await.insert(
            41,
            PendingRequest {
                method: "session/prompt".into(),
                sender: response,
            },
        );
        timeout(Duration::from_secs(1), async {
            for _ in 0..500 {
                handle_message(&inner, json!({"method":"session/update","params":{"sessionId":"bounded-session",
                    "update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"界"}}}})).await.unwrap();
            }
            handle_message(&inner, json!({"id":41,"result":{"stopReason":"end_turn"}})).await.unwrap();
            handle_message(&inner, json!({"id":42,"method":"session/request_permission","params":{"sessionId":"bounded-session"}})).await.unwrap();
        }).await.unwrap();
        assert_eq!(result.await.unwrap().unwrap()["stopReason"], "end_turn");
        assert!(state.tool_denied.load(Ordering::Acquire));
        assert_eq!(
            state.text.lock().await.as_str(),
            format!("{}{}", "x".repeat(128), "界".repeat(500))
        );
        root.revoke();
        assert!(matches!(
            delivery.await.unwrap(),
            Err(ProviderError::Cancelled)
        ));
        child.kill().await.unwrap();
        child.wait().await.unwrap();
        assert!(
            root.wait_for_settlement(std::time::Instant::now() + Duration::from_secs(1))
                .await
                .unwrap()
                .is_settled()
        );
    }

    #[tokio::test]
    async fn blocked_delivery_keeps_lease_until_revocation_and_reports_closed_consumer() {
        let root =
            crate::VerificationRequest::until(std::time::Instant::now() + Duration::from_secs(2));
        let (state, events) = state(1);
        state
            .events
            .send(PromptEvent::TextDelta("first".into()))
            .await
            .unwrap();
        assert!(!append_prompt_text(&state, "second").await);
        let lease = root.track_stream_operation().unwrap();
        let authority = root.effect_authority();
        let worker_state = state.clone();
        let worker = tokio::spawn(async move {
            let _lease = lease;
            deliver_prompt_text(worker_state, Some(&authority)).await
        });
        tokio::task::yield_now().await;
        assert_eq!(root.settlement().provider_operations, 1);
        let (control, progress) = oneshot::channel();
        control.send(()).unwrap();
        timeout(Duration::from_millis(100), progress)
            .await
            .unwrap()
            .unwrap();
        root.revoke();
        assert!(matches!(
            timeout(Duration::from_secs(1), worker)
                .await
                .unwrap()
                .unwrap(),
            Err(ProviderError::Cancelled)
        ));
        assert!(
            root.wait_for_settlement(std::time::Instant::now() + Duration::from_secs(1))
                .await
                .unwrap()
                .is_settled()
        );
        drop(events);
        let (closed, receiver) = self::state(1);
        drop(receiver);
        assert!(!append_prompt_text(&closed, "accepted").await);
        assert!(matches!(
            deliver_prompt_text(closed.clone(), None).await,
            Err(ProviderError::StreamConsumerClosed)
        ));
        assert!(!closed.event_limited.load(Ordering::Acquire));
        assert!(closed.consumer_closed.load(Ordering::Acquire));
    }
}

#[cfg(test)]
mod public_wire_budget_tests {
    use super::*;
    #[test]
    fn public_text_count_is_not_a_byte_limit_and_unknown_models_are_rejected() {
        let text = "a ".repeat(6 * 1024);
        assert!(text.len() > 8192);
        assert!(CodexAcpClient::public_text_token_count("gpt-5.5[high]", &text).unwrap() < 8192);
        assert!(CodexAcpClient::public_text_token_count("gpt-6-astra", &text).is_err());
        assert_eq!(
            CodexAcpClient::public_text_token_count("unknown-model", "safe").unwrap(),
            4
        );
        assert!(CodexAcpClient::public_text_token_count("gpt-5.5[latest]", &text).is_err());
    }
    #[test]
    fn wire_blocks_deny_remote_images_and_unknown_fields() {
        let text = json!({"type":"text","text":"public fixture"});
        assert!(CodexAcpClient::validate_prompt_blocks(std::slice::from_ref(&text)).is_ok());
        assert!(
            CodexAcpClient::validate_prompt_blocks(&[
                text.clone(),
                json!({"type":"image","uri":"https://example.com/image.png","mimeType":"image/png"})
            ])
            .is_err()
        );
        assert!(
            CodexAcpClient::validate_prompt_blocks(&[
                json!({"type":"text","text":"fixture","private":"extra"})
            ])
            .is_err()
        );
        assert!(
            CodexAcpClient::validate_prompt_blocks(&[
                text,
                json!({"type":"image","data":"YQ==","mimeType":"image/png"})
            ])
            .is_err()
        );
    }
}

#[cfg(test)]
mod local_process_freshness_tests {
    use super::*;
    #[tokio::test]
    async fn stale_owned_process_cannot_be_refreshed_by_concurrent_observers() {
        let mut child = tokio::process::Command::new("/bin/sleep")
            .arg("30")
            .process_group(0)
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let (_sender, exit_status) = watch::channel(None);
        let process = Arc::new(ProcessControl {
            process_group_id: child.id().unwrap() as i32,
            exit_status,
            observation: StdMutex::new(local_observation::LocalObservation::now()),
        });
        assert!(process.observe_owned_process());
        *process.observation.lock().unwrap() =
            local_observation::LocalObservation::aged_for_test(Duration::from_secs(16));
        let first = process.clone();
        let second = process.clone();
        let (a, b) = tokio::join!(
            tokio::task::spawn_blocking(move || first.observe_owned_process()),
            tokio::task::spawn_blocking(move || second.observe_owned_process())
        );
        assert!(!a.unwrap());
        assert!(!b.unwrap());
        assert!(!process.observation_is_fresh());
        process.signal_group(libc::SIGKILL);
        child.wait().await.unwrap();
    }
}

#[cfg(test)]
mod fixed_provider_deadline_tests {
    use super::*;
    #[tokio::test]
    async fn authenticated_phase_extends_once_without_old_watchdog_killing_new_phase() {
        let startup = std::time::Instant::now() + Duration::from_millis(100);
        let clock = ProviderPhaseClock::new(Some(startup));
        let closed = AtomicBool::new(false);
        let old = clock.snapshot().unwrap();
        let old_rpc_deadline = old.deadline;
        let mut updates = clock.updates.subscribe();
        let model = std::time::Instant::now() + Duration::from_millis(600);
        clock.transition(model, &closed).unwrap();
        updates.changed().await.unwrap();
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(!clock.close_expired_snapshot(old, &closed));
        assert!(!closed.load(Ordering::Acquire));
        assert!(old_rpc_deadline.unwrap() < std::time::Instant::now());
        assert!(
            clock
                .deadline()
                .unwrap()
                .saturating_duration_since(std::time::Instant::now())
                > Duration::from_millis(300)
        );
        assert!(
            clock
                .transition(std::time::Instant::now() + Duration::from_secs(1), &closed)
                .is_err()
        );
        let model_snapshot = clock.snapshot().unwrap();
        tokio::time::sleep_until(tokio::time::Instant::from_std(model)).await;
        assert!(clock.close_expired_snapshot(model_snapshot, &closed));
        assert!(
            clock
                .transition(std::time::Instant::now() + Duration::from_secs(1), &closed)
                .is_err()
        );
        let expired =
            ProviderPhaseClock::new(Some(std::time::Instant::now() - Duration::from_millis(1)));
        assert!(expired.transition(model, &AtomicBool::new(false)).is_err());
    }
    #[tokio::test]
    async fn local_stop_proof_requires_positive_child_reap_and_finished_proxy_workers() {
        let request =
            crate::VerificationRequest::until(std::time::Instant::now() + Duration::from_secs(5));
        let proxy = ProviderNetworkProxy::start_tracked(Some(request.provider_activity().unwrap()))
            .unwrap();
        let mut child = tokio::process::Command::new("/bin/sleep")
            .arg("30")
            .process_group(0)
            .spawn()
            .unwrap();
        let (sender, status) = watch::channel(None);
        let process = ProcessControl {
            process_group_id: child.id().unwrap() as i32,
            exit_status: status,
            observation: StdMutex::new(local_observation::LocalObservation::now()),
        };
        assert!(!owned_process_exit_confirmed(&process));
        process.signal_group(libc::SIGKILL);
        let exit = child.wait().await.unwrap();
        sender.send_replace(Some(exit.code().unwrap_or(-1)));
        assert!(owned_process_exit_confirmed(&process));
        assert!(
            proxy
                .stop_and_observe(std::time::Instant::now() + Duration::from_secs(1))
                .await
        );
        assert_eq!(request.settlement().provider_operations, 0);
        assert!(request.check().is_ok());
        let (_, missing_status) = watch::channel(None);
        let missing = ProcessControl {
            process_group_id: process.process_group_id,
            exit_status: missing_status,
            observation: StdMutex::new(local_observation::LocalObservation::now()),
        };
        assert!(!owned_process_exit_confirmed(&missing));
    }
    #[tokio::test]
    async fn blocked_writer_deadline_prevents_late_bytes_without_revoking_shared_root() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let request =
            crate::VerificationRequest::until(std::time::Instant::now() + Duration::from_secs(3));
        let authority = request.effect_authority();
        let closed = AtomicBool::new(false);
        let (mut writer, mut reader) = tokio::io::duplex(1);
        writer.write_all(b"x").await.unwrap();
        let deadline = std::time::Instant::now() + Duration::from_millis(100);
        assert!(
            tokio::time::timeout(
                Duration::from_secs(1),
                write_effect_frame(
                    &mut writer,
                    b"late",
                    Some(&authority),
                    Some((deadline, &closed))
                )
            )
            .await
            .unwrap()
            .is_err()
        );
        let mut byte = [0];
        reader.read_exact(&mut byte).await.unwrap();
        assert_eq!(&byte, b"x");
        assert!(
            write_effect_frame(
                &mut writer,
                b"late",
                Some(&authority),
                Some((deadline, &closed))
            )
            .await
            .is_err()
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(20), reader.read(&mut byte))
                .await
                .is_err()
        );
        assert!(request.check().is_ok());
        let lock = Mutex::new(Vec::<u8>::new());
        let _held = lock.lock().await;
        assert!(
            lock_effect_writer(
                &lock,
                Some(&authority),
                ProviderRemediation::ReviewRequest,
                Some(deadline)
            )
            .await
            .is_err()
        );
    }
    #[tokio::test]
    async fn fixed_deadline_stops_real_unanswered_rpc_and_reaps_owned_process() {
        let request =
            crate::VerificationRequest::until(std::time::Instant::now() + Duration::from_secs(3));
        let mut child = tokio::process::Command::new("/bin/cat")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .process_group(0)
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let pid = child.id().unwrap() as i32;
        let (exit_sender, exit_status) = watch::channel(None);
        let deadline = std::time::Instant::now() + Duration::from_millis(200);
        let inner = Arc::new(Inner {
            request_deadline: Arc::new(ProviderPhaseClock::new(Some(deadline))),
            effect_authority: Some(request.effect_authority()),
            writer: Mutex::new(child.stdin.take().unwrap()),
            pending: Mutex::new(HashMap::new()),
            prompts: Arc::new(Mutex::new(HashMap::new())),
            active_session_id: Mutex::new(None),
            source_reader: None,
            subscription: Mutex::new(None),
            subscription_authenticated: AtomicBool::new(false),
            rpc_failure: Mutex::new(None),
            provider_notices: Mutex::new(vec![]),
            event_gate: Mutex::new(()),
            next_request_id: AtomicU64::new(1),
            closed: AtomicBool::new(false),
            process: Arc::new(ProcessControl {
                process_group_id: pid,
                exit_status,
                observation: StdMutex::new(local_observation::LocalObservation::now()),
            }),
        });
        let supervised = tokio::spawn(supervise_child(inner.clone(), child, exit_sender, None));
        let rpc = inner
            .begin_call("fixture/unanswered", json!({"fixture":true}))
            .await
            .unwrap();
        let result = tokio::time::timeout(Duration::from_secs(2), rpc.wait(&inner))
            .await
            .expect("fixed request deadline must beat the generic thirty-minute timeout");
        assert!(matches!(result, Err(ProviderError::RpcTimeout { .. })));
        tokio::time::timeout(Duration::from_secs(1), supervised)
            .await
            .unwrap()
            .unwrap();
        assert!(inner.closed.load(Ordering::Acquire));
        assert!(inner.process.exit_status.borrow().is_some());
        assert_ne!(unsafe { libc::kill(pid, 0) }, 0);
        assert!(
            request.check().is_ok(),
            "local timeout must not revoke the shared root needed to persist unknown failure"
        );
        assert!(
            inner
                .write_message(
                    json!({"jsonrpc":"2.0","method":"fixture/late","params":{}}),
                    ProviderRemediation::ReviewRequest,
                    FramePurpose::Effect
                )
                .await
                .is_err()
        );
    }
}

#[cfg(test)]
mod disclosure_publication_tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    fn epoch_ms() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
    }

    #[tokio::test]
    async fn database_wait_crossing_expiry_publishes_zero_bytes() {
        let request =
            crate::VerificationRequest::until(std::time::Instant::now() + Duration::from_secs(2));
        let authority = request.effect_authority();
        let authorization = crate::verification::PromptPublicationAuthorization::new(
            epoch_ms() + 30,
            Arc::new(|| {
                std::thread::sleep(Duration::from_millis(60));
                Ok(())
            }),
        )
        .unwrap();
        let (mut writer, mut reader) = tokio::io::duplex(8);
        assert!(
            write_authorized_effect_frame(
                &mut writer,
                b"secret",
                Some(&authority),
                None,
                Some(&authorization)
            )
            .await
            .is_err()
        );
        drop(writer);
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await.unwrap();
        assert!(bytes.is_empty());
    }

    #[tokio::test]
    async fn partial_write_rechecks_exact_reservation_without_late_bytes() {
        let request =
            crate::VerificationRequest::until(std::time::Instant::now() + Duration::from_secs(2));
        let authority = request.effect_authority();
        let count = Arc::new(AtomicUsize::new(0));
        let polls = count.clone();
        let authorization = crate::verification::PromptPublicationAuthorization::new(
            epoch_ms() + 1000,
            Arc::new(move || {
                if polls.fetch_add(1, Ordering::SeqCst) == 0 {
                    Ok(())
                } else {
                    Err(ProviderError::Cancelled)
                }
            }),
        )
        .unwrap();
        let (mut writer, mut reader) = tokio::io::duplex(1);
        assert!(
            write_authorized_effect_frame(
                &mut writer,
                b"ab",
                Some(&authority),
                None,
                Some(&authorization)
            )
            .await
            .is_err()
        );
        drop(writer);
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await.unwrap();
        assert_eq!(bytes, b"a");
        assert_eq!(count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn flush_rechecks_grant_and_never_claims_complete_after_revoke() {
        let request =
            crate::VerificationRequest::until(std::time::Instant::now() + Duration::from_secs(2));
        let authority = request.effect_authority();
        let calls = Arc::new(AtomicUsize::new(0));
        let observed = calls.clone();
        let authorization = crate::verification::PromptPublicationAuthorization::new(
            epoch_ms() + 1000,
            Arc::new(move || {
                if observed.fetch_add(1, Ordering::SeqCst) == 0 {
                    Ok(())
                } else {
                    Err(ProviderError::Cancelled)
                }
            }),
        )
        .unwrap();
        let (mut writer, mut reader) = tokio::io::duplex(8);
        assert!(
            write_authorized_effect_frame(
                &mut writer,
                b"frame",
                Some(&authority),
                None,
                Some(&authorization)
            )
            .await
            .is_err()
        );
        drop(writer);
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await.unwrap();
        assert_eq!(bytes, b"frame");
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }
}
