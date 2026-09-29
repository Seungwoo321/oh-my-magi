use std::{
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
    AssessmentStage, Ballot, CommandEnvelope, CommandKind, CommandReceipt, CoreId,
    CoreRoleDefinition, Digest, DomainEvent, EventPayload, EventPosition, InputSnapshot,
    ProposalSnapshot, ProviderProfileInput, ProviderProfileRevision, RoleAssessment,
    RolePresetRevision, RunAggregate, RunPersistenceState, RunSnapshot, RunStatus, Tally,
    canonical_json, status_name,
};
use rusqlite::types::Value;
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use uuid::Uuid;

use crate::{
    ActiveProviderProfileSelection, ActiveRolePresetSelection, CommitResult, ConsoleSnapshot,
    ContentObjectRef, ContextDraft, ContextDraftSummary, ConversationSummary, DispatchBatchReceipt,
    DispatchEvent, DispatchEventCursor, DispatchEventPage, DispatchIdentity, DispatchRecord,
    DispatchState, DispatchTransitionDisposition, DispatchTransitionResult, PreparedDispatch,
    ProviderProfileSummary, RolePresetInput, RolePresetSource, RolePresetSummary, RunDossier,
    RunEventCursor, RunEventPage, RunHistoryCursor, RunHistoryPage, RunHistoryRequest, RunSummary,
    SourceFreshnessRecord, StorageError, StoreIdentity, StoredEvent,
};

const SCHEMA_VERSION: u32 = 4;
pub const MAX_OBJECT_BYTES: u64 = 100 * 1024 * 1024;
const MAX_EVENT_PAGE: usize = 10_000;
const MIGRATION_1: &str = include_str!("../migrations/0001_initial.sql");
const MIGRATION_2: &str = include_str!("../migrations/0002_role_history_freshness.sql");
const MIGRATION_3: &str = include_str!("../migrations/0003_dispatch_batches_and_events.sql");
const MIGRATION_4: &str =
    include_str!("../migrations/0004_provider_profiles_and_factory_presets.sql");

pub struct Storage {
    connection: Mutex<Connection>,
    _writer_lock: File,
    objects_root: PathBuf,
    temp_root: PathBuf,
    identity: StoreIdentity,
    startup_recovery: crate::RecoveryReport,
}

impl Storage {
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
        set_private_file_permissions(&database_path)?;
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "FULL")?;
        connection.pragma_update(None, "foreign_keys", true)?;
        initialize_schema(&mut connection)?;
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
        let current: Option<(i64, String, String)> = transaction
            .query_row(
                "SELECT h.revision, h.provider_id, r.runtime_home_id FROM provider_profile_heads h JOIN provider_profile_revisions r ON r.provider_profile_id = h.provider_profile_id AND r.revision = h.revision WHERE h.provider_profile_id = ?1",
                [&input.provider_profile_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        let current_revision = match current.as_ref().map(|(revision, _, _)| *revision) {
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
        if let Some((_, provider_id, runtime_home_id)) = &current {
            if provider_id != &input.provider_id {
                return Err(StorageError::ProviderProfileProviderMismatch);
            }
            if runtime_home_id != &input.runtime_home_id {
                return Err(StorageError::ProviderHomeBindingImmutable);
            }
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
            &state.run.run_id,
            dispatch_batch_key,
            dispatches,
            &dispatches_digest,
            accepted_at,
            &self.identity,
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
            DispatchState::Prepared,
            DispatchState::Dispatched,
            DispatchTransitionDisposition::Applied,
            None,
            None,
            None,
            at,
            true,
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
                "late_callback_quarantined",
                Some(record.state),
                record.state,
                DispatchTransitionDisposition::Quarantined,
                Some(provider_request_id),
                None,
                None,
                at,
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
            "provider_request_identified",
            Some(DispatchState::Dispatched),
            DispatchState::Dispatched,
            DispatchTransitionDisposition::Applied,
            Some(provider_request_id),
            None,
            None,
            at,
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
            DispatchState::Prepared,
            DispatchState::AbortedBeforeDispatch,
            DispatchTransitionDisposition::Applied,
            None,
            None,
            None,
            at,
            false,
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
                "settled",
                Some(DispatchState::Dispatched),
                DispatchState::Settled,
                DispatchTransitionDisposition::Applied,
                effective_request_id.as_deref(),
                Some(result_ref),
                None,
                at,
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
            "late_callback_quarantined",
            Some(record.state),
            record.state,
            DispatchTransitionDisposition::Quarantined,
            effective_request_id.as_deref(),
            Some(result_ref),
            None,
            at,
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
                kind,
                Some(prior),
                next,
                DispatchTransitionDisposition::Applied,
                record.provider_request_id.as_deref(),
                None,
                None,
                at,
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
            DispatchState::Unknown,
            DispatchState::Settled,
            DispatchTransitionDisposition::Reconciled,
            provider_request_id,
            Some(result_ref),
            None,
            at,
            false,
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
            DispatchState::Unknown,
            DispatchState::ReconciledNoEffect,
            DispatchTransitionDisposition::Reconciled,
            None,
            None,
            Some(evidence_reference),
            at,
            false,
        )
    }

    fn transition_dispatch(
        &self,
        identity: &DispatchIdentity,
        expected_state: DispatchState,
        next_state: DispatchState,
        disposition: DispatchTransitionDisposition,
        provider_request_id: Option<&str>,
        result_ref: Option<&ContentObjectRef>,
        reconciliation_reference: Option<&str>,
        at: &str,
        require_current_generation: bool,
    ) -> Result<DispatchTransitionResult, StorageError> {
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
            next_state.as_str(),
            Some(expected_state),
            next_state,
            disposition,
            effective_request_id.as_deref(),
            result_ref,
            reconciliation_reference,
            at,
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
        let transaction = connection.transaction()?;
        let state = load_persistence_state(&transaction, run_id)?;
        let aggregate = RunAggregate::restore(state)?;
        let high_water_sequence = run_event_high_water(&transaction, run_id)?;
        let snapshot = aggregate.snapshot(high_water_sequence);
        let capture_manifest = load_capture_manifest_from_db(
            &transaction,
            &snapshot.input.context_manifest.manifest_id,
        )?;
        if capture_manifest
            .as_ref()
            .is_some_and(|manifest| manifest.disclosure_state != DisclosureState::Approved)
        {
            return Err(StorageError::Corrupt(
                "run dossier references an unapproved source capture manifest".to_owned(),
            ));
        }
        let source_freshness = latest_source_freshness(&transaction, run_id)?;
        let dispatches = load_dispatches_for_run(&transaction, &self.identity, run_id)?;
        let dispatch_high_water = dispatch_event_high_water(&transaction, run_id)?;
        let decision_dossier_digest = verify_stored_decision_dossier(&transaction, &snapshot)?;
        transaction.commit()?;
        Ok(RunDossier {
            snapshot,
            capture_manifest,
            source_freshness,
            dispatches,
            decision_dossier_digest,
            replay_cursor: RunEventCursor {
                store_id: self.identity.store_id.clone(),
                store_generation: self.identity.generation,
                run_id: run_id.to_owned(),
                after_sequence: 0,
                high_water_sequence,
            },
            dispatch_replay_cursor: DispatchEventCursor {
                store_id: self.identity.store_id.clone(),
                store_generation: self.identity.generation,
                run_id: run_id.to_owned(),
                after_sequence: 0,
                high_water_sequence: dispatch_high_water,
            },
        })
    }

    pub fn events_after_for_run(
        &self,
        cursor: &RunEventCursor,
        limit: usize,
    ) -> Result<RunEventPage, StorageError> {
        if cursor.store_id != self.identity.store_id
            || cursor.store_generation != self.identity.generation
        {
            return Err(StorageError::CursorExpired);
        }
        if cursor.after_sequence > cursor.high_water_sequence {
            return Err(StorageError::Corrupt(
                "run replay cursor is beyond its high-water mark".to_owned(),
            ));
        }
        let limit = limit.min(MAX_EVENT_PAGE);
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
            let (sequence, stored_generation, revision, generation, payload_json, created_at) =
                row?;
            if sequence < 0 || stored_generation < 0 || revision < 0 || generation < 0 {
                return Err(StorageError::Corrupt(
                    "run replay event contains a negative sequence or revision".to_owned(),
                ));
            }
            let payload: EventPayload = serde_json::from_str(&payload_json)?;
            events.push(StoredEvent {
                position: EventPosition {
                    store_id: self.identity.store_id.clone(),
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
                connection.prepare("SELECT digest FROM content_objects ORDER BY digest")?;
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
            let unknown_external = has_unknown_external_dispatch(&transaction, &run_id)?;

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
                    aggregate.confirm_cancelled(
                        false,
                        "application restarted before cancellation was confirmed".to_owned(),
                        at.clone(),
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
        transaction.commit()?;
        Ok(crate::RecoveryReport {
            previous_generation,
            current_generation: self.identity.generation,
            findings,
        })
    }

    pub fn put_source_object(&self, bytes: &[u8]) -> Result<ContentObjectRef, StorageError> {
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
        if let Some((other_id, other_kind, other_target, other_digest, other_receipt)) = by_key {
            if other_id != command.command_id
                || other_kind != kind
                || other_target != target
                || other_digest != digest
                || other_receipt != receipt
            {
                return Err(StorageError::IdempotencyConflict);
            }
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
    run_id: &str,
    batch_key: &str,
    dispatches: &[PreparedDispatch],
    dispatches_digest: &Digest,
    created_at: &str,
    store: &StoreIdentity,
) -> Result<DispatchBatchReceipt, StorageError> {
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
            "prepared",
            None,
            DispatchState::Prepared,
            DispatchTransitionDisposition::Applied,
            None,
            None,
            None,
            &dispatch.created_at,
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
    let row: Option<(
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
    )> = connection
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
    event_kind: &str,
    previous_state: Option<DispatchState>,
    state: DispatchState,
    disposition: DispatchTransitionDisposition,
    provider_request_id: Option<&str>,
    result_ref: Option<&ContentObjectRef>,
    reconciliation_reference: Option<&str>,
    created_at: &str,
) -> Result<(), StorageError> {
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
    if let (Some(stored), Some(supplied)) = (stored, supplied) {
        if stored != supplied {
            return Err(StorageError::DispatchStateConflict {
                expected: "the previously recorded provider request ID".to_owned(),
                actual: "a different provider request ID".to_owned(),
            });
        }
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
            next.as_str(),
            Some(previous),
            next,
            DispatchTransitionDisposition::Applied,
            record.provider_request_id.as_deref(),
            None,
            None,
            at,
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

fn initialize_schema(connection: &mut Connection) -> Result<(), StorageError> {
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (version INTEGER PRIMARY KEY CHECK (version > 0), checksum TEXT NOT NULL, applied_at TEXT NOT NULL);",
    )?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let mut schema_version: u32 =
        transaction.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if schema_version > SCHEMA_VERSION {
        return Err(StorageError::FutureSchema {
            found: schema_version,
            supported: SCHEMA_VERSION,
        });
    }
    for version in 1..=schema_version {
        verify_migration_ledger(&transaction, version)?;
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
        _ => None,
    }
}

fn verify_migration_ledger(
    transaction: &Transaction<'_>,
    version: u32,
) -> Result<(), StorageError> {
    let expected_sql = migration_sql(version)
        .ok_or_else(|| StorageError::Integrity(format!("missing migration {version}")))?;
    let expected_checksum = Digest::from_bytes(expected_sql.as_bytes()).to_string();
    let recorded: Option<String> = transaction
        .query_row(
            "SELECT checksum FROM schema_migrations WHERE version = ?1",
            [version],
            |row| row.get(0),
        )
        .optional()?;
    match recorded {
        Some(value) if value == expected_checksum => Ok(()),
        Some(_) => Err(StorageError::MigrationChecksum { version }),
        None => Err(StorageError::Integrity(format!(
            "schema version {version} has no matching migration ledger entry"
        ))),
    }
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
