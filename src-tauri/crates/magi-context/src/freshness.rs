use crate::{
    Digest, FreshnessObservation, FreshnessStatus, MAX_FILE_BYTES, classify_selected_path,
};
use cap_std::fs::{Dir, Metadata, MetadataExt, OpenOptions, OpenOptionsExt};
use std::{
    ffi::OsString,
    io::{self, Read},
    path::{Component, Path},
};

pub struct FreshnessGrant {
    parent: Dir,
    name: OsString,
    identity: (u64, u64),
    max_bytes: u64,
    expires_at: u64,
    revoked: bool,
}

pub struct FreshnessRead {
    pub observation: FreshnessObservation,
    pub reason: Option<&'static str>,
    pub can_recheck: bool,
}

impl FreshnessGrant {
    /// Only the host may supply a path obtained through an explicit user file selection.
    /// The parent capability and selected inode are session-only; replacing the file requires reselection.
    pub fn selected_file(path: &Path, max_bytes: u64, expires_at: u64) -> io::Result<Self> {
        if max_bytes == 0 || max_bytes > MAX_FILE_BYTES || classify_selected_path(path).is_some() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Source selection is not permitted",
            ));
        }
        let name = path
            .file_name()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Missing selected file"))?;
        if !matches!(
            Path::new(name).components().next(),
            Some(Component::Normal(_))
        ) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Invalid selected file",
            ));
        }
        let parent = Dir::open_ambient_dir(
            path.parent().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "Missing selected directory")
            })?,
            cap_std::ambient_authority(),
        )?;
        let before = parent.symlink_metadata(name)?;
        if !before.is_file() || before.file_type().is_symlink() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Selection must be a regular file",
            ));
        }
        let mut options = OpenOptions::new();
        options
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        let file = parent.open_with(name, &options)?;
        if version(&before) != version(&file.metadata()?)
            || version(&before) != version(&parent.symlink_metadata(name)?)
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Selection changed before authorization",
            ));
        }
        Ok(Self {
            parent,
            name: name.to_owned(),
            identity: (before.dev(), before.ino()),
            max_bytes,
            expires_at,
            revoked: false,
        })
    }

    pub fn revoke(&mut self) {
        self.revoked = true;
    }

    pub fn observe(&self, captured: &Digest, at: u64) -> FreshnessRead {
        let result = |status, digest, reason, can_recheck| FreshnessRead {
            observation: FreshnessObservation {
                status,
                observed_digest: digest,
                observed_at_epoch_ms: at,
            },
            reason,
            can_recheck,
        };
        if self.revoked || at >= self.expires_at {
            return result(
                FreshnessStatus::Unchecked,
                None,
                Some("source_access_expired_or_revoked"),
                false,
            );
        }
        let before = match self.parent.symlink_metadata(&self.name) {
            Ok(m) => m,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return result(
                    FreshnessStatus::Missing,
                    None,
                    Some("selected_file_missing"),
                    true,
                );
            }
            Err(_) => {
                return result(
                    FreshnessStatus::Unreadable,
                    None,
                    Some("selected_file_unreadable"),
                    true,
                );
            }
        };
        if !before.is_file()
            || before.file_type().is_symlink()
            || (before.dev(), before.ino()) != self.identity
        {
            return result(
                FreshnessStatus::Unchecked,
                None,
                Some("selected_file_replaced_reselect_required"),
                false,
            );
        }
        if before.mode() & 0o444 == 0 || before.len() > self.max_bytes {
            return result(
                FreshnessStatus::Unreadable,
                None,
                Some("selected_file_permission_or_size_limit"),
                true,
            );
        }
        let mut options = OpenOptions::new();
        options
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        let mut file = match self.parent.open_with(&self.name, &options) {
            Ok(f) => f,
            Err(_) => {
                return result(
                    FreshnessStatus::Unreadable,
                    None,
                    Some("selected_file_unreadable"),
                    true,
                );
            }
        };
        if file.metadata().ok().as_ref().map(version) != Some(version(&before)) {
            return result(
                FreshnessStatus::Unchecked,
                None,
                Some("source_changed_during_observation"),
                false,
            );
        }
        let mut bytes = Vec::new();
        if (&mut file)
            .take(self.max_bytes + 1)
            .read_to_end(&mut bytes)
            .is_err()
            || bytes.len() as u64 > self.max_bytes
        {
            return result(
                FreshnessStatus::Unreadable,
                None,
                Some("selected_file_read_or_size_limit"),
                true,
            );
        }
        if file.metadata().ok().as_ref().map(version) != Some(version(&before))
            || self
                .parent
                .symlink_metadata(&self.name)
                .ok()
                .as_ref()
                .map(version)
                != Some(version(&before))
        {
            return result(
                FreshnessStatus::Unchecked,
                None,
                Some("source_changed_during_observation"),
                false,
            );
        }
        let digest = Digest::from_bytes(&bytes);
        result(
            if &digest == captured {
                FreshnessStatus::Unchanged
            } else {
                FreshnessStatus::Changed
            },
            Some(digest),
            None,
            true,
        )
    }
}

fn version(m: &Metadata) -> (u64, u64, u64, i64, i64, i64, i64) {
    (
        m.dev(),
        m.ino(),
        m.len(),
        m.mtime(),
        m.mtime_nsec(),
        m.ctime(),
        m.ctime_nsec(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        os::unix::fs::{PermissionsExt, symlink},
    };
    struct Fixture(std::path::PathBuf);
    impl Fixture {
        fn new() -> Self {
            let p = std::env::temp_dir().join(format!("magi-freshness-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&p).unwrap();
            Self(p)
        }
        fn file(&self) -> std::path::PathBuf {
            self.0.join("source.txt")
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn explicit_current_file_observation_preserves_original_digest_and_handles_missing_permission()
    {
        let f = Fixture::new();
        let p = f.file();
        fs::write(&p, b"original").unwrap();
        let captured = Digest::from_bytes(b"original");
        let grant = FreshnessGrant::selected_file(&p, 100, 1000).unwrap();
        assert_eq!(
            grant.observe(&captured, 1).observation.status,
            FreshnessStatus::Unchanged
        );
        fs::write(&p, b"changed").unwrap();
        let changed = grant.observe(&captured, 2);
        assert_eq!(changed.observation.status, FreshnessStatus::Changed);
        assert_eq!(
            changed.observation.observed_digest,
            Some(Digest::from_bytes(b"changed"))
        );
        assert_eq!(captured, Digest::from_bytes(b"original"));
        fs::set_permissions(&p, fs::Permissions::from_mode(0o0)).unwrap();
        assert_eq!(
            grant.observe(&captured, 3).observation.status,
            FreshnessStatus::Unreadable
        );
        fs::set_permissions(&p, fs::Permissions::from_mode(0o600)).unwrap();
        fs::remove_file(&p).unwrap();
        assert_eq!(
            grant.observe(&captured, 4).observation.status,
            FreshnessStatus::Missing
        );
    }
    #[test]
    fn replacement_and_symlink_cannot_extend_the_selected_file_grant() {
        let f = Fixture::new();
        let p = f.file();
        fs::write(&p, b"original").unwrap();
        let captured = Digest::from_bytes(b"original");
        let grant = FreshnessGrant::selected_file(&p, 100, 1000).unwrap();
        let other = f.0.join("other.txt");
        fs::write(&other, b"outside").unwrap();
        fs::rename(&other, &p).unwrap();
        let observed = grant.observe(&captured, 1);
        assert_eq!(observed.observation.status, FreshnessStatus::Unchecked);
        assert!(observed.observation.observed_digest.is_none());
        assert!(!observed.can_recheck);
        fs::remove_file(&p).unwrap();
        fs::write(&other, b"outside").unwrap();
        symlink(&other, &p).unwrap();
        assert!(FreshnessGrant::selected_file(&p, 100, 1000).is_err());
        assert_eq!(
            grant.observe(&captured, 2).observation.status,
            FreshnessStatus::Unchecked
        );
        let explicit = FreshnessGrant::selected_file(&other, 100, 1000).unwrap();
        assert_eq!(
            explicit.observe(&captured, 3).observation.status,
            FreshnessStatus::Changed
        );
    }
    #[test]
    fn source_expiry_revocation_and_read_size_limits_fail_closed() {
        let f = Fixture::new();
        let p = f.file();
        fs::write(&p, b"12345").unwrap();
        let digest = Digest::from_bytes(b"12345");
        let mut grant = FreshnessGrant::selected_file(&p, 4, 10).unwrap();
        assert_eq!(
            grant.observe(&digest, 1).observation.status,
            FreshnessStatus::Unreadable
        );
        assert_eq!(
            grant.observe(&digest, 10).observation.status,
            FreshnessStatus::Unchecked
        );
        grant.revoke();
        assert_eq!(
            grant.observe(&digest, 2).observation.status,
            FreshnessStatus::Unchecked
        );
    }
}
