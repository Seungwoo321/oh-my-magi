use serde::{Deserialize, Serialize};
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::{
    fs::{self, OpenOptions},
    io::Read,
    path::PathBuf,
    sync::Mutex,
};
use tauri::{AppHandle, Emitter, Manager, WebviewWindow};

#[path = "preferences_authority.rs"]
mod authority;

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

pub use authority::{SettingsCommand, SettingsReceipt, SettingsSnapshot};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReadPreferencesInput {
    defaults: ConsolePreferences,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationState {
    Delivered,
    Pending,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NotificationResult {
    state: NotificationState,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommittedPreferences {
    schema_version: u8,
    receipt: SettingsReceipt,
    notification: NotificationResult,
}

#[tauri::command]
pub fn get_console_preferences(
    window: WebviewWindow,
    app: AppHandle,
    input: ReadPreferencesInput,
) -> Result<SettingsSnapshot, String> {
    ensure_console_window(&window)?;
    let _guard = PREFERENCES_LOCK
        .lock()
        .map_err(|_| "Console preferences are temporarily unavailable.")?;
    let directory = state_directory(&app)?;
    let (mut connection, snapshot) = activate(&directory, input.defaults)?;
    let _ = deliver_events(&mut connection, |event| {
        app.emit("magi:preferences-changed", event).is_ok()
    });
    Ok(snapshot)
}

#[tauri::command]
pub fn save_console_preferences(
    window: WebviewWindow,
    app: AppHandle,
    input: SettingsCommand,
) -> Result<CommittedPreferences, String> {
    ensure_console_window(&window)?;
    let _guard = PREFERENCES_LOCK
        .lock()
        .map_err(|_| "Console preferences are temporarily unavailable.")?;
    let directory = state_directory(&app)?;
    commit_at(
        &directory,
        &input,
        &crate::profiles::now_rfc3339(),
        |event| app.emit("magi:preferences-changed", event).is_ok(),
    )
}

fn activate(
    directory: &std::path::Path,
    defaults: ConsolePreferences,
) -> Result<(rusqlite::Connection, SettingsSnapshot), String> {
    let mut connection = open_authority(directory)?;
    let version: u32 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|_| settings_error())?;
    let legacy = if version == 0 {
        read_legacy(directory)?
    } else {
        None
    };
    authority::initialize_with_defaults(&mut connection, legacy.as_deref(), defaults)
        .map_err(map_error)?;
    let snapshot = authority::load(&connection).map_err(map_error)?;
    Ok((connection, snapshot))
}

fn commit_at(
    directory: &std::path::Path,
    input: &SettingsCommand,
    at: &str,
    emit: impl FnMut(&authority::SettingsEvent) -> bool,
) -> Result<CommittedPreferences, String> {
    let mut connection = open_authority(directory)?;
    let current = authority::load(&connection).map_err(map_error)?;
    authority::initialize_with_defaults(&mut connection, None, current.preferences)
        .map_err(map_error)?;
    let receipt = authority::apply(&mut connection, input, at).map_err(map_error)?;
    let state = deliver_events(&mut connection, emit);
    Ok(CommittedPreferences {
        schema_version: 1,
        receipt,
        notification: NotificationResult { state },
    })
}

fn settings_error() -> String {
    "Console preference authority is unavailable or invalid.".into()
}
fn map_error(error: authority::SettingsError) -> String {
    match error {
        authority::SettingsError::Conflict => {
            "Console preference field revision conflict; input was retained.".into()
        }
        authority::SettingsError::IdempotencyConflict => {
            "Console preference command identity conflict.".into()
        }
        _ => settings_error(),
    }
}

fn deliver_events(
    connection: &mut rusqlite::Connection,
    mut emit: impl FnMut(&authority::SettingsEvent) -> bool,
) -> NotificationState {
    let page = match authority::pending_events(connection, 100) {
        Ok(page) => page,
        Err(_) => return NotificationState::Pending,
    };
    for event in page.events {
        if !emit(&event) || authority::acknowledge_event(connection, &event).is_err() {
            return NotificationState::Pending;
        }
    }
    if page.has_more {
        NotificationState::Pending
    } else {
        NotificationState::Delivered
    }
}

fn ensure_console_window(window: &WebviewWindow) -> Result<(), String> {
    if window.label() == "main" || window.label() == "companion" {
        Ok(())
    } else {
        Err("This window cannot access console preferences.".into())
    }
}

fn inspect_private_file(path: &std::path::Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                if metadata.uid() != unsafe { libc::geteuid() } || metadata.nlink() != 1 {
                    return Err(settings_error());
                }
            }
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(settings_error());
            }
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(settings_error()),
    }
}

fn open_authority(directory: &std::path::Path) -> Result<rusqlite::Connection, String> {
    inspect_directory_chain(directory)?;
    let metadata = fs::symlink_metadata(directory).map_err(|_| settings_error())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != unsafe { libc::geteuid() } {
            return Err(settings_error());
        }
    }
    for name in [
        "console-preferences.sqlite",
        "console-preferences.sqlite-wal",
        "console-preferences.sqlite-shm",
    ] {
        inspect_private_file(&directory.join(name))?;
    }
    let path = directory.join("console-preferences.sqlite");
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW).mode(0o600);
    let pinned = options.open(&path).map_err(|_| settings_error())?;
    let connection = rusqlite::Connection::open_with_flags(
        &path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .map_err(|_| settings_error())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let before = pinned.metadata().map_err(|_| settings_error())?;
        let after = fs::symlink_metadata(&path).map_err(|_| settings_error())?;
        if (before.dev(), before.ino()) != (after.dev(), after.ino()) {
            return Err(settings_error());
        }
    }
    connection
        .busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|_| settings_error())?;
    Ok(connection)
}

fn read_legacy(directory: &std::path::Path) -> Result<Option<Vec<u8>>, String> {
    let path = directory.join("console-preferences.json");
    inspect_private_file(&path)?;
    let metadata = match fs::symlink_metadata(&path) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(settings_error()),
    };
    if metadata.len() > MAX_PREFERENCES_BYTES {
        return Err(settings_error());
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW);
    let file = options.open(path).map_err(|_| settings_error())?;
    let mut bytes = Vec::new();
    file.take(MAX_PREFERENCES_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| settings_error())?;
    if bytes.len() as u64 > MAX_PREFERENCES_BYTES {
        return Err(settings_error());
    }
    Ok(Some(bytes))
}

fn inspect_directory_chain(path: &std::path::Path) -> Result<(), String> {
    if !path.is_absolute() {
        return Err(settings_error());
    }
    let mut prefix = PathBuf::new();
    for component in path.components() {
        if matches!(
            component,
            std::path::Component::ParentDir | std::path::Component::CurDir
        ) {
            return Err(settings_error());
        }
        prefix.push(component.as_os_str());
        match fs::symlink_metadata(&prefix) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err(settings_error());
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(settings_error()),
        }
    }
    Ok(())
}

fn state_directory(app: &AppHandle) -> Result<PathBuf, String> {
    let root = app.path().app_data_dir().map_err(|_| settings_error())?;
    inspect_directory_chain(&root)?;
    fs::create_dir_all(&root).map_err(|_| settings_error())?;
    let directory = root.join("state");
    inspect_directory_chain(&directory)?;
    fs::create_dir_all(&directory).map_err(|_| settings_error())?;
    for path in [&root, &directory] {
        let metadata = fs::symlink_metadata(path).map_err(|_| settings_error())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if metadata.uid() != unsafe { libc::geteuid() } {
                return Err(settings_error());
            }
        }
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(settings_error());
        }
        #[cfg(unix)]
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|_| settings_error())?;
    }
    inspect_directory_chain(&directory)?;
    Ok(directory)
}

#[cfg(test)]
mod tests {
    use super::*;
    struct OwnedDirectory(PathBuf);
    impl OwnedDirectory {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("magi-preferences-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&path).unwrap();
            Self(path.canonicalize().unwrap())
        }
    }
    impl Drop for OwnedDirectory {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
    fn defaults() -> ConsolePreferences {
        ConsolePreferences {
            motion: MotionPreference::Reduced,
            sound: false,
            theme: ThemePreference::Command,
            font_scale: 100,
            language: UiLanguage::Ko,
        }
    }
    fn command(id: &str, font: u16, revision: u64) -> SettingsCommand {
        serde_json::from_value(serde_json::json!({"schemaVersion":1,"commandId":id,"idempotencyKey":format!("key-{id}"),"target":"console_preferences","patch":{"fontScale":font},"expectedFieldRevisions":{"fontScale":revision}})).unwrap()
    }

    #[test]
    fn production_adapter_preserves_os_defaults_restart_and_committed_notification_failure() {
        let root = OwnedDirectory::new();
        let (connection, snapshot) = activate(&root.0, defaults()).unwrap();
        assert_eq!(snapshot.preferences.motion, MotionPreference::Reduced);
        drop(connection);
        let input = command("font", 150, 0);
        let first = commit_at(&root.0, &input, "time", |_| false).unwrap();
        assert!(matches!(
            first.notification.state,
            NotificationState::Pending
        ));
        let mut full_defaults = defaults();
        full_defaults.motion = MotionPreference::Full;
        let (mut connection, restored) = activate(&root.0, full_defaults).unwrap();
        assert_eq!(restored.preferences.motion, MotionPreference::Reduced);
        assert_eq!(restored.preferences.font_scale, 150);
        assert_eq!(
            authority::pending_events(&mut connection, 100)
                .unwrap()
                .events
                .len(),
            1
        );
        drop(connection);
        let replay = commit_at(&root.0, &input, "later", |_| true).unwrap();
        assert_eq!(first.receipt, replay.receipt);
        assert!(matches!(
            replay.notification.state,
            NotificationState::Delivered
        ));
    }

    #[test]
    fn production_adapter_imports_once_and_never_resets_corruption() {
        let root = OwnedDirectory::new();
        let legacy = root.0.join("console-preferences.json");
        fs::write(&legacy, br#"{"schemaVersion":2,"preferences":{"motion":"off","sound":true,"theme":"clear","fontScale":125,"language":"ko"}}"#).unwrap();
        let (connection, snapshot) = activate(&root.0, defaults()).unwrap();
        assert_eq!(snapshot.preferences.motion, MotionPreference::Off);
        drop(connection);
        fs::write(&legacy, b"corrupt-after-one-time-import").unwrap();
        let (connection, restored) = activate(&root.0, defaults()).unwrap();
        assert_eq!(snapshot, restored);
        connection
            .execute("DELETE FROM settings_import", [])
            .unwrap();
        drop(connection);
        assert!(activate(&root.0, defaults()).is_err());
        let corrupt = OwnedDirectory::new();
        fs::write(corrupt.0.join("console-preferences.json"), b"corrupt").unwrap();
        assert!(activate(&corrupt.0, defaults()).is_err());
    }

    #[test]
    #[cfg(unix)]
    fn production_adapter_rejects_symlink_database_sidecar_legacy_and_directory() {
        use std::os::unix::fs::symlink;
        let root = OwnedDirectory::new();
        let other = OwnedDirectory::new();
        let original = other.0.join("private");
        fs::write(&original, b"unchanged").unwrap();
        for leaf in [
            "console-preferences.sqlite",
            "console-preferences.sqlite-wal",
            "console-preferences.sqlite-shm",
            "console-preferences.json",
        ] {
            let link = root.0.join(leaf);
            symlink(&original, &link).unwrap();
            assert!(activate(&root.0, defaults()).is_err());
            fs::remove_file(link).unwrap();
            let db = root.0.join("console-preferences.sqlite");
            if db.exists() {
                fs::remove_file(db).unwrap();
            }
        }
        let linked = root.0.join("linked");
        symlink(&other.0, &linked).unwrap();
        assert!(open_authority(&linked).is_err());
        assert_eq!(fs::read(original).unwrap(), b"unchanged");
    }
}
