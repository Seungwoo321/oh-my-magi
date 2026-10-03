use crate::{ProviderError, VerificationRequest};
use fs2::FileExt;
use rusqlite::{Connection, TransactionBehavior, params};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

const SCHEMA_V1: &str = "
CREATE TABLE metadata(namespace TEXT PRIMARY KEY, version INTEGER NOT NULL CHECK(version=1), lock_identity TEXT NOT NULL, journal_identity TEXT NOT NULL);
CREATE TABLE operations(operation TEXT PRIMARY KEY, owner TEXT NOT NULL, generation TEXT NOT NULL);
CREATE TABLE observations(sequence INTEGER PRIMARY KEY AUTOINCREMENT, operation TEXT NOT NULL REFERENCES operations(operation), state TEXT NOT NULL CHECK(state IN ('unresolved','observed_stopped')));
CREATE TRIGGER operations_no_update BEFORE UPDATE ON operations BEGIN SELECT RAISE(ABORT,'immutable custody operation'); END;
CREATE TRIGGER operations_no_delete BEFORE DELETE ON operations BEGIN SELECT RAISE(ABORT,'immutable custody operation'); END;
CREATE TRIGGER observations_no_update BEFORE UPDATE ON observations BEGIN SELECT RAISE(ABORT,'immutable custody observation'); END;
CREATE TRIGGER observations_no_delete BEFORE DELETE ON observations BEGIN SELECT RAISE(ABORT,'immutable custody observation'); END;
";

const PUBLICATION_SCHEMA: &str = "
CREATE TABLE namespace_policy(singleton INTEGER PRIMARY KEY CHECK(singleton=1),requires_installation INTEGER NOT NULL CHECK(requires_installation IN (0,1)));
CREATE TRIGGER namespace_policy_no_update BEFORE UPDATE ON namespace_policy BEGIN SELECT RAISE(ABORT,'immutable namespace policy'); END;
CREATE TRIGGER namespace_policy_no_delete BEFORE DELETE ON namespace_policy BEGIN SELECT RAISE(ABORT,'immutable namespace policy'); END;
CREATE TABLE publication_operations(operation TEXT PRIMARY KEY REFERENCES operations(operation));
CREATE TABLE publication_outcomes(sequence INTEGER PRIMARY KEY AUTOINCREMENT,operation TEXT NOT NULL UNIQUE REFERENCES publication_operations(operation),outcome TEXT NOT NULL CHECK(outcome IN ('committed','rolled_back')),generation TEXT);
CREATE TABLE installed_generations(sequence INTEGER PRIMARY KEY AUTOINCREMENT,generation TEXT NOT NULL);
CREATE TRIGGER publication_operations_no_update BEFORE UPDATE ON publication_operations BEGIN SELECT RAISE(ABORT,'immutable publication operation'); END;
CREATE TRIGGER publication_operations_no_delete BEFORE DELETE ON publication_operations BEGIN SELECT RAISE(ABORT,'immutable publication operation'); END;
CREATE TRIGGER publication_outcomes_no_update BEFORE UPDATE ON publication_outcomes BEGIN SELECT RAISE(ABORT,'immutable publication outcome'); END;
CREATE TRIGGER publication_outcomes_no_delete BEFORE DELETE ON publication_outcomes BEGIN SELECT RAISE(ABORT,'immutable publication outcome'); END;
CREATE TRIGGER installed_generations_no_update BEFORE UPDATE ON installed_generations BEGIN SELECT RAISE(ABORT,'immutable installed generation'); END;
CREATE TRIGGER installed_generations_no_delete BEFORE DELETE ON installed_generations BEGIN SELECT RAISE(ABORT,'immutable installed generation'); END;
";
fn schema_v2() -> String {
    format!(
        "{}{}",
        SCHEMA_V1.replace("CHECK(version=1)", "CHECK(version=2)"),
        PUBLICATION_SCHEMA
    )
}

fn invalid() -> ProviderError {
    ProviderError::ArtifactVerification
}
fn hex_digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn nonce() -> Result<String, ProviderError> {
    let mut bytes = [0; 16];
    File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut bytes))
        .map_err(|_| invalid())?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}
fn valid_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[derive(Clone, Copy)]
pub enum DevelopmentProfile {
    Debug,
    Release,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum NamespacePurpose {
    Development,
    InstalledReadOnly,
}

/// One stable publication location covering both provider and extraction resources.
/// The control directory is independent of execution stores and generated generations.
pub struct ArtifactNamespace {
    purpose: NamespacePurpose,
    publication: PathBuf,
    control: PathBuf,
    digest: String,
    publication_ancestors: Vec<(PathBuf, File, Identity)>,
}
impl ArtifactNamespace {
    /// Uses native-resolved bundle resources and application configuration paths.
    /// Control state never resides in the bundle or an execution-store backup.
    pub fn installed_resources(resources: &Path, app_config: &Path) -> Result<Self, ProviderError> {
        if !resources.is_absolute() || !app_config.is_absolute() {
            return Err(invalid());
        }
        let resources = resources.to_path_buf();
        let bundle = resources
            .parent()
            .and_then(Path::parent)
            .ok_or_else(invalid)?;
        if resources.file_name().and_then(|name| name.to_str()) != Some("Resources")
            || resources
                .parent()
                .and_then(Path::file_name)
                .and_then(|name| name.to_str())
                != Some("Contents")
            || !resources
                .parent()
                .and_then(Path::parent)
                .and_then(Path::file_name)
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".app"))
            || app_config.starts_with(bundle)
            || bundle.starts_with(app_config)
        {
            return Err(invalid());
        }
        verify_directory(&resources, false)?;
        let digest = hex_digest(
            format!(
                "artifact-custody-v1\0dev.ohmymagi.console\0installed\0{}\0{}",
                resources.display(),
                app_config.display()
            )
            .as_bytes(),
        );
        let control = app_config.join("artifact-custody").join(&digest);
        let publication_ancestors = hold_namespace_ancestors(&resources, &control)?;
        let namespace = Self {
            purpose: NamespacePurpose::InstalledReadOnly,
            publication: resources,
            control,
            digest,
            publication_ancestors,
        };
        namespace.check()?;
        Ok(namespace)
    }
    fn check(&self) -> Result<(), ProviderError> {
        for (path, file, expected) in &self.publication_ancestors {
            for current in [
                identity(&fs::symlink_metadata(path).map_err(|_| invalid())?),
                identity(&file.metadata().map_err(|_| invalid())?),
            ] {
                if (current.0, current.1, current.2, current.3)
                    != (expected.0, expected.1, expected.2, expected.3)
                {
                    return Err(invalid());
                }
            }
        }
        Ok(())
    }
    fn prove_empty(&self) -> Result<(), ProviderError> {
        self.check()?;
        for path in [
            self.publication.join("provider"),
            self.publication.join("extraction"),
        ] {
            for component in path.ancestors() {
                match fs::symlink_metadata(component) {
                    Ok(metadata)
                        if component == path
                            || metadata.file_type().is_symlink()
                            || !metadata.is_dir() =>
                    {
                        return Err(invalid());
                    }
                    Ok(_) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(_) => return Err(invalid()),
                }
            }
        }
        Ok(())
    }
    pub fn development(
        workspace: &Path,
        profile: DevelopmentProfile,
    ) -> Result<Self, ProviderError> {
        let workspace = workspace.canonicalize().map_err(|_| invalid())?;
        if workspace.ancestors().any(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".app"))
        }) {
            return Err(invalid());
        }
        verify_directory(&workspace, false)?;
        let profile = match profile {
            DevelopmentProfile::Debug => "debug",
            DevelopmentProfile::Release => "release",
        };
        let publication = workspace.join("src-tauri/target").join(profile);
        let digest = hex_digest(
            format!(
                "artifact-custody-v1\0dev.ohmymagi.console\0{profile}\0{}",
                workspace.display()
            )
            .as_bytes(),
        );
        let control = workspace.join(".local/artifact-authority").join(&digest);
        let publication_ancestors = hold_namespace_ancestors(&publication, &control)?;
        Ok(Self {
            purpose: NamespacePurpose::Development,
            publication_ancestors,
            publication,
            control,
            digest,
        })
    }
}

fn hold_namespace_ancestors(
    publication: &Path,
    control: &Path,
) -> Result<Vec<(PathBuf, File, Identity)>, ProviderError> {
    let mut held = Vec::new();
    for path in publication.ancestors().chain(control.ancestors()) {
        if path.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )
        }) {
            return Err(invalid());
        }
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return Err(invalid()),
        };
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(invalid());
        }
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY | libc::O_CLOEXEC)
            .open(path)
            .map_err(|_| invalid())?;
        let opened = identity(&file.metadata().map_err(|_| invalid())?);
        let expected = identity(&metadata);
        if (opened.0, opened.1, opened.2, opened.3)
            != (expected.0, expected.1, expected.2, expected.3)
        {
            return Err(invalid());
        }
        held.push((path.to_owned(), file, identity(&metadata)));
    }
    Ok(held)
}

type Identity = (u64, u64, u32, u32, u64);
fn identity(metadata: &fs::Metadata) -> Identity {
    (
        metadata.dev(),
        metadata.ino(),
        metadata.mode(),
        metadata.uid(),
        metadata.nlink(),
    )
}
fn verify_directory(path: &Path, private: bool) -> Result<(), ProviderError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| invalid())?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o022 != 0
        || (private && metadata.mode() & 0o777 != 0o700)
    {
        return Err(invalid());
    }
    Ok(())
}
fn lock_version(metadata: &fs::Metadata) -> (i64, i64, u64) {
    (metadata.ctime(), metadata.ctime_nsec(), metadata.len())
}
struct HeldFile {
    version: Option<(i64, i64, u64)>,
    path: PathBuf,
    file: File,
    identity: Identity,
}
impl HeldFile {
    fn open(path: &Path) -> Result<Self, ProviderError> {
        let metadata = fs::symlink_metadata(path).map_err(|_| invalid())?;
        if !metadata.is_file()
            || metadata.file_type().is_symlink()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.nlink() != 1
            || metadata.mode() & 0o777 != 0o600
        {
            return Err(invalid());
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
            .open(path)
            .map_err(|_| invalid())?;
        let held = Self {
            version: (path.file_name().is_some_and(|name| name == "custody.lock"))
                .then(|| lock_version(&metadata)),
            path: path.to_owned(),
            file,
            identity: identity(&metadata),
        };
        held.check()?;
        Ok(held)
    }
    fn check(&self) -> Result<(), ProviderError> {
        if self.version.is_some_and(|expected| {
            self.file
                .metadata()
                .map_or(true, |metadata| lock_version(&metadata) != expected)
                || fs::symlink_metadata(&self.path)
                    .map_or(true, |metadata| lock_version(&metadata) != expected)
        }) {
            return Err(invalid());
        }
        if identity(&self.file.metadata().map_err(|_| invalid())?) != self.identity
            || identity(&fs::symlink_metadata(&self.path).map_err(|_| invalid())?) != self.identity
        {
            return Err(invalid());
        }
        Ok(())
    }
}
struct Inner {
    namespace: ArtifactNamespace,
    directory: File,
    directory_identity: Identity,
    journal: HeldFile,
    owner: String,
    lock_identity: Identity,
    lock_version: (i64, i64, u64),
    ancestors: Vec<(PathBuf, File, Identity)>,
    installed: Mutex<Option<Arc<crate::verification::VerifiedInstalledGeneration>>>,
}
impl Inner {
    fn check(&self) -> Result<(), ProviderError> {
        self.namespace.check()?;
        verify_directory(&self.namespace.control, true)?;
        for current in [
            identity(&self.directory.metadata().map_err(|_| invalid())?),
            identity(&fs::symlink_metadata(&self.namespace.control).map_err(|_| invalid())?),
        ] {
            if (current.0, current.1, current.2, current.3)
                != (
                    self.directory_identity.0,
                    self.directory_identity.1,
                    self.directory_identity.2,
                    self.directory_identity.3,
                )
            {
                return Err(invalid());
            }
        }
        for (path, held, expected) in &self.ancestors {
            let current = identity(&fs::symlink_metadata(path).map_err(|_| invalid())?);
            let descriptor = identity(&held.metadata().map_err(|_| invalid())?);
            if (current.0, current.1, current.2, current.3)
                != (expected.0, expected.1, expected.2, expected.3)
                || (descriptor.0, descriptor.1, descriptor.2, descriptor.3)
                    != (expected.0, expected.1, expected.2, expected.3)
            {
                return Err(invalid());
            }
        }
        self.journal.check()?;
        if let Some(proof) = self.installed.lock().map_err(|_| invalid())?.as_ref() {
            proof.check_held()?;
        }
        Ok(())
    }
    fn connect(&self) -> Result<Connection, ProviderError> {
        self.connect_checked(|| {})
    }
    fn connect_checked(&self, after_check: impl FnOnce()) -> Result<Connection, ProviderError> {
        self.check()?;
        after_check();
        let connection = Connection::open_with_flags(
            &self.journal.path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE
                | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX
                | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .map_err(|_| invalid())?;
        connection
            .busy_timeout(Duration::from_millis(100))
            .map_err(|_| invalid())?;
        connection
            .pragma_update(None, "foreign_keys", true)
            .map_err(|_| invalid())?;
        let metadata: (String, u32, String, String) = connection
            .query_row(
                "SELECT namespace,version,lock_identity,journal_identity FROM metadata",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .map_err(|_| invalid())?;
        if metadata
            != (
                self.namespace.digest.clone(),
                2,
                format!("{:?}:{:?}", self.lock_identity, self.lock_version),
                format!("{:?}", self.journal.identity),
            )
        {
            return Err(invalid());
        }
        let expected_schema = Connection::open_in_memory().map_err(|_| invalid())?;
        expected_schema
            .execute_batch(&schema_v2())
            .map_err(|_| invalid())?;
        fn schema(connection: &Connection) -> Result<Vec<(String, String)>, ProviderError> {
            let mut statement=connection.prepare("SELECT name,sql FROM sqlite_master WHERE name NOT LIKE 'sqlite_%' ORDER BY name").map_err(|_|invalid())?;
            statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                .map_err(|_| invalid())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| invalid())
        }
        if schema(&connection)? != schema(&expected_schema)? {
            return Err(invalid());
        }
        let count: u64 = connection
            .query_row("SELECT count(*) FROM metadata", [], |r| r.get(0))
            .map_err(|_| invalid())?;
        if count != 1 {
            return Err(invalid());
        }
        let integrity: String = connection
            .query_row("PRAGMA quick_check", [], |row| row.get(0))
            .map_err(|_| invalid())?;
        if integrity != "ok" {
            return Err(invalid());
        }
        self.check()?;
        Ok(connection)
    }
    fn lock(&self, exclusive: bool) -> Result<HeldFile, ProviderError> {
        self.check()?;
        let lock = HeldFile::open(&self.namespace.control.join("custody.lock"))?;
        if lock.identity != self.lock_identity || lock.version != Some(self.lock_version) {
            return Err(invalid());
        }
        if exclusive {
            lock.file.try_lock_exclusive()
        } else {
            FileExt::try_lock_shared(&lock.file)
        }
        .map_err(|_| invalid())?;
        self.check()?;
        lock.check()?;
        Ok(lock)
    }
}

/// A native coordinator retains its closed issuance epoch and actual cleanup proof.
/// Implementations must retain artifact-wide issuance closure, revoked settled roots,
/// native-operation completion and closed helper/cache consumers. A process lookup,
/// boolean, wire value or lock acquisition cannot implement this authority.
pub trait RetainedNativeQuiescence: Send + Sync {
    fn validate_quiescence(&self) -> Result<(), ProviderError>;
    fn bound_requests(&self) -> &[VerificationRequest];
    fn verified_extraction(&self) -> Result<Arc<dyn HeldExtractionAuthority>, ProviderError>;
}

/// Implemented by the shared signed extraction verifier's held resource object.
/// The digest covers the verified immutable helper generation, not a caller label.
pub trait HeldExtractionAuthority: Send + Sync {
    fn validate_held(&self) -> Result<(), ProviderError>;
    fn executable(&self) -> &Path;
    fn generation_digest(&self) -> &str;
}

fn validate_native(native: &dyn RetainedNativeQuiescence) -> Result<(), ProviderError> {
    native.validate_quiescence()?;
    if native
        .bound_requests()
        .iter()
        .any(|request| !request.observed_revoked_settlement())
    {
        return Err(invalid());
    }
    native.validate_quiescence()
}

pub struct ExclusivePublicationPermission {
    inner: Arc<Inner>,
    lock: HeldFile,
    installation: crate::verification::RetiredGenerationCapture,
    native: Arc<dyn RetainedNativeQuiescence>,
    operation: String,
    request: VerificationRequest,
    finished: bool,
    outcome: Option<PublicationOutcome>,
    previous_installed: Option<Arc<crate::verification::VerifiedInstalledGeneration>>,
}
enum PublicationOutcome {
    Committed(Arc<crate::verification::VerifiedInstalledGeneration>),
    RolledBack,
}
impl ExclusivePublicationPermission {
    pub fn validate_current(&self) -> Result<(), ProviderError> {
        self.request.check()?;
        if self.finished {
            return Err(invalid());
        }
        validate_native(self.native.as_ref())?;
        self.inner.check()?;
        self.lock.check()?;
        match &self.outcome {
            Some(PublicationOutcome::Committed(proof)) => proof.check()?,
            _ => self.installation.check()?,
        }
        let connection = self.inner.connect()?;
        let pending: u64 = connection.query_row("SELECT count(*) FROM operations o WHERE operation<>?1 AND coalesce((SELECT state FROM observations WHERE operation=o.operation ORDER BY sequence DESC LIMIT 1),'active') <> 'observed_stopped'", [&self.operation], |row| row.get(0)).map_err(|_| invalid())?;
        if pending != 0 {
            return Err(invalid());
        }
        validate_native(self.native.as_ref())
    }
    /// Linearizes a new publisher effect with its absolute deadline and revocation.
    pub fn with_effect_admission<T>(
        &self,
        lease: &crate::StreamOperationLease,
        operation: impl FnOnce() -> T,
    ) -> Result<T, ProviderError> {
        self.request.with_stream_effect_admission(lease, || {
            self.validate_current()?;
            Ok(operation())
        })?
    }
    /// Retains only exact namespace/owner custody for restoration after a denied effect.
    /// Callers must restrict this authority to restoring captured original generations.
    pub fn validate_rollback(&self) -> Result<(), ProviderError> {
        if self.finished {
            return Err(invalid());
        }
        validate_native(self.native.as_ref())?;
        self.inner.check()?;
        self.lock.check()
    }
    pub fn generation_digest(&self) -> &str {
        self.installation.generation_digest()
    }
    pub fn request(&self) -> VerificationRequest {
        self.request.clone()
    }
    pub fn record_committed(
        &mut self,
        proof: crate::verification::VerifiedInstalledGeneration,
    ) -> Result<(), ProviderError> {
        self.request.check()?;
        self.validate_rollback()?;
        if !self.request.shares_authority(&proof.request()) {
            return Err(invalid());
        }
        proof.check_namespace(&self.inner.namespace.publication)?;
        self.installation.check_retained()?;
        self.outcome = Some(PublicationOutcome::Committed(Arc::new(proof)));
        Ok(())
    }
    pub fn record_rolled_back(&mut self) -> Result<(), ProviderError> {
        self.validate_rollback()?;
        self.installation.check_original_held()?;
        self.outcome = Some(PublicationOutcome::RolledBack);
        Ok(())
    }
    /// Completes only the bound publisher's actually observed cleanup under closed issuance.
    pub fn finish_observed(&mut self) -> Result<(), ProviderError> {
        validate_native(self.native.as_ref())?;
        self.inner.check()?;
        self.lock.check()?;
        if !self.request.observed_revoked_settlement() {
            return Err(invalid());
        }
        let mut connection = self.inner.connect()?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| invalid())?;
        let (outcome, generation) = match &self.outcome {
            Some(PublicationOutcome::Committed(proof)) => {
                proof.check_held()?;
                ("committed", Some(proof.generation_digest()))
            }
            Some(PublicationOutcome::RolledBack) => {
                self.installation.check_original_held()?;
                ("rolled_back", None)
            }
            None => return Err(invalid()),
        };
        tx.execute(
            "INSERT INTO publication_outcomes(operation,outcome,generation) VALUES(?1,?2,?3)",
            params![self.operation, outcome, generation],
        )
        .map_err(|_| invalid())?;
        if let Some(generation) = generation {
            tx.execute(
                "INSERT INTO installed_generations(generation) VALUES(?1)",
                [generation],
            )
            .map_err(|_| invalid())?;
        }
        tx.execute(
            "INSERT INTO observations(operation,state) VALUES(?1,'observed_stopped')",
            [&self.operation],
        )
        .map_err(|_| invalid())?;
        self.inner.check()?;
        self.lock.check()?;
        validate_native(self.native.as_ref())?;
        if !self.request.observed_revoked_settlement() {
            return Err(invalid());
        }
        tx.commit().map_err(|_| invalid())?;
        drop(connection);
        *self.inner.installed.lock().map_err(|_| invalid())? = match self.outcome.as_ref() {
            Some(PublicationOutcome::Committed(proof)) => Some(proof.clone()),
            Some(PublicationOutcome::RolledBack) => self.previous_installed.clone(),
            None => return Err(invalid()),
        };
        FileExt::unlock(&self.lock.file).map_err(|_| invalid())?;
        self.finished = true;
        Ok(())
    }
}
impl Drop for ExclusivePublicationPermission {
    fn drop(&mut self) {
        if self.finished && self.inner.check().is_ok() && self.lock.check().is_ok() {
            let _ = FileExt::unlock(&self.lock.file);
        }
    }
}

#[derive(Clone)]
pub struct ResourceCustody {
    inner: Arc<Inner>,
}
impl ResourceCustody {
    /// Initializes only an empty publication namespace. Existing generations require
    /// a separate supported adoption authority and cannot be inferred quiescent.
    pub fn bootstrap_empty(namespace: ArtifactNamespace) -> Result<Self, ProviderError> {
        namespace.prove_empty()?;
        Self::initialize(namespace, true)
    }
    fn initialize(namespace: ArtifactNamespace, empty: bool) -> Result<Self, ProviderError> {
        namespace.check()?;
        let local = namespace
            .control
            .parent()
            .and_then(Path::parent)
            .ok_or_else(invalid)?;
        if !local.exists() {
            fs::create_dir(local).map_err(|_| invalid())?;
        }
        verify_directory(local, false)?;
        let parent = namespace.control.parent().ok_or_else(invalid)?;
        if !parent.exists() {
            fs::create_dir(parent).map_err(|_| invalid())?;
            fs::set_permissions(parent, fs::Permissions::from_mode(0o700))
                .map_err(|_| invalid())?;
        }
        verify_directory(parent, true)?;
        fs::create_dir(&namespace.control).map_err(|_| invalid())?;
        fs::set_permissions(&namespace.control, fs::Permissions::from_mode(0o700))
            .map_err(|_| invalid())?;
        for name in ["custody.lock", "custody.sqlite"] {
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(namespace.control.join(name))
                .map_err(|_| invalid())?
                .sync_all()
                .map_err(|_| invalid())?;
        }
        let connection = Connection::open_with_flags(
            namespace.control.join("custody.sqlite"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE
                | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX
                | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .map_err(|_| invalid())?;
        connection
            .execute_batch(&schema_v2())
            .map_err(|_| invalid())?;
        connection
            .execute(
                "INSERT INTO namespace_policy VALUES(1,?1)",
                [i64::from(!empty)],
            )
            .map_err(|_| invalid())?;
        connection
            .execute(
                "INSERT INTO metadata VALUES(?1,2,?2,?3)",
                params![
                    namespace.digest,
                    format!(
                        "{:?}:{:?}",
                        identity(
                            &fs::metadata(namespace.control.join("custody.lock"))
                                .map_err(|_| invalid())?
                        ),
                        lock_version(
                            &fs::metadata(namespace.control.join("custody.lock"))
                                .map_err(|_| invalid())?
                        )
                    ),
                    format!(
                        "{:?}",
                        identity(
                            &fs::metadata(namespace.control.join("custody.sqlite"))
                                .map_err(|_| invalid())?
                        )
                    )
                ],
            )
            .map_err(|_| invalid())?;
        drop(connection);
        File::open(&namespace.control)
            .and_then(|file| file.sync_all())
            .map_err(|_| invalid())?;
        if empty {
            namespace.prove_empty()?;
        } else {
            namespace.check()?;
        }
        Self::open(namespace)
    }
    pub fn open(namespace: ArtifactNamespace) -> Result<Self, ProviderError> {
        verify_directory(&namespace.control, true)?;
        let directory = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY | libc::O_CLOEXEC)
            .open(&namespace.control)
            .map_err(|_| invalid())?;
        let mut ancestors = Vec::new();
        for path in namespace.control.ancestors().skip(1) {
            let metadata = fs::symlink_metadata(path).map_err(|_| invalid())?;
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                return Err(invalid());
            }
            let file = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY | libc::O_CLOEXEC)
                .open(path)
                .map_err(|_| invalid())?;
            ancestors.push((path.to_owned(), file, identity(&metadata)));
        }
        let lock_identity = identity(
            &fs::symlink_metadata(namespace.control.join("custody.lock")).map_err(|_| invalid())?,
        );
        let lock_version = lock_version(
            &fs::symlink_metadata(namespace.control.join("custody.lock")).map_err(|_| invalid())?,
        );
        let inner = Arc::new(Inner {
            installed: Mutex::new(None),
            lock_version,
            lock_identity,
            ancestors,
            directory_identity: identity(&directory.metadata().map_err(|_| invalid())?),
            directory,
            journal: HeldFile::open(&namespace.control.join("custody.sqlite"))?,
            namespace,
            owner: nonce()?,
        });
        let result = Self { inner };
        let lock = result.inner.lock(false)?;
        result.inner.connect()?;
        lock.check()?;
        FileExt::unlock(&lock.file).map_err(|_| invalid())?;
        Ok(result)
    }
    /// Enrolls an existing generation only with retained full verification and closed issuance.
    pub fn adopt_installed(
        namespace: ArtifactNamespace,
        installation: crate::verification::VerifiedInstalledGeneration,
        native: Arc<dyn RetainedNativeQuiescence>,
    ) -> Result<Self, ProviderError> {
        validate_native(native.as_ref())?;
        installation.check_namespace(&namespace.publication)?;
        match fs::symlink_metadata(&namespace.control) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            _ => return Err(invalid()),
        }
        let custody = Self::initialize(namespace, false)?;
        validate_native(native.as_ref())?;
        installation.check_namespace(&custody.inner.namespace.publication)?;
        let mut connection = custody.inner.connect()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| invalid())?;
        transaction
            .execute(
                "INSERT INTO installed_generations(generation) VALUES(?1)",
                [installation.generation_digest()],
            )
            .map_err(|_| invalid())?;
        validate_native(native.as_ref())?;
        installation.check()?;
        custody.inner.check()?;
        transaction.commit().map_err(|_| invalid())?;
        *custody.inner.installed.lock().map_err(|_| invalid())? = Some(Arc::new(installation));
        Ok(custody)
    }
    pub fn reopen_verified(
        namespace: ArtifactNamespace,
        installation: crate::verification::VerifiedInstalledGeneration,
        native: Arc<dyn RetainedNativeQuiescence>,
    ) -> Result<Self, ProviderError> {
        validate_native(native.as_ref())?;
        installation.check_namespace(&namespace.publication)?;
        let custody = Self::open(namespace)?;
        let lock = custody.inner.lock(true)?;
        let mut connection = custody.inner.connect()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| invalid())?;
        let pending:u64=transaction.query_row("SELECT count(*) FROM operations o WHERE coalesce((SELECT state FROM observations WHERE operation=o.operation ORDER BY sequence DESC LIMIT 1),'active') <> 'observed_stopped'",[],|r|r.get(0)).map_err(|_|invalid())?;
        let generation: String = transaction
            .query_row(
                "SELECT generation FROM installed_generations ORDER BY sequence DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .map_err(|_| invalid())?;
        if pending != 0 || generation != installation.generation_digest() {
            return Err(invalid());
        }
        validate_native(native.as_ref())?;
        installation.check_namespace(&custody.inner.namespace.publication)?;
        custody.inner.check()?;
        lock.check()?;
        transaction.commit().map_err(|_| invalid())?;
        drop(connection);
        *custody.inner.installed.lock().map_err(|_| invalid())? = Some(Arc::new(installation));
        custody.inner.check()?;
        FileExt::unlock(&lock.file).map_err(|_| invalid())?;
        Ok(custody)
    }
    pub fn prepare_retired_publication(
        namespace: ArtifactNamespace,
        capture: crate::verification::RetiredGenerationCapture,
        native: Arc<dyn RetainedNativeQuiescence>,
    ) -> Result<(Self, ExclusivePublicationPermission), ProviderError> {
        if namespace.purpose != NamespacePurpose::Development {
            return Err(invalid());
        }
        validate_native(native.as_ref())?;
        capture.check_namespace(&namespace.publication)?;
        match fs::symlink_metadata(&namespace.control) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            _ => return Err(invalid()),
        }
        let custody = Self::initialize(namespace, false)?;
        let permission = custody.begin_publication(capture, native)?;
        Ok((custody, permission))
    }
    /// Upgrades only the exact legacy control schema under retained closed issuance.
    pub fn upgrade_legacy_control(
        namespace: ArtifactNamespace,
        native: Arc<dyn RetainedNativeQuiescence>,
    ) -> Result<Self, ProviderError> {
        validate_native(native.as_ref())?;
        namespace.check()?;
        verify_directory(&namespace.control, true)?;
        let lock = HeldFile::open(&namespace.control.join("custody.lock"))?;
        lock.file.try_lock_exclusive().map_err(|_| invalid())?;
        let journal = HeldFile::open(&namespace.control.join("custody.sqlite"))?;
        let mut connection = Connection::open_with_flags(
            &journal.path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE
                | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX
                | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .map_err(|_| invalid())?;
        connection
            .busy_timeout(Duration::from_millis(100))
            .map_err(|_| invalid())?;
        let expected = Connection::open_in_memory().map_err(|_| invalid())?;
        expected.execute_batch(SCHEMA_V1).map_err(|_| invalid())?;
        fn schema(connection: &Connection) -> Result<Vec<(String, String)>, ProviderError> {
            let mut statement=connection.prepare("SELECT name,sql FROM sqlite_master WHERE name NOT LIKE 'sqlite_%' ORDER BY name").map_err(|_|invalid())?;
            statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                .map_err(|_| invalid())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| invalid())
        }
        if schema(&connection)? != schema(&expected)? {
            return Err(invalid());
        }
        let metadata: (String, u32, String, String) = connection
            .query_row(
                "SELECT namespace,version,lock_identity,journal_identity FROM metadata",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .map_err(|_| invalid())?;
        if metadata
            != (
                namespace.digest.clone(),
                1,
                format!(
                    "{:?}:{:?}",
                    lock.identity,
                    lock.version.ok_or_else(invalid)?
                ),
                format!("{:?}", journal.identity),
            )
        {
            return Err(invalid());
        }
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| invalid())?;
        let pending:u64=tx.query_row("SELECT count(*) FROM operations o WHERE coalesce((SELECT state FROM observations WHERE operation=o.operation ORDER BY sequence DESC LIMIT 1),'active') <> 'observed_stopped'",[],|r|r.get(0)).map_err(|_|invalid())?;
        if pending != 0 {
            return Err(invalid());
        }
        tx.execute_batch(&format!("ALTER TABLE metadata RENAME TO legacy_metadata;{}; INSERT INTO metadata SELECT namespace,2,lock_identity,journal_identity FROM legacy_metadata; DROP TABLE legacy_metadata;",schema_v2().split(';').next().ok_or_else(invalid)?)).map_err(|_|invalid())?;
        tx.execute_batch(PUBLICATION_SCHEMA)
            .map_err(|_| invalid())?;
        tx.execute("INSERT INTO namespace_policy VALUES(1,1)", [])
            .map_err(|_| invalid())?;
        namespace.check()?;
        lock.check()?;
        journal.check()?;
        validate_native(native.as_ref())?;
        tx.commit().map_err(|_| invalid())?;
        drop(connection);
        FileExt::unlock(&lock.file).map_err(|_| invalid())?;
        Self::open(namespace)
    }
    pub fn begin_publication(
        &self,
        installation: crate::verification::RetiredGenerationCapture,
        native: Arc<dyn RetainedNativeQuiescence>,
    ) -> Result<ExclusivePublicationPermission, ProviderError> {
        if self.inner.namespace.purpose != NamespacePurpose::Development {
            return Err(invalid());
        }
        validate_native(native.as_ref())?;
        installation.check_namespace(&self.inner.namespace.publication)?;
        let lock = self.inner.lock(true)?;
        let operation = nonce()?;
        let request = installation.request();
        let mut connection = self.inner.connect()?;
        let tx = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| invalid())?;
        let pending: u64 = tx.query_row("SELECT count(*) FROM operations o WHERE coalesce((SELECT state FROM observations WHERE operation=o.operation ORDER BY sequence DESC LIMIT 1),'active') <> 'observed_stopped'",[],|row|row.get(0)).map_err(|_|invalid())?;
        if pending != 0 {
            return Err(invalid());
        }
        tx.execute(
            "INSERT INTO operations VALUES(?1,?2,?3)",
            params![
                operation,
                self.inner.owner,
                installation.generation_digest()
            ],
        )
        .map_err(|_| invalid())?;
        tx.execute(
            "INSERT INTO publication_operations(operation) VALUES(?1)",
            [&operation],
        )
        .map_err(|_| invalid())?;
        self.inner.check()?;
        lock.check()?;
        validate_native(native.as_ref())?;
        installation.check()?;
        tx.commit().map_err(|_| invalid())?;
        let previous_installed = self.inner.installed.lock().map_err(|_| invalid())?.take();
        let permission = ExclusivePublicationPermission {
            inner: self.inner.clone(),
            lock,
            installation,
            native,
            operation,
            request,
            finished: false,
            outcome: None,
            previous_installed,
        };
        permission.validate_current()?;
        Ok(permission)
    }
    pub fn begin_reader(
        &self,
        generation: &str,
        request: VerificationRequest,
    ) -> Result<ReaderCustody, ProviderError> {
        if !valid_hex(generation, 64) {
            return Err(invalid());
        }
        request.check()?;
        let installed = self.inner.installed.lock().map_err(|_| invalid())?.clone();
        if let Some(proof) = installed.as_ref() {
            proof.check_held()?;
        } else {
            #[cfg(not(test))]
            return Err(invalid());
        }
        let lock = self.inner.lock(false)?;
        let operation = nonce()?;
        let mut connection = self.inner.connect()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| invalid())?;
        let publication_pending: u64 = transaction.query_row("SELECT count(*) FROM publication_operations p WHERE coalesce((SELECT state FROM observations WHERE operation=p.operation ORDER BY sequence DESC LIMIT 1),'active') <> 'observed_stopped'",[],|r|r.get(0)).map_err(|_|invalid())?;
        let inactive: bool = transaction.query_row("SELECT (requires_installation=1 OR EXISTS(SELECT 1 FROM publication_operations)) AND NOT EXISTS(SELECT 1 FROM installed_generations) FROM namespace_policy WHERE singleton=1",[],|r|r.get(0)).map_err(|_|invalid())?;
        if publication_pending != 0 || inactive {
            return Err(invalid());
        }
        if let Some(proof) = installed.as_ref() {
            let generation: String = transaction
                .query_row(
                    "SELECT generation FROM installed_generations ORDER BY sequence DESC LIMIT 1",
                    [],
                    |r| r.get(0),
                )
                .map_err(|_| invalid())?;
            if generation != proof.generation_digest() {
                return Err(invalid());
            }
            proof.check_held()?;
        }
        transaction
            .execute(
                "INSERT INTO operations VALUES(?1,?2,?3)",
                params![operation, self.inner.owner, generation],
            )
            .map_err(|_| invalid())?;
        self.inner.check()?;
        lock.check()?;
        transaction.commit().map_err(|_| invalid())?;
        Ok(ReaderCustody {
            inner: self.inner.clone(),
            lock,
            operation,
            generation: generation.to_owned(),
            request,
            finished: false,
        })
    }
    pub(crate) fn check_provider_path(&self, executable: &Path) -> Result<(), ProviderError> {
        self.inner.check()?;
        let expected = self.inner.namespace.publication.join("provider");
        if !executable.starts_with(expected) {
            return Err(invalid());
        }
        Ok(())
    }
    pub fn unresolved_operations(&self) -> Result<u64, ProviderError> {
        let lock = self.inner.lock(false)?;
        let connection = self.inner.connect()?;
        let count = connection.query_row("SELECT count(*) FROM operations o WHERE coalesce((SELECT state FROM observations WHERE operation=o.operation ORDER BY sequence DESC LIMIT 1),'active') <> 'observed_stopped'",[],|row|row.get(0)).map_err(|_|invalid())?;
        drop(connection);
        self.inner.check()?;
        lock.check()?;
        FileExt::unlock(&lock.file).map_err(|_| invalid())?;
        Ok(count)
    }
}

pub struct ReaderCustody {
    inner: Arc<Inner>,
    lock: HeldFile,
    operation: String,
    generation: String,
    request: VerificationRequest,
    finished: bool,
}
pub struct ObservedCleanupProof {
    namespace: String,
    owner: String,
    operation: String,
    generation: String,
    request: VerificationRequest,
}
impl ReaderCustody {
    pub(crate) fn check_custody(&self) -> Result<(), ProviderError> {
        self.inner.check()?;
        self.lock.check()
    }
    pub(crate) fn retain_until_observed(self) {
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                if self.request.await_revoked_settlement().await.is_err() {
                    return;
                }
                if let Ok(proof) = self.observe_cleanup() {
                    let _ = self.finish_observed(proof);
                }
            });
        }
    }
    /// Holds the bound request fence and namespace custody through a synchronous effect.
    pub fn with_effect_admission<T>(
        &self,
        lease: &crate::StreamOperationLease,
        operation: impl FnOnce() -> T,
    ) -> Result<T, ProviderError> {
        self.request.with_stream_effect_admission(lease, || {
            self.inner.check()?;
            self.lock.check()?;
            Ok(operation())
        })?
    }
    /// This proves only the bound request's work; native operations, helper/cache
    /// custody and the exclusive publisher capability require separate integration.
    pub fn observe_cleanup(&self) -> Result<ObservedCleanupProof, ProviderError> {
        self.inner.check()?;
        self.lock.check()?;
        if !self.request.observed_revoked_settlement() {
            return Err(invalid());
        }
        Ok(ObservedCleanupProof {
            namespace: self.inner.namespace.digest.clone(),
            owner: self.inner.owner.clone(),
            operation: self.operation.clone(),
            generation: self.generation.clone(),
            request: self.request.clone(),
        })
    }
    pub fn finish_observed(mut self, proof: ObservedCleanupProof) -> Result<(), ProviderError> {
        if proof.namespace != self.inner.namespace.digest
            || proof.owner != self.inner.owner
            || proof.operation != self.operation
            || proof.generation != self.generation
            || !proof.request.observed_revoked_settlement()
        {
            return Err(invalid());
        }
        self.inner.check()?;
        self.lock.check()?;
        let mut connection = self.inner.connect()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| invalid())?;
        transaction
            .execute(
                "INSERT INTO observations(operation,state) VALUES(?1,'observed_stopped')",
                [&self.operation],
            )
            .map_err(|_| invalid())?;
        self.inner.check()?;
        self.lock.check()?;
        if !self.request.observed_revoked_settlement() {
            return Err(invalid());
        }
        transaction.commit().map_err(|_| invalid())?;
        drop(connection);
        self.inner.check()?;
        self.lock.check()?;
        FileExt::unlock(&self.lock.file).map_err(|_| invalid())?;
        self.finished = true;
        Ok(())
    }
}
impl Drop for ReaderCustody {
    fn drop(&mut self) {
        if !self.finished {
            // Failure to append preserves the original Active row and remains fenced.
            if let Ok(connection) = self.inner.connect() {
                let _ = connection.execute(
                    "INSERT INTO observations(operation,state) VALUES(?1,'unresolved')",
                    [&self.operation],
                );
            }
        }
    }
}

// Fixture exclusivity cannot mint production publication permission.
#[cfg(test)]
struct FixtureExclusive(HeldFile);
#[cfg(test)]
impl Drop for FixtureExclusive {
    fn drop(&mut self) {
        if self.0.check().is_ok() {
            let _ = FileExt::unlock(&self.0.file);
        }
    }
}
#[cfg(test)]
fn fixture_exclusive(custody: &ResourceCustody) -> Result<FixtureExclusive, ProviderError> {
    let lock = custody.inner.lock(true)?;
    let connection = custody.inner.connect()?;
    let pending:u64=connection.query_row("SELECT count(*) FROM operations o WHERE coalesce((SELECT state FROM observations WHERE operation=o.operation ORDER BY sequence DESC LIMIT 1),'active') <> 'observed_stopped'",[],|row|row.get(0)).map_err(|_|invalid())?;
    if pending != 0 {
        return Err(invalid());
    }
    Ok(FixtureExclusive(lock))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        process::{Command, Stdio},
        time::Instant,
    };
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("artifact-custody-{}", nonce().unwrap()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn namespace(&self) -> ArtifactNamespace {
            ArtifactNamespace::development(&self.0, DevelopmentProfile::Debug).unwrap()
        }
        fn custody(&self) -> ResourceCustody {
            ResourceCustody::bootstrap_empty(self.namespace()).unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn request() -> VerificationRequest {
        VerificationRequest::until(Instant::now() + Duration::from_secs(10))
    }
    struct FrozenFixture {
        roots: Vec<VerificationRequest>,
        closed: std::sync::atomic::AtomicBool,
    }
    impl RetainedNativeQuiescence for FrozenFixture {
        fn validate_quiescence(&self) -> Result<(), ProviderError> {
            if self.closed.load(std::sync::atomic::Ordering::SeqCst) {
                Ok(())
            } else {
                Err(invalid())
            }
        }
        fn bound_requests(&self) -> &[VerificationRequest] {
            &self.roots
        }
        fn verified_extraction(&self) -> Result<Arc<dyn HeldExtractionAuthority>, ProviderError> {
            Err(invalid())
        }
    }
    fn retired_fixture(f: &Fixture) -> PathBuf {
        let publication = f.0.join("src-tauri/target/debug");
        for (directory, name) in [
            ("provider/codex-acp", "old-provider"),
            ("extraction", "old-helper"),
        ] {
            let path = publication.join(directory);
            fs::create_dir_all(&path).unwrap();
            fs::write(
                path.join(name),
                b"withdrawn fixture bytes, not execution authority",
            )
            .unwrap();
            fs::set_permissions(path.join(name), fs::Permissions::from_mode(0o444)).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o555)).unwrap();
        }
        publication
    }
    #[test]
    fn installed_namespace_is_separate_and_fences_bundle_and_control_paths() {
        let f = Fixture::new();
        let root = f.0.canonicalize().unwrap();
        let resources = root.join("Console.app/Contents/Resources");
        let config = root.join("application-config");
        fs::create_dir_all(&resources).unwrap();
        fs::create_dir(&config).unwrap();
        let namespace = ArtifactNamespace::installed_resources(&resources, &config).unwrap();
        assert!(ArtifactNamespace::development(&resources, DevelopmentProfile::Debug).is_err());
        assert!(
            namespace
                .control
                .starts_with(config.join("artifact-custody"))
        );
        assert_ne!(namespace.digest, f.namespace().digest);
        assert!(ArtifactNamespace::installed_resources(&resources, &resources).is_err());
        assert!(
            ArtifactNamespace::installed_resources(&resources, resources.parent().unwrap())
                .is_err()
        );
        let old_config = f.0.join("old-config");
        fs::rename(&config, &old_config).unwrap();
        fs::create_dir(&config).unwrap();
        assert!(namespace.check().is_err());
        fs::remove_dir(&config).unwrap();
        std::os::unix::fs::symlink(&old_config, &config).unwrap();
        assert!(ArtifactNamespace::installed_resources(&resources, &config).is_err());
    }
    #[test]
    fn retired_capture_preserves_writable_wrapper_and_rejects_unknown_children() {
        let f = Fixture::new();
        let publication = retired_fixture(&f);
        let wrapper = publication.join("provider");
        fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();
        let capture = crate::RetiredGenerationCapture::capture(&publication, request()).unwrap();
        capture.check().unwrap();
        fs::write(wrapper.join("unexpected"), b"unaccounted rollback input").unwrap();
        assert!(capture.check().is_err());
        assert!(crate::RetiredGenerationCapture::capture(&publication, request()).is_err());
    }
    fn frozen_fixture(roots: Vec<VerificationRequest>) -> Arc<FrozenFixture> {
        Arc::new(FrozenFixture {
            roots,
            closed: std::sync::atomic::AtomicBool::new(true),
        })
    }
    #[test]
    fn publication_interrupted_adoption_initialization_remains_inert() {
        let f = Fixture::new();
        retired_fixture(&f);
        let custody = ResourceCustody::initialize(f.namespace(), false).unwrap();
        assert!(custody.begin_reader(&"a".repeat(64), request()).is_err());
        drop(custody);
        let reopened = ResourceCustody::open(f.namespace()).unwrap();
        assert!(reopened.begin_reader(&"a".repeat(64), request()).is_err());
        assert_eq!(reopened.unresolved_operations().unwrap(), 0);
    }
    #[test]
    fn publication_genuine_legacy_upgrade_preserves_history_and_denies_pending() {
        for stopped in [false, true] {
            let f = Fixture::new();
            let namespace = f.namespace();
            fs::create_dir_all(&namespace.control).unwrap();
            fs::set_permissions(
                namespace.control.parent().unwrap(),
                fs::Permissions::from_mode(0o700),
            )
            .unwrap();
            fs::set_permissions(&namespace.control, fs::Permissions::from_mode(0o700)).unwrap();
            for name in ["custody.lock", "custody.sqlite"] {
                OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(namespace.control.join(name))
                    .unwrap();
            }
            let lock = HeldFile::open(&namespace.control.join("custody.lock")).unwrap();
            let journal = HeldFile::open(&namespace.control.join("custody.sqlite")).unwrap();
            let connection = Connection::open(&journal.path).unwrap();
            connection.execute_batch(SCHEMA_V1).unwrap();
            connection
                .execute(
                    "INSERT INTO metadata VALUES(?1,1,?2,?3)",
                    params![
                        namespace.digest,
                        format!("{:?}:{:?}", lock.identity, lock.version.unwrap()),
                        format!("{:?}", journal.identity)
                    ],
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO operations VALUES('original-operation','original-owner',?1)",
                    ["d".repeat(64)],
                )
                .unwrap();
            if stopped {
                connection.execute("INSERT INTO observations(sequence,operation,state) VALUES(900,'original-operation','observed_stopped')",[]).unwrap();
            }
            drop(connection);
            assert!(ResourceCustody::open(f.namespace()).is_err());
            let result =
                ResourceCustody::upgrade_legacy_control(f.namespace(), frozen_fixture(vec![]));
            assert_eq!(result.is_ok(), stopped);
            let connection = Connection::open(&journal.path).unwrap();
            let version: u32 = connection
                .query_row("SELECT version FROM metadata", [], |r| r.get(0))
                .unwrap();
            assert_eq!(version, if stopped { 2 } else { 1 });
            let original: (String, String) = connection
                .query_row(
                    "SELECT owner,generation FROM operations WHERE operation='original-operation'",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap();
            assert_eq!(original, ("original-owner".into(), "d".repeat(64)));
            if stopped {
                let sequence: u64 = connection
                    .query_row(
                        "SELECT seq FROM sqlite_sequence WHERE name='observations'",
                        [],
                        |r| r.get(0),
                    )
                    .unwrap();
                assert_eq!(sequence, 900);
                assert!(
                    result
                        .unwrap()
                        .begin_reader(&"a".repeat(64), request())
                        .is_err()
                );
            }
        }
    }
    #[tokio::test]
    async fn publication_withdrawn_generation_is_not_adoption_and_crash_denies_readers() {
        let f = Fixture::new();
        let publication = retired_fixture(&f);
        let native = frozen_fixture(vec![]);
        assert!(ResourceCustody::bootstrap_empty(f.namespace()).is_err());
        assert!(
            crate::verify_installed_generation(
                publication.clone(),
                publication.join("provider/codex-acp/old-provider"),
                native.clone(),
                request()
            )
            .await
            .is_err()
        );
        let root = request();
        let capture = crate::RetiredGenerationCapture::capture_quiescent(
            publication.clone(),
            native.clone(),
            root.clone(),
        )
        .await
        .unwrap();
        assert!(root.settlement().is_settled());
        let (custody, permission) =
            ResourceCustody::prepare_retired_publication(f.namespace(), capture, native.clone())
                .unwrap();
        assert!(custody.begin_reader(&"a".repeat(64), request()).is_err());
        drop(permission);
        let reopened = ResourceCustody::open(f.namespace()).unwrap();
        assert_eq!(reopened.unresolved_operations().unwrap(), 1);
        assert!(reopened.begin_reader(&"a".repeat(64), request()).is_err());
        let capture = crate::RetiredGenerationCapture::capture(&publication, request()).unwrap();
        assert!(reopened.begin_publication(capture, native).is_err());
    }
    #[test]
    fn publication_revocation_denies_effect_but_exact_rollback_can_settle() {
        let f = Fixture::new();
        let publication = retired_fixture(&f);
        let native = frozen_fixture(vec![]);
        let root = request();
        let capture = crate::RetiredGenerationCapture::capture(&publication, root.clone()).unwrap();
        let (custody, mut permission) =
            ResourceCustody::prepare_retired_publication(f.namespace(), capture, native).unwrap();
        let lease = root.track_stream_operation().unwrap();
        let mut effects = 0;
        permission
            .with_effect_admission(&lease, || effects += 1)
            .unwrap();
        root.revoke();
        assert!(
            permission
                .with_effect_admission(&lease, || effects += 1)
                .is_err()
        );
        assert_eq!(effects, 1);
        permission.record_rolled_back().unwrap();
        assert!(permission.finish_observed().is_err());
        drop(lease);
        permission.finish_observed().unwrap();
        assert!(permission.validate_current().is_err());
        drop(permission);
        assert_eq!(custody.unresolved_operations().unwrap(), 0);
        assert!(custody.begin_reader(&"b".repeat(64), request()).is_err());
    }
    #[test]
    fn publication_expired_request_denies_effect_and_keeps_rollback_authority() {
        let f = Fixture::new();
        let publication = retired_fixture(&f);
        let deadline = Instant::now() + Duration::from_secs(1);
        let root = VerificationRequest::until(deadline);
        let capture = crate::RetiredGenerationCapture::capture(&publication, root.clone()).unwrap();
        let (_, mut permission) = ResourceCustody::prepare_retired_publication(
            f.namespace(),
            capture,
            frozen_fixture(vec![]),
        )
        .unwrap();
        let lease = root.track_stream_operation().unwrap();
        std::thread::sleep(
            deadline.saturating_duration_since(Instant::now()) + Duration::from_millis(1),
        );
        assert!(root.check().is_err());
        let mut effect = false;
        assert!(
            permission
                .with_effect_admission(&lease, || effect = true)
                .is_err()
        );
        assert!(!effect);
        permission.record_rolled_back().unwrap();
        assert!(permission.finish_observed().is_err());
        root.revoke();
        drop(lease);
        permission.finish_observed().unwrap();
    }
    #[test]
    fn publication_unsettled_native_root_and_replaced_generation_are_denied() {
        let f = Fixture::new();
        let publication = retired_fixture(&f);
        let native_root = request();
        let capture = crate::RetiredGenerationCapture::capture(&publication, request()).unwrap();
        assert!(
            ResourceCustody::prepare_retired_publication(
                f.namespace(),
                capture,
                frozen_fixture(vec![native_root.clone()])
            )
            .is_err()
        );
        native_root.revoke();
        let root = request();
        let capture = crate::RetiredGenerationCapture::capture(&publication, root.clone()).unwrap();
        let (_, mut permission) = ResourceCustody::prepare_retired_publication(
            f.namespace(),
            capture,
            frozen_fixture(vec![native_root]),
        )
        .unwrap();
        let path = publication.join("provider/codex-acp/old-provider");
        fs::set_permissions(path.parent().unwrap(), fs::Permissions::from_mode(0o755)).unwrap();
        fs::remove_file(&path).unwrap();
        fs::write(&path, b"replacement").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o444)).unwrap();
        fs::set_permissions(path.parent().unwrap(), fs::Permissions::from_mode(0o555)).unwrap();
        assert!(permission.validate_current().is_err());
        assert!(permission.record_rolled_back().is_err());
        root.revoke();
        assert!(permission.finish_observed().is_err());
    }
    #[test]
    fn custody_sqlite_open_rejects_symlink_after_identity_check() {
        let f = Fixture::new();
        let custody = f.custody();
        let canary = f.0.join("canary.sqlite");
        fs::copy(&custody.inner.journal.path, &canary).unwrap();
        let before = fs::read(&canary).unwrap();
        let original = custody.inner.journal.path.with_extension("held");
        assert!(
            custody
                .inner
                .connect_checked(|| {
                    fs::rename(&custody.inner.journal.path, &original).unwrap();
                    std::os::unix::fs::symlink(&canary, &custody.inner.journal.path).unwrap();
                })
                .is_err()
        );
        assert_eq!(fs::read(&canary).unwrap(), before);
    }
    #[test]
    fn custody_pre_exec_child_retains_inherited_lock_description() {
        for exclusive in [false, true] {
            let f = Fixture::new();
            let custody = f.custody();
            let root = request();
            let reader = if exclusive {
                None
            } else {
                Some(custody.begin_reader(&"d".repeat(64), root.clone()).unwrap())
            };
            let exclusive_guard = if exclusive {
                Some(fixture_exclusive(&custody).unwrap())
            } else {
                None
            };
            let mut pipe = [0; 2];
            assert_eq!(unsafe { libc::pipe(pipe.as_mut_ptr()) }, 0);
            let child = unsafe { libc::fork() };
            assert!(child >= 0);
            if child == 0 {
                unsafe {
                    libc::close(pipe[1]);
                    let mut go = 0u8;
                    libc::read(pipe[0], (&mut go as *mut u8).cast(), 1);
                    libc::close(pipe[0]);
                    let executable = c"/usr/bin/true";
                    let arguments = [executable.as_ptr(), std::ptr::null()];
                    libc::execv(executable.as_ptr(), arguments.as_ptr());
                    libc::_exit(127);
                }
            }
            unsafe {
                libc::close(pipe[0]);
            }
            root.revoke();
            if let Some(reader) = reader {
                let proof = reader.observe_cleanup().unwrap();
                reader.finish_observed(proof).unwrap();
            }
            drop(exclusive_guard);
            let inherited_busy = fixture_exclusive(&custody).is_err();
            unsafe {
                let go = 1u8;
                libc::write(pipe[1], (&go as *const u8).cast(), 1);
                libc::close(pipe[1]);
            }
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut status = 0;
            loop {
                if unsafe { libc::waitpid(child, &mut status, libc::WNOHANG) } == child {
                    break;
                }
                if Instant::now() >= deadline {
                    unsafe {
                        libc::kill(child, libc::SIGKILL);
                        libc::waitpid(child, &mut status, 0);
                    }
                    panic!("owned pre-exec child did not settle");
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            assert!(
                !inherited_busy,
                "observed retirement releases only its finished lock description"
            );
            assert!(fixture_exclusive(&custody).is_ok());
        }
    }
    #[test]
    fn custody_retirement_requires_exact_revoked_settled_request() {
        let f = Fixture::new();
        let custody = f.custody();
        let root = request();
        let first = custody.begin_reader(&"a".repeat(64), root.clone()).unwrap();
        assert!(first.observe_cleanup().is_err());
        let mut held = root.provider_activity().unwrap();
        held.mark_owned_process();
        root.revoke();
        assert!(first.observe_cleanup().is_err());
        held.settle();
        let proof = first.observe_cleanup().unwrap();
        first.finish_observed(proof).unwrap();
        assert_eq!(custody.unresolved_operations().unwrap(), 0);
        assert!(fixture_exclusive(&custody).is_ok());
    }
    #[test]
    fn custody_crash_owner_entrypoint() {
        let Some(workspace) = std::env::var_os("MAGI_CUSTODY_CRASH_FIXTURE") else {
            return;
        };
        let custody = ResourceCustody::open(
            ArtifactNamespace::development(Path::new(&workspace), DevelopmentProfile::Debug)
                .unwrap(),
        )
        .unwrap();
        let _reader = custody.begin_reader(&"f".repeat(64), request()).unwrap();
        let child = Command::new("/bin/sleep")
            .arg("30")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        fs::write(
            Path::new(&workspace).join("owned-child-pid"),
            child.id().to_string(),
        )
        .unwrap();
        std::process::exit(0);
    }
    #[test]
    fn custody_actual_owner_exit_does_not_retire_live_descendant() {
        let f = Fixture::new();
        let custody = f.custody();
        drop(custody);
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "resource_custody::tests::custody_crash_owner_entrypoint",
                "--nocapture",
            ])
            .env("MAGI_CUSTODY_CRASH_FIXTURE", &f.0)
            .status()
            .unwrap();
        assert!(status.success());
        let pid: i32 = fs::read_to_string(f.0.join("owned-child-pid"))
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(unsafe { libc::kill(pid, 0) }, 0);
        let reopened = ResourceCustody::open(f.namespace()).unwrap();
        assert_eq!(reopened.unresolved_operations().unwrap(), 1);
        assert!(fixture_exclusive(&reopened).is_err());
        assert_eq!(unsafe { libc::kill(pid, libc::SIGKILL) }, 0);
        assert!(fixture_exclusive(&reopened).is_err());
    }
    #[test]
    fn custody_bootstrap_rejects_symlink_and_replaced_publication_parent() {
        let f = Fixture::new();
        let namespace = f.namespace();
        fs::create_dir_all(namespace.publication.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&f.0, &namespace.publication).unwrap();
        assert!(ResourceCustody::bootstrap_empty(namespace).is_err());
        fs::remove_file(f.0.join("src-tauri/target/debug")).unwrap();
        let f = Fixture::new();
        fs::create_dir_all(f.0.join("src-tauri/target/debug")).unwrap();
        let namespace = f.namespace();
        fs::rename(
            f.0.join("src-tauri/target"),
            f.0.join("src-tauri/retired-target"),
        )
        .unwrap();
        fs::create_dir_all(f.0.join("src-tauri/target/debug")).unwrap();
        assert!(ResourceCustody::bootstrap_empty(namespace).is_err());
    }

    #[test]
    fn custody_drop_and_live_child_remain_unresolved_across_reopen() {
        let f = Fixture::new();
        let custody = f.custody();
        let reader = custody.begin_reader(&"b".repeat(64), request()).unwrap();
        let mut child = Command::new("/bin/sleep")
            .arg("30")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        drop(reader);
        drop(custody);
        let reopened = ResourceCustody::open(f.namespace()).unwrap();
        assert!(child.try_wait().unwrap().is_none());
        assert_eq!(reopened.unresolved_operations().unwrap(), 1);
        assert!(fixture_exclusive(&reopened).is_err());
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(fixture_exclusive(&reopened).is_err());
    }
    #[test]
    fn custody_reader_and_exclusive_order_and_lock_replacement_fail_closed() {
        let f = Fixture::new();
        let custody = f.custody();
        let exclusive = fixture_exclusive(&custody).unwrap();
        assert!(custody.begin_reader(&"c".repeat(64), request()).is_err());
        drop(exclusive);
        let root = request();
        let reader = custody.begin_reader(&"c".repeat(64), root.clone()).unwrap();
        assert!(fixture_exclusive(&custody).is_err());
        root.revoke();
        let proof = reader.observe_cleanup().unwrap();
        reader.finish_observed(proof).unwrap();
        let reader = custody.begin_reader(&"d".repeat(64), request()).unwrap();
        let path = custody.inner.namespace.control.join("custody.lock");
        fs::rename(&path, path.with_extension("old")).unwrap();
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .unwrap();
        assert!(reader.observe_cleanup().is_err());
        drop(reader);
        assert!(fixture_exclusive(&custody).is_err());
    }
    #[test]
    fn custody_lock_aba_and_journal_replacement_fail_closed_after_reopen() {
        let f = Fixture::new();
        let custody = f.custody();
        let path = custody.inner.namespace.control.join("custody.lock");
        let old = path.with_extension("retained");
        fs::rename(&path, &old).unwrap();
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .unwrap();
        fs::remove_file(&path).unwrap();
        fs::rename(&old, &path).unwrap();
        assert!(custody.begin_reader(&"a".repeat(64), request()).is_err());
        assert!(ResourceCustody::open(f.namespace()).is_err());
        let f = Fixture::new();
        let custody = f.custody();
        let path = custody.inner.journal.path.clone();
        let bytes = fs::read(&path).unwrap();
        fs::rename(&path, path.with_extension("old")).unwrap();
        fs::write(&path, bytes).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(ResourceCustody::open(f.namespace()).is_err());
    }

    #[test]
    fn custody_wrong_operation_proof_and_malformed_journal_never_grant_permission() {
        let f = Fixture::new();
        let custody = f.custody();
        let a = request();
        let b = request();
        let first = custody.begin_reader(&"e".repeat(64), a.clone()).unwrap();
        let second = custody.begin_reader(&"e".repeat(64), b.clone()).unwrap();
        a.revoke();
        b.revoke();
        let proof = first.observe_cleanup().unwrap();
        assert!(second.finish_observed(proof).is_err());
        drop(first);
        assert_eq!(custody.unresolved_operations().unwrap(), 2);
        fs::write(&custody.inner.journal.path, b"malformed").unwrap();
        assert!(custody.unresolved_operations().is_err());
    }
    #[test]
    fn custody_bootstrap_refuses_existing_resource_generation() {
        let f = Fixture::new();
        let namespace = f.namespace();
        fs::create_dir_all(namespace.publication.join("provider")).unwrap();
        assert!(ResourceCustody::bootstrap_empty(namespace).is_err());
    }
}
