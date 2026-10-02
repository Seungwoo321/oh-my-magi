use magi_domain::{Digest, ProviderCredentialHome, ProviderCredentialStore};
use magi_provider::subscription::{CredentialHome, CredentialStore, SubscriptionBroker};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

pub(crate) fn inspect(
    path: &str,
    user_home: Option<&Path>,
) -> Result<ProviderCredentialHome, String> {
    let path = entered_path(path, user_home)?;
    let inspected = CredentialHome::inspect(&path).map_err(str::to_owned)?;
    let pinned = SubscriptionBroker::open(inspected.clone()).map_err(str::to_owned)?;
    let account_digest = Digest::from_hex(
        pinned
            .account_digest()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>(),
    )
    .map_err(|_| "The selected subscription authority is invalid.")?;
    let authority = ProviderCredentialHome {
        authority_id: uuid::Uuid::new_v4().to_string(),
        canonical_path: inspected
            .path
            .to_str()
            .ok_or("The selected credential home path is unsupported.")?
            .to_owned(),
        device: inspected.device,
        inode: inspected.inode,
        credential_store: match inspected.store {
            CredentialStore::File => ProviderCredentialStore::File,
            CredentialStore::Keychain => ProviderCredentialStore::Keychain,
        },
        account_digest: Some(account_digest),
    };
    broker(&authority)?;
    Ok(authority)
}

pub(crate) fn display_path(path: &std::path::Path, user_home: Option<&std::path::Path>) -> String {
    if let Some(relative) = user_home.and_then(|home| path.strip_prefix(home).ok()) {
        if relative.as_os_str().is_empty() {
            return "~/".into();
        }
        return format!("~/{}", relative.to_string_lossy());
    }
    path.to_string_lossy().into_owned()
}

fn entered_path(path: &str, user_home: Option<&std::path::Path>) -> Result<PathBuf, String> {
    if path.trim().is_empty() || path.contains('\0') {
        return Err("Enter the existing credential home path.".into());
    }
    let path = if let Some(relative) = path.strip_prefix("~/") {
        user_home
            .ok_or("The user home is unavailable.")?
            .join(relative.trim_start_matches('/'))
    } else {
        PathBuf::from(path)
    };
    if !path.is_absolute() {
        return Err("Enter an absolute credential home path.".into());
    }
    Ok(path)
}

pub(crate) fn broker(home: &ProviderCredentialHome) -> Result<Arc<SubscriptionBroker>, String> {
    let source = CredentialHome {
        path: PathBuf::from(&home.canonical_path),
        device: home.device,
        inode: home.inode,
        store: match home.credential_store {
            ProviderCredentialStore::File => CredentialStore::File,
            ProviderCredentialStore::Keychain => CredentialStore::Keychain,
        },
    };
    let expected = home
        .account_digest
        .as_ref()
        .ok_or("Enter the existing credential home again to pin its subscription account.")?;
    let broker = SubscriptionBroker::open(source).map_err(str::to_owned)?;
    let observed: String = broker
        .account_digest()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    if observed != expected.as_str() {
        return Err("credential_account_changed".into());
    }
    Ok(Arc::new(broker))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_paths_and_display_respect_home_component_boundaries() {
        let home = Path::new("/Users/fixture");
        assert_eq!(display_path(home, Some(home)), "~/");
        assert_eq!(
            entered_path(&display_path(home, Some(home)), Some(home)).unwrap(),
            home
        );
        assert_eq!(
            entered_path("~/.codex", Some(home)).unwrap(),
            home.join(".codex")
        );
        assert!(entered_path(".codex", Some(home)).is_err());
        assert!(entered_path("~/.codex", None).is_err());
        assert!(entered_path("", Some(home)).is_err());
        assert_eq!(display_path(&home.join(".codex"), Some(home)), "~/.codex");
        assert_eq!(
            display_path(Path::new("/Users/fixture-other/.codex"), Some(home)),
            "/Users/fixture-other/.codex"
        );
    }
}
