#[path = "extraction_resource_authority.rs"]
mod authority;
use magi_context::{
    ExtractionCompletionProof, ExtractionHelperAuthority, ExtractionInvocation,
    ExtractionOperationLease,
};
use magi_provider::{
    StreamOperationLease, VerificationRequest,
    resource_custody::{ArtifactNamespace, DevelopmentProfile, ReaderCustody, ResourceCustody},
};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::{Child, Command},
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant},
};
const UNAVAILABLE: &str = "The verified native extraction helper is unavailable.";
static CONFIGURED: OnceLock<Arc<NativeHelperAuthority>> = OnceLock::new();
static CONFIGURATION_LOCK: Mutex<()> = Mutex::new(());

#[cfg(test)]
struct CompletionGate {
    ready: std::sync::mpsc::Sender<VerificationRequest>,
    release: std::sync::mpsc::Receiver<()>,
}
#[cfg(test)]
static COMPLETION_GATE: Mutex<Option<CompletionGate>> = Mutex::new(None);
#[cfg(test)]
static FENCE_NOTIFICATION: Mutex<Option<std::sync::mpsc::Sender<()>>> = Mutex::new(None);

#[cfg(test)]
pub(crate) fn observe_next_operation_fence(ready: std::sync::mpsc::Sender<()>) {
    let mut notification = FENCE_NOTIFICATION.lock().unwrap();
    assert!(notification.is_none());
    *notification = Some(ready);
}

#[cfg(test)]
pub(crate) fn hold_next_completion(
    ready: std::sync::mpsc::Sender<VerificationRequest>,
    release: std::sync::mpsc::Receiver<()>,
) {
    let mut gate = COMPLETION_GATE.lock().unwrap();
    assert!(gate.is_none());
    *gate = Some(CompletionGate { ready, release });
}

struct HelperState {
    closed: bool,
    next_operation: u64,
    operations: HashMap<u64, VerificationRequest>,
}
impl HelperState {
    fn admit_spawn<T>(&self, spawn: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
        if self.closed {
            Err(UNAVAILABLE.into())
        } else {
            spawn()
        }
    }
}
struct NativeHelperAuthority {
    path: PathBuf,
    generation: String,
    resource: Mutex<Option<Arc<authority::VerifiedExtractionResource>>>,
    retirement: Mutex<Option<Arc<RetiredExtractionAuthority>>>,
    custody: ResourceCustody,
    cache: Mutex<HelperCacheCustody>,
    cache_request: VerificationRequest,
    state: Arc<Mutex<HelperState>>,
}
pub(crate) struct RetiredExtractionAuthority {
    resource: Arc<authority::VerifiedExtractionResource>,
    generation: String,
}
impl magi_provider::resource_custody::HeldExtractionAuthority for RetiredExtractionAuthority {
    fn validate_held(&self) -> Result<(), magi_provider::ProviderError> {
        self.resource
            .check()
            .map_err(|_| magi_provider::ProviderError::ArtifactVerification)
    }
    fn executable(&self) -> &Path {
        self.resource.path()
    }
    fn generation_digest(&self) -> &str {
        &self.generation
    }
}
enum HelperCacheCustody {
    Live(Box<ReaderCustody>),
    Retired,
    Unresolved,
}
struct NativeHelperOperation {
    invocation: ExtractionInvocation,
    operation: u64,
    state: Arc<Mutex<HelperState>>,
    resource: Arc<authority::VerifiedExtractionResource>,
    request: VerificationRequest,
    reader: Option<ReaderCustody>,
    activity: Option<StreamOperationLease>,
}
impl Drop for NativeHelperOperation {
    fn drop(&mut self) {
        if let Some(activity) = self.activity.take() {
            activity.retain_unresolved();
        }
        // Unobserved operations remain in both the durable journal and this
        // registry. Neither caller cancellation nor Drop proves child cleanup.
    }
}
impl ExtractionOperationLease for NativeHelperOperation {
    fn check(&self) -> Result<(), String> {
        self.request.check().map_err(|_| UNAVAILABLE)?;
        self.resource.check()
    }
    fn spawn(&self, command: &mut Command) -> Result<Child, String> {
        let state = self.state.lock().map_err(|_| UNAVAILABLE)?;
        if command.get_program() != "/usr/bin/sandbox-exec"
            || !command
                .get_args()
                .any(|argument| argument == self.resource.path().as_os_str())
        {
            return Err(UNAVAILABLE.into());
        }
        state.admit_spawn(|| {
            self.resource.check()?;
            self.reader
                .as_ref()
                .ok_or(UNAVAILABLE)?
                .with_effect_admission(self.activity.as_ref().ok_or(UNAVAILABLE)?, || {
                    command.spawn()
                })
                .map_err(|_| UNAVAILABLE)?
                .map_err(|_| UNAVAILABLE.into())
        })
    }
    fn finish(mut self: Box<Self>, proof: ExtractionCompletionProof) -> Result<(), String> {
        if !proof.matches(&self.invocation) {
            return Err(UNAVAILABLE.into());
        }
        #[cfg(test)]
        if let Some(gate) = COMPLETION_GATE.lock().map_err(|_| UNAVAILABLE)?.take() {
            gate.ready
                .send(self.request.clone())
                .map_err(|_| UNAVAILABLE)?;
            gate.release
                .recv_timeout(Duration::from_secs(10))
                .map_err(|_| UNAVAILABLE)?;
        }
        drop(self.activity.take());
        self.request.revoke();
        let reader = self.reader.take().ok_or(UNAVAILABLE)?;
        let observed = reader.observe_cleanup().map_err(|_| UNAVAILABLE)?;
        reader.finish_observed(observed).map_err(|_| UNAVAILABLE)?;
        self.state
            .lock()
            .map_err(|_| UNAVAILABLE)?
            .operations
            .remove(&self.operation);
        Ok(())
    }
}
impl ExtractionHelperAuthority for NativeHelperAuthority {
    fn path(&self) -> &Path {
        &self.path
    }
    fn begin(
        &self,
        invocation: ExtractionInvocation,
    ) -> Result<Box<dyn ExtractionOperationLease>, String> {
        let mut state = self.state.lock().map_err(|_| UNAVAILABLE)?;
        if state.closed {
            return Err(UNAVAILABLE.into());
        }
        let resource = self
            .resource
            .lock()
            .map_err(|_| UNAVAILABLE)?
            .as_ref()
            .cloned()
            .ok_or(UNAVAILABLE)?;
        let request = VerificationRequest::until(invocation.deadline());
        let reader = self
            .custody
            .begin_reader(&self.generation, request.clone())
            .map_err(|_| UNAVAILABLE)?;
        resource.check()?;
        let activity = request.track_stream_operation().map_err(|_| UNAVAILABLE)?;
        let operation = state.next_operation;
        state.next_operation = operation.checked_add(1).ok_or(UNAVAILABLE)?;
        state.operations.insert(operation, request.clone());
        Ok(Box::new(NativeHelperOperation {
            invocation,
            operation,
            state: self.state.clone(),
            resource,
            request,
            reader: Some(reader),
            activity: Some(activity),
        }))
    }
    fn close(&self) -> Result<(), String> {
        let roots = {
            let mut state = self.state.lock().map_err(|_| UNAVAILABLE)?;
            state.closed = true;
            state.operations.values().cloned().collect::<Vec<_>>()
        };
        for request in roots {
            request.revoke();
        }
        if !self
            .state
            .lock()
            .map_err(|_| UNAVAILABLE)?
            .operations
            .is_empty()
        {
            return Err("Native extraction cleanup remains unresolved.".into());
        }
        let mut cache = self.cache.lock().map_err(|_| UNAVAILABLE)?;
        let mut retirement = self.retirement.lock().map_err(|_| UNAVAILABLE)?;
        if retirement.is_none() {
            let resource = self
                .resource
                .lock()
                .map_err(|_| UNAVAILABLE)?
                .take()
                .ok_or(UNAVAILABLE)?;
            *retirement = Some(Arc::new(RetiredExtractionAuthority {
                resource,
                generation: self.generation.clone(),
            }));
        }
        match std::mem::replace(&mut *cache, HelperCacheCustody::Unresolved) {
            HelperCacheCustody::Live(reader) => {
                self.cache_request.revoke();
                let proof = reader.observe_cleanup().map_err(|_| UNAVAILABLE)?;
                (*reader).finish_observed(proof).map_err(|_| UNAVAILABLE)?;
                *cache = HelperCacheCustody::Retired;
            }
            HelperCacheCustody::Retired => *cache = HelperCacheCustody::Retired,
            HelperCacheCustody::Unresolved => {
                return Err("Native extraction cache custody remains unresolved.".into());
            }
        }
        Ok(())
    }
}

pub(crate) fn ensure(resource_root: &Path) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(60);
    let _configuration = loop {
        match CONFIGURATION_LOCK.try_lock() {
            Ok(guard) => break guard,
            Err(std::sync::TryLockError::WouldBlock) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(1))
            }
            Err(_) => return Err(UNAVAILABLE.into()),
        }
    };
    if let Some(configured) = CONFIGURED.get() {
        if configured.path != resource_root.join("extraction/magi-extract") {
            return Err(UNAVAILABLE.into());
        }
        let state = configured.state.lock().map_err(|_| UNAVAILABLE)?;
        if state.closed {
            return Err(UNAVAILABLE.into());
        }
        return configured
            .resource
            .lock()
            .map_err(|_| UNAVAILABLE)?
            .as_ref()
            .ok_or(UNAVAILABLE)?
            .check();
    }
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or(UNAVAILABLE)?;
    let profile = if resource_root == workspace.join("src-tauri/target/debug") {
        DevelopmentProfile::Debug
    } else if resource_root == workspace.join("src-tauri/target/release") {
        DevelopmentProfile::Release
    } else {
        return Err(UNAVAILABLE.into());
    };
    let namespace = ArtifactNamespace::development(workspace, profile).map_err(|_| UNAVAILABLE)?;
    let custody = ResourceCustody::open(namespace).map_err(|_| UNAVAILABLE)?;
    let request = VerificationRequest::until(deadline);
    let provisional = magi_domain::Digest::from_bytes(include_bytes!("../native/extract.swift"));
    // This record protects verification work, not an asserted artifact identity.
    let verification_reader = custody
        .begin_reader(provisional.as_str(), request.clone())
        .map_err(|_| UNAVAILABLE)?;
    let resource = authority::verify_resource(resource_root, |path| {
        authority::verify_signature_with_request(path, request.clone())
    })
    .and_then(|resource| {
        authority::validate_source_binding(&resource, include_bytes!("../native/extract.swift"))?;
        Ok(resource)
    });
    let resource = match resource {
        Ok(resource) => resource,
        Err(error) => {
            request.revoke();
            let proof = verification_reader
                .observe_cleanup()
                .map_err(|_| UNAVAILABLE)?;
            verification_reader
                .finish_observed(proof)
                .map_err(|_| UNAVAILABLE)?;
            return Err(error);
        }
    };
    let generation = magi_domain::Digest::from_bytes(resource.as_ref())
        .as_str()
        .to_owned();
    let cache_request = VerificationRequest::until(Instant::now() + Duration::from_secs(60));
    let cache = custody
        .begin_reader(&generation, cache_request.clone())
        .map_err(|_| UNAVAILABLE)?;
    resource.check()?;
    request.revoke();
    let proof = verification_reader
        .observe_cleanup()
        .map_err(|_| UNAVAILABLE)?;
    verification_reader
        .finish_observed(proof)
        .map_err(|_| UNAVAILABLE)?;
    let configured = Arc::new(NativeHelperAuthority {
        path: resource.path().to_owned(),
        generation,
        resource: Mutex::new(Some(Arc::new(resource))),
        retirement: Mutex::new(None),
        custody,
        cache: Mutex::new(HelperCacheCustody::Live(Box::new(cache))),
        cache_request,
        state: Arc::new(Mutex::new(HelperState {
            closed: false,
            next_operation: 0,
            operations: HashMap::new(),
        })),
    });
    magi_context::configure_extraction_helper_authority(configured.clone())
        .map_err(|_| UNAVAILABLE)?;
    CONFIGURED.set(configured).map_err(|_| UNAVAILABLE.into())
}

pub(crate) fn retired_authority()
-> Result<Arc<dyn magi_provider::resource_custody::HeldExtractionAuthority>, String> {
    let configured = CONFIGURED.get().ok_or(UNAVAILABLE)?;
    let state = configured.state.lock().map_err(|_| UNAVAILABLE)?;
    if !state.closed || !state.operations.is_empty() {
        return Err(UNAVAILABLE.into());
    }
    let cache = configured.cache.lock().map_err(|_| UNAVAILABLE)?;
    if !matches!(*cache, HelperCacheCustody::Retired) {
        return Err(UNAVAILABLE.into());
    }
    let resource = configured
        .retirement
        .lock()
        .map_err(|_| UNAVAILABLE)?
        .as_ref()
        .cloned()
        .ok_or(UNAVAILABLE)?;
    resource.resource.check()?;
    Ok(resource)
}

pub(crate) fn verify_cold_resource(
    resource_root: &Path,
    request: VerificationRequest,
) -> Result<Arc<RetiredExtractionAuthority>, String> {
    if CONFIGURED.get().is_some() {
        return Err(UNAVAILABLE.into());
    }
    let resource = authority::verify_resource(resource_root, |path| {
        authority::verify_signature_with_request(path, request.clone())
    })?;
    authority::validate_source_binding(&resource, include_bytes!("../native/extract.swift"))?;
    resource.check()?;
    Ok(Arc::new(RetiredExtractionAuthority {
        generation: magi_domain::Digest::from_bytes(resource.as_ref())
            .as_str()
            .to_owned(),
        resource: Arc::new(resource),
    }))
}

pub(crate) fn fence_operation_roots() -> Result<Vec<VerificationRequest>, String> {
    let helper = CONFIGURED.get().ok_or(UNAVAILABLE)?;
    let roots = {
        let mut state = helper.state.lock().map_err(|_| UNAVAILABLE)?;
        state.closed = true;
        state.operations.values().cloned().collect::<Vec<_>>()
    };
    for root in &roots {
        root.revoke();
    }
    #[cfg(test)]
    if let Some(ready) = FENCE_NOTIFICATION.lock().map_err(|_| UNAVAILABLE)?.take() {
        let _ = ready.send(());
    }
    Ok(roots)
}

pub(crate) fn assert_unconfigured() -> Result<(), String> {
    let _configuration = CONFIGURATION_LOCK.try_lock().map_err(|_| UNAVAILABLE)?;
    if CONFIGURED.get().is_some() {
        return Err(UNAVAILABLE.into());
    }
    Ok(())
}

pub(crate) fn assert_unconfigured_until(
    deadline: Instant,
) -> Result<(), magi_provider::ProviderError> {
    loop {
        match CONFIGURATION_LOCK.try_lock() {
            Ok(_configuration) => {
                return if CONFIGURED.get().is_none() {
                    Ok(())
                } else {
                    Err(magi_provider::ProviderError::ArtifactVerification)
                };
            }
            Err(std::sync::TryLockError::Poisoned(_)) => {
                return Err(magi_provider::ProviderError::ArtifactVerification);
            }
            Err(std::sync::TryLockError::WouldBlock) => {
                if Instant::now() >= deadline {
                    return Err(magi_provider::ProviderError::Timeout);
                }
                std::thread::sleep(
                    Duration::from_millis(1)
                        .min(deadline.saturating_duration_since(Instant::now())),
                );
            }
        }
    }
}

#[cfg(test)]
pub(crate) fn hold_configuration_for_test(
    ready: std::sync::mpsc::Sender<()>,
    release: std::sync::mpsc::Receiver<()>,
) {
    let _configuration = CONFIGURATION_LOCK.lock().unwrap();
    ready.send(()).unwrap();
    release.recv_timeout(Duration::from_secs(5)).unwrap();
}

pub(crate) fn attach_installed_resource(
    verified: Arc<RetiredExtractionAuthority>,
    custody: ResourceCustody,
) -> Result<(), String> {
    let _configuration = CONFIGURATION_LOCK.try_lock().map_err(|_| UNAVAILABLE)?;
    if CONFIGURED.get().is_some() {
        return Err(UNAVAILABLE.into());
    }
    verified.resource.check()?;
    let cache_request = VerificationRequest::until(Instant::now() + Duration::from_secs(60));
    let cache = custody
        .begin_reader(&verified.generation, cache_request.clone())
        .map_err(|_| UNAVAILABLE)?;
    let configured = Arc::new(NativeHelperAuthority {
        path: verified.resource.path().to_owned(),
        generation: verified.generation.clone(),
        resource: Mutex::new(Some(verified.resource.clone())),
        retirement: Mutex::new(None),
        custody,
        cache: Mutex::new(HelperCacheCustody::Live(Box::new(cache))),
        cache_request,
        state: Arc::new(Mutex::new(HelperState {
            closed: true,
            next_operation: 0,
            operations: HashMap::new(),
        })),
    });
    magi_context::configure_extraction_helper_authority(configured.clone())
        .map_err(|_| UNAVAILABLE)?;
    CONFIGURED.set(configured).map_err(|_| UNAVAILABLE.into())
}

pub(crate) fn activate_installed_resource(
    _admission: &super::NativeResourceAdmission<'_>,
) -> Result<(), String> {
    let configured = CONFIGURED.get().ok_or(UNAVAILABLE)?;
    let mut state = configured.state.lock().map_err(|_| UNAVAILABLE)?;
    if !state.closed
        || !state.operations.is_empty()
        || configured
            .retirement
            .lock()
            .map_err(|_| UNAVAILABLE)?
            .is_some()
        || !matches!(
            *configured.cache.lock().map_err(|_| UNAVAILABLE)?,
            HelperCacheCustody::Live(_)
        )
    {
        return Err(UNAVAILABLE.into());
    }
    configured
        .resource
        .lock()
        .map_err(|_| UNAVAILABLE)?
        .as_ref()
        .ok_or(UNAVAILABLE)?
        .check()?;
    state.closed = false;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "Requires an explicit verified signed helper fixture; no publication or document input."]
    fn mismatched_helper_completion_preserves_actual_durable_custody() {
        use std::{os::unix::process::CommandExt, process::Stdio};
        struct Fixture(PathBuf);
        impl Drop for Fixture {
            fn drop(&mut self) {
                std::fs::remove_dir_all(&self.0).unwrap();
            }
        }
        let fixture = Fixture(
            std::env::temp_dir()
                .canonicalize()
                .unwrap()
                .join(format!("helper-custody-proof-{}", uuid::Uuid::new_v4())),
        );
        std::fs::create_dir(&fixture.0).unwrap();
        let root = PathBuf::from(
            std::env::var_os("MAGI_TEST_EXTRACTION_RESOURCE_ROOT")
                .expect("explicit signed fixture"),
        );
        let resource = Arc::new(
            authority::verify_resource(&root, |path| {
                authority::verify_signature_with_request(
                    path,
                    VerificationRequest::until(Instant::now() + Duration::from_secs(60)),
                )
            })
            .unwrap(),
        );
        authority::validate_source_binding(&resource, include_bytes!("../native/extract.swift"))
            .unwrap();
        struct ClosedFixture {
            helper: Arc<RetiredExtractionAuthority>,
        }
        impl magi_provider::resource_custody::RetainedNativeQuiescence for ClosedFixture {
            fn validate_quiescence(&self) -> Result<(), magi_provider::ProviderError> {
                magi_provider::resource_custody::HeldExtractionAuthority::validate_held(
                    self.helper.as_ref(),
                )
            }
            fn bound_requests(&self) -> &[VerificationRequest] {
                &[]
            }
            fn verified_extraction(
                &self,
            ) -> Result<
                Arc<dyn magi_provider::resource_custody::HeldExtractionAuthority>,
                magi_provider::ProviderError,
            > {
                self.validate_quiescence()?;
                Ok(self.helper.clone())
            }
        }
        let helper = Arc::new(RetiredExtractionAuthority {
            generation: magi_domain::Digest::from_bytes(resource.as_ref().as_ref())
                .as_str()
                .to_owned(),
            resource: resource.clone(),
        });
        let native = Arc::new(ClosedFixture { helper });
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let installed = runtime
            .block_on(magi_provider::verify_installed_generation(
                root.clone(),
                root.join("provider/codex-acp/darwin-arm64/codex-acp"),
                native.clone(),
                VerificationRequest::until(Instant::now() + Duration::from_secs(60)),
            ))
            .unwrap();
        let custody = ResourceCustody::adopt_installed(
            ArtifactNamespace::installed_resources(&root, &fixture.0).unwrap(),
            installed,
            native,
        )
        .unwrap();
        let image = fixture.0.join("approved.png");
        std::fs::write(
            &image,
            [137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82],
        )
        .unwrap();
        struct ProofFactory {
            invocations: Arc<Mutex<Vec<ExtractionInvocation>>>,
            proofs: Arc<Mutex<Vec<ExtractionCompletionProof>>>,
        }
        struct ProofLease {
            invocation: ExtractionInvocation,
            proofs: Arc<Mutex<Vec<ExtractionCompletionProof>>>,
        }
        impl ExtractionHelperAuthority for ProofFactory {
            fn path(&self) -> &Path {
                Path::new("/usr/bin/true")
            }
            fn begin(
                &self,
                invocation: ExtractionInvocation,
            ) -> Result<Box<dyn ExtractionOperationLease>, String> {
                self.invocations.lock().unwrap().push(invocation.clone());
                Ok(Box::new(ProofLease {
                    invocation,
                    proofs: self.proofs.clone(),
                }))
            }
            fn close(&self) -> Result<(), String> {
                Ok(())
            }
        }
        impl ExtractionOperationLease for ProofLease {
            fn check(&self) -> Result<(), String> {
                Ok(())
            }
            fn spawn(&self, _: &mut Command) -> Result<Child, String> {
                let mut command = Command::new("/usr/bin/true");
                command
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped());
                unsafe {
                    command.pre_exec(|| {
                        if libc::setpgid(0, 0) == 0 {
                            Ok(())
                        } else {
                            Err(std::io::Error::last_os_error())
                        }
                    });
                }
                command.spawn().map_err(|_| UNAVAILABLE.into())
            }
            fn finish(self: Box<Self>, proof: ExtractionCompletionProof) -> Result<(), String> {
                assert!(proof.matches(&self.invocation));
                self.proofs.lock().unwrap().push(proof);
                Ok(())
            }
        }
        let invocations = Arc::new(Mutex::new(Vec::new()));
        let proofs = Arc::new(Mutex::new(Vec::new()));
        magi_context::configure_extraction_helper_authority(Arc::new(ProofFactory {
            invocations: invocations.clone(),
            proofs: proofs.clone(),
        }))
        .unwrap();
        for _ in 0..2 {
            let file = cap_std::fs::File::from_std(std::fs::File::open(&image).unwrap());
            let mut grant =
                magi_context::SourceGrant::selected_files(vec![(file, "approved.png".into())])
                    .unwrap();
            let enumeration = grant
                .enumerate(magi_context::CaptureLimits::default_policy())
                .unwrap();
            let batch = grant
                .capture_selected(
                    &[magi_context::CaptureDirective {
                        source_id: enumeration.candidates[0].source_id.clone(),
                        line_range: None,
                    }],
                    magi_context::CaptureLimits::default_policy(),
                    1,
                )
                .unwrap();
            assert!(batch.objects.is_empty());
        }
        let invocation = invocations.lock().unwrap().remove(0);
        let wrong_proof = proofs.lock().unwrap().pop().unwrap();
        assert!(!wrong_proof.matches(&invocation));
        let request = VerificationRequest::until(invocation.deadline());
        let generation = magi_domain::Digest::from_bytes(resource.as_ref().as_ref());
        let reader = custody
            .begin_reader(generation.as_str(), request.clone())
            .unwrap();
        let activity = request.track_stream_operation().unwrap();
        let state = Arc::new(Mutex::new(HelperState {
            closed: false,
            next_operation: 1,
            operations: [(0, request.clone())].into_iter().collect(),
        }));
        let operation = Box::new(NativeHelperOperation {
            invocation,
            operation: 0,
            state: state.clone(),
            resource,
            request: request.clone(),
            reader: Some(reader),
            activity: Some(activity),
        });
        assert!(operation.finish(wrong_proof).is_err());
        assert_eq!(state.lock().unwrap().operations.len(), 1);
        assert_eq!(request.settlement().unresolved_cleanup, 1);
        assert!(custody.unresolved_operations().unwrap() > 0);
        drop(custody);
    }

    #[test]
    fn helper_close_and_actual_spawn_are_serialized_in_both_orders() {
        let state = Arc::new(Mutex::new(HelperState {
            closed: false,
            next_operation: 0,
            operations: HashMap::new(),
        }));
        state.lock().unwrap().closed = true;
        let mut invoked = false;
        assert!(
            state
                .lock()
                .unwrap()
                .admit_spawn(|| {
                    invoked = true;
                    Command::new("/usr/bin/true")
                        .spawn()
                        .map_err(|_| UNAVAILABLE.into())
                })
                .is_err()
        );
        assert!(!invoked);

        state.lock().unwrap().closed = false;
        let (entered_sender, entered_receiver) = std::sync::mpsc::sync_channel(1);
        let (release_sender, release_receiver) = std::sync::mpsc::sync_channel(1);
        let spawning = state.clone();
        let worker = std::thread::spawn(move || {
            spawning
                .lock()
                .unwrap()
                .admit_spawn(|| {
                    entered_sender.send(()).unwrap();
                    release_receiver
                        .recv_timeout(Duration::from_secs(2))
                        .unwrap();
                    Command::new("/usr/bin/true")
                        .spawn()
                        .map_err(|_| UNAVAILABLE.into())
                })
                .unwrap()
        });
        entered_receiver
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
        assert!(matches!(
            state.try_lock(),
            Err(std::sync::TryLockError::WouldBlock)
        ));
        release_sender.send(()).unwrap();
        let mut child = worker.join().unwrap();
        state.lock().unwrap().closed = true;
        assert!(child.wait().unwrap().success());
    }
}
