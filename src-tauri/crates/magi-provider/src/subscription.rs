use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Read,
    os::fd::{AsRawFd, FromRawFd},
    os::unix::{
        ffi::OsStrExt,
        fs::{MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};
use zeroize::Zeroizing;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CredentialStore {
    File,
    Keychain,
}

#[derive(Clone)]
pub struct CredentialHome {
    pub path: PathBuf,
    pub device: u64,
    pub inode: u64,
    pub store: CredentialStore,
}

#[derive(Deserialize)]
struct AuthFile {
    auth_mode: Option<String>,
    #[serde(rename = "OPENAI_API_KEY")]
    api_key: Option<String>,
    tokens: Option<Tokens>,
}

#[derive(Deserialize)]
struct Tokens {
    access_token: String,
    account_id: Option<String>,
    id_token: String,
}

#[derive(Deserialize)]
struct Claims {
    exp: Option<u64>,
    #[serde(rename = "https://api.openai.com/auth")]
    auth: Option<AccountClaims>,
}

#[derive(Deserialize)]
struct AccountClaims {
    chatgpt_account_id: Option<String>,
    chatgpt_plan_type: Option<String>,
}

struct SecretSnapshot {
    access: Zeroizing<String>,
    account: Zeroizing<String>,
    plan: Option<String>,
    fingerprint: [u8; 32],
}

pub struct SubscriptionBroker {
    home: CredentialHome,
    account_digest: [u8; 32],
    last_fingerprint: Mutex<[u8; 32]>,
}

impl CredentialHome {
    pub fn inspect(path: &Path) -> Result<Self, &'static str> {
        let metadata = fs::symlink_metadata(path).map_err(|_| "credential_home_missing")?;
        if !metadata.is_dir()
            || metadata.file_type().is_symlink()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o022 != 0
        {
            return Err("credential_home_invalid");
        }
        let path = path.canonicalize().map_err(|_| "credential_home_invalid")?;
        let store = if path.join("auth.json").exists() {
            CredentialStore::File
        } else {
            CredentialStore::Keychain
        };
        let home = Self {
            path,
            device: metadata.dev(),
            inode: metadata.ino(),
            store,
        };
        read_snapshot(&home)?;
        Ok(home)
    }
}

impl SubscriptionBroker {
    pub fn account_digest(&self) -> [u8; 32] {
        self.account_digest
    }
    pub fn open(home: CredentialHome) -> Result<Self, &'static str> {
        let snapshot = read_snapshot(&home)?;
        Ok(Self {
            home,
            account_digest: Sha256::digest(snapshot.account.as_bytes()).into(),
            last_fingerprint: Mutex::new(snapshot.fingerprint),
        })
    }

    pub fn connect_parameters(&self) -> Result<Value, &'static str> {
        let snapshot = self.read_pinned()?;
        *self
            .last_fingerprint
            .lock()
            .map_err(|_| "credential_authority_unknown")? = snapshot.fingerprint;
        Ok(parameters(&snapshot))
    }

    pub fn refresh_parameters(
        &self,
        previous_account_id: Option<&str>,
    ) -> Result<Value, &'static str> {
        if previous_account_id.is_some_and(|account| {
            <[u8; 32]>::from(Sha256::digest(account.as_bytes())) != self.account_digest
        }) {
            return Err("credential_account_changed");
        }
        let snapshot = self.read_pinned()?;
        let mut last = self
            .last_fingerprint
            .lock()
            .map_err(|_| "credential_authority_unknown")?;
        if *last == snapshot.fingerprint {
            return Err("credential_refresh_required");
        }
        *last = snapshot.fingerprint;
        Ok(parameters(&snapshot))
    }

    fn read_pinned(&self) -> Result<SecretSnapshot, &'static str> {
        let snapshot = read_snapshot(&self.home)?;
        if <[u8; 32]>::from(Sha256::digest(snapshot.account.as_bytes())) != self.account_digest {
            return Err("credential_account_changed");
        }
        Ok(snapshot)
    }
}

fn parameters(snapshot: &SecretSnapshot) -> Value {
    json!({"accessToken":snapshot.access.as_str(),"chatgptAccountId":snapshot.account.as_str(),"chatgptPlanType":snapshot.plan})
}

fn read_snapshot(home: &CredentialHome) -> Result<SecretSnapshot, &'static str> {
    let directory_file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&home.path)
        .map_err(|_| "credential_home_missing")?;
    let directory = directory_file
        .metadata()
        .map_err(|_| "credential_home_invalid")?;
    if !directory.is_dir()
        || directory.file_type().is_symlink()
        || directory.dev() != home.device
        || directory.ino() != home.inode
        || directory.uid() != unsafe { libc::geteuid() }
        || directory.mode() & 0o022 != 0
    {
        return Err("credential_home_changed");
    }
    let bytes = match home.store {
        CredentialStore::File => {
            let open_auth = || {
                let fd = unsafe {
                    libc::openat(
                        directory_file.as_raw_fd(),
                        c"auth.json".as_ptr(),
                        libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
                    )
                };
                if fd < 0 {
                    Err("credential_auth_missing")
                } else {
                    Ok(unsafe { fs::File::from_raw_fd(fd) })
                }
            };
            let mut file = open_auth()?;
            let before = file.metadata().map_err(|_| "credential_auth_invalid")?;
            if !before.is_file()
                || before.uid() != unsafe { libc::geteuid() }
                || before.mode() & 0o077 != 0
                || before.len() > 64 * 1024
            {
                return Err("credential_auth_invalid");
            }
            let mut bytes = Zeroizing::new(Vec::new());
            file.by_ref()
                .take(64 * 1024 + 1)
                .read_to_end(&mut bytes)
                .map_err(|_| "credential_auth_invalid")?;
            let after = file.metadata().map_err(|_| "credential_auth_invalid")?;
            let path_now = open_auth()?
                .metadata()
                .map_err(|_| "credential_auth_changed")?;
            if bytes.len() > 64 * 1024
                || before.ino() != path_now.ino()
                || before.dev() != path_now.dev()
                || before.len() != after.len()
                || before.mtime() != after.mtime()
                || before.mtime_nsec() != after.mtime_nsec()
                || before.ctime() != after.ctime()
                || before.ctime_nsec() != after.ctime_nsec()
            {
                return Err("credential_auth_changed");
            }
            bytes
        }
        CredentialStore::Keychain => read_keychain(&home.path)?,
    };
    if bytes.len() > 64 * 1024 {
        return Err("credential_auth_invalid");
    }
    let auth: AuthFile = serde_json::from_slice(&bytes).map_err(|_| "credential_auth_invalid")?;
    if auth.auth_mode.as_deref() != Some("chatgpt")
        || auth.api_key.is_some_and(|key| !key.is_empty())
    {
        return Err("subscription_auth_required");
    }
    let tokens = auth.tokens.ok_or("credential_auth_missing")?;
    let access = Zeroizing::new(tokens.access_token);
    let id_token = Zeroizing::new(tokens.id_token);
    let claims = parse_claims(&access)?;
    let id_claims = parse_claims(&id_token)?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "credential_authority_unknown")?
        .as_secs();
    if claims.exp.is_none_or(|expires| expires <= now + 30) {
        return Err("credential_refresh_required");
    }
    let account_claims = claims
        .auth
        .or(id_claims.auth)
        .ok_or("credential_auth_invalid")?;
    let account = account_claims
        .chatgpt_account_id
        .filter(|value| !value.is_empty() && value.len() <= 256)
        .ok_or("credential_auth_invalid")?;
    if tokens
        .account_id
        .is_some_and(|metadata| metadata != account)
    {
        return Err("credential_account_changed");
    }
    let after = fs::symlink_metadata(&home.path).map_err(|_| "credential_home_changed")?;
    if after.dev() != home.device || after.ino() != home.inode {
        return Err("credential_home_changed");
    }
    let fingerprint = Sha256::digest(access.as_bytes()).into();
    Ok(SecretSnapshot {
        access,
        account: Zeroizing::new(account),
        plan: account_claims.chatgpt_plan_type,
        fingerprint,
    })
}

fn parse_claims(token: &str) -> Result<Claims, &'static str> {
    let mut parts = token.split('.');
    let header = parts.next().ok_or("credential_auth_invalid")?;
    let payload = parts.next().ok_or("credential_auth_invalid")?;
    let signature = parts.next().ok_or("credential_auth_invalid")?;
    if header.is_empty()
        || signature.is_empty()
        || parts.next().is_some()
        || payload.len() > 32 * 1024
    {
        return Err("credential_auth_invalid");
    }
    let bytes = Zeroizing::new(
        URL_SAFE_NO_PAD
            .decode(payload)
            .map_err(|_| "credential_auth_invalid")?,
    );
    serde_json::from_slice(&bytes).map_err(|_| "credential_auth_invalid")
}

fn read_keychain(home: &Path) -> Result<Zeroizing<Vec<u8>>, &'static str> {
    // Pinned Codex auth/storage.rs uses "Codex Auth" and cli|SHA256(home)[0..16].
    let hash = format!("{:x}", Sha256::digest(home.as_os_str().as_bytes()));
    let account = format!("cli|{}", &hash[..16]);
    #[cfg(target_os = "macos")]
    {
        security_framework::passwords::get_generic_password("Codex Auth", &account)
            .map(Zeroizing::new)
            .map_err(|_| "credential_auth_missing")
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = account;
        Err("credential_store_unsupported")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn save(path: &Path, account: &str, metadata: &str, version: &str) {
        let expiry = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 3600;
        let claims = json!({"exp":expiry,"https://api.openai.com/auth":{"chatgpt_account_id":account,"chatgpt_plan_type":"plus"}});
        let token = format!(
            "fixture.{}.{}",
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap()),
            version
        );
        fs::write(path,json!({"auth_mode":"chatgpt","tokens":{"access_token":token,"id_token":token,"account_id":metadata}}).to_string()).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }

    #[test]
    fn selected_authority_is_read_only_and_refresh_account_is_pinned() {
        let root = std::env::temp_dir().join(format!(
            "magi-auth-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let file = root.join("auth.json");
        save(&file, "fixture-account", "fixture-account", "first");
        let original = fs::read(&file).unwrap();
        let broker = SubscriptionBroker::open(CredentialHome::inspect(&root).unwrap()).unwrap();
        assert_eq!(
            broker.connect_parameters().unwrap()["chatgptAccountId"],
            "fixture-account"
        );
        assert_eq!(
            broker.refresh_parameters(None).unwrap_err(),
            "credential_refresh_required"
        );
        assert_eq!(fs::read(&file).unwrap(), original);
        save(&file, "fixture-account", "fixture-account", "rotated");
        assert!(broker.refresh_parameters(Some("fixture-account")).is_ok());
        assert_eq!(
            broker
                .refresh_parameters(Some("other-account"))
                .unwrap_err(),
            "credential_account_changed"
        );
        save(&file, "other-account", "other-account", "other");
        assert_eq!(
            broker.connect_parameters().unwrap_err(),
            "credential_account_changed"
        );
        save(&file, "fixture-account", "different-metadata", "other");
        assert_eq!(
            broker.connect_parameters().unwrap_err(),
            "credential_account_changed"
        );
        fs::remove_file(&file).unwrap();
        std::os::unix::fs::symlink(root.join("absent"), &file).unwrap();
        assert!(broker.connect_parameters().is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
