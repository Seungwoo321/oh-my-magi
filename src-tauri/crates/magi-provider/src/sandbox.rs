use crate::{
    CODEX_ACP_GIT_COMMIT, CODEX_ACP_NPM_INTEGRITY, CODEX_ACP_PACKAGE, CODEX_ACP_VERSION,
    CODEX_BINARY_NPM_INTEGRITY, CODEX_BINARY_PACKAGE, CODEX_BINARY_VERSION, ProviderError,
    SandboxPolicyFailureCode,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command as StdCommand, Stdio};
use tokio::process::Command;

const PUBLIC_CA_SOURCE_URL: &str = "https://curl.se/ca/cacert-2026-09-25.pem";
const PUBLIC_CA_SHA256: &str = "a41b5d356aea97a529fe27e0f7316d2f9d946d75927476cf9cf1b90637d00505";
const CODEX_SOURCE_COMMIT: &str = "b412ff32c417f855c2b2d1581b77058eed87c84b";
const CODEX_SOURCE_SHA256: &str =
    "1ac6a92e7318b8acf3d767170c5c5e6dceeffdc074c73b1c5d422b46f0de4daf";
const CODEX_SOURCE_PATCH_ID: &str = "codex-http-ca-preserve-backend-v1";
const CODEX_SOURCE_PATCH_SHA256: &str =
    "b08f4099725b6394e5657691e10d2dc8d9696cd119623fa6155db28a70d76d54";
const CODEX_SOURCE_LOCK_SHA256: &str =
    "d722f05fc760bcd1f5749ec452452d81058458b788df3b765b80500d757eba4a";

const PROVIDER_CONFIG: &str = r#"approval_policy = "never"
allow_login_shell = false
default_permissions = "magi_role_read"
web_search = "disabled"
project_doc_max_bytes = 0
project_doc_fallback_filenames = []
include_environment_context = false

[permissions.magi_role_read.filesystem]
":root" = "deny"
":workspace_roots" = "read"
":tmpdir" = "deny"
":slash_tmp" = "deny"

[permissions.magi_role_read.network]
enabled = false

[features]
shell_tool = false
unified_exec = false
unified_exec_tty = false
shell_zsh_fork = false
code_mode = false
code_mode_host = false
code_mode_only = false
apps = false
multi_agent = false
multi_agent_v2 = false
agent_message_board = false
memories = false
external_agent_memory_import = false
chronicle = false
plugins = false
worktrees = false
view_image = false
unified_image_budget = false
realtime_conversation = false
sleep_tool = false
shell_snapshot = false
"#;
const CODEX_ACP_ADAPTER_PATCH_ID_SOURCE: &str = include_str!("codex_acp_adapter_patch_id.txt");
const CODEX_ACP_ADAPTER_PATCH_ID_PREFIX: &str = "magi-home-lock-auth-progress-v";
const APPLE_DEVELOPER_TEAM_ID: &str = "95B7J2U49K";
const CODEX_ACP_ARM64_UPSTREAM_ARTIFACT_SHA256: &str =
    "69a7752a9092ea7734518e59ddcba808bcf4b216737539b50382005055b6aa01";

const SEATBELT_PROFILE: &str = r#"(version 1)
(deny default)
(import "system.sb")
(allow file-read-metadata (path-ancestors (param "PROFILE_HOME")))
(allow file-read-metadata (path-ancestors (param "ROLE_WORKDIR")))
(allow file-read-metadata (path-ancestors (param "ARTIFACT_DIR")))
(allow file-read-metadata (path-ancestors (param "EXECUTABLE")))
(allow file-read* (subpath (param "PROFILE_HOME")))
(allow file-write* (subpath (param "PROFILE_HOME")))
(allow file-read* (subpath (param "ROLE_WORKDIR")))
(allow file-read* file-map-executable (subpath (param "ARTIFACT_DIR")))
(allow file-read* file-map-executable (subpath "/System"))
(allow file-read* file-map-executable (subpath "/usr/lib"))
(allow file-read* file-map-executable (subpath "/private/etc/ssl"))
(allow process-exec (literal (param "EXECUTABLE")))
(allow process-exec (literal (param "CODEX_EXECUTABLE")))
(allow process-exec (literal (param "RIPGREP_EXECUTABLE")))
; Codex's Seatbelt baseline permits process-fork for its app-server subprocess.
(allow process-fork)
; Platform TLS trust lookup requires this exact service; other Mach services remain denied.
(allow mach-lookup (global-name "com.apple.SecurityServer"))
(allow network-outbound (remote ip "localhost:{PROXY_PORT}"))
"#;
const SANDBOX_PROBE_STDERR_LIMIT: usize = 2048;
const SANDBOX_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

pub fn codex_acp_adapter_patch_id() -> Option<&'static str> {
    let identity = CODEX_ACP_ADAPTER_PATCH_ID_SOURCE
        .strip_suffix('\n')
        .unwrap_or(CODEX_ACP_ADAPTER_PATCH_ID_SOURCE);
    let version = identity.strip_prefix(CODEX_ACP_ADAPTER_PATCH_ID_PREFIX)?;
    if version.is_empty() || !version.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some(identity)
}

#[derive(Clone)]
pub struct CodexAcpLaunch {
    pub executable: PathBuf,
    pub expected_executable_sha256: String,
    pub provider_profile_id: String,
    pub profile_revision: u64,
    pub runtime_home_id: String,
    pub profile_home: PathBuf,
    pub role_workdir: PathBuf,
}

pub(crate) struct PreparedLaunch {
    pub artifact_identity: RuntimeArtifactIdentity,
    pub executable: PathBuf,
    pub codex_executable: PathBuf,
    pub ripgrep_executable: PathBuf,
    pub public_ca_bundle: PathBuf,
    pub artifact_dir: PathBuf,
    pub provider_profile_id: String,
    pub profile_revision: u64,
    pub runtime_home_id: String,
    pub profile_home: PathBuf,
    pub role_workdir: PathBuf,
    pub temporary_dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeArtifactIdentity {
    pub acp_executable_sha256: String,
    pub artifact_set_digest: Option<String>,
}

#[derive(Clone)]
pub(crate) struct VerifiedArtifact {
    codex_executable: PathBuf,
    ripgrep_executable: PathBuf,
    public_ca_bundle: PathBuf,
    pub(crate) identity: RuntimeArtifactIdentity,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct BuildManifest {
    schema_version: u32,
    provider: String,
    package: String,
    package_version: String,
    package_integrity: String,
    source_commit: String,
    bundled_codex_package: String,
    bundled_codex_version: String,
    bundled_codex_integrity: String,
    codex_platform_package: String,
    codex_platform_version: String,
    codex_platform_integrity: String,
    codex_target_triple: String,
    target: String,
    upstream_artifact_sha256: String,
    adapter_patch_id: String,
    artifact_sha256: String,
    codex_executable_sha256: String,
    ripgrep_sha256: String,
    public_ca_source_url: String,
    public_ca_sha256: String,
    #[serde(default)]
    codex_source_commit: Option<String>,
    #[serde(default)]
    codex_source_sha256: Option<String>,
    #[serde(default)]
    codex_source_patch_id: Option<String>,
    #[serde(default)]
    codex_source_patch_sha256: Option<String>,
    #[serde(default)]
    codex_source_lock_sha256: Option<String>,
    status: String,
}

fn runtime_provenance_matches(manifest: &BuildManifest) -> bool {
    let actual = [
        manifest.codex_source_commit.as_deref(),
        manifest.codex_source_sha256.as_deref(),
        manifest.codex_source_patch_id.as_deref(),
        manifest.codex_source_patch_sha256.as_deref(),
        manifest.codex_source_lock_sha256.as_deref(),
    ];
    match manifest.schema_version {
        4 => actual.iter().all(Option::is_none),
        5 => {
            actual
                == [
                    Some(CODEX_SOURCE_COMMIT),
                    Some(CODEX_SOURCE_SHA256),
                    Some(CODEX_SOURCE_PATCH_ID),
                    Some(CODEX_SOURCE_PATCH_SHA256),
                    Some(CODEX_SOURCE_LOCK_SHA256),
                ]
        }
        _ => false,
    }
}

fn parse_build_manifest(bytes: &[u8]) -> Result<BuildManifest, ProviderError> {
    if bytes.len() > 65_536 {
        return Err(ProviderError::ArtifactVerification);
    }
    let manifest: BuildManifest =
        serde_json::from_slice(bytes).map_err(|_| ProviderError::ArtifactVerification)?;
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| ProviderError::ArtifactVerification)?;
    if value
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        == Some(4)
        && [
            "codex_source_commit",
            "codex_source_sha256",
            "codex_source_patch_id",
            "codex_source_patch_sha256",
            "codex_source_lock_sha256",
        ]
        .iter()
        .any(|key| value.get(key).is_some())
    {
        return Err(ProviderError::ArtifactVerification);
    }
    if !runtime_provenance_matches(&manifest) {
        return Err(ProviderError::ArtifactVerification);
    }
    Ok(manifest)
}

#[cfg(test)]
pub(crate) fn runtime_manifest_provenance_valid(bytes: &[u8]) -> bool {
    parse_build_manifest(bytes).is_ok()
}

pub(crate) fn prepare(launch: &CodexAcpLaunch) -> Result<PreparedLaunch, ProviderError> {
    prepare_inner(launch, None)
}

pub(crate) fn prepare_verified(
    launch: &CodexAcpLaunch,
    artifact: &crate::VerifiedRuntimeArtifact,
    request: &crate::VerificationRequest,
) -> Result<PreparedLaunch, ProviderError> {
    artifact.check(request)?;
    if artifact.executable() != launch.executable
        || artifact.identity().acp_executable_sha256 != launch.expected_executable_sha256
    {
        return Err(ProviderError::ArtifactVerification);
    }
    prepare_inner(launch, Some(artifact.artifact.clone()))
}

fn prepare_inner(
    launch: &CodexAcpLaunch,
    verified: Option<VerifiedArtifact>,
) -> Result<PreparedLaunch, ProviderError> {
    if !cfg!(target_os = "macos") {
        return Err(ProviderError::UnsupportedPlatform);
    }

    let expected_target = match std::env::consts::ARCH {
        "aarch64" => "darwin-arm64",
        _ => return Err(ProviderError::UnsupportedPlatform),
    };
    if !launch.executable.is_absolute()
        || !launch.profile_home.is_absolute()
        || !launch.role_workdir.is_absolute()
        || !is_safe_profile_identity(&launch.provider_profile_id)
        || !is_safe_profile_identity(&launch.runtime_home_id)
        || launch.expected_executable_sha256.len() != 64
        || !launch
            .expected_executable_sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(ProviderError::InvalidLaunch);
    }

    let executable = canonical_regular_file(&launch.executable)?;
    if executable.file_name().and_then(|name| name.to_str()) != Some("codex-acp") {
        return Err(ProviderError::ArtifactVerification);
    }
    let artifact_dir = executable
        .parent()
        .ok_or(ProviderError::ArtifactVerification)?
        .canonicalize()
        .map_err(|_| ProviderError::ArtifactVerification)?;
    if artifact_dir.file_name().and_then(|name| name.to_str()) != Some(expected_target) {
        return Err(ProviderError::ArtifactVerification);
    }
    let profile_home = canonical_private_dir(&launch.profile_home, true)?;
    if profile_home.file_name().and_then(|name| name.to_str())
        != Some(launch.runtime_home_id.as_str())
    {
        return Err(ProviderError::InvalidLaunch);
    }
    let role_workdir = canonical_private_dir(&launch.role_workdir, false)?;
    if !is_empty(&role_workdir)?
        || overlaps(&profile_home, &role_workdir)
        || overlaps(&artifact_dir, &profile_home)
        || overlaps(&artifact_dir, &role_workdir)
    {
        return Err(ProviderError::InvalidLaunch);
    }

    let artifact = match verified {
        Some(artifact) => artifact,
        None => crate::verification::with_bounded_proof(
            crate::VerificationRequest::until(
                std::time::Instant::now() + std::time::Duration::from_secs(60),
            ),
            || {
                verify_artifact(
                    &executable,
                    &artifact_dir,
                    expected_target,
                    &launch.expected_executable_sha256,
                )
            },
        )?,
    };

    let temporary_dir = profile_home.join("tmp");
    create_private_directory(&temporary_dir, ProviderError::ProfileHomeUnavailable)?;
    persist_provider_config(&profile_home)?;

    Ok(PreparedLaunch {
        artifact_identity: artifact.identity,
        executable,
        codex_executable: artifact.codex_executable,
        ripgrep_executable: artifact.ripgrep_executable,
        public_ca_bundle: artifact.public_ca_bundle,
        artifact_dir,
        provider_profile_id: launch.provider_profile_id.clone(),
        profile_revision: launch.profile_revision,
        runtime_home_id: launch.runtime_home_id.clone(),
        profile_home,
        role_workdir,
        temporary_dir,
    })
}

pub fn verify_packaged_artifact(executable: &Path) -> Result<(), ProviderError> {
    verify_packaged_artifact_identity(executable).map(|_| ())
}

pub fn verify_packaged_artifact_identity(
    executable: &Path,
) -> Result<RuntimeArtifactIdentity, ProviderError> {
    crate::verification::with_bounded_proof(
        crate::VerificationRequest::until(
            std::time::Instant::now() + std::time::Duration::from_secs(60),
        ),
        || verify_packaged_artifact_details(executable).map(|artifact| artifact.identity),
    )
}

pub(crate) fn verify_packaged_artifact_details(
    executable: &Path,
) -> Result<VerifiedArtifact, ProviderError> {
    if !cfg!(target_os = "macos") {
        return Err(ProviderError::UnsupportedPlatform);
    }
    let target = match std::env::consts::ARCH {
        "aarch64" => "darwin-arm64",
        _ => return Err(ProviderError::UnsupportedPlatform),
    };
    let executable = canonical_regular_file(executable)?;
    if executable.file_name().and_then(|name| name.to_str()) != Some("codex-acp") {
        return Err(ProviderError::ArtifactVerification);
    }
    let artifact_dir = executable
        .parent()
        .ok_or(ProviderError::ArtifactVerification)?
        .canonicalize()
        .map_err(|_| ProviderError::ArtifactVerification)?;
    if artifact_dir.file_name().and_then(|name| name.to_str()) != Some(target) {
        return Err(ProviderError::ArtifactVerification);
    }
    let digest = sha256_file(&executable)?;
    verify_artifact(&executable, &artifact_dir, target, &digest)
}

fn is_safe_profile_identity(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

pub(crate) fn isolated_command(
    prepared: &PreparedLaunch,
    proxy_remote: SocketAddr,
    proxy_url: &str,
) -> Result<Command, ProviderError> {
    isolated_command_for(prepared, &prepared.executable, &[], proxy_remote, proxy_url)
}

fn isolated_command_for(
    prepared: &PreparedLaunch,
    executable: &Path,
    arguments: &[&str],
    proxy_remote: SocketAddr,
    proxy_url: &str,
) -> Result<Command, ProviderError> {
    if !Path::new("/usr/bin/sandbox-exec").is_file()
        || !matches!(proxy_remote.ip(), IpAddr::V4(ip) if ip == Ipv4Addr::LOCALHOST)
        || proxy_remote.port() == 0
        || !is_proxy_url_bound_to(proxy_url, proxy_remote.port())
    {
        return Err(ProviderError::IsolationUnavailable);
    }

    if verify_public_ca_bundle(&prepared.artifact_dir)? != prepared.public_ca_bundle {
        return Err(ProviderError::ArtifactVerification);
    }
    validate_sandbox_policy(prepared, proxy_remote.port())?;
    let policy = seatbelt_profile(proxy_remote.port(), false);
    let definitions = sandbox_definitions(prepared, &prepared.executable)?;

    let mut command = Command::new("/usr/bin/sandbox-exec");
    command.arg("-p").arg(policy);
    for definition in definitions {
        command.arg("-D").arg(definition);
    }
    command
        .arg(executable)
        .args(arguments)
        .current_dir(&prepared.role_workdir)
        .env_clear()
        .env("HOME", &prepared.profile_home)
        .env("CODEX_HOME", &prepared.profile_home)
        .env("CODEX_PATH", &prepared.codex_executable)
        .env("CODEX_CA_CERTIFICATE", &prepared.public_ca_bundle)
        .env("TMPDIR", &prepared.temporary_dir)
        .env("PATH", "/usr/bin:/bin")
        .env("HTTP_PROXY", proxy_url)
        .env("HTTPS_PROXY", proxy_url)
        .env("ALL_PROXY", proxy_url)
        .env("http_proxy", proxy_url)
        .env("https_proxy", proxy_url)
        .env("all_proxy", proxy_url)
        .env("NO_PROXY", "")
        .env("no_proxy", "")
        .env("LANG", "en_US.UTF-8")
        .env("LC_ALL", "en_US.UTF-8")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .process_group(0);
    Ok(command)
}

fn seatbelt_profile(proxy_port: u16, probe: bool) -> String {
    let policy = SEATBELT_PROFILE.replace("{PROXY_PORT}", &proxy_port.to_string());
    #[cfg(test)]
    let policy = if std::env::var_os("MAGI_TEST_DENY_PLATFORM_TRUST").is_some() {
        policy.replace(
            "(allow mach-lookup (global-name \"com.apple.SecurityServer\"))",
            "",
        )
    } else {
        policy
    };
    if probe {
        policy.replace(
            "(deny default)",
            "(deny default)\n(allow file-read* file-map-executable (literal \"/usr/bin/true\"))",
        )
    } else {
        policy
    }
}

fn sandbox_definitions(
    prepared: &PreparedLaunch,
    executable: &Path,
) -> Result<Vec<String>, ProviderError> {
    Ok(vec![
        format!("PROFILE_HOME={}", path_arg(&prepared.profile_home)?),
        format!("ROLE_WORKDIR={}", path_arg(&prepared.role_workdir)?),
        format!("ARTIFACT_DIR={}", path_arg(&prepared.artifact_dir)?),
        format!("EXECUTABLE={}", path_arg(executable)?),
        format!("CODEX_EXECUTABLE={}", path_arg(&prepared.codex_executable)?),
        format!(
            "RIPGREP_EXECUTABLE={}",
            path_arg(&prepared.ripgrep_executable)?
        ),
    ])
}

fn validate_sandbox_policy(
    prepared: &PreparedLaunch,
    proxy_port: u16,
) -> Result<(), ProviderError> {
    let executable = Path::new("/usr/bin/true");
    let definitions = sandbox_definitions(prepared, executable)?;
    let mut command = StdCommand::new("/usr/bin/sandbox-exec");
    command.arg("-p").arg(seatbelt_profile(proxy_port, true));
    for definition in definitions {
        command.arg("-D").arg(definition);
    }
    command
        .arg(executable)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());

    let mut child = command
        .spawn()
        .map_err(|_| ProviderError::IsolationUnavailable)?;
    let stderr = child
        .stderr
        .take()
        .ok_or(ProviderError::IsolationUnavailable)?;
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::with_capacity(SANDBOX_PROBE_STDERR_LIMIT);
        let _ = stderr
            .take(SANDBOX_PROBE_STDERR_LIMIT as u64)
            .read_to_end(&mut bytes);
        bytes
    });

    let started = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if started.elapsed() < SANDBOX_PROBE_TIMEOUT => {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                return Err(ProviderError::SandboxPolicyRejected {
                    code: SandboxPolicyFailureCode::ProbeTimedOut,
                    exit_status: None,
                });
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                return Err(ProviderError::IsolationUnavailable);
            }
        }
    };
    let diagnostic = reader.join().unwrap_or_default();
    let Some(status) = status else {
        return Err(ProviderError::IsolationUnavailable);
    };
    if status.success() {
        return Ok(());
    }

    let diagnostic = String::from_utf8_lossy(&diagnostic);
    let code = if diagnostic.contains("host must be * or localhost in network address") {
        SandboxPolicyFailureCode::NetworkRuleRejected
    } else {
        SandboxPolicyFailureCode::ProfileRejected
    };
    Err(ProviderError::SandboxPolicyRejected {
        code,
        exit_status: status.code(),
    })
}

fn is_proxy_url_bound_to(proxy_url: &str, port: u16) -> bool {
    let Some(credentials) = proxy_url.strip_prefix("http://magi:") else {
        return false;
    };
    let Some(token) = credentials.strip_suffix(&format!("@127.0.0.1:{port}")) else {
        return false;
    };
    token.len() == 64 && token.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn verify_artifact(
    executable: &Path,
    artifact_dir: &Path,
    target: &str,
    expected_digest: &str,
) -> Result<VerifiedArtifact, ProviderError> {
    let expected_adapter_patch_id =
        codex_acp_adapter_patch_id().ok_or(ProviderError::ArtifactVerification)?;
    let executable_digest = sha256_file(executable)?;
    if !executable_digest.eq_ignore_ascii_case(expected_digest) {
        return Err(ProviderError::ArtifactVerification);
    }

    let manifest_path = artifact_dir.join("build-manifest.json");
    let manifest_metadata =
        fs::symlink_metadata(&manifest_path).map_err(|_| ProviderError::ArtifactVerification)?;
    if !manifest_metadata.file_type().is_file()
        || manifest_metadata.mode() & 0o222 != 0
        || manifest_metadata.nlink() != 1
        || manifest_metadata.len() > 65_536
    {
        return Err(ProviderError::ArtifactVerification);
    }
    let mut bytes = Vec::new();
    crate::verification::open_proof_file(&manifest_path)?
        .take(65_537)
        .read_to_end(&mut bytes)
        .map_err(|_| ProviderError::ArtifactVerification)?;
    if bytes.len() > 65_536 {
        return Err(ProviderError::ArtifactVerification);
    }
    let manifest = parse_build_manifest(&bytes)?;
    let (
        expected_platform_package,
        expected_platform_version,
        expected_platform_integrity,
        target_triple,
        architecture,
        expected_upstream_artifact_sha256,
    ) = match target {
        "darwin-arm64" => (
            "@openai/codex-darwin-arm64",
            "0.156.1-darwin-arm64",
            "sha512-Jg6wbdV+wmMZczhwE74GSxOYEZlViKXn6KyCw/yfrz3PAKFD14xljuPopmdhWC1+8IKU2WdN5fdmXNPt2q4HPA==",
            "aarch64-apple-darwin",
            "arm64",
            CODEX_ACP_ARM64_UPSTREAM_ARTIFACT_SHA256,
        ),
        _ => return Err(ProviderError::UnsupportedPlatform),
    };
    if !runtime_provenance_matches(&manifest)
        || manifest.public_ca_source_url != PUBLIC_CA_SOURCE_URL
        || manifest.public_ca_sha256 != PUBLIC_CA_SHA256
        || manifest.provider != "codex-acp"
        || manifest.package != CODEX_ACP_PACKAGE
        || manifest.package_version != CODEX_ACP_VERSION
        || manifest.package_integrity != CODEX_ACP_NPM_INTEGRITY
        || manifest.source_commit != CODEX_ACP_GIT_COMMIT
        || manifest.bundled_codex_package != CODEX_BINARY_PACKAGE
        || manifest.bundled_codex_version != CODEX_BINARY_VERSION
        || manifest.bundled_codex_integrity != CODEX_BINARY_NPM_INTEGRITY
        || manifest.codex_platform_package != expected_platform_package
        || manifest.codex_platform_version != expected_platform_version
        || manifest.codex_platform_integrity != expected_platform_integrity
        || manifest.codex_target_triple != target_triple
        || manifest.target != target
        || manifest.upstream_artifact_sha256 != expected_upstream_artifact_sha256
        || manifest.adapter_patch_id != expected_adapter_patch_id
        || manifest.status != "built_unadmitted"
        || !manifest
            .artifact_sha256
            .eq_ignore_ascii_case(&executable_digest)
    {
        return Err(ProviderError::ArtifactVerification);
    }

    let codex_executable = canonical_regular_file(
        &artifact_dir
            .join("vendor")
            .join(target_triple)
            .join("bin")
            .join("codex"),
    )?;
    let ripgrep_executable = canonical_regular_file(
        &artifact_dir
            .join("vendor")
            .join(target_triple)
            .join("codex-path")
            .join("rg"),
    )?;
    if !codex_executable.starts_with(artifact_dir)
        || !ripgrep_executable.starts_with(artifact_dir)
        || !sha256_file(&codex_executable)?.eq_ignore_ascii_case(&manifest.codex_executable_sha256)
        || !sha256_file(&ripgrep_executable)?.eq_ignore_ascii_case(&manifest.ripgrep_sha256)
        || !verify_signed_architecture(executable, architecture)
        || !verify_signed_architecture(&codex_executable, architecture)
        || !verify_signed_architecture(&ripgrep_executable, architecture)
    {
        return Err(ProviderError::ArtifactVerification);
    }
    let public_ca_bundle = verify_public_ca_bundle(artifact_dir)?;
    Ok(VerifiedArtifact {
        codex_executable,
        ripgrep_executable,
        public_ca_bundle,
        identity: artifact_identity(&manifest)?,
    })
}

fn artifact_identity(manifest: &BuildManifest) -> Result<RuntimeArtifactIdentity, ProviderError> {
    let artifact_set_digest = if manifest.schema_version == 5 {
        let canonical =
            serde_json::to_vec(manifest).map_err(|_| ProviderError::ArtifactVerification)?;
        let mut digest = Sha256::new();
        digest.update(b"magi-runtime-artifact-set-v1\0");
        digest.update(canonical);
        Some(format!("{:x}", digest.finalize()))
    } else {
        None
    };
    Ok(RuntimeArtifactIdentity {
        acp_executable_sha256: manifest.artifact_sha256.to_ascii_lowercase(),
        artifact_set_digest,
    })
}

pub(crate) fn verify_public_ca_bundle(artifact_dir: &Path) -> Result<PathBuf, ProviderError> {
    let path = artifact_dir.join("cacert.pem");
    let canonical = canonical_regular_file(&path)?;
    if canonical.parent() != Some(artifact_dir) {
        return Err(ProviderError::ArtifactVerification);
    }
    let original = fs::symlink_metadata(&path).map_err(|_| ProviderError::ArtifactVerification)?;
    let file = crate::verification::open_proof_file(&canonical)?;
    let metadata = file
        .metadata()
        .map_err(|_| ProviderError::ArtifactVerification)?;
    if !metadata.is_file()
        || metadata.mode() & 0o222 != 0
        || metadata.nlink() != 1
        || metadata.len() > 4 * 1024 * 1024
        || metadata.dev() != original.dev()
        || metadata.ino() != original.ino()
    {
        return Err(ProviderError::ArtifactVerification);
    }
    let mut bytes = Vec::new();
    file.take(4 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ProviderError::ArtifactVerification)?;
    let started = std::time::Instant::now();
    let digest = format!("{:x}", Sha256::digest(&bytes));
    crate::verification::record_hash(bytes.len() as u64, started.elapsed());
    if digest != PUBLIC_CA_SHA256 {
        return Err(ProviderError::ArtifactVerification);
    }
    Ok(canonical)
}

fn verify_signed_architecture(path: &Path, architecture: &str) -> bool {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return false;
    };
    if !metadata.file_type().is_file() {
        return false;
    }
    let signature = crate::verification::bounded_output(
        std::process::Command::new("/usr/bin/codesign")
            .arg("--verify")
            .arg("--strict")
            .arg(path),
    );
    if !signature.is_ok_and(|output| output.status.success()) {
        return false;
    }
    let details = crate::verification::bounded_output(
        std::process::Command::new("/usr/bin/codesign")
            .arg("--display")
            .arg("--verbose=2")
            .arg(path),
    );
    let Ok(details) = details else {
        return false;
    };
    if !details.status.success() {
        return false;
    }
    let details = String::from_utf8_lossy(&details.stderr);
    let expected_team = APPLE_DEVELOPER_TEAM_ID;
    #[cfg(test)]
    let expected_team = if path.file_name().and_then(|name| name.to_str()) == Some("codex")
        && sha256_file(path).is_ok_and(|digest| {
            digest == "0196e89fe5a7598f816ee54232c3d7c26d75e502ab5cfe2c9240e81d90f7255a"
        }) {
        "2DC432GLL2"
    } else {
        expected_team
    };
    if !details.contains(&format!("TeamIdentifier={expected_team}"))
        || !details.contains("Authority=Developer ID Application:")
    {
        return false;
    }
    crate::verification::bounded_output(
        std::process::Command::new("/usr/bin/lipo")
            .arg(path)
            .arg("-verify_arch")
            .arg(architecture),
    )
    .is_ok_and(|output| output.status.success())
}

fn canonical_private_dir(path: &Path, profile_home: bool) -> Result<PathBuf, ProviderError> {
    if !path.is_absolute() {
        return Err(directory_error(profile_home));
    }
    let metadata = fs::symlink_metadata(path).map_err(|_| directory_error(profile_home))?;
    if !metadata.file_type().is_dir() {
        return Err(directory_error(profile_home));
    }
    let canonical = path
        .canonicalize()
        .map_err(|_| directory_error(profile_home))?;
    let metadata = fs::symlink_metadata(&canonical).map_err(|_| directory_error(profile_home))?;
    if !metadata.file_type().is_dir()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
        || canonical == Path::new("/")
    {
        return Err(directory_error(profile_home));
    }
    Ok(canonical)
}

fn canonical_regular_file(path: &Path) -> Result<PathBuf, ProviderError> {
    let original_metadata =
        fs::symlink_metadata(path).map_err(|_| ProviderError::ArtifactVerification)?;
    if !original_metadata.file_type().is_file() || original_metadata.mode() & 0o222 != 0 {
        return Err(ProviderError::ArtifactVerification);
    }
    let canonical = path
        .canonicalize()
        .map_err(|_| ProviderError::ArtifactVerification)?;
    let metadata =
        fs::symlink_metadata(&canonical).map_err(|_| ProviderError::ArtifactVerification)?;
    if !metadata.file_type().is_file() || metadata.mode() & 0o222 != 0 || metadata.nlink() != 1 {
        return Err(ProviderError::ArtifactVerification);
    }
    Ok(canonical)
}

fn create_private_directory(path: &Path, error: ProviderError) -> Result<(), ProviderError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if !metadata.file_type().is_dir()
                || metadata.uid() != unsafe { libc::geteuid() }
                || metadata.mode() & 0o077 != 0
            {
                return Err(error);
            }
        }
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            let mut builder = fs::DirBuilder::new();
            builder.mode(0o700);
            builder
                .create(path)
                .map_err(|_| ProviderError::ProfileHomeUnavailable)?;
        }
        Err(_) => return Err(error),
    }
    Ok(())
}

fn persist_provider_config(profile_home: &Path) -> Result<(), ProviderError> {
    let config_path = profile_home.join("config.toml");
    match fs::symlink_metadata(&config_path) {
        Ok(metadata) => {
            if !metadata.file_type().is_file()
                || metadata.uid() != unsafe { libc::geteuid() }
                || metadata.nlink() != 1
                || metadata.mode() & 0o077 != 0
            {
                return Err(ProviderError::ProfileHomeUnavailable);
            }
            let existing =
                fs::read(&config_path).map_err(|_| ProviderError::ProfileHomeUnavailable)?;
            if existing != PROVIDER_CONFIG.as_bytes() {
                return Err(ProviderError::ProfileHomeUnavailable);
            }
        }
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&config_path)
                .map_err(|_| ProviderError::ProfileHomeUnavailable)?;
            file.write_all(PROVIDER_CONFIG.as_bytes())
                .and_then(|_| file.sync_all())
                .map_err(|_| ProviderError::ProfileHomeUnavailable)?;
        }
        Err(_) => return Err(ProviderError::ProfileHomeUnavailable),
    }
    Ok(())
}

fn is_empty(path: &Path) -> Result<bool, ProviderError> {
    let entries = fs::read_dir(path).map_err(|_| ProviderError::RoleWorkdirUnavailable)?;
    Ok(entries.count() == 0)
}

fn overlaps(left: &Path, right: &Path) -> bool {
    left.starts_with(right) || right.starts_with(left)
}

fn directory_error(profile_home: bool) -> ProviderError {
    if profile_home {
        ProviderError::ProfileHomeUnavailable
    } else {
        ProviderError::RoleWorkdirUnavailable
    }
}

fn path_arg(path: &Path) -> Result<&str, ProviderError> {
    path.to_str().ok_or(ProviderError::InvalidLaunch)
}

fn sha256_file(path: &Path) -> Result<String, ProviderError> {
    crate::verification::checkpoint()?;
    if let Some(digest) = crate::verification::cached_hash(path) {
        return Ok(digest);
    }
    let mut file = crate::verification::open_proof_file(path)?;
    let started = std::time::Instant::now();
    let mut count = 0u64;
    let mut hasher = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        crate::verification::checkpoint()?;
        let read = file
            .read(&mut buffer)
            .map_err(|_| ProviderError::ArtifactVerification)?;
        if read == 0 {
            break;
        }
        count += read as u64;
        hasher.update(&buffer[..read]);
    }
    let digest: String = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    crate::verification::record_hash(count, started.elapsed());
    crate::verification::remember_hash(path, &digest);
    Ok(digest)
}

#[cfg(test)]
mod artifact_identity_tests {
    use super::*;

    fn manifest(version: u32) -> BuildManifest {
        BuildManifest {
            schema_version: version,
            provider: "codex-acp".into(),
            package: CODEX_ACP_PACKAGE.into(),
            package_version: CODEX_ACP_VERSION.into(),
            package_integrity: CODEX_ACP_NPM_INTEGRITY.into(),
            source_commit: CODEX_ACP_GIT_COMMIT.into(),
            bundled_codex_package: CODEX_BINARY_PACKAGE.into(),
            bundled_codex_version: CODEX_BINARY_VERSION.into(),
            bundled_codex_integrity: CODEX_BINARY_NPM_INTEGRITY.into(),
            codex_platform_package: "shape-control".into(),
            codex_platform_version: "shape-control".into(),
            codex_platform_integrity: "shape-control".into(),
            codex_target_triple: "aarch64-apple-darwin".into(),
            target: "darwin-arm64".into(),
            upstream_artifact_sha256: "a".repeat(64),
            adapter_patch_id: "shape-control".into(),
            artifact_sha256: "b".repeat(64),
            codex_executable_sha256: "c".repeat(64),
            ripgrep_sha256: "d".repeat(64),
            public_ca_source_url: PUBLIC_CA_SOURCE_URL.into(),
            public_ca_sha256: PUBLIC_CA_SHA256.into(),
            codex_source_commit: (version == 5).then(|| CODEX_SOURCE_COMMIT.into()),
            codex_source_sha256: (version == 5).then(|| CODEX_SOURCE_SHA256.into()),
            codex_source_patch_id: (version == 5).then(|| CODEX_SOURCE_PATCH_ID.into()),
            codex_source_patch_sha256: (version == 5).then(|| CODEX_SOURCE_PATCH_SHA256.into()),
            codex_source_lock_sha256: (version == 5).then(|| CODEX_SOURCE_LOCK_SHA256.into()),
            status: "built_unadmitted".into(),
        }
    }

    #[test]
    fn artifact_set_identity_preserves_legacy_and_binds_the_complete_manifest() {
        let old = artifact_identity(&manifest(4)).unwrap();
        assert_eq!(old.acp_executable_sha256, "b".repeat(64));
        assert!(old.artifact_set_digest.is_none());

        let current = manifest(5);
        let identity = artifact_identity(&current).unwrap();
        assert_eq!(identity.acp_executable_sha256, old.acp_executable_sha256);
        assert_eq!(identity.artifact_set_digest.as_ref().unwrap().len(), 64);
        let value = serde_json::to_value(&current).unwrap();
        let reordered = serde_json::to_vec_pretty(&value).unwrap();
        let parsed = parse_build_manifest(&reordered).unwrap();
        assert_eq!(artifact_identity(&parsed).unwrap(), identity);

        for field in [
            "codex_executable_sha256",
            "ripgrep_sha256",
            "public_ca_sha256",
        ] {
            let mut changed = value.clone();
            changed[field] = serde_json::json!("e".repeat(64));
            let changed: BuildManifest = serde_json::from_value(changed).unwrap();
            let changed = artifact_identity(&changed).unwrap();
            assert_eq!(
                changed.acp_executable_sha256,
                identity.acp_executable_sha256
            );
            assert_ne!(changed.artifact_set_digest, identity.artifact_set_digest);
        }
    }
}
