use crate::commands::DesktopState;
use magi_domain::{Digest, RolePresetRevision, canonical_json};
use magi_storage::{RolePresetInput, Storage};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock, mpsc},
    time::{SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, State, WebviewWindow};
use tauri_plugin_dialog::DialogExt;

const MAX_BYTES: usize = 8 * 1024 * 1024;
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoleImportPreview {
    preview_token: String,
    file_digest: Digest,
    preset: RolePresetRevision,
    duplicate_perspective: bool,
}
struct PendingImport {
    preview: RoleImportPreview,
    created: u64,
    new_id: String,
    applied: Option<(Option<String>, Option<u64>, RolePresetRevision)>,
}
static PREVIEWS: OnceLock<Mutex<BTreeMap<String, PendingImport>>> = OnceLock::new();
fn previews() -> &'static Mutex<BTreeMap<String, PendingImport>> {
    PREVIEWS.get_or_init(|| Mutex::new(BTreeMap::new()))
}
fn epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn main_only(window: &WebviewWindow) -> Result<(), String> {
    if window.label() == "main" {
        Ok(())
    } else {
        Err("Role files are available only in the main console.".into())
    }
}
fn parse(bytes: &[u8]) -> Result<RolePresetRevision, String> {
    if bytes.len() > MAX_BYTES {
        return Err("The role file exceeds the supported size.".into());
    }
    let text = std::str::from_utf8(bytes).map_err(|_| "The role file is not UTF-8.")?;
    let value = strict_json::parse(text)
        .map_err(|_| "The role file contains invalid or duplicate JSON fields.")?;
    let preset: RolePresetRevision = serde_json::from_value(value)
        .map_err(|_| "The role file does not match the closed role schema.")?;
    preset
        .validate()
        .map_err(|_| "The role file failed role or digest validation.")?;
    Ok(preset)
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture {
        root: PathBuf,
        storage: Storage,
    }
    impl Fixture {
        fn new() -> Self {
            let root =
                std::env::temp_dir().join(format!("magi-role-file-test-{}", uuid::Uuid::new_v4()));
            let storage = Storage::open_or_create(&root).unwrap();
            Self { root, storage }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
    fn factory(storage: &Storage) -> RolePresetRevision {
        storage
            .load_role_preset("factory.magi.default")
            .unwrap()
            .unwrap()
    }
    fn pending(preset: RolePresetRevision) -> PendingImport {
        PendingImport {
            preview: RoleImportPreview {
                preview_token: "preview".into(),
                file_digest: Digest::from_bytes(&canonical_json(&preset).unwrap()),
                preset,
                duplicate_perspective: false,
            },
            created: epoch(),
            new_id: uuid::Uuid::new_v4().to_string(),
            applied: None,
        }
    }
    #[test]
    fn role_import_rejects_unknown_duplicate_tampered_and_invalid_core_fields() {
        let f = Fixture::new();
        let preset = factory(&f.storage);
        let bytes = canonical_json(&preset).unwrap();
        assert_eq!(parse(&bytes).unwrap(), preset);
        let mut value = serde_json::to_value(&preset).unwrap();
        value["execute"] = serde_json::json!("sh");
        assert!(parse(&serde_json::to_vec(&value).unwrap()).is_err());
        let mut value = serde_json::to_value(&preset).unwrap();
        value["roles"][1]["core_id"] = value["roles"][0]["core_id"].clone();
        assert!(parse(&serde_json::to_vec(&value).unwrap()).is_err());
        let mut value = serde_json::to_value(&preset).unwrap();
        value["roles"][0]["review_purpose"] = serde_json::json!("");
        assert!(parse(&serde_json::to_vec(&value).unwrap()).is_err());
        let duplicate =
            String::from_utf8(bytes)
                .unwrap()
                .replacen('{', "{\"schema_version\":1,", 1);
        assert!(parse(duplicate.as_bytes()).is_err());
        assert!(parse(&vec![b' '; MAX_BYTES + 1]).is_err());
    }
    #[test]
    fn role_apply_is_revision_fenced_idempotent_and_never_activates_authority() {
        let f = Fixture::new();
        let previous_selection = f.storage.load_active_role_preset_selection().unwrap();
        let preset = factory(&f.storage);
        let mut first = pending(preset.clone());
        let saved = apply(&f.storage, &mut first, None, None).unwrap();
        assert_ne!(saved.preset_id, preset.preset_id);
        assert_eq!(saved.revision, 0);
        assert_eq!(apply(&f.storage, &mut first, None, None).unwrap(), saved);
        assert!(apply(&f.storage, &mut first, Some("different".into()), None).is_err());
        let mut edit = pending(preset.clone());
        let updated = apply(
            &f.storage,
            &mut edit,
            Some(saved.preset_id.clone()),
            Some(0),
        )
        .unwrap();
        assert_eq!(updated.revision, 1);
        let mut stale = pending(preset.clone());
        assert!(apply(&f.storage, &mut stale, Some(saved.preset_id), Some(0)).is_err());
        let mut factory_update = pending(preset);
        assert!(
            apply(
                &f.storage,
                &mut factory_update,
                Some("factory.magi.default".into()),
                Some(0)
            )
            .is_err()
        );
        assert_eq!(
            f.storage.load_active_role_preset_selection().unwrap(),
            previous_selection
        );
        assert!(f.storage.pending_dispatches(10).unwrap().is_empty());
    }
    #[test]
    fn role_export_is_complete_and_preserves_existing_files() {
        let f = Fixture::new();
        let bytes = canonical_json(&factory(&f.storage)).unwrap();
        let path = f.root.join("roles.json");
        export_new(&path, &bytes).unwrap();
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert!(export_new(&path, b"replace").is_err());
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert!(
            !fs::read_dir(&f.root).unwrap().any(|e| e
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".part"))
        );
        #[cfg(unix)]
        {
            let linked = f.root.join("linked.json");
            std::os::unix::fs::symlink(&path, &linked).unwrap();
            assert!(read_file(&linked).is_err());
        }
    }
}
fn read_file(path: &Path) -> Result<Vec<u8>, String> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file = options
        .open(path)
        .map_err(|_| "The selected role file could not be opened.")?;
    let metadata = file
        .metadata()
        .map_err(|_| "The role file metadata is unavailable.")?;
    if !metadata.is_file() || metadata.len() > MAX_BYTES as u64 {
        return Err("Choose a regular role JSON file within the supported size.".into());
    }
    let mut bytes = Vec::new();
    file.take(MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "The role file could not be read.")?;
    if bytes.len() > MAX_BYTES {
        return Err("The role file grew beyond the supported size.".into());
    }
    Ok(bytes)
}
#[tauri::command]
pub async fn preview_role_preset_import(
    window: WebviewWindow,
    app: AppHandle,
) -> Result<Option<RoleImportPreview>, String> {
    main_only(&window)?;
    let (tx, rx) = mpsc::sync_channel(1);
    app.dialog()
        .file()
        .add_filter("Role preset", &["json"])
        .pick_file(move |p| {
            let _ = tx.send(p);
        });
    let selected = tauri::async_runtime::spawn_blocking(move || rx.recv())
        .await
        .map_err(|_| "The role file dialog failed.")?
        .map_err(|_| "The role file dialog closed unexpectedly.")?;
    let Some(path) = selected else {
        return Ok(None);
    };
    let path = path
        .into_path()
        .map_err(|_| "The role file is not local.")?;
    let bytes = tauri::async_runtime::spawn_blocking(move || read_file(&path))
        .await
        .map_err(|_| "The role file reader failed.")??;
    let preset = parse(&bytes)?;
    let duplicate_perspective = preset.roles.iter().enumerate().any(|(i, a)| {
        preset.roles.iter().skip(i + 1).any(|b| {
            a.review_purpose == b.review_purpose
                && a.evaluation_criteria == b.evaluation_criteria
                && a.falsification_questions == b.falsification_questions
        })
    });
    let preview = RoleImportPreview {
        preview_token: uuid::Uuid::new_v4().to_string(),
        file_digest: Digest::from_bytes(&bytes),
        preset,
        duplicate_perspective,
    };
    let mut pending = previews()
        .lock()
        .map_err(|_| "The role preview state is unavailable.")?;
    pending.retain(|_, p| epoch().saturating_sub(p.created) < 600);
    if pending.len() >= 8
        && let Some(oldest) = pending
            .iter()
            .min_by_key(|(_, p)| p.created)
            .map(|(id, _)| id.clone())
    {
        pending.remove(&oldest);
    }
    pending.insert(
        preview.preview_token.clone(),
        PendingImport {
            preview: preview.clone(),
            created: epoch(),
            new_id: uuid::Uuid::new_v4().to_string(),
            applied: None,
        },
    );
    Ok(Some(preview))
}
fn apply(
    storage: &Storage,
    pending: &mut PendingImport,
    target: Option<String>,
    expected: Option<u64>,
) -> Result<RolePresetRevision, String> {
    if let Some((old_target, old_expected, preset)) = &pending.applied {
        return if old_target == &target && old_expected == &expected {
            Ok(preset.clone())
        } else {
            Err("The applied role preview cannot target another preset or revision.".into())
        };
    }
    if target.is_none() && expected.is_some() {
        return Err("A new role preset has no existing revision.".into());
    }
    let existing = target
        .as_deref()
        .map(|id| storage.load_role_preset(id))
        .transpose()
        .map_err(|_| "The target role preset is unavailable.")?
        .flatten();
    let mut roles = pending.preview.preset.roles.clone();
    for role in &mut roles {
        role.profile_id = existing
            .as_ref()
            .and_then(|preset| preset.roles.iter().find(|r| r.core_id == role.core_id))
            .map(|r| r.profile_id.clone())
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    }
    let input = RolePresetInput {
        preset_id: target.clone().unwrap_or_else(|| pending.new_id.clone()),
        display_name: pending.preview.preset.display_name.clone(),
        roles,
    };
    let saved=storage.save_role_preset(&input,expected,&epoch().to_string()).map_err(|_|"The role preset changed or cannot be replaced. Refresh it before applying this preview.")?;
    pending.applied = Some((target, expected, saved.clone()));
    Ok(saved)
}
#[tauri::command]
pub fn apply_role_preset_import(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    preview_token: String,
    target_preset_id: Option<String>,
    expected_revision: Option<u64>,
) -> Result<RolePresetRevision, String> {
    main_only(&window)?;
    let storage = state.storage()?;
    let mut map = previews()
        .lock()
        .map_err(|_| "The role preview state is unavailable.")?;
    let pending = map
        .get_mut(&preview_token)
        .filter(|p| epoch().saturating_sub(p.created) < 600)
        .ok_or("The role preview expired. Open the file again.")?;
    apply(&storage, pending, target_preset_id, expected_revision)
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoleExportReceipt {
    file_digest: Digest,
    preset_id: String,
    revision: u64,
}
struct TemporaryFile(PathBuf);
impl Drop for TemporaryFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
fn export_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if fs::symlink_metadata(path).is_ok() {
        return Err("Choose a new filename to preserve the existing file.".into());
    }
    let parent = path
        .parent()
        .ok_or("The export destination has no parent.")?;
    let temporary =
        TemporaryFile(parent.join(format!(".magi-roles-{}.part", uuid::Uuid::new_v4())));
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temporary.0)
        .map_err(|_| "The role export could not be created.")?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|_| "The role export could not be completed.")?;
    drop(file);
    fs::hard_link(&temporary.0, path)
        .map_err(|_| "The destination already exists or cannot accept the role export.")?;
    fs::File::open(parent)
        .and_then(|f| f.sync_all())
        .map_err(|_| "The role export could not be durably published.")?;
    Ok(())
}
#[tauri::command]
pub async fn export_role_preset(
    window: WebviewWindow,
    app: AppHandle,
    state: State<'_, DesktopState>,
    preset_id: String,
    revision: u64,
) -> Result<Option<RoleExportReceipt>, String> {
    main_only(&window)?;
    let preset = state
        .storage()?
        .load_role_preset_revision(&preset_id, revision)
        .map_err(|_| "The selected role revision is unavailable.")?
        .ok_or("The selected role revision is unavailable.")?;
    let bytes = canonical_json(&preset).map_err(|_| "The role export could not be encoded.")?;
    let (tx, rx) = mpsc::sync_channel(1);
    app.dialog()
        .file()
        .set_file_name("magi-roles.json")
        .save_file(move |p| {
            let _ = tx.send(p);
        });
    let selected = tauri::async_runtime::spawn_blocking(move || rx.recv())
        .await
        .map_err(|_| "The role export dialog failed.")?
        .map_err(|_| "The role export dialog closed unexpectedly.")?;
    let Some(path) = selected else {
        return Ok(None);
    };
    let path = path
        .into_path()
        .map_err(|_| "The export destination is not local.")?;
    let digest = Digest::from_bytes(&bytes);
    tauri::async_runtime::spawn_blocking(move || export_new(&path, &bytes))
        .await
        .map_err(|_| "The role exporter failed.")??;
    Ok(Some(RoleExportReceipt {
        file_digest: digest,
        preset_id,
        revision,
    }))
}

mod strict_json {
    use serde::{
        Deserialize, Deserializer,
        de::{Error, MapAccess, SeqAccess, Visitor},
    };
    use serde_json::{Map, Number, Value};
    use std::fmt;
    struct StrictValue(Value);
    impl<'de> Deserialize<'de> for StrictValue {
        fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            d.deserialize_any(StrictVisitor)
        }
    }
    struct StrictVisitor;
    impl<'de> Visitor<'de> for StrictVisitor {
        type Value = StrictValue;
        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("JSON without duplicate object keys")
        }
        fn visit_bool<E: Error>(self, v: bool) -> Result<Self::Value, E> {
            Ok(StrictValue(Value::Bool(v)))
        }
        fn visit_i64<E: Error>(self, v: i64) -> Result<Self::Value, E> {
            Ok(StrictValue(Value::Number(v.into())))
        }
        fn visit_u64<E: Error>(self, v: u64) -> Result<Self::Value, E> {
            Ok(StrictValue(Value::Number(v.into())))
        }
        fn visit_f64<E: Error>(self, v: f64) -> Result<Self::Value, E> {
            Number::from_f64(v)
                .map(|n| StrictValue(Value::Number(n)))
                .ok_or_else(|| E::custom("invalid JSON number"))
        }
        fn visit_str<E: Error>(self, v: &str) -> Result<Self::Value, E> {
            Ok(StrictValue(Value::String(v.into())))
        }
        fn visit_string<E: Error>(self, v: String) -> Result<Self::Value, E> {
            Ok(StrictValue(Value::String(v)))
        }
        fn visit_unit<E: Error>(self) -> Result<Self::Value, E> {
            Ok(StrictValue(Value::Null))
        }
        fn visit_none<E: Error>(self) -> Result<Self::Value, E> {
            self.visit_unit()
        }
        fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Self::Value, A::Error> {
            let mut values = Vec::new();
            while let Some(StrictValue(value)) = sequence.next_element()? {
                values.push(value);
            }
            Ok(StrictValue(Value::Array(values)))
        }
        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
            let mut values = Map::new();
            while let Some(key) = map.next_key::<String>()? {
                if values.contains_key(&key) {
                    return Err(A::Error::custom("duplicate JSON object key"));
                }
                let StrictValue(value) = map.next_value()?;
                values.insert(key, value);
            }
            Ok(StrictValue(Value::Object(values)))
        }
    }
    pub(crate) fn parse(text: &str) -> Result<Value, serde_json::Error> {
        serde_json::from_str::<StrictValue>(text).map(|v| v.0)
    }
}
