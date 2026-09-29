use cap_std::{
    ambient_authority,
    fs::{Dir, OpenOptions, OpenOptionsExt},
};
use magi_context::{
    CaptureDirective, CaptureLimits, MAX_MANIFEST_ITEMS, ManifestSource, ManifestSourceState,
    RepresentationKind, SourceCaptureManifest, SourceGrant, SourceOmission, SourceOmissionCode,
    classify_selected_path,
};
use magi_provider::{AdapterState, AdmissionReport};
use magi_storage::Storage;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Emitter, Manager, State, WebviewWindow};
use tauri_plugin_dialog::DialogExt;
use uuid::Uuid;

pub(crate) struct DesktopState {
    storage: Option<Arc<Storage>>,
}

impl DesktopState {
    pub(crate) fn open(app: &AppHandle) -> Self {
        let storage = app
            .path()
            .app_data_dir()
            .ok()
            .and_then(|data_root| Storage::open_or_create(data_root).ok())
            .map(Arc::new);
        Self { storage }
    }

    pub(crate) fn storage(&self) -> Result<Arc<Storage>, String> {
        self.storage
            .as_ref()
            .cloned()
            .ok_or_else(|| "Local storage is unavailable.".to_owned())
    }

    pub(crate) fn is_storage_ready(&self) -> bool {
        self.storage.is_some()
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConsoleSnapshot {
    schema_version: u8,
    connection: ConnectionState,
    storage: StorageState,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "lowercase")]
enum ConnectionState {
    Ready,
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

#[tauri::command]
pub fn get_console_snapshot(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<ConsoleSnapshot, String> {
    ensure_console_window(&window)?;
    let report = AdmissionReport::current();
    let connection = if matches!(report.state, AdapterState::Unavailable) {
        ConnectionState::Blocked
    } else {
        ConnectionState::Ready
    };
    let storage = if state.storage.is_some() {
        StorageState::Ready
    } else {
        StorageState::Error
    };
    Ok(ConsoleSnapshot {
        schema_version: 1,
        connection,
        storage,
    })
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
        capture_context_files(storage, draft_id, stored_revision, paths)
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
        capture_context_directory(storage, draft_id, stored_revision, path)
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

fn capture_context_files(
    storage: Arc<Storage>,
    draft_id: String,
    stored_revision: Option<u64>,
    paths: Vec<PathBuf>,
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
            let stored = storage
                .put_source_object(object.original_bytes())
                .map_err(|_| "A captured source could not be stored locally.")?;
            if stored.digest != object.object_digest || stored.byte_length != object.byte_length {
                return Err("A captured source failed its local integrity check.".into());
            }
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
    let canonical = fs::canonicalize(&path)
        .map_err(|_| "The selected folder could not be opened safely.")?;
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
        let stored = storage
            .put_source_object(object.original_bytes())
            .map_err(|_| "A captured source could not be stored locally.")?;
        if stored.digest != object.object_digest || stored.byte_length != object.byte_length {
            return Err("A captured source failed its local integrity check.".into());
        }
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
fn open_selected_file(path: &Path) -> Result<cap_std::fs::File, SourceOmissionCode> {
    let parent_path = path.parent().ok_or(SourceOmissionCode::AccessDenied)?;
    let filename = path.file_name().ok_or(SourceOmissionCode::AccessDenied)?;
    let parent = fs::canonicalize(parent_path).map_err(|_| SourceOmissionCode::AccessDenied)?;
    let directory = Dir::open_ambient_dir(parent, ambient_authority())
        .map_err(|_| SourceOmissionCode::AccessDenied)?;
    let metadata = directory
        .symlink_metadata(filename)
        .map_err(|_| SourceOmissionCode::AccessDenied)?;
    if metadata.file_type().is_symlink() {
        return Err(SourceOmissionCode::SymbolicLink);
    }
    if !metadata.is_file() {
        return Err(SourceOmissionCode::SpecialFile);
    }

    let mut options = OpenOptions::new();
    options.read(true).custom_flags(libc::O_NOFOLLOW);
    directory
        .open_with(filename, &options)
        .map_err(|_| SourceOmissionCode::AccessDenied)
}

#[cfg(not(unix))]
fn open_selected_file(_path: &Path) -> Result<cap_std::fs::File, SourceOmissionCode> {
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

fn context_summary(draft: magi_storage::ContextDraft) -> Result<ContextSelectionSummary, String> {
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
pub fn shell_confirm_exit(
    window: WebviewWindow,
    app: AppHandle,
    authorization: State<'_, ExitAuthorization>,
) -> Result<(), String> {
    if window.label() != "main" {
        return Err("Only the main console can confirm application exit.".into());
    }
    authorization.authorize();
    app.exit(0);
    Ok(())
}

pub(crate) fn focus_main_window(app: &AppHandle) -> Result<(), String> {
    let main = app
        .get_webview_window("main")
        .ok_or_else(|| "The main console window is unavailable.".to_owned())?;
    main.show().map_err(|error| error.to_string())?;
    main.set_focus().map_err(|error| error.to_string())
}
