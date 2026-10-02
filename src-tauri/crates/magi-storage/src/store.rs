use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use fs2::FileExt;
use magi_context::{
    DisclosureState, FreshnessObservation, FreshnessStatus, ManifestSourceState,
    SourceCaptureManifest,
};
use magi_domain::{
    AcpMode, AcpModelBindingSnapshot, AssessmentStage, Ballot, CommandEnvelope, CommandKind,
    CommandReceipt, CoreId, CoreRoleDefinition, Digest, DomainEvent, EventPayload, EventPosition,
    InputSnapshot, LiveProviderResult, LiveRunCancellationSnapshot, LiveRunEvent,
    LiveRunEventCursor, LiveRunEventKind, LiveRunFailure, LiveRunProviderOutcome,
    LiveRunQueueProjection, LiveRunSnapshot, LiveRunStatus, ProposalSnapshot,
    ProviderCatalogSnapshot, ProviderProfileInput, ProviderProfileRevision, RoleAssessment,
    RolePresetRevision, RunAggregate, RunPersistenceState, RunSnapshot, RunStatus, Tally,
    canonical_json, status_name,
};
use rusqlite::types::Value;
use rusqlite::{
    Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior, params,
};
use serde::Serialize;
use uuid::Uuid;

use crate::{
    ActiveProviderProfileSelection, ActiveRolePresetSelection, CommitResult, ConsoleSnapshot,
    ContentObjectRef, ContextDraft, ContextDraftSummary, ConversationSummary, DispatchBatchReceipt,
    DispatchEvent, DispatchEventCursor, DispatchEventPage, DispatchIdentity, DispatchRecord,
    DispatchState, DispatchTransitionDisposition, DispatchTransitionResult,
    LiveProviderResultInput, LiveRunAdmissionOutcome, LiveRunAdmissionRequest, LiveRunChange,
    LiveRunClaim, LiveRunReceipt, PreparedDispatch, ProviderProfileSummary, ProviderSourceScope,
    RolePresetInput, RolePresetSource, RolePresetSummary, RunDossier, RunEventCursor, RunEventPage,
    RunHistoryCursor, RunHistoryPage, RunHistoryRequest, RunSummary, SourceFreshnessRecord,
    StorageError, StoreIdentity, StoredEvent,
};

#[path = "evidence.rs"]
mod evidence;
#[path = "lifecycle.rs"]
mod lifecycle;
pub use lifecycle::{
    BackupManifest, DeletionReceipt, EvidenceDeletionPreview, EvidenceView, RestoreReceipt,
};

pub trait LiveRunPausePermission {
    fn validate(
        &self,
        expected: &crate::AdmissionExecutionAuthority,
        claim: &LiveRunClaim,
    ) -> Result<(), StorageError>;
}

struct LiveCancellationOutcome {
    next_status: LiveRunStatus,
    provider_outcome: LiveRunProviderOutcome,
    failure: Option<LiveRunFailure>,
}

const SCHEMA_VERSION: u32 = 16;
const LEGACY_V2_WEAK_MIGRATION_SHA256: &str =
    "19d2909b7e58e0c7f7ecd8cd4bec60e2265cc586633f1f31d8090cbfd6e2c021";
pub const LIVE_RUN_QUEUE_CAPACITY: u8 = 10;
pub const LIVE_RUN_DISPATCH_CAPACITY: u8 = 10;
const LIVE_RUN_ACTIVE_CAPACITY: u8 = 3;
pub const MAX_OBJECT_BYTES: u64 = 100 * 1024 * 1024;
const MAX_EVENT_PAGE: usize = 10_000;
const MIGRATION_1: &str = include_str!("../migrations/0001_initial.sql");
const MIGRATION_2: &str = include_str!("../migrations/0002_role_history_freshness.sql");
const MIGRATION_3: &str = include_str!("../migrations/0003_dispatch_batches_and_events.sql");
const MIGRATION_4: &str =
    include_str!("../migrations/0004_provider_profiles_and_factory_presets.sql");
const MIGRATION_5: &str = include_str!("../migrations/0005_provider_catalog_and_live_runs.sql");
const MIGRATION_6: &str = include_str!("../migrations/0006_live_run_cancellation.sql");
const MIGRATION_7: &str = include_str!("../migrations/0007_source_freshness_strict_check.sql");
const MIGRATION_8: &str = include_str!("../migrations/0008_provider_source_scopes.sql");
const MIGRATION_9: &str = include_str!("../migrations/0009_live_run_dispatch_reservations.sql");
const MIGRATION_10: &str = include_str!("../migrations/0010_provider_model_selections.sql");
const MIGRATION_11: &str = include_str!("../migrations/0011_record_lifecycle.sql");
const MIGRATION_12: &str = include_str!("../migrations/0012_record_deletion_authority.sql");
const MIGRATION_13: &str = include_str!("../migrations/0013_admission_request_authority.sql");
const MIGRATION_14: &str = include_str!("../migrations/0014_admission_execution_lineage.sql");
const MIGRATION_15: &str = include_str!("../migrations/0015_live_run_needs_input_pause.sql");
const MIGRATION_16: &str =
    include_str!("../migrations/0016_clarification_descendant_authority.sql");
const MIGRATION_2_LEGACY_WEAK_CONTRACT: &str =
    include_str!("../schema_contracts/v2_legacy_weak.sql");

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum V2MigrationSchema {
    Strict,
    LegacyWeak,
}

pub struct AdmissionPublication<'a, F> {
    accepted_at: &'a str,
    initial_conversation_title: Option<&'a str>,
    commit_permission: F,
    execution_authority: Option<crate::AdmissionExecutionAuthority>,
}

pub struct Storage {
    connection: Mutex<Connection>,
    _writer_lock: File,
    objects_root: PathBuf,
    temp_root: PathBuf,
    identity: StoreIdentity,
    startup_recovery: crate::RecoveryReport,
}

pub struct StorageReader {
    connection: Mutex<Connection>,
    objects_root: PathBuf,
}

impl StorageReader {
    pub fn load_admission_request_cancellation(
        &self,
        intent: &crate::AdmissionRequestIntent,
    ) -> Result<Option<crate::AdmissionRequestCancellationReceipt>, StorageError> {
        let binding = admission_request_binding(intent)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
        let identity = read_only_store_identity(&transaction)?;
        let result = if identity.schema_version >= 13 {
            validate_admission_binding_from(&transaction, &binding)?;
            load_admission_cancellation_from(&transaction, &binding)?
        } else {
            None
        };
        transaction.commit()?;
        Ok(result)
    }

    pub fn open_read_only(data_root: impl AsRef<Path>) -> Result<Self, StorageError> {
        let data_root = data_root.as_ref();
        let state_root = data_root.join("state");
        let database_path = state_root.join("magi.sqlite");
        let objects_root = data_root.join("objects");

        for path in [
            data_root,
            state_root.as_path(),
            database_path.as_path(),
            objects_root.as_path(),
        ] {
            reject_symlink_if_present(path)?;
        }
        if !fs::metadata(data_root)?.is_dir() || !fs::metadata(&state_root)?.is_dir() {
            return Err(StorageError::Integrity(
                "read-only store roots must be existing directories".to_owned(),
            ));
        }
        match fs::metadata(&objects_root) {
            Ok(metadata) if !metadata.is_dir() => {
                return Err(StorageError::Integrity(
                    "content object root must be a directory".to_owned(),
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(StorageError::Io(error)),
        }

        let connection = Connection::open_with_flags(
            &database_path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        connection.busy_timeout(Duration::from_secs(5))?;
        read_only_store_identity(&connection)?;
        Ok(Self {
            connection: Mutex::new(connection),
            objects_root,
        })
    }

    pub fn identity(&self) -> Result<StoreIdentity, StorageError> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
        let identity = read_only_store_identity(&transaction)?;
        transaction.commit()?;
        Ok(identity)
    }

    pub fn admission_execution_authority(
        &self,
    ) -> Result<crate::AdmissionExecutionAuthority, StorageError> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
        read_only_store_identity(&transaction)?;
        let authority = read_admission_execution_authority(&transaction)?;
        transaction.commit()?;
        Ok(authority)
    }

    pub fn load_provider_catalog_snapshot(
        &self,
        catalog_snapshot_id: &str,
    ) -> Result<Option<ProviderCatalogSnapshot>, StorageError> {
        validate_text("catalog_snapshot_id", catalog_snapshot_id, 128)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
        let identity = read_only_store_identity(&transaction)?;
        if identity.schema_version < 5 {
            transaction.commit()?;
            return Ok(None);
        }
        let snapshot = load_provider_catalog_snapshot_from(&transaction, catalog_snapshot_id)?;
        transaction.commit()?;
        Ok(snapshot)
    }

    pub fn load_core_execution_witnesses(
        &self,
        references: &[crate::CoreBindingReference],
    ) -> Result<[magi_domain::CatalogExecutionWitness; 3], StorageError> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
        read_only_store_identity(&transaction)?;
        let witnesses = load_core_execution_witnesses_from(&transaction, references)?;
        transaction.commit()?;
        Ok(witnesses)
    }

    pub fn get_live_run_snapshot(
        &self,
        run_id: &str,
        after_sequence: u64,
    ) -> Result<LiveRunSnapshot, StorageError> {
        validate_text("run_id", run_id, 128)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
        let identity = read_only_store_identity(&transaction)?;
        if identity.schema_version < 5 {
            transaction.commit()?;
            return Err(StorageError::RunNotFound(run_id.to_owned()));
        }
        let snapshot = load_live_run_snapshot_from(
            &transaction,
            &identity,
            &self.objects_root,
            run_id,
            after_sequence,
        )?;
        transaction.commit()?;
        Ok(snapshot)
    }

    pub fn load_run_dossier(&self, run_id: &str) -> Result<RunDossier, StorageError> {
        validate_text("run_id", run_id, 128)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
        let identity = read_only_store_identity(&transaction)?;
        let dossier = load_run_dossier_from(&transaction, &identity, run_id)?;
        transaction.commit()?;
        Ok(dossier)
    }

    pub fn load_live_dispatch_projection(
        &self,
        run_id: &str,
    ) -> Result<crate::LiveDispatchProjection, StorageError> {
        validate_text("run_id", run_id, 128)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
        let projection = load_live_dispatch_projection_from(&transaction, run_id)?;
        transaction.commit()?;
        Ok(projection)
    }

    pub fn events_after_for_run(
        &self,
        cursor: &RunEventCursor,
        limit: usize,
    ) -> Result<RunEventPage, StorageError> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
        let identity = read_only_store_identity(&transaction)?;
        let page = run_events_after_from(&transaction, &identity, cursor, limit)?;
        transaction.commit()?;
        Ok(page)
    }

    fn connection(&self) -> Result<std::sync::MutexGuard<'_, Connection>, StorageError> {
        self.connection
            .lock()
            .map_err(|_| StorageError::LockPoisoned)
    }
}

impl Storage {
    pub fn admission_execution_authority(
        &self,
    ) -> Result<crate::AdmissionExecutionAuthority, StorageError> {
        let connection = self.connection()?;
        read_admission_execution_authority(&connection)
    }

    pub fn validate_admission_execution_authority(
        &self,
        expected: &crate::AdmissionExecutionAuthority,
    ) -> Result<(), StorageError> {
        let connection = self.connection()?;
        let current = read_admission_execution_authority(&connection)?;
        if !current.active || current != *expected {
            return Err(StorageError::DispatchFenced);
        }
        Ok(())
    }

    /// Switches from an exclusively owned, quiescent prior writer. Closing the prior
    /// authority commits first, so a later failure leaves both roots inert. The native
    /// permission must hold revocation and prove all owned workers/effects settled;
    /// database terminal states alone do not prove external quiescence.
    pub fn activate_restored_execution_checked<F, P>(
        &mut self,
        previous: &mut Storage,
        quiescence_permission: F,
    ) -> Result<(crate::AdmissionExecutionAuthority, P), StorageError>
    where
        F: FnOnce(&crate::AdmissionExecutionAuthority) -> Result<P, StorageError>,
        P: crate::AdmissionActivationPermission,
    {
        let target = self.admission_execution_authority()?;
        if target.active {
            return Err(StorageError::DispatchFenced);
        }
        let prior = previous.admission_execution_authority()?;
        let generation = target
            .store_generation
            .max(prior.store_generation)
            .checked_add(1)
            .ok_or_else(|| StorageError::Integrity("execution generation overflow".into()))?;
        let permission = quiescence_permission(&prior)?;
        {
            let mut c = previous.connection()?;
            let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let unsettled: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM live_run_outbox WHERE state NOT IN ('completed','cancelled','failed') UNION ALL SELECT 1 FROM dispatch_outbox WHERE state IN ('prepared','dispatched','unknown') UNION ALL SELECT 1 FROM live_run_dispatch_reservations WHERE state NOT IN ('settled','released'))",[],|r|r.get(0))?;
            if unsettled {
                return Err(StorageError::DeletionBlocked);
            }
            permission.validate(&prior)?;
            tx.execute(
                "UPDATE store_meta SET value='inert' WHERE key='admission_activation_state'",
                [],
            )?;
            tx.execute(
                "UPDATE store_meta SET value=?1 WHERE key='store_generation'",
                [generation.to_string()],
            )?;
            tx.commit()?;
        }
        previous.identity.generation = generation;
        {
            let mut c = self.connection()?;
            let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let current = read_admission_execution_authority(&tx)?;
            if current != target {
                return Err(StorageError::DispatchFenced);
            }
            let restored: bool = tx.query_row(
                "SELECT restored=1 FROM admission_execution_lineages WHERE lineage_id=?1",
                [&target.lineage_id],
                |r| r.get(0),
            )?;
            if !restored {
                return Err(StorageError::DispatchFenced);
            }
            tx.execute(
                "UPDATE store_meta SET value=?1 WHERE key='store_generation'",
                [generation.to_string()],
            )?;
            tx.execute(
                "UPDATE store_meta SET value='active' WHERE key='admission_activation_state'",
                [],
            )?;
            tx.commit()?;
        }
        self.identity.generation = generation;
        Ok((self.admission_execution_authority()?, permission))
    }

    pub fn register_clarification_admission_request(
        &self,
        expected: &crate::AdmissionExecutionAuthority,
        intent: &crate::ClarificationAdmissionIntent,
        accepted_at: &str,
    ) -> Result<crate::AdmissionRequestBinding, StorageError> {
        validate_text("accepted_at", accepted_at, 64)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (binding, manifest_digest, base_digest) =
            clarification_request_binding(&transaction, expected, intent, true)?;
        validate_admission_binding_from(&transaction, &binding)?;
        validate_admission_command_namespace(&transaction, &binding)?;
        reject_cancelled_admission(&transaction, &binding)?;
        insert_admission_binding(&transaction, &binding, accepted_at)?;
        insert_clarification_intent(&transaction, intent, &manifest_digest, &base_digest)?;
        transaction.commit()?;
        Ok(binding)
    }

    pub fn validate_clarification_replay_intent(
        &self,
        intent: &crate::ClarificationAdmissionIntent,
    ) -> Result<(), StorageError> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
        read_only_store_identity(&transaction)?;
        validate_clarification_replay_intent_from(&transaction, intent)?;
        transaction.rollback()?;
        Ok(())
    }

    pub fn cancel_clarification_admission_request(
        &self,
        expected: &crate::AdmissionExecutionAuthority,
        intent: &crate::ClarificationAdmissionIntent,
        command_id: &str,
        idempotency_key: &str,
        accepted_at: &str,
    ) -> Result<crate::AdmissionRequestCancellationReceipt, StorageError> {
        self.cancel_admission_request_inner(
            &intent.request,
            Some((expected, intent)),
            command_id,
            idempotency_key,
            accepted_at,
        )
    }

    /// Registers immutable intent only; the binding does not grant execution permission.
    pub fn register_admission_request(
        &self,
        intent: &crate::AdmissionRequestIntent,
        accepted_at: &str,
    ) -> Result<crate::AdmissionRequestBinding, StorageError> {
        validate_text("accepted_at", accepted_at, 64)?;
        let binding = admission_request_binding(intent)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        reject_clarification_draft_downcast(&transaction, intent)?;
        validate_admission_binding_from(&transaction, &binding)?;
        validate_admission_command_namespace(&transaction, &binding)?;
        reject_cancelled_admission(&transaction, &binding)?;
        insert_admission_binding(&transaction, &binding, accepted_at)?;
        transaction.commit()?;
        Ok(binding)
    }

    pub fn load_admission_request_cancellation(
        &self,
        intent: &crate::AdmissionRequestIntent,
    ) -> Result<Option<crate::AdmissionRequestCancellationReceipt>, StorageError> {
        let binding = admission_request_binding(intent)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
        read_only_store_identity(&transaction)?;
        validate_admission_binding_from(&transaction, &binding)?;
        let receipt = load_admission_cancellation_from(&transaction, &binding)?;
        transaction.commit()?;
        Ok(receipt)
    }

    /// Commits the request fence. If admission already committed, the caller must cancel
    /// the returned run through the existing run cancellation authority before acknowledging stop.
    pub fn cancel_admission_request(
        &self,
        intent: &crate::AdmissionRequestIntent,
        command_id: &str,
        idempotency_key: &str,
        accepted_at: &str,
    ) -> Result<crate::AdmissionRequestCancellationReceipt, StorageError> {
        self.cancel_admission_request_inner(intent, None, command_id, idempotency_key, accepted_at)
    }

    fn cancel_admission_request_inner(
        &self,
        intent: &crate::AdmissionRequestIntent,
        clarification: Option<(
            &crate::AdmissionExecutionAuthority,
            &crate::ClarificationAdmissionIntent,
        )>,
        command_id: &str,
        idempotency_key: &str,
        accepted_at: &str,
    ) -> Result<crate::AdmissionRequestCancellationReceipt, StorageError> {
        validate_text("command_id", command_id, 128)?;
        validate_text("idempotency_key", idempotency_key, 256)?;
        validate_text("accepted_at", accepted_at, 64)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let rich = clarification
            .map(|(expected, rich)| {
                clarification_request_binding(&transaction, expected, rich, false)
            })
            .transpose()?;
        let binding = match &rich {
            Some((binding, _, _)) => binding.clone(),
            None => admission_request_binding(intent)?,
        };
        let payload_digest = admission_cancellation_payload_digest(&binding)?;
        validate_admission_binding_from(&transaction, &binding)?;
        validate_admission_command_namespace(&transaction, &binding)?;
        let prior =
            load_admission_cancellation_by_command(&transaction, command_id, idempotency_key)?;
        if let Some(receipt) = prior {
            if receipt.command_id != command_id
                || receipt.idempotency_key != idempotency_key
                || receipt.request != binding
            {
                return Err(StorageError::IdempotencyConflict);
            }
            transaction.rollback()?;
            return Ok(receipt);
        }
        if command_id == binding.command_id || idempotency_key == binding.idempotency_key
            || transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM commands WHERE command_id=?1 OR idempotency_key=?2 UNION ALL SELECT 1 FROM admission_request_bindings WHERE command_id=?1 OR idempotency_key=?2 UNION ALL SELECT 1 FROM live_run_receipts WHERE command_id=?1 OR idempotency_key=?2 UNION ALL SELECT 1 FROM run_command_tombstones WHERE command_id=?1 OR idempotency_key=?2 UNION ALL SELECT 1 FROM live_run_cancellation_receipts WHERE command_id=?1 OR idempotency_key=?2)",
                params![command_id,idempotency_key], |row|row.get::<_,bool>(0))? {
            return Err(StorageError::IdempotencyConflict);
        }
        insert_admission_binding(&transaction, &binding, accepted_at)?;
        if let (Some((_, intent)), Some((_, manifest, base))) = (clarification, &rich) {
            insert_clarification_intent(&transaction, intent, manifest, base)?;
        }
        let admitted_run_id: Option<String> = transaction
            .query_row(
                "SELECT run_id FROM live_run_receipts WHERE idempotency_key=?1",
                [&binding.idempotency_key],
                |row| row.get(0),
            )
            .optional()?;
        let receipt = crate::AdmissionRequestCancellationReceipt {
            schema_version: 1,
            command_id: command_id.to_owned(),
            idempotency_key: idempotency_key.to_owned(),
            request: binding,
            accepted_at: accepted_at.to_owned(),
            admitted_run_id,
        };
        transaction.execute(
            "INSERT INTO admission_request_cancellations(command_id,idempotency_key,request_command_id,request_idempotency_key,intent_digest,payload_digest,accepted_at,admitted_run_id,receipt_json) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![receipt.command_id,receipt.idempotency_key,receipt.request.command_id,receipt.request.idempotency_key,receipt.request.intent_digest.as_str(),payload_digest.as_str(),receipt.accepted_at,receipt.admitted_run_id,serde_json::to_string(&receipt)?],
        )?;
        transaction.commit()?;
        Ok(receipt)
    }

    pub fn admission_publication<'a, F>(
        accepted_at: &'a str,
        initial_conversation_title: Option<&'a str>,
        commit_permission: F,
    ) -> AdmissionPublication<'a, F> {
        AdmissionPublication {
            accepted_at,
            initial_conversation_title,
            commit_permission,
            execution_authority: None,
        }
    }

    pub fn admission_publication_with_authority<'a, F>(
        accepted_at: &'a str,
        initial_conversation_title: Option<&'a str>,
        expected: &crate::AdmissionExecutionAuthority,
        commit_permission: F,
    ) -> AdmissionPublication<'a, F> {
        AdmissionPublication {
            accepted_at,
            initial_conversation_title,
            commit_permission,
            execution_authority: Some(expected.clone()),
        }
    }

    pub fn open_or_create(data_root: impl AsRef<Path>) -> Result<Self, StorageError> {
        let data_root = data_root.as_ref().to_path_buf();
        create_private_dir(&data_root)?;
        let state_root = data_root.join("state");
        let objects_root = data_root.join("objects");
        let temp_root = data_root.join("tmp");
        create_private_dir(&state_root)?;
        create_private_dir(&objects_root)?;
        create_private_dir(&temp_root)?;

        let lock_path = state_root.join("writer.lock");
        reject_symlink_if_present(&lock_path)?;
        let writer_lock = private_file(&lock_path)?;
        match writer_lock.try_lock_exclusive() {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                return Err(StorageError::WriterAlreadyOpen);
            }
            Err(error) => return Err(StorageError::Io(error)),
        }

        let database_path = state_root.join("magi.sqlite");
        reject_symlink_if_present(&database_path)?;
        let mut connection = Connection::open(&database_path)?;
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.pragma_update(None, "foreign_keys", true)?;
        initialize_schema(&mut connection)?;
        set_private_file_permissions(&database_path)?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "FULL")?;
        seed_factory_role_presets(&mut connection)?;
        let identity = advance_store_generation(&mut connection)?;

        let mut storage = Self {
            connection: Mutex::new(connection),
            _writer_lock: writer_lock,
            objects_root,
            temp_root,
            identity,
            startup_recovery: crate::RecoveryReport {
                previous_generation: 0,
                current_generation: 0,
                findings: Vec::new(),
            },
        };
        storage.startup_recovery = storage.recover_after_open()?;
        Ok(storage)
    }

    pub fn identity(&self) -> &StoreIdentity {
        &self.identity
    }

    pub fn startup_recovery(&self) -> &crate::RecoveryReport {
        &self.startup_recovery
    }

    pub fn create_conversation(
        &self,
        conversation_id: &str,
        title: &str,
        created_at: &str,
    ) -> Result<ConversationSummary, StorageError> {
        validate_text("conversation_id", conversation_id, 128)?;
        validate_text("title", title, 512)?;
        validate_text("created_at", created_at, 64)?;
        let connection = self.connection()?;
        connection.execute(
            "INSERT INTO conversations (conversation_id, title, revision, created_at, updated_at) VALUES (?1, ?2, 0, ?3, ?3)",
            params![conversation_id, title, created_at],
        )?;
        Ok(ConversationSummary {
            conversation_id: conversation_id.to_owned(),
            title: title.to_owned(),
            revision: 0,
            created_at: created_at.to_owned(),
            updated_at: created_at.to_owned(),
        })
    }

    pub fn list_conversations(&self) -> Result<Vec<ConversationSummary>, StorageError> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT conversation_id, title, revision, created_at, updated_at FROM conversations WHERE deleted_at IS NULL ORDER BY updated_at DESC, conversation_id",
        )?;
        let rows = statement.query_map([], |row| {
            let revision: i64 = row.get(2)?;
            Ok(ConversationSummary {
                conversation_id: row.get(0)?,
                title: row.get(1)?,
                revision: revision.max(0) as u64,
                created_at: row.get(3)?,
                updated_at: row.get(4)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StorageError::from)
    }

    pub fn save_role_preset(
        &self,
        input: &RolePresetInput,
        expected_revision: Option<u64>,
        updated_at: &str,
    ) -> Result<RolePresetRevision, StorageError> {
        validate_text("updated_at", updated_at, 64)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current: Option<(i64, String)> = transaction
            .query_row(
                "SELECT revision, source FROM role_preset_heads WHERE preset_id = ?1",
                [&input.preset_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let (current_revision, source) = match current {
            Some((value, source)) if value >= 0 => {
                let source = RolePresetSource::parse(&source).ok_or_else(|| {
                    StorageError::Corrupt("role preset source is invalid".to_owned())
                })?;
                (Some(value as u64), Some(source))
            }
            Some((value, _)) => {
                return Err(StorageError::Corrupt(format!(
                    "role preset revision is negative: {value}"
                )));
            }
            None => (None, None),
        };
        if source == Some(RolePresetSource::Factory) {
            return Err(StorageError::FactoryPresetImmutable(
                input.preset_id.clone(),
            ));
        }
        if current_revision != expected_revision {
            return Err(StorageError::RolePresetRevisionConflict {
                expected: expected_revision,
                actual: current_revision,
            });
        }
        let revision = current_revision
            .map(|value| value.checked_add(1))
            .unwrap_or(Some(0))
            .ok_or_else(|| StorageError::Integrity("role preset revision overflow".to_owned()))?;
        let preset = RolePresetRevision::new(
            input.preset_id.clone(),
            revision,
            input.display_name.clone(),
            input.roles.clone(),
        )?;
        let payload = serde_json::to_string(&preset)?;
        transaction.execute(
            "INSERT INTO role_preset_revisions (preset_id, revision, digest, payload_json, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![preset.preset_id, to_sql_integer(revision)?, preset.digest.as_str(), payload, updated_at],
        )?;
        let source = RolePresetSource::User.as_str();
        transaction.execute(
            "INSERT INTO role_preset_heads (preset_id, revision, source, updated_at) VALUES (?1, ?2, ?3, ?4) ON CONFLICT(preset_id) DO UPDATE SET revision = excluded.revision, updated_at = excluded.updated_at",
            params![preset.preset_id, to_sql_integer(revision)?, source, updated_at],
        )?;
        transaction.commit()?;
        Ok(preset)
    }

    pub fn load_role_preset(
        &self,
        preset_id: &str,
    ) -> Result<Option<RolePresetRevision>, StorageError> {
        validate_draft_id(preset_id)?;
        let connection = self.connection()?;
        let revision: Option<i64> = connection
            .query_row(
                "SELECT revision FROM role_preset_heads WHERE preset_id = ?1",
                [preset_id],
                |row| row.get(0),
            )
            .optional()?;
        let Some(revision) = revision else {
            return Ok(None);
        };
        let revision = u64::try_from(revision)
            .map_err(|_| StorageError::Corrupt("role preset revision is negative".to_owned()))?;
        load_role_preset_revision(&connection, preset_id, revision)?
            .map(Some)
            .ok_or_else(|| {
                StorageError::Corrupt("role preset head references a missing revision".to_owned())
            })
    }

    pub fn load_role_preset_revision(
        &self,
        preset_id: &str,
        revision: u64,
    ) -> Result<Option<RolePresetRevision>, StorageError> {
        validate_draft_id(preset_id)?;
        let connection = self.connection()?;
        load_role_preset_revision(&connection, preset_id, revision)
    }

    pub fn clone_role_preset(
        &self,
        source_preset_id: &str,
        source_revision: u64,
        new_preset_id: &str,
        display_name: &str,
        updated_at: &str,
    ) -> Result<RolePresetRevision, StorageError> {
        validate_draft_id(source_preset_id)?;
        validate_draft_id(new_preset_id)?;
        validate_text("display_name", display_name, 128)?;
        let source = self
            .load_role_preset_revision(source_preset_id, source_revision)?
            .ok_or_else(|| StorageError::RolePresetNotFound(source_preset_id.to_owned()))?;
        let mut roles = source.roles;
        for role in &mut roles {
            role.profile_id = Uuid::new_v4().to_string();
        }
        self.save_role_preset(
            &RolePresetInput {
                preset_id: new_preset_id.to_owned(),
                display_name: display_name.to_owned(),
                roles,
            },
            None,
            updated_at,
        )
    }

    pub fn set_active_role_preset(
        &self,
        preset_id: &str,
        expected_selection_revision: u64,
        updated_at: &str,
    ) -> Result<ActiveRolePresetSelection, StorageError> {
        validate_draft_id(preset_id)?;
        validate_text("updated_at", updated_at, 64)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current: Option<i64> = transaction
            .query_row(
                "SELECT selection_revision FROM active_role_preset_selection WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .optional()?;
        let current = current
            .filter(|value| *value >= 0)
            .map(|value| value as u64)
            .ok_or_else(|| {
                StorageError::Corrupt(
                    "active role preset selection is missing or invalid".to_owned(),
                )
            })?;
        if current != expected_selection_revision {
            return Err(StorageError::ActiveRolePresetRevisionConflict {
                expected: expected_selection_revision,
                actual: current,
            });
        }
        let exists: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM role_preset_heads WHERE preset_id = ?1)",
            [preset_id],
            |row| row.get(0),
        )?;
        if !exists {
            return Err(StorageError::RolePresetNotFound(preset_id.to_owned()));
        }
        let next_revision = current.checked_add(1).ok_or_else(|| {
            StorageError::Integrity("active role preset revision overflow".to_owned())
        })?;
        transaction.execute(
            "UPDATE active_role_preset_selection SET preset_id = ?1, selection_revision = ?2, updated_at = ?3 WHERE singleton = 1",
            params![preset_id, to_sql_integer(next_revision)?, updated_at],
        )?;
        transaction.commit()?;
        Ok(ActiveRolePresetSelection {
            preset_id: preset_id.to_owned(),
            selection_revision: next_revision,
            updated_at: updated_at.to_owned(),
        })
    }

    pub fn load_active_role_preset_selection(
        &self,
    ) -> Result<Option<ActiveRolePresetSelection>, StorageError> {
        let connection = self.connection()?;
        let selection = connection
            .query_row(
                "SELECT preset_id, selection_revision, updated_at FROM active_role_preset_selection WHERE singleton = 1",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()?;
        selection
            .map(|(preset_id, revision, updated_at)| {
                let selection_revision = u64::try_from(revision).map_err(|_| {
                    StorageError::Corrupt("active role preset revision is negative".to_owned())
                })?;
                Ok(ActiveRolePresetSelection {
                    preset_id,
                    selection_revision,
                    updated_at,
                })
            })
            .transpose()
    }

    pub fn list_role_presets(&self, limit: usize) -> Result<Vec<RolePresetSummary>, StorageError> {
        let limit = limit.min(100);
        if limit == 0 {
            return Ok(Vec::new());
        }
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT h.preset_id, r.revision, r.digest, r.payload_json, h.source, h.updated_at FROM role_preset_heads h JOIN role_preset_revisions r ON r.preset_id = h.preset_id AND r.revision = h.revision ORDER BY CASE h.source WHEN 'factory' THEN 0 ELSE 1 END, h.updated_at DESC, h.preset_id LIMIT ?1",
        )?;
        let rows = statement.query_map([limit as i64], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
            ))
        })?;
        let mut presets = Vec::new();
        for row in rows {
            let (preset_id, revision, digest, payload, source, updated_at) = row?;
            let revision = u64::try_from(revision).map_err(|_| {
                StorageError::Corrupt("role preset revision is negative".to_owned())
            })?;
            let preset: RolePresetRevision = serde_json::from_str(&payload)?;
            preset.validate()?;
            if preset.preset_id != preset_id
                || preset.revision != revision
                || preset.digest.as_str() != digest
            {
                return Err(StorageError::Corrupt(
                    "role preset payload does not match its stored key or digest".to_owned(),
                ));
            }
            let source = RolePresetSource::parse(&source)
                .ok_or_else(|| StorageError::Corrupt("role preset source is invalid".to_owned()))?;
            presets.push(RolePresetSummary {
                preset_id: preset.preset_id,
                display_name: preset.display_name,
                revision: preset.revision,
                digest: preset.digest,
                source,
                updated_at,
            });
        }
        Ok(presets)
    }

    pub fn save_provider_profile(
        &self,
        input: &ProviderProfileInput,
        expected_revision: Option<u64>,
        updated_at: &str,
    ) -> Result<ProviderProfileRevision, StorageError> {
        input.validate()?;
        validate_text("updated_at", updated_at, 64)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current: Option<(i64, String, String, String, String)> = transaction
            .query_row(
                "SELECT h.revision, h.provider_id, r.runtime_home_id, r.payload_json, r.digest FROM provider_profile_heads h JOIN provider_profile_revisions r ON r.provider_profile_id = h.provider_profile_id AND r.revision = h.revision WHERE h.provider_profile_id = ?1",
                [&input.provider_profile_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            )
            .optional()?;
        let current_revision = match current.as_ref().map(|(revision, _, _, _, _)| *revision) {
            Some(value) if value >= 0 => Some(value as u64),
            Some(value) => {
                return Err(StorageError::Corrupt(format!(
                    "provider profile revision is negative: {value}"
                )));
            }
            None => None,
        };
        if current_revision != expected_revision {
            return Err(StorageError::ProviderProfileRevisionConflict {
                expected: expected_revision,
                actual: current_revision,
            });
        }
        let mut credential_home_changed = false;
        if let Some((old_revision, provider_id, runtime_home_id, old_payload, old_digest)) =
            &current
        {
            if provider_id != &input.provider_id {
                return Err(StorageError::ProviderProfileProviderMismatch);
            }
            if runtime_home_id != &input.runtime_home_id {
                return Err(StorageError::ProviderHomeBindingImmutable);
            }
            let previous = decode_provider_profile(
                &input.provider_profile_id,
                *old_revision,
                old_digest,
                runtime_home_id,
                old_payload,
            )?;
            credential_home_changed = previous.credential_home != input.credential_home;
        }
        let revision = current_revision
            .map(|value| value.checked_add(1))
            .unwrap_or(Some(0))
            .ok_or_else(|| {
                StorageError::Integrity("provider profile revision overflow".to_owned())
            })?;
        let profile = ProviderProfileRevision::new(input.clone(), revision)?;
        let payload = serde_json::to_string(&profile)?;

        transaction.execute(
            "INSERT INTO provider_profile_home_roots (runtime_home_id, provider_profile_id) VALUES (?1, ?2) ON CONFLICT(runtime_home_id) DO NOTHING",
            params![profile.runtime_home_id, profile.provider_profile_id],
        )?;
        let home_owner: String = transaction.query_row(
            "SELECT provider_profile_id FROM provider_profile_home_roots WHERE runtime_home_id = ?1",
            [&profile.runtime_home_id],
            |row| row.get(0),
        )?;
        if home_owner != profile.provider_profile_id {
            return Err(StorageError::ProviderHomeBindingConflict);
        }
        transaction.execute(
            "INSERT INTO provider_profile_revisions (provider_profile_id, revision, digest, runtime_home_id, payload_json, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                profile.provider_profile_id,
                to_sql_integer(revision)?,
                profile.digest.as_str(),
                profile.runtime_home_id,
                payload,
                updated_at,
            ],
        )?;
        transaction.execute(
            "INSERT INTO provider_profile_heads (provider_profile_id, provider_id, revision, updated_at) VALUES (?1, ?2, ?3, ?4) ON CONFLICT(provider_profile_id) DO UPDATE SET revision = excluded.revision, updated_at = excluded.updated_at",
            params![profile.provider_profile_id, profile.provider_id, to_sql_integer(revision)?, updated_at],
        )?;
        if credential_home_changed {
            let selection_revision: Option<i64> = transaction.query_row(
                "SELECT selection_revision FROM active_provider_profile_selections WHERE provider_id=?1 AND provider_profile_id=?2",
                params![profile.provider_id, profile.provider_profile_id],
                |row| row.get(0),
            ).optional()?;
            if let Some(previous) = selection_revision {
                let next = u64::try_from(previous)
                    .map_err(|_| {
                        StorageError::Integrity(
                            "active provider profile selection revision is negative".into(),
                        )
                    })?
                    .checked_add(1)
                    .ok_or_else(|| {
                        StorageError::Integrity("active provider profile revision overflow".into())
                    })?;
                transaction.execute(
                    "UPDATE active_provider_profile_selections SET selection_revision=?3,updated_at=?4 WHERE provider_id=?1 AND provider_profile_id=?2",
                    params![profile.provider_id, profile.provider_profile_id, to_sql_integer(next)?, updated_at],
                )?;
            }
        }
        transaction.commit()?;
        Ok(profile)
    }

    pub fn load_provider_profile(
        &self,
        provider_profile_id: &str,
    ) -> Result<Option<ProviderProfileRevision>, StorageError> {
        validate_draft_id(provider_profile_id)?;
        let connection = self.connection()?;
        let row: Option<(i64, String, String, String)> = connection
            .query_row(
                "SELECT r.revision, r.digest, r.runtime_home_id, r.payload_json FROM provider_profile_heads h JOIN provider_profile_revisions r ON r.provider_profile_id = h.provider_profile_id AND r.revision = h.revision WHERE h.provider_profile_id = ?1",
                [provider_profile_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()?;
        row.map(|(revision, digest, runtime_home_id, payload)| {
            decode_provider_profile(
                provider_profile_id,
                revision,
                &digest,
                &runtime_home_id,
                &payload,
            )
        })
        .transpose()
    }

    pub fn load_provider_profile_revision(
        &self,
        provider_profile_id: &str,
        revision: u64,
    ) -> Result<Option<ProviderProfileRevision>, StorageError> {
        validate_draft_id(provider_profile_id)?;
        let revision = to_sql_integer(revision)?;
        let connection = self.connection()?;
        let row: Option<(i64, String, String, String)> = connection
            .query_row(
                "SELECT revision, digest, runtime_home_id, payload_json FROM provider_profile_revisions WHERE provider_profile_id = ?1 AND revision = ?2",
                params![provider_profile_id, revision],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()?;
        row.map(|(revision, digest, runtime_home_id, payload)| {
            decode_provider_profile(
                provider_profile_id,
                revision,
                &digest,
                &runtime_home_id,
                &payload,
            )
        })
        .transpose()
    }

    pub fn list_provider_profiles(
        &self,
        limit: usize,
    ) -> Result<Vec<ProviderProfileSummary>, StorageError> {
        let limit = limit.min(100);
        if limit == 0 {
            return Ok(Vec::new());
        }
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT r.provider_profile_id, r.revision, r.digest, r.runtime_home_id, r.payload_json, h.updated_at FROM provider_profile_heads h JOIN provider_profile_revisions r ON r.provider_profile_id = h.provider_profile_id AND r.revision = h.revision ORDER BY h.updated_at DESC, r.provider_profile_id LIMIT ?1",
        )?;
        let rows = statement.query_map([limit as i64], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
            ))
        })?;
        let mut profiles = Vec::new();
        for row in rows {
            let (profile_id, revision, digest, runtime_home_id, payload, updated_at) = row?;
            let profile = decode_provider_profile(
                &profile_id,
                revision,
                &digest,
                &runtime_home_id,
                &payload,
            )?;
            profiles.push(ProviderProfileSummary::from_revision(profile, updated_at));
        }
        Ok(profiles)
    }

    pub fn add_provider_source_scope(
        &self,
        provider_profile_id: &str,
        canonical_path: &str,
        root_device: u64,
        root_inode: u64,
        created_at: &str,
    ) -> Result<ProviderSourceScope, StorageError> {
        validate_draft_id(provider_profile_id)?;
        validate_text("canonical_path", canonical_path, 4096)?;
        validate_text("created_at", created_at, 64)?;
        if !canonical_path.starts_with('/') {
            return Err(StorageError::Corrupt(
                "source scope directory must be absolute".to_owned(),
            ));
        }
        let root_device = to_sql_integer(root_device)?;
        let root_inode = to_sql_integer(root_inode)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let profile_exists: bool = transaction.query_row(
            "SELECT EXISTS (SELECT 1 FROM provider_profile_heads WHERE provider_profile_id = ?1)",
            [provider_profile_id],
            |row| row.get(0),
        )?;
        if !profile_exists {
            return Err(StorageError::ProviderProfileNotFound);
        }
        let existing: Option<(String, i64, i64, String)> = transaction
            .query_row(
                "SELECT grant_id, root_device, root_inode, created_at FROM provider_source_scopes WHERE provider_profile_id = ?1 AND canonical_path = ?2 AND revoked_at IS NULL",
                params![provider_profile_id, canonical_path],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()?;
        if let Some((grant_id, existing_device, existing_inode, existing_created_at)) = existing {
            if existing_device != root_device || existing_inode != root_inode {
                return Err(StorageError::Integrity(
                    "source scope directory identity changed; revoke before authorizing it again"
                        .to_owned(),
                ));
            }
            transaction.commit()?;
            return Ok(ProviderSourceScope {
                grant_id,
                provider_profile_id: provider_profile_id.to_owned(),
                canonical_path: canonical_path.to_owned(),
                root_device: u64::try_from(existing_device).map_err(|_| {
                    StorageError::Corrupt("source scope device is invalid".to_owned())
                })?,
                root_inode: u64::try_from(existing_inode).map_err(|_| {
                    StorageError::Corrupt("source scope inode is invalid".to_owned())
                })?,
                created_at: existing_created_at,
                revoked_at: None,
            });
        }
        let grant_id = Uuid::new_v4().to_string();
        transaction.execute(
            "INSERT INTO provider_source_scopes (grant_id, provider_profile_id, canonical_path, root_device, root_inode, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![grant_id, provider_profile_id, canonical_path, root_device, root_inode, created_at],
        )?;
        transaction.commit()?;
        Ok(ProviderSourceScope {
            grant_id,
            provider_profile_id: provider_profile_id.to_owned(),
            canonical_path: canonical_path.to_owned(),
            root_device: u64::try_from(root_device)
                .map_err(|_| StorageError::Corrupt("source scope device is invalid".to_owned()))?,
            root_inode: u64::try_from(root_inode)
                .map_err(|_| StorageError::Corrupt("source scope inode is invalid".to_owned()))?,
            created_at: created_at.to_owned(),
            revoked_at: None,
        })
    }

    pub fn list_provider_source_scopes(
        &self,
        provider_profile_id: &str,
    ) -> Result<Vec<ProviderSourceScope>, StorageError> {
        validate_draft_id(provider_profile_id)?;
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT grant_id, canonical_path, root_device, root_inode, created_at, revoked_at FROM provider_source_scopes WHERE provider_profile_id = ?1 AND revoked_at IS NULL ORDER BY created_at, grant_id",
        )?;
        let rows = statement.query_map([provider_profile_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
            ))
        })?;
        rows.map(|row| {
            let (grant_id, canonical_path, root_device, root_inode, created_at, revoked_at) = row?;
            Ok(ProviderSourceScope {
                grant_id,
                provider_profile_id: provider_profile_id.to_owned(),
                canonical_path,
                root_device: u64::try_from(root_device).map_err(|_| {
                    StorageError::Corrupt("source scope device is invalid".to_owned())
                })?,
                root_inode: u64::try_from(root_inode).map_err(|_| {
                    StorageError::Corrupt("source scope inode is invalid".to_owned())
                })?,
                created_at,
                revoked_at,
            })
        })
        .collect()
    }

    pub fn revoke_provider_source_scope(
        &self,
        provider_profile_id: &str,
        grant_id: &str,
        revoked_at: &str,
    ) -> Result<bool, StorageError> {
        validate_draft_id(provider_profile_id)?;
        validate_draft_id(grant_id)?;
        validate_text("revoked_at", revoked_at, 64)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = transaction.execute(
            "UPDATE provider_source_scopes SET revoked_at = ?1 WHERE provider_profile_id = ?2 AND grant_id = ?3 AND revoked_at IS NULL",
            params![revoked_at, provider_profile_id, grant_id],
        )?;
        transaction.commit()?;
        Ok(changed == 1)
    }

    pub fn set_active_provider_profile(
        &self,
        provider_id: &str,
        provider_profile_id: &str,
        expected_selection_revision: Option<u64>,
        updated_at: &str,
    ) -> Result<ActiveProviderProfileSelection, StorageError> {
        validate_provider_id(provider_id)?;
        validate_draft_id(provider_profile_id)?;
        validate_text("updated_at", updated_at, 64)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current: Option<i64> = transaction
            .query_row(
                "SELECT selection_revision FROM active_provider_profile_selections WHERE provider_id = ?1",
                [provider_id],
                |row| row.get(0),
            )
            .optional()?;
        let current_revision = match current {
            Some(value) if value >= 0 => Some(value as u64),
            Some(_) => {
                return Err(StorageError::Corrupt(
                    "active provider profile revision is negative".to_owned(),
                ));
            }
            None => None,
        };
        if current_revision != expected_selection_revision {
            return Err(StorageError::ActiveProviderProfileRevisionConflict {
                expected: expected_selection_revision,
                actual: current_revision,
            });
        }
        let profile_provider: Option<String> = transaction
            .query_row(
                "SELECT provider_id FROM provider_profile_heads WHERE provider_profile_id = ?1",
                [provider_profile_id],
                |row| row.get(0),
            )
            .optional()?;
        let profile_provider = profile_provider.ok_or(StorageError::ProviderProfileNotFound)?;
        if profile_provider != provider_id {
            return Err(StorageError::ProviderProfileProviderMismatch);
        }
        let next_revision = current_revision
            .map(|revision| revision.checked_add(1))
            .unwrap_or(Some(0))
            .ok_or_else(|| {
                StorageError::Integrity("active provider profile revision overflow".to_owned())
            })?;
        transaction.execute(
            "INSERT INTO active_provider_profile_selections (provider_id, provider_profile_id, selection_revision, updated_at) VALUES (?1, ?2, ?3, ?4) ON CONFLICT(provider_id) DO UPDATE SET provider_profile_id = excluded.provider_profile_id, selection_revision = excluded.selection_revision, updated_at = excluded.updated_at",
            params![provider_id, provider_profile_id, to_sql_integer(next_revision)?, updated_at],
        )?;
        transaction.commit()?;
        Ok(ActiveProviderProfileSelection {
            provider_id: provider_id.to_owned(),
            provider_profile_id: provider_profile_id.to_owned(),
            selection_revision: next_revision,
            updated_at: updated_at.to_owned(),
        })
    }

    pub fn load_active_provider_profile_selection(
        &self,
        provider_id: &str,
    ) -> Result<Option<ActiveProviderProfileSelection>, StorageError> {
        validate_provider_id(provider_id)?;
        let connection = self.connection()?;
        let selection = connection
            .query_row(
                "SELECT provider_id, provider_profile_id, selection_revision, updated_at FROM active_provider_profile_selections WHERE provider_id = ?1",
                [provider_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()?;
        selection
            .map(|(provider_id, provider_profile_id, revision, updated_at)| {
                let selection_revision = u64::try_from(revision).map_err(|_| {
                    StorageError::Corrupt("active provider profile revision is negative".to_owned())
                })?;
                Ok(ActiveProviderProfileSelection {
                    provider_id,
                    provider_profile_id,
                    selection_revision,
                    updated_at,
                })
            })
            .transpose()
    }

    pub fn list_active_provider_profile_selections(
        &self,
    ) -> Result<Vec<ActiveProviderProfileSelection>, StorageError> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT provider_id, provider_profile_id, selection_revision, updated_at FROM active_provider_profile_selections ORDER BY provider_id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;
        let mut selections = Vec::new();
        for row in rows {
            let (provider_id, provider_profile_id, revision, updated_at) = row?;
            let selection_revision = u64::try_from(revision).map_err(|_| {
                StorageError::Corrupt("active provider profile revision is negative".to_owned())
            })?;
            selections.push(ActiveProviderProfileSelection {
                provider_id,
                provider_profile_id,
                selection_revision,
                updated_at,
            });
        }
        Ok(selections)
    }

    pub fn save_provider_catalog_snapshot(
        &self,
        snapshot: &ProviderCatalogSnapshot,
    ) -> Result<ProviderCatalogSnapshot, StorageError> {
        snapshot.validate()?;
        validate_text("catalog_snapshot_id", &snapshot.catalog_snapshot_id, 128)?;
        validate_text("provider_id", &snapshot.provider_id, 128)?;
        validate_text("provider_profile_id", &snapshot.provider_profile_id, 128)?;
        validate_text("fetched_at", &snapshot.fetched_at, 64)?;
        let payload = serde_json::to_string(snapshot)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        ensure_catalog_profile_is_current(&transaction, snapshot)?;

        let existing: Option<(String, String)> = transaction
            .query_row(
                "SELECT catalog_digest, payload_json FROM provider_catalog_snapshots WHERE catalog_snapshot_id = ?1",
                [&snapshot.catalog_snapshot_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        if let Some((digest, stored_payload)) = existing {
            if digest != snapshot.catalog_digest.as_str() || stored_payload != payload {
                return Err(StorageError::ImmutableConflict(
                    "provider catalog snapshot ID already has different provenance or rows"
                        .to_owned(),
                ));
            }
            transaction.commit()?;
            return Ok(snapshot.clone());
        }

        transaction.execute(
            "INSERT INTO provider_catalog_snapshots (catalog_snapshot_id, catalog_digest, provider_id, acp_mode, provider_profile_id, profile_revision, adapter_id, adapter_version, adapter_digest, fetched_at, payload_json, created_at) VALUES (?1, ?2, ?3, 'acp', ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                snapshot.catalog_snapshot_id,
                snapshot.catalog_digest.as_str(),
                snapshot.provider_id,
                snapshot.provider_profile_id,
                to_sql_integer(snapshot.profile_revision)?,
                snapshot.adapter_id,
                snapshot.adapter_version,
                snapshot.adapter_digest.as_str(),
                snapshot.fetched_at,
                payload,
                epoch_millis_string(),
            ],
        )?;
        transaction.commit()?;
        Ok(snapshot.clone())
    }

    pub fn load_provider_catalog_snapshot(
        &self,
        catalog_snapshot_id: &str,
    ) -> Result<Option<ProviderCatalogSnapshot>, StorageError> {
        validate_text("catalog_snapshot_id", catalog_snapshot_id, 128)?;
        let connection = self.connection()?;
        load_provider_catalog_snapshot_from(&connection, catalog_snapshot_id)
    }

    pub fn load_latest_provider_catalog(
        &self,
        profile_id: &str,
        profile_revision: u64,
    ) -> Result<Option<ProviderCatalogSnapshot>, StorageError> {
        validate_text("provider_profile_id", profile_id, 128)?;
        let connection = self.connection()?;
        ensure_selection_profile_revision(&connection, profile_id, profile_revision)?;
        latest_selection_catalog(&connection, profile_id, profile_revision)
    }

    pub fn select_provider_model(
        &self,
        input: &crate::ProviderModelSelectionInput,
    ) -> Result<crate::ProviderModelSelection, StorageError> {
        validate_text("updated_at", &input.updated_at, 64)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        ensure_selection_profile_revision(
            &transaction,
            &input.provider_profile_id,
            input.profile_revision,
        )?;
        let catalog = latest_selection_catalog(
            &transaction,
            &input.provider_profile_id,
            input.profile_revision,
        )?
        .ok_or_else(|| {
            StorageError::ImmutableConflict("a current provider catalog is required".into())
        })?;
        if catalog.catalog_snapshot_id != input.catalog_snapshot_id
            || catalog.catalog_digest != input.catalog_digest
        {
            return Err(StorageError::ImmutableConflict(
                "provider catalog selection is stale".into(),
            ));
        }
        let binding = AcpModelBindingSnapshot::from_catalog_with_mode(
            &catalog,
            &input.model_id,
            input.mode_id.as_deref(),
        )?;
        let current = model_selection_row(&transaction, &input.provider_profile_id)?;
        let revision = next_selection_revision(
            current.as_ref().map(|row| row.selection_revision),
            input.expected_selection_revision,
        )?;
        let selection = crate::ProviderModelSelection {
            binding,
            selection_revision: revision,
            updated_at: input.updated_at.clone(),
        };
        transaction.execute(
            "INSERT INTO provider_model_selections (provider_profile_id, profile_revision, catalog_snapshot_id, selection_revision, payload_json) VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT(provider_profile_id) DO UPDATE SET profile_revision=excluded.profile_revision, catalog_snapshot_id=excluded.catalog_snapshot_id, selection_revision=excluded.selection_revision, payload_json=excluded.payload_json",
            params![input.provider_profile_id, to_sql_integer(input.profile_revision)?, input.catalog_snapshot_id, to_sql_integer(revision)?, serde_json::to_string(&selection)?],
        )?;
        transaction.commit()?;
        Ok(selection)
    }

    pub fn load_provider_model_selection(
        &self,
        profile_id: &str,
    ) -> Result<Option<crate::ProviderModelSelection>, StorageError> {
        let connection = self.connection()?;
        let selection = model_selection_row(&connection, profile_id)?;
        if let Some(selection) = &selection {
            validate_model_selection(&connection, selection)?;
        }
        Ok(selection)
    }

    pub fn provider_model_selection_revision(
        &self,
        profile_id: &str,
    ) -> Result<Option<u64>, StorageError> {
        let connection = self.connection()?;
        Ok(model_selection_row(&connection, profile_id)?.map(|row| row.selection_revision))
    }

    pub fn select_core_model(
        &self,
        input: &crate::CoreModelSelectionInput,
    ) -> Result<crate::CoreModelSelection, StorageError> {
        validate_text("updated_at", &input.updated_at, 64)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let model =
            model_selection_row(&transaction, &input.provider_profile_id)?.ok_or_else(|| {
                StorageError::ImmutableConflict(
                    "an explicit saved model selection is required".into(),
                )
            })?;
        validate_model_selection(&transaction, &model)?;
        if model.binding.profile_revision != input.profile_revision
            || model.selection_revision != input.model_selection_revision
        {
            return Err(StorageError::ImmutableConflict(
                "core model selection reference is stale".into(),
            ));
        }
        let current = core_selection_row(&transaction, input.core_id)?;
        let revision = next_selection_revision(
            current.as_ref().map(|row| row.selection_revision),
            input.expected_selection_revision,
        )?;
        let selection = crate::CoreModelSelection {
            core_id: input.core_id,
            provider_profile_id: input.provider_profile_id.clone(),
            profile_revision: input.profile_revision,
            model_selection_revision: input.model_selection_revision,
            selection_revision: revision,
            updated_at: input.updated_at.clone(),
        };
        transaction.execute(
            "INSERT INTO core_model_selections (core_id, provider_profile_id, profile_revision, model_selection_revision, selection_revision, payload_json) VALUES (?1, ?2, ?3, ?4, ?5, ?6) ON CONFLICT(core_id) DO UPDATE SET provider_profile_id=excluded.provider_profile_id, profile_revision=excluded.profile_revision, model_selection_revision=excluded.model_selection_revision, selection_revision=excluded.selection_revision, payload_json=excluded.payload_json",
            params![input.core_id.wire_name(), input.provider_profile_id, to_sql_integer(input.profile_revision)?, to_sql_integer(input.model_selection_revision)?, to_sql_integer(revision)?, serde_json::to_string(&selection)?],
        )?;
        transaction.commit()?;
        Ok(selection)
    }

    pub fn core_model_selection_revision(
        &self,
        core_id: CoreId,
    ) -> Result<Option<u64>, StorageError> {
        let connection = self.connection()?;
        Ok(core_selection_row(&connection, core_id)?.map(|row| row.selection_revision))
    }

    pub fn load_core_model_selection(
        &self,
        core_id: CoreId,
    ) -> Result<Option<crate::CoreModelSelection>, StorageError> {
        let connection = self.connection()?;
        let selection = core_selection_row(&connection, core_id)?;
        if let Some(core) = &selection {
            let model =
                model_selection_row(&connection, &core.provider_profile_id)?.ok_or_else(|| {
                    StorageError::ImmutableConflict("saved core model is unavailable".into())
                })?;
            validate_model_selection(&connection, &model)?;
            if model.binding.profile_revision != core.profile_revision
                || model.selection_revision != core.model_selection_revision
            {
                return Err(StorageError::ImmutableConflict(
                    "saved core model reference is stale".into(),
                ));
            }
        }
        Ok(selection)
    }

    pub fn existing_live_run_receipt(
        &self,
        request: &LiveRunAdmissionRequest,
    ) -> Result<Option<LiveRunReceipt>, StorageError> {
        validate_text("command_id", &request.command_id, 128)?;
        validate_text("idempotency_key", &request.idempotency_key, 256)?;
        validate_live_question(&request.question)?;
        let connection = self.connection()?;
        find_existing_live_run_receipt(&connection, request)
    }

    pub fn admit_live_run(
        &self,
        request: &LiveRunAdmissionRequest,
    ) -> Result<LiveRunAdmissionOutcome, StorageError> {
        validate_text("command_id", &request.command_id, 128)?;
        validate_text("idempotency_key", &request.idempotency_key, 256)?;
        validate_live_question(&request.question)?;
        let payload_digest = live_run_request_digest(&request.question, &request.model_binding)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;

        reject_reserved_admission_identity(
            &transaction,
            &request.command_id,
            &request.idempotency_key,
        )?;
        if let Some(receipt) = find_existing_live_run_receipt(&transaction, request)? {
            transaction.rollback()?;
            return Ok(LiveRunAdmissionOutcome::Accepted {
                receipt,
                duplicate: true,
            });
        }

        let catalog = load_provider_catalog_snapshot_from(
            &transaction,
            &request.model_binding.catalog_snapshot_id,
        )?
        .ok_or_else(|| {
            StorageError::Corrupt("model binding references a missing catalog snapshot".to_owned())
        })?;
        request.model_binding.validate_for_execution(&catalog)?;
        ensure_catalog_profile_is_current(&transaction, &catalog)?;

        let admitted_count = live_run_admitted_count(&transaction)?;
        if admitted_count >= i64::from(LIVE_RUN_QUEUE_CAPACITY) {
            transaction.rollback()?;
            return Ok(LiveRunAdmissionOutcome::QueueFull {
                capacity: LIVE_RUN_QUEUE_CAPACITY,
                admitted_count: u8::try_from(admitted_count).unwrap_or(LIVE_RUN_QUEUE_CAPACITY),
            });
        }

        let run_id = Uuid::new_v4().simple().to_string();
        let accepted_at = epoch_millis_string();
        let binding_json = serde_json::to_string(&request.model_binding)?;
        transaction.execute(
            "INSERT INTO live_runs (run_id, schema_version, question_text, revision, model_binding_json, catalog_snapshot_id, model_binding_digest, result_digest, result_byte_length, failure_json, created_at, updated_at) VALUES (?1, 1, ?2, 0, ?3, ?4, ?5, NULL, NULL, NULL, ?6, ?6)",
            params![
                run_id,
                request.question,
                binding_json,
                request.model_binding.catalog_snapshot_id,
                request.model_binding.binding_digest.as_str(),
                accepted_at,
            ],
        )?;
        transaction.execute(
            "INSERT INTO live_run_outbox (run_id, state, claim_generation, claim_owner, created_at, updated_at) VALUES (?1, 'queued', 0, NULL, ?2, ?2)",
            params![run_id, accepted_at],
        )?;
        let admission_sequence = u64::try_from(transaction.last_insert_rowid()).map_err(|_| {
            StorageError::Corrupt("live run admission sequence is invalid".to_owned())
        })?;
        let event_sequence = insert_live_run_event(
            &transaction,
            &self.identity,
            LiveRunEventInput {
                run_id: &run_id,
                revision: 0,
                claim_generation: 0,
                kind: LiveRunEventKind::StatusChanged,
                status: Some(LiveRunStatus::Queued),
                text_delta: None,
                failure: None,
                created_at: &accepted_at,
            },
        )?;
        let queue = load_live_run_queue_projection(
            &transaction,
            admission_sequence,
            LiveRunStatus::Queued,
        )?;
        let event_cursor = live_run_event_cursor(
            &self.identity,
            &run_id,
            event_sequence,
            event_sequence,
            true,
        );
        let receipt = LiveRunReceipt {
            command_id: request.command_id.clone(),
            run_id: run_id.clone(),
            accepted_revision: 0,
            queue,
            event_cursor,
        };
        let receipt_json = serde_json::to_string(&receipt)?;
        transaction.execute(
            "INSERT INTO live_run_receipts (idempotency_key, command_id, run_id, payload_digest, accepted_revision, admission_sequence, accepted_at, receipt_json) VALUES (?1, ?2, ?3, ?4, 0, ?5, ?6, ?7)",
            params![
                request.idempotency_key,
                request.command_id,
                run_id,
                payload_digest.as_str(),
                to_sql_integer(admission_sequence)?,
                accepted_at,
                receipt_json,
            ],
        )?;
        transaction.commit()?;
        Ok(LiveRunAdmissionOutcome::Accepted {
            receipt,
            duplicate: false,
        })
    }

    pub fn get_live_run_snapshot(
        &self,
        run_id: &str,
        after_sequence: u64,
    ) -> Result<LiveRunSnapshot, StorageError> {
        validate_text("run_id", run_id, 128)?;
        let connection = self.connection()?;
        load_live_run_snapshot_from(
            &connection,
            &self.identity,
            &self.objects_root,
            run_id,
            after_sequence,
        )
    }

    pub fn begin_live_run_cancel(
        &self,
        command_id: &str,
        idempotency_key: &str,
        run_id: &str,
        expected_revision: u64,
        at: &str,
    ) -> Result<LiveRunCancellationTransition, StorageError> {
        self.begin_run_cancel(
            command_id,
            idempotency_key,
            run_id,
            Some(expected_revision),
            at,
        )
    }

    pub fn begin_deliberation_cancel(
        &self,
        run_id: &str,
        at: &str,
    ) -> Result<LiveRunCancellationTransition, StorageError> {
        let command_id = format!(
            "deliberation-cancel:{}",
            Digest::from_bytes(run_id.as_bytes())
        );
        self.begin_run_cancel(&command_id, &command_id, run_id, None, at)
    }

    fn begin_run_cancel(
        &self,
        command_id: &str,
        idempotency_key: &str,
        run_id: &str,
        expected_revision: Option<u64>,
        at: &str,
    ) -> Result<LiveRunCancellationTransition, StorageError> {
        validate_text("command_id", command_id, 128)?;
        validate_text("idempotency_key", idempotency_key, 256)?;
        validate_text("run_id", run_id, 128)?;
        validate_text("created_at", at, 64)?;
        let payload_digest = match expected_revision {
            Some(revision) => Digest::from_bytes(&serde_json::to_vec(&(run_id, revision))?),
            None => Digest::from_bytes(&serde_json::to_vec(&("deliberation_cancel", run_id))?),
        }
        .to_string();
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        reject_reserved_admission_identity(&transaction, command_id, idempotency_key)?;
        reject_deleted_live_command(
            &transaction,
            "live_cancel",
            command_id,
            idempotency_key,
            &payload_digest,
            Some(run_id),
        )?;
        if expected_revision.is_none()
            && !transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM runs WHERE run_id=?1 AND deleted_at IS NULL)",
                [run_id],
                |row| row.get::<_, bool>(0),
            )?
        {
            return Err(StorageError::RunNotFound(run_id.to_owned()));
        }

        let prior: Option<LiveRunCancelCommandRow> = transaction
            .query_row(
                "SELECT command_id, run_id, payload_digest, accepted_status, origin_status, accepted_revision, claim_generation, provider_outcome, outcome_revision, event_sequence FROM live_run_cancellation_receipts WHERE idempotency_key = ?1",
                [idempotency_key],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                        row.get(8)?,
                        row.get(9)?,
                    ))
                },
            )
            .optional()?;
        if let Some((
            prior_command_id,
            prior_run_id,
            prior_digest,
            accepted_status,
            origin_status,
            accepted_revision,
            claim_generation,
            provider_outcome,
            outcome_revision,
            event_sequence,
        )) = prior
        {
            if prior_command_id != command_id
                || prior_run_id != run_id
                || prior_digest != payload_digest
            {
                return Err(StorageError::IdempotencyConflict);
            }
            let duplicate_command: Option<String> = transaction
                .query_row(
                    "SELECT idempotency_key FROM live_run_cancellation_receipts WHERE command_id = ?1",
                    [command_id],
                    |row| row.get(0),
                )
                .optional()?;
            if duplicate_command.as_deref() != Some(idempotency_key) {
                return Err(StorageError::IdempotencyConflict);
            }
            let current_status: String = transaction.query_row(
                "SELECT state FROM live_run_outbox WHERE run_id = ?1",
                [run_id],
                |row| row.get(0),
            )?;
            let accepted_revision = u64::try_from(accepted_revision).map_err(|_| {
                StorageError::Corrupt("live run cancellation revision is negative".to_owned())
            })?;
            let outcome_revision = u64::try_from(outcome_revision).map_err(|_| {
                StorageError::Corrupt(
                    "live run cancellation outcome revision is negative".to_owned(),
                )
            })?;
            let event_sequence = u64::try_from(event_sequence).map_err(|_| {
                StorageError::Corrupt("live run cancellation event sequence is invalid".to_owned())
            })?;
            let claim_generation = u64::try_from(claim_generation).map_err(|_| {
                StorageError::Corrupt("live run cancellation generation is negative".to_owned())
            })?;
            let _accepted_status = parse_live_run_status(&accepted_status)?;
            let origin_status = parse_live_run_status(&origin_status)?;
            let current_status = parse_live_run_status(&current_status)?;
            let _provider_outcome = parse_live_run_provider_outcome(&provider_outcome)?;
            let change = live_run_change(&self.identity, run_id, outcome_revision, event_sequence);
            transaction.commit()?;
            return Ok((
                current_status,
                origin_status,
                accepted_revision,
                claim_generation,
                change,
                true,
                prior_command_id,
            ));
        }

        let command_conflict: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM live_run_cancellation_receipts WHERE command_id = ?1) OR EXISTS(SELECT 1 FROM live_run_receipts WHERE command_id = ?1 OR idempotency_key = ?2)",
            params![command_id, idempotency_key],
            |row| row.get(0),
        )?;
        if command_conflict {
            return Err(StorageError::IdempotencyConflict);
        }

        let row: Option<(String, i64, i64, i64)> = transaction
            .query_row(
                "SELECT q.state, q.claim_generation, r.revision, q.admission_sequence FROM live_run_outbox q JOIN live_runs r ON r.run_id = q.run_id WHERE q.run_id = ?1",
                [run_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()?;
        let Some((state, raw_generation, raw_revision, _admission_sequence)) = row else {
            return Err(StorageError::RunNotFound(run_id.to_owned()));
        };
        let current_status = parse_live_run_status(&state)?;
        let revision = u64::try_from(raw_revision)
            .map_err(|_| StorageError::Corrupt("live run revision is negative".to_owned()))?;
        let mut aggregate = load_optional_deliberation(&transaction, run_id)?;
        if expected_revision.is_none() && aggregate.is_none() {
            return Err(StorageError::RunNotFound(run_id.to_owned()));
        }
        if let Some(aggregate) = aggregate
            .as_ref()
            .filter(|value| value.run().status.is_terminal())
        {
            let terminal_status = match aggregate.run().status {
                RunStatus::Completed { .. } => LiveRunStatus::Completed,
                RunStatus::Cancelled => LiveRunStatus::Cancelled,
                RunStatus::Failed { .. } => LiveRunStatus::Failed,
                _ => unreachable!(),
            };
            let mut terminal_revision = revision;
            let mut terminal_generation =
                u64::try_from(raw_generation).map_err(|_| StorageError::DispatchFenced)?;
            let mut event_sequence: u64 = transaction.query_row(
                "SELECT COALESCE(MAX(sequence),0) FROM live_run_events WHERE run_id=?1",
                [run_id],
                |row| row.get(0),
            )?;
            if current_status != terminal_status && terminal_status == LiveRunStatus::Completed {
                let change = self.finalize_completed_deliberation(&transaction, run_id, at)?;
                terminal_revision = change.revision;
                terminal_generation = terminal_generation
                    .checked_add(1)
                    .ok_or(StorageError::DispatchFenced)?;
                event_sequence = change.event_cursor.high_water_sequence;
            } else if current_status != terminal_status {
                terminal_revision = revision
                    .checked_add(1)
                    .ok_or(StorageError::DispatchFenced)?;
                terminal_generation = terminal_generation
                    .checked_add(1)
                    .ok_or(StorageError::DispatchFenced)?;
                transaction.execute("UPDATE live_run_outbox SET state=?2,claim_generation=?3,claim_owner=NULL,updated_at=?4 WHERE run_id=?1",params![run_id,terminal_status.as_str(),to_sql_integer(terminal_generation)?,at])?;
                transaction.execute(
                    "UPDATE live_runs SET revision=?2,updated_at=?3 WHERE run_id=?1",
                    params![run_id, to_sql_integer(terminal_revision)?, at],
                )?;
                event_sequence = insert_live_run_event(
                    &transaction,
                    &self.identity,
                    LiveRunEventInput {
                        run_id,
                        revision: terminal_revision,
                        claim_generation: terminal_generation,
                        kind: LiveRunEventKind::StatusChanged,
                        status: Some(terminal_status),
                        text_delta: None,
                        failure: None,
                        created_at: at,
                    },
                )?;
                settle_cancellation_reservations(
                    &transaction,
                    &self.identity,
                    run_id,
                    Some(true),
                    at,
                )?;
            }
            transaction.commit()?;
            return Ok((
                terminal_status,
                current_status,
                terminal_revision,
                terminal_generation,
                live_run_change(&self.identity, run_id, terminal_revision, event_sequence),
                true,
                command_id.to_owned(),
            ));
        }
        if let Some(expected) = expected_revision.filter(|expected| *expected != revision) {
            return Err(StorageError::RevisionConflict {
                expected,
                actual: revision,
            });
        }
        let generation = u64::try_from(raw_generation)
            .map_err(|_| StorageError::Corrupt("live run generation is negative".to_owned()))?;
        if current_status == LiveRunStatus::Cancelling {
            let pending: Option<(String, String, String, i64, i64, i64, i64)> = transaction
                .query_row(
                    "SELECT command_id, origin_status, accepted_status, accepted_revision, claim_generation, outcome_revision, event_sequence FROM live_run_cancellation_receipts WHERE run_id = ?1 AND claim_generation = ?2 AND provider_outcome = 'pending' ORDER BY accepted_revision, rowid LIMIT 1",
                    params![run_id, to_sql_integer(generation)?],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                            row.get(5)?,
                            row.get(6)?,
                        ))
                    },
                )
                .optional()?;
            let Some((
                prior_command_id,
                origin_status,
                accepted_status,
                accepted_revision,
                claim_generation,
                outcome_revision,
                event_sequence,
            )) = pending
            else {
                return Err(StorageError::Corrupt(
                    "cancelling live run has no pending cancellation receipt".to_owned(),
                ));
            };
            let accepted_revision = u64::try_from(accepted_revision).map_err(|_| {
                StorageError::Corrupt("live run cancellation revision is negative".to_owned())
            })?;
            let claim_generation = u64::try_from(claim_generation).map_err(|_| {
                StorageError::Corrupt("live run cancellation generation is negative".to_owned())
            })?;
            let outcome_revision = u64::try_from(outcome_revision).map_err(|_| {
                StorageError::Corrupt(
                    "live run cancellation outcome revision is negative".to_owned(),
                )
            })?;
            let event_sequence = u64::try_from(event_sequence).map_err(|_| {
                StorageError::Corrupt("live run cancellation event sequence is invalid".to_owned())
            })?;
            let origin_status = parse_live_run_status(&origin_status)?;
            let accepted_status = parse_live_run_status(&accepted_status)?;
            if accepted_status != LiveRunStatus::Cancelling || claim_generation != generation {
                return Err(StorageError::Corrupt(
                    "pending cancellation receipt does not match the active generation".to_owned(),
                ));
            }
            let change = live_run_change(&self.identity, run_id, outcome_revision, event_sequence);
            transaction.commit()?;
            return Ok((
                accepted_status,
                origin_status,
                accepted_revision,
                claim_generation,
                change,
                true,
                prior_command_id,
            ));
        }
        let origin_status = current_status;
        let mut local_stop = false;
        if let Some(aggregate) = aggregate.as_mut() {
            let previous_revision = aggregate.run().revision;
            let previous_status = aggregate.run().status.clone();
            let external_possible = cancellation_external_possible(&transaction, run_id)?
                || aggregate.persistence_state().external_effect_unknown;
            if matches!(
                previous_status,
                RunStatus::Preparing | RunStatus::AwaitingConfirmation
            ) && external_possible
            {
                return Err(StorageError::Integrity(
                    "unconfirmed run has possible external dispatch".to_owned(),
                ));
            }
            aggregate.request_cancel(previous_revision, at.to_owned())?;
            local_stop = !external_possible;
            if local_stop && origin_status != LiveRunStatus::Claimed {
                aggregate.confirm_cancelled(
                    true,
                    "no external dispatch was started".to_owned(),
                    at.to_owned(),
                )?;
            }
            persist_aggregate_transition(
                &transaction,
                &self.identity,
                aggregate,
                previous_revision,
                &previous_status,
            )?;
            settle_cancellation_reservations(
                &transaction,
                &self.identity,
                run_id,
                if local_stop { Some(true) } else { None },
                at,
            )?;
        }
        let (mut next_status, provider_outcome) =
            if local_stop && origin_status == LiveRunStatus::Claimed {
                (LiveRunStatus::Cancelling, LiveRunProviderOutcome::Pending)
            } else if local_stop {
                (LiveRunStatus::Cancelled, LiveRunProviderOutcome::NotStarted)
            } else {
                match origin_status {
                    LiveRunStatus::Queued | LiveRunStatus::Paused => {
                        (LiveRunStatus::Cancelled, LiveRunProviderOutcome::NotStarted)
                    }
                    LiveRunStatus::Claimed
                    | LiveRunStatus::SessionCreationIntent
                    | LiveRunStatus::Running
                    | LiveRunStatus::Cancelling => {
                        (LiveRunStatus::Cancelling, LiveRunProviderOutcome::Pending)
                    }
                    status => {
                        return Err(StorageError::DispatchStateConflict {
                            expected:
                                "queued, claimed, session creation intent, running, or cancelling"
                                    .to_owned(),
                            actual: status.as_str().to_owned(),
                        });
                    }
                }
            };
        let claim_generation = if origin_status == LiveRunStatus::Cancelling {
            generation
        } else {
            generation.checked_add(1).ok_or_else(|| {
                StorageError::Integrity("live run claim generation overflow".to_owned())
            })?
        };
        let accepted_revision = revision
            .checked_add(1)
            .ok_or_else(|| StorageError::Integrity("live run revision overflow".to_owned()))?;
        let updated_outbox = transaction.execute(
            "UPDATE live_run_outbox SET state = ?1, claim_generation = ?2, claim_owner = NULL, updated_at = ?3 WHERE run_id = ?4 AND state = ?5 AND claim_generation = ?6",
            params![
                next_status.as_str(),
                to_sql_integer(claim_generation)?,
                at,
                run_id,
                origin_status.as_str(),
                to_sql_integer(generation)?,
            ],
        )?;
        if updated_outbox != 1 {
            return Err(StorageError::DispatchFenced);
        }
        let updated_run = transaction.execute(
            "UPDATE live_runs SET revision = ?1, failure_json = NULL, updated_at = ?2 WHERE run_id = ?3 AND revision = ?4",
            params![
                to_sql_integer(accepted_revision)?,
                at,
                run_id,
                to_sql_integer(revision)?,
            ],
        )?;
        if updated_run != 1 {
            return Err(StorageError::DispatchFenced);
        }
        let event_sequence = insert_live_run_event(
            &transaction,
            &self.identity,
            LiveRunEventInput {
                run_id,
                revision: accepted_revision,
                claim_generation,
                kind: LiveRunEventKind::StatusChanged,
                status: Some(next_status),
                text_delta: None,
                failure: None,
                created_at: at,
            },
        )?;
        if event_sequence == 0 {
            return Err(StorageError::Corrupt(
                "live run has no durable event sequence".to_owned(),
            ));
        }
        transaction.execute(
            "INSERT INTO live_run_cancellation_receipts (idempotency_key, command_id, run_id, payload_digest, origin_status, accepted_status, accepted_revision, claim_generation, provider_outcome, outcome_revision, event_sequence, accepted_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                idempotency_key,
                command_id,
                run_id,
                payload_digest,
                origin_status.as_str(),
                next_status.as_str(),
                to_sql_integer(accepted_revision)?,
                to_sql_integer(claim_generation)?,
                provider_outcome.as_str(),
                to_sql_integer(accepted_revision)?,
                to_sql_integer(event_sequence)?,
                at,
            ],
        )?;
        let change = if local_stop && origin_status == LiveRunStatus::Claimed {
            next_status = LiveRunStatus::Cancelled;
            self.settle_live_run_cancellation(
                &transaction,
                run_id,
                claim_generation,
                LiveCancellationOutcome {
                    next_status,
                    provider_outcome: LiveRunProviderOutcome::NotStarted,
                    failure: None,
                },
                at,
            )?
        } else {
            live_run_change(&self.identity, run_id, accepted_revision, event_sequence)
        };
        transaction.commit()?;
        Ok((
            next_status,
            origin_status,
            accepted_revision,
            claim_generation,
            change,
            false,
            command_id.to_owned(),
        ))
    }

    fn finalize_completed_deliberation(
        &self,
        transaction: &Transaction<'_>,
        run_id: &str,
        at: &str,
    ) -> Result<LiveRunChange, StorageError> {
        let aggregate = RunAggregate::restore(load_persistence_state(transaction, run_id)?)?;
        if !matches!(aggregate.run().status, RunStatus::Completed { .. }) {
            return Err(StorageError::Integrity(
                "live result finalization requires durable completed deliberation".to_owned(),
            ));
        }
        let snapshot = aggregate.snapshot(run_event_high_water(transaction, run_id)?);
        if verify_stored_decision_dossier(transaction, &snapshot)?.is_none() {
            return Err(StorageError::Corrupt(
                "completed deliberation has no verified decision dossier".to_owned(),
            ));
        }
        load_live_dispatches_from(transaction, &snapshot)?;
        let (state,raw_generation,raw_revision):(String,i64,i64) = transaction.query_row(
            "SELECT q.state,q.claim_generation,r.revision FROM live_run_outbox q JOIN live_runs r USING(run_id) WHERE q.run_id=?1",
            [run_id], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)),
        )?;
        if parse_live_run_status(&state)? != LiveRunStatus::Running {
            return Err(StorageError::DispatchFenced);
        }
        let generation = u64::try_from(raw_generation)
            .map_err(|_| StorageError::DispatchFenced)?
            .checked_add(1)
            .ok_or(StorageError::DispatchFenced)?;
        let revision = u64::try_from(raw_revision)
            .map_err(|_| StorageError::DispatchFenced)?
            .checked_add(1)
            .ok_or(StorageError::DispatchFenced)?;
        let result = canonical_completed_deliberation_result(&snapshot)?;
        let content_ref = self.publish_source_object(&serde_json::to_vec(&result)?)?;
        transaction.execute("INSERT INTO content_objects(digest,byte_length,created_at) VALUES(?1,?2,?3) ON CONFLICT(digest) DO NOTHING",params![content_ref.digest.as_str(),to_sql_integer(content_ref.byte_length)?,at])?;
        let recorded_length: i64 = transaction.query_row(
            "SELECT byte_length FROM content_objects WHERE digest=?1",
            [content_ref.digest.as_str()],
            |row| row.get(0),
        )?;
        if recorded_length != to_sql_integer(content_ref.byte_length)? {
            return Err(StorageError::Integrity(
                "completed result object length mismatch".into(),
            ));
        }
        let changed=transaction.execute("UPDATE live_runs SET revision=?2,result_digest=?3,result_byte_length=?4,failure_json=NULL,updated_at=?5 WHERE run_id=?1 AND revision=?6",params![run_id,to_sql_integer(revision)?,content_ref.digest.as_str(),to_sql_integer(content_ref.byte_length)?,at,raw_revision])?;
        let fenced=transaction.execute("UPDATE live_run_outbox SET state='completed',claim_generation=?2,claim_owner=NULL,updated_at=?3 WHERE run_id=?1 AND state='running' AND claim_generation=?4",params![run_id,to_sql_integer(generation)?,at,raw_generation])?;
        if changed != 1 || fenced != 1 {
            return Err(StorageError::DispatchFenced);
        }
        settle_cancellation_reservations(transaction, &self.identity, run_id, Some(true), at)?;
        let sequence = insert_live_run_event(
            transaction,
            &self.identity,
            LiveRunEventInput {
                run_id,
                revision,
                claim_generation: generation,
                kind: LiveRunEventKind::StatusChanged,
                status: Some(LiveRunStatus::Completed),
                text_delta: None,
                failure: None,
                created_at: at,
            },
        )?;
        Ok(live_run_change(&self.identity, run_id, revision, sequence))
    }

    pub fn finish_live_run_cancel(
        &self,
        run_id: &str,
        claim_generation: u64,
        provider_outcome: LiveRunProviderOutcome,
        at: &str,
    ) -> Result<LiveRunChange, StorageError> {
        self.transition_live_run_cancellation(
            run_id,
            claim_generation,
            LiveRunStatus::Cancelled,
            provider_outcome,
            None,
            at,
        )
    }

    pub fn mark_live_run_cancel_unknown(
        &self,
        run_id: &str,
        claim_generation: u64,
        at: &str,
    ) -> Result<LiveRunChange, StorageError> {
        self.transition_live_run_cancellation(
            run_id,
            claim_generation,
            LiveRunStatus::Unknown,
            LiveRunProviderOutcome::Unknown,
            Some(LiveRunFailure {
                profile_binding: None,
                code: "cancellation_unconfirmed".to_owned(),
                detail: "The provider did not confirm that the request had stopped.".to_owned(),
                external_effect_unknown: true,
            }),
            at,
        )
    }

    fn transition_live_run_cancellation(
        &self,
        run_id: &str,
        claim_generation: u64,
        next_status: LiveRunStatus,
        provider_outcome: LiveRunProviderOutcome,
        failure: Option<LiveRunFailure>,
        at: &str,
    ) -> Result<LiveRunChange, StorageError> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let change = self.settle_live_run_cancellation(
            &transaction,
            run_id,
            claim_generation,
            LiveCancellationOutcome {
                next_status,
                provider_outcome,
                failure,
            },
            at,
        )?;
        transaction.commit()?;
        Ok(change)
    }

    fn settle_live_run_cancellation(
        &self,
        transaction: &Transaction<'_>,
        run_id: &str,
        claim_generation: u64,
        outcome: LiveCancellationOutcome,
        at: &str,
    ) -> Result<LiveRunChange, StorageError> {
        let LiveCancellationOutcome {
            next_status,
            provider_outcome,
            failure,
        } = outcome;
        validate_text("run_id", run_id, 128)?;
        validate_text("created_at", at, 64)?;
        let outcome_matches_status = match next_status {
            LiveRunStatus::Cancelled => matches!(
                provider_outcome,
                LiveRunProviderOutcome::NotStarted | LiveRunProviderOutcome::Confirmed
            ),
            LiveRunStatus::Unknown => provider_outcome == LiveRunProviderOutcome::Unknown,
            _ => false,
        };
        if !outcome_matches_status {
            return Err(StorageError::Integrity(
                "live run cancellation outcome does not match its terminal state".to_owned(),
            ));
        }
        let state: Option<(String, i64, i64)> = transaction
            .query_row(
                "SELECT q.state, q.claim_generation, r.revision FROM live_run_outbox q JOIN live_runs r ON r.run_id = q.run_id WHERE q.run_id = ?1",
                [run_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        let Some((state, raw_generation, raw_revision)) = state else {
            return Err(StorageError::RunNotFound(run_id.to_owned()));
        };
        let current_status = parse_live_run_status(&state)?;
        let current_generation = u64::try_from(raw_generation)
            .map_err(|_| StorageError::Corrupt("live run generation is negative".to_owned()))?;
        if current_status != LiveRunStatus::Cancelling || current_generation != claim_generation {
            return Err(StorageError::DispatchFenced);
        }
        let cancellation_origins: (bool, bool, bool, bool) = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM live_run_cancellation_receipts WHERE run_id = ?1 AND claim_generation = ?2 AND provider_outcome = 'pending'), EXISTS(SELECT 1 FROM live_run_cancellation_receipts WHERE run_id = ?1 AND claim_generation = ?2 AND provider_outcome = 'pending' AND origin_status = 'claimed'), EXISTS(SELECT 1 FROM live_run_cancellation_receipts WHERE run_id = ?1 AND claim_generation = ?2 AND provider_outcome = 'pending' AND origin_status = 'session_creation_intent'), EXISTS(SELECT 1 FROM live_run_cancellation_receipts WHERE run_id = ?1 AND claim_generation = ?2 AND provider_outcome = 'pending' AND origin_status IN ('running', 'cancelling'))",
            params![run_id, to_sql_integer(claim_generation)?],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )?;
        let outcome_matches_origin = match provider_outcome {
            LiveRunProviderOutcome::NotStarted => {
                cancellation_origins.0
                    && (cancellation_origins.1 || cancellation_origins.2)
                    && !cancellation_origins.3
            }
            LiveRunProviderOutcome::Confirmed => {
                cancellation_origins.0
                    && (cancellation_origins.2 || cancellation_origins.3)
                    && !cancellation_origins.1
            }
            LiveRunProviderOutcome::Unknown => {
                cancellation_origins.0
                    && (cancellation_origins.1 || cancellation_origins.2 || cancellation_origins.3)
            }
            LiveRunProviderOutcome::Pending => false,
        };
        if !outcome_matches_origin {
            return Err(StorageError::Integrity(
                "live run cancellation outcome is incompatible with its request origin".to_owned(),
            ));
        }
        let revision = u64::try_from(raw_revision)
            .map_err(|_| StorageError::Corrupt("live run revision is negative".to_owned()))?;
        let next_revision = revision
            .checked_add(1)
            .ok_or_else(|| StorageError::Integrity("live run revision overflow".to_owned()))?;
        let updated_outbox = transaction.execute(
            "UPDATE live_run_outbox SET state = ?1, claim_owner = NULL, updated_at = ?2 WHERE run_id = ?3 AND state = 'cancelling' AND claim_generation = ?4",
            params![
                next_status.as_str(),
                at,
                run_id,
                to_sql_integer(claim_generation)?,
            ],
        )?;
        if updated_outbox != 1 {
            return Err(StorageError::DispatchFenced);
        }
        let failure_json = failure.as_ref().map(serde_json::to_string).transpose()?;
        let updated_run = transaction.execute(
            "UPDATE live_runs SET revision = ?1, failure_json = ?2, updated_at = ?3 WHERE run_id = ?4 AND revision = ?5",
            params![
                to_sql_integer(next_revision)?,
                failure_json,
                at,
                run_id,
                to_sql_integer(revision)?,
            ],
        )?;
        if updated_run != 1 {
            return Err(StorageError::DispatchFenced);
        }
        let event_sequence = insert_live_run_event(
            transaction,
            &self.identity,
            LiveRunEventInput {
                run_id,
                revision: next_revision,
                claim_generation,
                kind: LiveRunEventKind::StatusChanged,
                status: Some(next_status),
                text_delta: None,
                failure: failure.as_ref(),
                created_at: at,
            },
        )?;
        let updated_receipts = transaction.execute(
            "UPDATE live_run_cancellation_receipts SET provider_outcome = ?1, outcome_revision = ?2, event_sequence = ?3 WHERE run_id = ?4 AND claim_generation = ?5 AND provider_outcome = 'pending'",
            params![
                provider_outcome.as_str(),
                to_sql_integer(next_revision)?,
                to_sql_integer(event_sequence)?,
                run_id,
                to_sql_integer(claim_generation)?,
            ],
        )?;
        if updated_receipts == 0 {
            return Err(StorageError::Corrupt(
                "live run cancellation has no pending durable receipt".to_owned(),
            ));
        }
        settle_deliberation_cancel(
            transaction,
            &self.identity,
            run_id,
            next_status == LiveRunStatus::Cancelled,
            at,
        )?;
        Ok(live_run_change(
            &self.identity,
            run_id,
            next_revision,
            event_sequence,
        ))
    }

    pub fn has_queued_live_runs(&self) -> Result<bool, StorageError> {
        let connection = self.connection()?;
        Ok(connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM live_run_outbox WHERE state = 'queued')",
            [],
            |row| row.get(0),
        )?)
    }

    /// Enumerates shutdown work from one authority-checked snapshot; overflow fails closed.
    pub fn list_live_run_ids_requiring_shutdown_with_authority(
        &self,
        expected: &crate::AdmissionExecutionAuthority,
    ) -> Result<Vec<String>, StorageError> {
        self.list_live_run_ids_requiring_shutdown_inner(expected, 4096)
    }

    fn list_live_run_ids_requiring_shutdown_inner(
        &self,
        expected: &crate::AdmissionExecutionAuthority,
        limit: usize,
    ) -> Result<Vec<String>, StorageError> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
        validate_expected_execution_authority(&transaction, Some(expected))?;
        let ids = {
            let mut statement = transaction.prepare(
                "SELECT run_id FROM live_run_outbox WHERE state IN ('queued', 'claimed', 'session_creation_intent', 'running', 'cancelling', 'unknown') ORDER BY admission_sequence LIMIT ?1",
            )?;
            statement
                .query_map(
                    [i64::try_from(limit + 1).map_err(|_| {
                        StorageError::Corrupt("invalid shutdown enumeration bound".into())
                    })?],
                    |row| row.get::<_, String>(0),
                )?
                .collect::<Result<Vec<_>, _>>()?
        };
        if ids.len() > limit {
            return Err(StorageError::Corrupt(
                "live run shutdown enumeration capacity exceeded".into(),
            ));
        }
        for id in &ids {
            ensure_current_run_lineage(&transaction, id)?;
        }
        transaction.commit()?;
        Ok(ids)
    }

    pub fn claim_next_live_run(
        &self,
        worker_id: &str,
        claimed_at: &str,
    ) -> Result<Option<LiveRunClaim>, StorageError> {
        self.claim_next_live_run_inner(worker_id, claimed_at, None)
    }

    pub fn claim_next_live_run_with_authority(
        &self,
        expected: &crate::AdmissionExecutionAuthority,
        worker_id: &str,
        claimed_at: &str,
    ) -> Result<Option<LiveRunClaim>, StorageError> {
        self.claim_next_live_run_inner(worker_id, claimed_at, Some(expected))
    }

    fn claim_next_live_run_inner(
        &self,
        worker_id: &str,
        claimed_at: &str,
        expected: Option<&crate::AdmissionExecutionAuthority>,
    ) -> Result<Option<LiveRunClaim>, StorageError> {
        validate_text("worker_id", worker_id, 128)?;
        validate_text("claimed_at", claimed_at, 64)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        validate_expected_execution_authority(&transaction, expected)?;
        if !read_admission_execution_authority(&transaction)?.active {
            return Err(StorageError::DispatchFenced);
        }
        let active_count: i64 = transaction.query_row(
            "SELECT count(*) FROM live_run_outbox WHERE state IN ('claimed', 'session_creation_intent', 'running', 'cancelling', 'unknown')",
            [],
            |row| row.get(0),
        )?;
        if active_count >= i64::from(LIVE_RUN_ACTIVE_CAPACITY) {
            transaction.commit()?;
            return Ok(None);
        }

        let candidate: Option<LiveRunClaimRow> = transaction
            .query_row(
                "SELECT q.admission_sequence, r.run_id, q.claim_generation, r.revision, r.question_text, r.catalog_snapshot_id, r.model_binding_digest, r.model_binding_json FROM live_run_outbox q JOIN live_runs r ON r.run_id = q.run_id WHERE q.state = 'queued' ORDER BY q.admission_sequence LIMIT 1",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                    ))
                },
            )
            .optional()?;
        let Some((
            admission_sequence,
            run_id,
            current_claim_generation,
            revision,
            question,
            row_catalog_snapshot_id,
            row_binding_digest,
            binding_json,
        )) = candidate
        else {
            transaction.commit()?;
            return Ok(None);
        };

        ensure_current_run_lineage(&transaction, &run_id)?;
        let blocked_by_unknown: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM live_run_outbox WHERE state IN ('cancelling', 'unknown') AND admission_sequence < ?1)",
            [admission_sequence],
            |row| row.get(0),
        )?;
        if blocked_by_unknown {
            transaction.commit()?;
            return Ok(None);
        }

        let model_binding: AcpModelBindingSnapshot = serde_json::from_str(&binding_json)?;
        if model_binding.catalog_snapshot_id != row_catalog_snapshot_id
            || model_binding.binding_digest.as_str() != row_binding_digest
        {
            return Err(StorageError::Corrupt(
                "queued live run binding does not match its immutable columns".to_owned(),
            ));
        }
        let catalog = load_provider_catalog_snapshot_from(&transaction, &row_catalog_snapshot_id)?
            .ok_or_else(|| {
                StorageError::Corrupt(
                    "queued live run references a missing catalog snapshot".to_owned(),
                )
            })?;
        model_binding.validate_ready(&catalog)?;

        let active_bindings = {
            let mut statement = transaction.prepare(
                "SELECT r.model_binding_json FROM live_run_outbox q JOIN live_runs r ON r.run_id = q.run_id WHERE q.state IN ('claimed', 'session_creation_intent', 'running', 'cancelling', 'unknown')",
            )?;
            let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        for active_binding_json in active_bindings {
            let active_binding: AcpModelBindingSnapshot =
                serde_json::from_str(&active_binding_json)?;
            let active_catalog = load_provider_catalog_snapshot_from(
                &transaction,
                &active_binding.catalog_snapshot_id,
            )?
            .ok_or_else(|| {
                StorageError::Corrupt(
                    "active live run references a missing catalog snapshot".to_owned(),
                )
            })?;
            active_binding.validate(&active_catalog)?;
            if active_binding.provider_profile_id == model_binding.provider_profile_id {
                transaction.commit()?;
                return Ok(None);
            }
        }

        let next_claim_generation = u64::try_from(current_claim_generation)
            .map_err(|_| StorageError::Corrupt("live run claim generation is negative".to_owned()))?
            .checked_add(1)
            .ok_or_else(|| {
                StorageError::Integrity("live run claim generation overflow".to_owned())
            })?;
        let next_revision = u64::try_from(revision)
            .map_err(|_| StorageError::Corrupt("live run revision is negative".to_owned()))?
            .checked_add(1)
            .ok_or_else(|| StorageError::Integrity("live run revision overflow".to_owned()))?;
        let updated = transaction.execute(
            "UPDATE live_run_outbox SET state = 'claimed', claim_generation = ?1, claim_owner = ?2, updated_at = ?3 WHERE run_id = ?4 AND state = 'queued' AND claim_generation = ?5",
            params![
                to_sql_integer(next_claim_generation)?,
                worker_id,
                claimed_at,
                run_id,
                current_claim_generation,
            ],
        )?;
        if updated != 1 {
            return Err(StorageError::DispatchFenced);
        }
        transaction.execute(
            "UPDATE live_runs SET revision = ?1, updated_at = ?2 WHERE run_id = ?3 AND revision = ?4",
            params![to_sql_integer(next_revision)?, claimed_at, run_id, revision],
        )?;
        insert_live_run_event(
            &transaction,
            &self.identity,
            LiveRunEventInput {
                run_id: &run_id,
                revision: next_revision,
                claim_generation: next_claim_generation,
                kind: LiveRunEventKind::StatusChanged,
                status: Some(LiveRunStatus::Claimed),
                text_delta: None,
                failure: None,
                created_at: claimed_at,
            },
        )?;
        validate_expected_execution_authority(&transaction, expected)?;
        transaction.commit()?;
        Ok(Some(LiveRunClaim {
            run_id,
            admission_sequence: u64::try_from(admission_sequence).map_err(|_| {
                StorageError::Corrupt("live run admission sequence is invalid".to_owned())
            })?,
            claim_generation: next_claim_generation,
            claim_owner: worker_id.to_owned(),
            question,
            model_binding,
        }))
    }

    pub fn mark_live_run_session_creation_intent(
        &self,
        claim: &LiveRunClaim,
        at: &str,
    ) -> Result<LiveRunChange, StorageError> {
        self.transition_live_run(
            claim,
            LiveRunStatus::Claimed,
            LiveRunStatus::SessionCreationIntent,
            None,
            at,
        )
    }

    pub fn mark_live_run_running(
        &self,
        claim: &LiveRunClaim,
        at: &str,
    ) -> Result<LiveRunChange, StorageError> {
        self.transition_live_run(
            claim,
            LiveRunStatus::SessionCreationIntent,
            LiveRunStatus::Running,
            None,
            at,
        )
    }

    pub fn append_live_run_text_delta(
        &self,
        claim: &LiveRunClaim,
        text_delta: &str,
        at: &str,
    ) -> Result<LiveRunChange, StorageError> {
        validate_live_text("text_delta", text_delta, MAX_OBJECT_BYTES as usize)?;
        validate_text("created_at", at, 64)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = load_live_run_claim_state(&transaction, claim)?;
        ensure_live_run_claim(&current, claim, LiveRunStatus::Running)?;
        let revision = current
            .revision
            .checked_add(1)
            .ok_or_else(|| StorageError::Integrity("live run revision overflow".to_owned()))?;
        transaction.execute(
            "UPDATE live_runs SET revision = ?1, updated_at = ?2 WHERE run_id = ?3 AND revision = ?4",
            params![
                to_sql_integer(revision)?,
                at,
                claim.run_id,
                to_sql_integer(current.revision)?,
            ],
        )?;
        let sequence = insert_live_run_event(
            &transaction,
            &self.identity,
            LiveRunEventInput {
                run_id: &claim.run_id,
                revision,
                claim_generation: claim.claim_generation,
                kind: LiveRunEventKind::TextDelta,
                status: None,
                text_delta: Some(text_delta),
                failure: None,
                created_at: at,
            },
        )?;
        transaction.commit()?;
        Ok(live_run_change(
            &self.identity,
            &claim.run_id,
            revision,
            sequence,
        ))
    }

    pub fn finish_live_run(
        &self,
        claim: &LiveRunClaim,
        result: &LiveProviderResultInput,
        at: &str,
    ) -> Result<LiveRunChange, StorageError> {
        validate_live_text("final_text", &result.final_text, MAX_OBJECT_BYTES as usize)?;
        validate_text("stop_reason", &result.stop_reason, 128)?;
        validate_text("created_at", at, 64)?;
        {
            let connection = self.connection()?;
            let current = load_live_run_claim_state(&connection, claim)?;
            ensure_live_run_claim(&current, claim, LiveRunStatus::Running)?;
        }
        let result_bytes = serde_json::to_vec(result)?;
        let content_ref = self.put_source_object(&result_bytes)?;

        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = load_live_run_claim_state(&transaction, claim)?;
        ensure_live_run_claim(&current, claim, LiveRunStatus::Running)?;
        let revision = current
            .revision
            .checked_add(1)
            .ok_or_else(|| StorageError::Integrity("live run revision overflow".to_owned()))?;
        let updated_outbox = transaction.execute(
            "UPDATE live_run_outbox SET state = 'completed', claim_owner = NULL, updated_at = ?1 WHERE run_id = ?2 AND state = 'running' AND claim_generation = ?3 AND claim_owner = ?4",
            params![
                at,
                claim.run_id,
                to_sql_integer(claim.claim_generation)?,
                claim.claim_owner,
            ],
        )?;
        if updated_outbox != 1 {
            return Err(StorageError::DispatchFenced);
        }
        let updated_run = transaction.execute(
            "UPDATE live_runs SET revision = ?1, result_digest = ?2, result_byte_length = ?3, failure_json = NULL, updated_at = ?4 WHERE run_id = ?5 AND revision = ?6",
            params![
                to_sql_integer(revision)?,
                content_ref.digest.as_str(),
                to_sql_integer(content_ref.byte_length)?,
                at,
                claim.run_id,
                to_sql_integer(current.revision)?,
            ],
        )?;
        if updated_run != 1 {
            return Err(StorageError::DispatchFenced);
        }
        let sequence = insert_live_run_event(
            &transaction,
            &self.identity,
            LiveRunEventInput {
                run_id: &claim.run_id,
                revision,
                claim_generation: claim.claim_generation,
                kind: LiveRunEventKind::StatusChanged,
                status: Some(LiveRunStatus::Completed),
                text_delta: None,
                failure: None,
                created_at: at,
            },
        )?;
        transaction.commit()?;
        Ok(live_run_change(
            &self.identity,
            &claim.run_id,
            revision,
            sequence,
        ))
    }

    pub fn fail_live_run(
        &self,
        claim: &LiveRunClaim,
        failure: &LiveRunFailure,
        security_violation: bool,
        at: &str,
    ) -> Result<LiveRunChange, StorageError> {
        if let Some(binding) = &failure.profile_binding {
            validate_text(
                "authentication_failure_profile_id",
                &binding.provider_profile_id,
                256,
            )?;
            if !magi_domain::is_profile_authentication_failure(&failure.code) {
                return Err(StorageError::Integrity(
                    "profile binding requires an authentication-specific failure".into(),
                ));
            }
        }
        validate_text("failure_code", &failure.code, 128)?;
        validate_live_text("failure_detail", &failure.detail, 4096)?;
        validate_text("created_at", at, 64)?;
        let next = if failure.external_effect_unknown {
            LiveRunStatus::Unknown
        } else {
            LiveRunStatus::Failed
        };
        self.transition_live_run_with_security_event(claim, next, failure, security_violation, at)
    }

    pub fn mark_live_run_unknown(
        &self,
        claim: &LiveRunClaim,
        code: &str,
        detail: &str,
        at: &str,
    ) -> Result<LiveRunChange, StorageError> {
        self.fail_live_run(
            claim,
            &LiveRunFailure {
                profile_binding: None,
                code: code.to_owned(),
                detail: detail.to_owned(),
                external_effect_unknown: true,
            },
            false,
            at,
        )
    }

    fn transition_live_run(
        &self,
        claim: &LiveRunClaim,
        expected: LiveRunStatus,
        next: LiveRunStatus,
        failure: Option<&LiveRunFailure>,
        at: &str,
    ) -> Result<LiveRunChange, StorageError> {
        validate_text("created_at", at, 64)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = load_live_run_claim_state(&transaction, claim)?;
        ensure_live_run_claim(&current, claim, expected)?;
        let revision = current
            .revision
            .checked_add(1)
            .ok_or_else(|| StorageError::Integrity("live run revision overflow".to_owned()))?;
        let owner = if matches!(
            next,
            LiveRunStatus::Claimed | LiveRunStatus::SessionCreationIntent | LiveRunStatus::Running
        ) {
            Some(claim.claim_owner.as_str())
        } else {
            None
        };
        let updated_outbox = transaction.execute(
            "UPDATE live_run_outbox SET state = ?1, claim_owner = ?2, updated_at = ?3 WHERE run_id = ?4 AND state = ?5 AND claim_generation = ?6 AND claim_owner = ?7",
            params![
                next.as_str(),
                owner,
                at,
                claim.run_id,
                expected.as_str(),
                to_sql_integer(claim.claim_generation)?,
                claim.claim_owner,
            ],
        )?;
        if updated_outbox != 1 {
            return Err(StorageError::DispatchFenced);
        }
        let failure_json = failure.map(serde_json::to_string).transpose()?;
        let updated_run = transaction.execute(
            "UPDATE live_runs SET revision = ?1, failure_json = ?2, updated_at = ?3 WHERE run_id = ?4 AND revision = ?5",
            params![
                to_sql_integer(revision)?,
                failure_json,
                at,
                claim.run_id,
                to_sql_integer(current.revision)?,
            ],
        )?;
        if updated_run != 1 {
            return Err(StorageError::DispatchFenced);
        }
        let event_sequence = insert_live_run_event(
            &transaction,
            &self.identity,
            LiveRunEventInput {
                run_id: &claim.run_id,
                revision,
                claim_generation: claim.claim_generation,
                kind: LiveRunEventKind::StatusChanged,
                status: Some(next),
                text_delta: None,
                failure,
                created_at: at,
            },
        )?;
        transaction.commit()?;
        Ok(live_run_change(
            &self.identity,
            &claim.run_id,
            revision,
            event_sequence,
        ))
    }

    fn transition_live_run_with_security_event(
        &self,
        claim: &LiveRunClaim,
        next: LiveRunStatus,
        failure: &LiveRunFailure,
        security_violation: bool,
        at: &str,
    ) -> Result<LiveRunChange, StorageError> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = load_live_run_claim_state(&transaction, claim)?;
        if !matches!(
            current.status,
            LiveRunStatus::Claimed | LiveRunStatus::SessionCreationIntent | LiveRunStatus::Running
        ) {
            return Err(StorageError::DispatchStateConflict {
                expected: "claimed, session_creation_intent, or running".to_owned(),
                actual: current.status.as_str().to_owned(),
            });
        }
        ensure_live_run_claim(&current, claim, current.status)?;
        if security_violation && current.status != LiveRunStatus::Running {
            return Err(StorageError::DispatchStateConflict {
                expected: LiveRunStatus::Running.as_str().to_owned(),
                actual: current.status.as_str().to_owned(),
            });
        }
        if next == LiveRunStatus::Unknown
            && !matches!(
                current.status,
                LiveRunStatus::SessionCreationIntent | LiveRunStatus::Running
            )
        {
            return Err(StorageError::DispatchStateConflict {
                expected: "session_creation_intent or running".to_owned(),
                actual: current.status.as_str().to_owned(),
            });
        }

        let revision = current
            .revision
            .checked_add(1)
            .ok_or_else(|| StorageError::Integrity("live run revision overflow".to_owned()))?;
        if let Some(binding) = &failure.profile_binding {
            if let Some(aggregate) = load_optional_deliberation(&transaction, &claim.run_id)? {
                validate_deliberation_failure_binding(
                    &aggregate.snapshot(run_event_high_water(&transaction, &claim.run_id)?),
                    failure,
                )?;
            } else if binding.provider_profile_id != claim.model_binding.provider_profile_id
                || binding.profile_revision != claim.model_binding.profile_revision
            {
                return Err(StorageError::Integrity(
                    "authentication failure does not match the frozen live profile".into(),
                ));
            }
        }
        let failure_json = serde_json::to_string(failure)?;
        let updated_outbox = transaction.execute(
            "UPDATE live_run_outbox SET state = ?1, claim_owner = NULL, updated_at = ?2 WHERE run_id = ?3 AND state = ?4 AND claim_generation = ?5 AND claim_owner = ?6",
            params![
                next.as_str(),
                at,
                claim.run_id,
                current.status.as_str(),
                to_sql_integer(claim.claim_generation)?,
                claim.claim_owner,
            ],
        )?;
        if updated_outbox != 1 {
            return Err(StorageError::DispatchFenced);
        }
        let updated_run = transaction.execute(
            "UPDATE live_runs SET revision = ?1, failure_json = ?2, updated_at = ?3 WHERE run_id = ?4 AND revision = ?5",
            params![
                to_sql_integer(revision)?,
                failure_json,
                at,
                claim.run_id,
                to_sql_integer(current.revision)?,
            ],
        )?;
        if updated_run != 1 {
            return Err(StorageError::DispatchFenced);
        }
        if security_violation {
            insert_live_run_event(
                &transaction,
                &self.identity,
                LiveRunEventInput {
                    run_id: &claim.run_id,
                    revision,
                    claim_generation: claim.claim_generation,
                    kind: LiveRunEventKind::SecurityViolation,
                    status: None,
                    text_delta: None,
                    failure: None,
                    created_at: at,
                },
            )?;
        }
        let event_sequence = insert_live_run_event(
            &transaction,
            &self.identity,
            LiveRunEventInput {
                run_id: &claim.run_id,
                revision,
                claim_generation: claim.claim_generation,
                kind: LiveRunEventKind::StatusChanged,
                status: Some(next),
                text_delta: None,
                failure: Some(failure),
                created_at: at,
            },
        )?;
        transaction.commit()?;
        Ok(live_run_change(
            &self.identity,
            &claim.run_id,
            revision,
            event_sequence,
        ))
    }

    pub fn list_runs(&self, request: &RunHistoryRequest) -> Result<RunHistoryPage, StorageError> {
        if request.page_size == 0 || request.page_size > 200 {
            return Err(StorageError::Corrupt(
                "run history page size must be between 1 and 200".to_owned(),
            ));
        }
        let filter = &request.filter;
        if let Some(query) = &filter.question_query {
            validate_text("question_query", query, 32_000)?;
        }
        if let Some(value) = &filter.created_after {
            validate_text("created_after", value, 64)?;
        }
        if let Some(value) = &filter.created_before {
            validate_text("created_before", value, 64)?;
        }
        if matches!((&filter.created_after, &filter.created_before), (Some(start), Some(end)) if start >= end)
        {
            return Err(StorageError::Corrupt(
                "run history time range must have created_after before created_before".to_owned(),
            ));
        }
        let filter_digest = Digest::from_bytes(&canonical_json(filter)?);
        let connection = self.connection()?;
        let membership_generation =
            load_store_meta_u64(&connection, "history_membership_generation")?;
        if let Some(cursor) = &request.cursor {
            if cursor.store_id != self.identity.store_id
                || cursor.store_generation != self.identity.generation
                || cursor.membership_generation != membership_generation
            {
                return Err(StorageError::HistoryCursorExpired);
            }
            if cursor.filter_digest != filter_digest {
                return Err(StorageError::HistoryFilterMismatch);
            }
        }

        let mut sql = String::from(
            "SELECT run_id, conversation_id, question_text, run_json, revision, input_digest, created_at, updated_at, status, input_json FROM runs WHERE deleted_at IS NULL",
        );
        let mut values = Vec::<Value>::new();
        if let Some(query) = filter
            .question_query
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            sql.push_str(" AND instr(lower(question_text), lower(?)) > 0");
            values.push(Value::Text(query.to_owned()));
        }
        if !filter.statuses.is_empty() {
            sql.push_str(" AND status IN (");
            for (index, status) in filter.statuses.iter().enumerate() {
                if index > 0 {
                    sql.push(',');
                }
                sql.push('?');
                values.push(Value::Text(status.as_str().to_owned()));
            }
            sql.push(')');
        }
        if let Some(start) = &filter.created_after {
            sql.push_str(" AND created_at >= ?");
            values.push(Value::Text(start.clone()));
        }
        if let Some(end) = &filter.created_before {
            sql.push_str(" AND created_at < ?");
            values.push(Value::Text(end.clone()));
        }
        if let Some(cursor) = &request.cursor {
            sql.push_str(" AND (created_at < ? OR (created_at = ? AND run_id < ?))");
            values.extend([
                Value::Text(cursor.last_created_at.clone()),
                Value::Text(cursor.last_created_at.clone()),
                Value::Text(cursor.last_run_id.clone()),
            ]);
        }
        sql.push_str(" ORDER BY created_at DESC, run_id DESC LIMIT ?");
        values.push(Value::Integer(i64::from(request.page_size) + 1));
        let mut statement = connection.prepare(&sql)?;
        let rows = statement.query_map(rusqlite::params_from_iter(values), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, String>(8)?,
                row.get::<_, String>(9)?,
            ))
        })?;
        let mut items = Vec::new();
        for row in rows {
            let (
                run_id,
                conversation_id,
                question,
                payload,
                revision,
                input_digest,
                created_at,
                updated_at,
                stored_status,
                input_payload,
            ) = row?;
            if revision < 0 {
                return Err(StorageError::Corrupt(
                    "run history contains a negative revision".to_owned(),
                ));
            }
            let run: magi_domain::Run = serde_json::from_str(&payload)?;
            let input: InputSnapshot = serde_json::from_str(&input_payload)?;
            input.validate()?;
            if run.run_id != run_id
                || run.conversation_id != conversation_id
                || run.revision != revision as u64
                || run.input_digest.as_str() != input_digest
                || input.question.prompt != question
                || input.input_digest != run.input_digest
                || status_name(&run.status) != stored_status
                || run.created_at != created_at
                || run.updated_at != updated_at
            {
                return Err(StorageError::Corrupt(
                    "run history columns do not match their frozen run payload".to_owned(),
                ));
            }
            items.push(RunSummary {
                run_id,
                conversation_id,
                question,
                status: run.status,
                revision: revision as u64,
                input_digest: Digest::from_hex(input_digest)?,
                created_at,
                updated_at,
            });
        }
        let has_more = items.len() > usize::from(request.page_size);
        if has_more {
            items.pop();
        }
        let next_cursor = if has_more {
            items.last().map(|last| RunHistoryCursor {
                store_id: self.identity.store_id.clone(),
                store_generation: self.identity.generation,
                membership_generation,
                filter_digest,
                last_created_at: last.created_at.clone(),
                last_run_id: last.run_id.clone(),
            })
        } else {
            None
        };
        Ok(RunHistoryPage { items, next_cursor })
    }

    pub fn commit_run(
        &self,
        command: &CommandEnvelope,
        aggregate: &mut RunAggregate,
        accepted_at: &str,
        initial_conversation_title: Option<&str>,
    ) -> Result<CommitResult, StorageError> {
        let dispatch_batch_key = format!(
            "command:{}",
            Digest::from_bytes(command.command_id.as_bytes())
        );
        self.commit_run_with_dispatches(
            command,
            aggregate,
            accepted_at,
            initial_conversation_title,
            &dispatch_batch_key,
            &[],
        )
    }

    pub fn commit_run_with_dispatches(
        &self,
        command: &CommandEnvelope,
        aggregate: &mut RunAggregate,
        accepted_at: &str,
        initial_conversation_title: Option<&str>,
        dispatch_batch_key: &str,
        dispatches: &[PreparedDispatch],
    ) -> Result<CommitResult, StorageError> {
        command.validate()?;
        validate_text("accepted_at", accepted_at, 64)?;
        validate_text("dispatch_batch_key", dispatch_batch_key, 128)?;
        if dispatch_batch_key.starts_with("legacy-command:")
            || dispatch_batch_key.starts_with("legacy-dispatch:")
        {
            return Err(StorageError::Corrupt(
                "dispatch batch key uses a reserved prefix".to_owned(),
            ));
        }
        let state = aggregate.persistence_state();
        state.run.validate()?;
        state.input.validate()?;
        if command.target_id != state.run.run_id {
            return Err(StorageError::Corrupt(
                "command target must be the aggregate run ID".to_owned(),
            ));
        }
        if aggregate.events().is_empty() {
            return Err(StorageError::Corrupt(
                "a state-changing command must emit at least one durable event".to_owned(),
            ));
        }
        validate_dispatch_batch(dispatches, &state.run, &state.input)?;
        let dispatches_digest = Digest::from_bytes(&canonical_json(&dispatches.to_vec())?);
        let event_drafts = aggregate.events().to_vec();
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;

        if command.command_kind == CommandKind::CreateRun
            && state.input.request_provenance.is_some()
            && state.input.role_set.frozen_core_selections.is_some()
        {
            let binding = admission_request_binding(&admission_intent_from_input(
                &command.command_id,
                &command.idempotency_key,
                &state.input,
            )?)?;
            validate_admission_binding_from(&transaction, &binding)?;
            validate_admission_command_namespace(&transaction, &binding)?;
            // Completed replay has no new effects. New creation must honor the durable fence.
            if find_duplicate_command(&transaction, command)?.is_none() {
                reject_cancelled_admission(&transaction, &binding)?;
            }
        } else {
            reject_reserved_admission_identity(
                &transaction,
                &command.command_id,
                &command.idempotency_key,
            )?;
        }

        if let Some(result) = find_duplicate_command(&transaction, command)? {
            let dispatch_batch = load_dispatch_batch(&transaction, &result.command_id)?
                .ok_or(StorageError::DispatchBatchConflict)?;
            if dispatch_batch.run_id != state.run.run_id
                || dispatch_batch.dispatch_count as usize != dispatches.len()
                || dispatch_batch.dispatches_digest != dispatches_digest
            {
                return Err(StorageError::IdempotencyConflict);
            }
            let event_position = result.event_position.clone();
            return Ok(CommitResult {
                receipt: result,
                event_position,
                dispatch_batch,
                duplicate: true,
            });
        }

        for source in &state.input.context_manifest.sources {
            verify_object_on_disk(&transaction, &self.objects_root, &source.object_digest)?;
        }

        let run_id = state.run.run_id.as_str();
        let current_run: Option<(i64, String)> = transaction
            .query_row(
                "SELECT revision, status FROM runs WHERE run_id = ?1 AND deleted_at IS NULL",
                [run_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        match current_run {
            None if matches!(
                command.command_kind,
                CommandKind::CreateRun | CommandKind::CreateChildRun
            ) =>
            {
                if command.expected_revision != 0 || state.run.revision != 0 {
                    return Err(StorageError::RevisionConflict {
                        expected: 0,
                        actual: state.run.revision,
                    });
                }
                let title = initial_conversation_title.unwrap_or(&state.input.question.prompt);
                insert_conversation_if_missing(
                    &transaction,
                    &state.run.conversation_id,
                    title,
                    accepted_at,
                )?;
                persist_input_snapshots(&transaction, &state.input, accepted_at)?;
                insert_run(&transaction, &state.run, &state.input)?;
                bump_history_membership_generation(&transaction)?;
            }
            Some((current, current_status)) if current >= 0 => {
                if matches!(
                    command.command_kind,
                    CommandKind::CreateRun | CommandKind::CreateChildRun
                ) {
                    return Err(StorageError::ImmutableConflict(
                        "a create-run command cannot replace an existing run".to_owned(),
                    ));
                }
                if current as u64 != command.expected_revision {
                    return Err(StorageError::RevisionConflict {
                        expected: command.expected_revision,
                        actual: current as u64,
                    });
                }
                if current_status != status_name(&state.run.status) {
                    bump_history_membership_generation(&transaction)?;
                }
                if state.run.revision <= command.expected_revision {
                    return Err(StorageError::Corrupt(
                        "accepted command did not advance the run revision".to_owned(),
                    ));
                }
                persist_input_snapshots(&transaction, &state.input, accepted_at)?;
                let changed = transaction.execute(
                    "UPDATE runs SET conversation_id = ?1, parent_run_id = ?2, status = ?3, revision = ?4, generation = ?5, input_digest = ?6, run_json = ?7, input_json = ?8, tally_json = ?9, updated_at = ?10 WHERE run_id = ?11 AND revision = ?12 AND deleted_at IS NULL",
                    params![
                        state.run.conversation_id,
                        state.run.parent_run_id,
                        status_name(&state.run.status),
                        to_sql_integer(state.run.revision)?,
                        to_sql_integer(state.run.generation)?,
                        state.run.input_digest.as_str(),
                        serde_json::to_string(&state.run)?,
                        serde_json::to_string(&state.input)?,
                        state.tally.as_ref().map(serde_json::to_string).transpose()?,
                        state.run.updated_at,
                        run_id,
                        to_sql_integer(command.expected_revision)?,
                    ],
                )?;
                if changed != 1 {
                    let actual = current_run_revision(&transaction, run_id)?;
                    return Err(StorageError::RevisionConflict {
                        expected: command.expected_revision,
                        actual,
                    });
                }
                transaction.execute(
                    "UPDATE conversations SET updated_at = ?1, revision = revision + 1 WHERE conversation_id = ?2 AND deleted_at IS NULL",
                    params![accepted_at, state.run.conversation_id],
                )?;
            }
            Some((current, _)) => {
                return Err(StorageError::Corrupt(format!(
                    "run revision is negative: {current}"
                )));
            }
            None => {
                return Err(StorageError::RunNotFound(run_id.to_owned()));
            }
        }

        persist_result_rows(&transaction, &state)?;
        let event_position = persist_events(&transaction, &event_drafts, &self.identity)?;
        let receipt = CommandReceipt {
            command_id: command.command_id.clone(),
            command_kind: command.command_kind,
            target_id: command.target_id.clone(),
            payload_digest: command.payload_digest.clone(),
            accepted_revision: state.run.revision,
            event_position: event_position.clone(),
            accepted_at: accepted_at.to_owned(),
        };
        transaction.execute(
            "INSERT INTO commands (command_id, command_kind, target_id, idempotency_key, payload_digest, receipt_json, accepted_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                command.command_id,
                command_kind_name(command.command_kind),
                command.target_id,
                command.idempotency_key,
                command.payload_digest.as_str(),
                serde_json::to_string(&receipt)?,
                accepted_at,
            ],
        )?;
        let dispatch_batch = insert_dispatch_batch(
            &transaction,
            command,
            DispatchBatchInput {
                run_id: &state.run.run_id,
                batch_key: dispatch_batch_key,
                dispatches,
                dispatches_digest: &dispatches_digest,
                created_at: accepted_at,
                store: &self.identity,
            },
        )?;
        let latest_sequence = latest_event_sequence(&transaction)?;
        persist_checkpoint(&transaction, aggregate, latest_sequence)?;
        transaction.commit()?;
        aggregate.acknowledge_committed_events(&event_drafts)?;
        Ok(CommitResult {
            receipt,
            event_position,
            dispatch_batch,
            duplicate: false,
        })
    }

    pub fn resolve_core_bindings(
        &self,
        references: &[crate::CoreBindingReference],
    ) -> Result<[AcpModelBindingSnapshot; 3], StorageError> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
        let bindings = resolve_core_bindings_from(&transaction, references)?;
        transaction.commit()?;
        Ok(bindings)
    }

    pub fn load_core_execution_witnesses(
        &self,
        references: &[crate::CoreBindingReference],
    ) -> Result<[magi_domain::CatalogExecutionWitness; 3], StorageError> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
        let witnesses = load_core_execution_witnesses_from(&transaction, references)?;
        transaction.commit()?;
        Ok(witnesses)
    }

    fn frozen_deliberation_role_from(
        connection: &Connection,
        claim: &LiveRunClaim,
        core: CoreId,
    ) -> Result<magi_domain::CoreRoleProfile, StorageError> {
        let current = load_live_run_claim_state(connection, claim)?;
        ensure_active_live_run_claim(&current, claim)?;
        let state = load_persistence_state(connection, &claim.run_id)?;
        state.input.validate()?;
        if state.input.role_set.frozen_core_selections.is_none()
            || state.input.role_set.roles[0].catalog_binding.as_ref() != Some(&claim.model_binding)
        {
            return Err(StorageError::Corrupt(
                "frozen deliberation provenance missing".into(),
            ));
        }
        let role = state
            .input
            .role_set
            .roles
            .into_iter()
            .find(|role| role.core_id == core)
            .ok_or_else(|| StorageError::Corrupt("frozen role missing".into()))?;
        let binding = role
            .catalog_binding
            .as_ref()
            .ok_or_else(|| StorageError::Corrupt("frozen model missing".into()))?;
        let catalog =
            load_provider_catalog_snapshot_from(connection, &binding.catalog_snapshot_id)?
                .ok_or_else(|| StorageError::Corrupt("frozen catalog missing".into()))?;
        binding.validate_for_execution(&catalog)?;
        Ok(role)
    }

    pub fn load_frozen_deliberation_role(
        &self,
        claim: &LiveRunClaim,
        core: CoreId,
    ) -> Result<magi_domain::CoreRoleProfile, StorageError> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
        let role = Self::frozen_deliberation_role_from(&transaction, claim, core)?;
        transaction.commit()?;
        Ok(role)
    }

    pub fn load_frozen_deliberation_role_with_authority(
        &self,
        expected: &crate::AdmissionExecutionAuthority,
        claim: &LiveRunClaim,
        core: CoreId,
    ) -> Result<magi_domain::CoreRoleProfile, StorageError> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
        validate_expected_execution_authority(&transaction, Some(expected))?;
        let role = Self::frozen_deliberation_role_from(&transaction, claim, core)?;
        transaction.commit()?;
        Ok(role)
    }

    pub fn load_frozen_deliberation_slot(
        &self,
        claim: &LiveRunClaim,
        slot: u8,
    ) -> Result<magi_domain::CoreRoleProfile, StorageError> {
        self.load_frozen_deliberation_slot_inner(None, claim, slot)
    }

    pub fn load_frozen_deliberation_slot_with_authority(
        &self,
        expected: &crate::AdmissionExecutionAuthority,
        claim: &LiveRunClaim,
        slot: u8,
    ) -> Result<magi_domain::CoreRoleProfile, StorageError> {
        self.load_frozen_deliberation_slot_inner(Some(expected), claim, slot)
    }

    fn load_frozen_deliberation_slot_inner(
        &self,
        expected: Option<&crate::AdmissionExecutionAuthority>,
        claim: &LiveRunClaim,
        slot: u8,
    ) -> Result<magi_domain::CoreRoleProfile, StorageError> {
        let core = match slot {
            0..=2 => CoreId::ALL[usize::from(slot)],
            3..=5 => CoreId::ALL[usize::from(slot - 3)],
            6 => CoreId::Melchior1,
            7..=9 => CoreId::ALL[usize::from(slot - 7)],
            _ => return Err(StorageError::DispatchFenced),
        };
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
        validate_expected_execution_authority(&transaction, expected)?;
        let role = Self::frozen_deliberation_role_from(&transaction, claim, core)?;
        let current = load_live_run_claim_state(&transaction, claim)?;
        ensure_active_live_run_claim(&current, claim)?;
        let active: bool = transaction.query_row("SELECT EXISTS(SELECT 1 FROM live_run_dispatch_reservations WHERE run_id=?1 AND slot_ordinal=?2 AND state='active')", params![claim.run_id, i64::from(slot)], |row| row.get(0))?;
        if !active {
            return Err(StorageError::DispatchFenced);
        }
        transaction.commit()?;
        Ok(role)
    }

    pub fn admit_deliberation_run(
        &self,
        command: &CommandEnvelope,
        aggregate: &mut RunAggregate,
        request: &LiveRunAdmissionRequest,
        core_bindings: &[crate::CoreBindingReference],
        accepted_at: &str,
        initial_conversation_title: Option<&str>,
    ) -> Result<LiveRunAdmissionOutcome, StorageError> {
        self.admit_deliberation_run_checked(
            command,
            aggregate,
            request,
            core_bindings,
            Self::admission_publication(accepted_at, initial_conversation_title, || Ok(())),
        )
    }

    pub fn admit_deliberation_run_checked(
        &self,
        command: &CommandEnvelope,
        aggregate: &mut RunAggregate,
        request: &LiveRunAdmissionRequest,
        core_bindings: &[crate::CoreBindingReference],
        publication: AdmissionPublication<'_, impl FnOnce() -> Result<(), StorageError>>,
    ) -> Result<LiveRunAdmissionOutcome, StorageError> {
        self.admit_deliberation_run_inner(
            command,
            aggregate,
            request,
            core_bindings,
            None,
            publication,
        )
    }

    pub fn admit_clarification_deliberation_run_checked(
        &self,
        command: &CommandEnvelope,
        aggregate: &mut RunAggregate,
        request: &LiveRunAdmissionRequest,
        core_bindings: &[crate::CoreBindingReference],
        intent: &crate::ClarificationAdmissionIntent,
        publication: AdmissionPublication<'_, impl FnOnce() -> Result<(), StorageError>>,
    ) -> Result<LiveRunAdmissionOutcome, StorageError> {
        self.admit_deliberation_run_inner(
            command,
            aggregate,
            request,
            core_bindings,
            Some((intent, LIVE_RUN_QUEUE_CAPACITY)),
            publication,
        )
    }

    fn admit_deliberation_run_inner(
        &self,
        command: &CommandEnvelope,
        aggregate: &mut RunAggregate,
        request: &LiveRunAdmissionRequest,
        core_bindings: &[crate::CoreBindingReference],
        clarification: Option<(&crate::ClarificationAdmissionIntent, u8)>,
        publication: AdmissionPublication<'_, impl FnOnce() -> Result<(), StorageError>>,
    ) -> Result<LiveRunAdmissionOutcome, StorageError> {
        let (clarification, queue_capacity) = match clarification {
            Some((intent, capacity)) => (Some(intent), capacity),
            None => (None, LIVE_RUN_QUEUE_CAPACITY),
        };
        let AdmissionPublication {
            accepted_at,
            initial_conversation_title,
            commit_permission,
            execution_authority,
        } = publication;
        command.validate()?;
        validate_text("accepted_at", accepted_at, 64)?;
        validate_text("command_id", &request.command_id, 128)?;
        validate_text("idempotency_key", &request.idempotency_key, 256)?;
        validate_live_question(&request.question)?;
        let state = aggregate.persistence_state();
        state.run.validate()?;
        state.input.validate()?;
        if state.input.request_provenance.is_none() {
            return Err(StorageError::Corrupt(
                "deliberation admission intent missing".into(),
            ));
        }
        if command.command_kind != CommandKind::CreateRun
            || command.expected_revision != 0
            || command.target_id != state.run.run_id
            || command.payload_digest != state.input.input_digest
            || state.run.revision != 0
            || !matches!(state.run.status, RunStatus::Preparing)
            || request.command_id != command.command_id
            || request.idempotency_key != command.idempotency_key
            || request.question != state.input.question.prompt
            || aggregate.events().is_empty()
        {
            return Err(StorageError::Corrupt(
                "deliberation admission does not match its immutable run input".to_owned(),
            ));
        }
        if state.input.role_set.roles[0].core_id != CoreId::Melchior1
            || state.input.role_set.roles[0].catalog_binding.as_ref()
                != Some(&request.model_binding)
        {
            return Err(StorageError::Corrupt(
                "deliberation queue binding must match the frozen synthesis role".to_owned(),
            ));
        }
        let run_id = state.run.run_id.as_str();
        if state
            .input
            .role_set
            .frozen_core_selections
            .as_ref()
            .map(|selections| selections.as_slice())
            != Some(core_bindings)
        {
            return Err(StorageError::Corrupt(
                "deliberation selection provenance differs from admission references".into(),
            ));
        }
        let model_binding_digest = state.input.input_digest.clone();
        let dispatch_batch_key = format!(
            "command:{}",
            Digest::from_bytes(command.command_id.as_bytes())
        );
        let dispatches_digest =
            Digest::from_bytes(&canonical_json(&Vec::<PreparedDispatch>::new())?);
        let event_drafts = aggregate.events().to_vec();
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;

        reject_deleted_live_command(
            &transaction,
            "live_admission",
            &request.command_id,
            &request.idempotency_key,
            live_run_request_digest(&request.question, &request.model_binding)?.as_str(),
            None,
        )?;
        let prior: Option<(String, String, String)> = transaction.query_row(
            "SELECT command_id, payload_digest, receipt_json FROM live_run_receipts WHERE idempotency_key = ?1",
            [&request.idempotency_key], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).optional()?;
        if let Some((prior_command, prior_digest, receipt_json)) = prior {
            if let Some(rich)=clarification {
                if rich.request != admission_intent_from_input(&request.command_id,&request.idempotency_key,&state.input)? || state.run.parent_run_id.as_deref()!=Some(rich.parent.run_id.as_str()) {return Err(StorageError::IdempotencyConflict);}
                validate_clarification_replay_intent_from(&transaction,rich)?;
            } else if transaction.query_row("SELECT EXISTS(SELECT 1 FROM clarification_admission_intents c JOIN admission_request_bindings b ON b.command_id=c.command_id WHERE b.command_id=?1 OR b.idempotency_key=?2)",params![request.command_id,request.idempotency_key],|r|r.get::<_,bool>(0))? {
                return Err(StorageError::IdempotencyConflict);
            }
            if prior_command != request.command_id || prior_digest != model_binding_digest.as_str()
            {
                return Err(StorageError::IdempotencyConflict);
            }
            let receipt: LiveRunReceipt = serde_json::from_str(&receipt_json)?;
            let command_receipt = find_duplicate_command(&transaction, command)?
                .ok_or(StorageError::IdempotencyConflict)?;
            if command_receipt.target_id != receipt.run_id {
                return Err(StorageError::IdempotencyConflict);
            }
            let prior_state = load_persistence_state(&transaction, &receipt.run_id)?;
            if prior_state.input.input_digest != state.input.input_digest
                || prior_state.run.parent_run_id != state.run.parent_run_id
                || prior_state.run.conversation_id != state.run.conversation_id
            {
                return Err(StorageError::IdempotencyConflict);
            }
            transaction.rollback()?;
            return Ok(LiveRunAdmissionOutcome::Accepted {
                receipt,
                duplicate: true,
            });
        }
        if find_duplicate_command(&transaction, command)?.is_some() {
            return Err(StorageError::IdempotencyConflict);
        }
        validate_expected_execution_authority(&transaction, execution_authority.as_ref())?;
        let intent = admission_intent_from_input(
            &request.command_id,
            &request.idempotency_key,
            &state.input,
        )?;
        let rich = if let Some(parent_intent) = clarification {
            let expected = execution_authority
                .as_ref()
                .ok_or(StorageError::DispatchFenced)?;
            if parent_intent.request != intent {
                return Err(StorageError::DispatchFenced);
            }
            let parent = validate_clarification_parent(&transaction, &parent_intent.parent)?;
            if state.run.parent_run_id.as_deref() != Some(parent.run.run_id.as_str())
                || state.run.conversation_id != parent.run.conversation_id
            {
                return Err(StorageError::DispatchFenced);
            }
            let draft = load_clarification_draft_from_db(
                &transaction,
                expected,
                &parent_intent.context_draft_id,
            )?;
            let capture = load_capture_manifest_from_db(
                &transaction,
                &state.input.context_manifest.manifest_id,
            )?
            .ok_or(StorageError::DispatchFenced)?;
            if capture.disclosure_state != DisclosureState::Approved
                || clarification_source_semantics(&capture)?
                    != clarification_source_semantics(&draft.context.manifest)?
            {
                return Err(StorageError::DispatchFenced);
            }
            for role in &state.input.role_set.roles {
                let binding = role
                    .catalog_binding
                    .as_ref()
                    .ok_or(StorageError::DispatchFenced)?;
                let recipient = magi_context::Recipient {
                    provider_id: binding.provider_id.clone(),
                    account_profile_id: binding.provider_profile_id.clone(),
                };
                if capture.run_manifest_for_recipient(&recipient)? != state.input.context_manifest {
                    return Err(StorageError::DispatchFenced);
                }
            }
            Some(clarification_request_binding(
                &transaction,
                expected,
                parent_intent,
                true,
            )?)
        } else {
            if state.run.parent_run_id.is_some() {
                return Err(StorageError::DispatchFenced);
            }
            reject_clarification_draft_downcast(&transaction, &intent)?;
            let parent_requires_child: bool = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM runs WHERE conversation_id=?1 AND deleted_at IS NULL AND status='paused' AND json_extract(run_json,'$.status.status')='paused' AND json_extract(run_json,'$.status.reason')='needs_input')",
                [&state.run.conversation_id], |row| row.get(0),
            )?;
            if parent_requires_child {
                return Err(StorageError::DispatchFenced);
            }
            None
        };
        let request_binding = match &rich {
            Some((binding, _, _)) => binding.clone(),
            None => admission_request_binding(&intent)?,
        };
        validate_admission_binding_from(&transaction, &request_binding)?;
        validate_admission_command_namespace(&transaction, &request_binding)?;
        reject_cancelled_admission(&transaction, &request_binding)?;
        insert_admission_binding(&transaction, &request_binding, accepted_at)?;
        if let (Some(parent), Some((_, manifest, base))) = (clarification, &rich) {
            insert_clarification_intent(&transaction, parent, manifest, base)?;
        }

        let resolved = resolve_core_bindings_from(&transaction, core_bindings)?;
        for (role, binding) in state.input.role_set.roles.iter().zip(&resolved) {
            let catalog =
                load_provider_catalog_snapshot_from(&transaction, &binding.catalog_snapshot_id)?
                    .ok_or_else(|| StorageError::Corrupt("admission catalog missing".into()))?;
            binding.validate_for_execution(&catalog)?;
            if role.catalog_binding.as_ref() != Some(binding) {
                return Err(StorageError::ImmutableConflict(
                    "frozen core binding changed before admission".into(),
                ));
            }
            role.validate()?;
        }
        let admitted_count = live_run_admitted_count(&transaction)?;
        let requested_capacity = i64::from(LIVE_RUN_DISPATCH_CAPACITY);
        if admitted_count + requested_capacity > i64::from(queue_capacity) {
            transaction.rollback()?;
            return Ok(LiveRunAdmissionOutcome::QueueFull {
                capacity: LIVE_RUN_QUEUE_CAPACITY,
                admitted_count: u8::try_from(admitted_count).unwrap_or(LIVE_RUN_QUEUE_CAPACITY),
            });
        }
        let existing_run: Option<i64> = transaction
            .query_row(
                "SELECT revision FROM runs WHERE run_id = ?1 AND deleted_at IS NULL",
                [run_id],
                |row| row.get(0),
            )
            .optional()?;
        if existing_run.is_some() {
            return Err(StorageError::ImmutableConflict(
                "a deliberation run cannot replace an existing run".to_owned(),
            ));
        }
        for source in &state.input.context_manifest.sources {
            verify_object_on_disk(&transaction, &self.objects_root, &source.object_digest)?;
        }

        let title = initial_conversation_title.unwrap_or(&state.input.question.prompt);
        insert_conversation_if_missing(
            &transaction,
            &state.run.conversation_id,
            title,
            accepted_at,
        )?;
        persist_input_snapshots(&transaction, &state.input, accepted_at)?;
        insert_run(&transaction, &state.run, &state.input)?;
        bump_history_membership_generation(&transaction)?;
        persist_result_rows(&transaction, &state)?;
        let event_position = persist_events(&transaction, &event_drafts, &self.identity)?;
        let command_receipt = CommandReceipt {
            command_id: command.command_id.clone(),
            command_kind: command.command_kind,
            target_id: command.target_id.clone(),
            payload_digest: command.payload_digest.clone(),
            accepted_revision: state.run.revision,
            event_position,
            accepted_at: accepted_at.to_owned(),
        };
        transaction.execute(
            "INSERT INTO commands (command_id, command_kind, target_id, idempotency_key, payload_digest, receipt_json, accepted_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                command.command_id,
                command_kind_name(command.command_kind),
                command.target_id,
                command.idempotency_key,
                command.payload_digest.as_str(),
                serde_json::to_string(&command_receipt)?,
                accepted_at,
            ],
        )?;
        insert_dispatch_batch(
            &transaction,
            command,
            DispatchBatchInput {
                run_id,
                batch_key: &dispatch_batch_key,
                dispatches: &[],
                dispatches_digest: &dispatches_digest,
                created_at: accepted_at,
                store: &self.identity,
            },
        )?;
        let latest_sequence = latest_event_sequence(&transaction)?;
        persist_checkpoint(&transaction, aggregate, latest_sequence)?;

        let binding_json = serde_json::to_string(&request.model_binding)?;
        transaction.execute(
            "INSERT INTO live_runs (run_id, schema_version, question_text, revision, model_binding_json, catalog_snapshot_id, model_binding_digest, result_digest, result_byte_length, failure_json, created_at, updated_at) VALUES (?1, 1, ?2, 0, ?3, ?4, ?5, NULL, NULL, NULL, ?6, ?6)",
            params![
                run_id,
                request.question,
                binding_json,
                request.model_binding.catalog_snapshot_id,
                request.model_binding.binding_digest.as_str(),
                accepted_at,
            ],
        )?;
        for slot_ordinal in 0..LIVE_RUN_DISPATCH_CAPACITY {
            transaction.execute(
                "INSERT INTO live_run_dispatch_reservations (run_id, slot_ordinal, state, created_at, updated_at) VALUES (?1, ?2, 'reserved', ?3, ?3)",
                params![run_id, i64::from(slot_ordinal), accepted_at],
            )?;
        }
        transaction.execute(
            "INSERT INTO live_run_outbox (run_id, state, claim_generation, claim_owner, created_at, updated_at) VALUES (?1, 'queued', 0, NULL, ?2, ?2)",
            params![run_id, accepted_at],
        )?;
        let admission_sequence = u64::try_from(transaction.last_insert_rowid()).map_err(|_| {
            StorageError::Corrupt("live run admission sequence is invalid".to_owned())
        })?;
        let event_sequence = insert_live_run_event(
            &transaction,
            &self.identity,
            LiveRunEventInput {
                run_id,
                revision: 0,
                claim_generation: 0,
                kind: LiveRunEventKind::StatusChanged,
                status: Some(LiveRunStatus::Queued),
                text_delta: None,
                failure: None,
                created_at: accepted_at,
            },
        )?;
        let queue = load_live_run_queue_projection(
            &transaction,
            admission_sequence,
            LiveRunStatus::Queued,
        )?;
        let event_cursor =
            live_run_event_cursor(&self.identity, run_id, event_sequence, event_sequence, true);
        let receipt = LiveRunReceipt {
            command_id: request.command_id.clone(),
            run_id: run_id.to_owned(),
            accepted_revision: 0,
            queue,
            event_cursor,
        };
        transaction.execute(
            "INSERT INTO live_run_receipts (idempotency_key, command_id, run_id, payload_digest, accepted_revision, admission_sequence, accepted_at, receipt_json) VALUES (?1, ?2, ?3, ?4, 0, ?5, ?6, ?7)",
            params![
                request.idempotency_key,
                request.command_id,
                run_id,
                model_binding_digest.as_str(),
                to_sql_integer(admission_sequence)?,
                accepted_at,
                serde_json::to_string(&receipt)?,
            ],
        )?;
        reject_cancelled_admission(&transaction, &request_binding)?;
        validate_expected_execution_authority(&transaction, execution_authority.as_ref())?;
        commit_permission()?;
        if let Some(intent) = clarification {
            clarification_request_binding(
                &transaction,
                execution_authority
                    .as_ref()
                    .ok_or(StorageError::DispatchFenced)?,
                intent,
                true,
            )?;
        }
        validate_expected_execution_authority(&transaction, execution_authority.as_ref())?;
        transaction.commit()?;
        aggregate.acknowledge_committed_events(&event_drafts)?;
        Ok(LiveRunAdmissionOutcome::Accepted {
            receipt,
            duplicate: false,
        })
    }

    pub fn load_run_aggregate(&self, run_id: &str) -> Result<RunAggregate, StorageError> {
        validate_text("run_id", run_id, 128)?;
        let connection = self.connection()?;
        RunAggregate::restore(load_persistence_state(&connection, run_id)?).map_err(Into::into)
    }

    pub fn has_persisted_run(&self, run_id: &str) -> Result<bool, StorageError> {
        validate_text("run_id", run_id, 128)?;
        let connection = self.connection()?;
        connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM runs WHERE run_id = ?1 AND deleted_at IS NULL)",
                [run_id],
                |row| row.get(0),
            )
            .map_err(Into::into)
    }

    pub fn commit_aggregate_transition(
        &self,
        aggregate: &mut RunAggregate,
        expected_revision: u64,
    ) -> Result<(), StorageError> {
        self.commit_aggregate_transition_with_dispatch_slot(
            aggregate,
            expected_revision,
            None,
            None,
        )
    }

    pub fn commit_live_run_aggregate_transition(
        &self,
        claim: &LiveRunClaim,
        aggregate: &mut RunAggregate,
        expected_revision: u64,
    ) -> Result<(), StorageError> {
        self.commit_aggregate_transition_with_dispatch_slot(
            aggregate,
            expected_revision,
            None,
            Some(claim),
        )
    }

    pub fn start_admitted_deliberation(
        &self,
        claim: &LiveRunClaim,
        aggregate: &mut RunAggregate,
        expected_revision: u64,
        at: &str,
    ) -> Result<(), StorageError> {
        validate_text("created_at", at, 64)?;
        if !aggregate
            .input()
            .request_provenance
            .as_ref()
            .is_some_and(|intent| intent.disclosure_confirmed)
            || !matches!(
                aggregate.run().status,
                RunStatus::Preparing | RunStatus::AwaitingConfirmation
            )
        {
            return Err(StorageError::Domain(
                magi_domain::DomainError::Precondition {
                    required:
                        "an admitted, explicitly confirmed deliberation awaiting its legal start"
                            .to_owned(),
                    actual: "confirmation authority or preparatory state is unavailable".to_owned(),
                },
            ));
        }
        let mut next = aggregate.clone();
        if matches!(next.run().status, RunStatus::Preparing) {
            next.request_confirmation(expected_revision, at.to_owned())?;
        } else if next.run().revision != expected_revision {
            return Err(StorageError::RevisionConflict {
                expected: expected_revision,
                actual: next.run().revision,
            });
        }
        next.confirm_and_start(next.run().revision, at.to_owned())?;
        self.commit_live_run_aggregate_transition(claim, &mut next, expected_revision)?;
        *aggregate = next;
        Ok(())
    }

    pub fn commit_aggregate_dispatch_transition(
        &self,
        claim: &LiveRunClaim,
        aggregate: &mut RunAggregate,
        expected_revision: u64,
        slot_ordinal: u8,
    ) -> Result<(), StorageError> {
        self.commit_aggregate_transition_with_dispatch_slot(
            aggregate,
            expected_revision,
            Some(slot_ordinal),
            Some(claim),
        )
    }

    pub fn pause_live_deliberation_for_input_with_authority(
        &self,
        expected: &crate::AdmissionExecutionAuthority,
        claim: &LiveRunClaim,
        aggregate: &mut RunAggregate,
        expected_revision: u64,
        permission: &impl LiveRunPausePermission,
        at: &str,
    ) -> Result<LiveRunChange, StorageError> {
        validate_text("created_at", at, 64)?;
        let original = aggregate.persistence_state();
        if original.run.run_id != claim.run_id
            || original.run.revision != expected_revision
            || !original.essential_input_pending
            || !aggregate.events().is_empty()
        {
            return Err(StorageError::DispatchFenced);
        }
        let mut candidate = aggregate.clone();
        candidate.pause(
            magi_domain::PauseReason::NeedsInput,
            "Essential information is required before continuing".to_owned(),
            true,
            expected_revision,
            at.to_owned(),
        )?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        validate_expected_execution_authority(&transaction, Some(expected))?;
        let current = load_live_run_claim_state(&transaction, claim)?;
        ensure_live_run_claim(&current, claim, LiveRunStatus::Running)?;
        let stored = load_persistence_state(&transaction, &claim.run_id)?;
        if canonical_json(&stored)? != canonical_json(&original)? {
            return Err(StorageError::DispatchFenced);
        }
        let (total, unresolved): (u64, u64) = transaction.query_row("SELECT count(*), coalesce(sum(state IN ('active','unknown')),0) FROM live_run_dispatch_reservations WHERE run_id = ?1", [&claim.run_id], |row| Ok((row.get(0)?, row.get(1)?)))?;
        if total != u64::from(LIVE_RUN_DISPATCH_CAPACITY) || unresolved != 0 {
            return Err(StorageError::DispatchFenced);
        }
        let snapshot = aggregate.snapshot(0);
        let slots = load_live_dispatches_from(&transaction, &snapshot)?;
        let mut remaining = false;
        let mut settled = 0usize;
        for slot in &slots {
            match slot.state {
                crate::LiveDispatchState::Settled if !remaining && slot.result_ref.is_some() => {
                    settled += 1;
                }
                crate::LiveDispatchState::Reserved if slot.result_ref.is_none() => {
                    remaining = true;
                }
                _ => return Err(StorageError::DispatchFenced),
            }
        }
        if settled == 0
            || settled != original.assessments.len()
            || original.proposal.is_some()
            || !original.sealed_ballots.is_empty()
        {
            return Err(StorageError::DispatchFenced);
        }
        permission.validate(expected, claim)?;
        transaction.execute("UPDATE dispatch_outbox SET state = 'aborted_before_dispatch' WHERE run_id = ?1 AND state = 'prepared'", [&claim.run_id])?;
        let unresolved: u64 = transaction.query_row("SELECT count(*) FROM dispatch_outbox WHERE run_id = ?1 AND state IN ('dispatched','unknown')", [&claim.run_id], |row| row.get(0))?;
        let result: Option<String> = transaction.query_row(
            "SELECT result_digest FROM live_runs WHERE run_id = ?1",
            [&claim.run_id],
            |row| row.get(0),
        )?;
        if unresolved != 0 || result.is_some() {
            return Err(StorageError::DispatchFenced);
        }
        persist_aggregate_transition(
            &transaction,
            &self.identity,
            &mut candidate,
            expected_revision,
            &original.run.status,
        )?;
        let generation = claim
            .claim_generation
            .checked_add(1)
            .ok_or_else(|| StorageError::Integrity("claim generation overflow".to_owned()))?;
        let revision = current
            .revision
            .checked_add(1)
            .ok_or_else(|| StorageError::Integrity("live revision overflow".to_owned()))?;
        transaction.execute("UPDATE live_run_outbox SET state = 'paused', claim_owner = NULL, claim_generation = ?1, updated_at = ?2 WHERE run_id = ?3",
            params![to_sql_integer(generation)?, at, claim.run_id])?;
        transaction.execute("UPDATE live_runs SET revision = ?1, updated_at = ?2 WHERE run_id = ?3 AND revision = ?4",
            params![to_sql_integer(revision)?, at, claim.run_id, to_sql_integer(current.revision)?])?;
        transaction.execute("UPDATE live_run_dispatch_reservations SET state = 'released', updated_at = ?1 WHERE run_id = ?2 AND state = 'reserved'", params![at, claim.run_id])?;
        let active: u64 = transaction.query_row("SELECT count(*) FROM live_run_dispatch_reservations WHERE run_id = ?1 AND state = 'active'", [&claim.run_id], |row| row.get(0))?;
        if active != 0 {
            return Err(StorageError::DispatchFenced);
        }
        let sequence = insert_live_run_event(
            &transaction,
            &self.identity,
            LiveRunEventInput {
                run_id: &claim.run_id,
                revision,
                claim_generation: generation,
                kind: LiveRunEventKind::StatusChanged,
                status: Some(LiveRunStatus::Paused),
                text_delta: None,
                failure: None,
                created_at: at,
            },
        )?;
        validate_expected_execution_authority(&transaction, Some(expected))?;
        permission.validate(expected, claim)?;
        let events = candidate.events().to_vec();
        candidate.acknowledge_committed_events(&events)?;
        transaction.commit()?;
        *aggregate = candidate;
        Ok(live_run_change(
            &self.identity,
            &claim.run_id,
            revision,
            sequence,
        ))
    }

    fn commit_aggregate_transition_with_dispatch_slot(
        &self,
        aggregate: &mut RunAggregate,
        expected_revision: u64,
        dispatch_slot: Option<u8>,
        claim: Option<&LiveRunClaim>,
    ) -> Result<(), StorageError> {
        let state = aggregate.persistence_state();
        state.run.validate()?;
        state.input.validate()?;
        if claim.is_some_and(|claim| claim.run_id != state.run.run_id) {
            return Err(StorageError::DispatchFenced);
        }
        if state.run.revision <= expected_revision || aggregate.events().is_empty() {
            return Err(StorageError::Corrupt(
                "a persisted run transition must advance revision and emit an event".to_owned(),
            ));
        }
        let event_drafts = aggregate.events().to_vec();
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(claim) = claim {
            let current = load_live_run_claim_state(&transaction, claim)?;
            ensure_active_live_run_claim(&current, claim)?;
        }
        let run_json: String = transaction
            .query_row(
                "SELECT run_json FROM runs WHERE run_id = ?1 AND deleted_at IS NULL",
                [&state.run.run_id],
                |row| row.get(0),
            )
            .optional()?
            .ok_or_else(|| StorageError::RunNotFound(state.run.run_id.clone()))?;
        let previous_run: magi_domain::Run = serde_json::from_str(&run_json)?;
        if previous_run.revision != expected_revision {
            return Err(StorageError::RevisionConflict {
                expected: expected_revision,
                actual: previous_run.revision,
            });
        }
        if previous_run.input_digest != state.run.input_digest {
            return Err(StorageError::ImmutableConflict(
                "a run transition cannot replace its frozen input".to_owned(),
            ));
        }
        persist_aggregate_transition(
            &transaction,
            &self.identity,
            aggregate,
            expected_revision,
            &previous_run.status,
        )?;
        if let Some(slot_ordinal) = dispatch_slot {
            let updated = transaction.execute(
                "UPDATE live_run_dispatch_reservations SET state = 'settled', updated_at = ?1 WHERE run_id = ?2 AND slot_ordinal = ?3 AND state = 'active'",
                params![state.run.updated_at, state.run.run_id, i64::from(slot_ordinal)],
            )?;
            if updated != 1 {
                return Err(StorageError::DispatchFenced);
            }
        }
        transaction.commit()?;
        aggregate.acknowledge_committed_events(&event_drafts)?;
        Ok(())
    }

    pub fn activate_live_run_dispatch_slot(
        &self,
        claim: &LiveRunClaim,
        slot_ordinal: u8,
        at: &str,
    ) -> Result<(), StorageError> {
        validate_text("created_at", at, 64)?;
        if slot_ordinal >= LIVE_RUN_DISPATCH_CAPACITY {
            return Err(StorageError::Corrupt(
                "deliberation dispatch slot is outside its reserved range".to_owned(),
            ));
        }
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = load_live_run_claim_state(&transaction, claim)?;
        ensure_active_live_run_claim(&current, claim)?;
        let updated = transaction.execute(
            "UPDATE live_run_dispatch_reservations SET state = 'active', updated_at = ?1 WHERE run_id = ?2 AND slot_ordinal = ?3 AND state = 'reserved'",
            params![at, claim.run_id, i64::from(slot_ordinal)],
        )?;
        if updated != 1 {
            return Err(StorageError::DispatchFenced);
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn pending_dispatches(&self, limit: usize) -> Result<Vec<DispatchRecord>, StorageError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let limit = limit.min(1_000);
        let connection = self.connection()?;
        let dispatch_ids = {
            let mut statement = connection.prepare(
                "SELECT o.dispatch_id FROM dispatch_outbox o JOIN runs r ON r.run_id = o.run_id WHERE o.state = 'prepared' AND o.store_generation = ?1 AND o.attempt_generation = r.generation AND r.deleted_at IS NULL AND r.status IN ('independent_review', 'cross_review', 'synthesis', 'balloting') ORDER BY o.created_at, o.batch_key, o.batch_ordinal LIMIT ?2",
            )?;
            let rows = statement.query_map(
                params![to_sql_integer(self.identity.generation)?, limit as i64],
                |row| row.get::<_, String>(0),
            )?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        dispatch_ids
            .iter()
            .map(|dispatch_id| {
                load_dispatch_record(&connection, &self.identity, dispatch_id)?
                    .ok_or_else(|| StorageError::DispatchNotFound(dispatch_id.clone()))
            })
            .collect()
    }

    pub fn list_dispatches(&self, run_id: &str) -> Result<Vec<DispatchRecord>, StorageError> {
        validate_text("run_id", run_id, 128)?;
        let connection = self.connection()?;
        let exists: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM runs WHERE run_id = ?1 AND deleted_at IS NULL)",
            [run_id],
            |row| row.get(0),
        )?;
        if !exists {
            return Err(StorageError::RunNotFound(run_id.to_owned()));
        }
        load_dispatches_for_run(&connection, &self.identity, run_id)
    }

    pub fn dispatch_events_after_for_run(
        &self,
        cursor: &DispatchEventCursor,
        limit: usize,
    ) -> Result<DispatchEventPage, StorageError> {
        if cursor.store_id != self.identity.store_id
            || cursor.store_generation != self.identity.generation
        {
            return Err(StorageError::CursorExpired);
        }
        if cursor.after_sequence > cursor.high_water_sequence {
            return Err(StorageError::Corrupt(
                "dispatch replay cursor is beyond its high-water mark".to_owned(),
            ));
        }
        let connection = self.connection()?;
        let run_exists: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM runs WHERE run_id = ?1 AND deleted_at IS NULL)",
            [&cursor.run_id],
            |row| row.get(0),
        )?;
        if !run_exists {
            return Err(StorageError::RunNotFound(cursor.run_id.clone()));
        }
        if limit == 0 {
            return Ok(DispatchEventPage {
                events: Vec::new(),
                next_cursor: cursor.clone(),
                complete: cursor.after_sequence >= cursor.high_water_sequence,
            });
        }
        let sequence_ids = {
            let mut statement = connection.prepare(
                "SELECT sequence FROM dispatch_transition_events WHERE run_id = ?1 AND sequence > ?2 AND sequence <= ?3 ORDER BY sequence LIMIT ?4",
            )?;
            let rows = statement.query_map(
                params![
                    cursor.run_id,
                    to_sql_integer(cursor.after_sequence)?,
                    to_sql_integer(cursor.high_water_sequence)?,
                    limit.min(MAX_EVENT_PAGE) as i64 + 1,
                ],
                |row| row.get::<_, i64>(0),
            )?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        let mut events = sequence_ids
            .iter()
            .map(|sequence| load_dispatch_event(&connection, &self.identity, *sequence))
            .collect::<Result<Vec<_>, _>>()?;
        let page_limit = limit.min(MAX_EVENT_PAGE);
        let has_more = events.len() > page_limit;
        if has_more {
            events.pop();
        }
        let mut next_cursor = cursor.clone();
        if let Some(event) = events.last() {
            next_cursor.after_sequence = event.sequence;
        }
        Ok(DispatchEventPage {
            events,
            next_cursor,
            complete: !has_more,
        })
    }

    pub fn dispatch_event_cursor(&self, run_id: &str) -> Result<DispatchEventCursor, StorageError> {
        validate_text("run_id", run_id, 128)?;
        let connection = self.connection()?;
        let exists: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM runs WHERE run_id = ?1 AND deleted_at IS NULL)",
            [run_id],
            |row| row.get(0),
        )?;
        if !exists {
            return Err(StorageError::RunNotFound(run_id.to_owned()));
        }
        let high_water_sequence = dispatch_event_high_water(&connection, run_id)?;
        Ok(DispatchEventCursor {
            store_id: self.identity.store_id.clone(),
            store_generation: self.identity.generation,
            run_id: run_id.to_owned(),
            after_sequence: 0,
            high_water_sequence,
        })
    }

    pub fn mark_dispatch_dispatched(
        &self,
        identity: &DispatchIdentity,
        at: &str,
    ) -> Result<DispatchTransitionResult, StorageError> {
        self.transition_dispatch(
            identity,
            DispatchTransitionInput {
                expected_state: DispatchState::Prepared,
                next_state: DispatchState::Dispatched,
                disposition: DispatchTransitionDisposition::Applied,
                provider_request_id: None,
                result_ref: None,
                reconciliation_reference: None,
                at,
                require_current_generation: true,
            },
        )
    }

    pub fn record_dispatch_provider_request_id(
        &self,
        identity: &DispatchIdentity,
        provider_request_id: &str,
        at: &str,
    ) -> Result<DispatchTransitionResult, StorageError> {
        validate_text("provider_request_id", provider_request_id, 512)?;
        validate_text("at", at, 64)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let record = load_dispatch_record(&transaction, &self.identity, &identity.dispatch_id)?
            .ok_or_else(|| StorageError::DispatchNotFound(identity.dispatch_id.clone()))?;
        ensure_dispatch_identity(&record.identity, identity)?;
        if record.provider_request_id.as_deref() == Some(provider_request_id) {
            return Ok(DispatchTransitionResult {
                dispatch: record,
                disposition: DispatchTransitionDisposition::Applied,
            });
        }

        let is_current = if record.state == DispatchState::Dispatched {
            match ensure_dispatch_is_current(&transaction, &self.identity, identity) {
                Ok(()) => true,
                Err(StorageError::DispatchFenced) => false,
                Err(error) => return Err(error),
            }
        } else {
            false
        };
        if !is_current {
            record_dispatch_event(
                &transaction,
                identity,
                DispatchEventInput {
                    event_kind: "late_callback_quarantined",
                    previous_state: Some(record.state),
                    state: record.state,
                    disposition: DispatchTransitionDisposition::Quarantined,
                    provider_request_id: Some(provider_request_id),
                    result_ref: None,
                    reconciliation_reference: None,
                    created_at: at,
                },
            )?;
            transaction.commit()?;
            return Ok(DispatchTransitionResult {
                dispatch: record,
                disposition: DispatchTransitionDisposition::Quarantined,
            });
        }
        if record.provider_request_id.is_some() {
            return Err(StorageError::DispatchStateConflict {
                expected: "same provider request ID".to_owned(),
                actual: "a different provider request ID is already stored".to_owned(),
            });
        }

        let changed = transaction.execute(
            "UPDATE dispatch_outbox SET provider_request_id = ?1, updated_at = ?2 WHERE dispatch_id = ?3 AND state = 'dispatched' AND provider_request_id IS NULL AND store_generation = ?4 AND attempt_generation = ?5 AND input_digest = ?6 AND stage = ?7 AND binding_digest = ?8 AND payload_digest = ?9",
            params![
                provider_request_id,
                at,
                identity.dispatch_id,
                to_sql_integer(identity.store_generation)?,
                to_sql_integer(identity.run_generation)?,
                identity.input_digest.as_str(),
                run_stage_name(identity.stage),
                identity.binding_digest.as_str(),
                identity.payload_digest.as_str(),
            ],
        )?;
        if changed != 1 {
            return Err(StorageError::DispatchFenced);
        }
        record_dispatch_event(
            &transaction,
            identity,
            DispatchEventInput {
                event_kind: "provider_request_identified",
                previous_state: Some(DispatchState::Dispatched),
                state: DispatchState::Dispatched,
                disposition: DispatchTransitionDisposition::Applied,
                provider_request_id: Some(provider_request_id),
                result_ref: None,
                reconciliation_reference: None,
                created_at: at,
            },
        )?;
        refresh_checkpoint(&transaction, &identity.run_id)?;
        transaction.commit()?;
        let dispatch = load_dispatch_record(&connection, &self.identity, &identity.dispatch_id)?
            .ok_or_else(|| StorageError::DispatchNotFound(identity.dispatch_id.clone()))?;
        Ok(DispatchTransitionResult {
            dispatch,
            disposition: DispatchTransitionDisposition::Applied,
        })
    }

    pub fn abort_prepared_dispatch(
        &self,
        identity: &DispatchIdentity,
        at: &str,
    ) -> Result<DispatchTransitionResult, StorageError> {
        self.transition_dispatch(
            identity,
            DispatchTransitionInput {
                expected_state: DispatchState::Prepared,
                next_state: DispatchState::AbortedBeforeDispatch,
                disposition: DispatchTransitionDisposition::Applied,
                provider_request_id: None,
                result_ref: None,
                reconciliation_reference: None,
                at,
                require_current_generation: false,
            },
        )
    }

    pub fn settle_dispatch(
        &self,
        identity: &DispatchIdentity,
        provider_request_id: Option<&str>,
        result_ref: &ContentObjectRef,
        at: &str,
    ) -> Result<DispatchTransitionResult, StorageError> {
        validate_text("at", at, 64)?;
        if let Some(request_id) = provider_request_id {
            validate_text("provider_request_id", request_id, 512)?;
        }
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let record = load_dispatch_record(&transaction, &self.identity, &identity.dispatch_id)?
            .ok_or_else(|| StorageError::DispatchNotFound(identity.dispatch_id.clone()))?;
        ensure_dispatch_identity(&record.identity, identity)?;
        verify_content_object_ref(&transaction, &self.objects_root, result_ref)?;
        let effective_request_id =
            merge_provider_request_id(record.provider_request_id.as_deref(), provider_request_id)?;

        if record.state == DispatchState::Settled {
            if record.provider_request_id.as_deref() == effective_request_id.as_deref()
                && record.result_ref.as_ref() == Some(result_ref)
            {
                return Ok(DispatchTransitionResult {
                    dispatch: record,
                    disposition: DispatchTransitionDisposition::Applied,
                });
            }
            return Err(StorageError::DispatchStateConflict {
                expected: "same settled result".to_owned(),
                actual: "settled with a different result".to_owned(),
            });
        }

        let current = match ensure_dispatch_is_current(&transaction, &self.identity, identity) {
            Ok(()) => true,
            Err(StorageError::DispatchFenced) => false,
            Err(error) => return Err(error),
        };
        if record.state == DispatchState::Dispatched && current {
            transaction.execute(
                "UPDATE dispatch_outbox SET state = 'settled', provider_request_id = ?1, result_digest = ?2, result_byte_length = ?3, updated_at = ?4 WHERE dispatch_id = ?5 AND state = 'dispatched' AND store_generation = ?6 AND attempt_generation = ?7 AND input_digest = ?8 AND stage = ?9 AND binding_digest = ?10 AND payload_digest = ?11",
                params![
                    effective_request_id.as_deref(),
                    result_ref.digest.as_str(),
                    to_sql_integer(result_ref.byte_length)?,
                    at,
                    identity.dispatch_id,
                    to_sql_integer(identity.store_generation)?,
                    to_sql_integer(identity.run_generation)?,
                    identity.input_digest.as_str(),
                    run_stage_name(identity.stage),
                    identity.binding_digest.as_str(),
                    identity.payload_digest.as_str(),
                ],
            )?;
            record_dispatch_event(
                &transaction,
                identity,
                DispatchEventInput {
                    event_kind: "settled",
                    previous_state: Some(DispatchState::Dispatched),
                    state: DispatchState::Settled,
                    disposition: DispatchTransitionDisposition::Applied,
                    provider_request_id: effective_request_id.as_deref(),
                    result_ref: Some(result_ref),
                    reconciliation_reference: None,
                    created_at: at,
                },
            )?;
            refresh_checkpoint(&transaction, &identity.run_id)?;
            transaction.commit()?;
            let dispatch =
                load_dispatch_record(&connection, &self.identity, &identity.dispatch_id)?
                    .ok_or_else(|| StorageError::DispatchNotFound(identity.dispatch_id.clone()))?;
            return Ok(DispatchTransitionResult {
                dispatch,
                disposition: DispatchTransitionDisposition::Applied,
            });
        }

        if !matches!(
            record.state,
            DispatchState::Dispatched | DispatchState::Unknown
        ) {
            return Err(dispatch_state_conflict(
                DispatchState::Dispatched,
                record.state,
            ));
        }
        record_dispatch_event(
            &transaction,
            identity,
            DispatchEventInput {
                event_kind: "late_callback_quarantined",
                previous_state: Some(record.state),
                state: record.state,
                disposition: DispatchTransitionDisposition::Quarantined,
                provider_request_id: effective_request_id.as_deref(),
                result_ref: Some(result_ref),
                reconciliation_reference: None,
                created_at: at,
            },
        )?;
        transaction.commit()?;
        Ok(DispatchTransitionResult {
            dispatch: record,
            disposition: DispatchTransitionDisposition::Quarantined,
        })
    }

    pub fn mark_dispatch_unknown(
        &self,
        identity: &DispatchIdentity,
        detail: &str,
        at: &str,
    ) -> Result<DispatchTransitionResult, StorageError> {
        validate_text("detail", detail, 8_000)?;
        validate_text("at", at, 64)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let requested = load_dispatch_record(&transaction, &self.identity, &identity.dispatch_id)?
            .ok_or_else(|| StorageError::DispatchNotFound(identity.dispatch_id.clone()))?;
        ensure_dispatch_identity(&requested.identity, identity)?;
        if requested.state != DispatchState::Dispatched {
            return Err(dispatch_state_conflict(
                DispatchState::Dispatched,
                requested.state,
            ));
        }
        ensure_dispatch_is_current(&transaction, &self.identity, identity)?;

        let mut aggregate =
            RunAggregate::restore(load_persistence_state(&transaction, &identity.run_id)?)?;
        let previous_revision = aggregate.run().revision;
        let previous_status = aggregate.run().status.clone();

        let rows = {
            let mut statement = transaction.prepare(
                "SELECT dispatch_id, state FROM dispatch_outbox WHERE run_id = ?1 AND attempt_generation = ?2 AND state IN ('prepared', 'dispatched') ORDER BY batch_key, batch_ordinal",
            )?;
            let rows = statement.query_map(
                params![identity.run_id, to_sql_integer(identity.run_generation)?],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        for (dispatch_id, state_name) in rows {
            let record = load_dispatch_record(&transaction, &self.identity, &dispatch_id)?
                .ok_or_else(|| StorageError::DispatchNotFound(dispatch_id.clone()))?;
            let prior = parse_dispatch_state(&state_name)?;
            let next = if prior == DispatchState::Prepared {
                DispatchState::AbortedBeforeDispatch
            } else {
                DispatchState::Unknown
            };
            transaction.execute(
                "UPDATE dispatch_outbox SET state = ?1, updated_at = ?2 WHERE dispatch_id = ?3 AND state = ?4 AND store_generation = ?5 AND attempt_generation = ?6",
                params![
                    next.as_str(),
                    at,
                    dispatch_id,
                    prior.as_str(),
                    to_sql_integer(record.identity.store_generation)?,
                    to_sql_integer(identity.run_generation)?,
                ],
            )?;
            let kind = next.as_str();
            record_dispatch_event(
                &transaction,
                &record.identity,
                DispatchEventInput {
                    event_kind: kind,
                    previous_state: Some(prior),
                    state: next,
                    disposition: DispatchTransitionDisposition::Applied,
                    provider_request_id: record.provider_request_id.as_deref(),
                    result_ref: None,
                    reconciliation_reference: None,
                    created_at: at,
                },
            )?;
        }

        aggregate.mark_interrupted(detail.to_owned(), true, at.to_owned())?;
        let committed_events = aggregate.events().to_vec();
        persist_aggregate_transition(
            &transaction,
            &self.identity,
            &mut aggregate,
            previous_revision,
            &previous_status,
        )?;
        transaction.commit()?;
        aggregate.acknowledge_committed_events(&committed_events)?;
        let dispatch = load_dispatch_record(&connection, &self.identity, &identity.dispatch_id)?
            .ok_or_else(|| StorageError::DispatchNotFound(identity.dispatch_id.clone()))?;
        Ok(DispatchTransitionResult {
            dispatch,
            disposition: DispatchTransitionDisposition::Applied,
        })
    }

    pub fn reconcile_dispatch_result(
        &self,
        identity: &DispatchIdentity,
        provider_request_id: Option<&str>,
        result_ref: &ContentObjectRef,
        at: &str,
    ) -> Result<DispatchTransitionResult, StorageError> {
        if let Some(request_id) = provider_request_id {
            validate_text("provider_request_id", request_id, 512)?;
        }
        self.transition_dispatch(
            identity,
            DispatchTransitionInput {
                expected_state: DispatchState::Unknown,
                next_state: DispatchState::Settled,
                disposition: DispatchTransitionDisposition::Reconciled,
                provider_request_id,
                result_ref: Some(result_ref),
                reconciliation_reference: None,
                at,
                require_current_generation: false,
            },
        )
    }

    pub fn reconcile_dispatch_no_effect(
        &self,
        identity: &DispatchIdentity,
        evidence_reference: &str,
        at: &str,
    ) -> Result<DispatchTransitionResult, StorageError> {
        validate_text("evidence_reference", evidence_reference, 512)?;
        self.transition_dispatch(
            identity,
            DispatchTransitionInput {
                expected_state: DispatchState::Unknown,
                next_state: DispatchState::ReconciledNoEffect,
                disposition: DispatchTransitionDisposition::Reconciled,
                provider_request_id: None,
                result_ref: None,
                reconciliation_reference: Some(evidence_reference),
                at,
                require_current_generation: false,
            },
        )
    }

    fn transition_dispatch(
        &self,
        identity: &DispatchIdentity,
        input: DispatchTransitionInput<'_>,
    ) -> Result<DispatchTransitionResult, StorageError> {
        let DispatchTransitionInput {
            expected_state,
            next_state,
            disposition,
            provider_request_id,
            result_ref,
            reconciliation_reference,
            at,
            require_current_generation,
        } = input;

        validate_text("at", at, 64)?;
        if let Some(result_ref) = result_ref {
            validate_content_object_ref(result_ref)?;
        }
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let record = load_dispatch_record(&transaction, &self.identity, &identity.dispatch_id)?
            .ok_or_else(|| StorageError::DispatchNotFound(identity.dispatch_id.clone()))?;
        ensure_dispatch_identity(&record.identity, identity)?;
        let effective_request_id =
            merge_provider_request_id(record.provider_request_id.as_deref(), provider_request_id)?;
        if require_current_generation {
            ensure_dispatch_is_current(&transaction, &self.identity, identity)?;
        }
        if record.state != expected_state {
            return Err(dispatch_state_conflict(expected_state, record.state));
        }
        if next_state == DispatchState::Settled {
            let result = result_ref.ok_or_else(|| {
                StorageError::Corrupt(
                    "a settled dispatch requires a durable result reference".to_owned(),
                )
            })?;
            verify_content_object_ref(&transaction, &self.objects_root, result)?;
        }
        if next_state == DispatchState::ReconciledNoEffect && reconciliation_reference.is_none() {
            return Err(StorageError::Corrupt(
                "no-effect reconciliation requires evidence".to_owned(),
            ));
        }

        let changed = transaction.execute(
            "UPDATE dispatch_outbox SET state = ?1, provider_request_id = ?2, result_digest = ?3, result_byte_length = ?4, reconciliation_reference = ?5, updated_at = ?6 WHERE dispatch_id = ?7 AND state = ?8 AND store_generation = ?9 AND attempt_generation = ?10 AND input_digest = ?11 AND stage = ?12 AND binding_digest = ?13 AND payload_digest = ?14",
            params![
                next_state.as_str(),
                effective_request_id.as_deref(),
                result_ref.map(|value| value.digest.as_str()),
                result_ref.map(|value| to_sql_integer(value.byte_length)).transpose()?,
                reconciliation_reference,
                at,
                identity.dispatch_id,
                expected_state.as_str(),
                to_sql_integer(identity.store_generation)?,
                to_sql_integer(identity.run_generation)?,
                identity.input_digest.as_str(),
                run_stage_name(identity.stage),
                identity.binding_digest.as_str(),
                identity.payload_digest.as_str(),
            ],
        )?;
        if changed != 1 {
            return Err(StorageError::DispatchFenced);
        }
        record_dispatch_event(
            &transaction,
            identity,
            DispatchEventInput {
                event_kind: next_state.as_str(),
                previous_state: Some(expected_state),
                state: next_state,
                disposition,
                provider_request_id: effective_request_id.as_deref(),
                result_ref,
                reconciliation_reference,
                created_at: at,
            },
        )?;
        refresh_checkpoint(&transaction, &identity.run_id)?;
        transaction.commit()?;
        let dispatch = load_dispatch_record(&connection, &self.identity, &identity.dispatch_id)?
            .ok_or_else(|| StorageError::DispatchNotFound(identity.dispatch_id.clone()))?;
        Ok(DispatchTransitionResult {
            dispatch,
            disposition,
        })
    }

    pub fn load_console_snapshot(&self, run_id: &str) -> Result<ConsoleSnapshot, StorageError> {
        validate_text("run_id", run_id, 128)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction()?;
        let state = load_persistence_state(&transaction, run_id)?;
        let aggregate = RunAggregate::restore(state)?;
        let high_water_sequence = latest_event_sequence(&transaction)?;
        let snapshot: RunSnapshot = aggregate.snapshot(high_water_sequence);
        let high_water = EventPosition {
            store_id: self.identity.store_id.clone(),
            generation: self.identity.generation,
            sequence: high_water_sequence,
        };
        transaction.commit()?;
        Ok(ConsoleSnapshot {
            run: snapshot,
            high_water,
        })
    }

    pub fn events_after(
        &self,
        cursor: &EventPosition,
        limit: usize,
    ) -> Result<Vec<StoredEvent>, StorageError> {
        if cursor.store_id != self.identity.store_id
            || cursor.generation != self.identity.generation
        {
            return Err(StorageError::CursorExpired);
        }
        if limit == 0 {
            return Ok(Vec::new());
        }
        let limit = limit.min(MAX_EVENT_PAGE);
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT sequence, run_id, run_revision, attempt_generation, payload_json, created_at FROM run_events WHERE sequence > ?1 AND store_generation = ?2 ORDER BY sequence LIMIT ?3",
        )?;
        let rows = statement.query_map(
            params![
                to_sql_integer(cursor.sequence)?,
                to_sql_integer(self.identity.generation)?,
                limit as i64,
            ],
            |row| {
                let sequence: i64 = row.get(0)?;
                let run_id: String = row.get(1)?;
                let run_revision: i64 = row.get(2)?;
                let attempt_generation: i64 = row.get(3)?;
                let payload_json: String = row.get(4)?;
                let created_at: String = row.get(5)?;
                Ok((
                    sequence,
                    run_id,
                    run_revision,
                    attempt_generation,
                    payload_json,
                    created_at,
                ))
            },
        )?;
        let mut events = Vec::new();
        for row in rows {
            let (sequence, run_id, run_revision, generation, payload_json, created_at) = row?;
            if sequence < 0 || run_revision < 0 || generation < 0 {
                return Err(StorageError::Corrupt(
                    "event contains a negative sequence or revision".to_owned(),
                ));
            }
            let payload: EventPayload = serde_json::from_str(&payload_json)?;
            let event = DomainEvent {
                run_id,
                run_revision: run_revision as u64,
                generation: generation as u64,
                event_type: payload.kind(),
                payload,
                created_at,
            };
            events.push(StoredEvent {
                position: EventPosition {
                    store_id: self.identity.store_id.clone(),
                    generation: self.identity.generation,
                    sequence: sequence as u64,
                },
                event,
            });
        }
        Ok(events)
    }

    pub fn load_run_dossier(&self, run_id: &str) -> Result<RunDossier, StorageError> {
        validate_text("run_id", run_id, 128)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
        let identity = read_only_store_identity(&transaction)?;
        let dossier = load_run_dossier_from(&transaction, &identity, run_id)?;
        transaction.commit()?;
        Ok(dossier)
    }

    pub fn load_run_dossier_with_live_failure(
        &self,
        run_id: &str,
    ) -> Result<(RunDossier, Option<LiveRunFailure>), StorageError> {
        validate_text("run_id", run_id, 128)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
        let identity = read_only_store_identity(&transaction)?;
        let dossier = load_run_dossier_from(&transaction, &identity, run_id)?;
        let raw: Option<Option<String>> = transaction
            .query_row(
                "SELECT failure_json FROM live_runs WHERE run_id=?1",
                [run_id],
                |row| row.get(0),
            )
            .optional()?;
        let failure: Option<LiveRunFailure> = raw
            .flatten()
            .map(|value| serde_json::from_str(&value))
            .transpose()?;
        if let Some(failure) = &failure {
            validate_deliberation_failure_binding(&dossier.snapshot, failure)?;
        }
        transaction.commit()?;
        Ok((dossier, failure))
    }

    pub fn load_live_dispatch_projection(
        &self,
        run_id: &str,
    ) -> Result<crate::LiveDispatchProjection, StorageError> {
        validate_text("run_id", run_id, 128)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
        let projection = load_live_dispatch_projection_from(&transaction, run_id)?;
        transaction.commit()?;
        Ok(projection)
    }

    pub fn events_after_for_run(
        &self,
        cursor: &RunEventCursor,
        limit: usize,
    ) -> Result<RunEventPage, StorageError> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
        let identity = read_only_store_identity(&transaction)?;
        let page = run_events_after_from(&transaction, &identity, cursor, limit)?;
        transaction.commit()?;
        Ok(page)
    }

    pub fn record_source_freshness(
        &self,
        run_id: &str,
        source_id: &str,
        observation: &FreshnessObservation,
    ) -> Result<SourceFreshnessRecord, StorageError> {
        validate_text("run_id", run_id, 128)?;
        validate_text("source_id", source_id, 128)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let input_payload: String = transaction
            .query_row(
                "SELECT input_json FROM runs WHERE run_id = ?1 AND deleted_at IS NULL",
                [run_id],
                |row| row.get(0),
            )
            .optional()?
            .ok_or_else(|| StorageError::RunNotFound(run_id.to_owned()))?;
        let input: InputSnapshot = serde_json::from_str(&input_payload)?;
        let source = input
            .context_manifest
            .sources
            .iter()
            .find(|source| source.source_id == source_id)
            .ok_or_else(|| {
                StorageError::Corrupt(
                    "freshness source is not present in the run's frozen context".to_owned(),
                )
            })?;
        let captured_digest = source.object_digest.clone();
        validate_freshness_observation(&captured_digest, observation)?;
        transaction.execute(
            "INSERT INTO source_freshness_observations (run_id, source_id, captured_digest, observed_digest, status, observed_at_epoch_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                run_id,
                source_id,
                captured_digest.as_str(),
                observation.observed_digest.as_ref().map(Digest::as_str),
                freshness_status_name(observation.status),
                to_sql_integer(observation.observed_at_epoch_ms)?,
            ],
        )?;
        transaction.commit()?;
        Ok(SourceFreshnessRecord {
            run_id: run_id.to_owned(),
            source_id: source_id.to_owned(),
            captured_digest,
            observation: observation.clone(),
        })
    }

    pub fn list_source_freshness(
        &self,
        run_id: &str,
    ) -> Result<Vec<SourceFreshnessRecord>, StorageError> {
        validate_text("run_id", run_id, 128)?;
        let connection = self.connection()?;
        let exists: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM runs WHERE run_id = ?1 AND deleted_at IS NULL)",
            [run_id],
            |row| row.get(0),
        )?;
        if !exists {
            return Err(StorageError::RunNotFound(run_id.to_owned()));
        }
        latest_source_freshness(&connection, run_id)
    }

    pub fn integrity_check(&self) -> Result<(), StorageError> {
        let object_digests = {
            let connection = self.connection()?;
            let result: String =
                connection.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
            if result != "ok" {
                return Err(StorageError::Integrity(result));
            }
            let foreign_key_violation: Option<String> = connection
                .query_row("PRAGMA foreign_key_check", [], |row| row.get(0))
                .optional()?;
            if let Some(table) = foreign_key_violation {
                return Err(StorageError::Integrity(format!(
                    "foreign key check failed in {table}"
                )));
            }
            let mut statement =
                connection.prepare("SELECT digest FROM content_objects WHERE digest NOT IN (SELECT digest FROM unavailable_objects) ORDER BY digest")?;
            let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        for value in object_digests {
            let digest = Digest::from_hex(value)?;
            self.read_source_object(&digest)?;
        }
        Ok(())
    }

    fn recover_after_open(&self) -> Result<crate::RecoveryReport, StorageError> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let previous_generation = self.identity.generation.saturating_sub(1);
        let mut run_ids = Vec::new();
        {
            let mut statement = transaction.prepare(
                "SELECT run_id FROM runs WHERE deleted_at IS NULL AND status NOT IN ('completed', 'cancelled', 'failed') ORDER BY created_at, run_id",
            )?;
            let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
            for row in rows {
                run_ids.push(row?);
            }
        }
        let mut findings = Vec::with_capacity(run_ids.len());
        for run_id in run_ids {
            let mut aggregate =
                RunAggregate::restore(load_persistence_state(&transaction, &run_id)?)?;
            let prior_status = aggregate.run().status.clone();
            let prior_revision = aggregate.run().revision;
            let at = epoch_millis_string();
            recover_open_dispatches(&transaction, &self.identity, &run_id, &at)?;
            let unknown_external = has_unknown_external_dispatch(&transaction, &run_id)?
                || cancellation_external_possible(&transaction, &run_id)?;

            let changed = match prior_status {
                RunStatus::Preparing | RunStatus::AwaitingConfirmation => {
                    if unknown_external {
                        return Err(StorageError::Corrupt(format!(
                            "run {run_id} has a possibly dispatched call before confirmation"
                        )));
                    }
                    false
                }
                RunStatus::Paused { .. } if !unknown_external => false,
                RunStatus::Interrupted { .. } => false,
                RunStatus::Cancelling => {
                    let stopped = aggregate.persistence_state().cancel_resume_stage.is_none()
                        && !unknown_external;
                    aggregate.confirm_cancelled(
                        stopped,
                        "application restarted before cancellation was confirmed".to_owned(),
                        at.clone(),
                    )?;
                    settle_cancellation_reservations(
                        &transaction,
                        &self.identity,
                        &run_id,
                        Some(stopped),
                        &at,
                    )?;
                    true
                }
                RunStatus::Paused { .. }
                | RunStatus::IndependentReview
                | RunStatus::CrossReview
                | RunStatus::Synthesis
                | RunStatus::Balloting => {
                    aggregate.mark_interrupted(
                        "application restarted; external request state requires reconciliation"
                            .to_owned(),
                        unknown_external,
                        at.clone(),
                    )?;
                    true
                }
                RunStatus::Completed { .. } | RunStatus::Cancelled | RunStatus::Failed { .. } => {
                    false
                }
            };

            if changed {
                let state = aggregate.persistence_state();
                bump_history_membership_generation(&transaction)?;
                let updated = transaction.execute(
                    "UPDATE runs SET status = ?1, revision = ?2, generation = ?3, run_json = ?4, input_json = ?5, tally_json = ?6, updated_at = ?7 WHERE run_id = ?8 AND revision = ?9 AND deleted_at IS NULL",
                    params![
                        status_name(&state.run.status),
                        to_sql_integer(state.run.revision)?,
                        to_sql_integer(state.run.generation)?,
                        serde_json::to_string(&state.run)?,
                        serde_json::to_string(&state.input)?,
                        state.tally.as_ref().map(serde_json::to_string).transpose()?,
                        state.run.updated_at,
                        run_id,
                        to_sql_integer(prior_revision)?,
                    ],
                )?;
                if updated != 1 {
                    return Err(StorageError::Corrupt(format!(
                        "run {run_id} changed while the recovery lock was held"
                    )));
                }
                let events = aggregate.events().to_vec();
                persist_result_rows(&transaction, &state)?;
                persist_events(&transaction, &events, &self.identity)?;
                aggregate.acknowledge_committed_events(&events)?;
            }
            let latest_sequence = latest_event_sequence(&transaction)?;
            persist_checkpoint(&transaction, &aggregate, latest_sequence)?;
            findings.push(crate::RecoveryFinding {
                run_id,
                status: status_name(&aggregate.run().status).to_owned(),
                external_effect_unknown: unknown_external
                    || aggregate.persistence_state().external_effect_unknown,
                changed_to_interrupted: changed
                    && matches!(aggregate.run().status, RunStatus::Interrupted { .. }),
            });
        }
        recover_live_runs(&transaction, self)?;
        transaction.commit()?;
        Ok(crate::RecoveryReport {
            previous_generation,
            current_generation: self.identity.generation,
            findings,
        })
    }

    fn publish_source_object(&self, bytes: &[u8]) -> Result<ContentObjectRef, StorageError> {
        if bytes.len() as u64 > MAX_OBJECT_BYTES {
            return Err(StorageError::Integrity(format!(
                "source object exceeds the {MAX_OBJECT_BYTES}-byte limit"
            )));
        }
        let digest = Digest::from_bytes(bytes);
        let target = self.object_path(&digest);
        let parent = target
            .parent()
            .ok_or_else(|| StorageError::Integrity("object path has no parent".to_owned()))?;
        create_private_dir(parent)?;
        reject_symlink_if_present(&target)?;

        let temporary = self
            .temp_root
            .join(format!("{}.part", Uuid::new_v4().simple()));
        let mut temp_file = private_file_new(&temporary)?;
        temp_file.write_all(bytes)?;
        temp_file.sync_all()?;
        drop(temp_file);

        let verified_bytes = read_bounded_file(&temporary, MAX_OBJECT_BYTES)?;
        if verified_bytes.len() != bytes.len() || Digest::from_bytes(&verified_bytes) != digest {
            let _ = fs::remove_file(&temporary);
            return Err(StorageError::Integrity(
                "temporary source object failed length or digest verification".to_owned(),
            ));
        }

        match fs::hard_link(&temporary, &target) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let existing = read_bounded_file(&target, MAX_OBJECT_BYTES)?;
                if existing.len() != bytes.len() || Digest::from_bytes(&existing) != digest {
                    let _ = fs::remove_file(&temporary);
                    return Err(StorageError::Integrity(format!(
                        "existing content object {digest} does not match its address"
                    )));
                }
            }
            Err(error) => {
                let _ = fs::remove_file(&temporary);
                return Err(StorageError::Io(error));
            }
        }
        fs::remove_file(&temporary)?;
        File::open(parent)?.sync_all()?;

        let byte_length = bytes.len() as u64;
        Ok(ContentObjectRef {
            digest,
            byte_length,
        })
    }

    pub fn put_source_object(&self, bytes: &[u8]) -> Result<ContentObjectRef, StorageError> {
        let content_ref = self.publish_source_object(bytes)?;
        let digest = content_ref.digest;
        let byte_length = content_ref.byte_length;
        let connection = self.connection()?;
        let transaction = connection.unchecked_transaction()?;
        transaction.execute(
            "INSERT INTO content_objects (digest, byte_length, created_at) VALUES (?1, ?2, ?3) ON CONFLICT(digest) DO NOTHING",
            params![digest.as_str(), to_sql_integer(byte_length)?, epoch_millis_string()],
        )?;
        let recorded_length: i64 = transaction.query_row(
            "SELECT byte_length FROM content_objects WHERE digest = ?1",
            [digest.as_str()],
            |row| row.get(0),
        )?;
        if recorded_length < 0 || recorded_length as u64 != byte_length {
            return Err(StorageError::Integrity(format!(
                "content object {digest} has conflicting stored length"
            )));
        }
        transaction.commit()?;
        Ok(ContentObjectRef {
            digest,
            byte_length,
        })
    }

    pub fn read_source_object(&self, digest: &Digest) -> Result<Vec<u8>, StorageError> {
        let connection = self.connection()?;
        verify_object_on_disk(&connection, &self.objects_root, digest)
    }

    pub fn create_clarification_draft(
        &self,
        expected: &crate::AdmissionExecutionAuthority,
        parent: &crate::ClarificationParentReference,
        draft_id: &str,
        manifest: &SourceCaptureManifest,
        updated_at_epoch_ms: u64,
    ) -> Result<crate::ClarificationDraft, StorageError> {
        validate_draft_id(draft_id)?;
        manifest.validate()?;
        if manifest.disclosure_state != DisclosureState::Draft
            || !manifest.content.sources.is_empty()
            || !manifest.content.recipients.is_empty()
        {
            return Err(StorageError::DispatchFenced);
        }
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        validate_expected_execution_authority(&transaction, Some(expected))?;
        let state = validate_clarification_parent(&transaction, parent)?;
        self.insert_source_capture_manifest(&transaction, manifest)?;
        transaction.execute(
            "INSERT INTO context_drafts (draft_id,revision,manifest_id,updated_at_epoch_ms) VALUES (?1,0,?2,?3)",
            params![draft_id, manifest.manifest_id, to_sql_integer(updated_at_epoch_ms)?],
        )?;
        transaction.execute(
            "INSERT INTO clarification_drafts (draft_id,parent_run_id,parent_revision,parent_generation,parent_input_digest,lineage_id,store_generation,question_text) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            params![draft_id,parent.run_id,to_sql_integer(parent.revision)?,to_sql_integer(parent.generation)?,parent.input_digest.as_str(),expected.lineage_id,to_sql_integer(expected.store_generation)?,state.input.question.prompt],
        )?;
        transaction.execute("INSERT INTO clarification_capture_owners(manifest_id,draft_id,parent_run_id,manifest_digest) VALUES (?1,?2,?3,?4)",params![manifest.manifest_id,draft_id,parent.run_id,manifest.digest.as_str()])?;
        let draft = load_clarification_draft_from_db(&transaction, expected, draft_id)?;
        transaction.commit()?;
        Ok(draft)
    }

    pub fn load_clarification_draft(
        &self,
        expected: &crate::AdmissionExecutionAuthority,
        draft_id: &str,
    ) -> Result<crate::ClarificationDraft, StorageError> {
        validate_draft_id(draft_id)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
        let draft = load_clarification_draft_from_db(&transaction, expected, draft_id)?;
        transaction.rollback()?;
        Ok(draft)
    }

    pub fn list_clarification_drafts(
        &self,
        expected: &crate::AdmissionExecutionAuthority,
        parent: &crate::ClarificationParentReference,
    ) -> Result<Vec<crate::ClarificationDraft>, StorageError> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
        validate_expected_execution_authority(&transaction, Some(expected))?;
        validate_clarification_parent(&transaction, parent)?;
        let mut statement=transaction.prepare("SELECT c.draft_id FROM clarification_drafts c JOIN context_drafts d ON d.draft_id=c.draft_id WHERE c.parent_run_id=?1 AND c.parent_revision=?2 AND c.parent_generation=?3 AND c.parent_input_digest=?4 AND c.lineage_id=?5 ORDER BY d.updated_at_epoch_ms DESC,c.draft_id LIMIT 101")?;
        let ids = statement
            .query_map(
                params![
                    parent.run_id,
                    to_sql_integer(parent.revision)?,
                    to_sql_integer(parent.generation)?,
                    parent.input_digest.as_str(),
                    expected.lineage_id
                ],
                |r| r.get::<_, String>(0),
            )?
            .collect::<Result<Vec<_>, _>>()?;
        drop(statement);
        if ids.len() > 100 {
            return Err(StorageError::Integrity(
                "clarification draft discovery capacity exceeded".into(),
            ));
        }
        let drafts = ids
            .into_iter()
            .map(|id| load_clarification_draft_from_db(&transaction, expected, &id))
            .collect::<Result<Vec<_>, _>>()?;
        transaction.rollback()?;
        Ok(drafts)
    }

    pub fn save_clarification_question(
        &self,
        expected: &crate::AdmissionExecutionAuthority,
        draft_id: &str,
        expected_revision: u64,
        question: &str,
        updated_at_epoch_ms: u64,
    ) -> Result<crate::ClarificationDraft, StorageError> {
        validate_live_question(question)?;
        validate_draft_id(draft_id)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let draft = load_clarification_draft_from_db(&transaction, expected, draft_id)?;
        if draft.context.revision != expected_revision {
            return Err(StorageError::DraftRevisionConflict {
                expected: Some(expected_revision),
                actual: Some(draft.context.revision),
            });
        }
        let next = expected_revision
            .checked_add(1)
            .ok_or(StorageError::DispatchFenced)?;
        transaction.execute(
            "UPDATE clarification_drafts SET question_text=?2 WHERE draft_id=?1",
            params![draft_id, question],
        )?;
        transaction.execute("UPDATE context_drafts SET revision=?2,updated_at_epoch_ms=?3 WHERE draft_id=?1 AND revision=?4", params![draft_id,to_sql_integer(next)?,to_sql_integer(updated_at_epoch_ms)?,to_sql_integer(expected_revision)?])?;
        let draft = load_clarification_draft_from_db(&transaction, expected, draft_id)?;
        transaction.commit()?;
        Ok(draft)
    }

    pub fn discard_clarification_draft(
        &self,
        expected: &crate::AdmissionExecutionAuthority,
        draft_id: &str,
        expected_revision: u64,
    ) -> Result<(), StorageError> {
        validate_draft_id(draft_id)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        validate_expected_execution_authority(&transaction, Some(expected))?;
        let (revision,lineage,generation):(u64,String,u64) = transaction.query_row("SELECT d.revision,c.lineage_id,c.store_generation FROM clarification_drafts c JOIN context_drafts d ON d.draft_id=c.draft_id WHERE c.draft_id=?1",[draft_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
        if lineage != expected.lineage_id || generation > expected.store_generation {
            return Err(StorageError::DispatchFenced);
        }
        if revision != expected_revision {
            return Err(StorageError::DraftRevisionConflict {
                expected: Some(expected_revision),
                actual: Some(revision),
            });
        }
        let collect = self.retire_clarification_draft_captures(
            &transaction,
            &[draft_id.to_owned()],
            &epoch_millis_string(),
        )?;
        transaction.commit()?;
        for digest in collect {
            let path = self.object_path(&digest);
            if path.exists() {
                reject_symlink_if_present(&path)?;
                fs::remove_file(path)?;
            }
        }
        Ok(())
    }

    pub fn save_context_draft(
        &self,
        draft_id: &str,
        expected_revision: Option<u64>,
        manifest: &SourceCaptureManifest,
        updated_at_epoch_ms: u64,
    ) -> Result<ContextDraft, StorageError> {
        manifest.validate()?;
        if manifest.disclosure_state != DisclosureState::Draft {
            return Err(StorageError::ImmutableConflict(
                "only a draft disclosure manifest can update a context draft".to_owned(),
            ));
        }
        validate_draft_id(draft_id)?;
        let connection = self.connection()?;
        let transaction = connection.unchecked_transaction()?;
        let parent_bound: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM clarification_drafts WHERE draft_id=?1)",
            [draft_id],
            |r| r.get(0),
        )?;
        if parent_bound {
            let expected = read_admission_execution_authority(&transaction)?;
            load_clarification_draft_from_db(&transaction, &expected, draft_id)?;
        }
        let current_revision: Option<u64> = transaction
            .query_row(
                "SELECT revision FROM context_drafts WHERE draft_id = ?1",
                [draft_id],
                |row| row.get::<_, i64>(0).map(|revision| revision as u64),
            )
            .optional()?;
        if current_revision != expected_revision {
            return Err(StorageError::DraftRevisionConflict {
                expected: expected_revision,
                actual: current_revision,
            });
        }
        self.insert_source_capture_manifest(&transaction, manifest)?;
        if parent_bound {
            transaction.execute("INSERT INTO clarification_capture_owners(manifest_id,draft_id,parent_run_id,manifest_digest) SELECT ?2,draft_id,parent_run_id,?3 FROM clarification_drafts WHERE draft_id=?1 ON CONFLICT(manifest_id,draft_id) DO NOTHING",params![draft_id,manifest.manifest_id,manifest.digest.as_str()])?;
        }
        let revision = match current_revision {
            Some(revision) => revision.checked_add(1).ok_or_else(|| {
                StorageError::Integrity("context draft revision overflow".to_owned())
            })?,
            None => 0,
        };
        transaction.execute(
            "INSERT INTO context_drafts (draft_id, revision, manifest_id, updated_at_epoch_ms) VALUES (?1, ?2, ?3, ?4) ON CONFLICT(draft_id) DO UPDATE SET revision = excluded.revision, manifest_id = excluded.manifest_id, updated_at_epoch_ms = excluded.updated_at_epoch_ms",
            params![draft_id, to_sql_integer(revision)?, manifest.manifest_id, to_sql_integer(updated_at_epoch_ms)?],
        )?;
        transaction.commit()?;
        Ok(ContextDraft {
            draft_id: draft_id.to_owned(),
            revision,
            manifest: manifest.clone(),
            updated_at_epoch_ms,
        })
    }

    pub fn load_context_draft(&self, draft_id: &str) -> Result<Option<ContextDraft>, StorageError> {
        validate_draft_id(draft_id)?;
        let connection = self.connection()?;
        let row = connection
            .query_row(
                "SELECT d.revision, d.updated_at_epoch_ms, m.payload_json FROM context_drafts d JOIN source_capture_manifests m ON m.manifest_id = d.manifest_id WHERE d.draft_id = ?1",
                [draft_id],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?, row.get::<_, String>(2)?)),
            )
            .optional()?;
        row.map(|(revision, updated_at_epoch_ms, payload)| {
            if revision < 0 || updated_at_epoch_ms < 0 {
                return Err(StorageError::Corrupt(
                    "context draft contains a negative revision or timestamp".to_owned(),
                ));
            }
            let manifest: SourceCaptureManifest = serde_json::from_str(&payload)?;
            manifest.validate()?;
            if manifest.disclosure_state != DisclosureState::Draft {
                return Err(StorageError::Corrupt(
                    "context draft references a finalized manifest".to_owned(),
                ));
            }
            Ok(ContextDraft {
                draft_id: draft_id.to_owned(),
                revision: revision as u64,
                manifest,
                updated_at_epoch_ms: updated_at_epoch_ms as u64,
            })
        })
        .transpose()
    }

    pub fn list_context_drafts(
        &self,
        limit: usize,
    ) -> Result<Vec<ContextDraftSummary>, StorageError> {
        let limit = limit.min(100);
        if limit == 0 {
            return Ok(Vec::new());
        }
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT d.draft_id, d.revision, d.manifest_id, d.updated_at_epoch_ms, m.payload_json FROM context_drafts d JOIN source_capture_manifests m ON m.manifest_id = d.manifest_id ORDER BY d.updated_at_epoch_ms DESC, d.draft_id LIMIT ?1",
        )?;
        let rows = statement.query_map([limit as i64], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, String>(4)?,
            ))
        })?;
        let mut drafts = Vec::new();
        for row in rows {
            let (draft_id, revision, manifest_id, updated_at, payload) = row?;
            if revision < 0 || updated_at < 0 {
                return Err(StorageError::Corrupt(
                    "context draft contains a negative revision or timestamp".to_owned(),
                ));
            }
            let manifest: SourceCaptureManifest = serde_json::from_str(&payload)?;
            manifest.validate()?;
            if manifest.disclosure_state != DisclosureState::Draft
                || manifest.manifest_id != manifest_id
            {
                return Err(StorageError::Corrupt(
                    "context draft references an invalid capture manifest".to_owned(),
                ));
            }
            drafts.push(ContextDraftSummary {
                draft_id,
                revision: revision as u64,
                manifest_id,
                source_count: manifest.content.sources.len() as u64,
                captured_source_count: manifest
                    .content
                    .sources
                    .iter()
                    .filter(|source| source.state == ManifestSourceState::Captured)
                    .count() as u64,
                updated_at_epoch_ms: updated_at as u64,
            });
        }
        Ok(drafts)
    }

    pub fn load_source_capture_manifest(
        &self,
        manifest_id: &str,
    ) -> Result<Option<SourceCaptureManifest>, StorageError> {
        validate_draft_id(manifest_id)?;
        let connection = self.connection()?;
        let payload: Option<String> = connection
            .query_row(
                "SELECT payload_json FROM source_capture_manifests WHERE manifest_id = ?1",
                [manifest_id],
                |row| row.get(0),
            )
            .optional()?;
        payload
            .map(|value| {
                let manifest: SourceCaptureManifest = serde_json::from_str(&value)?;
                manifest.validate()?;
                if manifest.manifest_id != manifest_id {
                    return Err(StorageError::Corrupt(
                        "capture manifest payload does not match its key".to_owned(),
                    ));
                }
                Ok(manifest)
            })
            .transpose()
    }

    pub fn commit_context_manifest(
        &self,
        manifest: &SourceCaptureManifest,
    ) -> Result<(), StorageError> {
        manifest.validate()?;
        if manifest.disclosure_state != DisclosureState::Approved {
            return Err(StorageError::ImmutableConflict(
                "a run can reference only an approved context manifest".to_owned(),
            ));
        }
        let connection = self.connection()?;
        let transaction = connection.unchecked_transaction()?;
        self.insert_source_capture_manifest(&transaction, manifest)?;
        transaction.commit()?;
        Ok(())
    }

    fn insert_source_capture_manifest(
        &self,
        transaction: &Transaction<'_>,
        manifest: &SourceCaptureManifest,
    ) -> Result<(), StorageError> {
        let payload = serde_json::to_string(manifest)?;
        let disclosure_state = match manifest.disclosure_state {
            DisclosureState::Draft => "draft",
            DisclosureState::Approved => "approved",
        };
        transaction.execute(
            "INSERT INTO source_capture_manifests (manifest_id, manifest_digest, disclosure_state, payload_json, created_at_epoch_ms) VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT(manifest_id) DO NOTHING",
            params![manifest.manifest_id, manifest.digest.as_str(), disclosure_state, payload, to_sql_integer(manifest.created_at_epoch_ms)?],
        )?;
        let saved: (String, String, String) = transaction.query_row(
            "SELECT manifest_digest, disclosure_state, payload_json FROM source_capture_manifests WHERE manifest_id = ?1",
            [&manifest.manifest_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        if saved.0 != manifest.digest.as_str() || saved.1 != disclosure_state || saved.2 != payload
        {
            return Err(StorageError::ImmutableConflict(format!(
                "source capture manifest {} already names different content",
                manifest.manifest_id
            )));
        }

        for source in &manifest.content.sources {
            if source.state != ManifestSourceState::Captured {
                continue;
            }
            let digest = source.object_digest.as_ref().ok_or_else(|| {
                StorageError::Corrupt(format!(
                    "captured source {} has no original object digest",
                    source.source_id
                ))
            })?;
            let length = source.byte_length.ok_or_else(|| {
                StorageError::Corrupt(format!(
                    "captured source {} has no original object length",
                    source.source_id
                ))
            })?;
            let stored_length: Option<i64> = transaction
                .query_row(
                    "SELECT byte_length FROM content_objects WHERE digest = ?1",
                    [digest.as_str()],
                    |row| row.get(0),
                )
                .optional()?;
            if stored_length != Some(to_sql_integer(length)?) {
                return Err(StorageError::MissingObject(digest.to_string()));
            }
            verify_object_on_disk(transaction, &self.objects_root, digest)?;
            transaction.execute(
                "INSERT INTO source_capture_manifest_objects (manifest_id, source_id, object_digest) VALUES (?1, ?2, ?3) ON CONFLICT(manifest_id, source_id) DO NOTHING",
                params![manifest.manifest_id, source.source_id, digest.as_str()],
            )?;
            let linked_digest: String = transaction.query_row(
                "SELECT object_digest FROM source_capture_manifest_objects WHERE manifest_id = ?1 AND source_id = ?2",
                params![manifest.manifest_id, source.source_id],
                |row| row.get(0),
            )?;
            if linked_digest != digest.as_str() {
                return Err(StorageError::ImmutableConflict(format!(
                    "source {} in manifest {} already references another object",
                    source.source_id, manifest.manifest_id
                )));
            }
        }
        Ok(())
    }

    fn object_path(&self, digest: &Digest) -> PathBuf {
        let hex = digest.as_str();
        self.objects_root.join(&hex[..2]).join(&hex[2..])
    }

    fn connection(&self) -> Result<std::sync::MutexGuard<'_, Connection>, StorageError> {
        self.connection
            .lock()
            .map_err(|_| StorageError::LockPoisoned)
    }
}

fn next_selection_revision(
    actual: Option<u64>,
    expected: Option<u64>,
) -> Result<u64, StorageError> {
    if actual != expected {
        return Err(StorageError::ImmutableConflict(format!(
            "selection revision conflict: expected {expected:?}, actual {actual:?}"
        )));
    }
    actual.map_or(Ok(0), |revision| {
        revision.checked_add(1).ok_or(StorageError::Integrity(
            "selection revision overflow".into(),
        ))
    })
}

fn ensure_selection_profile_revision(
    connection: &Connection,
    profile_id: &str,
    expected: u64,
) -> Result<(), StorageError> {
    validate_text("provider_profile_id", profile_id, 128)?;
    let actual: Option<i64> = connection
        .query_row(
            "SELECT revision FROM provider_profile_heads WHERE provider_profile_id=?1",
            [profile_id],
            |row| row.get(0),
        )
        .optional()?;
    let actual = actual
        .map(|revision| {
            u64::try_from(revision)
                .map_err(|_| StorageError::Corrupt("negative profile revision".into()))
        })
        .transpose()?;
    if actual != Some(expected) {
        return Err(StorageError::ProviderProfileRevisionConflict {
            expected: Some(expected),
            actual,
        });
    }
    Ok(())
}

fn latest_selection_catalog(
    connection: &Connection,
    profile_id: &str,
    revision: u64,
) -> Result<Option<ProviderCatalogSnapshot>, StorageError> {
    let id: Option<String> = connection.query_row("SELECT catalog_snapshot_id FROM provider_catalog_snapshots WHERE provider_profile_id=?1 AND profile_revision=?2 ORDER BY rowid DESC LIMIT 1", params![profile_id, to_sql_integer(revision)?], |row| row.get(0)).optional()?;
    id.map(|id| {
        load_provider_catalog_snapshot_from(connection, &id)?
            .ok_or_else(|| StorageError::Corrupt("catalog head missing".into()))
    })
    .transpose()
}

fn model_selection_row(
    connection: &Connection,
    profile_id: &str,
) -> Result<Option<crate::ProviderModelSelection>, StorageError> {
    validate_text("provider_profile_id", profile_id, 128)?;
    let row: Option<(i64, String, i64, String)> = connection.query_row("SELECT profile_revision, catalog_snapshot_id, selection_revision, payload_json FROM provider_model_selections WHERE provider_profile_id=?1", [profile_id], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?))).optional()?;
    row.map(|(revision, catalog, selection_revision, json)| {
        let selection: crate::ProviderModelSelection = serde_json::from_str(&json)?;
        if selection.binding.provider_profile_id != profile_id
            || to_sql_integer(selection.binding.profile_revision)? != revision
            || selection.binding.catalog_snapshot_id != catalog
            || to_sql_integer(selection.selection_revision)? != selection_revision
        {
            return Err(StorageError::Corrupt(
                "model selection columns disagree".into(),
            ));
        }
        Ok(selection)
    })
    .transpose()
}

fn validate_model_selection(
    connection: &Connection,
    selection: &crate::ProviderModelSelection,
) -> Result<(), StorageError> {
    let binding = &selection.binding;
    ensure_selection_profile_revision(
        connection,
        &binding.provider_profile_id,
        binding.profile_revision,
    )?;
    if binding.artifact_set_digest.is_some() {
        load_selection_execution_witness_from(connection, binding)?;
        return Ok(());
    }
    let catalog = latest_selection_catalog(
        connection,
        &binding.provider_profile_id,
        binding.profile_revision,
    )?
    .ok_or_else(|| StorageError::ImmutableConflict("current model catalog unavailable".into()))?;
    binding.validate_ready(&catalog)?;
    Ok(())
}

fn load_core_execution_witnesses_from(
    connection: &Connection,
    references: &[crate::CoreBindingReference],
) -> Result<[magi_domain::CatalogExecutionWitness; 3], StorageError> {
    resolve_core_bindings_from(connection, references)?
        .iter()
        .map(|binding| load_selection_execution_witness_from(connection, binding))
        .collect::<Result<Vec<_>, _>>()?
        .try_into()
        .map_err(|_| StorageError::Corrupt("core witness count invalid".into()))
}

fn load_selection_execution_witness_from(
    connection: &Connection,
    binding: &AcpModelBindingSnapshot,
) -> Result<magi_domain::CatalogExecutionWitness, StorageError> {
    ensure_selection_profile_revision(
        connection,
        &binding.provider_profile_id,
        binding.profile_revision,
    )?;
    let original_catalog =
        load_provider_catalog_snapshot_from(connection, &binding.catalog_snapshot_id)?.ok_or_else(
            || StorageError::ImmutableConflict("original catalog unavailable".into()),
        )?;
    binding.validate_for_execution(&original_catalog)?;
    ensure_catalog_profile_is_current(connection, &original_catalog)?;
    let profile_row: Option<(i64, String, String, String)> = connection.query_row(
        "SELECT revision, digest, runtime_home_id, payload_json FROM provider_profile_revisions WHERE provider_profile_id=?1 AND revision=?2",
        params![binding.provider_profile_id, to_sql_integer(binding.profile_revision)?],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    ).optional()?;
    let (revision, digest, runtime_home, payload) =
        profile_row.ok_or(StorageError::ProviderProfileNotFound)?;
    let profile = decode_provider_profile(
        &binding.provider_profile_id,
        revision,
        &digest,
        &runtime_home,
        &payload,
    )?;
    if profile.provider_id != binding.provider_id {
        return Err(StorageError::ProviderProfileProviderMismatch);
    }
    let fresh_catalog = latest_selection_catalog(
        connection,
        &binding.provider_profile_id,
        binding.profile_revision,
    )?
    .ok_or_else(|| StorageError::ImmutableConflict("current catalog unavailable".into()))?;
    original_catalog.execution_equivalent_to(&fresh_catalog)?;
    Ok(magi_domain::CatalogExecutionWitness {
        binding: binding.clone(),
        original_catalog,
        fresh_catalog,
    })
}

fn resolve_core_bindings_from(
    connection: &Connection,
    references: &[crate::CoreBindingReference],
) -> Result<[AcpModelBindingSnapshot; 3], StorageError> {
    if references.len() != 3
        || references
            .iter()
            .zip(CoreId::ALL)
            .any(|(reference, core)| reference.core_id != core)
    {
        return Err(StorageError::ImmutableConflict(
            "exactly three canonical ordered core selections are required".into(),
        ));
    }
    let mut bindings = Vec::with_capacity(3);
    for reference in references {
        let core = core_selection_row(connection, reference.core_id)?
            .ok_or_else(|| StorageError::ImmutableConflict("core selection missing".into()))?;
        if core.provider_profile_id != reference.provider_profile_id
            || core.profile_revision != reference.profile_revision
            || core.model_selection_revision != reference.model_selection_revision
            || core.selection_revision != reference.core_selection_revision
        {
            return Err(StorageError::ImmutableConflict(
                "core selection reference is stale".into(),
            ));
        }
        let model = model_selection_row(connection, &core.provider_profile_id)?
            .ok_or_else(|| StorageError::ImmutableConflict("model selection missing".into()))?;
        if model.selection_revision != core.model_selection_revision
            || model.binding.profile_revision != core.profile_revision
        {
            return Err(StorageError::ImmutableConflict(
                "core model selection is stale".into(),
            ));
        }
        validate_model_selection(connection, &model)?;
        bindings.push(model.binding);
    }
    bindings
        .try_into()
        .map_err(|_| StorageError::Corrupt("core binding count invalid".into()))
}

fn core_selection_row(
    connection: &Connection,
    core_id: CoreId,
) -> Result<Option<crate::CoreModelSelection>, StorageError> {
    let row: Option<(String,i64,i64,i64,String)> = connection.query_row("SELECT provider_profile_id, profile_revision, model_selection_revision, selection_revision, payload_json FROM core_model_selections WHERE core_id=?1", [core_id.wire_name()], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?))).optional()?;
    row.map(
        |(profile, revision, model_revision, selection_revision, json)| {
            let selection: crate::CoreModelSelection = serde_json::from_str(&json)?;
            if selection.core_id != core_id
                || selection.provider_profile_id != profile
                || to_sql_integer(selection.profile_revision)? != revision
                || to_sql_integer(selection.model_selection_revision)? != model_revision
                || to_sql_integer(selection.selection_revision)? != selection_revision
            {
                return Err(StorageError::Corrupt(
                    "core selection columns disagree".into(),
                ));
            }
            Ok(selection)
        },
    )
    .transpose()
}

fn ensure_catalog_profile_is_current(
    connection: &Connection,
    catalog: &ProviderCatalogSnapshot,
) -> Result<(), StorageError> {
    let current: Option<(String, i64)> = connection
        .query_row(
            "SELECT provider_id, revision FROM provider_profile_heads WHERE provider_profile_id = ?1",
            [&catalog.provider_profile_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let (provider_id, revision) = current.ok_or(StorageError::ProviderProfileNotFound)?;
    if provider_id != catalog.provider_id {
        return Err(StorageError::ProviderProfileProviderMismatch);
    }
    let current_revision = u64::try_from(revision)
        .map_err(|_| StorageError::Corrupt("provider profile revision is negative".to_owned()))?;
    if current_revision != catalog.profile_revision {
        return Err(StorageError::ProviderProfileRevisionConflict {
            expected: Some(catalog.profile_revision),
            actual: Some(current_revision),
        });
    }
    Ok(())
}

fn load_provider_catalog_snapshot_from(
    connection: &Connection,
    catalog_snapshot_id: &str,
) -> Result<Option<ProviderCatalogSnapshot>, StorageError> {
    let row: Option<ProviderCatalogRow> = connection
        .query_row(
            "SELECT catalog_digest, provider_id, acp_mode, profile_revision, provider_profile_id, adapter_id, adapter_version, adapter_digest, payload_json FROM provider_catalog_snapshots WHERE catalog_snapshot_id = ?1",
            [catalog_snapshot_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                ))
            },
        )
        .optional()?;
    row.map(
        |(
            catalog_digest,
            provider_id,
            acp_mode,
            profile_revision,
            provider_profile_id,
            adapter_id,
            adapter_version,
            adapter_digest,
            payload_json,
        )| {
            let snapshot: ProviderCatalogSnapshot = serde_json::from_str(&payload_json)?;
            snapshot.validate()?;
            let profile_revision = u64::try_from(profile_revision).map_err(|_| {
                StorageError::Corrupt("catalog profile revision is negative".to_owned())
            })?;
            if snapshot.catalog_snapshot_id != catalog_snapshot_id
                || snapshot.catalog_digest.as_str() != catalog_digest
                || snapshot.provider_id != provider_id
                || snapshot.acp_mode != AcpMode::Acp
                || acp_mode != "acp"
                || snapshot.provider_profile_id != provider_profile_id
                || snapshot.profile_revision != profile_revision
                || snapshot.adapter_id != adapter_id
                || snapshot.adapter_version != adapter_version
                || snapshot.adapter_digest.as_str() != adapter_digest
            {
                return Err(StorageError::Corrupt(
                    "provider catalog payload does not match its immutable provenance columns"
                        .to_owned(),
                ));
            }
            Ok(snapshot)
        },
    )
    .transpose()
}

fn reject_reserved_admission_identity(
    connection: &Connection,
    command_id: &str,
    idempotency_key: &str,
) -> Result<(), StorageError> {
    if connection.query_row("SELECT EXISTS(SELECT 1 FROM admission_request_bindings WHERE command_id=?1 OR idempotency_key=?2 UNION ALL SELECT 1 FROM admission_request_cancellations WHERE command_id=?1 OR idempotency_key=?2)",params![command_id,idempotency_key],|row|row.get::<_,bool>(0))? {
        return Err(StorageError::IdempotencyConflict);
    }
    Ok(())
}

fn validate_clarification_replay_intent_from(
    connection: &Connection,
    intent: &crate::ClarificationAdmissionIntent,
) -> Result<(), StorageError> {
    let stored:bool=connection.query_row("SELECT EXISTS(SELECT 1 FROM clarification_admission_intents c JOIN admission_request_bindings b ON b.command_id=c.command_id WHERE c.command_id=?1 OR b.idempotency_key=?2)",params![intent.request.command_id,intent.request.idempotency_key],|r|r.get(0))?;
    if !stored {
        return Err(StorageError::IdempotencyConflict);
    }
    let expected = read_admission_execution_authority(connection)?;
    let (binding, _, _) = clarification_request_binding(connection, &expected, intent, false)?;
    validate_admission_binding_from(connection, &binding)
}

fn clarification_request_binding(
    connection: &Connection,
    expected: &crate::AdmissionExecutionAuthority,
    intent: &crate::ClarificationAdmissionIntent,
    require_current_draft: bool,
) -> Result<(crate::AdmissionRequestBinding, Digest, Digest), StorageError> {
    let base = admission_request_binding(&intent.request)?;
    type StoredClarificationIntent = (String, u64, u64, String, String, u64, String, String);
    let stored: Option<StoredClarificationIntent> = connection.query_row(
        "SELECT parent_run_id,parent_revision,parent_generation,parent_input_digest,draft_id,draft_revision,manifest_digest,base_intent_digest FROM clarification_admission_intents c JOIN admission_request_bindings b ON b.command_id=c.command_id WHERE c.command_id=?1 OR b.idempotency_key=?2 ORDER BY c.command_id=?1 DESC LIMIT 1",
        params![intent.request.command_id,intent.request.idempotency_key], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?)),
    ).optional()?;
    let manifest_digest = if let Some(row) = stored {
        if row.0 != intent.parent.run_id
            || row.1 != intent.parent.revision
            || row.2 != intent.parent.generation
            || row.3 != intent.parent.input_digest.as_str()
            || row.4 != intent.context_draft_id
            || row.5 != intent.context_draft_revision
            || row.7 != base.intent_digest.as_str()
        {
            return Err(StorageError::IdempotencyConflict);
        }
        Digest::from_hex(row.6)?
    } else {
        let draft =
            load_clarification_draft_from_db(connection, expected, &intent.context_draft_id)?;
        validate_clarification_intent_draft(intent, &draft)?;
        draft.context.manifest.digest
    };
    if require_current_draft {
        let draft =
            load_clarification_draft_from_db(connection, expected, &intent.context_draft_id)?;
        validate_clarification_intent_draft(intent, &draft)?;
        if draft.context.manifest.digest != manifest_digest {
            return Err(StorageError::DispatchFenced);
        }
        let parent = validate_clarification_parent(connection, &intent.parent)?;
        let original_capture =
            load_capture_manifest_from_db(connection, &parent.input.context_manifest.manifest_id)?;
        let changed_sources = match original_capture {
            Some(original) => {
                clarification_source_semantics(&original)?
                    != clarification_source_semantics(&draft.context.manifest)?
            }
            None => {
                if !parent.input.context_manifest.sources.is_empty() {
                    return Err(StorageError::DispatchFenced);
                }
                !clarification_source_semantics(&draft.context.manifest)?.is_empty()
            }
        };
        if draft.question.trim() == parent.input.question.prompt.trim() && !changed_sources {
            return Err(StorageError::DispatchFenced);
        }
    }
    let digest = clarification_intent_digest(
        &intent.parent,
        &intent.context_draft_id,
        intent.context_draft_revision,
        &manifest_digest,
        &base.intent_digest,
    )?;
    Ok((
        crate::AdmissionRequestBinding {
            intent_digest: digest,
            ..base.clone()
        },
        manifest_digest,
        base.intent_digest,
    ))
}

fn clarification_source_semantics(
    manifest: &SourceCaptureManifest,
) -> Result<Vec<Vec<u8>>, StorageError> {
    let mut sources = Vec::new();
    for source in &manifest.content.sources {
        if source.state != ManifestSourceState::Captured {
            continue;
        }
        let mut locators = source
            .included_locators
            .iter()
            .map(|locator| {
                canonical_json(&(
                    locator.start_line,
                    locator.end_line,
                    locator.total_lines,
                    locator.page,
                    locator.width,
                    locator.height,
                ))
            })
            .collect::<Result<Vec<_>, _>>()?;
        locators.sort();
        sources.push(canonical_json(&(
            &source.object_digest,
            &source.derived_digest,
            &source.representation_kind,
            &source.mime_type,
            locators,
        ))?);
    }
    sources.sort();
    Ok(sources)
}

fn validate_clarification_intent_draft(
    intent: &crate::ClarificationAdmissionIntent,
    draft: &crate::ClarificationDraft,
) -> Result<(), StorageError> {
    if intent.parent != draft.parent
        || intent.context_draft_revision != draft.context.revision
        || intent.request.question != draft.question
        || intent
            .request
            .request_provenance
            .context_draft_id
            .as_deref()
            != Some(intent.context_draft_id.as_str())
        || intent.request.request_provenance.context_revision != Some(intent.context_draft_revision)
    {
        return Err(StorageError::DispatchFenced);
    }
    Ok(())
}

fn clarification_intent_digest(
    parent: &crate::ClarificationParentReference,
    draft_id: &str,
    revision: u64,
    manifest: &Digest,
    base: &Digest,
) -> Result<Digest, StorageError> {
    Ok(Digest::from_bytes(&canonical_json(&(
        parent, draft_id, revision, manifest, base,
    ))?))
}

fn insert_clarification_intent(
    connection: &Connection,
    intent: &crate::ClarificationAdmissionIntent,
    manifest: &Digest,
    base: &Digest,
) -> Result<(), StorageError> {
    connection.execute("INSERT INTO clarification_admission_intents (command_id,parent_run_id,parent_revision,parent_generation,parent_input_digest,draft_id,draft_revision,manifest_digest,base_intent_digest) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9) ON CONFLICT(command_id) DO NOTHING", params![intent.request.command_id,intent.parent.run_id,to_sql_integer(intent.parent.revision)?,to_sql_integer(intent.parent.generation)?,intent.parent.input_digest.as_str(),intent.context_draft_id,to_sql_integer(intent.context_draft_revision)?,manifest.as_str(),base.as_str()])?;
    Ok(())
}

fn admission_request_binding(
    intent: &crate::AdmissionRequestIntent,
) -> Result<crate::AdmissionRequestBinding, StorageError> {
    validate_text("command_id", &intent.command_id, 128)?;
    validate_text("idempotency_key", &intent.idempotency_key, 256)?;
    validate_live_question(&intent.question)?;
    let mut references = intent.core_bindings.clone();
    references.sort_by_key(|item| item.core_id.wire_name());
    let mut expected = [CoreId::Melchior1, CoreId::Balthasar2, CoreId::Casper3];
    expected.sort_by_key(|core| core.wire_name());
    if references.len() != 3 || references.iter().map(|item| item.core_id).ne(expected) {
        return Err(StorageError::Corrupt(
            "admission request requires three canonical core references".into(),
        ));
    }
    for reference in &references {
        validate_text("provider_profile_id", &reference.provider_profile_id, 128)?;
    }
    let provenance = &intent.request_provenance;
    validate_text("role_preset_id", &provenance.role_preset_id, 128)?;
    if let Some(id) = &provenance.context_draft_id {
        validate_text("context_draft_id", id, 128)?;
    }
    if provenance.context_draft_id.is_some() != provenance.context_revision.is_some()
        || !provenance.disclosure_confirmed
    {
        return Err(StorageError::Corrupt(
            "admission request provenance is invalid".into(),
        ));
    }
    #[derive(Serialize)]
    struct IntentDocument<'a> {
        question: &'a str,
        core_bindings: &'a [crate::CoreBindingReference],
        request_provenance: &'a magi_domain::DeliberationRequestProvenance,
    }
    let digest = Digest::from_bytes(&canonical_json(&IntentDocument {
        question: &intent.question,
        core_bindings: &references,
        request_provenance: provenance,
    })?);
    Ok(crate::AdmissionRequestBinding {
        command_id: intent.command_id.clone(),
        idempotency_key: intent.idempotency_key.clone(),
        intent_digest: digest,
    })
}

fn validate_expected_execution_authority(
    connection: &Connection,
    expected: Option<&crate::AdmissionExecutionAuthority>,
) -> Result<(), StorageError> {
    if let Some(expected) = expected {
        let current = read_admission_execution_authority(connection)?;
        if !current.active || current != *expected {
            return Err(StorageError::DispatchFenced);
        }
    }
    Ok(())
}

fn read_admission_execution_authority(
    connection: &Connection,
) -> Result<crate::AdmissionExecutionAuthority, StorageError> {
    let lineage_id: String = connection.query_row(
        "SELECT value FROM store_meta WHERE key='admission_lineage_id'",
        [],
        |r| r.get(0),
    )?;
    let generation: String = connection.query_row(
        "SELECT value FROM store_meta WHERE key='store_generation'",
        [],
        |r| r.get(0),
    )?;
    let state: String = connection.query_row(
        "SELECT value FROM store_meta WHERE key='admission_activation_state'",
        [],
        |r| r.get(0),
    )?;
    if lineage_id.len() != 32
        || !lineage_id.bytes().all(|b| b.is_ascii_hexdigit())
        || !matches!(state.as_str(), "active" | "inert")
    {
        return Err(StorageError::Integrity(
            "invalid execution authority".into(),
        ));
    }
    let exists: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM admission_execution_lineages WHERE lineage_id=?1)",
        [&lineage_id],
        |r| r.get(0),
    )?;
    if !exists {
        return Err(StorageError::Integrity("missing execution lineage".into()));
    }
    Ok(crate::AdmissionExecutionAuthority {
        lineage_id,
        store_generation: generation
            .parse()
            .map_err(|_| StorageError::Integrity("invalid execution generation".into()))?,
        active: state == "active",
    })
}

fn validate_admission_lineage_contract(connection: &Connection) -> Result<(), StorageError> {
    read_admission_execution_authority(connection)?;
    let invalid: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM admission_request_bindings b LEFT JOIN admission_binding_lineages l ON l.command_id=b.command_id LEFT JOIN admission_execution_lineages e ON e.lineage_id=l.lineage_id WHERE e.lineage_id IS NULL UNION ALL SELECT 1 FROM live_runs r LEFT JOIN live_execution_lineages l ON l.run_id=r.run_id LEFT JOIN admission_execution_lineages e ON e.lineage_id=l.lineage_id WHERE e.lineage_id IS NULL)",[],|r|r.get(0))?;
    if invalid {
        return Err(StorageError::Integrity(
            "incomplete execution lineage mapping".into(),
        ));
    }
    Ok(())
}

fn ensure_current_admission_binding(
    connection: &Connection,
    binding: &crate::AdmissionRequestBinding,
) -> Result<(), StorageError> {
    let current = read_admission_execution_authority(connection)?;
    if !current.active {
        return Err(StorageError::DispatchFenced);
    }
    let retired: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM admission_request_bindings b JOIN admission_binding_lineages l ON l.command_id=b.command_id WHERE (b.command_id=?1 OR b.idempotency_key=?2) AND l.lineage_id<>?3)",params![binding.command_id,binding.idempotency_key,current.lineage_id],|r|r.get(0))?;
    if retired {
        return Err(StorageError::DispatchFenced);
    }
    Ok(())
}

fn ensure_current_run_lineage(connection: &Connection, run_id: &str) -> Result<(), StorageError> {
    let current = read_admission_execution_authority(connection)?;
    if !current.active {
        return Err(StorageError::DispatchFenced);
    }
    let valid: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM live_execution_lineages WHERE run_id=?1 AND lineage_id=?2)",
        params![run_id, current.lineage_id],
        |r| r.get(0),
    )?;
    if !valid {
        return Err(StorageError::DispatchFenced);
    }
    Ok(())
}

fn admission_intent_from_input(
    command_id: &str,
    idempotency_key: &str,
    input: &InputSnapshot,
) -> Result<crate::AdmissionRequestIntent, StorageError> {
    Ok(crate::AdmissionRequestIntent {
        command_id: command_id.into(),
        idempotency_key: idempotency_key.into(),
        question: input.question.prompt.clone(),
        core_bindings: input
            .role_set
            .frozen_core_selections
            .as_ref()
            .ok_or_else(|| StorageError::Corrupt("admission selections missing".into()))?
            .to_vec(),
        request_provenance: input
            .request_provenance
            .clone()
            .ok_or_else(|| StorageError::Corrupt("admission intent missing".into()))?,
    })
}

fn frozen_admission_intent_digest(
    connection: &Connection,
    command_id: &str,
    key: &str,
    run_id: &str,
) -> Result<Digest, StorageError> {
    let state = load_persistence_state(connection, run_id)?;
    let base =
        admission_request_binding(&admission_intent_from_input(command_id, key, &state.input)?)?;
    if state.run.parent_run_id.is_none() {
        return Ok(base.intent_digest);
    }
    let row:(String,u64,u64,String,String,u64,String,String)=connection.query_row(
        "SELECT c.parent_run_id,c.parent_revision,c.parent_generation,c.parent_input_digest,c.draft_id,c.draft_revision,c.manifest_digest,c.base_intent_digest FROM clarification_admission_intents c JOIN admission_request_bindings b ON b.command_id=c.command_id WHERE b.idempotency_key=?1 LIMIT 1",[key],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?)),
    ).optional()?.ok_or(StorageError::IdempotencyConflict)?;
    if state.run.parent_run_id.as_deref() != Some(row.0.as_str())
        || row.7 != base.intent_digest.as_str()
        || state
            .input
            .request_provenance
            .as_ref()
            .and_then(|p| p.context_draft_id.as_deref())
            != Some(row.4.as_str())
        || state
            .input
            .request_provenance
            .as_ref()
            .and_then(|p| p.context_revision)
            != Some(row.5)
    {
        return Err(StorageError::IdempotencyConflict);
    }
    clarification_intent_digest(
        &crate::ClarificationParentReference {
            run_id: row.0,
            revision: row.1,
            generation: row.2,
            input_digest: Digest::from_hex(row.3)?,
        },
        &row.4,
        row.5,
        &Digest::from_hex(row.6)?,
        &base.intent_digest,
    )
}

fn validate_admission_binding_from(
    connection: &Connection,
    binding: &crate::AdmissionRequestBinding,
) -> Result<(), StorageError> {
    let mut statement=connection.prepare("SELECT command_id,idempotency_key,intent_digest FROM admission_request_bindings WHERE command_id=?1 OR idempotency_key=?2")?;
    let rows = statement.query_map(
        params![binding.command_id, binding.idempotency_key],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        },
    )?;
    for row in rows {
        let (_id, key, digest) = row?;
        if key != binding.idempotency_key || digest != binding.intent_digest.as_str() {
            return Err(StorageError::IdempotencyConflict);
        }
    }
    Ok(())
}

fn insert_admission_binding(
    connection: &Connection,
    binding: &crate::AdmissionRequestBinding,
    accepted_at: &str,
) -> Result<(), StorageError> {
    ensure_current_admission_binding(connection, binding)?;
    connection.execute("INSERT INTO admission_request_bindings(command_id,idempotency_key,intent_digest,accepted_at) VALUES(?1,?2,?3,?4) ON CONFLICT(command_id) DO NOTHING",params![binding.command_id,binding.idempotency_key,binding.intent_digest.as_str(),accepted_at])?;
    Ok(())
}

fn validate_admission_command_namespace(
    connection: &Connection,
    binding: &crate::AdmissionRequestBinding,
) -> Result<(), StorageError> {
    if connection.query_row("SELECT EXISTS(SELECT 1 FROM admission_request_cancellations WHERE command_id=?1 OR idempotency_key=?2 UNION ALL SELECT 1 FROM run_command_tombstones WHERE command_id=?1 OR idempotency_key=?2 UNION ALL SELECT 1 FROM live_run_cancellation_receipts WHERE command_id=?1 OR idempotency_key=?2)",params![binding.command_id,binding.idempotency_key],|row|row.get::<_,bool>(0))? { return Err(StorageError::IdempotencyConflict); }
    let mut live_statement=connection.prepare("SELECT command_id,run_id,idempotency_key,payload_digest,receipt_json FROM live_run_receipts WHERE command_id=?1 OR idempotency_key=?2")?;
    let live_rows = live_statement.query_map(
        params![binding.command_id, binding.idempotency_key],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        },
    )?;
    for row in live_rows {
        let (command_id, run_id, key, payload, json) = row?;
        let receipt: LiveRunReceipt = serde_json::from_str(&json)
            .map_err(|_| StorageError::Corrupt("admitted request receipt is invalid".into()))?;
        if receipt.command_id != command_id || receipt.run_id != run_id {
            return Err(StorageError::Corrupt(
                "admitted request receipt differs from authority".into(),
            ));
        }
        if key != binding.idempotency_key {
            return Err(StorageError::IdempotencyConflict);
        }
        let input = load_persistence_state(connection, &run_id)
            .map_err(|_| StorageError::IdempotencyConflict)?
            .input;
        if input.input_digest.as_str() != payload {
            return Err(StorageError::Corrupt(
                "admitted request payload differs from frozen input".into(),
            ));
        }
        if frozen_admission_intent_digest(connection, &binding.command_id, &key, &run_id)?
            != binding.intent_digest
        {
            return Err(StorageError::IdempotencyConflict);
        }
    }
    let mut statement=connection.prepare("SELECT command_id,command_kind,target_id,idempotency_key FROM commands WHERE command_id=?1 OR idempotency_key=?2")?;
    let rows = statement.query_map(
        params![binding.command_id, binding.idempotency_key],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        },
    )?;
    for row in rows {
        let (_, kind, target, key) = row?;
        if kind != "create_run" || key != binding.idempotency_key {
            return Err(StorageError::IdempotencyConflict);
        }
        if frozen_admission_intent_digest(connection, &binding.command_id, &key, &target)?
            != binding.intent_digest
        {
            return Err(StorageError::IdempotencyConflict);
        }
    }
    Ok(())
}

fn admission_cancellation_payload_digest(
    binding: &crate::AdmissionRequestBinding,
) -> Result<Digest, StorageError> {
    Ok(Digest::from_bytes(&canonical_json(binding)?))
}

type AdmissionCancellationRow = (
    String,
    String,
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    String,
);

fn decode_admission_cancellation_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<AdmissionCancellationRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
    ))
}

fn validate_admission_cancellation_row(
    raw: AdmissionCancellationRow,
) -> Result<crate::AdmissionRequestCancellationReceipt, StorageError> {
    let (id, key, request_id, request_key, digest, payload, at, run, json) = raw;
    let receipt: crate::AdmissionRequestCancellationReceipt = serde_json::from_str(&json)
        .map_err(|_| StorageError::Corrupt("admission cancellation receipt is invalid".into()))?;
    if receipt.schema_version != 1
        || receipt.command_id != id
        || receipt.idempotency_key != key
        || receipt.request.command_id != request_id
        || receipt.request.idempotency_key != request_key
        || receipt.request.intent_digest.as_str() != digest
        || !receipt.request.intent_digest.is_valid()
        || admission_cancellation_payload_digest(&receipt.request)?.as_str() != payload
        || receipt.accepted_at != at
        || receipt.admitted_run_id != run
    {
        return Err(StorageError::Corrupt(
            "admission cancellation receipt differs from immutable authority".into(),
        ));
    }
    validate_text("command_id", &id, 128)?;
    validate_text("idempotency_key", &key, 256)?;
    validate_text("request_command_id", &request_id, 128)?;
    validate_text("request_idempotency_key", &request_key, 256)?;
    validate_text("accepted_at", &at, 64)?;
    if let Some(run) = &run {
        validate_text("run_id", run, 128)?;
    }
    Ok(receipt)
}

const ADMISSION_CANCELLATION_COLUMNS: &str = "command_id,idempotency_key,request_command_id,request_idempotency_key,intent_digest,payload_digest,accepted_at,admitted_run_id,receipt_json";

fn load_admission_cancellation_by_command(
    connection: &Connection,
    id: &str,
    key: &str,
) -> Result<Option<crate::AdmissionRequestCancellationReceipt>, StorageError> {
    let mut statement=connection.prepare(&format!("SELECT {ADMISSION_CANCELLATION_COLUMNS} FROM admission_request_cancellations WHERE command_id=?1 OR idempotency_key=?2"))?;
    let rows = statement.query_map(params![id, key], decode_admission_cancellation_row)?;
    let mut result = None;
    for row in rows {
        let receipt = validate_admission_cancellation_row(row?)?;
        if result.as_ref().is_some_and(|prior| prior != &receipt) {
            return Err(StorageError::IdempotencyConflict);
        }
        result = Some(receipt);
    }
    Ok(result)
}

fn load_admission_cancellation_from(
    connection: &Connection,
    binding: &crate::AdmissionRequestBinding,
) -> Result<Option<crate::AdmissionRequestCancellationReceipt>, StorageError> {
    let mut statement=connection.prepare(&format!("SELECT {ADMISSION_CANCELLATION_COLUMNS} FROM admission_request_cancellations WHERE request_command_id=?1 OR request_idempotency_key=?2 ORDER BY rowid"))?;
    let rows = statement.query_map(
        params![binding.command_id, binding.idempotency_key],
        decode_admission_cancellation_row,
    )?;
    let mut result = None;
    for row in rows {
        let receipt = validate_admission_cancellation_row(row?)?;
        if receipt.request.idempotency_key != binding.idempotency_key
            || receipt.request.intent_digest != binding.intent_digest
        {
            return Err(StorageError::IdempotencyConflict);
        }
        if result.is_none() {
            result = Some(receipt);
        }
    }
    Ok(result)
}

fn reject_cancelled_admission(
    connection: &Connection,
    binding: &crate::AdmissionRequestBinding,
) -> Result<(), StorageError> {
    ensure_current_admission_binding(connection, binding)?;
    if load_admission_cancellation_from(connection, binding)?.is_some() {
        return Err(StorageError::DispatchFenced);
    }
    Ok(())
}

fn validate_admission_request_data_contract(connection: &Connection) -> Result<(), StorageError> {
    let mut statement=connection.prepare("SELECT command_id,idempotency_key,intent_digest,accepted_at FROM admission_request_bindings")?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
        ))
    })?;
    for row in rows {
        let (id, key, digest, at) = row?;
        validate_text("command_id", &id, 128)?;
        validate_text("idempotency_key", &key, 256)?;
        validate_text("accepted_at", &at, 64)?;
        let binding = crate::AdmissionRequestBinding {
            command_id: id,
            idempotency_key: key,
            intent_digest: Digest::from_hex(digest)?,
        };
        validate_admission_binding_from(connection, &binding)?;
    }
    let mut statement = connection.prepare(&format!(
        "SELECT {ADMISSION_CANCELLATION_COLUMNS} FROM admission_request_cancellations"
    ))?;
    for row in statement.query_map([], decode_admission_cancellation_row)? {
        let receipt = validate_admission_cancellation_row(row?)?;
        let exists:bool=connection.query_row("SELECT EXISTS(SELECT 1 FROM admission_request_bindings WHERE command_id=?1 AND idempotency_key=?2 AND intent_digest=?3)",params![receipt.request.command_id,receipt.request.idempotency_key,receipt.request.intent_digest.as_str()],|row|row.get(0))?;
        if !exists {
            return Err(StorageError::Corrupt(
                "admission cancellation binding missing".into(),
            ));
        }
    }
    Ok(())
}

fn validate_live_question(question: &str) -> Result<(), StorageError> {
    if question.trim().is_empty()
        || question.len() > 32_000
        || question
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    {
        return Err(StorageError::Corrupt(
            "question must contain 1 to 32000 UTF-8 bytes and no unsupported control characters"
                .to_owned(),
        ));
    }
    Ok(())
}

fn validate_live_text(path: &str, value: &str, maximum: usize) -> Result<(), StorageError> {
    if value.len() > maximum
        || value
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    {
        return Err(StorageError::Corrupt(format!(
            "{path} exceeds {maximum} UTF-8 bytes or contains unsupported control characters"
        )));
    }
    Ok(())
}

fn live_run_request_digest(
    question: &str,
    model_binding: &AcpModelBindingSnapshot,
) -> Result<Digest, StorageError> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct ModelSelection<'a> {
        schema_version: u16,
        provider_id: &'a str,
        acp_mode: AcpMode,
        provider_profile_id: &'a str,
        profile_revision: u64,
        adapter_id: &'a str,
        adapter_version: &'a str,
        adapter_digest: &'a Digest,
        model_id: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        mode_id: Option<&'a str>,
    }

    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Payload<'a> {
        question: &'a str,
        model_selection: ModelSelection<'a>,
    }

    Ok(Digest::from_bytes(&canonical_json(&Payload {
        question,
        model_selection: ModelSelection {
            schema_version: model_binding.schema_version,
            provider_id: &model_binding.provider_id,
            acp_mode: model_binding.acp_mode,
            provider_profile_id: &model_binding.provider_profile_id,
            profile_revision: model_binding.profile_revision,
            adapter_id: &model_binding.adapter_id,
            adapter_version: &model_binding.adapter_version,
            adapter_digest: &model_binding.adapter_digest,
            model_id: &model_binding.model_id,
            mode_id: model_binding.mode_id.as_deref(),
        },
    })?))
}

fn legacy_live_run_request_digest(
    question: &str,
    model_binding: &AcpModelBindingSnapshot,
) -> Result<Digest, StorageError> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Payload<'a> {
        question: &'a str,
        model_binding: &'a AcpModelBindingSnapshot,
    }

    Ok(Digest::from_bytes(&canonical_json(&Payload {
        question,
        model_binding,
    })?))
}

fn live_run_request_matches(
    stored_payload_digest: &str,
    stored_question: &str,
    stored_model_binding_json: &str,
    request: &LiveRunAdmissionRequest,
) -> Result<bool, StorageError> {
    let stored_model_binding: AcpModelBindingSnapshot =
        serde_json::from_str(stored_model_binding_json)?;
    let requested_digest = live_run_request_digest(&request.question, &request.model_binding)?;
    let stored_selection_digest = live_run_request_digest(stored_question, &stored_model_binding)?;
    if requested_digest != stored_selection_digest {
        return Ok(false);
    }
    if stored_payload_digest == stored_selection_digest.as_str() {
        return Ok(true);
    }
    let legacy_digest = legacy_live_run_request_digest(stored_question, &stored_model_binding)?;
    Ok(stored_payload_digest == legacy_digest.as_str())
}

fn reject_deleted_live_command(
    connection: &Connection,
    kind: &str,
    command_id: &str,
    key: &str,
    payload_digest: &str,
    target: Option<&str>,
) -> Result<(), StorageError> {
    let deleted: Option<(String,String,String,String,String)> = connection.query_row(
        "SELECT command_kind,command_id,idempotency_key,payload_digest,run_id FROM run_command_tombstones WHERE command_id=?1 OR (command_kind=?3 AND idempotency_key=?2) ORDER BY (command_kind=?3) DESC LIMIT 1",
        params![command_id,key,kind], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional()?;
    if let Some((stored_kind, id, stored_key, digest, run_id)) = deleted {
        return Err(
            if stored_kind == kind
                && id == command_id
                && stored_key == key
                && digest == payload_digest
                && target.is_none_or(|target| target == run_id)
            {
                StorageError::RunNotFound(run_id)
            } else {
                StorageError::IdempotencyConflict
            },
        );
    }
    Ok(())
}

fn find_existing_live_run_receipt(
    connection: &Connection,
    request: &LiveRunAdmissionRequest,
) -> Result<Option<LiveRunReceipt>, StorageError> {
    reject_deleted_live_command(
        connection,
        "live_admission",
        &request.command_id,
        &request.idempotency_key,
        live_run_request_digest(&request.question, &request.model_binding)?.as_str(),
        None,
    )?;
    let existing: Option<(String, String, String, String, String)> = connection
        .query_row(
            "SELECT rr.payload_digest, rr.command_id, rr.receipt_json, r.question_text, r.model_binding_json FROM live_run_receipts rr JOIN live_runs r ON r.run_id = rr.run_id WHERE rr.idempotency_key = ?1",
            [&request.idempotency_key],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .optional()?;
    if let Some((stored_digest, stored_command_id, receipt_json, question, model_binding_json)) =
        existing
    {
        if stored_command_id != request.command_id
            || !live_run_request_matches(&stored_digest, &question, &model_binding_json, request)?
        {
            return Err(StorageError::IdempotencyConflict);
        }
        let receipt: LiveRunReceipt = serde_json::from_str(&receipt_json)?;
        if receipt.command_id != request.command_id {
            return Err(StorageError::Corrupt(
                "live run receipt does not match its command identifier".to_owned(),
            ));
        }
        return Ok(Some(receipt));
    }

    let duplicate_command: Option<String> = connection
        .query_row(
            "SELECT idempotency_key FROM live_run_receipts WHERE command_id = ?1 LIMIT 1",
            [&request.command_id],
            |row| row.get(0),
        )
        .optional()?;
    if duplicate_command.is_some() {
        return Err(StorageError::IdempotencyConflict);
    }
    Ok(None)
}

fn live_run_admitted_count(connection: &Connection) -> Result<i64, StorageError> {
    Ok(connection.query_row(
        "SELECT (SELECT count(*) FROM live_run_dispatch_reservations WHERE state IN ('reserved', 'active', 'unknown')) + (SELECT count(*) FROM live_run_outbox o WHERE o.state IN ('queued', 'claimed', 'session_creation_intent', 'running', 'cancelling', 'unknown') AND NOT EXISTS (SELECT 1 FROM live_run_dispatch_reservations r WHERE r.run_id = o.run_id AND r.state IN ('reserved', 'active', 'unknown')))",
        [],
        |row| row.get(0),
    )?)
}

fn load_live_run_queue_projection(
    connection: &Connection,
    admission_sequence: u64,
    status: LiveRunStatus,
) -> Result<LiveRunQueueProjection, StorageError> {
    let admitted_count = live_run_admitted_count(connection)?;
    let queue_position: Option<i64> = if status == LiveRunStatus::Queued {
        Some(
            connection.query_row(
                "SELECT count(*) + 1 FROM live_run_outbox WHERE state = 'queued' AND admission_sequence < ?1",
                [to_sql_integer(admission_sequence)?],
                |row| row.get(0),
            )?,
        )
    } else {
        None
    };
    let admitted_count = u8::try_from(admitted_count).map_err(|_| {
        StorageError::Corrupt("live run admitted count exceeds its capacity".to_owned())
    })?;
    let position = queue_position
        .map(|value| {
            u8::try_from(value)
                .map_err(|_| StorageError::Corrupt("live run queue position is invalid".to_owned()))
        })
        .transpose()?;
    Ok(LiveRunQueueProjection {
        admission_sequence,
        state: status,
        position,
        admitted_count,
        capacity: LIVE_RUN_QUEUE_CAPACITY,
    })
}

fn live_run_event_cursor(
    identity: &StoreIdentity,
    run_id: &str,
    after_sequence: u64,
    high_water_sequence: u64,
    complete: bool,
) -> LiveRunEventCursor {
    LiveRunEventCursor {
        store_id: identity.store_id.clone(),
        store_generation: identity.generation,
        run_id: run_id.to_owned(),
        after_sequence,
        high_water_sequence,
        complete,
    }
}

fn live_run_change(
    identity: &StoreIdentity,
    run_id: &str,
    revision: u64,
    event_sequence: u64,
) -> LiveRunChange {
    LiveRunChange {
        run_id: run_id.to_owned(),
        revision,
        event_cursor: live_run_event_cursor(identity, run_id, event_sequence, event_sequence, true),
    }
}

fn insert_live_run_event(
    transaction: &Transaction<'_>,
    identity: &StoreIdentity,
    input: LiveRunEventInput<'_>,
) -> Result<u64, StorageError> {
    let LiveRunEventInput {
        run_id,
        revision,
        claim_generation,
        kind,
        status,
        text_delta,
        failure,
        created_at,
    } = input;

    let failure_json = failure.map(serde_json::to_string).transpose()?;
    transaction.execute(
        "INSERT INTO live_run_events (store_generation, run_id, run_revision, claim_generation, kind, status, text_delta, failure_json, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            to_sql_integer(identity.generation)?,
            run_id,
            to_sql_integer(revision)?,
            to_sql_integer(claim_generation)?,
            kind.as_str(),
            status.map(LiveRunStatus::as_str),
            text_delta,
            failure_json,
            created_at,
        ],
    )?;
    u64::try_from(transaction.last_insert_rowid())
        .map_err(|_| StorageError::Corrupt("live run event sequence is invalid".to_owned()))
}

fn canonical_completed_deliberation_result(
    snapshot: &RunSnapshot,
) -> Result<LiveProviderResultInput, StorageError> {
    if !matches!(snapshot.run.status, RunStatus::Completed { .. }) {
        return Err(StorageError::Integrity(
            "canonical provider result requires a completed deliberation".into(),
        ));
    }
    let proposal = snapshot.proposal.as_ref().ok_or_else(|| {
        StorageError::Corrupt("completed deliberation has no frozen proposal".into())
    })?;
    Ok(LiveProviderResultInput {
        final_text: proposal.body.clone(),
        stop_reason: "deliberation_completed".into(),
        usage: None,
    })
}

fn load_live_run_snapshot_from(
    connection: &Connection,
    identity: &StoreIdentity,
    objects_root: &Path,
    run_id: &str,
    after_sequence: u64,
) -> Result<LiveRunSnapshot, StorageError> {
    let row: Option<LiveRunSnapshotRow> = connection
        .query_row(
            "SELECT r.schema_version, q.state, r.question_text, r.revision, r.model_binding_json, r.catalog_snapshot_id, r.model_binding_digest, q.admission_sequence, r.result_digest, r.result_byte_length, r.failure_json, r.created_at, r.updated_at FROM live_runs r JOIN live_run_outbox q ON q.run_id = r.run_id WHERE r.run_id = ?1",
            [run_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                    row.get(9)?,
                    row.get(10)?,
                    row.get(11)?,
                    row.get(12)?,
                ))
            },
        )
        .optional()?;
    let (
        schema_version,
        state,
        question,
        revision,
        model_binding_json,
        row_catalog_snapshot_id,
        row_binding_digest,
        admission_sequence,
        result_digest,
        result_byte_length,
        failure_json,
        created_at,
        updated_at,
    ) = row.ok_or_else(|| StorageError::RunNotFound(run_id.to_owned()))?;
    if schema_version != i64::from(magi_domain::LIVE_RUN_SCHEMA_VERSION) {
        return Err(StorageError::Corrupt(
            "live run uses an unsupported snapshot schema".to_owned(),
        ));
    }
    let status = parse_live_run_status(&state)?;
    let revision = u64::try_from(revision)
        .map_err(|_| StorageError::Corrupt("live run revision is negative".to_owned()))?;
    let admission_sequence = u64::try_from(admission_sequence)
        .map_err(|_| StorageError::Corrupt("live run admission sequence is invalid".to_owned()))?;
    let model_binding: AcpModelBindingSnapshot = serde_json::from_str(&model_binding_json)?;
    if model_binding.catalog_snapshot_id != row_catalog_snapshot_id
        || model_binding.binding_digest.as_str() != row_binding_digest
    {
        return Err(StorageError::Corrupt(
            "live run binding does not match its immutable columns".to_owned(),
        ));
    }
    let catalog = load_provider_catalog_snapshot_from(connection, &row_catalog_snapshot_id)?
        .ok_or_else(|| {
            StorageError::Corrupt(
                "live run references a missing provider catalog snapshot".to_owned(),
            )
        })?;
    model_binding.validate(&catalog)?;

    let failure = failure_json
        .as_deref()
        .map(serde_json::from_str::<LiveRunFailure>)
        .transpose()?;
    let result = match (result_digest, result_byte_length) {
        (Some(digest), Some(byte_length)) => {
            let content_digest = Digest::from_hex(digest)?;
            let content_byte_length = u64::try_from(byte_length).map_err(|_| {
                StorageError::Corrupt("live run result length is negative".to_owned())
            })?;
            let bytes = verify_object_on_disk(connection, objects_root, &content_digest)?;
            if bytes.len() as u64 != content_byte_length {
                return Err(StorageError::Corrupt(
                    "live run result length does not match its stored object".to_owned(),
                ));
            }
            let result: LiveProviderResultInput = serde_json::from_slice(&bytes)?;
            Some(LiveProviderResult {
                final_text: result.final_text,
                stop_reason: result.stop_reason,
                usage: result.usage,
                content_digest,
                content_byte_length,
            })
        }
        (None, None) => None,
        _ => {
            return Err(StorageError::Corrupt(
                "live run result digest and length must be stored together".to_owned(),
            ));
        }
    };
    if (status == LiveRunStatus::Completed) != result.is_some() {
        return Err(StorageError::Corrupt(
            "completed live runs must have exactly one durable provider result".to_owned(),
        ));
    }
    if status == LiveRunStatus::Completed
        && connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM runs WHERE run_id=?1)",
            [run_id],
            |row| row.get::<_, bool>(0),
        )?
    {
        let aggregate = RunAggregate::restore(load_persistence_state(connection, run_id)?)?;
        let snapshot = aggregate.snapshot(run_event_high_water(connection, run_id)?);
        if verify_stored_decision_dossier(connection, &snapshot)?.is_none() {
            return Err(StorageError::Corrupt(
                "completed deliberation has no verified decision dossier".into(),
            ));
        }
        load_live_dispatches_from(connection, &snapshot)?;
        let expected = canonical_completed_deliberation_result(&snapshot)?;
        let encoded = serde_json::to_vec(&expected)?;
        let actual = result.as_ref().ok_or_else(|| {
            StorageError::Corrupt("completed deliberation has no provider result".into())
        })?;
        if actual.final_text != expected.final_text
            || actual.stop_reason != expected.stop_reason
            || actual.usage != expected.usage
            || actual.content_digest != Digest::from_bytes(&encoded)
            || actual.content_byte_length != encoded.len() as u64
        {
            return Err(StorageError::Integrity(
                "completed provider result differs from its canonical deliberation".into(),
            ));
        }
    }
    if matches!(status, LiveRunStatus::Unknown | LiveRunStatus::Failed) != failure.is_some() {
        return Err(StorageError::Corrupt(
            "failed or unknown live runs must have a durable failure detail".to_owned(),
        ));
    }

    let high_water: i64 = connection.query_row(
        "SELECT COALESCE(MAX(sequence), 0) FROM live_run_events WHERE run_id = ?1",
        [run_id],
        |row| row.get(0),
    )?;
    let high_water_sequence = u64::try_from(high_water)
        .map_err(|_| StorageError::Corrupt("live run event high-water is negative".to_owned()))?;
    let cancellation_row: Option<LiveRunCancellationRow> = connection
        .query_row(
            "SELECT origin_status, accepted_status, accepted_revision, claim_generation, provider_outcome, outcome_revision, event_sequence, accepted_at FROM live_run_cancellation_receipts WHERE run_id = ?1 ORDER BY accepted_revision DESC, rowid DESC LIMIT 1",
            [run_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                ))
            },
        )
        .optional()?;
    let cancellation = cancellation_row
        .map(
            |(
                origin_status,
                accepted_status,
                accepted_revision,
                claim_generation,
                provider_outcome,
                outcome_revision,
                outcome_sequence,
                requested_at,
            )| {
                let origin_status = parse_live_run_status(&origin_status)?;
                let accepted_status = parse_live_run_status(&accepted_status)?;
                let accepted_revision = u64::try_from(accepted_revision).map_err(|_| {
                    StorageError::Corrupt("live run cancellation revision is negative".to_owned())
                })?;
                let claim_generation = u64::try_from(claim_generation).map_err(|_| {
                    StorageError::Corrupt("live run cancellation generation is negative".to_owned())
                })?;
                let provider_outcome = parse_live_run_provider_outcome(&provider_outcome)?;
                let outcome_revision = u64::try_from(outcome_revision).map_err(|_| {
                    StorageError::Corrupt("live run cancellation outcome revision is negative".to_owned())
                })?;
                let outcome_sequence = u64::try_from(outcome_sequence).map_err(|_| {
                    StorageError::Corrupt("live run cancellation event sequence is invalid".to_owned())
                })?;
                if accepted_revision > outcome_revision
                    || outcome_revision > revision
                    || outcome_sequence == 0
                    || outcome_sequence > high_water_sequence
                {
                    return Err(StorageError::Corrupt(
                        "live run cancellation receipt is outside the run event history".to_owned(),
                    ));
                }
                let outcome_event: (String, i64, i64, String, Option<String>) = connection.query_row(
                    "SELECT run_id, run_revision, claim_generation, kind, status FROM live_run_events WHERE sequence = ?1",
                    [to_sql_integer(outcome_sequence)?],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
                )?;
                let event_revision = u64::try_from(outcome_event.1).map_err(|_| {
                    StorageError::Corrupt("live run cancellation event revision is negative".to_owned())
                })?;
                let event_generation = u64::try_from(outcome_event.2).map_err(|_| {
                    StorageError::Corrupt("live run cancellation event generation is negative".to_owned())
                })?;
                if outcome_event.0 != run_id
                    || event_revision != outcome_revision
                    || event_generation < claim_generation
                    || parse_live_run_event_kind(&outcome_event.3)?
                        != LiveRunEventKind::StatusChanged
                {
                    return Err(StorageError::Corrupt(
                        "live run cancellation receipt does not match its outcome event".to_owned(),
                    ));
                }
                let event_status = outcome_event
                    .4
                    .as_deref()
                    .map(parse_live_run_status)
                    .transpose()?;
                let active_origin = matches!(
                    origin_status,
                    LiveRunStatus::Claimed
                        | LiveRunStatus::SessionCreationIntent
                        | LiveRunStatus::Running
                        | LiveRunStatus::Cancelling
                );
                let compatible = match provider_outcome {
                    LiveRunProviderOutcome::NotStarted => {
                        status == LiveRunStatus::Cancelled
                            && event_status == Some(LiveRunStatus::Cancelled)
                            && match origin_status {
                                LiveRunStatus::Queued | LiveRunStatus::Paused => {
                                    accepted_status == LiveRunStatus::Cancelled
                                }
                                LiveRunStatus::Claimed
                                | LiveRunStatus::SessionCreationIntent => {
                                    accepted_status == LiveRunStatus::Cancelling
                                }
                                _ => false,
                            }
                    }
                    LiveRunProviderOutcome::Pending => {
                        active_origin
                            && accepted_status == LiveRunStatus::Cancelling
                            && status == LiveRunStatus::Cancelling
                            && event_status == Some(LiveRunStatus::Cancelling)
                    }
                    LiveRunProviderOutcome::Confirmed => {
                        matches!(
                            origin_status,
                            LiveRunStatus::SessionCreationIntent
                                | LiveRunStatus::Running
                                | LiveRunStatus::Cancelling
                        ) && accepted_status == LiveRunStatus::Cancelling
                            && status == LiveRunStatus::Cancelled
                            && event_status == Some(LiveRunStatus::Cancelled)
                    }
                    LiveRunProviderOutcome::Unknown => {
                        active_origin
                            && accepted_status == LiveRunStatus::Cancelling
                            && status == LiveRunStatus::Unknown
                            && event_status == Some(LiveRunStatus::Unknown)
                    }
                };
                if !compatible {
                    return Err(StorageError::Corrupt(
                        "live run cancellation outcome is incompatible with its run history"
                            .to_owned(),
                    ));
                }
                Ok(LiveRunCancellationSnapshot {
                    requested_at,
                    provider_outcome,
                })
            },
        )
        .transpose()?;
    let mut previous_event_generation = connection
        .query_row(
            "SELECT store_generation FROM live_run_events WHERE run_id = ?1 AND sequence <= ?2 ORDER BY sequence DESC LIMIT 1",
            params![run_id, to_sql_integer(after_sequence)?],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
        .map(|generation| {
            let generation = u64::try_from(generation).map_err(|_| {
                StorageError::Corrupt("live run event generation is negative".to_owned())
            })?;
            if generation == 0 || generation > identity.generation {
                return Err(StorageError::Corrupt(
                    "live run event generation is outside the ordered store history".to_owned(),
                ));
            }
            Ok(generation)
        })
        .transpose()?
        .unwrap_or(0);
    let mut statement = connection.prepare(
        "SELECT sequence, store_generation, run_revision, claim_generation, kind, status, text_delta, failure_json, created_at FROM live_run_events WHERE run_id = ?1 AND sequence > ?2 ORDER BY sequence LIMIT ?3",
    )?;
    let rows = statement.query_map(
        params![
            run_id,
            to_sql_integer(after_sequence)?,
            i64::try_from(MAX_EVENT_PAGE + 1).unwrap_or(i64::MAX),
        ],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, String>(8)?,
            ))
        },
    )?;
    let mut events = Vec::new();
    let mut has_more = false;
    for row in rows {
        let (
            sequence,
            event_generation,
            run_revision,
            event_claim_generation,
            kind,
            event_status,
            text_delta,
            event_failure_json,
            created_at,
        ) = row?;
        if events.len() == MAX_EVENT_PAGE {
            has_more = true;
            break;
        }
        let sequence = u64::try_from(sequence)
            .map_err(|_| StorageError::Corrupt("live run event sequence is negative".to_owned()))?;
        let event_generation = u64::try_from(event_generation).map_err(|_| {
            StorageError::Corrupt("live run event generation is negative".to_owned())
        })?;
        if event_generation == 0
            || event_generation > identity.generation
            || event_generation < previous_event_generation
        {
            return Err(StorageError::Corrupt(
                "live run event generation is outside the ordered store history".to_owned(),
            ));
        }
        previous_event_generation = event_generation;
        let run_revision = u64::try_from(run_revision)
            .map_err(|_| StorageError::Corrupt("live run event revision is negative".to_owned()))?;
        let event_claim_generation = u64::try_from(event_claim_generation).map_err(|_| {
            StorageError::Corrupt("live run event claim generation is negative".to_owned())
        })?;
        events.push(LiveRunEvent {
            sequence,
            store_generation: event_generation,
            run_revision,
            claim_generation: event_claim_generation,
            kind: parse_live_run_event_kind(&kind)?,
            status: event_status
                .as_deref()
                .map(parse_live_run_status)
                .transpose()?,
            text_delta,
            failure: event_failure_json
                .as_deref()
                .map(serde_json::from_str::<LiveRunFailure>)
                .transpose()?,
            created_at,
        });
    }
    let page_after_sequence = events
        .last()
        .map(|event| event.sequence)
        .unwrap_or(after_sequence);
    let complete = !has_more && page_after_sequence >= high_water_sequence;
    let queue = load_live_run_queue_projection(connection, admission_sequence, status)?;
    if schema_version != 1 {
        return Err(StorageError::Corrupt(
            "stored live run schema version is invalid".to_owned(),
        ));
    }
    let wire_version = magi_domain::live_run_failure_wire_version(
        failure
            .iter()
            .chain(events.iter().filter_map(|event| event.failure.as_ref())),
    )
    .map_err(|detail| StorageError::Corrupt(detail.to_owned()))?;
    let wire_version = if status == LiveRunStatus::Paused
        || events
            .iter()
            .any(|event| event.status == Some(LiveRunStatus::Paused))
    {
        3
    } else {
        wire_version
    };
    Ok(LiveRunSnapshot {
        schema_version: wire_version,
        store_id: identity.store_id.clone(),
        store_generation: identity.generation,
        run_id: run_id.to_owned(),
        question,
        status,
        revision,
        model_binding,
        queue,
        event_cursor: live_run_event_cursor(
            identity,
            run_id,
            page_after_sequence,
            high_water_sequence,
            complete,
        ),
        events,
        cancellation,
        result,
        failure,
        created_at,
        updated_at,
    })
}

struct LiveRunClaimState {
    status: LiveRunStatus,
    admission_sequence: u64,
    claim_generation: u64,
    claim_owner: String,
    revision: u64,
    question: String,
    model_binding: AcpModelBindingSnapshot,
}

fn load_live_run_claim_state(
    connection: &Connection,
    claim: &LiveRunClaim,
) -> Result<LiveRunClaimState, StorageError> {
    ensure_current_run_lineage(connection, &claim.run_id)?;
    let row: Option<LiveRunAdmissionRow> = connection
        .query_row(
            "SELECT q.state, q.admission_sequence, q.claim_generation, q.claim_owner, r.revision, r.question_text, r.model_binding_json, r.catalog_snapshot_id, r.model_binding_digest FROM live_run_outbox q JOIN live_runs r ON r.run_id = q.run_id WHERE q.run_id = ?1",
            [&claim.run_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                ))
            },
        )
        .optional()?;
    let (
        status,
        admission_sequence,
        claim_generation,
        claim_owner,
        revision,
        question,
        binding_json,
        row_catalog_snapshot_id,
        row_binding_digest,
    ) = row.ok_or_else(|| StorageError::RunNotFound(claim.run_id.clone()))?;
    let claim_owner = claim_owner.ok_or(StorageError::DispatchFenced)?;
    let model_binding: AcpModelBindingSnapshot = serde_json::from_str(&binding_json)?;
    if model_binding.catalog_snapshot_id != row_catalog_snapshot_id
        || model_binding.binding_digest.as_str() != row_binding_digest
    {
        return Err(StorageError::Corrupt(
            "live run binding does not match its immutable columns".to_owned(),
        ));
    }
    let catalog = load_provider_catalog_snapshot_from(connection, &row_catalog_snapshot_id)?
        .ok_or_else(|| {
            StorageError::Corrupt(
                "live run references a missing provider catalog snapshot".to_owned(),
            )
        })?;
    model_binding.validate(&catalog)?;
    Ok(LiveRunClaimState {
        status: parse_live_run_status(&status)?,
        admission_sequence: u64::try_from(admission_sequence).map_err(|_| {
            StorageError::Corrupt("live run admission sequence is invalid".to_owned())
        })?,
        claim_generation: u64::try_from(claim_generation).map_err(|_| {
            StorageError::Corrupt("live run claim generation is negative".to_owned())
        })?,
        claim_owner,
        revision: u64::try_from(revision)
            .map_err(|_| StorageError::Corrupt("live run revision is negative".to_owned()))?,
        question,
        model_binding,
    })
}

fn ensure_live_run_claim(
    current: &LiveRunClaimState,
    claim: &LiveRunClaim,
    expected: LiveRunStatus,
) -> Result<(), StorageError> {
    if current.admission_sequence != claim.admission_sequence
        || current.claim_generation != claim.claim_generation
        || current.claim_owner != claim.claim_owner
        || current.question != claim.question
        || current.model_binding != claim.model_binding
    {
        return Err(StorageError::DispatchFenced);
    }
    if current.status != expected {
        return Err(StorageError::DispatchStateConflict {
            expected: expected.as_str().to_owned(),
            actual: current.status.as_str().to_owned(),
        });
    }
    Ok(())
}

fn ensure_active_live_run_claim(
    current: &LiveRunClaimState,
    claim: &LiveRunClaim,
) -> Result<(), StorageError> {
    if !matches!(
        current.status,
        LiveRunStatus::Claimed | LiveRunStatus::SessionCreationIntent | LiveRunStatus::Running
    ) {
        return Err(StorageError::DispatchStateConflict {
            expected: "claimed, session_creation_intent, or running".to_owned(),
            actual: current.status.as_str().to_owned(),
        });
    }
    ensure_live_run_claim(current, claim, current.status)
}

fn parse_live_run_status(value: &str) -> Result<LiveRunStatus, StorageError> {
    match value {
        "queued" => Ok(LiveRunStatus::Queued),
        "claimed" => Ok(LiveRunStatus::Claimed),
        "session_creation_intent" => Ok(LiveRunStatus::SessionCreationIntent),
        "running" => Ok(LiveRunStatus::Running),
        "paused" => Ok(LiveRunStatus::Paused),
        "cancelling" => Ok(LiveRunStatus::Cancelling),
        "unknown" => Ok(LiveRunStatus::Unknown),
        "completed" => Ok(LiveRunStatus::Completed),
        "cancelled" => Ok(LiveRunStatus::Cancelled),
        "failed" => Ok(LiveRunStatus::Failed),
        _ => Err(StorageError::Corrupt(
            "live run has an unsupported state".to_owned(),
        )),
    }
}

fn parse_live_run_provider_outcome(value: &str) -> Result<LiveRunProviderOutcome, StorageError> {
    match value {
        "not_started" => Ok(LiveRunProviderOutcome::NotStarted),
        "pending" => Ok(LiveRunProviderOutcome::Pending),
        "confirmed" => Ok(LiveRunProviderOutcome::Confirmed),
        "unknown" => Ok(LiveRunProviderOutcome::Unknown),
        _ => Err(StorageError::Corrupt(
            "live run cancellation has an unsupported provider outcome".to_owned(),
        )),
    }
}

fn parse_live_run_event_kind(value: &str) -> Result<LiveRunEventKind, StorageError> {
    match value {
        "status_changed" => Ok(LiveRunEventKind::StatusChanged),
        "text_delta" => Ok(LiveRunEventKind::TextDelta),
        "security_violation" => Ok(LiveRunEventKind::SecurityViolation),
        _ => Err(StorageError::Corrupt(
            "live run contains an unsupported event kind".to_owned(),
        )),
    }
}

fn recover_live_runs(transaction: &Transaction<'_>, storage: &Storage) -> Result<(), StorageError> {
    let identity = &storage.identity;
    let active_runs = {
        let mut statement = transaction.prepare(
            "SELECT run_id, state, claim_generation, revision FROM live_run_outbox JOIN live_runs USING (run_id) WHERE state IN ('claimed', 'session_creation_intent', 'running', 'cancelling') ORDER BY admission_sequence",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };

    for (run_id, state, raw_claim_generation, raw_revision) in active_runs {
        let previous_status = parse_live_run_status(&state)?;
        if previous_status == LiveRunStatus::Running
            && load_optional_deliberation(transaction, &run_id)?.is_some_and(|aggregate| {
                matches!(aggregate.run().status, RunStatus::Completed { .. })
            })
        {
            storage.finalize_completed_deliberation(
                transaction,
                &run_id,
                &epoch_millis_string(),
            )?;
            continue;
        }

        let claim_generation = u64::try_from(raw_claim_generation).map_err(|_| {
            StorageError::Corrupt("live run claim generation is negative".to_owned())
        })?;
        let revision = u64::try_from(raw_revision)
            .map_err(|_| StorageError::Corrupt("live run revision is negative".to_owned()))?;
        let next_claim_generation = claim_generation.checked_add(1).ok_or_else(|| {
            StorageError::Integrity("live run claim generation overflow".to_owned())
        })?;
        let next_revision = revision
            .checked_add(1)
            .ok_or_else(|| StorageError::Integrity("live run revision overflow".to_owned()))?;
        let updated_at = epoch_millis_string();
        let (next_status, failure) = match previous_status {
            LiveRunStatus::Claimed => (LiveRunStatus::Queued, None),
            LiveRunStatus::SessionCreationIntent | LiveRunStatus::Running => (
                LiveRunStatus::Unknown,
                Some(LiveRunFailure {
            profile_binding: None,
                    code: "process_restarted".to_owned(),
                    detail:
                        "The application restarted while the provider request state was uncertain."
                            .to_owned(),
                    external_effect_unknown: true,
                }),
            ),
            LiveRunStatus::Cancelling => (
                LiveRunStatus::Unknown,
                Some(LiveRunFailure {
            profile_binding: None,
                    code: "cancellation_unconfirmed".to_owned(),
                    detail: "The application restarted before the provider confirmed that the request had stopped."
                        .to_owned(),
                    external_effect_unknown: true,
                }),
            ),
            _ => continue,
        };
        let updated_outbox = transaction.execute(
            "UPDATE live_run_outbox SET state = ?1, claim_generation = ?2, claim_owner = NULL, updated_at = ?3 WHERE run_id = ?4 AND state = ?5 AND claim_generation = ?6",
            params![
                next_status.as_str(),
                to_sql_integer(next_claim_generation)?,
                updated_at,
                run_id,
                previous_status.as_str(),
                to_sql_integer(claim_generation)?,
            ],
        )?;
        if updated_outbox != 1 {
            return Err(StorageError::Corrupt(
                "live run changed during startup recovery".to_owned(),
            ));
        }
        let failure_json = failure.as_ref().map(serde_json::to_string).transpose()?;
        let updated_run = transaction.execute(
            "UPDATE live_runs SET revision = ?1, failure_json = ?2, updated_at = ?3 WHERE run_id = ?4 AND revision = ?5",
            params![
                to_sql_integer(next_revision)?,
                failure_json,
                updated_at,
                run_id,
                to_sql_integer(revision)?,
            ],
        )?;
        if updated_run != 1 {
            return Err(StorageError::Corrupt(
                "live run changed during startup recovery".to_owned(),
            ));
        }
        let outcome_event_sequence = insert_live_run_event(
            transaction,
            identity,
            LiveRunEventInput {
                run_id: &run_id,
                revision: next_revision,
                claim_generation: next_claim_generation,
                kind: LiveRunEventKind::StatusChanged,
                status: Some(next_status),
                text_delta: None,
                failure: failure.as_ref(),
                created_at: &updated_at,
            },
        )?;
        if previous_status == LiveRunStatus::Cancelling {
            let updated_receipts = transaction.execute(
                "UPDATE live_run_cancellation_receipts SET provider_outcome = 'unknown', outcome_revision = ?1, event_sequence = ?2 WHERE run_id = ?3 AND claim_generation = ?4 AND provider_outcome = 'pending'",
                params![
                    to_sql_integer(next_revision)?,
                    to_sql_integer(outcome_event_sequence)?,
                    run_id,
                    to_sql_integer(claim_generation)?,
                ],
            )?;
            if updated_receipts == 0 {
                return Err(StorageError::Corrupt(
                    "recovered cancellation has no pending durable receipt".to_owned(),
                ));
            }
        }
    }
    Ok(())
}

fn validate_text(path: &str, value: &str, maximum: usize) -> Result<(), StorageError> {
    if value.trim().is_empty() || value.len() > maximum || value.chars().any(char::is_control) {
        return Err(StorageError::Corrupt(format!(
            "{path} must be non-empty, at most {maximum} UTF-8 bytes, and contain no control characters"
        )));
    }
    Ok(())
}

fn verify_object_on_disk(
    connection: &Connection,
    objects_root: &Path,
    digest: &Digest,
) -> Result<Vec<u8>, StorageError> {
    if !digest.is_valid() {
        return Err(StorageError::Corrupt("invalid object digest".to_owned()));
    }
    let has_unavailable:bool=connection.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='unavailable_objects')",[],|row|row.get(0))?;
    if has_unavailable
        && connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM unavailable_objects WHERE digest=?1)",
            [digest.as_str()],
            |row| row.get::<_, bool>(0),
        )?
    {
        return Err(StorageError::MissingObject(digest.to_string()));
    }
    let expected_length: i64 = connection
        .query_row(
            "SELECT byte_length FROM content_objects WHERE digest = ?1",
            [digest.as_str()],
            |row| row.get(0),
        )
        .optional()?
        .ok_or_else(|| StorageError::MissingObject(digest.to_string()))?;
    if expected_length < 0 || expected_length as u64 > MAX_OBJECT_BYTES {
        return Err(StorageError::Corrupt(format!(
            "object {digest} has an invalid stored length"
        )));
    }
    let hex = digest.as_str();
    reject_symlink_if_present(objects_root)?;
    reject_symlink_if_present(&objects_root.join(&hex[..2]))?;
    let path = objects_root.join(&hex[..2]).join(&hex[2..]);
    let metadata = fs::symlink_metadata(&path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            StorageError::MissingObject(digest.to_string())
        } else {
            StorageError::Io(error)
        }
    })?;
    if !metadata.file_type().is_file() {
        return Err(StorageError::Corrupt(format!(
            "object {digest} is not a regular file"
        )));
    }
    let bytes = read_bounded_file(&path, MAX_OBJECT_BYTES)?;
    if bytes.len() as u64 != expected_length as u64 || Digest::from_bytes(&bytes) != *digest {
        return Err(StorageError::Integrity(format!(
            "content object {digest} failed verification"
        )));
    }
    Ok(bytes)
}

fn command_kind_name(kind: CommandKind) -> &'static str {
    match kind {
        CommandKind::CreateRun => "create_run",
        CommandKind::ConfirmRun => "confirm_run",
        CommandKind::PauseRun => "pause_run",
        CommandKind::ResumeRun => "resume_run",
        CommandKind::CancelRun => "cancel_run",
        CommandKind::AddAssessment => "add_assessment",
        CommandKind::FreezeProposal => "freeze_proposal",
        CommandKind::AddBallot => "add_ballot",
        CommandKind::DeleteRun => "delete_run",
        CommandKind::CreateChildRun => "create_child_run",
    }
}

fn find_duplicate_command(
    transaction: &Transaction<'_>,
    command: &CommandEnvelope,
) -> Result<Option<CommandReceipt>, StorageError> {
    let deleted: Option<(String,String,String,String,String)> = transaction.query_row(
        "SELECT command_kind,target_id,idempotency_key,payload_digest,run_id FROM run_command_tombstones WHERE command_kind <> 'live_admission' AND (command_id=?1 OR (command_kind=?2 AND target_id=?3 AND idempotency_key=?4)) LIMIT 1",
        params![command.command_id,command_kind_name(command.command_kind),command.target_id,command.idempotency_key],
        |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional()?;
    if let Some((kind, target, key, digest, run_id)) = deleted {
        return Err(
            if kind == command_kind_name(command.command_kind)
                && target == command.target_id
                && key == command.idempotency_key
                && digest == command.payload_digest.as_str()
            {
                StorageError::RunNotFound(run_id)
            } else {
                StorageError::IdempotencyConflict
            },
        );
    }
    if transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM run_tombstones WHERE run_id=?1)",
        [&command.target_id],
        |r| r.get::<_, bool>(0),
    )? {
        return Err(StorageError::RunNotFound(command.target_id.clone()));
    }
    let by_id: Option<(String, String, String, String, String)> = transaction
        .query_row(
            "SELECT command_kind, target_id, idempotency_key, payload_digest, receipt_json FROM commands WHERE command_id = ?1",
            [&command.command_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
        )
        .optional()?;
    let by_key: Option<(String, String, String, String, String)> = transaction
        .query_row(
            "SELECT command_id, command_kind, target_id, payload_digest, receipt_json FROM commands WHERE command_kind = ?1 AND target_id = ?2 AND idempotency_key = ?3",
            params![command_kind_name(command.command_kind), command.target_id, command.idempotency_key],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
        )
        .optional()?;
    if let Some((kind, target, key, digest, receipt)) = by_id {
        if kind != command_kind_name(command.command_kind)
            || target != command.target_id
            || key != command.idempotency_key
            || digest != command.payload_digest.as_str()
        {
            return Err(StorageError::IdempotencyConflict);
        }
        if let Some((other_id, other_kind, other_target, other_digest, other_receipt)) = by_key
            && (other_id != command.command_id
                || other_kind != kind
                || other_target != target
                || other_digest != digest
                || other_receipt != receipt)
        {
            return Err(StorageError::IdempotencyConflict);
        }
        return Ok(Some(serde_json::from_str(&receipt)?));
    }
    if let Some((_id, kind, target, digest, receipt)) = by_key {
        if kind != command_kind_name(command.command_kind)
            || target != command.target_id
            || digest != command.payload_digest.as_str()
        {
            return Err(StorageError::IdempotencyConflict);
        }
        return Ok(Some(serde_json::from_str(&receipt)?));
    }
    Ok(None)
}

fn validate_dispatch_batch(
    dispatches: &[PreparedDispatch],
    run: &magi_domain::Run,
    input: &InputSnapshot,
) -> Result<(), StorageError> {
    if dispatches.len() > 1_000 {
        return Err(StorageError::Corrupt(
            "dispatch batch exceeds the 1000-item limit".to_owned(),
        ));
    }
    if dispatches.is_empty() {
        return Ok(());
    }
    let active_stage = match run.status {
        RunStatus::IndependentReview => Some(magi_domain::RunStage::IndependentReview),
        RunStatus::CrossReview => Some(magi_domain::RunStage::CrossReview),
        RunStatus::Synthesis => Some(magi_domain::RunStage::Synthesis),
        RunStatus::Balloting => Some(magi_domain::RunStage::Balloting),
        _ => None,
    }
    .ok_or_else(|| StorageError::Corrupt("dispatches require an active run stage".to_owned()))?;
    let mut dispatch_ids = std::collections::HashSet::new();
    let mut slots = std::collections::HashSet::new();
    for dispatch in dispatches {
        dispatch
            .validate(&run.run_id, &input.input_digest, run.generation)
            .map_err(StorageError::Corrupt)?;
        validate_text("dispatch_id", &dispatch.dispatch_id, 128)?;
        validate_text("slot_id", &dispatch.slot_id, 128)?;
        validate_text("dispatch.created_at", &dispatch.created_at, 64)?;
        if dispatch.stage != active_stage {
            return Err(StorageError::Corrupt(
                "dispatch stage must match the current run stage".to_owned(),
            ));
        }
        if !dispatch_ids.insert(dispatch.dispatch_id.as_str()) {
            return Err(StorageError::Corrupt(
                "dispatch IDs must be unique within a batch".to_owned(),
            ));
        }
        if !slots.insert(dispatch.slot_id.as_str()) {
            return Err(StorageError::Corrupt(
                "a batch cannot dispatch the same stage slot twice".to_owned(),
            ));
        }
    }
    Ok(())
}

fn load_dispatch_batch(
    connection: &Connection,
    command_id: &str,
) -> Result<Option<DispatchBatchReceipt>, StorageError> {
    let row: Option<(String, String, i64, String)> = connection
        .query_row(
            "SELECT batch_key, run_id, dispatch_count, dispatches_digest FROM dispatch_batches WHERE command_id = ?1",
            [command_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()?;
    row.map(|(batch_key, run_id, count, digest)| {
        if count < 0 || count > i64::from(u32::MAX) {
            return Err(StorageError::Corrupt(
                "dispatch batch has an invalid item count".to_owned(),
            ));
        }
        Ok(DispatchBatchReceipt {
            batch_key,
            command_id: command_id.to_owned(),
            run_id,
            dispatch_count: count as u32,
            dispatches_digest: Digest::from_hex(digest)?,
        })
    })
    .transpose()
}

fn insert_dispatch_batch(
    transaction: &Transaction<'_>,
    command: &CommandEnvelope,
    input: DispatchBatchInput<'_>,
) -> Result<DispatchBatchReceipt, StorageError> {
    let DispatchBatchInput {
        run_id,
        batch_key,
        dispatches,
        dispatches_digest,
        created_at,
        store,
    } = input;

    let key_exists: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM dispatch_batches WHERE batch_key = ?1)",
        [batch_key],
        |row| row.get(0),
    )?;
    if key_exists {
        return Err(StorageError::DispatchBatchConflict);
    }
    transaction.execute(
        "INSERT INTO dispatch_batches (batch_key, command_id, run_id, dispatch_count, dispatches_digest, created_at, is_legacy) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0)",
        params![
            batch_key,
            command.command_id,
            run_id,
            dispatches.len() as i64,
            dispatches_digest.as_str(),
            created_at,
        ],
    )?;
    for (index, dispatch) in dispatches.iter().enumerate() {
        let ordinal = u32::try_from(index)
            .map_err(|_| StorageError::Corrupt("dispatch batch ordinal overflow".to_owned()))?;
        transaction.execute(
            "INSERT INTO dispatch_outbox (dispatch_id, batch_key, command_id, batch_ordinal, run_id, slot_id, stage, core_id, attempt_generation, store_generation, input_digest, binding_digest, payload_digest, state, provider_request_id, result_digest, result_byte_length, reconciliation_reference, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, 'prepared', NULL, NULL, NULL, NULL, ?14, ?14)",
            params![
                dispatch.dispatch_id,
                batch_key,
                command.command_id,
                i64::from(ordinal),
                dispatch.run_id,
                dispatch.slot_id,
                run_stage_name(dispatch.stage),
                dispatch.core_id.map(|core| core.wire_name()),
                to_sql_integer(dispatch.attempt_generation)?,
                to_sql_integer(store.generation)?,
                dispatch.input_digest.as_str(),
                dispatch.binding_digest.as_str(),
                dispatch.payload_digest.as_str(),
                dispatch.created_at,
            ],
        )?;
        let identity = dispatch_identity(store, batch_key, ordinal, dispatch);
        record_dispatch_event(
            transaction,
            &identity,
            DispatchEventInput {
                event_kind: "prepared",
                previous_state: None,
                state: DispatchState::Prepared,
                disposition: DispatchTransitionDisposition::Applied,
                provider_request_id: None,
                result_ref: None,
                reconciliation_reference: None,
                created_at: &dispatch.created_at,
            },
        )?;
    }
    Ok(DispatchBatchReceipt {
        batch_key: batch_key.to_owned(),
        command_id: command.command_id.clone(),
        run_id: run_id.to_owned(),
        dispatch_count: dispatches.len() as u32,
        dispatches_digest: dispatches_digest.clone(),
    })
}

fn dispatch_identity(
    store: &StoreIdentity,
    batch_key: &str,
    batch_ordinal: u32,
    dispatch: &PreparedDispatch,
) -> DispatchIdentity {
    DispatchIdentity {
        store_id: store.store_id.clone(),
        store_generation: store.generation,
        dispatch_id: dispatch.dispatch_id.clone(),
        batch_key: batch_key.to_owned(),
        batch_ordinal,
        run_id: dispatch.run_id.clone(),
        run_generation: dispatch.attempt_generation,
        slot_id: dispatch.slot_id.clone(),
        stage: dispatch.stage,
        core_id: dispatch.core_id,
        input_digest: dispatch.input_digest.clone(),
        binding_digest: dispatch.binding_digest.clone(),
        payload_digest: dispatch.payload_digest.clone(),
    }
}

fn load_dispatch_record(
    connection: &Connection,
    store: &StoreIdentity,
    dispatch_id: &str,
) -> Result<Option<DispatchRecord>, StorageError> {
    let row = connection
        .query_row(
            "SELECT dispatch_id, batch_key, batch_ordinal, run_id, attempt_generation, store_generation, slot_id, stage, core_id, input_digest, binding_digest, payload_digest, state, provider_request_id, result_digest, result_byte_length, reconciliation_reference, created_at, updated_at FROM dispatch_outbox WHERE dispatch_id = ?1",
            [dispatch_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, String>(10)?,
                    row.get::<_, String>(11)?,
                    row.get::<_, String>(12)?,
                    row.get::<_, Option<String>>(13)?,
                    row.get::<_, Option<String>>(14)?,
                    row.get::<_, Option<i64>>(15)?,
                    row.get::<_, Option<String>>(16)?,
                    row.get::<_, String>(17)?,
                    row.get::<_, String>(18)?,
                ))
            },
        )
        .optional()?;
    let Some((
        dispatch_id,
        batch_key,
        ordinal,
        run_id,
        run_generation,
        store_generation,
        slot_id,
        stage,
        core_id,
        input_digest,
        binding_digest,
        payload_digest,
        state,
        provider_request_id,
        result_digest,
        result_byte_length,
        reconciliation_reference,
        created_at,
        updated_at,
    )) = row
    else {
        return Ok(None);
    };
    if ordinal < 0 || ordinal > i64::from(u32::MAX) || run_generation < 0 || store_generation < 0 {
        return Err(StorageError::Corrupt(
            "dispatch identity contains a negative or oversized generation".to_owned(),
        ));
    }
    let result_ref = match (result_digest, result_byte_length) {
        (None, None) => None,
        (Some(digest), Some(byte_length)) if byte_length >= 0 => Some(ContentObjectRef {
            digest: Digest::from_hex(digest)?,
            byte_length: byte_length as u64,
        }),
        _ => {
            return Err(StorageError::Corrupt(
                "dispatch result reference is incomplete".to_owned(),
            ));
        }
    };
    Ok(Some(DispatchRecord {
        identity: DispatchIdentity {
            store_id: store.store_id.clone(),
            store_generation: store_generation as u64,
            dispatch_id,
            batch_key,
            batch_ordinal: ordinal as u32,
            run_id,
            run_generation: run_generation as u64,
            slot_id,
            stage: parse_run_stage(&stage)?,
            core_id: core_id.map(|value| parse_core_id(&value)).transpose()?,
            input_digest: Digest::from_hex(input_digest)?,
            binding_digest: Digest::from_hex(binding_digest)?,
            payload_digest: Digest::from_hex(payload_digest)?,
        },
        state: parse_dispatch_state(&state)?,
        provider_request_id,
        result_ref,
        reconciliation_reference,
        created_at,
        updated_at,
    }))
}

fn load_dispatches_for_run(
    connection: &Connection,
    store: &StoreIdentity,
    run_id: &str,
) -> Result<Vec<DispatchRecord>, StorageError> {
    let dispatch_ids = {
        let mut statement = connection.prepare(
            "SELECT dispatch_id FROM dispatch_outbox WHERE run_id = ?1 ORDER BY created_at, batch_key, batch_ordinal",
        )?;
        let rows = statement.query_map([run_id], |row| row.get::<_, String>(0))?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    dispatch_ids
        .iter()
        .map(|dispatch_id| {
            load_dispatch_record(connection, store, dispatch_id)?
                .ok_or_else(|| StorageError::DispatchNotFound(dispatch_id.clone()))
        })
        .collect()
}

fn load_dispatch_event(
    connection: &Connection,
    store: &StoreIdentity,
    sequence: i64,
) -> Result<DispatchEvent, StorageError> {
    let row: Option<DispatchEventRow> = connection
        .query_row(
            "SELECT store_generation, dispatch_id, batch_key, run_id, run_generation, event_kind, previous_state, state, disposition, provider_request_id, result_digest, result_byte_length, reconciliation_reference, created_at FROM dispatch_transition_events WHERE sequence = ?1",
            [sequence],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                    row.get(9)?,
                    row.get(10)?,
                    row.get(11)?,
                    row.get(12)?,
                    row.get(13)?,
                ))
            },
        )
        .optional()?;
    let Some((
        event_store_generation,
        dispatch_id,
        event_batch_key,
        event_run_id,
        event_run_generation,
        event_kind,
        previous_state,
        state,
        disposition,
        provider_request_id,
        result_digest,
        result_byte_length,
        reconciliation_reference,
        created_at,
    )) = row
    else {
        return Err(StorageError::Corrupt(format!(
            "dispatch event {sequence} does not exist"
        )));
    };
    if sequence < 0 || event_store_generation < 0 || event_run_generation < 0 {
        return Err(StorageError::Corrupt(
            "dispatch event contains a negative sequence or generation".to_owned(),
        ));
    }
    let identity = load_dispatch_record(connection, store, &dispatch_id)?
        .ok_or_else(|| StorageError::DispatchNotFound(dispatch_id.clone()))?
        .identity;
    if identity.store_generation != event_store_generation as u64
        || identity.batch_key != event_batch_key
        || identity.run_id != event_run_id
        || identity.run_generation != event_run_generation as u64
    {
        return Err(StorageError::Corrupt(
            "dispatch event identity does not match its immutable dispatch".to_owned(),
        ));
    }
    let result_ref = match (result_digest, result_byte_length) {
        (None, None) => None,
        (Some(digest), Some(byte_length)) if byte_length >= 0 => Some(ContentObjectRef {
            digest: Digest::from_hex(digest)?,
            byte_length: byte_length as u64,
        }),
        _ => {
            return Err(StorageError::Corrupt(
                "dispatch event result reference is incomplete".to_owned(),
            ));
        }
    };
    Ok(DispatchEvent {
        sequence: sequence as u64,
        identity,
        event_kind: parse_dispatch_event_kind(&event_kind)?,
        previous_state: previous_state
            .as_deref()
            .map(parse_dispatch_state)
            .transpose()?,
        state: parse_dispatch_state(&state)?,
        disposition: parse_dispatch_disposition(&disposition)?,
        provider_request_id,
        result_ref,
        reconciliation_reference,
        created_at,
    })
}

fn dispatch_event_high_water(connection: &Connection, run_id: &str) -> Result<u64, StorageError> {
    let sequence: i64 = connection.query_row(
        "SELECT COALESCE(MAX(sequence), 0) FROM dispatch_transition_events WHERE run_id = ?1",
        [run_id],
        |row| row.get(0),
    )?;
    if sequence < 0 {
        return Err(StorageError::Corrupt(
            "dispatch event sequence is negative".to_owned(),
        ));
    }
    Ok(sequence as u64)
}

fn record_dispatch_event(
    transaction: &Transaction<'_>,
    identity: &DispatchIdentity,
    input: DispatchEventInput<'_>,
) -> Result<(), StorageError> {
    let DispatchEventInput {
        event_kind,
        previous_state,
        state,
        disposition,
        provider_request_id,
        result_ref,
        reconciliation_reference,
        created_at,
    } = input;

    transaction.execute(
        "INSERT INTO dispatch_transition_events (store_generation, dispatch_id, batch_key, run_id, run_generation, event_kind, previous_state, state, disposition, provider_request_id, result_digest, result_byte_length, reconciliation_reference, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        params![
            to_sql_integer(identity.store_generation)?,
            identity.dispatch_id,
            identity.batch_key,
            identity.run_id,
            to_sql_integer(identity.run_generation)?,
            event_kind,
            previous_state.map(DispatchState::as_str),
            state.as_str(),
            dispatch_disposition_name(disposition),
            provider_request_id,
            result_ref.map(|value| value.digest.as_str()),
            result_ref.map(|value| to_sql_integer(value.byte_length)).transpose()?,
            reconciliation_reference,
            created_at,
        ],
    )?;
    Ok(())
}

fn verify_content_object_ref(
    transaction: &Transaction<'_>,
    objects_root: &Path,
    content_ref: &ContentObjectRef,
) -> Result<(), StorageError> {
    validate_content_object_ref(content_ref)?;
    let bytes = verify_object_on_disk(transaction, objects_root, &content_ref.digest)?;
    if bytes.len() as u64 != content_ref.byte_length {
        return Err(StorageError::Integrity(format!(
            "content object {} has a mismatched reference length",
            content_ref.digest
        )));
    }
    Ok(())
}

fn validate_content_object_ref(content_ref: &ContentObjectRef) -> Result<(), StorageError> {
    if !content_ref.digest.is_valid() || content_ref.byte_length > MAX_OBJECT_BYTES {
        return Err(StorageError::Corrupt(
            "content object reference is invalid or exceeds the object limit".to_owned(),
        ));
    }
    Ok(())
}

fn ensure_dispatch_identity(
    stored: &DispatchIdentity,
    supplied: &DispatchIdentity,
) -> Result<(), StorageError> {
    if stored == supplied {
        Ok(())
    } else {
        Err(StorageError::DispatchFenced)
    }
}

fn ensure_dispatch_is_current(
    connection: &Connection,
    store: &StoreIdentity,
    identity: &DispatchIdentity,
) -> Result<(), StorageError> {
    if identity.store_id != store.store_id || identity.store_generation != store.generation {
        return Err(StorageError::DispatchFenced);
    }
    let state = load_persistence_state(connection, &identity.run_id)?;
    let active = matches!(
        state.run.status,
        RunStatus::IndependentReview
            | RunStatus::CrossReview
            | RunStatus::Synthesis
            | RunStatus::Balloting
    );
    if state.run.generation != identity.run_generation
        || state.run.input_digest != identity.input_digest
        || state.run.status.stage() != Some(identity.stage)
        || !active
    {
        return Err(StorageError::DispatchFenced);
    }
    Ok(())
}

fn dispatch_state_conflict(expected: DispatchState, actual: DispatchState) -> StorageError {
    StorageError::DispatchStateConflict {
        expected: expected.as_str().to_owned(),
        actual: actual.as_str().to_owned(),
    }
}

fn merge_provider_request_id(
    stored: Option<&str>,
    supplied: Option<&str>,
) -> Result<Option<String>, StorageError> {
    if let (Some(stored), Some(supplied)) = (stored, supplied)
        && stored != supplied
    {
        return Err(StorageError::DispatchStateConflict {
            expected: "the previously recorded provider request ID".to_owned(),
            actual: "a different provider request ID".to_owned(),
        });
    }
    Ok(stored.or(supplied).map(str::to_owned))
}

fn run_stage_name(stage: magi_domain::RunStage) -> &'static str {
    match stage {
        magi_domain::RunStage::IndependentReview => "independent_review",
        magi_domain::RunStage::CrossReview => "cross_review",
        magi_domain::RunStage::Synthesis => "synthesis",
        magi_domain::RunStage::Balloting => "balloting",
    }
}

fn parse_run_stage(value: &str) -> Result<magi_domain::RunStage, StorageError> {
    match value {
        "independent_review" => Ok(magi_domain::RunStage::IndependentReview),
        "cross_review" => Ok(magi_domain::RunStage::CrossReview),
        "synthesis" => Ok(magi_domain::RunStage::Synthesis),
        "balloting" => Ok(magi_domain::RunStage::Balloting),
        _ => Err(StorageError::Corrupt(format!(
            "unknown dispatch stage {value}"
        ))),
    }
}

fn parse_core_id(value: &str) -> Result<magi_domain::CoreId, StorageError> {
    match value {
        "MELCHIOR-1" => Ok(magi_domain::CoreId::Melchior1),
        "BALTHASAR-2" => Ok(magi_domain::CoreId::Balthasar2),
        "CASPER-3" => Ok(magi_domain::CoreId::Casper3),
        _ => Err(StorageError::Corrupt(format!(
            "unknown dispatch core {value}"
        ))),
    }
}

fn parse_dispatch_state(value: &str) -> Result<DispatchState, StorageError> {
    match value {
        "prepared" => Ok(DispatchState::Prepared),
        "dispatched" => Ok(DispatchState::Dispatched),
        "settled" => Ok(DispatchState::Settled),
        "unknown" => Ok(DispatchState::Unknown),
        "aborted_before_dispatch" => Ok(DispatchState::AbortedBeforeDispatch),
        "reconciled_no_effect" => Ok(DispatchState::ReconciledNoEffect),
        _ => Err(StorageError::Corrupt(format!(
            "unknown dispatch state {value}"
        ))),
    }
}

fn parse_dispatch_event_kind(value: &str) -> Result<crate::DispatchEventKind, StorageError> {
    match value {
        "prepared" => Ok(crate::DispatchEventKind::Prepared),
        "dispatched" => Ok(crate::DispatchEventKind::Dispatched),
        "provider_request_identified" => Ok(crate::DispatchEventKind::ProviderRequestIdentified),
        "settled" => Ok(crate::DispatchEventKind::Settled),
        "unknown" => Ok(crate::DispatchEventKind::Unknown),
        "aborted_before_dispatch" => Ok(crate::DispatchEventKind::AbortedBeforeDispatch),
        "reconciled_no_effect" => Ok(crate::DispatchEventKind::ReconciledNoEffect),
        "late_callback_quarantined" => Ok(crate::DispatchEventKind::LateCallbackQuarantined),
        _ => Err(StorageError::Corrupt(format!(
            "unknown dispatch event kind {value}"
        ))),
    }
}

fn dispatch_disposition_name(disposition: DispatchTransitionDisposition) -> &'static str {
    match disposition {
        DispatchTransitionDisposition::Applied => "applied",
        DispatchTransitionDisposition::Reconciled => "reconciled",
        DispatchTransitionDisposition::Quarantined => "quarantined",
    }
}

fn parse_dispatch_disposition(value: &str) -> Result<DispatchTransitionDisposition, StorageError> {
    match value {
        "applied" => Ok(DispatchTransitionDisposition::Applied),
        "reconciled" => Ok(DispatchTransitionDisposition::Reconciled),
        "quarantined" => Ok(DispatchTransitionDisposition::Quarantined),
        _ => Err(StorageError::Corrupt(format!(
            "unknown dispatch transition disposition {value}"
        ))),
    }
}

fn insert_conversation_if_missing(
    transaction: &Transaction<'_>,
    conversation_id: &str,
    title: &str,
    created_at: &str,
) -> Result<(), StorageError> {
    validate_text("conversation_id", conversation_id, 128)?;
    let safe_title = if title.trim().is_empty() {
        "새 심의".to_owned()
    } else {
        title.chars().take(80).collect::<String>()
    };
    transaction.execute(
        "INSERT INTO conversations (conversation_id, title, revision, created_at, updated_at) VALUES (?1, ?2, 0, ?3, ?3) ON CONFLICT(conversation_id) DO NOTHING",
        params![conversation_id, safe_title, created_at],
    )?;
    let deleted: Option<String> = transaction.query_row(
        "SELECT deleted_at FROM conversations WHERE conversation_id = ?1",
        [conversation_id],
        |row| row.get(0),
    )?;
    if deleted.is_some() {
        return Err(StorageError::DeletionBlocked);
    }
    Ok(())
}

fn insert_run(
    transaction: &Transaction<'_>,
    run: &magi_domain::Run,
    input: &InputSnapshot,
) -> Result<(), StorageError> {
    transaction.execute(
        "INSERT INTO runs (run_id, conversation_id, parent_run_id, status, revision, generation, input_digest, run_json, input_json, question_text, tally_json, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, NULL, ?11, ?12)",
        params![
            run.run_id,
            run.conversation_id,
            run.parent_run_id,
            status_name(&run.status),
            to_sql_integer(run.revision)?,
            to_sql_integer(run.generation)?,
            run.input_digest.as_str(),
            serde_json::to_string(run)?,
            serde_json::to_string(input)?,
            input.question.prompt,
            run.created_at,
            run.updated_at,
        ],
    )?;
    Ok(())
}

fn current_run_revision(transaction: &Transaction<'_>, run_id: &str) -> Result<u64, StorageError> {
    let current: Option<i64> = transaction
        .query_row(
            "SELECT revision FROM runs WHERE run_id = ?1 AND deleted_at IS NULL",
            [run_id],
            |row| row.get(0),
        )
        .optional()?;
    current
        .filter(|revision| *revision >= 0)
        .map(|revision| revision as u64)
        .ok_or_else(|| StorageError::RunNotFound(run_id.to_owned()))
}

fn load_optional_deliberation(
    transaction: &Transaction<'_>,
    run_id: &str,
) -> Result<Option<RunAggregate>, StorageError> {
    let exists: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM runs WHERE run_id=?1 AND deleted_at IS NULL)",
        [run_id],
        |row| row.get(0),
    )?;
    if exists {
        Ok(Some(RunAggregate::restore(load_persistence_state(
            transaction,
            run_id,
        )?)?))
    } else {
        Ok(None)
    }
}

fn cancellation_external_possible(
    transaction: &Transaction<'_>,
    run_id: &str,
) -> Result<bool, StorageError> {
    Ok(transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM dispatch_outbox WHERE run_id=?1 AND state IN ('dispatched','unknown')) OR EXISTS(SELECT 1 FROM live_run_outbox WHERE run_id=?1 AND state IN ('session_creation_intent','running','cancelling','unknown')) OR EXISTS(SELECT 1 FROM live_run_dispatch_reservations WHERE run_id=?1 AND state IN ('active','unknown'))",
        [run_id], |row| row.get(0),
    )?)
}

fn settle_cancellation_reservations(
    transaction: &Transaction<'_>,
    identity: &StoreIdentity,
    run_id: &str,
    stopped: Option<bool>,
    at: &str,
) -> Result<(), StorageError> {
    transaction.execute("UPDATE live_run_dispatch_reservations SET state='released',updated_at=?2 WHERE run_id=?1 AND state='reserved'", params![run_id,at])?;
    if let Some(stopped) = stopped {
        transaction.execute("UPDATE live_run_dispatch_reservations SET state=?3,updated_at=?2 WHERE run_id=?1 AND state IN ('active','unknown')", params![run_id,at,if stopped { "released" } else { "unknown" }])?;
    }
    recover_open_dispatches(transaction, identity, run_id, at)?;
    Ok(())
}

fn settle_deliberation_cancel(
    transaction: &Transaction<'_>,
    identity: &StoreIdentity,
    run_id: &str,
    stopped: bool,
    at: &str,
) -> Result<(), StorageError> {
    if let Some(mut aggregate) = load_optional_deliberation(transaction, run_id)? {
        if !matches!(aggregate.run().status, RunStatus::Cancelling) {
            return Err(StorageError::DispatchFenced);
        }
        let revision = aggregate.run().revision;
        aggregate.confirm_cancelled(
            stopped,
            "provider stop could not be confirmed".to_owned(),
            at.to_owned(),
        )?;
        persist_aggregate_transition(
            transaction,
            identity,
            &mut aggregate,
            revision,
            &RunStatus::Cancelling,
        )?;
        settle_cancellation_reservations(transaction, identity, run_id, Some(stopped), at)?;
    }
    Ok(())
}

fn has_unknown_external_dispatch(
    transaction: &Transaction<'_>,
    run_id: &str,
) -> Result<bool, StorageError> {
    let count: i64 = transaction.query_row(
        "SELECT COUNT(*) FROM dispatch_outbox WHERE run_id = ?1 AND state IN ('dispatched', 'unknown')",
        [run_id],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

fn recover_open_dispatches(
    transaction: &Transaction<'_>,
    store: &StoreIdentity,
    run_id: &str,
    at: &str,
) -> Result<(), StorageError> {
    let rows = {
        let mut statement = transaction.prepare(
            "SELECT dispatch_id, state FROM dispatch_outbox WHERE run_id = ?1 AND state IN ('prepared', 'dispatched') ORDER BY batch_key, batch_ordinal",
        )?;
        let rows = statement.query_map([run_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    for (dispatch_id, state_name) in rows {
        let record = load_dispatch_record(transaction, store, &dispatch_id)?
            .ok_or_else(|| StorageError::DispatchNotFound(dispatch_id.clone()))?;
        let previous = parse_dispatch_state(&state_name)?;
        let next = match previous {
            DispatchState::Prepared => DispatchState::AbortedBeforeDispatch,
            DispatchState::Dispatched => DispatchState::Unknown,
            _ => continue,
        };
        let changed = transaction.execute(
            "UPDATE dispatch_outbox SET state = ?1, updated_at = ?2 WHERE dispatch_id = ?3 AND state = ?4 AND store_generation = ?5 AND attempt_generation = ?6",
            params![
                next.as_str(),
                at,
                dispatch_id,
                previous.as_str(),
                to_sql_integer(record.identity.store_generation)?,
                to_sql_integer(record.identity.run_generation)?,
            ],
        )?;
        if changed != 1 {
            return Err(StorageError::Integrity(format!(
                "dispatch {dispatch_id} changed during startup recovery"
            )));
        }
        record_dispatch_event(
            transaction,
            &record.identity,
            DispatchEventInput {
                event_kind: next.as_str(),
                previous_state: Some(previous),
                state: next,
                disposition: DispatchTransitionDisposition::Applied,
                provider_request_id: record.provider_request_id.as_deref(),
                result_ref: None,
                reconciliation_reference: None,
                created_at: at,
            },
        )?;
    }
    Ok(())
}

fn persist_input_snapshots(
    transaction: &Transaction<'_>,
    input: &InputSnapshot,
    created_at: &str,
) -> Result<(), StorageError> {
    input.validate()?;
    let snapshots = [
        (
            "question",
            input.question.question_id.as_str(),
            input.question.digest.as_str(),
            serde_json::to_string(&input.question)?,
        ),
        (
            "run_context",
            input.context_manifest.manifest_id.as_str(),
            input.context_manifest.digest.as_str(),
            serde_json::to_string(&input.context_manifest)?,
        ),
        (
            "role_set",
            input.role_set.role_set_id.as_str(),
            input.role_set.digest.as_str(),
            serde_json::to_string(&input.role_set)?,
        ),
    ];
    for (kind, id, digest, payload) in snapshots {
        transaction.execute(
            "INSERT INTO immutable_snapshots (object_kind, object_id, digest, payload_json, created_at) VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT(object_kind, object_id) DO NOTHING",
            params![kind, id, digest, payload, created_at],
        )?;
        let saved: (String, String) = transaction.query_row(
            "SELECT digest, payload_json FROM immutable_snapshots WHERE object_kind = ?1 AND object_id = ?2",
            params![kind, id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        if saved.0 != digest || saved.1 != payload {
            return Err(StorageError::ImmutableConflict(format!(
                "{kind} snapshot {id} already names different content"
            )));
        }
    }
    Ok(())
}

fn persist_result_rows(
    transaction: &Transaction<'_>,
    state: &RunPersistenceState,
) -> Result<(), StorageError> {
    for assessment in &state.assessments {
        let payload = serde_json::to_string(assessment)?;
        let digest = Digest::from_bytes(&canonical_json(assessment)?);
        let stage = match assessment.stage {
            AssessmentStage::IndependentReview => "independent_review",
            AssessmentStage::CrossReview => "cross_review",
        };
        transaction.execute(
            "INSERT INTO role_assessments (run_id, stage, core_id, attempt_id, attempt_generation, payload_digest, payload_json, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) ON CONFLICT(run_id, stage, core_id) DO NOTHING",
            params![state.run.run_id, stage, assessment.core_id.wire_name(), assessment.attempt_id, to_sql_integer(assessment.attempt_generation)?, digest.as_str(), payload, assessment.created_at],
        )?;
        ensure_immutable_payload(
            transaction,
            "SELECT payload_digest, payload_json FROM role_assessments WHERE run_id = ?1 AND stage = ?2 AND core_id = ?3",
            params![state.run.run_id, stage, assessment.core_id.wire_name()],
            &digest,
            &payload,
            "role assessment",
        )?;
    }

    if let Some(proposal) = &state.proposal {
        let payload = serde_json::to_string(proposal)?;
        transaction.execute(
            "INSERT INTO proposals (run_id, proposal_id, proposal_digest, payload_json, created_at) VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT(run_id) DO NOTHING",
            params![state.run.run_id, proposal.proposal_id, proposal.digest.as_str(), payload, proposal.created_at],
        )?;
        ensure_immutable_payload(
            transaction,
            "SELECT proposal_digest, payload_json FROM proposals WHERE run_id = ?1",
            params![state.run.run_id],
            &proposal.digest,
            &payload,
            "frozen proposal",
        )?;
    }

    for ballot in &state.sealed_ballots {
        let payload = serde_json::to_string(ballot)?;
        let digest = Digest::from_bytes(&canonical_json(ballot)?);
        let vote = vote_name(ballot.vote);
        transaction.execute(
            "INSERT INTO ballots (run_id, core_id, attempt_id, attempt_generation, proposal_id, proposal_digest, vote, payload_digest, payload_json, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10) ON CONFLICT(run_id, core_id) DO NOTHING",
            params![state.run.run_id, ballot.core_id.wire_name(), ballot.attempt_id, to_sql_integer(ballot.attempt_generation)?, ballot.proposal_id, ballot.proposal_digest.as_str(), vote, digest.as_str(), payload, ballot.created_at],
        )?;
        ensure_immutable_payload(
            transaction,
            "SELECT payload_digest, payload_json FROM ballots WHERE run_id = ?1 AND core_id = ?2",
            params![state.run.run_id, ballot.core_id.wire_name()],
            &digest,
            &payload,
            "sealed ballot",
        )?;
    }

    if let Some(tally) = &state.tally {
        let proposal = state.proposal.as_ref().ok_or_else(|| {
            StorageError::Corrupt("completed tally has no frozen proposal".to_owned())
        })?;
        let ballot_payloads = serde_json::to_value(&state.sealed_ballots)?;
        let dossier_payload = serde_json::json!({
            "proposal": proposal,
            "ballots": ballot_payloads,
            "tally": tally,
        });
        let payload = serde_json::to_string(&dossier_payload)?;
        let digest = Digest::from_bytes(&canonical_json(&dossier_payload)?);
        transaction.execute(
            "UPDATE ballots SET revealed_at = ?1 WHERE run_id = ?2 AND revealed_at IS NULL",
            params![state.run.updated_at, state.run.run_id],
        )?;
        transaction.execute(
            "INSERT INTO decision_dossiers (run_id, payload_digest, payload_json, created_at) VALUES (?1, ?2, ?3, ?4) ON CONFLICT(run_id) DO NOTHING",
            params![state.run.run_id, digest.as_str(), payload, state.run.updated_at],
        )?;
        ensure_immutable_payload(
            transaction,
            "SELECT payload_digest, payload_json FROM decision_dossiers WHERE run_id = ?1",
            params![state.run.run_id],
            &digest,
            &payload,
            "decision dossier",
        )?;
    }
    Ok(())
}

fn ensure_immutable_payload<P: rusqlite::Params>(
    transaction: &Transaction<'_>,
    query: &str,
    parameters: P,
    expected_digest: &Digest,
    expected_payload: &str,
    kind: &str,
) -> Result<(), StorageError> {
    let saved: (String, String) =
        transaction.query_row(query, parameters, |row| Ok((row.get(0)?, row.get(1)?)))?;
    if saved.0 != expected_digest.as_str() || saved.1 != expected_payload {
        return Err(StorageError::ImmutableConflict(format!(
            "stored {kind} cannot be replaced"
        )));
    }
    Ok(())
}

fn vote_name(value: magi_domain::VoteValue) -> &'static str {
    match value {
        magi_domain::VoteValue::Support => "support",
        magi_domain::VoteValue::Oppose => "oppose",
        magi_domain::VoteValue::Abstain => "abstain",
    }
}

fn persist_events(
    transaction: &Transaction<'_>,
    events: &[DomainEvent],
    identity: &StoreIdentity,
) -> Result<Option<EventPosition>, StorageError> {
    let mut last_position = None;
    for event in events {
        let payload = serde_json::to_string(&event.payload)?;
        transaction.execute(
            "INSERT INTO run_events (store_generation, run_id, run_revision, attempt_generation, event_type, payload_json, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![to_sql_integer(identity.generation)?, event.run_id, to_sql_integer(event.run_revision)?, to_sql_integer(event.generation)?, event.event_type.as_str(), payload, event.created_at],
        )?;
        let sequence = transaction.last_insert_rowid();
        if sequence < 0 {
            return Err(StorageError::Integrity(
                "SQLite returned a negative event sequence".to_owned(),
            ));
        }
        last_position = Some(EventPosition {
            store_id: identity.store_id.clone(),
            generation: identity.generation,
            sequence: sequence as u64,
        });
    }
    Ok(last_position)
}

fn latest_event_sequence(transaction: &Transaction<'_>) -> Result<u64, StorageError> {
    let sequence: i64 = transaction.query_row(
        "SELECT COALESCE(MAX(sequence), 0) FROM run_events",
        [],
        |row| row.get(0),
    )?;
    if sequence < 0 {
        return Err(StorageError::Integrity(
            "SQLite event sequence is negative".to_owned(),
        ));
    }
    Ok(sequence as u64)
}

fn run_event_high_water(connection: &Connection, run_id: &str) -> Result<u64, StorageError> {
    let sequence: i64 = connection.query_row(
        "SELECT COALESCE(MAX(sequence), 0) FROM run_events WHERE run_id = ?1",
        [run_id],
        |row| row.get(0),
    )?;
    if sequence < 0 {
        return Err(StorageError::Integrity(
            "run event sequence is negative".to_owned(),
        ));
    }
    Ok(sequence as u64)
}

fn load_store_meta_u64(connection: &Connection, key: &str) -> Result<u64, StorageError> {
    let value: String = connection
        .query_row(
            "SELECT value FROM store_meta WHERE key = ?1",
            [key],
            |row| row.get(0),
        )
        .optional()?
        .ok_or_else(|| StorageError::Corrupt(format!("store metadata key {key} is missing")))?;
    value.parse().map_err(|_| {
        StorageError::Corrupt(format!(
            "store metadata key {key} is not an unsigned integer"
        ))
    })
}

fn bump_history_membership_generation(transaction: &Transaction<'_>) -> Result<(), StorageError> {
    let current = load_store_meta_u64(transaction, "history_membership_generation")?;
    let next = current.checked_add(1).ok_or_else(|| {
        StorageError::Integrity("history membership generation overflow".to_owned())
    })?;
    transaction.execute(
        "UPDATE store_meta SET value = ?1 WHERE key = 'history_membership_generation'",
        [next.to_string()],
    )?;
    Ok(())
}

fn load_capture_manifest_from_db(
    connection: &Connection,
    manifest_id: &str,
) -> Result<Option<SourceCaptureManifest>, StorageError> {
    let row: Option<(String, String, String)> = connection
        .query_row(
            "SELECT manifest_digest, disclosure_state, payload_json FROM source_capture_manifests WHERE manifest_id = ?1",
            [manifest_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    row.map(|(digest, state, payload)| {
        let manifest: SourceCaptureManifest = serde_json::from_str(&payload)?;
        manifest.validate()?;
        let expected_state = match manifest.disclosure_state {
            DisclosureState::Draft => "draft",
            DisclosureState::Approved => "approved",
        };
        if manifest.manifest_id != manifest_id
            || manifest.digest.as_str() != digest
            || expected_state != state
        {
            return Err(StorageError::Corrupt(
                "stored source capture manifest columns do not match its payload".to_owned(),
            ));
        }
        Ok(manifest)
    })
    .transpose()
}

fn latest_source_freshness(
    connection: &Connection,
    run_id: &str,
) -> Result<Vec<SourceFreshnessRecord>, StorageError> {
    let mut statement = connection.prepare(
        "SELECT source_id, captured_digest, observed_digest, status, observed_at_epoch_ms FROM source_freshness_observations WHERE run_id = ?1 AND observation_id IN (SELECT MAX(observation_id) FROM source_freshness_observations WHERE run_id = ?1 GROUP BY source_id) ORDER BY source_id",
    )?;
    let rows = statement.query_map([run_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, i64>(4)?,
        ))
    })?;
    let mut records = Vec::new();
    for row in rows {
        let (source_id, captured_digest, observed_digest, status, observed_at) = row?;
        if observed_at < 0 {
            return Err(StorageError::Corrupt(
                "source freshness observation has a negative timestamp".to_owned(),
            ));
        }
        records.push(SourceFreshnessRecord {
            run_id: run_id.to_owned(),
            source_id,
            captured_digest: Digest::from_hex(captured_digest)?,
            observation: FreshnessObservation {
                status: parse_freshness_status(&status)?,
                observed_at_epoch_ms: observed_at as u64,
                observed_digest: observed_digest.map(Digest::from_hex).transpose()?,
            },
        });
    }
    Ok(records)
}

fn validate_freshness_observation(
    captured_digest: &Digest,
    observation: &FreshnessObservation,
) -> Result<(), StorageError> {
    let valid = match observation.status {
        FreshnessStatus::Unchanged => observation.observed_digest.as_ref() == Some(captured_digest),
        FreshnessStatus::Changed => observation
            .observed_digest
            .as_ref()
            .is_some_and(|observed| observed != captured_digest),
        FreshnessStatus::Missing | FreshnessStatus::Unreadable | FreshnessStatus::Unchecked => {
            observation.observed_digest.is_none()
        }
    };
    if !valid {
        return Err(StorageError::Corrupt(
            "source freshness status and observed digest are inconsistent".to_owned(),
        ));
    }
    Ok(())
}

fn freshness_status_name(status: FreshnessStatus) -> &'static str {
    match status {
        FreshnessStatus::Unchanged => "unchanged",
        FreshnessStatus::Changed => "changed",
        FreshnessStatus::Missing => "missing",
        FreshnessStatus::Unreadable => "unreadable",
        FreshnessStatus::Unchecked => "unchecked",
    }
}

fn parse_freshness_status(value: &str) -> Result<FreshnessStatus, StorageError> {
    match value {
        "unchanged" => Ok(FreshnessStatus::Unchanged),
        "changed" => Ok(FreshnessStatus::Changed),
        "missing" => Ok(FreshnessStatus::Missing),
        "unreadable" => Ok(FreshnessStatus::Unreadable),
        "unchecked" => Ok(FreshnessStatus::Unchecked),
        _ => Err(StorageError::Corrupt(format!(
            "unknown source freshness status {value}"
        ))),
    }
}

fn verify_stored_decision_dossier(
    connection: &Connection,
    snapshot: &RunSnapshot,
) -> Result<Option<Digest>, StorageError> {
    let stored: Option<(String, String)> = connection
        .query_row(
            "SELECT payload_digest, payload_json FROM decision_dossiers WHERE run_id = ?1",
            [&snapshot.run.run_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    match (snapshot.tally.as_ref(), stored) {
        (None, None) => Ok(None),
        (None, Some(_)) => Err(StorageError::Corrupt(
            "run has a decision dossier before a final tally".to_owned(),
        )),
        (Some(_), None) => Err(StorageError::Corrupt(
            "completed run has no durable decision dossier".to_owned(),
        )),
        (Some(_), Some((digest, payload))) => {
            let value: serde_json::Value = serde_json::from_str(&payload)?;
            let calculated = Digest::from_bytes(&canonical_json(&value)?);
            let expected = serde_json::json!({
                "proposal": snapshot.proposal,
                "ballots": snapshot.ballots_revealed,
                "tally": snapshot.tally,
            });
            if digest != calculated.as_str() || value != expected {
                return Err(StorageError::Corrupt(
                    "durable decision dossier does not match the completed run".to_owned(),
                ));
            }
            Ok(Some(calculated))
        }
    }
}

fn refresh_checkpoint(transaction: &Transaction<'_>, run_id: &str) -> Result<(), StorageError> {
    let aggregate = RunAggregate::restore(load_persistence_state(transaction, run_id)?)?;
    let latest_sequence = latest_event_sequence(transaction)?;
    persist_checkpoint(transaction, &aggregate, latest_sequence)?;
    Ok(())
}

fn persist_aggregate_transition(
    transaction: &Transaction<'_>,
    store: &StoreIdentity,
    aggregate: &mut RunAggregate,
    previous_revision: u64,
    previous_status: &RunStatus,
) -> Result<(), StorageError> {
    let state = aggregate.persistence_state();
    state.run.validate()?;
    state.input.validate()?;
    if state.run.revision <= previous_revision {
        return Err(StorageError::Corrupt(
            "internal run transition did not advance its revision".to_owned(),
        ));
    }
    if *previous_status != state.run.status {
        bump_history_membership_generation(transaction)?;
    }
    let changed = transaction.execute(
        "UPDATE runs SET status = ?1, revision = ?2, generation = ?3, run_json = ?4, input_json = ?5, tally_json = ?6, updated_at = ?7 WHERE run_id = ?8 AND revision = ?9 AND deleted_at IS NULL",
        params![
            status_name(&state.run.status),
            to_sql_integer(state.run.revision)?,
            to_sql_integer(state.run.generation)?,
            serde_json::to_string(&state.run)?,
            serde_json::to_string(&state.input)?,
            state.tally.as_ref().map(serde_json::to_string).transpose()?,
            state.run.updated_at,
            state.run.run_id,
            to_sql_integer(previous_revision)?,
        ],
    )?;
    if changed != 1 {
        return Err(StorageError::RevisionConflict {
            expected: previous_revision,
            actual: current_run_revision(transaction, &state.run.run_id)?,
        });
    }
    transaction.execute(
        "UPDATE conversations SET updated_at = ?1, revision = revision + 1 WHERE conversation_id = ?2 AND deleted_at IS NULL",
        params![state.run.updated_at, state.run.conversation_id],
    )?;
    persist_result_rows(transaction, &state)?;
    let events = aggregate.events().to_vec();
    persist_events(transaction, &events, store)?;
    let latest_sequence = latest_event_sequence(transaction)?;
    persist_checkpoint(transaction, aggregate, latest_sequence)?;
    Ok(())
}

fn persist_checkpoint(
    transaction: &Transaction<'_>,
    aggregate: &RunAggregate,
    latest_sequence: u64,
) -> Result<(), StorageError> {
    let state = aggregate.persistence_state();
    let mut statement = transaction.prepare(
        "SELECT dispatch_id FROM dispatch_outbox WHERE run_id = ?1 AND state IN ('prepared', 'dispatched', 'unknown') ORDER BY created_at, dispatch_id",
    )?;
    let rows = statement.query_map([&state.run.run_id], |row| row.get::<_, String>(0))?;
    let pending_dispatch_ids = rows.collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    let checkpoint = aggregate.checkpoint(latest_sequence, pending_dispatch_ids);
    let payload = serde_json::to_string(&checkpoint)?;
    transaction.execute(
        "INSERT INTO run_checkpoints (run_id, generation, revision, latest_event_sequence, checkpoint_json, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6) ON CONFLICT(run_id) DO UPDATE SET generation = excluded.generation, revision = excluded.revision, latest_event_sequence = excluded.latest_event_sequence, checkpoint_json = excluded.checkpoint_json, updated_at = excluded.updated_at",
        params![state.run.run_id, to_sql_integer(state.run.generation)?, to_sql_integer(state.run.revision)?, to_sql_integer(latest_sequence)?, payload, state.run.updated_at],
    )?;
    Ok(())
}

fn validate_clarification_parent(
    connection: &Connection,
    parent: &crate::ClarificationParentReference,
) -> Result<RunPersistenceState, StorageError> {
    let state = load_persistence_state(connection, &parent.run_id)?;
    if state.run.revision != parent.revision
        || state.run.generation != parent.generation
        || state.run.input_digest != parent.input_digest
        || !matches!(
            state.run.status,
            RunStatus::Paused {
                reason: magi_domain::PauseReason::NeedsInput,
                ..
            }
        )
        || !state.essential_input_pending
        || state.external_effect_unknown
    {
        return Err(StorageError::DispatchFenced);
    }
    let common: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM live_run_outbox o JOIN live_runs l USING(run_id) WHERE o.run_id=?1 AND o.state='paused' AND o.claim_owner IS NULL AND l.result_digest IS NULL AND l.failure_json IS NULL)",
        [&parent.run_id], |row| row.get(0),
    )?;
    let unresolved: u64 = connection.query_row(
        "SELECT count(*) FROM dispatch_outbox WHERE run_id=?1 AND state IN ('prepared','dispatched','unknown')",
        [&parent.run_id], |row| row.get(0),
    )?;
    let aggregate = RunAggregate::restore(state.clone())?;
    let slots = load_live_dispatches_from(connection, &aggregate.snapshot(0))?;
    let mut released = false;
    let mut settled = 0usize;
    for slot in &slots {
        match slot.state {
            crate::LiveDispatchState::Settled if !released && slot.result_ref.is_some() => {
                settled += 1
            }
            crate::LiveDispatchState::Released if slot.result_ref.is_none() => released = true,
            _ => return Err(StorageError::DispatchFenced),
        }
    }
    if !common
        || unresolved != 0
        || slots.len() != usize::from(LIVE_RUN_DISPATCH_CAPACITY)
        || settled == 0
        || settled != state.assessments.len()
        || state.proposal.is_some()
        || !state.sealed_ballots.is_empty()
    {
        return Err(StorageError::DispatchFenced);
    }
    Ok(state)
}

fn reject_clarification_draft_downcast(
    connection: &Connection,
    intent: &crate::AdmissionRequestIntent,
) -> Result<(), StorageError> {
    if let Some(draft_id) = &intent.request_provenance.context_draft_id {
        let owned: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM clarification_drafts WHERE draft_id=?1 UNION ALL SELECT 1 FROM clarification_capture_owners WHERE draft_id=?1)",
            [draft_id], |row| row.get(0),
        )?;
        if owned {
            return Err(StorageError::DispatchFenced);
        }
    }
    Ok(())
}

fn load_clarification_draft_from_db(
    connection: &Connection,
    expected: &crate::AdmissionExecutionAuthority,
    draft_id: &str,
) -> Result<crate::ClarificationDraft, StorageError> {
    validate_expected_execution_authority(connection, Some(expected))?;
    let row: (String,u64,u64,String,String,u64,String,u64,u64,String) = connection.query_row(
        "SELECT c.parent_run_id,c.parent_revision,c.parent_generation,c.parent_input_digest,c.lineage_id,c.store_generation,c.question_text,d.revision,d.updated_at_epoch_ms,m.payload_json FROM clarification_drafts c JOIN context_drafts d ON d.draft_id=c.draft_id JOIN source_capture_manifests m ON m.manifest_id=d.manifest_id WHERE c.draft_id=?1",
        [draft_id], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?,row.get(6)?,row.get(7)?,row.get(8)?,row.get(9)?)),
    ).optional()?.ok_or(StorageError::DispatchFenced)?;
    if row.4 != expected.lineage_id || row.5 > expected.store_generation {
        return Err(StorageError::DispatchFenced);
    }
    let parent = crate::ClarificationParentReference {
        run_id: row.0,
        revision: row.1,
        generation: row.2,
        input_digest: Digest::from_hex(row.3)?,
    };
    validate_clarification_parent(connection, &parent)?;
    validate_live_question(&row.6)?;
    let manifest: SourceCaptureManifest = serde_json::from_str(&row.9)?;
    manifest.validate()?;
    if manifest.disclosure_state != DisclosureState::Draft {
        return Err(StorageError::DispatchFenced);
    }
    Ok(crate::ClarificationDraft {
        parent,
        context: ContextDraft {
            draft_id: draft_id.to_owned(),
            revision: row.7,
            updated_at_epoch_ms: row.8,
            manifest,
        },
        question: row.6,
        execution_authority: expected.clone(),
    })
}

fn load_persistence_state(
    connection: &Connection,
    run_id: &str,
) -> Result<RunPersistenceState, StorageError> {
    let (run_payload, input_payload, tally_payload): (String, String, Option<String>) = connection
        .query_row(
            "SELECT run_json, input_json, tally_json FROM runs WHERE run_id = ?1 AND deleted_at IS NULL",
            [run_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?
        .ok_or_else(|| StorageError::RunNotFound(run_id.to_owned()))?;
    let run: magi_domain::Run = serde_json::from_str(&run_payload)?;
    let input: InputSnapshot = serde_json::from_str(&input_payload)?;
    let mut assessments = Vec::<RoleAssessment>::new();
    {
        let mut statement = connection.prepare(
            "SELECT payload_json FROM role_assessments WHERE run_id = ?1 ORDER BY CASE stage WHEN 'independent_review' THEN 0 ELSE 1 END, core_id",
        )?;
        let rows = statement.query_map([run_id], |row| row.get::<_, String>(0))?;
        for row in rows {
            assessments.push(serde_json::from_str(&row?)?);
        }
    }
    let proposal_payload: Option<String> = connection
        .query_row(
            "SELECT payload_json FROM proposals WHERE run_id = ?1",
            [run_id],
            |row| row.get(0),
        )
        .optional()?;
    let proposal: Option<ProposalSnapshot> = proposal_payload
        .map(|payload| serde_json::from_str(&payload))
        .transpose()?;
    let mut sealed_ballots = Vec::<Ballot>::new();
    {
        let mut statement = connection.prepare(
            "SELECT payload_json FROM ballots WHERE run_id = ?1 ORDER BY CASE core_id WHEN 'MELCHIOR-1' THEN 0 WHEN 'BALTHASAR-2' THEN 1 ELSE 2 END",
        )?;
        let rows = statement.query_map([run_id], |row| row.get::<_, String>(0))?;
        for row in rows {
            sealed_ballots.push(serde_json::from_str(&row?)?);
        }
    }
    let tally: Option<Tally> = tally_payload
        .map(|payload| serde_json::from_str(&payload))
        .transpose()?;
    let checkpoint_payload: String = connection
        .query_row(
            "SELECT checkpoint_json FROM run_checkpoints WHERE run_id = ?1",
            [run_id],
            |row| row.get(0),
        )
        .optional()?
        .ok_or_else(|| StorageError::Corrupt(format!("run {run_id} has no checkpoint")))?;
    let checkpoint: magi_domain::RunCheckpoint = serde_json::from_str(&checkpoint_payload)?;
    if checkpoint.run_id != run_id
        || checkpoint.revision != run.revision
        || checkpoint.generation != run.generation
        || checkpoint.input_digest != run.input_digest
    {
        return Err(StorageError::Corrupt(format!(
            "run {run_id} checkpoint does not match its durable state"
        )));
    }
    Ok(RunPersistenceState {
        run,
        input,
        assessments,
        proposal,
        sealed_ballots,
        tally,
        essential_input_pending: checkpoint.essential_input_pending,
        external_effect_unknown: checkpoint.external_effect_unknown,
        cancel_resume_stage: if matches!(checkpoint.status, RunStatus::Cancelling) {
            checkpoint.resume_stage
        } else {
            None
        },
        event_drafts: Vec::new(),
    })
}

type FactoryRoleSpec = (
    &'static str,
    &'static str,
    &'static [&'static str],
    &'static [&'static str],
);

fn build_factory_role(
    preset_id: &str,
    core_id: CoreId,
    spec: FactoryRoleSpec,
) -> CoreRoleDefinition {
    CoreRoleDefinition {
        core_id,
        profile_id: format!("{preset_id}.{}", core_id.wire_name()),
        display_name: spec.0.to_owned(),
        review_purpose: spec.1.to_owned(),
        evaluation_criteria: spec.2.iter().map(|item| (*item).to_owned()).collect(),
        falsification_questions: spec.3.iter().map(|item| (*item).to_owned()).collect(),
        response_language: "same_as_question".to_owned(),
    }
}

fn build_factory_preset(
    preset_id: &str,
    display_name: &str,
    specs: [FactoryRoleSpec; 3],
) -> Result<RolePresetRevision, StorageError> {
    RolePresetRevision::new(
        preset_id.to_owned(),
        0,
        display_name.to_owned(),
        [
            build_factory_role(preset_id, CoreId::Melchior1, specs[0]),
            build_factory_role(preset_id, CoreId::Balthasar2, specs[1]),
            build_factory_role(preset_id, CoreId::Casper3, specs[2]),
        ],
    )
    .map_err(StorageError::from)
}

fn factory_role_presets() -> Result<Vec<RolePresetRevision>, StorageError> {
    Ok(vec![
        build_factory_preset(
            "factory.magi.default",
            "MAGI · Original Roles",
            [
                (
                    "Scientist",
                    "Use the original Scientist identity as an evidence and feasibility review lens. Distinguish established facts from inference and test whether the proposal can work under its stated conditions.",
                    &[
                        "Separate supported facts, assumptions, and inference.",
                        "Check feasibility against stated technical and operational conditions.",
                        "Identify evidence or tests that could overturn the conclusion.",
                    ],
                    &[
                        "Which premise, if false, would change the conclusion?",
                        "What observation would falsify the central claim?",
                    ],
                ),
                (
                    "Mother",
                    "Use the original Mother identity as a review lens for long-term sustainability, care, and who bears the costs. The label preserves the source character; it does not assign traits based on gender or parenthood.",
                    &[
                        "Assess long-term maintenance and sustainability costs.",
                        "Identify affected people and how benefits, burdens, and risks are distributed.",
                        "Check whether harm can be prevented, detected, and repaired.",
                    ],
                    &[
                        "Who bears a cost or risk that the proposal leaves unstated?",
                        "What long-term consequence would make this unacceptable?",
                    ],
                ),
                (
                    "Woman",
                    "Use the original Woman identity as a review lens for user autonomy, meaningful alternatives, and the values a choice may displace. The label preserves the source character; it does not assert a gender stereotype.",
                    &[
                        "Check that the proposal matches the user's stated intent and constraints.",
                        "Identify alternatives and the meaningful choice each preserves.",
                        "Make trade-offs and values displaced by the recommendation explicit.",
                    ],
                    &[
                        "Which viable alternative has not been considered?",
                        "What condition would make the user prefer a different option?",
                    ],
                ),
            ],
        )?,
        build_factory_preset(
            "factory.planning.review",
            "Planning Review",
            [
                (
                    "Evidence & Feasibility",
                    "Review whether the plan's goals, assumptions, dependencies, and measures are supported and achievable.",
                    &[
                        "Separate goals from assumptions and measurable outcomes.",
                        "Check that dependencies and capacity support the proposed sequence.",
                        "Identify the smallest evidence that would validate the plan.",
                    ],
                    &[
                        "Which dependency could invalidate the schedule?",
                        "How would progress be measured without relying on activity alone?",
                    ],
                ),
                (
                    "Sustainability & Impact",
                    "Review whether the plan remains sustainable and how its ongoing costs and effects are distributed.",
                    &[
                        "Identify recurring costs, maintenance, and opportunity costs.",
                        "Check who is affected by delays, failure, or success.",
                        "Assess whether the plan can adapt when assumptions change.",
                    ],
                    &[
                        "Which ongoing cost is missing from the plan?",
                        "What change would make this plan harmful or unsustainable?",
                    ],
                ),
                (
                    "Options & Agency",
                    "Review whether the plan preserves meaningful choices and matches the user's priorities.",
                    &[
                        "Compare the proposed path with at least one viable alternative.",
                        "Make irreversible commitments and decision points visible.",
                        "Check that the plan reflects the user's stated priorities.",
                    ],
                    &[
                        "What alternative preserves more flexibility?",
                        "Which unstated preference could reverse the ranking?",
                    ],
                ),
            ],
        )?,
        build_factory_preset(
            "factory.service.architecture",
            "Service Architecture Review",
            [
                (
                    "Correctness & Reliability",
                    "Review whether the service design has clear invariants, failure handling, and verifiable correctness.",
                    &[
                        "Trace ownership and data flow across service boundaries.",
                        "Check failure isolation, recovery, and idempotency.",
                        "Identify deterministic checks for the core invariants.",
                    ],
                    &[
                        "Which partial failure can leave state inconsistent?",
                        "What invariant has no direct verification path?",
                    ],
                ),
                (
                    "Operations & Change",
                    "Review operational burden, security boundaries, and the effect of change over the service lifecycle.",
                    &[
                        "Assess observability, recovery, deployment, and maintenance needs.",
                        "Check least privilege and boundaries around sensitive data.",
                        "Identify migration and rollback conditions.",
                    ],
                    &[
                        "What operational signal would reveal this failure early?",
                        "Which migration or recovery path is missing?",
                    ],
                ),
                (
                    "Alternatives & Evolvability",
                    "Review whether the architecture preserves useful options and can evolve without weakening its contracts.",
                    &[
                        "Compare the design with a simpler alternative against the same constraints.",
                        "Identify coupling that makes likely changes expensive or unsafe.",
                        "Check that public contracts have one canonical owner.",
                    ],
                    &[
                        "What alternative reduces coupling without losing a required guarantee?",
                        "Which likely change would force an incompatible redesign?",
                    ],
                ),
            ],
        )?,
        build_factory_preset(
            "factory.personal.planning",
            "Personal Planning",
            [
                (
                    "Evidence & Practicality",
                    "Review whether a personal plan is grounded in the user's known constraints and practical next actions.",
                    &[
                        "Distinguish known constraints from estimates and guesses.",
                        "Check that steps fit the user's stated time and resources.",
                        "Prefer observable milestones over vague intentions.",
                    ],
                    &[
                        "Which assumption would most change this plan?",
                        "What concrete result would show the first step worked?",
                    ],
                ),
                (
                    "Wellbeing & Sustainability",
                    "Review whether the plan can be sustained without ignoring rest, relationships, or foreseeable costs.",
                    &[
                        "Account for recovery time and recurring effort.",
                        "Identify effects on wellbeing and other responsibilities.",
                        "Check whether the plan has a humane adjustment path.",
                    ],
                    &[
                        "What early sign would show the pace is unsustainable?",
                        "Which responsibility or cost has been left out?",
                    ],
                ),
                (
                    "Autonomy & Possibilities",
                    "Review whether the recommendation reflects the user's own values and leaves room for alternatives.",
                    &[
                        "Keep the user's stated values distinct from assumed preferences.",
                        "Compare plausible paths and the trade-offs each entails.",
                        "Identify reversible steps that preserve future choices.",
                    ],
                    &[
                        "Which alternative better matches a different user priority?",
                        "What new information should trigger a change of plan?",
                    ],
                ),
            ],
        )?,
    ])
}

fn seed_factory_role_presets(connection: &mut Connection) -> Result<(), StorageError> {
    let presets = factory_role_presets()?;
    let initialized_at = epoch_millis_string();
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    for preset in &presets {
        let existing: Option<(i64, String)> = transaction
            .query_row(
                "SELECT revision, source FROM role_preset_heads WHERE preset_id = ?1",
                [&preset.preset_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        if let Some((revision, source)) = existing {
            if revision != 0 || RolePresetSource::parse(&source) != Some(RolePresetSource::Factory)
            {
                return Err(StorageError::Integrity(
                    "factory role preset identity is occupied by an incompatible record".to_owned(),
                ));
            }
            let stored = load_role_preset_revision(&transaction, &preset.preset_id, 0)?
                .ok_or_else(|| {
                    StorageError::Integrity("factory role preset revision is missing".to_owned())
                })?;
            if stored.digest != preset.digest {
                return Err(StorageError::Integrity(
                    "factory role preset content differs from its seed".to_owned(),
                ));
            }
            continue;
        }
        let payload = serde_json::to_string(preset)?;
        transaction.execute(
            "INSERT INTO role_preset_revisions (preset_id, revision, digest, payload_json, created_at) VALUES (?1, 0, ?2, ?3, ?4)",
            params![preset.preset_id, preset.digest.as_str(), payload, initialized_at],
        )?;
        transaction.execute(
            "INSERT INTO role_preset_heads (preset_id, revision, source, updated_at) VALUES (?1, 0, 'factory', ?2)",
            params![preset.preset_id, initialized_at],
        )?;
    }
    transaction.execute(
        "INSERT INTO active_role_preset_selection (singleton, preset_id, selection_revision, updated_at) VALUES (1, 'factory.magi.default', 0, ?1) ON CONFLICT(singleton) DO NOTHING",
        [&initialized_at],
    )?;
    transaction.commit()?;
    Ok(())
}

fn load_role_preset_revision(
    connection: &Connection,
    preset_id: &str,
    revision: u64,
) -> Result<Option<RolePresetRevision>, StorageError> {
    let revision = to_sql_integer(revision)?;
    let row: Option<(i64, String, String)> = connection
        .query_row(
            "SELECT revision, digest, payload_json FROM role_preset_revisions WHERE preset_id = ?1 AND revision = ?2",
            params![preset_id, revision],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    row.map(|(revision, digest, payload)| {
        let revision = u64::try_from(revision)
            .map_err(|_| StorageError::Corrupt("role preset revision is negative".to_owned()))?;
        let preset: RolePresetRevision = serde_json::from_str(&payload)?;
        preset.validate()?;
        if preset.preset_id != preset_id
            || preset.revision != revision
            || preset.digest.as_str() != digest
        {
            return Err(StorageError::Corrupt(
                "role preset payload does not match its stored key or digest".to_owned(),
            ));
        }
        Ok(preset)
    })
    .transpose()
}

fn decode_provider_profile(
    provider_profile_id: &str,
    revision: i64,
    digest: &str,
    runtime_home_id: &str,
    payload: &str,
) -> Result<ProviderProfileRevision, StorageError> {
    let revision = u64::try_from(revision)
        .map_err(|_| StorageError::Corrupt("provider profile revision is negative".to_owned()))?;
    let profile: ProviderProfileRevision = serde_json::from_str(payload)?;
    profile.validate()?;
    if profile.provider_profile_id != provider_profile_id
        || profile.revision != revision
        || profile.digest.as_str() != digest
        || profile.runtime_home_id != runtime_home_id
    {
        return Err(StorageError::Corrupt(
            "provider profile payload does not match its stored key, digest, or home binding"
                .to_owned(),
        ));
    }
    Ok(profile)
}

fn load_live_dispatch_projection_from(
    connection: &Connection,
    run_id: &str,
) -> Result<crate::LiveDispatchProjection, StorageError> {
    read_only_store_identity(connection)?;
    let state = load_persistence_state(connection, run_id)?;
    let aggregate = RunAggregate::restore(state)?;
    let snapshot = aggregate.snapshot(run_event_high_water(connection, run_id)?);
    let dispatches = load_live_dispatches_from(connection, &snapshot)?;
    Ok(crate::LiveDispatchProjection {
        schema_version: 1,
        run_id: snapshot.run.run_id,
        run_revision: snapshot.run.revision,
        input_digest: snapshot.run.input_digest,
        run_generation: snapshot.run.generation,
        dispatches,
    })
}

fn load_live_dispatches_from(
    connection: &Connection,
    snapshot: &RunSnapshot,
) -> Result<Vec<crate::LiveDispatchRecord>, StorageError> {
    let mut statement = connection.prepare("SELECT slot_ordinal,state FROM live_run_dispatch_reservations WHERE run_id=?1 ORDER BY slot_ordinal")?;
    let rows = statement.query_map([&snapshot.run.run_id], |row| {
        Ok((row.get::<_, u8>(0)?, row.get::<_, String>(1)?))
    })?;
    let reservations = rows.collect::<Result<Vec<_>, _>>()?;
    if reservations.is_empty() {
        let live: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM live_runs WHERE run_id=?1)",
            [&snapshot.run.run_id],
            |row| row.get(0),
        )?;
        if live {
            return Err(StorageError::Corrupt(
                "live run dispatch reservations are missing".to_owned(),
            ));
        }
        return Ok(Vec::new());
    }
    if reservations.len() != usize::from(LIVE_RUN_DISPATCH_CAPACITY) {
        return Err(StorageError::Corrupt(
            "live run dispatch reservation count is invalid".to_owned(),
        ));
    }
    reservations
        .into_iter()
        .enumerate()
        .map(|(ordinal, (slot, state))| {
            if usize::from(slot) != ordinal {
                return Err(StorageError::Corrupt(
                    "live run dispatch reservation order is invalid".to_owned(),
                ));
            }
            let state = match state.as_str() {
                "reserved" => crate::LiveDispatchState::Reserved,
                "active" => crate::LiveDispatchState::Active,
                "settled" => crate::LiveDispatchState::Settled,
                "released" => crate::LiveDispatchState::Released,
                "unknown" => crate::LiveDispatchState::Unknown,
                _ => {
                    return Err(StorageError::Corrupt(
                        "live run dispatch reservation state is invalid".to_owned(),
                    ));
                }
            };
            let (stage, core) = match slot {
                0..=2 => (
                    magi_domain::RunStage::IndependentReview,
                    Some(CoreId::ALL[usize::from(slot)]),
                ),
                3..=5 => (
                    magi_domain::RunStage::CrossReview,
                    Some(CoreId::ALL[usize::from(slot - 3)]),
                ),
                6 => (magi_domain::RunStage::Synthesis, None),
                7..=9 => (
                    magi_domain::RunStage::Balloting,
                    Some(CoreId::ALL[usize::from(slot - 7)]),
                ),
                _ => {
                    return Err(StorageError::Corrupt(
                        "live run dispatch slot is invalid".to_owned(),
                    ));
                }
            };
            let binding_core = core.unwrap_or(CoreId::ALL[0]);
            let role = snapshot
                .input
                .role_set
                .roles
                .iter()
                .find(|role| role.core_id == binding_core)
                .ok_or_else(|| {
                    StorageError::Corrupt("live dispatch frozen role is missing".to_owned())
                })?;
            let binding = role.catalog_binding.as_ref().ok_or_else(|| {
                StorageError::Corrupt("live dispatch frozen binding is missing".to_owned())
            })?;
            let catalog =
                load_provider_catalog_snapshot_from(connection, &binding.catalog_snapshot_id)?
                    .ok_or_else(|| {
                        StorageError::Corrupt("live dispatch frozen catalog is missing".to_owned())
                    })?;
            binding.validate_ready(&catalog)?;
            let result_ref = match stage {
                magi_domain::RunStage::IndependentReview | magi_domain::RunStage::CrossReview => {
                    let assessment_stage = if stage == magi_domain::RunStage::IndependentReview {
                        AssessmentStage::IndependentReview
                    } else {
                        AssessmentStage::CrossReview
                    };
                    snapshot
                        .assessments
                        .iter()
                        .find(|assessment| {
                            assessment.stage == assessment_stage && Some(assessment.core_id) == core
                        })
                        .map(|assessment| assessment.attempt_id.clone())
                }
                magi_domain::RunStage::Synthesis => snapshot
                    .proposal
                    .as_ref()
                    .map(|proposal| proposal.proposal_id.clone()),
                magi_domain::RunStage::Balloting => snapshot
                    .ballots_revealed
                    .as_ref()
                    .and_then(|ballots| ballots.iter().find(|ballot| Some(ballot.core_id) == core))
                    .map(|ballot| ballot.attempt_id.clone()),
            };
            if matches!(snapshot.run.status, RunStatus::Completed { .. })
                && (state != crate::LiveDispatchState::Settled || result_ref.is_none())
            {
                return Err(StorageError::Corrupt(
                    "completed live dispatch lacks a settled persisted result".to_owned(),
                ));
            }
            Ok(crate::LiveDispatchRecord {
                slot_ordinal: slot,
                state,
                stage,
                core_id: core,
                binding_core_id: binding_core,
                provider_profile_id: binding.provider_profile_id.clone(),
                profile_revision: binding.profile_revision,
                binding_digest: binding.binding_digest.clone(),
                run_generation: snapshot.run.generation,
                result_ref,
            })
        })
        .collect()
}

fn validate_deliberation_failure_binding(
    snapshot: &RunSnapshot,
    failure: &LiveRunFailure,
) -> Result<(), StorageError> {
    if !failure.validate_profile_binding() {
        return Err(StorageError::Corrupt(
            "invalid authentication failure binding".into(),
        ));
    }
    if let Some(binding) = &failure.profile_binding
        && !snapshot.input.role_set.roles.iter().any(|role| {
            role.catalog_binding.as_ref().is_some_and(|frozen| {
                frozen.provider_profile_id == binding.provider_profile_id
                    && frozen.profile_revision == binding.profile_revision
            })
        })
    {
        return Err(StorageError::Corrupt(
            "authentication failure does not match a frozen deliberation recipient".into(),
        ));
    }
    Ok(())
}

fn load_run_dossier_from(
    connection: &Connection,
    identity: &StoreIdentity,
    run_id: &str,
) -> Result<RunDossier, StorageError> {
    let state = load_persistence_state(connection, run_id)?;
    let aggregate = RunAggregate::restore(state)?;
    let high_water_sequence = run_event_high_water(connection, run_id)?;
    let snapshot = aggregate.snapshot(high_water_sequence);
    let capture_manifest =
        load_capture_manifest_from_db(connection, &snapshot.input.context_manifest.manifest_id)?;
    if capture_manifest
        .as_ref()
        .is_some_and(|manifest| manifest.disclosure_state != DisclosureState::Approved)
    {
        return Err(StorageError::Corrupt(
            "run dossier references an unapproved source capture manifest".to_owned(),
        ));
    }
    let source_freshness = latest_source_freshness(connection, run_id)?;
    let dispatches = load_dispatches_for_run(connection, identity, run_id)?;
    let dispatch_high_water = dispatch_event_high_water(connection, run_id)?;
    let decision_dossier_digest = verify_stored_decision_dossier(connection, &snapshot)?;
    Ok(RunDossier {
        snapshot,
        capture_manifest,
        source_freshness,
        dispatches,
        decision_dossier_digest,
        replay_cursor: RunEventCursor {
            store_id: identity.store_id.clone(),
            store_generation: identity.generation,
            run_id: run_id.to_owned(),
            after_sequence: 0,
            high_water_sequence,
        },
        dispatch_replay_cursor: DispatchEventCursor {
            store_id: identity.store_id.clone(),
            store_generation: identity.generation,
            run_id: run_id.to_owned(),
            after_sequence: 0,
            high_water_sequence: dispatch_high_water,
        },
    })
}

fn run_events_after_from(
    connection: &Connection,
    identity: &StoreIdentity,
    cursor: &RunEventCursor,
    limit: usize,
) -> Result<RunEventPage, StorageError> {
    if cursor.store_id != identity.store_id || cursor.store_generation != identity.generation {
        return Err(StorageError::CursorExpired);
    }
    if cursor.after_sequence > cursor.high_water_sequence {
        return Err(StorageError::Corrupt(
            "run replay cursor is beyond its high-water mark".to_owned(),
        ));
    }
    let limit = limit.min(MAX_EVENT_PAGE);
    let run_exists: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM runs WHERE run_id = ?1 AND deleted_at IS NULL)",
        [&cursor.run_id],
        |row| row.get(0),
    )?;
    if !run_exists {
        return Err(StorageError::RunNotFound(cursor.run_id.clone()));
    }
    if limit == 0 {
        return Ok(RunEventPage {
            events: Vec::new(),
            next_cursor: cursor.clone(),
            complete: cursor.after_sequence >= cursor.high_water_sequence,
        });
    }
    let mut statement = connection.prepare(
            "SELECT sequence, store_generation, run_revision, attempt_generation, payload_json, created_at FROM run_events WHERE run_id = ?1 AND sequence > ?2 AND sequence <= ?3 ORDER BY sequence LIMIT ?4",
        )?;
    let rows = statement.query_map(
        params![
            cursor.run_id,
            to_sql_integer(cursor.after_sequence)?,
            to_sql_integer(cursor.high_water_sequence)?,
            limit as i64 + 1,
        ],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
            ))
        },
    )?;
    let mut events = Vec::new();
    for row in rows {
        let (sequence, stored_generation, revision, generation, payload_json, created_at) = row?;
        if sequence < 0 || stored_generation < 0 || revision < 0 || generation < 0 {
            return Err(StorageError::Corrupt(
                "run replay event contains a negative sequence or revision".to_owned(),
            ));
        }
        let payload: EventPayload = serde_json::from_str(&payload_json)?;
        events.push(StoredEvent {
            position: EventPosition {
                store_id: identity.store_id.clone(),
                generation: stored_generation as u64,
                sequence: sequence as u64,
            },
            event: DomainEvent {
                run_id: cursor.run_id.clone(),
                run_revision: revision as u64,
                generation: generation as u64,
                event_type: payload.kind(),
                payload,
                created_at,
            },
        });
    }
    let has_more = events.len() > limit;
    if has_more {
        events.pop();
    }
    let mut next_cursor = cursor.clone();
    if let Some(last) = events.last() {
        next_cursor.after_sequence = last.position.sequence;
    }
    Ok(RunEventPage {
        events,
        next_cursor,
        complete: !has_more,
    })
}

fn read_only_store_identity(connection: &Connection) -> Result<StoreIdentity, StorageError> {
    let raw_schema_version: i64 =
        connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    let schema_version = u32::try_from(raw_schema_version)
        .map_err(|_| StorageError::Corrupt("database schema version is invalid".to_owned()))?;
    if schema_version > SCHEMA_VERSION {
        return Err(StorageError::FutureSchema {
            found: schema_version,
            supported: SCHEMA_VERSION,
        });
    }

    let v2_schema = verify_migration_ledger(connection, schema_version)?;
    if schema_version > 0 {
        validate_schema_contract(connection, schema_version, v2_schema)?;
    }
    if schema_version >= 2 {
        validate_v2_data_contract(connection)?;
    }
    if schema_version >= 13 {
        validate_admission_request_data_contract(connection)?;
    }
    if schema_version >= 14 {
        validate_admission_lineage_contract(connection)?;
    }

    let store_id: Option<String> = connection
        .query_row(
            "SELECT value FROM store_meta WHERE key = 'store_id'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    let generation: Option<String> = connection
        .query_row(
            "SELECT value FROM store_meta WHERE key = 'store_generation'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    let metadata_schema_version: Option<String> = connection
        .query_row(
            "SELECT value FROM store_meta WHERE key = 'schema_version'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    let store_id =
        store_id.ok_or_else(|| StorageError::Corrupt("store identity is missing".to_owned()))?;
    validate_text("store_id", &store_id, 128)?;
    let generation = generation
        .ok_or_else(|| StorageError::Corrupt("store generation is missing".to_owned()))?
        .parse::<u64>()
        .map_err(|_| StorageError::Corrupt("store generation is invalid".to_owned()))?;
    let metadata_schema_version = metadata_schema_version
        .ok_or_else(|| StorageError::Corrupt("store schema metadata is missing".to_owned()))?
        .parse::<u32>()
        .map_err(|_| StorageError::Corrupt("store schema metadata is invalid".to_owned()))?;
    if metadata_schema_version != schema_version {
        return Err(StorageError::Integrity(
            "store metadata and SQLite schema versions disagree".to_owned(),
        ));
    }
    Ok(StoreIdentity {
        store_id,
        generation,
        schema_version,
    })
}

fn initialize_schema(connection: &mut Connection) -> Result<(), StorageError> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let mut schema_version: u32 =
        transaction.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if schema_version > SCHEMA_VERSION {
        return Err(StorageError::FutureSchema {
            found: schema_version,
            supported: SCHEMA_VERSION,
        });
    }
    let migration_table_exists: bool = transaction.query_row(
        "SELECT EXISTS (SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = 'schema_migrations')",
        [],
        |row| row.get(0),
    )?;
    let other_user_objects: i64 = transaction.query_row(
        "SELECT count(*) FROM sqlite_schema WHERE type IN ('table', 'index', 'trigger', 'view') AND name NOT LIKE 'sqlite_%' AND name <> 'schema_migrations'",
        [],
        |row| row.get(0),
    )?;
    if !migration_table_exists {
        if schema_version != 0 || other_user_objects != 0 {
            return Err(StorageError::Integrity(
                "database schema has no valid migration ledger".to_owned(),
            ));
        }
        transaction.execute_batch(
            "CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY CHECK (version > 0), checksum TEXT NOT NULL, applied_at TEXT NOT NULL);",
        )?;
    } else if schema_version == 0 && other_user_objects != 0 {
        return Err(StorageError::Integrity(
            "database contains schema objects without an applied migration".to_owned(),
        ));
    }
    if schema_version == 0 {
        verify_migration_ledger(&transaction, schema_version)?;
    } else {
        read_only_store_identity(&transaction)?;
    }
    while schema_version < SCHEMA_VERSION {
        let next_version = schema_version + 1;
        let sql = migration_sql(next_version)
            .ok_or_else(|| StorageError::Integrity(format!("missing migration {next_version}")))?;
        transaction.execute_batch(sql)?;
        let checksum = Digest::from_bytes(sql.as_bytes()).to_string();
        transaction.execute(
            "INSERT INTO schema_migrations (version, checksum, applied_at) VALUES (?1, ?2, ?3)",
            params![next_version, checksum, epoch_millis_string()],
        )?;
        transaction.execute_batch(&format!("PRAGMA user_version = {next_version};"))?;
        if next_version == 2 {
            backfill_question_text(&transaction)?;
        }
        schema_version = next_version;
    }
    transaction.commit()?;
    Ok(())
}

fn migration_sql(version: u32) -> Option<&'static str> {
    match version {
        1 => Some(MIGRATION_1),
        2 => Some(MIGRATION_2),
        3 => Some(MIGRATION_3),
        4 => Some(MIGRATION_4),
        5 => Some(MIGRATION_5),
        6 => Some(MIGRATION_6),
        7 => Some(MIGRATION_7),
        8 => Some(MIGRATION_8),
        9 => Some(MIGRATION_9),
        10 => Some(MIGRATION_10),
        11 => Some(MIGRATION_11),
        12 => Some(MIGRATION_12),
        13 => Some(MIGRATION_13),
        14 => Some(MIGRATION_14),
        15 => Some(MIGRATION_15),
        16 => Some(MIGRATION_16),
        _ => None,
    }
}

fn verify_migration_ledger(
    connection: &Connection,
    schema_version: u32,
) -> Result<Option<V2MigrationSchema>, StorageError> {
    let ledger_exists: bool = connection.query_row(
        "SELECT EXISTS (SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = 'schema_migrations')",
        [],
        |row| row.get(0),
    )?;
    if !ledger_exists {
        return if schema_version == 0 {
            Ok(None)
        } else {
            Err(StorageError::Integrity(
                "database schema has no valid migration ledger".to_owned(),
            ))
        };
    }

    let rows = {
        let mut statement = connection
            .prepare("SELECT version, checksum FROM schema_migrations ORDER BY version")?;
        let rows = statement.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    if rows.len() != schema_version as usize {
        return Err(StorageError::Integrity(
            "migration ledger is not contiguous with the SQLite schema version".to_owned(),
        ));
    }

    let mut v2_schema = None;
    for (index, (recorded_version, recorded_checksum)) in rows.into_iter().enumerate() {
        let expected_version = u32::try_from(index + 1)
            .map_err(|_| StorageError::Integrity("migration version overflow".to_owned()))?;
        if recorded_version != i64::from(expected_version) {
            return Err(StorageError::Integrity(
                "migration ledger versions are not contiguous".to_owned(),
            ));
        }
        let sql = migration_sql(expected_version).ok_or_else(|| {
            StorageError::Integrity(format!("missing migration {expected_version}"))
        })?;
        let expected_checksum = Digest::from_bytes(sql.as_bytes()).to_string();
        if expected_version == 2 && recorded_checksum == LEGACY_V2_WEAK_MIGRATION_SHA256 {
            v2_schema = Some(V2MigrationSchema::LegacyWeak);
        } else if recorded_checksum == expected_checksum {
            if expected_version == 2 {
                v2_schema = Some(V2MigrationSchema::Strict);
            }
        } else {
            return Err(StorageError::MigrationChecksum {
                version: expected_version,
            });
        }
    }
    Ok(v2_schema)
}

fn validate_schema_contract(
    connection: &Connection,
    schema_version: u32,
    v2_schema: Option<V2MigrationSchema>,
) -> Result<(), StorageError> {
    if schema_version >= 2 && v2_schema.is_none() {
        return Err(StorageError::Integrity(
            "migration 2 has no recognized schema contract".to_owned(),
        ));
    }
    if v2_schema == Some(V2MigrationSchema::LegacyWeak)
        && Digest::from_bytes(MIGRATION_2_LEGACY_WEAK_CONTRACT.as_bytes()).to_string()
            != LEGACY_V2_WEAK_MIGRATION_SHA256
    {
        return Err(StorageError::Integrity(
            "legacy migration schema contract does not match its recorded digest".to_owned(),
        ));
    }

    let actual = schema_contract_objects(connection)?;
    let expected_connection = Connection::open_in_memory()?;
    expected_connection.pragma_update(None, "foreign_keys", true)?;
    for version in 1..=schema_version {
        let sql = if version == 2 {
            match v2_schema {
                Some(V2MigrationSchema::Strict) => MIGRATION_2,
                Some(V2MigrationSchema::LegacyWeak) => MIGRATION_2_LEGACY_WEAK_CONTRACT,
                None => {
                    return Err(StorageError::Integrity(
                        "migration 2 has no recognized schema contract".to_owned(),
                    ));
                }
            }
        } else {
            migration_sql(version)
                .ok_or_else(|| StorageError::Integrity(format!("missing migration {version}")))?
        };
        expected_connection.execute_batch(sql)?;
    }
    let expected = schema_contract_objects(&expected_connection)?;
    let actual_tables = schema_table_contracts(connection)?;
    let expected_tables = schema_table_contracts(&expected_connection)?;

    if actual != expected || actual_tables != expected_tables {
        return Err(StorageError::Integrity(
            "schema does not match its recorded migration history; stored records were left unchanged".to_owned(),
        ));
    }
    Ok(())
}

type SchemaObjectContract = (String, String, String, Option<Vec<String>>);
type SchemaColumnContract = (i64, String, Vec<String>, i64, Option<Vec<String>>, i64, i64);
type SchemaForeignKeyColumn = (String, Option<String>);
type SchemaForeignKeyContract = (String, Vec<SchemaForeignKeyColumn>, String, String, String);
type SchemaIndexColumn = (i64, i64, Option<String>, i64, Option<String>, i64);
type SchemaIndexContract = (String, i64, String, i64, Vec<SchemaIndexColumn>);
type SchemaTableContract = (
    String,
    Vec<SchemaColumnContract>,
    Vec<SchemaForeignKeyContract>,
    Vec<SchemaIndexContract>,
);

fn schema_contract_objects(
    connection: &Connection,
) -> Result<Vec<SchemaObjectContract>, StorageError> {
    let mut statement = connection.prepare(
        "SELECT type, name, tbl_name, sql FROM sqlite_schema WHERE name NOT GLOB 'sqlite_*' ORDER BY type, name",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Option<String>>(3)?,
        ))
    })?;
    rows.map(|row| {
        let (object_type, name, table_name, sql) = row?;
        Ok((
            object_type,
            name,
            table_name,
            sql.map(|sql| normalize_schema_sql(&sql)).transpose()?,
        ))
    })
    .collect::<Result<Vec<_>, StorageError>>()
}

fn schema_table_contracts(
    connection: &Connection,
) -> Result<Vec<SchemaTableContract>, StorageError> {
    let table_names = {
        let mut statement = connection.prepare(
            "SELECT name FROM sqlite_schema WHERE type = 'table' AND name NOT GLOB 'sqlite_*' ORDER BY name",
        )?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    table_names
        .into_iter()
        .map(|table_name| {
            let quoted_table = quote_sqlite_identifier(&table_name);
            let columns = {
                let mut statement =
                    connection.prepare(&format!("PRAGMA table_xinfo({quoted_table})"))?;
                let rows = statement.query_map([], |row| {
                    let declared_type: String = row.get(2)?;
                    let default_value: Option<String> = row.get(4)?;
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        declared_type,
                        row.get::<_, i64>(3)?,
                        default_value,
                        row.get::<_, i64>(5)?,
                        row.get::<_, i64>(6)?,
                    ))
                })?;
                rows.map(|row| {
                    let (cid, name, declared_type, not_null, default_value, primary_key, hidden) =
                        row?;
                    Ok((
                        cid,
                        name,
                        normalize_schema_sql(&declared_type)?,
                        not_null,
                        default_value
                            .map(|value| normalize_schema_sql(&value))
                            .transpose()?,
                        primary_key,
                        hidden,
                    ))
                })
                .collect::<Result<Vec<_>, StorageError>>()?
            };

            let foreign_keys = {
                let mut groups = BTreeMap::<
                    i64,
                    (
                        String,
                        String,
                        String,
                        String,
                        Vec<(i64, String, Option<String>)>,
                    ),
                >::new();
                let mut statement =
                    connection.prepare(&format!("PRAGMA foreign_key_list({quoted_table})"))?;
                let rows = statement.query_map([], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, String>(5)?,
                        row.get::<_, String>(6)?,
                        row.get::<_, String>(7)?,
                    ))
                })?;
                for row in rows {
                    let (id, sequence, target, from, to, on_update, on_delete, match_kind) = row?;
                    let group = groups.entry(id).or_insert_with(|| {
                        (
                            target.clone(),
                            on_update.clone(),
                            on_delete.clone(),
                            match_kind.clone(),
                            Vec::new(),
                        )
                    });
                    if group.0 != target
                        || group.1 != on_update
                        || group.2 != on_delete
                        || group.3 != match_kind
                    {
                        return Err(StorageError::Integrity(
                            "foreign key metadata is inconsistent".to_owned(),
                        ));
                    }
                    group.4.push((sequence, from, to));
                }
                let mut contracts = groups
                    .into_values()
                    .map(|(target, on_update, on_delete, match_kind, mut columns)| {
                        columns.sort_by_key(|(sequence, _, _)| *sequence);
                        (
                            target,
                            columns
                                .into_iter()
                                .map(|(_, from, to)| (from, to))
                                .collect(),
                            on_update,
                            on_delete,
                            match_kind,
                        )
                    })
                    .collect::<Vec<SchemaForeignKeyContract>>();
                contracts.sort();
                contracts
            };

            let indexes = {
                let index_rows = {
                    let mut statement =
                        connection.prepare(&format!("PRAGMA index_list({quoted_table})"))?;
                    let rows = statement.query_map([], |row| {
                        Ok((
                            row.get::<_, String>(1)?,
                            row.get::<_, i64>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, i64>(4)?,
                        ))
                    })?;
                    rows.collect::<Result<Vec<_>, _>>()?
                };
                let mut indexes = index_rows
                    .into_iter()
                    .map(|(name, unique, origin, partial)| {
                        let quoted_index = quote_sqlite_identifier(&name);
                        let mut statement =
                            connection.prepare(&format!("PRAGMA index_xinfo({quoted_index})"))?;
                        let rows = statement.query_map([], |row| {
                            Ok((
                                row.get::<_, i64>(0)?,
                                row.get::<_, i64>(1)?,
                                row.get::<_, Option<String>>(2)?,
                                row.get::<_, i64>(3)?,
                                row.get::<_, Option<String>>(4)?,
                                row.get::<_, i64>(5)?,
                            ))
                        })?;
                        let columns = rows.collect::<Result<Vec<_>, _>>()?;
                        Ok((
                            if origin == "c" { name } else { String::new() },
                            unique,
                            origin,
                            partial,
                            columns,
                        ))
                    })
                    .collect::<Result<Vec<SchemaIndexContract>, StorageError>>()?;
                indexes.sort();
                indexes
            };

            Ok((table_name, columns, foreign_keys, indexes))
        })
        .collect()
}

fn quote_sqlite_identifier(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('\"', "\"\""))
}

fn normalize_schema_sql(sql: &str) -> Result<Vec<String>, StorageError> {
    const KEYWORDS: &[&str] = &[
        "ABORT",
        "ACTION",
        "ADD",
        "AFTER",
        "ALL",
        "ALTER",
        "ANALYZE",
        "AND",
        "AS",
        "ASC",
        "ATTACH",
        "AUTOINCREMENT",
        "BEFORE",
        "BEGIN",
        "BETWEEN",
        "BY",
        "CASCADE",
        "CASE",
        "CAST",
        "CHECK",
        "COLLATE",
        "COLUMN",
        "COMMIT",
        "CONFLICT",
        "CONSTRAINT",
        "CREATE",
        "CROSS",
        "CURRENT",
        "CURRENT_DATE",
        "CURRENT_TIME",
        "CURRENT_TIMESTAMP",
        "DATABASE",
        "DEFAULT",
        "DEFERRABLE",
        "DEFERRED",
        "DELETE",
        "DESC",
        "DETACH",
        "DISTINCT",
        "DO",
        "DROP",
        "EACH",
        "ELSE",
        "END",
        "ESCAPE",
        "EXCEPT",
        "EXCLUDE",
        "EXCLUSIVE",
        "EXISTS",
        "EXPLAIN",
        "FAIL",
        "FILTER",
        "FIRST",
        "FOLLOWING",
        "FOR",
        "FOREIGN",
        "FROM",
        "FULL",
        "GENERATED",
        "GLOB",
        "GROUP",
        "GROUPS",
        "HAVING",
        "IF",
        "IGNORE",
        "IMMEDIATE",
        "IN",
        "INDEX",
        "INDEXED",
        "INITIALLY",
        "INNER",
        "INSERT",
        "INSTEAD",
        "INTERSECT",
        "INTO",
        "IS",
        "ISNULL",
        "JOIN",
        "KEY",
        "LAST",
        "LEFT",
        "LIKE",
        "LIMIT",
        "MATCH",
        "MATERIALIZED",
        "NATURAL",
        "NO",
        "NOT",
        "NOTHING",
        "NOTNULL",
        "NULL",
        "NULLS",
        "OF",
        "OFFSET",
        "ON",
        "OR",
        "ORDER",
        "OTHERS",
        "OUTER",
        "OVER",
        "PARTITION",
        "PLAN",
        "PRAGMA",
        "PRECEDING",
        "PRIMARY",
        "QUERY",
        "RAISE",
        "RANGE",
        "RECURSIVE",
        "REFERENCES",
        "REGEXP",
        "REINDEX",
        "RELEASE",
        "RENAME",
        "REPLACE",
        "RESTRICT",
        "RETURNING",
        "RIGHT",
        "ROLLBACK",
        "ROW",
        "ROWS",
        "SAVEPOINT",
        "SELECT",
        "SET",
        "TABLE",
        "TEMP",
        "TEMPORARY",
        "THEN",
        "TIES",
        "TO",
        "TRANSACTION",
        "TRIGGER",
        "UNBOUNDED",
        "UNION",
        "UNIQUE",
        "UPDATE",
        "USING",
        "VACUUM",
        "VALUES",
        "VIEW",
        "VIRTUAL",
        "WHEN",
        "WHERE",
        "WINDOW",
        "WITH",
        "WITHOUT",
    ];
    let characters = sql.chars().collect::<Vec<_>>();
    let mut tokens = Vec::new();
    let mut index = 0;
    while index < characters.len() {
        let character = characters[index];
        if character.is_ascii_whitespace() {
            index += 1;
            continue;
        }
        if character == '-' && characters.get(index + 1) == Some(&'-') {
            index += 2;
            while index < characters.len() && characters[index] != '\n' {
                index += 1;
            }
            continue;
        }
        if character == '/' && characters.get(index + 1) == Some(&'*') {
            index += 2;
            while index + 1 < characters.len()
                && !(characters[index] == '*' && characters[index + 1] == '/')
            {
                index += 1;
            }
            if index + 1 >= characters.len() {
                return Err(StorageError::Integrity(
                    "schema definition cannot be validated".to_owned(),
                ));
            }
            index += 2;
            continue;
        }
        if character == '\'' {
            let start = index;
            index += 1;
            let mut closed = false;
            while index < characters.len() {
                if characters[index] == '\'' {
                    if characters.get(index + 1) == Some(&'\'') {
                        index += 2;
                    } else {
                        index += 1;
                        closed = true;
                        break;
                    }
                } else {
                    index += 1;
                }
            }
            if !closed {
                return Err(StorageError::Integrity(
                    "schema definition cannot be validated".to_owned(),
                ));
            }
            tokens.push(format!(
                "string:{}",
                characters[start..index].iter().collect::<String>()
            ));
            continue;
        }
        if matches!(character, '"' | '`' | '[') {
            let start = index;
            let close = if character == '[' { ']' } else { character };
            index += 1;
            let mut closed = false;
            while index < characters.len() {
                if characters[index] == close {
                    if character != '[' && characters.get(index + 1) == Some(&close) {
                        index += 2;
                    } else {
                        index += 1;
                        closed = true;
                        break;
                    }
                } else {
                    index += 1;
                }
            }
            if !closed {
                return Err(StorageError::Integrity(
                    "schema definition cannot be validated".to_owned(),
                ));
            }
            tokens.push(format!(
                "quoted:{}",
                characters[start..index].iter().collect::<String>()
            ));
            continue;
        }
        if character.is_ascii_alphabetic()
            || matches!(character, '_' | '$')
            || !character.is_ascii()
        {
            let start = index;
            index += 1;
            while index < characters.len()
                && (characters[index].is_ascii_alphanumeric()
                    || matches!(characters[index], '_' | '$')
                    || !characters[index].is_ascii())
            {
                index += 1;
            }
            let word = characters[start..index].iter().collect::<String>();
            let token = if KEYWORDS
                .iter()
                .any(|keyword| word.eq_ignore_ascii_case(keyword))
            {
                format!("keyword:{}", word.to_ascii_lowercase())
            } else {
                format!("identifier:{word}")
            };
            tokens.push(token);
            continue;
        }
        if character.is_ascii_digit() {
            let start = index;
            index += 1;
            while index < characters.len()
                && (characters[index].is_ascii_alphanumeric()
                    || matches!(characters[index], '.' | '_'))
            {
                index += 1;
            }
            tokens.push(format!(
                "number:{}",
                characters[start..index].iter().collect::<String>()
            ));
            continue;
        }
        let operator =
            if index + 2 < characters.len() && characters[index..index + 3] == ['-', '>', '>'] {
                Some("->>")
            } else if index + 1 < characters.len() {
                match (character, characters[index + 1]) {
                    ('-', '>') => Some("->"),
                    ('=', '=') => Some("=="),
                    ('!', '=') => Some("!="),
                    ('<', '=') => Some("<="),
                    ('>', '=') => Some(">="),
                    ('<', '>') => Some("<>"),
                    ('|', '|') => Some("||"),
                    ('<', '<') => Some("<<"),
                    ('>', '>') => Some(">>"),
                    _ => None,
                }
            } else {
                None
            };
        if let Some(operator) = operator {
            tokens.push(format!("operator:{operator}"));
            index += operator.chars().count();
        } else {
            tokens.push(format!("symbol:{character}"));
            index += 1;
        }
    }
    Ok(tokens)
}

fn validate_v2_data_contract(connection: &Connection) -> Result<(), StorageError> {
    let history_generation: Option<String> = connection
        .query_row(
            "SELECT value FROM store_meta WHERE key = 'history_membership_generation'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    history_generation
        .ok_or_else(|| {
            StorageError::Integrity(
                "legacy migration data contract is incomplete; stored records were left unchanged"
                    .to_owned(),
            )
        })?
        .parse::<u64>()
        .map_err(|_| {
            StorageError::Integrity(
                "legacy migration data contract is invalid; stored records were left unchanged"
                    .to_owned(),
            )
        })?;

    let mut statement = connection.prepare("SELECT question_text, input_json FROM runs r WHERE NOT (deleted_at IS NOT NULL AND run_json='{}' AND input_json='{}' AND question_text='' AND tally_json IS NULL AND EXISTS(SELECT 1 FROM run_tombstones t WHERE t.run_id=r.run_id AND t.payload_digest=r.input_digest AND t.deleted_at=r.deleted_at))")?;
    let rows = statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (question_text, input_json) = row?;
        let input = serde_json::from_str::<InputSnapshot>(&input_json).map_err(|_| {
            StorageError::Integrity(
                "legacy run backfill cannot be verified; stored records were left unchanged"
                    .to_owned(),
            )
        })?;
        if question_text != input.question.prompt {
            return Err(StorageError::Integrity(
                "legacy run backfill is incomplete; stored records were left unchanged".to_owned(),
            ));
        }
    }
    Ok(())
}

fn backfill_question_text(transaction: &Transaction<'_>) -> Result<(), StorageError> {
    let rows = {
        let mut statement = transaction.prepare("SELECT run_id, input_json FROM runs")?;
        let rows = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    for (run_id, payload) in rows {
        let input: InputSnapshot = serde_json::from_str(&payload)?;
        transaction.execute(
            "UPDATE runs SET question_text = ?1 WHERE run_id = ?2",
            params![input.question.prompt, run_id],
        )?;
    }
    Ok(())
}

fn advance_store_generation(connection: &mut Connection) -> Result<StoreIdentity, StorageError> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let store_id: Option<String> = transaction
        .query_row(
            "SELECT value FROM store_meta WHERE key = 'store_id'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    let generation: Option<String> = transaction
        .query_row(
            "SELECT value FROM store_meta WHERE key = 'store_generation'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    let store_id = store_id.unwrap_or_else(|| Uuid::new_v4().simple().to_string());
    let previous_generation = match generation {
        Some(value) => value.parse::<u64>().map_err(|_| {
            StorageError::Corrupt("store generation is not an unsigned integer".to_owned())
        })?,
        None => 0,
    };
    let generation = previous_generation
        .checked_add(1)
        .ok_or_else(|| StorageError::Integrity("store generation overflow".to_owned()))?;
    let schema_version = SCHEMA_VERSION.to_string();
    for (key, value) in [
        ("store_id", store_id.as_str()),
        ("store_generation", ""),
        ("schema_version", schema_version.as_str()),
    ] {
        transaction.execute(
            "INSERT INTO store_meta (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
    }
    transaction.execute(
        "UPDATE store_meta SET value = ?1 WHERE key = 'store_generation'",
        [generation.to_string()],
    )?;
    transaction.commit()?;
    Ok(StoreIdentity {
        store_id,
        generation,
        schema_version: SCHEMA_VERSION,
    })
}

fn validate_draft_id(draft_id: &str) -> Result<(), StorageError> {
    if draft_id.trim().is_empty() || draft_id.len() > 128 || draft_id.chars().any(char::is_control)
    {
        return Err(StorageError::Corrupt(
            "context draft ID must be a non-empty opaque ID".to_owned(),
        ));
    }
    Ok(())
}

fn validate_provider_id(provider_id: &str) -> Result<(), StorageError> {
    if provider_id.is_empty()
        || provider_id.len() > 128
        || !provider_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(StorageError::Corrupt(
            "provider ID must use ASCII letters, digits, hyphens, or underscores".to_owned(),
        ));
    }
    Ok(())
}

fn to_sql_integer(value: u64) -> Result<i64, StorageError> {
    i64::try_from(value)
        .map_err(|_| StorageError::Integrity("integer exceeds SQLite's signed range".to_owned()))
}

fn epoch_millis_string() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .to_string()
}

fn read_bounded_file(path: &Path, maximum: u64) -> Result<Vec<u8>, StorageError> {
    let file = File::open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > maximum {
        return Err(StorageError::Integrity(
            "object file is not regular or exceeds the configured size limit".to_owned(),
        ));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(maximum.saturating_add(1))
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > maximum {
        return Err(StorageError::Integrity(
            "object file grew beyond the configured size limit".to_owned(),
        ));
    }
    Ok(bytes)
}

fn reject_symlink_if_present(path: &Path) -> Result<(), StorageError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(StorageError::Integrity(
            "storage-owned path must not be a symbolic link".to_owned(),
        )),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(StorageError::Io(error)),
    }
}

fn create_private_dir(path: &Path) -> Result<(), StorageError> {
    fs::create_dir_all(path)?;
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_dir() {
        return Err(StorageError::Integrity(
            "storage-owned directory must not be a symbolic link or special file".to_owned(),
        ));
    }
    set_private_dir_permissions(path)?;
    Ok(())
}

fn private_file(path: &Path) -> Result<File, StorageError> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    set_create_mode(&mut options, 0o600);
    let file = options.open(path)?;
    set_private_file_permissions(path)?;
    Ok(file)
}

fn private_file_new(path: &Path) -> Result<File, StorageError> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    set_create_mode(&mut options, 0o600);
    Ok(options.open(path)?)
}

#[cfg(unix)]
fn set_create_mode(options: &mut OpenOptions, mode: u32) {
    use std::os::unix::fs::OpenOptionsExt;
    options.mode(mode);
}

#[cfg(not(unix))]
fn set_create_mode(_: &mut OpenOptions, _: u32) {}

#[cfg(unix)]
fn set_private_dir_permissions(path: &Path) -> Result<(), StorageError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_private_dir_permissions(_: &Path) -> Result<(), StorageError> {
    Ok(())
}

#[cfg(unix)]
fn set_private_file_permissions(path: &Path) -> Result<(), StorageError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_private_file_permissions(_: &Path) -> Result<(), StorageError> {
    Ok(())
}

struct DispatchTransitionInput<'a> {
    expected_state: DispatchState,
    next_state: DispatchState,
    disposition: DispatchTransitionDisposition,
    provider_request_id: Option<&'a str>,
    result_ref: Option<&'a ContentObjectRef>,
    reconciliation_reference: Option<&'a str>,
    at: &'a str,
    require_current_generation: bool,
}

struct LiveRunEventInput<'a> {
    run_id: &'a str,
    revision: u64,
    claim_generation: u64,
    kind: LiveRunEventKind,
    status: Option<LiveRunStatus>,
    text_delta: Option<&'a str>,
    failure: Option<&'a LiveRunFailure>,
    created_at: &'a str,
}

struct DispatchBatchInput<'a> {
    run_id: &'a str,
    batch_key: &'a str,
    dispatches: &'a [PreparedDispatch],
    dispatches_digest: &'a Digest,
    created_at: &'a str,
    store: &'a StoreIdentity,
}

struct DispatchEventInput<'a> {
    event_kind: &'a str,
    previous_state: Option<DispatchState>,
    state: DispatchState,
    disposition: DispatchTransitionDisposition,
    provider_request_id: Option<&'a str>,
    result_ref: Option<&'a ContentObjectRef>,
    reconciliation_reference: Option<&'a str>,
    created_at: &'a str,
}

type DispatchEventRow = (
    i64,
    String,
    String,
    String,
    i64,
    String,
    Option<String>,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<i64>,
    Option<String>,
    String,
);

type LiveRunAdmissionRow = (
    String,
    i64,
    i64,
    Option<String>,
    i64,
    String,
    String,
    String,
    String,
);

type LiveRunCancellationRow = (String, String, i64, i64, String, i64, i64, String);

type LiveRunSnapshotRow = (
    i64,
    String,
    String,
    i64,
    String,
    String,
    String,
    i64,
    Option<String>,
    Option<i64>,
    Option<String>,
    String,
    String,
);

type ProviderCatalogRow = (
    String,
    String,
    String,
    i64,
    String,
    String,
    String,
    String,
    String,
);

type LiveRunClaimRow = (i64, String, i64, i64, String, String, String, String);

type LiveRunCancelCommandRow = (
    String,
    String,
    String,
    String,
    String,
    i64,
    i64,
    String,
    i64,
    i64,
);

pub type LiveRunCancellationTransition = (
    LiveRunStatus,
    LiveRunStatus,
    u64,
    u64,
    LiveRunChange,
    bool,
    String,
);

#[cfg(test)]
mod profile_authority_storage_tests {
    use super::*;

    #[test]
    fn credential_authority_edit_fences_selection_and_preserves_prior_revision() {
        let root =
            std::env::temp_dir().join(format!("magi-authority-store-{}", uuid::Uuid::new_v4()));
        let storage = Storage::open_or_create(&root).unwrap();
        let mut input = ProviderProfileInput {
            provider_profile_id: "profile-fixture".into(),
            provider_id: "codex-acp".into(),
            display_name: "Fixture".into(),
            account_alias: "Fixture".into(),
            authentication_method: magi_domain::ProviderAuthenticationMethod::LocalSubscription,
            secret_reference: None,
            runtime_home_id: "runtime-fixture".into(),
            credential_home: Some(magi_domain::ProviderCredentialHome {
                authority_id: "authority-fixture".into(),
                canonical_path: "/Users/fixture/.codex".into(),
                device: 1,
                inode: 2,
                credential_store: magi_domain::ProviderCredentialStore::File,
                account_digest: Some(Digest::from_bytes(b"fixture-account")),
            }),
        };
        let first = storage
            .save_provider_profile(&input, None, "2026-10-01T00:00:00Z")
            .unwrap();
        let selected = storage
            .set_active_provider_profile(
                "codex-acp",
                "profile-fixture",
                None,
                "2026-10-01T00:00:00Z",
            )
            .unwrap();
        input.credential_home.as_mut().unwrap().inode = 3;
        let second = storage
            .save_provider_profile(&input, Some(first.revision), "2026-10-01T00:00:01Z")
            .unwrap();
        assert_ne!(second.digest, first.digest);
        assert_eq!(
            storage
                .load_provider_profile_revision("profile-fixture", first.revision)
                .unwrap()
                .unwrap(),
            first
        );
        assert_eq!(
            storage
                .load_active_provider_profile_selection("codex-acp")
                .unwrap()
                .unwrap()
                .selection_revision,
            selected.selection_revision + 1
        );
        drop(storage);
        let reopened = Storage::open_or_create(&root).unwrap();
        assert_eq!(
            reopened
                .load_provider_profile("profile-fixture")
                .unwrap()
                .unwrap(),
            second
        );
        drop(reopened);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
mod read_only_deliberation_tests {
    use super::*;
    use magi_domain::{
        ContextManifest, CoreRoleProfile, ModelBindingSnapshot, QuestionKind, QuestionSnapshot,
        RoleSetSnapshot, Run,
    };

    fn aggregate() -> RunAggregate {
        let roles = CoreId::ALL.map(|core_id| CoreRoleProfile {
            core_id,
            profile_id: format!("role-{}", core_id.wire_name()),
            revision: 1,
            display_name: core_id.wire_name().to_owned(),
            review_purpose: "Review fixture evidence".to_owned(),
            evaluation_criteria: vec!["Evidence".to_owned()],
            falsification_questions: vec!["What disproves the finding?".to_owned()],
            response_language: "en".to_owned(),
            binding: ModelBindingSnapshot {
                provider_profile_id: "provider-fixture".to_owned(),
                revision: 1,
                adapter_id: "adapter-fixture".to_owned(),
                adapter_version: "1".to_owned(),
                adapter_digest: Digest::from_bytes(b"adapter-fixture"),
                model_id: "model-fixture".to_owned(),
                context_window_tokens: None,
                maximum_output_tokens: None,
            },
            catalog_binding: None,
        });
        let input = InputSnapshot::new(
            QuestionSnapshot::new(
                "question-fixture".to_owned(),
                QuestionKind::Answer,
                "Evaluate the fixture".to_owned(),
                vec![],
                vec![],
                vec![],
            )
            .unwrap(),
            ContextManifest::new("manifest-fixture".to_owned(), vec![]).unwrap(),
            RoleSetSnapshot::new("roles-fixture".to_owned(), roles).unwrap(),
            Digest::from_bytes(b"policy-fixture"),
        )
        .unwrap();
        let run = Run::new(
            "run-fixture".to_owned(),
            "conversation-fixture".to_owned(),
            None,
            &input,
            "2026-10-01T00:00:00Z".to_owned(),
        )
        .unwrap();
        RunAggregate::new(run, input).unwrap()
    }

    #[test]
    fn reader_matches_writer_dossier_events_without_mutation_and_rejects_stale_fences() {
        let root = std::env::temp_dir().join(format!("magi-read-only-dossier-{}", Uuid::new_v4()));
        let storage = Storage::open_or_create(&root).unwrap();
        let mut aggregate = aggregate();
        let command = CommandEnvelope {
            command_id: "create-fixture".to_owned(),
            idempotency_key: "create-fixture".to_owned(),
            command_kind: CommandKind::CreateRun,
            target_id: "run-fixture".to_owned(),
            expected_revision: 0,
            payload_digest: aggregate.input().input_digest.clone(),
        };
        storage
            .commit_run(
                &command,
                &mut aggregate,
                "2026-10-01T00:00:00Z",
                Some("Fixture"),
            )
            .unwrap();
        let at = "2026-10-01T00:00:01Z";
        aggregate.request_confirmation(0, at.to_owned()).unwrap();
        aggregate
            .confirm_and_start(aggregate.run().revision, at.to_owned())
            .unwrap();
        for stage in [
            magi_domain::AssessmentStage::IndependentReview,
            magi_domain::AssessmentStage::CrossReview,
        ] {
            for core_id in CoreId::ALL {
                aggregate
                    .accept_assessment(
                        magi_domain::RoleAssessment {
                            schema_version: magi_domain::CONTRACT_SCHEMA_VERSION,
                            run_id: "run-fixture".to_owned(),
                            attempt_id: format!("assessment-{stage:?}-{}", core_id.wire_name()),
                            core_id,
                            stage,
                            input_digest: aggregate.input().input_digest.clone(),
                            attempt_generation: aggregate.run().generation,
                            position_summary: "Fixture finding".to_owned(),
                            claims: vec![magi_domain::Claim {
                                claim_id: format!("claim-{stage:?}-{}", core_id.wire_name()),
                                kind: magi_domain::ClaimKind::Inference,
                                text: "Fixture inference".to_owned(),
                                evidence_refs: vec![],
                                limitations: vec![],
                            }],
                            assumptions: vec![],
                            information_gaps: vec![],
                            counterarguments: vec![],
                            claim_responses: vec![],
                            position_changes: vec![],
                            created_at: at.to_owned(),
                        },
                        at.to_owned(),
                    )
                    .unwrap();
            }
        }
        let input = aggregate.input();
        let proposal = magi_domain::ProposalSnapshot {
            schema_version: magi_domain::CONTRACT_SCHEMA_VERSION,
            proposal_id: "proposal-fixture".to_owned(),
            run_id: "run-fixture".to_owned(),
            question_digest: input.question.digest.clone(),
            context_digest: input.context_manifest.digest.clone(),
            roles_digest: input.role_set.digest.clone(),
            kind: QuestionKind::Answer,
            body: "Fixture proposal".to_owned(),
            claims: vec![],
            conditions: vec![],
            alternatives: vec![],
            open_objections: vec![],
            digest: Digest::from_bytes(b"unsealed"),
            created_at: at.to_owned(),
        }
        .seal()
        .unwrap();
        aggregate
            .freeze_proposal(proposal.clone(), at.to_owned())
            .unwrap();
        for core_id in CoreId::ALL.into_iter().take(2) {
            aggregate
                .accept_ballot(
                    magi_domain::Ballot {
                        schema_version: magi_domain::CONTRACT_SCHEMA_VERSION,
                        run_id: "run-fixture".to_owned(),
                        attempt_id: format!("ballot-{}", core_id.wire_name()),
                        attempt_generation: aggregate.run().generation,
                        core_id,
                        input_digest: aggregate.input().input_digest.clone(),
                        proposal_id: proposal.proposal_id.clone(),
                        proposal_digest: proposal.digest.clone(),
                        vote: magi_domain::VoteValue::Support,
                        rationale: "Private sealed rationale".to_owned(),
                        objection_refs: vec![],
                        created_at: at.to_owned(),
                    },
                    at.to_owned(),
                )
                .unwrap();
        }
        let ballot_command = CommandEnvelope {
            command_id: "ballot-fixture".to_owned(),
            idempotency_key: "ballot-fixture".to_owned(),
            command_kind: CommandKind::AddBallot,
            target_id: "run-fixture".to_owned(),
            expected_revision: 0,
            payload_digest: aggregate.input().input_digest.clone(),
        };
        storage
            .commit_run(&ballot_command, &mut aggregate, at, None)
            .unwrap();
        let before = fs::read(root.join("state/magi.sqlite")).unwrap();
        let database_state = || {
            let connection = storage.connection().unwrap();
            let schema: String = connection
                .query_row(
                    "SELECT group_concat(name || ':' || coalesce(sql, ''), '|') FROM sqlite_master",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            let counts: (i64, i64, i64) = connection.query_row("SELECT (SELECT COUNT(*) FROM runs), (SELECT COUNT(*) FROM run_events), (SELECT COUNT(*) FROM store_meta)", [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).unwrap();
            (schema, counts)
        };
        let before_state = database_state();
        let reader = StorageReader::open_read_only(&root).unwrap();
        let writer_dossier = storage.load_run_dossier("run-fixture").unwrap();
        let dossier = reader.load_run_dossier("run-fixture").unwrap();
        assert_eq!(dossier, writer_dossier);
        assert_eq!(dossier.snapshot.submitted_ballot_count, 2);
        assert!(dossier.snapshot.tally.is_none());
        assert!(dossier.snapshot.ballots_revealed.is_none());
        assert!(
            !serde_json::to_string(&dossier)
                .unwrap()
                .contains("Private sealed rationale")
        );
        let page = reader
            .events_after_for_run(&dossier.replay_cursor, 256)
            .unwrap();
        assert_eq!(
            page,
            storage
                .events_after_for_run(&dossier.replay_cursor, 256)
                .unwrap()
        );
        assert!(!page.events.is_empty());
        assert!(
            !serde_json::to_string(&page)
                .unwrap()
                .contains("Private sealed rationale")
        );
        assert!(page.complete);
        assert_eq!(database_state(), before_state);
        assert_eq!(fs::read(root.join("state/magi.sqlite")).unwrap(), before);
        let mut stale = dossier.replay_cursor.clone();
        stale.store_generation += 1;
        assert!(matches!(
            reader.events_after_for_run(&stale, 256),
            Err(StorageError::CursorExpired)
        ));
        let mut beyond = dossier.replay_cursor;
        beyond.after_sequence = beyond.high_water_sequence + 1;
        assert!(reader.events_after_for_run(&beyond, 256).is_err());
        storage
            .connection()
            .unwrap()
            .execute_batch("DROP TABLE run_events")
            .unwrap();
        assert!(reader.load_run_dossier("run-fixture").is_err());
        assert!(StorageReader::open_read_only(&root).is_err());
        drop(reader);
        drop(storage);
        fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
mod saved_model_selection_tests {
    use super::*;
    use crate::{CoreModelSelectionInput, ProviderModelSelectionInput};

    fn profile(id: &str) -> ProviderProfileInput {
        ProviderProfileInput {
            provider_profile_id: id.into(),
            provider_id: "codex-acp".into(),
            display_name: id.into(),
            account_alias: id.into(),
            authentication_method: magi_domain::ProviderAuthenticationMethod::LocalSubscription,
            secret_reference: None,
            runtime_home_id: format!("runtime-{id}"),
            credential_home: Some(magi_domain::ProviderCredentialHome {
                authority_id: format!("authority-{id}"),
                canonical_path: format!("/Users/fixture/{id}"),
                device: 1,
                inode: 2,
                credential_store: magi_domain::ProviderCredentialStore::File,
                account_digest: Some(Digest::from_bytes(id.as_bytes())),
            }),
        }
    }

    fn catalog(
        id: &str,
        profile: &ProviderProfileRevision,
        model: &str,
    ) -> ProviderCatalogSnapshot {
        ProviderCatalogSnapshot::new(
            magi_domain::ProviderCatalogInput {
                catalog_snapshot_id: id.into(),
                provider_id: profile.provider_id.clone(),
                provider_profile_id: profile.provider_profile_id.clone(),
                profile_revision: profile.revision,
                adapter_id: "codex-acp".into(),
                adapter_version: "1.13.1".into(),
                adapter_digest: Digest::from_bytes(b"signed-adapter"),
                fetched_at: "2026-10-01T00:00:00Z".into(),
            },
            vec![magi_domain::ProviderCatalogModel {
                model_id: model.into(),
                name: None,
                description: None,
                context_window_tokens: None,
                max_output_tokens: None,
            }],
        )
        .unwrap()
        .with_negotiated_modes(magi_domain::NegotiatedModeState {
            current_mode_id: None,
            modes: vec![],
        })
        .unwrap()
        .with_artifact_set_digest(Digest::from_bytes(b"verified-runtime-set-fixture"))
        .unwrap()
    }

    fn model_input(
        catalog: &ProviderCatalogSnapshot,
        model: &str,
        revision: Option<u64>,
    ) -> ProviderModelSelectionInput {
        ProviderModelSelectionInput {
            provider_profile_id: catalog.provider_profile_id.clone(),
            profile_revision: catalog.profile_revision,
            catalog_snapshot_id: catalog.catalog_snapshot_id.clone(),
            catalog_digest: catalog.catalog_digest.clone(),
            model_id: model.into(),
            mode_id: None,
            expected_selection_revision: revision,
            updated_at: "2026-10-01T00:00:01Z".into(),
        }
    }

    #[test]
    fn equivalent_catalog_observations_preserve_three_saved_authorities() {
        let root = std::env::temp_dir().join(format!("magi-catalog-witness-{}", Uuid::new_v4()));
        let storage = Storage::open_or_create(&root).unwrap();
        let at = "2026-10-01T00:00:00Z";
        let mut references = Vec::new();
        let mut originals = Vec::new();
        for (index, core_id) in CoreId::ALL.into_iter().enumerate() {
            let id = format!("witness-profile-{index}");
            let profile = storage
                .save_provider_profile(&profile(&id), None, at)
                .unwrap();
            let original = catalog(&format!("original-{index}"), &profile, "model");
            storage.save_provider_catalog_snapshot(&original).unwrap();
            let selected = storage
                .select_provider_model(&model_input(&original, "model", None))
                .unwrap();
            let core = storage
                .select_core_model(&crate::CoreModelSelectionInput {
                    core_id,
                    provider_profile_id: id.clone(),
                    profile_revision: profile.revision,
                    model_selection_revision: selected.selection_revision,
                    expected_selection_revision: None,
                    updated_at: at.into(),
                })
                .unwrap();
            references.push(crate::CoreBindingReference {
                core_id,
                provider_profile_id: id,
                profile_revision: profile.revision,
                model_selection_revision: selected.selection_revision,
                core_selection_revision: core.selection_revision,
            });
            let mut fresh = original.clone();
            fresh.catalog_snapshot_id = format!("fresh-{index}");
            fresh.fetched_at = "2026-10-02T00:00:00Z".into();
            fresh.catalog_digest = fresh.calculate_digest().unwrap();
            storage.save_provider_catalog_snapshot(&fresh).unwrap();
            originals.push((original, selected));
        }
        let resolved = storage.resolve_core_bindings(&references).unwrap();
        drop(storage);
        let storage = Storage::open_or_create(&root).unwrap();
        let witnesses = storage.load_core_execution_witnesses(&references).unwrap();
        let database_before = fs::read(root.join("state/magi.sqlite")).unwrap();
        let wal_before = fs::read(root.join("state/magi.sqlite-wal")).ok();
        let reader = StorageReader::open_read_only(&root).unwrap();
        assert_eq!(
            reader.load_core_execution_witnesses(&references).unwrap(),
            witnesses
        );
        drop(reader);
        assert_eq!(
            fs::read(root.join("state/magi.sqlite")).unwrap(),
            database_before
        );
        assert_eq!(
            fs::read(root.join("state/magi.sqlite-wal")).ok(),
            wal_before
        );
        for (index, witness) in witnesses.iter().enumerate() {
            assert_eq!(witness.binding, originals[index].1.binding);
            assert_eq!(witness.binding, resolved[index]);
            assert_eq!(witness.original_catalog, originals[index].0);
            assert_ne!(
                witness.fresh_catalog.catalog_snapshot_id,
                witness.original_catalog.catalog_snapshot_id
            );
        }
        let mut changed = witnesses[0].fresh_catalog.clone();
        changed.catalog_snapshot_id = "changed-capacity".into();
        changed.models[0].context_window_tokens = Some(1234);
        changed.catalog_digest = changed.calculate_digest().unwrap();
        storage.save_provider_catalog_snapshot(&changed).unwrap();
        assert!(storage.resolve_core_bindings(&references).is_err());
        assert!(storage.load_core_execution_witnesses(&references).is_err());
        assert_eq!(
            storage
                .provider_model_selection_revision(&references[0].provider_profile_id)
                .unwrap(),
            Some(originals[0].1.selection_revision)
        );
        drop(storage);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn negotiated_modes_are_explicit_persisted_and_catalog_refresh_fenced() {
        let root = std::env::temp_dir().join(format!("magi-mode-store-{}", Uuid::new_v4()));
        let storage = Storage::open_or_create(&root).unwrap();
        let profile = storage
            .save_provider_profile(&profile("profile"), None, "2026-10-01T00:00:00Z")
            .unwrap();
        let mut legacy = catalog("catalog-legacy", &profile, "model");
        legacy.artifact_set_digest = None;
        legacy.negotiated_modes = None;
        legacy.schema_version = magi_domain::PROVIDER_CATALOG_SCHEMA_VERSION;
        legacy.catalog_digest = legacy.calculate_digest().unwrap();
        storage.save_provider_catalog_snapshot(&legacy).unwrap();
        assert!(
            storage
                .select_provider_model(&model_input(&legacy, "model", None))
                .is_err()
        );
        assert_eq!(
            storage
                .provider_model_selection_revision("profile")
                .unwrap(),
            None
        );
        let catalog = catalog("catalog-modes", &profile, "model")
            .with_negotiated_modes(magi_domain::NegotiatedModeState {
                current_mode_id: Some("review".into()),
                modes: vec![magi_domain::ProviderCatalogMode {
                    mode_id: "review".into(),
                    name: "Review".into(),
                    description: None,
                }],
            })
            .unwrap();
        storage.save_provider_catalog_snapshot(&catalog).unwrap();
        let mut input = model_input(&catalog, "model", None);
        assert!(storage.select_provider_model(&input).is_err());
        input.mode_id = Some("unknown".into());
        assert!(storage.select_provider_model(&input).is_err());
        input.mode_id = Some("review".into());
        let selected = storage.select_provider_model(&input).unwrap();
        assert_eq!(selected.binding.mode_id.as_deref(), Some("review"));
        storage
            .select_core_model(&CoreModelSelectionInput {
                core_id: CoreId::Casper3,
                provider_profile_id: "profile".into(),
                profile_revision: profile.revision,
                model_selection_revision: selected.selection_revision,
                expected_selection_revision: None,
                updated_at: "2026-10-01T00:00:02Z".into(),
            })
            .unwrap();
        drop(storage);
        let storage = Storage::open_or_create(&root).unwrap();
        assert_eq!(
            storage
                .load_provider_model_selection("profile")
                .unwrap()
                .unwrap()
                .binding
                .mode_id
                .as_deref(),
            Some("review")
        );
        let refreshed =
            super::saved_model_selection_tests::catalog("catalog-absence", &profile, "model");
        storage.save_provider_catalog_snapshot(&refreshed).unwrap();
        assert!(storage.load_core_model_selection(CoreId::Casper3).is_err());
        assert_eq!(
            storage
                .core_model_selection_revision(CoreId::Casper3)
                .unwrap(),
            Some(0)
        );
        drop(storage);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn historical_artifact_absence_reopens_but_new_admission_is_atomic_and_fenced() {
        let root = std::env::temp_dir().join(format!("magi-artifact-authority-{}", Uuid::new_v4()));
        let storage = Storage::open_or_create(&root).unwrap();
        let profile = storage
            .save_provider_profile(&profile("profile"), None, "2026-10-01T00:00:00Z")
            .unwrap();
        let mut historical = catalog("catalog-history", &profile, "model");
        historical.artifact_set_digest = None;
        historical.schema_version = magi_domain::MODE_ATTESTED_CATALOG_SCHEMA_VERSION;
        historical.catalog_digest = historical.calculate_digest().unwrap();
        storage.save_provider_catalog_snapshot(&historical).unwrap();
        let bytes = serde_json::to_vec(&historical).unwrap();
        let binding =
            AcpModelBindingSnapshot::from_catalog_with_mode(&historical, "model", None).unwrap();
        binding.validate_ready(&historical).unwrap();
        let mut request = LiveRunAdmissionRequest {
            command_id: "artifact-command".into(),
            idempotency_key: "artifact-key".into(),
            question: "Evaluate evidence".into(),
            model_binding: binding,
        };
        assert!(storage.admit_live_run(&request).is_err());
        let count: i64 = storage
            .connection()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM live_run_outbox", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
        let mut current = historical.clone();
        current.catalog_snapshot_id = "catalog-current".into();
        let current = current
            .with_artifact_set_digest(Digest::from_bytes(b"verified-runtime-set-fixture"))
            .unwrap();
        storage.save_provider_catalog_snapshot(&current).unwrap();
        request.model_binding =
            AcpModelBindingSnapshot::from_catalog_with_mode(&current, "model", None).unwrap();
        let valid = request.clone();
        request.model_binding.artifact_set_digest = Some(Digest::from_bytes(b"other-runtime"));
        request.model_binding.binding_digest = request.model_binding.calculate_digest().unwrap();
        assert!(storage.admit_live_run(&request).is_err());
        assert!(matches!(
            storage.admit_live_run(&valid).unwrap(),
            LiveRunAdmissionOutcome::Accepted {
                duplicate: false,
                ..
            }
        ));
        drop(storage);
        let reader = StorageReader::open_read_only(&root).unwrap();
        let reopened = reader
            .load_provider_catalog_snapshot("catalog-history")
            .unwrap()
            .unwrap();
        reopened.validate().unwrap();
        assert!(reopened.artifact_set_digest.is_none());
        assert_eq!(serde_json::to_vec(&reopened).unwrap(), bytes);
        drop(reader);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn direct_live_idempotency_distinguishes_modes_and_preserves_legacy_digest() {
        let root = std::env::temp_dir().join(format!("magi-mode-key-{}", Uuid::new_v4()));
        let storage = Storage::open_or_create(&root).unwrap();
        let profile = storage
            .save_provider_profile(&profile("profile"), None, "2026-10-01T00:00:00Z")
            .unwrap();
        let mut legacy = catalog("catalog-legacy", &profile, "model");
        legacy.artifact_set_digest = None;
        legacy.negotiated_modes = None;
        legacy.schema_version = 1;
        legacy.catalog_digest = legacy.calculate_digest().unwrap();
        let binding = AcpModelBindingSnapshot::from_catalog(&legacy, "model").unwrap();
        assert_eq!(
            live_run_request_digest("Evaluate evidence", &binding)
                .unwrap()
                .as_str(),
            "24a1cca747f0fad18ceb1a3bc447053e59a927a106cfc571ac507803df5cba4d"
        );
        let catalog = legacy
            .with_negotiated_modes(magi_domain::NegotiatedModeState {
                current_mode_id: Some("review".into()),
                modes: ["review", "plan"]
                    .map(|id| magi_domain::ProviderCatalogMode {
                        mode_id: id.into(),
                        name: id.into(),
                        description: None,
                    })
                    .into(),
            })
            .unwrap()
            .with_artifact_set_digest(Digest::from_bytes(b"verified-runtime-set-fixture"))
            .unwrap();
        storage.save_provider_catalog_snapshot(&catalog).unwrap();
        let review =
            AcpModelBindingSnapshot::from_catalog_with_mode(&catalog, "model", Some("review"))
                .unwrap();
        let plan = AcpModelBindingSnapshot::from_catalog_with_mode(&catalog, "model", Some("plan"))
            .unwrap();
        let request = LiveRunAdmissionRequest {
            command_id: "mode-command".into(),
            idempotency_key: "mode-key".into(),
            question: "Evaluate evidence".into(),
            model_binding: review,
        };
        storage.admit_live_run(&request).unwrap();
        let changed = LiveRunAdmissionRequest {
            model_binding: plan,
            ..request.clone()
        };
        assert!(matches!(
            storage.admit_live_run(&changed),
            Err(StorageError::IdempotencyConflict)
        ));
        drop(storage);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn three_core_admission_is_atomic_frozen_and_idempotent_across_selection_edits() {
        use magi_domain::*;
        let root = std::env::temp_dir().join(format!("magi-core-admission-{}", Uuid::new_v4()));
        let storage = Storage::open_or_create(&root).unwrap();
        let at = "2026-10-01T00:00:00Z";
        let mut references = Vec::new();
        for (index, core_id) in CoreId::ALL.into_iter().enumerate() {
            let id = format!("profile-{index}");
            let profile = storage
                .save_provider_profile(&profile(&id), None, at)
                .unwrap();
            let catalog = catalog(
                &format!("catalog-{index}"),
                &profile,
                &format!("model-{index}"),
            );
            storage.save_provider_catalog_snapshot(&catalog).unwrap();
            let selected = storage
                .select_provider_model(&model_input(&catalog, &format!("model-{index}"), None))
                .unwrap();
            let core = storage
                .select_core_model(&crate::CoreModelSelectionInput {
                    core_id,
                    provider_profile_id: id.clone(),
                    profile_revision: profile.revision,
                    model_selection_revision: selected.selection_revision,
                    expected_selection_revision: None,
                    updated_at: at.into(),
                })
                .unwrap();
            references.push(crate::CoreBindingReference {
                core_id,
                provider_profile_id: id,
                profile_revision: profile.revision,
                model_selection_revision: selected.selection_revision,
                core_selection_revision: core.selection_revision,
            });
        }
        let bindings = storage.resolve_core_bindings(&references).unwrap();
        assert!(storage.resolve_core_bindings(&references[..2]).is_err());
        let mut duplicate = references.clone();
        duplicate[2] = duplicate[1].clone();
        assert!(storage.resolve_core_bindings(&duplicate).is_err());
        let make_input = |refs: &[crate::CoreBindingReference]| {
            let roles = CoreId::ALL.map(|core_id| {
                let index = CoreId::ALL.iter().position(|id| *id == core_id).unwrap();
                let binding = &bindings[index];
                CoreRoleProfile {
                    core_id,
                    profile_id: format!("role-{index}"),
                    revision: 0,
                    display_name: core_id.wire_name().into(),
                    review_purpose: "Review evidence".into(),
                    evaluation_criteria: vec!["Evidence".into()],
                    falsification_questions: vec!["What disproves it?".into()],
                    response_language: "en".into(),
                    binding: ModelBindingSnapshot {
                        provider_profile_id: binding.provider_profile_id.clone(),
                        revision: binding.profile_revision,
                        adapter_id: binding.adapter_id.clone(),
                        adapter_version: binding.adapter_version.clone(),
                        adapter_digest: binding.adapter_digest.clone(),
                        model_id: binding.model_id.clone(),
                        context_window_tokens: None,
                        maximum_output_tokens: None,
                    },
                    catalog_binding: Some(binding.clone()),
                }
            });
            InputSnapshot::new_with_request_provenance(
                QuestionSnapshot::new(
                    "question-core".into(),
                    QuestionKind::Answer,
                    "Evaluate evidence".into(),
                    vec![],
                    vec![],
                    vec![],
                )
                .unwrap(),
                ContextManifest::new("context-core".into(), vec![]).unwrap(),
                RoleSetSnapshot::new("roles-core".into(), roles)
                    .unwrap()
                    .with_frozen_core_selections(refs.to_vec().try_into().unwrap())
                    .unwrap(),
                Digest::from_bytes(b"policy"),
                DeliberationRequestProvenance {
                    context_draft_id: None,
                    context_revision: None,
                    role_preset_id: "preset-core".into(),
                    role_revision: 0,
                    disclosure_confirmed: true,
                },
            )
            .unwrap()
        };
        let mut input = make_input(&references);
        assert_eq!(input.schema_version, 2);
        assert_eq!(input.role_set.schema_version, 2);
        let restored: InputSnapshot =
            serde_json::from_str(&serde_json::to_string(&input).unwrap()).unwrap();
        restored.validate().unwrap();
        assert_eq!(restored.input_digest, input.input_digest);
        let make_aggregate = |input: InputSnapshot| {
            RunAggregate::new(
                Run::new(
                    "run-core".into(),
                    "conversation-core".into(),
                    None,
                    &input,
                    at.into(),
                )
                .unwrap(),
                input,
            )
            .unwrap()
        };
        let mut command = CommandEnvelope {
            command_id: "command-core".into(),
            idempotency_key: "key-core".into(),
            command_kind: CommandKind::CreateRun,
            target_id: "run-core".into(),
            expected_revision: 0,
            payload_digest: input.input_digest.clone(),
        };
        let request = LiveRunAdmissionRequest {
            command_id: command.command_id.clone(),
            idempotency_key: command.idempotency_key.clone(),
            question: input.question.prompt.clone(),
            model_binding: bindings[0].clone(),
        };
        let frozen_input_bytes = serde_json::to_vec(&input).unwrap();
        for (index, binding) in bindings.iter().enumerate() {
            let mut renewed = storage
                .load_provider_catalog_snapshot(&binding.catalog_snapshot_id)
                .unwrap()
                .unwrap();
            renewed.catalog_snapshot_id = format!("renewed-admission-{index}");
            renewed.fetched_at = "2026-10-02T00:00:00Z".into();
            renewed.catalog_digest = renewed.calculate_digest().unwrap();
            storage.save_provider_catalog_snapshot(&renewed).unwrap();
        }
        assert_eq!(
            storage.resolve_core_bindings(&references).unwrap(),
            bindings
        );
        let mut changed = storage
            .load_latest_provider_catalog(
                &references[0].provider_profile_id,
                references[0].profile_revision,
            )
            .unwrap()
            .unwrap();
        let unchanged = changed.clone();
        changed.catalog_snapshot_id = "capacity-race-before-admission".into();
        changed.models[0].max_output_tokens = Some(123);
        changed.catalog_digest = changed.calculate_digest().unwrap();
        storage.save_provider_catalog_snapshot(&changed).unwrap();
        assert!(
            storage
                .admit_deliberation_run(
                    &command,
                    &mut make_aggregate(input.clone()),
                    &request,
                    &references,
                    at,
                    None
                )
                .is_err()
        );
        for table in [
            "runs",
            "live_runs",
            "commands",
            "live_run_dispatch_reservations",
        ] {
            assert_eq!(
                storage
                    .connection()
                    .unwrap()
                    .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row
                        .get::<_, i64>(0))
                    .unwrap(),
                0
            );
        }
        let mut recovered = unchanged;
        recovered.catalog_snapshot_id = "equivalent-after-race".into();
        storage.save_provider_catalog_snapshot(&recovered).unwrap();
        assert_eq!(serde_json::to_vec(&input).unwrap(), frozen_input_bytes);
        let mut stale = references.clone();
        stale[2].core_selection_revision += 1;
        let stale_input = make_input(&stale);
        let mut stale_command = command.clone();
        stale_command.payload_digest = stale_input.input_digest.clone();
        assert!(
            storage
                .admit_deliberation_run(
                    &stale_command,
                    &mut make_aggregate(stale_input),
                    &request,
                    &stale,
                    at,
                    None
                )
                .is_err()
        );
        assert!(
            references
                .iter()
                .all(|reference| reference.model_selection_revision == 0
                    && reference.core_selection_revision == 0
                    && reference.profile_revision == 0)
        );
        storage
            .select_core_model(&crate::CoreModelSelectionInput {
                core_id: references[2].core_id,
                provider_profile_id: references[2].provider_profile_id.clone(),
                profile_revision: references[2].profile_revision,
                model_selection_revision: 0,
                expected_selection_revision: Some(0),
                updated_at: at.into(),
            })
            .unwrap();
        assert!(
            storage
                .admit_deliberation_run(
                    &command,
                    &mut make_aggregate(input.clone()),
                    &request,
                    &references,
                    at,
                    None
                )
                .is_err()
        );
        for table in ["runs", "live_runs", "live_run_dispatch_reservations"] {
            assert_eq!(
                storage
                    .connection()
                    .unwrap()
                    .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row
                        .get::<_, i64>(0))
                    .unwrap(),
                0
            );
        }
        references[2].core_selection_revision = 1;
        input = make_input(&references);
        command.payload_digest = input.input_digest.clone();
        let mut aggregate = make_aggregate(input.clone());
        assert!(matches!(
            storage
                .admit_deliberation_run(&command, &mut aggregate, &request, &references, at, None)
                .unwrap(),
            LiveRunAdmissionOutcome::Accepted {
                duplicate: false,
                ..
            }
        ));
        assert_eq!(
            storage
                .connection()
                .unwrap()
                .query_row(
                    "SELECT COUNT(*) FROM live_run_dispatch_reservations",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            10
        );
        let stored = storage.load_run_aggregate("run-core").unwrap();
        let projection = storage.load_live_dispatch_projection("run-core").unwrap();
        assert_eq!(projection.run_revision, stored.run().revision);
        assert_eq!(projection.input_digest, stored.input().input_digest);
        assert_eq!(projection.dispatches.len(), 10);
        for (slot, dispatch) in projection.dispatches.iter().enumerate() {
            let index = match slot {
                0..=2 => slot,
                3..=5 => slot - 3,
                6 => 0,
                _ => slot - 7,
            };
            assert_eq!(dispatch.binding_digest, bindings[index].binding_digest);
            assert_eq!(
                dispatch.provider_profile_id,
                references[index].provider_profile_id
            );
            assert_eq!(dispatch.state, crate::LiveDispatchState::Reserved);
            assert!(dispatch.result_ref.is_none());
        }
        {
            let mut connection = storage.connection().unwrap();
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .unwrap();
            transaction.execute("UPDATE live_run_dispatch_reservations SET state='active' WHERE run_id='run-core' AND slot_ordinal IN (7,8)", []).unwrap();
            transaction.execute("UPDATE live_run_dispatch_reservations SET state='settled' WHERE run_id='run-core' AND slot_ordinal IN (7,8)", []).unwrap();
            let mut snapshot = stored.snapshot(0);
            snapshot.run.status = RunStatus::Balloting;
            snapshot.submitted_ballot_count = 2;
            let sealed = load_live_dispatches_from(&transaction, &snapshot).unwrap();
            assert!(sealed[7..].iter().all(|slot| slot.result_ref.is_none()));
            snapshot.run.status = RunStatus::Cancelled;
            assert_eq!(
                load_live_dispatches_from(&transaction, &snapshot).unwrap()[7].state,
                crate::LiveDispatchState::Settled
            );
            snapshot.run.status = RunStatus::Completed {
                outcome: magi_domain::Outcome::Unanimous,
            };
            transaction.execute("UPDATE live_run_dispatch_reservations SET state='active' WHERE run_id='run-core' AND state='reserved'", []).unwrap();
            transaction.execute("UPDATE live_run_dispatch_reservations SET state='settled' WHERE run_id='run-core' AND state='active'", []).unwrap();
            assert!(load_live_dispatches_from(&transaction, &snapshot).is_err());
            transaction.execute("DELETE FROM live_run_dispatch_reservations WHERE run_id='run-core' AND slot_ordinal=9", []).unwrap();
            assert!(load_live_dispatches_from(&transaction, &snapshot).is_err());
            transaction.rollback().unwrap();
        }
        let reader = StorageReader::open_read_only(&root).unwrap();
        assert_eq!(
            reader.load_live_dispatch_projection("run-core").unwrap(),
            projection
        );
        {
            let connection = storage.connection().unwrap();
            connection
                .execute(
                    "UPDATE store_meta SET value='invalid' WHERE key='store_generation'",
                    [],
                )
                .unwrap();
        }
        assert!(reader.load_live_dispatch_projection("run-core").is_err());
        storage
            .connection()
            .unwrap()
            .execute(
                "UPDATE store_meta SET value=?1 WHERE key='store_generation'",
                [storage.identity.generation.to_string()],
            )
            .unwrap();
        assert_eq!(
            stored
                .input()
                .role_set
                .frozen_core_selections
                .as_ref()
                .unwrap()
                .as_slice(),
            references.as_slice()
        );
        for (index, binding) in bindings.iter().enumerate() {
            assert_eq!(
                stored.input().role_set.roles[index]
                    .catalog_binding
                    .as_ref(),
                Some(binding)
            );
        }
        let mut edited = profile("profile-1");
        edited.display_name = "Edited head".into();
        storage
            .save_provider_profile(&edited, Some(references[1].profile_revision), at)
            .unwrap();
        assert!(storage.resolve_core_bindings(&references).is_err());
        assert_eq!(
            reader.load_live_dispatch_projection("run-core").unwrap(),
            projection
        );
        assert!(matches!(
            storage
                .admit_deliberation_run(
                    &command,
                    &mut make_aggregate(input.clone()),
                    &request,
                    &references,
                    at,
                    None
                )
                .unwrap(),
            LiveRunAdmissionOutcome::Accepted {
                duplicate: true,
                ..
            }
        ));
        let mut changed = references.clone();
        changed[2].model_selection_revision += 1;
        let changed_input = make_input(&changed);
        let mut changed_command = command.clone();
        changed_command.payload_digest = changed_input.input_digest.clone();
        assert!(
            storage
                .admit_deliberation_run(
                    &changed_command,
                    &mut make_aggregate(changed_input),
                    &request,
                    &changed,
                    at,
                    None
                )
                .is_err()
        );
        let altered_intent = input
            .clone()
            .with_request_provenance(DeliberationRequestProvenance {
                context_draft_id: Some("different-draft".into()),
                context_revision: Some(1),
                role_preset_id: "preset-core".into(),
                role_revision: 0,
                disclosure_confirmed: true,
            })
            .unwrap();
        let mut altered_command = command.clone();
        altered_command.payload_digest = altered_intent.input_digest.clone();
        assert!(
            storage
                .admit_deliberation_run(
                    &altered_command,
                    &mut make_aggregate(altered_intent),
                    &request,
                    &references,
                    at,
                    None
                )
                .is_err()
        );
        let claim = storage.claim_next_live_run("worker", at).unwrap().unwrap();
        for slot in 0..10 {
            storage
                .activate_live_run_dispatch_slot(&claim, slot, at)
                .unwrap();
            let index = match slot {
                0..=2 => slot,
                3..=5 => slot - 3,
                6 => 0,
                _ => slot - 7,
            } as usize;
            let role = storage.load_frozen_deliberation_slot(&claim, slot).unwrap();
            assert_eq!(role.catalog_binding.as_ref(), Some(&bindings[index]));
            assert_eq!(role.binding.model_id, format!("model-{index}"));
            let mut fenced = claim.clone();
            fenced.claim_generation += 1;
            assert!(
                storage
                    .load_frozen_deliberation_slot(&fenced, slot)
                    .is_err()
            );
            storage.connection().unwrap().execute("UPDATE live_run_dispatch_reservations SET state='settled' WHERE run_id='run-core' AND slot_ordinal=?1", [i64::from(slot)]).unwrap();
        }
        assert_eq!(
            storage
                .connection()
                .unwrap()
                .query_row(
                    "SELECT COUNT(*) FROM live_run_dispatch_reservations",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            10
        );
        drop(storage);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn explicit_models_and_three_independent_core_pins_survive_restart_and_fence_updates() {
        let root = std::env::temp_dir().join(format!("magi-model-store-{}", Uuid::new_v4()));
        let storage = Storage::open_or_create(&root).unwrap();
        let mut chosen = Vec::new();
        for (index, core) in CoreId::ALL.into_iter().enumerate() {
            assert!(storage.load_core_model_selection(core).unwrap().is_none());
            let id = format!("profile-{index}");
            let profile = storage
                .save_provider_profile(&profile(&id), None, "2026-10-01T00:00:00Z")
                .unwrap();
            let model = format!("model-{index}");
            let catalog = catalog(&format!("catalog-{index}"), &profile, &model);
            storage.save_provider_catalog_snapshot(&catalog).unwrap();
            assert!(
                storage
                    .load_provider_model_selection(&id)
                    .unwrap()
                    .is_none()
            );
            assert!(
                storage
                    .select_provider_model(&model_input(&catalog, "absent", None))
                    .is_err()
            );
            let selected = storage
                .select_provider_model(&model_input(&catalog, &model, None))
                .unwrap();
            assert_eq!(selected.selection_revision, 0);
            assert!(
                storage
                    .select_provider_model(&model_input(&catalog, &model, None))
                    .is_err()
            );
            let input = CoreModelSelectionInput {
                core_id: core,
                provider_profile_id: id.clone(),
                profile_revision: profile.revision,
                model_selection_revision: selected.selection_revision,
                expected_selection_revision: None,
                updated_at: "2026-10-01T00:00:02Z".into(),
            };
            let saved = storage.select_core_model(&input).unwrap();
            assert!(storage.select_core_model(&input).is_err());
            chosen.push((catalog, selected, saved));
        }
        drop(storage);
        let storage = Storage::open_or_create(&root).unwrap();
        for (catalog, model, core) in &chosen {
            assert_eq!(
                storage
                    .load_latest_provider_catalog(
                        &catalog.provider_profile_id,
                        catalog.profile_revision
                    )
                    .unwrap()
                    .unwrap(),
                *catalog
            );
            assert_eq!(
                storage
                    .load_provider_model_selection(&catalog.provider_profile_id)
                    .unwrap()
                    .unwrap(),
                *model
            );
            assert_eq!(
                storage
                    .load_core_model_selection(core.core_id)
                    .unwrap()
                    .unwrap(),
                *core
            );
        }
        let (catalog, _, core) = &chosen[0];
        let next = storage
            .select_provider_model(&model_input(catalog, &catalog.models[0].model_id, Some(0)))
            .unwrap();
        assert_eq!(next.selection_revision, 1);
        assert!(storage.load_core_model_selection(core.core_id).is_err());
        assert!(
            storage
                .load_core_model_selection(chosen[1].2.core_id)
                .unwrap()
                .is_some()
        );
        assert!(
            storage
                .load_core_model_selection(chosen[2].2.core_id)
                .unwrap()
                .is_some()
        );
        let input = CoreModelSelectionInput {
            core_id: core.core_id,
            provider_profile_id: core.provider_profile_id.clone(),
            profile_revision: core.profile_revision,
            model_selection_revision: 1,
            expected_selection_revision: Some(0),
            updated_at: "2026-10-01T00:00:03Z".into(),
        };
        assert_eq!(
            storage
                .select_core_model(&input)
                .unwrap()
                .selection_revision,
            1
        );
        drop(storage);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn refreshed_catalog_removed_model_and_changed_profile_fail_closed() {
        let root = std::env::temp_dir().join(format!("magi-stale-store-{}", Uuid::new_v4()));
        let storage = Storage::open_or_create(&root).unwrap();
        let mut input = profile("profile");
        let first = storage
            .save_provider_profile(&input, None, "2026-10-01T00:00:00Z")
            .unwrap();
        let old = catalog("catalog-old", &first, "removed");
        storage.save_provider_catalog_snapshot(&old).unwrap();
        storage
            .select_provider_model(&model_input(&old, "removed", None))
            .unwrap();
        let new = catalog("catalog-new", &first, "replacement");
        storage.save_provider_catalog_snapshot(&new).unwrap();
        assert!(storage.load_provider_model_selection("profile").is_err());
        assert!(
            storage
                .select_provider_model(&model_input(&old, "removed", Some(0)))
                .is_err()
        );
        let mut wrong_digest = model_input(&new, "replacement", Some(0));
        wrong_digest.catalog_digest = Digest::from_bytes(b"wrong");
        assert!(storage.select_provider_model(&wrong_digest).is_err());
        assert!(
            storage
                .select_provider_model(&model_input(&new, "removed", Some(0)))
                .is_err()
        );
        assert_eq!(
            storage
                .provider_model_selection_revision("profile")
                .unwrap(),
            Some(0)
        );
        storage
            .select_provider_model(&model_input(&new, "replacement", Some(0)))
            .unwrap();
        input.display_name = "edited".into();
        storage
            .save_provider_profile(&input, Some(first.revision), "2026-10-01T00:00:04Z")
            .unwrap();
        assert!(
            storage
                .load_latest_provider_catalog("profile", first.revision)
                .is_err()
        );
        assert!(storage.load_provider_model_selection("profile").is_err());
        assert!(
            storage
                .select_provider_model(&model_input(&new, "replacement", Some(1)))
                .is_err()
        );
        drop(storage);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn same_model_requires_three_explicit_core_choices_and_columns_are_verified() {
        let root = std::env::temp_dir().join(format!("magi-explicit-store-{}", Uuid::new_v4()));
        let storage = Storage::open_or_create(&root).unwrap();
        let profile = storage
            .save_provider_profile(&profile("shared"), None, "2026-10-01T00:00:00Z")
            .unwrap();
        let catalog = catalog("shared-catalog", &profile, "shared-model");
        storage.save_provider_catalog_snapshot(&catalog).unwrap();
        storage
            .select_provider_model(&model_input(&catalog, "shared-model", None))
            .unwrap();
        for core_id in CoreId::ALL {
            assert!(
                storage
                    .load_core_model_selection(core_id)
                    .unwrap()
                    .is_none()
            );
            let choice = CoreModelSelectionInput {
                core_id,
                provider_profile_id: "shared".into(),
                profile_revision: 0,
                model_selection_revision: 0,
                expected_selection_revision: None,
                updated_at: "2026-10-01T00:00:01Z".into(),
            };
            let encoded = serde_json::to_value(&choice).unwrap();
            assert!(encoded["expectedSelectionRevision"].is_null());
            assert!(encoded.get("modeId").is_none());
            storage.select_core_model(&choice).unwrap();
        }
        storage.connection().unwrap().execute("UPDATE core_model_selections SET model_selection_revision=1 WHERE core_id='MELCHIOR-1'", []).unwrap();
        assert!(matches!(
            storage.load_core_model_selection(CoreId::Melchior1),
            Err(StorageError::Corrupt(_))
        ));
        assert!(
            storage
                .load_core_model_selection(CoreId::Casper3)
                .unwrap()
                .is_some()
        );
        drop(storage);
        std::fs::remove_dir_all(root).unwrap();
    }

    fn construct_historical_fixture(root: &std::path::Path, version: u32) {
        let database = root.join("state/magi.sqlite");
        let source = root.join("state/historical-fixture-source.sqlite");
        let prior = Connection::open(&database).unwrap();
        prior
            .execute("VACUUM INTO ?1", [source.to_str().unwrap()])
            .unwrap();
        prior
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .unwrap();
        drop(prior);
        fs::remove_file(&database).unwrap();
        let historical = Connection::open(&database).unwrap();
        historical.execute_batch("CREATE TABLE schema_migrations(version INTEGER PRIMARY KEY CHECK(version>0),checksum TEXT NOT NULL,applied_at TEXT NOT NULL);").unwrap();
        for migration in 1..=version {
            historical
                .execute_batch(migration_sql(migration).unwrap())
                .unwrap();
        }
        historical
            .execute("ATTACH DATABASE ?1 AS prior", [source.to_str().unwrap()])
            .unwrap();
        let tables: Vec<String> = historical.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' AND name<>'schema_migrations' ORDER BY rowid").unwrap().query_map([], |r|r.get(0)).unwrap().map(Result::unwrap).collect();
        for table in tables {
            if table == "store_meta" {
                let metadata: Vec<(String,String)> = historical.prepare("SELECT key,value FROM prior.store_meta WHERE key NOT IN ('admission_lineage_id','admission_activation_state')").unwrap().query_map([], |r|Ok((r.get(0)?,r.get(1)?))).unwrap().map(Result::unwrap).collect();
                for (key, value) in metadata {
                    let value = if key == "schema_version" {
                        version.to_string()
                    } else {
                        value
                    };
                    let changed = historical
                        .execute(
                            "UPDATE store_meta SET value=?2 WHERE key=?1",
                            params![key, value],
                        )
                        .unwrap();
                    if changed == 0 {
                        historical
                            .execute(
                                "INSERT INTO store_meta(key,value) VALUES (?1,?2)",
                                params![key, value],
                            )
                            .unwrap();
                    }
                }
                continue;
            }
            historical
                .execute_batch(&format!(
                    "INSERT INTO main.\"{table}\" SELECT * FROM prior.\"{table}\";"
                ))
                .unwrap();
        }
        historical.execute("INSERT INTO schema_migrations SELECT * FROM prior.schema_migrations WHERE version<=?1", [version]).unwrap();
        historical
            .execute(
                "UPDATE store_meta SET value=?1 WHERE key='schema_version'",
                [version.to_string()],
            )
            .unwrap();
        historical.execute("DELETE FROM store_meta WHERE key IN ('admission_lineage_id','admission_activation_state')", []).unwrap();
        let highwaters: Vec<(String, i64)> = historical
            .prepare("SELECT name,seq FROM prior.sqlite_sequence")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        for (table, seq) in highwaters {
            historical
                .execute(
                    "UPDATE sqlite_sequence SET seq=max(seq,?2) WHERE name=?1",
                    params![table, seq],
                )
                .unwrap();
        }
        historical
            .execute_batch(&format!(
                "PRAGMA user_version={version}; DETACH DATABASE prior;"
            ))
            .unwrap();
        assert!(
            historical
                .prepare("PRAGMA foreign_key_check")
                .unwrap()
                .query([])
                .unwrap()
                .next()
                .unwrap()
                .is_none()
        );
        read_only_store_identity(&historical).unwrap();
        drop(historical);
        fs::remove_file(source).unwrap();
    }

    #[test]
    fn lifecycle_migration_preserves_ten_revision_authorities_and_enforces_record_constraints() {
        let (root, storage, aggregate) = cancellation_fixture();
        let expected_input = aggregate.input().clone();
        let selections: Vec<_> = CoreId::ALL
            .into_iter()
            .map(|core| storage.load_core_model_selection(core).unwrap().unwrap())
            .collect();
        let ledger: Vec<(i64, String)> = {
            let connection = storage.connection().unwrap();
            let mut statement=connection.prepare("SELECT version,checksum FROM schema_migrations WHERE version<=10 ORDER BY version").unwrap();
            statement
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap()
        };
        drop(storage);
        construct_historical_fixture(&root, 10);
        let upgraded = Storage::open_or_create(&root).unwrap();
        assert_eq!(upgraded.identity.schema_version, SCHEMA_VERSION);
        assert_eq!(
            upgraded.load_run_aggregate("run-core").unwrap().input(),
            &expected_input
        );
        for (core, selection) in CoreId::ALL.into_iter().zip(selections) {
            assert_eq!(
                upgraded.load_core_model_selection(core).unwrap(),
                Some(selection)
            );
        }
        assert_eq!(
            upgraded
                .load_live_dispatch_projection("run-core")
                .unwrap()
                .dispatches
                .len(),
            10
        );
        let connection = upgraded.connection().unwrap();
        let recorded: Vec<(i64, String)> = {
            let mut statement=connection.prepare("SELECT version,checksum FROM schema_migrations WHERE version<=10 ORDER BY version").unwrap();
            statement
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap()
        };
        assert_eq!(recorded, ledger);
        assert!(
            connection
                .execute(
                    "INSERT INTO disclosure_grants VALUES('run-core','invalid',NULL)",
                    []
                )
                .is_err()
        );
        connection
            .execute(
                "INSERT INTO disclosure_grants VALUES('run-core','{}',NULL)",
                [],
            )
            .unwrap();
        assert!(
            connection
                .execute("UPDATE disclosure_grants SET revoked_at_epoch_ms=-1", [])
                .is_err()
        );
        assert!(
            connection
                .execute(
                    "INSERT INTO external_replays VALUES('replay','invalid','{}',0)",
                    []
                )
                .is_err()
        );
        assert!(
            connection
                .execute(
                    "INSERT INTO unavailable_objects VALUES(?1,'now','user_deleted')",
                    [Digest::from_bytes(b"missing").as_str()]
                )
                .is_err()
        );
        assert!(
            connection
                .execute(
                    "INSERT INTO evidence_tombstones VALUES('run-core','invalid','now')",
                    []
                )
                .is_err()
        );
        let error = connection
            .execute(
                "UPDATE runs SET parent_run_id=run_id WHERE run_id='run-core'",
                [],
            )
            .unwrap_err();
        assert!(error.to_string().contains("run ancestry is immutable"));
        drop(connection);
        drop(upgraded);
        let reader = StorageReader::open_read_only(&root).unwrap();
        assert_eq!(reader.identity().unwrap().schema_version, SCHEMA_VERSION);
        drop(reader);
        let connection = Connection::open(root.join("state/magi.sqlite")).unwrap();
        connection
            .execute(
                "UPDATE schema_migrations SET checksum=?1 WHERE version=11",
                [Digest::from_bytes(b"tampered").as_str()],
            )
            .unwrap();
        drop(connection);
        assert!(Storage::open_or_create(&root).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn migration_ten_upgrades_nine_and_ledger_verification_remains_strict() {
        let root = std::env::temp_dir().join(format!("magi-upgrade-store-{}", Uuid::new_v4()));
        let storage = Storage::open_or_create(&root).unwrap();
        let profile = storage
            .save_provider_profile(&profile("upgrade"), None, "2026-10-01T00:00:00Z")
            .unwrap();
        let catalog = catalog("upgrade-catalog", &profile, "model");
        storage.save_provider_catalog_snapshot(&catalog).unwrap();
        drop(storage);
        construct_historical_fixture(&root, 9);
        let storage = Storage::open_or_create(&root).unwrap();
        assert_eq!(storage.identity.schema_version, SCHEMA_VERSION);
        assert_eq!(
            storage
                .load_provider_catalog_snapshot("upgrade-catalog")
                .unwrap()
                .unwrap(),
            catalog
        );
        storage
            .select_provider_model(&model_input(&catalog, "model", None))
            .unwrap();
        drop(storage);
        let connection = Connection::open(root.join("state/magi.sqlite")).unwrap();
        connection
            .execute(
                "UPDATE schema_migrations SET checksum=?1 WHERE version=10",
                [Digest::from_bytes(b"tampered").as_str()],
            )
            .unwrap();
        drop(connection);
        assert!(Storage::open_or_create(&root).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
    fn cancellation_fixture() -> (PathBuf, Storage, RunAggregate) {
        cancellation_fixture_with_source(false)
    }
    fn cancellation_fixture_with_source(with_source: bool) -> (PathBuf, Storage, RunAggregate) {
        cancellation_fixture_with_source_bytes(with_source, None)
    }
    fn cancellation_fixture_with_source_bytes(
        with_source: bool,
        source_bytes: Option<&[u8]>,
    ) -> (PathBuf, Storage, RunAggregate) {
        cancellation_fixture_build(with_source, source_bytes, true)
    }

    fn cancellation_fixture_build(
        with_source: bool,
        source_bytes: Option<&[u8]>,
        admit: bool,
    ) -> (PathBuf, Storage, RunAggregate) {
        use magi_domain::*;
        let root = std::env::temp_dir().join(format!("magi-cancellation-{}", Uuid::new_v4()));
        let storage = Storage::open_or_create(&root).unwrap();
        let at = "2026-10-01T00:00:00Z";
        let mut references = Vec::new();
        for (index, core_id) in CoreId::ALL.into_iter().enumerate() {
            let id = format!("profile-{index}");
            let profile = storage
                .save_provider_profile(&profile(&id), None, at)
                .unwrap();
            let catalog = catalog(
                &format!("catalog-{index}"),
                &profile,
                &format!("model-{index}"),
            );
            storage.save_provider_catalog_snapshot(&catalog).unwrap();
            let selected = storage
                .select_provider_model(&model_input(&catalog, &format!("model-{index}"), None))
                .unwrap();
            let core = storage
                .select_core_model(&crate::CoreModelSelectionInput {
                    core_id,
                    provider_profile_id: id.clone(),
                    profile_revision: profile.revision,
                    model_selection_revision: selected.selection_revision,
                    expected_selection_revision: None,
                    updated_at: at.into(),
                })
                .unwrap();
            references.push(crate::CoreBindingReference {
                core_id,
                provider_profile_id: id,
                profile_revision: profile.revision,
                model_selection_revision: selected.selection_revision,
                core_selection_revision: core.selection_revision,
            });
        }
        let bindings = storage.resolve_core_bindings(&references).unwrap();
        let recipients: Vec<_> = bindings
            .iter()
            .map(|binding| magi_context::Recipient {
                provider_id: binding.provider_id.clone(),
                account_profile_id: binding.provider_profile_id.clone(),
            })
            .collect();
        let capture_manifest = if with_source {
            let original = storage
                .put_source_object(
                    source_bytes.unwrap_or(b"PRIVATE_BEFORE\nPUBLIC_SELECTED\nPRIVATE_AFTER\n"),
                )
                .unwrap();
            let derived = storage
                .put_source_object(source_bytes.unwrap_or(b"PUBLIC_SELECTED\n"))
                .unwrap();
            let locator = magi_context::EvidenceLocator {
                source_id: "memo".into(),
                object_digest: original.digest.clone(),
                start_line: Some(if source_bytes.is_some() { 1 } else { 2 }),
                end_line: Some(if source_bytes.is_some() { 1 } else { 2 }),
                total_lines: Some(if source_bytes.is_some() { 1 } else { 3 }),
                page: None,
                width: None,
                height: None,
            };
            let source = magi_context::ManifestSource {
                source_id: "memo".into(),
                display_name: "memo.txt".into(),
                state: ManifestSourceState::Captured,
                byte_length: Some(original.byte_length),
                mime_type: Some("text/plain".into()),
                object_digest: Some(original.digest),
                derived_digest: Some(derived.digest),
                representation_kind: Some(magi_context::RepresentationKind::Utf8Text),
                extractor_id: Some("utf8-text".into()),
                extractor_version: Some("1".into()),
                included_locators: vec![locator],
                omission: None,
                captured_at_epoch_ms: Some(1),
                secret_pattern_findings: vec![],
                secret_scan_incomplete: false,
            };
            let draft = SourceCaptureManifest::draft(vec![source], 1).unwrap();
            Some(
                draft
                    .confirm_disclosure(recipients.clone(), &draft.digest, 2)
                    .unwrap(),
            )
        } else {
            None
        };
        if let Some(manifest) = &capture_manifest {
            storage.commit_context_manifest(manifest).unwrap();
        }
        let make_input = |refs: &[crate::CoreBindingReference]| {
            let roles = CoreId::ALL.map(|core_id| {
                let index = CoreId::ALL.iter().position(|id| *id == core_id).unwrap();
                let binding = &bindings[index];
                CoreRoleProfile {
                    core_id,
                    profile_id: format!("role-{index}"),
                    revision: 0,
                    display_name: core_id.wire_name().into(),
                    review_purpose: "Review evidence".into(),
                    evaluation_criteria: vec!["Evidence".into()],
                    falsification_questions: vec!["What disproves it?".into()],
                    response_language: "en".into(),
                    binding: ModelBindingSnapshot {
                        provider_profile_id: binding.provider_profile_id.clone(),
                        revision: binding.profile_revision,
                        adapter_id: binding.adapter_id.clone(),
                        adapter_version: binding.adapter_version.clone(),
                        adapter_digest: binding.adapter_digest.clone(),
                        model_id: binding.model_id.clone(),
                        context_window_tokens: None,
                        maximum_output_tokens: None,
                    },
                    catalog_binding: Some(binding.clone()),
                }
            });
            InputSnapshot::new_with_request_provenance(
                QuestionSnapshot::new(
                    "question-core".into(),
                    QuestionKind::Answer,
                    "Evaluate evidence".into(),
                    vec![],
                    vec![],
                    vec![],
                )
                .unwrap(),
                capture_manifest
                    .as_ref()
                    .map(|manifest| manifest.run_manifest_for_recipient(&recipients[0]).unwrap())
                    .unwrap_or_else(|| {
                        ContextManifest::new("context-core".into(), vec![]).unwrap()
                    }),
                RoleSetSnapshot::new("roles-core".into(), roles)
                    .unwrap()
                    .with_frozen_core_selections(refs.to_vec().try_into().unwrap())
                    .unwrap(),
                Digest::from_bytes(b"policy"),
                DeliberationRequestProvenance {
                    context_draft_id: None,
                    context_revision: None,
                    role_preset_id: "preset-core".into(),
                    role_revision: 0,
                    disclosure_confirmed: true,
                },
            )
            .unwrap()
        };
        let input = make_input(&references);
        let mut aggregate = RunAggregate::new(
            magi_domain::Run::new(
                "run-core".into(),
                "conversation-core".into(),
                None,
                &input,
                at.into(),
            )
            .unwrap(),
            input,
        )
        .unwrap();
        let command = CommandEnvelope {
            command_id: "cancel-fixture-create".into(),
            idempotency_key: "cancel-fixture-create".into(),
            command_kind: CommandKind::CreateRun,
            target_id: "run-core".into(),
            expected_revision: 0,
            payload_digest: aggregate.input().input_digest.clone(),
        };
        let request = LiveRunAdmissionRequest {
            command_id: command.command_id.clone(),
            idempotency_key: command.idempotency_key.clone(),
            question: aggregate.input().question.prompt.clone(),
            model_binding: bindings[0].clone(),
        };
        if admit {
            storage
                .admit_deliberation_run(&command, &mut aggregate, &request, &references, at, None)
                .unwrap();
        }
        (root, storage, aggregate)
    }

    fn cancellation_claim(storage: &Storage, aggregate: &mut RunAggregate) -> LiveRunClaim {
        let at = "2026-10-01T00:00:01Z";
        let claim = storage
            .claim_next_live_run("cancel-worker", at)
            .unwrap()
            .unwrap();
        let revision = aggregate.run().revision;
        aggregate.request_confirmation(revision, at.into()).unwrap();
        aggregate
            .confirm_and_start(aggregate.run().revision, at.into())
            .unwrap();
        storage
            .commit_live_run_aggregate_transition(&claim, aggregate, revision)
            .unwrap();
        storage
            .mark_live_run_session_creation_intent(&claim, at)
            .unwrap();
        storage.mark_live_run_running(&claim, at).unwrap();
        storage
            .activate_live_run_dispatch_slot(&claim, 0, at)
            .unwrap();
        claim
    }

    #[test]
    fn run_deletion_purges_terminal_private_rows_and_retains_only_replay_authority() {
        for completed in [false, true] {
            let (root, storage, mut aggregate) = cancellation_fixture();
            let request = LiveRunAdmissionRequest {
                command_id: "cancel-fixture-create".into(),
                idempotency_key: "cancel-fixture-create".into(),
                question: aggregate.input().question.prompt.clone(),
                model_binding: aggregate.input().role_set.roles[0]
                    .catalog_binding
                    .clone()
                    .unwrap(),
            };
            let command = CommandEnvelope {
                command_id: request.command_id.clone(),
                idempotency_key: request.idempotency_key.clone(),
                command_kind: CommandKind::CreateRun,
                target_id: "run-core".into(),
                expected_revision: 0,
                payload_digest: aggregate.input().input_digest.clone(),
            };
            let mut late = None;
            if completed {
                let claim = cancellation_claim(&storage, &mut aggregate);
                let revision = aggregate.run().revision;
                cancellation_balloting(&mut aggregate);
                storage
                    .commit_live_run_aggregate_transition(&claim, &mut aggregate, revision)
                    .unwrap();
                {
                    let c = storage.connection().unwrap();
                    c.execute("UPDATE live_run_dispatch_reservations SET state='active' WHERE state='reserved'",[]).unwrap();
                    c.execute("UPDATE live_run_dispatch_reservations SET state='settled' WHERE state='active' AND slot_ordinal<9",[]).unwrap();
                }
                let revision = aggregate.run().revision;
                aggregate
                    .accept_ballot(cancellation_last_ballot(&aggregate), "fixture".into())
                    .unwrap();
                storage
                    .commit_aggregate_dispatch_transition(&claim, &mut aggregate, revision, 9)
                    .unwrap();
                let result = LiveProviderResultInput {
                    final_text: aggregate.persistence_state().proposal.unwrap().body,
                    stop_reason: "deliberation_completed".into(),
                    usage: None,
                };
                storage.finish_live_run(&claim, &result, "fixture").unwrap();
                late = Some((claim, result));
            } else {
                storage
                    .begin_deliberation_cancel("run-core", "fixture")
                    .unwrap();
            }
            let revision = storage
                .load_run_aggregate("run-core")
                .unwrap()
                .run()
                .revision;
            let admission_intent = admission_intent_from_input(
                &command.command_id,
                &command.idempotency_key,
                aggregate.input(),
            )
            .unwrap();
            let minimal_fence = storage
                .cancel_admission_request(
                    &admission_intent,
                    "retained-cancel",
                    "retained-cancel-key",
                    "fixture",
                )
                .unwrap();
            let fence_bytes = serde_json::to_vec(&minimal_fence).unwrap();
            let receipt = storage.delete_run("run-core", revision, "fixture").unwrap();
            assert!(receipt.deleted);
            if completed {
                assert_eq!(receipt.collected_objects, 1);
            }
            assert_eq!(
                serde_json::to_vec(
                    &storage
                        .load_admission_request_cancellation(&admission_intent)
                        .unwrap()
                        .unwrap()
                )
                .unwrap(),
                fence_bytes
            );
            let c = storage.connection().unwrap();
            for table in [
                "live_runs",
                "live_run_outbox",
                "live_run_events",
                "live_run_receipts",
                "live_run_cancellation_receipts",
                "live_run_dispatch_reservations",
                "commands",
                "role_assessments",
                "proposals",
                "ballots",
                "decision_dossiers",
                "run_events",
                "run_checkpoints",
                "immutable_snapshots",
            ] {
                assert_eq!(
                    c.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r
                        .get::<_, i64>(0))
                        .unwrap(),
                    0,
                    "{table}"
                );
            }
            assert!(
                c.query_row("SELECT count(*) FROM run_command_tombstones", [], |r| r
                    .get::<_, i64>(0))
                    .unwrap()
                    >= 2
            );
            assert_eq!(
                c.query_row(
                    "SELECT input_json||run_json||question_text FROM runs WHERE run_id='run-core'",
                    [],
                    |r| r.get::<_, String>(0)
                )
                .unwrap(),
                "{}{}"
            );
            let tx = c.unchecked_transaction().unwrap();
            assert!(matches!(
                find_duplicate_command(&tx, &command),
                Err(StorageError::RunNotFound(_))
            ));
            let mut changed = command.clone();
            changed.command_id = "new-command".into();
            changed.idempotency_key = "new-key".into();
            assert!(matches!(
                find_duplicate_command(&tx, &changed),
                Err(StorageError::RunNotFound(_))
            ));
            drop(tx);
            drop(c);
            assert!(matches!(
                storage.existing_live_run_receipt(&request),
                Err(StorageError::RunNotFound(_))
            ));
            let mut altered = request.clone();
            altered.question = "Different private question".into();
            assert!(matches!(
                storage.existing_live_run_receipt(&altered),
                Err(StorageError::IdempotencyConflict)
            ));
            if !completed {
                assert!(matches!(
                    storage.begin_deliberation_cancel("run-core", "retry"),
                    Err(StorageError::RunNotFound(_))
                ));
                let cancel_id = format!("deliberation-cancel:{}", Digest::from_bytes(b"run-core"));
                assert!(matches!(
                    storage.begin_live_run_cancel(
                        &cancel_id,
                        &cancel_id,
                        "different-run",
                        0,
                        "retry"
                    ),
                    Err(StorageError::IdempotencyConflict)
                ));
                let mut recycled = request.clone();
                recycled.command_id = cancel_id.clone();
                recycled.idempotency_key = cancel_id;
                assert!(matches!(
                    storage.admit_live_run(&recycled),
                    Err(StorageError::IdempotencyConflict)
                ));
            }
            let mut conflicting = request.clone();
            conflicting.command_id = "new-command".into();
            assert!(matches!(
                storage.admit_live_run(&conflicting),
                Err(StorageError::IdempotencyConflict)
            ));
            let orphan = late.as_ref().map(|(_, result)| {
                storage.object_path(&Digest::from_bytes(&serde_json::to_vec(result).unwrap()))
            });
            if let Some((claim, result)) = late {
                assert!(storage.finish_live_run(&claim, &result, "fixture").is_err());
                fs::write(
                    orphan.as_ref().unwrap(),
                    serde_json::to_vec(&result).unwrap(),
                )
                .unwrap();
            }
            assert_eq!(
                storage
                    .delete_run("run-core", 0, "retry")
                    .unwrap()
                    .collected_objects,
                0
            );
            if let Some(orphan) = orphan {
                assert!(!orphan.exists());
            }
            drop(storage);
            let reopened = Storage::open_or_create(&root).unwrap();
            assert!(matches!(
                reopened.existing_live_run_receipt(&request),
                Err(StorageError::RunNotFound(_))
            ));
            assert!(reopened.delete_run("run-core", 0, "retry").unwrap().deleted);
            assert!(!reopened.has_queued_live_runs().unwrap());
            drop(reopened);
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn run_deletion_preserves_shared_draft_objects_and_saved_core_preferences() {
        let (root, storage, _) = cancellation_fixture_with_source(true);
        let manifest = storage
            .load_run_dossier("run-core")
            .unwrap()
            .capture_manifest
            .unwrap();
        let draft = SourceCaptureManifest::draft(manifest.content.sources.clone(), 100).unwrap();
        assert_ne!(draft.manifest_id, manifest.manifest_id);
        storage
            .save_context_draft("shared-draft", None, &draft, 100)
            .unwrap();
        let preferences =
            CoreId::ALL.map(|core| storage.load_core_model_selection(core).unwrap().unwrap());
        let objects: Vec<_> = manifest
            .content
            .sources
            .iter()
            .flat_map(|source| {
                source
                    .object_digest
                    .iter()
                    .chain(source.derived_digest.iter())
            })
            .map(|digest| (digest.clone(), storage.read_source_object(digest).unwrap()))
            .collect();
        storage
            .begin_deliberation_cancel("run-core", "fixture")
            .unwrap();
        let revision = storage
            .load_run_aggregate("run-core")
            .unwrap()
            .run()
            .revision;
        assert_eq!(
            storage
                .delete_run("run-core", revision, "fixture")
                .unwrap()
                .collected_objects,
            0
        );
        assert_eq!(
            storage
                .load_context_draft("shared-draft")
                .unwrap()
                .unwrap()
                .manifest,
            draft
        );
        {
            let c = storage.connection().unwrap();
            for table in [
                "source_capture_manifests",
                "source_capture_manifest_objects",
            ] {
                assert_eq!(
                    c.query_row(
                        &format!("SELECT count(*) FROM {table} WHERE manifest_id=?1"),
                        [&manifest.manifest_id],
                        |r| r.get::<_, i64>(0)
                    )
                    .unwrap(),
                    0
                );
            }
            assert_eq!(
                c.query_row(
                    "SELECT count(*) FROM source_capture_manifests WHERE manifest_id=?1",
                    [&draft.manifest_id],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
                1
            );
        }
        for (digest, bytes) in objects {
            assert_eq!(storage.read_source_object(&digest).unwrap(), bytes);
        }
        assert_eq!(
            CoreId::ALL.map(|core| storage.load_core_model_selection(core).unwrap().unwrap()),
            preferences
        );
        drop(storage);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn run_deletion_removes_unique_capture_metadata_and_objects_with_scoped_authority() {
        let (root, storage, _) = cancellation_fixture_with_source(true);
        let manifest = storage
            .load_run_dossier("run-core")
            .unwrap()
            .capture_manifest
            .unwrap();
        let digests: Vec<_> = manifest
            .content
            .sources
            .iter()
            .flat_map(|source| {
                source
                    .object_digest
                    .iter()
                    .chain(source.derived_digest.iter())
            })
            .cloned()
            .collect();
        {
            let c = storage.connection().unwrap();
            assert!(
                c.execute(
                    "DELETE FROM source_capture_manifests WHERE manifest_id=?1",
                    [&manifest.manifest_id]
                )
                .is_err()
            );
            assert_eq!(
                c.query_row(
                    "SELECT count(*) FROM source_capture_manifests WHERE manifest_id=?1",
                    [&manifest.manifest_id],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
                1
            );
        }
        storage
            .begin_deliberation_cancel("run-core", "fixture")
            .unwrap();
        let revision = storage
            .load_run_aggregate("run-core")
            .unwrap()
            .run()
            .revision;
        storage.delete_run("run-core", revision, "fixture").unwrap();
        {
            let c = storage.connection().unwrap();
            for table in [
                "source_capture_manifests",
                "source_capture_manifest_objects",
            ] {
                assert_eq!(
                    c.query_row(
                        &format!("SELECT count(*) FROM {table} WHERE manifest_id=?1"),
                        [&manifest.manifest_id],
                        |r| r.get::<_, i64>(0)
                    )
                    .unwrap(),
                    0
                );
            }
            assert_eq!(c.query_row("SELECT manifest_digest FROM run_capture_manifest_tombstones WHERE run_id='run-core' AND manifest_id=?1",[&manifest.manifest_id],|r|r.get::<_,String>(0)).unwrap(),manifest.digest.to_string());
        }
        for digest in digests {
            assert!(storage.read_source_object(&digest).is_err());
        }
        drop(storage);
        let reopened = Storage::open_or_create(&root).unwrap();
        assert!(reopened.load_run_dossier("run-core").is_err());
        drop(reopened);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn run_deletion_redacted_inputs_require_exact_matching_tombstone_authority() {
        for fault in 0..4 {
            let (root, storage, aggregate) = cancellation_fixture();
            let c = storage.connection().unwrap();
            c.execute("UPDATE runs SET deleted_at='fixture',run_json='{}',input_json='{}',question_text='',tally_json=NULL WHERE run_id='run-core'",[]).unwrap();
            if fault != 0 {
                let id = if fault == 1 { "other-run" } else { "run-core" };
                let digest = if fault == 2 {
                    Digest::from_bytes(b"wrong")
                } else {
                    aggregate.input().input_digest.clone()
                };
                c.execute("INSERT INTO run_tombstones(run_id,conversation_id,deleted_at,payload_digest) VALUES(?1,'conversation-core','fixture',?2)",params![id,digest.as_str()]).unwrap();
                if fault == 3 {
                    c.execute("UPDATE runs SET question_text='private remains'", [])
                        .unwrap();
                }
            }
            assert!(validate_v2_data_contract(&c).is_err(), "fault {fault}");
            drop(c);
            assert!(storage.load_run_dossier("run-core").is_err());
            assert!(storage.delete_run("run-core", 0, "retry").is_err());
            drop(storage);
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn record_deletion_migration_preserves_all_eleven_checksums_and_frozen_authorities() {
        let (root, storage, aggregate) = cancellation_fixture();
        let input = aggregate.input().clone();
        drop(storage);
        construct_historical_fixture(&root, 11);
        let upgraded = Storage::open_or_create(&root).unwrap();
        assert_eq!(upgraded.identity.schema_version, SCHEMA_VERSION);
        assert_eq!(
            upgraded.load_run_aggregate("run-core").unwrap().input(),
            &input
        );
        let c = upgraded.connection().unwrap();
        for version in 1..=11 {
            let checksum: String = c
                .query_row(
                    "SELECT checksum FROM schema_migrations WHERE version=?1",
                    [version],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(
                checksum,
                Digest::from_bytes(migration_sql(version).unwrap().as_bytes()).to_string()
            );
        }
        assert_eq!(
            c.query_row(
                "SELECT count(*) FROM live_run_dispatch_reservations",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            10
        );
        assert_eq!(
            c.query_row("SELECT count(*) FROM core_model_selections", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            3
        );
        drop(c);
        drop(upgraded);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn run_deletion_rejects_corrupt_completed_dispatch_and_result_proof_atomically() {
        for fault in 0..7 {
            let (root, storage, mut aggregate) = cancellation_fixture();
            let claim = cancellation_claim(&storage, &mut aggregate);
            let revision = aggregate.run().revision;
            cancellation_balloting(&mut aggregate);
            storage
                .commit_live_run_aggregate_transition(&claim, &mut aggregate, revision)
                .unwrap();
            {
                let c = storage.connection().unwrap();
                c.execute("UPDATE live_run_dispatch_reservations SET state='active' WHERE state='reserved'",[]).unwrap();
                c.execute("UPDATE live_run_dispatch_reservations SET state='settled' WHERE state='active' AND slot_ordinal<9",[]).unwrap();
            }
            let revision = aggregate.run().revision;
            aggregate
                .accept_ballot(cancellation_last_ballot(&aggregate), "fixture".into())
                .unwrap();
            storage
                .commit_aggregate_dispatch_transition(&claim, &mut aggregate, revision, 9)
                .unwrap();
            storage
                .finish_live_run(
                    &claim,
                    &LiveProviderResultInput {
                        final_text: aggregate.persistence_state().proposal.unwrap().body,
                        stop_reason: "deliberation_completed".into(),
                        usage: None,
                    },
                    "fixture",
                )
                .unwrap();
            let revision = storage
                .load_run_aggregate("run-core")
                .unwrap()
                .run()
                .revision;
            let substituted = if fault >= 4 {
                let mut result = LiveProviderResultInput {
                    final_text: aggregate.persistence_state().proposal.unwrap().body,
                    stop_reason: "deliberation_completed".into(),
                    usage: None,
                };
                match fault {
                    4 => result.final_text = "Different structurally valid result".into(),
                    5 => result.stop_reason = "different_stop".into(),
                    _ => result.usage = Some(serde_json::from_str("{}").unwrap()),
                }
                let bytes = serde_json::to_vec(&result).unwrap();
                Some((storage.put_source_object(&bytes).unwrap(), bytes.len()))
            } else {
                None
            };
            {
                let c = storage.connection().unwrap();
                match fault {
                    0 => {
                        c.execute(
                            "DELETE FROM live_run_dispatch_reservations WHERE slot_ordinal=9",
                            [],
                        )
                        .unwrap();
                    }
                    1 => {
                        c.execute("DELETE FROM role_assessments WHERE core_id='CASPER-3'", [])
                            .unwrap();
                    }
                    2 => {
                        c.execute(
                            "UPDATE live_runs SET result_digest=NULL,result_byte_length=NULL",
                            [],
                        )
                        .unwrap();
                    }
                    3 => {
                        let digest: String = c
                            .query_row("SELECT result_digest FROM live_runs", [], |r| r.get(0))
                            .unwrap();
                        fs::write(
                            storage.object_path(&Digest::from_hex(digest).unwrap()),
                            b"corrupt result",
                        )
                        .unwrap();
                    }
                    _ => {
                        let (digest, length) = substituted.as_ref().unwrap();
                        c.execute(
                            "UPDATE live_runs SET result_digest=?1,result_byte_length=?2",
                            params![digest.digest.as_str(), length],
                        )
                        .unwrap();
                    }
                }
            }
            assert!(
                storage.get_live_run_snapshot("run-core", 0).is_err(),
                "fault {fault}"
            );
            assert!(
                storage.delete_run("run-core", revision, "fixture").is_err(),
                "fault {fault}"
            );
            let c = storage.connection().unwrap();
            assert_eq!(
                c.query_row("SELECT count(*) FROM run_tombstones", [], |r| r
                    .get::<_, i64>(0))
                    .unwrap(),
                0
            );
            assert_eq!(
                c.query_row("SELECT count(*) FROM run_command_tombstones", [], |r| r
                    .get::<_, i64>(0))
                    .unwrap(),
                0
            );
            assert_eq!(
                c.query_row("SELECT count(*) FROM commands", [], |r| r.get::<_, i64>(0))
                    .unwrap(),
                1
            );
            assert_eq!(
                c.query_row("SELECT question_text FROM live_runs", [], |r| r
                    .get::<_, String>(0))
                    .unwrap(),
                aggregate.input().question.prompt
            );
            drop(c);
            drop(storage);
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn run_deletion_refuses_pending_and_unknown_external_effects_without_mutation() {
        for unknown in [false, true] {
            let (root, storage, mut aggregate) = cancellation_fixture();
            if unknown {
                cancellation_claim(&storage, &mut aggregate);
                let cancelled = storage
                    .begin_deliberation_cancel("run-core", "fixture")
                    .unwrap();
                storage
                    .mark_live_run_cancel_unknown("run-core", cancelled.3, "fixture")
                    .unwrap();
            }
            let revision = storage
                .load_run_aggregate("run-core")
                .unwrap()
                .run()
                .revision;
            assert!(matches!(
                storage.delete_run("run-core", revision, "fixture"),
                Err(StorageError::DeletionBlocked)
            ));
            let c = storage.connection().unwrap();
            assert_eq!(
                c.query_row("SELECT count(*) FROM run_tombstones", [], |r| r
                    .get::<_, i64>(0))
                    .unwrap(),
                0
            );
            assert_eq!(
                c.query_row("SELECT count(*) FROM run_command_tombstones", [], |r| r
                    .get::<_, i64>(0))
                    .unwrap(),
                0
            );
            assert_eq!(
                c.query_row(
                    "SELECT count(*) FROM live_run_dispatch_reservations",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
                10
            );
            drop(c);
            drop(storage);
            fs::remove_dir_all(root).unwrap();
        }
    }

    fn guarded_fixture_admission_parts(
        aggregate: &RunAggregate,
    ) -> (
        CommandEnvelope,
        LiveRunAdmissionRequest,
        Vec<crate::CoreBindingReference>,
    ) {
        let references: Vec<crate::CoreBindingReference> = aggregate
            .input()
            .role_set
            .frozen_core_selections
            .as_ref()
            .unwrap()
            .iter()
            .map(|item| crate::CoreBindingReference {
                core_id: item.core_id,
                provider_profile_id: item.provider_profile_id.clone(),
                profile_revision: item.profile_revision,
                model_selection_revision: item.model_selection_revision,
                core_selection_revision: item.core_selection_revision,
            })
            .collect();
        let command = CommandEnvelope {
            command_id: "revoked-admission".into(),
            idempotency_key: "revoked-admission".into(),
            command_kind: CommandKind::CreateRun,
            target_id: "run-core".into(),
            expected_revision: 0,
            payload_digest: aggregate.input().input_digest.clone(),
        };
        let request = LiveRunAdmissionRequest {
            command_id: command.command_id.clone(),
            idempotency_key: command.idempotency_key.clone(),
            question: aggregate.input().question.prompt.clone(),
            model_binding: aggregate.input().role_set.roles[0]
                .catalog_binding
                .clone()
                .unwrap(),
        };
        (command, request, references)
    }

    fn admission_authority_fixture_intent(
        aggregate: &RunAggregate,
    ) -> crate::AdmissionRequestIntent {
        let (command, _, _) = guarded_fixture_admission_parts(aggregate);
        admission_intent_from_input(
            &command.command_id,
            &command.idempotency_key,
            aggregate.input(),
        )
        .unwrap()
    }

    struct FixtureActivationPermission(crate::AdmissionExecutionAuthority);
    impl crate::AdmissionActivationPermission for FixtureActivationPermission {
        fn validate(&self, prior: &crate::AdmissionExecutionAuthority) -> Result<(), StorageError> {
            if prior != &self.0 {
                return Err(StorageError::DispatchFenced);
            }
            Ok(())
        }
    }

    #[test]
    fn captured_execution_authority_is_checked_inside_admission_and_claim_transactions() {
        let (root, storage, mut aggregate) = cancellation_fixture_build(false, None, false);
        let old = storage.admission_execution_authority().unwrap();
        drop(storage);
        let storage = Storage::open_or_create(&root).unwrap();
        let current = storage.admission_execution_authority().unwrap();
        assert_eq!(old.lineage_id, current.lineage_id);
        assert!(current.store_generation > old.store_generation);
        let (command, request, refs) = guarded_fixture_admission_parts(&aggregate);
        let mut wrong_lineage = current.clone();
        wrong_lineage.lineage_id = "0".repeat(32);
        let mut inactive = current.clone();
        inactive.active = false;
        for stale in [&old, &wrong_lineage, &inactive] {
            assert!(matches!(
                storage.admit_deliberation_run_checked(
                    &command,
                    &mut aggregate,
                    &request,
                    &refs,
                    Storage::admission_publication_with_authority(
                        "fixture",
                        None,
                        stale,
                        || panic!("stale authority must not reach effects")
                    )
                ),
                Err(StorageError::DispatchFenced)
            ));
        }
        {
            let c = storage.connection().unwrap();
            for table in [
                "runs",
                "live_runs",
                "commands",
                "live_run_dispatch_reservations",
                "admission_request_bindings",
            ] {
                assert_eq!(
                    c.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r
                        .get::<_, u64>(0))
                        .unwrap(),
                    0
                );
            }
        }
        storage
            .admit_deliberation_run_checked(
                &command,
                &mut aggregate,
                &request,
                &refs,
                Storage::admission_publication_with_authority("fixture", None, &current, || Ok(())),
            )
            .unwrap();
        let before =
            serde_json::to_vec(&storage.get_live_run_snapshot("run-core", 0).unwrap()).unwrap();
        assert!(matches!(
            storage.claim_next_live_run_with_authority(&old, "worker", "fixture"),
            Err(StorageError::DispatchFenced)
        ));
        assert_eq!(
            serde_json::to_vec(&storage.get_live_run_snapshot("run-core", 0).unwrap()).unwrap(),
            before
        );
        let claim = storage
            .claim_next_live_run_with_authority(&current, "worker", "fixture")
            .unwrap()
            .unwrap();
        for expected in [&old, &current] {
            assert!(matches!(
                storage.load_frozen_deliberation_slot_with_authority(expected, &claim, 0),
                Err(StorageError::DispatchFenced)
            ));
        }
        assert!(matches!(
            storage.load_frozen_deliberation_role_with_authority(&old, &claim, CoreId::Casper3),
            Err(StorageError::DispatchFenced)
        ));
        assert_eq!(
            storage
                .load_frozen_deliberation_role_with_authority(&current, &claim, CoreId::Casper3)
                .unwrap()
                .core_id,
            CoreId::Casper3
        );
        storage
            .activate_live_run_dispatch_slot(&claim, 0, "fixture")
            .unwrap();
        assert!(matches!(
            storage.load_frozen_deliberation_slot_with_authority(&old, &claim, 0),
            Err(StorageError::DispatchFenced)
        ));
        assert_eq!(
            storage
                .load_frozen_deliberation_slot_with_authority(&current, &claim, 0)
                .unwrap()
                .core_id,
            CoreId::Melchior1
        );
        drop(storage);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn admission_lineage_restore_retires_imported_commands_and_requires_explicit_quiescent_switch()
    {
        let (root, mut original, mut aggregate) = cancellation_fixture_build(false, None, false);
        let first = original.admission_execution_authority().unwrap();
        let intent = admission_authority_fixture_intent(&aggregate);
        original
            .register_admission_request(&intent, "2026-10-01T00:00:00Z")
            .unwrap();
        let backup = root.join("lineage-backup");
        original.create_backup(&backup).unwrap();
        let later_cancel = original
            .cancel_admission_request(
                &intent,
                "later-cancel",
                "later-cancel-key",
                "2026-10-01T00:00:01Z",
            )
            .unwrap();
        let receipt_bytes = serde_json::to_vec(&later_cancel).unwrap();
        let destination = root.join("lineage-restored");
        Storage::restore_backup(&backup, &destination).unwrap();
        let mut restored = Storage::open_or_create(&destination).unwrap();
        let inert = restored.admission_execution_authority().unwrap();
        assert!(!inert.active);
        assert_ne!(inert.lineage_id, first.lineage_id);
        assert!(
            restored
                .validate_admission_execution_authority(&first)
                .is_err()
        );
        assert!(
            restored
                .load_admission_request_cancellation(&intent)
                .unwrap()
                .is_none()
        );
        assert!(
            restored
                .claim_next_live_run("worker", "2026-10-01T00:00:02Z")
                .is_err()
        );
        let mut alias = intent.clone();
        alias.command_id = "restored-alias".into();
        assert!(
            restored
                .register_admission_request(&alias, "2026-10-01T00:00:02Z")
                .is_err()
        );
        assert!(
            restored
                .activate_restored_execution_checked(&mut original, |_| Err::<
                    FixtureActivationPermission,
                    _,
                >(
                    StorageError::DispatchFenced
                ))
                .is_err()
        );
        assert!(original.admission_execution_authority().unwrap().active);
        let (active, _permission) = restored
            .activate_restored_execution_checked(&mut original, |authority| {
                assert_eq!(authority, &first);
                Ok(FixtureActivationPermission(authority.clone()))
            })
            .unwrap();
        assert!(active.active);
        assert!(active.store_generation > inert.store_generation.max(first.store_generation));
        assert!(!original.admission_execution_authority().unwrap().active);
        assert!(
            original
                .validate_admission_execution_authority(&first)
                .is_err()
        );
        assert!(
            restored
                .register_admission_request(&alias, "2026-10-01T00:00:03Z")
                .is_err()
        );
        let (command, request, refs) = guarded_fixture_admission_parts(&aggregate);
        assert!(
            restored
                .admit_deliberation_run_checked(
                    &command,
                    &mut aggregate,
                    &request,
                    &refs,
                    Storage::admission_publication("2026-10-01T00:00:03Z", None, || panic!(
                        "retired admission must not reach effect permission"
                    ))
                )
                .is_err()
        );
        assert!(restored.get_live_run_snapshot("run-core", 0).is_err());
        let mut fresh = intent.clone();
        fresh.command_id = "fresh-restored-command".into();
        fresh.idempotency_key = "fresh-restored-key".into();
        restored
            .register_admission_request(&fresh, "2026-10-01T00:00:03Z")
            .unwrap();
        assert_eq!(
            serde_json::to_vec(
                &original
                    .load_admission_request_cancellation(&intent)
                    .unwrap()
                    .unwrap()
            )
            .unwrap(),
            receipt_bytes
        );
        drop(restored);
        let reopened = Storage::open_or_create(&destination).unwrap();
        let reopened_authority = reopened.admission_execution_authority().unwrap();
        assert_eq!(reopened_authority.lineage_id, active.lineage_id);
        assert!(reopened_authority.store_generation > active.store_generation);
        assert!(
            reopened
                .validate_admission_execution_authority(&active)
                .is_err()
        );
        assert!(
            reopened
                .register_admission_request(&alias, "2026-10-01T00:00:04Z")
                .is_err()
        );
        reopened
            .register_admission_request(&fresh, "2026-10-01T00:00:04Z")
            .unwrap();
        drop(reopened);
        drop(original);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn admission_lineage_activation_failure_preserves_inert_roots_and_stale_claim_fence() {
        let (root, mut original, mut aggregate) = cancellation_fixture();
        let prior = original.admission_execution_authority().unwrap();
        let claim = cancellation_claim(&original, &mut aggregate);
        let backup = root.join("activation-backup");
        original.create_backup(&backup).unwrap();
        let target = root.join("activation-target");
        Storage::restore_backup(&backup, &target).unwrap();
        let mut restored = Storage::open_or_create(&target).unwrap();
        assert!(load_live_run_claim_state(&restored.connection().unwrap(), &claim).is_err());
        assert!(
            restored
                .activate_restored_execution_checked(&mut original, |p| Ok(
                    FixtureActivationPermission(p.clone())
                ))
                .is_err()
        );
        assert_eq!(original.admission_execution_authority().unwrap(), prior);
        drop(restored);
        drop(original);
        fs::remove_dir_all(root).unwrap();

        let root = std::env::temp_dir().join(format!("magi-lineage-activation-{}", Uuid::new_v4()));
        let mut original = Storage::open_or_create(root.join("old")).unwrap();
        let prior = original.admission_execution_authority().unwrap();
        original.create_backup(&root.join("backup")).unwrap();
        Storage::restore_backup(&root.join("backup"), &root.join("new")).unwrap();
        let mut restored = Storage::open_or_create(root.join("new")).unwrap();
        restored.connection().unwrap().execute_batch("CREATE TRIGGER fixture_activation_failure BEFORE UPDATE ON store_meta WHEN NEW.key='admission_activation_state' AND NEW.value='active' BEGIN SELECT RAISE(ABORT,'fixture activation failure'); END;").unwrap();
        assert!(
            restored
                .activate_restored_execution_checked(&mut original, |p| Ok(
                    FixtureActivationPermission(p.clone())
                ))
                .is_err()
        );
        assert!(!original.admission_execution_authority().unwrap().active);
        assert!(!restored.admission_execution_authority().unwrap().active);
        assert!(
            original
                .validate_admission_execution_authority(&prior)
                .is_err()
        );
        restored
            .connection()
            .unwrap()
            .execute_batch("DROP TRIGGER fixture_activation_failure;")
            .unwrap();
        drop(restored);
        drop(original);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn admission_authority_cancel_first_fences_alias_and_restart_without_effects() {
        let (root, storage, mut aggregate) = cancellation_fixture_build(false, None, false);
        let intent = admission_authority_fixture_intent(&aggregate);
        let receipt = storage
            .cancel_admission_request(
                &intent,
                "cancel-before",
                "cancel-before-key",
                "2026-10-01T00:00:00Z",
            )
            .unwrap();
        assert!(receipt.admitted_run_id.is_none());
        assert_eq!(
            storage
                .cancel_admission_request(
                    &intent,
                    "cancel-before",
                    "cancel-before-key",
                    "2026-10-01T00:00:01Z"
                )
                .unwrap(),
            receipt
        );
        let mut alias = intent.clone();
        alias.command_id = "start-alias".into();
        alias.core_bindings.reverse();
        assert!(matches!(
            storage.register_admission_request(&intent, "2026-10-01T00:00:02Z"),
            Err(StorageError::DispatchFenced)
        ));
        assert!(matches!(
            storage.register_admission_request(&alias, "2026-10-01T00:00:02Z"),
            Err(StorageError::DispatchFenced)
        ));
        let (mut command, mut request, refs) = guarded_fixture_admission_parts(&aggregate);
        for id in [&intent.command_id, &alias.command_id] {
            command.command_id = id.clone();
            request.command_id = id.clone();
            assert!(matches!(
                storage.admit_deliberation_run(
                    &command,
                    &mut aggregate,
                    &request,
                    &refs,
                    "2026-10-01T00:00:02Z",
                    None
                ),
                Err(StorageError::DispatchFenced)
            ));
        }
        assert!(!storage.has_persisted_run("run-core").unwrap());
        let c = storage.connection().unwrap();
        for table in [
            "runs",
            "commands",
            "live_run_receipts",
            "live_run_outbox",
            "live_run_dispatch_reservations",
        ] {
            assert_eq!(
                c.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r
                    .get::<_, i64>(0))
                    .unwrap(),
                0
            );
        }
        let serialized = serde_json::to_string(&receipt).unwrap();
        assert!(!serialized.contains(&intent.question));
        drop(c);
        drop(storage);
        let reopened = Storage::open_or_create(&root).unwrap();
        assert_eq!(
            reopened
                .load_admission_request_cancellation(&alias)
                .unwrap(),
            Some(receipt.clone())
        );
        assert!(matches!(
            reopened.register_admission_request(&alias, "2026-10-01T00:00:03Z"),
            Err(StorageError::DispatchFenced)
        ));
        let (mut command, mut request, refs) = guarded_fixture_admission_parts(&aggregate);
        command.command_id = alias.command_id.clone();
        request.command_id = alias.command_id.clone();
        assert!(matches!(
            reopened.admit_deliberation_run(
                &command,
                &mut aggregate,
                &request,
                &refs,
                "2026-10-01T00:00:03Z",
                None
            ),
            Err(StorageError::DispatchFenced)
        ));
        drop(reopened);
        let db = root.join("state/magi.sqlite");
        let before = fs::read(&db).unwrap();
        let reader = StorageReader::open_read_only(&root).unwrap();
        assert_eq!(
            reader.load_admission_request_cancellation(&alias).unwrap(),
            Some(receipt)
        );
        drop(reader);
        assert_eq!(fs::read(&db).unwrap(), before);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn admission_authority_registration_normalizes_order_and_rejects_conflicting_intents() {
        let (root, storage, aggregate) = cancellation_fixture_build(false, None, false);
        let intent = admission_authority_fixture_intent(&aggregate);
        let binding = storage
            .register_admission_request(&intent, "2026-10-01T00:00:00Z")
            .unwrap();
        let mut alias = intent.clone();
        alias.command_id = "alias".into();
        alias.core_bindings.reverse();
        let other = storage
            .register_admission_request(&alias, "2026-10-01T00:00:01Z")
            .unwrap();
        assert_eq!(binding.intent_digest, other.intent_digest);
        assert_eq!(
            binding,
            storage
                .register_admission_request(&intent, "2026-10-01T00:00:02Z")
                .unwrap()
        );
        for fault in 0..4 {
            let mut changed = intent.clone();
            match fault {
                0 => changed.question.push('!'),
                1 => changed.core_bindings[0].core_selection_revision += 1,
                2 => changed.request_provenance.role_revision += 1,
                _ => changed.idempotency_key = "different-key".into(),
            }
            assert!(matches!(
                storage.register_admission_request(&changed, "2026-10-01T00:00:03Z"),
                Err(StorageError::IdempotencyConflict)
            ));
            assert!(matches!(
                storage.cancel_admission_request(
                    &changed,
                    "cancel-conflict",
                    "cancel-key",
                    "2026-10-01T00:00:03Z"
                ),
                Err(StorageError::IdempotencyConflict)
            ));
        }
        let mut changed = intent.clone();
        changed.command_id = "new-alias".into();
        changed.question.push('!');
        assert!(matches!(
            storage.register_admission_request(&changed, "2026-10-01T00:00:03Z"),
            Err(StorageError::IdempotencyConflict)
        ));
        let receipt = storage
            .cancel_admission_request(&alias, "cancel", "cancel-key", "2026-10-01T00:00:03Z")
            .unwrap();
        assert!(matches!(
            storage.register_admission_request(&intent, "2026-10-01T00:00:03Z"),
            Err(StorageError::DispatchFenced)
        ));
        assert!(matches!(
            storage.cancel_admission_request(
                &intent,
                "cancel",
                "cancel-key",
                "2026-10-01T00:00:03Z"
            ),
            Err(StorageError::IdempotencyConflict)
        ));
        assert_eq!(
            storage
                .load_admission_request_cancellation(&intent)
                .unwrap(),
            Some(receipt)
        );
        let mut malformed = intent.clone();
        malformed.core_bindings[1] = malformed.core_bindings[0].clone();
        assert!(
            storage
                .register_admission_request(&malformed, "2026-10-01T00:00:03Z")
                .is_err()
        );
        let mut undisclosed = intent.clone();
        undisclosed.request_provenance.disclosure_confirmed = false;
        assert!(
            storage
                .register_admission_request(&undisclosed, "2026-10-01T00:00:03Z")
                .is_err()
        );
        drop(storage);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn admission_authority_commit_first_returns_exact_run_then_cancels_ten_slots() {
        let (root, storage, mut aggregate) = cancellation_fixture_build(false, None, false);
        let intent = admission_authority_fixture_intent(&aggregate);
        let (command, request, refs) = guarded_fixture_admission_parts(&aggregate);
        let accepted = storage
            .admit_deliberation_run(
                &command,
                &mut aggregate,
                &request,
                &refs,
                "2026-10-01T00:00:00Z",
                None,
            )
            .unwrap();
        let receipt = match accepted {
            LiveRunAdmissionOutcome::Accepted {
                receipt,
                duplicate: false,
            } => receipt,
            _ => panic!("genuine admission required"),
        };
        let cancellation = storage
            .cancel_admission_request(
                &intent,
                "cancel-admitted",
                "cancel-admitted-key",
                "2026-10-01T00:00:01Z",
            )
            .unwrap();
        assert_eq!(
            cancellation.admitted_run_id.as_deref(),
            Some(receipt.run_id.as_str())
        );
        storage
            .begin_deliberation_cancel(&receipt.run_id, "2026-10-01T00:00:01Z")
            .unwrap();
        let projection = storage
            .load_live_dispatch_projection(&receipt.run_id)
            .unwrap();
        assert_eq!(projection.dispatches.len(), 10);
        assert!(
            projection
                .dispatches
                .iter()
                .all(|slot| slot.state == crate::LiveDispatchState::Released)
        );
        assert!(
            storage
                .claim_next_live_run("late-worker", "2026-10-01T00:00:02Z")
                .unwrap()
                .is_none()
        );
        let mut replay = RunAggregate::new(
            magi_domain::Run::new(
                "run-core".into(),
                "conversation-core".into(),
                None,
                aggregate.input(),
                "2026-10-01T00:00:00Z".into(),
            )
            .unwrap(),
            aggregate.input().clone(),
        )
        .unwrap();
        assert!(
            matches!(storage.admit_deliberation_run_checked(&command,&mut replay,&request,&refs,Storage::admission_publication("2026-10-01T00:00:03Z",None,||panic!("replay cannot grant effects"))).unwrap(),LiveRunAdmissionOutcome::Accepted{receipt:prior,duplicate:true} if prior==receipt)
        );
        drop(storage);
        let reopened = Storage::open_or_create(&root).unwrap();
        assert_eq!(
            reopened
                .load_admission_request_cancellation(&intent)
                .unwrap(),
            Some(cancellation)
        );
        assert_eq!(
            reopened
                .get_live_run_snapshot("run-core", 0)
                .unwrap()
                .status,
            LiveRunStatus::Cancelled
        );
        drop(reopened);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn admission_authority_concurrent_cancel_orders_after_guarded_commit() {
        let (root, storage, mut aggregate) = cancellation_fixture_build(false, None, false);
        let storage = std::sync::Arc::new(storage);
        let intent = admission_authority_fixture_intent(&aggregate);
        let (command, request, refs) = guarded_fixture_admission_parts(&aggregate);
        let (commit_tx, commit_rx) = std::sync::mpsc::channel();
        let (cancel_tx, cancel_rx) = std::sync::mpsc::channel();
        let cancelling = storage.clone();
        let worker = std::thread::spawn(move || {
            commit_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            assert!(cancelling.connection.try_lock().is_err());
            cancel_tx.send(()).unwrap();
            let receipt = cancelling
                .cancel_admission_request(
                    &intent,
                    "concurrent-cancel",
                    "concurrent-cancel-key",
                    "2026-10-01T00:00:01Z",
                )
                .unwrap();
            let run_id = receipt.admitted_run_id.as_ref().unwrap();
            cancelling
                .begin_deliberation_cancel(run_id, "2026-10-01T00:00:01Z")
                .unwrap();
            receipt
        });
        let outcome = storage
            .admit_deliberation_run_checked(
                &command,
                &mut aggregate,
                &request,
                &refs,
                Storage::admission_publication("2026-10-01T00:00:00Z", None, || {
                    commit_tx.send(()).unwrap();
                    cancel_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                    Ok(())
                }),
            )
            .unwrap();
        let admitted = match outcome {
            LiveRunAdmissionOutcome::Accepted {
                receipt,
                duplicate: false,
            } => receipt,
            _ => panic!("genuine admission required"),
        };
        let stopped = worker.join().unwrap();
        assert_eq!(
            stopped.admitted_run_id.as_deref(),
            Some(admitted.run_id.as_str())
        );
        let slots = storage
            .load_live_dispatch_projection(&admitted.run_id)
            .unwrap();
        assert_eq!(slots.dispatches.len(), 10);
        assert!(
            slots
                .dispatches
                .iter()
                .all(|slot| slot.state == crate::LiveDispatchState::Released)
        );
        assert!(
            storage
                .claim_next_live_run("late", "2026-10-01T00:00:02Z")
                .unwrap()
                .is_none()
        );
        drop(storage);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn admission_authority_rejects_direct_live_identity_collisions_both_orders() {
        let (root, storage, aggregate) = cancellation_fixture_build(false, None, false);
        let mut intent = admission_authority_fixture_intent(&aggregate);
        let (_, mut direct, _) = guarded_fixture_admission_parts(&aggregate);
        direct.command_id = "direct-command".into();
        direct.idempotency_key = "direct-key".into();
        let receipt = storage.admit_live_run(&direct).unwrap();
        assert!(matches!(
            receipt,
            LiveRunAdmissionOutcome::Accepted {
                duplicate: false,
                ..
            }
        ));
        for by_key in [false, true] {
            let mut collision = intent.clone();
            if by_key {
                collision.idempotency_key = direct.idempotency_key.clone();
            } else {
                collision.command_id = direct.command_id.clone();
            }
            assert!(matches!(
                storage.register_admission_request(&collision, "2026-10-01T00:00:00Z"),
                Err(StorageError::IdempotencyConflict)
            ));
            assert!(matches!(
                storage.cancel_admission_request(
                    &collision,
                    "cancel",
                    "cancel-key",
                    "2026-10-01T00:00:00Z"
                ),
                Err(StorageError::IdempotencyConflict)
            ));
        }
        assert!(matches!(
            storage.cancel_admission_request(
                &intent,
                &direct.command_id,
                "cancel-key",
                "2026-10-01T00:00:00Z"
            ),
            Err(StorageError::IdempotencyConflict)
        ));
        assert!(matches!(
            storage.cancel_admission_request(
                &intent,
                "cancel",
                &direct.idempotency_key,
                "2026-10-01T00:00:00Z"
            ),
            Err(StorageError::IdempotencyConflict)
        ));
        storage
            .cancel_admission_request(&intent, "cancel", "cancel-key", "2026-10-01T00:00:00Z")
            .unwrap();
        for (id, key) in [
            (intent.command_id.clone(), "other-key".into()),
            ("other-command".into(), intent.idempotency_key.clone()),
            ("cancel".into(), "other-key".into()),
            ("other-command".into(), "cancel-key".into()),
        ] {
            direct.command_id = id;
            direct.idempotency_key = key;
            assert!(matches!(
                storage.admit_live_run(&direct),
                Err(StorageError::IdempotencyConflict)
            ));
        }
        intent.command_id = "alias-after-direct".into();
        assert!(matches!(
            storage.register_admission_request(&intent, "2026-10-01T00:00:00Z"),
            Err(StorageError::DispatchFenced)
        ));
        drop(storage);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn admission_authority_receipt_corruption_and_transaction_failure_fail_closed() {
        let (root, storage, aggregate) = cancellation_fixture_build(false, None, false);
        let intent = admission_authority_fixture_intent(&aggregate);
        let c = storage.connection().unwrap();
        c.execute_batch("CREATE TRIGGER test_cancel_failure BEFORE INSERT ON admission_request_cancellations BEGIN SELECT RAISE(ABORT,'injected failure'); END;").unwrap();
        drop(c);
        assert!(
            storage
                .cancel_admission_request(&intent, "cancel", "cancel-key", "2026-10-01T00:00:00Z")
                .is_err()
        );
        let c = storage.connection().unwrap();
        assert_eq!(
            c.query_row("SELECT count(*) FROM admission_request_bindings", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        c.execute_batch("DROP TRIGGER test_cancel_failure;")
            .unwrap();
        drop(c);
        storage
            .cancel_admission_request(&intent, "cancel", "cancel-key", "2026-10-01T00:00:00Z")
            .unwrap();
        let c = storage.connection().unwrap();
        assert!(
            c.execute("DELETE FROM admission_request_cancellations", [])
                .is_err()
        );
        assert!(
            c.execute(
                "UPDATE admission_request_bindings SET intent_digest=?1",
                [Digest::from_bytes(b"changed").as_str()]
            )
            .is_err()
        );
        c.execute_batch("DROP TRIGGER admission_request_cancellations_no_update;")
            .unwrap();
        c.execute("UPDATE admission_request_cancellations SET receipt_json='{\"schemaVersion\":1,\"private_token_canary\":true}'",[]).unwrap();
        c.execute_batch("CREATE TRIGGER admission_request_cancellations_no_update BEFORE UPDATE ON admission_request_cancellations BEGIN SELECT RAISE(ABORT,'admission cancellation authority is immutable'); END;").unwrap();
        drop(c);
        let error = storage
            .load_admission_request_cancellation(&intent)
            .unwrap_err();
        assert!(!error.to_string().contains("private_token_canary"));
        drop(storage);
        assert!(Storage::open_or_create(&root).is_err());
        assert!(StorageReader::open_read_only(&root).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn admission_authority_migration_preserves_twelve_checksums_and_frozen_admission() {
        let (root, storage, aggregate) = cancellation_fixture_build(false, None, true);
        let before = canonical_json(&aggregate.input()).unwrap();
        let identity = storage.identity().clone();
        let c = storage.connection().unwrap();
        let ledger: Vec<(u32, String)> = c
            .prepare(
                "SELECT version,checksum FROM schema_migrations WHERE version<=12 ORDER BY version",
            )
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        drop(c);
        drop(storage);
        let database = root.join("state/magi.sqlite");
        let source = root.join("state/fixture-source.sqlite");
        fs::rename(&database, &source).unwrap();
        let twelve = Connection::open(&database).unwrap();
        twelve.execute_batch("CREATE TABLE schema_migrations(version INTEGER PRIMARY KEY CHECK(version>0),checksum TEXT NOT NULL,applied_at TEXT NOT NULL);").unwrap();
        for version in 1..=12 {
            twelve
                .execute_batch(migration_sql(version).unwrap())
                .unwrap();
        }
        twelve
            .execute("ATTACH DATABASE ?1 AS prior", [source.to_str().unwrap()])
            .unwrap();
        let tables:Vec<String>=twelve.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' AND name<>'schema_migrations' ORDER BY rowid").unwrap().query_map([],|row|row.get(0)).unwrap().map(Result::unwrap).collect();
        for table in tables {
            if table == "store_meta" {
                twelve.execute_batch("DELETE FROM store_meta;").unwrap();
            }
            twelve
                .execute_batch(&format!(
                    "INSERT INTO main.\"{table}\" SELECT * FROM prior.\"{table}\";"
                ))
                .unwrap();
        }
        twelve.execute_batch("INSERT INTO schema_migrations SELECT * FROM prior.schema_migrations WHERE version<=12; PRAGMA user_version=12; UPDATE store_meta SET value='12' WHERE key='schema_version'; DELETE FROM store_meta WHERE key IN ('admission_lineage_id','admission_activation_state'); DETACH DATABASE prior;").unwrap();
        read_only_store_identity(&twelve).unwrap();
        drop(twelve);
        fs::remove_file(source).unwrap();
        let reader = StorageReader::open_read_only(&root).unwrap();
        assert_eq!(
            reader.get_live_run_snapshot("run-core", 0).unwrap().run_id,
            "run-core"
        );
        assert!(
            reader
                .load_admission_request_cancellation(&admission_authority_fixture_intent(
                    &aggregate
                ))
                .unwrap()
                .is_none()
        );
        drop(reader);
        let upgraded = Storage::open_or_create(&root).unwrap();
        assert_eq!(upgraded.identity().store_id, identity.store_id);
        assert_eq!(upgraded.identity().generation, identity.generation + 1);
        assert_eq!(upgraded.identity().schema_version, SCHEMA_VERSION);
        assert_eq!(
            canonical_json(upgraded.load_run_aggregate("run-core").unwrap().input()).unwrap(),
            before
        );
        assert_eq!(
            upgraded
                .load_live_dispatch_projection("run-core")
                .unwrap()
                .dispatches
                .len(),
            10
        );
        let c = upgraded.connection().unwrap();
        let after: Vec<(u32, String)> = c
            .prepare(
                "SELECT version,checksum FROM schema_migrations WHERE version<=12 ORDER BY version",
            )
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(after, ledger);
        c.execute(
            "UPDATE schema_migrations SET checksum=?1 WHERE version=13",
            [Digest::from_bytes(b"wrong").as_str()],
        )
        .unwrap();
        drop(c);
        drop(upgraded);
        assert!(matches!(
            StorageReader::open_read_only(&root),
            Err(StorageError::MigrationChecksum { version: 13 })
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn admission_lineage_migration_preserves_thirteen_checksums_and_frozen_admission() {
        let (root, storage, aggregate) = cancellation_fixture_build(false, None, true);
        let before = canonical_json(&aggregate.input()).unwrap();
        let identity = storage.identity().clone();
        let c = storage.connection().unwrap();
        let ledger: Vec<(u32, String)> = c
            .prepare(
                "SELECT version,checksum FROM schema_migrations WHERE version<=13 ORDER BY version",
            )
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        drop(c);
        drop(storage);
        let database = root.join("state/magi.sqlite");
        let source = root.join("state/fixture-source.sqlite");
        fs::rename(&database, &source).unwrap();
        let twelve = Connection::open(&database).unwrap();
        twelve.execute_batch("CREATE TABLE schema_migrations(version INTEGER PRIMARY KEY CHECK(version>0),checksum TEXT NOT NULL,applied_at TEXT NOT NULL);").unwrap();
        for version in 1..=13 {
            twelve
                .execute_batch(migration_sql(version).unwrap())
                .unwrap();
        }
        twelve
            .execute("ATTACH DATABASE ?1 AS prior", [source.to_str().unwrap()])
            .unwrap();
        let tables:Vec<String>=twelve.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' AND name<>'schema_migrations' ORDER BY rowid").unwrap().query_map([],|row|row.get(0)).unwrap().map(Result::unwrap).collect();
        for table in tables {
            if table == "store_meta" {
                twelve.execute_batch("DELETE FROM store_meta;").unwrap();
            }
            twelve
                .execute_batch(&format!(
                    "INSERT INTO main.\"{table}\" SELECT * FROM prior.\"{table}\";"
                ))
                .unwrap();
        }
        twelve.execute_batch("INSERT INTO schema_migrations SELECT * FROM prior.schema_migrations WHERE version<=13; PRAGMA user_version=13; UPDATE store_meta SET value='13' WHERE key='schema_version'; DELETE FROM store_meta WHERE key IN ('admission_lineage_id','admission_activation_state'); DETACH DATABASE prior;").unwrap();
        read_only_store_identity(&twelve).unwrap();
        drop(twelve);
        fs::remove_file(source).unwrap();
        let reader = StorageReader::open_read_only(&root).unwrap();
        assert_eq!(
            reader.get_live_run_snapshot("run-core", 0).unwrap().run_id,
            "run-core"
        );
        assert!(
            reader
                .load_admission_request_cancellation(&admission_authority_fixture_intent(
                    &aggregate
                ))
                .unwrap()
                .is_none()
        );
        drop(reader);
        let upgraded = Storage::open_or_create(&root).unwrap();
        assert_eq!(upgraded.identity().store_id, identity.store_id);
        assert_eq!(upgraded.identity().generation, identity.generation + 1);
        assert_eq!(upgraded.identity().schema_version, SCHEMA_VERSION);
        assert_eq!(
            canonical_json(upgraded.load_run_aggregate("run-core").unwrap().input()).unwrap(),
            before
        );
        assert_eq!(
            upgraded
                .load_live_dispatch_projection("run-core")
                .unwrap()
                .dispatches
                .len(),
            10
        );
        let c = upgraded.connection().unwrap();
        let after: Vec<(u32, String)> = c
            .prepare(
                "SELECT version,checksum FROM schema_migrations WHERE version<=13 ORDER BY version",
            )
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(after, ledger);
        c.execute(
            "UPDATE schema_migrations SET checksum=?1 WHERE version=13",
            [Digest::from_bytes(b"wrong").as_str()],
        )
        .unwrap();
        drop(c);
        drop(upgraded);
        assert!(matches!(
            StorageReader::open_read_only(&root),
            Err(StorageError::MigrationChecksum { version: 13 })
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn revoked_final_admission_permission_rolls_back_all_durable_authority() {
        let (root, storage, mut aggregate) = cancellation_fixture_build(false, None, false);
        let before = canonical_json(&aggregate.persistence_state()).unwrap();
        let (command, request, references) = guarded_fixture_admission_parts(&aggregate);
        let called = std::sync::atomic::AtomicBool::new(false);
        assert!(matches!(
            storage.admit_deliberation_run_checked(
                &command,
                &mut aggregate,
                &request,
                &references,
                Storage::admission_publication("2026-10-01T00:00:00Z", None, || {
                    called.store(true, std::sync::atomic::Ordering::SeqCst);
                    Err(StorageError::DispatchFenced)
                })
            ),
            Err(StorageError::DispatchFenced)
        ));
        assert!(called.load(std::sync::atomic::Ordering::SeqCst));
        assert_eq!(
            canonical_json(&aggregate.persistence_state()).unwrap(),
            before
        );
        assert!(!storage.has_persisted_run("run-core").unwrap());
        let connection = storage.connection().unwrap();
        for table in [
            "live_run_receipts",
            "live_run_outbox",
            "live_run_dispatch_reservations",
            "admission_request_bindings",
        ] {
            let count: i64 = connection
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(count, 0);
        }
        drop(connection);
        drop(storage);
        let reopened = Storage::open_or_create(&root).unwrap();
        assert!(!reopened.has_persisted_run("run-core").unwrap());
        drop(reopened);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn admission_committing_first_preserves_receipt_then_durably_cancels_before_dispatch() {
        let (root, storage, mut aggregate) = cancellation_fixture_build(false, None, false);
        let (command, request, references) = guarded_fixture_admission_parts(&aggregate);
        let outcome = storage
            .admit_deliberation_run_checked(
                &command,
                &mut aggregate,
                &request,
                &references,
                Storage::admission_publication("2026-10-01T00:00:00Z", None, || Ok(())),
            )
            .unwrap();
        let receipt = match outcome {
            LiveRunAdmissionOutcome::Accepted {
                receipt,
                duplicate: false,
            } => receipt,
            _ => panic!("Genuine admission required"),
        };
        let cancellation = storage
            .begin_deliberation_cancel(&receipt.run_id, "2026-10-01T00:00:01Z")
            .unwrap();
        assert_eq!(cancellation.0, LiveRunStatus::Cancelled);
        let live = storage.get_live_run_snapshot(&receipt.run_id, 0).unwrap();
        assert_eq!(live.status, LiveRunStatus::Cancelled);
        assert!(live.result.is_none());
        let projection = storage
            .load_live_dispatch_projection(&receipt.run_id)
            .unwrap();
        assert_eq!(projection.dispatches.len(), 10);
        assert!(
            projection
                .dispatches
                .iter()
                .all(|slot| slot.state == crate::LiveDispatchState::Released
                    && slot.result_ref.is_none())
        );
        assert!(
            storage
                .claim_next_live_run("late-dispatch-worker", "2026-10-01T00:00:02Z")
                .unwrap()
                .is_none()
        );
        let replay_input = aggregate.input().clone();
        let mut replay = RunAggregate::new(
            magi_domain::Run::new(
                "run-core".into(),
                "conversation-core".into(),
                None,
                &replay_input,
                "2026-10-01T00:00:00Z".into(),
            )
            .unwrap(),
            replay_input,
        )
        .unwrap();
        let duplicate_admission = storage
            .admit_deliberation_run_checked(
                &command,
                &mut replay,
                &request,
                &references,
                Storage::admission_publication("2026-10-01T00:00:00Z", None, || {
                    panic!("Duplicate cannot acquire new execution permission")
                }),
            )
            .unwrap();
        assert!(
            matches!(duplicate_admission,LiveRunAdmissionOutcome::Accepted{receipt:ref prior,duplicate:true} if prior==&receipt)
        );
        let duplicate = storage
            .begin_deliberation_cancel(&receipt.run_id, "2026-10-01T00:00:03Z")
            .unwrap();
        assert_eq!(duplicate.2, cancellation.2);
        let rows: i64 = storage
            .connection()
            .unwrap()
            .query_row(
                "SELECT count(*) FROM live_run_cancellation_receipts",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(rows, 1);
        drop(storage);
        let reader = StorageReader::open_read_only(&root).unwrap();
        assert_eq!(
            reader
                .load_live_dispatch_projection(&receipt.run_id)
                .unwrap(),
            projection
        );
        drop(reader);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn admitted_preparing_start_reopens_and_commits_legal_confirmation_before_effects() {
        let (root, storage, _) = cancellation_fixture();
        drop(storage);
        let storage = Storage::open_or_create(&root).unwrap();
        let mut aggregate = storage.load_run_aggregate("run-core").unwrap();
        assert!(matches!(aggregate.run().status, RunStatus::Preparing));
        assert_eq!(aggregate.run().revision, 0);
        assert!(matches!(
            aggregate
                .clone()
                .confirm_and_start(0, "2026-10-01T00:00:01Z".into()),
            Err(magi_domain::DomainError::InvalidTransition { .. })
        ));
        let claim = storage
            .claim_next_live_run("legal-start-worker", "2026-10-01T00:00:01Z")
            .unwrap()
            .unwrap();
        storage
            .start_admitted_deliberation(&claim, &mut aggregate, 0, "2026-10-01T00:00:02Z")
            .unwrap();
        assert!(matches!(
            aggregate.run().status,
            RunStatus::IndependentReview
        ));
        assert_eq!(aggregate.run().revision, 2);
        assert!(aggregate.events().is_empty());
        let events = {
            let connection = storage.connection().unwrap();
            let mut statement = connection.prepare("SELECT event_type, run_revision FROM run_events WHERE run_id='run-core' ORDER BY sequence").unwrap();
            statement
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
                })
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        assert_eq!(
            events[events.len() - 2],
            ("confirmation_required".into(), 1)
        );
        assert_eq!(events[events.len() - 1], ("run_started".into(), 2));
        let before = storage.load_run_dossier("run-core").unwrap();
        assert!(
            storage
                .start_admitted_deliberation(&claim, &mut aggregate, 2, "2026-10-01T00:00:03Z")
                .is_err()
        );
        assert_eq!(storage.load_run_dossier("run-core").unwrap(), before);
        let connection = storage.connection().unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM live_run_dispatch_reservations WHERE state='reserved'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            10
        );
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM role_assessments", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        drop(connection);
        drop(storage);
        let reader = StorageReader::open_read_only(&root).unwrap();
        assert_eq!(reader.load_run_dossier("run-core").unwrap(), before);
        drop(reader);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn admitted_preparing_start_fences_stale_claim_revision_cancel_and_transaction_failure() {
        for failure in ["claim", "revision", "cancel", "transaction"] {
            let (root, storage, _) = cancellation_fixture();
            let mut aggregate = storage.load_run_aggregate("run-core").unwrap();
            let mut claim = storage
                .claim_next_live_run("legal-start-worker", "2026-10-01T00:00:01Z")
                .unwrap()
                .unwrap();
            let expected = if failure == "revision" { 1 } else { 0 };
            if failure == "claim" {
                claim.claim_generation += 1;
            }
            if failure == "cancel" {
                storage
                    .begin_deliberation_cancel("run-core", "2026-10-01T00:00:02Z")
                    .unwrap();
            }
            let before = storage.load_run_dossier("run-core").unwrap();
            let events_before: i64 = storage
                .connection()
                .unwrap()
                .query_row("SELECT COUNT(*) FROM run_events", [], |r| r.get(0))
                .unwrap();
            if failure == "transaction" {
                storage.connection().unwrap().execute_batch("CREATE TRIGGER controlled_start_failure BEFORE UPDATE OF status ON runs WHEN NEW.status='independent_review' BEGIN SELECT RAISE(ABORT,'controlled start failure'); END;").unwrap();
            }
            assert!(
                storage
                    .start_admitted_deliberation(
                        &claim,
                        &mut aggregate,
                        expected,
                        "2026-10-01T00:00:03Z"
                    )
                    .is_err()
            );
            if failure == "transaction" {
                storage
                    .connection()
                    .unwrap()
                    .execute_batch("DROP TRIGGER controlled_start_failure;")
                    .unwrap();
            }
            assert!(matches!(aggregate.run().status, RunStatus::Preparing));
            assert_eq!(aggregate.run().revision, 0);
            assert!(aggregate.events().is_empty());
            assert_eq!(storage.load_run_dossier("run-core").unwrap(), before);
            let connection = storage.connection().unwrap();
            assert_eq!(
                connection
                    .query_row("SELECT COUNT(*) FROM run_events", [], |r| r
                        .get::<_, i64>(0))
                    .unwrap(),
                events_before
            );
            assert_eq!(connection.query_row("SELECT COUNT(*) FROM live_run_dispatch_reservations WHERE state IN ('active','settled','unknown')", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
            assert_eq!(
                connection
                    .query_row("SELECT COUNT(*) FROM role_assessments", [], |r| r
                        .get::<_, i64>(0))
                    .unwrap(),
                0
            );
            drop(connection);
            drop(storage);
            let reader = StorageReader::open_read_only(&root).unwrap();
            assert_eq!(reader.load_run_dossier("run-core").unwrap(), before);
            drop(reader);
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn claimed_prestart_cancel_settles_not_started_atomically_and_preserves_acceptance() {
        for rollback in [false, true] {
            let (root, storage, _) = cancellation_fixture();
            let claim = storage
                .claim_next_live_run("prestart-worker", "2026-10-01T00:00:01Z")
                .unwrap()
                .unwrap();
            let before = storage.load_run_dossier("run-core").unwrap();
            if rollback {
                storage.connection().unwrap().execute_batch("CREATE TRIGGER controlled_cancel_failure BEFORE UPDATE OF provider_outcome ON live_run_cancellation_receipts WHEN NEW.provider_outcome='not_started' BEGIN SELECT RAISE(ABORT,'controlled cancellation failure'); END;").unwrap();
                assert!(
                    storage
                        .begin_deliberation_cancel("run-core", "2026-10-01T00:00:02Z")
                        .is_err()
                );
                let c = storage.connection().unwrap();
                assert_eq!(
                    c.query_row(
                        "SELECT count(*) FROM live_run_cancellation_receipts",
                        [],
                        |r| r.get::<_, i64>(0)
                    )
                    .unwrap(),
                    0
                );
                assert_eq!(c.query_row("SELECT count(*) FROM live_run_dispatch_reservations WHERE state='reserved'", [], |r| r.get::<_,i64>(0)).unwrap(), 10);
                assert_eq!(
                    c.query_row(
                        "SELECT state FROM live_run_outbox WHERE run_id='run-core'",
                        [],
                        |r| r.get::<_, String>(0)
                    )
                    .unwrap(),
                    "claimed"
                );
                c.execute_batch("DROP TRIGGER controlled_cancel_failure;")
                    .unwrap();
                drop(c);
                assert_eq!(storage.load_run_dossier("run-core").unwrap(), before);
            }
            let result = storage
                .begin_deliberation_cancel("run-core", "2026-10-01T00:00:03Z")
                .unwrap();
            assert_eq!(result.0, LiveRunStatus::Cancelled);
            assert_eq!(result.1, LiveRunStatus::Claimed);
            assert_eq!(result.2, 2);
            assert_eq!(result.4.revision, 3);
            let duplicate = storage
                .begin_deliberation_cancel("run-core", "2026-10-01T00:00:04Z")
                .unwrap();
            assert!(duplicate.5);
            assert_eq!(duplicate.2, result.2);
            assert_eq!(duplicate.4, result.4);
            assert_eq!(duplicate.6, result.6);
            let live = storage.get_live_run_snapshot("run-core", 0).unwrap();
            assert_eq!(live.revision, result.4.revision);
            let c = storage.connection().unwrap();
            let receipt: (String,String,i64,i64) = c.query_row("SELECT origin_status,provider_outcome,accepted_revision,outcome_revision FROM live_run_cancellation_receipts", [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).unwrap();
            assert_eq!(receipt, ("claimed".into(), "not_started".into(), 2, 3));
            assert_eq!(
                c.query_row(
                    "SELECT count(*) FROM live_run_dispatch_reservations WHERE state='released'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
                10
            );
            assert_eq!(
                c.query_row("SELECT count(*) FROM role_assessments", [], |r| r
                    .get::<_, i64>(0))
                    .unwrap(),
                0
            );
            drop(c);
            assert!(
                storage
                    .load_frozen_deliberation_role(&claim, CoreId::Melchior1)
                    .is_err()
            );
            assert!(
                storage
                    .finish_live_run_cancel(
                        "run-core",
                        claim.claim_generation,
                        LiveRunProviderOutcome::Confirmed,
                        "2026-10-01T00:00:05Z"
                    )
                    .is_err()
            );
            let final_dossier = storage.load_run_dossier("run-core").unwrap();
            drop(storage);
            let reader = StorageReader::open_read_only(&root).unwrap();
            assert_eq!(reader.load_run_dossier("run-core").unwrap(), final_dossier);
            drop(reader);
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn cancellation_prestart_and_confirmation_release_ten_slots_without_external_dispatch() {
        for awaiting in [false, true] {
            let (root, storage, mut aggregate) = cancellation_fixture();
            if awaiting {
                let revision = aggregate.run().revision;
                aggregate
                    .request_confirmation(revision, "2026-10-01T00:00:01Z".into())
                    .unwrap();
                storage
                    .commit_aggregate_transition(&mut aggregate, revision)
                    .unwrap();
            }
            let result = storage
                .begin_deliberation_cancel("run-core", "2026-10-01T00:00:02Z")
                .unwrap();
            assert_eq!(result.0, LiveRunStatus::Cancelled);
            assert!(matches!(
                storage.load_run_aggregate("run-core").unwrap().run().status,
                RunStatus::Cancelled
            ));
            assert!(
                storage
                    .claim_next_live_run("worker", "2026-10-01T00:00:03Z")
                    .unwrap()
                    .is_none()
            );
            let connection = storage.connection().unwrap();
            assert_eq!(connection.query_row("SELECT COUNT(*) FROM live_run_dispatch_reservations WHERE state='released'",[],|r|r.get::<_,i64>(0)).unwrap(),10);
            assert_eq!(connection.query_row("SELECT COUNT(*) FROM dispatch_outbox WHERE state IN ('dispatched','unknown')",[],|r|r.get::<_,i64>(0)).unwrap(),0);
            drop(connection);
            assert!(
                storage
                    .begin_deliberation_cancel("run-core", "2026-10-01T00:00:04Z")
                    .unwrap()
                    .5
            );
            drop(storage);
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn cancellation_active_stop_and_unknown_settle_domain_dispatch_and_restart_intent() {
        for confirmed in [true, false] {
            let (root, storage, mut aggregate) = cancellation_fixture();
            let claim = cancellation_claim(&storage, &mut aggregate);
            let result = storage
                .begin_deliberation_cancel("run-core", "2026-10-01T00:00:02Z")
                .unwrap();
            assert_eq!(result.0, LiveRunStatus::Cancelling);
            assert!(matches!(
                storage.load_run_aggregate("run-core").unwrap().run().status,
                RunStatus::Cancelling
            ));
            assert!(
                storage
                    .load_frozen_deliberation_role(&claim, CoreId::Melchior1)
                    .is_err()
            );
            let revision = aggregate.run().revision;
            assert!(
                storage
                    .commit_live_run_aggregate_transition(&claim, &mut aggregate, revision)
                    .is_err()
            );
            if confirmed {
                storage
                    .finish_live_run_cancel(
                        "run-core",
                        result.3,
                        LiveRunProviderOutcome::Confirmed,
                        "2026-10-01T00:00:03Z",
                    )
                    .unwrap();
            } else {
                storage
                    .mark_live_run_cancel_unknown("run-core", result.3, "2026-10-01T00:00:03Z")
                    .unwrap();
            }
            let snapshot = storage.load_run_dossier("run-core").unwrap().snapshot;
            if confirmed {
                assert!(matches!(snapshot.run.status, RunStatus::Cancelled));
            } else {
                assert!(matches!(
                    snapshot.run.status,
                    RunStatus::Interrupted {
                        resume_stage: magi_domain::RunStage::IndependentReview,
                        cancellation_requested: true,
                        ..
                    }
                ));
                assert!(snapshot.external_effect_unknown);
            }
            assert!(snapshot.ballots_revealed.is_none());
            assert!(snapshot.tally.is_none());
            drop(storage);
            let reopened = Storage::open_or_create(&root).unwrap();
            assert!(!reopened.has_queued_live_runs().unwrap());
            assert_eq!(
                reopened
                    .load_run_dossier("run-core")
                    .unwrap()
                    .snapshot
                    .run
                    .status,
                snapshot.run.status
            );
            drop(reopened);
            fs::remove_dir_all(root).unwrap();
        }
        let (root, storage, mut aggregate) = cancellation_fixture();
        cancellation_claim(&storage, &mut aggregate);
        storage
            .begin_deliberation_cancel("run-core", "2026-10-01T00:00:02Z")
            .unwrap();
        drop(storage);
        let reopened = Storage::open_or_create(&root).unwrap();
        let snapshot = reopened.load_run_dossier("run-core").unwrap().snapshot;
        assert!(matches!(
            snapshot.run.status,
            RunStatus::Interrupted {
                cancellation_requested: true,
                ..
            }
        ));
        assert!(snapshot.external_effect_unknown);
        assert!(snapshot.ballots_revealed.is_none());
        assert!(snapshot.tally.is_none());
        assert!(!reopened.has_queued_live_runs().unwrap());
        drop(reopened);
        fs::remove_dir_all(root).unwrap();
    }
    fn cancellation_balloting(aggregate: &mut RunAggregate) {
        let at = "2026-10-01T00:00:01Z";
        for stage in [
            magi_domain::AssessmentStage::IndependentReview,
            magi_domain::AssessmentStage::CrossReview,
        ] {
            for core_id in CoreId::ALL {
                aggregate
                    .accept_assessment(
                        magi_domain::RoleAssessment {
                            schema_version: magi_domain::CONTRACT_SCHEMA_VERSION,
                            run_id: "run-core".to_owned(),
                            attempt_id: format!("assessment-{stage:?}-{}", core_id.wire_name()),
                            core_id,
                            stage,
                            input_digest: aggregate.input().input_digest.clone(),
                            attempt_generation: aggregate.run().generation,
                            position_summary: "Fixture finding".to_owned(),
                            claims: vec![magi_domain::Claim {
                                claim_id: format!("claim-{stage:?}-{}", core_id.wire_name()),
                                kind: magi_domain::ClaimKind::Inference,
                                text: "Fixture inference".to_owned(),
                                evidence_refs: vec![],
                                limitations: vec![],
                            }],
                            assumptions: vec![],
                            information_gaps: vec![],
                            counterarguments: vec![],
                            claim_responses: vec![],
                            position_changes: vec![],
                            created_at: at.to_owned(),
                        },
                        at.to_owned(),
                    )
                    .unwrap();
            }
        }
        let input = aggregate.input();
        let proposal = magi_domain::ProposalSnapshot {
            schema_version: magi_domain::CONTRACT_SCHEMA_VERSION,
            proposal_id: "proposal-fixture".to_owned(),
            run_id: "run-core".to_owned(),
            question_digest: input.question.digest.clone(),
            context_digest: input.context_manifest.digest.clone(),
            roles_digest: input.role_set.digest.clone(),
            kind: magi_domain::QuestionKind::Answer,
            body: "Fixture proposal".to_owned(),
            claims: vec![],
            conditions: vec![],
            alternatives: vec![],
            open_objections: vec![],
            digest: Digest::from_bytes(b"unsealed"),
            created_at: at.to_owned(),
        }
        .seal()
        .unwrap();
        aggregate
            .freeze_proposal(proposal.clone(), at.to_owned())
            .unwrap();
        for core_id in CoreId::ALL.into_iter().take(2) {
            aggregate
                .accept_ballot(
                    magi_domain::Ballot {
                        schema_version: magi_domain::CONTRACT_SCHEMA_VERSION,
                        run_id: "run-core".to_owned(),
                        attempt_id: format!("ballot-{}", core_id.wire_name()),
                        attempt_generation: aggregate.run().generation,
                        core_id,
                        input_digest: aggregate.input().input_digest.clone(),
                        proposal_id: proposal.proposal_id.clone(),
                        proposal_digest: proposal.digest.clone(),
                        vote: magi_domain::VoteValue::Support,
                        rationale: "Private sealed rationale".to_owned(),
                        objection_refs: vec![],
                        created_at: at.to_owned(),
                    },
                    at.to_owned(),
                )
                .unwrap();
        }
    }

    fn cancellation_last_ballot(aggregate: &RunAggregate) -> magi_domain::Ballot {
        let proposal = aggregate.persistence_state().proposal.unwrap();
        magi_domain::Ballot {
            schema_version: magi_domain::CONTRACT_SCHEMA_VERSION,
            run_id: aggregate.run().run_id.clone(),
            attempt_id: "last-ballot".into(),
            attempt_generation: aggregate.run().generation,
            core_id: CoreId::Casper3,
            input_digest: aggregate.input().input_digest.clone(),
            proposal_id: proposal.proposal_id,
            proposal_digest: proposal.digest,
            vote: magi_domain::VoteValue::Support,
            rationale: "Final private rationale".into(),
            objection_refs: vec![],
            created_at: "2026-10-01T00:00:02Z".into(),
        }
    }

    #[test]
    fn live_dispatch_projection_preserves_sealed_results_and_requires_completed_provenance() {
        let (root, storage, mut aggregate) = cancellation_fixture();
        let claim = cancellation_claim(&storage, &mut aggregate);
        let revision = aggregate.run().revision;
        cancellation_balloting(&mut aggregate);
        storage
            .commit_live_run_aggregate_transition(&claim, &mut aggregate, revision)
            .unwrap();
        let sealed = storage.load_run_dossier("run-core").unwrap().snapshot;
        assert_eq!(sealed.submitted_ballot_count, 2);
        assert!(sealed.ballots_revealed.is_none());
        let projected = storage.load_live_dispatch_projection("run-core").unwrap();
        assert!(
            projected.dispatches[7..]
                .iter()
                .all(|slot| slot.result_ref.is_none())
        );
        let revision = aggregate.run().revision;
        aggregate
            .accept_ballot(
                cancellation_last_ballot(&aggregate),
                "2026-10-01T00:00:02Z".into(),
            )
            .unwrap();
        storage
            .commit_live_run_aggregate_transition(&claim, &mut aggregate, revision)
            .unwrap();
        let completed = storage.load_run_dossier("run-core").unwrap().snapshot;
        let mut connection = storage.connection().unwrap();
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        transaction.execute("UPDATE live_run_dispatch_reservations SET state='active' WHERE run_id='run-core' AND state='reserved'", []).unwrap();
        transaction.execute("UPDATE live_run_dispatch_reservations SET state='settled' WHERE run_id='run-core' AND state='active'", []).unwrap();
        let projection = load_live_dispatches_from(&transaction, &completed).unwrap();
        assert!(projection.iter().all(|slot| slot.result_ref.is_some()));
        for kind in 0..3 {
            let mut missing = completed.clone();
            match kind {
                0 => missing.assessments.clear(),
                1 => missing.proposal = None,
                _ => missing.ballots_revealed = None,
            }
            assert!(load_live_dispatches_from(&transaction, &missing).is_err());
        }
        transaction.commit().unwrap();
        drop(connection);
        let database = root.join("state/magi.sqlite");
        let before = fs::read(&database).unwrap();
        let reader = StorageReader::open_read_only(&root).unwrap();
        assert_eq!(
            reader
                .load_live_dispatch_projection("run-core")
                .unwrap()
                .dispatches,
            projection
        );
        assert_eq!(fs::read(database).unwrap(), before);
        drop(reader);
        drop(storage);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn completed_deliberation_crash_window_recovers_exact_result_without_replaying_calls() {
        for completed in [true, false] {
            let (root, storage, mut aggregate) = cancellation_fixture();
            let claim = cancellation_claim(&storage, &mut aggregate);
            let revision = aggregate.run().revision;
            cancellation_balloting(&mut aggregate);
            storage
                .commit_live_run_aggregate_transition(&claim, &mut aggregate, revision)
                .unwrap();
            {
                let connection = storage.connection().unwrap();
                connection.execute("UPDATE live_run_dispatch_reservations SET state='active' WHERE run_id='run-core' AND state='reserved'",[]).unwrap();
                connection.execute("UPDATE live_run_dispatch_reservations SET state='settled' WHERE run_id='run-core' AND state='active' AND slot_ordinal<9",[]).unwrap();
            }
            let expected = LiveProviderResultInput {
                final_text: aggregate.persistence_state().proposal.unwrap().body,
                stop_reason: "deliberation_completed".into(),
                usage: None,
            };
            if completed {
                let revision = aggregate.run().revision;
                aggregate
                    .accept_ballot(
                        cancellation_last_ballot(&aggregate),
                        "2026-10-01T00:00:02Z".into(),
                    )
                    .unwrap();
                storage
                    .commit_aggregate_dispatch_transition(&claim, &mut aggregate, revision, 9)
                    .unwrap();
                assert!(
                    storage
                        .load_live_dispatch_projection("run-core")
                        .unwrap()
                        .dispatches
                        .iter()
                        .all(|slot| slot.state == crate::LiveDispatchState::Settled
                            && slot.result_ref.is_some())
                );
            }
            let canonical = storage.load_run_dossier("run-core").unwrap().snapshot;
            let before = storage.get_live_run_snapshot("run-core", 0).unwrap();
            assert_eq!(before.status, LiveRunStatus::Running);
            assert!(before.result.is_none());
            drop(storage);
            let reopened = Storage::open_or_create(&root).unwrap();
            let live = reopened.get_live_run_snapshot("run-core", 0).unwrap();
            let dossier = reopened.load_run_dossier("run-core").unwrap().snapshot;
            let projection = reopened.load_live_dispatch_projection("run-core").unwrap();
            assert!(!reopened.has_queued_live_runs().unwrap());
            if completed {
                assert_eq!(live.status, LiveRunStatus::Completed);
                assert_eq!(dossier.run, canonical.run);
                assert_eq!(dossier.ballots_revealed.as_ref().unwrap().len(), 3);
                assert!(
                    projection
                        .dispatches
                        .iter()
                        .all(|slot| slot.state == crate::LiveDispatchState::Settled
                            && slot.result_ref.is_some())
                );
                let result = live.result.as_ref().unwrap();
                let encoded = serde_json::to_vec(&expected).unwrap();
                assert_eq!(result.final_text, expected.final_text);
                assert_eq!(result.stop_reason, expected.stop_reason);
                assert!(result.usage.is_none());
                assert_eq!(result.content_digest, Digest::from_bytes(&encoded));
                assert_eq!(result.content_byte_length, encoded.len() as u64);
                assert_eq!(
                    reopened.read_source_object(&result.content_digest).unwrap(),
                    encoded
                );
                assert!(
                    reopened
                        .finish_live_run(&claim, &expected, "2026-10-01T00:00:03Z")
                        .is_err()
                );
            } else {
                assert_eq!(live.status, LiveRunStatus::Unknown);
                assert!(live.result.is_none());
                assert!(matches!(
                    dossier.run.status,
                    RunStatus::Interrupted {
                        resume_stage: magi_domain::RunStage::Balloting,
                        ..
                    }
                ));
                assert_eq!(dossier.submitted_ballot_count, 2);
                assert!(dossier.ballots_revealed.is_none());
                assert!(dossier.tally.is_none());
                assert!(
                    projection.dispatches[7..]
                        .iter()
                        .all(|slot| slot.result_ref.is_none())
                );
            }
            drop(reopened);
            let again = Storage::open_or_create(&root).unwrap();
            let repeated = again.get_live_run_snapshot("run-core", 0).unwrap();
            assert_eq!(repeated.status, live.status);
            assert_eq!(repeated.result, live.result);
            assert_eq!(repeated.revision, live.revision);
            assert_eq!(
                again.load_run_dossier("run-core").unwrap().snapshot.run,
                dossier.run
            );
            drop(again);
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn completed_recovery_rejects_missing_or_corrupt_decision_proof() {
        for missing in [true, false] {
            let (root, storage, mut aggregate) = cancellation_fixture();
            let claim = cancellation_claim(&storage, &mut aggregate);
            let revision = aggregate.run().revision;
            cancellation_balloting(&mut aggregate);
            aggregate
                .accept_ballot(
                    cancellation_last_ballot(&aggregate),
                    "2026-10-01T00:00:02Z".into(),
                )
                .unwrap();
            storage
                .commit_live_run_aggregate_transition(&claim, &mut aggregate, revision)
                .unwrap();
            let connection = storage.connection().unwrap();
            if missing {
                connection
                    .execute("DELETE FROM decision_dossiers WHERE run_id='run-core'", [])
                    .unwrap();
            } else {
                connection
                    .execute(
                        "UPDATE decision_dossiers SET payload_digest=?1 WHERE run_id='run-core'",
                        [Digest::from_bytes(b"invalid proof").as_str()],
                    )
                    .unwrap();
            }
            drop(connection);
            drop(storage);
            assert!(Storage::open_or_create(&root).is_err());
            let connection = Connection::open(root.join("state/magi.sqlite")).unwrap();
            let tables: i64 = connection
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE name='live_runs'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(tables, 1);
            let state: (String, Option<String>) = connection.query_row("SELECT q.state,r.result_digest FROM live_run_outbox q JOIN live_runs r USING(run_id) WHERE run_id='run-core'", [], |r| Ok((r.get(0)?,r.get(1)?))).unwrap();
            assert_eq!(state, ("running".into(), None));
            drop(connection);
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn completed_recovery_rejects_incomplete_dispatch_evidence_before_publication() {
        for defect in ["missing_slot", "unsettled_slot", "missing_result"] {
            let (root, storage, mut aggregate) = cancellation_fixture();
            let claim = cancellation_claim(&storage, &mut aggregate);
            let revision = aggregate.run().revision;
            cancellation_balloting(&mut aggregate);
            aggregate
                .accept_ballot(
                    cancellation_last_ballot(&aggregate),
                    "2026-10-01T00:00:02Z".into(),
                )
                .unwrap();
            storage
                .commit_live_run_aggregate_transition(&claim, &mut aggregate, revision)
                .unwrap();
            let connection = storage.connection().unwrap();
            connection.execute("UPDATE live_run_dispatch_reservations SET state='active' WHERE run_id='run-core' AND state='reserved'", []).unwrap();
            connection.execute("UPDATE live_run_dispatch_reservations SET state='settled' WHERE run_id='run-core' AND (slot_ordinal<9 OR ?1!='unsettled_slot')", [defect]).unwrap();
            match defect {
                "missing_slot" => {
                    connection.execute("DELETE FROM live_run_dispatch_reservations WHERE run_id='run-core' AND slot_ordinal=9", []).unwrap();
                }
                "unsettled_slot" => {}
                _ => {
                    connection.execute("DELETE FROM role_assessments WHERE run_id='run-core' AND stage='independent_review'", []).unwrap();
                }
            }
            drop(connection);
            assert!(
                storage
                    .begin_deliberation_cancel("run-core", "2026-10-01T00:00:03Z")
                    .is_err()
            );
            let before = storage.get_live_run_snapshot("run-core", 0).unwrap();
            assert_eq!(before.status, LiveRunStatus::Running);
            assert!(before.result.is_none());
            drop(storage);
            assert!(Storage::open_or_create(&root).is_err());
            let connection = Connection::open(root.join("state/magi.sqlite")).unwrap();
            let state: (String, Option<String>) = connection.query_row("SELECT q.state,r.result_digest FROM live_run_outbox q JOIN live_runs r USING(run_id) WHERE run_id='run-core'", [], |r| Ok((r.get(0)?,r.get(1)?))).unwrap();
            assert_eq!(state, ("running".into(), None));
            drop(connection);
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn authentication_failure_persists_frozen_recipient_and_versions_event_only_projection() {
        let (root, storage, mut aggregate) = cancellation_fixture();
        let claim = cancellation_claim(&storage, &mut aggregate);
        let frozen = aggregate.input().role_set.roles[1]
            .catalog_binding
            .as_ref()
            .unwrap();
        let mut failure = LiveRunFailure {
            profile_binding: Some(magi_domain::AuthFailureProfileBinding {
                provider_profile_id: frozen.provider_profile_id.clone(),
                profile_revision: frozen.profile_revision,
            }),
            code: "authentication_status_rpc_failed".into(),
            detail: "Verification unavailable".into(),
            external_effect_unknown: false,
        };
        let valid = failure.clone();
        failure.profile_binding.as_mut().unwrap().profile_revision += 1;
        assert!(
            storage
                .fail_live_run(&claim, &failure, false, "2026-10-01T00:00:02Z")
                .is_err()
        );
        failure = valid.clone();
        failure.code = "provider_timeout".into();
        assert!(
            storage
                .fail_live_run(&claim, &failure, false, "2026-10-01T00:00:02Z")
                .is_err()
        );
        let revision = aggregate.run().revision;
        aggregate
            .fail(valid.code.clone(), revision, "2026-10-01T00:00:02Z".into())
            .unwrap();
        storage
            .commit_live_run_aggregate_transition(&claim, &mut aggregate, revision)
            .unwrap();
        storage
            .fail_live_run(&claim, &valid, false, "2026-10-01T00:00:02Z")
            .unwrap();
        let wire = storage.get_live_run_snapshot("run-core", 0).unwrap();
        assert_eq!(wire.schema_version, 2);
        assert!(wire.validate_failure_wire_version());
        let mut wrong = wire.clone();
        wrong.schema_version = 1;
        assert!(!wrong.validate_failure_wire_version());
        let (dossier, persisted) = storage
            .load_run_dossier_with_live_failure("run-core")
            .unwrap();
        assert_eq!(dossier.snapshot.run.status, aggregate.run().status);
        assert_eq!(persisted, Some(valid.clone()));
        drop(storage);
        let reopened = Storage::open_or_create(&root).unwrap();
        assert_eq!(
            reopened
                .load_run_dossier_with_live_failure("run-core")
                .unwrap()
                .1,
            Some(valid)
        );
        reopened
            .connection()
            .unwrap()
            .execute(
                "UPDATE live_runs SET failure_json=?1 WHERE run_id='run-core'",
                [r#"{"code":"provider_timeout","detail":"timeout","externalEffectUnknown":false}"#],
            )
            .unwrap();
        let event_only = reopened.get_live_run_snapshot("run-core", 0).unwrap();
        assert!(
            event_only
                .failure
                .as_ref()
                .unwrap()
                .profile_binding
                .is_none()
        );
        assert_eq!(event_only.schema_version, 2);
        assert!(event_only.validate_failure_wire_version());
        drop(reopened);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn direct_live_recovery_preserves_unknown_external_effect() {
        let root = std::env::temp_dir().join(format!("magi-direct-recovery-{}", Uuid::new_v4()));
        let storage = Storage::open_or_create(&root).unwrap();
        let at = "2026-10-01T00:00:00Z";
        let provider = storage
            .save_provider_profile(&profile("direct-profile"), None, at)
            .unwrap();
        let observed = catalog("direct-catalog", &provider, "direct-model");
        storage.save_provider_catalog_snapshot(&observed).unwrap();
        let selected = storage
            .select_provider_model(&model_input(&observed, "direct-model", None))
            .unwrap();
        let request = LiveRunAdmissionRequest {
            command_id: "direct-recovery".into(),
            idempotency_key: "direct-recovery".into(),
            question: "Direct request".into(),
            model_binding: selected.binding,
        };
        let run_id = match storage.admit_live_run(&request).unwrap() {
            LiveRunAdmissionOutcome::Accepted { receipt, .. } => receipt.run_id,
            _ => panic!("direct admission"),
        };
        let claim = storage.claim_next_live_run("worker", at).unwrap().unwrap();
        storage
            .mark_live_run_session_creation_intent(&claim, at)
            .unwrap();
        storage.mark_live_run_running(&claim, at).unwrap();
        drop(storage);
        let reopened = Storage::open_or_create(&root).unwrap();
        let live = reopened.get_live_run_snapshot(&run_id, 0).unwrap();
        assert_eq!(live.status, LiveRunStatus::Unknown);
        assert!(live.result.is_none());
        assert!(!reopened.has_queued_live_runs().unwrap());
        assert!(reopened.load_run_aggregate(&run_id).is_err());
        drop(reopened);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cancellation_last_ballot_order_fences_late_completion_and_preserves_sealed_recovery() {
        for completion_first in [true, false] {
            let (root, storage, mut aggregate) = cancellation_fixture();
            let claim = cancellation_claim(&storage, &mut aggregate);
            let revision = aggregate.run().revision;
            cancellation_balloting(&mut aggregate);
            storage
                .commit_live_run_aggregate_transition(&claim, &mut aggregate, revision)
                .unwrap();
            let last = cancellation_last_ballot(&aggregate);
            if completion_first {
                let revision = aggregate.run().revision;
                aggregate
                    .accept_ballot(last, "2026-10-01T00:00:02Z".into())
                    .unwrap();
                storage
                    .commit_live_run_aggregate_transition(&claim, &mut aggregate, revision)
                    .unwrap();
                let connection = storage.connection().unwrap();
                connection.execute("UPDATE live_run_dispatch_reservations SET state='active' WHERE run_id='run-core' AND state='reserved'", []).unwrap();
                connection.execute("UPDATE live_run_dispatch_reservations SET state='settled' WHERE run_id='run-core'", []).unwrap();
                drop(connection);
                let before = storage.load_run_dossier("run-core").unwrap().snapshot;
                let result = storage
                    .begin_deliberation_cancel("run-core", "2026-10-01T00:00:03Z")
                    .unwrap();
                assert_eq!(result.0, LiveRunStatus::Completed);
                assert_eq!(
                    storage.get_live_run_snapshot("run-core", 0).unwrap().status,
                    LiveRunStatus::Completed
                );
                assert_eq!(
                    storage.load_run_dossier("run-core").unwrap().snapshot.run,
                    before.run
                );
                assert_eq!(before.ballots_revealed.as_ref().unwrap().len(), 3);
                let expected = LiveProviderResultInput {
                    final_text: before.proposal.as_ref().unwrap().body.clone(),
                    stop_reason: "deliberation_completed".into(),
                    usage: None,
                };
                let live_result = storage
                    .get_live_run_snapshot("run-core", 0)
                    .unwrap()
                    .result
                    .unwrap();
                let encoded = serde_json::to_vec(&expected).unwrap();
                assert_eq!(live_result.final_text, expected.final_text);
                assert_eq!(live_result.stop_reason, expected.stop_reason);
                assert!(live_result.usage.is_none());
                assert_eq!(live_result.content_digest, Digest::from_bytes(&encoded));
                assert_eq!(live_result.content_byte_length, encoded.len() as u64);
                assert_eq!(
                    storage
                        .read_source_object(&live_result.content_digest)
                        .unwrap(),
                    encoded
                );
                let before_replay = storage.get_live_run_snapshot("run-core", 0).unwrap();
                assert!(
                    storage
                        .begin_deliberation_cancel("run-core", "2026-10-01T00:00:04Z")
                        .unwrap()
                        .5
                );
                assert_eq!(
                    storage.get_live_run_snapshot("run-core", 0).unwrap(),
                    before_replay
                );
                assert!(
                    storage
                        .finish_live_run(&claim, &expected, "2026-10-01T00:00:04Z")
                        .is_err()
                );
            } else {
                let result = storage
                    .begin_deliberation_cancel("run-core", "2026-10-01T00:00:02Z")
                    .unwrap();
                let revision = aggregate.run().revision;
                aggregate
                    .accept_ballot(last, "2026-10-01T00:00:03Z".into())
                    .unwrap();
                assert!(
                    storage
                        .commit_live_run_aggregate_transition(&claim, &mut aggregate, revision)
                        .is_err()
                );
                let pending = storage.load_run_dossier("run-core").unwrap().snapshot;
                assert!(matches!(pending.run.status, RunStatus::Cancelling));
                assert!(pending.ballots_revealed.is_none());
                assert!(pending.tally.is_none());
                assert_eq!(pending.submitted_ballot_count, 2);
                assert!(
                    storage
                        .finish_live_run_cancel(
                            "run-core",
                            result.3 - 1,
                            LiveRunProviderOutcome::Confirmed,
                            "2026-10-01T00:00:03Z"
                        )
                        .is_err()
                );
            }
            drop(storage);
            let reopened = Storage::open_or_create(&root).unwrap();
            let snapshot = reopened.load_run_dossier("run-core").unwrap().snapshot;
            if completion_first {
                assert!(matches!(snapshot.run.status, RunStatus::Completed { .. }));
                assert_eq!(snapshot.ballots_revealed.unwrap().len(), 3);
                let expected = LiveProviderResultInput {
                    final_text: snapshot.proposal.unwrap().body,
                    stop_reason: "deliberation_completed".into(),
                    usage: None,
                };
                let result = reopened
                    .get_live_run_snapshot("run-core", 0)
                    .unwrap()
                    .result
                    .unwrap();
                assert_eq!(result.final_text, expected.final_text);
                assert_eq!(result.stop_reason, expected.stop_reason);
                assert!(result.usage.is_none());
                assert_eq!(
                    result.content_digest,
                    Digest::from_bytes(&serde_json::to_vec(&expected).unwrap())
                );
            } else {
                assert!(matches!(
                    snapshot.run.status,
                    RunStatus::Interrupted {
                        resume_stage: magi_domain::RunStage::Balloting,
                        cancellation_requested: true,
                        ..
                    }
                ));
                assert!(snapshot.ballots_revealed.is_none());
                assert!(snapshot.tally.is_none());
                assert_eq!(snapshot.submitted_ballot_count, 2);
            }
            assert!(!reopened.has_queued_live_runs().unwrap());
            drop(reopened);
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn cancellation_prestart_rejects_ambiguous_session_intent_and_direct_live_kind() {
        let (root, storage, _aggregate) = cancellation_fixture();
        let claim = storage
            .claim_next_live_run("worker", "2026-10-01T00:00:01Z")
            .unwrap()
            .unwrap();
        storage
            .mark_live_run_session_creation_intent(&claim, "2026-10-01T00:00:01Z")
            .unwrap();
        assert!(
            storage
                .begin_deliberation_cancel("run-core", "2026-10-01T00:00:02Z")
                .is_err()
        );
        assert!(matches!(
            storage.load_run_aggregate("run-core").unwrap().run().status,
            RunStatus::Preparing
        ));
        assert_eq!(
            storage
                .connection()
                .unwrap()
                .query_row(
                    "SELECT COUNT(*) FROM live_run_cancellation_receipts",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        storage
            .connection()
            .unwrap()
            .execute(
                "UPDATE runs SET deleted_at='2026-10-01T00:00:02Z' WHERE run_id='run-core'",
                [],
            )
            .unwrap();
        assert!(
            storage
                .begin_deliberation_cancel("run-core", "2026-10-01T00:00:03Z")
                .is_err()
        );
        drop(storage);
        fs::remove_dir_all(root).unwrap();
        let direct_root =
            std::env::temp_dir().join(format!("magi-direct-cancel-kind-{}", Uuid::new_v4()));
        let direct = Storage::open_or_create(&direct_root).unwrap();
        let at = "2026-10-01T00:00:00Z";
        let provider = direct
            .save_provider_profile(&profile("direct-profile"), None, at)
            .unwrap();
        let observed = catalog("direct-catalog", &provider, "direct-model");
        direct.save_provider_catalog_snapshot(&observed).unwrap();
        let selected = direct
            .select_provider_model(&model_input(&observed, "direct-model", None))
            .unwrap();
        let request = LiveRunAdmissionRequest {
            command_id: "direct-create".into(),
            idempotency_key: "direct-create".into(),
            question: "Direct request".into(),
            model_binding: selected.binding,
        };
        let receipt = match direct.admit_live_run(&request).unwrap() {
            LiveRunAdmissionOutcome::Accepted { receipt, .. } => receipt,
            _ => panic!("direct admission"),
        };
        assert!(
            direct
                .begin_deliberation_cancel(&receipt.run_id, "2026-10-01T00:00:01Z")
                .is_err()
        );
        assert_eq!(
            direct
                .get_live_run_snapshot(&receipt.run_id, 0)
                .unwrap()
                .status,
            LiveRunStatus::Queued
        );
        assert_eq!(
            direct
                .connection()
                .unwrap()
                .query_row(
                    "SELECT COUNT(*) FROM live_run_cancellation_receipts",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        direct
            .begin_live_run_cancel(
                "direct-cancel",
                "direct-cancel",
                &receipt.run_id,
                receipt.accepted_revision,
                "2026-10-01T00:00:01Z",
            )
            .unwrap();
        assert_eq!(
            direct
                .get_live_run_snapshot(&receipt.run_id, 0)
                .unwrap()
                .status,
            LiveRunStatus::Cancelled
        );
        drop(direct);
        fs::remove_dir_all(direct_root).unwrap();
    }
    #[test]
    fn lifecycle_backup_preserves_three_saved_selections_and_fences_restored_authority() {
        let (root, storage, mut aggregate) = cancellation_fixture_with_source(true);
        cancellation_claim(&storage, &mut aggregate);
        let original_input = aggregate.input().clone();
        let selections = CoreId::ALL.map(|core| storage.load_core_model_selection(core).unwrap());
        let connection = storage.connection().unwrap();
        connection
            .execute(
                "INSERT INTO disclosure_grants(run_id,payload_json) VALUES('run-core','{}')",
                [],
            )
            .unwrap();
        connection.execute("INSERT INTO provider_source_scopes(grant_id,provider_profile_id,canonical_path,root_device,root_inode,created_at) VALUES('source-scope','profile-0','/selected/source',1,1,'fixture')",[]).unwrap();
        drop(connection);
        let backup = root.join("backup");
        let destination = root.join("restored");
        let manifest = storage.create_backup(&backup).unwrap();
        assert_eq!(manifest.schema_version, SCHEMA_VERSION);
        let receipt = Storage::restore_backup(&backup, &destination).unwrap();
        assert!(receipt.requires_reauthorization);
        assert!(receipt.generation > storage.identity.generation);
        let restored = Storage::open_or_create(&destination).unwrap();
        assert_eq!(
            CoreId::ALL.map(|core| restored.load_core_model_selection(core).unwrap()),
            selections
        );
        let dossier = restored.load_run_dossier("run-core").unwrap();
        assert_eq!(dossier.snapshot.input, original_input);
        assert!(matches!(
            dossier.snapshot.run.status,
            RunStatus::Interrupted { .. }
        ));
        assert!(dossier.snapshot.ballots_revealed.is_none());
        assert!(!restored.has_queued_live_runs().unwrap());
        let connection = restored.connection().unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT revoked_at_epoch_ms FROM disclosure_grants WHERE run_id='run-core'",
                    [],
                    |row| row.get::<_, u64>(0)
                )
                .unwrap(),
            0
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT revoked_at FROM provider_source_scopes WHERE grant_id='source-scope'",
                    [],
                    |row| row.get::<_, String>(0)
                )
                .unwrap(),
            "restored"
        );
        drop(connection);
        assert!(restored.load_live_dispatch_projection("run-core").is_ok());
        drop(restored);
        drop(storage);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn lifecycle_evidence_deletion_is_revision_bound_and_blocks_unknown_live_effects() {
        for unknown in [false, true] {
            let (root, storage, mut aggregate) = cancellation_fixture_with_source(true);
            let dossier = storage.load_run_dossier("run-core").unwrap();
            let locator = dossier.capture_manifest.as_ref().unwrap().content.sources[0]
                .included_locators[0]
                .clone();
            assert_eq!(
                storage
                    .load_evidence("run-core", &locator)
                    .unwrap()
                    .text
                    .as_deref(),
                Some("PUBLIC_SELECTED")
            );
            let preview = storage
                .preview_evidence_deletion("run-core", "memo")
                .unwrap();
            assert_eq!(preview.affected_run_ids, vec!["run-core"]);
            assert!(
                storage
                    .delete_evidence(
                        "run-core",
                        "memo",
                        aggregate.run().revision,
                        &preview.affected_run_ids,
                        "fixture"
                    )
                    .is_err()
            );
            if unknown {
                cancellation_claim(&storage, &mut aggregate);
            }
            let cancel = storage
                .begin_deliberation_cancel("run-core", "2026-10-01T00:00:02Z")
                .unwrap();
            if unknown {
                storage
                    .mark_live_run_cancel_unknown("run-core", cancel.3, "2026-10-01T00:00:03Z")
                    .unwrap();
            }
            let revision = storage
                .load_run_dossier("run-core")
                .unwrap()
                .snapshot
                .run
                .revision;
            assert!(
                storage
                    .delete_evidence(
                        "run-core",
                        "memo",
                        revision + 1,
                        &preview.affected_run_ids,
                        "fixture"
                    )
                    .is_err()
            );
            assert!(
                storage
                    .delete_evidence("run-core", "memo", revision, &[], "fixture")
                    .is_err()
            );
            if unknown {
                assert!(
                    storage
                        .delete_evidence(
                            "run-core",
                            "memo",
                            revision,
                            &preview.affected_run_ids,
                            "fixture"
                        )
                        .is_err()
                );
                assert!(storage.read_source_object(&locator.object_digest).is_ok());
            } else {
                let bytes = storage.read_source_object(&locator.object_digest).unwrap();
                storage
                    .delete_evidence(
                        "run-core",
                        "memo",
                        revision,
                        &preview.affected_run_ids,
                        "fixture",
                    )
                    .unwrap();
                assert!(
                    storage
                        .load_evidence("run-core", &locator)
                        .unwrap()
                        .evidence_unavailable
                );
                fs::write(storage.object_path(&locator.object_digest), bytes).unwrap();
                assert!(matches!(
                    storage.read_source_object(&locator.object_digest),
                    Err(StorageError::MissingObject(_))
                ));
                drop(storage);
                let reopened = Storage::open_or_create(&root).unwrap();
                assert!(
                    reopened
                        .load_evidence("run-core", &locator)
                        .unwrap()
                        .evidence_unavailable
                );
                assert_eq!(
                    reopened
                        .load_run_dossier("run-core")
                        .unwrap()
                        .snapshot
                        .input,
                    dossier.snapshot.input
                );
                reopened.integrity_check().unwrap();
                let backup = root.join("deleted-backup");
                let copy = root.join("deleted-copy");
                let manifest = reopened.create_backup(&backup).unwrap();
                assert!(manifest.objects.is_empty());
                Storage::restore_backup(&backup, &copy).unwrap();
                let restored = Storage::open_or_create(&copy).unwrap();
                assert!(
                    restored
                        .load_evidence("run-core", &locator)
                        .unwrap()
                        .evidence_unavailable
                );
                restored.integrity_check().unwrap();
                drop(restored);
                drop(reopened);
                fs::remove_dir_all(root).unwrap();
                continue;
            }
            drop(storage);
            fs::remove_dir_all(root).unwrap();
        }
    }
    #[test]
    fn lifecycle_evidence_deletion_cannot_remove_an_identical_completed_provider_result() {
        let result = LiveProviderResultInput {
            final_text: "Public completed result".into(),
            stop_reason: "completed".into(),
            usage: None,
        };
        let bytes = serde_json::to_vec(&result).unwrap();
        let (root, storage, _aggregate) =
            cancellation_fixture_with_source_bytes(true, Some(&bytes));
        storage
            .begin_deliberation_cancel("run-core", "2026-10-01T00:00:01Z")
            .unwrap();
        let frozen = storage.load_run_dossier("run-core").unwrap();
        let locator = frozen.capture_manifest.as_ref().unwrap().content.sources[0]
            .included_locators[0]
            .clone();
        let preview = storage
            .preview_evidence_deletion("run-core", "memo")
            .unwrap();
        let request = LiveRunAdmissionRequest {
            command_id: "direct-shared-object".into(),
            idempotency_key: "direct-shared-object".into(),
            question: "Direct result".into(),
            model_binding: storage
                .load_provider_model_selection("profile-0")
                .unwrap()
                .unwrap()
                .binding,
        };
        let receipt = match storage.admit_live_run(&request).unwrap() {
            LiveRunAdmissionOutcome::Accepted { receipt, .. } => receipt,
            _ => panic!("direct admission required"),
        };
        let claim = storage
            .claim_next_live_run("worker", "2026-10-01T00:00:02Z")
            .unwrap()
            .unwrap();
        storage
            .mark_live_run_session_creation_intent(&claim, "2026-10-01T00:00:02Z")
            .unwrap();
        storage
            .mark_live_run_running(&claim, "2026-10-01T00:00:02Z")
            .unwrap();
        storage
            .finish_live_run(&claim, &result, "2026-10-01T00:00:03Z")
            .unwrap();
        let completed = storage.get_live_run_snapshot(&receipt.run_id, 0).unwrap();
        assert_eq!(completed.status, LiveRunStatus::Completed);
        assert_eq!(
            completed.result.as_ref().unwrap().content_digest,
            locator.object_digest
        );
        assert!(matches!(
            storage.delete_evidence(
                "run-core",
                "memo",
                frozen.snapshot.run.revision,
                &preview.affected_run_ids,
                "fixture"
            ),
            Err(StorageError::DeletionBlocked)
        ));
        assert_eq!(
            storage.get_live_run_snapshot(&receipt.run_id, 0).unwrap(),
            completed
        );
        assert_eq!(
            storage.read_source_object(&locator.object_digest).unwrap(),
            bytes
        );
        assert!(
            !storage
                .load_evidence("run-core", &locator)
                .unwrap()
                .evidence_unavailable
        );
        drop(storage);
        fs::remove_dir_all(root).unwrap();
    }
    struct ObservedStop;
    impl LiveRunPausePermission for ObservedStop {
        fn validate(
            &self,
            _: &crate::AdmissionExecutionAuthority,
            _: &LiveRunClaim,
        ) -> Result<(), StorageError> {
            Ok(())
        }
    }
    fn needs_input_fixture() -> (PathBuf, Storage, RunAggregate, LiveRunClaim) {
        needs_input_fixture_from(cancellation_fixture())
    }
    fn needs_input_fixture_from(
        fixture: (PathBuf, Storage, RunAggregate),
    ) -> (PathBuf, Storage, RunAggregate, LiveRunClaim) {
        let (root, storage, mut aggregate) = fixture;
        let at = "2026-10-02T00:00:00Z";
        let claim = storage
            .claim_next_live_run("needs-input-worker", at)
            .unwrap()
            .unwrap();
        storage
            .start_admitted_deliberation(&claim, &mut aggregate, 0, at)
            .unwrap();
        storage
            .mark_live_run_session_creation_intent(&claim, at)
            .unwrap();
        storage.mark_live_run_running(&claim, at).unwrap();
        storage
            .activate_live_run_dispatch_slot(&claim, 0, at)
            .unwrap();
        let revision = aggregate.run().revision;
        aggregate
            .accept_assessment(
                magi_domain::RoleAssessment {
                    schema_version: magi_domain::CONTRACT_SCHEMA_VERSION,
                    run_id: "run-core".into(),
                    attempt_id: "essential-assessment".into(),
                    core_id: CoreId::ALL[0],
                    stage: AssessmentStage::IndependentReview,
                    input_digest: aggregate.input().input_digest.clone(),
                    attempt_generation: aggregate.run().generation,
                    position_summary: "Essential policy information is missing".into(),
                    claims: vec![],
                    assumptions: vec![],
                    information_gaps: vec![magi_domain::InformationGap {
                        missing_information: "Approved disclosure policy".into(),
                        impact: "Cannot evaluate disclosure".into(),
                        essential: true,
                    }],
                    counterarguments: vec![],
                    claim_responses: vec![],
                    position_changes: vec![],
                    created_at: at.into(),
                },
                at.into(),
            )
            .unwrap();
        storage
            .commit_aggregate_dispatch_transition(&claim, &mut aggregate, revision, 0)
            .unwrap();
        (root, storage, aggregate, claim)
    }
    fn clarification_fixture(
        with_source: bool,
    ) -> (
        PathBuf,
        Storage,
        RunAggregate,
        crate::AdmissionExecutionAuthority,
        crate::ClarificationDraft,
    ) {
        let fixture = if with_source {
            cancellation_fixture_with_source_bytes(true, None)
        } else {
            cancellation_fixture()
        };
        let (root, storage, mut aggregate, claim) = needs_input_fixture_from(fixture);
        let expected = storage.admission_execution_authority().unwrap();
        let revision = aggregate.run().revision;
        storage
            .pause_live_deliberation_for_input_with_authority(
                &expected,
                &claim,
                &mut aggregate,
                revision,
                &ObservedStop,
                "2026-10-02T00:00:01Z",
            )
            .unwrap();
        let parent = crate::ClarificationParentReference {
            run_id: aggregate.run().run_id.clone(),
            revision: aggregate.run().revision,
            input_digest: aggregate.run().input_digest.clone(),
            generation: aggregate.run().generation,
        };
        let empty = SourceCaptureManifest::draft(vec![], 10).unwrap();
        let draft = storage
            .create_clarification_draft(&expected, &parent, "clarification-draft", &empty, 10)
            .unwrap();
        (root, storage, aggregate, expected, draft)
    }
    fn clarification_intent(
        aggregate: &RunAggregate,
        draft: &crate::ClarificationDraft,
    ) -> crate::ClarificationAdmissionIntent {
        let mut request = admission_authority_fixture_intent(aggregate);
        request.command_id = "clarification-start".into();
        request.idempotency_key = "clarification-start-key".into();
        request.question = draft.question.clone();
        request.request_provenance.context_draft_id = Some(draft.context.draft_id.clone());
        request.request_provenance.context_revision = Some(draft.context.revision);
        crate::ClarificationAdmissionIntent {
            parent: draft.parent.clone(),
            context_draft_id: draft.context.draft_id.clone(),
            context_draft_revision: draft.context.revision,
            request,
        }
    }

    #[test]
    fn clarification_descendant_question_cas_and_cancel_alias_survive_reopen() {
        let (root, storage, parent, expected, draft) = clarification_fixture(false);
        let unchanged = clarification_intent(&parent, &draft);
        assert!(
            storage
                .register_clarification_admission_request(&expected, &unchanged, "fixture")
                .is_err()
        );
        let exact = parent.persistence_state();
        let text = "Explicit approved disclosure: only selected public source representations";
        let changed = storage
            .save_clarification_question(&expected, &draft.context.draft_id, 0, text, 11)
            .unwrap();
        assert_eq!(changed.context.revision, 1);
        assert!(matches!(
            storage.save_clarification_question(
                &expected,
                &draft.context.draft_id,
                0,
                "stale overwrite",
                12
            ),
            Err(StorageError::DraftRevisionConflict { .. })
        ));
        let intent = clarification_intent(&parent, &changed);
        let binding = storage
            .register_clarification_admission_request(&expected, &intent, "fixture")
            .unwrap();
        assert_eq!(
            storage
                .register_clarification_admission_request(&expected, &intent, "fixture")
                .unwrap(),
            binding
        );
        assert!(
            storage
                .register_admission_request(&intent.request, "fixture")
                .is_err()
        );
        storage
            .save_clarification_question(
                &expected,
                &draft.context.draft_id,
                1,
                "Later explicit answer",
                12,
            )
            .unwrap();
        let receipt = storage
            .cancel_clarification_admission_request(
                &expected,
                &intent,
                "clarification-cancel",
                "clarification-cancel-key",
                "fixture",
            )
            .unwrap();
        assert_eq!(receipt.request, binding);
        assert!(receipt.admitted_run_id.is_none());
        let mut alias = intent.clone();
        alias.request.command_id = "clarification-alias".into();
        let alias_receipt = storage
            .cancel_clarification_admission_request(
                &expected,
                &alias,
                "clarification-alias-cancel",
                "clarification-alias-cancel-key",
                "fixture",
            )
            .unwrap();
        assert_eq!(alias_receipt.request.intent_digest, binding.intent_digest);
        assert_eq!(
            canonical_json(
                &storage
                    .load_run_aggregate(&parent.run().run_id)
                    .unwrap()
                    .persistence_state()
            )
            .unwrap(),
            canonical_json(&exact).unwrap()
        );
        drop(storage);
        let reopened = Storage::open_or_create(&root).unwrap();
        let current = reopened.admission_execution_authority().unwrap();
        assert!(
            reopened
                .load_clarification_draft(&expected, &draft.context.draft_id)
                .is_err()
        );
        assert!(
            reopened
                .load_clarification_draft(&current, &draft.context.draft_id)
                .is_ok()
        );
        assert_eq!(
            reopened
                .list_clarification_drafts(&current, &draft.parent)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            reopened
                .cancel_clarification_admission_request(
                    &current,
                    &intent,
                    "clarification-cancel",
                    "clarification-cancel-key",
                    "fixture"
                )
                .unwrap(),
            receipt
        );
        drop(reopened);
        fs::remove_dir_all(root).unwrap();
    }

    fn clarification_child(
        storage: &Storage,
        parent: &RunAggregate,
        draft: &crate::ClarificationDraft,
    ) -> (
        RunAggregate,
        CommandEnvelope,
        LiveRunAdmissionRequest,
        crate::ClarificationAdmissionIntent,
    ) {
        let intent = clarification_intent(parent, draft);
        let recipients = parent
            .input()
            .role_set
            .roles
            .iter()
            .map(|role| {
                let binding = role.catalog_binding.as_ref().unwrap();
                magi_context::Recipient {
                    provider_id: binding.provider_id.clone(),
                    account_profile_id: binding.provider_profile_id.clone(),
                }
            })
            .collect();
        let capture = draft
            .context
            .manifest
            .confirm_disclosure(recipients, &draft.context.manifest.digest, 20)
            .unwrap();
        storage.commit_context_manifest(&capture).unwrap();
        let binding = parent.input().role_set.roles[0]
            .catalog_binding
            .as_ref()
            .unwrap();
        let context = capture
            .run_manifest_for_recipient(&magi_context::Recipient {
                provider_id: binding.provider_id.clone(),
                account_profile_id: binding.provider_profile_id.clone(),
            })
            .unwrap();
        let input = InputSnapshot::new_with_request_provenance(
            magi_domain::QuestionSnapshot::new(
                "clarification-question".into(),
                magi_domain::QuestionKind::Answer,
                draft.question.clone(),
                vec![],
                vec![],
                vec![],
            )
            .unwrap(),
            context,
            parent.input().role_set.clone(),
            parent.input().policy_digest.clone(),
            intent.request.request_provenance.clone(),
        )
        .unwrap();
        let aggregate = RunAggregate::new(
            magi_domain::Run::new(
                "clarification-child".into(),
                parent.run().conversation_id.clone(),
                Some(parent.run().run_id.clone()),
                &input,
                "fixture".into(),
            )
            .unwrap(),
            input,
        )
        .unwrap();
        let command = CommandEnvelope {
            command_id: intent.request.command_id.clone(),
            idempotency_key: intent.request.idempotency_key.clone(),
            command_kind: CommandKind::CreateRun,
            target_id: aggregate.run().run_id.clone(),
            expected_revision: 0,
            payload_digest: aggregate.input().input_digest.clone(),
        };
        let request = LiveRunAdmissionRequest {
            command_id: command.command_id.clone(),
            idempotency_key: command.idempotency_key.clone(),
            question: draft.question.clone(),
            model_binding: binding.clone(),
        };
        (aggregate, command, request, intent)
    }

    #[test]
    fn clarification_descendant_generic_start_cannot_strip_parent_authority() {
        for owned_draft in [true, false] {
            let (root, storage, parent, expected, draft) = clarification_fixture(false);
            let draft = storage
                .save_clarification_question(
                    &expected,
                    &draft.context.draft_id,
                    0,
                    "Explicit changed policy answer",
                    12,
                )
                .unwrap();
            let (rich_child, _, _, rich_intent) = clarification_child(&storage, &parent, &draft);
            let mut intent = rich_intent.request;
            intent.command_id = "generic-downcast".into();
            intent.idempotency_key = "generic-downcast-key".into();
            if owned_draft {
                assert!(
                    storage
                        .register_admission_request(&intent, "fixture")
                        .is_err()
                );
            } else {
                storage
                    .save_context_draft("ordinary-draft", None, &draft.context.manifest, 12)
                    .unwrap();
                intent.request_provenance.context_draft_id = Some("ordinary-draft".into());
                intent.request_provenance.context_revision = Some(0);
                storage
                    .register_admission_request(&intent, "fixture")
                    .unwrap();
            }
            let input = InputSnapshot::new_with_request_provenance(
                rich_child.input().question.clone(),
                rich_child.input().context_manifest.clone(),
                rich_child.input().role_set.clone(),
                rich_child.input().policy_digest.clone(),
                intent.request_provenance.clone(),
            )
            .unwrap();
            let conversation = if owned_draft {
                "new-unrelated-conversation".into()
            } else {
                parent.run().conversation_id.clone()
            };
            let mut generic = RunAggregate::new(
                magi_domain::Run::new(
                    "generic-downcast-run".into(),
                    conversation,
                    None,
                    &input,
                    "fixture".into(),
                )
                .unwrap(),
                input,
            )
            .unwrap();
            let command = CommandEnvelope {
                command_id: intent.command_id.clone(),
                idempotency_key: intent.idempotency_key.clone(),
                command_kind: CommandKind::CreateRun,
                target_id: generic.run().run_id.clone(),
                expected_revision: 0,
                payload_digest: generic.input().input_digest.clone(),
            };
            let request = LiveRunAdmissionRequest {
                command_id: intent.command_id.clone(),
                idempotency_key: intent.idempotency_key.clone(),
                question: intent.question.clone(),
                model_binding: generic.input().role_set.roles[0]
                    .catalog_binding
                    .clone()
                    .unwrap(),
            };
            let mut called = false;
            assert!(
                storage
                    .admit_deliberation_run_checked(
                        &command,
                        &mut generic,
                        &request,
                        &intent.core_bindings,
                        Storage::admission_publication_with_authority(
                            "fixture",
                            None,
                            &expected,
                            || {
                                called = true;
                                Ok(())
                            }
                        )
                    )
                    .is_err()
            );
            assert!(!called && !storage.has_persisted_run("generic-downcast-run").unwrap());
            assert_eq!(
                storage
                    .load_run_aggregate(&parent.run().run_id)
                    .unwrap()
                    .persistence_state(),
                parent.persistence_state()
            );
            drop(storage);
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn clarification_descendant_parent_requires_common_paused_quiescence() {
        let (root, storage, parent, expected, draft) = clarification_fixture(false);
        let connection = storage.connection().unwrap();
        connection
            .execute(
                "UPDATE live_runs SET failure_json='{}' WHERE run_id=?1",
                [&parent.run().run_id],
            )
            .unwrap();
        drop(connection);
        assert!(
            storage
                .load_clarification_draft(&expected, &draft.context.draft_id)
                .is_err()
        );
        assert!(
            storage
                .create_clarification_draft(
                    &expected,
                    &draft.parent,
                    "invalid-child-draft",
                    &SourceCaptureManifest::draft(vec![], 20).unwrap(),
                    20
                )
                .is_err()
        );
        drop(storage);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn clarification_descendant_genuine_fifteen_upgrade_preserves_populated_authority() {
        let (root, storage, parent, _, _) = clarification_fixture(false);
        let c = storage.connection().unwrap();
        let old_input: String = c
            .query_row(
                "SELECT input_json FROM runs WHERE run_id=?1",
                [&parent.run().run_id],
                |r| r.get(0),
            )
            .unwrap();
        let old_receipt: String = c
            .query_row(
                "SELECT receipt_json FROM live_run_receipts WHERE run_id=?1",
                [&parent.run().run_id],
                |r| r.get(0),
            )
            .unwrap();
        let checksums: Vec<(u32, String)> = c
            .prepare(
                "SELECT version,checksum FROM schema_migrations WHERE version<=15 ORDER BY version",
            )
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        drop(c);
        drop(storage);
        let database = root.join("state/magi.sqlite");
        let source = root.join("state/fifteen-source.sqlite");
        fs::rename(&database, &source).unwrap();
        let old = Connection::open(&database).unwrap();
        old.execute_batch("CREATE TABLE schema_migrations(version INTEGER PRIMARY KEY CHECK(version>0),checksum TEXT NOT NULL,applied_at TEXT NOT NULL);").unwrap();
        for version in 1..=15 {
            old.execute_batch(migration_sql(version).unwrap()).unwrap();
        }
        old.execute("ATTACH DATABASE ?1 AS prior", [source.to_str().unwrap()])
            .unwrap();
        let tables: Vec<String> = old.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' AND name<>'schema_migrations' ORDER BY rowid").unwrap().query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
        for table in tables {
            if matches!(
                table.as_str(),
                "admission_binding_lineages" | "live_execution_lineages"
            ) {
                for (left, right) in [("main", "prior"), ("prior", "main")] {
                    let different: u64 = old.query_row(&format!("SELECT count(*) FROM (SELECT * FROM {left}.\"{table}\" EXCEPT SELECT * FROM {right}.\"{table}\")"), [], |r| r.get(0)).unwrap();
                    assert_eq!(different, 0);
                }
                continue;
            }
            if table == "store_meta" {
                old.execute_batch("DELETE FROM store_meta;").unwrap();
            }
            old.execute_batch(&format!(
                "INSERT INTO main.\"{table}\" SELECT * FROM prior.\"{table}\";"
            ))
            .unwrap();
        }
        old.execute_batch("INSERT INTO schema_migrations SELECT * FROM prior.schema_migrations WHERE version<=15; PRAGMA user_version=15; UPDATE store_meta SET value='15' WHERE key='schema_version'; DETACH DATABASE prior;").unwrap();
        assert_eq!(
            old.query_row(
                "SELECT count(*) FROM sqlite_master WHERE name='clarification_drafts'",
                [],
                |r| r.get::<_, u64>(0)
            )
            .unwrap(),
            0
        );
        drop(old);
        fs::remove_file(source).unwrap();
        let upgraded = Storage::open_or_create(&root).unwrap();
        let c = upgraded.connection().unwrap();
        let after: Vec<(u32, String)> = c
            .prepare(
                "SELECT version,checksum FROM schema_migrations WHERE version<=15 ORDER BY version",
            )
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(checksums, after);
        assert_eq!(
            c.query_row(
                "SELECT input_json FROM runs WHERE run_id=?1",
                [&parent.run().run_id],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            old_input
        );
        assert_eq!(
            c.query_row(
                "SELECT receipt_json FROM live_run_receipts WHERE run_id=?1",
                [&parent.run().run_id],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            old_receipt
        );
        assert!(
            c.prepare("PRAGMA foreign_key_check")
                .unwrap()
                .query([])
                .unwrap()
                .next()
                .unwrap()
                .is_none()
        );
        drop(c);
        drop(upgraded);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn clarification_descendant_concurrent_siblings_have_one_active_child() {
        let (root, storage, parent, expected, draft) = clarification_fixture(false);
        let draft = storage
            .save_clarification_question(
                &expected,
                &draft.context.draft_id,
                0,
                "Explicit changed policy answer",
                12,
            )
            .unwrap();
        let (first, command, request, intent) = clarification_child(&storage, &parent, &draft);
        let second = RunAggregate::new(
            magi_domain::Run::new(
                "clarification-sibling".into(),
                parent.run().conversation_id.clone(),
                Some(parent.run().run_id.clone()),
                first.input(),
                "fixture".into(),
            )
            .unwrap(),
            first.input().clone(),
        )
        .unwrap();
        let mut second_command = command.clone();
        second_command.command_id = "sibling-start".into();
        second_command.idempotency_key = "sibling-start-key".into();
        second_command.target_id = second.run().run_id.clone();
        let mut second_request = request.clone();
        second_request.command_id = second_command.command_id.clone();
        second_request.idempotency_key = second_command.idempotency_key.clone();
        let mut second_intent = intent.clone();
        second_intent.request.command_id = second_command.command_id.clone();
        second_intent.request.idempotency_key = second_command.idempotency_key.clone();
        let barrier = std::sync::Barrier::new(2);
        let outcomes = std::thread::scope(|scope| {
            let admit = |mut aggregate: RunAggregate,
                         command: CommandEnvelope,
                         request: LiveRunAdmissionRequest,
                         intent: crate::ClarificationAdmissionIntent| {
                barrier.wait();
                matches!(
                    storage.admit_deliberation_run_inner(
                        &command,
                        &mut aggregate,
                        &request,
                        &intent.request.core_bindings,
                        Some((&intent, 20)),
                        Storage::admission_publication_with_authority(
                            "fixture",
                            None,
                            &expected,
                            || Ok(()),
                        ),
                    ),
                    Ok(LiveRunAdmissionOutcome::Accepted {
                        duplicate: false,
                        ..
                    })
                )
            };
            let a = scope.spawn(move || admit(first, command, request, intent));
            let b =
                scope.spawn(move || admit(second, second_command, second_request, second_intent));
            [a.join().unwrap(), b.join().unwrap()]
        });
        assert_eq!(outcomes.into_iter().filter(|accepted| *accepted).count(), 1);
        let connection = storage.connection().unwrap();
        let children: u64 = connection
            .query_row(
                "SELECT count(*) FROM runs WHERE parent_run_id=?1",
                [&parent.run().run_id],
                |row| row.get(0),
            )
            .unwrap();
        let slots: u64 = connection.query_row("SELECT count(*) FROM live_run_dispatch_reservations WHERE run_id IN (SELECT run_id FROM runs WHERE parent_run_id=?1)", [&parent.run().run_id], |row| row.get(0)).unwrap();
        assert_eq!((children, slots), (1, 10));
        drop(connection);
        drop(storage);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn clarification_descendant_admission_and_cancellation_preserve_parent_and_ten_slot_fences() {
        for cancel_first in [true, false] {
            let (root, storage, parent, expected, draft) = clarification_fixture(false);
            let draft = storage
                .save_clarification_question(
                    &expected,
                    &draft.context.draft_id,
                    0,
                    "Explicit changed approved policy answer",
                    11,
                )
                .unwrap();
            let (mut child, command, request, intent) =
                clarification_child(&storage, &parent, &draft);
            let refs = intent.request.core_bindings.clone();
            let original = canonical_json(&parent.persistence_state()).unwrap();
            storage
                .register_clarification_admission_request(&expected, &intent, "fixture")
                .unwrap();
            if cancel_first {
                storage
                    .cancel_clarification_admission_request(
                        &expected,
                        &intent,
                        "child-cancel",
                        "child-cancel-key",
                        "fixture",
                    )
                    .unwrap();
                let mut called = false;
                assert!(
                    storage
                        .admit_clarification_deliberation_run_checked(
                            &command,
                            &mut child,
                            &request,
                            &refs,
                            &intent,
                            Storage::admission_publication_with_authority(
                                "fixture",
                                None,
                                &expected,
                                || {
                                    called = true;
                                    Ok(())
                                }
                            )
                        )
                        .is_err()
                );
                assert!(!called && !storage.has_persisted_run(&child.run().run_id).unwrap());
            } else {
                let pristine = child.clone();
                let receipt = match storage
                    .admit_clarification_deliberation_run_checked(
                        &command,
                        &mut child,
                        &request,
                        &refs,
                        &intent,
                        Storage::admission_publication_with_authority(
                            "fixture",
                            None,
                            &expected,
                            || Ok(()),
                        ),
                    )
                    .unwrap()
                {
                    LiveRunAdmissionOutcome::Accepted {
                        receipt,
                        duplicate: false,
                    } => receipt,
                    _ => panic!("new child admission"),
                };
                assert!(
                    storage
                        .validate_clarification_replay_intent(&intent)
                        .is_ok()
                );
                for index in 0..6 {
                    let mut altered = intent.clone();
                    match index {
                        0 => altered.parent.run_id = "wrong-parent".into(),
                        1 => altered.parent.revision += 1,
                        2 => altered.parent.generation += 1,
                        3 => altered.parent.input_digest = Digest::from_bytes(b"wrong"),
                        4 => altered.context_draft_id = "wrong-draft".into(),
                        _ => altered.context_draft_revision += 1,
                    }
                    assert!(
                        storage
                            .validate_clarification_replay_intent(&altered)
                            .is_err()
                    );
                    let mut retried = pristine.clone();
                    assert!(
                        storage
                            .admit_clarification_deliberation_run_checked(
                                &command,
                                &mut retried,
                                &request,
                                &refs,
                                &altered,
                                Storage::admission_publication_with_authority(
                                    "fixture",
                                    None,
                                    &expected,
                                    || panic!("historical replay must have no effects")
                                )
                            )
                            .is_err()
                    );
                }
                let mut generic = pristine.clone();
                assert!(
                    storage
                        .admit_deliberation_run_checked(
                            &command,
                            &mut generic,
                            &request,
                            &refs,
                            Storage::admission_publication_with_authority(
                                "fixture",
                                None,
                                &expected,
                                || panic!("generic replay must have no effects")
                            )
                        )
                        .is_err()
                );
                storage
                    .discard_clarification_draft(
                        &expected,
                        &draft.context.draft_id,
                        draft.context.revision,
                    )
                    .unwrap();
                let mut replay = pristine.clone();
                assert!(matches!(
                    storage
                        .admit_clarification_deliberation_run_checked(
                            &command,
                            &mut replay,
                            &request,
                            &refs,
                            &intent,
                            Storage::admission_publication_with_authority(
                                "fixture",
                                None,
                                &expected,
                                || panic!("historical replay must have no effects")
                            )
                        )
                        .unwrap(),
                    LiveRunAdmissionOutcome::Accepted {
                        duplicate: true,
                        ..
                    }
                ));
                assert_eq!(
                    storage
                        .load_live_dispatch_projection(&receipt.run_id)
                        .unwrap()
                        .dispatches
                        .len(),
                    10
                );
                let cancellation = storage
                    .cancel_clarification_admission_request(
                        &expected,
                        &intent,
                        "child-cancel",
                        "child-cancel-key",
                        "fixture",
                    )
                    .unwrap();
                assert_eq!(
                    cancellation.admitted_run_id.as_deref(),
                    Some(receipt.run_id.as_str())
                );
                storage
                    .begin_deliberation_cancel(&receipt.run_id, "fixture")
                    .unwrap();
                assert_eq!(
                    storage
                        .get_live_run_snapshot(&receipt.run_id, 0)
                        .unwrap()
                        .status,
                    LiveRunStatus::Cancelled
                );
            }
            assert_eq!(
                canonical_json(
                    &storage
                        .load_run_aggregate(&parent.run().run_id)
                        .unwrap()
                        .persistence_state()
                )
                .unwrap(),
                original
            );
            if !cancel_first {
                storage
                    .begin_deliberation_cancel(&parent.run().run_id, "fixture")
                    .unwrap();
                let mut replay =
                    RunAggregate::new(child.run().clone(), child.input().clone()).unwrap();
                assert!(matches!(
                    storage
                        .admit_clarification_deliberation_run_checked(
                            &command,
                            &mut replay,
                            &request,
                            &refs,
                            &intent,
                            Storage::admission_publication("fixture", None, || panic!(
                                "historical replay must have no effects"
                            ))
                        )
                        .unwrap(),
                    LiveRunAdmissionOutcome::Accepted {
                        duplicate: true,
                        ..
                    }
                ));
            }
            drop(storage);
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn clarification_descendant_discard_and_parent_purge_remove_private_metadata_but_retain_fences()
    {
        for (parent_purge, shared) in [(false, false), (false, true), (true, false), (true, true)] {
            let (root, storage, parent, expected, draft) = clarification_fixture(false);
            let unique = storage
                .put_source_object(b"private clarification selected source")
                .unwrap();
            let source = magi_context::ManifestSource {
                source_id: "child-source".into(),
                display_name: "child.txt".into(),
                state: ManifestSourceState::Captured,
                byte_length: Some(unique.byte_length),
                mime_type: Some("text/plain".into()),
                object_digest: Some(unique.digest.clone()),
                derived_digest: Some(unique.digest.clone()),
                representation_kind: Some(magi_context::RepresentationKind::Utf8Text),
                extractor_id: Some("utf8".into()),
                extractor_version: Some("1".into()),
                included_locators: vec![magi_context::EvidenceLocator {
                    source_id: "child-source".into(),
                    object_digest: unique.digest.clone(),
                    start_line: Some(1),
                    end_line: Some(1),
                    total_lines: Some(1),
                    page: None,
                    width: None,
                    height: None,
                }],
                omission: None,
                captured_at_epoch_ms: Some(1),
                secret_pattern_findings: vec![],
                secret_scan_incomplete: false,
            };
            let shared_manifest = if shared {
                Some(SourceCaptureManifest::draft(vec![source.clone()], 14).unwrap())
            } else {
                None
            };
            if let Some(shared) = &shared_manifest {
                storage
                    .save_context_draft("independent-shared-draft", None, shared, 14)
                    .unwrap();
            }
            let manifest = SourceCaptureManifest::draft(vec![source], 12).unwrap();
            storage
                .save_context_draft(&draft.context.draft_id, Some(0), &manifest, 12)
                .unwrap();
            let changed = storage
                .save_clarification_question(
                    &expected,
                    &draft.context.draft_id,
                    1,
                    "PRIVATE_CLARIFICATION_QUESTION_CANARY",
                    13,
                )
                .unwrap();
            let intent = clarification_intent(&parent, &changed);
            storage
                .register_clarification_admission_request(&expected, &intent, "fixture")
                .unwrap();
            let fence = storage
                .cancel_clarification_admission_request(
                    &expected,
                    &intent,
                    "child-cancel",
                    "child-cancel-key",
                    "fixture",
                )
                .unwrap();
            if parent_purge {
                storage
                    .begin_deliberation_cancel(&parent.run().run_id, "fixture")
                    .unwrap();
                let revision = storage
                    .load_run_aggregate(&parent.run().run_id)
                    .unwrap()
                    .run()
                    .revision;
                storage
                    .delete_run(&parent.run().run_id, revision, "fixture")
                    .unwrap();
            } else {
                storage
                    .discard_clarification_draft(&expected, &draft.context.draft_id, 2)
                    .unwrap();
            }
            let c = storage.connection().unwrap();
            assert_eq!(
                c.query_row("SELECT count(*) FROM clarification_drafts", [], |r| r
                    .get::<_, u64>(0))
                    .unwrap(),
                0
            );
            assert_eq!(
                c.query_row(
                    "SELECT count(*) FROM context_drafts WHERE draft_id='clarification-draft'",
                    [],
                    |r| r.get::<_, u64>(0)
                )
                .unwrap(),
                0
            );
            assert_eq!(
                c.query_row(
                    "SELECT count(*) FROM source_capture_manifests WHERE manifest_id=?1",
                    [&manifest.manifest_id],
                    |r| r.get::<_, u64>(0)
                )
                .unwrap(),
                0
            );
            assert_eq!(
                c.query_row(
                    "SELECT count(*) FROM clarification_admission_intents",
                    [],
                    |r| r.get::<_, u64>(0)
                )
                .unwrap(),
                1
            );
            drop(c);
            assert_eq!(storage.object_path(&unique.digest).exists(), shared);
            if let Some(shared) = &shared_manifest {
                assert_ne!(shared.manifest_id, manifest.manifest_id);
                assert!(
                    storage
                        .load_context_draft("independent-shared-draft")
                        .unwrap()
                        .is_some()
                );
                assert!(storage.read_source_object(&unique.digest).is_ok());
            }
            assert_eq!(
                storage
                    .cancel_clarification_admission_request(
                        &expected,
                        &intent,
                        "child-cancel",
                        "child-cancel-key",
                        "fixture"
                    )
                    .unwrap(),
                fence
            );
            drop(storage);
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn clarification_descendant_source_semantics_ignore_ids_times_and_grants() {
        let (root, storage, parent, expected, draft) = clarification_fixture(true);
        let original = storage
            .load_run_dossier(&parent.run().run_id)
            .unwrap()
            .capture_manifest
            .unwrap();
        let refreshed = SourceCaptureManifest::draft(original.content.sources.clone(), 99).unwrap();
        let copied = storage
            .save_context_draft(&draft.context.draft_id, Some(0), &refreshed, 99)
            .unwrap();
        let mut loaded = storage
            .load_clarification_draft(&expected, &draft.context.draft_id)
            .unwrap();
        assert_eq!(loaded.context.revision, copied.revision);
        assert!(
            storage
                .register_clarification_admission_request(
                    &expected,
                    &clarification_intent(&parent, &loaded),
                    "fixture"
                )
                .is_err()
        );
        let mut noise = original.content.sources.clone();
        noise[0].source_id = "different-source-id".into();
        noise[0].display_name = "different-safe-name".into();
        noise[0].captured_at_epoch_ms = Some(100);
        for locator in &mut noise[0].included_locators {
            locator.source_id = "different-source-id".into();
        }
        let noise = SourceCaptureManifest::draft(noise, 100).unwrap();
        storage
            .save_context_draft(&draft.context.draft_id, Some(1), &noise, 100)
            .unwrap();
        loaded = storage
            .load_clarification_draft(&expected, &draft.context.draft_id)
            .unwrap();
        assert!(
            storage
                .register_clarification_admission_request(
                    &expected,
                    &clarification_intent(&parent, &loaded),
                    "fixture"
                )
                .is_err()
        );
        let mut changed = original.content.sources.clone();
        changed[0].derived_digest = Some(
            storage
                .put_source_object(b"PRIVATE_BEFORE\n")
                .unwrap()
                .digest,
        );
        changed[0].included_locators[0].start_line = Some(1);
        changed[0].included_locators[0].end_line = Some(1);
        let changed = SourceCaptureManifest::draft(changed, 101).unwrap();
        storage
            .save_context_draft(&draft.context.draft_id, Some(2), &changed, 101)
            .unwrap();
        loaded = storage
            .load_clarification_draft(&expected, &draft.context.draft_id)
            .unwrap();
        let intent = clarification_intent(&parent, &loaded);
        assert!(
            storage
                .register_clarification_admission_request(&expected, &intent, "fixture")
                .is_ok()
        );
        storage
            .begin_deliberation_cancel(&parent.run().run_id, "fixture")
            .unwrap();
        assert!(
            storage
                .register_clarification_admission_request(&expected, &intent, "fixture")
                .is_err()
        );
        storage
            .discard_clarification_draft(&expected, &draft.context.draft_id, 3)
            .unwrap();
        drop(storage);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn clarification_descendant_restore_drafts_are_private_but_never_inherit_execution() {
        let (root, mut storage, parent, expected, draft) = clarification_fixture(false);
        let draft = storage
            .save_clarification_question(
                &expected,
                &draft.context.draft_id,
                0,
                "Explicit changed disclosure answer",
                12,
            )
            .unwrap();
        let intent = clarification_intent(&parent, &draft);
        storage
            .register_clarification_admission_request(&expected, &intent, "fixture")
            .unwrap();
        let backup = root.join("clarification-backup");
        let target = root.join("clarification-restored");
        storage.create_backup(&backup).unwrap();
        Storage::restore_backup(&backup, &target).unwrap();
        let mut restored = Storage::open_or_create(&target).unwrap();
        let inert = restored.admission_execution_authority().unwrap();
        assert!(!inert.active);
        assert!(
            restored
                .load_clarification_draft(&inert, &draft.context.draft_id)
                .is_err()
        );
        assert!(
            restored
                .register_clarification_admission_request(&inert, &intent, "fixture")
                .is_err()
        );
        storage
            .begin_deliberation_cancel(&parent.run().run_id, "fixture")
            .unwrap();
        let (active, _permission) = restored
            .activate_restored_execution_checked(&mut storage, |p| {
                Ok(FixtureActivationPermission(p.clone()))
            })
            .unwrap();
        assert!(active.active && active.lineage_id != expected.lineage_id);
        assert!(
            restored
                .load_clarification_draft(&active, &draft.context.draft_id)
                .is_err()
        );
        assert!(
            restored
                .list_clarification_drafts(&active, &draft.parent)
                .unwrap()
                .is_empty()
        );
        assert!(
            restored
                .register_clarification_admission_request(&active, &intent, "fixture")
                .is_err()
        );
        drop(restored);
        drop(storage);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn needs_input_pause_is_atomic_retains_review_and_reopens_without_dispatch() {
        let (root, storage, mut aggregate, claim) = needs_input_fixture();
        let authority = storage.admission_execution_authority().unwrap();
        let revision = aggregate.run().revision;
        storage
            .pause_live_deliberation_for_input_with_authority(
                &authority,
                &claim,
                &mut aggregate,
                revision,
                &ObservedStop,
                "2026-10-02T00:00:01Z",
            )
            .unwrap();
        assert!(matches!(
            aggregate.run().status,
            RunStatus::Paused {
                reason: magi_domain::PauseReason::NeedsInput,
                ..
            }
        ));
        assert!(aggregate.events().is_empty());
        let live = storage.get_live_run_snapshot("run-core", 0).unwrap();
        assert_eq!(live.status, LiveRunStatus::Paused);
        assert_eq!(live.schema_version, 3);
        assert!(live.validate_failure_wire_version());
        assert!(live.failure.is_none() && live.result.is_none());
        assert!(!storage.has_queued_live_runs().unwrap());
        assert!(
            storage
                .append_live_run_text_delta(&claim, "late", "fixture")
                .is_err()
        );
        let dossier = storage.load_run_dossier("run-core").unwrap();
        assert_eq!(dossier.snapshot.assessments.len(), 1);
        drop(storage);
        let reopened = Storage::open_or_create(&root).unwrap();
        assert_eq!(
            reopened
                .get_live_run_snapshot("run-core", 0)
                .unwrap()
                .status,
            LiveRunStatus::Paused
        );
        assert!(
            reopened
                .claim_next_live_run("late-worker", "fixture")
                .unwrap()
                .is_none()
        );
        let c = reopened.connection().unwrap();
        let counts: (u64,u64) = c.query_row("SELECT sum(state='settled'),sum(state='released') FROM live_run_dispatch_reservations WHERE run_id='run-core'", [], |r| Ok((r.get(0)?,r.get(1)?))).unwrap();
        assert_eq!(counts, (1, 9));
        assert!(c.execute("UPDATE live_run_outbox SET state='claimed',claim_owner='late' WHERE run_id='run-core'", []).is_err());
        drop(c);
        drop(reopened);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn needs_input_pause_rejects_unsettled_authority_and_rolls_back_candidate() {
        for fault in ["permission", "unknown", "missing_review", "stale", "commit"] {
            let (root, storage, mut aggregate, claim) = needs_input_fixture();
            let mut authority = storage.admission_execution_authority().unwrap();
            if fault == "stale" {
                authority.store_generation += 1;
            }
            let before = canonical_json(&aggregate.persistence_state()).unwrap();
            let c = storage.connection().unwrap();
            match fault {
                "unknown" => {
                    c.execute("UPDATE live_run_dispatch_reservations SET state='unknown' WHERE run_id='run-core' AND slot_ordinal=1", []).unwrap();
                }
                "missing_review" => {
                    c.execute("DELETE FROM role_assessments WHERE run_id='run-core'", [])
                        .unwrap();
                }
                "commit" => {
                    c.execute_batch("CREATE TEMP TRIGGER deny_pause BEFORE UPDATE OF state ON live_run_outbox WHEN NEW.state='paused' BEGIN SELECT RAISE(ABORT,'injected pause failure'); END;").unwrap();
                }
                _ => {}
            }
            drop(c);
            struct Denied;
            impl LiveRunPausePermission for Denied {
                fn validate(
                    &self,
                    _: &crate::AdmissionExecutionAuthority,
                    _: &LiveRunClaim,
                ) -> Result<(), StorageError> {
                    Err(StorageError::DispatchFenced)
                }
            }
            let revision = aggregate.run().revision;
            let outcome = if fault == "permission" {
                storage.pause_live_deliberation_for_input_with_authority(
                    &authority,
                    &claim,
                    &mut aggregate,
                    revision,
                    &Denied,
                    "fixture",
                )
            } else {
                storage.pause_live_deliberation_for_input_with_authority(
                    &authority,
                    &claim,
                    &mut aggregate,
                    revision,
                    &ObservedStop,
                    "fixture",
                )
            };
            assert!(outcome.is_err(), "{fault}");
            assert_eq!(
                canonical_json(&aggregate.persistence_state()).unwrap(),
                before
            );
            let c = storage.connection().unwrap();
            assert_eq!(
                c.query_row(
                    "SELECT state FROM live_run_outbox WHERE run_id='run-core'",
                    [],
                    |r| r.get::<_, String>(0)
                )
                .unwrap(),
                "running"
            );
            c.execute_batch("DROP TRIGGER IF EXISTS temp.deny_pause;")
                .unwrap();
            drop(c);
            drop(storage);
            fs::remove_dir_all(root).unwrap();
        }
    }
    #[test]
    fn needs_input_pause_can_be_cancelled_without_resuming_parent() {
        let (root, storage, mut aggregate, claim) = needs_input_fixture();
        let authority = storage.admission_execution_authority().unwrap();
        let revision = aggregate.run().revision;
        storage
            .pause_live_deliberation_for_input_with_authority(
                &authority,
                &claim,
                &mut aggregate,
                revision,
                &ObservedStop,
                "fixture",
            )
            .unwrap();
        storage
            .begin_deliberation_cancel("run-core", "fixture")
            .unwrap();
        assert_eq!(
            storage.get_live_run_snapshot("run-core", 0).unwrap().status,
            LiveRunStatus::Cancelled
        );
        assert!(matches!(
            storage.load_run_aggregate("run-core").unwrap().run().status,
            RunStatus::Cancelled
        ));
        drop(storage);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn needs_input_pause_migration_preserves_fourteen_checksums_receipts_and_sequences() {
        let (root, storage, aggregate) = cancellation_fixture_build(false, None, true);
        let before = canonical_json(&aggregate.input()).unwrap();
        let identity = storage.identity().clone();
        let c = storage.connection().unwrap();
        let ledger: Vec<(u32, String)> = c
            .prepare(
                "SELECT version,checksum FROM schema_migrations WHERE version<=14 ORDER BY version",
            )
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        drop(c);
        drop(storage);
        let database = root.join("state/magi.sqlite");
        let source = root.join("state/fixture-source.sqlite");
        fs::rename(&database, &source).unwrap();
        let twelve = Connection::open(&database).unwrap();
        twelve.execute_batch("CREATE TABLE schema_migrations(version INTEGER PRIMARY KEY CHECK(version>0),checksum TEXT NOT NULL,applied_at TEXT NOT NULL);").unwrap();
        for version in 1..=14 {
            twelve
                .execute_batch(migration_sql(version).unwrap())
                .unwrap();
        }
        twelve
            .execute("ATTACH DATABASE ?1 AS prior", [source.to_str().unwrap()])
            .unwrap();
        let tables:Vec<String>=twelve.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' AND name<>'schema_migrations' ORDER BY rowid").unwrap().query_map([],|row|row.get(0)).unwrap().map(Result::unwrap).collect();
        for table in tables {
            if matches!(
                table.as_str(),
                "admission_binding_lineages" | "live_execution_lineages"
            ) {
                for (left, right) in [("main", "prior"), ("prior", "main")] {
                    let different: u64 = twelve.query_row(&format!("SELECT count(*) FROM (SELECT * FROM {left}.\"{table}\" EXCEPT SELECT * FROM {right}.\"{table}\")"), [], |row| row.get(0)).unwrap();
                    assert_eq!(different, 0, "derived authority must equal original rows");
                }
                continue;
            }
            if table == "store_meta" {
                twelve.execute_batch("DELETE FROM store_meta;").unwrap();
            }
            twelve
                .execute_batch(&format!(
                    "INSERT INTO main.\"{table}\" SELECT * FROM prior.\"{table}\";"
                ))
                .unwrap();
        }
        twelve.execute_batch("INSERT INTO schema_migrations SELECT * FROM prior.schema_migrations WHERE version<=14; PRAGMA user_version=14; UPDATE store_meta SET value='14' WHERE key='schema_version'; DETACH DATABASE prior;").unwrap();
        let receipt: String = twelve
            .query_row(
                "SELECT receipt_json FROM live_run_receipts WHERE run_id='run-core'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        twelve.execute("UPDATE sqlite_sequence SET seq=100000 WHERE name IN ('live_run_events','live_run_outbox')",[]).unwrap();
        read_only_store_identity(&twelve).unwrap();
        drop(twelve);
        fs::remove_file(source).unwrap();
        let reader = StorageReader::open_read_only(&root).unwrap();
        assert_eq!(
            reader.get_live_run_snapshot("run-core", 0).unwrap().run_id,
            "run-core"
        );
        assert!(
            reader
                .load_admission_request_cancellation(&admission_authority_fixture_intent(
                    &aggregate
                ))
                .unwrap()
                .is_none()
        );
        drop(reader);
        let upgraded = Storage::open_or_create(&root).unwrap();
        assert_eq!(upgraded.identity().store_id, identity.store_id);
        assert_eq!(upgraded.identity().generation, identity.generation + 1);
        assert_eq!(upgraded.identity().schema_version, SCHEMA_VERSION);
        assert_eq!(
            canonical_json(upgraded.load_run_aggregate("run-core").unwrap().input()).unwrap(),
            before
        );
        assert_eq!(
            upgraded
                .load_live_dispatch_projection("run-core")
                .unwrap()
                .dispatches
                .len(),
            10
        );
        let c = upgraded.connection().unwrap();
        assert_eq!(
            c.query_row(
                "SELECT receipt_json FROM live_run_receipts WHERE run_id='run-core'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            receipt
        );
        assert_eq!(c.query_row("SELECT min(seq) FROM sqlite_sequence WHERE name IN ('live_run_events','live_run_outbox')",[],|r|r.get::<_,u64>(0)).unwrap(),100000);
        assert_eq!(
            c.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| r
                .get::<_, u64>(
                0
            ))
            .unwrap(),
            0
        );
        let after: Vec<(u32, String)> = c
            .prepare(
                "SELECT version,checksum FROM schema_migrations WHERE version<=14 ORDER BY version",
            )
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(after, ledger);
        c.execute(
            "UPDATE schema_migrations SET checksum=?1 WHERE version=14",
            [Digest::from_bytes(b"wrong").as_str()],
        )
        .unwrap();
        drop(c);
        drop(upgraded);
        assert!(matches!(
            StorageReader::open_read_only(&root),
            Err(StorageError::MigrationChecksum { version: 14 })
        ));
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn needs_input_pause_cancellation_wins_without_mutating_stale_candidate() {
        let (root, storage, mut aggregate, claim) = needs_input_fixture();
        let authority = storage.admission_execution_authority().unwrap();
        let before = canonical_json(&aggregate.persistence_state()).unwrap();
        let revision = aggregate.run().revision;
        storage
            .begin_deliberation_cancel("run-core", "fixture")
            .unwrap();
        assert!(
            storage
                .pause_live_deliberation_for_input_with_authority(
                    &authority,
                    &claim,
                    &mut aggregate,
                    revision,
                    &ObservedStop,
                    "fixture"
                )
                .is_err()
        );
        assert_eq!(
            canonical_json(&aggregate.persistence_state()).unwrap(),
            before
        );
        assert_ne!(
            storage.get_live_run_snapshot("run-core", 0).unwrap().status,
            LiveRunStatus::Paused
        );
        drop(storage);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn shutdown_enumeration_is_authority_bound_readonly_and_exhaustive() {
        let (root, storage, aggregate) = cancellation_fixture_build(false, None, false);
        let (_, request, _) = guarded_fixture_admission_parts(&aggregate);
        let seed = match storage.admit_live_run(&request).unwrap() {
            LiveRunAdmissionOutcome::Accepted { receipt, .. } => receipt.run_id,
            other => panic!("unexpected admission: {other:?}"),
        };
        let states = [
            "queued",
            "claimed",
            "session_creation_intent",
            "running",
            "cancelling",
            "unknown",
            "paused",
            "completed",
            "cancelled",
            "failed",
        ];
        {
            let mut connection = storage.connection().unwrap();
            let transaction = connection.transaction().unwrap();
            for (index, state) in states.iter().enumerate() {
                let id = format!("shutdown-{index}");
                transaction.execute("INSERT INTO live_runs SELECT ?1,schema_version,question_text,revision,model_binding_json,catalog_snapshot_id,model_binding_digest,result_digest,result_byte_length,failure_json,created_at,updated_at FROM live_runs WHERE run_id=?2", params![id,seed]).unwrap();
                let owner = matches!(*state, "claimed" | "session_creation_intent" | "running")
                    .then_some("worker");
                transaction.execute("INSERT INTO live_run_outbox(run_id,state,claim_generation,claim_owner,created_at,updated_at) VALUES(?1,?2,0,?3,'2026-10-01T00:00:00Z','2026-10-01T00:00:00Z')",params![id,state,owner]).unwrap();
            }
            transaction.commit().unwrap();
        }
        let authority = storage.admission_execution_authority().unwrap();
        let before = std::fs::read(root.join("state/magi.sqlite")).unwrap();
        let ids = storage
            .list_live_run_ids_requiring_shutdown_with_authority(&authority)
            .unwrap();
        let mut expected = vec![seed];
        expected.extend((0..6).map(|index| format!("shutdown-{index}")));
        assert_eq!(ids, expected);
        assert_eq!(
            before,
            std::fs::read(root.join("state/magi.sqlite")).unwrap()
        );
        let mut wrong = authority.clone();
        wrong.lineage_id = "0".repeat(32);
        assert!(matches!(
            storage.list_live_run_ids_requiring_shutdown_with_authority(&wrong),
            Err(StorageError::DispatchFenced)
        ));
        let mut inactive = authority.clone();
        inactive.active = false;
        assert!(matches!(
            storage.list_live_run_ids_requiring_shutdown_with_authority(&inactive),
            Err(StorageError::DispatchFenced)
        ));
        drop(storage);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn shutdown_enumeration_reopen_requires_fresh_execution_authority() {
        let (root, storage, aggregate) = cancellation_fixture_build(false, None, false);
        let (_, request, _) = guarded_fixture_admission_parts(&aggregate);
        let seed = match storage.admit_live_run(&request).unwrap() {
            LiveRunAdmissionOutcome::Accepted { receipt, .. } => receipt.run_id,
            other => panic!("unexpected admission: {other:?}"),
        };
        let authority = storage.admission_execution_authority().unwrap();
        drop(storage);
        let storage = Storage::open_or_create(&root).unwrap();
        assert!(matches!(
            storage.list_live_run_ids_requiring_shutdown_with_authority(&authority),
            Err(StorageError::DispatchFenced)
        ));
        assert_eq!(
            storage
                .list_live_run_ids_requiring_shutdown_with_authority(
                    &storage.admission_execution_authority().unwrap()
                )
                .unwrap(),
            vec![seed]
        );
        drop(storage);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn shutdown_enumeration_rejects_overflow_without_truncation_or_writes() {
        let (root, storage, aggregate) = cancellation_fixture_build(false, None, false);
        let (_, request, _) = guarded_fixture_admission_parts(&aggregate);
        let seed = match storage.admit_live_run(&request).unwrap() {
            LiveRunAdmissionOutcome::Accepted { receipt, .. } => receipt.run_id,
            other => panic!("unexpected admission: {other:?}"),
        };
        {
            let mut connection = storage.connection().unwrap();
            let transaction = connection.transaction().unwrap();
            for index in 0..2 {
                let id = format!("shutdown-overflow-{index}");
                transaction.execute("INSERT INTO live_runs SELECT ?1,schema_version,question_text,revision,model_binding_json,catalog_snapshot_id,model_binding_digest,result_digest,result_byte_length,failure_json,created_at,updated_at FROM live_runs WHERE run_id=?2",params![id,seed]).unwrap();
                transaction.execute("INSERT INTO live_run_outbox(run_id,state,claim_generation,claim_owner,created_at,updated_at) VALUES(?1,'queued',0,NULL,'2026-10-01T00:00:00Z','2026-10-01T00:00:00Z')",[id]).unwrap();
            }
            transaction.commit().unwrap();
        }
        let before = std::fs::read(root.join("state/magi.sqlite")).unwrap();
        assert!(
            matches!(storage.list_live_run_ids_requiring_shutdown_inner(&storage.admission_execution_authority().unwrap(), 2),Err(StorageError::Corrupt(message)) if message == "live run shutdown enumeration capacity exceeded")
        );
        assert_eq!(
            before,
            std::fs::read(root.join("state/magi.sqlite")).unwrap()
        );
        drop(storage);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn shutdown_enumeration_preserves_genuine_settled_needs_input_parent() {
        let (root, storage, mut aggregate, claim) = needs_input_fixture();
        let authority = storage.admission_execution_authority().unwrap();
        let revision = aggregate.run().revision;
        storage
            .pause_live_deliberation_for_input_with_authority(
                &authority,
                &claim,
                &mut aggregate,
                revision,
                &ObservedStop,
                "2026-10-02T00:00:01Z",
            )
            .unwrap();
        assert!(matches!(
            aggregate.run().status,
            RunStatus::Paused {
                reason: magi_domain::PauseReason::NeedsInput,
                ..
            }
        ));
        let parent = storage.load_run_dossier("run-core").unwrap();
        assert_eq!(parent.snapshot.assessments.len(), 1);
        assert_eq!(
            storage.get_live_run_snapshot("run-core", 0).unwrap().status,
            LiveRunStatus::Paused
        );
        {
            let connection = storage.connection().unwrap();
            let slots:(u64,u64,u64)=connection.query_row("SELECT count(*),coalesce(sum(state='settled'),0),coalesce(sum(state='released'),0) FROM live_run_dispatch_reservations WHERE run_id='run-core'",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
            assert_eq!(slots, (10, 1, 9));
            let unresolved:u64=connection.query_row("SELECT count(*) FROM dispatch_outbox WHERE run_id='run-core' AND state IN ('prepared','dispatched','unknown')",[],|r|r.get(0)).unwrap();
            assert_eq!(unresolved, 0);
            let claimed: bool = connection
                .query_row(
                    "SELECT claim_owner IS NOT NULL FROM live_run_outbox WHERE run_id='run-core'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert!(!claimed);
        }
        let before = std::fs::read(root.join("state/magi.sqlite")).unwrap();
        let wal_before = std::fs::read(root.join("state/magi.sqlite-wal")).ok();
        assert!(
            storage
                .list_live_run_ids_requiring_shutdown_with_authority(&authority)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            before,
            std::fs::read(root.join("state/magi.sqlite")).unwrap()
        );
        assert_eq!(
            wal_before,
            std::fs::read(root.join("state/magi.sqlite-wal")).ok()
        );
        let after = storage.load_run_dossier("run-core").unwrap();
        assert_eq!(
            canonical_json(&parent.snapshot).unwrap(),
            canonical_json(&after.snapshot).unwrap()
        );
        assert_eq!(
            storage.get_live_run_snapshot("run-core", 0).unwrap().status,
            LiveRunStatus::Paused
        );
        drop(storage);
        std::fs::remove_dir_all(root).unwrap();
    }
}
