use crate::{commands, profiles};
#[path = "../native/connection-ui-fixture.rs"]
mod connection_ui_fixture;

use std::os::unix::fs::PermissionsExt;
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU8, Ordering},
    },
    time::{Duration, Instant},
};
use tauri::Manager;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProbePurpose {
    Deliberation,
    SavedProfileDeliberation,
    CatalogDiagnostic,
    RecoveryCancel,
    PdfUi,
    ConnectionsUi,
}

impl ProbePurpose {
    fn parse(value: Option<&str>) -> Result<Self, &'static str> {
        match value {
            None | Some("deliberation") => Ok(Self::Deliberation),
            Some("saved-profile-deliberation") => Ok(Self::SavedProfileDeliberation),
            Some("catalog-diagnostic") => Ok(Self::CatalogDiagnostic),
            Some("pdf-ui") => Ok(Self::PdfUi),
            Some("connections-ui") => Ok(Self::ConnectionsUi),
            Some("recovery-cancel") => Ok(Self::RecoveryCancel),
            _ => Err("invalid native probe purpose"),
        }
    }

    fn profile_count(self) -> usize {
        match self {
            Self::Deliberation | Self::SavedProfileDeliberation => 3,
            Self::CatalogDiagnostic => 1,
            Self::PdfUi | Self::RecoveryCancel | Self::ConnectionsUi => 0,
        }
    }

    fn permits_admission(self) -> bool {
        matches!(self, Self::Deliberation | Self::SavedProfileDeliberation)
    }
}

#[derive(Clone, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SavedProfileBinding {
    core_id: magi_domain::CoreId,
    provider_profile_id: String,
    profile_revision: u64,
    model_profile_revision: u64,
    model_selection_revision: u64,
    model_id: String,
    mode_id: Option<String>,
    core_selection_revision: Option<u64>,
}
#[derive(Clone, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SavedProfileInput {
    schema_version: u16,
    prepared_store_digest: String,
    bindings: [SavedProfileBinding; 3],
}
impl SavedProfileInput {
    fn validate(&self) -> Result<(), &'static str> {
        let token = |value: &str, limit: usize| {
            !value.is_empty() && value.len() <= limit && !value.chars().any(char::is_control)
        };
        let cores = self
            .bindings
            .iter()
            .map(|binding| binding.core_id)
            .collect::<std::collections::BTreeSet<_>>();
        let profiles = self
            .bindings
            .iter()
            .map(|binding| binding.provider_profile_id.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        if self.schema_version != 1
            || self.prepared_store_digest.len() != 64
            || !self
                .prepared_store_digest
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
            || cores.len() != 3
            || profiles.len() != 3
            || self.bindings.iter().any(|binding| {
                !token(&binding.provider_profile_id, 128)
                    || !token(&binding.model_id, 256)
                    || binding
                        .mode_id
                        .as_ref()
                        .is_some_and(|mode| !token(mode, 128))
                    || binding.model_profile_revision > binding.profile_revision
            })
        {
            return Err("saved profile input invalid");
        }
        Ok(())
    }
    fn binding(&self, core: magi_domain::CoreId) -> Result<&SavedProfileBinding, &'static str> {
        self.bindings
            .iter()
            .find(|binding| binding.core_id == core)
            .ok_or("saved core binding missing")
    }
}
fn read_saved_profile_input(path: &std::path::Path) -> Result<SavedProfileInput, &'static str> {
    use std::io::Read;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| "saved profile input unavailable")?;
    let before = file
        .metadata()
        .map_err(|_| "saved profile input metadata")?;
    if !before.is_file()
        || before.nlink() != 1
        || before.uid() != unsafe { libc::geteuid() }
        || before.mode() & 0o777 != 0o600
        || before.len() > 16384
    {
        return Err("saved profile input custody invalid");
    }
    let mut bytes = Vec::new();
    file.take(16385)
        .read_to_end(&mut bytes)
        .map_err(|_| "saved profile input read")?;
    let after = std::fs::symlink_metadata(path).map_err(|_| "saved profile input replaced")?;
    if (
        before.dev(),
        before.ino(),
        before.mode(),
        before.nlink(),
        before.uid(),
        before.len(),
        before.mtime(),
        before.mtime_nsec(),
        before.ctime(),
        before.ctime_nsec(),
    ) != (
        after.dev(),
        after.ino(),
        after.mode(),
        after.nlink(),
        after.uid(),
        after.len(),
        after.mtime(),
        after.mtime_nsec(),
        after.ctime(),
        after.ctime_nsec(),
    ) {
        return Err("saved profile input changed");
    }
    let input: SavedProfileInput =
        serde_json::from_slice(&bytes).map_err(|_| "saved profile input shape")?;
    input.validate()?;
    Ok(input)
}
fn validate_saved_profile_records(
    storage: &magi_storage::Storage,
    input: &SavedProfileInput,
) -> Result<(), &'static str> {
    input.validate()?;
    for binding in &input.bindings {
        let profile = storage
            .load_provider_profile(&binding.provider_profile_id)
            .map_err(|_| "saved profile unavailable")?
            .ok_or("saved profile missing")?;
        let model = storage
            .load_historical_model_selection_metadata(
                &binding.provider_profile_id,
                binding.model_profile_revision,
                binding.model_selection_revision,
            )
            .map_err(|_| "saved model unavailable")?
            .ok_or("saved model missing")?;
        if profile.revision != binding.profile_revision
            || profile.provider_id != "codex-acp"
            || model.selection_revision != binding.model_selection_revision
            || model.binding.provider_profile_id != binding.provider_profile_id
            || model.binding.profile_revision != binding.model_profile_revision
            || model.binding.model_id != binding.model_id
            || model.binding.mode_id != binding.mode_id
        {
            return Err("saved profile reference drift");
        }
        let core = storage
            .load_core_model_selection_intent(binding.core_id)
            .map_err(|_| "saved core unavailable")?;
        match (core, binding.core_selection_revision) {
            (None, None) => {}
            (Some(core), Some(revision))
                if core.selection_revision == revision
                    && core.provider_profile_id == binding.provider_profile_id
                    && core.profile_revision == binding.model_profile_revision
                    && core.model_selection_revision == binding.model_selection_revision => {}
            _ => return Err("saved core reference drift"),
        }
    }
    Ok(())
}

fn derive_saved_profile_bindings(
    storage: &magi_storage::Storage,
    input: &SavedProfileInput,
) -> Result<[SavedProfileBinding; 3], &'static str> {
    validate_saved_profile_records(storage, input)?;
    let bindings = input
        .bindings
        .iter()
        .map(|reference| {
            let profile = storage
                .load_provider_profile(&reference.provider_profile_id)
                .map_err(|_| "saved profile unavailable")?
                .ok_or("saved profile missing")?;
            let model = storage
                .load_historical_model_selection_metadata(
                    &reference.provider_profile_id,
                    reference.model_profile_revision,
                    reference.model_selection_revision,
                )
                .map_err(|_| "saved model unavailable")?
                .ok_or("saved model missing")?;
            let core = storage
                .load_core_model_selection_intent(reference.core_id)
                .map_err(|_| "saved core unavailable")?;
            let core_selection_revision = match core {
                None if reference.core_selection_revision.is_none() => None,
                Some(core)
                    if Some(core.selection_revision) == reference.core_selection_revision
                        && core.provider_profile_id == reference.provider_profile_id
                        && core.profile_revision == model.binding.profile_revision
                        && core.model_selection_revision == model.selection_revision =>
                {
                    Some(core.selection_revision)
                }
                _ => return Err("saved core reference drift"),
            };
            Ok(SavedProfileBinding {
                core_id: reference.core_id,
                provider_profile_id: profile.provider_profile_id,
                profile_revision: profile.revision,
                model_profile_revision: model.binding.profile_revision,
                model_selection_revision: model.selection_revision,
                model_id: model.binding.model_id,
                mode_id: model.binding.mode_id,
                core_selection_revision,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    bindings.try_into().map_err(|_| "saved core binding count")
}

fn saved_store_digest(root: &Path) -> Result<String, &'static str> {
    use std::io::Read;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let database = root.join("state/magi.sqlite");
    if root.join("state/magi.sqlite-wal").exists() || root.join("state/magi.sqlite-shm").exists() {
        return Err("active saved store has open SQLite sidecars");
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&database)
        .map_err(|_| "active saved store unavailable")?;
    let before = file.metadata().map_err(|_| "active saved store metadata")?;
    if !before.is_file()
        || before.nlink() != 1
        || before.uid() != unsafe { libc::geteuid() }
        || before.len() > 20 * 1024 * 1024
    {
        return Err("active saved store custody invalid");
    }
    let mut bytes = Vec::new();
    file.take(20 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "active saved store read")?;
    if bytes.len() > 20 * 1024 * 1024 {
        return Err("active saved store exceeds bound");
    }
    let after = std::fs::symlink_metadata(&database).map_err(|_| "active saved store replaced")?;
    if before.dev() != after.dev()
        || before.ino() != after.ino()
        || before.mode() != after.mode()
        || before.nlink() != after.nlink()
        || before.len() != after.len()
        || before.mtime() != after.mtime()
        || before.mtime_nsec() != after.mtime_nsec()
        || before.ctime() != after.ctime()
        || before.ctime_nsec() != after.ctime_nsec()
    {
        return Err("active saved store changed during digest");
    }
    Ok(magi_domain::Digest::from_bytes(&bytes).as_str().to_owned())
}

fn write_saved_profile_input(path: &Path, input: &SavedProfileInput) -> Result<(), &'static str> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    input.validate()?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .custom_flags(libc::O_NOFOLLOW)
        .mode(0o600)
        .open(path)
        .map_err(|_| "saved binding output unavailable")?;
    file.write_all(&serde_json::to_vec_pretty(input).map_err(|_| "saved binding output encoding")?)
        .map_err(|_| "saved binding output write")?;
    file.sync_all()
        .map_err(|_| "saved binding output synchronization")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProbeFrontendMode {
    DebugServer,
    EmbeddedAssets,
}

impl ProbeFrontendMode {
    fn select_os(
        mode: Option<&std::ffi::OsStr>,
        url: Option<&std::ffi::OsStr>,
        has_index: bool,
    ) -> Result<Self, &'static str> {
        let mode = mode
            .map(|value| value.to_str().ok_or("invalid probe frontend mode encoding"))
            .transpose()?;
        let url = url
            .map(|value| value.to_str().ok_or("invalid probe frontend URL encoding"))
            .transpose()?;
        Self::select(mode, url, has_index)
    }

    fn select(
        mode: Option<&str>,
        url: Option<&str>,
        has_index: bool,
    ) -> Result<Self, &'static str> {
        match mode {
            None | Some("debug-server") => {
                if url == Some("http://127.0.0.1:1427") {
                    Ok(Self::DebugServer)
                } else {
                    Err("probe debug frontend requires exact dedicated server URL")
                }
            }
            Some("embedded-assets") => {
                if url.is_some() {
                    Err("embedded probe frontend refuses external URL")
                } else if !has_index {
                    Err("embedded probe frontend assets unavailable")
                } else {
                    Ok(Self::EmbeddedAssets)
                }
            }
            _ => Err("invalid probe frontend mode"),
        }
    }
}

const UI_DRAFT_QUESTION: &str = "Owned WebView draft preservation check";

struct ConnectionsUiState {
    enabled: bool,
    nonce: String,
    checkpoints: Mutex<Vec<serde_json::Value>>,
    cores: Mutex<Option<serde_json::Value>>,
    progress: Mutex<Vec<serde_json::Value>>,
    failed: AtomicU8,
    failure_checkpoint: Mutex<Option<serde_json::Value>>,
}
impl Default for ConnectionsUiState {
    fn default() -> Self {
        Self {
            enabled: false,
            nonce: uuid::Uuid::new_v4().to_string(),
            checkpoints: Mutex::new(Vec::new()),
            cores: Mutex::new(None),
            progress: Mutex::new(Vec::new()),
            failed: AtomicU8::new(0),
            failure_checkpoint: Mutex::new(None),
        }
    }
}
#[derive(serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ConnectionUiRow {
    profile_id: String,
    revision: u64,
    model_id: Option<String>,
    mode_id: Option<String>,
    checked_at: Option<String>,
    label_matched: bool,
}
#[derive(serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum ConnectionUiFailureStage {
    ProfileQuery,
    Connections,
    RowProjection,
    DraftReturn,
    ModalAdd,
    ModalEdit,
    ModalSave,
    IpcIntegrity,
    Complete,
}
impl ConnectionUiFailureStage {
    fn progress_position(&self) -> (usize, usize) {
        match self {
            Self::ProfileQuery => (0, 0),
            Self::Connections => (1, 0),
            Self::RowProjection => (2, 0),
            Self::DraftReturn => (3, 1),
            Self::ModalAdd => (4, 2),
            Self::ModalEdit => (5, 2),
            Self::ModalSave => (6, 2),
            Self::IpcIntegrity => (7, 3),
            Self::Complete => (8, 3),
        }
    }
}
fn validate_ui_progress(
    stage: &ConnectionUiFailureStage,
    count: usize,
    accepted: usize,
    previous: usize,
) -> Result<(), &'static str> {
    let (position, required) = stage.progress_position();
    if position == previous && required == count && count == accepted {
        Ok(())
    } else {
        Err("UI progress order mismatch")
    }
}
#[derive(serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum ConnectionUiPhase {
    ScriptInitialized,
    HomeWait,
    HomeReady,
    ProfileListReady,
    ProjectionPending,
    ProjectionReady,
    ProjectionError,
    Comparison,
    EditControlWait,
    EditControlFound,
    EditClicked,
    EditDialogFound,
    EditDialogFocusWait,
    EditDialogFocusReady,
    EditTriggerFocusRestored,
    EditFocusFailed,
}
fn validate_ui_phase(
    stage: &ConnectionUiFailureStage,
    phase: &ConnectionUiPhase,
    count: usize,
    accepted: usize,
    progress: &[serde_json::Value],
) -> Result<(), &'static str> {
    let modal_edit = matches!(
        phase,
        ConnectionUiPhase::EditControlWait
            | ConnectionUiPhase::EditControlFound
            | ConnectionUiPhase::EditClicked
            | ConnectionUiPhase::EditDialogFound
            | ConnectionUiPhase::EditDialogFocusWait
            | ConnectionUiPhase::EditDialogFocusReady
            | ConnectionUiPhase::EditTriggerFocusRestored
            | ConnectionUiPhase::EditFocusFailed
    );
    if count != accepted
        || progress.len() >= 64
        || (matches!(phase, ConnectionUiPhase::ScriptInitialized)
            && (!matches!(stage, ConnectionUiFailureStage::ProfileQuery) || !progress.is_empty()))
        || (matches!(
            phase,
            ConnectionUiPhase::ProjectionPending
                | ConnectionUiPhase::ProjectionReady
                | ConnectionUiPhase::ProjectionError
                | ConnectionUiPhase::Comparison
        ) && !matches!(stage, ConnectionUiFailureStage::RowProjection))
        || (modal_edit && (!matches!(stage, ConnectionUiFailureStage::ModalEdit) || count != 2))
    {
        Err("UI phase authority mismatch")
    } else {
        Ok(())
    }
}
fn ui_script_initialized(progress: &[serde_json::Value]) -> bool {
    progress
        .iter()
        .any(|item| item["kind"] == "phase" && item["phase"] == "script_initialized")
}
fn write_ui_observation(
    root: &std::path::Path,
    name: &str,
    body: &serde_json::Value,
) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .custom_flags(libc::O_NOFOLLOW)
        .mode(0o600)
        .open(root.join(name))
        .map_err(|_| "UI observation output unavailable")?;
    file.write_all(&serde_json::to_vec_pretty(body).map_err(|_| "UI observation encoding")?)
        .map_err(|_| "UI observation write")?;
    file.sync_all()
        .map_err(|_| "UI observation synchronization".into())
}
#[derive(serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum ConnectionUiFailureCode {
    BoundedUiWait,
    ProfileCount,
    ProfileLabel,
    SavedModel,
    CatalogProjectionError,
    AuthenticationTime,
    DraftReturn,
    ProfileSaveReadback,
    InvokeReplaced,
    NativeReportRejected,
    UnexpectedException,
}
#[derive(serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ConnectionUiModelMismatch {
    actual_model_id: Option<String>,
    actual_mode_id: Option<String>,
    expected_model_id: Option<String>,
    expected_mode_id: Option<String>,
}
impl ConnectionUiModelMismatch {
    fn valid(&self) -> bool {
        [
            &self.actual_model_id,
            &self.actual_mode_id,
            &self.expected_model_id,
            &self.expected_mode_id,
        ]
        .into_iter()
        .all(|token| {
            token.as_ref().is_none_or(|token| {
                !token.is_empty() && token.len() <= 256 && !token.chars().any(char::is_control)
            })
        })
    }
}
#[derive(serde::Deserialize, serde::Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum ConnectionUiCheckpoint {
    Phase {
        stage: ConnectionUiFailureStage,
        phase: ConnectionUiPhase,
        checkpoint_count: usize,
    },
    Progress {
        stage: ConnectionUiFailureStage,
        checkpoint_count: usize,
    },
    Rows {
        rows: Vec<ConnectionUiRow>,
    },
    DraftReturn {
        question_digest: String,
        question_length: usize,
        cores_preserved: bool,
    },
    Modal {
        profile_id: String,
        previous_revision: u64,
        saved_revision: u64,
        add_initial_empty: bool,
        add_focused: bool,
        add_focus_returned: bool,
        edit_initial_matches: bool,
        edit_focused: bool,
        edit_cancelled: bool,
        discard_continue_retained: bool,
        discard_committed: bool,
        save_error_retained: bool,
        save_committed: bool,
    },
    Complete {},
    Failed {
        stage: ConnectionUiFailureStage,
        code: ConnectionUiFailureCode,
        checkpoint_count: usize,
        model_mismatch: Option<ConnectionUiModelMismatch>,
    },
}
impl ConnectionUiCheckpoint {
    fn validate_failure_count(&self, accepted: usize) -> Result<(), &'static str> {
        match self {
            Self::Failed {
                checkpoint_count,
                stage,
                code,
                model_mismatch,
            } if *checkpoint_count == accepted
                && accepted <= 3
                && match model_mismatch {
                    Some(tokens) => {
                        matches!(stage, ConnectionUiFailureStage::RowProjection)
                            && matches!(code, ConnectionUiFailureCode::SavedModel)
                            && tokens.valid()
                    }
                    None => {
                        !matches!(code, ConnectionUiFailureCode::SavedModel)
                            && (!matches!(code, ConnectionUiFailureCode::CatalogProjectionError)
                                || matches!(stage, ConnectionUiFailureStage::RowProjection))
                    }
                } =>
            {
                Ok(())
            }
            _ => Err("UI failure checkpoint count mismatch"),
        }
    }
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ConnectionUiReport {
    schema_version: u32,
    nonce: String,
    pid: u32,
    checkpoint: ConnectionUiCheckpoint,
}
impl ConnectionUiReport {
    fn validates_authority(&self, enabled: bool, nonce: &str, pid: u32, window: &str) -> bool {
        enabled
            && window == "main"
            && self.schema_version == 1
            && self.pid == pid
            && self.nonce == nonce
    }
}
#[tauri::command]
fn connections_ui_probe_report(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, commands::DesktopState>,
    report: tauri::State<'_, ConnectionsUiState>,
    input: ConnectionUiReport,
) -> Result<(), &'static str> {
    if !input.validates_authority(
        report.enabled,
        &report.nonce,
        std::process::id(),
        window.label(),
    ) {
        return Err("UI report authority mismatch");
    }
    let storage = state.storage().map_err(|_| "UI store unavailable")?;
    let mut accepted = report
        .checkpoints
        .lock()
        .map_err(|_| "UI state unavailable")?;
    match &input.checkpoint {
        ConnectionUiCheckpoint::Phase {
            stage,
            phase,
            checkpoint_count,
        } => {
            let mut progress = report
                .progress
                .lock()
                .map_err(|_| "UI progress state unavailable")?;
            validate_ui_phase(stage, phase, *checkpoint_count, accepted.len(), &progress)?;
            progress
                .push(serde_json::to_value(&input.checkpoint).map_err(|_| "UI phase encoding")?);
            return Ok(());
        }
        ConnectionUiCheckpoint::Progress {
            stage,
            checkpoint_count,
        } => {
            let mut progress = report
                .progress
                .lock()
                .map_err(|_| "UI progress state unavailable")?;
            validate_ui_progress(
                stage,
                *checkpoint_count,
                accepted.len(),
                progress
                    .iter()
                    .filter(|item| item["kind"] == "progress")
                    .count(),
            )?;
            progress
                .push(serde_json::to_value(&input.checkpoint).map_err(|_| "UI progress encoding")?);
            return Ok(());
        }
        ConnectionUiCheckpoint::Rows { rows } => {
            if !accepted.is_empty() || rows.is_empty() || rows.len() > 100 {
                return Err("UI row count invalid");
            }
            let profiles = storage
                .list_provider_profiles(100)
                .map_err(|_| "UI profile query")?;
            let local = profiles
                .iter()
                .filter(|profile| {
                    profile.authentication_method
                        == magi_domain::ProviderAuthenticationMethod::LocalSubscription
                })
                .collect::<Vec<_>>();
            if rows.len() != local.len() {
                return Err("UI native profile count mismatch");
            }
            for (row, profile) in rows.iter().zip(local) {
                if row.profile_id != profile.provider_profile_id
                    || row.revision != profile.revision
                    || row.checked_at.is_some()
                    || !row.label_matched
                {
                    return Err("UI native profile mismatch");
                }
                let catalog = profiles::load_provider_catalog(
                    window.clone(),
                    state.clone(),
                    row.profile_id.clone(),
                    row.revision,
                )
                .map_err(|_| "UI native catalog query")?;
                let value = serde_json::to_value(catalog).map_err(|_| "UI catalog encoding")?;
                let binding = &value["modelSelection"]["binding"];
                if row.model_id.as_deref() != binding["modelId"].as_str()
                    || row.mode_id.as_deref() != binding["modeId"].as_str()
                {
                    return Err("UI saved model mismatch");
                }
            }
            *report.cores.lock().map_err(|_| "UI core state")? = Some(
                serde_json::to_value(
                    profiles::load_core_model_selections(window.clone(), state.clone())
                        .map_err(|_| "UI core query")?,
                )
                .map_err(|_| "UI core encoding")?,
            );
        }
        ConnectionUiCheckpoint::DraftReturn {
            question_digest,
            question_length,
            cores_preserved,
        } => {
            if accepted.len() != 1
                || !cores_preserved
                || *question_length != UI_DRAFT_QUESTION.len()
                || question_digest
                    != magi_domain::Digest::from_bytes(UI_DRAFT_QUESTION.as_bytes()).as_str()
            {
                return Err("UI draft proof mismatch");
            }
            let current = serde_json::to_value(
                profiles::load_core_model_selections(window.clone(), state.clone())
                    .map_err(|_| "UI core query")?,
            )
            .map_err(|_| "UI core encoding")?;
            if report.cores.lock().map_err(|_| "UI core state")?.as_ref() != Some(&current) {
                return Err("UI saved cores changed");
            }
        }
        ConnectionUiCheckpoint::Modal {
            profile_id,
            previous_revision,
            saved_revision,
            add_initial_empty,
            add_focused,
            add_focus_returned,
            edit_initial_matches,
            edit_focused,
            edit_cancelled,
            discard_continue_retained,
            discard_committed,
            save_error_retained,
            save_committed,
        } => {
            if accepted.len() != 2
                || ![
                    add_initial_empty,
                    add_focused,
                    add_focus_returned,
                    edit_initial_matches,
                    edit_focused,
                    edit_cancelled,
                    discard_continue_retained,
                    discard_committed,
                    save_error_retained,
                    save_committed,
                ]
                .into_iter()
                .all(|value| *value)
                || previous_revision.checked_add(1) != Some(*saved_revision)
            {
                return Err("UI modal proof invalid");
            }
            let original = &accepted[0]["rows"][0];
            if original["profileId"].as_str() != Some(profile_id.as_str())
                || original["revision"].as_u64() != Some(*previous_revision)
            {
                return Err("UI modal original profile mismatch");
            }
            let profile = storage
                .load_provider_profile(profile_id)
                .map_err(|_| "UI saved profile query")?
                .ok_or("UI saved profile missing")?;
            if profile.revision != *saved_revision {
                return Err("UI saved profile revision mismatch");
            }
        }
        ConnectionUiCheckpoint::Complete {} => {
            if accepted.len() != 3 {
                return Err("UI checkpoints incomplete");
            }
        }
        ConnectionUiCheckpoint::Failed { .. } => {
            input.checkpoint.validate_failure_count(accepted.len())?;
            let safe_checkpoint =
                serde_json::to_value(&input.checkpoint).map_err(|_| "UI failure encoding")?;
            *report
                .failure_checkpoint
                .lock()
                .map_err(|_| "UI failure state unavailable")? = Some(safe_checkpoint);
            report.failed.store(1, Ordering::Release);
            return Ok(());
        }
    }
    accepted.push(serde_json::to_value(input.checkpoint).map_err(|_| "UI report encoding")?);
    Ok(())
}

async fn connections_ui_probe(
    app: &tauri::AppHandle,
    window: &tauri::WebviewWindow,
    root: &std::path::Path,
    monitor: &ProbeMonitor,
) -> Result<(), String> {
    monitor.mark("connections_ui");
    let state = app.state::<ConnectionsUiState>();
    let remaining = Duration::from_secs(300).saturating_sub(monitor.started.elapsed());
    let config = serde_json::json!({"nonce": state.nonce, "pid":std::process::id(), "remainingMs":remaining.as_millis(), "question":UI_DRAFT_QUESTION, "unavailableHome":root.join("unavailable-home")});
    window
        .eval(format!(
            "window.__magiConnectionProbe={config};\n{}",
            include_str!("../native/connection-ui-probe.js")
        ))
        .map_err(|_| "UI script initialization")?;
    let mut persisted_progress = 0;
    loop {
        let progress = state
            .progress
            .lock()
            .map_err(|_| "UI progress state")?
            .clone();
        let storage = app.state::<commands::DesktopState>().storage()?;
        for (index, checkpoint) in progress.iter().enumerate().skip(persisted_progress) {
            write_ui_observation(
                root,
                &format!("connections-ui-progress-{index}.json"),
                &serde_json::json!({"schemaVersion":1,"pid":std::process::id(),"window":"main","nonce":state.nonce,"store":storage.identity(),"checkpoint":checkpoint,"providerActions":false}),
            )?;
            persisted_progress += 1;
        }
        if monitor.lifecycle.check().is_err() {
            let accepted = state
                .checkpoints
                .lock()
                .map_err(|_| "UI checkpoint state")?
                .len();
            write_ui_observation(
                root,
                "connections-ui-deadline.json",
                &serde_json::json!({"schemaVersion":1,"pid":std::process::id(),"window":"main","nonce":state.nonce,"store":storage.identity(),"lastProgress":progress.last(),"scriptInitialized":ui_script_initialized(&progress),"acceptedCheckpointCount":accepted,"failureReceived":state.failed.load(Ordering::Acquire)!=0,"providerActions":false}),
            )?;
            return Err("UI absolute deadline".into());
        }
        if state.failed.load(Ordering::Acquire) != 0 {
            let checkpoint = state
                .failure_checkpoint
                .lock()
                .map_err(|_| "UI failure state unavailable")?
                .clone()
                .ok_or("UI failure checkpoint missing")?;
            let storage = app.state::<commands::DesktopState>().storage()?;
            let body = serde_json::to_vec_pretty(&serde_json::json!({"schemaVersion":1,"pid":std::process::id(),"window":"main","nonce":state.nonce,"store":storage.identity(),"checkpoint":checkpoint,"providerActions":false})).map_err(|_| "UI failure output encoding")?;
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            let mut output = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .custom_flags(libc::O_NOFOLLOW)
                .mode(0o600)
                .open(root.join("connections-ui-failure.json"))
                .map_err(|_| "UI failure output unavailable")?;
            output
                .write_all(&body)
                .map_err(|_| "UI failure output write")?;
            output
                .sync_all()
                .map_err(|_| "UI failure output synchronization")?;
            return Err("physical UI checkpoint failed".into());
        }
        let complete = state
            .checkpoints
            .lock()
            .map_err(|_| "UI report state")?
            .len()
            == 4;
        if complete {
            let body = {
                let checkpoints = state.checkpoints.lock().map_err(|_| "UI report state")?;
                let store = app.state::<commands::DesktopState>().storage()?;
                serde_json::to_vec_pretty(&serde_json::json!({"schemaVersion":1,"pid":std::process::id(),"window":"main","nonce":state.nonce,"store":store.identity(),"checkpoints":*checkpoints,"providerActions":false})).map_err(|_| "UI output encoding")?
            };
            let digest = magi_domain::Digest::from_bytes(&body);
            std::fs::write(root.join("connections-ui-report.json"), &body)
                .map_err(|_| "UI report output")?;
            std::fs::write(root.join("connections-ui-ready.json"), serde_json::to_vec(&serde_json::json!({"pid":std::process::id(),"nonce":state.nonce,"reportDigest":digest})).map_err(|_| "UI checkpoint encoding")?).map_err(|_| "UI checkpoint output")?;
            monitor.mark("connections_ui_checkpoint");
            loop {
                monitor
                    .lifecycle
                    .check()
                    .map_err(|_| "UI checkpoint deadline")?;
                let release = root.join("connections-ui-release.json");
                if release.exists() {
                    use std::io::Read;
                    use std::os::unix::fs::OpenOptionsExt;
                    let file = std::fs::OpenOptions::new()
                        .read(true)
                        .custom_flags(libc::O_NOFOLLOW)
                        .open(release)
                        .map_err(|_| "UI release unavailable")?;
                    let metadata = file.metadata().map_err(|_| "UI release metadata")?;
                    if !metadata.is_file() || metadata.len() > 4096 {
                        return Err("UI release invalid".into());
                    }
                    let mut bytes = Vec::new();
                    file.take(4097)
                        .read_to_end(&mut bytes)
                        .map_err(|_| "UI release read")?;
                    let value: serde_json::Value =
                        serde_json::from_slice(&bytes).map_err(|_| "UI release shape")?;
                    if value
                        != serde_json::json!({"pid":std::process::id(),"nonce":state.nonce,"reportDigest":digest})
                    {
                        return Err("UI release authority mismatch".into());
                    }
                    return Ok(());
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[derive(Default)]
struct PdfUiProbeState {
    metadata: Mutex<Option<(String, u32)>>,
    rendered: Mutex<Option<(String, u64)>>,
    failed: AtomicU8,
}

#[derive(serde::Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum PdfUiReport {
    Metadata {
        draft_id: String,
        context_revision: Option<u64>,
        total_pages: u32,
    },
    Rendered {
        draft_id: String,
        revision: u64,
    },
    Failed,
}

#[tauri::command]
fn pdf_ui_probe_report(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, commands::DesktopState>,
    report: tauri::State<'_, PdfUiProbeState>,
    input: PdfUiReport,
) -> Result<(), &'static str> {
    if window.label() != "main" {
        return Err("native probe report requires main window");
    }
    match input {
        PdfUiReport::Metadata {
            draft_id,
            context_revision,
            total_pages,
        } => {
            if draft_id.is_empty()
                || draft_id.len() > 256
                || context_revision.is_some()
                || !(2..=1000).contains(&total_pages)
            {
                return Err("invalid PDF metadata proof");
            }
            let storage = state
                .storage()
                .map_err(|_| "PDF probe storage unavailable")?;
            if !storage
                .list_context_drafts(1)
                .map_err(|_| "PDF draft query failed")?
                .is_empty()
            {
                return Err("metadata selection mutated a draft");
            }
            *report
                .metadata
                .lock()
                .map_err(|_| "PDF probe state unavailable")? = Some((draft_id, total_pages));
        }
        PdfUiReport::Rendered { draft_id, revision } => {
            if revision != 0
                || report
                    .metadata
                    .lock()
                    .map_err(|_| "PDF probe state unavailable")?
                    .as_ref()
                    .is_none_or(|(id, _)| id != &draft_id)
            {
                return Err("PDF capture differs from selected metadata");
            }
            *report
                .rendered
                .lock()
                .map_err(|_| "PDF probe state unavailable")? = Some((draft_id, revision));
        }
        PdfUiReport::Failed => {
            report.failed.store(1, Ordering::Release);
        }
    }
    Ok(())
}

async fn pdf_ui_probe(
    app: &tauri::AppHandle,
    window: &tauri::WebviewWindow,
    root: &std::path::Path,
    monitor: &ProbeMonitor,
) -> Result<(), String> {
    monitor.mark("pdf_ui");
    window.set_focus().map_err(|_| "PDF console focus failed")?;
    window.eval(r#"(async()=>{
      const wait=async fn=>{for(let i=0;i<600;i++){const value=fn();if(value)return value;await new Promise(r=>setTimeout(r,50));}throw Error('bounded UI wait');};
      const button=text=>Array.from(document.querySelectorAll('button')).find(el=>el.textContent.trim().includes(text));
      const original=window.__TAURI_INTERNALS__.invoke.bind(window.__TAURI_INTERNALS__);
      window.__TAURI_INTERNALS__.invoke=async(cmd,args,options)=>{const result=await original(cmd,args,options);
        if(cmd==='prepare_pdf_range_capture'&&result)await original('pdf_ui_probe_report',{input:{kind:'metadata',draftId:result.draftId,contextRevision:result.contextRevision,totalPages:result.totalPages}});
        if(cmd==='apply_pdf_range_capture')window.__pdfCaptureResult=result;
        return result;
      };
      try{
        (await wait(()=>button('새 심의 시작'))).click();
        const question=await wait(()=>document.querySelector('#question-draft'));
        Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype,'value').set.call(question,'Selected page approval');question.dispatchEvent(new Event('input',{bubbles:true}));
        (await wait(()=>button('자료 보기'))).click();
        (await wait(()=>button('페이지 범위를 정할 PDF 선택'))).click();
        const fields=await wait(()=>{const inputs=document.querySelectorAll('fieldset input[type=number]');return inputs.length===2?inputs:null});
        for(const field of fields){Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value').set.call(field,'2');field.dispatchEvent(new Event('input',{bubbles:true}));}
        (await wait(()=>{const el=button('이 페이지 범위만 접수');return el&&!el.disabled?el:null})).click();
        const result=await wait(()=>window.__pdfCaptureResult);
        await wait(()=>document.querySelector('.source-row.source-captured')&&!document.querySelector('fieldset input[type=number]'));
        if(document.querySelector('#question-draft').value!=='Selected page approval')throw Error('draft question changed');
        await original('pdf_ui_probe_report',{input:{kind:'rendered',draftId:result.draftId,revision:result.revision}});
      }catch{await original('pdf_ui_probe_report',{input:{kind:'failed'}});}
    })()"#).map_err(|_| "PDF WebView script failed")?;
    let started = Instant::now();
    loop {
        let report = app.state::<PdfUiProbeState>();
        if report.failed.load(Ordering::Acquire) != 0 {
            return Err("PDF rendered native flow failed".into());
        }
        let rendered = report
            .rendered
            .lock()
            .map_err(|_| "PDF probe state unavailable")?
            .clone();
        if let Some((draft_id, revision)) = rendered {
            let storage = app.state::<commands::DesktopState>().storage()?;
            let draft = storage
                .load_context_draft(&draft_id)
                .map_err(|_| "PDF persisted draft failed")?
                .ok_or("PDF persisted draft missing")?;
            if draft.revision != revision
                || draft.manifest.content.sources.len() != 1
                || storage
                    .has_queued_live_runs()
                    .map_err(|_| "PDF queue check failed")?
            {
                return Err("PDF draft or no-dispatch proof failed".into());
            }
            let source = &draft.manifest.content.sources[0];
            if source.included_locators.is_empty()
                || source
                    .included_locators
                    .iter()
                    .any(|locator| locator.page != Some(2))
            {
                return Err("PDF capture includes unapproved page".into());
            }
            let digest = source
                .derived_digest
                .as_ref()
                .ok_or("PDF derived object missing")?;
            let bytes = storage
                .read_source_object(digest)
                .map_err(|_| "PDF derived object unreadable")?;
            let text = std::str::from_utf8(&bytes).map_err(|_| "PDF derived text invalid")?;
            if !text.contains("CONTROLLED PAGE TWO") || text.contains("UNSELECTED PAGE ONE") {
                return Err("PDF immutable selected text differs".into());
            }
            let proof = serde_json::json!({"kind":"native_webview_pdf_capture","draftId":draft_id,"revision":revision,"manifestDigest":draft.manifest.digest,"derivedDigest":digest,"page":2,"metadataOnlyBeforeApply":true,"rendered":true,"modelDispatches":0});
            std::fs::write(
                root.join("pdf-ui-capture-proof.json"),
                serde_json::to_vec_pretty(&proof).map_err(|_| "PDF proof encoding failed")?,
            )
            .map_err(|_| "PDF proof write failed")?;
            println!(
                "native WebView selected PDF page two persisted and rendered without model dispatch"
            );
            return Ok(());
        }
        if started.elapsed() > Duration::from_secs(240) {
            return Err("PDF native UI deadline exceeded".into());
        }
        tauri::async_runtime::spawn_blocking(|| std::thread::sleep(Duration::from_millis(100)))
            .await
            .map_err(|_| "PDF probe poll failed")?;
    }
}

struct ProbeMonitor {
    outcome: AtomicU8,
    phase: Mutex<(&'static str, Instant)>,
    active_request: Mutex<Option<profiles::CancelDeliberationRequestInputDto>>,
    started: Instant,
    lifecycle: commands::AdmissionRequestLifecycle,
}

impl commands::AdmissionRequestLifecycle {
    pub(crate) fn revoke_with_cancellation_lease(
        &self,
    ) -> Option<commands::AdmissionOperationLease> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.terminal {
            self.verification.revoke();
            return None;
        }
        self.operations.fetch_add(1, Ordering::SeqCst);
        state.revoked = true;
        self.verification.revoke();
        Some(commands::AdmissionOperationLease(self.operations.clone()))
    }
}
impl commands::AdmissionRequestRegistry {
    pub(crate) fn supervised_capability(
        &self,
        lifecycle: &commands::AdmissionRequestLifecycle,
        command_id: &str,
        key: &str,
    ) -> Option<commands::AdmissionAuthorityDto> {
        self.requests
            .lock()
            .ok()?
            .iter()
            .find_map(|(token, request)| {
                (Arc::ptr_eq(&request.state, &lifecycle.state)
                    && request.binding.command_id == command_id
                    && request.binding.idempotency_key == key)
                    .then(|| commands::AdmissionAuthorityDto {
                        token: token.clone(),
                        process_epoch: self.epoch.as_ref().clone(),
                    })
            })
    }
}
impl ProbeMonitor {
    fn ensure_running(&self) -> Result<(), String> {
        if self.outcome.load(Ordering::Acquire) != 0 {
            return Err("supervised probe permission revoked".into());
        }
        self.lifecycle
            .check()
            .map_err(|_| "supervised probe permission revoked".into())
    }

    fn revoke_for_timeout(&self) -> Option<commands::AdmissionOperationLease> {
        self.lifecycle.revoke_with_cancellation_lease()
    }
    fn mark_verified(&self, app: &tauri::AppHandle, phase: &'static str, root: &std::path::Path) {
        self.mark(phase);
        let Ok(service) = app.state::<commands::DesktopState>().runtime_service() else {
            return;
        };
        let metrics = service.metrics();
        eprintln!("native verification phase metrics: {phase}; {metrics:?}");
        let value = serde_json::json!({"phase":phase,"elapsedMicros":self.started.elapsed().as_micros(),"calls":metrics.calls,"proofs":metrics.proofs,"reused":metrics.reused,"hashedBytes":metrics.hashed_bytes,"hashMicros":metrics.hash_micros,"subprocessMicros":metrics.subprocess_micros,"queueMicros":metrics.queue_micros,"heldChecks":metrics.held_checks,"heldCheckMicros":metrics.held_check_micros});
        if let Ok(bytes) = serde_json::to_vec_pretty(&value) {
            let _ = std::fs::write(root.join(format!("verification-{phase}.json")), bytes);
        }
    }

    async fn record_application_shutdown(
        &self,
        app: &tauri::AppHandle,
        root: &std::path::Path,
    ) -> bool {
        let deadline = Instant::now() + Duration::from_secs(30);
        let monitored = self.record_terminal_settlement_until(root, deadline).await;
        let state = app.state::<commands::DesktopState>();
        let global = state.freeze_resource_consumers(deadline, None).await;
        let observed_closed = global.is_ok();
        let timed_out = matches!(global, Err(magi_provider::ProviderError::Timeout));
        let evidence = serde_json::json!({
            "monitoredSettled": monitored,
            "artifactConsumersObservedClosed": observed_closed,
            "deadlineReached": timed_out,
            "unknownCleanup": !observed_closed
        });
        let written = serde_json::to_vec_pretty(&evidence)
            .ok()
            .and_then(|bytes| std::fs::write(root.join("application-shutdown.json"), bytes).ok())
            .is_some();
        monitored && observed_closed && written
    }

    async fn record_terminal_settlement_until(
        &self,
        root: &std::path::Path,
        deadline: Instant,
    ) -> bool {
        let mut revoked = matches!(
            self.lifecycle.verification.check(),
            Err(magi_provider::ProviderError::Cancelled)
        );
        while !revoked && Instant::now() < deadline {
            if let Ok(mut state) = self.lifecycle.state.try_lock() {
                state.revoked = true;
                self.lifecycle.verification.revoke();
                revoked = true;
                break;
            }
            if self
                .lifecycle
                .verification
                .wait_for_stream_checkpoint(deadline)
                .await
                .is_err()
            {
                break;
            }
        }
        let provider_wait = if revoked {
            self.lifecycle
                .verification
                .wait_for_settlement(deadline)
                .await
        } else {
            Err(magi_provider::ProviderError::Timeout)
        };
        while self.lifecycle.operations.load(Ordering::SeqCst) != 0 && Instant::now() < deadline {
            if self
                .lifecycle
                .verification
                .wait_for_stream_checkpoint(deadline)
                .await
                .is_err()
            {
                break;
            }
        }
        let snapshot = self.lifecycle.verification.settlement();
        let native_operations = self.lifecycle.operations.load(Ordering::SeqCst);
        let settled = provider_wait.is_ok() && snapshot.is_settled() && native_operations == 0;
        let evidence = serde_json::json!({
            "revoked": revoked, "settled": settled,
            "queuedVerifications": snapshot.queued_verifications,
            "verificationWorkers": snapshot.verification_workers,
            "providerOperations": snapshot.provider_operations,
            "unresolvedCleanup": snapshot.unresolved_cleanup,
            "nativeOperations": native_operations,
            "deadlineReached": !settled && Instant::now() >= deadline
        });
        eprintln!("native terminal settlement: {evidence}");
        let written = serde_json::to_vec_pretty(&evidence)
            .ok()
            .and_then(|bytes| std::fs::write(root.join("terminal-settlement.json"), bytes).ok())
            .is_some();
        settled && written
    }

    fn mark(&self, phase: &'static str) {
        *self.phase.lock().unwrap() = (phase, Instant::now());
        eprintln!(
            "native live probe phase: {phase}; elapsed_micros={}",
            self.started.elapsed().as_micros()
        );
    }
}

fn valid_recovery_run(run: &str) -> bool {
    run.strip_prefix("run-").is_some_and(|digest| {
        digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}

fn prepare_connections_ui_store() -> Result<(), &'static str> {
    if std::env::var_os("MAGI_TEST_UI_SOURCE_ROOT").is_some() {
        return Err("legacy source root input is ambiguous");
    }
    let source = PathBuf::from(
        std::env::var_os("MAGI_TEST_UI_SOURCE_DATABASE")
            .ok_or("explicit source database file required")?,
    );
    let destination = PathBuf::from(
        std::env::var_os("MAGI_TEST_DATA_ROOT").ok_or("explicit destination required")?,
    );
    let expected = std::env::var("MAGI_TEST_UI_SOURCE_DIGEST")
        .map_err(|_| "explicit source digest required")?;
    prepare_connection_metadata(&source, &destination, &expected)
}

fn prepare_connection_metadata(
    source: &std::path::Path,
    destination: &std::path::Path,
    expected: &str,
) -> Result<(), &'static str> {
    use std::io::{Read, Seek, SeekFrom};
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    if expected.len() != 64
        || !expected.bytes().all(|byte| byte.is_ascii_hexdigit())
        || destination.exists()
    {
        return Err("source pin or fresh destination invalid");
    }
    let database = source;
    let sidecar = |suffix: &str| {
        let mut path = database.as_os_str().to_os_string();
        path.push(suffix);
        PathBuf::from(path)
    };
    if sidecar("-wal").exists() || sidecar("-shm").exists() {
        return Err("closed coherent source snapshot required");
    }
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(database)
        .map_err(|_| "source database unavailable")?;
    let before = file.metadata().map_err(|_| "source identity unavailable")?;
    if !before.is_file()
        || before.nlink() != 1
        || before.uid() != unsafe { libc::geteuid() }
        || before.len() > 20 * 1024 * 1024
    {
        return Err("bounded owned source required");
    }
    let mut bytes = Vec::new();
    file.try_clone()
        .map_err(|_| "source custody unavailable")?
        .take(20 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "source read failed")?;
    if magi_domain::Digest::from_bytes(&bytes).as_str() != expected {
        return Err("source digest mismatch");
    }
    let metadata =
        magi_storage::StorageReader::connection_metadata_from_captured_snapshot(&bytes, expected)
            .map_err(|_| "read-only metadata snapshot failed")?;
    if metadata.schema_version != 1 {
        return Err("unsupported metadata projection");
    }
    let entries = metadata
        .profiles
        .iter()
        .map(|profile| {
            Ok((
                profile.history.as_slice(),
                profile
                    .selected_catalog
                    .as_ref()
                    .ok_or("actual catalog required")?,
                profile.latest_catalog.as_ref(),
                profile
                    .model_selection
                    .as_ref()
                    .ok_or("actual model selection required")?,
            ))
        })
        .collect::<Result<Vec<_>, &'static str>>()?;
    let after =
        std::fs::symlink_metadata(database).map_err(|_| "source final identity unavailable")?;
    let opened = file
        .metadata()
        .map_err(|_| "source held identity unavailable")?;
    let same = |value: &std::fs::Metadata| {
        value.dev() == before.dev()
            && value.ino() == before.ino()
            && value.mode() == before.mode()
            && value.nlink() == before.nlink()
            && value.len() == before.len()
            && value.mtime() == before.mtime()
            && value.mtime_nsec() == before.mtime_nsec()
            && value.ctime() == before.ctime()
            && value.ctime_nsec() == before.ctime_nsec()
    };
    if !same(&after) || !same(&opened) {
        return Err("source database identity changed during metadata preparation");
    }
    if sidecar("-wal").exists() || sidecar("-shm").exists() {
        return Err("read-only metadata query acquired SQLite sidecars");
    }
    file.seek(SeekFrom::Start(0))
        .map_err(|_| "source final seek failed")?;
    let mut actual = Vec::new();
    (&mut file)
        .take(20 * 1024 * 1024 + 1)
        .read_to_end(&mut actual)
        .map_err(|_| "source final held read failed")?;
    if magi_domain::Digest::from_bytes(&actual).as_str() != expected {
        return Err("source final digest mismatch");
    }
    let (summaries, mut destination_cleanup) = connection_ui_fixture::seed(destination, &entries)?;
    destination_cleanup.validate()?;
    let destination_digest =
        magi_domain::Digest::from_bytes(&destination_cleanup.database_bytes()?);
    let body=serde_json::to_vec_pretty(&serde_json::json!({"schemaVersion":1,"sourceStore":metadata.store_identity,"sourceDigest":expected,"sourceCatalogHighWater":metadata.catalog_high_water,"preparedStoreDigest":destination_digest,"profiles":summaries,"coreSelectionsImported":false,"authenticationImported":false,"credentialsCopied":false})).map_err(|_| "metadata output encoding")?;
    destination_cleanup.write_provenance(&body)?;
    destination_cleanup.commit()?;
    Ok(())
}

fn verified_policy_resource(
    resource_root: &std::path::Path,
    expected: &str,
) -> Result<PathBuf, &'static str> {
    use std::io::Read;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    if expected.len() != 64 || !expected.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("invalid approved policy digest");
    }
    let directory = resource_root.join("policy");
    let directory_metadata =
        std::fs::symlink_metadata(&directory).map_err(|_| "policy resource missing")?;
    if !directory_metadata.is_dir() || directory_metadata.mode() & 0o222 != 0 {
        return Err("policy resource directory unsealed");
    }
    let path = directory.join("SECURITY.md");
    let before = std::fs::symlink_metadata(&path).map_err(|_| "policy resource missing")?;
    if !before.is_file()
        || before.nlink() != 1
        || before.mode() & 0o222 != 0
        || before.len() > 1024 * 1024
    {
        return Err("policy resource unsealed");
    }
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&path)
        .map_err(|_| "policy resource unavailable")?;
    let identity = |metadata: &std::fs::Metadata| {
        (
            metadata.dev(),
            metadata.ino(),
            metadata.mode(),
            metadata.nlink(),
            metadata.uid(),
            metadata.len(),
            metadata.mtime(),
            metadata.mtime_nsec(),
            metadata.ctime(),
            metadata.ctime_nsec(),
        )
    };
    if identity(&before) != identity(&file.metadata().map_err(|_| "policy resource identity")?) {
        return Err("policy resource replaced");
    }
    let mut bytes = Vec::new();
    file.by_ref()
        .take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "policy resource read")?;
    if magi_domain::Digest::from_bytes(&bytes).as_str() != expected {
        return Err("approved policy resource mismatch");
    }
    if identity(&before) != identity(&file.metadata().map_err(|_| "policy resource identity")?)
        || identity(&before)
            != identity(&std::fs::symlink_metadata(&path).map_err(|_| "policy resource missing")?)
        || (
            directory_metadata.dev(),
            directory_metadata.ino(),
            directory_metadata.mode(),
        ) != {
            let after = std::fs::symlink_metadata(&directory)
                .map_err(|_| "policy resource directory missing")?;
            (after.dev(), after.ino(), after.mode())
        }
    {
        return Err("policy resource replaced");
    }
    Ok(path)
}

struct SavedRunAdmissionActivationPermission {
    prior: magi_storage::AdmissionExecutionAuthority,
    quiescence: commands::NativeResourceQuiescence,
    deadline: std::time::Instant,
}
impl magi_storage::AdmissionActivationPermission for SavedRunAdmissionActivationPermission {
    fn validate(
        &self,
        previous: &magi_storage::AdmissionExecutionAuthority,
    ) -> Result<(), magi_storage::StorageError> {
        if previous != &self.prior || !previous.active || std::time::Instant::now() >= self.deadline
        {
            return Err(magi_storage::StorageError::DispatchFenced);
        }
        self.quiescence
            .validate()
            .map_err(|_| magi_storage::StorageError::DispatchFenced)
    }
}

async fn retained_saved_preparation(
    storage: magi_storage::Storage,
    deadline: std::time::Instant,
) -> Result<
    (magi_storage::Storage, SavedRunAdmissionActivationPermission),
    magi_provider::ProviderError,
> {
    let source = Arc::new(storage);
    let prior = source
        .admission_execution_authority()
        .map_err(|_| magi_provider::ProviderError::ArtifactVerification)?;
    if !prior.active {
        return Err(magi_provider::ProviderError::ArtifactVerification);
    }
    let state = commands::DesktopState::from_storage_parts(Some(source.clone()), None);
    let quiescence = state.freeze_resource_consumers(deadline, None).await?;
    quiescence.validate()?;
    state.release_storage_owner();
    let source =
        Arc::try_unwrap(source).map_err(|_| magi_provider::ProviderError::ArtifactVerification)?;
    Ok((
        source,
        SavedRunAdmissionActivationPermission {
            prior,
            quiescence,
            deadline,
        },
    ))
}

struct OwnedPreparationPath {
    path: PathBuf,
    keep: bool,
}
impl Drop for OwnedPreparationPath {
    fn drop(&mut self) {
        if !self.keep
            && self.path.exists()
            && let Err(error) = std::fs::remove_dir_all(&self.path)
        {
            if std::thread::panicking() {
                eprintln!("Owned preparation cleanup failed: {error}");
            } else {
                panic!("Owned preparation cleanup failed: {error}");
            }
        }
    }
}

fn prepare_retained_saved_profile_backup() -> Result<(), String> {
    let required = |name| {
        std::env::var_os(name)
            .map(PathBuf::from)
            .ok_or_else(|| format!("{name} required"))
    };
    prepare_retained_saved_profile_backup_at(
        required("MAGI_TEST_RETAINED_PROFILE_ROOT")?,
        required("MAGI_TEST_OWNED_COPY_ROOT")?,
        required("MAGI_TEST_SAVED_PROFILE_BACKUP_ROOT")?,
    )
}

fn prepare_retained_saved_profile_backup_at(
    retained: PathBuf,
    copy: PathBuf,
    backup: PathBuf,
) -> Result<(), String> {
    use std::io::Read;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    if copy.exists() || backup.exists() || retained == copy || retained == backup {
        return Err("fresh distinct owned destinations required".into());
    }
    use std::os::fd::{AsRawFd, FromRawFd};
    fn open_at(
        parent: &std::fs::File,
        name: &str,
        directory: bool,
    ) -> Result<std::fs::File, String> {
        let name = std::ffi::CString::new(name).map_err(|_| "retained name invalid")?;
        let flags = libc::O_RDONLY
            | libc::O_NOFOLLOW
            | libc::O_CLOEXEC
            | libc::O_NONBLOCK
            | if directory { libc::O_DIRECTORY } else { 0 };
        let fd = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags) };
        if fd < 0 {
            return Err("retained relative acquisition failed".into());
        }
        let file = unsafe { std::fs::File::from_raw_fd(fd) };
        let metadata = file.metadata().map_err(|_| "retained metadata")?;
        if metadata.uid() != unsafe { libc::geteuid() } || (directory && !metadata.is_dir()) {
            return Err("retained owner invalid".into());
        }
        Ok(file)
    }
    fn names(parent: &std::fs::File) -> Result<Vec<String>, String> {
        let fd = unsafe { libc::dup(parent.as_raw_fd()) };
        if fd < 0 {
            return Err("directory enumeration failed".into());
        }
        let directory = unsafe { libc::fdopendir(fd) };
        if directory.is_null() {
            unsafe {
                libc::close(fd);
            }
            return Err("directory enumeration failed".into());
        }
        let result = (|| {
            let mut names = Vec::new();
            loop {
                let entry = unsafe { libc::readdir(directory) };
                if entry.is_null() {
                    break;
                }
                let name = unsafe { std::ffi::CStr::from_ptr((*entry).d_name.as_ptr()) }
                    .to_str()
                    .map_err(|_| "directory name invalid")?;
                if name != "." && name != ".." {
                    names.push(name.to_owned());
                }
            }
            Ok(names)
        })();
        if unsafe { libc::closedir(directory) } != 0 {
            return Err("directory close failed".into());
        }
        result
    }
    let retained_directory = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY | libc::O_CLOEXEC)
        .open(&retained)
        .map_err(|_| "retained directory unavailable")?;
    let state_directory = open_at(&retained_directory, "state", true)?;
    let objects_directory = open_at(&retained_directory, "objects", true)?;
    let check_closed = || -> Result<(), String> {
        for suffix in ["-wal", "-shm"] {
            let name = std::ffi::CString::new(format!("magi.sqlite{suffix}"))
                .map_err(|_| "retained name")?;
            let mut metadata = std::mem::MaybeUninit::<libc::stat>::uninit();
            let result = unsafe {
                libc::fstatat(
                    state_directory.as_raw_fd(),
                    name.as_ptr(),
                    metadata.as_mut_ptr(),
                    libc::AT_SYMLINK_NOFOLLOW,
                )
            };
            if result == 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::ENOENT) {
                return Err("closed retained store required".into());
            }
        }
        Ok(())
    };
    let read_file = |parent: &std::fs::File, name: &str| -> Result<Vec<u8>, String> {
        let mut file = open_at(parent, name, false)?;
        let before = file.metadata().map_err(|_| "retained metadata")?;
        if !before.is_file() || before.nlink() != 1 || before.len() > 500 * 1024 * 1024 {
            return Err("retained file custody invalid".into());
        }
        let mut bytes = Vec::new();
        file.by_ref()
            .take(500 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "retained read")?;
        let identity = |m: &std::fs::Metadata| {
            (
                m.dev(),
                m.ino(),
                m.len(),
                m.mtime(),
                m.mtime_nsec(),
                m.ctime(),
                m.ctime_nsec(),
            )
        };
        if bytes.len() as u64 != before.len()
            || identity(&before) != identity(&file.metadata().map_err(|_| "retained metadata")?)
            || identity(&before)
                != identity(
                    &open_at(parent, name, false)?
                        .metadata()
                        .map_err(|_| "retained metadata")?,
                )
        {
            return Err("retained file changed".into());
        }
        Ok(bytes)
    };
    let copy_file = |parent: &std::fs::File,
                     name: &str,
                     destination: &Path|
     -> Result<magi_domain::Digest, String> {
        use std::io::Write;
        let bytes = read_file(parent, name)?;
        let digest = magi_domain::Digest::from_bytes(&bytes);
        let mut output = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(destination)
            .map_err(|_| "owned copy write")?;
        output.write_all(&bytes).map_err(|_| "owned copy write")?;
        Ok(digest)
    };
    for path in [
        &retained,
        &retained.join("state"),
        &retained.join("objects"),
    ] {
        let m = std::fs::symlink_metadata(path).map_err(|_| "retained root unavailable")?;
        if !m.is_dir() || m.file_type().is_symlink() || m.uid() != unsafe { libc::geteuid() } {
            return Err("retained root custody invalid".into());
        }
    }
    check_closed()?;
    std::fs::create_dir(&copy).map_err(|_| "owned copy root")?;
    let _copy_guard = OwnedPreparationPath {
        path: copy.clone(),
        keep: false,
    };
    std::fs::set_permissions(&copy, std::fs::Permissions::from_mode(0o700))
        .map_err(|_| "owned copy permissions")?;
    for name in ["state", "objects"] {
        std::fs::create_dir(copy.join(name)).map_err(|_| "owned copy directory")?;
        std::fs::set_permissions(copy.join(name), std::fs::Permissions::from_mode(0o700))
            .map_err(|_| "owned directory permissions")?;
    }
    let original_db = copy_file(
        &state_directory,
        "magi.sqlite",
        &copy.join("state/magi.sqlite"),
    )?;
    for prefix in names(&objects_directory)? {
        if prefix.len() != 2 || !prefix.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("object prefix invalid".into());
        }
        let object_directory = open_at(&objects_directory, &prefix, true)?;
        std::fs::create_dir(copy.join("objects").join(&prefix)).map_err(|_| "owned objects")?;
        for name in names(&object_directory)? {
            if name.len() != 62 || !name.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err("object name invalid".into());
            }
            let digest = copy_file(
                &object_directory,
                &name,
                &copy.join("objects").join(&prefix).join(&name),
            )?;
            if digest.as_str() != format!("{prefix}{name}") {
                return Err("retained object integrity mismatch".into());
            }
        }
    }
    check_closed()?;
    let original_after =
        magi_domain::Digest::from_bytes(&read_file(&state_directory, "magi.sqlite")?);
    if original_db != original_after {
        return Err("retained database changed".into());
    }
    let storage =
        magi_storage::Storage::open_or_create(&copy).map_err(|_| "owned copy migration failed")?;
    let mut backup_guard = OwnedPreparationPath {
        path: backup.clone(),
        keep: false,
    };
    let manifest = storage
        .create_backup(&backup)
        .map_err(|_| "coherent backup failed")?;
    std::fs::write(backup.join("preparation-proof.json"),serde_json::to_vec_pretty(&serde_json::json!({"sourceDatabaseDigest":original_db,"backup":manifest,"sourceWrites":0})).map_err(|_| "backup proof encoding")?).map_err(|_| "backup proof write")?;
    std::fs::set_permissions(
        backup.join("preparation-proof.json"),
        std::fs::Permissions::from_mode(0o600),
    )
    .map_err(|_| "proof permissions")?;
    backup_guard.keep = true;
    Ok(())
}

#[derive(Default)]
struct SavedDeliberationUiState {
    nonce: Mutex<Option<String>>,
    progress: Mutex<Option<SavedDeliberationUiPhase>>,
    outcome: Mutex<Option<Result<serde_json::Value, String>>>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum SavedDeliberationUiPhase {
    BridgeReady,
    DocumentReady,
    NewDeliberation,
    QuestionWritable,
    SourcesView,
    NativePicker,
    CaptureComplete,
    InputReview,
    ConsentReady,
    StartReady,
    StartReceipt,
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SavedDeliberationUiProgress {
    nonce: String,
    phase: SavedDeliberationUiPhase,
}
#[tauri::command]
fn saved_deliberation_ui_progress(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, SavedDeliberationUiState>,
    input: SavedDeliberationUiProgress,
) -> Result<(), String> {
    if window.label() != "main"
        || state.nonce.lock().map_err(|_| "UI nonce lock")?.as_deref() != Some(input.nonce.as_str())
        || state
            .outcome
            .lock()
            .map_err(|_| "UI outcome lock")?
            .is_some()
    {
        return Err("UI progress fenced".into());
    }
    let mut progress = state.progress.lock().map_err(|_| "UI progress lock")?;
    if progress.is_some_and(|prior| input.phase < prior) {
        return Err("UI progress cannot move backwards".into());
    }
    if *progress != Some(input.phase) {
        eprintln!("native actual UI phase: {:?}", input.phase);
        *progress = Some(input.phase);
    }
    Ok(())
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SavedDeliberationUiReport {
    nonce: String,
    failure: String,
}
#[tauri::command]
fn saved_deliberation_ui_report(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, SavedDeliberationUiState>,
    input: SavedDeliberationUiReport,
) -> Result<(), String> {
    if window.label() != "main"
        || state.nonce.lock().map_err(|_| "UI nonce lock")?.as_deref() != Some(input.nonce.as_str())
    {
        return Err("UI report fenced".into());
    }
    let mut outcome = state.outcome.lock().map_err(|_| "UI outcome lock")?;
    if outcome.is_some() {
        return Err("UI report duplicate".into());
    }
    *outcome = Some({
        let failure = input.failure;
        let category = match failure.as_str() {
            "physical_ui_bridge_ready"
            | "physical_ui_document_ready"
            | "physical_ui_new_deliberation"
            | "physical_ui_question_writable"
            | "physical_ui_sources_view"
            | "physical_ui_native_picker"
            | "physical_ui_capture_complete"
            | "physical_ui_input_review"
            | "physical_ui_consent_ready"
            | "physical_ui_start_ready"
            | "physical_ui_start_receipt" => failure.as_str(),
            _ => "unknown_ui_action",
        };
        Err(format!("actual UI action failed: {category}"))
    });
    Ok(())
}

#[tauri::command]
async fn start_deliberation(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    state: tauri::State<'_, commands::DesktopState>,
    input: profiles::StartDeliberationInputDto,
) -> Result<profiles::StartDeliberationReceiptDto, profiles::LiveRunError> {
    let request = serde_json::to_value(&input);
    let main_window = window.label() == "main";
    let observed_nonce = if main_window {
        let observer = app.state::<SavedDeliberationUiState>();
        observer.nonce.lock().ok().and_then(|nonce| nonce.clone())
    } else {
        None
    };
    let result = profiles::start_deliberation(window, app.clone(), state, input).await;
    if main_window && let Ok(receipt) = &result {
        let observer = app.state::<SavedDeliberationUiState>();
        if let (Ok(nonce), Ok(mut progress), Ok(mut outcome)) = (
            observer.nonce.lock(),
            observer.progress.lock(),
            observer.outcome.lock(),
        ) && observed_nonce.is_some()
            && *nonce == observed_nonce
            && *progress == Some(SavedDeliberationUiPhase::StartReady)
            && outcome.is_none()
        {
            *outcome = Some(match (request, serde_json::to_value(receipt)) {
                (Ok(mut request), Ok(receipt)) => {
                    if let Some(object) = request.as_object_mut() {
                        object.remove("admissionAuthority");
                    }
                    *progress = Some(SavedDeliberationUiPhase::StartReceipt);
                    eprintln!("native actual UI phase: StartReceipt");
                    Ok(serde_json::json!({"receipt":receipt,"request":request}))
                }
                _ => Err("actual UI receipt observation unavailable".into()),
            });
        }
    }
    result
}

#[cfg(test)]
mod saved_ui_report_contract_tests {
    use super::SavedDeliberationUiReport;

    #[test]
    fn renderer_can_report_failure_but_cannot_supply_execution_receipt() {
        assert!(
            serde_json::from_value::<SavedDeliberationUiReport>(
                serde_json::json!({"nonce":"observed","failure":"physical_ui_start_ready"})
            )
            .is_ok()
        );
        for forged in [
            serde_json::json!({"nonce":"observed","receipt":{"runId":"forged"},"request":{}}),
            serde_json::json!({"nonce":"observed","failure":"physical_ui_start_ready","receipt":{"runId":"forged"}}),
            serde_json::json!({"nonce":"observed","failure":null}),
        ] {
            assert!(serde_json::from_value::<SavedDeliberationUiReport>(forged).is_err());
        }
    }
}

fn physical_probe_bundle_identifier() -> Result<String, String> {
    let executable = std::env::current_exe()
        .and_then(|path| path.canonicalize())
        .map_err(|_| "physical probe executable unavailable")?;
    let macos = executable
        .parent()
        .ok_or("physical probe executable parent missing")?;
    let contents = macos.parent().ok_or("physical probe contents missing")?;
    let bundle = contents.parent().ok_or("physical probe bundle missing")?;
    if macos.file_name() != Some(std::ffi::OsStr::new("MacOS"))
        || contents.file_name() != Some(std::ffi::OsStr::new("Contents"))
        || bundle.extension() != Some(std::ffi::OsStr::new("app"))
    {
        return Err("physical probe requires its packaged application host".into());
    }
    read_physical_probe_identifier(&contents.join("Info.plist"))
}

fn read_physical_probe_identifier(path: &Path) -> Result<String, String> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|_| "physical probe bundle metadata unavailable")?;
    let metadata = file
        .metadata()
        .map_err(|_| "physical probe bundle metadata unavailable")?;
    if !metadata.is_file() || metadata.len() > 64 * 1024 {
        return Err("physical probe bundle metadata invalid".into());
    }
    let mut bytes = Vec::new();
    file.take(64 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "physical probe bundle metadata unreadable")?;
    parse_physical_probe_identifier(&bytes)
}

fn parse_physical_probe_identifier(bytes: &[u8]) -> Result<String, String> {
    if bytes.len() > 64 * 1024 {
        return Err("physical probe bundle metadata oversized".into());
    }
    let value = plist::Value::from_reader(std::io::Cursor::new(bytes))
        .map_err(|_| "physical probe bundle metadata malformed")?;
    let identifier = value
        .as_dictionary()
        .and_then(|dictionary| dictionary.get("CFBundleIdentifier"))
        .and_then(plist::Value::as_string)
        .ok_or("physical probe bundle identifier unavailable")?;
    if identifier != "local.magi.installed.probe" {
        return Err("physical probe bundle identity mismatch".into());
    }
    Ok(identifier.into())
}

#[cfg(test)]
mod physical_probe_identity_tests {
    use super::*;

    #[test]
    fn bounded_binary_and_xml_identity_fail_closed_for_invalid_or_nonregular_inputs() {
        struct OwnedRoot(PathBuf);
        impl Drop for OwnedRoot {
            fn drop(&mut self) {
                if let Err(error) = std::fs::remove_dir_all(&self.0) {
                    if std::thread::panicking() {
                        eprintln!("Owned bundle identity fixture cleanup failed: {error}");
                    } else {
                        panic!("Owned bundle identity fixture cleanup failed: {error}");
                    }
                }
            }
        }
        let path =
            std::env::temp_dir().join(format!("magi-bundle-identity-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap();
        let root = OwnedRoot(path);
        let path = root.0.join("Info.plist");
        let mut dictionary = plist::Dictionary::new();
        dictionary.insert(
            "CFBundleIdentifier".into(),
            plist::Value::String("local.magi.installed.probe".into()),
        );
        let value = plist::Value::Dictionary(dictionary);
        let mut binary = Vec::new();
        value.to_writer_binary(&mut binary).unwrap();
        let mut xml = Vec::new();
        value.to_writer_xml(&mut xml).unwrap();
        for bytes in [binary, xml] {
            std::fs::write(&path, bytes).unwrap();
            assert_eq!(
                read_physical_probe_identifier(&path).unwrap(),
                "local.magi.installed.probe"
            );
        }
        let mut wrong = plist::Dictionary::new();
        wrong.insert(
            "CFBundleIdentifier".into(),
            plist::Value::String("other.application".into()),
        );
        let mut bytes = Vec::new();
        plist::Value::Dictionary(wrong)
            .to_writer_xml(&mut bytes)
            .unwrap();
        assert!(parse_physical_probe_identifier(&bytes).is_err());
        let mut wrong_type = plist::Dictionary::new();
        wrong_type.insert("CFBundleIdentifier".into(), plist::Value::Boolean(true));
        let mut wrong_type_bytes = Vec::new();
        plist::Value::Dictionary(wrong_type)
            .to_writer_binary(&mut wrong_type_bytes)
            .unwrap();
        assert!(parse_physical_probe_identifier(&wrong_type_bytes).is_err());
        assert!(parse_physical_probe_identifier(b"malformed").is_err());
        assert!(parse_physical_probe_identifier(&vec![b'x'; 64 * 1024 + 1]).is_err());
        std::fs::write(&path, vec![b'x'; 64 * 1024 + 1]).unwrap();
        assert!(read_physical_probe_identifier(&path).is_err());
        assert!(read_physical_probe_identifier(&root.0).is_err());
        let link = root.0.join("symlink.plist");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(read_physical_probe_identifier(&link).is_err());
    }
}

fn prepare_saved_profile_run_store() -> Result<(), String> {
    let source = PathBuf::from(
        std::env::var_os("MAGI_TEST_SAVED_PROFILE_SOURCE_ROOT")
            .ok_or("saved source root required")?,
    );
    let backup = PathBuf::from(
        std::env::var_os("MAGI_TEST_SAVED_PROFILE_BACKUP_ROOT")
            .ok_or("saved backup root required")?,
    );
    let destination =
        PathBuf::from(std::env::var_os("MAGI_TEST_DATA_ROOT").ok_or("saved destination required")?);
    let input_path = std::env::var_os("MAGI_TEST_SAVED_PROFILE_BINDINGS_FILE")
        .ok_or("saved bindings required")?;
    let expected_store = std::env::var("MAGI_TEST_SAVED_PROFILE_SOURCE_STORE_ID")
        .map_err(|_| "saved source identity required")?;
    prepare_saved_profile_run_store_at(
        &source,
        &backup,
        &destination,
        Path::new(&input_path),
        &expected_store,
    )
}

fn prepare_saved_profile_run_store_at(
    fence_source: &Path,
    backup: &Path,
    destination: &Path,
    input_path: &Path,
    expected_store: &str,
) -> Result<(), String> {
    let input = read_saved_profile_input(input_path)?;
    if fence_source.exists() || !backup.is_dir() || destination.exists() {
        return Err("fresh fence source, preserved backup, and fresh destination required".into());
    }
    let manifest: magi_storage::BackupManifest = serde_json::from_slice(
        &std::fs::read(backup.join("manifest.json"))
            .map_err(|_| "saved backup manifest unavailable")?,
    )
    .map_err(|_| "saved backup manifest invalid")?;
    if manifest.store_id != expected_store {
        return Err("saved backup identity mismatch".into());
    }
    let storage = magi_storage::Storage::open_or_create(fence_source)
        .map_err(|_| "saved fence writer authority unavailable")?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    let (mut source, permission) =
        tauri::async_runtime::block_on(retained_saved_preparation(storage, deadline))
            .map_err(|_| "saved fence native closure unavailable")?;
    let receipt = magi_storage::Storage::restore_backup(backup, destination)
        .map_err(|_| "saved backup restore failed")?;
    if receipt.store_id != expected_store || receipt.store_id != manifest.store_id {
        return Err("saved restored identity mismatch".into());
    }
    let mut restored = magi_storage::Storage::open_or_create(destination)
        .map_err(|_| "saved restored authority unavailable")?;
    let (active, retained_permission) = restored
        .activate_restored_execution_checked(&mut source, |prior| {
            magi_storage::AdmissionActivationPermission::validate(&permission, prior)?;
            Ok(permission)
        })
        .map_err(|_| "saved restored activation denied")?;
    if !active.active {
        return Err("saved restored activation inactive".into());
    }
    let bindings = derive_saved_profile_bindings(&restored, &input)?;
    drop(restored);
    drop(source);
    drop(retained_permission);
    let prepared_input = SavedProfileInput {
        schema_version: input.schema_version,
        prepared_store_digest: saved_store_digest(destination)?,
        bindings,
    };
    let bindings_path = destination.join("saved-profile-bindings.json");
    write_saved_profile_input(&bindings_path, &prepared_input)?;
    std::fs::write(
        destination.join("saved-profile-preparation.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "schemaVersion": 1,
            "backup": manifest,
            "restore": receipt,
            "preparedStoreDigest": prepared_input.prepared_store_digest,
            "bindings": prepared_input.bindings,
            "bindingsFile": "saved-profile-bindings.json"
        }))
        .map_err(|_| "saved preparation encoding")?,
    )
    .map_err(|_| "saved preparation publication")?;
    Ok(())
}

pub(crate) fn run() {
    if std::env::args().nth(1).as_deref() == Some("--prepare-retained-saved-profile-backup") {
        assert_eq!(
            std::env::args().count(),
            2,
            "closed backup preparation arguments required"
        );
        prepare_retained_saved_profile_backup().expect("retained backup preparation failed");
        return;
    }
    if std::env::args().nth(1).as_deref() == Some("--prepare-saved-profile-run-store") {
        assert_eq!(
            std::env::args().count(),
            2,
            "closed saved preparation arguments required"
        );
        prepare_saved_profile_run_store().expect("saved profile preparation failed");
        return;
    }
    if std::env::args().nth(1).as_deref() == Some("--prepare-connections-ui-store") {
        assert_eq!(
            std::env::args().count(),
            2,
            "closed preparation arguments required"
        );
        prepare_connections_ui_store().expect("owned metadata fixture preparation failed");
        return;
    }
    let configured_purpose = match std::env::var("MAGI_TEST_PROBE_PURPOSE") {
        Ok(value) => Some(value),
        Err(std::env::VarError::NotPresent) => None,
        Err(std::env::VarError::NotUnicode(_)) => panic!("probe purpose must be valid UTF-8"),
    };
    let purpose =
        ProbePurpose::parse(configured_purpose.as_deref()).expect("invalid probe purpose");
    let root = PathBuf::from(
        std::env::var_os("MAGI_TEST_DATA_ROOT").expect("explicit disposable data root required"),
    );
    let saved_input = if purpose == ProbePurpose::SavedProfileDeliberation {
        let path = std::env::var_os("MAGI_TEST_SAVED_PROFILE_BINDINGS_FILE")
            .expect("explicit saved profile bindings required");
        Some(
            read_saved_profile_input(std::path::Path::new(&path))
                .expect("invalid saved profile bindings"),
        )
    } else {
        None
    };
    let recovery_run = if purpose == ProbePurpose::RecoveryCancel {
        let run = std::env::var("MAGI_TEST_CANCEL_RUN_ID").expect("explicit recovery run required");
        assert!(valid_recovery_run(&run), "invalid recovery run reference");
        Some(run)
    } else {
        None
    };
    if purpose == ProbePurpose::Deliberation {
        let model = std::env::var("MAGI_TEST_MODEL_ID").expect("explicit observed model required");
        let mode = std::env::var("MAGI_TEST_MODE_ID")
            .expect("explicit observed mode or explicit empty absent facility required");
        let source_digest = std::env::var("MAGI_TEST_POLICY_SOURCE_DIGEST")
            .expect("explicit approved canonical source digest required");
        assert!(
            source_digest.len() == 64 && source_digest.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "valid approved canonical source digest required"
        );
        assert!(
            !model.is_empty()
                && model.len() <= 256
                && mode.len() <= 128
                && !model.chars().chain(mode.chars()).any(char::is_control),
            "valid explicit observed selection required"
        );
    }
    if purpose == ProbePurpose::SavedProfileDeliberation {
        let digest = std::env::var("MAGI_TEST_POLICY_SOURCE_DIGEST")
            .expect("approved policy source digest required");
        assert!(
            digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "invalid policy source digest"
        );
    }
    let selected = if matches!(
        purpose,
        ProbePurpose::PdfUi
            | ProbePurpose::RecoveryCancel
            | ProbePurpose::ConnectionsUi
            | ProbePurpose::SavedProfileDeliberation
    ) {
        String::new()
    } else {
        std::env::var("MAGI_TEST_CREDENTIAL_HOME").expect("explicit selected authority required")
    };
    if matches!(
        purpose,
        ProbePurpose::RecoveryCancel
            | ProbePurpose::ConnectionsUi
            | ProbePurpose::SavedProfileDeliberation
    ) {
        assert!(
            root.is_dir() && root.join("state/magi.sqlite").is_file(),
            "explicit existing owned store required"
        );
    } else {
        assert!(!root.exists(), "probe data root must be fresh");
        std::fs::create_dir_all(&root).expect("create disposable data root");
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))
            .expect("private disposable data root");
    }
    if matches!(
        purpose,
        ProbePurpose::ConnectionsUi | ProbePurpose::SavedProfileDeliberation
    ) {
        use std::io::Read;
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
        let metadata = std::fs::symlink_metadata(&root).expect("UI root metadata");
        assert!(
            metadata.is_dir()
                && metadata.mode() & 0o777 == 0o700
                && metadata.uid() == unsafe { libc::geteuid() },
            "private owned UI root required"
        );
        let expected = match &saved_input {
            Some(input) => input.prepared_store_digest.clone(),
            None => std::env::var("MAGI_TEST_UI_STORE_DIGEST")
                .expect("explicit prepared UI store digest required"),
        };
        assert!(
            expected.len() == 64 && expected.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "valid UI store digest required"
        );
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(root.join("state/magi.sqlite"))
            .expect("prepared UI store");
        let meta = file.metadata().expect("UI store metadata");
        assert!(
            meta.is_file()
                && meta.nlink() == 1
                && meta.uid() == unsafe { libc::geteuid() }
                && meta.len() <= 20 * 1024 * 1024,
            "bounded owned UI store required"
        );
        let mut bytes = Vec::new();
        file.take(20 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .expect("UI store read");
        assert_eq!(
            magi_domain::Digest::from_bytes(&bytes).as_str(),
            expected,
            "prepared UI store authority mismatch"
        );
        assert!(
            !root.join("state/magi.sqlite-wal").exists()
                && !root.join("state/magi.sqlite-shm").exists(),
            "coherent closed UI store required"
        );
    }
    let root = root.canonicalize().expect("canonical disposable data root");
    let mut context = crate::desktop::product_context();
    if purpose == ProbePurpose::RecoveryCancel {
        assert!(
            std::env::var_os("MAGI_TEST_FRONTEND_MODE").is_none(),
            "inert recovery refuses frontend mode"
        );
        context.config_mut().build.dev_url = None;
        for window in &mut context.config_mut().app.windows {
            window.url =
                tauri::WebviewUrl::External("about:blank".parse().expect("inert recovery URL"));
            window.visible = false;
        }
    } else if cfg!(debug_assertions) || std::env::var_os("MAGI_TEST_FRONTEND_MODE").is_some() {
        let mode = std::env::var_os("MAGI_TEST_FRONTEND_MODE");
        let frontend = std::env::var_os("MAGI_TEST_FRONTEND_URL");
        let has_index = context
            .assets()
            .get(&"index.html".into())
            .is_some_and(|bytes| !bytes.is_empty());
        match ProbeFrontendMode::select_os(mode.as_deref(), frontend.as_deref(), has_index)
            .expect("invalid explicit probe frontend authority")
        {
            ProbeFrontendMode::DebugServer => {
                context.config_mut().build.dev_url = Some(
                    "http://127.0.0.1:1427"
                        .parse()
                        .expect("dedicated frontend URL"),
                );
            }
            ProbeFrontendMode::EmbeddedAssets => {
                context.config_mut().build.dev_url = None;
                for window in &mut context.config_mut().app.windows {
                    window.url = tauri::WebviewUrl::App("index.html".into());
                }
            }
        }
    }
    context.config_mut().identifier = if std::env::var("MAGI_TEST_ACTUAL_DOM_START").ok().as_deref()
        == Some("1")
    {
        assert_eq!(
            purpose,
            ProbePurpose::SavedProfileDeliberation,
            "physical start requires saved-profile purpose"
        );
        physical_probe_bundle_identifier().expect("physical probe packaged host identity required")
    } else {
        format!("local.magi.probe.p{}", std::process::id())
    };
    context.config_mut().app.app_directories_override = Some(
        tauri::utils::config::AppDirectoriesOverride::Root(root.clone()),
    );
    let monitor = Arc::new(ProbeMonitor {
        outcome: AtomicU8::new(0),
        phase: Mutex::new(("initializing", Instant::now())),
        active_request: Mutex::new(None),
        started: Instant::now(),
        lifecycle: commands::AdmissionRequestLifecycle::until(
            Instant::now() + Duration::from_secs(300),
        ),
    });
    let setup_monitor = monitor.clone();
    let app = tauri::Builder::default()
        .plugin(
            tauri_plugin_updater::Builder::new()
                .pubkey(crate::release::updater_public_key())
                .build(),
        )
        .plugin(tauri_plugin_dialog::init())
        .on_page_load(|webview, payload| {
            if webview.label() == "main" {
                let url = payload.url();
                let owned_debug_origin = url.scheme() == "http"
                    && url.host_str() == Some("127.0.0.1")
                    && url.port_or_known_default() == Some(1427);
                eprintln!(
                    "native main page load: event={:?}; owned_debug_origin={owned_debug_origin}",
                    payload.event()
                );
            }
        })
        .invoke_handler({
            let start_handler: fn(tauri::ipc::Invoke<tauri::Wry>) -> bool =
                tauri::generate_handler![start_deliberation];
            let product_handler: fn(tauri::ipc::Invoke<tauri::Wry>) -> bool =
                crate::desktop::product_invoke_handler!(
                    get_console_snapshot,
                    pdf_ui_probe_report,
                    connections_ui_probe_report,
                    saved_deliberation_ui_progress,
                    saved_deliberation_ui_report,
                );
            move |invoke: tauri::ipc::Invoke<tauri::Wry>| {
                if invoke.message.command() == "start_deliberation" {
                    start_handler(invoke)
                } else {
                    product_handler(invoke)
                }
            }
        })
        .setup(move |app| {
            setup_monitor.mark("setup");
            app.manage(saved_input.clone());
            app.manage(commands::ExitAuthorization::new());
            app.manage(
                if matches!(
                    purpose,
                    ProbePurpose::RecoveryCancel | ProbePurpose::ConnectionsUi
                ) {
                    commands::DesktopState::open_quiet(app.handle())
                } else {
                    commands::DesktopState::open(app.handle())
                },
            );
            app.manage(PdfUiProbeState::default());
            app.manage(SavedDeliberationUiState::default());
            app.manage(ConnectionsUiState {
                enabled: purpose == ProbePurpose::ConnectionsUi,
                ..ConnectionsUiState::default()
            });
            if purpose == ProbePurpose::ConnectionsUi {
                let desktop = app.state::<commands::DesktopState>();
                let storage = desktop.storage().map_err(std::io::Error::other)?;
                for core in magi_domain::CoreId::ALL {
                    if storage.load_core_model_selection(core)?.is_some() {
                        return Err(std::io::Error::other(
                            "UI-only store must not contain execution core selections",
                        )
                        .into());
                    }
                }
                if storage.list_provider_profiles(100)?.is_empty() {
                    return Err(std::io::Error::other(
                        "UI-only store requires actual profile metadata",
                    )
                    .into());
                }
            }
            let handle = app.handle().clone();
            let selected = selected.clone();
            let root = root.clone();
            if purpose.permits_admission() {
                start_probe_control_bridge(handle.clone(), setup_monitor.clone(), root.clone())?;
            }
            let watchdog_root = root.clone();
            let watchdog_handle = handle.clone();
            let watchdog_monitor = setup_monitor.clone();
            std::thread::spawn(move || {
                let started = Instant::now();
                loop {
                    std::thread::sleep(Duration::from_secs(1));
                    if watchdog_monitor.outcome.load(Ordering::Acquire) != 0 {
                        break;
                    }
                    let (phase, since) = *watchdog_monitor.phase.lock().unwrap();
                    let phase_limit = if phase == "dispatching" { 1800 } else { 300 };
                    if since.elapsed() > Duration::from_secs(phase_limit)
                        || started.elapsed() > Duration::from_secs(2100)
                    {
                        if watchdog_monitor
                            .outcome
                            .compare_exchange(0, 2, Ordering::AcqRel, Ordering::Acquire)
                            .is_ok()
                        {
                            eprintln!("native live probe failed: bounded phase deadline ({phase})");
                            let cleanup = watchdog_monitor.revoke_for_timeout();
                            let cancelled =
                                cancel_probe_request(&watchdog_handle, &watchdog_monitor, cleanup);
                            let _ = std::fs::write(
                                watchdog_root.join("deadline-cancellation.json"),
                                serde_json::to_vec_pretty(&cancelled).unwrap_or_default(),
                            );
                            tauri::async_runtime::block_on(
                                watchdog_monitor
                                    .record_application_shutdown(&watchdog_handle, &watchdog_root),
                            );
                            watchdog_handle.exit(1);
                        }
                        break;
                    }
                }
            });
            let task_monitor = setup_monitor.clone();
            tauri::async_runtime::spawn(async move {
                let result = probe(
                    &handle,
                    selected,
                    root.clone(),
                    purpose,
                    recovery_run,
                    &task_monitor,
                )
                .await;
                let settled = task_monitor
                    .record_application_shutdown(&handle, &root)
                    .await;
                match result {
                    Ok(()) if settled => {
                        if task_monitor
                            .outcome
                            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
                            .is_ok()
                        {
                            handle.exit(0);
                        }
                    }
                    Ok(()) => {
                        task_monitor.outcome.store(2, Ordering::Release);
                        eprintln!(
                            "native live probe failed: terminal settlement remains unresolved"
                        );
                        handle.exit(1);
                    }
                    Err(error) => {
                        task_monitor.outcome.store(2, Ordering::Release);
                        eprintln!("native live probe failed: {error}");
                        handle.exit(1);
                    }
                }
            });
            Ok(())
        })
        .build(context)
        .expect("real native application initialization");
    let exit_code = app.run_return(|app, event| {
        if let tauri::RunEvent::ExitRequested {
            code: None, api, ..
        } = event
            && !app.state::<commands::ExitAuthorization>().is_authorized()
        {
            api.prevent_exit();
        }
    });
    if monitor.outcome.load(Ordering::Acquire) != 1 || exit_code != 0 {
        eprintln!("native live probe failed: event loop ended without verified completion");
        std::process::exit(1);
    }
}

async fn probe(
    app: &tauri::AppHandle,
    selected: String,
    root: PathBuf,
    purpose: ProbePurpose,
    recovery_run: Option<String>,
    monitor: &ProbeMonitor,
) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or("production main window missing")?;
    if purpose == ProbePurpose::RecoveryCancel {
        monitor.mark("recovery_cancelling");
        let run_id = recovery_run.ok_or("explicit recovery run missing")?;
        let storage = app.state::<commands::DesktopState>().storage()?;
        let before = storage
            .get_live_run_snapshot(&run_id, 0)
            .map_err(|_| "recovery authority snapshot")?;
        let before_dispatches = storage
            .load_live_dispatch_projection(&run_id)
            .map_err(|_| "recovery authority dispatch projection")?;
        std::fs::write(root.join("recovery-before-cancellation.json"), serde_json::to_vec_pretty(&serde_json::json!({"runId":run_id,"status":before.status,"revision":before.revision,"cancellation":before.cancellation,"dispatches":before_dispatches})).map_err(|_| "recovery authority encoding")?).map_err(|_| "recovery authority publication")?;
        let cancellation_result =
            profiles::cancel_deliberation(window, app.clone(), app.state(), run_id.clone()).await;
        let cancellation_accepted = cancellation_result.is_ok();
        let storage = app.state::<commands::DesktopState>().storage()?;
        let live = storage
            .get_live_run_snapshot(&run_id, 0)
            .map_err(|_| "recovered cancellation snapshot")?;
        let dispatches = storage
            .load_live_dispatch_projection(&run_id)
            .map_err(|_| "recovered dispatch projection")?;
        std::fs::write(root.join("recovery-cancellation.json"), serde_json::to_vec_pretty(&serde_json::json!({"purpose":"recovery-cancel","cancellationAccepted":cancellation_accepted,"errorCategory":if cancellation_accepted {None} else {Some("production_recovery_cancellation_rejected")},"runId":run_id,"status":live.status,"revision":live.revision,"cancellation":live.cancellation,"dispatches":dispatches,"inferenceAcceptance":false,"renderedUiAcceptance":false})).map_err(|_| "recovery receipt encoding")?).map_err(|_| "recovery receipt publication")?;
        if !cancellation_accepted {
            return Err("production recovery cancellation rejected; post-state preserved".into());
        }
        if live.cancellation.is_none() {
            return Err("durable cancellation receipt missing; post-state preserved".into());
        }
        monitor.mark("recovery_cancelled_or_interrupted");
        return Ok(());
    }
    if purpose == ProbePurpose::PdfUi {
        return pdf_ui_probe(app, &window, &root, monitor).await;
    }
    if purpose == ProbePurpose::ConnectionsUi {
        return connections_ui_probe(app, &window, &root, monitor).await;
    }
    let mut saved_input = if purpose == ProbePurpose::SavedProfileDeliberation {
        Some(
            app.state::<Option<SavedProfileInput>>()
                .inner()
                .clone()
                .ok_or("saved profile bindings missing")?,
        )
    } else {
        None
    };
    if let Some(input) = &saved_input {
        validate_saved_profile_records(
            app.state::<commands::DesktopState>().storage()?.as_ref(),
            input,
        )?;
    }
    if saved_input.is_some() {
        monitor
            .lifecycle
            .bind_execution(app.state::<commands::DesktopState>().storage()?)
            .map_err(|_| "saved profile execution authority unavailable")?;
    }
    let mut core_bindings = Vec::new();
    let mut observed_selections = Vec::new();
    monitor.ensure_running()?;
    let _probe_operation = monitor
        .lifecycle
        .lease()
        .map_err(|_| "supervised probe operation revoked")?;
    for (index, core_id) in magi_domain::CoreId::ALL
        .into_iter()
        .enumerate()
        .take(purpose.profile_count())
    {
        monitor.ensure_running()?;
        let saved_binding = saved_input
            .as_ref()
            .map(|input| input.binding(core_id).cloned())
            .transpose()?;
        let (profile_id, revision) = if let Some(binding) = &saved_binding {
            validate_saved_profile_records(
                app.state::<commands::DesktopState>().storage()?.as_ref(),
                saved_input.as_ref().ok_or("saved input missing")?,
            )?;
            (
                binding.provider_profile_id.clone(),
                binding.profile_revision,
            )
        } else {
            let draft = serde_json::from_value(serde_json::json!({"credentialHomePath":selected,"profileId":null,"expectedRevision":null,"displayName":format!("Subscription Review {}", index + 1),"adapterId":"codex-acp"})).map_err(|_| "profile draft encoding")?;
            monitor.mark("saving_profile");
            let profile =
                profiles::save_provider_profile(window.clone(), app.clone(), app.state(), draft)
                    .await?;
            monitor.ensure_running()?;
            let profile = serde_json::to_value(profile).map_err(|_| "profile receipt encoding")?;
            let profile_id = profile["providerProfileId"]
                .as_str()
                .ok_or("profile id missing")?
                .to_owned();
            let revision = profile["revision"]
                .as_u64()
                .ok_or("profile revision missing")?;
            (profile_id, revision)
        };
        monitor.mark("reading_catalog");
        let catalog = profiles::refresh_provider_model_catalog_inner(
            window.clone(),
            app.clone(),
            app.state(),
            profile_id.clone(),
            revision,
            Some(&monitor.lifecycle),
        )
        .await
        .map_err(|e| format!("catalog: {e:?}"))?;
        monitor.ensure_running()?;
        if catalog.schema_version != magi_domain::ARTIFACT_ATTESTED_CATALOG_SCHEMA_VERSION
            || catalog.artifact_set_digest.is_none()
        {
            return Err("catalog lacks verified artifact set authority".into());
        }
        if purpose == ProbePurpose::CatalogDiagnostic {
            std::fs::write(root.join("catalog-diagnostic.json"), serde_json::to_vec_pretty(&serde_json::json!({"purpose":"catalog-diagnostic","providerProfileId":profile_id,"profileRevision":revision,"catalog":catalog})).map_err(|_| "catalog diagnostic encoding")?).map_err(|_| "catalog diagnostic output")?;
            monitor.mark("catalog_diagnostic_completed");
            return Ok(());
        }
        let requested = match &saved_binding {
            Some(binding) => binding.model_id.clone(),
            None => std::env::var("MAGI_TEST_MODEL_ID")
                .map_err(|_| "explicit observed model required")?,
        };
        let model = catalog
            .models
            .iter()
            .find(|model| model.model_id == requested)
            .ok_or("explicit model is not in actual catalog")?;
        let facility = catalog
            .negotiated_modes
            .as_ref()
            .ok_or("actual mode evidence missing")?;
        let requested_mode = match &saved_binding {
            Some(binding) => binding.mode_id.clone().unwrap_or_default(),
            None => {
                std::env::var("MAGI_TEST_MODE_ID").map_err(|_| "explicit observed mode required")?
            }
        };
        if let Some(input) = &saved_input {
            validate_saved_profile_records(
                app.state::<commands::DesktopState>().storage()?.as_ref(),
                input,
            )?;
        }
        let mode_id = if facility.modes.is_empty() {
            if !requested_mode.is_empty() {
                return Err("mode requested for attested absent facility".into());
            }
            None
        } else {
            Some(
                facility
                    .modes
                    .iter()
                    .find(|mode| mode.mode_id == requested_mode)
                    .ok_or("explicit mode is not in actual catalog")?
                    .mode_id
                    .clone(),
            )
        };
        let selection_input = serde_json::from_value(serde_json::json!({"providerProfileId":profile_id,"profileRevision":revision,"catalogSnapshotId":catalog.catalog_snapshot_id,"catalogDigest":catalog.catalog_digest,"modelId":model.model_id,"modeId":mode_id,"expectedSelectionRevision":saved_binding.as_ref().map(|binding| binding.model_selection_revision)})).map_err(|_| "model selection input encoding")?;
        let selected_model =
            profiles::select_provider_model(window.clone(), app.state(), selection_input)
                .map_err(|_| "explicit provider model selection")?;
        let selection_input = serde_json::from_value(serde_json::json!({"coreId":core_id,"providerProfileId":profile_id,"profileRevision":revision,"modelSelectionRevision":selected_model.selection_revision,"expectedSelectionRevision":saved_binding.as_ref().and_then(|binding| binding.core_selection_revision)})).map_err(|_| "core selection input encoding")?;
        let selected_core =
            profiles::select_core_model(window.clone(), app.state(), selection_input)
                .map_err(|_| "independent core selection")?;
        if let Some(input) = &mut saved_input {
            let binding = input
                .bindings
                .iter_mut()
                .find(|binding| binding.core_id == core_id)
                .ok_or("saved core missing")?;
            binding.model_profile_revision = revision;
            binding.model_selection_revision = selected_model.selection_revision;
            binding.core_selection_revision = Some(selected_core.selection_revision);
        }
        observed_selections.push(serde_json::json!({"coreId":core_id,"providerProfileId":profile_id,"catalogSnapshotId":catalog.catalog_snapshot_id,"artifactSetDigest":catalog.artifact_set_digest,"acpExecutableSha256":catalog.adapter_digest,"modelId":model.model_id,"modeId":mode_id,"modelPolicy":if saved_binding.is_some() {"durable_saved_member"} else {"explicit_environment_member"},"modePolicy":if saved_binding.is_some() {"durable_saved_member"} else {"explicit_environment_member"}}));
        core_bindings.push(magi_storage::CoreBindingReference {
            core_id,
            provider_profile_id: profile_id,
            profile_revision: revision,
            model_selection_revision: selected_model.selection_revision,
            core_selection_revision: selected_core.selection_revision,
        });
    }
    if core_bindings
        .iter()
        .map(|binding| &binding.provider_profile_id)
        .collect::<std::collections::BTreeSet<_>>()
        .len()
        != 3
    {
        return Err("distinct profile routing required".into());
    }
    std::fs::write(
        root.join("observed-selections.json"),
        serde_json::to_vec_pretty(&observed_selections)
            .map_err(|_| "observed selection encoding")?,
    )
    .map_err(|_| "observed selection output")?;
    if !purpose.permits_admission() {
        return Err("catalog diagnostic cannot admit a deliberation".into());
    }
    let witnesses = profiles::load_core_execution_witnesses(
        window.clone(),
        app.state(),
        serde_json::from_value(serde_json::json!({"coreBindings":core_bindings}))
            .map_err(|_| "witness input encoding")?,
    )
    .map_err(|_| "fresh execution witness unavailable")?;
    std::fs::write(
        root.join("core-execution-witnesses.json"),
        serde_json::to_vec_pretty(&witnesses).map_err(|_| "witness output encoding")?,
    )
    .map_err(|_| "witness output")?;
    let storage = app.state::<commands::DesktopState>().storage()?;
    let role = storage
        .list_role_presets(100)
        .map_err(|_| "role list")?
        .into_iter()
        .next()
        .ok_or("factory role missing")?;
    let command_id = uuid::Uuid::new_v4().to_string();
    monitor.ensure_running()?;
    monitor.mark("capturing_sources");
    let draft_id = uuid::Uuid::new_v4().to_string();
    let capture_storage = storage.clone();
    let capture_draft = draft_id.clone();
    let expected_source_digest = std::env::var("MAGI_TEST_POLICY_SOURCE_DIGEST")
        .map_err(|_| "explicit approved canonical source digest required")?;
    if expected_source_digest.len() != 64
        || !expected_source_digest
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("invalid approved canonical source digest".into());
    }
    let capture_lease = monitor
        .lifecycle
        .lease()
        .map_err(|_| "source capture revoked")?;
    let capture_operation = app
        .state::<commands::DesktopState>()
        .resource_coordinator
        .enter(Some(monitor.lifecycle.verification.clone()))
        .map_err(|_| "source capture resources frozen")?;
    let approved_digest = expected_source_digest.clone();
    let resource_root = app
        .path()
        .resource_dir()
        .map_err(|_| "source capture resources")?;
    let capture_digest = expected_source_digest.clone();
    let captured = tauri::async_runtime::spawn_blocking(move || {
        let _capture_lease = capture_lease;
        let _capture_operation = capture_operation;
        _capture_operation
            .check()
            .map_err(|_| "source capture resources frozen".to_owned())?;
        let approved_source =
            verified_policy_resource(&resource_root, &capture_digest).map_err(str::to_owned)?;
        commands::capture_context_files(
            capture_storage,
            capture_draft,
            None,
            vec![approved_source],
            &resource_root,
        )
    })
    .await
    .map_err(|_| "source capture worker")??;
    monitor.ensure_running()?;
    let captured = serde_json::to_value(captured).map_err(|_| "source capture summary")?;
    if captured["sources"].as_array().map(Vec::len) != Some(1)
        || captured["sources"][0]["status"] != "captured"
        || captured["sources"][0]["digest"].as_str() != Some(approved_digest.as_str())
    {
        return Err("the approved canonical source was not fully captured".into());
    }
    let context_revision = captured["revision"]
        .as_u64()
        .ok_or("source capture revision")?;
    let draft = storage
        .load_context_draft(&draft_id)
        .map_err(|_| "source capture durable read")?
        .ok_or("source capture durable draft missing")?;
    let source = draft
        .manifest
        .content
        .sources
        .first()
        .ok_or("source capture durable entry missing")?;
    if draft.revision != context_revision
        || draft.manifest.content.sources.len() != 1
        || source
            .object_digest
            .as_ref()
            .map(magi_domain::Digest::as_str)
            != captured["sources"][0]["digest"].as_str()
        || source.included_locators.is_empty()
    {
        return Err("source capture durable authority mismatch".into());
    }
    let capture_evidence = serde_json::json!({"capture":captured,"canonicalSourceDigest":source.object_digest,"manifestDigest":draft.manifest.digest,"includedLocators":source.included_locators});
    std::fs::write(
        root.join("approved-policy-source-capture.json"),
        serde_json::to_vec_pretty(&capture_evidence)
            .map_err(|_| "source capture evidence encoding")?,
    )
    .map_err(|_| "source capture evidence")?;
    let question = "Using only the explicitly captured product security contract, produce the confirmation checklist for this desktop product before a saved subscription profile sends an approved source representation to its selected external provider. Identify the fields that must be disclosed and the changes that require reconfirmation. The captured contract already defines the decision scope: do not request additional user data, do not infer facts outside the source, and record unknown provider retention, training, deletion, or cancellation behavior as a non-essential limitation of the policy. Return a complete policy decision grounded in the approved source so the review can proceed to cross-review, synthesis, and private ballots.";
    let physical = std::env::var("MAGI_TEST_ACTUAL_DOM_START").ok().as_deref() == Some("1");
    if physical && purpose != ProbePurpose::SavedProfileDeliberation {
        return Err("physical start requires saved-profile purpose".into());
    }
    let receipt = if physical {
        commands::focus_main_window(app).map_err(|_| "physical console restoration failed")?;
        if !window
            .is_visible()
            .map_err(|_| "physical console visibility unavailable")?
        {
            return Err("physical console is not visible".into());
        }
        let state = app.state::<SavedDeliberationUiState>();
        let nonce = uuid::Uuid::new_v4().simple().to_string();
        *state.nonce.lock().map_err(|_| "UI nonce lock")? = Some(nonce.clone());
        let config = serde_json::to_string(&serde_json::json!({"nonce":nonce,"question":question}))
            .map_err(|_| "UI config encoding")?;
        window
            .eval(format!(
                "window.__MAGI_SAVED_UI_CONFIG={config};\n{}",
                include_str!("../native/saved-deliberation-ui-probe.js")
            ))
            .map_err(|_| "UI script")?;
        monitor.mark("physical_ui_confirmation");
        let deadline = Instant::now() + Duration::from_secs(300);
        let observed = loop {
            monitor.ensure_running()?;
            if let Some(outcome) = state.outcome.lock().map_err(|_| "UI outcome lock")?.take() {
                break outcome?;
            }
            if Instant::now() >= deadline {
                return Err("actual UI start deadline".into());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        };
        *state.nonce.lock().map_err(|_| "UI nonce lock")? = None;
        let request = observed["request"].clone();
        if request["question"] != question
            || request["coreBindings"]
                != serde_json::to_value(&core_bindings).map_err(|_| "UI binding encoding")?
            || request["disclosureConfirmed"] != true
        {
            return Err("actual UI admitted input differs from reviewed saved selections".into());
        }
        let actual_draft = storage
            .load_context_draft(
                request["contextDraftId"]
                    .as_str()
                    .ok_or("UI capture identity missing")?,
            )
            .map_err(|_| "UI capture reload")?
            .ok_or("UI capture missing")?;
        if Some(actual_draft.revision) != request["contextRevision"].as_u64()
            || actual_draft.manifest.content.sources.len() != 1
            || actual_draft.manifest.content.sources[0]
                .object_digest
                .as_ref()
                .map(magi_domain::Digest::as_str)
                != Some(expected_source_digest.as_str())
        {
            return Err("actual UI source differs from approved policy".into());
        }
        *monitor.active_request.lock().map_err(|_|"request reference lock")?=Some(serde_json::from_value(serde_json::json!({"request":request,"commandId":uuid::Uuid::new_v4().to_string(),"idempotencyKey":uuid::Uuid::new_v4().to_string()})).map_err(|_|"UI cancellation intent")?);
        observed["receipt"].clone()
    } else {
        let mut request = serde_json::json!({"commandId":command_id,"idempotencyKey":command_id,"question":question,"contextDraftId":draft_id,"contextRevision":context_revision,"coreBindings":core_bindings,"rolePresetId":role.preset_id,"roleRevision":role.revision,"disclosureConfirmed":true});
        monitor.ensure_running()?;
        *monitor.active_request.lock().map_err(|_| "request reference lock")? = Some(serde_json::from_value(serde_json::json!({"request":request,"commandId":uuid::Uuid::new_v4().to_string(),"idempotencyKey":uuid::Uuid::new_v4().to_string()})).map_err(|_| "cancellation intent encoding")?);
        let registration = profiles::register_deliberation_request_inner(
            window.clone(),
            app.clone(),
            app.state(),
            serde_json::from_value(request.clone()).map_err(|_| "registration request encoding")?,
            Some(&monitor.lifecycle),
        )
        .await
        .map_err(|_| "request registration")?;
        monitor.ensure_running()?;
        let registration = serde_json::to_value(registration).map_err(|_| "registration output")?;
        if registration["kind"] != "registered" {
            return Err("fresh probe request unexpectedly replayed".into());
        }
        let authority = &registration["admissionAuthority"];
        request["admissionAuthority"] = serde_json::json!({"token":authority["token"],"processEpoch":authority["processEpoch"]});
        let input =
            serde_json::from_value(request.clone()).map_err(|_| "deliberation input encoding")?;
        request
            .as_object_mut()
            .ok_or("request object")?
            .remove("admissionAuthority");
        if let Some(reference) = monitor
            .active_request
            .lock()
            .map_err(|_| "request reference lock")?
            .as_mut()
        {
            reference.admission_authority = Some(serde_json::from_value(serde_json::json!({"token":authority["token"],"processEpoch":authority["processEpoch"]})).map_err(|_| "cancellation authority encoding")?);
        }
        monitor.ensure_running()?;
        monitor.mark_verified(app, "admitting", &root);
        let receipt = profiles::start_deliberation(window.clone(), app.clone(), app.state(), input)
            .await
            .map_err(|e| format!("admission: {e:?}"))?;
        serde_json::to_value(receipt).map_err(|_| "admission receipt")?
    };
    let run_id = receipt["runId"].as_str().ok_or("run id missing")?;
    std::fs::write(root.join("admission.json"), serde_json::to_vec_pretty(&serde_json::json!({"runId":run_id,"observedSelections":observed_selections,"coreBindings":core_bindings})).map_err(|_| "admission evidence encoding")?).map_err(|_| "admission evidence output")?;
    eprintln!("native live probe: production deliberation admitted; run {run_id}");
    monitor.mark_verified(app, "dispatching", &root);
    let started = Instant::now();
    loop {
        let dossier = storage
            .load_run_dossier(run_id)
            .map_err(|_| "durable dossier read")?;
        if dossier.snapshot.submitted_ballot_count < 3
            && dossier.snapshot.ballots_revealed.is_some()
        {
            return Err("partial sealed ballots exposed".into());
        }
        if matches!(
            dossier.snapshot.run.status,
            magi_domain::RunStatus::Paused {
                reason: magi_domain::PauseReason::NeedsInput,
                ..
            }
        ) {
            let clarification = crate::run_projection::clarification_view(&dossier.snapshot)
                .map_err(|_| "paused clarification integrity")?
                .ok_or("paused clarification missing")?;
            std::fs::write(
                root.join("paused-clarification.json"),
                serde_json::to_vec_pretty(&clarification).map_err(|_| "clarification encoding")?,
            )
            .map_err(|_| "clarification evidence")?;
            return Err(
                "production run paused awaiting essential information; consensus not completed"
                    .into(),
            );
        }
        if dossier.snapshot.run.status.is_terminal() {
            if !matches!(
                dossier.snapshot.run.status,
                magi_domain::RunStatus::Completed { .. }
            ) {
                return Err(format!(
                    "production run ended: {:?}",
                    dossier.snapshot.run.status
                ));
            }
            if dossier.snapshot.ballots_revealed.as_ref().map(Vec::len) != Some(3)
                || dossier.decision_dossier_digest.is_none()
            {
                return Err("completed durable ten-turn consensus invariant failed".into());
            }
            let proposal = dossier
                .snapshot
                .proposal
                .as_ref()
                .ok_or("completed proposal missing")?;
            let ballots = dossier
                .snapshot
                .ballots_revealed
                .as_ref()
                .ok_or("ballots missing")?;
            let mut cores = std::collections::BTreeSet::new();
            for ballot in ballots {
                ballot
                    .validate_for(
                        run_id,
                        &dossier.snapshot.run.input_digest,
                        dossier.snapshot.run.generation,
                        proposal,
                    )
                    .map_err(|_| "ballot binding mismatch")?;
                if !cores.insert(format!("{:?}", ballot.core_id)) {
                    return Err("duplicate ballot core".into());
                }
            }
            let execution = storage
                .load_live_dispatch_projection(run_id)
                .map_err(|_| "durable live dispatch projection")?;
            if execution.run_revision != dossier.snapshot.run.revision
                || execution.input_digest != dossier.snapshot.run.input_digest
                || execution.run_generation != dossier.snapshot.run.generation
                || execution.dispatches.len() != 10
            {
                return Err("execution projection snapshot mismatch".into());
            }
            for dispatch in &execution.dispatches {
                let reference = core_bindings
                    .iter()
                    .find(|reference| reference.core_id == dispatch.binding_core_id)
                    .ok_or("execution core reference missing")?;
                let frozen = dossier
                    .snapshot
                    .input
                    .role_set
                    .roles
                    .iter()
                    .find(|role| role.core_id == dispatch.binding_core_id)
                    .and_then(|role| role.catalog_binding.as_ref())
                    .ok_or("execution frozen binding missing")?;
                if dispatch.provider_profile_id != reference.provider_profile_id
                    || dispatch.profile_revision != reference.profile_revision
                    || dispatch.binding_digest != frozen.binding_digest
                {
                    return Err("execution routed profile mismatch".into());
                }
            }
            let frozen_refs = dossier
                .snapshot
                .input
                .role_set
                .frozen_core_selections
                .as_ref()
                .ok_or("frozen core selections missing")?;
            for reference in &core_bindings {
                if !frozen_refs.iter().any(|frozen| {
                    frozen.core_id == reference.core_id
                        && frozen.provider_profile_id == reference.provider_profile_id
                        && frozen.profile_revision == reference.profile_revision
                        && frozen.model_selection_revision == reference.model_selection_revision
                        && frozen.core_selection_revision == reference.core_selection_revision
                }) {
                    return Err("frozen core selection mismatch".into());
                }
            }
            if execution.dispatches.iter().any(|dispatch| {
                dispatch.state != magi_storage::LiveDispatchState::Settled
                    || dispatch.result_ref.is_none()
            }) {
                return Err("unsettled production dispatch".into());
            }
            let reader = magi_storage::StorageReader::open_read_only(
                app.path()
                    .app_data_dir()
                    .map_err(|_| "app data authority")?,
            )
            .map_err(|_| "reopen durable reader")?;
            let reopened = reader
                .load_run_dossier(run_id)
                .map_err(|_| "reopened durable dossier")?;
            if reopened != dossier {
                return Err("reopened durable projection mismatch".into());
            }
            if reader
                .load_live_dispatch_projection(run_id)
                .map_err(|_| "reopened live dispatch projection")?
                != execution
            {
                return Err("reopened execution projection mismatch".into());
            }
            std::fs::write(
                root.join("completed-live-dispatches.json"),
                serde_json::to_vec_pretty(&execution).map_err(|_| "execution evidence encoding")?,
            )
            .map_err(|_| "execution evidence output")?;
            let output = serde_json::to_vec_pretty(&dossier).map_err(|_| "dossier encoding")?;
            std::fs::write(root.join("completed-dossier.json"), output)
                .map_err(|_| "durable evidence output")?;
            let public = commands::load_run_dossier(window.clone(), app.state(), run_id.to_owned())
                .map_err(|_| "production UI dossier command")?;
            let public = serde_json::to_value(public).map_err(|_| "UI dossier value encoding")?;
            std::fs::write(
                root.join("completed-ui-dossier.json"),
                serde_json::to_vec_pretty(&public).map_err(|_| "UI dossier encoding")?,
            )
            .map_err(|_| "UI dossier evidence output")?;
            let public_run_id = public
                .get("runId")
                .and_then(serde_json::Value::as_str)
                .ok_or("UI dossier run identity missing")?;
            if public_run_id != run_id {
                return Err("UI dossier run identity mismatch".into());
            }
            if public["status"] != "completed"
                || public["votes"].as_array().map(Vec::len) != Some(3)
                || public["proposal"]["body"] != proposal.body
            {
                return Err("UI dossier does not match completed durable consensus".into());
            }
            for ballot in ballots {
                let expected_core =
                    serde_json::to_value(ballot.core_id).map_err(|_| "ballot core encoding")?;
                let expected_choice =
                    serde_json::to_value(ballot.vote).map_err(|_| "ballot choice encoding")?;
                if !public["votes"]
                    .as_array()
                    .ok_or("UI votes missing")?
                    .iter()
                    .any(|vote| {
                        vote["coreId"] == expected_core
                            && vote["choice"] == expected_choice
                            && vote["rationale"] == ballot.rationale
                    })
                {
                    return Err("UI ballot does not match frozen durable ballot".into());
                }
            }
            if let magi_domain::RunStatus::Completed { outcome } = &dossier.snapshot.run.status
                && public["outcome"]
                    != serde_json::to_value(outcome).map_err(|_| "durable outcome encoding")?
            {
                return Err("UI outcome does not match durable consensus".into());
            }
            eprintln!(
                "native live probe: durable ten-turn run and three revealed ballots verified; run {run_id}"
            );
            monitor.mark_verified(app, "verified", &root);
            return Ok(());
        }
        if started.elapsed() > Duration::from_secs(1800) {
            return Err("bounded production deliberation deadline".into());
        }
        tauri::async_runtime::spawn_blocking(|| std::thread::sleep(Duration::from_millis(250)))
            .await
            .map_err(|_| "poll worker")?;
    }
}

#[cfg(test)]
mod purpose_tests {
    use super::{
        ConnectionUiFailureStage, ConnectionUiReport, PdfUiReport, ProbeFrontendMode, ProbePurpose,
        ui_script_initialized, valid_recovery_run, validate_ui_phase, validate_ui_progress,
        write_ui_observation,
    };

    struct MetadataTestCleanup(Vec<(std::path::PathBuf, u64, u64)>);
    impl MetadataTestCleanup {
        fn track(&mut self, path: &std::path::Path) {
            use std::os::unix::fs::MetadataExt;
            if let Ok(metadata) = std::fs::symlink_metadata(path) {
                assert!(metadata.is_dir() && metadata.uid() == unsafe { libc::geteuid() });
                self.0
                    .push((path.to_path_buf(), metadata.dev(), metadata.ino()));
            }
        }
    }
    impl Drop for MetadataTestCleanup {
        fn drop(&mut self) {
            use std::os::unix::fs::MetadataExt;
            for (path, device, inode) in &self.0 {
                if std::fs::symlink_metadata(path).is_ok_and(|metadata| {
                    metadata.is_dir()
                        && metadata.dev() == *device
                        && metadata.ino() == *inode
                        && metadata.uid() == unsafe { libc::geteuid() }
                }) {
                    let _ = std::fs::remove_dir_all(path);
                }
            }
        }
    }

    #[test]
    fn installed_policy_resource_requires_sealed_exact_approved_bytes() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("policy-resource-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let mut cleanup = MetadataTestCleanup(Vec::new());
        cleanup.track(&root);
        let bytes = b"Explicit approved policy";
        let digest = magi_domain::Digest::from_bytes(bytes);
        assert!(super::verified_policy_resource(&root, digest.as_str()).is_err());
        let policy = root.join("policy");
        std::fs::create_dir(&policy).unwrap();
        let path = policy.join("SECURITY.md");
        std::fs::write(&path, bytes).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).unwrap();
        std::fs::set_permissions(&policy, std::fs::Permissions::from_mode(0o555)).unwrap();
        assert_eq!(
            super::verified_policy_resource(&root, digest.as_str()).unwrap(),
            path
        );
        assert!(
            super::verified_policy_resource(
                &root,
                magi_domain::Digest::from_bytes(b"changed").as_str()
            )
            .is_err()
        );
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(super::verified_policy_resource(&root, digest.as_str()).is_err());
        std::fs::set_permissions(&policy, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn saved_profile_intake_historical_metadata_never_authorizes_current_execution() {
        let (root, storage, _, _) =
            crate::profiles::catalog_selection_ipc_tests::publication_fault_fixture();
        let mut cleanup = MetadataTestCleanup(Vec::new());
        cleanup.track(&root);
        let id = "pause-profile-0";
        let profile = storage.load_provider_profile(id).unwrap().unwrap();
        let model = storage.load_provider_model_selection(id).unwrap().unwrap();
        let input = magi_domain::ProviderProfileInput {
            provider_profile_id: id.into(),
            provider_id: profile.provider_id,
            display_name: "Observed renamed profile".into(),
            account_alias: profile.account_alias,
            authentication_method: profile.authentication_method,
            secret_reference: None,
            runtime_home_id: profile.runtime_home_id,
            credential_home: profile.credential_home,
        };
        let head = storage
            .save_provider_profile(&input, Some(0), "2026-10-02T18:00:00Z")
            .unwrap();
        assert_eq!(head.revision, 1);
        assert!(storage.load_provider_model_selection(id).is_err());
        let historical = storage
            .load_historical_model_selection_metadata(id, 0, model.selection_revision)
            .unwrap()
            .unwrap();
        assert_eq!(historical.binding.model_id, model.binding.model_id);
        assert_eq!(historical.binding.mode_id, model.binding.mode_id);
        assert!(
            storage
                .load_historical_model_selection_metadata(id, 1, model.selection_revision)
                .is_err()
        );
        assert!(
            storage
                .load_historical_model_selection_metadata(id, 0, model.selection_revision + 1)
                .is_err()
        );
        assert!(
            storage
                .load_historical_model_selection_metadata("missing-profile", 0, 0)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn saved_profile_intake_preserves_exact_durable_routing_and_rejects_drift() {
        let (root, storage, _, _) =
            crate::profiles::catalog_selection_ipc_tests::publication_fault_fixture();
        let mut cleanup = MetadataTestCleanup(Vec::new());
        cleanup.track(&root);
        let bindings = magi_domain::CoreId::ALL.map(|core_id| {
            let core = storage.load_core_model_selection(core_id).unwrap().unwrap();
            let profile = storage
                .load_provider_profile(&core.provider_profile_id)
                .unwrap()
                .unwrap();
            let model = storage
                .load_provider_model_selection(&core.provider_profile_id)
                .unwrap()
                .unwrap();
            super::SavedProfileBinding {
                core_id,
                provider_profile_id: profile.provider_profile_id,
                profile_revision: profile.revision,
                model_profile_revision: model.binding.profile_revision,
                model_selection_revision: model.selection_revision,
                model_id: model.binding.model_id,
                mode_id: model.binding.mode_id,
                core_selection_revision: Some(core.selection_revision),
            }
        });
        let input = super::SavedProfileInput {
            schema_version: 1,
            prepared_store_digest: "a".repeat(64),
            bindings,
        };
        assert!(super::validate_saved_profile_records(&storage, &input).is_ok());
        let backup = root.with_file_name(format!("saved-backup-{}", uuid::Uuid::new_v4()));
        let restored = root.with_file_name(format!("saved-restored-{}", uuid::Uuid::new_v4()));
        let manifest = storage.create_backup(&backup).unwrap();
        cleanup.track(&backup);
        assert_eq!(manifest.store_id, storage.identity().store_id);
        let receipt = magi_storage::Storage::restore_backup(&backup, &restored).unwrap();
        cleanup.track(&restored);
        assert_eq!(receipt.store_id, manifest.store_id);
        let restored_storage = magi_storage::Storage::open_or_create(&restored).unwrap();
        for binding in &input.bindings {
            let profile = restored_storage
                .load_provider_profile(&binding.provider_profile_id)
                .unwrap()
                .unwrap();
            let model = restored_storage
                .load_provider_model_selection(&binding.provider_profile_id)
                .unwrap()
                .unwrap();
            assert_eq!(profile.revision, binding.profile_revision);
            assert_eq!(model.binding.model_id, binding.model_id);
            assert_eq!(model.binding.mode_id, binding.mode_id);
        }
        drop(restored_storage);

        for kind in 0..6 {
            let mut bad = input.clone();
            match kind {
                0 => bad.bindings[0].provider_profile_id = "missing-profile".into(),
                1 => bad.bindings[0].profile_revision += 1,
                2 => bad.bindings[0].model_selection_revision += 1,
                3 => bad.bindings[0].model_id = "different-model".into(),
                4 => bad.bindings.swap(0, 1),
                _ => bad.bindings[0].mode_id = Some("different-mode".into()),
            }
            if kind == 4 {
                bad.bindings[0].core_id = input.bindings[0].core_id;
                bad.bindings[1].core_id = input.bindings[1].core_id;
            }
            assert!(super::validate_saved_profile_records(&storage, &bad).is_err());
        }
        let mut duplicate = input.clone();
        duplicate.bindings[1].provider_profile_id =
            duplicate.bindings[0].provider_profile_id.clone();
        assert!(duplicate.validate().is_err());
        duplicate = input.clone();
        duplicate.bindings[1].core_id = duplicate.bindings[0].core_id;
        assert!(duplicate.validate().is_err());
        let mut value = serde_json::to_value(&input).unwrap();
        value["fallbackModel"] = serde_json::json!("global-model");
        assert!(serde_json::from_value::<super::SavedProfileInput>(value).is_err());
        assert!(serde_json::from_str::<super::SavedProfileInput>("{}").is_err());
    }

    #[test]
    fn connections_metadata_preparation_preserves_source_and_excludes_execution_bindings() {
        let (root, storage, aggregate, _) =
            crate::profiles::catalog_selection_ipc_tests::publication_fault_fixture();
        let run_id = aggregate.run().run_id.clone();
        drop(storage);
        let source = root.canonicalize().unwrap();
        let destination =
            source.with_file_name(format!("connection-metadata-{}", uuid::Uuid::new_v4()));
        let mut cleanup = MetadataTestCleanup(Vec::new());
        cleanup.track(&source);
        let database = source.join("state/magi.sqlite");
        let bytes = std::fs::read(&database).unwrap();
        let digest = magi_domain::Digest::from_bytes(&bytes);
        assert!(
            super::prepare_connection_metadata(
                &database,
                &destination,
                magi_domain::Digest::from_bytes(b"different source").as_str()
            )
            .is_err()
        );
        assert!(!destination.exists());
        assert_eq!(std::fs::read(&database).unwrap(), bytes);
        let result = super::prepare_connection_metadata(&database, &destination, digest.as_str());
        cleanup.track(&destination);
        if let Err(error) = result {
            panic!("metadata preparation control failed: {error}");
        }
        assert_eq!(std::fs::read(&database).unwrap(), bytes);
        let prepared_bytes = std::fs::read(destination.join("state/magi.sqlite")).unwrap();
        assert!(
            super::prepare_connection_metadata(&database, &destination, digest.as_str()).is_err()
        );
        assert_eq!(
            std::fs::read(destination.join("state/magi.sqlite")).unwrap(),
            prepared_bytes
        );
        assert!(!source.join("state/magi.sqlite-wal").exists());
        assert!(!source.join("state/magi.sqlite-shm").exists());
        let original = magi_storage::StorageReader::connection_metadata_from_captured_snapshot(
            &bytes,
            digest.as_str(),
        )
        .unwrap();
        let target = magi_storage::Storage::open_or_create(&destination).unwrap();
        assert_eq!(target.list_provider_profiles(100).unwrap().len(), 3);
        for profile in &original.profiles {
            let head = profile.history.last().unwrap();
            assert_eq!(
                target
                    .load_provider_profile(&head.provider_profile_id)
                    .unwrap()
                    .as_ref(),
                Some(head)
            );
            assert_eq!(
                target
                    .load_provider_model_selection(&head.provider_profile_id)
                    .unwrap()
                    .unwrap()
                    .binding,
                profile.model_selection.as_ref().unwrap().binding
            );
        }
        for core in magi_domain::CoreId::ALL {
            assert!(target.load_core_model_selection(core).unwrap().is_none());
        }
        assert!(target.load_run_aggregate(&run_id).is_err());
        drop(target);
        drop(cleanup);
        assert!(!destination.exists() && !root.exists());
    }

    #[test]
    fn connections_metadata_flat_backup_file_replays_and_sidecars_reject() {
        let (root, storage, _, _) =
            crate::profiles::catalog_selection_ipc_tests::publication_fault_fixture();
        drop(storage);
        let root = root.canonicalize().unwrap();
        let mut cleanup = MetadataTestCleanup(Vec::new());
        cleanup.track(&root);
        let database = root.join("closed backup.sqlite");
        let bytes = std::fs::read(root.join("state/magi.sqlite")).unwrap();
        std::fs::write(&database, &bytes).unwrap();
        let digest = magi_domain::Digest::from_bytes(&bytes);
        let destination = root.join("prepared");
        for suffix in ["-wal", "-shm"] {
            let mut sidecar = database.as_os_str().to_os_string();
            sidecar.push(suffix);
            let sidecar = std::path::PathBuf::from(sidecar);
            std::fs::write(&sidecar, b"pending snapshot").unwrap();
            assert_eq!(
                super::prepare_connection_metadata(&database, &destination, digest.as_str()),
                Err("closed coherent source snapshot required")
            );
            assert!(!destination.exists());
            assert_eq!(std::fs::read(&database).unwrap(), bytes);
            assert_eq!(std::fs::read(&sidecar).unwrap(), b"pending snapshot");
            std::fs::remove_file(sidecar).unwrap();
        }
        super::prepare_connection_metadata(&database, &destination, digest.as_str()).unwrap();
        assert_eq!(std::fs::read(&database).unwrap(), bytes);
        let target = magi_storage::Storage::open_or_create(&destination).unwrap();
        assert_eq!(target.list_provider_profiles(100).unwrap().len(), 3);
        for core in magi_domain::CoreId::ALL {
            assert!(target.load_core_model_selection(core).unwrap().is_none());
        }
        drop(target);
    }

    #[test]
    fn connections_metadata_source_pin_and_existing_destination_fail_before_writes() {
        let root = std::env::temp_dir().join(format!(
            "connection-metadata-invalid-{}",
            uuid::Uuid::new_v4()
        ));
        assert!(super::prepare_connection_metadata(&root, &root, "invalid").is_err());
        assert!(!root.exists());
        std::fs::create_dir(&root).unwrap();
        assert!(super::prepare_connection_metadata(&root, &root, &"a".repeat(64)).is_err());
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
        std::fs::remove_dir(&root).unwrap();
    }

    #[test]
    fn connections_metadata_captured_corrupt_and_pin_inputs_fail_closed() {
        let corrupt = b"not a sqlite database";
        assert!(
            magi_storage::StorageReader::connection_metadata_from_captured_snapshot(
                corrupt,
                magi_domain::Digest::from_bytes(corrupt).as_str()
            )
            .is_err()
        );
        assert!(
            magi_storage::StorageReader::connection_metadata_from_captured_snapshot(
                corrupt,
                magi_domain::Digest::from_bytes(b"wrong pin").as_str()
            )
            .is_err()
        );
        assert!(
            magi_storage::StorageReader::connection_metadata_from_captured_snapshot(
                &[],
                magi_domain::Digest::from_bytes(&[]).as_str()
            )
            .is_err()
        );
    }

    #[test]
    fn connections_metadata_failed_model_replay_removes_only_owned_destination() {
        let (root, storage, _, _) =
            crate::profiles::catalog_selection_ipc_tests::publication_fault_fixture();
        drop(storage);
        let mut cleanup = MetadataTestCleanup(Vec::new());
        cleanup.track(&root);
        let bytes = std::fs::read(root.join("state/magi.sqlite")).unwrap();
        let digest = magi_domain::Digest::from_bytes(&bytes);
        let mut metadata = magi_storage::StorageReader::connection_metadata_from_captured_snapshot(
            &bytes,
            digest.as_str(),
        )
        .unwrap();
        metadata.profiles[0]
            .model_selection
            .as_mut()
            .unwrap()
            .binding
            .model_id = "not-in-captured-catalog".into();
        let entries = metadata
            .profiles
            .iter()
            .map(|profile| {
                (
                    profile.history.as_slice(),
                    profile.selected_catalog.as_ref().unwrap(),
                    profile.latest_catalog.as_ref(),
                    profile.model_selection.as_ref().unwrap(),
                )
            })
            .collect::<Vec<_>>();
        let destination = root.with_file_name(format!(
            "connection-metadata-rollback-{}",
            uuid::Uuid::new_v4()
        ));
        assert_eq!(
            super::connection_ui_fixture::seed(&destination, &entries)
                .err()
                .unwrap(),
            "saved model metadata replay failed"
        );
        assert!(!destination.exists());
        assert_eq!(
            std::fs::read(root.join("state/magi.sqlite")).unwrap(),
            bytes
        );
        let wal = root.join("state/magi.sqlite-wal");
        std::fs::write(&wal, b"uncheckpointed-source-control").unwrap();
        assert!(
            super::prepare_connection_metadata(
                &root.join("state/magi.sqlite"),
                &destination,
                digest.as_str()
            )
            .is_err()
        );
        assert!(!destination.exists());
        assert_eq!(
            std::fs::read(&wal).unwrap(),
            b"uncheckpointed-source-control"
        );
        drop(entries);
        let lawful_model = metadata.profiles[0]
            .selected_catalog
            .as_ref()
            .unwrap()
            .models[0]
            .model_id
            .clone();
        metadata.profiles[0]
            .model_selection
            .as_mut()
            .unwrap()
            .binding
            .model_id = lawful_model;
        let entries = metadata
            .profiles
            .iter()
            .map(|profile| {
                (
                    profile.history.as_slice(),
                    profile.selected_catalog.as_ref().unwrap(),
                    profile.latest_catalog.as_ref(),
                    profile.model_selection.as_ref().unwrap(),
                )
            })
            .collect::<Vec<_>>();
        let (_, mut authority) = super::connection_ui_fixture::seed(&destination, &entries)
            .unwrap_or_else(|error| panic!("lawful metadata seed failed: {error}"));
        let displaced = destination.with_file_name(format!(
            "connection-metadata-displaced-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::rename(&destination, &displaced).unwrap();
        cleanup.track(&displaced);
        std::fs::create_dir(&destination).unwrap();
        cleanup.track(&destination);
        std::fs::write(
            destination.join("foreign-sentinel"),
            b"preserve foreign replacement",
        )
        .unwrap();
        assert!(authority.write_provenance(b"rejected").is_err());
        assert!(authority.commit().is_err());
        drop(authority);
        assert_eq!(
            std::fs::read(destination.join("foreign-sentinel")).unwrap(),
            b"preserve foreign replacement"
        );
        assert!(
            !destination
                .join("connection-ui-metadata-provenance.json")
                .exists()
        );
    }

    #[test]
    fn connections_metadata_history_revision_64_replays_and_65_rejects() {
        let (root, storage, _, _) =
            crate::profiles::catalog_selection_ipc_tests::publication_fault_fixture();
        let mut cleanup = MetadataTestCleanup(Vec::new());
        cleanup.track(&root);
        let id = "pause-profile-0";
        let original = storage.load_provider_profile(id).unwrap().unwrap();
        let mut input = magi_domain::ProviderProfileInput {
            provider_profile_id: id.into(),
            provider_id: original.provider_id.clone(),
            display_name: original.display_name.clone(),
            account_alias: original.account_alias.clone(),
            authentication_method: original.authentication_method,
            secret_reference: None,
            runtime_home_id: original.runtime_home_id.clone(),
            credential_home: original.credential_home.clone(),
        };
        let selection = storage.load_provider_model_selection(id).unwrap().unwrap();
        let mut catalog = storage
            .load_provider_catalog_snapshot(&selection.binding.catalog_snapshot_id)
            .unwrap()
            .unwrap();
        for revision in 1..=64 {
            input.display_name = format!("History boundary {revision}");
            assert_eq!(
                storage
                    .save_provider_profile(&input, Some(revision - 1), "2026-10-02T00:00:00Z")
                    .unwrap()
                    .revision,
                revision
            );
        }
        catalog.catalog_snapshot_id = "history-boundary-catalog-64".into();
        catalog.profile_revision = 64;
        catalog.catalog_digest = catalog.calculate_digest().unwrap();
        storage.save_provider_catalog_snapshot(&catalog).unwrap();
        storage
            .select_provider_model(&magi_storage::ProviderModelSelectionInput {
                provider_profile_id: id.into(),
                profile_revision: 64,
                catalog_snapshot_id: catalog.catalog_snapshot_id.clone(),
                catalog_digest: catalog.catalog_digest.clone(),
                model_id: selection.binding.model_id,
                mode_id: selection.binding.mode_id,
                expected_selection_revision: Some(selection.selection_revision),
                updated_at: selection.updated_at,
            })
            .unwrap();
        drop(storage);
        let bytes = std::fs::read(root.join("state/magi.sqlite")).unwrap();
        let metadata = magi_storage::StorageReader::connection_metadata_from_captured_snapshot(
            &bytes,
            magi_domain::Digest::from_bytes(&bytes).as_str(),
        )
        .unwrap();
        assert_eq!(
            metadata
                .profiles
                .iter()
                .find(|profile| profile.history[0].provider_profile_id == id)
                .unwrap()
                .history
                .len(),
            65
        );
        let destination = root.with_file_name(format!(
            "connection-metadata-history-{}",
            uuid::Uuid::new_v4()
        ));
        super::prepare_connection_metadata(
            &root.join("state/magi.sqlite"),
            &destination,
            magi_domain::Digest::from_bytes(&bytes).as_str(),
        )
        .unwrap();
        cleanup.track(&destination);
        let imported = magi_storage::Storage::open_or_create(&destination).unwrap();
        assert_eq!(
            imported
                .load_provider_profile(id)
                .unwrap()
                .unwrap()
                .revision,
            64
        );
        assert!(
            imported
                .load_provider_profile_revision(id, 0)
                .unwrap()
                .is_some()
        );
        drop(imported);
        let storage = magi_storage::Storage::open_or_create(&root).unwrap();
        input.display_name = "History boundary 65".into();
        assert_eq!(
            storage
                .save_provider_profile(&input, Some(64), "2026-10-02T00:00:00Z")
                .unwrap()
                .revision,
            65
        );
        drop(storage);
        let bytes = std::fs::read(root.join("state/magi.sqlite")).unwrap();
        let error = magi_storage::StorageReader::connection_metadata_from_captured_snapshot(
            &bytes,
            magi_domain::Digest::from_bytes(&bytes).as_str(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("history capacity exceeded"));
    }

    #[test]
    fn connections_ui_phase_distinguishes_missing_script_from_stalled_projection() {
        assert!(!ui_script_initialized(&[]));
        let initialized = serde_json::json!({"kind":"phase","stage":"profile_query","phase":"script_initialized","checkpointCount":0});
        let stalled = serde_json::json!({"kind":"phase","stage":"row_projection","phase":"projection_pending","checkpointCount":0});
        assert!(ui_script_initialized(&[initialized, stalled.clone()]));
        let value =
            serde_json::json!({"schemaVersion":1,"nonce":"bound","pid":7,"checkpoint":stalled});
        assert!(serde_json::from_value::<ConnectionUiReport>(value.clone()).is_ok());
        for field in ["phase", "stage"] {
            let mut bad = value.clone();
            bad["checkpoint"][field] = serde_json::json!("private_canary");
            assert!(serde_json::from_value::<ConnectionUiReport>(bad).is_err());
        }
        let mut bad = value;
        bad["checkpoint"]["rawDom"] = serde_json::json!("private_canary");
        assert!(serde_json::from_value::<ConnectionUiReport>(bad).is_err());
    }

    #[test]
    fn connections_ui_modal_edit_phases_are_closed_and_stage_bound() {
        for phase in [
            "edit_control_wait",
            "edit_control_found",
            "edit_clicked",
            "edit_dialog_found",
            "edit_dialog_focus_wait",
            "edit_dialog_focus_ready",
            "edit_trigger_focus_restored",
            "edit_focus_failed",
        ] {
            let checkpoint = serde_json::json!({"kind":"phase","stage":"modal_edit","phase":phase,"checkpointCount":2});
            let value = serde_json::json!({"schemaVersion":1,"nonce":"bound","pid":7,"checkpoint":checkpoint});
            let report: ConnectionUiReport = serde_json::from_value(value.clone()).unwrap();
            let super::ConnectionUiCheckpoint::Phase {
                stage,
                phase,
                checkpoint_count,
            } = report.checkpoint
            else {
                panic!("expected phase")
            };
            assert!(validate_ui_phase(&stage, &phase, checkpoint_count, 2, &[]).is_ok());
            assert!(
                validate_ui_phase(&ConnectionUiFailureStage::ModalAdd, &phase, 2, 2, &[]).is_err()
            );
            assert!(validate_ui_phase(&stage, &phase, 1, 1, &[]).is_err());
            assert!(validate_ui_phase(&stage, &phase, 2, 1, &[]).is_err());
            assert!(
                validate_ui_phase(&stage, &phase, 2, 2, &vec![serde_json::Value::Null; 64])
                    .is_err()
            );
            for field in ["html", "rawDom", "rawError"] {
                let mut invalid = value.clone();
                invalid["checkpoint"][field] = serde_json::json!("private_canary");
                assert!(serde_json::from_value::<ConnectionUiReport>(invalid).is_err());
            }
        }
        for phase in ["edit_unknown", "edit_control_wait\n", "<div>private</div>"] {
            let value = serde_json::json!({"schemaVersion":1,"nonce":"bound","pid":7,"checkpoint":{"kind":"phase","stage":"modal_edit","phase":phase,"checkpointCount":2}});
            assert!(serde_json::from_value::<ConnectionUiReport>(value).is_err());
        }
    }

    #[test]
    fn connections_ui_progress_is_closed_and_deadline_retains_stage() {
        assert!(validate_ui_progress(&ConnectionUiFailureStage::ProfileQuery, 0, 0, 0).is_ok());
        assert!(validate_ui_progress(&ConnectionUiFailureStage::DraftReturn, 1, 1, 3).is_ok());
        assert!(validate_ui_progress(&ConnectionUiFailureStage::DraftReturn, 0, 0, 3).is_err());
        assert!(validate_ui_progress(&ConnectionUiFailureStage::ModalAdd, 2, 2, 3).is_err());
        let value = serde_json::json!({"schemaVersion":1,"nonce":"bound","pid":7,"checkpoint":{"kind":"progress","stage":"row_projection","checkpointCount":0}});
        assert!(serde_json::from_value::<ConnectionUiReport>(value.clone()).is_ok());
        let mut unknown = value.clone();
        unknown["checkpoint"]["stage"] = serde_json::json!("private_canary");
        assert!(serde_json::from_value::<ConnectionUiReport>(unknown).is_err());
        let mut extra = value;
        extra["checkpoint"]["html"] = serde_json::json!("private_canary");
        assert!(serde_json::from_value::<ConnectionUiReport>(extra).is_err());
        let root =
            std::env::temp_dir().join(format!("deadline-observation-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let body = serde_json::json!({"lastProgress":{"kind":"progress","stage":"row_projection","checkpointCount":0},"acceptedCheckpointCount":0,"failureReceived":false});
        write_ui_observation(&root, "connections-ui-deadline.json", &body).unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(
                &std::fs::read(root.join("connections-ui-deadline.json")).unwrap()
            )
            .unwrap(),
            body
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn connections_ui_failure_is_allowlisted_and_count_bound() {
        let value = serde_json::json!({"schemaVersion":1,"nonce":"bound","pid":7,"checkpoint":{"kind":"failed","stage":"row_projection","code":"saved_model","checkpointCount":0,"modelMismatch":{"actualModelId":"選択","actualModeId":null,"expectedModelId":"model","expectedModeId":"mode"}}});
        let report: ConnectionUiReport = serde_json::from_value(value.clone()).unwrap();
        assert!(report.validates_authority(true, "bound", 7, "main"));
        assert!(report.checkpoint.validate_failure_count(0).is_ok());
        assert!(report.checkpoint.validate_failure_count(1).is_err());
        for field in ["stage", "code"] {
            let mut invalid = value.clone();
            invalid["checkpoint"][field] = serde_json::json!("private_canary");
            assert!(serde_json::from_value::<ConnectionUiReport>(invalid).is_err());
        }
        let mut extra = value.clone();
        extra["checkpoint"]["rawError"] = serde_json::json!("private_canary");
        assert!(serde_json::from_value::<ConnectionUiReport>(extra).is_err());
        for field in ["actualModelId", "expectedModeId"] {
            for token in [
                serde_json::json!(""),
                serde_json::json!("x".repeat(257)),
                serde_json::json!("private\ncanary"),
            ] {
                let mut invalid = value.clone();
                invalid["checkpoint"]["modelMismatch"][field] = token;
                let report: ConnectionUiReport = serde_json::from_value(invalid).unwrap();
                assert!(report.checkpoint.validate_failure_count(0).is_err());
            }
        }
        let mut extra_tokens = value.clone();
        extra_tokens["checkpoint"]["modelMismatch"]["html"] = serde_json::json!("private_canary");
        assert!(serde_json::from_value::<ConnectionUiReport>(extra_tokens).is_err());
        let mut wrong_stage = value.clone();
        wrong_stage["checkpoint"]["stage"] = serde_json::json!("connections");
        let report: ConnectionUiReport = serde_json::from_value(wrong_stage).unwrap();
        assert!(report.checkpoint.validate_failure_count(0).is_err());
        let mut projection = value.clone();
        projection["checkpoint"]["code"] = serde_json::json!("catalog_projection_error");
        projection["checkpoint"]["modelMismatch"] = serde_json::Value::Null;
        let report: ConnectionUiReport = serde_json::from_value(projection.clone()).unwrap();
        assert!(report.checkpoint.validate_failure_count(0).is_ok());
        let mut with_tokens = projection.clone();
        with_tokens["checkpoint"]["modelMismatch"] = value["checkpoint"]["modelMismatch"].clone();
        let report: ConnectionUiReport = serde_json::from_value(with_tokens).unwrap();
        assert!(report.checkpoint.validate_failure_count(0).is_err());
        projection["checkpoint"]["stage"] = serde_json::json!("connections");
        let report: ConnectionUiReport = serde_json::from_value(projection).unwrap();
        assert!(report.checkpoint.validate_failure_count(0).is_err());
        let mut excessive = value;
        excessive["checkpoint"]["checkpointCount"] = serde_json::json!(4);
        let report: ConnectionUiReport = serde_json::from_value(excessive).unwrap();
        assert!(report.checkpoint.validate_failure_count(4).is_err());
    }

    #[test]
    fn probe_frontend_mode_is_closed_and_embedded_assets_are_required() {
        use std::os::unix::ffi::OsStrExt;
        let invalid = std::ffi::OsStr::from_bytes(&[0xff]);
        let server = std::ffi::OsStr::new("http://127.0.0.1:1427");
        assert!(ProbeFrontendMode::select_os(Some(invalid), Some(server), true).is_err());
        assert!(
            ProbeFrontendMode::select_os(
                Some(std::ffi::OsStr::new("embedded-assets")),
                Some(invalid),
                true
            )
            .is_err()
        );

        assert_eq!(
            ProbeFrontendMode::select(None, Some("http://127.0.0.1:1427"), false),
            Ok(ProbeFrontendMode::DebugServer)
        );
        assert_eq!(
            ProbeFrontendMode::select(Some("embedded-assets"), None, true),
            Ok(ProbeFrontendMode::EmbeddedAssets)
        );
        assert!(ProbeFrontendMode::select(Some("embedded-assets"), None, false).is_err());
        assert!(
            ProbeFrontendMode::select(Some("embedded-assets"), Some("http://127.0.0.1:1427"), true)
                .is_err()
        );
        for mode in ["", "automatic", "https://example.com"] {
            assert!(ProbeFrontendMode::select(Some(mode), None, true).is_err());
        }
        assert!(ProbeFrontendMode::select(None, Some("http://127.0.0.1:1420"), true).is_err());
    }

    #[test]
    fn connections_ui_reports_are_closed_and_bound_to_enabled_main_process() {
        let value = serde_json::json!({"schemaVersion":1,"nonce":"bound","pid":7,"checkpoint":{"kind":"complete"}});
        let report: ConnectionUiReport = serde_json::from_value(value.clone()).unwrap();
        assert!(report.validates_authority(true, "bound", 7, "main"));
        for (enabled, nonce, pid, window) in [
            (false, "bound", 7, "main"),
            (true, "other", 7, "main"),
            (true, "bound", 8, "main"),
            (true, "bound", 7, "companion"),
        ] {
            assert!(!report.validates_authority(enabled, nonce, pid, window));
        }
        for invalid in [
            serde_json::json!({"schemaVersion":1,"nonce":"bound","pid":7,"checkpoint":{"kind":"complete","private":"canary"}}),
            serde_json::json!({"schemaVersion":1,"nonce":"bound","pid":7,"checkpoint":{"kind":"unknown"}}),
            serde_json::json!({"schemaVersion":1,"nonce":"bound","pid":7,"checkpoint":{"kind":"complete"},"private":"canary"}),
        ] {
            assert!(serde_json::from_value::<ConnectionUiReport>(invalid).is_err());
        }
        let purpose = ProbePurpose::parse(Some("connections-ui")).unwrap();
        assert_eq!(purpose.profile_count(), 0);
        assert!(!purpose.permits_admission());
    }

    #[test]
    fn catalog_diagnostic_is_single_profile_and_cannot_admit_inference() {
        let purpose = ProbePurpose::parse(Some("catalog-diagnostic")).unwrap();
        assert_eq!(purpose.profile_count(), 1);
        assert!(!purpose.permits_admission());
        let normal = ProbePurpose::parse(None).unwrap();
        assert_eq!(normal.profile_count(), 3);
        assert!(normal.permits_admission());
        for invalid in ["", "catalog", "automatic", "42"] {
            assert!(ProbePurpose::parse(Some(invalid)).is_err());
        }
    }

    #[test]
    fn recovery_cancellation_has_no_profiles_or_admission_and_requires_exact_run_reference() {
        let purpose = ProbePurpose::parse(Some("recovery-cancel")).unwrap();
        assert_eq!(purpose.profile_count(), 0);
        assert!(!purpose.permits_admission());
        assert!(valid_recovery_run(&format!("run-{}", "a".repeat(64))));
        for invalid in ["", "run-private_canary", "run-../../private", "automatic"] {
            assert!(!valid_recovery_run(invalid));
        }
    }

    #[test]
    fn pdf_ui_purpose_has_no_profiles_or_admission_and_closed_reports() {
        let purpose = ProbePurpose::parse(Some("pdf-ui")).unwrap();
        assert_eq!(purpose.profile_count(), 0);
        assert!(!purpose.permits_admission());
        assert!(serde_json::from_value::<PdfUiReport>(serde_json::json!({"kind":"metadata","draftId":"draft","contextRevision":null,"totalPages":2})).is_ok());
        for value in [
            serde_json::json!({"kind":"metadata","draftId":"draft","contextRevision":null,"totalPages":2,"path":"private_canary"}),
            serde_json::json!({"kind":"private_canary"}),
            serde_json::json!({"kind":"rendered","draftId":"draft","revision":-1}),
        ] {
            assert!(serde_json::from_value::<PdfUiReport>(value).is_err());
        }
    }
}

#[tauri::command]
async fn get_console_snapshot(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::commands::DesktopState>,
) -> Result<crate::commands::ConsoleSnapshot, String> {
    crate::commands::verified_console_snapshot(window, app, state).await
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ProbeControlMessage {
    action: ProbeControlAction,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum ProbeControlAction {
    Cancel,
}

fn cancel_probe_request(
    app: &tauri::AppHandle,
    monitor: &ProbeMonitor,
    cleanup: Option<commands::AdmissionOperationLease>,
) -> serde_json::Value {
    cancel_probe_request_with_cleanup(
        app,
        monitor,
        Instant::now() + Duration::from_secs(60),
        cleanup,
    )
}

fn cancel_probe_request_until(
    app: &tauri::AppHandle,
    monitor: &ProbeMonitor,
    deadline: Instant,
) -> serde_json::Value {
    cancel_probe_request_with_cleanup(app, monitor, deadline, None)
}

fn cancel_probe_request_with_cleanup(
    app: &tauri::AppHandle,
    monitor: &ProbeMonitor,
    deadline: Instant,
    cleanup: Option<commands::AdmissionOperationLease>,
) -> serde_json::Value {
    let reference = match monitor.active_request.try_lock() {
        Ok(value) => value.clone(),
        Err(_) => return serde_json::json!({"accepted":false,"code":"control_busy"}),
    };
    let Some(reference) = reference else {
        return serde_json::json!({"accepted":false,"code":"request_not_registered"});
    };
    let Some(window) = app.get_webview_window("main") else {
        return serde_json::json!({"accepted":false,"code":"window_unavailable"});
    };
    let app = app.clone();
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    let lifecycle = monitor.lifecycle.clone();
    tauri::async_runtime::spawn(async move {
        let _cleanup = cleanup.or_else(|| lifecycle.revoke_with_cancellation_lease());
        let mut reference = reference;
        if reference.admission_authority.is_none() {
            reference.admission_authority = app
                .state::<commands::DesktopState>()
                .admission_requests
                .supervised_capability(
                    &lifecycle,
                    &reference.request.command_id,
                    &reference.request.idempotency_key,
                );
        }
        let accepted =
            profiles::cancel_deliberation_request(window, app.clone(), app.state(), reference)
                .await
                .is_ok();
        let _ = sender.send(accepted);
    });
    wait_probe_cancellation(receiver, deadline)
}

fn wait_probe_cancellation(
    receiver: std::sync::mpsc::Receiver<bool>,
    deadline: Instant,
) -> serde_json::Value {
    match receiver.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        Ok(true) => {
            serde_json::json!({"accepted":true,"authority":"production_request_cancellation"})
        }
        Ok(false) => {
            serde_json::json!({"accepted":false,"code":"production_cancellation_rejected"})
        }
        Err(_) => {
            serde_json::json!({"accepted":false,"code":"cancellation_pending"})
        }
    }
}

fn serve_probe_control(
    mut stream: std::os::unix::net::UnixStream,
    cancellation: impl FnOnce(Instant) -> serde_json::Value,
) -> std::io::Result<()> {
    use std::{
        io::{Read, Write},
        os::fd::AsRawFd,
    };
    let mut uid = 0;
    let mut gid = 0;
    if unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) } != 0
        || uid != unsafe { libc::geteuid() }
    {
        return Err(std::io::Error::other("Control peer authority rejected"));
    }
    stream.set_nonblocking(false)?;
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut bytes = Vec::new();
    let remaining = || {
        deadline
            .checked_duration_since(Instant::now())
            .filter(|value| !value.is_zero())
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::TimedOut, "Control deadline expired")
            })
    };
    while bytes.len() < 257 {
        stream.set_read_timeout(Some(remaining()?))?;
        let mut buffer = [0_u8; 257];
        let count = stream.read(&mut buffer[..257 - bytes.len()])?;
        if count == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
    remaining()?;
    let message = if bytes.len() <= 256 {
        serde_json::from_slice::<ProbeControlMessage>(&bytes).ok()
    } else {
        None
    };
    let result = match message {
        Some(ProbeControlMessage {
            action: ProbeControlAction::Cancel,
        }) => cancellation(deadline),
        None => serde_json::json!({"accepted":false,"code":"invalid_control_message"}),
    };
    let output = serde_json::to_vec(&result)
        .map_err(|_| std::io::Error::other("Control result encoding failed"))?;
    let mut written = 0;
    while written < output.len() {
        stream.set_write_timeout(Some(remaining()?))?;
        let count = stream.write(&output[written..])?;
        if count == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WriteZero,
                "Control response unavailable",
            ));
        }
        written += count;
    }
    Ok(())
}

fn start_probe_control_bridge(
    app: tauri::AppHandle,
    monitor: Arc<ProbeMonitor>,
    root: PathBuf,
) -> std::io::Result<()> {
    use std::os::unix::net::UnixListener;
    let directory = PathBuf::from("/tmp")
        .canonicalize()?
        .join(format!("magi-control-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir(&directory)?;
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
    let socket = directory.join("cancel.sock");
    let listener = UnixListener::bind(&socket)?;
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;
    std::fs::write(root.join("control-reference.json"), serde_json::to_vec_pretty(&serde_json::json!({"socketPath":socket,"processId":std::process::id(),"action":"cancel","scope":"owned_live_process"})).map_err(|_| std::io::Error::other("Control reference encoding failed"))?)?;
    std::thread::spawn(move || {
        while monitor.outcome.load(Ordering::Acquire) == 0 {
            match listener.accept() {
                Ok((stream, _)) => {
                    let _ = serve_probe_control(stream, |deadline| {
                        cancel_probe_request_until(&app, &monitor, deadline)
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(25))
                }
                Err(_) => break,
            }
        }
        drop(listener);
        let _ = std::fs::remove_file(socket);
        let _ = std::fs::remove_dir(directory);
    });
    Ok(())
}

#[cfg(test)]
mod probe_control_bridge_tests {
    fn native_binding(id: &str) -> magi_storage::AdmissionRequestBinding {
        magi_storage::AdmissionRequestBinding {
            command_id: id.into(),
            idempotency_key: id.into(),
            intent_digest: magi_domain::Digest::from_bytes(id.as_bytes()),
        }
    }
    #[test]
    fn supervised_timeout_before_issuance_cannot_mint_fresh_authority() {
        let registry = commands::AdmissionRequestRegistry::default();
        let lifecycle =
            commands::AdmissionRequestLifecycle::until(Instant::now() + Duration::from_secs(30));
        let cleanup = lifecycle.revoke_with_cancellation_lease().unwrap();
        assert!(
            registry
                .issue_from_lifecycle(native_binding("supervised"), Some(&lifecycle), None)
                .is_err()
        );
        assert!(registry.requests.lock().unwrap().is_empty());
        assert!(
            lifecycle
                .verification
                .with_deadline(Instant::now() + Duration::from_secs(60))
                .check()
                .is_err()
        );
        assert_eq!(lifecycle.operations.load(Ordering::SeqCst), 1);
        drop(cleanup);
        assert_eq!(lifecycle.operations.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn supervised_issued_timeout_holds_cleanup_against_retirement() {
        let registry = commands::AdmissionRequestRegistry::default();
        let binding = native_binding("supervised-issued");
        let lifecycle =
            commands::AdmissionRequestLifecycle::until(Instant::now() + Duration::from_secs(30));
        let issued = registry
            .issue_from_lifecycle(binding.clone(), Some(&lifecycle), None)
            .unwrap();
        let capability = registry
            .supervised_capability(&lifecycle, &binding.command_id, &binding.idempotency_key)
            .unwrap();
        let authority = registry.resolve(&capability, &binding).unwrap();
        let cleanup = lifecycle.revoke_with_cancellation_lease().unwrap();
        authority.state.lock().unwrap().terminal = true;
        registry
            .issue(native_binding("retire-trigger-held"))
            .unwrap();
        assert!(
            registry
                .requests
                .lock()
                .unwrap()
                .contains_key(&issued.token)
        );
        assert!(authority.check().is_err());
        assert!(authority.effect_request().is_err());
        assert_eq!(issued.expires_at, lifecycle.expires_at);
        drop(cleanup);
        registry
            .issue(native_binding("retire-trigger-settled"))
            .unwrap();
        assert!(
            !registry
                .requests
                .lock()
                .unwrap()
                .contains_key(&issued.token)
        );
        assert!(lifecycle.revoke_with_cancellation_lease().is_none());
        assert_eq!(lifecycle.operations.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn supervised_original_deadline_and_admitted_lifetime_are_distinct() {
        let registry = commands::AdmissionRequestRegistry::default();
        let expired =
            commands::AdmissionRequestLifecycle::until(Instant::now() - Duration::from_millis(1));
        assert!(
            registry
                .issue_from_lifecycle(native_binding("expired-supervised"), Some(&expired), None)
                .is_err()
        );
        let lifecycle =
            commands::AdmissionRequestLifecycle::until(Instant::now() + Duration::from_secs(30));
        let binding = native_binding("admitted-supervised");
        registry
            .issue_from_lifecycle(binding.clone(), Some(&lifecycle), None)
            .unwrap();
        lifecycle.state.lock().unwrap().admitted_run = Some("owned-run".into());
        assert!(lifecycle.check().is_ok());
        let effects: Vec<_> = (0..3)
            .map(|_| {
                lifecycle
                    .verification
                    .with_deadline(Instant::now() + Duration::from_secs(60))
            })
            .collect();
        let cleanup = lifecycle.revoke_with_cancellation_lease().unwrap();
        assert!(lifecycle.check().is_err());
        assert!(effects.iter().all(|effect| effect.check().is_err()));
        drop(cleanup);
    }

    #[test]
    fn contested_authority_does_not_extend_control_processing_deadline_or_claim_revocation() {
        let registry = super::commands::AdmissionRequestRegistry::default();
        let authority = registry.reference("contested-control").unwrap();
        let permission = authority.state.lock().unwrap();
        let pending_authority = authority.clone();
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        let worker = std::thread::spawn(move || {
            pending_authority.revoke();
            let _ = sender.send(true);
        });
        let started = std::time::Instant::now();
        let result = super::wait_probe_cancellation(
            receiver,
            started + std::time::Duration::from_millis(80),
        );
        assert_eq!(
            result,
            serde_json::json!({"accepted":false,"code":"cancellation_pending"})
        );
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
        assert!(!permission.revoked);
        drop(permission);
        worker.join().unwrap();
        assert!(authority.state.lock().unwrap().revoked);
    }
    #[test]
    fn slow_fragmented_control_cannot_extend_absolute_connection_deadline() {
        use std::io::Write;
        let (server, mut client) = std::os::unix::net::UnixStream::pair().unwrap();
        let started = std::time::Instant::now();
        let worker = std::thread::spawn(move || {
            super::serve_probe_control(server, |_| panic!("Incomplete request cannot cancel"))
        });
        for _ in 0..5 {
            if client.write_all(b" ").is_err() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(800));
        }
        assert!(worker.join().unwrap().is_err());
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }
    use super::*;
    use std::{
        os::unix::net::UnixListener,
        process::{Command, Stdio},
        sync::atomic::AtomicUsize,
    };

    #[test]
    fn owned_cross_process_delayed_fragmented_cancel_reaches_closed_control_authority() {
        for (message, expected) in [
            ("{\"action\":\"cancel\"}", true),
            (
                "{\"action\":\"cancel\",\"private_canary\":\"private_value_canary\"}",
                false,
            ),
            ("{\"action\":\"other\"}", false),
        ] {
            let directory = PathBuf::from("/tmp").canonicalize().unwrap().join(format!(
                "magi-control-test-{}",
                uuid::Uuid::new_v4().simple()
            ));
            std::fs::create_dir(&directory).unwrap();
            std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).unwrap();
            let path = directory.join("cancel.sock");
            let listener = UnixListener::bind(&path).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
            listener.set_nonblocking(true).unwrap();
            let calls = Arc::new(AtomicUsize::new(0));
            let observed = calls.clone();
            let server = std::thread::spawn(move || {
                let stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(5))
                        }
                        Err(_) => panic!("Owned control accept failed"),
                    }
                };
                serve_probe_control(stream, |_| {
                    observed.fetch_add(1, Ordering::SeqCst);
                    serde_json::json!({"accepted":true})
                })
            });
            let script = "import socket,sys,time,json
s=socket.socket(socket.AF_UNIX)
s.connect(sys.argv[1])
time.sleep(0.1)
p=sys.argv[2].encode()
s.sendall(p[:5])
time.sleep(0.05)
s.sendall(p[5:])
s.shutdown(socket.SHUT_WR)
data=b''
while True:
 chunk=s.recv(4096)
 if not chunk: break
 data+=chunk
assert b'private_value_canary' not in data and b'private_canary' not in data
assert json.loads(data)['accepted']==(sys.argv[3]=='true')
";
            let status = Command::new("/usr/bin/python3")
                .args([
                    "-c",
                    script,
                    path.to_str().unwrap(),
                    message,
                    if expected { "true" } else { "false" },
                ])
                .env_clear()
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .unwrap();
            assert!(
                status.success(),
                "Owned cross-process control assertion failed"
            );
            assert!(server.join().unwrap().is_ok());
            assert_eq!(calls.load(Ordering::SeqCst), usize::from(expected));
            std::fs::remove_file(path).unwrap();
            std::fs::remove_dir(directory).unwrap();
        }
    }
}

#[cfg(test)]
mod terminal_stream_settlement_tests {
    use super::*;

    async fn contested_admission_state_cannot_extend_settlement_capture_deadline() {
        let monitor = Arc::new(ProbeMonitor {
            outcome: AtomicU8::new(2),
            phase: Mutex::new(("terminal", Instant::now())),
            active_request: Mutex::new(None),
            started: Instant::now(),
            lifecycle: commands::AdmissionRequestLifecycle::until(
                Instant::now() + Duration::from_secs(2),
            ),
        });
        let provider_lease = monitor
            .lifecycle
            .verification
            .track_stream_operation()
            .unwrap();
        let native_lease = monitor.lifecycle.lease().unwrap();
        let (locked_sender, locked) = std::sync::mpsc::channel();
        let (release, held) = std::sync::mpsc::channel();
        let worker_monitor = monitor.clone();
        let worker = std::thread::spawn(move || {
            let _provider_lease = provider_lease;
            let _native_lease = native_lease;
            let _guard = worker_monitor.lifecycle.state.lock().unwrap();
            locked_sender.send(()).unwrap();
            held.recv().unwrap();
        });
        locked.recv_timeout(Duration::from_secs(1)).unwrap();
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../.local/verification")
            .join(format!(
                "contested-terminal-settlement-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
        std::fs::create_dir_all(&root).unwrap();
        let started = Instant::now();
        assert!(
            !monitor
                .record_terminal_settlement_until(&root, started + Duration::from_millis(20))
                .await
        );
        assert!(started.elapsed() < Duration::from_millis(250));
        let evidence: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.join("terminal-settlement.json")).unwrap())
                .unwrap();
        assert_eq!(evidence["settled"], false);
        assert_eq!(evidence["revoked"], false);
        assert_eq!(evidence["providerOperations"], 1);
        assert_eq!(evidence["nativeOperations"], 1);
        release.send(()).unwrap();
        worker.join().unwrap();
        assert!(
            monitor
                .record_terminal_settlement_until(&root, Instant::now() + Duration::from_secs(1))
                .await
        );
        assert!(monitor.lifecycle.verification.check().is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    async fn aborted_delivery_caller_cannot_publish_zero_settlement_before_worker_exit() {
        let monitor = ProbeMonitor {
            outcome: AtomicU8::new(2),
            phase: Mutex::new(("terminal", Instant::now())),
            active_request: Mutex::new(None),
            started: Instant::now(),
            lifecycle: commands::AdmissionRequestLifecycle::until(
                Instant::now() + Duration::from_secs(2),
            ),
        };
        let provider_lease = monitor
            .lifecycle
            .verification
            .track_stream_operation()
            .unwrap();
        let native_lease = monitor.lifecycle.revoke_with_cancellation_lease().unwrap();
        let (release, mut held) = tauri::async_runtime::channel::<()>(1);
        let worker = tauri::async_runtime::spawn(async move {
            let _provider_lease = provider_lease;
            let _native_lease = native_lease;
            let _ = held.recv().await;
        });
        let caller = tauri::async_runtime::spawn(async move {
            let _ = worker.await;
        });
        caller.abort();
        let _ = caller.await;
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../.local/verification")
            .join(format!(
                "terminal-stream-settlement-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
        std::fs::create_dir_all(&root).unwrap();
        assert!(
            !monitor
                .record_terminal_settlement_until(&root, Instant::now() + Duration::from_millis(20))
                .await
        );
        let evidence: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.join("terminal-settlement.json")).unwrap())
                .unwrap();
        assert_eq!(evidence["settled"], false);
        assert_eq!(evidence["providerOperations"], 1);
        assert_eq!(evidence["nativeOperations"], 1);
        assert!(monitor.lifecycle.verification.check().is_err());
        release.send(()).await.unwrap();
        assert!(
            monitor
                .record_terminal_settlement_until(&root, Instant::now() + Duration::from_secs(1))
                .await
        );
        let evidence: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.join("terminal-settlement.json")).unwrap())
                .unwrap();
        assert_eq!(evidence["settled"], true);
        for field in [
            "queuedVerifications",
            "verificationWorkers",
            "providerOperations",
            "unresolvedCleanup",
            "nativeOperations",
        ] {
            assert_eq!(evidence[field], 0);
        }
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn contested_admission_state_capture_control() {
        tauri::async_runtime::block_on(
            contested_admission_state_cannot_extend_settlement_capture_deadline(),
        );
    }
    #[test]
    fn aborted_delivery_caller_settlement_control() {
        tauri::async_runtime::block_on(
            aborted_delivery_caller_cannot_publish_zero_settlement_before_worker_exit(),
        );
    }
}

#[cfg(test)]
mod saved_activation_tests {
    use super::*;
    use magi_storage::Storage;
    use std::{
        fs,
        time::{Duration, Instant},
    };
    use tauri::async_runtime;
    struct ApplicationFixture(PathBuf);
    impl Drop for ApplicationFixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn saved_profile_fixture(root: &std::path::Path) -> (Storage, SavedProfileInput) {
        let storage = Storage::open_or_create(root).unwrap();
        let bindings = magi_domain::CoreId::ALL.map(|core_id| {
            let index = core_id as usize;
            let provider_profile_id = format!("saved-profile-{index}");
            let profile = storage
                .save_provider_profile(
                    &magi_domain::ProviderProfileInput {
                        provider_profile_id: provider_profile_id.clone(),
                        provider_id: "codex-acp".into(),
                        display_name: provider_profile_id.clone(),
                        account_alias: "fixture-account".into(),
                        authentication_method:
                            magi_domain::ProviderAuthenticationMethod::LocalSubscription,
                        secret_reference: None,
                        runtime_home_id: format!("runtime-{provider_profile_id}"),
                        credential_home: None,
                    },
                    None,
                    "2026-10-03T00:00:00Z",
                )
                .unwrap();
            let catalog = magi_domain::ProviderCatalogSnapshot::new(
                magi_domain::ProviderCatalogInput {
                    catalog_snapshot_id: format!("saved-catalog-{index}"),
                    provider_id: profile.provider_id.clone(),
                    provider_profile_id: profile.provider_profile_id.clone(),
                    profile_revision: profile.revision,
                    adapter_id: "codex-acp".into(),
                    adapter_version: "1".into(),
                    adapter_digest: magi_domain::Digest::from_bytes(b"saved-adapter"),
                    fetched_at: "2026-10-03T00:00:00Z".into(),
                },
                vec![magi_domain::ProviderCatalogModel {
                    model_id: "saved-model".into(),
                    name: Some("Saved model".into()),
                    description: None,
                    context_window_tokens: Some(4096),
                    max_output_tokens: Some(1024),
                }],
            )
            .unwrap()
            .with_negotiated_modes(magi_domain::NegotiatedModeState {
                current_mode_id: Some("agent".into()),
                modes: vec![magi_domain::ProviderCatalogMode {
                    mode_id: "agent".into(),
                    name: "Agent".into(),
                    description: None,
                }],
            })
            .unwrap()
            .with_artifact_set_digest(magi_domain::Digest::from_bytes(b"saved-catalog-set"))
            .unwrap();
            storage.save_provider_catalog_snapshot(&catalog).unwrap();
            let selection = storage
                .select_provider_model(&magi_storage::ProviderModelSelectionInput {
                    provider_profile_id: provider_profile_id.clone(),
                    profile_revision: profile.revision,
                    catalog_snapshot_id: catalog.catalog_snapshot_id.clone(),
                    catalog_digest: catalog.catalog_digest.clone(),
                    model_id: "saved-model".into(),
                    mode_id: Some("agent".into()),
                    expected_selection_revision: None,
                    updated_at: "2026-10-03T00:00:00Z".into(),
                })
                .unwrap();
            let core = storage
                .select_core_model(&magi_storage::CoreModelSelectionInput {
                    core_id,
                    provider_profile_id: provider_profile_id.clone(),
                    profile_revision: profile.revision,
                    model_selection_revision: selection.selection_revision,
                    expected_selection_revision: None,
                    updated_at: "2026-10-03T00:00:00Z".into(),
                })
                .unwrap();
            SavedProfileBinding {
                core_id,
                provider_profile_id,
                profile_revision: profile.revision,
                model_profile_revision: selection.binding.profile_revision,
                model_selection_revision: selection.selection_revision,
                model_id: selection.binding.model_id,
                mode_id: selection.binding.mode_id,
                core_selection_revision: Some(core.selection_revision),
            }
        });
        let input = SavedProfileInput {
            schema_version: 1,
            prepared_store_digest: "a".repeat(64),
            bindings,
        };
        (storage, input)
    }

    #[test]
    fn retained_backup_copies_coherently_without_mutating_original() {
        let root = std::env::temp_dir().join(format!(
            "magi-retained-backup-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir(&root).unwrap();
        let _owned = OwnedPreparationPath {
            path: root.clone(),
            keep: false,
        };
        let retained = root.join("retained");
        let (storage, _input) = saved_profile_fixture(&retained);
        drop(storage);
        let before = std::fs::read(retained.join("state/magi.sqlite")).unwrap();
        let copy = root.join("copy");
        let backup = root.join("backup");
        prepare_retained_saved_profile_backup_at(retained.clone(), copy.clone(), backup.clone())
            .unwrap();
        assert!(!copy.exists());
        assert_eq!(
            std::fs::read(retained.join("state/magi.sqlite")).unwrap(),
            before
        );
        let manifest: magi_storage::BackupManifest =
            serde_json::from_slice(&std::fs::read(backup.join("manifest.json")).unwrap()).unwrap();
        assert_eq!(manifest.schema_version, 18);
        let restored = Storage::restore_backup(&backup, &root.join("verified-restored")).unwrap();
        assert_eq!(restored.store_id, manifest.store_id);
        assert!(prepare_retained_saved_profile_backup_at(retained, copy, backup).is_err());
    }

    #[test]
    fn saved_profile_preparation_uses_fresh_active_fence_before_activation() {
        let root = std::env::temp_dir().join(format!("saved-preparation-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let _cleanup = ApplicationFixture(root.clone());
        let source_root = root.join("source");
        let (storage, input) = saved_profile_fixture(&source_root);
        let expected_store = storage.identity().store_id.clone();
        let input_path = root.join("saved-input.json");
        write_saved_profile_input(&input_path, &input).unwrap();
        let backup = root.join("backup");
        storage.create_backup(&backup).unwrap();
        let backup_database = fs::read(backup.join("magi.sqlite")).unwrap();
        drop(storage);
        let inert_root = root.join("inert");
        Storage::restore_backup(&backup, &inert_root).unwrap();
        let inert = Storage::open_or_create(&inert_root).unwrap();
        assert!(!inert.admission_execution_authority().unwrap().active);
        assert!(
            async_runtime::block_on(retained_saved_preparation(
                inert,
                Instant::now() + Duration::from_secs(30),
            ))
            .is_err()
        );
        let fence_root = root.join("fence");
        let destination = root.join("destination");
        prepare_saved_profile_run_store_at(
            &fence_root,
            &backup,
            &destination,
            &input_path,
            &expected_store,
        )
        .unwrap();
        let active = Storage::open_or_create(&destination).unwrap();
        assert!(active.admission_execution_authority().unwrap().active);
        drop(active);
        assert_eq!(
            fs::read(backup.join("magi.sqlite")).unwrap(),
            backup_database
        );
        let fenced = Storage::open_or_create(&fence_root).unwrap();
        assert!(!fenced.admission_execution_authority().unwrap().active);
        assert!(destination.join("saved-profile-bindings.json").is_file());
    }

    #[test]
    fn saved_run_activation_retains_native_closure_and_reopens_active() {
        let root = std::env::temp_dir().join(format!("saved-activation-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let _cleanup = ApplicationFixture(root.clone());
        let source_root = root.join("source");
        let source = Storage::open_or_create(&source_root).unwrap();
        let initial_prior = source.admission_execution_authority().unwrap();
        source.create_backup(&root.join("backup")).unwrap();
        Storage::restore_backup(&root.join("backup"), &root.join("target")).unwrap();
        let mut target = Storage::open_or_create(root.join("target")).unwrap();
        assert!(!target.admission_execution_authority().unwrap().active);
        let (mut source, mut permission) = async_runtime::block_on(retained_saved_preparation(
            source,
            Instant::now() + Duration::from_secs(30),
        ))
        .unwrap();
        assert_eq!(
            permission.prior.store_generation,
            initial_prior.store_generation
        );
        let expected_prior = permission.prior.clone();
        let mut wrong = expected_prior.clone();
        wrong.lineage_id.push_str("-wrong");
        assert!(
            magi_storage::AdmissionActivationPermission::validate(&permission, &wrong).is_err()
        );
        let deadline = permission.deadline;
        permission.deadline = Instant::now();
        assert!(
            magi_storage::AdmissionActivationPermission::validate(&permission, &expected_prior)
                .is_err()
        );
        permission.deadline = deadline;
        let (active, permission) = target
            .activate_restored_execution_checked(&mut source, |expected| {
                magi_storage::AdmissionActivationPermission::validate(&permission, expected)?;
                Ok(permission)
            })
            .unwrap();
        assert!(active.active);
        assert!(!source.admission_execution_authority().unwrap().active);
        drop(target);
        drop(source);
        let reopened = Storage::open_or_create(root.join("target")).unwrap();
        let current = reopened.admission_execution_authority().unwrap();
        assert!(current.active);
        assert_eq!(current.lineage_id, active.lineage_id);
        permission.quiescence.validate().unwrap();
        let lifecycle =
            commands::AdmissionRequestLifecycle::until(Instant::now() + Duration::from_secs(30));
        assert!(lifecycle.check_execution().is_err());
        let lifecycle =
            commands::AdmissionRequestLifecycle::until(Instant::now() + Duration::from_secs(30));
        lifecycle.bind_execution(Arc::new(reopened)).unwrap();
        lifecycle.check_execution().unwrap();
    }

    #[test]
    fn saved_run_activation_rejects_real_unsettled_source_without_switch() {
        let (root, source, _, _) =
            crate::profiles::catalog_selection_ipc_tests::publication_fault_fixture();
        let _cleanup = ApplicationFixture(root.clone());
        source
            .create_backup(&root.join("activation-backup"))
            .unwrap();
        Storage::restore_backup(
            &root.join("activation-backup"),
            &root.join("activation-target"),
        )
        .unwrap();
        let mut target = Storage::open_or_create(root.join("activation-target")).unwrap();
        let (mut source, permission) = async_runtime::block_on(retained_saved_preparation(
            source,
            Instant::now() + Duration::from_secs(30),
        ))
        .unwrap();
        assert!(
            target
                .activate_restored_execution_checked(&mut source, |_| Ok(permission))
                .is_err()
        );
        assert!(source.admission_execution_authority().unwrap().active);
        assert!(!target.admission_execution_authority().unwrap().active);
        let lifecycle =
            commands::AdmissionRequestLifecycle::until(Instant::now() + Duration::from_secs(30));
        assert!(lifecycle.bind_execution(Arc::new(target)).is_err());
        assert!(lifecycle.check_execution().is_err());
    }
}
