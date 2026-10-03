use crate::{ProviderError, RuntimeArtifactIdentity, sandbox};
use sha2::Digest as _;
use std::{
    cell::RefCell,
    collections::HashMap,
    fs::{self, File, Metadata, OpenOptions},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

/// Revocable authority for a bounded verification request. Run lifetime is separate.
#[derive(Clone)]
pub struct VerificationRequest {
    deadline: Instant,
    revoked: Arc<AtomicBool>,
    publication: Arc<std::sync::Mutex<()>>,
    revocation: Arc<tokio::sync::watch::Sender<bool>>,
    activities: Arc<ActivityLedger>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RequestSettlement {
    pub queued_verifications: u32,
    pub verification_workers: u32,
    pub provider_operations: u32,
    pub unresolved_cleanup: u32,
}

impl RequestSettlement {
    pub fn is_settled(&self) -> bool {
        self.queued_verifications == 0
            && self.verification_workers == 0
            && self.provider_operations == 0
            && self.unresolved_cleanup == 0
    }
}

struct ActivityLedger {
    state: std::sync::Mutex<RequestSettlement>,
    updates: tokio::sync::watch::Sender<RequestSettlement>,
}

#[derive(Clone, Copy)]
enum ActivityKind {
    QueuedVerification,
    VerificationWorker,
    Provider,
}

pub(crate) struct ActivityLease {
    ledger: Arc<ActivityLedger>,
    kind: ActivityKind,
    unresolved_on_drop: bool,
}

impl ActivityLedger {
    fn update(&self, update: impl FnOnce(&mut RequestSettlement)) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        update(&mut state);
        self.updates.send_replace(*state);
    }
}

impl ActivityLease {
    fn new(ledger: Arc<ActivityLedger>, kind: ActivityKind) -> Self {
        ledger.update(|state| match kind {
            ActivityKind::QueuedVerification => state.queued_verifications += 1,
            ActivityKind::VerificationWorker => state.verification_workers += 1,
            ActivityKind::Provider => state.provider_operations += 1,
        });
        Self {
            ledger,
            kind,
            unresolved_on_drop: false,
        }
    }

    fn start_verification(&mut self) {
        self.ledger.update(|state| {
            state.queued_verifications -= 1;
            state.verification_workers += 1;
        });
        self.kind = ActivityKind::VerificationWorker;
    }

    pub(crate) fn mark_owned_process(&mut self) {
        self.unresolved_on_drop = true;
    }
    pub(crate) fn settle(mut self) {
        self.unresolved_on_drop = false;
    }
}

impl Drop for ActivityLease {
    fn drop(&mut self) {
        self.ledger.update(|state| {
            match self.kind {
                ActivityKind::QueuedVerification => state.queued_verifications -= 1,
                ActivityKind::VerificationWorker => state.verification_workers -= 1,
                ActivityKind::Provider => state.provider_operations -= 1,
            }
            if self.unresolved_on_drop {
                state.unresolved_cleanup += 1;
            }
        });
    }
}

pub(crate) struct PromptPublicationAuthorization {
    expires_at_epoch_ms: u64,
    deadline: std::time::Instant,
    check_current: Arc<dyn Fn() -> Result<(), ProviderError> + Send + Sync>,
}
impl PromptPublicationAuthorization {
    pub(crate) fn new(
        expires_at_epoch_ms: u64,
        check_current: Arc<dyn Fn() -> Result<(), ProviderError> + Send + Sync>,
    ) -> Result<Self, ProviderError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| ProviderError::Cancelled)?
            .as_millis();
        let remaining = u128::from(expires_at_epoch_ms)
            .checked_sub(now)
            .filter(|n| *n > 0)
            .ok_or(ProviderError::Cancelled)?;
        let deadline = std::time::Instant::now()
            .checked_add(std::time::Duration::from_millis(
                u64::try_from(remaining).map_err(|_| ProviderError::Cancelled)?,
            ))
            .ok_or(ProviderError::Cancelled)?;
        Ok(Self {
            expires_at_epoch_ms,
            deadline,
            check_current,
        })
    }
    pub(crate) fn check(&self) -> Result<(), ProviderError> {
        (self.check_current)()?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| ProviderError::Cancelled)?
            .as_millis();
        if now >= u128::from(self.expires_at_epoch_ms) || std::time::Instant::now() >= self.deadline
        {
            return Err(ProviderError::Cancelled);
        }
        Ok(())
    }
}

#[derive(Clone)]
pub(crate) struct EffectAuthority {
    revoked: Arc<AtomicBool>,
    publication: Arc<std::sync::Mutex<()>>,
    revocation: Arc<tokio::sync::watch::Sender<bool>>,
}

impl EffectAuthority {
    pub(crate) async fn cancelled(&self) {
        let mut receiver = self.revocation.subscribe();
        let _ = receiver.wait_for(|revoked| *revoked).await;
    }
    pub(crate) fn publish<T>(&self, operation: impl FnOnce() -> T) -> Result<T, ProviderError> {
        let _guard = self
            .publication
            .lock()
            .map_err(|_| ProviderError::Cancelled)?;
        if self.revoked.load(Ordering::SeqCst) {
            return Err(ProviderError::Cancelled);
        }
        Ok(operation())
    }
}

impl VerificationRequest {
    pub(crate) async fn await_revoked_settlement(&self) -> Result<(), ProviderError> {
        let mut revoked = self.revocation.subscribe();
        while !*revoked.borrow_and_update() {
            revoked
                .changed()
                .await
                .map_err(|_| ProviderError::ProcessClosed)?;
        }
        self.wait_for_settlement(Instant::now() + Duration::from_secs(30))
            .await?;
        Ok(())
    }
    /// Compares shared revocation/effect authority, never cleanup or consumer counts.
    pub fn shares_authority(&self, other: &Self) -> bool {
        self.same_root(other)
    }
    pub(crate) fn same_root(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.publication, &other.publication)
            && Arc::ptr_eq(&self.activities, &other.activities)
    }
    /// Observes revocation and actual completion of every tracked activity.
    /// This does not prove native or retained artifact consumers have closed.
    pub fn observed_revoked_settlement(&self) -> bool {
        let _guard = self
            .publication
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.revoked.load(Ordering::SeqCst) && self.settlement().is_settled()
    }

    /// Reports explicit work ownership; caller abort and deadline expiry do not settle workers.
    pub fn settlement(&self) -> RequestSettlement {
        *self
            .activities
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Requires revocation first. Unconfirmed cleanup remains unsettled after a bounded wait.
    pub async fn wait_for_settlement(
        &self,
        deadline: Instant,
    ) -> Result<RequestSettlement, ProviderError> {
        if !self.revoked.load(Ordering::SeqCst) {
            return Err(ProviderError::InvalidLaunch);
        }
        let mut updates = self.activities.updates.subscribe();
        tokio::time::timeout_at(deadline.into(), async {
            loop {
                let snapshot = *updates.borrow_and_update();
                if snapshot.is_settled() {
                    return Ok(snapshot);
                }
                updates
                    .changed()
                    .await
                    .map_err(|_| ProviderError::ProcessClosed)?;
            }
        })
        .await
        .map_err(|_| ProviderError::Timeout)?
    }

    pub(crate) fn provider_activity(&self) -> Result<ActivityLease, ProviderError> {
        self.effect_authority()
            .publish(|| ActivityLease::new(self.activities.clone(), ActivityKind::Provider))
    }
    pub fn with_deadline(&self, deadline: Instant) -> Self {
        Self {
            deadline,
            ..self.clone()
        }
    }
    pub fn deadline(&self) -> Instant {
        self.deadline
    }
    pub(crate) fn effect_authority(&self) -> EffectAuthority {
        EffectAuthority {
            revoked: self.revoked.clone(),
            publication: self.publication.clone(),
            revocation: self.revocation.clone(),
        }
    }
    pub fn until(deadline: Instant) -> Self {
        Self {
            deadline,
            revoked: Arc::new(AtomicBool::new(false)),
            publication: Arc::new(std::sync::Mutex::new(())),
            revocation: Arc::new(tokio::sync::watch::channel(false).0),
            activities: Arc::new(ActivityLedger {
                state: std::sync::Mutex::new(RequestSettlement::default()),
                updates: tokio::sync::watch::channel(RequestSettlement::default()).0,
            }),
        }
    }

    pub fn revoke(&self) {
        let _guard = self
            .publication
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.revoked.store(true, Ordering::SeqCst);
        self.revocation.send_replace(true);
    }

    pub(crate) fn publish<T>(&self, operation: impl FnOnce() -> T) -> Result<T, ProviderError> {
        let _guard = self
            .publication
            .lock()
            .map_err(|_| ProviderError::Cancelled)?;
        self.check()?;
        Ok(operation())
    }

    pub fn check(&self) -> Result<(), ProviderError> {
        if self.revoked.load(Ordering::SeqCst) {
            return Err(ProviderError::Cancelled);
        }
        if Instant::now() >= self.deadline {
            return Err(ProviderError::Timeout);
        }
        Ok(())
    }
}

#[derive(PartialEq, Eq)]
struct FileVersion {
    dev: u64,
    ino: u64,
    size: u64,
    mode: u32,
    uid: u32,
    links: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}

impl From<&Metadata> for FileVersion {
    fn from(value: &Metadata) -> Self {
        Self {
            dev: value.dev(),
            ino: value.ino(),
            size: value.len(),
            mode: value.mode(),
            uid: value.uid(),
            links: value.nlink(),
            modified: (value.mtime(), value.mtime_nsec()),
            changed: (value.ctime(), value.ctime_nsec()),
        }
    }
}

struct HeldFile {
    path: PathBuf,
    file: File,
    version: FileVersion,
}

impl HeldFile {
    fn open(path: PathBuf) -> Result<Self, ProviderError> {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
            .open(&path)
            .map_err(|_| ProviderError::ArtifactVerification)?;
        let metadata = file
            .metadata()
            .map_err(|_| ProviderError::ArtifactVerification)?;
        if !metadata.is_file()
            || metadata.mode() & 0o222 != 0
            || metadata.nlink() != 1
            || metadata.uid() != unsafe { libc::geteuid() }
        {
            return Err(ProviderError::ArtifactVerification);
        }
        let held = Self {
            path,
            file,
            version: FileVersion::from(&metadata),
        };
        held.check()?;
        Ok(held)
    }

    fn check(&self) -> Result<(), ProviderError> {
        let current =
            fs::symlink_metadata(&self.path).map_err(|_| ProviderError::ArtifactVerification)?;
        let opened = self
            .file
            .metadata()
            .map_err(|_| ProviderError::ArtifactVerification)?;
        if !current.is_file()
            || FileVersion::from(&current) != self.version
            || FileVersion::from(&opened) != self.version
        {
            return Err(ProviderError::ArtifactVerification);
        }
        Ok(())
    }
}

struct HeldDirectory {
    path: PathBuf,
    file: File,
    version: FileVersion,
    contents_fenced: bool,
}

struct HeldTree {
    files: Vec<HeldFile>,
    directories: Vec<HeldDirectory>,
}

impl HeldTree {
    fn open(root: &Path, request: &VerificationRequest) -> Result<Self, ProviderError> {
        let mut tree = Self {
            files: Vec::new(),
            directories: Vec::new(),
        };
        for ancestor in root.ancestors().skip(1) {
            request.check()?;
            let file = OpenOptions::new()
                .read(true)
                .custom_flags(
                    libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
                )
                .open(ancestor)
                .map_err(|_| ProviderError::ArtifactVerification)?;
            let metadata = file
                .metadata()
                .map_err(|_| ProviderError::ArtifactVerification)?;
            tree.directories.push(HeldDirectory {
                path: ancestor.to_owned(),
                file,
                version: FileVersion::from(&metadata),
                contents_fenced: false,
            });
        }
        tree.visit(root, request)?;
        tree.check(request)?;
        Ok(tree)
    }

    fn visit(&mut self, path: &Path, request: &VerificationRequest) -> Result<(), ProviderError> {
        request.check()?;
        if self.files.len() + self.directories.len() >= 256 {
            return Err(ProviderError::ArtifactVerification);
        }
        let metadata =
            fs::symlink_metadata(path).map_err(|_| ProviderError::ArtifactVerification)?;
        if metadata.is_dir() {
            if metadata.mode() & 0o222 != 0 || metadata.uid() != unsafe { libc::geteuid() } {
                return Err(ProviderError::ArtifactVerification);
            }
            let file = OpenOptions::new()
                .read(true)
                .custom_flags(
                    libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
                )
                .open(path)
                .map_err(|_| ProviderError::ArtifactVerification)?;
            self.directories.push(HeldDirectory {
                path: path.to_owned(),
                file,
                version: FileVersion::from(&metadata),
                contents_fenced: true,
            });
            for entry in fs::read_dir(path).map_err(|_| ProviderError::ArtifactVerification)? {
                self.visit(
                    &entry
                        .map_err(|_| ProviderError::ArtifactVerification)?
                        .path(),
                    request,
                )?;
            }
        } else {
            self.files.push(HeldFile::open(path.to_owned())?);
        }
        Ok(())
    }

    fn check(&self, request: &VerificationRequest) -> Result<(), ProviderError> {
        request.check()?;
        self.check_held()?;
        request.check()
    }
    fn check_held(&self) -> Result<(), ProviderError> {
        for directory in &self.directories {
            let current = fs::symlink_metadata(&directory.path)
                .map_err(|_| ProviderError::ArtifactVerification)?;
            let opened = directory
                .file
                .metadata()
                .map_err(|_| ProviderError::ArtifactVerification)?;
            let unchanged = |metadata: &Metadata| {
                let actual = FileVersion::from(metadata);
                if directory.contents_fenced {
                    actual == directory.version
                } else {
                    actual.dev == directory.version.dev
                        && actual.ino == directory.version.ino
                        && actual.mode == directory.version.mode
                        && actual.uid == directory.version.uid
                }
            };
            if !current.is_dir() || !unchanged(&current) || !unchanged(&opened) {
                return Err(ProviderError::ArtifactVerification);
            }
        }
        for file in &self.files {
            file.check()?;
        }
        Ok(())
    }
}

/// Full signed installed authority; fields can only be minted by the fixed verifier.
pub struct VerifiedInstalledGeneration {
    publication: PathBuf,
    provider: Arc<HeldTree>,
    helper: Arc<dyn crate::resource_custody::HeldExtractionAuthority>,
    generation: String,
    request: VerificationRequest,
}

/// A held cryptographic proof for nonexecuting resource inspection.
/// This type cannot authorize runtime adoption, readers or publication.
pub struct VerifiedReadOnlyInstallation {
    provider: Arc<HeldTree>,
    helper: Arc<dyn crate::resource_custody::HeldExtractionAuthority>,
    request: VerificationRequest,
}
impl VerifiedReadOnlyInstallation {
    pub fn validate_current(&self) -> Result<(), ProviderError> {
        self.provider.check(&self.request)?;
        self.helper.validate_held()?;
        self.request.check()
    }
}
impl VerifiedInstalledGeneration {
    pub(crate) fn check(&self) -> Result<(), ProviderError> {
        self.provider.check(&self.request)?;
        self.helper.validate_held()?;
        // Artifact identity is independent of a retained publication operation.
        // Its cleanup authority is checked separately before journal retirement.
        Ok(())
    }
    pub(crate) fn check_namespace(&self, publication: &Path) -> Result<(), ProviderError> {
        if self.publication != publication {
            return Err(ProviderError::ArtifactVerification);
        }
        self.check()
    }
    pub(crate) fn check_held(&self) -> Result<(), ProviderError> {
        self.provider.check_held()?;
        self.helper.validate_held()
    }
    pub(crate) fn request(&self) -> VerificationRequest {
        self.request.clone()
    }
    pub fn generation_digest(&self) -> &str {
        &self.generation
    }
}

/// Exact withdrawn bytes and descriptors. This capture never authorizes execution.
pub struct RetiredGenerationCapture {
    publication: PathBuf,
    provider_wrapper: HeldDirectory,
    provider: Arc<HeldTree>,
    helper: Arc<HeldTree>,
    generation: String,
    request: VerificationRequest,
}
impl RetiredGenerationCapture {
    fn check_provider_wrapper(&self, original_path: bool) -> Result<(), ProviderError> {
        let held = &self.provider_wrapper;
        let actual = FileVersion::from(
            &held
                .file
                .metadata()
                .map_err(|_| ProviderError::ArtifactVerification)?,
        );
        let expected = &held.version;
        if (actual.dev, actual.ino, actual.mode, actual.uid)
            != (expected.dev, expected.ino, expected.mode, expected.uid)
        {
            return Err(ProviderError::ArtifactVerification);
        }
        if original_path {
            let path = fs::symlink_metadata(&held.path)
                .map_err(|_| ProviderError::ArtifactVerification)?;
            if path.file_type().is_symlink()
                || !path.is_dir()
                || (path.dev(), path.ino(), path.mode(), path.uid())
                    != (expected.dev, expected.ino, expected.mode, expected.uid)
            {
                return Err(ProviderError::ArtifactVerification);
            }
            let entries = fs::read_dir(&held.path)
                .map_err(|_| ProviderError::ArtifactVerification)?
                .map(|entry| entry.map(|entry| entry.file_name()))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| ProviderError::ArtifactVerification)?;
            if entries != vec![std::ffi::OsString::from("codex-acp")] {
                return Err(ProviderError::ArtifactVerification);
            }
        }
        Ok(())
    }
    pub async fn capture_quiescent(
        publication: PathBuf,
        native: Arc<dyn crate::resource_custody::RetainedNativeQuiescence>,
        request: VerificationRequest,
    ) -> Result<Self, ProviderError> {
        native.validate_quiescence()?;
        let mut abort_guard = RevokeOnDrop(Some(request.clone()));
        let worker_request = request.clone();
        let worker_native = native.clone();
        let mut activity =
            ActivityLease::new(request.activities.clone(), ActivityKind::QueuedVerification);
        let capture = tokio::task::spawn_blocking(move || {
            activity.start_verification();
            let _activity = activity;
            worker_native.validate_quiescence()?;
            let capture = Self::capture(&publication, worker_request)?;
            worker_native.validate_quiescence()?;
            Ok::<_, ProviderError>(capture)
        })
        .await
        .map_err(|_| ProviderError::ArtifactVerification)??;
        request.check()?;
        native.validate_quiescence()?;
        capture.check()?;
        abort_guard.0 = None;
        Ok(capture)
    }
    pub(crate) fn capture(
        publication: &Path,
        request: VerificationRequest,
    ) -> Result<Self, ProviderError> {
        let publication = publication
            .canonicalize()
            .map_err(|_| ProviderError::ArtifactVerification)?;
        let wrapper_path = publication.join("provider");
        let wrapper_file = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&wrapper_path)
            .map_err(|_| ProviderError::ArtifactVerification)?;
        let wrapper_metadata = wrapper_file
            .metadata()
            .map_err(|_| ProviderError::ArtifactVerification)?;
        if !wrapper_metadata.is_dir()
            || wrapper_metadata.uid() != unsafe { libc::geteuid() }
            || wrapper_metadata.mode() & 0o022 != 0
        {
            return Err(ProviderError::ArtifactVerification);
        }
        let provider_wrapper = HeldDirectory {
            path: wrapper_path,
            file: wrapper_file,
            version: FileVersion::from(&wrapper_metadata),
            contents_fenced: true,
        };
        let provider = Arc::new(HeldTree::open(
            &publication.join("provider/codex-acp"),
            &request,
        )?);
        let helper = Arc::new(HeldTree::open(&publication.join("extraction"), &request)?);
        let mut digest = sha2::Sha256::new();
        for tree in [&provider, &helper] {
            for file in &tree.files {
                request.check()?;
                file.check()?;
                use std::io::Read;
                let path = file
                    .path
                    .strip_prefix(&publication)
                    .map_err(|_| ProviderError::ArtifactVerification)?
                    .as_os_str()
                    .as_encoded_bytes();
                digest.update((path.len() as u64).to_le_bytes());
                digest.update(path);
                let mut file_digest = sha2::Sha256::new();
                let mut total = 0u64;
                let mut bytes = file
                    .file
                    .try_clone()
                    .map_err(|_| ProviderError::ArtifactVerification)?;
                let mut block = [0u8; 65536];
                loop {
                    request.check()?;
                    let count = bytes
                        .read(&mut block)
                        .map_err(|_| ProviderError::ArtifactVerification)?;
                    if count == 0 {
                        break;
                    }
                    total = total
                        .checked_add(count as u64)
                        .ok_or(ProviderError::ArtifactVerification)?;
                    if total > 1024 * 1024 * 1024 {
                        return Err(ProviderError::ArtifactVerification);
                    }
                    file_digest.update(&block[..count]);
                }
                digest.update(total.to_le_bytes());
                digest.update(file_digest.finalize());
                file.check()?;
            }
        }
        let capture = Self {
            publication,
            provider_wrapper,
            provider,
            helper,
            generation: format!("{:x}", digest.finalize()),
            request,
        };
        capture.check()?;
        Ok(capture)
    }
    pub(crate) fn check(&self) -> Result<(), ProviderError> {
        self.check_provider_wrapper(true)?;
        self.provider.check(&self.request)?;
        self.helper.check(&self.request)
    }
    pub(crate) fn check_namespace(&self, publication: &Path) -> Result<(), ProviderError> {
        if self.publication != publication {
            return Err(ProviderError::ArtifactVerification);
        }
        self.check()
    }
    pub fn generation_digest(&self) -> &str {
        &self.generation
    }
    pub(crate) fn check_retained(&self) -> Result<(), ProviderError> {
        self.check_provider_wrapper(false)?;
        for tree in [&self.provider, &self.helper] {
            for directory in &tree.directories {
                let actual = FileVersion::from(
                    &directory
                        .file
                        .metadata()
                        .map_err(|_| ProviderError::ArtifactVerification)?,
                );
                let expected = &directory.version;
                if actual.dev != expected.dev
                    || actual.ino != expected.ino
                    || actual.mode != expected.mode
                    || actual.uid != expected.uid
                {
                    return Err(ProviderError::ArtifactVerification);
                }
            }
            for file in &tree.files {
                if FileVersion::from(
                    &file
                        .file
                        .metadata()
                        .map_err(|_| ProviderError::ArtifactVerification)?,
                ) != file.version
                {
                    return Err(ProviderError::ArtifactVerification);
                }
            }
        }
        Ok(())
    }
    pub(crate) fn check_original_held(&self) -> Result<(), ProviderError> {
        self.check_retained()?;
        self.check_provider_wrapper(true)?;
        for tree in [&self.provider, &self.helper] {
            for directory in &tree.directories {
                let actual = FileVersion::from(
                    &fs::symlink_metadata(&directory.path)
                        .map_err(|_| ProviderError::ArtifactVerification)?,
                );
                let expected = &directory.version;
                if actual.dev != expected.dev
                    || actual.ino != expected.ino
                    || actual.mode != expected.mode
                    || actual.uid != expected.uid
                {
                    return Err(ProviderError::ArtifactVerification);
                }
                if directory.contents_fenced {
                    let mut actual: Vec<_> = fs::read_dir(&directory.path)
                        .map_err(|_| ProviderError::ArtifactVerification)?
                        .map(|entry| entry.map(|e| e.file_name()))
                        .collect::<Result<_, _>>()
                        .map_err(|_| ProviderError::ArtifactVerification)?;
                    let mut expected: Vec<_> = tree
                        .files
                        .iter()
                        .map(|f| &f.path)
                        .chain(tree.directories.iter().map(|d| &d.path))
                        .filter(|path| path.parent() == Some(directory.path.as_path()))
                        .filter_map(|path| path.file_name().map(|n| n.to_owned()))
                        .collect();
                    actual.sort();
                    expected.sort();
                    if actual != expected {
                        return Err(ProviderError::ArtifactVerification);
                    }
                }
            }
            for file in &tree.files {
                file.check()?;
            }
        }
        Ok(())
    }
    pub(crate) fn request(&self) -> VerificationRequest {
        self.request.clone()
    }
}

/// Runs the same complete provider proof while native issuance remains closed.
pub async fn verify_installed_generation(
    publication: PathBuf,
    executable: PathBuf,
    native: Arc<dyn crate::resource_custody::RetainedNativeQuiescence>,
    request: VerificationRequest,
) -> Result<VerifiedInstalledGeneration, ProviderError> {
    let mut abort_guard = RevokeOnDrop(Some(request.clone()));
    native.validate_quiescence()?;
    let helper = native.verified_extraction()?;
    let (provider, provider_digest) = verify_complete_installation(
        publication.clone(),
        executable,
        helper.clone(),
        request.clone(),
        Some(native.clone()),
    )
    .await?;
    native.validate_quiescence()?;
    let generation = format!(
        "{:x}",
        sha2::Sha256::digest(
            format!(
                "installed-artifacts-v1\0{provider_digest}\0{}",
                helper.generation_digest()
            )
            .as_bytes()
        )
    );
    let proof = VerifiedInstalledGeneration {
        publication,
        provider,
        helper,
        generation,
        request,
    };
    proof.check()?;
    abort_guard.0 = None;
    Ok(proof)
}

/// Verifies complete signed resources without allocating execution custody or control state.
pub async fn verify_readonly_installation(
    publication: PathBuf,
    executable: PathBuf,
    helper: Arc<dyn crate::resource_custody::HeldExtractionAuthority>,
    request: VerificationRequest,
) -> Result<VerifiedReadOnlyInstallation, ProviderError> {
    let mut abort_guard = RevokeOnDrop(Some(request.clone()));
    let (provider, _) = verify_complete_installation(
        publication,
        executable,
        helper.clone(),
        request.clone(),
        None,
    )
    .await?;
    let proof = VerifiedReadOnlyInstallation {
        provider,
        helper,
        request,
    };
    proof.validate_current()?;
    abort_guard.0 = None;
    Ok(proof)
}

async fn verify_complete_installation(
    publication: PathBuf,
    executable: PathBuf,
    helper: Arc<dyn crate::resource_custody::HeldExtractionAuthority>,
    request: VerificationRequest,
    native: Option<Arc<dyn crate::resource_custody::RetainedNativeQuiescence>>,
) -> Result<(Arc<HeldTree>, String), ProviderError> {
    if !executable.starts_with(publication.join("provider")) {
        return Err(ProviderError::ArtifactVerification);
    }
    helper.validate_held()?;
    if helper.executable().parent() != Some(publication.join("extraction").as_path())
        || helper.generation_digest().len() != 64
        || !helper
            .generation_digest()
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        return Err(ProviderError::ArtifactVerification);
    }
    let mut abort_guard = RevokeOnDrop(Some(request.clone()));
    let worker_native = native.clone();
    let worker_helper = helper.clone();
    let worker_request = request.clone();
    let mut activity =
        ActivityLease::new(request.activities.clone(), ActivityKind::QueuedVerification);
    let (provider, provider_digest) = tokio::task::spawn_blocking(move || {
        activity.start_verification();
        let _activity = activity;
        if let Some(native) = &worker_native {
            native.validate_quiescence()?;
        }
        worker_helper.validate_held()?;
        verify_codesign_identity(worker_helper.executable(), worker_request.clone())?;
        let tree = Arc::new(HeldTree::open(
            executable
                .parent()
                .ok_or(ProviderError::ArtifactVerification)?,
            &worker_request,
        )?);
        let verified = with_proof_tree(worker_request.clone(), Some(tree.clone()), None, || {
            sandbox::verify_packaged_artifact_details(&executable)
        })?;
        tree.check(&worker_request)?;
        Ok::<_, ProviderError>((
            tree,
            verified
                .identity
                .artifact_set_digest
                .ok_or(ProviderError::ArtifactVerification)?,
        ))
    })
    .await
    .map_err(|_| ProviderError::ArtifactVerification)??;
    if let Some(native) = &native {
        native.validate_quiescence()?;
    }
    helper.validate_held()?;
    provider.check(&request)?;
    abort_guard.0 = None;
    Ok((provider, provider_digest))
}

/// Complete content/signature provenance attached to held immutable file authorities.
pub struct VerifiedRuntimeArtifact {
    executable: PathBuf,
    identity: RuntimeArtifactIdentity,
    pub(crate) artifact: sandbox::VerifiedArtifact,
    tree: Arc<HeldTree>,
    custody: Option<Arc<ArtifactCustody>>,
}

struct ArtifactCustody {
    authority: crate::resource_custody::ResourceCustody,
    generation: String,
    readers: std::sync::Mutex<Vec<crate::resource_custody::ReaderCustody>>,
    clients: std::sync::atomic::AtomicU32,
}
impl Drop for ArtifactCustody {
    fn drop(&mut self) {
        for reader in self
            .readers
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .drain(..)
        {
            reader.retain_until_observed();
        }
    }
}
impl ArtifactCustody {
    fn can_close(&self) -> Result<(), ProviderError> {
        let readers = self
            .readers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.clients.load(Ordering::SeqCst) != 0
            || readers
                .iter()
                .any(|reader| reader.observe_cleanup().is_err())
        {
            return Err(ProviderError::ArtifactVerification);
        }
        Ok(())
    }
}
pub(crate) struct ArtifactClientCustody {
    artifact: Arc<ArtifactCustody>,
    reader: Option<crate::resource_custody::ReaderCustody>,
    request: VerificationRequest,
    tree: Arc<HeldTree>,
}
impl ArtifactClientCustody {
    pub(crate) fn retain_until_settlement(mut self) {
        let Some(reader) = self.reader.take() else {
            return;
        };
        let request = self.request.clone();
        let artifact = self.artifact.clone();
        let tree = self.tree.clone();
        tokio::spawn(async move {
            if request.await_revoked_settlement().await.is_err() {
                return;
            }
            drop(tree);
            if let Ok(proof) = reader.observe_cleanup()
                && reader.finish_observed(proof).is_ok()
            {
                artifact.clients.fetch_sub(1, Ordering::SeqCst);
            }
        });
    }
}
impl Drop for ArtifactClientCustody {
    fn drop(&mut self) {
        if let Some(reader) = self.reader.take()
            && let Ok(runtime) = tokio::runtime::Handle::try_current()
        {
            let request = self.request.clone();
            let artifact = self.artifact.clone();
            let tree = self.tree.clone();
            runtime.spawn(async move {
                if request.await_revoked_settlement().await.is_err() {
                    return;
                }
                drop(tree);
                if let Ok(proof) = reader.observe_cleanup()
                    && reader.finish_observed(proof).is_ok()
                {
                    artifact.clients.fetch_sub(1, Ordering::SeqCst);
                }
            });
        }
    }
}
impl VerifiedRuntimeArtifact {
    pub(crate) fn client_custody(
        &self,
        request: VerificationRequest,
    ) -> Result<Option<ArtifactClientCustody>, ProviderError> {
        let Some(artifact) = &self.custody else {
            #[cfg(test)]
            {
                return Ok(None);
            }
            #[cfg(not(test))]
            {
                return Err(ProviderError::ArtifactVerification);
            }
        };
        artifact.authority.check_provider_path(&self.executable)?;
        let reader = artifact
            .authority
            .begin_reader(&artifact.generation, request.clone())?;
        artifact.clients.fetch_add(1, Ordering::SeqCst);
        Ok(Some(ArtifactClientCustody {
            artifact: artifact.clone(),
            reader: Some(reader),
            request,
            tree: self.tree.clone(),
        }))
    }
    pub fn identity(&self) -> &RuntimeArtifactIdentity {
        &self.identity
    }
    pub fn executable(&self) -> &Path {
        &self.executable
    }
    pub fn check(&self, request: &VerificationRequest) -> Result<(), ProviderError> {
        request.check()?;
        if let Some(custody) = &self.custody {
            custody.authority.check_provider_path(&self.executable)?;
            for reader in custody
                .readers
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .iter()
            {
                reader.check_custody()?;
            }
        }
        if self
            .executable
            .canonicalize()
            .map_err(|_| ProviderError::ArtifactVerification)?
            != self.executable
        {
            return Err(ProviderError::ArtifactVerification);
        }
        self.tree.check(request)
    }
}

#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub struct VerificationMetrics {
    pub calls: u64,
    pub proofs: u64,
    pub reused: u64,
    pub hashed_bytes: u64,
    pub hash_micros: u64,
    pub subprocess_micros: u64,
    pub queue_micros: u64,
    pub held_checks: u64,
    pub held_check_micros: u64,
}

type Metrics = Arc<std::sync::Mutex<VerificationMetrics>>;

pub struct RuntimeVerificationService {
    state: tokio::sync::Mutex<Option<Arc<VerifiedRuntimeArtifact>>>,
    worker: Arc<tokio::sync::Semaphore>,
    metrics: Metrics,
    custody: Option<crate::resource_custody::ResourceCustody>,
    #[cfg(test)]
    controlled_fixture: bool,
}

impl Default for RuntimeVerificationService {
    fn default() -> Self {
        Self {
            state: tokio::sync::Mutex::new(None),
            worker: Arc::new(tokio::sync::Semaphore::new(1)),
            metrics: Arc::new(std::sync::Mutex::new(VerificationMetrics::default())),
            custody: None,
            #[cfg(test)]
            controlled_fixture: false,
        }
    }
}

struct VerificationCustody {
    reader: Option<crate::resource_custody::ReaderCustody>,
}
impl VerificationCustody {
    fn take(&mut self) -> Option<crate::resource_custody::ReaderCustody> {
        self.reader.take()
    }
}
impl Drop for VerificationCustody {
    fn drop(&mut self) {
        if let Some(reader) = self.reader.take() {
            reader.retain_until_observed();
        }
    }
}

struct RevokeOnDrop(Option<VerificationRequest>);
fn spawn_verification_worker<T: Send + 'static>(
    mut activity: ActivityLease,
    operation: impl FnOnce() -> T + Send + 'static,
) -> tokio::task::JoinHandle<T> {
    tokio::task::spawn_blocking(move || {
        activity.start_verification();
        let _activity = activity;
        operation()
    })
}
impl Drop for RevokeOnDrop {
    fn drop(&mut self) {
        if let Some(request) = &self.0 {
            request.revoke();
        }
    }
}

async fn wait_bounded<T>(
    request: &VerificationRequest,
    operation: impl std::future::Future<Output = T>,
) -> Result<T, ProviderError> {
    tokio::pin!(operation);
    loop {
        request.check()?;
        tokio::select! {
            value = &mut operation => { request.check()?; return Ok(value); },
            _ = tokio::time::sleep(Duration::from_millis(5)) => {},
        }
    }
}

impl RuntimeVerificationService {
    #[cfg(test)]
    pub(crate) fn controlled_fixture() -> Self {
        Self {
            controlled_fixture: true,
            ..Self::default()
        }
    }
    pub fn with_custody(custody: crate::resource_custody::ResourceCustody) -> Self {
        Self {
            custody: Some(custody),
            ..Self::default()
        }
    }
    /// Removes held cache authority only after all consumers and bound work have settled.
    pub async fn close_custody(&self) -> Result<(), ProviderError> {
        let _permit = self
            .worker
            .clone()
            .try_acquire_owned()
            .map_err(|_| ProviderError::ArtifactVerification)?;
        let mut state = self.state.lock().await;
        let Some(artifact) = state.take() else {
            return Ok(());
        };
        let artifact = match Arc::try_unwrap(artifact) {
            Ok(value) => value,
            Err(value) => {
                *state = Some(value);
                return Err(ProviderError::ArtifactVerification);
            }
        };
        if let Some(custody) = &artifact.custody
            && custody.can_close().is_err()
        {
            *state = Some(Arc::new(artifact));
            return Err(ProviderError::ArtifactVerification);
        }
        let VerifiedRuntimeArtifact { custody, tree, .. } = artifact;
        drop(tree);
        if let Some(custody) = custody {
            let mut readers = custody
                .readers
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            for reader in readers.drain(..) {
                let proof = reader.observe_cleanup()?;
                reader.finish_observed(proof)?;
            }
        }
        Ok(())
    }
    pub fn metrics(&self) -> VerificationMetrics {
        self.metrics
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    #[cfg(test)]
    pub(crate) async fn wait_for_worker_settlement(&self) -> Result<(), ProviderError> {
        let _permit =
            tokio::time::timeout(Duration::from_secs(2), self.worker.clone().acquire_owned())
                .await
                .map_err(|_| ProviderError::Timeout)?
                .map_err(|_| ProviderError::ArtifactVerification)?;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) async fn has_published_authority(&self) -> bool {
        self.state.lock().await.is_some()
    }

    pub async fn verify(
        &self,
        executable: PathBuf,
        request: VerificationRequest,
    ) -> Result<Arc<VerifiedRuntimeArtifact>, ProviderError> {
        request.check()?;
        let custody = self.custody.clone();
        if custody.is_none() {
            #[cfg(test)]
            if !self.controlled_fixture {
                return Err(ProviderError::ArtifactVerification);
            }
            #[cfg(not(test))]
            return Err(ProviderError::ArtifactVerification);
        }
        let queued_reader = if let Some(authority) = &custody {
            authority.check_provider_path(&executable)?;
            let workload_binding = format!(
                "{:x}",
                sha2::Sha256::digest(executable.as_os_str().as_encoded_bytes())
            );
            Some(authority.begin_reader(&workload_binding, request.clone())?)
        } else {
            None
        };
        let mut queued_reader = VerificationCustody {
            reader: queued_reader,
        };
        let mut guard = RevokeOnDrop(Some(request.clone()));
        let activity = request.publish(|| {
            ActivityLease::new(request.activities.clone(), ActivityKind::QueuedVerification)
        })?;
        let queued = Instant::now();
        self.metrics
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .calls += 1;
        let mut state = wait_bounded(&request, self.state.lock()).await?;
        request.check()?;
        let previous = state.take();
        let permit = wait_bounded(&request, self.worker.clone().acquire_owned())
            .await?
            .map_err(|_| ProviderError::ArtifactVerification)?;
        self.metrics
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .queue_micros += queued.elapsed().as_micros() as u64;
        let worker_request = request.clone();
        let metrics = self.metrics.clone();
        let result = spawn_verification_worker(activity, move || {
            let _permit = permit;
            worker_request.check()?;
            if let Some(artifact) = previous
                && artifact.executable == executable
            {
                let checking = Instant::now();
                let checked = artifact.check(&worker_request);
                {
                    let mut counts = metrics
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    counts.held_checks += 1;
                    counts.held_check_micros += checking.elapsed().as_micros() as u64;
                    if checked.is_ok() {
                        counts.reused += 1;
                    }
                }
                match checked {
                    Ok(()) => {
                        if let Some(custody) = &artifact.custody
                            && let Some(reader) = queued_reader.take()
                        {
                            custody
                                .readers
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                .push(reader);
                        }
                        return Ok(artifact);
                    }
                    Err(ProviderError::ArtifactVerification) => {}
                    Err(error) => return Err(error),
                }
            }

            let canonical = executable
                .canonicalize()
                .map_err(|_| ProviderError::ArtifactVerification)?;
            if canonical != executable {
                return Err(ProviderError::ArtifactVerification);
            }
            let root = executable
                .parent()
                .ok_or(ProviderError::ArtifactVerification)?;
            let tree = Arc::new(HeldTree::open(root, &worker_request)?);
            metrics
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .proofs += 1;
            let artifact = with_proof_tree(
                worker_request.clone(),
                Some(tree.clone()),
                Some(metrics),
                || sandbox::verify_packaged_artifact_details(&executable),
            )?;
            let identity = artifact.identity.clone();
            tree.check(&worker_request)?;
            let binding = if let Some(custody) = custody {
                let generation = identity
                    .artifact_set_digest
                    .clone()
                    .ok_or(ProviderError::ArtifactVerification)?;
                let verified_reader = custody.begin_reader(&generation, worker_request.clone())?;
                Some(Arc::new(ArtifactCustody {
                    authority: custody,
                    generation,
                    readers: std::sync::Mutex::new(vec![
                        queued_reader
                            .take()
                            .ok_or(ProviderError::ArtifactVerification)?,
                        verified_reader,
                    ]),
                    clients: std::sync::atomic::AtomicU32::new(0),
                }))
            } else {
                None
            };
            Ok(Arc::new(VerifiedRuntimeArtifact {
                executable,
                identity,
                artifact,
                tree,
                custody: binding,
            }))
        })
        .await
        .map_err(|_| ProviderError::ArtifactVerification)?;
        request.check()?;
        let artifact = result?;
        request.publish(|| {
            *state = Some(artifact.clone());
        })?;
        guard.0 = None;
        Ok(artifact)
    }
}

struct ProofContext {
    request: VerificationRequest,
    hashes: HashMap<PathBuf, String>,
    tree: Option<Arc<HeldTree>>,
    metrics: Option<Metrics>,
}
thread_local! { static PROOF: RefCell<Option<ProofContext>> = const { RefCell::new(None) }; }

pub(crate) fn with_bounded_proof<T>(
    request: VerificationRequest,
    operation: impl FnOnce() -> Result<T, ProviderError>,
) -> Result<T, ProviderError> {
    with_proof_tree(request, None, None, operation)
}

fn with_proof_tree<T>(
    request: VerificationRequest,
    tree: Option<Arc<HeldTree>>,
    metrics: Option<Metrics>,
    operation: impl FnOnce() -> Result<T, ProviderError>,
) -> Result<T, ProviderError> {
    struct Clear;
    impl Drop for Clear {
        fn drop(&mut self) {
            PROOF.with(|proof| *proof.borrow_mut() = None);
        }
    }
    PROOF.with(|proof| {
        *proof.borrow_mut() = Some(ProofContext {
            request,
            hashes: HashMap::new(),
            tree,
            metrics,
        })
    });
    let _clear = Clear;
    operation()
}

pub(crate) fn record_hash(bytes: u64, elapsed: Duration) {
    PROOF.with(|proof| {
        if let Some(metrics) = proof
            .borrow()
            .as_ref()
            .and_then(|proof| proof.metrics.as_ref())
        {
            let mut metrics = metrics
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            metrics.hashed_bytes += bytes;
            metrics.hash_micros += elapsed.as_micros() as u64;
        }
    });
}

struct SubprocessMetric {
    started: Instant,
    metrics: Option<Metrics>,
}
impl Drop for SubprocessMetric {
    fn drop(&mut self) {
        if let Some(metrics) = &self.metrics {
            metrics
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .subprocess_micros += self.started.elapsed().as_micros() as u64;
        }
    }
}

pub(crate) fn checkpoint() -> Result<(), ProviderError> {
    PROOF.with(|proof| {
        proof
            .borrow()
            .as_ref()
            .map_or(Ok(()), |proof| proof.request.check())
    })
}

pub(crate) fn open_proof_file(path: &Path) -> Result<File, ProviderError> {
    use std::io::{Seek, SeekFrom};
    checkpoint()?;
    PROOF.with(|proof| {
        let proof = proof.borrow();
        if let Some(tree) = proof.as_ref().and_then(|proof| proof.tree.as_ref()) {
            let held = tree
                .files
                .iter()
                .find(|held| held.path == path)
                .ok_or(ProviderError::ArtifactVerification)?;
            held.check()?;
            let mut file = held
                .file
                .try_clone()
                .map_err(|_| ProviderError::ArtifactVerification)?;
            file.seek(SeekFrom::Start(0))
                .map_err(|_| ProviderError::ArtifactVerification)?;
            Ok(file)
        } else {
            OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
                .open(path)
                .map_err(|_| ProviderError::ArtifactVerification)
        }
    })
}

pub(crate) fn cached_hash(path: &Path) -> Option<String> {
    PROOF.with(|proof| {
        proof
            .borrow()
            .as_ref()
            .and_then(|proof| proof.hashes.get(path).cloned())
    })
}

pub(crate) fn remember_hash(path: &Path, digest: &str) {
    PROOF.with(|proof| {
        if let Some(proof) = proof.borrow_mut().as_mut() {
            proof.hashes.insert(path.to_owned(), digest.to_owned());
        }
    });
}

/// Verifies the fixed Developer ID team without allowing a caller-selected subprocess.
/// The request deadline covers both signature operations and their owned cleanup.
pub fn verify_codesign_identity(
    path: &Path,
    request: VerificationRequest,
) -> Result<(), ProviderError> {
    with_bounded_proof(request, || {
        let verify = bounded_output_with_limit(
            std::process::Command::new("/usr/bin/codesign")
                .args(["--verify", "--strict", "--verbose=0"])
                .arg(path),
            16 * 1024,
        )?;
        if !verify.status.success() {
            return Err(ProviderError::ArtifactVerification);
        }
        let display = bounded_output_with_limit(
            std::process::Command::new("/usr/bin/codesign")
                .args(["--display", "--verbose=4"])
                .arg(path),
            16 * 1024,
        )?;
        if !display.status.success()
            || !std::str::from_utf8(&display.stderr)
                .map_err(|_| ProviderError::ArtifactVerification)?
                .lines()
                .any(|line| line == "TeamIdentifier=95B7J2U49K")
        {
            return Err(ProviderError::ArtifactVerification);
        }
        checkpoint()
    })
}

pub(crate) fn bounded_output(
    command: &mut std::process::Command,
) -> Result<std::process::Output, ProviderError> {
    bounded_output_with_limit(command, 65_536)
}

fn bounded_output_with_limit(
    command: &mut std::process::Command,
    output_limit: usize,
) -> Result<std::process::Output, ProviderError> {
    use std::os::unix::process::CommandExt;
    checkpoint()?;
    let request = PROOF
        .with(|proof| proof.borrow().as_ref().map(|proof| proof.request.clone()))
        .unwrap_or_else(|| VerificationRequest::until(Instant::now() + Duration::from_secs(15)));
    let deadline = request
        .deadline
        .min(Instant::now() + Duration::from_secs(15));
    let activity = ActivityLease::new(request.activities.clone(), ActivityKind::VerificationWorker);
    let child = request
        .publish(|| {
            command
                .env_clear()
                .env("LC_ALL", "C")
                .process_group(0)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
        })?
        .map_err(|_| ProviderError::ArtifactVerification)?;
    let metrics = PROOF.with(|proof| {
        proof
            .borrow()
            .as_ref()
            .and_then(|proof| proof.metrics.clone())
    });
    supervise_owned_child(child, request, deadline, activity, metrics, output_limit)
}
fn supervise_owned_child(
    child: std::process::Child,
    request: VerificationRequest,
    deadline: Instant,
    activity: ActivityLease,
    metrics: Option<Metrics>,
    output_limit: usize,
) -> Result<std::process::Output, ProviderError> {
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    let worker_request = request.clone();
    std::thread::spawn(move || {
        let mut activity = activity;
        activity.mark_owned_process();
        let pid = child.id() as i32;
        let result = with_proof_tree(worker_request, None, metrics, || {
            bounded_child_output(child, deadline, output_limit)
        });
        let cleanup_deadline = Instant::now() + Duration::from_secs(1);
        let settled = loop {
            let exists = unsafe { libc::kill(-pid, 0) };
            if exists != 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
                break true;
            }
            if Instant::now() >= cleanup_deadline {
                break false;
            }
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        if settled {
            activity.settle();
        } else {
            drop(activity);
        }
        let _ = sender.send(if settled {
            result
        } else {
            Err(ProviderError::ArtifactVerification)
        });
    });
    loop {
        request.check()?;
        if Instant::now() >= deadline {
            return Err(ProviderError::Timeout);
        }
        match receiver.recv_timeout(
            deadline
                .saturating_duration_since(Instant::now())
                .min(Duration::from_millis(5)),
        ) {
            Ok(result) => return result,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                return Err(ProviderError::ArtifactVerification);
            }
        }
    }
}

fn bounded_child_output(
    child: std::process::Child,
    deadline: Instant,
    output_limit: usize,
) -> Result<std::process::Output, ProviderError> {
    use std::{io::Read, os::fd::AsRawFd};
    let _metric = SubprocessMetric {
        started: Instant::now(),
        metrics: PROOF.with(|proof| {
            proof
                .borrow()
                .as_ref()
                .and_then(|proof| proof.metrics.clone())
        }),
    };
    let request = PROOF.with(|proof| proof.borrow().as_ref().unwrap().request.clone());
    struct ChildGuard {
        child: std::process::Child,
        armed: bool,
    }
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            if self.armed {
                unsafe {
                    libc::kill(-(self.child.id() as i32), libc::SIGKILL);
                }
                let _ = self.child.kill();
                let _ = self.child.wait();
            }
        }
    }
    let mut guard = ChildGuard { child, armed: true };
    let reader_failed = Arc::new(AtomicBool::new(false));
    fn reader(
        mut stream: impl Read + AsRawFd + Send + 'static,
        deadline: Instant,
        request: VerificationRequest,
        output_limit: usize,
        failed: Arc<AtomicBool>,
    ) -> std::thread::JoinHandle<Result<Vec<u8>, ProviderError>> {
        std::thread::spawn(move || {
            let flags = unsafe { libc::fcntl(stream.as_raw_fd(), libc::F_GETFL) };
            if flags < 0
                || unsafe {
                    libc::fcntl(stream.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK)
                } < 0
            {
                return Err(ProviderError::ArtifactVerification);
            }
            let mut bytes = Vec::new();
            let mut buffer = [0; 4096];
            loop {
                request.check()?;
                if Instant::now() >= deadline {
                    return Err(ProviderError::Timeout);
                }
                match stream.read(&mut buffer) {
                    Ok(0) => return Ok(bytes),
                    Ok(read) => {
                        if bytes.len() + read > output_limit {
                            failed.store(true, Ordering::SeqCst);
                            return Err(ProviderError::ArtifactVerification);
                        }
                        bytes.extend_from_slice(&buffer[..read]);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5))
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(_) => return Err(ProviderError::ArtifactVerification),
                }
            }
        })
    }
    let stdout = reader(
        guard
            .child
            .stdout
            .take()
            .ok_or(ProviderError::ArtifactVerification)?,
        deadline,
        request.clone(),
        output_limit,
        reader_failed.clone(),
    );
    let stderr = reader(
        guard
            .child
            .stderr
            .take()
            .ok_or(ProviderError::ArtifactVerification)?,
        deadline,
        request.clone(),
        output_limit,
        reader_failed.clone(),
    );
    let result = (|| {
        let status = loop {
            if reader_failed.load(Ordering::SeqCst) {
                return Err(ProviderError::ArtifactVerification);
            }
            request.check()?;
            if Instant::now() >= deadline {
                return Err(ProviderError::Timeout);
            }
            match guard.child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => std::thread::sleep(Duration::from_millis(5)),
                Err(_) => return Err(ProviderError::ArtifactVerification),
            }
        };
        Ok(status)
    })();
    if result.is_err() {
        unsafe {
            libc::kill(-(guard.child.id() as i32), libc::SIGKILL);
        }
        let _ = guard.child.kill();
        let _ = guard.child.wait();
    }
    let stdout = stdout
        .join()
        .map_err(|_| ProviderError::ArtifactVerification)?;
    let stderr = stderr
        .join()
        .map_err(|_| ProviderError::ArtifactVerification)?;
    let output = std::process::Output {
        status: result?,
        stdout: stdout?,
        stderr: stderr?,
    };
    guard.armed = false;
    Ok(output)
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn aborted_caller_cannot_settle_its_live_blocking_worker() {
        let request = VerificationRequest::until(Instant::now() + Duration::from_secs(2));
        let activity = request
            .publish(|| {
                ActivityLease::new(request.activities.clone(), ActivityKind::QueuedVerification)
            })
            .unwrap();
        let release = Arc::new(std::sync::Barrier::new(2));
        let held = release.clone();
        let (entered, observed) = tokio::sync::oneshot::channel();
        let caller = tokio::spawn(async move {
            spawn_verification_worker(activity, move || {
                let _ = entered.send(());
                held.wait();
            })
            .await
        });
        observed.await.unwrap();
        assert_eq!(request.settlement().queued_verifications, 0);
        assert_eq!(request.settlement().verification_workers, 1);
        caller.abort();
        assert!(caller.await.unwrap_err().is_cancelled());
        request.revoke();
        assert!(matches!(
            request
                .wait_for_settlement(Instant::now() + Duration::from_millis(30))
                .await,
            Err(ProviderError::Timeout)
        ));
        assert_eq!(request.settlement().verification_workers, 1);
        release.wait();
        assert!(
            request
                .wait_for_settlement(Instant::now() + Duration::from_secs(1))
                .await
                .unwrap()
                .is_settled()
        );
    }

    #[tokio::test]
    async fn unresolved_process_cleanup_never_becomes_settled_on_expiry_or_drop() {
        let request = VerificationRequest::until(Instant::now());
        let derived = request.with_deadline(Instant::now() + Duration::from_secs(1));
        let mut process = derived.provider_activity().unwrap();
        process.mark_owned_process();
        assert_eq!(request.settlement().provider_operations, 1);
        drop(process);
        request.revoke();
        assert_eq!(request.settlement().provider_operations, 0);
        assert_eq!(request.settlement().unresolved_cleanup, 1);
        assert!(matches!(
            request
                .wait_for_settlement(Instant::now() + Duration::from_millis(20))
                .await,
            Err(ProviderError::Timeout)
        ));
        assert!(matches!(
            derived.provider_activity(),
            Err(ProviderError::Cancelled)
        ));
    }
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static NEXT_FIXTURE: std::sync::atomic::AtomicU64 =
                std::sync::atomic::AtomicU64::new(1);
            let ordinal = NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().canonicalize().unwrap().join(format!(
                "verification-authority-{}-{}-{}",
                std::process::id(),
                ordinal,
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn tree(&self) -> PathBuf {
            let path = self.0.join("tree");
            fs::create_dir(&path).unwrap();
            fs::write(path.join("payload"), b"verified generation").unwrap();
            fs::set_permissions(path.join("payload"), fs::Permissions::from_mode(0o444)).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o555)).unwrap();
            path
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            for directory in [
                self.0.join("tree"),
                self.0.join("old"),
                self.0.join("parent/tree"),
                self.0.join("moved/tree"),
            ] {
                let _ = fs::set_permissions(directory, fs::Permissions::from_mode(0o755));
            }
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn all_derived_client_effects_share_one_global_revocation_fence() {
        let root = VerificationRequest::until(Instant::now() + Duration::from_secs(1));
        let clients: Vec<_> = (0..3)
            .map(|_| {
                root.with_deadline(Instant::now() + Duration::from_secs(2))
                    .effect_authority()
            })
            .collect();
        for client in &clients {
            assert!(Arc::ptr_eq(&client.publication, &root.publication));
            assert!(client.publish(|| ()).is_ok());
        }
        root.revoke();
        for client in &clients {
            assert!(matches!(
                client.publish(|| panic!("Revoked client cannot publish an effect")),
                Err(ProviderError::Cancelled)
            ));
        }
    }

    fn request() -> VerificationRequest {
        VerificationRequest::until(Instant::now() + Duration::from_secs(5))
    }

    #[test]
    fn revocation_winning_after_the_last_check_prevents_publication() {
        let authority = request();
        let checked = Arc::new(std::sync::Barrier::new(2));
        let resume = Arc::new(std::sync::Barrier::new(2));
        let published = Arc::new(AtomicBool::new(false));
        let producer = {
            let authority = authority.clone();
            let checked = checked.clone();
            let resume = resume.clone();
            let published = published.clone();
            std::thread::spawn(move || {
                authority.check().unwrap();
                checked.wait();
                resume.wait();
                authority.publish(|| published.store(true, Ordering::SeqCst))
            })
        };
        checked.wait();
        authority.revoke();
        resume.wait();
        assert!(matches!(
            producer.join().unwrap(),
            Err(ProviderError::Cancelled)
        ));
        assert!(!published.load(Ordering::SeqCst));
    }

    #[test]
    fn publication_winning_first_is_ordered_before_revocation() {
        let authority = request();
        let entered = Arc::new(std::sync::Barrier::new(2));
        let resume = Arc::new(std::sync::Barrier::new(2));
        let published = Arc::new(AtomicBool::new(false));
        let producer = {
            let authority = authority.clone();
            let entered = entered.clone();
            let resume = resume.clone();
            let published = published.clone();
            std::thread::spawn(move || {
                authority.publish(|| {
                    entered.wait();
                    resume.wait();
                    published.store(true, Ordering::SeqCst);
                })
            })
        };
        entered.wait();
        let revoked = Arc::new(AtomicBool::new(false));
        let revoker = {
            let authority = authority.clone();
            let revoked = revoked.clone();
            std::thread::spawn(move || {
                authority.revoke();
                revoked.store(true, Ordering::SeqCst);
            })
        };
        assert!(!revoked.load(Ordering::SeqCst));
        resume.wait();
        producer.join().unwrap().unwrap();
        revoker.join().unwrap();
        assert!(published.load(Ordering::SeqCst));
        assert!(matches!(authority.check(), Err(ProviderError::Cancelled)));
    }

    #[test]
    fn owner_writable_directory_cannot_create_held_authority() {
        let fixture = Fixture::new();
        let root = fixture.tree();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(HeldTree::open(&root, &request()).is_err());
    }

    #[test]
    fn held_authority_rejects_permissions_links_content_and_path_replacement() {
        let fixture = Fixture::new();
        let root = fixture.tree();
        let held = HeldTree::open(&root, &request()).unwrap();
        held.check(&request()).unwrap();
        let payload = root.join("payload");
        fs::set_permissions(&payload, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(held.check(&request()).is_err());
        fs::set_permissions(&payload, fs::Permissions::from_mode(0o444)).unwrap();
        assert!(held.check(&request()).is_err());
        let held = HeldTree::open(&root, &request()).unwrap();
        fs::hard_link(&payload, fixture.0.join("alias")).unwrap();
        assert!(held.check(&request()).is_err());
        fs::remove_file(fixture.0.join("alias")).unwrap();
        let held = HeldTree::open(&root, &request()).unwrap();
        fs::set_permissions(&payload, fs::Permissions::from_mode(0o644)).unwrap();
        fs::write(&payload, b"modified generation").unwrap();
        fs::set_permissions(&payload, fs::Permissions::from_mode(0o444)).unwrap();
        assert!(held.check(&request()).is_err());
        let held = HeldTree::open(&root, &request()).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();
        fs::rename(&root, fixture.0.join("old")).unwrap();
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o555)).unwrap();
        assert!(held.check(&request()).is_err());
    }

    #[test]
    fn held_authority_rejects_parent_symlink_even_to_the_same_files() {
        let fixture = Fixture::new();
        let root = fixture.tree();
        let parent = fixture.0.join("parent");
        fs::create_dir(&parent).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();
        fs::rename(&root, parent.join("tree")).unwrap();
        fs::set_permissions(parent.join("tree"), fs::Permissions::from_mode(0o555)).unwrap();
        let held = HeldTree::open(&parent.join("tree"), &request()).unwrap();
        fs::rename(&parent, fixture.0.join("moved")).unwrap();
        std::os::unix::fs::symlink(fixture.0.join("moved"), &parent).unwrap();
        assert!(held.check(&request()).is_err());
        fs::remove_file(parent).unwrap();
    }

    #[test]
    fn proof_reads_the_held_descriptor_and_rejects_late_revoked_results() {
        let fixture = Fixture::new();
        let root = fixture.tree();
        let authority = request();
        let tree = Arc::new(HeldTree::open(&root, &authority).unwrap());
        let revoked = authority.clone();
        let outcome = with_proof_tree(authority, Some(tree), None, || {
            use std::io::Read;
            let mut bytes = Vec::new();
            open_proof_file(&root.join("payload"))?
                .read_to_end(&mut bytes)
                .unwrap();
            assert_eq!(bytes, b"verified generation");
            revoked.revoke();
            checkpoint()
        });
        assert!(matches!(outcome, Err(ProviderError::Cancelled)));
        assert!(checkpoint().is_ok());
    }

    fn ready_owned_child(
        command: &mut std::process::Command,
        markers: &[&Path],
    ) -> std::process::Child {
        use std::os::unix::process::CommandExt;
        let mut child = command
            .env_clear()
            .env("LC_ALL", "C")
            .process_group(0)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let setup_deadline = Instant::now() + Duration::from_secs(5);
        while markers.iter().any(|marker| !marker.exists()) {
            if Instant::now() >= setup_deadline || child.try_wait().unwrap().is_some() {
                unsafe {
                    libc::kill(-(child.id() as i32), libc::SIGKILL);
                }
                let _ = child.kill();
                let _ = child.wait();
                panic!("owned supervision fixture did not become ready");
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        child
    }
    fn supervise_ready_fixture(
        child: std::process::Child,
        request: VerificationRequest,
        output_limit: usize,
    ) -> Result<std::process::Output, ProviderError> {
        let activity = request
            .publish(|| {
                ActivityLease::new(request.activities.clone(), ActivityKind::VerificationWorker)
            })
            .unwrap();
        let deadline = request.deadline;
        supervise_owned_child(child, request, deadline, activity, None, output_limit)
    }
    #[test]
    fn verification_subprocess_deadline_kills_reaps_and_closes_descendant_pipes() {
        use std::os::unix::ffi::OsStrExt;
        let fixture = Fixture::new();
        let parent_path = fixture.0.join("parent-pid");
        let descendant_path = fixture.0.join("descendant-pid");
        let child = ready_owned_child(
            std::process::Command::new("/bin/sh")
                .arg("-c")
                .arg("echo $$ > \"$1\"; sleep 30 & descendant=$!; echo $descendant > \"$2\"; wait")
                .arg("owned-deadline-control")
                .arg(&parent_path)
                .arg(&descendant_path),
            &[&parent_path, &descendant_path],
        );
        let deadline = VerificationRequest::until(Instant::now() + Duration::from_millis(200));
        let started = Instant::now();
        let outcome = supervise_ready_fixture(child, deadline, 65_536);
        assert!(matches!(outcome, Err(ProviderError::Timeout)));
        assert!(started.elapsed() < Duration::from_secs(2));
        let pids: Vec<i32> = [&parent_path, &descendant_path]
            .into_iter()
            .map(|path| fs::read_to_string(path).unwrap().trim().parse().unwrap())
            .collect();
        assert_ne!(pids[0], pids[1]);
        let settled = Instant::now() + Duration::from_secs(2);
        for pid in pids {
            loop {
                let exists = unsafe { libc::kill(pid, 0) } == 0;
                if !exists {
                    break;
                }
                assert!(Instant::now() < settled, "owned subprocess did not settle");
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        let fifo = fixture.0.join("fifo");
        let path = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o444) }, 0);
        let started = Instant::now();
        assert!(HeldFile::open(fifo).is_err());
        assert!(started.elapsed() < Duration::from_millis(100));
        let output =
            bounded_output(std::process::Command::new("/usr/bin/printf").arg("verified")).unwrap();
        assert_eq!(output.stdout, b"verified");
        assert!(output.status.success());
    }

    #[test]
    fn verification_subprocess_output_limit_never_retains_unbounded_data() {
        let outcome = bounded_output(
            std::process::Command::new("/bin/sh")
                .arg("-c")
                .arg("yes canary-private-value"),
        );
        assert!(matches!(
            outcome,
            Err(ProviderError::ArtifactVerification) | Err(ProviderError::Timeout)
        ));
    }

    #[test]
    fn codesign_supervision_uses_logical_limit_and_retains_cleanup_ownership() {
        let fixture = Fixture::new();
        let ready = fixture.0.join("output-ready");
        let child = ready_owned_child(
            std::process::Command::new("/bin/sh")
                .arg("-c")
                .arg("echo ready > \"$1\"; yes bounded-control")
                .arg("output-control")
                .arg(&ready),
            &[&ready],
        );
        let request = VerificationRequest::until(Instant::now() + Duration::from_secs(2));
        let started = Instant::now();
        let result = supervise_ready_fixture(child, request.clone(), 16 * 1024);
        assert!(matches!(result, Err(ProviderError::ArtifactVerification)));
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(request.settlement().is_settled());
    }

    #[test]
    fn codesign_supervision_deadline_returns_before_owned_cleanup_and_then_settles() {
        let fixture = Fixture::new();
        let pid_path = fixture.0.join("supervised-parent");
        let descendant_path = fixture.0.join("supervised-descendant");
        let child = ready_owned_child(
            std::process::Command::new("/bin/sh")
                .arg("-c")
                .arg("echo $$ > \"$1\"; sleep 30 & echo $! > \"$2\"; wait")
                .arg("supervised-control")
                .arg(&pid_path)
                .arg(&descendant_path),
            &[&pid_path, &descendant_path],
        );
        let request = VerificationRequest::until(Instant::now() + Duration::from_millis(150));
        let started = Instant::now();
        let result = supervise_ready_fixture(child, request.clone(), 16 * 1024);
        assert!(matches!(result, Err(ProviderError::Timeout)));
        assert!(started.elapsed() < Duration::from_secs(1));
        let deadline = Instant::now() + Duration::from_secs(2);
        while !request.settlement().is_settled() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        for path in [&pid_path, &descendant_path] {
            let pid: i32 = fs::read_to_string(path).unwrap().trim().parse().unwrap();
            assert_ne!(unsafe { libc::kill(pid, 0) }, 0);
            assert_eq!(
                std::io::Error::last_os_error().raw_os_error(),
                Some(libc::ESRCH)
            );
        }
    }

    #[test]
    fn codesign_fixed_api_rejects_revoked_request_without_starting_work() {
        let request = VerificationRequest::until(Instant::now() + Duration::from_secs(1));
        request.revoke();
        assert!(matches!(
            verify_codesign_identity(Path::new("/unreachable"), request.clone()),
            Err(ProviderError::Cancelled)
        ));
        assert!(request.settlement().is_settled());
    }

    #[tokio::test]
    async fn custody_attachment_default_has_no_uncustodied_verification_fallback() {
        let service = RuntimeVerificationService::default();
        let root = request();
        assert!(matches!(
            service
                .verify(PathBuf::from("/unreachable"), root.clone())
                .await,
            Err(ProviderError::ArtifactVerification)
        ));
        assert_eq!(service.metrics().proofs, 0);
        assert!(root.settlement().is_settled());
    }
    fn fixture_custody(f: &Fixture) -> crate::resource_custody::ResourceCustody {
        crate::resource_custody::ResourceCustody::bootstrap_empty(
            crate::resource_custody::ArtifactNamespace::development(
                &f.0,
                crate::resource_custody::DevelopmentProfile::Debug,
            )
            .unwrap(),
        )
        .unwrap()
    }
    #[tokio::test]
    async fn custody_attachment_aborted_verifier_keeps_reader_until_actual_worker_finishes() {
        let f = Fixture::new();
        let authority = fixture_custody(&f);
        let root = request();
        let reader = authority
            .begin_reader(&"a".repeat(64), root.clone())
            .unwrap();
        let activity = root
            .publish(|| {
                ActivityLease::new(root.activities.clone(), ActivityKind::QueuedVerification)
            })
            .unwrap();
        let (release, blocked) = std::sync::mpsc::channel();
        let (started, observed) = tokio::sync::oneshot::channel();
        let worker = spawn_verification_worker(activity, move || {
            let _custody = VerificationCustody {
                reader: Some(reader),
            };
            started.send(()).unwrap();
            blocked.recv().unwrap();
        });
        observed.await.unwrap();
        root.revoke();
        worker.abort();
        assert_eq!(authority.unresolved_operations().unwrap(), 1);
        assert_eq!(root.settlement().verification_workers, 1);
        release.send(()).unwrap();
        root.wait_for_settlement(Instant::now() + Duration::from_secs(2))
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while authority.unresolved_operations().unwrap() != 0 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
    }
    #[tokio::test]
    async fn custody_attachment_three_clients_and_cache_wait_for_real_cleanup() {
        let f = Fixture::new();
        let authority = fixture_custody(&f);
        let root = request();
        let tree = Arc::new(HeldTree::open(&f.tree(), &root).unwrap());
        let binding = Arc::new(ArtifactCustody {
            authority: authority.clone(),
            generation: "b".repeat(64),
            readers: std::sync::Mutex::new(vec![
                authority
                    .begin_reader(&"b".repeat(64), root.clone())
                    .unwrap(),
            ]),
            clients: std::sync::atomic::AtomicU32::new(3),
        });
        let mut actual_work = root.provider_activity().unwrap();
        actual_work.mark_owned_process();
        for _ in 0..3 {
            ArtifactClientCustody {
                artifact: binding.clone(),
                reader: Some(
                    authority
                        .begin_reader(&binding.generation, root.clone())
                        .unwrap(),
                ),
                request: root.clone(),
                tree: tree.clone(),
            }
            .retain_until_settlement();
        }
        root.revoke();
        assert!(binding.can_close().is_err());
        assert_eq!(authority.unresolved_operations().unwrap(), 4);
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(binding.clients.load(Ordering::SeqCst), 3);
        actual_work.settle();
        tokio::time::timeout(Duration::from_secs(2), async {
            while binding.clients.load(Ordering::SeqCst) != 0 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        binding.can_close().unwrap();
        assert_eq!(authority.unresolved_operations().unwrap(), 1);
        drop(tree);
        drop(binding);
        tokio::time::timeout(Duration::from_secs(2), async {
            while authority.unresolved_operations().unwrap() != 0 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
    }
    #[test]
    fn custody_attachment_effect_lease_rejects_other_root_and_retains_unknown_cleanup() {
        let root = request();
        let wrong = request();
        let lease = root.track_stream_operation().unwrap();
        assert!(
            wrong
                .with_stream_effect_admission(&lease, || panic!("wrong root effect"))
                .is_err()
        );
        root.revoke();
        assert!(
            root.with_stream_effect_admission(&lease, || panic!("revoked effect"))
                .is_err()
        );
        lease.retain_unresolved();
        assert_eq!(root.settlement().unresolved_cleanup, 1);
        assert!(!root.settlement().is_settled());
    }
    #[tokio::test]
    async fn revoked_and_expired_requests_never_start_or_publish_verification() {
        let service = RuntimeVerificationService::default();
        let revoked = request();
        revoked.revoke();
        assert!(matches!(
            service.verify(PathBuf::from("/unreachable"), revoked).await,
            Err(ProviderError::Cancelled)
        ));
        assert!(matches!(
            service
                .verify(
                    PathBuf::from("/unreachable"),
                    VerificationRequest::until(Instant::now())
                )
                .await,
            Err(ProviderError::Timeout)
        ));
        assert_eq!(service.metrics().proofs, 0);
        assert!(service.state.lock().await.is_none());
    }
}
