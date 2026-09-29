use crate::sandbox::{self, PreparedLaunch};
use crate::{CODEX_ACP_VERSION, ProviderError};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::process::ExitStatus;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::{Mutex, mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::timeout;

const MAX_PROTOCOL_FRAME: usize = 16 * 1024 * 1024;
const MAX_PROMPT_BYTES: usize = 10 * 1024 * 1024;
const MAX_RESPONSE_BYTES: usize = 256 * 1024;
const MAX_PROMPT_EVENTS: usize = 4096;
const PROMPT_EVENT_QUEUE: usize = 128;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const CANCEL_GRACE: Duration = Duration::from_secs(2);

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
pub struct SessionInfo {
    pub session_id: String,
    pub current_model_id: String,
    pub available_models: Vec<AvailableModel>,
    pub context_window_tokens: Option<u64>,
    pub max_output_tokens: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "text", rename_all = "snake_case")]
pub enum PromptEvent {
    TextDelta(String),
    SecurityViolation,
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
}

pub struct PromptHandle {
    pub events: mpsc::Receiver<PromptEvent>,
    completion: oneshot::Receiver<Result<PromptResult, ProviderError>>,
}

impl PromptHandle {
    pub async fn finish(self) -> Result<PromptResult, ProviderError> {
        self.completion
            .await
            .map_err(|_| ProviderError::ProcessClosed)?
    }
}

struct PendingPrompt {
    events: mpsc::Sender<PromptEvent>,
    text: Mutex<String>,
    tool_denied: AtomicBool,
    output_limited: AtomicBool,
    event_limited: AtomicBool,
    event_count: AtomicUsize,
    finished: watch::Receiver<bool>,
}

struct Inner {
    writer: Mutex<ChildStdin>,
    pending: Mutex<HashMap<u64, oneshot::Sender<Result<Value, ProviderError>>>>,
    prompts: Mutex<HashMap<String, Arc<PendingPrompt>>>,
    next_request_id: AtomicU64,
    closed: AtomicBool,
    process: Arc<ProcessControl>,
}

struct ProcessControl {
    process_group_id: i32,
    exit_status: watch::Receiver<Option<i32>>,
}

impl ProcessControl {
    fn signal_group(&self, signal: i32) {
        unsafe {
            libc::kill(-self.process_group_id, signal);
        }
    }

    async fn wait_for_exit(&self, wait: Duration) -> bool {
        let mut status = self.exit_status.clone();
        if status.borrow().is_some() {
            return true;
        }
        matches!(timeout(wait, status.changed()).await, Ok(Ok(()))) && status.borrow().is_some()
    }
}

pub struct CodexAcpClient {
    inner: Arc<Inner>,
    prepared: PreparedLaunch,
    auth_methods: Mutex<Vec<AuthMethod>>,
    session_info: Mutex<Option<SessionInfo>>,
    initializing: AtomicBool,
    initialized: AtomicBool,
    session_opening: AtomicBool,
    reader_task: Mutex<Option<JoinHandle<()>>>,
    supervisor_task: Mutex<Option<JoinHandle<()>>>,
}

impl CodexAcpClient {
    pub async fn spawn(launch: crate::CodexAcpLaunch) -> Result<Self, ProviderError> {
        let prepared = sandbox::prepare(&launch)?;
        let mut command = sandbox::isolated_command(&prepared)?;
        let mut child = command.spawn().map_err(|_| ProviderError::ProcessStart)?;
        let process_group_id = child.id().ok_or(ProviderError::ProcessStart)? as i32;
        let stdin = child.stdin.take().ok_or(ProviderError::ProcessStart)?;
        let stdout = child.stdout.take().ok_or(ProviderError::ProcessStart)?;
        let (exit_sender, exit_status) = watch::channel(None);
        let process = Arc::new(ProcessControl {
            process_group_id,
            exit_status,
        });
        let inner = Arc::new(Inner {
            writer: Mutex::new(stdin),
            pending: Mutex::new(HashMap::new()),
            prompts: Mutex::new(HashMap::new()),
            next_request_id: AtomicU64::new(1),
            closed: AtomicBool::new(false),
            process: process.clone(),
        });

        let reader_inner = inner.clone();
        let reader_task = tokio::spawn(async move {
            if read_protocol(reader_inner.clone(), stdout).await.is_err() {
                close_transport(&reader_inner, ProviderError::Protocol).await;
                reader_inner.process.signal_group(libc::SIGTERM);
            }
        });

        let supervisor_inner = inner.clone();
        let supervisor_task = tokio::spawn(async move {
            supervise_child(supervisor_inner, child, exit_sender).await;
        });

        Ok(Self {
            inner,
            prepared,
            auth_methods: Mutex::new(Vec::new()),
            session_info: Mutex::new(None),
            initializing: AtomicBool::new(false),
            initialized: AtomicBool::new(false),
            session_opening: AtomicBool::new(false),
            reader_task: Mutex::new(Some(reader_task)),
            supervisor_task: Mutex::new(Some(supervisor_task)),
        })
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
                    "clientCapabilities": {}
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
            home_binding: HomeBindingProof {
                explicit_codex_home_environment: true,
                operating_system_sandbox_applied: true,
                adapter_reported_home: false,
            },
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

    pub async fn authenticate(
        &self,
        method: AuthMethod,
    ) -> Result<AuthenticationStatus, ProviderError> {
        self.ensure_initialized()?;
        if !self.auth_methods.lock().await.contains(&method) {
            return Err(ProviderError::AuthenticationUnavailable);
        }
        self.inner
            .call("authenticate", json!({ "methodId": method.acp_id() }))
            .await?;
        match self.authentication_status().await? {
            AuthenticationStatus::Authenticated { method: actual } if actual == method => {
                Ok(AuthenticationStatus::Authenticated { method: actual })
            }
            AuthenticationStatus::Unauthenticated => Err(ProviderError::Unauthenticated),
            AuthenticationStatus::Authenticated { .. } | AuthenticationStatus::Unsupported => {
                Err(ProviderError::AuthenticationUnavailable)
            }
        }
    }

    pub async fn new_session(
        &self,
        model_id: Option<String>,
    ) -> Result<SessionInfo, ProviderError> {
        self.ensure_initialized()?;
        if self.session_info.lock().await.is_some()
            || self
                .session_opening
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        {
            return Err(ProviderError::SessionAlreadyCreated);
        }
        let _opening = FlagReset(&self.session_opening);
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
            .ok_or(ProviderError::RoleWorkdirUnavailable)?;
        let response = self
            .inner
            .call(
                "session/new",
                json!({
                    "cwd": cwd,
                    "mcpServers": []
                }),
            )
            .await?;
        let session_id = response
            .get("sessionId")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or(ProviderError::InvalidResponse)?
            .to_owned();
        let model_state = response
            .get("models")
            .ok_or(ProviderError::InvalidResponse)?;
        let current_model_id = model_state
            .get("currentModelId")
            .and_then(Value::as_str)
            .ok_or(ProviderError::InvalidResponse)?
            .to_owned();
        let available_models = parse_available_models(model_state)?;
        if !available_models
            .iter()
            .any(|model| model.model_id == current_model_id)
        {
            return Err(ProviderError::InvalidResponse);
        }

        let current_model_id = if let Some(selected) = model_id {
            if !available_models
                .iter()
                .any(|model| model.model_id == selected)
            {
                return Err(ProviderError::ModelUnavailable);
            }
            set_model(&self.inner, &session_id, &selected).await?;
            selected
        } else {
            current_model_id
        };
        let selected_model = available_models
            .iter()
            .find(|model| model.model_id == current_model_id)
            .ok_or(ProviderError::InvalidResponse)?;
        let context_window_tokens = selected_model.context_window_tokens;
        let max_output_tokens = selected_model.max_output_tokens;
        let info = SessionInfo {
            session_id,
            current_model_id,
            available_models,
            context_window_tokens,
            max_output_tokens,
        };
        *self.session_info.lock().await = Some(info.clone());
        Ok(info)
    }

    pub async fn prompt(
        &self,
        session_id: &str,
        text: String,
    ) -> Result<PromptHandle, ProviderError> {
        self.ensure_initialized()?;
        if text.len() > MAX_PROMPT_BYTES {
            return Err(ProviderError::InputLimit);
        }
        let session = self
            .session_info
            .lock()
            .await
            .clone()
            .ok_or(ProviderError::SessionUnavailable)?;
        if session.session_id != session_id {
            return Err(ProviderError::SessionUnavailable);
        }

        let (event_sender, events) = mpsc::channel(PROMPT_EVENT_QUEUE);
        let (finished_sender, finished) = watch::channel(false);
        let state = Arc::new(PendingPrompt {
            events: event_sender,
            text: Mutex::new(String::new()),
            tool_denied: AtomicBool::new(false),
            output_limited: AtomicBool::new(false),
            event_limited: AtomicBool::new(false),
            event_count: AtomicUsize::new(0),
            finished,
        });
        {
            let mut prompts = self.inner.prompts.lock().await;
            if prompts.contains_key(session_id) {
                return Err(ProviderError::PromptInProgress);
            }
            prompts.insert(session_id.to_owned(), state.clone());
        }

        let inner = self.inner.clone();
        let request_session_id = session_id.to_owned();
        let prompt_text = text;
        let (completion_sender, completion) = oneshot::channel();
        tokio::spawn(async move {
            let response = inner
                .call(
                    "session/prompt",
                    json!({
                        "sessionId": request_session_id,
                        "prompt": [{ "type": "text", "text": prompt_text }]
                    }),
                )
                .await;
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
                Ok(response) => {
                    let stop_reason = response
                        .get("stopReason")
                        .and_then(Value::as_str)
                        .ok_or(ProviderError::InvalidResponse);
                    match stop_reason {
                        Err(error) => Err(error),
                        Ok(stop_reason) if stop_reason == "cancelled" => {
                            Err(ProviderError::Cancelled)
                        }
                        Ok(stop_reason) => {
                            let text = state.text.lock().await.clone();
                            if text.len() > MAX_RESPONSE_BYTES {
                                Err(ProviderError::OutputLimit)
                            } else {
                                Ok(PromptResult {
                                    final_text: text,
                                    stop_reason: stop_reason.to_owned(),
                                    usage: parse_usage(response.get("usage")),
                                })
                            }
                        }
                    }
                }
            };
            let _ = completion_sender.send(result);
            let _ = finished_sender.send(true);
            inner.prompts.lock().await.remove(&request_session_id);
        });

        Ok(PromptHandle { events, completion })
    }

    pub async fn cancel(&self, session_id: &str) -> Result<(), ProviderError> {
        self.ensure_initialized()?;
        let state = self.inner.prompts.lock().await.get(session_id).cloned();
        let Some(state) = state else {
            return Ok(());
        };
        self.inner
            .notify("session/cancel", json!({ "sessionId": session_id }))
            .await?;
        let mut finished = state.finished.clone();
        if *finished.borrow() || timeout(CANCEL_GRACE, finished.changed()).await.is_ok() {
            return Ok(());
        }
        self.terminate().await;
        Ok(())
    }

    pub async fn shutdown(&self) {
        self.terminate().await;
        if let Some(task) = self.reader_task.lock().await.take() {
            let _ = timeout(CANCEL_GRACE, task).await;
        }
        if let Some(task) = self.supervisor_task.lock().await.take() {
            let _ = timeout(CANCEL_GRACE, task).await;
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
        self.inner.closed.store(true, Ordering::Release);
        self.inner.process.signal_group(libc::SIGTERM);
        if !self.inner.process.wait_for_exit(CANCEL_GRACE).await {
            self.inner.process.signal_group(libc::SIGKILL);
            let _ = self.inner.process.wait_for_exit(CANCEL_GRACE).await;
        }
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
    async fn call(&self, method: &str, params: Value) -> Result<Value, ProviderError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(ProviderError::ProcessClosed);
        }
        let id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = oneshot::channel();
        self.pending.lock().await.insert(id, sender);
        if self
            .write_message(
                json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }),
            )
            .await
            .is_err()
        {
            self.pending.lock().await.remove(&id);
            return Err(ProviderError::ProcessClosed);
        }
        match timeout(REQUEST_TIMEOUT, receiver).await {
            Err(_) => {
                self.pending.lock().await.remove(&id);
                Err(ProviderError::Timeout)
            }
            Ok(Err(_)) => Err(ProviderError::ProcessExited),
            Ok(Ok(result)) => result,
        }
    }

    async fn notify(&self, method: &str, params: Value) -> Result<(), ProviderError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(ProviderError::ProcessClosed);
        }
        self.write_message(json!({ "jsonrpc": "2.0", "method": method, "params": params }))
            .await
    }

    async fn write_message(&self, message: Value) -> Result<(), ProviderError> {
        let mut bytes = serde_json::to_vec(&message).map_err(|_| ProviderError::Protocol)?;
        if bytes.len() > MAX_PROTOCOL_FRAME {
            return Err(ProviderError::OutputLimit);
        }
        bytes.push(b'\n');
        let mut writer = self.writer.lock().await;
        writer
            .write_all(&bytes)
            .await
            .map_err(|_| ProviderError::ProcessClosed)?;
        writer
            .flush()
            .await
            .map_err(|_| ProviderError::ProcessClosed)
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
            } else {
                json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": { "code": -32601, "message": "Unsupported client method" }
                })
            };
            inner.write_message(response).await?;
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
            let update_type = update
                .get("sessionUpdate")
                .and_then(Value::as_str)
                .unwrap_or_default();
            match update_type {
                "agent_message_chunk" => {
                    let content = update.get("content").unwrap_or(&Value::Null);
                    if content.get("type").and_then(Value::as_str) == Some("text") {
                        if let Some(text) = content.get("text").and_then(Value::as_str) {
                            push_text(inner, session_id, text).await;
                        }
                    }
                }
                "tool_call" | "tool_call_update" => deny_tool_use(inner, session_id).await,
                _ => {}
            }
        }
        return Ok(());
    }

    let id = message
        .get("id")
        .and_then(Value::as_u64)
        .ok_or(ProviderError::Protocol)?;
    let sender = inner.pending.lock().await.remove(&id);
    let Some(sender) = sender else {
        return Ok(());
    };
    let result = if let Some(result) = message.get("result") {
        Ok(result.clone())
    } else if message.get("error").is_some() {
        Err(ProviderError::RemoteRequestFailed)
    } else {
        Err(ProviderError::InvalidResponse)
    };
    let _ = sender.send(result);
    Ok(())
}

async fn push_text(inner: &Arc<Inner>, session_id: &str, text: &str) {
    let state = inner.prompts.lock().await.get(session_id).cloned();
    let Some(state) = state else {
        return;
    };
    let next_size = {
        let mut accumulated = state.text.lock().await;
        let next_size = accumulated.len().saturating_add(text.len());
        if next_size <= MAX_RESPONSE_BYTES {
            accumulated.push_str(text);
        }
        next_size
    };
    if next_size > MAX_RESPONSE_BYTES {
        state.output_limited.store(true, Ordering::Release);
        let _ = inner
            .notify("session/cancel", json!({ "sessionId": session_id }))
            .await;
        return;
    }
    if state.event_count.fetch_add(1, Ordering::Relaxed) >= MAX_PROMPT_EVENTS
        || state
            .events
            .try_send(PromptEvent::TextDelta(text.to_owned()))
            .is_err()
    {
        state.event_limited.store(true, Ordering::Release);
        let _ = inner
            .notify("session/cancel", json!({ "sessionId": session_id }))
            .await;
    }
}

async fn deny_tool_use(inner: &Arc<Inner>, session_id: &str) {
    let state = inner.prompts.lock().await.get(session_id).cloned();
    if let Some(state) = state {
        if !state.tool_denied.swap(true, Ordering::AcqRel) {
            let _ = state.events.try_send(PromptEvent::SecurityViolation);
        }
    }
    let _ = inner
        .notify("session/cancel", json!({ "sessionId": session_id }))
        .await;
}

async fn close_transport(inner: &Arc<Inner>, error: ProviderError) {
    inner.closed.store(true, Ordering::Release);
    let protocol_error = matches!(error, ProviderError::Protocol);
    let mut pending = inner.pending.lock().await;
    for (_, sender) in pending.drain() {
        let error = if protocol_error {
            ProviderError::Protocol
        } else {
            ProviderError::ProcessExited
        };
        let _ = sender.send(Err(error));
    }
}

async fn supervise_child(
    inner: Arc<Inner>,
    mut child: Child,
    exit_sender: watch::Sender<Option<i32>>,
) {
    let status = child.wait().await;
    let exit_code = status
        .as_ref()
        .ok()
        .and_then(ExitStatus::code)
        .unwrap_or(-1);
    let _ = exit_sender.send(Some(exit_code));
    close_transport(&inner, ProviderError::ProcessExited).await;
}

struct FlagReset<'a>(&'a AtomicBool);

impl Drop for FlagReset<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

fn parse_available_models(model_state: &Value) -> Result<Vec<AvailableModel>, ProviderError> {
    let models = model_state
        .get("availableModels")
        .and_then(Value::as_array)
        .ok_or(ProviderError::InvalidResponse)?;
    let parsed = models
        .iter()
        .map(|model| {
            let model_id = model
                .get("modelId")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .ok_or(ProviderError::InvalidResponse)?;
            Ok(AvailableModel {
                model_id: model_id.to_owned(),
                name: model.get("name").and_then(Value::as_str).map(str::to_owned),
                description: model
                    .get("description")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                context_window_tokens: model.get("contextWindowTokens").and_then(Value::as_u64),
                max_output_tokens: model.get("maxOutputTokens").and_then(Value::as_u64),
            })
        })
        .collect::<Result<Vec<_>, ProviderError>>()?;
    if parsed.is_empty() {
        return Err(ProviderError::InvalidResponse);
    }
    Ok(parsed)
}

async fn set_model(inner: &Inner, session_id: &str, selected: &str) -> Result<(), ProviderError> {
    let (model, effort) = split_model_id(selected).ok_or(ProviderError::ModelUnavailable)?;
    let model_response = inner
        .call(
            "session/set_config_option",
            json!({ "sessionId": session_id, "configId": "model", "value": model }),
        )
        .await?;
    if config_value(&model_response, "model") != Some(model) {
        return Err(ProviderError::InvalidResponse);
    }
    let effort_response = inner
        .call(
            "session/set_config_option",
            json!({ "sessionId": session_id, "configId": "reasoning_effort", "value": effort }),
        )
        .await?;
    if config_value(&effort_response, "reasoning_effort") != Some(effort) {
        return Err(ProviderError::InvalidResponse);
    }
    Ok(())
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
