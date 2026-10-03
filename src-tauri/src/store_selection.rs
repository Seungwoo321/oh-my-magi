use crate::commands::DesktopState;
use magi_storage::{
    AdmissionActivationPermission, AdmissionExecutionAuthority, Storage, StorageError,
    StorageReader, StoreIdentity,
};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};
use tauri::{AppHandle, Manager, State, WebviewWindow};

const LIMIT: u64 = 16 * 1024;
const LIFETIME: Duration = Duration::from_secs(15 * 60);
static SELECTION: OnceLock<Mutex<Option<HeldSelection>>> = OnceLock::new();

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Receipt {
    schema_version: u32,
    selection_token: String,
    command_id: String,
    execution_authorized: bool,
    execution_applied: bool,
    phase: String,
    attempts: u8,
    transfer_generation: Option<u64>,
    previous_authority: AdmissionExecutionAuthority,
    previous_root: PathBuf,
    previous_identity: StoreIdentity,
    previous_receipt: Option<Box<Receipt>>,
    target: PathBuf,
    identity: StoreIdentity,
    authority: AdmissionExecutionAuthority,
    device: u64,
    inode: u64,
    database_device: u64,
    database_inode: u64,
}
struct HeldSelection {
    receipt: Receipt,
    directory: fs::File,
    database: fs::File,
    reader: StorageReader,
    expires: Instant,
    activated: bool,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StoreState {
    store_id: String,
    store_generation: u64,
    location_kind: &'static str,
    location_label: String,
    pending_restart: bool,
    pending_selection: Option<PendingSelection>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PendingSelection {
    selection_token: String,
    command_id: String,
    target_store_id: String,
    target_store_generation: u64,
    location_label: String,
    schema_version: u32,
    activated: bool,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SelectionReview {
    selection_token: String,
    target_store_id: String,
    target_store_generation: u64,
    location_label: String,
    schema_version: u32,
    record_count: u64,
    requires_reauthorization: bool,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ActivationReceipt {
    target_store_id: String,
    restart_required: bool,
    requires_reauthorization: bool,
}
fn selections() -> &'static Mutex<Option<HeldSelection>> {
    SELECTION.get_or_init(|| Mutex::new(None))
}
fn main_window(window: &WebviewWindow) -> Result<(), String> {
    if window.label() != "main" {
        return Err("Store selection requires the main window.".into());
    }
    Ok(())
}
fn label(path: &Path) -> String {
    path.file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("Selected store")
        .to_owned()
}
fn private_directory(path: &Path) -> Result<fs::File, String> {
    if !path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err("Store path must be absolute and unambiguous.".into());
    }
    for ancestor in path.ancestors() {
        if fs::symlink_metadata(ancestor)
            .map_err(|_| "Store ancestor unavailable.")?
            .file_type()
            .is_symlink()
        {
            return Err("Store ancestors must not be symbolic links.".into());
        }
    }
    let m = fs::symlink_metadata(path).map_err(|_| "Store directory is unavailable.")?;
    if !m.is_dir() || m.uid() != unsafe { libc::geteuid() } || m.mode() & 0o022 != 0 {
        return Err("Store directory ownership or permissions are invalid.".into());
    }
    let held = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY | libc::O_CLOEXEC)
        .open(path)
        .map_err(|_| "Cannot hold store directory.")?;
    let h = held
        .metadata()
        .map_err(|_| "Store directory identity unavailable.")?;
    if (h.dev(), h.ino()) != (m.dev(), m.ino()) {
        return Err("Store directory changed.".into());
    }
    Ok(held)
}
fn held_file(path: &Path) -> Result<fs::File, String> {
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
        .map_err(|_| "Cannot hold store database.")?;
    let m = file
        .metadata()
        .map_err(|_| "Store database identity unavailable.")?;
    if !m.is_file() || m.uid() != unsafe { libc::geteuid() } || m.mode() & 0o022 != 0 {
        return Err("Store database ownership or permissions are invalid.".into());
    }
    Ok(file)
}
fn inspect(
    path: &Path,
    current: &StoreIdentity,
) -> Result<(StorageReader, StoreIdentity, AdmissionExecutionAuthority), String> {
    let reader = StorageReader::open_read_only(path)
        .map_err(|_| "The selected folder is not a verified store.")?;
    let identity = reader
        .identity()
        .map_err(|_| "Store identity is invalid.")?;
    let authority = reader
        .admission_execution_authority()
        .map_err(|_| "Store authority is invalid.")?;
    if identity == *current
        || identity.schema_version != current.schema_version
        || authority.active
        || authority.store_generation != identity.generation
    {
        return Err(
            "Select a compatible inert restored store distinct from the current store.".into(),
        );
    }
    Ok((reader, identity, authority))
}
impl HeldSelection {
    fn check(&self) -> Result<(), String> {
        if !self.activated && Instant::now() > self.expires {
            return Err("Store review expired; choose the folder again.".into());
        }
        private_directory(&self.receipt.target)?;
        held_file(&self.receipt.target.join("state/magi.sqlite"))?;
        let path = fs::symlink_metadata(&self.receipt.target)
            .map_err(|_| "Selected store disappeared.")?;
        let held = self
            .directory
            .metadata()
            .map_err(|_| "Selected store handle unavailable.")?;
        if !path.is_dir()
            || (path.dev(), path.ino()) != (held.dev(), held.ino())
            || (path.dev(), path.ino()) != (self.receipt.device, self.receipt.inode)
        {
            return Err("Selected store directory changed.".into());
        }
        let database = fs::symlink_metadata(self.receipt.target.join("state/magi.sqlite"))
            .map_err(|_| "Selected database disappeared.")?;
        let held_database = self
            .database
            .metadata()
            .map_err(|_| "Selected database handle unavailable.")?;
        if !database.is_file()
            || (database.dev(), database.ino()) != (held_database.dev(), held_database.ino())
        {
            return Err("Selected database changed.".into());
        }
        if self
            .reader
            .identity()
            .map_err(|_| "Selected identity unavailable.")?
            != self.receipt.identity
            || self
                .reader
                .admission_execution_authority()
                .map_err(|_| "Selected authority unavailable.")?
                != self.receipt.authority
        {
            return Err("Selected store authority changed.".into());
        }
        Ok(())
    }
}
fn read_receipt(root: &Path) -> Result<Option<Receipt>, String> {
    let path = root.join("selected-store.json");
    let m = match fs::symlink_metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("Store selection receipt unavailable.".into()),
        Ok(m) => m,
    };
    if !m.is_file()
        || m.uid() != unsafe { libc::geteuid() }
        || m.mode() & 0o077 != 0
        || m.len() > LIMIT
    {
        return Err("Store selection receipt is unsafe.".into());
    }
    let mut file = held_file(&path)?;
    if file
        .metadata()
        .map_err(|_| "Receipt identity unavailable.")?
        .ino()
        != m.ino()
    {
        return Err("Store selection receipt changed.".into());
    }
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "Cannot read store selection receipt.")?;
    if bytes.len() as u64 > LIMIT {
        return Err("Store selection receipt is too large.".into());
    }
    let value = crate::strict_json::parse(
        std::str::from_utf8(&bytes).map_err(|_| "Invalid selection encoding.")?,
    )
    .map_err(|_| "Ambiguous selection receipt.")?;
    let r: Receipt = serde_json::from_value(value).map_err(|_| "Invalid selection receipt.")?;
    if r.schema_version != 1
        || !r.target.is_absolute()
        || !r.previous_root.is_absolute()
        || (r.authority.active && !(r.execution_authorized && r.execution_applied))
        || (r.execution_applied && !r.execution_authorized)
        || !["stable", "opening", "opening_source", "transferring"].contains(&r.phase.as_str())
        || r.attempts > 3
        || uuid::Uuid::parse_str(&r.command_id).is_err()
        || uuid::Uuid::parse_str(&r.selection_token).is_err()
    {
        return Err("Unsupported selection receipt.".into());
    }
    Ok(Some(r))
}
fn persist(root: &Path, receipt: &Receipt) -> Result<(), String> {
    let directory = private_directory(root)?;
    let temporary = root.join(format!(".selected-store-{}.pending", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(|_| "Cannot prepare store selection receipt.")?;
        file.write_all(
            &serde_json::to_vec(receipt).map_err(|_| "Cannot encode selection receipt.")?,
        )
        .map_err(|_| "Cannot write selection receipt.")?;
        file.sync_all()
            .map_err(|_| "Cannot sync selection receipt.")?;
        fs::rename(&temporary, root.join("selected-store.json"))
            .map_err(|_| "Cannot commit selection receipt.")?;
        directory
            .sync_all()
            .map_err(|_| "Selection committed but durability is unknown.".to_owned())
    })();
    let _ = fs::remove_file(temporary);
    result
}
fn selected_root(default: &Path) -> Result<PathBuf, String> {
    let Some(r) = read_receipt(default)? else {
        return Ok(default.to_path_buf());
    };
    let directory = private_directory(&r.target)?;
    let m = directory
        .metadata()
        .map_err(|_| "Selected root unavailable.")?;
    if (m.dev(), m.ino()) != (r.device, r.inode) {
        return Err("Selected root identity changed.".into());
    }
    let database = held_file(&r.target.join("state/magi.sqlite"))?
        .metadata()
        .map_err(|_| "Selected database unavailable.")?;
    if (database.dev(), database.ino()) != (r.database_device, r.database_inode) {
        return Err("Selected database identity changed.".into());
    }
    let reader =
        StorageReader::open_read_only(&r.target).map_err(|_| "Selected store unavailable.")?;
    let identity = reader
        .identity()
        .map_err(|_| "Selected identity unavailable.")?;
    let authority = reader
        .admission_execution_authority()
        .map_err(|_| "Selected authority unavailable.")?;
    if identity != r.identity || authority != r.authority {
        return Err("Selected store proof changed.".into());
    }
    Ok(r.target)
}
fn same_store_at(actual: &StoreIdentity, expected: &StoreIdentity, generation: u64) -> bool {
    actual.store_id == expected.store_id
        && actual.schema_version == expected.schema_version
        && actual.generation == generation
}
fn proof_at(path: &Path) -> Result<(StoreIdentity, AdmissionExecutionAuthority), String> {
    let reader =
        StorageReader::open_read_only(path).map_err(|_| "Store recovery reader unavailable.")?;
    Ok((
        reader
            .identity()
            .map_err(|_| "Store recovery identity unavailable.")?,
        reader
            .admission_execution_authority()
            .map_err(|_| "Store recovery authority unavailable.")?,
    ))
}
fn recover_receipt(root: &Path, receipt: &mut Receipt) -> Result<(), String> {
    if receipt.phase == "stable" {
        return Ok(());
    }
    let directory = private_directory(&receipt.target)?
        .metadata()
        .map_err(|_| "Recovery directory unavailable.")?;
    let database = held_file(&receipt.target.join("state/magi.sqlite"))?
        .metadata()
        .map_err(|_| "Recovery database unavailable.")?;
    if (
        directory.dev(),
        directory.ino(),
        database.dev(),
        database.ino(),
    ) != (
        receipt.device,
        receipt.inode,
        receipt.database_device,
        receipt.database_inode,
    ) {
        return Err("Recovery custody changed; explicit review is required.".into());
    }
    let (target, authority) = proof_at(&receipt.target)?;
    if authority.lineage_id != receipt.authority.lineage_id
        || authority.store_generation != target.generation
    {
        return Err("Recovery execution lineage changed.".into());
    }
    match receipt.phase.as_str() {
        "opening" => {
            let next = receipt
                .identity
                .generation
                .checked_add(1)
                .ok_or("Store generation overflow.")?;
            if ![receipt.identity.generation, next]
                .into_iter()
                .any(|generation| same_store_at(&target, &receipt.identity, generation))
                || authority.active != receipt.authority.active
            {
                return Err("Unjournaled selected-store generation or authority drift.".into());
            }
        }
        "opening_source" => {
            if target != receipt.identity || authority != receipt.authority {
                return Err("Selected store changed during source open.".into());
            }
            let (prior, prior_authority) = proof_at(&receipt.previous_root)?;
            let next = receipt
                .previous_identity
                .generation
                .checked_add(1)
                .ok_or("Store generation overflow.")?;
            if ![receipt.previous_identity.generation, next]
                .into_iter()
                .any(|generation| same_store_at(&prior, &receipt.previous_identity, generation))
                || prior_authority.lineage_id != receipt.previous_authority.lineage_id
                || prior_authority.active != receipt.previous_authority.active
                || prior_authority.store_generation != prior.generation
            {
                return Err("Unjournaled source-store drift.".into());
            }
            receipt.previous_identity = prior;
            receipt.previous_authority = prior_authority;
        }
        "transferring" => {
            if !receipt.execution_authorized || receipt.execution_applied {
                return Err("Authority transfer lacks explicit pending consent.".into());
            }
            let generation = receipt
                .transfer_generation
                .ok_or("Missing authority transfer generation.")?;
            if generation
                != receipt
                    .identity
                    .generation
                    .max(receipt.previous_identity.generation)
                    .checked_add(1)
                    .ok_or("Store generation overflow.")?
            {
                return Err("Invalid authority-transfer generation.".into());
            }
            let (prior, prior_authority) = proof_at(&receipt.previous_root)?;
            if prior_authority.lineage_id != receipt.previous_authority.lineage_id
                || prior_authority.store_generation != prior.generation
            {
                return Err("Source recovery lineage changed.".into());
            }
            let source_unchanged =
                prior == receipt.previous_identity && prior_authority == receipt.previous_authority;
            let source_retired = same_store_at(&prior, &receipt.previous_identity, generation)
                && !prior_authority.active;
            if authority.active {
                if !source_retired || !same_store_at(&target, &receipt.identity, generation) {
                    return Err("Authority-transfer recovery has inconsistent roots.".into());
                }
                receipt.execution_applied = true;
            } else if target != receipt.identity || !(source_unchanged || source_retired) {
                return Err("Authority-transfer recovery state is unproven.".into());
            }
            receipt.previous_identity = prior;
            receipt.previous_authority = prior_authority;
        }
        _ => return Err("Unknown store recovery phase.".into()),
    }
    receipt.identity = target;
    receipt.authority = authority;
    receipt.phase = "stable".into();
    receipt.transfer_generation = None;
    persist(root, receipt)
}

#[derive(Debug)]
pub(crate) enum OpenSelectedStoreError {
    Storage(StorageError),
    Selection(String),
}
impl From<String> for OpenSelectedStoreError {
    fn from(message: String) -> Self {
        Self::Selection(message)
    }
}
impl From<&str> for OpenSelectedStoreError {
    fn from(message: &str) -> Self {
        Self::Selection(message.into())
    }
}
impl std::fmt::Display for OpenSelectedStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Storage(error) => write!(f, "{error}"),
            Self::Selection(message) => f.write_str(message),
        }
    }
}
pub(crate) fn open_selected_storage<F, P>(
    _app: &AppHandle,
    default: &Path,
    quiescence_permission: F,
) -> Result<Storage, OpenSelectedStoreError>
where
    F: FnOnce(&AdmissionExecutionAuthority) -> Result<P, StorageError>,
    P: AdmissionActivationPermission,
{
    bootstrap_storage(default, quiescence_permission)
}
fn bootstrap_storage<F, P>(
    default: &Path,
    quiescence_permission: F,
) -> Result<Storage, OpenSelectedStoreError>
where
    F: FnOnce(&AdmissionExecutionAuthority) -> Result<P, StorageError>,
    P: AdmissionActivationPermission,
{
    let Some(mut receipt) = read_receipt(default)? else {
        return Storage::open_or_create(default).map_err(OpenSelectedStoreError::Storage);
    };
    recover_receipt(default, &mut receipt)?;
    let target_root = selected_root(default)?;
    if receipt.attempts >= 3 {
        return Err("Store startup recovery requires a new explicit native review.".into());
    }
    receipt.attempts += 1;
    receipt.phase = "opening".into();
    persist(default, &receipt)?;
    let mut target =
        Storage::open_or_create(&target_root).map_err(OpenSelectedStoreError::Storage)?;
    if target.identity().store_id != receipt.identity.store_id
        || target.identity().schema_version != receipt.identity.schema_version
        || target.identity().generation
            != receipt
                .identity
                .generation
                .checked_add(1)
                .ok_or("Store generation overflow.")?
    {
        return Err("Selected store generation changed unexpectedly during writer open.".into());
    }
    receipt.identity = target.identity().clone();
    receipt.authority = target
        .admission_execution_authority()
        .map_err(OpenSelectedStoreError::Storage)?;
    if receipt.execution_authorized && !receipt.execution_applied {
        receipt.phase = "opening_source".into();
        persist(default, &receipt)?;
        let previous_reader = StorageReader::open_read_only(&receipt.previous_root)
            .map_err(|_| "Previous store is unavailable for authority transfer.")?;
        if previous_reader
            .identity()
            .map_err(|_| "Previous store identity unavailable.")?
            != receipt.previous_identity
        {
            return Err("Previous store changed after authority-transfer review.".into());
        }
        drop(previous_reader);
        let mut previous = Storage::open_or_create(&receipt.previous_root)
            .map_err(OpenSelectedStoreError::Storage)?;
        if previous.identity().store_id != receipt.previous_identity.store_id
            || previous.identity().schema_version != receipt.previous_identity.schema_version
            || previous.identity().generation
                != receipt
                    .previous_identity
                    .generation
                    .checked_add(1)
                    .ok_or("Store generation overflow.")?
        {
            return Err(
                "Previous store changed while opening the authority-transfer writer.".into(),
            );
        }
        receipt.previous_identity = previous.identity().clone();
        receipt.previous_authority = previous
            .admission_execution_authority()
            .map_err(OpenSelectedStoreError::Storage)?;
        receipt.transfer_generation = Some(
            receipt
                .identity
                .generation
                .max(receipt.previous_identity.generation)
                .checked_add(1)
                .ok_or("Store generation overflow.")?,
        );
        receipt.phase = "transferring".into();
        persist(default, &receipt)?;
        let (_authority, _permission) = target
            .activate_restored_execution_checked(&mut previous, quiescence_permission)
            .map_err(OpenSelectedStoreError::Storage)?;
        receipt.execution_applied = true;
    }
    let authority = target
        .admission_execution_authority()
        .map_err(|_| "Post-open execution authority unavailable.")?;
    if authority.active != (receipt.execution_authorized && receipt.execution_applied) {
        return Err("Writer open changed execution authority without explicit consent.".into());
    }
    receipt.identity = target.identity().clone();
    receipt.authority = authority;
    receipt.phase = "stable".into();
    receipt.attempts = 0;
    receipt.transfer_generation = None;
    persist(default, &receipt)?;
    Ok(target)
}

fn recover_pending_selection(
    receipt: Receipt,
    current: &StoreIdentity,
    current_authority: &AdmissionExecutionAuthority,
) -> Result<HeldSelection, String> {
    if receipt.previous_identity != *current
        || receipt.previous_authority != *current_authority
        || !receipt.execution_authorized
        || receipt.execution_applied
        || receipt.phase != "stable"
    {
        return Err("Pending store switch requires recovery review.".into());
    }
    let _source_directory = private_directory(&receipt.previous_root)?;
    let _source_database = held_file(&receipt.previous_root.join("state/magi.sqlite"))?;
    let source = StorageReader::open_read_only(&receipt.previous_root)
        .map_err(|_| "Pending source store unavailable.")?;
    if source
        .identity()
        .map_err(|_| "Pending source identity unavailable.")?
        != *current
        || source
            .admission_execution_authority()
            .map_err(|_| "Pending source authority unavailable.")?
            != *current_authority
    {
        return Err("Pending source store changed.".into());
    }
    let directory = private_directory(&receipt.target)?;
    let database = held_file(&receipt.target.join("state/magi.sqlite"))?;
    let database_identity = database
        .metadata()
        .map_err(|_| "Pending database custody unavailable.")?;
    if (database_identity.dev(), database_identity.ino())
        != (receipt.database_device, receipt.database_inode)
    {
        return Err("Pending target database changed.".into());
    }
    let (reader, identity, authority) = inspect(&receipt.target, current)?;
    if identity != receipt.identity || authority != receipt.authority {
        return Err("Pending target authority changed.".into());
    }
    let held = HeldSelection {
        receipt,
        directory,
        database,
        reader,
        expires: Instant::now() + LIFETIME,
        activated: true,
    };
    held.check()?;
    Ok(held)
}
fn commit_activation(control: &Path, held: &mut HeldSelection) -> Result<(), String> {
    if held.activated {
        let durable = read_receipt(control)?
            .ok_or("Store switch receipt is missing; recovery is required.")?;
        if serde_json::to_vec(&durable).map_err(|_| "Cannot verify selection receipt.")?
            != serde_json::to_vec(&held.receipt).map_err(|_| "Cannot verify selection receipt.")?
        {
            return Err("Store switch receipt changed; recovery is required.".into());
        }
    }
    // Recommit also resolves a previous directory-sync failure before acknowledging success.
    persist(control, &held.receipt)?;
    held.activated = true;
    Ok(())
}
fn pending_view(held: &HeldSelection) -> PendingSelection {
    PendingSelection {
        selection_token: held.receipt.selection_token.clone(),
        command_id: held.receipt.command_id.clone(),
        target_store_id: held.receipt.identity.store_id.clone(),
        target_store_generation: held.receipt.identity.generation,
        location_label: label(&held.receipt.target),
        schema_version: held.receipt.identity.schema_version,
        activated: true,
    }
}
#[tauri::command]
pub(crate) fn get_store_selection_state(
    window: WebviewWindow,
    app: AppHandle,
    state: State<'_, DesktopState>,
) -> Result<StoreState, String> {
    main_window(&window)?;
    let storage = state.storage()?;
    let identity = storage.identity();
    let root = app
        .path()
        .app_data_dir()
        .map_err(|_| "Store control directory unavailable.")?;
    let receipt = read_receipt(&root)?;
    let restored = receipt.as_ref().is_some_and(|r| r.identity == *identity);
    let pending = receipt.as_ref().filter(|r| r.identity != *identity);
    let pending_selection = if let Some(receipt) = pending {
        let authority = storage
            .admission_execution_authority()
            .map_err(|_| "Current authority unavailable.")?;
        let recovered = recover_pending_selection(receipt.clone(), identity, &authority)?;
        let mut lock = selections()
            .lock()
            .map_err(|_| "Store selection unavailable.")?;
        if let Some(held) = lock.as_mut().filter(|s| s.receipt.execution_authorized) {
            held.check()?;
            if held.receipt.selection_token != receipt.selection_token
                || held.receipt.command_id != receipt.command_id
            {
                return Err("Pending store switch ownership changed.".into());
            }
            commit_activation(&root, held)?;
        } else {
            crate::commands::acquire_update_admission(&app, &storage)?;
            *lock = Some(recovered);
        }
        Some(pending_view(
            lock.as_ref().ok_or("Pending store switch unavailable.")?,
        ))
    } else {
        None
    };
    Ok(StoreState {
        store_id: identity.store_id.clone(),
        store_generation: identity.generation,
        location_kind: if restored { "restored" } else { "default" },
        location_label: if restored {
            label(&receipt.as_ref().unwrap().target)
        } else {
            label(&root)
        },
        pending_restart: pending_selection.is_some(),
        pending_selection,
    })
}
fn restored_picker_path(
    outcome: Result<
        crate::native_source_picker::PickerOutcome,
        crate::native_source_picker::PickerError,
    >,
) -> Result<Option<PathBuf>, String> {
    use crate::native_source_picker::{PickerError, PickerOutcome};
    match outcome {
        Ok(PickerOutcome::Cancelled) => Ok(None),
        Ok(PickerOutcome::Selected(mut paths)) if paths.len() == 1 => Ok(paths.pop()),
        Ok(PickerOutcome::Selected(_)) | Err(PickerError::InvalidSelection) => {
            Err("The restored-store folder selection is invalid.".into())
        }
        Err(PickerError::Unavailable) => {
            Err("The restored-store folder picker is unavailable.".into())
        }
        Err(PickerError::TimedOut) => Err("The restored-store folder picker timed out.".into()),
    }
}

#[tauri::command]
pub(crate) async fn select_restored_store(
    window: WebviewWindow,
    app: AppHandle,
    state: State<'_, DesktopState>,
) -> Result<Option<SelectionReview>, String> {
    main_window(&window)?;
    let Some(path) =
        restored_picker_path(crate::native_source_picker::select(&window, true).await)?
    else {
        return Ok(None);
    };
    let directory = private_directory(&path)?;
    let database = held_file(&path.join("state/magi.sqlite"))?;
    let current = state.storage()?.identity().clone();
    let (reader, identity, authority) = inspect(&path, &current)?;
    let connection = rusqlite::Connection::open_with_flags(
        path.join("state/magi.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .map_err(|_| "Cannot read record count.")?;
    let count: u64 = connection.query_row("SELECT COUNT(*) FROM (SELECT run_id FROM runs UNION SELECT run_id FROM live_runs LIMIT 100001)",[],|row|row.get(0)).map_err(|_| "Cannot inspect restored records.")?;
    if count > 100000 {
        return Err("Restored record count exceeds the review limit.".into());
    }
    let database_metadata = database
        .metadata()
        .map_err(|_| "Database custody unavailable.")?;
    let metadata = directory
        .metadata()
        .map_err(|_| "Selected folder identity unavailable.")?;
    let control = app
        .path()
        .app_data_dir()
        .map_err(|_| "Store control directory unavailable.")?;
    let existing = read_receipt(&control)?;
    let mut previous = existing.clone().filter(|r| r.identity == current);
    if let Some(receipt) = previous.as_mut() {
        receipt.previous_receipt = None;
    }
    let persisted = existing.filter(|r| r.identity != current);
    let token = persisted
        .as_ref()
        .filter(|r| r.identity == identity && r.target == path)
        .map(|r| r.selection_token.clone())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let receipt = Receipt {
        schema_version: 1,
        selection_token: token.clone(),
        command_id: String::new(),
        execution_authorized: false,
        execution_applied: false,
        phase: "stable".into(),
        attempts: 0,
        transfer_generation: None,
        previous_authority: state
            .storage()?
            .admission_execution_authority()
            .map_err(|_| "Current execution authority unavailable.")?,
        previous_root: previous
            .as_ref()
            .map(|r| r.target.clone())
            .unwrap_or_else(|| control.clone()),
        previous_identity: current,
        previous_receipt: previous.map(Box::new),
        target: path.clone(),
        identity: identity.clone(),
        authority,
        device: metadata.dev(),
        inode: metadata.ino(),
        database_device: database_metadata.dev(),
        database_inode: database_metadata.ino(),
    };
    let mut lock = selections()
        .lock()
        .map_err(|_| "Store selection unavailable.")?;
    if lock
        .as_ref()
        .is_some_and(|s| s.receipt.execution_authorized)
    {
        return Err("Cancel the pending store switch before selecting another store.".into());
    }
    let mut held = HeldSelection {
        receipt,
        directory,
        database,
        reader,
        expires: Instant::now() + LIFETIME,
        activated: false,
    };
    if let Some(persisted) = persisted {
        if persisted.target != path || persisted.identity != identity {
            return Err(
                "A different store switch is pending; resolve it before selecting another folder."
                    .into(),
            );
        }
        held.receipt = persisted;
        held.activated = true;
    }
    held.check()?;
    *lock = Some(held);
    Ok(Some(SelectionReview {
        selection_token: token,
        target_store_id: identity.store_id,
        target_store_generation: identity.generation,
        location_label: label(&path),
        schema_version: identity.schema_version,
        record_count: count,
        requires_reauthorization: true,
    }))
}
#[tauri::command]
pub(crate) fn activate_restored_store(
    window: WebviewWindow,
    app: AppHandle,
    state: State<'_, DesktopState>,
    selection_token: String,
    expected_store_id: String,
    expected_store_generation: u64,
    command_id: String,
) -> Result<ActivationReceipt, String> {
    main_window(&window)?;
    uuid::Uuid::parse_str(&command_id).map_err(|_| "Invalid store-switch command identity.")?;
    let mut lock = selections()
        .lock()
        .map_err(|_| "Store selection unavailable.")?;
    let held = lock
        .as_mut()
        .filter(|s| s.receipt.selection_token == selection_token)
        .ok_or("Choose and review a restored store first.")?;
    held.check()?;
    let storage = state.storage()?;
    if !current_matches(
        storage.identity(),
        &held.receipt.previous_identity,
        &expected_store_id,
        expected_store_generation,
    ) {
        return Err("Current store changed; review the selection again.".into());
    }
    let control = app
        .path()
        .app_data_dir()
        .map_err(|_| "Store control directory unavailable.")?;
    if held.receipt.execution_authorized {
        if held.receipt.command_id != command_id {
            return Err("Store switch already belongs to another command.".into());
        }
    } else {
        crate::commands::acquire_update_admission(&app, &storage)?;
        held.receipt.command_id = command_id;
        held.receipt.execution_authorized = true;
    }
    commit_activation(&control, held)?;
    Ok(ActivationReceipt {
        target_store_id: held.receipt.identity.store_id.clone(),
        restart_required: true,
        requires_reauthorization: true,
    })
}
#[tauri::command]
pub(crate) fn cancel_store_selection(
    window: WebviewWindow,
    app: AppHandle,
    selection_token: String,
) -> Result<(), String> {
    main_window(&window)?;
    let mut lock = selections()
        .lock()
        .map_err(|_| "Store selection unavailable.")?;
    let held = lock
        .as_ref()
        .filter(|s| s.receipt.selection_token == selection_token)
        .ok_or("Unknown store selection.")?;
    if held.receipt.execution_authorized {
        let root = app
            .path()
            .app_data_dir()
            .map_err(|_| "Store control directory unavailable.")?;
        let persisted = read_receipt(&root)?;
        if persisted.as_ref().is_some_and(|persisted| {
            persisted.selection_token != selection_token
                || persisted.command_id != held.receipt.command_id
        }) {
            return Err("Store switch receipt changed; recovery is required.".into());
        }
        if let Some(previous) = held.receipt.previous_receipt.as_ref() {
            persist(&root, previous)?;
        } else {
            if persisted.is_some() {
                fs::remove_file(root.join("selected-store.json"))
                    .map_err(|_| "Cannot cancel store switch.")?;
            }
        }
        private_directory(&root)?
            .sync_all()
            .map_err(|_| "Store cancellation durability is unknown.")?;
        crate::commands::release_update_admission(&app);
    }
    *lock = None;
    Ok(())
}
#[tauri::command]
pub(crate) fn restart_into_selected_store(
    window: WebviewWindow,
    app: AppHandle,
    selection_token: String,
) -> Result<(), String> {
    main_window(&window)?;
    let lock = selections()
        .lock()
        .map_err(|_| "Store selection unavailable.")?;
    let held = lock
        .as_ref()
        .filter(|s| s.activated && s.receipt.selection_token == selection_token)
        .ok_or("No reviewed store switch is ready.")?;
    held.check()?;
    let root = app
        .path()
        .app_data_dir()
        .map_err(|_| "Store control directory unavailable.")?;
    let persisted = read_receipt(&root)?.ok_or("Store switch receipt is missing.")?;
    if persisted.command_id != held.receipt.command_id || persisted.target != held.receipt.target {
        return Err("Store switch receipt changed.".into());
    }
    app.restart()
}

fn current_matches(
    current: &StoreIdentity,
    reviewed: &StoreIdentity,
    expected_id: &str,
    expected_generation: u64,
) -> bool {
    current == reviewed
        && current.store_id == expected_id
        && current.generation == expected_generation
}
#[cfg(test)]
mod tests {
    use super::*;
    use magi_storage::Storage;
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("magi-store-selection-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&path).unwrap();
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
            Self(fs::canonicalize(path).unwrap())
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn restored(fixture: &Fixture) -> (Storage, PathBuf, Receipt) {
        let source = fixture.0.join("source");
        let original = Storage::open_or_create(&source).unwrap();
        let backup = fixture.0.join("backup");
        original.create_backup(&backup).unwrap();
        let target = fixture.0.join("restored");
        Storage::restore_backup(&backup, &target).unwrap();
        let (_, identity, authority) = inspect(&target, original.identity()).unwrap();
        let dir = private_directory(&target).unwrap().metadata().unwrap();
        let db = held_file(&target.join("state/magi.sqlite"))
            .unwrap()
            .metadata()
            .unwrap();
        let receipt = Receipt {
            schema_version: 1,
            selection_token: uuid::Uuid::new_v4().to_string(),
            command_id: uuid::Uuid::new_v4().to_string(),
            execution_authorized: false,
            execution_applied: false,
            phase: "stable".into(),
            attempts: 0,
            transfer_generation: None,
            previous_authority: original.admission_execution_authority().unwrap(),
            previous_root: source,
            previous_identity: original.identity().clone(),
            previous_receipt: None,
            target: target.clone(),
            identity,
            authority,
            device: dir.dev(),
            inode: dir.ino(),
            database_device: db.dev(),
            database_inode: db.ino(),
        };
        (original, target, receipt)
    }
    #[test]
    fn inert_restored_store_is_readable_without_writer_activation() {
        let f = Fixture::new();
        let (original, target, r) = restored(&f);
        assert!(!r.authority.active);
        assert!(inspect(&target, original.identity()).is_ok());
        assert!(inspect(&r.previous_root, original.identity()).is_err());
        persist(&f.0, &r).unwrap();
        assert_eq!(selected_root(&f.0).unwrap(), target);
        assert!(
            !StorageReader::open_read_only(&target)
                .unwrap()
                .admission_execution_authority()
                .unwrap()
                .active
        );
    }
    #[test]
    fn store_identity_and_generation_are_both_required_for_activation() {
        let identity = StoreIdentity {
            store_id: "reviewed".into(),
            generation: 3,
            schema_version: 16,
        };
        assert!(current_matches(&identity, &identity, "reviewed", 3));
        assert!(!current_matches(&identity, &identity, "other", 3));
        assert!(!current_matches(&identity, &identity, "reviewed", 4));
        let mut changed = identity.clone();
        changed.schema_version = 17;
        assert!(!current_matches(&changed, &identity, "reviewed", 3));
    }
    #[test]
    fn pending_receipt_rejects_duplicate_fields_symlinks_and_database_replacement() {
        let f = Fixture::new();
        let (_, target, r) = restored(&f);
        persist(&f.0, &r).unwrap();
        let db = target.join("state/magi.sqlite");
        let replacement = target.join("state/replacement.sqlite");
        fs::copy(&db, &replacement).unwrap();
        fs::rename(replacement, &db).unwrap();
        assert!(selected_root(&f.0).is_err());
        let receipt = f.0.join("selected-store.json");
        fs::remove_file(&receipt).unwrap();
        std::os::unix::fs::symlink(&db, &receipt).unwrap();
        assert!(read_receipt(&f.0).is_err());
        fs::remove_file(&receipt).unwrap();
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&receipt)
            .unwrap();
        file.write_all(b"{\"schemaVersion\":1,\"schemaVersion\":1}")
            .unwrap();
        assert!(read_receipt(&f.0).is_err());
    }
    #[test]
    fn expired_review_and_replaced_directory_invalidate_native_custody() {
        let f = Fixture::new();
        let (_, target, r) = restored(&f);
        let mut held = HeldSelection {
            directory: private_directory(&target).unwrap(),
            database: held_file(&target.join("state/magi.sqlite")).unwrap(),
            reader: StorageReader::open_read_only(&target).unwrap(),
            receipt: r,
            expires: Instant::now() - Duration::from_secs(1),
            activated: false,
        };
        assert!(held.check().is_err());
        held.expires = Instant::now() + LIFETIME;
        assert!(held.check().is_ok());
        fs::rename(&target, f.0.join("moved")).unwrap();
        fs::create_dir(&target).unwrap();
        assert!(held.check().is_err());
    }
    struct VerifiedPermission(AdmissionExecutionAuthority);
    impl AdmissionActivationPermission for VerifiedPermission {
        fn validate(&self, current: &AdmissionExecutionAuthority) -> Result<(), StorageError> {
            if &self.0 == current {
                Ok(())
            } else {
                Err(StorageError::DispatchFenced)
            }
        }
    }
    #[test]
    fn explicit_execution_consent_transfers_authority_and_survives_two_restarts() {
        let f = Fixture::new();
        let (original, target, mut receipt) = restored(&f);
        receipt.execution_authorized = true;
        persist(&f.0, &receipt).unwrap();
        drop(original);
        let selected =
            bootstrap_storage(&f.0, |authority| Ok(VerifiedPermission(authority.clone()))).unwrap();
        let first = selected.identity().clone();
        assert!(selected.admission_execution_authority().unwrap().active);
        assert_eq!(read_receipt(&f.0).unwrap().unwrap().identity, first);
        assert!(
            !StorageReader::open_read_only(&receipt.previous_root)
                .unwrap()
                .admission_execution_authority()
                .unwrap()
                .active
        );
        drop(selected);
        assert_eq!(selected_root(&f.0).unwrap(), target);
        let reopened = bootstrap_storage(&f.0, |_| {
            Err::<VerifiedPermission, _>(StorageError::DispatchFenced)
        })
        .unwrap();
        assert_eq!(reopened.identity().generation, first.generation + 1);
        assert!(reopened.admission_execution_authority().unwrap().active);
        assert_eq!(
            read_receipt(&f.0).unwrap().unwrap().identity,
            *reopened.identity()
        );
        drop(reopened);
        assert!(selected_root(&f.0).is_ok());
    }
    #[test]
    fn plain_store_selection_never_grants_execution_and_acknowledges_writer_generation() {
        let f = Fixture::new();
        let (original, _, receipt) = restored(&f);
        persist(&f.0, &receipt).unwrap();
        drop(original);
        let selected = bootstrap_storage::<_, VerifiedPermission>(&f.0, |_| {
            panic!("plain selection must not ask for activation permission")
        })
        .unwrap();
        assert!(!selected.admission_execution_authority().unwrap().active);
        assert_eq!(
            selected.identity().generation,
            receipt.identity.generation + 1
        );
        drop(selected);
        let reopened = bootstrap_storage(&f.0, |_| {
            Err::<VerifiedPermission, _>(StorageError::DispatchFenced)
        })
        .unwrap();
        assert!(!reopened.admission_execution_authority().unwrap().active);
    }
    #[test]
    fn unresolved_native_custody_refuses_transfer_even_with_durable_user_consent() {
        let f = Fixture::new();
        let (original, target, mut receipt) = restored(&f);
        receipt.execution_authorized = true;
        persist(&f.0, &receipt).unwrap();
        drop(original);
        let result = bootstrap_storage(&f.0, |_| {
            Err::<VerifiedPermission, _>(StorageError::DeletionBlocked)
        });
        assert!(result.is_err());
        assert!(
            !StorageReader::open_read_only(&target)
                .unwrap()
                .admission_execution_authority()
                .unwrap()
                .active
        );
        assert!(!read_receipt(&f.0).unwrap().unwrap().execution_applied);
    }
    #[test]
    fn crash_after_writer_open_recovers_only_the_journaled_single_generation_step() {
        let f = Fixture::new();
        let (original, target, mut receipt) = restored(&f);
        drop(original);
        receipt.phase = "opening".into();
        receipt.attempts = 1;
        persist(&f.0, &receipt).unwrap();
        let opened = Storage::open_or_create(&target).unwrap();
        drop(opened);
        let selected = bootstrap_storage(&f.0, |_| {
            Err::<VerifiedPermission, _>(StorageError::DispatchFenced)
        })
        .unwrap();
        assert_eq!(
            selected.identity().generation,
            receipt.identity.generation + 2
        );
        assert!(!selected.admission_execution_authority().unwrap().active);
        drop(selected);
        assert!(selected_root(&f.0).is_ok());
        let mut acknowledged = read_receipt(&f.0).unwrap().unwrap();
        acknowledged.phase = "opening".into();
        acknowledged.attempts = 1;
        persist(&f.0, &acknowledged).unwrap();
        drop(Storage::open_or_create(&target).unwrap());
        drop(Storage::open_or_create(&target).unwrap());
        assert!(
            bootstrap_storage(&f.0, |_| Err::<VerifiedPermission, _>(
                StorageError::DispatchFenced
            ))
            .is_err()
        );
    }
    #[test]
    fn crash_after_checked_authority_transfer_recovers_exact_coupled_roots_without_new_grant() {
        let f = Fixture::new();
        let (original, target, mut receipt) = restored(&f);
        drop(original);
        let mut selected = Storage::open_or_create(&target).unwrap();
        let mut previous = Storage::open_or_create(&receipt.previous_root).unwrap();
        receipt.execution_authorized = true;
        receipt.phase = "transferring".into();
        receipt.attempts = 1;
        receipt.identity = selected.identity().clone();
        receipt.authority = selected.admission_execution_authority().unwrap();
        receipt.previous_identity = previous.identity().clone();
        receipt.previous_authority = previous.admission_execution_authority().unwrap();
        receipt.transfer_generation = Some(
            receipt
                .identity
                .generation
                .max(receipt.previous_identity.generation)
                + 1,
        );
        persist(&f.0, &receipt).unwrap();
        selected
            .activate_restored_execution_checked(&mut previous, |authority| {
                Ok(VerifiedPermission(authority.clone()))
            })
            .unwrap();
        let transferred = selected.identity().generation;
        drop(selected);
        drop(previous);
        let recovered = bootstrap_storage(&f.0, |_| {
            Err::<VerifiedPermission, _>(StorageError::DispatchFenced)
        })
        .unwrap();
        assert_eq!(recovered.identity().generation, transferred + 1);
        assert!(recovered.admission_execution_authority().unwrap().active);
        let acknowledged = read_receipt(&f.0).unwrap().unwrap();
        assert!(acknowledged.execution_applied);
        assert_eq!(acknowledged.phase, "stable");
    }
    #[test]
    fn crash_between_source_retirement_and_target_activation_requires_fresh_native_permission() {
        let f = Fixture::new();
        let (original, target, mut receipt) = restored(&f);
        drop(original);
        let selected = Storage::open_or_create(&target).unwrap();
        let previous = Storage::open_or_create(&receipt.previous_root).unwrap();
        receipt.execution_authorized = true;
        receipt.phase = "transferring".into();
        receipt.attempts = 1;
        receipt.identity = selected.identity().clone();
        receipt.authority = selected.admission_execution_authority().unwrap();
        receipt.previous_identity = previous.identity().clone();
        receipt.previous_authority = previous.admission_execution_authority().unwrap();
        let generation = receipt
            .identity
            .generation
            .max(receipt.previous_identity.generation)
            + 1;
        receipt.transfer_generation = Some(generation);
        persist(&f.0, &receipt).unwrap();
        let connection =
            rusqlite::Connection::open(receipt.previous_root.join("state/magi.sqlite")).unwrap();
        connection
            .execute(
                "UPDATE store_meta SET value='inert' WHERE key='admission_activation_state'",
                [],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE store_meta SET value=?1 WHERE key='store_generation'",
                [generation.to_string()],
            )
            .unwrap();
        drop(connection);
        drop(selected);
        drop(previous);
        let mut recovered = read_receipt(&f.0).unwrap().unwrap();
        recover_receipt(&f.0, &mut recovered).unwrap();
        assert!(!recovered.execution_applied);
        assert!(!recovered.previous_authority.active);
        let selected = bootstrap_storage(&f.0, |authority| {
            assert!(!authority.active);
            Ok(VerifiedPermission(authority.clone()))
        })
        .unwrap();
        assert!(selected.admission_execution_authority().unwrap().active);
    }
    #[test]
    fn pending_resume_keeps_original_command_and_consent_without_granting_authority() {
        let f = Fixture::new();
        let (source, _, mut receipt) = restored(&f);
        receipt.execution_authorized = true;
        persist(&f.0, &receipt).unwrap();
        let durable = read_receipt(&f.0).unwrap().unwrap();
        let held = recover_pending_selection(
            durable,
            source.identity(),
            &source.admission_execution_authority().unwrap(),
        )
        .unwrap();
        let view = pending_view(&held);
        assert_eq!(view.selection_token, receipt.selection_token);
        assert_eq!(view.command_id, receipt.command_id);
        assert!(view.activated);
        assert!(!held.reader.admission_execution_authority().unwrap().active);
        let remount = pending_view(&held);
        assert_eq!(remount.command_id, view.command_id);
        let mut wrong_source = source.identity().clone();
        wrong_source.generation += 1;
        assert!(
            recover_pending_selection(
                receipt,
                &wrong_source,
                &source.admission_execution_authority().unwrap()
            )
            .is_err()
        );
    }
    #[test]
    fn failed_activation_commit_does_not_acknowledge_and_same_command_can_retry() {
        let f = Fixture::new();
        let (source, _, mut receipt) = restored(&f);
        receipt.execution_authorized = true;
        let mut held = recover_pending_selection(
            receipt.clone(),
            source.identity(),
            &source.admission_execution_authority().unwrap(),
        )
        .unwrap();
        held.activated = false;
        fs::create_dir(f.0.join("selected-store.json")).unwrap();
        assert!(commit_activation(&f.0, &mut held).is_err());
        assert!(!held.activated);
        assert!(held.receipt.execution_authorized);
        fs::remove_dir(f.0.join("selected-store.json")).unwrap();
        commit_activation(&f.0, &mut held).unwrap();
        assert!(held.activated);
        assert_eq!(
            read_receipt(&f.0).unwrap().unwrap().command_id,
            receipt.command_id
        );
        commit_activation(&f.0, &mut held).unwrap();
        let mut changed = read_receipt(&f.0).unwrap().unwrap();
        changed.command_id = uuid::Uuid::new_v4().to_string();
        persist(&f.0, &changed).unwrap();
        assert!(commit_activation(&f.0, &mut held).is_err());
    }
}
