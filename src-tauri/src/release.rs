use crate::commands::DesktopState;
use base64::{Engine, engine::general_purpose::STANDARD};
use ed25519_dalek::{Signature, Verifier, VerifyingKey, pkcs8::DecodePublicKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    sync::{Mutex, OnceLock},
    time::Duration,
};
use tauri::{AppHandle, Manager, State, WebviewWindow};
use tauri_plugin_updater::{Update, UpdaterExt};

const MANIFEST_LIMIT: usize = 128 * 1024;
const PAYLOAD_LIMIT: u64 = 512 * 1024 * 1024;
static RELEASE_STATE: OnceLock<Mutex<ReleaseState>> = OnceLock::new();
#[derive(Default)]
struct ReleaseState {
    candidate: Option<(Manifest, Update)>,
    installed: Option<String>,
    busy: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Trust {
    schema_version: u32,
    endpoint: Option<String>,
    allowed_download_hosts: Vec<String>,
    updater_public_key: String,
    manifest_public_key_pem: String,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    version: String,
    target: String,
    minimum_macos: String,
    schema_min: u32,
    schema_max: u32,
    payload: Payload,
    notes: String,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Payload {
    url: String,
    sha256: String,
    size: u64,
    signature: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SignedDocument {
    signed_payload: String,
    manifest_signature: String,
    version: String,
    notes: String,
    platforms: std::collections::HashMap<String, Platform>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Platform {
    url: String,
    signature: String,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UpdateStatus {
    status: &'static str,
    current_version: String,
    version: Option<String>,
    notes: Option<String>,
    reason: Option<String>,
    candidate_id: Option<String>,
}
fn trust() -> Result<Trust, String> {
    serde_json::from_str(include_str!("../release-trust.json"))
        .map_err(|_| "Release trust configuration is invalid.".into())
}
pub(crate) fn updater_public_key() -> String {
    trust()
        .expect("valid embedded release trust")
        .updater_public_key
}
fn state() -> &'static Mutex<ReleaseState> {
    RELEASE_STATE.get_or_init(|| Mutex::new(ReleaseState::default()))
}
fn response(
    app: &AppHandle,
    status: &'static str,
    manifest: Option<&Manifest>,
    reason: Option<String>,
) -> UpdateStatus {
    UpdateStatus {
        status,
        current_version: app.package_info().version.to_string(),
        version: manifest.map(|m| m.version.clone()),
        notes: manifest.map(|m| m.notes.clone()),
        reason,
        candidate_id: manifest.map(candidate_id),
    }
}
fn candidate_id(manifest: &Manifest) -> String {
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(manifest).expect("serializable verified manifest"))
    )
}
fn validate_reviewed_candidate(manifest: &Manifest, expected: &str) -> Result<(), String> {
    if candidate_id(manifest) != expected {
        return Err("The reviewed release candidate has changed. Check and review again.".into());
    }
    Ok(())
}
fn require_main(window: &WebviewWindow) -> Result<(), String> {
    if window.label() == "main" {
        Ok(())
    } else {
        Err("Update controls are available only in the main window.".into())
    }
}
fn safe_url(value: &str, hosts: &[String]) -> Result<reqwest::Url, String> {
    let url = reqwest::Url::parse(value).map_err(|_| "Invalid release URL.")?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || url.port().is_some_and(|p| p != 443)
        || !hosts.iter().any(|h| Some(h.as_str()) == url.host_str())
    {
        return Err("Release URL is outside the trusted HTTPS hosts.".into());
    }
    Ok(url)
}
fn repository_asset_url(url: &reqwest::Url) -> Result<(), String> {
    let segments: Vec<_> = url
        .path_segments()
        .ok_or("Invalid release asset path.")?
        .collect();
    let route = segments.len() == 6
        && segments[0] == "Seungwoo321"
        && segments[1] == "oh-my-magi"
        && segments[2] == "releases"
        && ((segments[3] == "latest" && segments[4] == "download")
            || (segments[3] == "download" && !segments[4].is_empty()))
        && !segments[5].is_empty();
    if url.host_str() != Some("github.com")
        || url.query().is_some()
        || !route
        || url.path().to_ascii_lowercase().contains("%2f")
        || url.path().to_ascii_lowercase().contains("%5c")
    {
        return Err("Release asset is outside the approved GitHub repository.".into());
    }
    Ok(())
}
fn verified_redirect(previous: &reqwest::Url, next: &str) -> Result<reqwest::Url, String> {
    repository_asset_url(previous)?;
    let joined = previous
        .join(next)
        .map_err(|_| "Invalid release redirect.")?;
    let url = safe_url(joined.as_str(), &trust()?.allowed_download_hosts)?;
    match url.host_str() {
        Some("github.com") => repository_asset_url(&url)?,
        Some("release-assets.githubusercontent.com")
            if url.path().starts_with("/github-production-release-asset/") => {}
        _ => return Err("Release redirect is outside the approved download chain.".into()),
    }
    Ok(url)
}
async fn bounded_download(mut url: reqwest::Url, limit: usize) -> Result<Vec<u8>, String> {
    repository_asset_url(&url)?;
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|_| "Release transport unavailable.")?;
    let deadline = std::time::Instant::now() + Duration::from_secs(120);
    let mut hops = 0;
    let mut response = loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            return Err("Release download timed out.".into());
        }
        let response = client
            .get(url.clone())
            .timeout(remaining)
            .send()
            .await
            .map_err(|_| "Release download failed.")?;
        if !response.status().is_redirection() {
            break response;
        }
        if hops >= 3 {
            return Err("Release redirect limit exceeded.".into());
        }
        let location = response
            .headers()
            .get(reqwest::header::LOCATION)
            .ok_or("Release redirect has no destination.")?
            .to_str()
            .map_err(|_| "Invalid release redirect header.")?;
        url = verified_redirect(&url, location)?;
        hops += 1;
    };
    if !response.status().is_success()
        || response.content_length().is_some_and(|n| n > limit as u64)
    {
        return Err("Release download status or size is invalid.".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "Release download interrupted.")?
    {
        if chunk.len() > limit.saturating_sub(bytes.len()) {
            return Err("Release download exceeds its size limit.".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
fn verify_manifest(bytes: &[u8], trust: &Trust, schema: u32) -> Result<Manifest, String> {
    if trust.schema_version != 1 {
        return Err("Unsupported release trust schema.".into());
    }
    if bytes.len() > MANIFEST_LIMIT {
        return Err("Signed manifest exceeds its size limit.".into());
    }
    let strict = crate::strict_json::parse(
        std::str::from_utf8(bytes).map_err(|_| "Invalid manifest encoding.")?,
    )
    .map_err(|_| "Ambiguous signed release document.")?;
    let document: SignedDocument =
        serde_json::from_value(strict).map_err(|_| "Invalid signed release document.")?;
    let payload = STANDARD
        .decode(document.signed_payload)
        .map_err(|_| "Invalid signed manifest encoding.")?;
    let signature = STANDARD
        .decode(document.manifest_signature)
        .map_err(|_| "Invalid manifest signature encoding.")?;
    let key = VerifyingKey::from_public_key_pem(&trust.manifest_public_key_pem)
        .map_err(|_| "Invalid manifest verification key.")?;
    key.verify(
        &payload,
        &Signature::from_slice(&signature).map_err(|_| "Invalid manifest signature.")?,
    )
    .map_err(|_| "Release manifest signature verification failed.")?;
    let strict = crate::strict_json::parse(
        std::str::from_utf8(&payload).map_err(|_| "Invalid payload encoding.")?,
    )
    .map_err(|_| "Ambiguous signed manifest fields.")?;
    let manifest: Manifest =
        serde_json::from_value(strict).map_err(|_| "Invalid signed manifest fields.")?;
    let platform = document
        .platforms
        .get(&manifest.target)
        .ok_or("Missing release platform.")?;
    if document.version != manifest.version
        || document.notes != manifest.notes
        || platform.url != manifest.payload.url
        || platform.signature != manifest.payload.signature
    {
        return Err("Unsigned metadata differs from the signed manifest.".into());
    }
    if manifest.target != format!("darwin-{}", std::env::consts::ARCH)
        || manifest.schema_min > schema
        || manifest.schema_max < schema
        || manifest.schema_min > manifest.schema_max
        || manifest.payload.size == 0
        || manifest.payload.size > PAYLOAD_LIMIT
        || manifest.notes.len() > 32 * 1024
        || manifest.payload.sha256.len() != 64
        || !manifest
            .payload
            .sha256
            .bytes()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    {
        return Err("Release architecture, schema or payload limits are incompatible.".into());
    }
    semver::Version::parse(&manifest.version).map_err(|_| "Invalid release version.")?;
    repository_asset_url(&safe_url(
        &manifest.payload.url,
        &trust.allowed_download_hosts,
    )?)?;
    Ok(manifest)
}
fn verify_payload(bytes: &[u8], manifest: &Manifest, public_key: &str) -> Result<(), String> {
    if bytes.len() as u64 != manifest.payload.size
        || format!("{:x}", Sha256::digest(bytes)) != manifest.payload.sha256
    {
        return Err("Release payload digest or size mismatch.".into());
    }
    let decode = |s: &str| -> Result<String, String> {
        String::from_utf8(
            STANDARD
                .decode(s)
                .map_err(|_| "Invalid updater signature encoding.")?,
        )
        .map_err(|_| "Invalid updater signature text.".into())
    };
    let key = minisign_verify::PublicKey::decode(&decode(public_key)?)
        .map_err(|_| "Invalid updater verification key.")?;
    let signature = minisign_verify::Signature::decode(&decode(&manifest.payload.signature)?)
        .map_err(|_| "Invalid updater signature.")?;
    key.verify(bytes, &signature, true)
        .map_err(|_| "Updater signature verification failed.".into())
}
fn macos_supported(minimum: &str) -> Result<(), String> {
    let output = std::process::Command::new("/usr/bin/sw_vers")
        .arg("-productVersion")
        .output()
        .map_err(|_| "Cannot verify the macOS version.")?;
    let parse = |value: &str| -> Option<Vec<u32>> {
        value.trim().split('.').map(|s| s.parse().ok()).collect()
    };
    let mut current =
        parse(std::str::from_utf8(&output.stdout).map_err(|_| "Invalid macOS version.")?)
            .ok_or("Invalid macOS version.")?;
    let mut required = parse(minimum).ok_or("Invalid minimum macOS version.")?;
    if !output.status.success()
        || current.is_empty()
        || required.is_empty()
        || current.len() > 3
        || required.len() > 3
    {
        return Err("Cannot verify macOS compatibility.".into());
    }
    current.resize(3, 0);
    required.resize(3, 0);
    if current < required {
        return Err("This release requires a newer macOS version.".into());
    }
    Ok(())
}
fn journal(app: &AppHandle, status: &str, version: &str) -> Result<(), String> {
    let root = app
        .path()
        .app_data_dir()
        .map_err(|_| "Update journal location unavailable.")?;
    std::fs::create_dir_all(&root).map_err(|_| "Update journal directory unavailable.")?;
    let pending = root.join("update-journal.pending");
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut output = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&pending)
            .map_err(|_| "Cannot create update journal.")?;
        std::io::Write::write_all(
            &mut output,
            &serde_json::to_vec(&serde_json::json!({"status":status,"version":version})).unwrap(),
        )
        .map_err(|_| "Cannot persist update journal.")?;
    }

    std::fs::File::open(&pending)
        .and_then(|f| f.sync_all())
        .map_err(|_| "Cannot sync update journal.")?;
    std::fs::rename(&pending, root.join("update-journal.json"))
        .map_err(|_| "Cannot commit update journal.")?;
    std::fs::File::open(&root)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| "Cannot sync update journal directory.".into())
}

pub(crate) fn installation_requires_maintenance(app: &AppHandle) -> bool {
    use std::{
        fs,
        io::Read,
        os::unix::fs::{MetadataExt, OpenOptionsExt},
    };
    let Ok(root) = app.path().app_data_dir() else {
        return true;
    };
    if std::fs::symlink_metadata(root.join("update-journal.pending")).is_ok() {
        return true;
    }
    let path = root.join("update-journal.json");
    let metadata = match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return false,
        Err(_) => return true,
        Ok(value) => value,
    };
    if !metadata.is_file()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
        || metadata.len() > 4096
    {
        return true;
    }
    let Ok(mut file) = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&path)
    else {
        return true;
    };
    let Ok(opened) = file.metadata() else {
        return true;
    };
    if opened.ino() != metadata.ino() || opened.dev() != metadata.dev() {
        return true;
    }
    let mut bytes = Vec::new();
    if file.by_ref().take(4097).read_to_end(&mut bytes).is_err() || bytes.len() > 4096 {
        return true;
    }
    let Ok(text) = std::str::from_utf8(&bytes) else {
        return true;
    };
    let Ok(value) = crate::strict_json::parse(text) else {
        return true;
    };
    if value.as_object().is_none_or(|object| object.len() != 2) {
        return true;
    }
    let Some(status) = value.get("status").and_then(serde_json::Value::as_str) else {
        return true;
    };
    let Some(version) = value.get("version").and_then(serde_json::Value::as_str) else {
        return true;
    };
    if semver::Version::parse(version).is_err() {
        return true;
    }
    match status {
        "interrupted" => false,
        "installed" => version != app.package_info().version.to_string(),
        _ => true,
    }
}

struct CheckOperation;
impl CheckOperation {
    fn begin() -> Result<Self, String> {
        let mut state = state().lock().map_err(|_| "Update state unavailable.")?;
        if state.busy || state.installed.is_some() {
            return Err("An update operation is in progress or awaiting restart.".into());
        }
        state.candidate = None;
        state.busy = true;
        Ok(Self)
    }
}
impl Drop for CheckOperation {
    fn drop(&mut self) {
        if let Ok(mut state) = state().lock() {
            state.busy = false;
        }
    }
}

#[tauri::command]
pub(crate) async fn check_for_update(
    window: WebviewWindow,
    app: AppHandle,
    storage: State<'_, DesktopState>,
) -> Result<UpdateStatus, String> {
    require_main(&window)?;
    let _operation = CheckOperation::begin()?;
    let trust = trust()?;
    let Some(endpoint) = trust.endpoint.as_ref() else {
        return Ok(response(
            &app,
            "blocked",
            None,
            Some("A trusted release endpoint has not been configured.".into()),
        ));
    };
    let endpoint_url = safe_url(endpoint, &trust.allowed_download_hosts)?;
    let document = bounded_download(endpoint_url.clone(), MANIFEST_LIMIT).await?;
    let manifest = verify_manifest(
        &document,
        &trust,
        storage.storage()?.identity().schema_version,
    )?;
    macos_supported(&manifest.minimum_macos)?;
    if semver::Version::parse(&manifest.version).unwrap() <= app.package_info().version {
        return Ok(response(&app, "up_to_date", None, None));
    }
    let update = app
        .updater_builder()
        .pubkey(trust.updater_public_key)
        .endpoints(vec![endpoint_url])
        .map_err(|_| "Invalid updater endpoint.")?
        .configure_client(|client| client.redirect(reqwest::redirect::Policy::none()))
        .build()
        .map_err(|_| "Updater unavailable.")?
        .check_from_json(
            crate::strict_json::parse(
                std::str::from_utf8(&document).map_err(|_| "Invalid release text.")?,
            )
            .map_err(|_| "Ambiguous release JSON.")?,
        )
        .map_err(|_| "Cannot reconcile updater metadata.")?
        .ok_or("Updater metadata does not match the signed manifest.")?;
    if update.version != manifest.version
        || update.download_url.as_str() != manifest.payload.url
        || update.signature != manifest.payload.signature
    {
        return Err("Updater metadata differs from the signed release manifest.".into());
    }
    let result = response(&app, "available", Some(&manifest), None);
    let mut state = state().lock().map_err(|_| "Update state unavailable.")?;
    state.candidate = Some((manifest, update));
    Ok(result)
}

#[tauri::command]
pub(crate) async fn install_update(
    window: WebviewWindow,
    app: AppHandle,
    state: State<'_, DesktopState>,
    expected_candidate_id: String,
) -> Result<UpdateStatus, String> {
    require_main(&window)?;
    let (manifest, update) = {
        let mut state = self::state()
            .lock()
            .map_err(|_| "Update state unavailable.")?;
        if state.busy || state.installed.is_some() {
            return Err("An update is already in progress or awaiting restart.".into());
        }
        validate_reviewed_candidate(
            &state
                .candidate
                .as_ref()
                .ok_or("Check and review a signed release before installation.")?
                .0,
            &expected_candidate_id,
        )?;
        let candidate = state
            .candidate
            .take()
            .ok_or("Check and review a signed release before installation.")?;
        state.busy = true;
        candidate
    };
    let mut admitted = false;
    let mut installed = false;
    let mut durability_unknown = false;
    let outcome = async {
        let storage = state.storage()?;
        crate::commands::acquire_update_admission(&app, &storage)?;
        admitted = true;
        let trust = trust()?;
        let schema = storage.identity().schema_version;
        if schema < manifest.schema_min || schema > manifest.schema_max { return Err("The current database schema is incompatible with this release.".into()); }
        macos_supported(&manifest.minimum_macos)?;
        journal(&app, "downloading", &manifest.version)?;
        let bytes = bounded_download(safe_url(&manifest.payload.url, &trust.allowed_download_hosts)?, manifest.payload.size as usize).await?;
        verify_payload(&bytes, &manifest, &trust.updater_public_key)?;
        let backup_root = app.path().app_data_dir().map_err(|_| "Update backup location unavailable.")?.join("update-backups");
        std::fs::create_dir_all(&backup_root).map_err(|_| "Cannot prepare update backup directory.")?;
        let backup = backup_root.join(uuid::Uuid::new_v4().to_string());
        storage.create_backup(&backup).map_err(|_| "A consistent verified backup is required before installation.")?;
        journal(&app, "installing", &manifest.version)?;
        update.install_macos_with_minimum(&bytes, &manifest.minimum_macos).map_err(|error| {
            durability_unknown = matches!(error, tauri_plugin_updater::Error::InstallationDurabilityUnknown);
            "Verified release installation did not finish. Check the retained recovery bundle before retrying."
        })?;
        installed = true;
        journal(&app, "installed", &manifest.version)?;
        Ok::<_, String>(response(&app, "ready_to_restart", Some(&manifest), None))
    }.await;
    let mut state = self::state()
        .lock()
        .map_err(|_| "Update state unavailable.")?;
    state.busy = false;
    if durability_unknown {
        return Ok(response(&app, "blocked", Some(&manifest), Some("Application exchange committed but journal durability is unconfirmed. New deliberations remain disabled; verify the retained recovery bundle before relaunching.".into())));
    }
    if installed {
        state.installed = Some(manifest.version.clone());
        return Ok(response(
            &app,
            "ready_to_restart",
            Some(&manifest),
            outcome.err(),
        ));
    }
    if admitted {
        crate::commands::release_update_admission(&app);
    }
    if admitted {
        let _ = journal(&app, "interrupted", &manifest.version);
    }
    outcome
}

#[tauri::command]
pub(crate) fn restart_after_update(window: WebviewWindow, app: AppHandle) -> Result<(), String> {
    require_main(&window)?;
    if state()
        .lock()
        .map_err(|_| "Update state unavailable.")?
        .installed
        .is_none()
    {
        return Err("No verified installed update is ready to restart.".into());
    }
    app.restart()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey, pkcs8::EncodePublicKey};
    fn fixture() -> (Trust, Manifest, Vec<u8>) {
        let key = SigningKey::from_bytes(&[41; 32]);
        let trust = Trust {
            schema_version: 1,
            endpoint: None,
            allowed_download_hosts: vec!["github.com".into()],
            updater_public_key: String::new(),
            manifest_public_key_pem: key
                .verifying_key()
                .to_public_key_pem(Default::default())
                .unwrap(),
        };
        let manifest = Manifest {
            version: "0.2.0".into(),
            target: format!("darwin-{}", std::env::consts::ARCH),
            minimum_macos: "12.0".into(),
            schema_min: 7,
            schema_max: 7,
            payload: Payload {
                url:
                    "https://github.com/Seungwoo321/oh-my-magi/releases/download/v0.2.0/app.tar.gz"
                        .into(),
                sha256: "a".repeat(64),
                size: 42,
                signature: String::new(),
            },
            notes: "Reviewed notes".into(),
        };
        let payload = serde_json::to_vec(&manifest).unwrap();
        let document = serde_json::to_vec(&serde_json::json!({"signedPayload": STANDARD.encode(&payload), "manifestSignature": STANDARD.encode(key.sign(&payload).to_bytes()), "version": manifest.version, "notes": manifest.notes, "platforms": {manifest.target.clone(): {"url": manifest.payload.url, "signature": manifest.payload.signature}}})).unwrap();
        (trust, manifest, document)
    }
    #[test]
    fn reviewed_candidate_is_bound_to_every_verified_manifest_field() {
        let (_, manifest, _) = fixture();
        let id = candidate_id(&manifest);
        validate_reviewed_candidate(&manifest, &id).unwrap();
        let mut changed = manifest.clone();
        changed.version = "0.9.0".into();
        assert!(validate_reviewed_candidate(&changed, &id).is_err());
        changed = manifest;
        changed.notes.push_str(" changed");
        assert!(validate_reviewed_candidate(&changed, &id).is_err());
    }
    #[test]
    fn checks_exclude_concurrent_operations_and_release_on_failure_but_not_restart_wait() {
        let operation = CheckOperation::begin().unwrap();
        assert!(CheckOperation::begin().is_err());
        drop(operation);
        let retried = CheckOperation::begin().unwrap();
        drop(retried);
        state().lock().unwrap().installed = Some("0.2.0".into());
        assert!(CheckOperation::begin().is_err());
        state().lock().unwrap().installed = None;
    }
    #[test]
    fn signed_manifest_is_verified_and_tampering_is_rejected() {
        let (trust, _, document) = fixture();
        assert_eq!(
            verify_manifest(&document, &trust, 7).unwrap().version,
            "0.2.0"
        );
        let mut value: serde_json::Value = serde_json::from_slice(&document).unwrap();
        value["signedPayload"] = serde_json::Value::String(STANDARD.encode(b"tampered"));
        assert!(verify_manifest(&serde_json::to_vec(&value).unwrap(), &trust, 7).is_err());
    }
    #[test]
    fn transport_rejects_http_credentials_redirect_hosts_and_nonstandard_ports() {
        let (trust, _, _) = fixture();
        for url in [
            "http://releases.example.test/app",
            "https://user@releases.example.test/app",
            "https://elsewhere.example.test/app",
            "https://releases.example.test:8443/app",
        ] {
            assert!(safe_url(url, &trust.allowed_download_hosts).is_err());
        }
    }
    #[test]
    fn github_download_chain_rejects_other_repositories_wildcards_credentials_and_cdn_redirects() {
        let initial = reqwest::Url::parse("https://github.com/Seungwoo321/oh-my-magi/releases/latest/download/release-manifest.json").unwrap();
        let pinned = "https://github.com/Seungwoo321/oh-my-magi/releases/download/v0.2.0/release-manifest.json";
        let cdn = "https://release-assets.githubusercontent.com/github-production-release-asset/123/asset?signature=inert-test";
        verified_redirect(&initial, pinned).unwrap();
        let final_url = verified_redirect(&reqwest::Url::parse(pinned).unwrap(), cdn).unwrap();
        assert!(verified_redirect(&final_url, pinned).is_err());
        for destination in [
            "http://github.com/Seungwoo321/oh-my-magi/releases/download/v0.2.0/app",
            "https://github.com/elsewhere/project/releases/download/v0.2.0/app",
            "https://release-assets.githubusercontent.com.evil.test/github-production-release-asset/1/asset",
            "https://user:password@release-assets.githubusercontent.com/github-production-release-asset/1/asset",
            "https://release-assets.githubusercontent.com/elsewhere",
        ] {
            assert!(verified_redirect(&initial, destination).is_err());
        }
        assert!(repository_asset_url(&reqwest::Url::parse(cdn).unwrap()).is_err());
    }
    #[test]
    fn unsigned_payload_never_reaches_installation() {
        let (trust, manifest, _) = fixture();
        assert!(verify_payload(b"unexpected bytes", &manifest, &trust.updater_public_key).is_err());
    }
    #[test]
    fn compatibility_uses_actual_storage_schema_and_rejects_old_schema_only_release() {
        let (trust, _, document) = fixture();
        assert!(verify_manifest(&document, &trust, 7).is_ok());
        assert!(verify_manifest(&document, &trust, 1).is_err());
        assert!(verify_manifest(&document, &trust, 8).is_err());
    }
    #[test]
    fn duplicate_or_unknown_release_fields_are_rejected_before_interpretation() {
        let (trust, _, document) = fixture();
        let text = String::from_utf8(document).unwrap();
        let duplicate = format!("{{\"version\":\"untrusted\",{}", &text[1..]);
        assert!(verify_manifest(duplicate.as_bytes(), &trust, 7).is_err());
        let mut value: serde_json::Value = serde_json::from_str(&text).unwrap();
        value["executable"] = serde_json::json!("untrusted command");
        assert!(verify_manifest(&serde_json::to_vec(&value).unwrap(), &trust, 7).is_err());
    }
    #[test]
    #[ignore = "requires a locally generated Tauri-signed artifact"]
    fn actual_tauri_signer_output_matches_updater_verifier_when_supplied() {
        let artifact =
            std::env::var("MAGI_UPDATER_TEST_ARTIFACT").expect("local signed artifact path");
        let bytes = std::fs::read(&artifact).unwrap();
        let signature = std::fs::read_to_string(format!("{artifact}.sig")).unwrap();
        let (_, mut manifest, _) = fixture();
        manifest.payload.size = bytes.len() as u64;
        manifest.payload.sha256 = format!("{:x}", Sha256::digest(&bytes));
        manifest.payload.signature = signature.trim().into();
        verify_payload(&bytes, &manifest, &trust().unwrap().updater_public_key).unwrap();
        let mut changed = bytes;
        changed.push(1);
        manifest.payload.size = changed.len() as u64;
        manifest.payload.sha256 = format!("{:x}", Sha256::digest(&changed));
        assert!(verify_payload(&changed, &manifest, &trust().unwrap().updater_public_key).is_err());
    }
}
