use thiserror::Error;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("SQLite operation failed: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("filesystem operation failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON serialization failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("domain operation failed: {0}")]
    Domain(#[from] magi_domain::DomainError),
    #[error("context manifest validation failed: {0}")]
    Context(#[from] magi_context::DisclosureError),
    #[error("another application process owns the local store")]
    WriterAlreadyOpen,
    #[error("database schema version {found} is newer than supported version {supported}")]
    FutureSchema { found: u32, supported: u32 },
    #[error("database migration {version} checksum differs from the recorded migration")]
    MigrationChecksum { version: u32 },
    #[error("database integrity check failed: {0}")]
    Integrity(String),
    #[error("database record is invalid or inconsistent: {0}")]
    Corrupt(String),
    #[error("stored command key was reused with a different payload")]
    IdempotencyConflict,
    #[error("event cursor belongs to an earlier store generation")]
    CursorExpired,
    #[error("run history changed while paging; refresh the first page")]
    HistoryCursorExpired,
    #[error("run history cursor belongs to different filters")]
    HistoryFilterMismatch,
    #[error("run does not exist: {0}")]
    RunNotFound(String),
    #[error("run revision conflict: expected {expected}, current {actual}")]
    RevisionConflict { expected: u64, actual: u64 },
    #[error("dispatch batch is missing or its immutable payload does not match")]
    DispatchBatchConflict,
    #[error("dispatch does not exist: {0}")]
    DispatchNotFound(String),
    #[error("dispatch belongs to a fenced store or run generation")]
    DispatchFenced,
    #[error("dispatch state conflict: expected {expected}, current {actual}")]
    DispatchStateConflict { expected: String, actual: String },
    #[error("context draft revision conflict: expected {expected:?}, current {actual:?}")]
    DraftRevisionConflict {
        expected: Option<u64>,
        actual: Option<u64>,
    },
    #[error("role preset revision conflict: expected {expected:?}, current {actual:?}")]
    RolePresetRevisionConflict {
        expected: Option<u64>,
        actual: Option<u64>,
    },
    #[error("factory role preset is immutable: {0}")]
    FactoryPresetImmutable(String),
    #[error("role preset does not exist: {0}")]
    RolePresetNotFound(String),
    #[error("active role preset selection conflict: expected {expected}, current {actual}")]
    ActiveRolePresetRevisionConflict { expected: u64, actual: u64 },
    #[error("provider profile revision conflict: expected {expected:?}, current {actual:?}")]
    ProviderProfileRevisionConflict {
        expected: Option<u64>,
        actual: Option<u64>,
    },
    #[error("provider profile does not exist")]
    ProviderProfileNotFound,
    #[error("provider profile belongs to a different provider")]
    ProviderProfileProviderMismatch,
    #[error(
        "active provider profile selection conflict: expected {expected:?}, current {actual:?}"
    )]
    ActiveProviderProfileRevisionConflict {
        expected: Option<u64>,
        actual: Option<u64>,
    },
    #[error("runtime home binding is already owned by another provider profile")]
    ProviderHomeBindingConflict,
    #[error("a provider profile runtime home cannot be changed")]
    ProviderHomeBindingImmutable,
    #[error("immutable content object is missing: {0}")]
    MissingObject(String),
    #[error("storage state lock was poisoned")]
    LockPoisoned,
    #[error("this operation cannot replace an immutable stored object: {0}")]
    ImmutableConflict(String),
    #[error("run has unresolved references or active work and cannot be deleted")]
    DeletionBlocked,
}
