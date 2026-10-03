use crate::commands::DesktopState;
use magi_context::{Digest, FreshnessGrant, FreshnessObservation, FreshnessRead, FreshnessStatus};
use magi_storage::Storage;
use serde::Serialize;
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tauri::{State, WebviewWindow};

const GRANT_LIFETIME: Duration = Duration::from_secs(30 * 60);
const MAX_GRANTS: usize = 64;
struct Entry {
    grant: FreshnessGrant,
    selected_at: Instant,
}
type Grants = HashMap<(String, String), Entry>;
static GRANTS: OnceLock<Mutex<Grants>> = OnceLock::new();
fn grants() -> &'static Mutex<Grants> {
    GRANTS.get_or_init(|| Mutex::new(HashMap::new()))
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}
fn main_only(window: &WebviewWindow) -> Result<(), String> {
    if window.label() == "main" {
        Ok(())
    } else {
        Err("Source observation is available only in the main console.".into())
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FreshnessView {
    run_id: String,
    source_id: String,
    captured_digest: Digest,
    observed_digest: Option<Digest>,
    status: FreshnessStatus,
    observed_at_epoch_ms: u64,
    can_recheck: bool,
    reason: Option<String>,
}

fn source(storage: &Storage, run: &str, source: &str) -> Result<(Digest, String), String> {
    let dossier = storage
        .load_run_dossier(run)
        .map_err(|_| "The stored deliberation is unavailable.")?;
    let selected = dossier
        .snapshot
        .input
        .context_manifest
        .sources
        .iter()
        .find(|item| item.source_id == source)
        .ok_or("The source is not in this deliberation's immutable input.")?;
    let display = dossier
        .capture_manifest
        .as_ref()
        .and_then(|manifest| {
            manifest
                .content
                .sources
                .iter()
                .find(|item| item.source_id == source)
        })
        .map_or("selected source".to_owned(), |item| {
            item.display_name.clone()
        });
    Ok((selected.object_digest.clone(), display))
}

fn persist(
    storage: &Storage,
    run: &str,
    source: &str,
    captured: Digest,
    read: FreshnessRead,
) -> Result<FreshnessView, String> {
    storage
        .record_source_freshness(run, source, &read.observation)
        .map_err(|_| "The source observation could not be persisted.")?;
    Ok(FreshnessView {
        run_id: run.into(),
        source_id: source.into(),
        captured_digest: captured,
        observed_digest: read.observation.observed_digest,
        status: read.observation.status,
        observed_at_epoch_ms: read.observation.observed_at_epoch_ms,
        can_recheck: read.can_recheck,
        reason: read.reason.map(str::to_owned),
    })
}
fn unchecked(reason: &'static str) -> FreshnessRead {
    FreshnessRead {
        observation: FreshnessObservation {
            status: FreshnessStatus::Unchecked,
            observed_at_epoch_ms: now(),
            observed_digest: None,
        },
        can_recheck: false,
        reason: Some(reason),
    }
}

#[tauri::command]
pub(crate) async fn select_source_freshness_file(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    run_id: String,
    source_id: String,
) -> Result<Option<FreshnessView>, String> {
    main_only(&window)?;
    let storage = state.storage()?;
    let (captured, name) = source(&storage, &run_id, &source_id)?;
    let selected = crate::native_source_picker::select_request_with_title(
        &window,
        crate::native_source_picker::PickerRequest::SingleFile { filters: vec![] },
        format!("Select the current file for {name}"),
    )
    .await
    .map_err(|error| error.message().to_owned())?;
    let path = match selected {
        crate::native_source_picker::PickerOutcome::Cancelled => return Ok(None),
        crate::native_source_picker::PickerOutcome::Selected(mut paths)
            if paths.len() == 1 && paths[0].is_absolute() =>
        {
            paths.remove(0)
        }
        _ => return Err("The selected local path is unavailable.".to_owned()),
    };
    tauri::async_runtime::spawn_blocking(move || {
        let time = now();
        let grant = FreshnessGrant::selected_file(&path, magi_context::CaptureLimits::default_policy().max_file_bytes, time.saturating_add(GRANT_LIFETIME.as_millis() as u64)).map_err(|_| "The selected current file cannot be authorized safely. Select a readable regular file outside credential-sensitive locations.")?;
        let mut cache = grants().lock().map_err(|_| "Source observation is temporarily unavailable.")?;
        let read = grant.observe(&captured, now());
        let view = persist(&storage, &run_id, &source_id, captured, read)?;
        cache.remove(&(run_id.clone(), source_id.clone()));
        cache.retain(|_, item| item.selected_at.elapsed() < GRANT_LIFETIME);
        if cache.len() >= MAX_GRANTS && let Some(oldest) = cache.iter().min_by_key(|(_, item)| item.selected_at).map(|(key, _)| key.clone()) { cache.remove(&oldest); }
        if view.can_recheck { cache.insert((run_id, source_id), Entry { grant, selected_at: Instant::now() }); }
        Ok(Some(view))
    }).await.map_err(|_| "Source observation worker failed.")?
}

#[tauri::command]
pub(crate) async fn recheck_source_freshness(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    run_id: String,
    source_id: String,
) -> Result<FreshnessView, String> {
    main_only(&window)?;
    let storage = state.storage()?;
    tauri::async_runtime::spawn_blocking(move || {
        let (captured, _) = source(&storage, &run_id, &source_id)?;
        let mut cache = grants()
            .lock()
            .map_err(|_| "Source observation is temporarily unavailable.")?;
        let key = (run_id.clone(), source_id.clone());
        if cache
            .get(&key)
            .is_some_and(|entry| entry.selected_at.elapsed() >= GRANT_LIFETIME)
        {
            cache.remove(&key);
        }
        let read = cache.get(&key).map_or_else(
            || unchecked("explicit_source_reselection_required"),
            |entry| entry.grant.observe(&captured, now()),
        );
        if !read.can_recheck {
            cache.remove(&key);
        }
        persist(&storage, &run_id, &source_id, captured, read)
    })
    .await
    .map_err(|_| "Source observation worker failed.")?
}

#[tauri::command]
pub(crate) fn revoke_source_freshness_access(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    run_id: String,
    source_id: String,
) -> Result<FreshnessView, String> {
    main_only(&window)?;
    let storage = state.storage()?;
    let (captured, _) = source(&storage, &run_id, &source_id)?;
    let mut cache = grants()
        .lock()
        .map_err(|_| "Source observation is temporarily unavailable.")?;
    cache.remove(&(run_id.clone(), source_id.clone()));
    persist(
        &storage,
        &run_id,
        &source_id,
        captured,
        unchecked("source_observation_access_revoked"),
    )
}
