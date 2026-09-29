use serde::{Deserialize, Serialize};
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::PathBuf,
    sync::Mutex,
};
use tauri::{AppHandle, Manager, State, WebviewWindow};

use crate::commands::DesktopState;

const SCHEMA_VERSION: u8 = 2;
const LEGACY_SCHEMA_VERSION: u8 = 1;
const MAX_PREFERENCES_BYTES: u64 = 4 * 1024;
static PREFERENCES_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MotionPreference {
    Full,
    Reduced,
    Off,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemePreference {
    Command,
    Clear,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UiLanguage {
    Ko,
    En,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConsolePreferences {
    pub motion: MotionPreference,
    pub sound: bool,
    pub theme: ThemePreference,
    #[serde(default = "default_font_scale")]
    pub font_scale: u16,
    #[serde(default = "default_ui_language")]
    pub language: UiLanguage,
}

impl ConsolePreferences {
    fn validate(&self) -> Result<(), String> {
        if [100, 125, 150, 200].contains(&self.font_scale) {
            Ok(())
        } else {
            Err("Console text size is not supported.".into())
        }
    }
}

fn default_font_scale() -> u16 {
    100
}

fn default_ui_language() -> UiLanguage {
    UiLanguage::Ko
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredPreferences {
    schema_version: u8,
    preferences: ConsolePreferences,
}

#[tauri::command]
pub fn get_console_preferences(
    window: WebviewWindow,
    app: AppHandle,
) -> Result<Option<ConsolePreferences>, String> {
    ensure_console_window(&window)?;
    let _guard = PREFERENCES_LOCK
        .lock()
        .map_err(|_| "Console preferences are temporarily unavailable.")?;
    read_preferences(&app)
}

#[tauri::command]
pub fn save_console_preferences(
    window: WebviewWindow,
    app: AppHandle,
    state: State<'_, DesktopState>,
    preferences: ConsolePreferences,
) -> Result<(), String> {
    ensure_console_window(&window)?;
    if !state.is_storage_ready() {
        return Err("Local storage is unavailable; preferences were not saved.".into());
    }
    let _guard = PREFERENCES_LOCK
        .lock()
        .map_err(|_| "Console preferences are temporarily unavailable.")?;
    write_preferences(&app, preferences)
}

fn ensure_console_window(window: &WebviewWindow) -> Result<(), String> {
    if window.label() == "main" || window.label() == "companion" {
        Ok(())
    } else {
        Err("This window cannot access console preferences.".into())
    }
}

fn read_preferences(app: &AppHandle) -> Result<Option<ConsolePreferences>, String> {
    let directory = state_directory(app)?;
    let path = directory.join("console-preferences.json");
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("Console preferences could not be inspected.".into()),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err("Console preferences are not a regular local file.".into());
    }
    if metadata.len() > MAX_PREFERENCES_BYTES {
        return Err("Console preferences exceed the supported size.".into());
    }

    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW);
    let file = options
        .open(&path)
        .map_err(|_| "Console preferences could not be read safely.")?;
    let opened_metadata = file
        .metadata()
        .map_err(|_| "Console preferences could not be inspected.")?;
    if !opened_metadata.is_file() || opened_metadata.len() > MAX_PREFERENCES_BYTES {
        return Err("Console preferences failed local validation.".into());
    }
    let mut bytes = Vec::with_capacity(opened_metadata.len() as usize);
    file.take(MAX_PREFERENCES_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "Console preferences could not be read.")?;
    if bytes.len() as u64 > MAX_PREFERENCES_BYTES {
        return Err("Console preferences exceed the supported size.".into());
    }
    let stored: StoredPreferences = serde_json::from_slice(&bytes)
        .map_err(|_| "Console preferences are invalid and were not changed.")?;
    if ![LEGACY_SCHEMA_VERSION, SCHEMA_VERSION].contains(&stored.schema_version) {
        return Err("Console preferences use an unsupported version.".into());
    }
    stored.preferences.validate()?;
    Ok(Some(stored.preferences))
}

fn write_preferences(app: &AppHandle, preferences: ConsolePreferences) -> Result<(), String> {
    preferences.validate()?;
    let directory = state_directory(app)?;
    let destination = directory.join("console-preferences.json");
    match fs::symlink_metadata(&destination) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err("Console preferences are not a regular local file.".into());
        }
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            return Err("Console preferences could not be inspected.".into());
        }
        _ => {}
    }

    let stored = StoredPreferences {
        schema_version: SCHEMA_VERSION,
        preferences,
    };
    let bytes =
        serde_json::to_vec(&stored).map_err(|_| "Console preferences could not be encoded.")?;
    if bytes.len() as u64 > MAX_PREFERENCES_BYTES {
        return Err("Console preferences exceed the supported size.".into());
    }
    let temporary = directory.join(format!(".console-preferences-{}.tmp", uuid::Uuid::new_v4()));
    let result = write_atomically(&directory, &temporary, &destination, &bytes);
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

fn write_atomically(
    directory: &PathBuf,
    temporary: &PathBuf,
    destination: &PathBuf,
    bytes: &[u8],
) -> Result<(), String> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(temporary)
        .map_err(|_| "Console preferences could not be staged safely.")?;
    file.write_all(bytes)
        .map_err(|_| "Console preferences could not be written.")?;
    file.sync_all()
        .map_err(|_| "Console preferences could not be synchronized.")?;
    fs::rename(temporary, destination)
        .map_err(|_| "Console preferences could not be committed atomically.")?;
    fs::File::open(directory)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| "Console preference storage could not be synchronized.".into())
}

fn state_directory(app: &AppHandle) -> Result<PathBuf, String> {
    let data_root = app
        .path()
        .app_data_dir()
        .map_err(|_| "The application data location is unavailable.")?;
    let directory = data_root.join("state");
    fs::create_dir_all(&directory)
        .map_err(|_| "Console preference storage could not be created.")?;
    let metadata = fs::symlink_metadata(&directory)
        .map_err(|_| "Console preference storage could not be inspected.")?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err("Console preference storage is not a regular local directory.".into());
    }
    #[cfg(unix)]
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
        .map_err(|_| "Console preference storage permissions could not be secured.")?;
    Ok(directory)
}
