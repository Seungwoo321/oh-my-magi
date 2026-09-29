use magi_domain::{
    CoreId, CoreRoleDefinition, ProviderAuthenticationMethod, ProviderProfileInput,
    RolePresetRevision, RunStatus,
};
use magi_storage::{
    ActiveProviderProfileSelection, ActiveRolePresetSelection, RolePresetInput, RolePresetSource,
    RunHistoryFilter, RunHistoryRequest,
};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Manager, State, WebviewWindow};
use uuid::Uuid;

use crate::commands::DesktopState;

const CODEX_ACP_PROVIDER: &str = "codex-acp";

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentRunDto {
    run_id: String,
    question: String,
    status: &'static str,
    created_at: String,
}

#[tauri::command]
pub fn list_recent_runs(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<Vec<RecentRunDto>, String> {
    ensure_main_window(&window)?;
    let page = state
        .storage()?
        .list_runs(&RunHistoryRequest {
            filter: RunHistoryFilter::default(),
            cursor: None,
            page_size: 5,
        })
        .map_err(|_| "Recent local records could not be loaded.")?;
    Ok(page
        .items
        .into_iter()
        .map(|run| RecentRunDto {
            run_id: run.run_id,
            question: run.question,
            status: run_status_code(&run.status),
            created_at: run.created_at,
        })
        .collect())
}

fn run_status_code(status: &RunStatus) -> &'static str {
    match status {
        RunStatus::Preparing => "preparing",
        RunStatus::AwaitingConfirmation => "awaiting_confirmation",
        RunStatus::IndependentReview => "independent_review",
        RunStatus::CrossReview => "cross_review",
        RunStatus::Synthesis => "synthesis",
        RunStatus::Balloting => "balloting",
        RunStatus::Paused { .. } => "paused",
        RunStatus::Interrupted { .. } => "interrupted",
        RunStatus::Cancelling => "cancelling",
        RunStatus::Completed { .. } => "completed",
        RunStatus::Cancelled => "cancelled",
        RunStatus::Failed { .. } => "failed",
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpAdapterDto {
    id: &'static str,
    display_name: &'static str,
    state: &'static str,
    reason: &'static str,
}

#[tauri::command]
pub fn list_acp_adapters(window: WebviewWindow) -> Result<Vec<AcpAdapterDto>, String> {
    ensure_main_window(&window)?;
    Ok(vec![AcpAdapterDto {
        id: CODEX_ACP_PROVIDER,
        display_name: "Codex ACP",
        state: "supported",
        reason: "runtime_unavailable",
    }])
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderProfileDto {
    provider_profile_id: String,
    revision: u64,
    provider_id: String,
    display_name: String,
    account_alias: String,
    authentication_method: &'static str,
    credential_configured: bool,
    digest: String,
    updated_at: String,
}

fn provider_profile_dto(profile: magi_storage::ProviderProfileSummary) -> ProviderProfileDto {
    ProviderProfileDto {
        provider_profile_id: profile.provider_profile_id,
        revision: profile.revision,
        provider_id: profile.provider_id,
        display_name: profile.display_name,
        account_alias: profile.account_alias,
        authentication_method: match profile.authentication_method {
            ProviderAuthenticationMethod::LocalSubscription => "local_subscription",
            ProviderAuthenticationMethod::ByokApi => "byok_api",
        },
        credential_configured: profile.credential_configured,
        digest: profile.digest.as_str().to_owned(),
        updated_at: profile.updated_at,
    }
}

#[tauri::command]
pub fn list_provider_profiles(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<Vec<ProviderProfileDto>, String> {
    ensure_main_window(&window)?;
    state
        .storage()?
        .list_provider_profiles(100)
        .map(|profiles| profiles.into_iter().map(provider_profile_dto).collect())
        .map_err(|_| "Local provider profiles could not be loaded.".into())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveProviderProfileDto {
    provider_id: String,
    provider_profile_id: String,
    selection_revision: u64,
    updated_at: String,
}

fn active_provider_dto(selection: ActiveProviderProfileSelection) -> ActiveProviderProfileDto {
    ActiveProviderProfileDto {
        provider_id: selection.provider_id,
        provider_profile_id: selection.provider_profile_id,
        selection_revision: selection.selection_revision,
        updated_at: selection.updated_at,
    }
}

#[tauri::command]
pub fn load_active_provider_profile_selection(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    provider_id: String,
) -> Result<Option<ActiveProviderProfileDto>, String> {
    ensure_main_window(&window)?;
    state
        .storage()?
        .load_active_provider_profile_selection(&provider_id)
        .map(|selection| selection.map(active_provider_dto))
        .map_err(|_| "The active provider profile could not be loaded.".into())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderProfileDraftDto {
    profile_id: Option<String>,
    expected_revision: Option<u64>,
    display_name: String,
    adapter_id: String,
}

#[tauri::command]
pub fn save_provider_profile(
    window: WebviewWindow,
    app: AppHandle,
    state: State<'_, DesktopState>,
    draft: ProviderProfileDraftDto,
) -> Result<ProviderProfileDto, String> {
    ensure_main_window(&window)?;
    if draft.display_name.trim().is_empty() || draft.display_name.len() > 128 {
        return Err("Enter a profile name within 128 bytes.".into());
    }
    if draft.adapter_id != CODEX_ACP_PROVIDER {
        return Err("The selected ACP adapter is not available for profile setup.".into());
    }

    let storage = state.storage()?;
    let is_new = draft.profile_id.is_none();
    let (profile_id, runtime_home_id, expected_revision, account_alias) = match draft.profile_id {
        Some(profile_id) => {
            let current = storage
                .load_provider_profile(&profile_id)
                .map_err(|_| "The local provider profile could not be loaded safely.")?
                .ok_or_else(|| "The provider profile no longer exists.".to_owned())?;
            if Some(current.revision) != draft.expected_revision {
                return Err(
                    "The provider profile revision changed. Reload it before saving.".into(),
                );
            }
            if current.provider_id != draft.adapter_id {
                return Err("An existing profile's adapter cannot be changed.".into());
            }
            (
                current.provider_profile_id,
                current.runtime_home_id,
                draft.expected_revision,
                draft.display_name.clone(),
            )
        }
        None if draft.expected_revision.is_none() => (
            Uuid::new_v4().to_string(),
            Uuid::new_v4().to_string(),
            None,
            draft.display_name.clone(),
        ),
        None => return Err("A new profile cannot specify an existing revision.".into()),
    };

    let provision = ensure_profile_home(&app, &runtime_home_id)?;
    let profile = ProviderProfileInput {
        provider_profile_id: profile_id,
        provider_id: draft.adapter_id,
        display_name: draft.display_name,
        account_alias,
        authentication_method: ProviderAuthenticationMethod::LocalSubscription,
        secret_reference: None,
        runtime_home_id,
    };
    let updated_at = now_rfc3339();
    let saved = match storage.save_provider_profile(&profile, expected_revision, &updated_at) {
        Ok(saved) => saved,
        Err(_) => {
            if is_new && provision.created {
                let _ = fs::remove_dir(&provision.path);
            }
            return Err(
                "The provider profile could not be saved. Check whether another window changed it."
                    .into(),
            );
        }
    };
    Ok(ProviderProfileDto {
        provider_profile_id: saved.provider_profile_id,
        revision: saved.revision,
        provider_id: saved.provider_id,
        display_name: saved.display_name,
        account_alias: saved.account_alias,
        authentication_method: "local_subscription",
        credential_configured: saved.secret_reference.is_some(),
        digest: saved.digest.as_str().to_owned(),
        updated_at,
    })
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSelectionDto {
    provider_id: String,
    provider_profile_id: String,
    selection_revision: u64,
    updated_at: String,
}

#[tauri::command]
pub fn set_active_provider_profile(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    provider_id: String,
    profile_id: String,
    expected_selection_revision: Option<u64>,
) -> Result<ProviderSelectionDto, String> {
    ensure_main_window(&window)?;
    state
        .storage()?
        .set_active_provider_profile(
            &provider_id,
            &profile_id,
            expected_selection_revision,
            &now_rfc3339(),
        )
        .map(|selection| ProviderSelectionDto {
            provider_id: selection.provider_id,
            provider_profile_id: selection.provider_profile_id,
            selection_revision: selection.selection_revision,
            updated_at: selection.updated_at,
        })
        .map_err(|_| "The active provider profile changed or could not be saved.".into())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderAdmissionDto {
    state: &'static str,
    reason: &'static str,
    checked_at: String,
}

#[tauri::command]
pub fn validate_provider_profile(
    window: WebviewWindow,
    app: AppHandle,
    state: State<'_, DesktopState>,
    profile_id: String,
) -> Result<ProviderAdmissionDto, String> {
    ensure_main_window(&window)?;
    let storage = state.storage()?;
    let profile = storage
        .load_provider_profile(&profile_id)
        .map_err(|_| "The provider profile could not be read safely.")?
        .ok_or_else(|| "The provider profile no longer exists.".to_owned())?;
    let home_is_valid = inspect_profile_home(&app, &profile.runtime_home_id).is_ok();
    Ok(ProviderAdmissionDto {
        state: "blocked",
        reason: if home_is_valid {
            "attestation_missing"
        } else {
            "home_mismatch"
        },
        checked_at: now_rfc3339(),
    })
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredRolePresetDto {
    preset_id: String,
    revision: u64,
    display_name: String,
    roles: [CoreRoleDefinition; 3],
    digest: String,
    source: &'static str,
}

fn role_preset_dto(preset: RolePresetRevision, source: RolePresetSource) -> StoredRolePresetDto {
    StoredRolePresetDto {
        preset_id: preset.preset_id,
        revision: preset.revision,
        display_name: preset.display_name,
        roles: preset.roles,
        digest: preset.digest.as_str().to_owned(),
        source: match source {
            RolePresetSource::Factory => "factory",
            RolePresetSource::User => "user",
        },
    }
}

#[tauri::command]
pub fn list_role_presets(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<Vec<StoredRolePresetDto>, String> {
    ensure_main_window(&window)?;
    let storage = state.storage()?;
    let summaries = storage
        .list_role_presets(100)
        .map_err(|_| "Role profiles could not be loaded from local storage.")?;
    summaries
        .into_iter()
        .map(|summary| {
            let revision = storage
                .load_role_preset_revision(&summary.preset_id, summary.revision)
                .map_err(|_| "A saved role profile could not be read safely.")?
                .ok_or_else(|| "A saved role profile revision is missing.".to_owned())?;
            Ok(role_preset_dto(revision, summary.source))
        })
        .collect()
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveRolePresetDto {
    preset_id: String,
    selection_revision: u64,
    updated_at: String,
}

fn active_role_dto(selection: ActiveRolePresetSelection) -> ActiveRolePresetDto {
    ActiveRolePresetDto {
        preset_id: selection.preset_id,
        selection_revision: selection.selection_revision,
        updated_at: selection.updated_at,
    }
}

#[tauri::command]
pub fn load_active_role_preset_selection(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
) -> Result<Option<ActiveRolePresetDto>, String> {
    ensure_main_window(&window)?;
    state
        .storage()?
        .load_active_role_preset_selection()
        .map(|selection| selection.map(active_role_dto))
        .map_err(|_| "The active role profile could not be loaded.".into())
}

#[tauri::command]
pub fn set_active_role_preset(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    preset_id: String,
    expected_selection_revision: u64,
) -> Result<ActiveRolePresetDto, String> {
    ensure_main_window(&window)?;
    state
        .storage()?
        .set_active_role_preset(&preset_id, expected_selection_revision, &now_rfc3339())
        .map(active_role_dto)
        .map_err(|_| "The active role profile changed or could not be saved.".into())
}

#[tauri::command]
pub fn clone_role_preset(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    source_preset_id: String,
    source_revision: u64,
    display_name: String,
) -> Result<StoredRolePresetDto, String> {
    ensure_main_window(&window)?;
    if display_name.trim().is_empty() || display_name.len() > 128 {
        return Err("Enter a role profile name within 128 bytes.".into());
    }
    state
        .storage()?
        .clone_role_preset(
            &source_preset_id,
            source_revision,
            &Uuid::new_v4().to_string(),
            &display_name,
            &now_rfc3339(),
        )
        .map(|preset| role_preset_dto(preset, RolePresetSource::User))
        .map_err(|_| "The selected role profile changed and could not be copied.".into())
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DraftCoreId {
    Melchior,
    Balthasar,
    Casper,
}

impl DraftCoreId {
    fn domain_id(self) -> CoreId {
        match self {
            Self::Melchior => CoreId::Melchior1,
            Self::Balthasar => CoreId::Balthasar2,
            Self::Casper => CoreId::Casper3,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RoleDefinitionDraftDto {
    core: DraftCoreId,
    label: String,
    perspective: String,
    criteria: Vec<String>,
    challenge_condition: Vec<String>,
    output_language: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RolePresetDraftDto {
    preset_id: Option<String>,
    expected_revision: Option<u64>,
    display_name: String,
    roles: Vec<RoleDefinitionDraftDto>,
}

#[tauri::command]
pub fn save_role_preset(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    draft: RolePresetDraftDto,
) -> Result<StoredRolePresetDto, String> {
    ensure_main_window(&window)?;
    if draft.display_name.trim().is_empty() || draft.display_name.len() > 128 {
        return Err("Enter a role profile name within 128 bytes.".into());
    }
    if draft.roles.len() != 3 {
        return Err("A role profile must define exactly three MAGI cores.".into());
    }

    let preset_id = draft
        .preset_id
        .clone()
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let storage = state.storage()?;
    let existing = match (&draft.preset_id, draft.expected_revision) {
        (Some(id), Some(expected)) => {
            let current = storage
                .load_role_preset(id)
                .map_err(|_| "The selected role profile could not be loaded safely.")?
                .ok_or_else(|| "The selected role profile no longer exists.".to_owned())?;
            if current.revision != expected {
                return Err("The role profile revision changed. Reload it before saving.".into());
            }
            Some(current)
        }
        (Some(_), None) => return Err("An existing role profile requires its revision.".into()),
        (None, None) => None,
        (None, Some(_)) => {
            return Err("A new role profile cannot specify an existing revision.".into());
        }
    };

    let mut roles_by_core = draft
        .roles
        .into_iter()
        .map(|role| (role.core.domain_id(), role))
        .collect::<std::collections::BTreeMap<_, _>>();
    if roles_by_core.len() != 3
        || CoreId::ALL
            .iter()
            .any(|core| !roles_by_core.contains_key(core))
    {
        return Err("A role profile must define each MAGI core exactly once.".into());
    }

    let mut definitions = Vec::with_capacity(3);
    for core_id in CoreId::ALL {
        let role = roles_by_core
            .remove(&core_id)
            .ok_or_else(|| "A MAGI core role is missing.".to_owned())?;
        let profile_id = existing
            .as_ref()
            .and_then(|preset| {
                preset
                    .roles
                    .iter()
                    .find(|current| current.core_id == core_id)
            })
            .map(|current| current.profile_id.clone())
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        definitions.push(CoreRoleDefinition {
            core_id,
            profile_id,
            display_name: role.label,
            review_purpose: role.perspective,
            evaluation_criteria: role.criteria,
            falsification_questions: role.challenge_condition,
            response_language: role.output_language,
        });
    }
    let roles: [CoreRoleDefinition; 3] = definitions
        .try_into()
        .map_err(|_| "A role profile must define exactly three MAGI cores.".to_owned())?;
    let saved = storage
        .save_role_preset(
            &RolePresetInput {
                preset_id,
                display_name: draft.display_name,
                roles,
            },
            draft.expected_revision,
            &now_rfc3339(),
        )
        .map_err(|_| {
            "The role profile could not be saved. Check whether another window changed it."
                .to_owned()
        })?;
    Ok(role_preset_dto(saved, RolePresetSource::User))
}

struct ProvisionedHome {
    path: PathBuf,
    created: bool,
}

fn ensure_profile_home(app: &AppHandle, runtime_home_id: &str) -> Result<ProvisionedHome, String> {
    if !is_safe_path_component(runtime_home_id) {
        return Err("The local provider home binding is invalid.".into());
    }
    let data_root = app
        .path()
        .app_data_dir()
        .map_err(|_| "The application data location is unavailable.")?;
    let data_root = fs::canonicalize(data_root)
        .map_err(|_| "The application data location could not be inspected safely.")?;
    let provider_root = data_root.join("provider-homes");
    ensure_private_directory(&provider_root)?;
    let canonical_provider_root = fs::canonicalize(&provider_root)
        .map_err(|_| "The provider home storage could not be inspected safely.")?;
    if !canonical_provider_root.starts_with(&data_root) {
        return Err("The provider home storage is outside the application data location.".into());
    }
    let path = canonical_provider_root.join(runtime_home_id);
    let created = match fs::create_dir(&path) {
        Ok(()) => true,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => false,
        Err(_) => return Err("The profile-specific provider home could not be created.".into()),
    };
    if let Err(error) = verify_private_child(&canonical_provider_root, &path) {
        if created {
            let _ = fs::remove_dir(&path);
        }
        return Err(error);
    }
    Ok(ProvisionedHome { path, created })
}

fn inspect_profile_home(app: &AppHandle, runtime_home_id: &str) -> Result<(), String> {
    if !is_safe_path_component(runtime_home_id) {
        return Err("The provider home binding is invalid.".into());
    }
    let data_root = app
        .path()
        .app_data_dir()
        .map_err(|_| "The application data location is unavailable.")?;
    let data_root = fs::canonicalize(data_root)
        .map_err(|_| "The application data location could not be inspected safely.")?;
    let provider_root = data_root.join("provider-homes");
    let metadata = fs::symlink_metadata(&provider_root)
        .map_err(|_| "The profile-specific provider home is unavailable.")?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err("The provider home storage must be a real directory.".into());
    }
    let canonical_provider_root = fs::canonicalize(&provider_root)
        .map_err(|_| "The profile-specific provider home is unavailable.")?;
    if !canonical_provider_root.starts_with(&data_root) {
        return Err("The provider home storage is outside the application data location.".into());
    }
    let path = canonical_provider_root.join(runtime_home_id);
    verify_private_child(&canonical_provider_root, &path)
}

fn ensure_private_directory(path: &Path) -> Result<(), String> {
    match fs::create_dir(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(_) => return Err("The provider home storage could not be created.".into()),
    }
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| "The provider home storage could not be inspected safely.")?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err("The provider home storage must be a real directory.".into());
    }
    set_private_permissions(path)
}

fn verify_private_child(parent: &Path, path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| "The profile-specific provider home is unavailable.")?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err("The profile-specific provider home must be a real directory.".into());
    }
    let canonical = fs::canonicalize(path)
        .map_err(|_| "The profile-specific provider home could not be inspected safely.")?;
    if !canonical.starts_with(parent) {
        return Err("The profile-specific provider home is outside its managed root.".into());
    }
    set_private_permissions(&canonical)?;
    Ok(())
}

#[cfg(unix)]
fn set_private_permissions(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|_| "Provider home permissions could not be secured.".into())
}

#[cfg(not(unix))]
fn set_private_permissions(_path: &Path) -> Result<(), String> {
    Ok(())
}

fn is_safe_path_component(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn now_rfc3339() -> String {
    let milliseconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64;
    let seconds = milliseconds / 1000;
    let days = (seconds / 86_400) as i64;
    let day_seconds = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    let hour = day_seconds / 3_600;
    let minute = day_seconds % 3_600 / 60;
    let second = day_seconds % 60;
    let fraction = milliseconds % 1000;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{fraction:03}Z")
}

fn civil_from_days(days_since_epoch: i64) -> (i64, i64, i64) {
    let shifted = days_since_epoch + 719_468;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year, month, day)
}

fn ensure_main_window(window: &WebviewWindow) -> Result<(), String> {
    if window.label() == "main" {
        Ok(())
    } else {
        Err("This command is available only in the main MAGI console.".into())
    }
}
