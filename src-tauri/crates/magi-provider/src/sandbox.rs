use crate::{
    CODEX_ACP_GIT_COMMIT, CODEX_ACP_NPM_INTEGRITY, CODEX_ACP_PACKAGE, CODEX_ACP_VERSION,
    CODEX_BINARY_NPM_INTEGRITY, CODEX_BINARY_PACKAGE, CODEX_BINARY_VERSION, ProviderError,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::process::Command;

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
sleep_tool = false
shell_snapshot = false
"#;

const SEATBELT_PROFILE: &str = r#"(version 1)
(deny default)
(import "system.sb")
(allow file-read-metadata (path-ancestors (param "PROFILE_HOME")))
(allow file-read-metadata (path-ancestors (param "ROLE_WORKDIR")))
(allow file-read-metadata (path-ancestors (param "ARTIFACT_DIR")))
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
(allow process-exec (literal "/usr/bin/open"))
(allow network-outbound (remote ip "*:443"))
(allow network-outbound (literal "/private/var/run/mDNSResponder"))
"#;

#[derive(Clone)]
pub struct CodexAcpLaunch {
    pub executable: PathBuf,
    pub expected_executable_sha256: String,
    pub profile_home: PathBuf,
    pub role_workdir: PathBuf,
}

pub(crate) struct PreparedLaunch {
    pub executable: PathBuf,
    pub codex_executable: PathBuf,
    pub ripgrep_executable: PathBuf,
    pub artifact_dir: PathBuf,
    pub profile_home: PathBuf,
    pub role_workdir: PathBuf,
    pub temporary_dir: PathBuf,
}

#[derive(Deserialize)]
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
    artifact_sha256: String,
    codex_executable_sha256: String,
    ripgrep_sha256: String,
    status: String,
}

pub(crate) fn prepare(launch: &CodexAcpLaunch) -> Result<PreparedLaunch, ProviderError> {
    if !cfg!(target_os = "macos") {
        return Err(ProviderError::UnsupportedPlatform);
    }

    let expected_target = match std::env::consts::ARCH {
        "aarch64" => "darwin-arm64",
        "x86_64" => "darwin-x64",
        _ => return Err(ProviderError::UnsupportedPlatform),
    };
    if !launch.executable.is_absolute()
        || !launch.profile_home.is_absolute()
        || !launch.role_workdir.is_absolute()
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
    let role_workdir = canonical_private_dir(&launch.role_workdir, false)?;
    if !is_empty(&role_workdir)?
        || overlaps(&profile_home, &role_workdir)
        || overlaps(&artifact_dir, &profile_home)
        || overlaps(&artifact_dir, &role_workdir)
    {
        return Err(ProviderError::InvalidLaunch);
    }

    let (codex_executable, ripgrep_executable) = verify_artifact(
        &executable,
        &artifact_dir,
        expected_target,
        &launch.expected_executable_sha256,
    )?;

    let temporary_dir = profile_home.join("tmp");
    create_private_directory(&temporary_dir, ProviderError::ProfileHomeUnavailable)?;
    persist_provider_config(&profile_home)?;

    Ok(PreparedLaunch {
        executable,
        codex_executable,
        ripgrep_executable,
        artifact_dir,
        profile_home,
        role_workdir,
        temporary_dir,
    })
}

pub(crate) fn isolated_command(prepared: &PreparedLaunch) -> Result<Command, ProviderError> {
    if !Path::new("/usr/bin/sandbox-exec").is_file() {
        return Err(ProviderError::IsolationUnavailable);
    }

    let mut command = Command::new("/usr/bin/sandbox-exec");
    command
        .arg("-p")
        .arg(SEATBELT_PROFILE)
        .arg("-D")
        .arg(format!(
            "PROFILE_HOME={}",
            path_arg(&prepared.profile_home)?
        ))
        .arg("-D")
        .arg(format!(
            "ROLE_WORKDIR={}",
            path_arg(&prepared.role_workdir)?
        ))
        .arg("-D")
        .arg(format!(
            "ARTIFACT_DIR={}",
            path_arg(&prepared.artifact_dir)?
        ))
        .arg("-D")
        .arg(format!("EXECUTABLE={}", path_arg(&prepared.executable)?))
        .arg("-D")
        .arg(format!(
            "CODEX_EXECUTABLE={}",
            path_arg(&prepared.codex_executable)?
        ))
        .arg("-D")
        .arg(format!(
            "RIPGREP_EXECUTABLE={}",
            path_arg(&prepared.ripgrep_executable)?
        ))
        .arg(&prepared.executable)
        .current_dir(&prepared.role_workdir)
        .env_clear()
        .env("HOME", &prepared.profile_home)
        .env("CODEX_HOME", &prepared.profile_home)
        .env("CODEX_PATH", &prepared.codex_executable)
        .env("TMPDIR", &prepared.temporary_dir)
        .env("PATH", "/usr/bin:/bin")
        .env("LANG", "en_US.UTF-8")
        .env("LC_ALL", "en_US.UTF-8")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .process_group(0);
    Ok(command)
}

fn verify_artifact(
    executable: &Path,
    artifact_dir: &Path,
    target: &str,
    expected_digest: &str,
) -> Result<(PathBuf, PathBuf), ProviderError> {
    let executable_digest = sha256_file(executable)?;
    if !executable_digest.eq_ignore_ascii_case(expected_digest) {
        return Err(ProviderError::ArtifactVerification);
    }

    let manifest_path = artifact_dir.join("build-manifest.json");
    let manifest_metadata =
        fs::symlink_metadata(&manifest_path).map_err(|_| ProviderError::ArtifactVerification)?;
    if !manifest_metadata.file_type().is_file() || manifest_metadata.mode() & 0o022 != 0 {
        return Err(ProviderError::ArtifactVerification);
    }
    let bytes = fs::read(&manifest_path).map_err(|_| ProviderError::ArtifactVerification)?;
    let manifest: BuildManifest =
        serde_json::from_slice(&bytes).map_err(|_| ProviderError::ArtifactVerification)?;
    let (
        expected_platform_package,
        expected_platform_version,
        expected_platform_integrity,
        target_triple,
    ) = match target {
        "darwin-arm64" => (
            "@openai/codex-darwin-arm64",
            "0.156.1-darwin-arm64",
            "sha512-Jg6wbdV+wmMZczhwE74GSxOYEZlViKXn6KyCw/yfrz3PAKFD14xljuPopmdhWC1+8IKU2WdN5fdmXNPt2q4HPA==",
            "aarch64-apple-darwin",
        ),
        "darwin-x64" => (
            "@openai/codex-darwin-x64",
            "0.156.1-darwin-x64",
            "sha512-BVjqNOoltWrnNUrgMRepvDIIBmd4XY+ikAE4pYVHlrwiizNlEWQuCQOTrFMJKp54LFRG8jDTjLZyYQ1QrliuOg==",
            "x86_64-apple-darwin",
        ),
        _ => return Err(ProviderError::UnsupportedPlatform),
    };
    if manifest.schema_version != 2
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
    {
        return Err(ProviderError::ArtifactVerification);
    }
    Ok((codex_executable, ripgrep_executable))
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
    if !original_metadata.file_type().is_file() || original_metadata.mode() & 0o022 != 0 {
        return Err(ProviderError::ArtifactVerification);
    }
    let canonical = path
        .canonicalize()
        .map_err(|_| ProviderError::ArtifactVerification)?;
    let metadata =
        fs::symlink_metadata(&canonical).map_err(|_| ProviderError::ArtifactVerification)?;
    if !metadata.file_type().is_file() || metadata.mode() & 0o022 != 0 || metadata.nlink() != 1 {
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
    let mut file = File::open(path).map_err(|_| ProviderError::ArtifactVerification)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|_| ProviderError::ArtifactVerification)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}
