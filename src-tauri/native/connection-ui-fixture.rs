use magi_domain::{
    CoreId, ProviderAuthenticationMethod, ProviderCatalogSnapshot, ProviderProfileInput,
    ProviderProfileRevision,
};
use magi_storage::{ProviderModelSelection, ProviderModelSelectionInput, Storage};
use std::{
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::Path,
};

pub(super) struct OwnedDestination {
    path: std::path::PathBuf,
    dev: u64,
    ino: u64,
    committed: bool,
    directory: std::fs::File,
}
impl OwnedDestination {
    pub(super) fn capture(path: &Path) -> Result<Self, &'static str> {
        use std::os::unix::fs::OpenOptionsExt;
        let directory = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY)
            .open(path)
            .map_err(|_| "destination custody unavailable")?;
        let metadata = directory
            .metadata()
            .map_err(|_| "destination identity unavailable")?;
        if !metadata.is_dir() {
            return Err("destination directory required");
        }
        Ok(Self {
            path: path.to_owned(),
            dev: metadata.dev(),
            ino: metadata.ino(),
            committed: false,
            directory,
        })
    }
    pub(super) fn validate(&self) -> Result<(), &'static str> {
        let current =
            std::fs::symlink_metadata(&self.path).map_err(|_| "destination path unavailable")?;
        let held = self
            .directory
            .metadata()
            .map_err(|_| "destination custody unavailable")?;
        if !current.is_dir()
            || current.dev() != self.dev
            || current.ino() != self.ino
            || held.dev() != self.dev
            || held.ino() != self.ino
        {
            return Err("destination custody changed");
        }
        Ok(())
    }
    pub(super) fn database_bytes(&self) -> Result<Vec<u8>, &'static str> {
        use std::io::Read;
        use std::os::fd::{AsRawFd, FromRawFd};
        self.validate()?;
        let state_fd = unsafe {
            libc::openat(
                self.directory.as_raw_fd(),
                c"state".as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW,
            )
        };
        if state_fd < 0 {
            return Err("prepared state custody unavailable");
        }
        let state = unsafe { std::fs::File::from_raw_fd(state_fd) };
        let fd = unsafe {
            libc::openat(
                state.as_raw_fd(),
                c"magi.sqlite".as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW,
            )
        };
        if fd < 0 {
            return Err("prepared database custody unavailable");
        }
        let file = unsafe { std::fs::File::from_raw_fd(fd) };
        let metadata = file
            .metadata()
            .map_err(|_| "prepared database identity unavailable")?;
        if !metadata.is_file()
            || metadata.nlink() != 1
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.len() > 20 * 1024 * 1024
        {
            return Err("bounded prepared database required");
        }
        let mut bytes = Vec::new();
        file.take(20 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "prepared database read failed")?;
        if bytes.len() > 20 * 1024 * 1024 {
            return Err("prepared database exceeds bound");
        }
        self.validate()?;
        Ok(bytes)
    }
    pub(super) fn write_provenance(&self, bytes: &[u8]) -> Result<(), &'static str> {
        use std::io::Write;
        use std::os::fd::{AsRawFd, FromRawFd};
        self.validate()?;
        let fd = unsafe {
            libc::openat(
                self.directory.as_raw_fd(),
                c"connection-ui-metadata-provenance.json".as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW,
                0o600,
            )
        };
        if fd < 0 {
            return Err("metadata provenance creation failed");
        }
        let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
        file.write_all(bytes)
            .map_err(|_| "metadata provenance output failed")?;
        file.sync_all()
            .map_err(|_| "metadata provenance sync failed")?;
        self.validate()
    }
    pub(super) fn commit(&mut self) -> Result<(), &'static str> {
        self.validate()?;
        self.committed = true;
        Ok(())
    }
}
impl Drop for OwnedDestination {
    fn drop(&mut self) {
        if !self.committed
            && let Ok(metadata) = std::fs::symlink_metadata(&self.path)
            && metadata.is_dir()
            && metadata.dev() == self.dev
            && metadata.ino() == self.ino
        {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

pub(super) type MetadataEntry<'a> = (
    &'a [ProviderProfileRevision],
    &'a ProviderCatalogSnapshot,
    Option<&'a ProviderCatalogSnapshot>,
    &'a ProviderModelSelection,
);

pub(super) fn seed(
    destination: &Path,
    entries: &[MetadataEntry<'_>],
) -> Result<(Vec<serde_json::Value>, OwnedDestination), &'static str> {
    if destination.exists() || entries.len() != 3 {
        return Err("fresh three-profile metadata destination required");
    }
    let mut identities = std::collections::BTreeSet::new();
    for (history, catalog, latest, selection) in entries {
        let head = history.last().ok_or("profile history missing")?;
        if history.len() > 65
            || !identities.insert(head.provider_profile_id.clone())
            || head.secret_reference.is_some()
            || head.authentication_method != ProviderAuthenticationMethod::LocalSubscription
            || catalog.provider_profile_id != head.provider_profile_id
            || catalog.profile_revision != head.revision
            || selection.binding.provider_profile_id != head.provider_profile_id
            || selection.binding.profile_revision != head.revision
            || selection.binding.catalog_snapshot_id != catalog.catalog_snapshot_id
            || selection.binding.catalog_digest != catalog.catalog_digest
        {
            return Err("metadata source binding invalid");
        }
        catalog.validate().map_err(|_| "catalog source invalid")?;
        if let Some(latest) = latest {
            latest
                .validate()
                .map_err(|_| "latest catalog source invalid")?;
            if latest.provider_profile_id != head.provider_profile_id
                || latest.profile_revision != head.revision
            {
                return Err("latest catalog profile authority mismatch");
            }
        }
        for (revision, profile) in history.iter().enumerate() {
            if profile.revision != revision as u64
                || profile.provider_profile_id != head.provider_profile_id
                || profile.secret_reference.is_some()
                || profile.authentication_method != ProviderAuthenticationMethod::LocalSubscription
            {
                return Err("complete local profile history required");
            }
            profile.validate().map_err(|_| "profile source invalid")?;
        }
    }
    std::fs::create_dir(destination)
        .map_err(|_| "exclusive metadata destination creation failed")?;
    let cleanup = OwnedDestination::capture(destination)?;
    std::fs::set_permissions(destination, std::fs::Permissions::from_mode(0o700))
        .map_err(|_| "private metadata destination failed")?;
    let storage =
        Storage::open_or_create(destination).map_err(|_| "metadata destination unavailable")?;
    let mut summaries = Vec::new();
    for (history, catalog, latest, selection) in entries {
        let mut expected = None;
        for profile in *history {
            let input = ProviderProfileInput {
                provider_profile_id: profile.provider_profile_id.clone(),
                provider_id: profile.provider_id.clone(),
                display_name: profile.display_name.clone(),
                account_alias: profile.account_alias.clone(),
                authentication_method: profile.authentication_method,
                secret_reference: None,
                runtime_home_id: profile.runtime_home_id.clone(),
                credential_home: profile.credential_home.clone(),
            };
            let saved = storage
                .save_provider_profile(&input, expected, &selection.updated_at)
                .map_err(|_| "profile metadata replay failed")?;
            if &saved != profile {
                return Err("profile metadata authority mismatch");
            }
            expected = Some(saved.revision);
        }
        let saved_catalog = storage
            .save_provider_catalog_snapshot(catalog)
            .map_err(|_| "catalog metadata replay failed")?;
        if saved_catalog != **catalog {
            return Err("catalog metadata authority mismatch");
        }
        let saved = storage
            .select_provider_model(&ProviderModelSelectionInput {
                provider_profile_id: selection.binding.provider_profile_id.clone(),
                profile_revision: selection.binding.profile_revision,
                catalog_snapshot_id: catalog.catalog_snapshot_id.clone(),
                catalog_digest: catalog.catalog_digest.clone(),
                model_id: selection.binding.model_id.clone(),
                mode_id: selection.binding.mode_id.clone(),
                expected_selection_revision: None,
                updated_at: selection.updated_at.clone(),
            })
            .map_err(|_| "saved model metadata replay failed")?;
        if saved.binding != selection.binding {
            return Err("saved model authority mismatch");
        }
        if let Some(latest) = latest {
            let stored = storage
                .save_provider_catalog_snapshot(latest)
                .map_err(|_| "latest catalog replay failed")?;
            if stored != **latest {
                return Err("latest catalog authority mismatch");
            }
        }
        summaries.push(serde_json::json!({"profileId":saved.binding.provider_profile_id,"profileRevision":saved.binding.profile_revision,"catalogDigest":catalog.catalog_digest,"latestCatalogDigest":latest.map(|value|value.catalog_digest.clone()),"modelId":saved.binding.model_id,"modeId":saved.binding.mode_id,"sourceSelectionRevision":selection.selection_revision,"freshSelectionRevision":saved.selection_revision}));
    }
    for core in CoreId::ALL {
        if storage
            .load_core_model_selection(core)
            .map_err(|_| "core metadata query failed")?
            .is_some()
        {
            return Err("metadata destination unexpectedly contains execution core selections");
        }
    }
    drop(storage);
    cleanup.validate()?;
    Ok((summaries, cleanup))
}
