use super::*;
use crate::{ExternalReplay, ExternalReplaySummary, SharedReplay};
use magi_context::EvidenceLocator;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EvidenceView {
    pub source_id: String,
    pub object_digest: Digest,
    pub start_line: u64,
    pub end_line: u64,
    pub total_lines: u64,
    pub text: Option<String>,
    pub evidence_unavailable: bool,
    pub freshness: FreshnessStatus,
    pub representation_kind: Option<magi_context::RepresentationKind>,
    pub page: Option<u32>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub data_url: Option<String>,
    pub warnings: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeletionReceipt {
    pub run_id: String,
    pub deleted: bool,
    pub collected_objects: usize,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BackupManifest {
    pub format: String,
    pub schema_version: u32,
    pub store_id: String,
    pub generation: u64,
    pub event_high_water: u64,
    pub database_digest: Digest,
    pub objects: Vec<ContentObjectRef>,
    pub contains_private_sources: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RestoreReceipt {
    pub store_id: String,
    pub generation: u64,
    pub restored_objects: usize,
    pub requires_reauthorization: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EvidenceDeletionPreview {
    pub run_id: String,
    pub source_id: String,
    pub object_digests: Vec<Digest>,
    pub affected_run_ids: Vec<String>,
}

fn validate_frozen_capture_authority(
    connection: &Connection,
    dossier: &RunDossier,
) -> Result<(), StorageError> {
    let frozen = &dossier.snapshot.input.context_manifest;
    let Some(capture) = &dossier.capture_manifest else {
        return if frozen.sources.is_empty() {
            Ok(())
        } else {
            Err(StorageError::Corrupt(
                "run capture authority unavailable".into(),
            ))
        };
    };
    for role in &dossier.snapshot.input.role_set.roles {
        let provider_id = if let Some(binding) = &role.catalog_binding {
            if binding.provider_profile_id != role.binding.provider_profile_id
                || binding.profile_revision != role.binding.revision
            {
                return Err(StorageError::Corrupt("frozen recipient mismatch".into()));
            }
            binding.provider_id.clone()
        } else {
            let row: Option<(i64, String, String, String)> = connection.query_row(
                "SELECT revision,digest,runtime_home_id,payload_json FROM provider_profile_revisions WHERE provider_profile_id=?1 AND revision=?2",
                params![role.binding.provider_profile_id, to_sql_integer(role.binding.revision)?],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            ).optional()?;
            let (revision, digest, home, payload) =
                row.ok_or_else(|| StorageError::Corrupt("frozen recipient unavailable".into()))?;
            decode_provider_profile(
                &role.binding.provider_profile_id,
                revision,
                &digest,
                &home,
                &payload,
            )?
            .provider_id
        };
        let recipient = magi_context::Recipient {
            provider_id,
            account_profile_id: role.binding.provider_profile_id.clone(),
        };
        let generated = capture
            .run_manifest_for_recipient(&recipient)
            .map_err(|_| StorageError::Corrupt("run disclosure authority mismatch".into()))?;
        if generated != *frozen {
            return Err(StorageError::Corrupt(
                "run capture differs from frozen context".into(),
            ));
        }
    }
    Ok(())
}

impl Storage {
    pub(super) fn retire_clarification_draft_captures(
        &self,
        transaction: &Transaction<'_>,
        draft_ids: &[String],
        retired_at: &str,
    ) -> Result<Vec<Digest>, StorageError> {
        let mut manifests = std::collections::BTreeSet::new();
        let mut candidates = std::collections::BTreeSet::new();
        for draft_id in draft_ids {
            let mut statement=transaction.prepare("SELECT manifest_id,manifest_digest FROM clarification_capture_owners WHERE draft_id=?1")?;
            let owners = statement
                .query_map([draft_id], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            drop(statement);
            for (id, digest) in owners {
                if let Some(capture) = load_capture_manifest_from_db(transaction, &id)? {
                    if capture.digest.as_str() != digest {
                        return Err(StorageError::DispatchFenced);
                    }
                    for source in capture.content.sources {
                        for object in source
                            .object_digest
                            .into_iter()
                            .chain(source.derived_digest)
                        {
                            candidates.insert(object.to_string());
                        }
                    }
                }
                transaction.execute("INSERT INTO clarification_capture_retirements(manifest_id,draft_id,manifest_digest,retired_at) VALUES (?1,?2,?3,?4) ON CONFLICT(manifest_id,draft_id) DO NOTHING",params![id,draft_id,digest,retired_at])?;
                manifests.insert(id);
            }
            transaction.execute(
                "DELETE FROM clarification_drafts WHERE draft_id=?1",
                [draft_id],
            )?;
            transaction.execute("DELETE FROM context_drafts WHERE draft_id=?1", [draft_id])?;
        }
        for id in manifests {
            let referenced:bool=transaction.query_row("SELECT EXISTS(SELECT 1 FROM context_drafts WHERE manifest_id=?1 UNION ALL SELECT 1 FROM immutable_snapshots WHERE object_kind='run_context' AND object_id=?1 UNION ALL SELECT 1 FROM runs WHERE deleted_at IS NULL AND json_extract(input_json,'$.context_manifest.manifest_id')=?1)",[&id],|r|r.get(0))?;
            if !referenced {
                transaction.execute(
                    "DELETE FROM source_capture_manifest_objects WHERE manifest_id=?1",
                    [&id],
                )?;
                transaction.execute(
                    "DELETE FROM source_capture_manifests WHERE manifest_id=?1",
                    [&id],
                )?;
            }
        }
        let mut statement =
            transaction.prepare("SELECT input_json FROM runs WHERE deleted_at IS NULL")?;
        let inputs = statement
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .map(|json| serde_json::from_str::<InputSnapshot>(&json))
            .collect::<Result<Vec<_>, _>>()?;
        drop(statement);
        let mut statement=transaction.prepare("SELECT m.payload_json FROM context_drafts d JOIN source_capture_manifests m USING(manifest_id)")?;
        let drafts = statement
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .map(|json| serde_json::from_str::<SourceCaptureManifest>(&json))
            .collect::<Result<Vec<_>, _>>()?;
        drop(statement);
        for draft in &drafts {
            draft.validate()?;
        }
        let mut collect = Vec::new();
        for hex in candidates {
            let digest = Digest::from_hex(hex)?;
            let live: bool = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM live_runs WHERE result_digest=?1)",
                [digest.as_str()],
                |r| r.get(0),
            )?;
            let run = inputs.iter().any(|input| {
                input.context_manifest.sources.iter().any(|source| {
                    source.object_digest == digest
                        || source
                            .allowed_locators
                            .iter()
                            .any(|locator| locator.split(':').nth(1) == Some(digest.as_str()))
                })
            });
            let draft = drafts.iter().any(|manifest| {
                manifest.content.sources.iter().any(|source| {
                    source.object_digest.as_ref() == Some(&digest)
                        || source.derived_digest.as_ref() == Some(&digest)
                })
            });
            if !live && !run && !draft {
                transaction.execute("INSERT INTO unavailable_objects(digest,removed_at,reason) VALUES (?1,?2,'unreferenced') ON CONFLICT(digest) DO NOTHING",params![digest.as_str(),retired_at])?;
                collect.push(digest);
            }
        }
        Ok(collect)
    }

    pub fn load_run_evidence_dossier(&self, run_id: &str) -> Result<RunDossier, StorageError> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
        let dossier = load_run_dossier_from(&transaction, &self.identity, run_id)?;
        validate_frozen_capture_authority(&transaction, &dossier)?;
        transaction.commit()?;
        Ok(dossier)
    }

    pub fn restore_run(&self, run_id: &str) -> Result<RunAggregate, StorageError> {
        validate_text("run_id", run_id, 128)?;
        let mut c = self.connection()?;
        let tx = c.transaction()?;
        let run = RunAggregate::restore(load_persistence_state(&tx, run_id)?)?;
        tx.commit()?;
        Ok(run)
    }
    pub fn read_run_source_object(
        &self,
        run_id: &str,
        digest: &Digest,
    ) -> Result<Vec<u8>, StorageError> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
        let dossier = load_run_dossier_from(&transaction, &self.identity, run_id)?;
        validate_frozen_capture_authority(&transaction, &dossier)?;
        let manifest = dossier
            .capture_manifest
            .ok_or_else(|| StorageError::Corrupt("no source manifest".into()))?;
        if !manifest.content.sources.iter().any(|source| {
            source.state == ManifestSourceState::Captured
                && (source.object_digest.as_ref() == Some(digest)
                    || source.derived_digest.as_ref() == Some(digest))
        }) {
            return Err(StorageError::Corrupt(
                "object is outside run capture authority".into(),
            ));
        }
        let bytes = verify_object_on_disk(&transaction, &self.objects_root, digest)?;
        transaction.commit()?;
        Ok(bytes)
    }

    pub fn load_evidence(
        &self,
        run_id: &str,
        locator: &EvidenceLocator,
    ) -> Result<EvidenceView, StorageError> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
        let dossier = load_run_dossier_from(&transaction, &self.identity, run_id)?;
        validate_frozen_capture_authority(&transaction, &dossier)?;
        let manifest = dossier
            .capture_manifest
            .ok_or_else(|| StorageError::Corrupt("no source manifest".into()))?;
        let source = manifest
            .content
            .sources
            .iter()
            .find(|source| source.source_id == locator.source_id)
            .ok_or_else(|| StorageError::Corrupt("source not in run".into()))?;
        let freshness = dossier
            .source_freshness
            .iter()
            .find(|record| record.source_id == source.source_id)
            .map(|record| record.observation.status)
            .unwrap_or(FreshnessStatus::Unchecked);
        let view = super::evidence::render(source, locator, freshness, |digest| {
            verify_object_on_disk(&transaction, &self.objects_root, digest)
        })?;
        transaction.commit()?;
        Ok(view)
    }

    pub fn load_context_evidence(
        &self,
        draft_id: &str,
        context_revision: u64,
        locator: &EvidenceLocator,
    ) -> Result<EvidenceView, StorageError> {
        validate_draft_id(draft_id)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
        let row: Option<(u64, String)> = transaction.query_row("SELECT d.revision,m.payload_json FROM context_drafts d JOIN source_capture_manifests m ON m.manifest_id=d.manifest_id WHERE d.draft_id=?1", [draft_id], |row| Ok((row.get(0)?,row.get(1)?))).optional()?;
        let (actual, json) =
            row.ok_or_else(|| StorageError::Corrupt("context draft unavailable".into()))?;
        if actual != context_revision {
            return Err(StorageError::DraftRevisionConflict {
                expected: Some(context_revision),
                actual: Some(actual),
            });
        }
        let manifest: SourceCaptureManifest = serde_json::from_str(&json)?;
        manifest.validate()?;
        if manifest.disclosure_state != DisclosureState::Draft {
            return Err(StorageError::Corrupt(
                "context draft is not a draft manifest".into(),
            ));
        }
        let source = manifest
            .content
            .sources
            .iter()
            .find(|source| source.source_id == locator.source_id)
            .ok_or_else(|| StorageError::Corrupt("source not in draft".into()))?;
        let view =
            super::evidence::render(source, locator, FreshnessStatus::Unchecked, |digest| {
                verify_object_on_disk(&transaction, &self.objects_root, digest)
            })?;
        transaction.commit()?;
        Ok(view)
    }

    pub fn preview_evidence_deletion(
        &self,
        run_id: &str,
        source_id: &str,
    ) -> Result<EvidenceDeletionPreview, StorageError> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
        let preview = evidence_deletion_preview(&transaction, &self.identity, run_id, source_id)?;
        transaction.commit()?;
        Ok(preview)
    }

    pub fn delete_evidence(
        &self,
        run_id: &str,
        source_id: &str,
        expected_revision: u64,
        expected_affected: &[String],
        removed_at: &str,
    ) -> Result<EvidenceDeletionPreview, StorageError> {
        validate_text("removed_at", removed_at, 64)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let preview = evidence_deletion_preview(&transaction, &self.identity, run_id, source_id)?;
        let actual = current_run_revision(&transaction, run_id)?;
        if actual != expected_revision {
            return Err(StorageError::RevisionConflict {
                expected: expected_revision,
                actual,
            });
        }
        if preview.affected_run_ids != expected_affected {
            return Err(StorageError::DeletionBlocked);
        }
        for digest in &preview.object_digests {
            let result_reference: bool = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM live_runs WHERE result_digest=?1)",
                [digest.as_str()],
                |row| row.get(0),
            )?;
            if result_reference {
                return Err(StorageError::DeletionBlocked);
            }
        }
        for affected in &preview.affected_run_ids {
            require_no_external_work(&transaction, affected)?;
            transaction.execute("UPDATE disclosure_grants SET revoked_at_epoch_ms=COALESCE(revoked_at_epoch_ms,0) WHERE run_id=?1",[affected])?;
            for digest in &preview.object_digests {
                transaction.execute("INSERT INTO evidence_tombstones(run_id,digest,removed_at) VALUES(?1,?2,?3) ON CONFLICT(run_id,digest) DO NOTHING",params![affected,digest.as_str(),removed_at])?;
            }
        }
        for digest in &preview.object_digests {
            transaction.execute("INSERT INTO unavailable_objects(digest,removed_at,reason) VALUES(?1,?2,'user_deleted') ON CONFLICT(digest) DO NOTHING",params![digest.as_str(),removed_at])?;
        }
        transaction.commit()?;
        for digest in &preview.object_digests {
            let path = self.object_path(digest);
            if path.exists() {
                reject_symlink_if_present(&path)?;
                fs::remove_file(path)?;
            }
        }
        Ok(preview)
    }

    pub fn delete_run(
        &self,
        run_id: &str,
        expected_revision: u64,
        deleted_at: &str,
    ) -> Result<DeletionReceipt, StorageError> {
        validate_text("run_id", run_id, 128)?;
        validate_text("deleted_at", deleted_at, 64)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM run_tombstones WHERE run_id=?1)",
            [run_id],
            |r| r.get::<_, bool>(0),
        )? {
            let valid:bool=transaction.query_row("SELECT EXISTS(SELECT 1 FROM runs r JOIN run_tombstones t USING(run_id) WHERE r.run_id=?1 AND r.deleted_at=t.deleted_at AND r.input_digest=t.payload_digest AND r.run_json='{}' AND r.input_json='{}' AND r.question_text='' AND r.tally_json IS NULL AND NOT EXISTS(SELECT 1 FROM live_runs l WHERE l.run_id=r.run_id) AND NOT EXISTS(SELECT 1 FROM commands c WHERE c.target_id=r.run_id))",[run_id],|r|r.get(0))?;
            if !valid {
                return Err(StorageError::Integrity(
                    "deleted run authority does not match its redacted record".into(),
                ));
            }
            let mut statement=transaction.prepare("SELECT e.digest FROM evidence_tombstones e JOIN unavailable_objects u USING(digest) WHERE e.run_id=?1")?;
            let digests = statement
                .query_map([run_id], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            drop(statement);
            transaction.commit()?;
            for hex in digests {
                let path = self.object_path(&Digest::from_hex(hex)?);
                if path.exists() {
                    reject_symlink_if_present(&path)?;
                    fs::remove_file(path)?;
                }
            }
            return Ok(DeletionReceipt {
                run_id: run_id.into(),
                deleted: true,
                collected_objects: 0,
            });
        }
        let aggregate = RunAggregate::restore(load_persistence_state(&transaction, run_id)?)?;
        let actual = aggregate.run().revision;
        if actual != expected_revision {
            return Err(StorageError::RevisionConflict {
                expected: expected_revision,
                actual,
            });
        }
        require_no_external_work(&transaction, run_id)?;
        if transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM runs WHERE parent_run_id=?1 AND deleted_at IS NULL)",
            [run_id],
            |r| r.get::<_, bool>(0),
        )? {
            return Err(StorageError::DeletionBlocked);
        }
        let has_live: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM live_runs WHERE run_id=?1)",
            [run_id],
            |r| r.get(0),
        )?;
        let mut candidates = std::collections::BTreeSet::new();
        let mut live_admission_digest = None;
        if has_live {
            let live = load_live_run_snapshot_from(
                &transaction,
                &self.identity,
                &self.objects_root,
                run_id,
                0,
            )?;
            live_admission_digest = Some(live_run_request_digest(
                &live.question,
                &live.model_binding,
            )?);
            if let Some(result) = live.result {
                candidates.insert(result.content_digest.to_string());
            }
        }
        let dossier = load_run_dossier_from(&transaction, &self.identity, run_id)?;
        let capture_authority = dossier
            .capture_manifest
            .as_ref()
            .map(|manifest| (manifest.manifest_id.clone(), manifest.digest.clone()));
        if let Some(manifest) = dossier.capture_manifest {
            for source in manifest.content.sources {
                for digest in source
                    .object_digest
                    .into_iter()
                    .chain(source.derived_digest)
                {
                    candidates.insert(digest.to_string());
                }
            }
        }
        let input = aggregate.input();
        transaction.execute("INSERT INTO run_tombstones(run_id,conversation_id,deleted_at,payload_digest) VALUES(?1,?2,?3,?4)",params![run_id,aggregate.run().conversation_id,deleted_at,input.input_digest.as_str()])?;
        if let Some((id, digest)) = &capture_authority {
            transaction.execute("INSERT INTO run_capture_manifest_tombstones(run_id,manifest_id,manifest_digest) VALUES(?1,?2,?3)",params![run_id,id,digest.as_str()])?;
        }
        transaction.execute("INSERT INTO run_command_tombstones(command_kind,command_id,target_id,idempotency_key,payload_digest,run_id,deleted_at) SELECT command_kind,command_id,target_id,idempotency_key,payload_digest,?1,?2 FROM commands WHERE target_id=?1",params![run_id,deleted_at])?;
        if let Some(digest) = live_admission_digest {
            transaction.execute("INSERT INTO run_command_tombstones(command_kind,command_id,target_id,idempotency_key,payload_digest,run_id,deleted_at) SELECT 'live_admission',command_id,run_id,idempotency_key,?3,run_id,?2 FROM live_run_receipts WHERE run_id=?1",params![run_id,deleted_at,digest.as_str()])?;
        }
        transaction.execute("INSERT INTO run_command_tombstones(command_kind,command_id,target_id,idempotency_key,payload_digest,run_id,deleted_at) SELECT 'live_cancel',command_id,run_id,idempotency_key,payload_digest,run_id,?2 FROM live_run_cancellation_receipts WHERE run_id=?1",params![run_id,deleted_at])?;
        for table in [
            "live_run_cancellation_receipts",
            "live_run_receipts",
            "live_run_events",
            "live_run_dispatch_reservations",
            "live_run_outbox",
            "live_runs",
            "ballots",
            "role_assessments",
            "proposals",
            "decision_dossiers",
            "run_checkpoints",
            "run_events",
            "source_freshness_observations",
            "dispatch_transition_events",
            "dispatch_outbox",
            "dispatch_batches",
            "disclosure_grants",
        ] {
            transaction.execute(&format!("DELETE FROM {table} WHERE run_id=?1"), [run_id])?;
        }
        transaction.execute("DELETE FROM commands WHERE target_id=?1", [run_id])?;
        transaction.execute("UPDATE runs SET deleted_at=?2,run_json='{}',input_json='{}',question_text='',tally_json=NULL WHERE run_id=?1",params![run_id,deleted_at])?;
        bump_history_membership_generation(&transaction)?;
        let mut clarification_statement = transaction
            .prepare("SELECT draft_id FROM clarification_drafts WHERE parent_run_id=?1")?;
        let clarification_ids = clarification_statement
            .query_map([run_id], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        drop(clarification_statement);
        for digest in
            self.retire_clarification_draft_captures(&transaction, &clarification_ids, deleted_at)?
        {
            candidates.insert(digest.to_string());
        }
        let mut statement =
            transaction.prepare("SELECT input_json FROM runs WHERE deleted_at IS NULL")?;
        let remaining = statement
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .map(|json| serde_json::from_str::<InputSnapshot>(&json))
            .collect::<Result<Vec<_>, _>>()?;
        drop(statement);
        for (kind, id) in [
            ("question", input.question.question_id.as_str()),
            ("run_context", input.context_manifest.manifest_id.as_str()),
            ("role_set", input.role_set.role_set_id.as_str()),
        ] {
            let referenced = remaining.iter().any(|other| match kind {
                "question" => other.question.question_id.as_str() == id,
                "run_context" => other.context_manifest.manifest_id.as_str() == id,
                _ => other.role_set.role_set_id.as_str() == id,
            });
            if !referenced {
                transaction.execute(
                    "DELETE FROM immutable_snapshots WHERE object_kind=?1 AND object_id=?2",
                    params![kind, id],
                )?;
            }
        }
        if let Some((id, _)) = &capture_authority {
            let referenced: bool = transaction.query_row("SELECT EXISTS(SELECT 1 FROM context_drafts WHERE manifest_id=?1) OR EXISTS(SELECT 1 FROM immutable_snapshots WHERE object_kind='run_context' AND object_id=?1)",[id],|row| row.get(0))?;
            if !referenced
                && !remaining
                    .iter()
                    .any(|other| &other.context_manifest.manifest_id == id)
            {
                transaction.execute(
                    "DELETE FROM source_capture_manifest_objects WHERE manifest_id=?1",
                    [id],
                )?;
                transaction.execute(
                    "DELETE FROM source_capture_manifests WHERE manifest_id=?1",
                    [id],
                )?;
            }
        }
        let mut draft_statement=transaction.prepare("SELECT m.payload_json FROM context_drafts d JOIN source_capture_manifests m USING(manifest_id)")?;
        let draft_manifests = draft_statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .map(|json| serde_json::from_str::<SourceCaptureManifest>(&json))
            .collect::<Result<Vec<_>, _>>()?;
        for manifest in &draft_manifests {
            manifest.validate()?;
        }
        drop(draft_statement);
        let remaining_capture_manifests = remaining
            .iter()
            .map(|input| {
                load_capture_manifest_from_db(&transaction, &input.context_manifest.manifest_id)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut collect = Vec::new();
        for hex in candidates {
            let digest = Digest::from_hex(hex)?;
            let other_result: bool = transaction.query_row(
                "SELECT EXISTS(SELECT 1 FROM live_runs WHERE result_digest=?1)",
                [digest.as_str()],
                |r| r.get(0),
            )?;
            let other_run = remaining_capture_manifests
                .iter()
                .flatten()
                .any(|manifest| {
                    manifest.content.sources.iter().any(|source| {
                        source.object_digest.as_ref() == Some(&digest)
                            || source.derived_digest.as_ref() == Some(&digest)
                    })
                })
                || remaining.iter().any(|other| {
                    other.context_manifest.sources.iter().any(|source| {
                        source.object_digest == digest
                            || source
                                .allowed_locators
                                .iter()
                                .any(|locator| locator.split(':').nth(1) == Some(digest.as_str()))
                    })
                });
            let draft = draft_manifests.iter().any(|manifest| {
                manifest.content.sources.iter().any(|source| {
                    source.object_digest.as_ref() == Some(&digest)
                        || source.derived_digest.as_ref() == Some(&digest)
                })
            });
            if !other_result && !other_run && !draft {
                transaction.execute("INSERT INTO unavailable_objects(digest,removed_at,reason) VALUES(?1,?2,'unreferenced') ON CONFLICT(digest) DO NOTHING",params![digest.as_str(),deleted_at])?;
                transaction.execute("INSERT INTO evidence_tombstones(run_id,digest,removed_at) VALUES(?1,?2,?3) ON CONFLICT(run_id,digest) DO NOTHING",params![run_id,digest.as_str(),deleted_at])?;
                collect.push(digest);
            }
        }
        transaction.commit()?;
        for digest in &collect {
            let path = self.object_path(digest);
            if path.exists() {
                reject_symlink_if_present(&path)?;
                fs::remove_file(path)?;
            }
        }
        Ok(DeletionReceipt {
            run_id: run_id.into(),
            deleted: true,
            collected_objects: collect.len(),
        })
    }

    pub fn import_external_replay(
        &self,
        bytes: &[u8],
        now: u64,
    ) -> Result<ExternalReplay, StorageError> {
        let replay = crate::import_replay(bytes, now)?;
        self.persist_external_replay(&replay)?;
        Ok(replay)
    }

    pub fn save_external_replay(
        &self,
        replay: &ExternalReplay,
        original_bytes: &[u8],
    ) -> Result<(), StorageError> {
        replay.validate()?;
        let original = SharedReplay::parse(original_bytes)?;
        let encode = |data: &SharedReplay| {
            serde_json::to_vec(data)
                .map_err(|_| StorageError::Corrupt("invalid shared replay contract".into()))
        };
        if replay.file_digest != Digest::from_bytes(original_bytes)
            || encode(&original)? != encode(&replay.data)?
        {
            return Err(StorageError::Integrity(
                "external replay differs from its imported bytes".into(),
            ));
        }
        self.persist_external_replay(replay)
    }

    fn persist_external_replay(&self, replay: &ExternalReplay) -> Result<(), StorageError> {
        replay.validate()?;
        if replay.imported_at_epoch_ms > i64::MAX as u64 {
            return Err(StorageError::Corrupt(
                "invalid external replay import time".into(),
            ));
        }
        let payload = serde_json::to_string(replay)
            .map_err(|_| StorageError::Corrupt("invalid shared replay contract".into()))?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing: Option<(String, u64, String)> = transaction.query_row("SELECT file_digest,imported_at_epoch_ms,payload_json FROM external_replays WHERE external_replay_id=?1",[&replay.external_replay_id],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).optional()?;
        if let Some((digest, imported_at, stored)) = existing {
            let prior = validated_external_replay_row(
                &replay.external_replay_id,
                &digest,
                imported_at,
                &stored,
            )?;
            let normalized = serde_json::to_string(&prior)
                .map_err(|_| StorageError::Corrupt("invalid shared replay contract".into()))?;
            if normalized != payload {
                return Err(StorageError::ImmutableConflict("external replay".into()));
            }
        } else {
            transaction.execute("INSERT INTO external_replays(external_replay_id,file_digest,payload_json,imported_at_epoch_ms) VALUES(?1,?2,?3,?4)",params![replay.external_replay_id,replay.file_digest.as_str(),payload,replay.imported_at_epoch_ms])?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn load_external_replay(&self, id: &str) -> Result<ExternalReplay, StorageError> {
        validate_external_replay_id(id)?;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
        let row: Option<(String,u64,String)> = transaction.query_row("SELECT file_digest,imported_at_epoch_ms,payload_json FROM external_replays WHERE external_replay_id=?1",[id],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?))).optional()?;
        let (digest, imported_at, payload) =
            row.ok_or_else(|| StorageError::Corrupt("external replay does not exist".into()))?;
        let replay = validated_external_replay_row(id, &digest, imported_at, &payload)?;
        transaction.commit()?;
        Ok(replay)
    }

    pub fn list_external_replays(
        &self,
        before_imported_at_epoch_ms: Option<u64>,
        before_external_replay_id: Option<&str>,
        limit: u32,
    ) -> Result<Vec<ExternalReplaySummary>, StorageError> {
        if !(1..=100).contains(&limit)
            || before_imported_at_epoch_ms.is_some() != before_external_replay_id.is_some()
            || before_imported_at_epoch_ms.is_some_and(|at| at > i64::MAX as u64)
        {
            return Err(StorageError::Corrupt(
                "invalid external replay page request".into(),
            ));
        }
        if let Some(id) = before_external_replay_id {
            validate_external_replay_id(id)?;
        }
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
        let mut statement = transaction.prepare("SELECT external_replay_id,file_digest,imported_at_epoch_ms,payload_json FROM external_replays WHERE ?1 IS NULL OR imported_at_epoch_ms<?1 OR (imported_at_epoch_ms=?1 AND external_replay_id<?2) ORDER BY imported_at_epoch_ms DESC,external_replay_id DESC LIMIT ?3")?;
        let rows = statement.query_map(
            params![
                before_imported_at_epoch_ms,
                before_external_replay_id,
                limit
            ],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, u64>(2)?,
                    row.get::<_, String>(3)?,
                ))
            },
        )?;
        let mut summaries = Vec::new();
        for row in rows {
            let (id, digest, imported_at, payload) = row?;
            let replay = validated_external_replay_row(&id, &digest, imported_at, &payload)?;
            summaries.push(ExternalReplaySummary {
                external_replay_id: id,
                file_digest: replay.file_digest,
                schema_version: replay.schema_version,
                imported_at_epoch_ms: imported_at,
                title: replay.data.title,
                question: replay.data.question,
                external: true,
            });
        }
        drop(statement);
        transaction.commit()?;
        Ok(summaries)
    }

    pub fn create_backup(&self, destination: &Path) -> Result<BackupManifest, StorageError> {
        prepare_empty(destination)?;
        let c = self.connection()?;
        let db = destination.join("magi.sqlite");
        c.execute(
            "VACUUM INTO ?1",
            [db.to_str()
                .ok_or_else(|| StorageError::Corrupt("non-UTF8 backup path".into()))?],
        )?;
        set_private_file_permissions(&db)?;
        File::open(&db)?.sync_all()?;
        let snapshot =
            Connection::open_with_flags(&db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let snapshot_identity = read_only_store_identity(&snapshot)?;
        let high: u64 = snapshot.query_row(
            "SELECT COALESCE(MAX(sequence),0) FROM run_events",
            [],
            |r| r.get(0),
        )?;
        let mut q = snapshot.prepare("SELECT digest,byte_length FROM content_objects WHERE digest NOT IN (SELECT digest FROM unavailable_objects) ORDER BY digest")?;
        let objects = q
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, u64>(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        let mut refs = Vec::new();
        for (hex, byte_length) in objects {
            let digest = Digest::from_hex(hex)?;
            let b = verify_object_on_disk(&snapshot, &self.objects_root, &digest)?;
            let dir = destination.join("objects").join(&digest.as_str()[..2]);
            create_private_dir(&dir)?;
            write_new(&dir.join(&digest.as_str()[2..]), &b)?;
            File::open(&dir)?.sync_all()?;
            refs.push(ContentObjectRef {
                digest,
                byte_length,
            });
        }
        if destination.join("objects").exists() {
            File::open(destination.join("objects"))?.sync_all()?;
        }
        let m = BackupManifest {
            format: "magi-backup".into(),
            schema_version: snapshot_identity.schema_version,
            store_id: snapshot_identity.store_id,
            generation: snapshot_identity.generation,
            event_high_water: high,
            database_digest: Digest::from_bytes(&read_backup_file(&db, 1024 * 1024 * 1024)?),
            objects: refs,
            contains_private_sources: true,
        };
        let b = serde_json::to_vec_pretty(&m)?;
        write_new(&destination.join("manifest.json"), &b)?;
        write_new(
            &destination.join("COMPLETE"),
            Digest::from_bytes(&b).as_str().as_bytes(),
        )?;
        File::open(destination)?.sync_all()?;
        Ok(m)
    }
    pub fn restore_backup(
        backup: &Path,
        destination: &Path,
    ) -> Result<RestoreReceipt, StorageError> {
        reject_symlink_if_present(destination)?;
        if destination.exists() && fs::read_dir(destination)?.next().is_some() {
            return Err(StorageError::ImmutableConflict(
                "destination must be an empty separate directory".into(),
            ));
        }
        let parent = destination
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        reject_symlink_if_present(parent)?;
        let stage = parent.join(format!(".magi-restore-{}", Uuid::new_v4()));
        fs::create_dir(&stage)?;
        let cleanup = RestoreStage(stage.clone());
        let receipt = Self::restore_backup_staged(backup, &stage)?;
        File::open(&stage)?.sync_all()?;
        reject_symlink_if_present(destination)?;
        fs::rename(&stage, destination)?;
        File::open(parent)?.sync_all()?;
        drop(cleanup);
        Ok(receipt)
    }
    fn restore_backup_staged(
        backup: &Path,
        destination: &Path,
    ) -> Result<RestoreReceipt, StorageError> {
        reject_symlink_if_present(backup)?;
        let b = read_backup_file(&backup.join("manifest.json"), 10 * 1024 * 1024)?;
        if read_backup_file(&backup.join("COMPLETE"), 64)?
            != Digest::from_bytes(&b).as_str().as_bytes()
        {
            return Err(StorageError::Integrity("invalid complete marker".into()));
        }
        let m: BackupManifest = serde_json::from_slice(&b)?;
        if m.format != "magi-backup" || !(1..=SCHEMA_VERSION).contains(&m.schema_version) {
            return Err(StorageError::Corrupt("unsupported backup".into()));
        }
        let db = read_backup_file(&backup.join("magi.sqlite"), 1024 * 1024 * 1024)?;
        if Digest::from_bytes(&db) != m.database_digest {
            return Err(StorageError::Integrity("database digest mismatch".into()));
        }
        prepare_empty(destination)?;
        create_private_dir(&destination.join("state"))?;
        write_new(&destination.join("state/magi.sqlite"), &db)?;
        let c = Connection::open(destination.join("state/magi.sqlite"))?;
        let check: String = c.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
        if check != "ok" {
            return Err(StorageError::Integrity(check));
        }
        let schema: u32 = c.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if schema != m.schema_version {
            return Err(StorageError::Integrity(
                "backup schema differs from manifest".into(),
            ));
        }
        let identity = read_only_store_identity(&c)?;
        let high: u64 = c.query_row(
            "SELECT COALESCE(MAX(sequence),0) FROM run_events",
            [],
            |row| row.get(0),
        )?;
        if identity.store_id != m.store_id
            || identity.generation != m.generation
            || high != m.event_high_water
        {
            return Err(StorageError::Integrity(
                "backup identity or event fence differs from manifest".into(),
            ));
        }
        let unavailable: bool = c.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='unavailable_objects')",
            [],
            |row| row.get(0),
        )?;
        let inventory = if unavailable {
            "SELECT COUNT(*) FROM content_objects WHERE digest NOT IN (SELECT digest FROM unavailable_objects)"
        } else {
            "SELECT COUNT(*) FROM content_objects"
        };
        let count: u64 = c.query_row(inventory, [], |r| r.get(0))?;
        if count as usize != m.objects.len() {
            return Err(StorageError::Integrity(
                "incomplete object inventory".into(),
            ));
        }
        let mut seen = std::collections::BTreeSet::new();
        for o in &m.objects {
            if !o.digest.is_valid() || !seen.insert(o.digest.to_string()) {
                return Err(StorageError::Corrupt("invalid or duplicate digest".into()));
            }
            let b = verify_object_on_disk(&c, &backup.join("objects"), &o.digest)?;
            if b.len() as u64 != o.byte_length {
                return Err(StorageError::Integrity("object length mismatch".into()));
            }
            let dir = destination.join("objects").join(&o.digest.as_str()[..2]);
            create_private_dir(&dir)?;
            write_new(&dir.join(&o.digest.as_str()[2..]), &b)?;
        }
        drop(c);
        let restored = Storage::open_or_create(destination)?;
        restored.integrity_check()?;
        {
            let mut connection = restored.connection()?;
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let lineage = Uuid::new_v4().simple().to_string();
            transaction.execute(
                "INSERT INTO admission_execution_lineages(lineage_id,restored) VALUES(?1,1)",
                [&lineage],
            )?;
            transaction.execute(
                "UPDATE store_meta SET value=?1 WHERE key='admission_lineage_id'",
                [&lineage],
            )?;
            transaction.execute(
                "UPDATE store_meta SET value='inert' WHERE key='admission_activation_state'",
                [],
            )?;
            transaction.commit()?;
            connection.execute("UPDATE disclosure_grants SET revoked_at_epoch_ms=0", [])?;
            connection.execute(
                "UPDATE provider_source_scopes SET revoked_at='restored' WHERE revoked_at IS NULL",
                [],
            )?;
            connection.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        }
        File::open(destination.join("state/magi.sqlite"))?.sync_all()?;
        File::open(destination.join("state"))?.sync_all()?;
        if destination.join("objects").exists() {
            for entry in fs::read_dir(destination.join("objects"))? {
                File::open(entry?.path())?.sync_all()?;
            }
            File::open(destination.join("objects"))?.sync_all()?;
        }
        Ok(RestoreReceipt {
            store_id: restored.identity.store_id.clone(),
            generation: restored.identity.generation,
            restored_objects: m.objects.len(),
            requires_reauthorization: true,
        })
    }
}
fn read_backup_file(path: &Path, maximum: u64) -> Result<Vec<u8>, StorageError> {
    let before = fs::symlink_metadata(path)?;
    if !before.file_type().is_file() || before.len() > maximum {
        return Err(StorageError::Integrity(
            "backup file must be a bounded regular file".into(),
        ));
    }
    let file = File::open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let opened = file.metadata()?;
        let after = fs::symlink_metadata(path)?;
        if !after.file_type().is_file()
            || before.dev() != opened.dev()
            || before.ino() != opened.ino()
            || before.dev() != after.dev()
            || before.ino() != after.ino()
        {
            return Err(StorageError::Integrity(
                "backup file changed before reading".into(),
            ));
        }
    }
    let mut bytes = Vec::new();
    file.take(maximum.saturating_add(1))
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > maximum {
        return Err(StorageError::Integrity(
            "backup file exceeds byte limit".into(),
        ));
    }
    Ok(bytes)
}

struct RestoreStage(PathBuf);
impl Drop for RestoreStage {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn prepare_empty(p: &Path) -> Result<(), StorageError> {
    reject_symlink_if_present(p)?;
    if p.exists() && fs::read_dir(p)?.next().is_some() {
        return Err(StorageError::ImmutableConflict(
            "destination must be an empty separate directory".into(),
        ));
    }
    create_private_dir(p)
}
fn write_new(p: &Path, b: &[u8]) -> Result<(), StorageError> {
    let mut o = OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    let mut f = o.open(p)?;
    f.write_all(b)?;
    f.sync_all()?;
    Ok(())
}

fn require_no_external_work(
    transaction: &Transaction<'_>,
    run_id: &str,
) -> Result<(), StorageError> {
    let dossier =
        load_run_dossier_from(transaction, &read_only_store_identity(transaction)?, run_id)?;
    if !matches!(
        dossier.snapshot.run.status,
        RunStatus::Completed { .. } | RunStatus::Cancelled | RunStatus::Failed { .. }
    ) || has_unknown_external_dispatch(transaction, run_id)?
    {
        return Err(StorageError::DeletionBlocked);
    }
    let live: Option<String> = transaction
        .query_row(
            "SELECT state FROM live_run_outbox WHERE run_id=?1",
            [run_id],
            |row| row.get(0),
        )
        .optional()?;
    if live
        .as_deref()
        .is_some_and(|state| !matches!(state, "completed" | "cancelled" | "failed"))
    {
        return Err(StorageError::DeletionBlocked);
    }
    if live.is_some() {
        load_live_dispatches_from(transaction, &dossier.snapshot)?;
    }
    let unresolved:bool=transaction.query_row("SELECT EXISTS(SELECT 1 FROM live_run_dispatch_reservations WHERE run_id=?1 AND state IN ('reserved','active','unknown'))",[run_id],|row|row.get(0))?;
    if unresolved {
        return Err(StorageError::DeletionBlocked);
    }
    Ok(())
}

fn evidence_deletion_preview(
    connection: &Connection,
    identity: &StoreIdentity,
    run_id: &str,
    source_id: &str,
) -> Result<EvidenceDeletionPreview, StorageError> {
    let dossier = load_run_dossier_from(connection, identity, run_id)?;
    let source = dossier
        .capture_manifest
        .as_ref()
        .and_then(|manifest| {
            manifest
                .content
                .sources
                .iter()
                .find(|source| source.source_id == source_id)
        })
        .ok_or_else(|| StorageError::Corrupt("source not in run".into()))?;
    let mut object_digests: Vec<_> = source
        .object_digest
        .iter()
        .chain(source.derived_digest.iter())
        .cloned()
        .collect();
    object_digests.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    object_digests.dedup();
    if object_digests.is_empty() {
        return Err(StorageError::Corrupt(
            "source has no captured objects".into(),
        ));
    }
    let ids = connection
        .prepare("SELECT run_id FROM runs WHERE deleted_at IS NULL ORDER BY run_id")?
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    let mut affected_run_ids = Vec::new();
    for id in ids {
        let run = load_run_dossier_from(connection, identity, &id)?;
        let referenced = run.capture_manifest.as_ref().is_some_and(|manifest| {
            manifest.content.sources.iter().any(|source| {
                source
                    .object_digest
                    .iter()
                    .chain(source.derived_digest.iter())
                    .any(|digest| object_digests.contains(digest))
            })
        });
        if referenced {
            affected_run_ids.push(id);
        }
    }
    Ok(EvidenceDeletionPreview {
        run_id: run_id.into(),
        source_id: source_id.into(),
        object_digests,
        affected_run_ids,
    })
}

fn validate_external_replay_id(id: &str) -> Result<(), StorageError> {
    if Uuid::parse_str(id).is_err() {
        return Err(StorageError::Corrupt(
            "invalid external replay identifier".into(),
        ));
    }
    Ok(())
}

fn validated_external_replay_row(
    id: &str,
    digest: &str,
    imported_at: u64,
    payload: &str,
) -> Result<ExternalReplay, StorageError> {
    validate_external_replay_id(id)?;
    let replay = ExternalReplay::parse_stored(payload)?;
    if replay.external_replay_id != id
        || replay.file_digest.as_str() != digest
        || replay.imported_at_epoch_ms != imported_at
    {
        return Err(StorageError::Integrity(
            "external replay metadata mismatch".into(),
        ));
    }
    Ok(replay)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn captured_evidence_fixture(storage: &Storage) -> (RunAggregate, EvidenceLocator) {
        use magi_context::{ManifestSource, Recipient, RepresentationKind};
        use magi_domain::{
            CoreRoleProfile, ModelBindingSnapshot, ProviderCatalogInput, ProviderCatalogModel,
            QuestionKind, QuestionSnapshot, RoleSetSnapshot, Run,
        };
        let original = storage
            .put_source_object(b"PRIVATE_BEFORE\nPUBLIC_SELECTED\nPRIVATE_AFTER\n")
            .unwrap();
        let derived = storage.put_source_object(b"PUBLIC_SELECTED\n").unwrap();
        let locator = EvidenceLocator {
            source_id: "memo".into(),
            object_digest: original.digest.clone(),
            start_line: Some(2),
            end_line: Some(2),
            total_lines: Some(3),
            page: None,
            width: None,
            height: None,
        };
        let draft = SourceCaptureManifest::draft(
            vec![ManifestSource {
                source_id: "memo".into(),
                display_name: "memo.txt".into(),
                state: ManifestSourceState::Captured,
                byte_length: Some(original.byte_length),
                mime_type: Some("text/plain".into()),
                object_digest: Some(original.digest),
                derived_digest: Some(derived.digest),
                representation_kind: Some(RepresentationKind::Utf8Text),
                extractor_id: Some("utf8-text".into()),
                extractor_version: Some("1".into()),
                included_locators: vec![locator.clone()],
                omission: None,
                captured_at_epoch_ms: Some(1),
                secret_pattern_findings: vec![],
                secret_scan_incomplete: false,
            }],
            1,
        )
        .unwrap();
        let recipient = Recipient {
            provider_id: "codex".into(),
            account_profile_id: "saved-profile".into(),
        };
        let capture = draft
            .confirm_disclosure(vec![recipient.clone()], &draft.digest, 2)
            .unwrap();
        storage.commit_context_manifest(&capture).unwrap();
        let catalog = ProviderCatalogSnapshot::new(
            ProviderCatalogInput {
                catalog_snapshot_id: "saved-catalog".into(),
                provider_id: "codex".into(),
                provider_profile_id: "saved-profile".into(),
                profile_revision: 1,
                adapter_id: "codex-acp".into(),
                adapter_version: "1".into(),
                adapter_digest: Digest::from_bytes(b"adapter"),
                fetched_at: "2026-10-01T00:00:00Z".into(),
            },
            vec![ProviderCatalogModel {
                model_id: "observed-model".into(),
                name: None,
                description: None,
                context_window_tokens: None,
                max_output_tokens: None,
            }],
        )
        .unwrap();
        let binding = AcpModelBindingSnapshot::from_catalog(&catalog, "observed-model").unwrap();
        let roles = CoreId::ALL.map(|core_id| CoreRoleProfile {
            core_id,
            profile_id: format!("role-{}", core_id.wire_name()),
            revision: 1,
            display_name: core_id.wire_name().into(),
            review_purpose: "Review".into(),
            evaluation_criteria: vec!["Evidence".into()],
            falsification_questions: vec!["Counterexample?".into()],
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
        });
        let input = InputSnapshot::new(
            QuestionSnapshot::new(
                "question".into(),
                QuestionKind::Answer,
                "Evaluate evidence".into(),
                vec![],
                vec![],
                vec![],
            )
            .unwrap(),
            capture.run_manifest_for_recipient(&recipient).unwrap(),
            RoleSetSnapshot::new("roles".into(), roles).unwrap(),
            Digest::from_bytes(b"policy"),
        )
        .unwrap();
        let run = Run::new(
            "evidence-run".into(),
            "evidence-conversation".into(),
            None,
            &input,
            "2026-10-01T00:00:00Z".into(),
        )
        .unwrap();
        let mut aggregate = RunAggregate::new(run, input).unwrap();
        let command = CommandEnvelope {
            command_id: "evidence-create".into(),
            idempotency_key: "evidence-create".into(),
            command_kind: CommandKind::CreateRun,
            target_id: "evidence-run".into(),
            expected_revision: 0,
            payload_digest: aggregate.input().input_digest.clone(),
        };
        storage
            .commit_run(&command, &mut aggregate, "2026-10-01T00:00:00Z", None)
            .unwrap();
        (aggregate, locator)
    }

    #[test]
    fn frozen_evidence_source_list_reopens_and_reads_only_selected_range() {
        let temp = Temp::new();
        let storage = Storage::open_or_create(&temp.0).unwrap();
        let (_, locator) = captured_evidence_fixture(&storage);
        let dossier = storage.load_run_evidence_dossier("evidence-run").unwrap();
        assert_eq!(
            dossier.capture_manifest.as_ref().unwrap().content.sources[0].included_locators,
            vec![locator.clone()]
        );
        let view = storage.load_evidence("evidence-run", &locator).unwrap();
        assert_eq!(view.text.as_deref(), Some("PUBLIC_SELECTED"));
        let mut broader = locator.clone();
        broader.start_line = Some(1);
        broader.end_line = Some(3);
        assert!(storage.load_evidence("evidence-run", &broader).is_err());
        let mut mismatched = dossier.clone();
        mismatched.snapshot.input.context_manifest.sources[0]
            .allowed_locators
            .clear();
        assert!(
            validate_frozen_capture_authority(&storage.connection().unwrap(), &mismatched).is_err()
        );
        mismatched = dossier.clone();
        mismatched.capture_manifest = None;
        assert!(
            validate_frozen_capture_authority(&storage.connection().unwrap(), &mismatched).is_err()
        );
        fs::remove_file(storage.object_path(&locator.object_digest)).unwrap();
        assert!(storage.load_run_evidence_dossier("evidence-run").is_ok());
        assert!(
            storage
                .load_evidence("evidence-run", &locator)
                .unwrap()
                .evidence_unavailable
        );
        drop(storage);
        let reopened = Storage::open_or_create(&temp.0).unwrap();
        assert_eq!(
            reopened
                .load_run_evidence_dossier("evidence-run")
                .unwrap()
                .snapshot
                .run
                .revision,
            dossier.snapshot.run.revision
        );
        assert!(
            reopened
                .load_evidence("evidence-run", &locator)
                .unwrap()
                .evidence_unavailable
        );
    }

    #[test]
    fn frozen_evidence_source_list_rejects_valid_broadened_capture_after_reopen() {
        let temp = Temp::new();
        let storage = Storage::open_or_create(&temp.0).unwrap();
        let (_, locator) = captured_evidence_fixture(&storage);
        let mut capture = storage
            .load_run_evidence_dossier("evidence-run")
            .unwrap()
            .capture_manifest
            .unwrap();
        capture.content.sources[0].included_locators[0].start_line = Some(1);
        capture.content.sources[0].included_locators[0].end_line = Some(3);
        capture.digest = capture.calculate_digest().unwrap();
        capture.validate().unwrap();
        let mut connection = storage.connection().unwrap();
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        let trigger: String = transaction.query_row("SELECT sql FROM sqlite_master WHERE type='trigger' AND name='source_capture_manifests_no_update'", [], |row| row.get(0)).unwrap();
        transaction
            .execute_batch("DROP TRIGGER source_capture_manifests_no_update")
            .unwrap();
        transaction.execute("UPDATE source_capture_manifests SET manifest_digest=?1,payload_json=?2 WHERE manifest_id=?3", params![capture.digest.as_str(), serde_json::to_string(&capture).unwrap(), capture.manifest_id]).unwrap();
        transaction.execute_batch(&trigger).unwrap();
        transaction.commit().unwrap();
        drop(connection);
        assert!(storage.load_run_evidence_dossier("evidence-run").is_err());
        assert!(storage.load_evidence("evidence-run", &locator).is_err());
        drop(storage);
        let reopened = Storage::open_or_create(&temp.0).unwrap();
        assert!(reopened.load_run_evidence_dossier("evidence-run").is_err());
        assert!(reopened.load_evidence("evidence-run", &locator).is_err());
    }

    #[test]
    fn frozen_evidence_source_list_rejects_deleted_run_after_reopen() {
        let temp = Temp::new();
        let storage = Storage::open_or_create(&temp.0).unwrap();
        let (mut aggregate, locator) = captured_evidence_fixture(&storage);
        let revision = aggregate.run().revision;
        aggregate
            .request_cancel(revision, "2026-10-01T00:00:01Z".into())
            .unwrap();
        aggregate
            .confirm_cancelled(
                true,
                "No external dispatch".into(),
                "2026-10-01T00:00:02Z".into(),
            )
            .unwrap();
        let command = CommandEnvelope {
            command_id: "evidence-cancel".into(),
            idempotency_key: "evidence-cancel".into(),
            command_kind: CommandKind::CancelRun,
            target_id: "evidence-run".into(),
            expected_revision: revision,
            payload_digest: Digest::from_bytes(b"cancel"),
        };
        storage
            .commit_run(&command, &mut aggregate, "2026-10-01T00:00:02Z", None)
            .unwrap();
        storage
            .delete_run(
                "evidence-run",
                aggregate.run().revision,
                "2026-10-01T00:00:03Z",
            )
            .unwrap();
        assert!(matches!(
            storage.load_run_evidence_dossier("evidence-run"),
            Err(StorageError::RunNotFound(_))
        ));
        assert!(storage.load_evidence("evidence-run", &locator).is_err());
        drop(storage);
        let reopened = Storage::open_or_create(&temp.0).unwrap();
        assert!(matches!(
            reopened.load_run_evidence_dossier("evidence-run"),
            Err(StorageError::RunNotFound(_))
        ));
        let count: i64 = reopened
            .connection()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM source_capture_manifests", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 0);
    }

    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let p = std::env::temp_dir().join(format!("magi-storage-test-{}", Uuid::new_v4()));
            fs::create_dir(&p).unwrap();
            Self(p)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn public_replay_bytes(pretty: bool) -> Vec<u8> {
        use crate::{
            PublicBallot, PublicProposalContent, ReplayEnd, ReplayEvent, ReplayPayload,
            ReplayPhase, SharedProposal,
        };
        let content = PublicProposalContent {
            kind: magi_domain::QuestionKind::Answer,
            body: "Public answer".into(),
            conditions: vec![],
            alternatives: vec![],
            open_objections: vec![],
        };
        let digest = Digest::from_bytes(&magi_domain::canonical_json(&content).unwrap());
        let data = SharedReplay {
            format: "magi-replay".into(),
            schema_version: 1,
            title: "Public title".into(),
            question: "Public question".into(),
            cores: CoreId::ALL.to_vec(),
            assessments: vec![],
            proposal: SharedProposal {
                content,
                proposal_digest: digest.clone(),
            },
            ballots: CoreId::ALL
                .into_iter()
                .map(|core_id| PublicBallot {
                    core_id,
                    vote: magi_domain::VoteValue::Support,
                    rationale: "Public rationale".into(),
                    proposal_digest: digest.clone(),
                })
                .collect(),
            events: vec![
                ReplayEvent {
                    sequence: 1,
                    offset_ms: 0,
                    payload: ReplayPayload::PhaseEntered {
                        phase: ReplayPhase::Synthesis,
                    },
                },
                ReplayEvent {
                    sequence: 2,
                    offset_ms: 1,
                    payload: ReplayPayload::ProposalFrozen {
                        proposal_digest: digest.clone(),
                    },
                },
                ReplayEvent {
                    sequence: 3,
                    offset_ms: 2,
                    payload: ReplayPayload::PhaseEntered {
                        phase: ReplayPhase::Balloting,
                    },
                },
                ReplayEvent {
                    sequence: 4,
                    offset_ms: 3,
                    payload: ReplayPayload::BallotsRevealed {
                        proposal_digest: digest,
                    },
                },
                ReplayEvent {
                    sequence: 5,
                    offset_ms: 4,
                    payload: ReplayPayload::RunEnded {
                        status: ReplayEnd::Completed,
                    },
                },
            ],
            redactions: vec![],
            redacted: false,
            attribution: "Unverified external replay".into(),
        };
        if pretty {
            serde_json::to_vec_pretty(&data).unwrap()
        } else {
            serde_json::to_vec(&data).unwrap()
        }
    }

    #[test]
    fn external_replay_persistence_preserves_original_bytes_and_pages_after_reopen() {
        let temp = Temp::new();
        let bytes = public_replay_bytes(true);
        let compact = public_replay_bytes(false);
        assert_ne!(Digest::from_bytes(&bytes), Digest::from_bytes(&compact));
        let (first, second) = {
            let storage = Storage::open_or_create(&temp.0).unwrap();
            let first = storage.import_external_replay(&bytes, 100).unwrap();
            let second = storage.import_external_replay(&compact, 100).unwrap();
            assert_eq!(first.file_digest, Digest::from_bytes(&bytes));
            assert_eq!(second.file_digest, Digest::from_bytes(&compact));
            storage.save_external_replay(&first, &bytes).unwrap();
            assert!(storage.save_external_replay(&first, &compact).is_err());
            (first, second)
        };
        let storage = Storage::open_or_create(&temp.0).unwrap();
        let loaded = storage
            .load_external_replay(&first.external_replay_id)
            .unwrap();
        assert_eq!(
            serde_json::to_value(&loaded).unwrap(),
            serde_json::to_value(&first).unwrap()
        );
        let page = storage.list_external_replays(None, None, 1).unwrap();
        let next = storage
            .list_external_replays(
                Some(page[0].imported_at_epoch_ms),
                Some(&page[0].external_replay_id),
                1,
            )
            .unwrap();
        assert_eq!(next.len(), 1);
        assert_ne!(page[0].external_replay_id, next[0].external_replay_id);
        let mut ids = vec![
            page[0].external_replay_id.clone(),
            next[0].external_replay_id.clone(),
        ];
        ids.sort();
        let mut expected = vec![first.external_replay_id, second.external_replay_id];
        expected.sort();
        assert_eq!(ids, expected);
        assert!(storage.list_external_replays(Some(100), None, 1).is_err());
        assert!(
            storage
                .list_external_replays(None, Some("invalid"), 1)
                .is_err()
        );
        assert!(storage.list_external_replays(None, None, 101).is_err());
        assert!(storage.list_external_replays(None, None, 0).is_err());
        let connection = storage.connection().unwrap();
        for table in [
            "runs",
            "commands",
            "live_runs",
            "live_run_outbox",
            "dispatch_outbox",
            "disclosure_grants",
            "provider_profile_heads",
            "provider_profile_revisions",
            "core_model_selections",
        ] {
            assert_eq!(
                connection
                    .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r
                        .get::<_, i64>(0))
                    .unwrap(),
                0
            );
        }
    }

    #[test]
    fn external_replay_import_and_duplicate_conflicts_leave_no_partial_authority() {
        let temp = Temp::new();
        let storage = Storage::open_or_create(&temp.0).unwrap();
        let bytes = public_replay_bytes(true);
        for invalid in [
            b"{\"PRIVATE_IMPORT_CANARY\":0}".as_slice(),
            b"{\"format\":0,\"format\":1}".as_slice(),
        ] {
            let error = storage
                .import_external_replay(invalid, 100)
                .unwrap_err()
                .to_string();
            assert!(!error.contains("PRIVATE_IMPORT_CANARY"));
        }
        assert!(storage.import_external_replay(&bytes, u64::MAX).is_err());
        assert!(
            storage
                .list_external_replays(None, None, 100)
                .unwrap()
                .is_empty()
        );
        let original = storage.import_external_replay(&bytes, 100).unwrap();
        let mut forged = original.clone();
        forged.data.title = "Substituted valid public content".into();
        assert!(storage.save_external_replay(&forged, &bytes).is_err());
        let mut conflicting = original.clone();
        conflicting.imported_at_epoch_ms = 101;
        assert!(matches!(
            storage.save_external_replay(&conflicting, &bytes),
            Err(StorageError::ImmutableConflict(_))
        ));
        let mut local = original.clone();
        local.external = false;
        assert!(storage.save_external_replay(&local, &bytes).is_err());
        assert_eq!(
            storage
                .list_external_replays(None, None, 100)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            storage
                .load_external_replay(&original.external_replay_id)
                .unwrap()
                .data
                .title,
            original.data.title
        );
    }

    #[test]
    fn external_replay_reload_and_list_reject_metadata_or_payload_tampering() {
        for fault in 0..4 {
            let temp = Temp::new();
            let storage = Storage::open_or_create(&temp.0).unwrap();
            let replay = storage
                .import_external_replay(&public_replay_bytes(true), 100)
                .unwrap();
            {
                let c = storage.connection().unwrap();
                match fault {
                    0 => {
                        c.execute(
                            "UPDATE external_replays SET file_digest=?1",
                            [Digest::from_bytes(b"other-bytes").as_str()],
                        )
                        .unwrap();
                    }
                    1 => {
                        c.execute("UPDATE external_replays SET imported_at_epoch_ms=101", [])
                            .unwrap();
                    }
                    _ => {
                        let mut value = serde_json::to_value(&replay).unwrap();
                        if fault == 2 {
                            value["externalReplayId"] =
                                serde_json::json!(Uuid::new_v4().to_string());
                        } else {
                            value["data"]["PRIVATE_IMPORT_CANARY"] =
                                serde_json::json!("PRIVATE_IMPORT_CANARY");
                        }
                        c.execute(
                            "UPDATE external_replays SET payload_json=?1",
                            [value.to_string()],
                        )
                        .unwrap();
                    }
                }
            }
            drop(storage);
            let storage = Storage::open_or_create(&temp.0).unwrap();
            let error = storage
                .load_external_replay(&replay.external_replay_id)
                .unwrap_err()
                .to_string();
            assert!(!error.contains("PRIVATE_IMPORT_CANARY"));
            assert!(storage.list_external_replays(None, None, 100).is_err());
            assert_eq!(
                storage
                    .connection()
                    .unwrap()
                    .query_row("SELECT count(*) FROM external_replays", [], |r| r
                        .get::<_, i64>(0))
                    .unwrap(),
                1
            );
        }
    }

    #[test]
    fn backup_restores_verified_objects_with_new_generation() {
        let t = Temp::new();
        let s = Storage::open_or_create(t.0.join("live")).unwrap();
        let object = s.put_source_object(b"immutable original\n").unwrap();
        let manifest = s.create_backup(&t.0.join("backup")).unwrap();
        assert_eq!(manifest.objects.len(), 1);
        assert!(t.0.join("backup/COMPLETE").is_file());
        let restored = Storage::restore_backup(&t.0.join("backup"), &t.0.join("restored")).unwrap();
        assert!(restored.requires_reauthorization);
        assert!(restored.generation > s.identity().generation);
        let copy = Storage::open_or_create(t.0.join("restored")).unwrap();
        assert_eq!(
            copy.read_source_object(&object.digest).unwrap(),
            b"immutable original\n"
        );
        assert_eq!(copy.identity().store_id, s.identity().store_id);
    }
    #[test]
    fn corrupt_or_partial_backup_never_replaces_existing_root() {
        let t = Temp::new();
        let s = Storage::open_or_create(t.0.join("live")).unwrap();
        s.create_backup(&t.0.join("backup")).unwrap();
        fs::remove_file(t.0.join("backup/COMPLETE")).unwrap();
        assert!(Storage::restore_backup(&t.0.join("backup"), &t.0.join("copy")).is_err());
        assert!(!t.0.join("copy").exists());
        fs::create_dir(t.0.join("occupied")).unwrap();
        fs::write(t.0.join("occupied/keep"), b"keep").unwrap();
        assert!(s.create_backup(&t.0.join("occupied")).is_err());
        assert_eq!(fs::read(t.0.join("occupied/keep")).unwrap(), b"keep");
    }
    #[test]
    fn object_corruption_prevents_complete_marker() {
        let t = Temp::new();
        let s = Storage::open_or_create(t.0.join("live")).unwrap();
        let o = s.put_source_object(b"captured").unwrap();
        fs::write(s.object_path(&o.digest), b"tampered").unwrap();
        assert!(s.create_backup(&t.0.join("backup")).is_err());
        assert!(!t.0.join("backup/COMPLETE").exists());
    }

    #[test]
    fn backup_identity_tampering_is_rejected_before_destination_publication() {
        let t = Temp::new();
        let storage = Storage::open_or_create(t.0.join("live")).unwrap();
        storage.create_backup(&t.0.join("backup")).unwrap();
        let path = t.0.join("backup/manifest.json");
        let mut manifest: BackupManifest =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        manifest.generation += 1;
        let bytes = serde_json::to_vec(&manifest).unwrap();
        fs::write(&path, &bytes).unwrap();
        fs::write(
            t.0.join("backup/COMPLETE"),
            Digest::from_bytes(&bytes).as_str(),
        )
        .unwrap();
        assert!(Storage::restore_backup(&t.0.join("backup"), &t.0.join("restored")).is_err());
        assert!(!t.0.join("restored").exists());
        assert_eq!(storage.identity().store_id, manifest.store_id);
    }

    #[test]
    fn concurrent_capture_and_backup_keep_each_database_object_inventory_coherent() {
        let t = Temp::new();
        let storage = Storage::open_or_create(t.0.join("live")).unwrap();
        std::thread::scope(|scope| {
            let writer = scope.spawn(|| {
                for index in 0..20 {
                    storage
                        .put_source_object(format!("captured-{index}").as_bytes())
                        .unwrap();
                }
            });
            for index in 0..3 {
                let backup = t.0.join(format!("backup-{index}"));
                let manifest = storage.create_backup(&backup).unwrap();
                let destination = t.0.join(format!("copy-{index}"));
                Storage::restore_backup(&backup, &destination).unwrap();
                let restored = Storage::open_or_create(&destination).unwrap();
                for object in manifest.objects {
                    assert_eq!(
                        Digest::from_bytes(&restored.read_source_object(&object.digest).unwrap()),
                        object.digest
                    );
                }
            }
            writer.join().unwrap();
        });
    }
    #[cfg(unix)]
    #[test]
    fn backup_manifest_symlink_is_rejected_without_publishing_restore() {
        let t = Temp::new();
        let storage = Storage::open_or_create(t.0.join("live")).unwrap();
        storage.create_backup(&t.0.join("backup")).unwrap();
        let manifest = t.0.join("backup/manifest.json");
        let renamed = t.0.join("saved-manifest");
        fs::rename(&manifest, &renamed).unwrap();
        std::os::unix::fs::symlink(&renamed, &manifest).unwrap();
        assert!(Storage::restore_backup(&t.0.join("backup"), &t.0.join("copy")).is_err());
        assert!(!t.0.join("copy").exists());
    }
}
