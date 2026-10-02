use crate::commands::DesktopState;
use magi_context::{EvidenceLocator, FreshnessStatus, ManifestSourceState, RepresentationKind};
use magi_domain::{CoreId, EventPayload, Outcome, RunStage, RunStatus, status_name};
use magi_storage::{
    BackupManifest, DeletionReceipt, EvidenceDeletionPreview, EvidenceView, ExternalReplay,
    ExternalReplaySummary, MAX_REPLAY_BYTES, RestoreReceipt, RunEventCursor, RunHistoryCursor,
    RunHistoryRequest, Storage, StorageError, StoredEvent,
};
use serde::Serialize;
use std::{io::Read, path::Path, path::PathBuf, sync::mpsc, time::SystemTime};
use tauri::{AppHandle, Emitter, State, WebviewWindow};
use tauri_plugin_dialog::DialogExt;

fn require_main(label: &str) -> Result<(), String> {
    if label == "main" {
        Ok(())
    } else {
        Err("record_window_denied".into())
    }
}

fn record_error(error: StorageError) -> String {
    match error {
        StorageError::RevisionConflict { .. } => "record_revision_conflict",
        StorageError::CursorExpired => "record_cursor_expired",
        StorageError::DeletionBlocked => "record_deletion_blocked",
        StorageError::RunNotFound(_) => "record_not_found",
        _ => "record_operation_failed",
    }
    .into()
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordSummaryView {
    run_id: String,
    conversation_id: String,
    question: String,
    status: &'static str,
    revision: u64,
    created_at: String,
    updated_at: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordHistoryPage {
    items: Vec<RecordSummaryView>,
    next_cursor: Option<RunHistoryCursor>,
}

fn history_page(
    storage: &Storage,
    request: &RunHistoryRequest,
) -> Result<RecordHistoryPage, String> {
    let page = storage.list_runs(request).map_err(record_error)?;
    Ok(RecordHistoryPage {
        items: page
            .items
            .into_iter()
            .map(|run| RecordSummaryView {
                run_id: run.run_id,
                conversation_id: run.conversation_id,
                question: run.question,
                status: status_name(&run.status),
                revision: run.revision,
                created_at: run.created_at,
                updated_at: run.updated_at,
            })
            .collect(),
        next_cursor: page.next_cursor,
    })
}

#[tauri::command]
pub fn list_records(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    request: RunHistoryRequest,
) -> Result<RecordHistoryPage, String> {
    require_main(window.label())?;
    history_page(state.storage()?.as_ref(), &request)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordReplayEventView {
    sequence: u64,
    created_at: String,
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    phase: Option<RunStage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    assessment_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    core_id: Option<CoreId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    votes: Option<Vec<crate::run_projection::VoteView>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    outcome: Option<Outcome>,
}

fn replay_event(event: &StoredEvent, reveal: bool) -> RecordReplayEventView {
    let mut view = RecordReplayEventView {
        sequence: event.position.sequence,
        created_at: event.event.created_at.clone(),
        kind: event.event.event_type.as_str(),
        phase: None,
        assessment_id: None,
        core_id: None,
        votes: None,
        outcome: None,
    };
    match &event.event.payload {
        EventPayload::RunStarted { stage, .. } => view.phase = Some(*stage),
        EventPayload::PhaseAdvanced { to, .. } => view.phase = Some(*to),
        EventPayload::AssessmentAccepted {
            assessment_id,
            core_id,
            ..
        } => {
            view.assessment_id = Some(assessment_id.clone());
            view.core_id = Some(*core_id);
        }
        EventPayload::BallotsRevealed { ballots, tally } if reveal => {
            view.votes = Some(
                ballots
                    .iter()
                    .map(|ballot| crate::run_projection::VoteView {
                        core_id: ballot.core_id,
                        choice: ballot.vote,
                        rationale: ballot.rationale.clone(),
                    })
                    .collect(),
            );
            view.outcome = Some(tally.outcome);
        }
        _ => {}
    }
    view
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordReplayPage {
    events: Vec<RecordReplayEventView>,
    next_cursor: RunEventCursor,
    complete: bool,
}

fn replay_page(
    storage: &Storage,
    run_id: &str,
    cursor: Option<RunEventCursor>,
    limit: u32,
) -> Result<RecordReplayPage, String> {
    if !(1..=200).contains(&limit) {
        return Err("record_page_invalid".into());
    }
    let dossier = storage.load_run_dossier(run_id).map_err(record_error)?;
    let cursor = cursor.unwrap_or_else(|| dossier.replay_cursor.clone());
    if cursor.run_id != run_id
        || cursor.high_water_sequence > dossier.replay_cursor.high_water_sequence
    {
        return Err("record_cursor_invalid".into());
    }
    let reveal = matches!(dossier.snapshot.run.status, RunStatus::Completed { .. })
        && dossier.snapshot.ballots_revealed.is_some();
    let page = storage
        .events_after_for_run(&cursor, limit as usize)
        .map_err(record_error)?;
    Ok(RecordReplayPage {
        events: page
            .events
            .iter()
            .map(|event| replay_event(event, reveal))
            .collect(),
        next_cursor: page.next_cursor,
        complete: page.complete,
    })
}

#[tauri::command]
pub fn load_record_replay(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    run_id: String,
    cursor: Option<RunEventCursor>,
    limit: u32,
) -> Result<RecordReplayPage, String> {
    require_main(window.label())?;
    replay_page(state.storage()?.as_ref(), &run_id, cursor, limit)
}

#[tauri::command]
pub fn list_record_evidence(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    run_id: String,
) -> Result<RecordEvidenceList, String> {
    require_main(window.label())?;
    evidence_list(state.storage()?.as_ref(), &run_id)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordEvidenceList {
    run_id: String,
    revision: u64,
    sources: Vec<RecordEvidenceSource>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordEvidenceSource {
    source_id: String,
    display_name: String,
    representation: RepresentationKind,
    byte_length: u64,
    captured_at_epoch_ms: Option<u64>,
    locators: Vec<EvidenceLocator>,
    freshness: FreshnessStatus,
    availability: &'static str,
}

fn evidence_list(storage: &Storage, run_id: &str) -> Result<RecordEvidenceList, String> {
    let dossier = storage
        .load_run_evidence_dossier(run_id)
        .map_err(record_error)?;
    let sources = dossier
        .capture_manifest
        .as_ref()
        .map(|manifest| {
            manifest
                .content
                .sources
                .iter()
                .filter(|source| source.state == ManifestSourceState::Captured)
                .map(|source| {
                    Ok(RecordEvidenceSource {
                        source_id: source.source_id.clone(),
                        display_name: source.display_name.clone(),
                        representation: source
                            .representation_kind
                            .ok_or("record_operation_failed")?,
                        byte_length: source.byte_length.ok_or("record_operation_failed")?,
                        captured_at_epoch_ms: source.captured_at_epoch_ms,
                        locators: source.included_locators.clone(),
                        freshness: dossier
                            .source_freshness
                            .iter()
                            .find(|record| record.source_id == source.source_id)
                            .map(|record| record.observation.status)
                            .unwrap_or(FreshnessStatus::Unchecked),
                        availability: "not_checked",
                    })
                })
                .collect::<Result<Vec<_>, String>>()
        })
        .transpose()?
        .unwrap_or_default();
    Ok(RecordEvidenceList {
        run_id: run_id.into(),
        revision: dossier.snapshot.run.revision,
        sources,
    })
}

#[tauri::command]
pub fn load_evidence(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    run_id: String,
    locator: EvidenceLocator,
) -> Result<EvidenceView, String> {
    require_main(window.label())?;
    state
        .storage()?
        .load_evidence(&run_id, &locator)
        .map_err(record_error)
}

#[tauri::command]
pub fn load_context_evidence(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    draft_id: String,
    context_revision: u64,
    locator: EvidenceLocator,
) -> Result<EvidenceView, String> {
    require_main(window.label())?;
    state
        .storage()?
        .load_context_evidence(&draft_id, context_revision, &locator)
        .map_err(record_error)
}

#[tauri::command]
pub fn preview_evidence_deletion(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    run_id: String,
    source_id: String,
) -> Result<EvidenceDeletionPreview, String> {
    require_main(window.label())?;
    state
        .storage()?
        .preview_evidence_deletion(&run_id, &source_id)
        .map_err(record_error)
}

#[tauri::command]
pub async fn delete_record_evidence(
    window: WebviewWindow,
    app: AppHandle,
    state: State<'_, DesktopState>,
    run_id: String,
    source_id: String,
    expected_revision: u64,
    expected_affected_run_ids: Vec<String>,
) -> Result<EvidenceDeletionPreview, String> {
    require_main(window.label())?;
    let result = state
        .storage()?
        .delete_evidence(
            &run_id,
            &source_id,
            expected_revision,
            &expected_affected_run_ids,
            &now().to_string(),
        )
        .map_err(record_error)?;
    if let Ok(snapshot) =
        crate::commands::verified_console_snapshot(window, app.clone(), state).await
    {
        let _ = app.emit_to("main", "console_snapshot_changed", &snapshot);
    }
    Ok(result)
}

#[tauri::command]
pub async fn delete_record(
    window: WebviewWindow,
    app: AppHandle,
    state: State<'_, DesktopState>,
    run_id: String,
    expected_revision: u64,
) -> Result<DeletionReceipt, String> {
    require_main(window.label())?;
    let result = state
        .storage()?
        .delete_run(&run_id, expected_revision, &now().to_string())
        .map_err(record_error)?;
    if let Ok(snapshot) =
        crate::commands::verified_console_snapshot(window, app.clone(), state).await
    {
        let _ = app.emit_to("main", "console_snapshot_changed", &snapshot);
    }
    Ok(result)
}

fn read_selected_replay(path: &Path) -> Result<Vec<u8>, String> {
    let file = crate::commands::open_selected_file(path).map_err(|_| "replay_file_denied")?;
    let before = file.metadata().map_err(|_| "replay_file_unavailable")?;
    if before.len() > MAX_REPLAY_BYTES as u64 {
        return Err("replay_file_too_large".into());
    }
    let mut bytes = Vec::new();
    (&file)
        .take(MAX_REPLAY_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "replay_file_unavailable")?;
    let after = file.metadata().map_err(|_| "replay_file_unavailable")?;
    #[cfg(unix)]
    {
        use cap_std::fs::MetadataExt;
        if (
            before.dev(),
            before.ino(),
            before.ctime(),
            before.ctime_nsec(),
        ) != (after.dev(), after.ino(), after.ctime(), after.ctime_nsec())
        {
            return Err("replay_file_changed".into());
        }
    }
    if bytes.len() > MAX_REPLAY_BYTES
        || bytes.len() as u64 != before.len()
        || before.len() != after.len()
        || before.modified().ok() != after.modified().ok()
    {
        return Err("replay_file_changed".into());
    }
    Ok(bytes)
}

async fn choose_file(app: &AppHandle, folder: bool) -> Result<Option<PathBuf>, String> {
    let (sender, receiver) = mpsc::sync_channel(1);
    let dialog = app.dialog().file();
    if folder {
        dialog.pick_folder(move |path| {
            let _ = sender.send(path);
        });
    } else {
        dialog
            .add_filter("MAGI replay", &["json"])
            .pick_file(move |path| {
                let _ = sender.send(path);
            });
    }
    let path = tauri::async_runtime::spawn_blocking(move || receiver.recv())
        .await
        .map_err(|_| "record_dialog_failed")?
        .map_err(|_| "record_dialog_failed")?;
    path.map(|path| path.into_path().map_err(|_| "record_file_denied".into()))
        .transpose()
}

#[tauri::command]
pub async fn import_shared_replay(
    window: WebviewWindow,
    app: AppHandle,
    state: State<'_, DesktopState>,
) -> Result<Option<ExternalReplay>, String> {
    require_main(window.label())?;
    let storage = state.storage()?;
    let Some(path) = choose_file(&app, false).await? else {
        return Ok(None);
    };
    tauri::async_runtime::spawn_blocking(move || {
        import_selected_replay(&storage, &path, now()).map(Some)
    })
    .await
    .map_err(|_| "replay_import_failed".to_string())?
}

fn import_selected_replay(
    storage: &Storage,
    path: &Path,
    at: u64,
) -> Result<ExternalReplay, String> {
    let bytes = read_selected_replay(path)?;
    storage
        .import_external_replay(&bytes, at)
        .map_err(record_error)
}

#[tauri::command]
pub fn load_external_replay(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    external_replay_id: String,
) -> Result<ExternalReplay, String> {
    require_main(window.label())?;
    state
        .storage()?
        .load_external_replay(&external_replay_id)
        .map_err(record_error)
}

#[tauri::command]
pub fn list_external_replays(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    before_imported_at_epoch_ms: Option<u64>,
    before_external_replay_id: Option<String>,
    limit: u32,
) -> Result<Vec<ExternalReplaySummary>, String> {
    require_main(window.label())?;
    state
        .storage()?
        .list_external_replays(
            before_imported_at_epoch_ms,
            before_external_replay_id.as_deref(),
            limit,
        )
        .map_err(record_error)
}

#[tauri::command]
pub async fn create_local_backup(
    window: WebviewWindow,
    app: AppHandle,
    state: State<'_, DesktopState>,
) -> Result<Option<BackupManifest>, String> {
    require_main(window.label())?;
    let storage = state.storage()?;
    let Some(path) = choose_file(&app, true).await? else {
        return Ok(None);
    };
    tauri::async_runtime::spawn_blocking(move || {
        storage.create_backup(&path).map(Some).map_err(record_error)
    })
    .await
    .map_err(|_| "record_backup_failed".to_string())?
}

#[tauri::command]
pub async fn restore_local_backup(
    window: WebviewWindow,
    app: AppHandle,
) -> Result<Option<RestoreReceipt>, String> {
    require_main(window.label())?;
    let Some(backup) = choose_file(&app, true).await? else {
        return Ok(None);
    };
    let Some(destination) = choose_file(&app, true).await? else {
        return Ok(None);
    };
    tauri::async_runtime::spawn_blocking(move || {
        Storage::restore_backup(&backup, &destination)
            .map(Some)
            .map_err(record_error)
    })
    .await
    .map_err(|_| "record_restore_failed".to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    use magi_domain::{CommandEnvelope, CommandKind};
    use magi_storage::RunHistoryFilter;

    #[test]
    fn record_evidence_metadata_exposes_only_safe_selected_locator_authority() {
        let source = RecordEvidenceSource {
            source_id: "memo".into(),
            display_name: "memo.txt".into(),
            representation: RepresentationKind::Utf8Text,
            byte_length: 42,
            captured_at_epoch_ms: None,
            locators: vec![EvidenceLocator {
                source_id: "memo".into(),
                object_digest: magi_domain::Digest::from_bytes(b"captured"),
                start_line: Some(2),
                end_line: Some(2),
                total_lines: Some(3),
                page: None,
                width: None,
                height: None,
            }],
            freshness: FreshnessStatus::Unchecked,
            availability: "not_checked",
        };
        let json = serde_json::to_value(RecordEvidenceList {
            run_id: "saved-run".into(),
            revision: 7,
            sources: vec![source],
        })
        .unwrap();
        let source = &json["sources"][0];
        assert_eq!(source["availability"], "not_checked");
        assert!(source["capturedAtEpochMs"].is_null());
        assert_eq!(source["locators"][0]["start_line"], 2);
        for private in [
            "canonicalPath",
            "recipients",
            "credentialHome",
            "secretPatternFindings",
            "derivedDigest",
        ] {
            assert!(source.get(private).is_none());
        }
        assert_eq!(source.as_object().unwrap().len(), 8);
        assert!(require_main("companion").is_err());
    }

    #[test]
    fn records_history_and_replay_use_persisted_authority_and_hide_sealed_votes() {
        let root = std::env::temp_dir().join(format!("magi-records-{}", uuid::Uuid::new_v4()));
        let storage = Storage::open_or_create(&root).unwrap();
        let mut aggregate = crate::run_projection::tests::aggregate();
        let command = CommandEnvelope {
            command_id: "record-create".into(),
            idempotency_key: "record-create".into(),
            command_kind: CommandKind::CreateRun,
            target_id: "run-fixture".into(),
            expected_revision: 0,
            payload_digest: aggregate.input().input_digest.clone(),
        };
        storage
            .commit_run(&command, &mut aggregate, "2026-10-01T00:00:00Z", None)
            .unwrap();
        let evidence = evidence_list(&storage, "run-fixture").unwrap();
        assert_eq!(evidence.run_id, "run-fixture");
        assert_eq!(evidence.revision, aggregate.run().revision);
        assert!(evidence.sources.is_empty());
        let request = RunHistoryRequest {
            filter: RunHistoryFilter::default(),
            cursor: None,
            page_size: 1,
        };
        let history = history_page(&storage, &request).unwrap();
        assert_eq!(history.items.len(), 1);
        assert_eq!(history.items[0].run_id, "run-fixture");
        assert_eq!(history.items[0].status, "preparing");
        let wire = serde_json::to_string(&history).unwrap();
        assert!(!wire.contains("inputDigest"));
        assert!(!wire.contains("catalog_binding"));
        assert!(replay_page(&storage, "run-fixture", None, 0).is_err());
        assert!(replay_page(&storage, "missing", None, 1).is_err());
        let page = replay_page(&storage, "run-fixture", None, 1).unwrap();
        let mut forged = page.next_cursor.clone();
        forged.run_id = "different-run".into();
        assert!(replay_page(&storage, "run-fixture", Some(forged), 1).is_err());
        let mut forged = page.next_cursor;
        forged.high_water_sequence = u64::MAX;
        assert!(replay_page(&storage, "run-fixture", Some(forged), 1).is_err());
        crate::run_projection::tests::advance_ballots(&mut aggregate, 3);
        let command = CommandEnvelope {
            command_id: "record-complete".into(),
            idempotency_key: "record-complete".into(),
            command_kind: CommandKind::AddBallot,
            target_id: "run-fixture".into(),
            expected_revision: 0,
            payload_digest: aggregate.input().input_digest.clone(),
        };
        storage
            .commit_run(&command, &mut aggregate, "2026-10-01T00:00:01Z", None)
            .unwrap();
        let dossier = storage.load_run_dossier("run-fixture").unwrap();
        let raw = storage
            .events_after_for_run(&dossier.replay_cursor, 200)
            .unwrap();
        let revealed = raw
            .events
            .iter()
            .find(|event| matches!(event.event.payload, EventPayload::BallotsRevealed { .. }))
            .unwrap();
        let hidden = serde_json::to_string(&replay_event(revealed, false)).unwrap();
        assert!(!hidden.contains("rationale"));
        assert!(!hidden.contains("votes"));
        assert!(!hidden.contains("outcome"));
        let page = replay_page(&storage, "run-fixture", None, 200).unwrap();
        assert_eq!(
            page.events
                .iter()
                .find_map(|event| event.votes.as_ref())
                .unwrap()
                .len(),
            3
        );
        let wire = serde_json::to_string(&page).unwrap();
        assert!(!wire.contains("attempt_generation"));
        assert!(!wire.contains("input_digest"));
        drop(storage);
        let reopened = Storage::open_or_create(&root).unwrap();
        let reopened_page = replay_page(&reopened, "run-fixture", None, 200).unwrap();
        assert_eq!(
            serde_json::to_value(&reopened_page.events).unwrap(),
            serde_json::to_value(&page.events).unwrap()
        );
        assert!(reopened_page.complete);
        assert!(replay_page(&reopened, "run-fixture", Some(page.next_cursor), 200).is_err());
        drop(reopened);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn records_import_reads_only_bounded_selected_regular_capabilities() {
        let root = std::env::temp_dir().join(format!("magi-replay-file-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let file = root.join("replay.json");
        std::fs::write(&file, b"bounded fixture").unwrap();
        assert_eq!(read_selected_replay(&file).unwrap(), b"bounded fixture");
        assert!(read_selected_replay(&root).is_err());
        let oversized = std::fs::File::create(&file).unwrap();
        oversized.set_len(MAX_REPLAY_BYTES as u64 + 1).unwrap();
        assert_eq!(
            read_selected_replay(&file).unwrap_err(),
            "replay_file_too_large"
        );
        #[cfg(unix)]
        {
            let link = root.join("link.json");
            std::os::unix::fs::symlink(&file, &link).unwrap();
            assert_eq!(
                read_selected_replay(&link).unwrap_err(),
                "replay_file_denied"
            );
        }
        assert!(require_main("companion").is_err());
        assert!(require_main("main").is_ok());
        let error = record_error(StorageError::Corrupt("private-canary-path".into()));
        assert!(!error.contains("private-canary"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cancelled_record_replay_never_discloses_persisted_sealed_ballots() {
        let root =
            std::env::temp_dir().join(format!("magi-cancelled-replay-{}", uuid::Uuid::new_v4()));
        let storage = Storage::open_or_create(&root).unwrap();
        let mut aggregate = crate::run_projection::tests::aggregate();
        let initial = CommandEnvelope {
            command_id: "cancelled-record-create".into(),
            idempotency_key: "cancelled-record-create".into(),
            command_kind: CommandKind::CreateRun,
            target_id: "run-fixture".into(),
            expected_revision: 0,
            payload_digest: aggregate.input().input_digest.clone(),
        };
        storage
            .commit_run(&initial, &mut aggregate, "2026-10-01T00:00:00Z", None)
            .unwrap();
        crate::run_projection::tests::advance_ballots(&mut aggregate, 2);
        let revision = aggregate.run().revision;
        aggregate
            .request_cancel(revision, "2026-10-01T00:00:02Z".into())
            .unwrap();
        aggregate
            .confirm_cancelled(
                true,
                "Local fixture stopped".into(),
                "2026-10-01T00:00:03Z".into(),
            )
            .unwrap();
        let command = CommandEnvelope {
            command_id: "cancelled-record".into(),
            idempotency_key: "cancelled-record".into(),
            command_kind: CommandKind::AddBallot,
            target_id: "run-fixture".into(),
            expected_revision: 0,
            payload_digest: aggregate.input().input_digest.clone(),
        };
        storage
            .commit_run(&command, &mut aggregate, "2026-10-01T00:00:03Z", None)
            .unwrap();
        let page = replay_page(&storage, "run-fixture", None, 200).unwrap();
        assert!(
            page.events
                .iter()
                .any(|event| event.kind == "ballot_sealed")
        );
        assert!(
            page.events
                .iter()
                .all(|event| event.votes.is_none() && event.outcome.is_none())
        );
        assert!(!serde_json::to_string(&page).unwrap().contains("rationale"));
        drop(storage);
        let reopened = Storage::open_or_create(&root).unwrap();
        assert!(
            replay_page(&reopened, "run-fixture", None, 200)
                .unwrap()
                .events
                .iter()
                .all(|event| event.votes.is_none())
        );
        drop(reopened);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn selected_replay_import_persists_exact_original_digest_and_stays_inert() {
        use magi_domain::{Digest, QuestionKind, VoteValue, canonical_json};
        use magi_storage::{
            PublicBallot, PublicProposalContent, ReplayEnd, ReplayEvent, ReplayPayload,
            ReplayPhase, SharedProposal, SharedReplay,
        };
        let root =
            std::env::temp_dir().join(format!("magi-selected-replay-{}", uuid::Uuid::new_v4()));
        let storage = Storage::open_or_create(root.join("store")).unwrap();
        let proposal = PublicProposalContent {
            kind: QuestionKind::Answer,
            body: "Public fixture".into(),
            conditions: vec![],
            alternatives: vec![],
            open_objections: vec![],
        };
        let digest = Digest::from_bytes(&canonical_json(&proposal).unwrap());
        let payloads = [
            ReplayPayload::PhaseEntered {
                phase: ReplayPhase::Synthesis,
            },
            ReplayPayload::ProposalFrozen {
                proposal_digest: digest.clone(),
            },
            ReplayPayload::PhaseEntered {
                phase: ReplayPhase::Balloting,
            },
            ReplayPayload::BallotsRevealed {
                proposal_digest: digest.clone(),
            },
            ReplayPayload::RunEnded {
                status: ReplayEnd::Completed,
            },
        ];
        let data = SharedReplay {
            format: "magi-replay".into(),
            schema_version: 1,
            title: "Public fixture".into(),
            question: "Public question".into(),
            cores: CoreId::ALL.to_vec(),
            assessments: vec![],
            proposal: SharedProposal {
                content: proposal,
                proposal_digest: digest.clone(),
            },
            ballots: CoreId::ALL
                .map(|core_id| PublicBallot {
                    core_id,
                    vote: VoteValue::Support,
                    rationale: "Public evidence".into(),
                    proposal_digest: digest.clone(),
                })
                .to_vec(),
            events: payloads
                .into_iter()
                .enumerate()
                .map(|(index, payload)| ReplayEvent {
                    sequence: index as u64 + 1,
                    offset_ms: index as u64,
                    payload,
                })
                .collect(),
            redactions: vec![],
            redacted: false,
            attribution: "Unverified public fixture".into(),
        };
        let bytes = serde_json::to_vec_pretty(&data).unwrap();
        let path = root.join("replay.json");
        std::fs::write(&path, &bytes).unwrap();
        let imported = import_selected_replay(&storage, &path, 100).unwrap();
        assert_eq!(imported.file_digest, Digest::from_bytes(&bytes));
        assert!(imported.external);
        assert!(
            history_page(
                &storage,
                &RunHistoryRequest {
                    filter: RunHistoryFilter::default(),
                    cursor: None,
                    page_size: 10
                }
            )
            .unwrap()
            .items
            .is_empty()
        );
        std::fs::write(&path, br#"{"private-path-canary":true}"#).unwrap();
        let error = import_selected_replay(&storage, &path, 101).unwrap_err();
        assert!(!error.contains("private-path-canary"));
        assert_eq!(
            storage.list_external_replays(None, None, 10).unwrap().len(),
            1
        );
        drop(storage);
        let reopened = Storage::open_or_create(root.join("store")).unwrap();
        assert_eq!(
            serde_json::to_value(
                reopened
                    .load_external_replay(&imported.external_replay_id)
                    .unwrap()
            )
            .unwrap(),
            serde_json::to_value(imported).unwrap()
        );
        drop(reopened);
        std::fs::remove_dir_all(root).unwrap();
    }
}
