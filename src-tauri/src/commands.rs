#[path = "extraction_helper.rs"]
pub(crate) mod extraction_helper;

use cap_std::{
    ambient_authority,
    fs::{Dir, OpenOptions, OpenOptionsExt},
};
use magi_context::{
    CaptureDirective, CaptureLimits, MAX_MANIFEST_ITEMS, ManifestSource, ManifestSourceState,
    RepresentationKind, SourceCaptureManifest, SourceGrant, SourceOmission, SourceOmissionCode,
    classify_selected_path,
};
use magi_provider::{CodexAcpClient, RuntimeVerificationService, VerificationRequest};
use magi_storage::{Storage, StorageError};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Emitter, Manager, State, WebviewWindow, async_runtime};
use tauri_plugin_dialog::DialogExt;
use uuid::Uuid;

use crate::profiles::ProviderWorkdir;

#[derive(Clone, Default)]
pub(crate) struct ProviderOperationRegistry {
    by_runtime_home: Arc<Mutex<HashMap<String, Arc<async_runtime::Mutex<()>>>>>,
    by_provider_profile: Arc<Mutex<HashMap<String, Arc<async_runtime::Mutex<()>>>>>,
}

impl ProviderOperationRegistry {
    pub(crate) fn for_runtime_home(&self, runtime_home_id: &str) -> Arc<async_runtime::Mutex<()>> {
        let mut locks = self
            .by_runtime_home
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        locks
            .entry(runtime_home_id.to_owned())
            .or_insert_with(|| Arc::new(async_runtime::Mutex::new(())))
            .clone()
    }

    pub(crate) fn for_provider_profile(
        &self,
        provider_profile_id: &str,
    ) -> Arc<async_runtime::Mutex<()>> {
        let mut locks = self
            .by_provider_profile
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        locks
            .entry(provider_profile_id.to_owned())
            .or_insert_with(|| Arc::new(async_runtime::Mutex::new(())))
            .clone()
    }
}

type ActiveProviderAuthentications = HashMap<(String, u64), Arc<ProviderAuthenticationControl>>;

#[derive(Clone, Default)]
pub(crate) struct ProviderAuthenticationRegistry {
    active: Arc<Mutex<ActiveProviderAuthentications>>,
}

pub(crate) struct ProviderAuthenticationControl {
    client: Mutex<Option<Arc<CodexAcpClient>>>,
    cancellation_requested: AtomicBool,
}

impl ProviderAuthenticationRegistry {
    pub(crate) fn begin(
        &self,
        profile_id: &str,
        revision: u64,
    ) -> Option<Arc<ProviderAuthenticationControl>> {
        let mut active = self
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let key = (profile_id.to_owned(), revision);
        if active.contains_key(&key) {
            return None;
        }
        let control = Arc::new(ProviderAuthenticationControl {
            client: Mutex::new(None),
            cancellation_requested: AtomicBool::new(false),
        });
        active.insert(key, control.clone());
        Some(control)
    }

    pub(crate) fn install_client(
        &self,
        control: &Arc<ProviderAuthenticationControl>,
        client: Arc<CodexAcpClient>,
    ) -> bool {
        *control
            .client
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(client);
        control.cancellation_requested.load(Ordering::Acquire)
    }

    pub(crate) fn cancellation_requested(&self, control: &ProviderAuthenticationControl) -> bool {
        control.cancellation_requested.load(Ordering::Acquire)
    }

    pub(crate) fn cancel(
        &self,
        profile_id: &str,
        revision: u64,
    ) -> Option<(
        Arc<ProviderAuthenticationControl>,
        Option<Arc<CodexAcpClient>>,
    )> {
        let control = self
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&(profile_id.to_owned(), revision))
            .cloned()?;
        control
            .cancellation_requested
            .store(true, Ordering::Release);
        let client = control
            .client
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        Some((control, client))
    }

    pub(crate) fn remove(
        &self,
        profile_id: &str,
        revision: u64,
        control: &Arc<ProviderAuthenticationControl>,
    ) {
        let mut active = self
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let key = (profile_id.to_owned(), revision);
        if active
            .get(&key)
            .is_some_and(|current| Arc::ptr_eq(current, control))
        {
            active.remove(&key);
        }
    }
}

const ADMISSION_REQUEST_LIMIT: usize = 4096;
const ADMISSION_REQUEST_DEADLINE: Duration = Duration::from_secs(300);

#[derive(Clone, Default)]
pub(crate) struct NativeResourceCoordinator {
    state: Arc<Mutex<NativeResourceState>>,
    closed: Arc<AtomicBool>,
}
#[derive(Default)]
struct NativeResourceState {
    cold_initialization: bool,
    frozen: bool,
    epoch: u64,
    operations: usize,
    roots: HashMap<String, VerificationRequest>,
}
pub(crate) struct NativeResourceOperation {
    coordinator: NativeResourceCoordinator,
    epoch: u64,
}
pub(crate) struct NativeResourceAdmission<'a> {
    coordinator: &'a NativeResourceCoordinator,
}
struct StartupRequestGuard(Option<VerificationRequest>);
impl Drop for StartupRequestGuard {
    fn drop(&mut self) {
        if let Some(request) = self.0.take() {
            request.revoke();
        }
    }
}
impl Drop for NativeResourceOperation {
    fn drop(&mut self) {
        if let Ok(mut state) = self.coordinator.state.lock() {
            state.operations -= 1;
        }
    }
}
impl NativeResourceOperation {
    pub(crate) fn admit<T>(
        &self,
        effect: impl FnOnce(&NativeResourceAdmission<'_>) -> T,
    ) -> Result<T, magi_provider::ProviderError> {
        let state = self
            .coordinator
            .state
            .lock()
            .map_err(|_| magi_provider::ProviderError::Cancelled)?;
        if state.frozen || state.epoch != self.epoch {
            return Err(magi_provider::ProviderError::Cancelled);
        }
        Ok(effect(&NativeResourceAdmission {
            coordinator: &self.coordinator,
        }))
    }
    pub(crate) fn check(&self) -> Result<(), magi_provider::ProviderError> {
        let state = self
            .coordinator
            .state
            .lock()
            .map_err(|_| magi_provider::ProviderError::Cancelled)?;
        if state.frozen || state.epoch != self.epoch {
            Err(magi_provider::ProviderError::Cancelled)
        } else {
            Ok(())
        }
    }
}
#[derive(Clone)]
pub(crate) struct NativeResourceQuiescence {
    coordinator: NativeResourceCoordinator,
    epoch: u64,
    roots: Vec<VerificationRequest>,
    native_operations: Vec<Arc<AtomicUsize>>,
    consumers_closed: bool,
    extraction: Option<Arc<dyn magi_provider::resource_custody::HeldExtractionAuthority>>,
}
impl NativeResourceCoordinator {
    fn cold_start() -> Self {
        Self {
            state: Arc::new(Mutex::new(NativeResourceState {
                frozen: true,
                cold_initialization: true,
                epoch: 1,
                ..NativeResourceState::default()
            })),
            closed: Arc::new(AtomicBool::new(true)),
        }
    }
    fn cold_permission(&self) -> Result<NativeResourceQuiescence, magi_provider::ProviderError> {
        let state = self
            .state
            .lock()
            .map_err(|_| magi_provider::ProviderError::Cancelled)?;
        if !state.cold_initialization
            || !state.frozen
            || state.epoch != 1
            || state.operations != 0
            || !state.roots.is_empty()
        {
            return Err(magi_provider::ProviderError::ArtifactVerification);
        }
        extraction_helper::assert_unconfigured()
            .map_err(|_| magi_provider::ProviderError::ArtifactVerification)?;
        Ok(NativeResourceQuiescence {
            coordinator: self.clone(),
            epoch: state.epoch,
            roots: Vec::new(),
            native_operations: Vec::new(),
            consumers_closed: true,
            extraction: None,
        })
    }
    fn cold_shutdown_with_prepare<T>(
        &self,
        operation: &NativeResourceOperation,
        prepare: impl FnOnce() -> Result<T, magi_provider::ProviderError>,
    ) -> Result<(NativeResourceQuiescence, T), magi_provider::ProviderError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| magi_provider::ProviderError::Cancelled)?;
        if !state.cold_initialization
            || !state.frozen
            || state.epoch != 1
            || state.operations != 1
            || operation.epoch != state.epoch
            || !Arc::ptr_eq(&operation.coordinator.state, &self.state)
            || !state.roots.is_empty()
        {
            return Err(magi_provider::ProviderError::ArtifactVerification);
        }
        extraction_helper::assert_unconfigured()
            .map_err(|_| magi_provider::ProviderError::ArtifactVerification)?;
        let prepared = prepare()?;
        state.cold_initialization = false;
        Ok((
            NativeResourceQuiescence {
                coordinator: self.clone(),
                epoch: state.epoch,
                roots: Vec::new(),
                native_operations: Vec::new(),
                consumers_closed: true,
                extraction: None,
            },
            prepared,
        ))
    }
    pub(crate) fn preflight_operation(
        &self,
    ) -> Result<NativeResourceOperation, magi_provider::ProviderError> {
        if self.check_open().is_ok() {
            return self.enter(None);
        }
        let mut state = self
            .state
            .lock()
            .map_err(|_| magi_provider::ProviderError::Cancelled)?;
        if !state.cold_initialization
            || !state.frozen
            || state.epoch != 1
            || !state.roots.is_empty()
        {
            return Err(magi_provider::ProviderError::Cancelled);
        }
        state.operations = state
            .operations
            .checked_add(1)
            .ok_or(magi_provider::ProviderError::InvalidLaunch)?;
        Ok(NativeResourceOperation {
            coordinator: self.clone(),
            epoch: state.epoch,
        })
    }
    async fn shutdown_operation_until(
        &self,
        deadline: Instant,
    ) -> Result<NativeResourceOperation, magi_provider::ProviderError> {
        loop {
            let attempt = {
                match self.state.try_lock() {
                    Ok(mut state) => {
                        if state.frozen
                            && !(state.cold_initialization
                                && state.epoch == 1
                                && state.roots.is_empty())
                        {
                            Some(Err(magi_provider::ProviderError::Cancelled))
                        } else {
                            state.operations = state
                                .operations
                                .checked_add(1)
                                .ok_or(magi_provider::ProviderError::InvalidLaunch)?;
                            Some(Ok(NativeResourceOperation {
                                coordinator: self.clone(),
                                epoch: state.epoch,
                            }))
                        }
                    }
                    Err(std::sync::TryLockError::WouldBlock) => None,
                    Err(std::sync::TryLockError::Poisoned(_)) => {
                        Some(Err(magi_provider::ProviderError::Cancelled))
                    }
                }
            };
            if let Some(result) = attempt {
                return result;
            }
            if Instant::now() >= deadline {
                return Err(magi_provider::ProviderError::Timeout);
            }
            tokio::time::sleep(
                Duration::from_millis(1).min(deadline.saturating_duration_since(Instant::now())),
            )
            .await;
        }
    }
    fn activate_installed(
        &self,
        permission: &NativeResourceQuiescence,
        activate: impl FnOnce(&NativeResourceAdmission<'_>) -> Result<(), magi_provider::ProviderError>,
    ) -> Result<(), magi_provider::ProviderError> {
        permission.validate()?;
        let mut state = self
            .state
            .lock()
            .map_err(|_| magi_provider::ProviderError::Cancelled)?;
        if !Arc::ptr_eq(&self.state, &permission.coordinator.state)
            || !state.cold_initialization
            || !state.frozen
            || state.epoch != 1
            || state.epoch != permission.epoch
            || state.operations != 0
        {
            return Err(magi_provider::ProviderError::ArtifactVerification);
        }
        let next_epoch = state
            .epoch
            .checked_add(1)
            .ok_or(magi_provider::ProviderError::InvalidLaunch)?;
        activate(&NativeResourceAdmission { coordinator: self })?;
        state.epoch = next_epoch;
        state.cold_initialization = false;
        state.frozen = false;
        self.closed.store(false, Ordering::Release);
        Ok(())
    }
    pub(crate) fn check_open(&self) -> Result<(), magi_provider::ProviderError> {
        if self.closed.load(Ordering::Acquire) {
            Err(magi_provider::ProviderError::Cancelled)
        } else {
            Ok(())
        }
    }
    pub(crate) fn enter(
        &self,
        root: Option<VerificationRequest>,
    ) -> Result<NativeResourceOperation, magi_provider::ProviderError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| magi_provider::ProviderError::Cancelled)?;
        if state.frozen {
            return Err(magi_provider::ProviderError::Cancelled);
        }
        state
            .roots
            .retain(|_, root| !root.observed_revoked_settlement());
        if let Some(root) = root {
            root.check()?;
            if !state
                .roots
                .values()
                .any(|existing| existing.shares_authority(&root))
            {
                if state.roots.len() >= ADMISSION_REQUEST_LIMIT {
                    return Err(magi_provider::ProviderError::InvalidLaunch);
                }
                state.roots.insert(Uuid::new_v4().to_string(), root);
            }
        }
        state.operations = state
            .operations
            .checked_add(1)
            .ok_or(magi_provider::ProviderError::InvalidLaunch)?;
        Ok(NativeResourceOperation {
            coordinator: self.clone(),
            epoch: state.epoch,
        })
    }
    pub(crate) fn freeze(
        &self,
        additional_roots: Vec<VerificationRequest>,
    ) -> Result<NativeResourceQuiescence, magi_provider::ProviderError> {
        self.freeze_with_prepare(additional_roots, || Ok(()))
            .map(|(permission, ())| permission)
    }
    fn freeze_with_prepare<T>(
        &self,
        additional_roots: Vec<VerificationRequest>,
        prepare: impl FnOnce() -> Result<T, magi_provider::ProviderError>,
    ) -> Result<(NativeResourceQuiescence, T), magi_provider::ProviderError> {
        let (epoch, roots, prepared) = {
            let mut state = self
                .state
                .lock()
                .map_err(|_| magi_provider::ProviderError::Cancelled)?;
            if state.frozen {
                return Err(magi_provider::ProviderError::Cancelled);
            }
            let prepared = prepare()?;
            state.frozen = true;
            self.closed.store(true, Ordering::Release);
            state.epoch = state
                .epoch
                .checked_add(1)
                .ok_or(magi_provider::ProviderError::InvalidLaunch)?;
            let mut roots: Vec<_> = state.roots.values().cloned().collect();
            roots.extend(additional_roots);
            (state.epoch, roots, prepared)
        };
        for root in &roots {
            root.revoke();
        }
        Ok((
            NativeResourceQuiescence {
                coordinator: self.clone(),
                epoch,
                roots,
                native_operations: Vec::new(),
                consumers_closed: false,
                extraction: None,
            },
            prepared,
        ))
    }
}

impl magi_provider::resource_custody::RetainedNativeQuiescence for NativeResourceQuiescence {
    fn validate_quiescence(&self) -> Result<(), magi_provider::ProviderError> {
        self.validate()?;
        if let Some(resource) = &self.extraction {
            resource.validate_held()?;
        }
        Ok(())
    }
    fn bound_requests(&self) -> &[VerificationRequest] {
        &self.roots
    }
    fn verified_extraction(
        &self,
    ) -> Result<
        Arc<dyn magi_provider::resource_custody::HeldExtractionAuthority>,
        magi_provider::ProviderError,
    > {
        self.validate()?;
        let resource = self
            .extraction
            .as_ref()
            .ok_or(magi_provider::ProviderError::ArtifactVerification)?;
        resource.validate_held()?;
        Ok(resource.clone())
    }
}
impl NativeResourceQuiescence {
    fn check_epoch(&self) -> Result<(), magi_provider::ProviderError> {
        let state = self
            .coordinator
            .state
            .try_lock()
            .map_err(|_| magi_provider::ProviderError::Cancelled)?;
        if !state.frozen || state.epoch != self.epoch {
            return Err(magi_provider::ProviderError::Cancelled);
        }
        Ok(())
    }
    fn cleanup_operation(&self) -> Result<NativeResourceOperation, magi_provider::ProviderError> {
        let mut state = self
            .coordinator
            .state
            .try_lock()
            .map_err(|_| magi_provider::ProviderError::Cancelled)?;
        if !state.frozen || state.epoch != self.epoch {
            return Err(magi_provider::ProviderError::Cancelled);
        }
        state.operations = state
            .operations
            .checked_add(1)
            .ok_or(magi_provider::ProviderError::InvalidLaunch)?;
        Ok(NativeResourceOperation {
            coordinator: self.coordinator.clone(),
            epoch: self.epoch,
        })
    }
    pub(crate) fn check_settlement(&self) -> Result<(), magi_provider::ProviderError> {
        let state = self
            .coordinator
            .state
            .try_lock()
            .map_err(|_| magi_provider::ProviderError::Cancelled)?;
        if !state.frozen
            || state.epoch != self.epoch
            || state.operations != 0
            || self
                .native_operations
                .iter()
                .any(|operations| operations.load(Ordering::SeqCst) != 0)
            || self
                .roots
                .iter()
                .any(|root| !root.observed_revoked_settlement())
        {
            return Err(magi_provider::ProviderError::Cancelled);
        }
        Ok(())
    }
    pub(crate) fn validate(&self) -> Result<(), magi_provider::ProviderError> {
        self.check_settlement()?;
        if self.consumers_closed {
            Ok(())
        } else {
            Err(magi_provider::ProviderError::Cancelled)
        }
    }
}

#[derive(Clone)]
pub(crate) struct NativeExecutionAuthority {
    pub(crate) storage: Arc<Storage>,
    pub(crate) expected: magi_storage::AdmissionExecutionAuthority,
}
impl NativeExecutionAuthority {
    pub(crate) fn capture(storage: Arc<Storage>) -> Result<Self, magi_provider::ProviderError> {
        let expected = storage
            .admission_execution_authority()
            .map_err(|_| magi_provider::ProviderError::Cancelled)?;
        if !expected.active {
            return Err(magi_provider::ProviderError::Cancelled);
        }
        Ok(Self { storage, expected })
    }
    pub(crate) fn validate(&self) -> Result<(), magi_provider::ProviderError> {
        self.storage
            .validate_admission_execution_authority(&self.expected)
            .map_err(|_| magi_provider::ProviderError::Cancelled)
    }
}

#[derive(Default)]
pub(crate) struct AdmissionRequestState {
    pub(crate) revoked: bool,
    pub(crate) admitted_run: Option<String>,
    pub(crate) terminal: bool,
    pub(crate) execution: Option<NativeExecutionAuthority>,
}

pub(crate) struct AdmissionRequestAuthority {
    pub(crate) verification: VerificationRequest,
    pub(crate) state: Arc<Mutex<AdmissionRequestState>>,
    pub(crate) binding: magi_storage::AdmissionRequestBinding,
    expires: Instant,
    expires_at: String,
    operations: Arc<AtomicUsize>,
    resources: NativeResourceCoordinator,
}

pub(crate) struct AdmissionOperationLease(pub(crate) Arc<AtomicUsize>);
impl Drop for AdmissionOperationLease {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

#[derive(Clone)]
pub(crate) struct AdmissionRequestLifecycle {
    pub(crate) verification: VerificationRequest,
    pub(crate) state: Arc<Mutex<AdmissionRequestState>>,
    pub(crate) operations: Arc<AtomicUsize>,
    expires: Instant,
    pub(crate) expires_at: String,
}

impl AdmissionRequestLifecycle {
    pub(crate) fn until(expires: Instant) -> Self {
        Self {
            verification: VerificationRequest::until(expires),
            state: Arc::new(Mutex::new(AdmissionRequestState::default())),
            operations: Arc::new(AtomicUsize::new(0)),
            expires,
            expires_at: crate::profiles::admission_expiry_at(expires),
        }
    }

    pub(crate) fn check(&self) -> Result<(), magi_provider::ProviderError> {
        let state = self
            .state
            .lock()
            .map_err(|_| magi_provider::ProviderError::Cancelled)?;
        if state.revoked || state.terminal {
            return Err(magi_provider::ProviderError::Cancelled);
        }
        if state.admitted_run.is_none() {
            self.verification.check()?;
        }
        Ok(())
    }

    pub(crate) fn bind_execution(
        &self,
        storage: Arc<Storage>,
    ) -> Result<(), magi_provider::ProviderError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| magi_provider::ProviderError::Cancelled)?;
        if state.revoked || state.terminal {
            return Err(magi_provider::ProviderError::Cancelled);
        }
        if let Some(execution) = &state.execution {
            if execution.validate().is_err() {
                state.revoked = true;
                self.verification.revoke();
                return Err(magi_provider::ProviderError::Cancelled);
            }
        } else {
            state.execution = Some(NativeExecutionAuthority::capture(storage)?);
        }
        Ok(())
    }
    pub(crate) fn check_execution(&self) -> Result<(), magi_provider::ProviderError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| magi_provider::ProviderError::Cancelled)?;
        if state.revoked
            || state.terminal
            || state
                .execution
                .as_ref()
                .is_none_or(|capture| capture.validate().is_err())
        {
            state.revoked = true;
            self.verification.revoke();
            return Err(magi_provider::ProviderError::Cancelled);
        }
        Ok(())
    }

    pub(crate) fn lease(&self) -> Result<AdmissionOperationLease, magi_provider::ProviderError> {
        let state = self
            .state
            .lock()
            .map_err(|_| magi_provider::ProviderError::Cancelled)?;
        if state.revoked || state.terminal {
            return Err(magi_provider::ProviderError::Cancelled);
        }
        if state.admitted_run.is_none() {
            self.verification.check()?;
        }
        self.operations.fetch_add(1, Ordering::SeqCst);
        Ok(AdmissionOperationLease(self.operations.clone()))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdmissionAuthorityDto {
    pub token: String,
    pub process_epoch: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IssuedAdmissionAuthorityDto {
    pub token: String,
    pub process_epoch: String,
    pub expires_at: String,
}

impl AdmissionRequestAuthority {
    pub(crate) fn check(&self) -> Result<(), magi_provider::ProviderError> {
        let state = self
            .state
            .lock()
            .map_err(|_| magi_provider::ProviderError::Cancelled)?;
        self.check_locked(&state)
    }

    pub(crate) fn check_locked(
        &self,
        state: &AdmissionRequestState,
    ) -> Result<(), magi_provider::ProviderError> {
        self.resources.check_open()?;
        if state.revoked || state.terminal {
            return Err(magi_provider::ProviderError::Cancelled);
        }
        if state.admitted_run.is_none() {
            self.verification.check()?;
        }
        Ok(())
    }

    pub(crate) fn execution(
        &self,
    ) -> Result<NativeExecutionAuthority, magi_provider::ProviderError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| magi_provider::ProviderError::Cancelled)?;
        self.check_locked(&state)?;
        if let Some(capture) = state
            .execution
            .as_ref()
            .filter(|capture| capture.validate().is_ok())
        {
            return Ok(capture.clone());
        }
        state.revoked = true;
        self.verification.revoke();
        Err(magi_provider::ProviderError::Cancelled)
    }
    pub(crate) fn effect_request(
        &self,
    ) -> Result<VerificationRequest, magi_provider::ProviderError> {
        self.execution()?;
        Ok(self
            .verification
            .with_deadline(Instant::now() + Duration::from_secs(60)))
    }

    pub(crate) fn revoke(&self) -> Option<String> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.revoked = true;
        self.verification.revoke();

        state.admitted_run.clone()
    }
}

impl AdmissionRequestAuthority {
    pub(crate) fn lease(
        self: &Arc<Self>,
    ) -> Result<AdmissionOperationLease, magi_provider::ProviderError> {
        self.resources.check_open()?;
        let state = self
            .state
            .lock()
            .map_err(|_| magi_provider::ProviderError::Cancelled)?;
        self.check_locked(&state)?;
        self.operations.fetch_add(1, Ordering::SeqCst);
        Ok(AdmissionOperationLease(self.operations.clone()))
    }
    fn retireable(&self) -> bool {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !state.terminal && (state.admitted_run.is_some() || Instant::now() < self.expires) {
            return false;
        }
        state.revoked = true;
        self.verification.revoke();
        let settled = self.operations.load(Ordering::SeqCst) == 0
            && self.verification.settlement().is_settled();
        if settled {
            state.terminal = true;
        }
        settled
    }
}

#[derive(Clone)]
pub(crate) struct AdmissionRequestRegistry {
    pub(crate) requests: Arc<Mutex<HashMap<String, Arc<AdmissionRequestAuthority>>>>,
    pub(crate) epoch: Arc<String>,
    resources: NativeResourceCoordinator,
}
impl Default for AdmissionRequestRegistry {
    fn default() -> Self {
        Self {
            requests: Arc::new(Mutex::new(HashMap::new())),
            epoch: Arc::new(Uuid::new_v4().to_string()),
            resources: NativeResourceCoordinator::default(),
        }
    }
}
impl AdmissionRequestRegistry {
    pub(crate) fn issue_for_execution_admitted(
        &self,
        admission: &NativeResourceAdmission<'_>,
        binding: magi_storage::AdmissionRequestBinding,
        lifecycle: Option<&AdmissionRequestLifecycle>,
        storage: Arc<Storage>,
    ) -> Result<IssuedAdmissionAuthorityDto, magi_provider::ProviderError> {
        if !Arc::ptr_eq(&self.resources.state, &admission.coordinator.state)
            || admission.coordinator.closed.load(Ordering::Acquire)
        {
            return Err(magi_provider::ProviderError::Cancelled);
        }
        self.issue_from_lifecycle_inner(binding, lifecycle, Some(storage))
    }
    #[cfg(test)]
    pub(crate) fn issue(
        &self,
        binding: magi_storage::AdmissionRequestBinding,
    ) -> Result<IssuedAdmissionAuthorityDto, magi_provider::ProviderError> {
        self.issue_from_lifecycle(binding, None, None)
    }

    #[cfg(test)]
    pub(crate) fn issue_for_execution(
        &self,
        binding: magi_storage::AdmissionRequestBinding,
        lifecycle: Option<&AdmissionRequestLifecycle>,
        storage: Arc<Storage>,
    ) -> Result<IssuedAdmissionAuthorityDto, magi_provider::ProviderError> {
        self.issue_from_lifecycle(binding, lifecycle, Some(storage))
    }

    #[cfg(test)]
    pub(crate) fn issue_from_lifecycle(
        &self,
        binding: magi_storage::AdmissionRequestBinding,
        lifecycle: Option<&AdmissionRequestLifecycle>,
        execution_storage: Option<Arc<Storage>>,
    ) -> Result<IssuedAdmissionAuthorityDto, magi_provider::ProviderError> {
        let operation = self.resources.enter(None)?;
        let boundary = self
            .resources
            .state
            .lock()
            .map_err(|_| magi_provider::ProviderError::Cancelled)?;
        if boundary.frozen || boundary.epoch != operation.epoch {
            return Err(magi_provider::ProviderError::Cancelled);
        }
        let result = self.issue_from_lifecycle_inner(binding, lifecycle, execution_storage);
        drop(boundary);
        result
    }
    fn issue_from_lifecycle_inner(
        &self,
        binding: magi_storage::AdmissionRequestBinding,
        lifecycle: Option<&AdmissionRequestLifecycle>,
        execution_storage: Option<Arc<Storage>>,
    ) -> Result<IssuedAdmissionAuthorityDto, magi_provider::ProviderError> {
        let mut requests = self
            .requests
            .lock()
            .map_err(|_| magi_provider::ProviderError::Cancelled)?;
        requests.retain(|_, request| !request.retireable());
        if let Some((token, request)) = requests
            .iter()
            .find(|(_, request)| request.binding == binding && request.check().is_ok())
        {
            if let Some(lifecycle) = lifecycle {
                lifecycle.check()?;
                if !Arc::ptr_eq(&request.state, &lifecycle.state) {
                    return Err(magi_provider::ProviderError::InvalidLaunch);
                }
            }
            if execution_storage.is_some() {
                request.execution()?;
            }
            return Ok(IssuedAdmissionAuthorityDto {
                token: token.clone(),
                process_epoch: self.epoch.as_ref().clone(),
                expires_at: request.expires_at.clone(),
            });
        }
        if requests.len() >= ADMISSION_REQUEST_LIMIT {
            return Err(magi_provider::ProviderError::InvalidLaunch);
        }
        let default_lifecycle;
        let lifecycle = match lifecycle {
            Some(lifecycle) => lifecycle,
            None => {
                default_lifecycle =
                    AdmissionRequestLifecycle::until(Instant::now() + ADMISSION_REQUEST_DEADLINE);
                &default_lifecycle
            }
        };
        if let Some(storage) = execution_storage {
            lifecycle.bind_execution(storage)?;
        }
        let permission = lifecycle
            .state
            .lock()
            .map_err(|_| magi_provider::ProviderError::Cancelled)?;
        if permission.revoked || permission.terminal {
            return Err(magi_provider::ProviderError::Cancelled);
        }
        lifecycle.verification.check()?;
        let token = Uuid::new_v4().to_string();
        let expires = lifecycle.expires;
        let expires_at = lifecycle.expires_at.clone();
        requests.insert(
            token.clone(),
            Arc::new(AdmissionRequestAuthority {
                verification: lifecycle.verification.clone(),
                state: lifecycle.state.clone(),
                binding,
                expires,
                expires_at: expires_at.clone(),
                operations: lifecycle.operations.clone(),
                resources: self.resources.clone(),
            }),
        );
        Ok(IssuedAdmissionAuthorityDto {
            token,
            process_epoch: self.epoch.as_ref().clone(),
            expires_at,
        })
    }
    pub(crate) fn resolve(
        &self,
        capability: &AdmissionAuthorityDto,
        binding: &magi_storage::AdmissionRequestBinding,
    ) -> Result<Arc<AdmissionRequestAuthority>, magi_provider::ProviderError> {
        self.resources.check_open()?;
        if capability.process_epoch != *self.epoch {
            return Err(magi_provider::ProviderError::InvalidLaunch);
        }
        let request = self
            .requests
            .lock()
            .map_err(|_| magi_provider::ProviderError::Cancelled)?
            .get(&capability.token)
            .cloned()
            .ok_or(magi_provider::ProviderError::InvalidLaunch)?;
        if request.binding != *binding {
            return Err(magi_provider::ProviderError::InvalidLaunch);
        }
        if Instant::now() >= request.expires {
            return Err(magi_provider::ProviderError::Timeout);
        }
        request.check()?;
        Ok(request)
    }
    #[cfg(test)]
    pub(crate) fn matching(
        &self,
        binding: &magi_storage::AdmissionRequestBinding,
    ) -> Vec<Arc<AdmissionRequestAuthority>> {
        self.requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .filter(|r| {
                r.binding.idempotency_key == binding.idempotency_key
                    && r.binding.intent_digest == binding.intent_digest
            })
            .cloned()
            .collect()
    }
    pub(crate) fn cancellation_authorities(
        &self,
        capability: Option<&AdmissionAuthorityDto>,
        command_id: &str,
        key: &str,
    ) -> Result<Vec<Arc<AdmissionRequestAuthority>>, magi_provider::ProviderError> {
        let requests = self
            .requests
            .lock()
            .map_err(|_| magi_provider::ProviderError::Cancelled)?;
        let mut matching: Vec<_> = requests
            .iter()
            .filter(|(_, request)| request.binding.idempotency_key == key)
            .collect();
        matching.sort_by(|left, right| left.0.cmp(right.0));
        let matching: Vec<_> = matching
            .into_iter()
            .map(|(_, request)| request.clone())
            .collect();
        if let Some(capability) = capability {
            if capability.process_epoch != *self.epoch {
                return Err(magi_provider::ProviderError::InvalidLaunch);
            }
            let request = requests
                .get(&capability.token)
                .ok_or(magi_provider::ProviderError::InvalidLaunch)?;
            if request.binding.command_id != command_id || request.binding.idempotency_key != key {
                return Err(magi_provider::ProviderError::InvalidLaunch);
            }
        } else if !matching.is_empty() {
            return Err(magi_provider::ProviderError::InvalidLaunch);
        }
        Ok(matching)
    }
    #[cfg(test)]
    pub(crate) fn reference(
        &self,
        command_id: &str,
    ) -> Result<Arc<AdmissionRequestAuthority>, magi_provider::ProviderError> {
        let binding = magi_storage::AdmissionRequestBinding {
            command_id: command_id.into(),
            idempotency_key: command_id.into(),
            intent_digest: magi_domain::Digest::from_bytes(command_id.as_bytes()),
        };
        if let Some(value) = self.matching(&binding).into_iter().next() {
            return Ok(value);
        }
        let issued = self.issue(binding.clone())?;
        self.resolve(
            &AdmissionAuthorityDto {
                token: issued.token,
                process_epoch: issued.process_epoch,
            },
            &binding,
        )
    }
}

#[derive(Clone, Default)]
pub(crate) struct LiveRunControlRegistry {
    by_run: Arc<Mutex<HashMap<String, Arc<LiveRunControl>>>>,
    coordination: Arc<Mutex<()>>,
}

pub(crate) struct LiveRunControl {
    pub(crate) cancellation_requested: AtomicBool,
    pub(crate) operation: async_runtime::Mutex<()>,
    provider_startup_started: AtomicBool,
    session_creation_started: AtomicBool,
    session_creation_outcome_unknown: AtomicBool,
    active_session: Mutex<ActiveRunSession>,
    pub(crate) admission_request: Mutex<Option<Arc<AdmissionRequestAuthority>>>,
    effect_root: VerificationRequest,
    execution: Mutex<Option<NativeExecutionAuthority>>,
}

#[derive(Default)]
struct ActiveRunSession {
    client: Option<Arc<CodexAcpClient>>,
    session_id: Option<String>,
    _workdir: Option<Arc<ProviderWorkdir>>,
}

impl LiveRunControlRegistry {
    pub(crate) fn coordinate(&self) -> std::sync::MutexGuard<'_, ()> {
        self.coordination
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(crate) fn register(&self, run_id: &str) -> Arc<LiveRunControl> {
        let mut controls = self
            .by_run
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        controls
            .entry(run_id.to_owned())
            .or_insert_with(|| {
                Arc::new(LiveRunControl {
                    cancellation_requested: AtomicBool::new(false),
                    operation: async_runtime::Mutex::new(()),
                    provider_startup_started: AtomicBool::new(false),
                    session_creation_started: AtomicBool::new(false),
                    session_creation_outcome_unknown: AtomicBool::new(false),
                    active_session: Mutex::new(ActiveRunSession::default()),
                    admission_request: Mutex::new(None),
                    execution: Mutex::new(None),
                    effect_root: VerificationRequest::until(
                        Instant::now() + Duration::from_secs(60),
                    ),
                })
            })
            .clone()
    }

    pub(crate) fn remove(&self, run_id: &str, control: &Arc<LiveRunControl>) {
        let removed = {
            let mut controls = self
                .by_run
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if controls
                .get(run_id)
                .is_some_and(|current| Arc::ptr_eq(current, control))
            {
                controls.remove(run_id);
                true
            } else {
                false
            }
        };
        if removed {
            let authority = control
                .admission_request
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            if let Some(authority) = authority {
                authority
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .terminal = true;
                authority.revoke();
            }
        }
    }

    pub(crate) fn get(&self, run_id: &str) -> Option<Arc<LiveRunControl>> {
        self.by_run
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(run_id)
            .cloned()
    }
}

impl LiveRunControl {
    pub(crate) fn bind_execution(
        &self,
        capture: NativeExecutionAuthority,
    ) -> Result<(), magi_provider::ProviderError> {
        capture.validate()?;
        let mut current = self
            .execution
            .lock()
            .map_err(|_| magi_provider::ProviderError::Cancelled)?;
        if current
            .as_ref()
            .is_some_and(|old| old.expected != capture.expected)
        {
            drop(current);
            self.revoke_effects();
            return Err(magi_provider::ProviderError::Cancelled);
        }
        *current = Some(capture);
        Ok(())
    }
    pub(crate) fn execution(
        &self,
    ) -> Result<NativeExecutionAuthority, magi_provider::ProviderError> {
        let capture = self
            .execution
            .lock()
            .map_err(|_| magi_provider::ProviderError::Cancelled)?
            .clone();
        if let Some(capture) = capture.filter(|capture| capture.validate().is_ok()) {
            return Ok(capture);
        }
        self.revoke_effects();
        Err(magi_provider::ProviderError::Cancelled)
    }
    pub(crate) fn effect_request(
        &self,
    ) -> Result<VerificationRequest, magi_provider::ProviderError> {
        self.check_effect_authority()?;
        let admission = self
            .admission_request
            .lock()
            .map_err(|_| magi_provider::ProviderError::Cancelled)?
            .clone();
        let request = if let Some(admission) = admission {
            admission.effect_request()?
        } else {
            self.effect_root
                .with_deadline(Instant::now() + Duration::from_secs(60))
        };
        if self.cancellation_requested.load(Ordering::Acquire) {
            request.revoke();
            return Err(magi_provider::ProviderError::Cancelled);
        }
        Ok(request)
    }

    pub(crate) fn revoke_effects(&self) {
        self.cancellation_requested.store(true, Ordering::Release);
        self.effect_root.revoke();
        let authority = self
            .admission_request
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if let Some(authority) = authority {
            authority.revoke();
        }
    }

    pub(crate) fn check_effect_authority(&self) -> Result<(), magi_provider::ProviderError> {
        if self.cancellation_requested.load(Ordering::Acquire) {
            return Err(magi_provider::ProviderError::Cancelled);
        }
        self.execution()?;
        let request = self
            .admission_request
            .lock()
            .map_err(|_| magi_provider::ProviderError::Cancelled)?
            .clone();
        if let Some(request) = request {
            request.check()?;
        }
        Ok(())
    }

    pub(crate) fn mark_provider_startup_started(&self) {
        self.provider_startup_started.store(true, Ordering::Release);
    }

    pub(crate) fn provider_startup_started(&self) -> bool {
        self.provider_startup_started.load(Ordering::Acquire)
    }

    pub(crate) fn session_creation_started(&self) -> bool {
        self.session_creation_started.load(Ordering::Acquire)
    }

    pub(crate) fn mark_session_creation_started(&self) {
        self.session_creation_started.store(true, Ordering::Release);
        self.session_creation_outcome_unknown
            .store(true, Ordering::Release);
    }

    pub(crate) fn set_session_creation_outcome_unknown(&self, unknown: bool) {
        self.session_creation_outcome_unknown
            .store(unknown, Ordering::Release);
    }

    pub(crate) fn session_creation_outcome_unknown(&self) -> bool {
        self.session_creation_outcome_unknown
            .load(Ordering::Acquire)
    }

    pub(crate) fn install_client(
        &self,
        client: Arc<CodexAcpClient>,
        workdir: Arc<ProviderWorkdir>,
    ) {
        let mut session = self
            .active_session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        session.client = Some(client);
        session.session_id = None;
        session._workdir = Some(workdir);
    }

    pub(crate) fn clear_provider_turn(&self, client: &Arc<CodexAcpClient>) {
        let mut session = self
            .active_session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if session
            .client
            .as_ref()
            .is_some_and(|active| Arc::ptr_eq(active, client))
        {
            *session = ActiveRunSession::default();
        }
    }

    pub(crate) fn install_session(&self, session_id: String) {
        self.active_session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .session_id = Some(session_id);
    }

    pub(crate) fn active_session(&self) -> Option<(Arc<CodexAcpClient>, String)> {
        let session = self
            .active_session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Some((session.client.clone()?, session.session_id.clone()?))
    }

    pub(crate) fn client(&self) -> Option<Arc<CodexAcpClient>> {
        self.active_session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .client
            .clone()
    }
}

fn prepare_application_cancellation(
    storage: Option<Arc<Storage>>,
    controls: &LiveRunControlRegistry,
) -> Result<Vec<String>, magi_provider::ProviderError> {
    let _coordination = controls.coordinate();
    let Some(storage) = storage else {
        return Ok(Vec::new());
    };
    let expected = storage
        .admission_execution_authority()
        .map_err(|_| magi_provider::ProviderError::ArtifactVerification)?;
    let ids = storage
        .list_live_run_ids_requiring_shutdown_with_authority(&expected)
        .map_err(|_| magi_provider::ProviderError::ArtifactVerification)?;
    for id in &ids {
        storage
            .begin_deliberation_cancel(id, &crate::profiles::now_rfc3339())
            .map_err(|_| magi_provider::ProviderError::ArtifactVerification)?;
    }
    Ok(ids)
}

#[derive(Clone)]
struct PreparedApplicationShutdown {
    permission: NativeResourceQuiescence,
    pending_runs: Vec<String>,
    durable_cancellation_prepared: bool,
}

#[derive(Default)]
enum ApplicationShutdownState {
    #[default]
    Open,
    Preparing,
    Frozen(PreparedApplicationShutdown),
}

pub(crate) struct DesktopState {
    storage: Option<Arc<Storage>>,
    storage_diagnostic: Option<StorageUnavailableDiagnostic>,
    live_run_dispatcher: Option<async_runtime::Sender<()>>,
    dispatcher_receiver: Mutex<Option<async_runtime::Receiver<()>>>,
    provider_operations: ProviderOperationRegistry,
    provider_authentications: ProviderAuthenticationRegistry,
    pub(crate) live_run_controls: LiveRunControlRegistry,
    pub(crate) admission_requests: AdmissionRequestRegistry,
    runtime_verification: OnceLock<Arc<RuntimeVerificationService>>,
    resource_initialization: async_runtime::Mutex<()>,
    application_shutdown: Arc<Mutex<ApplicationShutdownState>>,
    shutdown_cleanup: async_runtime::Mutex<()>,
    #[cfg(test)]
    shutdown_preparation_gate: Mutex<Option<(mpsc::Sender<()>, mpsc::Receiver<()>)>>,
    pub(crate) resource_coordinator: NativeResourceCoordinator,
}

impl DesktopState {
    pub(crate) fn runtime_service(
        &self,
    ) -> Result<Arc<RuntimeVerificationService>, magi_provider::ProviderError> {
        self.resource_coordinator.check_open()?;
        self.runtime_verification
            .get()
            .cloned()
            .ok_or(magi_provider::ProviderError::ArtifactVerification)
    }
    pub(crate) async fn ensure_installed_resources(
        &self,
        publication: PathBuf,
        app_config: PathBuf,
        executable: PathBuf,
        request: VerificationRequest,
    ) -> Result<Arc<RuntimeVerificationService>, magi_provider::ProviderError> {
        let _initialization = tokio::time::timeout_at(
            tokio::time::Instant::from_std(request.deadline()),
            self.resource_initialization.lock(),
        )
        .await
        .map_err(|_| magi_provider::ProviderError::Timeout)?;
        if let Ok(service) = self.runtime_service() {
            return Ok(service);
        }
        let mut startup_guard = StartupRequestGuard(Some(request.clone()));
        let mut permission = self.resource_coordinator.cold_permission()?;
        let operation = permission.cleanup_operation()?;
        let helper_root = publication.clone();
        let helper_request = VerificationRequest::until(request.deadline());
        let _helper_abort = StartupRequestGuard(Some(helper_request.clone()));
        let worker_request = helper_request.clone();
        let worker = async_runtime::spawn_blocking(move || {
            let _operation = operation;
            extraction_helper::verify_cold_resource(&helper_root, worker_request)
        });
        let helper =
            tokio::time::timeout_at(tokio::time::Instant::from_std(request.deadline()), worker)
                .await
                .map_err(|_| magi_provider::ProviderError::Timeout)?
                .map_err(|_| magi_provider::ProviderError::ArtifactVerification)?
                .map_err(|_| magi_provider::ProviderError::ArtifactVerification)?;
        helper_request.revoke();
        helper_request
            .wait_for_settlement(request.deadline())
            .await?;
        permission.roots.push(helper_request);
        permission.extraction = Some(helper.clone());
        permission.validate()?;
        let native = Arc::new(permission);
        let proof = magi_provider::verify_installed_generation(
            publication.clone(),
            executable,
            native.clone(),
            request.clone(),
        )
        .await?;
        let lease = request.track_stream_operation()?;
        let worker_request = request.clone();
        let worker_native = native.clone();
        let worker = async_runtime::spawn_blocking(move || {
            let _lease = lease;
            worker_request.check()?;
            let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
                .parent()
                .ok_or(magi_provider::ProviderError::ArtifactVerification)?;
            let profile = if publication == workspace.join("src-tauri/target/debug") {
                Some(magi_provider::resource_custody::DevelopmentProfile::Debug)
            } else if publication == workspace.join("src-tauri/target/release") {
                Some(magi_provider::resource_custody::DevelopmentProfile::Release)
            } else {
                None
            };
            let namespace = || match profile {
                Some(profile) => magi_provider::resource_custody::ArtifactNamespace::development(
                    workspace, profile,
                ),
                None => magi_provider::resource_custody::ArtifactNamespace::installed_resources(
                    &publication,
                    &app_config,
                ),
            };
            let existing_namespace = namespace()?;
            let namespace = namespace()?;
            let custody =
                if magi_provider::resource_custody::ResourceCustody::open(existing_namespace)
                    .is_ok()
                {
                    magi_provider::resource_custody::ResourceCustody::reopen_verified(
                        namespace,
                        proof,
                        worker_native.clone(),
                    )?
                } else {
                    magi_provider::resource_custody::ResourceCustody::adopt_installed(
                        namespace,
                        proof,
                        worker_native.clone(),
                    )?
                };
            extraction_helper::attach_installed_resource(helper, custody.clone())
                .map_err(|_| magi_provider::ProviderError::ArtifactVerification)?;
            worker_request.check()?;
            Ok::<_, magi_provider::ProviderError>(custody)
        });
        let custody =
            tokio::time::timeout_at(tokio::time::Instant::from_std(request.deadline()), worker)
                .await
                .map_err(|_| magi_provider::ProviderError::Timeout)?
                .map_err(|_| magi_provider::ProviderError::ArtifactVerification)??;
        let service = Arc::new(RuntimeVerificationService::with_custody(custody));
        self.runtime_verification
            .set(service.clone())
            .map_err(|_| magi_provider::ProviderError::ArtifactVerification)?;
        self.resource_coordinator
            .activate_installed(native.as_ref(), |admission| {
                extraction_helper::activate_installed_resource(admission)
                    .map_err(|_| magi_provider::ProviderError::ArtifactVerification)
            })?;
        self.wake_live_run_dispatcher();
        startup_guard.0 = None;
        Ok(service)
    }
    async fn prepare_application_shutdown(
        &self,
        deadline: Instant,
        persist_cancellation: bool,
    ) -> Result<PreparedApplicationShutdown, magi_provider::ProviderError> {
        let previous = {
            let mut state = self
                .application_shutdown
                .try_lock()
                .map_err(|_| magi_provider::ProviderError::Cancelled)?;
            match &*state {
                ApplicationShutdownState::Frozen(prepared) => {
                    prepared.permission.check_epoch()?;
                    Some(prepared.clone())
                }
                ApplicationShutdownState::Preparing => {
                    return Err(magi_provider::ProviderError::Timeout);
                }
                ApplicationShutdownState::Open => {
                    *state = ApplicationShutdownState::Preparing;
                    None
                }
            }
        };
        if let Some(prepared) = previous {
            if persist_cancellation && !prepared.durable_cancellation_prepared {
                return self
                    .prepare_frozen_application_cancellation(prepared, deadline)
                    .await;
            }
            return Ok(prepared);
        }
        let operation = match self
            .resource_coordinator
            .shutdown_operation_until(deadline)
            .await
        {
            Ok(operation) => operation,
            Err(error) => {
                *self
                    .application_shutdown
                    .lock()
                    .map_err(|_| magi_provider::ProviderError::Cancelled)? =
                    ApplicationShutdownState::Open;
                return Err(error);
            }
        };
        let coordinator = self.resource_coordinator.clone();
        let requests = self.admission_requests.clone();
        let controls = self.live_run_controls.clone();
        let retained = self.application_shutdown.clone();
        let cold = self.runtime_verification.get().is_none();
        let storage = if persist_cancellation {
            match self.storage() {
                Ok(storage) => Some(storage),
                Err(_) => {
                    drop(operation);
                    *retained
                        .lock()
                        .map_err(|_| magi_provider::ProviderError::Cancelled)? =
                        ApplicationShutdownState::Open;
                    return Err(magi_provider::ProviderError::ArtifactVerification);
                }
            }
        } else {
            None
        };
        #[cfg(test)]
        let preparation_gate = self
            .shutdown_preparation_gate
            .lock()
            .map_err(|_| magi_provider::ProviderError::Cancelled)?
            .take();
        let worker = async_runtime::spawn_blocking(move || {
            #[cfg(test)]
            if let Some((ready, release)) = preparation_gate {
                let _ = ready.send(());
                let _ = release.recv();
            }
            let result = (|| {
                let (mut permission, pending_runs) = if cold {
                    coordinator.cold_shutdown_with_prepare(&operation, || {
                        prepare_application_cancellation(storage, &controls)
                    })?
                } else if storage.is_some() {
                    coordinator.freeze_with_prepare(Vec::new(), || {
                        prepare_application_cancellation(storage, &controls)
                    })?
                } else {
                    (coordinator.freeze(Vec::new())?, Vec::new())
                };
                if !cold {
                    let authorities: Vec<_> = requests
                        .requests
                        .lock()
                        .map_err(|_| magi_provider::ProviderError::Cancelled)?
                        .values()
                        .cloned()
                        .collect();
                    for authority in authorities {
                        authority.revoke();
                        permission.roots.push(authority.verification.clone());
                        permission
                            .native_operations
                            .push(authority.operations.clone());
                    }
                    let active: Vec<_> = controls
                        .by_run
                        .lock()
                        .map_err(|_| magi_provider::ProviderError::Cancelled)?
                        .values()
                        .cloned()
                        .collect();
                    for control in active {
                        control.revoke_effects();
                        permission.roots.push(control.effect_root.clone());
                    }
                }
                Ok::<_, magi_provider::ProviderError>(PreparedApplicationShutdown {
                    permission,
                    pending_runs,
                    durable_cancellation_prepared: persist_cancellation,
                })
            })();
            if let Ok(prepared) = &result {
                *retained
                    .lock()
                    .map_err(|_| magi_provider::ProviderError::Cancelled)? =
                    ApplicationShutdownState::Frozen(prepared.clone());
            }
            drop(operation);
            if result.is_err() {
                let safe_retry = coordinator.state.try_lock().is_ok_and(|state| {
                    !state.frozen
                        || (state.cold_initialization
                            && state.epoch == 1
                            && state.operations == 0
                            && state.roots.is_empty())
                });
                if safe_retry {
                    *retained
                        .lock()
                        .map_err(|_| magi_provider::ProviderError::Cancelled)? =
                        ApplicationShutdownState::Open;
                }
            }
            result
        });
        tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), worker)
            .await
            .map_err(|_| magi_provider::ProviderError::Timeout)?
            .map_err(|_| magi_provider::ProviderError::ArtifactVerification)?
    }
    async fn prepare_frozen_application_cancellation(
        &self,
        mut prepared: PreparedApplicationShutdown,
        deadline: Instant,
    ) -> Result<PreparedApplicationShutdown, magi_provider::ProviderError> {
        let storage = self
            .storage()
            .map_err(|_| magi_provider::ProviderError::ArtifactVerification)?;
        let controls = self.live_run_controls.clone();
        let retained = self.application_shutdown.clone();
        let operation = prepared.permission.cleanup_operation()?;
        let worker = async_runtime::spawn_blocking(move || {
            let _operation = operation;
            {
                let state = prepared
                    .permission
                    .coordinator
                    .state
                    .lock()
                    .map_err(|_| magi_provider::ProviderError::Cancelled)?;
                if !state.frozen || state.epoch != prepared.permission.epoch {
                    return Err(magi_provider::ProviderError::Cancelled);
                }
                prepared.pending_runs = prepare_application_cancellation(Some(storage), &controls)?;
            }
            prepared.durable_cancellation_prepared = true;
            let mut target = retained
                .lock()
                .map_err(|_| magi_provider::ProviderError::Cancelled)?;
            let ApplicationShutdownState::Frozen(previous) = &*target else {
                return Err(magi_provider::ProviderError::Cancelled);
            };
            if previous.permission.epoch != prepared.permission.epoch {
                return Err(magi_provider::ProviderError::Cancelled);
            }
            *target = ApplicationShutdownState::Frozen(prepared.clone());
            Ok(prepared)
        });
        tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), worker)
            .await
            .map_err(|_| magi_provider::ProviderError::Timeout)?
            .map_err(|_| magi_provider::ProviderError::ArtifactVerification)?
    }
    pub(crate) async fn freeze_resource_consumers(
        &self,
        deadline: Instant,
        exit: Option<(WebviewWindow, AppHandle)>,
    ) -> Result<NativeResourceQuiescence, magi_provider::ProviderError> {
        let _cleanup_serialization = tokio::time::timeout_at(
            tokio::time::Instant::from_std(deadline),
            self.shutdown_cleanup.lock(),
        )
        .await
        .map_err(|_| magi_provider::ProviderError::Timeout)?;
        let prepared = self
            .prepare_application_shutdown(deadline, exit.is_some())
            .await?;
        let mut permission = prepared.permission;
        let pending_runs = prepared.pending_runs;
        if let Some((window, app)) = exit {
            for run_id in pending_runs {
                tokio::time::timeout_at(
                    tokio::time::Instant::from_std(deadline),
                    crate::profiles::stop_run_for_application_shutdown(
                        window.clone(),
                        app.clone(),
                        self,
                        run_id,
                    ),
                )
                .await
                .map_err(|_| magi_provider::ProviderError::Timeout)?
                .map_err(|_| magi_provider::ProviderError::ArtifactVerification)?;
            }
        }
        if permission.consumers_closed {
            permission.validate()?;
            return Ok(permission);
        }
        let cleanup = permission.cleanup_operation()?;
        let helper_fence = async_runtime::spawn_blocking(move || {
            let _cleanup = cleanup;
            magi_context::close_extraction_helper_authority()
        });
        // Closing first revokes active helper work; an unresolved return is not
        // cleanup proof and must be retried only after actual worker settlement.
        let _fence_result =
            tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), helper_fence)
                .await
                .map_err(|_| magi_provider::ProviderError::Timeout)?
                .map_err(|_| magi_provider::ProviderError::ArtifactVerification)?;
        loop {
            if permission.check_settlement().is_ok() {
                break;
            }
            if Instant::now() >= deadline {
                return Err(magi_provider::ProviderError::Timeout);
            }
            tokio::time::sleep(
                Duration::from_millis(1).min(deadline.saturating_duration_since(Instant::now())),
            )
            .await;
        }
        let cleanup = permission.cleanup_operation()?;
        let helper_close = async_runtime::spawn_blocking(move || {
            let _cleanup = cleanup;
            magi_context::close_extraction_helper_authority()
        });
        tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), helper_close)
            .await
            .map_err(|_| magi_provider::ProviderError::Timeout)?
            .map_err(|_| magi_provider::ProviderError::ArtifactVerification)?
            .map_err(|_| magi_provider::ProviderError::ArtifactVerification)?;
        let service = self
            .runtime_verification
            .get()
            .ok_or(magi_provider::ProviderError::ArtifactVerification)?;
        tokio::time::timeout_at(
            tokio::time::Instant::from_std(deadline),
            service.close_custody(),
        )
        .await
        .map_err(|_| magi_provider::ProviderError::Timeout)??;
        permission.check_settlement()?;
        permission.extraction = Some(
            extraction_helper::retired_authority()
                .map_err(|_| magi_provider::ProviderError::ArtifactVerification)?,
        );
        permission.consumers_closed = true;
        permission.validate()?;
        let mut retained = self
            .application_shutdown
            .try_lock()
            .map_err(|_| magi_provider::ProviderError::Cancelled)?;
        let ApplicationShutdownState::Frozen(prepared) = &mut *retained else {
            return Err(magi_provider::ProviderError::Cancelled);
        };
        if prepared.permission.epoch != permission.epoch {
            return Err(magi_provider::ProviderError::Cancelled);
        }
        prepared.permission = permission.clone();
        Ok(permission)
    }
    pub(crate) fn open(app: &AppHandle) -> Self {
        let state = Self::open_quiet(app);
        state.activate_dispatcher(app);
        state.wake_live_run_dispatcher();
        state
    }

    pub(crate) fn open_quiet(app: &AppHandle) -> Self {
        let (storage, storage_diagnostic) = match app.path().app_data_dir() {
            Ok(data_root) => match Storage::open_or_create(data_root) {
                Ok(storage) => (Some(Arc::new(storage)), None),
                Err(error) => (None, Some(StorageUnavailableDiagnostic::from_error(&error))),
            },
            Err(_) => (
                None,
                Some(StorageUnavailableDiagnostic::app_data_unavailable()),
            ),
        };
        let provider_operations = ProviderOperationRegistry::default();
        let provider_authentications = ProviderAuthenticationRegistry::default();
        let live_run_controls = LiveRunControlRegistry::default();
        let (live_run_dispatcher, receiver) = async_runtime::channel(1);
        let live_run_dispatcher = storage.as_ref().map(|_| live_run_dispatcher);
        let resource_coordinator = NativeResourceCoordinator::cold_start();
        Self {
            storage,
            storage_diagnostic,
            live_run_dispatcher,
            dispatcher_receiver: Mutex::new(Some(receiver)),
            provider_operations,
            provider_authentications,
            live_run_controls,
            admission_requests: AdmissionRequestRegistry {
                resources: resource_coordinator.clone(),
                ..AdmissionRequestRegistry::default()
            },
            runtime_verification: OnceLock::new(),
            resource_initialization: async_runtime::Mutex::new(()),
            application_shutdown: Arc::new(Mutex::new(ApplicationShutdownState::Open)),
            shutdown_cleanup: async_runtime::Mutex::new(()),
            #[cfg(test)]
            shutdown_preparation_gate: Mutex::new(None),
            resource_coordinator,
        }
    }

    pub(crate) fn storage(&self) -> Result<Arc<Storage>, String> {
        self.storage.as_ref().cloned().ok_or_else(|| {
            self.storage_diagnostic
                .map(|diagnostic| format!("{} ({})", diagnostic.message, diagnostic.code))
                .unwrap_or_else(|| "Local storage is unavailable.".to_owned())
        })
    }

    pub(crate) fn is_storage_ready(&self) -> bool {
        self.storage.is_some()
    }

    pub(crate) fn provider_operation_for_home(
        &self,
        runtime_home_id: &str,
    ) -> Arc<async_runtime::Mutex<()>> {
        self.provider_operations.for_runtime_home(runtime_home_id)
    }

    pub(crate) fn provider_profile_revision_registry(&self) -> ProviderOperationRegistry {
        self.provider_operations.clone()
    }

    pub(crate) fn provider_authentications(&self) -> ProviderAuthenticationRegistry {
        self.provider_authentications.clone()
    }

    pub(crate) fn activate_dispatcher(&self, app: &AppHandle) {
        let receiver = self
            .dispatcher_receiver
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let (Some(storage), Some(receiver)) = (&self.storage, receiver) {
            crate::profiles::start_live_run_dispatcher(
                app.clone(),
                storage.clone(),
                receiver,
                self.provider_operations.clone(),
                self.live_run_controls.clone(),
                self.resource_coordinator.clone(),
            );
        }
    }

    pub(crate) fn wake_live_run_dispatcher(&self) {
        if let Some(dispatcher) = &self.live_run_dispatcher {
            let _ = dispatcher.try_send(());
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConsoleSnapshot {
    schema_version: u8,
    connection: ConnectionState,
    storage: StorageState,
    #[serde(skip_serializing_if = "Option::is_none")]
    storage_diagnostic: Option<StorageUnavailableDiagnostic>,
    #[serde(skip_serializing_if = "Option::is_none")]
    active_run: Option<crate::run_projection::ConsoleRunSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    confirmed_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    event_sequence: Option<u64>,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct StorageUnavailableDiagnostic {
    code: &'static str,
    message: &'static str,
    action: &'static str,
}

impl StorageUnavailableDiagnostic {
    fn app_data_unavailable() -> Self {
        Self {
            code: "app_data_unavailable",
            message: "앱 데이터 저장 위치에 접근하지 못했습니다.",
            action: "앱 데이터 접근 권한을 확인한 뒤 앱을 다시 여십시오.",
        }
    }

    fn from_error(error: &StorageError) -> Self {
        match error {
            StorageError::MigrationChecksum { .. } => Self {
                code: "migration_checksum_mismatch",
                message: "저장소의 마이그레이션 이력이 앱의 기록과 일치하지 않습니다. 데이터는 변경하지 않았습니다.",
                action: "저장소 파일을 삭제하거나 교체하지 말고 이 진단 코드를 지원 담당자에게 전달하십시오.",
            },
            StorageError::FutureSchema { .. } => Self {
                code: "unsupported_schema_version",
                message: "저장소가 이 앱 버전보다 새 스키마를 사용합니다.",
                action: "저장소를 그대로 유지하고 해당 버전을 지원하는 앱으로 여십시오.",
            },
            StorageError::Integrity(_) => Self {
                code: "store_integrity_check_failed",
                message: "저장소의 스키마 또는 무결성 확인에 실패했습니다. 데이터는 변경하지 않았습니다.",
                action: "저장소 파일을 수정하거나 삭제하지 말고 이 진단 코드를 지원 담당자에게 전달하십시오.",
            },
            StorageError::Io(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                Self {
                    code: "store_access_denied",
                    message: "앱이 로컬 저장소에 접근할 권한이 없습니다.",
                    action: "앱 데이터 접근 권한을 확인한 뒤 앱을 다시 여십시오.",
                }
            }
            StorageError::WriterAlreadyOpen => Self {
                code: "store_already_open",
                message: "다른 앱 프로세스가 로컬 저장소를 사용 중입니다.",
                action: "다른 MAGI 창을 닫은 뒤 다시 시도하십시오.",
            },
            _ => Self {
                code: "store_initialization_failed",
                message: "로컬 저장소를 시작하지 못했습니다.",
                action: "저장소 파일을 그대로 유지하고 이 진단 코드를 지원 담당자에게 전달하십시오.",
            },
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum ConnectionState {
    RuntimeAvailable,
    Blocked,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "lowercase")]
enum StorageState {
    Ready,
    Error,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContextSelectionRequest {
    draft_id: String,
    expected_revision: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextSelectionSummary {
    draft_id: String,
    manifest_id: String,
    manifest_digest: String,
    revision: u64,
    sources: Vec<CapturedSourceSummary>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CapturedSourceSummary {
    source_id: String,
    display_name: String,
    status: &'static str,
    byte_length: u64,
    representation: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    digest: Option<String>,
    issue_codes: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClarificationDraftView {
    schema_version: u16,
    parent: magi_storage::ClarificationParentReference,
    question: String,
    context: ContextSelectionSummary,
}

fn clarification_draft_view(
    draft: magi_storage::ClarificationDraft,
) -> Result<ClarificationDraftView, String> {
    Ok(ClarificationDraftView {
        schema_version: 1,
        parent: draft.parent,
        question: draft.question,
        context: context_summary(draft.context)?,
    })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateClarificationDraftInput {
    parent: magi_storage::ClarificationParentReference,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SaveClarificationQuestionInput {
    draft_id: String,
    expected_revision: u64,
    question: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiscardClarificationDraftInput {
    draft_id: String,
    expected_revision: u64,
}

#[tauri::command]
pub fn create_clarification_draft(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    input: CreateClarificationDraftInput,
) -> Result<ClarificationDraftView, String> {
    ensure_console_window(&window)?;
    if window.label() != "main" {
        return Err("Clarification drafts are available only in the main console.".into());
    }
    let _coordination = state.live_run_controls.coordinate();
    let storage = state.storage()?;
    let authority = NativeExecutionAuthority::capture(storage.clone())
        .map_err(|_| "The current execution authority cannot create a clarification draft.")?;
    let at = now_epoch_ms();
    let manifest = SourceCaptureManifest::draft(Vec::new(), at)
        .map_err(|_| "The empty clarification context could not be created.")?;
    let draft = storage
        .create_clarification_draft(
            &authority.expected,
            &input.parent,
            &format!("clarification-{}", uuid::Uuid::new_v4()),
            &manifest,
            at,
        )
        .map_err(|_| "The paused parent changed or cannot create a clarification draft.")?;
    clarification_draft_view(draft)
}

#[tauri::command]
pub fn list_clarification_drafts(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    parent: magi_storage::ClarificationParentReference,
) -> Result<Vec<ClarificationDraftView>, String> {
    ensure_console_window(&window)?;
    if window.label() != "main" {
        return Err("Clarification drafts are available only in the main console.".into());
    }
    let _coordination = state.live_run_controls.coordinate();
    let storage = state.storage()?;
    let authority = NativeExecutionAuthority::capture(storage.clone())
        .map_err(|_| "The current execution authority cannot discover clarification drafts.")?;
    storage
        .list_clarification_drafts(&authority.expected, &parent)
        .map_err(|_| "The paused parent changed or its clarification drafts are retired.")?
        .into_iter()
        .map(clarification_draft_view)
        .collect()
}

#[tauri::command]
pub fn load_clarification_draft(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    draft_id: String,
) -> Result<ClarificationDraftView, String> {
    ensure_console_window(&window)?;
    if window.label() != "main" {
        return Err("Clarification drafts are available only in the main console.".into());
    }
    let _coordination = state.live_run_controls.coordinate();
    let storage = state.storage()?;
    let authority = NativeExecutionAuthority::capture(storage.clone())
        .map_err(|_| "The current execution authority cannot load this clarification draft.")?;
    clarification_draft_view(
        storage
            .load_clarification_draft(&authority.expected, &draft_id)
            .map_err(|_| "The clarification draft is missing, stale, or retired.")?,
    )
}

#[tauri::command]
pub fn save_clarification_question(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    input: SaveClarificationQuestionInput,
) -> Result<ClarificationDraftView, String> {
    ensure_console_window(&window)?;
    if window.label() != "main" {
        return Err("Clarification edits are available only in the main console.".into());
    }
    let _coordination = state.live_run_controls.coordinate();
    let storage = state.storage()?;
    let authority = NativeExecutionAuthority::capture(storage.clone())
        .map_err(|_| "The current execution authority cannot edit this clarification draft.")?;
    clarification_draft_view(
        storage
            .save_clarification_question(
                &authority.expected,
                &input.draft_id,
                input.expected_revision,
                &input.question,
                now_epoch_ms(),
            )
            .map_err(|_| {
                "The clarification draft changed. Refresh its question and sources before saving."
            })?,
    )
}

#[tauri::command]
pub fn discard_clarification_draft(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    input: DiscardClarificationDraftInput,
) -> Result<(), String> {
    ensure_console_window(&window)?;
    if window.label() != "main" {
        return Err("Clarification edits are available only in the main console.".into());
    }
    let _coordination = state.live_run_controls.coordinate();
    let storage = state.storage()?;
    let authority = NativeExecutionAuthority::capture(storage.clone())
        .map_err(|_| "The current execution authority cannot discard this clarification draft.")?;
    storage
        .discard_clarification_draft(
            &authority.expected,
            &input.draft_id,
            input.expected_revision,
        )
        .map_err(|_| "The clarification draft changed or cannot be discarded safely.".into())
}

pub async fn verified_console_snapshot(
    window: WebviewWindow,
    app: AppHandle,
    state: State<'_, DesktopState>,
) -> Result<ConsoleSnapshot, String> {
    ensure_console_window(&window)?;
    let connection = if crate::profiles::packaged_provider_runtime_available(&app).await {
        ConnectionState::RuntimeAvailable
    } else {
        ConnectionState::Blocked
    };
    console_snapshot_with_connection(window, state, connection)
}

fn console_snapshot_with_connection(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    connection: ConnectionState,
) -> Result<ConsoleSnapshot, String> {
    ensure_console_window(&window)?;
    let storage = if state.storage.is_some() {
        StorageState::Ready
    } else {
        StorageState::Error
    };
    let selected = state
        .storage
        .as_ref()
        .map(|storage| {
            crate::run_projection::selected_console_snapshot(storage)
                .map_err(|_| "The persisted console run could not be loaded safely.")
        })
        .transpose()?
        .flatten();
    Ok(ConsoleSnapshot {
        schema_version: 1,
        connection,
        storage,
        storage_diagnostic: state.storage_diagnostic,
        active_run: selected
            .as_ref()
            .map(|snapshot| crate::run_projection::console_run_summary(&snapshot.run)),
        confirmed_at: selected
            .as_ref()
            .map(|snapshot| snapshot.run.run.updated_at.clone()),
        event_sequence: selected
            .as_ref()
            .map(|snapshot| snapshot.high_water.sequence),
    })
}

#[tauri::command]
pub fn load_run_dossier(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    run_id: String,
) -> Result<crate::run_projection::RunDossierView, String> {
    ensure_console_window(&window)?;
    load_run_dossier_view(state.storage()?.as_ref(), &run_id)
}

fn load_run_dossier_view(
    storage: &Storage,
    run_id: &str,
) -> Result<crate::run_projection::RunDossierView, String> {
    let (dossier, failure) = storage
        .load_run_dossier_with_live_failure(run_id)
        .map_err(|_| "The persisted deliberation dossier could not be loaded safely.")?;
    Ok(crate::run_projection::dossier_view_with_failure(
        &dossier.snapshot,
        failure.as_ref(),
    ))
}

#[tauri::command]
pub async fn select_context_files(
    window: WebviewWindow,
    app: AppHandle,
    state: State<'_, DesktopState>,
    request: Option<ContextSelectionRequest>,
) -> Result<Option<ContextSelectionSummary>, String> {
    if window.label() != "main" {
        return Err("Source selection is available only in the main console.".into());
    }

    let operation = state
        .resource_coordinator
        .enter(None)
        .map_err(|_| "Native resources are being closed.")?;

    let storage = state.storage()?;
    let (draft_id, stored_revision) = match request {
        Some(request) => {
            if request.draft_id.trim().is_empty() {
                return Err("The context draft is invalid.".into());
            }
            let current = storage
                .load_context_draft(&request.draft_id)
                .map_err(|_| "The context draft could not be loaded safely.")?;
            match current {
                Some(draft) if draft.revision == request.expected_revision => {
                    (request.draft_id, Some(request.expected_revision))
                }
                _ => return Err("The context draft changed. Review it and try again.".into()),
            }
        }
        None => (Uuid::new_v4().to_string(), None),
    };

    let (sender, receiver) = mpsc::sync_channel(1);
    app.dialog().file().pick_files(move |paths| {
        let _ = sender.send(paths);
    });
    let paths = tauri::async_runtime::spawn_blocking(move || receiver.recv())
        .await
        .map_err(|_| "The native file picker stopped unexpectedly.")?
        .map_err(|_| "The native file picker did not return a result.")?;
    let Some(paths) = paths else {
        return Ok(None);
    };
    if paths.is_empty() {
        return Ok(None);
    }
    if paths.len() > MAX_MANIFEST_ITEMS {
        return Err("Select no more than 1,000 files at a time.".into());
    }

    let paths = paths
        .into_iter()
        .map(|path| {
            path.into_path()
                .map_err(|_| "The selected item is not a local file.")
        })
        .collect::<Result<Vec<_>, _>>()?;
    let summary = tauri::async_runtime::spawn_blocking(move || {
        let _operation = operation;
        _operation
            .check()
            .map_err(|_| "Native resources are being closed.")?;
        let resource_root = app
            .path()
            .resource_dir()
            .map_err(|_| "Native extraction resources are unavailable.")?;
        capture_context_files(storage, draft_id, stored_revision, paths, &resource_root)
    })
    .await
    .map_err(|_| "The local capture operation stopped unexpectedly.")??;
    Ok(Some(summary))
}

#[tauri::command]
pub async fn select_context_directory(
    window: WebviewWindow,
    app: AppHandle,
    state: State<'_, DesktopState>,
    request: Option<ContextSelectionRequest>,
) -> Result<Option<ContextSelectionSummary>, String> {
    if window.label() != "main" {
        return Err("Source selection is available only in the main console.".into());
    }

    let operation = state
        .resource_coordinator
        .enter(None)
        .map_err(|_| "Native resources are being closed.")?;

    let storage = state.storage()?;
    let (draft_id, stored_revision) = match request {
        Some(request) => {
            if request.draft_id.trim().is_empty() {
                return Err("The context draft is invalid.".into());
            }
            let current = storage
                .load_context_draft(&request.draft_id)
                .map_err(|_| "The context draft could not be loaded safely.")?;
            match current {
                Some(draft) if draft.revision == request.expected_revision => {
                    (request.draft_id, Some(request.expected_revision))
                }
                _ => return Err("The context draft changed. Review it and try again.".into()),
            }
        }
        None => (Uuid::new_v4().to_string(), None),
    };

    let (sender, receiver) = mpsc::sync_channel(1);
    app.dialog().file().pick_folder(move |path| {
        let _ = sender.send(path);
    });
    let path = tauri::async_runtime::spawn_blocking(move || receiver.recv())
        .await
        .map_err(|_| "The native folder picker stopped unexpectedly.")?
        .map_err(|_| "The native folder picker did not return a result.")?;
    let Some(path) = path else {
        return Ok(None);
    };
    let path = path
        .into_path()
        .map_err(|_| "The selected folder is not a local directory.")?;
    let summary = tauri::async_runtime::spawn_blocking(move || {
        let _operation = operation;
        _operation
            .check()
            .map_err(|_| "Native resources are being closed.")?;
        let resource_root = app
            .path()
            .resource_dir()
            .map_err(|_| "Native extraction resources are unavailable.")?;
        capture_context_directory(storage, draft_id, stored_revision, path, &resource_root)
    })
    .await
    .map_err(|_| "The local folder capture operation stopped unexpectedly.")??;
    Ok(Some(summary))
}

fn ensure_console_window(window: &WebviewWindow) -> Result<(), String> {
    if window.label() == "main" || window.label() == "companion" {
        Ok(())
    } else {
        Err("This window cannot access the MAGI console state.".into())
    }
}

fn store_captured_object(
    storage: &Storage,
    object: &magi_context::CapturedObject,
) -> Result<(), String> {
    for (bytes, expected) in [
        (object.original_bytes(), &object.object_digest),
        (
            object.representation.text.as_bytes(),
            &object.derived_digest,
        ),
    ] {
        let stored = storage
            .put_source_object(bytes)
            .map_err(|_| "A captured source could not be stored locally.")?;
        if &stored.digest != expected || stored.byte_length != bytes.len() as u64 {
            return Err("A captured source failed its local integrity check.".into());
        }
    }
    Ok(())
}

pub(crate) fn capture_context_files(
    storage: Arc<Storage>,
    draft_id: String,
    stored_revision: Option<u64>,
    paths: Vec<PathBuf>,
    resource_root: &Path,
) -> Result<ContextSelectionSummary, String> {
    let existing = storage
        .load_context_draft(&draft_id)
        .map_err(|_| "The context draft could not be loaded safely.")?;
    if existing.as_ref().map(|draft| draft.revision) != stored_revision {
        return Err("The context draft changed. Review it and try again.".into());
    }
    let mut sources = existing
        .as_ref()
        .map(|draft| draft.manifest.content.sources.clone())
        .unwrap_or_default();
    if sources.len().saturating_add(paths.len()) > MAX_MANIFEST_ITEMS {
        return Err("The context draft cannot contain more than 1,000 items.".into());
    }

    let mut granted_files = Vec::new();
    let mut excluded_sources = Vec::new();
    for path in paths {
        let display_name = selected_display_name(&path);
        if let Some(code) = classify_selected_path(&path) {
            excluded_sources.push(omitted_source(code, display_name));
            continue;
        }
        match open_selected_file(&path) {
            Ok(file) => granted_files.push((file, display_name)),
            Err(code) => excluded_sources.push(omitted_source(code, display_name)),
        }
    }

    if !granted_files.is_empty() {
        let mut grant = SourceGrant::selected_files(granted_files)
            .map_err(|_| "The selected files could not be granted safely.")?;
        let enumeration = grant
            .enumerate(CaptureLimits::default_policy())
            .map_err(|_| "The selected files could not be inspected safely.")?;
        if enumeration.candidates.iter().any(|candidate| {
            matches!(
                candidate.mime_type.as_deref(),
                Some("application/pdf" | "image/png" | "image/jpeg")
            )
        }) {
            extraction_helper::ensure(resource_root)?;
        }
        let directives = enumeration
            .candidates
            .into_iter()
            .map(|candidate| CaptureDirective {
                source_id: candidate.source_id,
                line_range: None,
            })
            .collect::<Vec<_>>();
        let batch = grant
            .capture_selected(&directives, CaptureLimits::default_policy(), now_epoch_ms())
            .map_err(|_| "The selected files exceeded a capture safety limit.")?;
        for object in &batch.objects {
            store_captured_object(&storage, object)?;
        }
        sources.extend(batch.manifest.content.sources);
    }
    sources.extend(excluded_sources);

    let created_at = now_epoch_ms();
    let manifest = SourceCaptureManifest::draft(sources, created_at)
        .map_err(|_| "The context manifest could not be finalized safely.")?;
    let saved = storage
        .save_context_draft(&draft_id, stored_revision, &manifest, created_at)
        .map_err(|_| "The context draft changed or could not be saved locally.")?;
    context_summary(saved)
}

fn capture_context_directory(
    storage: Arc<Storage>,
    draft_id: String,
    stored_revision: Option<u64>,
    path: PathBuf,
    resource_root: &Path,
) -> Result<ContextSelectionSummary, String> {
    let existing = storage
        .load_context_draft(&draft_id)
        .map_err(|_| "The context draft could not be loaded safely.")?;
    if existing.as_ref().map(|draft| draft.revision) != stored_revision {
        return Err("The context draft changed. Review it and try again.".into());
    }

    let display_name = selected_display_name(&path);
    if classify_selected_path(&path).is_some() {
        return Err("Hidden or credential-sensitive folders cannot be captured.".into());
    }
    let metadata = fs::symlink_metadata(&path)
        .map_err(|_| "The selected folder could not be inspected safely.")?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err("The selected item is not a regular folder.".into());
    }
    let canonical =
        fs::canonicalize(&path).map_err(|_| "The selected folder could not be opened safely.")?;
    let root = Dir::open_ambient_dir(&canonical, ambient_authority())
        .map_err(|_| "The selected folder could not be opened safely.")?;
    let mut grant = SourceGrant::selected_directory(root, display_name)
        .map_err(|_| "The selected folder could not be granted safely.")?;
    let enumeration = grant
        .enumerate(CaptureLimits::default_policy())
        .map_err(|_| "The selected folder could not be inspected safely.")?;
    let existing_sources = existing
        .as_ref()
        .map(|draft| draft.manifest.content.sources.len())
        .unwrap_or_default();
    if existing_sources.saturating_add(enumeration.candidates.len()) > MAX_MANIFEST_ITEMS {
        return Err("The context draft cannot contain more than 1,000 items.".into());
    }

    if enumeration.candidates.iter().any(|candidate| {
        matches!(
            candidate.mime_type.as_deref(),
            Some("application/pdf" | "image/png" | "image/jpeg")
        )
    }) {
        extraction_helper::ensure(resource_root)?;
    }
    let directives = enumeration
        .candidates
        .into_iter()
        .map(|candidate| CaptureDirective {
            source_id: candidate.source_id,
            line_range: None,
        })
        .collect::<Vec<_>>();
    let batch = grant
        .capture_selected(&directives, CaptureLimits::default_policy(), now_epoch_ms())
        .map_err(|_| "The selected folder exceeded a capture safety limit.")?;
    for object in &batch.objects {
        store_captured_object(&storage, object)?;
    }

    let mut sources = existing
        .as_ref()
        .map(|draft| draft.manifest.content.sources.clone())
        .unwrap_or_default();
    sources.extend(batch.manifest.content.sources);
    let created_at = now_epoch_ms();
    let manifest = SourceCaptureManifest::draft(sources, created_at)
        .map_err(|_| "The context manifest could not be finalized safely.")?;
    let saved = storage
        .save_context_draft(&draft_id, stored_revision, &manifest, created_at)
        .map_err(|_| "The context draft changed or could not be saved locally.")?;
    context_summary(saved)
}

#[cfg(unix)]
pub(crate) fn pin_selected_parent(
    path: &Path,
) -> Result<(Dir, std::ffi::OsString), SourceOmissionCode> {
    pin_selected_parent_with_observer(path, || {})
}
#[cfg(unix)]
fn pin_selected_parent_with_observer(
    path: &Path,
    after_resolution: impl FnOnce(),
) -> Result<(Dir, std::ffi::OsString), SourceOmissionCode> {
    pin_selected_parent_with_component_observer(path, after_resolution, |_| {})
}
#[cfg(unix)]
fn pin_selected_parent_with_component_observer(
    path: &Path,
    after_resolution: impl FnOnce(),
    mut before_open: impl FnMut(&std::ffi::OsStr),
) -> Result<(Dir, std::ffi::OsString), SourceOmissionCode> {
    use cap_std::fs::MetadataExt;
    if let Some(code) = classify_selected_path(path) {
        return Err(code);
    }
    let name = path
        .file_name()
        .ok_or(SourceOmissionCode::AccessDenied)?
        .to_owned();
    let parent = path
        .parent()
        .ok_or(SourceOmissionCode::AccessDenied)?
        .canonicalize()
        .map_err(|_| SourceOmissionCode::AccessDenied)?;
    if let Some(code) = classify_selected_path(&parent.join(&name)) {
        return Err(code);
    }
    after_resolution();
    let mut directory = Dir::open_ambient_dir("/", ambient_authority())
        .map_err(|_| SourceOmissionCode::AccessDenied)?;
    for component in parent.components() {
        let name = match component {
            std::path::Component::RootDir => continue,
            std::path::Component::Normal(name) => name,
            _ => return Err(SourceOmissionCode::AccessDenied),
        };
        let before = directory
            .symlink_metadata(name)
            .map_err(|_| SourceOmissionCode::AccessDenied)?;
        if !before.is_dir() || before.file_type().is_symlink() {
            return Err(SourceOmissionCode::SymbolicLink);
        }
        before_open(name);
        let mut options = OpenOptions::new();
        options
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_NONBLOCK);
        let file = directory
            .open_with(name, &options)
            .map_err(|_| SourceOmissionCode::AccessDenied)?;
        let opened = file
            .metadata()
            .map_err(|_| SourceOmissionCode::AccessDenied)?;
        let after = directory
            .symlink_metadata(name)
            .map_err(|_| SourceOmissionCode::AccessDenied)?;
        if (before.dev(), before.ino()) != (opened.dev(), opened.ino())
            || (before.dev(), before.ino()) != (after.dev(), after.ino())
            || !opened.is_dir()
            || !after.is_dir()
            || after.file_type().is_symlink()
        {
            return Err(SourceOmissionCode::AccessDenied);
        }
        directory = Dir::from_std_file(file.into_std());
    }
    Ok((directory, name))
}

#[cfg(unix)]
pub(crate) fn open_selected_file(path: &Path) -> Result<cap_std::fs::File, SourceOmissionCode> {
    let (directory, filename) = pin_selected_parent(path)?;
    let metadata = directory
        .symlink_metadata(&filename)
        .map_err(|_| SourceOmissionCode::AccessDenied)?;
    if metadata.file_type().is_symlink() {
        return Err(SourceOmissionCode::SymbolicLink);
    }
    if !metadata.is_file() {
        return Err(SourceOmissionCode::SpecialFile);
    }
    let mut options = OpenOptions::new();
    options
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    use cap_std::fs::MetadataExt;
    let file = directory
        .open_with(&filename, &options)
        .map_err(|_| SourceOmissionCode::AccessDenied)?;
    let opened = file
        .metadata()
        .map_err(|_| SourceOmissionCode::AccessDenied)?;
    let after = directory
        .symlink_metadata(filename)
        .map_err(|_| SourceOmissionCode::AccessDenied)?;
    let identity = |value: &cap_std::fs::Metadata| {
        (
            value.dev(),
            value.ino(),
            value.len(),
            value.ctime(),
            value.ctime_nsec(),
        )
    };
    if identity(&metadata) != identity(&opened)
        || identity(&metadata) != identity(&after)
        || after.file_type().is_symlink()
    {
        return Err(SourceOmissionCode::AccessDenied);
    }
    Ok(file)
}

#[cfg(not(unix))]
pub(crate) fn open_selected_file(_path: &Path) -> Result<cap_std::fs::File, SourceOmissionCode> {
    Err(SourceOmissionCode::AccessDenied)
}

fn selected_display_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| {
            name.chars()
                .filter(|character| !character.is_control())
                .take(160)
                .collect()
        })
        .filter(|name: &String| !name.trim().is_empty())
        .unwrap_or_else(|| "selected item".to_owned())
}

fn omitted_source(code: SourceOmissionCode, display_name: String) -> ManifestSource {
    let (safe_name, state, detail) = match code {
        SourceOmissionCode::HiddenPath => (
            "hidden item".to_owned(),
            ManifestSourceState::Excluded,
            "hidden items are excluded from automatic capture",
        ),
        SourceOmissionCode::CredentialPath => (
            "credential-sensitive item".to_owned(),
            ManifestSourceState::Excluded,
            "credential-sensitive paths are excluded from automatic capture",
        ),
        SourceOmissionCode::SymbolicLink => (
            display_name,
            ManifestSourceState::Excluded,
            "symbolic links are not followed",
        ),
        SourceOmissionCode::SpecialFile => (
            display_name,
            ManifestSourceState::Excluded,
            "special files are not captured",
        ),
        _ => (
            display_name,
            ManifestSourceState::Failed,
            "the selected item could not be opened within its local grant",
        ),
    };
    ManifestSource {
        source_id: Uuid::new_v4().to_string(),
        display_name: safe_name,
        state,
        byte_length: None,
        mime_type: None,
        object_digest: None,
        derived_digest: None,
        representation_kind: None,
        extractor_id: None,
        extractor_version: None,
        included_locators: Vec::new(),
        omission: Some(SourceOmission {
            code,
            detail: detail.to_owned(),
        }),
        captured_at_epoch_ms: None,
        secret_pattern_findings: Vec::new(),
        secret_scan_incomplete: true,
    }
}

pub(crate) fn context_summary(
    draft: magi_storage::ContextDraft,
) -> Result<ContextSelectionSummary, String> {
    let sources = draft
        .manifest
        .safe_source_summaries()
        .map_err(|_| "The context manifest failed its safe display validation.")?
        .into_iter()
        .map(|source| {
            let status = match source.result {
                ManifestSourceState::Captured => "captured",
                ManifestSourceState::Excluded => "excluded",
                ManifestSourceState::Failed => "failed",
            };
            let representation = match source.representation_kind {
                Some(RepresentationKind::Utf8Text) => "utf8_text",
                Some(RepresentationKind::PdfText) => "pdf_text",
                Some(RepresentationKind::PdfRaster) => "pdf_raster",
                Some(RepresentationKind::Image) => "image",
                None if matches!(
                    source.omission_code,
                    Some(SourceOmissionCode::UnsupportedFormat | SourceOmissionCode::InvalidUtf8)
                ) =>
                {
                    "unsupported"
                }
                None => "unknown",
            };
            let mut issue_codes = Vec::new();
            if let Some(code) = source.omission_code {
                issue_codes.push(enum_code(&code));
            }
            issue_codes.extend(source.warning_codes.iter().map(enum_code));
            if source.secret_scan_incomplete {
                issue_codes.push("secret_scan_incomplete".to_owned());
            }
            CapturedSourceSummary {
                source_id: source.source_id,
                display_name: source.display_name,
                status,
                byte_length: source.byte_length.unwrap_or_default(),
                representation,
                digest: source
                    .object_digest
                    .map(|digest| digest.as_str().to_owned()),
                issue_codes,
            }
        })
        .collect();
    Ok(ContextSelectionSummary {
        draft_id: draft.draft_id,
        manifest_id: draft.manifest.manifest_id,
        manifest_digest: draft.manifest.digest.as_str().to_owned(),
        revision: draft.revision,
        sources,
    })
}

fn enum_code<T: Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| "unknown_issue".to_owned())
}

fn now_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

pub(crate) struct ExitAuthorization(AtomicBool);

impl ExitAuthorization {
    pub(crate) fn new() -> Self {
        Self(AtomicBool::new(false))
    }

    pub(crate) fn is_authorized(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }

    fn authorize(&self) {
        self.0.store(true, Ordering::Release);
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellContext {
    window_label: String,
    platform: &'static str,
}

#[tauri::command]
pub fn shell_context(window: WebviewWindow) -> ShellContext {
    ShellContext {
        window_label: window.label().to_owned(),
        platform: std::env::consts::OS,
    }
}

#[tauri::command]
pub fn shell_open_console(window: WebviewWindow, app: AppHandle) -> Result<(), String> {
    if window.label() != "companion" {
        return Err("This action is only available from the menu bar companion.".into());
    }
    focus_main_window(&app)?;
    if let Some(companion) = app.get_webview_window("companion") {
        let _ = companion.hide();
    }
    Ok(())
}

#[tauri::command]
pub fn shell_open_settings(window: WebviewWindow, app: AppHandle) -> Result<(), String> {
    if window.label() != "main" && window.label() != "companion" {
        return Err("This window cannot open settings.".into());
    }
    focus_main_window(&app)?;
    app.emit_to("main", "magi:open-settings", ())
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn shell_close_companion(window: WebviewWindow) -> Result<(), String> {
    if window.label() != "companion" {
        return Err("This action is only available from the menu bar companion.".into());
    }
    window.hide().map_err(|error| error.to_string())
}

#[tauri::command]
pub fn shell_request_exit(window: WebviewWindow, app: AppHandle) -> Result<(), String> {
    if window.label() != "main" && window.label() != "companion" {
        return Err("This window cannot request application exit.".into());
    }
    focus_main_window(&app)?;
    app.emit_to("main", "magi:exit-requested", ())
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn shell_confirm_exit(window: WebviewWindow, app: AppHandle) -> Result<(), String> {
    if window.label() != "main" {
        return Err("Only the main console can confirm application exit.".into());
    }
    let shutdown = async_runtime::spawn(async move {
        let state = app.state::<DesktopState>();
        let deadline = Instant::now() + Duration::from_secs(30);
        let _initialization = tokio::time::timeout_at(
            tokio::time::Instant::from_std(deadline),
            state.resource_initialization.lock(),
        )
        .await
        .map_err(|_| "Application cleanup remains unresolved.".to_owned())?;
        let permission = state
            .freeze_resource_consumers(deadline, Some((window, app.clone())))
            .await
            .map_err(|_| "Application cleanup remains unresolved.".to_owned())?;
        permission
            .validate()
            .map_err(|_| "Application cleanup remains unresolved.".to_owned())?;
        app.state::<ExitAuthorization>().authorize();
        app.exit(0);
        Ok::<_, String>(())
    });
    shutdown
        .await
        .map_err(|_| "Application cleanup remains unresolved.".to_owned())?
}

pub(crate) fn focus_main_window(app: &AppHandle) -> Result<(), String> {
    let main = match app.get_webview_window("main") {
        Some(main) => main,
        None => {
            let config = app
                .config()
                .app
                .windows
                .iter()
                .find(|window| window.label == "main")
                .ok_or_else(|| {
                    "The main console window configuration is unavailable.".to_owned()
                })?;
            match tauri::WebviewWindowBuilder::from_config(app, config)
                .and_then(|builder| builder.build())
            {
                Ok(main) => main,
                Err(_) => app
                    .get_webview_window("main")
                    .ok_or_else(|| "The main console window could not be recreated.".to_owned())?,
            }
        }
    };
    main.show()
        .map_err(|_| "The main console window could not be shown.".to_owned())?;
    main.set_focus()
        .map_err(|_| "The main console window could not be focused.".to_owned())
}

#[cfg(test)]
pub(crate) mod resource_quiescence_tests {
    use super::*;

    struct ApplicationFixture(PathBuf);
    impl Drop for ApplicationFixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn application_fixture() -> (ApplicationFixture, Arc<DesktopState>, String) {
        let (root, storage, aggregate, claim) =
            crate::profiles::catalog_selection_ipc_tests::publication_fault_fixture();
        let coordinator = NativeResourceCoordinator::cold_start();
        let state = DesktopState {
            storage: Some(Arc::new(storage)),
            storage_diagnostic: None,
            live_run_dispatcher: None,
            dispatcher_receiver: Mutex::new(None),
            provider_operations: ProviderOperationRegistry::default(),
            provider_authentications: ProviderAuthenticationRegistry::default(),
            live_run_controls: LiveRunControlRegistry::default(),
            admission_requests: AdmissionRequestRegistry {
                resources: coordinator.clone(),
                ..AdmissionRequestRegistry::default()
            },
            runtime_verification: OnceLock::new(),
            resource_initialization: async_runtime::Mutex::new(()),
            application_shutdown: Arc::new(Mutex::new(ApplicationShutdownState::Open)),
            shutdown_cleanup: async_runtime::Mutex::new(()),
            shutdown_preparation_gate: Mutex::new(None),
            resource_coordinator: coordinator,
        };
        assert_eq!(aggregate.run().run_id, claim.run_id);
        (ApplicationFixture(root), Arc::new(state), claim.run_id)
    }
    pub(crate) struct SignedInstallationFixture {
        state: Arc<DesktopState>,
        fixture: Option<ApplicationFixture>,
        pub(crate) publication: PathBuf,
    }
    impl SignedInstallationFixture {
        pub(crate) fn open() -> Self {
            let publication = PathBuf::from(
                std::env::var_os("MAGI_TEST_NATIVE_SIGNED_PUBLICATION")
                    .expect("explicit owned signed installation"),
            );
            let (fixture, state, _) = application_fixture();
            application_runtime()
                .block_on(state.ensure_installed_resources(
                    publication.clone(),
                    fixture.0.clone(),
                    publication.join("provider/codex-acp/darwin-arm64/codex-acp"),
                    VerificationRequest::until(Instant::now() + Duration::from_secs(60)),
                ))
                .unwrap();
            Self {
                state,
                fixture: Some(fixture),
                publication,
            }
        }
        pub(crate) fn finish(mut self) {
            self.close().expect("actual signed extraction cleanup");
        }
        fn close(&mut self) -> Result<(), magi_provider::ProviderError> {
            if self.fixture.is_some() {
                let permission = application_runtime().block_on(
                    self.state
                        .freeze_resource_consumers(Instant::now() + Duration::from_secs(30), None),
                )?;
                permission.validate()?;
                self.fixture.take();
            }
            Ok(())
        }
    }
    impl Drop for SignedInstallationFixture {
        fn drop(&mut self) {
            if self.close().is_err()
                && let Some(fixture) = self.fixture.take()
            {
                std::mem::forget(fixture);
            }
        }
    }
    fn application_runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    #[test]
    fn application_sqlite_preparation_timeout_retains_worker_and_same_epoch_retry() {
        let (_fixture, state, run_id) = application_fixture();
        let storage = state.storage().unwrap();
        let (ready_tx, ready_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        *state.shutdown_preparation_gate.lock().unwrap() = Some((ready_tx, release_rx));
        let caller_state = state.clone();
        let caller = std::thread::spawn(move || {
            application_runtime().block_on(
                caller_state
                    .prepare_application_shutdown(Instant::now() + Duration::from_millis(20), true),
            )
        });
        ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(matches!(
            caller.join().unwrap(),
            Err(magi_provider::ProviderError::Timeout)
        ));
        assert_eq!(
            state.resource_coordinator.state.lock().unwrap().operations,
            1
        );
        assert!(matches!(
            *state.application_shutdown.lock().unwrap(),
            ApplicationShutdownState::Preparing
        ));
        assert!(matches!(
            application_runtime().block_on(
                state.prepare_application_shutdown(Instant::now() + Duration::from_secs(5), true)
            ),
            Err(magi_provider::ProviderError::Timeout)
        ));
        assert_eq!(
            storage.get_live_run_snapshot(&run_id, 0).unwrap().status,
            magi_storage::LiveRunStatus::Running
        );
        release_tx.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let ready = matches!(
                *state.application_shutdown.lock().unwrap(),
                ApplicationShutdownState::Frozen(_)
            ) && state.resource_coordinator.state.lock().unwrap().operations == 0;
            if ready {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "actual preparation worker did not settle"
            );
            std::thread::yield_now();
        }
        let prepared = application_runtime()
            .block_on(state.prepare_application_shutdown(deadline, true))
            .unwrap();
        assert_eq!(prepared.permission.epoch, 1);
        assert_eq!(prepared.pending_runs, vec![run_id.clone()]);
        assert!(prepared.durable_cancellation_prepared);
        assert!(prepared.permission.validate().is_ok());
        let retried = application_runtime()
            .block_on(state.freeze_resource_consumers(deadline, None))
            .unwrap();
        assert_eq!(retried.epoch, prepared.permission.epoch);
        assert!(retried.validate().is_ok());
        assert!(state.resource_coordinator.preflight_operation().is_err());
        assert_eq!(
            storage.get_live_run_snapshot(&run_id, 0).unwrap().status,
            magi_storage::LiveRunStatus::Cancelling
        );
        assert_eq!(
            storage.load_run_aggregate(&run_id).unwrap().run().run_id,
            run_id
        );
    }

    #[test]
    fn application_sqlite_cross_purpose_retry_adds_durable_cancel_without_new_epoch() {
        let (_fixture, state, run_id) = application_fixture();
        let runtime = application_runtime();
        let deadline = Instant::now() + Duration::from_secs(5);
        let original = runtime
            .block_on(state.prepare_application_shutdown(deadline, false))
            .unwrap();
        assert!(!original.durable_cancellation_prepared);
        assert!(original.pending_runs.is_empty());
        let upgraded = runtime
            .block_on(state.prepare_application_shutdown(deadline, true))
            .unwrap();
        assert!(upgraded.durable_cancellation_prepared);
        assert_eq!(original.permission.epoch, upgraded.permission.epoch);
        assert_eq!(upgraded.pending_runs, vec![run_id.clone()]);
        assert_eq!(
            state
                .storage()
                .unwrap()
                .get_live_run_snapshot(&run_id, 0)
                .unwrap()
                .status,
            magi_storage::LiveRunStatus::Cancelling
        );
        assert!(state.resource_coordinator.enter(None).is_err());
    }

    #[test]
    fn application_retry_rejects_stale_frozen_epoch_without_reissuing_authority() {
        let (_fixture, state, run_id) = application_fixture();
        let runtime = application_runtime();
        let deadline = Instant::now() + Duration::from_secs(5);
        let original = runtime
            .block_on(state.prepare_application_shutdown(deadline, true))
            .unwrap();
        state.resource_coordinator.state.lock().unwrap().epoch += 1;
        assert!(
            runtime
                .block_on(state.prepare_application_shutdown(deadline, true))
                .is_err()
        );
        assert!(original.permission.validate().is_err());
        assert!(state.resource_coordinator.enter(None).is_err());
        assert_eq!(
            state
                .storage()
                .unwrap()
                .get_live_run_snapshot(&run_id, 0)
                .unwrap()
                .status,
            magi_storage::LiveRunStatus::Cancelling
        );
    }

    #[test]
    #[ignore = "Requires explicit owned signed installation inputs; no signing, publication, account calls, or document input."]
    fn application_signed_cache_timeout_release_and_retry_preserve_frozen_epoch() {
        let publication = PathBuf::from(
            std::env::var_os("MAGI_TEST_NATIVE_SIGNED_PUBLICATION")
                .expect("explicit owned signed installation"),
        );
        let executable = publication.join("provider/codex-acp/darwin-arm64/codex-acp");
        let (_fixture, state, run_id) = application_fixture();
        let runtime = application_runtime();
        let startup = VerificationRequest::until(Instant::now() + Duration::from_secs(60));
        let service = runtime
            .block_on(state.ensure_installed_resources(
                publication.clone(),
                _fixture.0.clone(),
                executable.clone(),
                startup.clone(),
            ))
            .unwrap();
        let request = VerificationRequest::until(Instant::now() + Duration::from_secs(60));
        let operation = state
            .resource_coordinator
            .enter(Some(request.clone()))
            .unwrap();
        let artifact = runtime
            .block_on(service.verify(executable, request.clone()))
            .unwrap();
        let stream = request.track_stream_operation().unwrap();
        let (ready_tx, ready_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let _operation = operation;
            let _stream = stream;
            ready_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let first = runtime.block_on(
            state.freeze_resource_consumers(Instant::now() + Duration::from_millis(20), None),
        );
        assert!(matches!(first, Err(magi_provider::ProviderError::Timeout)));
        assert_eq!(request.settlement().provider_operations, 1);
        assert_eq!(
            state.resource_coordinator.state.lock().unwrap().operations,
            1
        );
        let epoch = match &*state.application_shutdown.lock().unwrap() {
            ApplicationShutdownState::Frozen(prepared) => {
                assert!(!prepared.permission.consumers_closed);
                prepared.permission.epoch
            }
            _ => panic!("frozen shutdown authority was not retained"),
        };
        release_tx.send(()).unwrap();
        worker.join().unwrap();
        assert!(request.observed_revoked_settlement());
        let deadline = Instant::now() + Duration::from_secs(5);
        let prepared = runtime
            .block_on(state.prepare_application_shutdown(deadline, true))
            .unwrap();
        assert_eq!(prepared.permission.epoch, epoch);
        assert_eq!(prepared.pending_runs, vec![run_id.clone()]);
        let cache_held = runtime.block_on(state.freeze_resource_consumers(deadline, None));
        assert!(cache_held.is_err());
        assert_eq!(request.settlement().provider_operations, 0);
        assert!(state.resource_coordinator.enter(None).is_err());
        drop(artifact);
        let stopped = runtime
            .block_on(state.freeze_resource_consumers(deadline, None))
            .unwrap();
        assert_eq!(stopped.epoch, epoch);
        assert!(stopped.validate().is_ok());
        assert!(request.observed_revoked_settlement());
        let namespace = magi_provider::resource_custody::ArtifactNamespace::development(
            Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap(),
            magi_provider::resource_custody::DevelopmentProfile::Debug,
        )
        .unwrap();
        let journal = magi_provider::resource_custody::ResourceCustody::open(namespace).unwrap();
        assert_eq!(journal.unresolved_operations().unwrap(), 0);
        assert_eq!(
            state
                .storage()
                .unwrap()
                .get_live_run_snapshot(&run_id, 0)
                .unwrap()
                .status,
            magi_storage::LiveRunStatus::Cancelling
        );
    }

    #[test]
    fn retired_epoch_cannot_be_reinterpreted_as_cold_initialization() {
        let coordinator = NativeResourceCoordinator::default();
        let frozen = coordinator.freeze(Vec::new()).unwrap();
        assert_eq!(frozen.epoch, 1);
        assert!(coordinator.cold_permission().is_err());
        assert!(coordinator.preflight_operation().is_err());
    }

    #[test]
    fn guarded_mint_does_not_reacquire_its_resource_mutex() {
        struct Fixture(PathBuf);
        impl Drop for Fixture {
            fn drop(&mut self) {
                fs::remove_dir_all(&self.0).unwrap();
            }
        }
        let fixture =
            Fixture(std::env::temp_dir().join(format!("native-guarded-mint-{}", Uuid::new_v4())));
        let storage = Arc::new(Storage::open_or_create(&fixture.0).unwrap());
        let registry = AdmissionRequestRegistry::default();
        let operation = registry.resources.enter(None).unwrap();
        let binding = magi_storage::AdmissionRequestBinding {
            command_id: "guarded-mint".into(),
            idempotency_key: "guarded-mint-key".into(),
            intent_digest: magi_domain::Digest::from_bytes(b"guarded-mint-intent"),
        };
        let token = operation
            .admit(|admission| {
                registry.issue_for_execution_admitted(
                    admission,
                    binding.clone(),
                    None,
                    storage.clone(),
                )
            })
            .unwrap()
            .unwrap();
        assert!(
            registry
                .resolve(
                    &AdmissionAuthorityDto {
                        token: token.token,
                        process_epoch: token.process_epoch,
                    },
                    &binding
                )
                .is_ok()
        );
        drop(operation);
        drop(storage);
    }

    #[test]
    fn registration_and_dispatcher_share_resource_then_live_control_order() {
        let coordinator = NativeResourceCoordinator::default();
        let controls = LiveRunControlRegistry::default();
        let registration = coordinator.enter(None).unwrap();
        let dispatcher = coordinator.enter(None).unwrap();
        let (held_tx, held_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let registration_controls = controls.clone();
        let register = std::thread::spawn(move || {
            registration
                .admit(|_| {
                    held_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    let _live = registration_controls.coordinate();
                })
                .unwrap();
        });
        held_rx.recv().unwrap();
        let dispatch = std::thread::spawn(move || {
            entered_tx.send(()).unwrap();
            dispatcher.admit(|_| {
                let _live = controls.coordinate();
            })
        });
        entered_rx.recv().unwrap();
        let freeze_coordinator = coordinator.clone();
        let (freeze_tx, freeze_rx) = std::sync::mpsc::channel();
        let freeze = std::thread::spawn(move || {
            freeze_tx.send(()).unwrap();
            freeze_coordinator.freeze(Vec::new()).unwrap()
        });
        freeze_rx.recv().unwrap();
        release_tx.send(()).unwrap();
        register.join().unwrap();
        let dispatch_result = dispatch.join().unwrap();
        assert!(
            dispatch_result.is_ok()
                || matches!(
                    dispatch_result,
                    Err(magi_provider::ProviderError::Cancelled)
                )
        );
        let frozen = freeze.join().unwrap();
        assert!(frozen.check_settlement().is_ok());
        assert!(coordinator.enter(None).is_err());
    }

    #[test]
    fn abandoned_caller_cannot_settle_a_held_native_worker() {
        let coordinator = NativeResourceCoordinator::default();
        let operation = coordinator.enter(None).unwrap();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let (result_tx, result_rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let _operation = operation;
            ready_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            let _ = result_tx.send(());
        });
        ready_rx.recv().unwrap();
        drop(result_rx);
        let frozen = coordinator.freeze(Vec::new()).unwrap();
        assert!(frozen.check_settlement().is_err());
        release_tx.send(()).unwrap();
        worker.join().unwrap();
        assert!(frozen.check_settlement().is_ok());
        assert!(frozen.validate().is_err());
    }

    #[test]
    fn application_shutdown_persists_before_revocation_and_blocks_new_admission() {
        let coordinator = NativeResourceCoordinator::default();
        let root = VerificationRequest::until(Instant::now() + Duration::from_secs(5));
        let operation = coordinator.enter(Some(root.clone())).unwrap();
        let (permission, persisted) = coordinator
            .freeze_with_prepare(Vec::new(), || {
                assert!(root.check().is_ok());
                Ok("durable cancellation")
            })
            .unwrap();
        assert_eq!(persisted, "durable cancellation");
        assert!(root.check().is_err());
        assert!(operation.admit(|_| ()).is_err());
        assert!(permission.check_settlement().is_err());
        drop(operation);
        assert!(permission.check_settlement().is_ok());
        assert!(permission.validate().is_err());
    }

    #[test]
    fn failed_shutdown_publication_does_not_authorize_exit_or_claim_revocation() {
        let coordinator = NativeResourceCoordinator::default();
        let root = VerificationRequest::until(Instant::now() + Duration::from_secs(5));
        let operation = coordinator.enter(Some(root.clone())).unwrap();
        assert!(
            coordinator
                .freeze_with_prepare(Vec::new(), || Err::<(), _>(
                    magi_provider::ProviderError::ArtifactVerification
                ))
                .is_err()
        );
        assert!(root.check().is_ok());
        assert!(operation.check().is_ok());
    }

    #[test]
    fn never_issued_cold_shutdown_permanently_closes_initialization() {
        let coordinator = NativeResourceCoordinator::cold_start();
        let operation = coordinator.preflight_operation().unwrap();
        let (permission, ()) = coordinator
            .cold_shutdown_with_prepare(&operation, || Ok(()))
            .unwrap();
        assert!(permission.validate().is_err());
        drop(operation);
        assert!(permission.validate().is_ok());
        assert!(
            magi_provider::resource_custody::RetainedNativeQuiescence::verified_extraction(
                &permission
            )
            .is_err()
        );
        assert!(coordinator.preflight_operation().is_err());
        assert!(coordinator.cold_permission().is_err());
    }

    #[test]
    fn abandoned_cold_shutdown_caller_retains_the_real_preparation_worker() {
        let coordinator = NativeResourceCoordinator::cold_start();
        let operation = coordinator.preflight_operation().unwrap();
        let worker_coordinator = coordinator.clone();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let (result_tx, result_rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let permission = worker_coordinator
                .cold_shutdown_with_prepare(&operation, || {
                    ready_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    Ok(())
                })
                .unwrap()
                .0;
            assert!(permission.validate().is_err());
            drop(operation);
            assert!(permission.validate().is_ok());
            let _ = result_tx.send(permission);
        });
        ready_rx.recv().unwrap();
        drop(result_rx);
        assert!(coordinator.state.try_lock().is_err());
        release_tx.send(()).unwrap();
        worker.join().unwrap();
        assert_eq!(coordinator.state.lock().unwrap().operations, 0);
        assert!(coordinator.preflight_operation().is_err());
    }

    #[test]
    fn freeze_winning_prevents_the_actual_admission_callback() {
        let coordinator = NativeResourceCoordinator::default();
        let operation = coordinator.enter(None).unwrap();
        let frozen = coordinator.freeze(Vec::new()).unwrap();
        let called = AtomicBool::new(false);
        assert!(
            operation
                .admit(|_| called.store(true, Ordering::SeqCst))
                .is_err()
        );
        assert!(!called.load(Ordering::SeqCst));
        assert!(coordinator.enter(None).is_err());
        drop(operation);
        assert!(frozen.check_settlement().is_ok());
    }
}

#[cfg(test)]
mod dossier_authentication_command_tests {
    use super::*;
    use magi_domain::*;
    use magi_storage::{LiveRunAdmissionOutcome, LiveRunAdmissionRequest};

    #[test]
    fn clarification_draft_projection_exposes_only_safe_context_and_closed_inputs() {
        let parent = magi_storage::ClarificationParentReference {
            run_id: "parent".into(),
            revision: 7,
            input_digest: Digest::from_bytes(b"parent-input"),
            generation: 3,
        };
        let manifest = SourceCaptureManifest::draft(Vec::new(), 1).unwrap();
        let manifest_id = manifest.manifest_id.clone();
        let view = clarification_draft_view(magi_storage::ClarificationDraft {
            parent: parent.clone(),
            context: magi_storage::ContextDraft {
                draft_id: "child".into(),
                revision: 2,
                manifest,
                updated_at_epoch_ms: 1,
            },
            question: "Changed question".into(),
            execution_authority: magi_storage::AdmissionExecutionAuthority {
                lineage_id: "private-lineage".into(),
                store_generation: 3,
                active: true,
            },
        })
        .unwrap();
        let value = serde_json::to_value(view).unwrap();
        assert_eq!(value["schemaVersion"], 1);
        assert_eq!(value["parent"], serde_json::to_value(&parent).unwrap());
        assert_eq!(value["context"]["draftId"], "child");
        assert_eq!(value["context"]["revision"], 2);
        assert_eq!(
            value["context"]["manifestId"],
            serde_json::to_value(manifest_id).unwrap()
        );
        assert_eq!(value["context"]["sources"], serde_json::json!([]));
        assert_eq!(value.as_object().unwrap().len(), 4);
        assert_eq!(value["context"].as_object().unwrap().len(), 5);
        assert!(!value.to_string().contains("private-lineage"));
        let mut create = serde_json::json!({"parent": parent});
        assert!(serde_json::from_value::<CreateClarificationDraftInput>(create.clone()).is_ok());
        create["executionAuthority"] = serde_json::json!({"active": true});
        assert!(serde_json::from_value::<CreateClarificationDraftInput>(create).is_err());
        let save = serde_json::json!({"draftId":"child","expectedRevision":2,"question":"changed"});
        assert!(serde_json::from_value::<SaveClarificationQuestionInput>(save.clone()).is_ok());
        let mut invalid = save;
        invalid["parent"] = serde_json::json!({});
        assert!(serde_json::from_value::<SaveClarificationQuestionInput>(invalid).is_err());
        assert!(
            serde_json::from_value::<DiscardClarificationDraftInput>(
                serde_json::json!({"draftId":"child","expectedRevision":-1})
            )
            .is_err()
        );
    }

    fn native_binding(id: &str) -> magi_storage::AdmissionRequestBinding {
        magi_storage::AdmissionRequestBinding {
            command_id: id.into(),
            idempotency_key: id.into(),
            intent_digest: Digest::from_bytes(id.as_bytes()),
        }
    }
    #[test]
    fn issued_capabilities_reject_unknown_epoch_changed_intent_and_expired_execution() {
        let registry = AdmissionRequestRegistry::default();
        let binding = native_binding("issued");
        let issued = registry.issue(binding.clone()).unwrap();
        let capability = AdmissionAuthorityDto {
            token: issued.token.clone(),
            process_epoch: issued.process_epoch.clone(),
        };
        assert!(registry.resolve(&capability, &binding).is_ok());
        let mut changed = capability.clone();
        changed.token = Uuid::new_v4().to_string();
        assert!(registry.resolve(&changed, &binding).is_err());
        changed = capability.clone();
        changed.process_epoch = Uuid::new_v4().to_string();
        assert!(registry.resolve(&changed, &binding).is_err());
        assert!(
            registry
                .resolve(&capability, &native_binding("changed"))
                .is_err()
        );
        assert!(
            registry
                .cancellation_authorities(None, "issued", "issued")
                .is_err()
        );
        assert!(
            registry
                .cancellation_authorities(Some(&capability), "issued", "issued")
                .is_ok()
        );
        let mut map = registry.requests.lock().unwrap();
        let request = map.get_mut(&issued.token).unwrap();
        Arc::get_mut(request).unwrap().expires = Instant::now();
        drop(map);
        assert!(registry.resolve(&capability, &binding).is_err());
        assert!(
            registry
                .cancellation_authorities(Some(&capability), "issued", "issued")
                .is_ok()
        );
    }
    #[test]
    fn settled_terminal_registry_churn_retires_but_owned_operations_and_active_runs_do_not() {
        let registry = AdmissionRequestRegistry::default();
        for index in 0..ADMISSION_REQUEST_LIMIT + 8 {
            let binding = native_binding(&format!("terminal-{index}"));
            let issued = registry.issue(binding.clone()).unwrap();
            let authority = registry
                .resolve(
                    &AdmissionAuthorityDto {
                        token: issued.token,
                        process_epoch: issued.process_epoch,
                    },
                    &binding,
                )
                .unwrap();
            authority.state.lock().unwrap().terminal = true;
            authority.revoke();
        }
        assert!(registry.requests.lock().unwrap().len() <= 1);
        let binding = native_binding("held");
        let issued = registry.issue(binding.clone()).unwrap();
        let capability = AdmissionAuthorityDto {
            token: issued.token.clone(),
            process_epoch: issued.process_epoch,
        };
        let authority = registry.resolve(&capability, &binding).unwrap();
        let lease = authority.lease().unwrap();
        authority.state.lock().unwrap().terminal = true;
        authority.revoke();
        registry.issue(native_binding("other")).unwrap();
        assert!(
            registry
                .requests
                .lock()
                .unwrap()
                .contains_key(&issued.token)
        );
        drop(lease);
        registry.issue(native_binding("next")).unwrap();
        assert!(
            !registry
                .requests
                .lock()
                .unwrap()
                .contains_key(&issued.token)
        );
        let binding = native_binding("active");
        let issued = registry.issue(binding.clone()).unwrap();
        {
            let mut map = registry.requests.lock().unwrap();
            let request = Arc::get_mut(map.get_mut(&issued.token).unwrap()).unwrap();
            request.expires = Instant::now();
            request.state.lock().unwrap().admitted_run = Some("actual-run".into());
        }
        registry.issue(native_binding("after-active")).unwrap();
        assert!(
            registry
                .requests
                .lock()
                .unwrap()
                .contains_key(&issued.token)
        );
    }

    #[test]
    fn captured_authority_cannot_acquire_new_operation_after_retirement() {
        let registry = AdmissionRequestRegistry::default();
        let binding = native_binding("retire-race");
        let issued = registry.issue(binding.clone()).unwrap();
        let captured = registry
            .resolve(
                &AdmissionAuthorityDto {
                    token: issued.token.clone(),
                    process_epoch: issued.process_epoch,
                },
                &binding,
            )
            .unwrap();
        captured.state.lock().unwrap().terminal = true;
        captured.revoke();
        registry.issue(native_binding("trigger-retire")).unwrap();
        assert!(
            !registry
                .requests
                .lock()
                .unwrap()
                .contains_key(&issued.token)
        );
        assert!(captured.lease().is_err());
        assert!(captured.effect_request().is_err());
        assert_eq!(captured.operations.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn cancellation_before_registration_fences_late_start_and_effects() {
        let registry = AdmissionRequestRegistry::default();
        let cancelled = registry.reference("pending-before-start").unwrap();
        assert!(cancelled.revoke().is_none());
        let late = registry.reference("pending-before-start").unwrap();
        assert!(Arc::ptr_eq(&cancelled, &late));
        assert!(matches!(
            late.check(),
            Err(magi_provider::ProviderError::Cancelled)
        ));
        assert!(late.effect_request().is_err());
    }

    #[test]
    fn admission_commit_and_revocation_are_ordered_in_both_directions() {
        for commit_wins in [false, true] {
            let registry = AdmissionRequestRegistry::default();
            let authority = registry.reference("ordered-request").unwrap();
            if !commit_wins {
                authority.revoke();
                let state = authority.state.lock().unwrap();
                assert!(authority.check_locked(&state).is_err());
                assert!(state.admitted_run.is_none());
            } else {
                let entered = Arc::new(std::sync::Barrier::new(2));
                let finished = Arc::new(std::sync::Barrier::new(2));
                let committing = authority.clone();
                let producer = {
                    let entered = entered.clone();
                    let finished = finished.clone();
                    std::thread::spawn(move || {
                        let mut state = committing.state.lock().unwrap();
                        committing.check_locked(&state).unwrap();
                        entered.wait();
                        finished.wait();
                        state.admitted_run = Some("actual-committed-run-reference".into());
                    })
                };
                entered.wait();
                let revoking = authority.clone();
                let revoker = std::thread::spawn(move || revoking.revoke());
                finished.wait();
                producer.join().unwrap();
                assert_eq!(
                    revoker.join().unwrap().as_deref(),
                    Some("actual-committed-run-reference")
                );
                assert!(authority.check().is_err());
                assert!(authority.effect_request().is_err());
            }
        }
    }

    #[test]
    fn admission_deadline_is_checked_at_commit_and_does_not_become_run_lifetime() {
        let root = std::env::temp_dir().join(format!("run-lifetime-{}", Uuid::new_v4()));
        let storage = Arc::new(Storage::open_or_create(&root).unwrap());
        let execution = NativeExecutionAuthority::capture(storage).unwrap();
        let authority = AdmissionRequestAuthority {
            verification: VerificationRequest::until(Instant::now()),
            state: Arc::new(Mutex::new(AdmissionRequestState::default())),
            binding: magi_storage::AdmissionRequestBinding {
                command_id: "test".into(),
                idempotency_key: "test".into(),
                intent_digest: Digest::from_bytes(b"test"),
            },
            expires: Instant::now(),
            expires_at: crate::profiles::admission_expiry(),
            operations: Arc::new(AtomicUsize::new(0)),
            resources: NativeResourceCoordinator::default(),
        };
        let mut state = authority.state.lock().unwrap();
        assert!(matches!(
            authority.check_locked(&state),
            Err(magi_provider::ProviderError::Timeout)
        ));
        state.admitted_run = Some("committed-run".into());
        state.execution = Some(execution.clone());
        assert!(authority.check_locked(&state).is_ok());
        drop(state);
        let effects = std::array::from_fn::<_, 3, _>(|_| authority.effect_request().unwrap());
        authority.revoke();
        for effect in effects {
            assert!(matches!(
                effect.check(),
                Err(magi_provider::ProviderError::Cancelled)
            ));
        }
        let controls = LiveRunControlRegistry::default();
        let direct = controls.register("direct");
        direct.bind_execution(execution).unwrap();
        let effects = std::array::from_fn::<_, 3, _>(|_| direct.effect_request().unwrap());
        direct.revoke_effects();
        for effect in effects {
            assert!(matches!(
                effect.check(),
                Err(magi_provider::ProviderError::Cancelled)
            ));
        }
    }

    #[test]
    fn captured_lineage_mismatch_revokes_every_derived_effect_and_missing_capture_is_denied() {
        let root = std::env::temp_dir().join(format!("native-lineage-{}", Uuid::new_v4()));
        let storage = Storage::open_or_create(&root).unwrap();
        let old = storage.admission_execution_authority().unwrap();
        drop(storage);
        let storage = Arc::new(Storage::open_or_create(&root).unwrap());
        let current = storage.admission_execution_authority().unwrap();
        assert!(current.store_generation > old.store_generation);
        let registry = AdmissionRequestRegistry::default();
        let binding = native_binding("lineage-issued");
        let lifecycle = AdmissionRequestLifecycle::until(Instant::now() + Duration::from_secs(30));
        let issued = registry
            .issue_for_execution(binding.clone(), Some(&lifecycle), storage.clone())
            .unwrap();
        let authority = registry
            .resolve(
                &AdmissionAuthorityDto {
                    token: issued.token,
                    process_epoch: issued.process_epoch,
                },
                &binding,
            )
            .unwrap();
        let effects: Vec<_> = (0..3)
            .map(|_| authority.effect_request().unwrap())
            .collect();
        authority.state.lock().unwrap().execution = Some(NativeExecutionAuthority {
            storage: storage.clone(),
            expected: old.clone(),
        });
        assert!(authority.effect_request().is_err());
        assert!(effects.iter().all(|effect| effect.check().is_err()));
        assert!(authority.state.lock().unwrap().revoked);
        let controls = LiveRunControlRegistry::default();
        let missing = controls.register("missing-capture");
        assert!(missing.effect_request().is_err());
        let direct = controls.register("direct-lineage");
        direct
            .bind_execution(NativeExecutionAuthority::capture(storage.clone()).unwrap())
            .unwrap();
        let direct_effect = direct.effect_request().unwrap();
        *direct.execution.lock().unwrap() = Some(NativeExecutionAuthority {
            storage,
            expected: old,
        });
        assert!(direct.effect_request().is_err());
        assert!(direct_effect.check().is_err());
    }

    #[test]
    fn sibling_namespace_churn_does_not_change_pinned_directory_authority() {
        let root = std::env::temp_dir().join(format!("selected-siblings-{}", Uuid::new_v4()));
        let selected = root.join("Public");
        fs::create_dir_all(&selected).unwrap();
        fs::write(selected.join("Selected.txt"), b"selected bytes").unwrap();
        let (parent, name) = pin_selected_parent_with_component_observer(
            &selected.join("Selected.txt"),
            || {},
            |component| {
                if component == std::ffi::OsStr::new("Public") {
                    fs::write(selected.join("Sibling.txt"), b"unrelated sibling").unwrap();
                }
            },
        )
        .unwrap();
        assert_eq!(parent.read(&name).unwrap(), b"selected bytes");
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn replaced_component_between_metadata_and_open_never_acquires_new_directory() {
        let root = std::env::temp_dir().join(format!("selected-component-{}", Uuid::new_v4()));
        let selected = root.join("Public");
        fs::create_dir_all(&selected).unwrap();
        fs::write(selected.join("Selected.txt"), b"selected bytes").unwrap();
        let result = pin_selected_parent_with_component_observer(
            &selected.join("Selected.txt"),
            || {},
            |component| {
                if component == std::ffi::OsStr::new("Public") {
                    fs::rename(&selected, root.join("Old")).unwrap();
                    fs::create_dir(&selected).unwrap();
                    fs::write(selected.join("Selected.txt"), b"replacement").unwrap();
                }
            },
        );
        assert!(result.is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn replaced_ancestor_cannot_redirect_selected_capability_to_credentials() {
        let root = std::env::temp_dir().join(format!("selected-ancestor-{}", Uuid::new_v4()));
        let selected = root.join("Public");
        let credential = root.join(".codex");
        fs::create_dir_all(&selected).unwrap();
        fs::create_dir_all(&credential).unwrap();
        fs::write(selected.join("Selected.pdf"), b"public selected bytes").unwrap();
        fs::write(credential.join("Selected.pdf"), b"private_token_canary").unwrap();
        let path = selected.join("Selected.pdf");
        let result = pin_selected_parent_with_observer(&path, || {
            fs::rename(&selected, root.join("Original")).unwrap();
            std::os::unix::fs::symlink(&credential, &selected).unwrap();
        });
        assert!(result.is_err());
        assert_eq!(
            fs::read(credential.join("Selected.pdf")).unwrap(),
            b"private_token_canary"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn malformed_derived_capture_digest_cannot_publish_manifest_authority() {
        let root = std::env::temp_dir().join(format!("capture-digest-{}", Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let path = root.join("Public.txt");
        fs::write(&path, b"public captured text").unwrap();
        let storage = Storage::open_or_create(root.join("store")).unwrap();
        let mut grant = SourceGrant::selected_files(vec![(
            open_selected_file(&path).unwrap(),
            "Public.txt".into(),
        )])
        .unwrap();
        let enumeration = grant.enumerate(CaptureLimits::default_policy()).unwrap();
        let directives = enumeration
            .candidates
            .into_iter()
            .map(|candidate| CaptureDirective {
                source_id: candidate.source_id,
                line_range: None,
            })
            .collect::<Vec<_>>();
        let mut batch = grant
            .capture_selected(&directives, CaptureLimits::default_policy(), now_epoch_ms())
            .unwrap();
        batch.objects[0].derived_digest = Digest::from_bytes(b"different representation");
        assert!(store_captured_object(&storage, &batch.objects[0]).is_err());
        assert!(storage.load_context_draft("draft").unwrap().is_none());
        drop(storage);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn persisted_dossier_command_projects_exact_authentication_binding_after_reopen() {
        for code in ["authentication_status_rpc_failed", "provider_timeout"] {
            let root =
                std::env::temp_dir().join(format!("magi-dossier-command-{}", Uuid::new_v4()));
            let storage = Storage::open_or_create(&root).unwrap();
            let at = "2026-10-01T00:00:00Z";
            let profile = storage
                .save_provider_profile(
                    &ProviderProfileInput {
                        provider_profile_id: "fixture-profile".into(),
                        provider_id: "codex-acp".into(),
                        display_name: "Fixture".into(),
                        account_alias: "Fixture".into(),
                        authentication_method: ProviderAuthenticationMethod::LocalSubscription,
                        secret_reference: None,
                        runtime_home_id: "runtime-fixture".into(),
                        credential_home: Some(ProviderCredentialHome {
                            authority_id: "authority-fixture".into(),
                            canonical_path: "/Users/fixture/auth".into(),
                            device: 1,
                            inode: 2,
                            credential_store: ProviderCredentialStore::File,
                            account_digest: Some(Digest::from_bytes(b"fixture-account")),
                        }),
                    },
                    None,
                    at,
                )
                .unwrap();
            let catalog = ProviderCatalogSnapshot::new(
                ProviderCatalogInput {
                    catalog_snapshot_id: "catalog-fixture".into(),
                    provider_id: profile.provider_id.clone(),
                    provider_profile_id: profile.provider_profile_id.clone(),
                    profile_revision: profile.revision,
                    adapter_id: "codex-acp".into(),
                    adapter_version: "1".into(),
                    adapter_digest: Digest::from_bytes(b"adapter"),
                    fetched_at: at.into(),
                },
                vec![ProviderCatalogModel {
                    model_id: "model".into(),
                    name: None,
                    description: None,
                    context_window_tokens: None,
                    max_output_tokens: None,
                }],
            )
            .unwrap()
            .with_negotiated_modes(NegotiatedModeState {
                current_mode_id: None,
                modes: vec![],
            })
            .unwrap()
            .with_artifact_set_digest(Digest::from_bytes(b"verified-runtime-set-fixture"))
            .unwrap();
            storage.save_provider_catalog_snapshot(&catalog).unwrap();
            let binding =
                AcpModelBindingSnapshot::from_catalog_with_mode(&catalog, "model", None).unwrap();
            let request = LiveRunAdmissionRequest {
                command_id: "live-command".into(),
                idempotency_key: "live-key".into(),
                question: "Review fixture".into(),
                model_binding: binding.clone(),
            };
            let run_id = match storage.admit_live_run(&request).unwrap() {
                LiveRunAdmissionOutcome::Accepted { receipt, .. } => receipt.run_id,
                _ => panic!("admission"),
            };
            let roles = CoreId::ALL.map(|core_id| CoreRoleProfile {
                core_id,
                profile_id: format!("role-{}", core_id.wire_name()),
                revision: 0,
                display_name: core_id.wire_name().into(),
                review_purpose: "Review evidence".into(),
                evaluation_criteria: vec![],
                falsification_questions: vec![],
                response_language: "en".into(),
                binding: ModelBindingSnapshot {
                    provider_profile_id: profile.provider_profile_id.clone(),
                    revision: profile.revision,
                    adapter_id: "codex-acp".into(),
                    adapter_version: "1".into(),
                    adapter_digest: catalog.adapter_digest.clone(),
                    model_id: "model".into(),
                    context_window_tokens: None,
                    maximum_output_tokens: None,
                },
                catalog_binding: Some(binding.clone()),
            });
            let input = InputSnapshot::new(
                QuestionSnapshot::new(
                    "question".into(),
                    QuestionKind::Answer,
                    request.question.clone(),
                    vec![],
                    vec![],
                    vec![],
                )
                .unwrap(),
                ContextManifest::new("manifest".into(), vec![]).unwrap(),
                RoleSetSnapshot::new("roles".into(), roles).unwrap(),
                Digest::from_bytes(b"policy"),
            )
            .unwrap();
            let run = Run::new(
                run_id.clone(),
                "conversation".into(),
                None,
                &input,
                at.into(),
            )
            .unwrap();
            let mut aggregate = RunAggregate::new(run, input).unwrap();
            let command = CommandEnvelope {
                command_id: "domain-command".into(),
                idempotency_key: "domain-key".into(),
                command_kind: CommandKind::CreateRun,
                target_id: run_id.clone(),
                expected_revision: 0,
                payload_digest: aggregate.input().input_digest.clone(),
            };
            storage
                .commit_run(&command, &mut aggregate, at, None)
                .unwrap();
            let claim = storage.claim_next_live_run("worker", at).unwrap().unwrap();
            storage
                .mark_live_run_session_creation_intent(&claim, at)
                .unwrap();
            storage.mark_live_run_running(&claim, at).unwrap();
            let revision = aggregate.run().revision;
            aggregate.fail(code.into(), revision, at.into()).unwrap();
            storage
                .commit_live_run_aggregate_transition(&claim, &mut aggregate, revision)
                .unwrap();
            let expected = AuthFailureProfileBinding {
                provider_profile_id: profile.provider_profile_id.clone(),
                profile_revision: profile.revision,
            };
            storage
                .fail_live_run(
                    &claim,
                    &LiveRunFailure {
                        code: code.into(),
                        detail: "Verification unavailable".into(),
                        external_effect_unknown: false,
                        profile_binding: (code == "authentication_status_rpc_failed")
                            .then_some(expected.clone()),
                    },
                    false,
                    at,
                )
                .unwrap();
            drop(storage);
            let reopened = Storage::open_or_create(&root).unwrap();
            let value =
                serde_json::to_value(load_run_dossier_view(&reopened, &run_id).unwrap()).unwrap();
            assert_eq!(value["error"]["code"], code);
            assert_eq!(value["votes"], serde_json::json!([]));
            if code == "authentication_status_rpc_failed" {
                assert_eq!(
                    value["error"]["profileBinding"],
                    serde_json::to_value(expected).unwrap()
                );
            } else {
                assert!(value["error"].get("profileBinding").is_none());
            }
            assert!(!value.to_string().contains("canonicalPath"));
            assert!(!value.to_string().contains("authority-fixture"));
            assert!(load_run_dossier_view(&reopened, "missing-run").is_err());
            drop(reopened);
            fs::remove_dir_all(root).unwrap();
        }
    }
}
