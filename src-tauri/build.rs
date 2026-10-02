use extraction_authority::{Identity, identity};
use std::{
    fs,
    fs::OpenOptions,
    io,
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};
const UNAVAILABLE: &str = "The verified build input is unavailable.";

#[path = "src/extraction_resource_authority.rs"]
mod extraction_authority;

pub(crate) struct HeldBuildInput {
    path: PathBuf,
    file: fs::File,
    expected: Identity,
    bytes: Vec<u8>,
    ancestors: Vec<(PathBuf, fs::File, Identity)>,
}
impl HeldBuildInput {
    pub(crate) fn open(path: &Path, limit: u64) -> Result<Self, String> {
        let request = magi_provider::VerificationRequest::until(
            std::time::Instant::now() + std::time::Duration::from_secs(60),
        );
        Self::open_with_request(path, limit, &request)
    }
    pub(crate) fn open_with_request(
        path: &Path,
        limit: u64,
        request: &magi_provider::VerificationRequest,
    ) -> Result<Self, String> {
        request.check().map_err(|_| UNAVAILABLE)?;
        if !path.is_absolute()
            || path
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(UNAVAILABLE.into());
        }
        let before = fs::symlink_metadata(path).map_err(|_| UNAVAILABLE)?;
        if !before.is_file()
            || before.file_type().is_symlink()
            || before.nlink() != 1
            || before.uid() != unsafe { libc::geteuid() }
            || before.mode() & 0o022 != 0
            || before.len() > limit
        {
            return Err(UNAVAILABLE.into());
        }
        let mut file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(path)
            .map_err(|_| UNAVAILABLE)?;
        let expected = identity(&before);
        if identity(&file.metadata().map_err(|_| UNAVAILABLE)?) != expected {
            return Err(UNAVAILABLE.into());
        }
        let mut bytes = Vec::new();
        let mut chunk = [0u8; 64 * 1024];
        loop {
            request.check().map_err(|_| UNAVAILABLE)?;
            let count = file.read(&mut chunk).map_err(|_| UNAVAILABLE)?;
            if count == 0 {
                break;
            }
            if bytes.len() as u64 + count as u64 > limit {
                return Err(UNAVAILABLE.into());
            }
            bytes.extend_from_slice(&chunk[..count]);
        }
        request.check().map_err(|_| UNAVAILABLE)?;
        let mut ancestors = Vec::new();
        for parent in path.parent().ok_or(UNAVAILABLE)?.ancestors() {
            let (held, expected) = hold_build_ancestor(parent, || {})?;
            ancestors.push((parent.to_owned(), held, expected));
        }
        let input = Self {
            path: path.to_owned(),
            file,
            expected,
            bytes,
            ancestors,
        };
        input.check()?;
        Ok(input)
    }
    pub(crate) fn directory(path: &Path) -> Result<Self, String> {
        let before = fs::symlink_metadata(path).map_err(|_| UNAVAILABLE)?;
        if !before.is_dir()
            || before.file_type().is_symlink()
            || before.uid() != unsafe { libc::geteuid() }
            || before.mode() & 0o022 != 0
        {
            return Err(UNAVAILABLE.into());
        }
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_DIRECTORY | libc::O_CLOEXEC)
            .open(path)
            .map_err(|_| UNAVAILABLE)?;
        let input = Self {
            path: path.to_owned(),
            file,
            expected: identity(&before),
            bytes: Vec::new(),
            ancestors: Vec::new(),
        };
        input.check()?;
        Ok(input)
    }
    pub(crate) fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub(crate) fn check(&self) -> Result<(), String> {
        if identity(&self.file.metadata().map_err(|_| UNAVAILABLE)?) != self.expected
            || identity(&fs::symlink_metadata(&self.path).map_err(|_| UNAVAILABLE)?)
                != self.expected
        {
            return Err(UNAVAILABLE.into());
        }
        for (path, file, expected) in &self.ancestors {
            for metadata in [
                file.metadata().map_err(|_| UNAVAILABLE)?,
                fs::symlink_metadata(path).map_err(|_| UNAVAILABLE)?,
            ] {
                let actual = identity(&metadata);
                if (actual.0, actual.1, actual.7, actual.8)
                    != (expected.0, expected.1, expected.7, expected.8)
                {
                    return Err(UNAVAILABLE.into());
                }
            }
        }
        Ok(())
    }
}
fn hold_build_ancestor(
    path: &Path,
    before_open: impl FnOnce(),
) -> Result<(fs::File, Identity), String> {
    let metadata = fs::symlink_metadata(path).map_err(|_| UNAVAILABLE)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(UNAVAILABLE.into());
    }
    let expected = identity(&metadata);
    before_open();
    let held = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_DIRECTORY | libc::O_CLOEXEC)
        .open(path)
        .map_err(|_| UNAVAILABLE)?;
    let actual = identity(&held.metadata().map_err(|_| UNAVAILABLE)?);
    if (actual.0, actual.1, actual.7, actual.8) != (expected.0, expected.1, expected.7, expected.8)
    {
        return Err(UNAVAILABLE.into());
    }
    Ok((held, expected))
}

#[derive(Debug, PartialEq, Eq)]
enum BuildPurpose {
    Verify,
    Publish,
}

impl BuildPurpose {
    fn parse(value: Option<&str>) -> io::Result<Self> {
        match value {
            None | Some("verify") => Ok(Self::Verify),
            Some("publish-quiescent") => Ok(Self::Publish),
            _ => Err(io::Error::other("Unknown resource build purpose")),
        }
    }
}

#[cfg(not(test))]
fn main() -> io::Result<()> {
    let out_dir = std::env::var_os("OUT_DIR")
        .ok_or_else(|| io::Error::other("Cargo build output directory is required"))?;
    println!("cargo:rerun-if-env-changed=MAGI_RESOURCE_BUILD_PURPOSE");
    let purpose = std::env::var_os("MAGI_RESOURCE_BUILD_PURPOSE")
        .map(|value| {
            value
                .into_string()
                .map_err(|_| io::Error::other("Invalid resource build purpose"))
        })
        .transpose()?;
    match BuildPurpose::parse(purpose.as_deref())? {
        BuildPurpose::Verify => verify_existing_resources(Path::new(&out_dir)),
        BuildPurpose::Publish => {
            require_publication_authority()?;
            prepare_extraction_resources(Path::new(&out_dir))?;
            publish_fresh_resources(Path::new(&out_dir), || {
                tauri_build::try_build(tauri_build::Attributes::default())
                    .map_err(|error| io::Error::other(error.to_string()))
            })
        }
    }
}

fn validate_configuration_override(value: Option<&std::ffi::OsStr>) -> io::Result<()> {
    if value.is_some() {
        return Err(io::Error::other(
            "Ambient runtime configuration cannot alter verification resources",
        ));
    }
    Ok(())
}

fn require_publication_authority() -> io::Result<()> {
    Err(io::Error::other(
        "A settled exclusive resource publication authority is required",
    ))
}

fn verify_existing_resources(out_dir: &Path) -> io::Result<()> {
    let manifest = Path::new(
        &std::env::var_os("CARGO_MANIFEST_DIR")
            .ok_or_else(|| io::Error::other("Manifest directory is unavailable"))?,
    )
    .canonicalize()?;
    verify_existing_resources_with(
        out_dir,
        &manifest,
        std::env::var_os("TAURI_CONFIG").as_deref(),
        |path| {
            tauri_build::try_build(tauri_build::Attributes::default().config_path(path))
                .map_err(|_| io::Error::other("Read-only Tauri configuration generation failed"))
        },
    )
}

fn verify_existing_resources_with(
    out_dir: &Path,
    manifest: &Path,
    configuration_override: Option<&std::ffi::OsStr>,
    codegen: impl FnOnce(&Path) -> io::Result<()>,
) -> io::Result<()> {
    validate_configuration_override(configuration_override)?;
    let out_dir = out_dir.canonicalize()?;
    let target = out_dir
        .ancestors()
        .nth(3)
        .ok_or_else(|| io::Error::other("Cargo target directory is unavailable"))?;
    let helper_candidate = manifest
        .parent()
        .ok_or_else(|| io::Error::other("Resource input root missing"))?
        .join(".local/native-build/extraction");
    verify_existing_authorities(
        &out_dir,
        manifest,
        target,
        target,
        &helper_candidate,
        codegen,
    )
}

struct ReadOnlyExtractionAuthority {
    resource: extraction_authority::VerifiedExtractionResource,
    source: HeldBuildInput,
    generation: String,
}
impl magi_provider::resource_custody::HeldExtractionAuthority for ReadOnlyExtractionAuthority {
    fn validate_held(&self) -> Result<(), magi_provider::ProviderError> {
        self.resource
            .check()
            .map_err(|_| magi_provider::ProviderError::ArtifactVerification)?;
        self.source
            .check()
            .map_err(|_| magi_provider::ProviderError::ArtifactVerification)?;
        extraction_authority::validate_source_binding(&self.resource, self.source.bytes())
            .map_err(|_| magi_provider::ProviderError::ArtifactVerification)
    }
    fn executable(&self) -> &Path {
        self.resource.path()
    }
    fn generation_digest(&self) -> &str {
        &self.generation
    }
}

struct OwnedCodegenConfiguration {
    directory: PathBuf,
    identity: (u64, u64),
    file: Option<(u64, u64)>,
    removed: bool,
}
impl OwnedCodegenConfiguration {
    fn create(directory: PathBuf) -> io::Result<Self> {
        fs::create_dir(&directory)?;
        let metadata = fs::symlink_metadata(&directory)?;
        Ok(Self {
            directory,
            identity: (metadata.dev(), metadata.ino()),
            file: None,
            removed: false,
        })
    }
    fn cleanup(&mut self) -> io::Result<()> {
        if self.removed {
            return Ok(());
        }
        let metadata = fs::symlink_metadata(&self.directory)?;
        if metadata.file_type().is_symlink()
            || !metadata.is_dir()
            || metadata.uid() != unsafe { libc::geteuid() }
            || (metadata.dev(), metadata.ino()) != self.identity
        {
            return Err(io::Error::other("Code generation cleanup identity changed"));
        }
        let entries = fs::read_dir(&self.directory)?.collect::<io::Result<Vec<_>>>()?;
        if entries.len() > usize::from(self.file.is_some())
            || entries
                .iter()
                .any(|entry| entry.file_name() != "tauri.conf.json")
        {
            return Err(io::Error::other(
                "Code generation cleanup contains unknown files",
            ));
        }
        let path = self.directory.join("tauri.conf.json");
        if let Some(expected) = self.file {
            let metadata = fs::symlink_metadata(&path)?;
            if !metadata.is_file()
                || metadata.file_type().is_symlink()
                || metadata.uid() != unsafe { libc::geteuid() }
                || (metadata.dev(), metadata.ino()) != expected
            {
                return Err(io::Error::other("Code generation cleanup file changed"));
            }
        }
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&self.directory, fs::Permissions::from_mode(0o700))?;
        if self.file.is_some() {
            fs::remove_file(path)?;
        }
        fs::remove_dir(&self.directory)?;
        self.removed = true;
        Ok(())
    }
}
impl Drop for OwnedCodegenConfiguration {
    fn drop(&mut self) {
        if self.cleanup().is_err() {
            eprintln!("Owned code generation configuration cleanup remains unresolved");
        }
    }
}

fn verify_existing_authorities(
    out_dir: &Path,
    manifest: &Path,
    target: &Path,
    helper_root: &Path,
    helper_candidate: &Path,
    codegen: impl FnOnce(&Path) -> io::Result<()>,
) -> io::Result<()> {
    use magi_provider::VerificationRequest;
    use std::time::{Duration, Instant};
    let started = Instant::now();
    let executable = target.join("provider/codex-acp/darwin-arm64/codex-acp");
    let request = VerificationRequest::until(Instant::now() + Duration::from_secs(60));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let release_inputs = verify_release_inputs(manifest, target, &request)?;
    let candidate_inputs = verify_configured_candidate_inputs(manifest, target, &request)?;
    let helper = extraction_authority::verify_resource(helper_root, |path| {
        extraction_authority::verify_signature_with_request(path, request.clone())
    })
    .map_err(|_| io::Error::other("Existing extraction authority verification failed"))?;
    let source = HeldBuildInput::open_with_request(
        &manifest.join("native/extract.swift"),
        1024 * 1024,
        &request,
    )
    .map_err(|_| io::Error::other("Extraction input authority is unavailable"))?;
    extraction_authority::validate_source_binding(&helper, source.bytes())
        .map_err(|_| io::Error::other("Extraction source authority is missing or changed"))?;
    let helper_inputs = verify_configured_helper_inputs(
        helper_candidate,
        helper
            .path()
            .parent()
            .ok_or_else(|| io::Error::other("Extraction authority directory unavailable"))?,
        &request,
    )?;
    let helper = std::sync::Arc::new(ReadOnlyExtractionAuthority {
        generation: magi_domain::Digest::from_bytes(helper.as_ref()).to_string(),
        resource: helper,
        source,
    });
    let proof_started = Instant::now();
    let provider = runtime
        .block_on(magi_provider::verify_readonly_installation(
            target.to_path_buf(),
            executable,
            helper.clone(),
            request.clone(),
        ))
        .map_err(|error| {
            let settlement = request.settlement();
            let category = match error {
                magi_provider::ProviderError::Timeout => 1,
                magi_provider::ProviderError::Cancelled => 2,
                magi_provider::ProviderError::ArtifactVerification => 3,
                magi_provider::ProviderError::UnsupportedPlatform => 4,
                _ => 0,
            };
            io::Error::other(format!(
                "Read-only installation proof rejected: category={category}, elapsed_ms={}, proof_ms={}, queued={}, workers={}, provider={}, unresolved={}",
                started.elapsed().as_millis(),
                proof_started.elapsed().as_millis(),
                settlement.queued_verifications,
                settlement.verification_workers,
                settlement.provider_operations,
                settlement.unresolved_cleanup
            ))
        })?;
    let config_path = manifest.join("tauri.conf.json");
    let config_input = HeldBuildInput::open_with_request(&config_path, 1024 * 1024, &request)
        .map_err(|_| io::Error::other("Configuration input authority is unavailable"))?;
    let config_bytes = config_input.bytes();
    let mut config: serde_json::Value = serde_json::from_slice(config_bytes)?;
    validate_resource_configuration(&config)?;
    config["bundle"]["resources"] = serde_json::json!([]);
    config["bundle"]["externalBin"] = serde_json::json!([]);
    use std::{io::Write, os::unix::fs::PermissionsExt};
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(io::Error::other)?
        .as_nanos();
    let configuration = out_dir.join(format!(
        "verified-configuration-{}-{nonce}",
        std::process::id()
    ));
    let mut cleanup = OwnedCodegenConfiguration::create(configuration.clone())?;
    let staged = configuration.join("tauri.conf.json");
    let mut staged_file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&staged)?;
    let staged_metadata = staged_file.metadata()?;
    cleanup.file = Some((staged_metadata.dev(), staged_metadata.ino()));
    staged_file.write_all(&serde_json::to_vec(&config)?)?;
    staged_file.sync_all()?;
    fs::set_permissions(&staged, fs::Permissions::from_mode(0o444))?;
    fs::set_permissions(&configuration, fs::Permissions::from_mode(0o555))?;
    let staged_input = HeldBuildInput::open_with_request(&staged, 1024 * 1024, &request)
        .map_err(|_| io::Error::other("Code generation configuration authority unavailable"))?;
    let staged_directory = HeldBuildInput::directory(&configuration)
        .map_err(|_| io::Error::other("Code generation configuration directory unavailable"))?;
    provider
        .validate_current()
        .map_err(|_| io::Error::other("Provider authority changed"))?;
    staged_input
        .check()
        .map_err(|_| io::Error::other("Code generation configuration changed"))?;
    staged_directory
        .check()
        .map_err(|_| io::Error::other("Code generation configuration directory changed"))?;
    codegen(&staged)?;
    staged_input
        .check()
        .map_err(|_| io::Error::other("Code generation configuration changed"))?;
    staged_directory
        .check()
        .map_err(|_| io::Error::other("Code generation configuration directory changed"))?;
    provider
        .validate_current()
        .map_err(|_| io::Error::other("Provider authority changed"))?;
    magi_provider::resource_custody::HeldExtractionAuthority::validate_held(helper.as_ref())
        .map_err(|_| io::Error::other("Extraction authority changed"))?;
    for input in release_inputs
        .iter()
        .chain(candidate_inputs.iter())
        .chain(helper_inputs.iter())
    {
        input
            .check()
            .map_err(|_| io::Error::other("Provider release input changed"))?;
    }
    helper
        .source
        .check()
        .map_err(|_| io::Error::other("Extraction input changed"))?;
    config_input
        .check()
        .map_err(|_| io::Error::other("Configuration input changed"))?;
    extraction_authority::validate_source_binding(&helper.resource, helper.source.bytes())
        .map_err(|_| io::Error::other("Extraction input changed"))?;
    cleanup.cleanup()?;
    Ok(())
}

fn verify_configured_helper_inputs(
    candidate: &Path,
    admitted: &Path,
    request: &magi_provider::VerificationRequest,
) -> io::Result<Vec<HeldBuildInput>> {
    let expected = std::collections::BTreeSet::from([
        std::ffi::OsString::from("magi-extract"),
        std::ffi::OsString::from("magi-extract.sha256"),
    ]);
    let mut held = Vec::new();
    for directory in [candidate, admitted] {
        held.push(
            HeldBuildInput::directory(directory)
                .map_err(|_| io::Error::other("Configured helper directory unavailable"))?,
        );
        let actual = fs::read_dir(directory)?
            .map(|entry| entry.map(|entry| entry.file_name()))
            .collect::<io::Result<std::collections::BTreeSet<_>>>()?;
        if actual != expected {
            return Err(io::Error::other("Configured helper resource set differs"));
        }
    }
    for name in expected {
        let input =
            HeldBuildInput::open_with_request(&candidate.join(&name), 32 * 1024 * 1024, request)
                .map_err(|_| io::Error::other("Configured helper input unavailable"))?;
        let output =
            HeldBuildInput::open_with_request(&admitted.join(&name), 32 * 1024 * 1024, request)
                .map_err(|_| io::Error::other("Admitted helper input unavailable"))?;
        if input.bytes() != output.bytes() {
            return Err(io::Error::other(
                "Configured helper differs from verified generation",
            ));
        }
        held.push(input);
        held.push(output);
    }
    Ok(held)
}

fn verify_configured_candidate_inputs(
    manifest: &Path,
    target: &Path,
    request: &magi_provider::VerificationRequest,
) -> io::Result<Vec<HeldBuildInput>> {
    fn visit(
        path: &Path,
        relative: &Path,
        files: &mut std::collections::BTreeSet<std::path::PathBuf>,
        directories: &mut Vec<HeldBuildInput>,
    ) -> io::Result<()> {
        use std::os::unix::fs::MetadataExt;
        let metadata = fs::symlink_metadata(path)?;
        if metadata.file_type().is_symlink()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o022 != 0
        {
            return Err(io::Error::other("Untrusted configured resource input"));
        }
        if files.len() + directories.len() >= 256 || relative.components().count() > 16 {
            return Err(io::Error::other(
                "Configured resource input exceeds bounded tree",
            ));
        }
        if metadata.is_file() {
            files.insert(relative.to_owned());
        } else if metadata.is_dir() {
            directories.push(
                HeldBuildInput::directory(path)
                    .map_err(|_| io::Error::other("Configured resource directory unavailable"))?,
            );
            for entry in fs::read_dir(path)? {
                let entry = entry?;
                visit(
                    &entry.path(),
                    &relative.join(entry.file_name()),
                    files,
                    directories,
                )?;
            }
        } else {
            return Err(io::Error::other("Nonregular configured resource input"));
        }
        Ok(())
    }
    let repository = manifest
        .parent()
        .ok_or_else(|| io::Error::other("Resource input root missing"))?;
    let source = repository.join(
        ".local/provider-build/codex-http-ca-preserve-backend-v1-package/codex-acp-tauri-resources",
    );
    let admitted = target.join("provider/codex-acp");
    let mut source_files = std::collections::BTreeSet::new();
    let mut admitted_files = std::collections::BTreeSet::new();
    let mut held = Vec::new();
    visit(&source, Path::new(""), &mut source_files, &mut held)?;
    visit(&admitted, Path::new(""), &mut admitted_files, &mut held)?;
    if source_files != admitted_files || source_files.len() > 256 {
        return Err(io::Error::other("Configured provider resource set differs"));
    }
    let mut total_bytes = 0u64;
    for relative in source_files {
        total_bytes = total_bytes
            .checked_add(fs::symlink_metadata(source.join(&relative))?.len())
            .filter(|value| *value <= 512 * 1024 * 1024)
            .ok_or_else(|| io::Error::other("Configured resource bytes exceed bounded input"))?;
        let input =
            HeldBuildInput::open_with_request(&source.join(&relative), 256 * 1024 * 1024, request)
                .map_err(|_| io::Error::other("Configured provider input unavailable"))?;
        let output = HeldBuildInput::open_with_request(
            &admitted.join(&relative),
            256 * 1024 * 1024,
            request,
        )
        .map_err(|_| io::Error::other("Admitted provider input unavailable"))?;
        let equal_length = input.bytes().len() == output.bytes().len();
        let mut equal_bytes = equal_length;
        if equal_length {
            for (configured, installed) in input
                .bytes()
                .chunks(64 * 1024)
                .zip(output.bytes().chunks(64 * 1024))
            {
                request
                    .check()
                    .map_err(|_| io::Error::other("Configured provider comparison expired"))?;
                if configured != installed {
                    equal_bytes = false;
                    break;
                }
            }
        }
        request
            .check()
            .map_err(|_| io::Error::other("Configured provider comparison expired"))?;
        if !equal_bytes {
            return Err(io::Error::other(
                "Configured provider input differs from verified generation",
            ));
        }
        held.push(input);
        held.push(output);
    }
    Ok(held)
}

fn verify_release_inputs(
    manifest: &Path,
    target: &Path,
    request: &magi_provider::VerificationRequest,
) -> io::Result<Vec<HeldBuildInput>> {
    let repository = manifest
        .parent()
        .ok_or_else(|| io::Error::other("Release input root missing"))?;
    let authority = HeldBuildInput::open_with_request(
        &target.join("provider/codex-acp/darwin-arm64/build-manifest.json"),
        65_536,
        request,
    )
    .map_err(|_| io::Error::other("Provider manifest input unavailable"))?;
    let signed: serde_json::Value = serde_json::from_slice(authority.bytes())?;
    if signed.get("schema_version") != Some(&serde_json::json!(5)) {
        return Err(io::Error::other(
            "Complete provider release provenance is required",
        ));
    }
    let selected = [
        (
            "scripts/patches/codex-http-ca-backend.patch",
            "codex_source_patch_sha256",
            1024 * 1024,
        ),
        (
            ".local/provider-build/codex-http-ca-preserve-backend-v1/source.tar.gz",
            "codex_source_sha256",
            100 * 1024 * 1024,
        ),
        (
            ".local/provider-build/codex-http-ca-preserve-backend-v1/source/codex-rs/Cargo.lock",
            "codex_source_lock_sha256",
            4 * 1024 * 1024,
        ),
    ];
    let mut held = vec![authority];
    for (relative, key, limit) in selected {
        let input = HeldBuildInput::open_with_request(&repository.join(relative), limit, request)
            .map_err(|_| io::Error::other("Provider release input unavailable"))?;
        let expected = signed
            .get(key)
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| io::Error::other("Provider release input binding missing"))?;
        if magi_domain::Digest::from_bytes(input.bytes()).as_str() != expected {
            return Err(io::Error::other("Provider release input binding changed"));
        }
        held.push(input);
    }
    Ok(held)
}

fn validate_resource_configuration(config: &serde_json::Value) -> io::Result<()> {
    let expected = serde_json::json!({
        "../.local/provider-build/codex-http-ca-preserve-backend-v1-package/codex-acp-tauri-resources/": "provider/codex-acp/",
        "../.local/native-build/extraction/": "extraction/"
    });
    if config.pointer("/bundle/resources") != Some(&expected)
        || config
            .pointer("/bundle/externalBin")
            .is_some_and(|value| value != &serde_json::json!([]))
    {
        return Err(io::Error::other("Unverified configured resource input"));
    }
    Ok(())
}

fn select_developer_identity(output: &str) -> io::Result<Option<String>> {
    let mut candidates = output.lines().filter_map(|line| {
        let (_, quoted) = line.split_once('"')?;
        let (identity, _) = quoted.split_once('"')?;
        (identity.starts_with("Developer ID Application: ") && identity.ends_with(" (95B7J2U49K)"))
            .then(|| identity.to_owned())
    });
    let selected = candidates.next();
    if candidates.next().is_some() {
        return Err(io::Error::other("Expected signing identity is ambiguous"));
    }
    Ok(selected)
}

#[cfg(not(test))]
fn signing_identity() -> io::Result<Option<std::ffi::OsString>> {
    if let Some(identity) = std::env::var_os("APPLE_SIGNING_IDENTITY") {
        return Ok(Some(identity));
    }
    let output = std::process::Command::new("/usr/bin/security")
        .env_clear()
        .args(["find-identity", "-v", "-p", "codesigning"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()?;
    if !output.status.success() || output.stdout.len() > 64 * 1024 {
        return Err(io::Error::other("Signing identity discovery failed"));
    }
    let identities = std::str::from_utf8(&output.stdout)
        .map_err(|_| io::Error::other("Signing identity discovery failed"))?;
    Ok(select_developer_identity(identities)?.map(Into::into))
}

#[cfg(not(test))]
fn prepare_extraction_resources(out_dir: &Path) -> io::Result<()> {
    use std::{
        os::unix::fs::PermissionsExt,
        process::{Command, Stdio},
        time::{SystemTime, UNIX_EPOCH},
    };
    let out_dir = out_dir.canonicalize()?;
    if out_dir.file_name().and_then(|name| name.to_str()) != Some("out")
        || out_dir
            .ancestors()
            .nth(2)
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            != Some("build")
    {
        return Err(io::Error::other(
            "Unexpected Cargo build output directory structure",
        ));
    }
    println!("cargo:rerun-if-changed=native/extract.swift");
    println!("cargo:rerun-if-env-changed=APPLE_SIGNING_IDENTITY");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return Err(io::Error::other(
            "Native extraction packaging requires macOS",
        ));
    }
    let architecture = match std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("aarch64") => "arm64",
        Ok("x86_64") => "x86_64",
        _ => return Err(io::Error::other("Unsupported extraction architecture")),
    };
    let manifest = Path::new(
        &std::env::var_os("CARGO_MANIFEST_DIR")
            .ok_or_else(|| io::Error::other("Manifest directory missing"))?,
    )
    .canonicalize()?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(io::Error::other)?
        .as_nanos();
    let output = out_dir.join(format!("magi-extract-{}-{nonce}", std::process::id()));
    let source = HeldBuildInput::open(&manifest.join("native/extract.swift"), 1024 * 1024)
        .map_err(|_| io::Error::other("Extraction compilation input unavailable"))?;
    let snapshot = out_dir.join(format!("extraction-input-{nonce}"));
    fs::create_dir(&snapshot)?;
    let compiler_source = snapshot.join("extract.swift");
    let source_binding = snapshot.join("source.sha256");
    let source_digest = magi_domain::Digest::from_bytes(source.bytes());
    for (path, bytes) in [
        (&compiler_source, source.bytes()),
        (&source_binding, source_digest.as_str().as_bytes()),
    ] {
        use std::io::Write;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o444))?;
    }
    fs::set_permissions(&snapshot, fs::Permissions::from_mode(0o555))?;
    let captured = HeldBuildInput::open(&compiler_source, 1024 * 1024)
        .map_err(|_| io::Error::other("Captured extraction source unavailable"))?;
    let captured_binding = HeldBuildInput::open(&source_binding, 64)
        .map_err(|_| io::Error::other("Captured extraction source binding unavailable"))?;
    let captured_directory = HeldBuildInput::directory(&snapshot)
        .map_err(|_| io::Error::other("Captured extraction directory unavailable"))?;
    let mut compiler = Command::new("/usr/bin/xcrun");
    compiler
        .env_clear()
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .env("LANG", "en_US.UTF-8")
        .args([
            "swiftc",
            "-target",
            &format!("{architecture}-apple-macosx13.0"),
            "-O",
        ])
        .args([
            "-Xlinker",
            "-sectcreate",
            "-Xlinker",
            "__TEXT",
            "-Xlinker",
            "__magi_source",
            "-Xlinker",
        ])
        .arg(&source_binding)
        .arg(&compiler_source)
        .arg("-o")
        .arg(&output)
        .stdin(Stdio::null());
    if !compiler.status()?.success() {
        return Err(io::Error::other("Native extraction compilation failed"));
    }
    source
        .check()
        .map_err(|_| io::Error::other("Extraction source changed during compilation"))?;
    captured
        .check()
        .map_err(|_| io::Error::other("Captured extraction source changed during compilation"))?;
    captured_binding
        .check()
        .map_err(|_| io::Error::other("Captured extraction binding changed during compilation"))?;
    captured_directory.check().map_err(|_| {
        io::Error::other("Captured extraction directory changed during compilation")
    })?;
    if let Some(identity) = signing_identity()? {
        let status = Command::new("/usr/bin/codesign")
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .args(["--force", "--options", "runtime", "--timestamp", "--sign"])
            .arg(identity)
            .arg(&output)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        if !status.success() {
            return Err(io::Error::other("Native extraction signing failed"));
        }
    } else {
        println!(
            "cargo:warning=Extraction helper is unavailable at runtime until a verified Developer ID build is supplied"
        );
    }
    let hash = Command::new("/usr/bin/shasum")
        .env_clear()
        .args(["-a", "256"])
        .arg(&output)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()?;
    if !hash.status.success() {
        return Err(io::Error::other("Extraction digest failed"));
    }
    let hash = std::str::from_utf8(&hash.stdout)
        .map_err(|_| io::Error::other("Extraction digest failed"))?
        .split_whitespace()
        .next()
        .ok_or_else(|| io::Error::other("Extraction digest missing"))?;
    if hash.len() != 64
        || !hash
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(io::Error::other("Extraction digest invalid"));
    }
    let workspace = manifest
        .parent()
        .ok_or_else(|| io::Error::other("Workspace parent missing"))?;
    let local = workspace.join(".local");
    let parent = local.join("native-build");
    for directory in [&local, &parent] {
        validate_directory(directory)?;
        if !directory.exists() {
            fs::create_dir(directory)?;
        }
        validate_directory(directory)?;
    }
    let staged = parent.join(format!("extraction-stage-{}-{nonce}", std::process::id()));
    fs::create_dir(&staged)?;
    fs::set_permissions(&staged, fs::Permissions::from_mode(0o700))?;
    let executable = staged.join("magi-extract");
    fs::copy(&output, &executable)?;
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o555))?;
    fs::File::open(&executable)?.sync_all()?;
    let checksum = staged.join("magi-extract.sha256");
    fs::write(&checksum, format!("{hash}\n"))?;
    fs::set_permissions(&checksum, fs::Permissions::from_mode(0o444))?;
    fs::File::open(&checksum)?.sync_all()?;
    fs::File::open(&staged)?.sync_all()?;
    let destination = parent.join("extraction");
    validate_directory(&destination)?;
    let previous = parent.join(format!(
        "extraction-retained-{}-{nonce}",
        std::process::id()
    ));
    if destination.exists() {
        fs::rename(&destination, &previous)?;
    }
    if let Err(error) = fs::rename(&staged, &destination) {
        if previous.exists() {
            fs::rename(&previous, &destination)?;
        }
        return Err(error);
    }
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}

fn validate_directory(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() => Ok(()),
        Ok(_) => Err(io::Error::other(
            "Generated resource directory is not a regular directory",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn swap_directories(first: &Path, second: &Path) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        use std::{ffi::CString, os::unix::ffi::OsStrExt};
        unsafe extern "C" {
            fn renamex_np(
                from: *const std::ffi::c_char,
                to: *const std::ffi::c_char,
                flags: u32,
            ) -> std::ffi::c_int;
        }
        let first = CString::new(first.as_os_str().as_bytes()).map_err(io::Error::other)?;
        let second = CString::new(second.as_os_str().as_bytes()).map_err(io::Error::other)?;
        if unsafe { renamex_np(first.as_ptr(), second.as_ptr(), 2) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let temporary = second.with_extension("swap-retired");
        if temporary.exists() {
            return Err(io::Error::other("Resource swap path is occupied"));
        }
        fs::rename(first, &temporary)?;
        if let Err(error) = fs::rename(second, first) {
            fs::rename(&temporary, first)?;
            return Err(error);
        }
        if let Err(error) = fs::rename(&temporary, second) {
            fs::rename(first, second)?;
            fs::rename(&temporary, first)?;
            return Err(error);
        }
        Ok(())
    }
}

fn remove_retired_resources(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::MetadataExt;
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() {
        return fs::remove_file(path);
    }
    let owner = metadata.uid();
    fn writable(directory: &Path, owner: u32) -> io::Result<()> {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let metadata = fs::symlink_metadata(directory)?;
        if !metadata.is_dir() || metadata.uid() != owner {
            return Err(io::Error::other(
                "Retired resource directory authority is invalid",
            ));
        }
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
        for entry in fs::read_dir(directory)? {
            let path = entry?.path();
            if fs::symlink_metadata(&path)?.is_dir() {
                writable(&path, owner)?;
            }
        }
        Ok(())
    }
    writable(path, owner)?;
    fs::remove_dir_all(path)
}

fn retire_resources(destination: &Path, previous: &Path) -> io::Result<()> {
    retire_resources_using(destination, previous, |path| fs::remove_dir(path))
}

fn retire_resources_using(
    destination: &Path,
    previous: &Path,
    remove_placeholder: impl FnOnce(&Path) -> io::Result<()>,
) -> io::Result<()> {
    fs::create_dir(previous)?;
    if let Err(error) = swap_directories(destination, previous) {
        fs::remove_dir(previous)?;
        return Err(error);
    }
    if let Err(error) = remove_placeholder(destination) {
        swap_directories(destination, previous)?;
        fs::remove_dir(previous)?;
        return Err(error);
    }
    Ok(())
}

fn restore_previous(destination: &Path, previous: &Path) -> io::Result<()> {
    validate_directory(
        destination
            .parent()
            .ok_or_else(|| io::Error::other("Generated resource parent is unavailable"))?,
    )?;
    validate_directory(previous)?;
    if previous.exists() {
        match fs::symlink_metadata(destination) {
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => {
                fs::remove_file(destination)?;
                fs::create_dir(destination)?;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => fs::create_dir(destination)?,
            Err(error) => return Err(error),
        }
        swap_directories(destination, previous)?;
        remove_retired_resources(previous)?;
    } else if destination.exists() {
        remove_retired_resources(destination)?;
    }
    Ok(())
}

fn publish_fresh_resources(
    out_dir: &Path,
    build: impl FnOnce() -> io::Result<()>,
) -> io::Result<()> {
    let out_dir = out_dir.canonicalize()?;
    if out_dir.file_name().and_then(|name| name.to_str()) != Some("out")
        || out_dir
            .ancestors()
            .nth(2)
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            != Some("build")
    {
        return Err(io::Error::other(
            "Unexpected Cargo build output directory structure",
        ));
    }
    let target_dir = out_dir
        .ancestors()
        .nth(3)
        .ok_or_else(|| io::Error::other("Cargo target directory is unavailable"))?;
    let provider_dir = target_dir.join("provider");
    validate_directory(&provider_dir)?;
    let trees = [
        (
            provider_dir.join("codex-acp"),
            provider_dir.join("previous-codex-acp-resources"),
        ),
        (
            target_dir.join("extraction"),
            target_dir.join("previous-extraction-resources"),
        ),
    ];
    for (destination, previous) in &trees {
        validate_directory(destination)?;
        validate_directory(previous)?;
    }
    for (prepared, (destination, previous)) in trees.iter().enumerate() {
        let prepare = (|| {
            if previous.exists() {
                restore_previous(destination, previous)?;
            }
            if destination.exists() {
                retire_resources(destination, previous)?;
            }
            Ok(())
        })();
        if let Err(error) = prepare {
            for (destination, previous) in trees[..prepared].iter().rev() {
                restore_previous(destination, previous)?;
            }
            return Err(error);
        }
    }
    // All copied trust resources retire together before Tauri writes fresh inodes.
    let result = build().and_then(|()| {
        for (destination, _) in &trees {
            let metadata = fs::symlink_metadata(destination)?;
            if !metadata.file_type().is_dir() {
                return Err(io::Error::other(
                    "Published resources are not regular directories",
                ));
            }
            seal_generated_resources(destination)?;
        }
        Ok(())
    });
    match result {
        Ok(()) => {
            for (_, previous) in &trees {
                if previous.exists() {
                    remove_retired_resources(previous)?;
                }
            }
            Ok(())
        }
        Err(error) => {
            for (destination, previous) in trees.iter().rev() {
                restore_previous(destination, previous)?;
            }
            Err(error)
        }
    }
}

fn seal_generated_resources(directory: &Path) -> io::Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let owner = fs::symlink_metadata(directory)?.uid();
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.uid() != owner || metadata.file_type().is_symlink() {
            return Err(io::Error::other(
                "Generated trust resource authority is invalid",
            ));
        }
        if metadata.file_type().is_dir() {
            seal_generated_resources(&path)?;
        } else if metadata.file_type().is_file() && metadata.nlink() == 1 {
            let mode = if metadata.mode() & 0o111 != 0 {
                0o555
            } else {
                0o444
            };
            fs::set_permissions(&path, fs::Permissions::from_mode(mode))?;
        } else {
            return Err(io::Error::other(
                "Generated trust resource is not a single-link regular file",
            ));
        }
    }
    fs::set_permissions(directory, fs::Permissions::from_mode(0o555))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        os::unix::fs::{MetadataExt, PermissionsExt},
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let root = std::env::temp_dir().canonicalize().unwrap().join(format!(
                "resource-publication-{}-{nonce}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            fs::create_dir(&root).unwrap();
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
            fs::create_dir_all(root.join("target/debug/build/package/out")).unwrap();
            Self(root)
        }
        fn out(&self) -> PathBuf {
            self.0.join("target/debug/build/package/out")
        }
        fn destination(&self) -> PathBuf {
            self.0.join("target/debug/provider/codex-acp")
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = remove_retired_resources(&self.0);
        }
    }

    #[test]
    fn ancestor_sibling_churn_is_allowed_but_replacement_and_symlink_are_denied() {
        let fixture = Fixture::new();
        let parent = fixture.0.join("ancestor");
        fs::create_dir(&parent).unwrap();
        let mut changed_version = false;
        let (_, expected) = hold_build_ancestor(&parent, || {
            fs::create_dir(parent.join("unrelated-sibling")).unwrap();
            changed_version = true;
        })
        .unwrap();
        assert!(changed_version);
        assert_ne!(identity(&fs::metadata(&parent).unwrap()), expected);
        assert!(
            hold_build_ancestor(&parent, || {
                fs::rename(&parent, fixture.0.join("retired-ancestor")).unwrap();
                fs::create_dir(&parent).unwrap();
            })
            .is_err()
        );
        assert!(
            hold_build_ancestor(&parent, || {
                fs::remove_dir(&parent).unwrap();
                std::os::unix::fs::symlink(fixture.0.join("retired-ancestor"), &parent).unwrap();
            })
            .is_err()
        );
    }

    fn owned_codegen_fixture(path: PathBuf) -> OwnedCodegenConfiguration {
        let mut owned = OwnedCodegenConfiguration::create(path.clone()).unwrap();
        let file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path.join("tauri.conf.json"))
            .unwrap();
        let metadata = file.metadata().unwrap();
        owned.file = Some((metadata.dev(), metadata.ino()));
        fs::set_permissions(
            path.join("tauri.conf.json"),
            fs::Permissions::from_mode(0o444),
        )
        .unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o555)).unwrap();
        owned
    }

    #[test]
    fn owned_codegen_configuration_is_removed_on_success_and_error_unwind() {
        let fixture = Fixture::new();
        let success = fixture.0.join("success");
        let mut owned = owned_codegen_fixture(success.clone());
        owned.cleanup().unwrap();
        assert!(!success.exists());
        drop(owned);
        let error = fixture.0.join("error");
        let outcome: io::Result<()> = (|| {
            let _owned = owned_codegen_fixture(error.clone());
            Err(io::Error::other("injected code generation failure"))
        })();
        assert!(outcome.is_err());
        assert!(!error.exists());
    }

    #[test]
    fn owned_codegen_cleanup_never_removes_a_replacement_directory() {
        let fixture = Fixture::new();
        let path = fixture.0.join("configuration");
        let mut owned = owned_codegen_fixture(path.clone());
        let original = fixture.0.join("original");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        fs::rename(&path, &original).unwrap();
        fs::set_permissions(&original, fs::Permissions::from_mode(0o555)).unwrap();
        fs::create_dir(&path).unwrap();
        fs::write(path.join("foreign"), b"preserve foreign configuration").unwrap();
        assert!(owned.cleanup().is_err());
        drop(owned);
        assert_eq!(
            fs::read(path.join("foreign")).unwrap(),
            b"preserve foreign configuration"
        );
        assert!(original.join("tauri.conf.json").is_file());
    }

    #[test]
    fn release_inputs_reject_each_current_source_drift_without_repair() {
        let fixture = Fixture::new();
        let manifest = fixture.0.join("src-tauri");
        fs::create_dir(&manifest).unwrap();
        let target = fixture.0.join("target/debug");
        let selected = [
            (
                "scripts/patches/codex-http-ca-backend.patch",
                "codex_source_patch_sha256",
            ),
            (
                ".local/provider-build/codex-http-ca-preserve-backend-v1/source.tar.gz",
                "codex_source_sha256",
            ),
            (
                ".local/provider-build/codex-http-ca-preserve-backend-v1/source/codex-rs/Cargo.lock",
                "codex_source_lock_sha256",
            ),
        ];
        let mut authority = serde_json::json!({"schema_version":5});
        for (relative, key) in selected {
            let path = fixture.0.join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, b"captured release input").unwrap();
            authority[key] = serde_json::json!(
                magi_domain::Digest::from_bytes(b"captured release input").as_str()
            );
        }
        let authority_path = target.join("provider/codex-acp/darwin-arm64/build-manifest.json");
        fs::create_dir_all(authority_path.parent().unwrap()).unwrap();
        fs::write(&authority_path, serde_json::to_vec(&authority).unwrap()).unwrap();
        let request = magi_provider::VerificationRequest::until(
            std::time::Instant::now() + std::time::Duration::from_secs(5),
        );
        let held = verify_release_inputs(&manifest, &target, &request).unwrap();
        for (relative, _) in selected {
            let path = fixture.0.join(relative);
            fs::write(&path, b"changed release input").unwrap();
            assert!(verify_release_inputs(&manifest, &target, &request).is_err());
            assert!(held.iter().any(|input| input.check().is_err()));
            assert_eq!(fs::read(&path).unwrap(), b"changed release input");
            fs::write(path, b"captured release input").unwrap();
        }
    }

    #[test]
    fn configured_candidate_drift_and_added_entries_fail_without_mutation() {
        let fixture = Fixture::new();
        let manifest = fixture.0.join("src-tauri");
        fs::create_dir(&manifest).unwrap();
        let target = fixture.0.join("target/debug");
        let source = fixture.0.join(".local/provider-build/codex-http-ca-preserve-backend-v1-package/codex-acp-tauri-resources");
        let admitted = target.join("provider/codex-acp");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&admitted).unwrap();
        let approved = vec![b'a'; 2 * 64 * 1024 + 1];
        fs::write(source.join("artifact"), &approved).unwrap();
        fs::write(admitted.join("artifact"), &approved).unwrap();
        let request = magi_provider::VerificationRequest::until(
            std::time::Instant::now() + std::time::Duration::from_secs(5),
        );
        let held = verify_configured_candidate_inputs(&manifest, &target, &request).unwrap();
        let admitted_identity = identity(&fs::symlink_metadata(admitted.join("artifact")).unwrap());
        for at in [0, approved.len() - 1] {
            let mut changed = approved.clone();
            changed[at] = b'b';
            fs::write(source.join("artifact"), &changed).unwrap();
            assert!(verify_configured_candidate_inputs(&manifest, &target, &request).is_err());
        }
        assert!(held.iter().any(|input| input.check().is_err()));
        let mut longer = approved.clone();
        longer.push(b'a');
        fs::write(source.join("artifact"), longer).unwrap();
        assert!(verify_configured_candidate_inputs(&manifest, &target, &request).is_err());
        fs::write(source.join("artifact"), &approved).unwrap();
        let revoked = magi_provider::VerificationRequest::until(
            std::time::Instant::now() + std::time::Duration::from_secs(5),
        );
        revoked.revoke();
        assert!(verify_configured_candidate_inputs(&manifest, &target, &revoked).is_err());
        fs::write(source.join("extra"), b"unapproved entry").unwrap();
        assert!(verify_configured_candidate_inputs(&manifest, &target, &request).is_err());
        assert_eq!(
            identity(&fs::symlink_metadata(admitted.join("artifact")).unwrap()),
            admitted_identity
        );
        assert_eq!(fs::read(admitted.join("artifact")).unwrap(), approved);
    }

    #[test]
    fn held_build_input_rejects_replacement_and_preserves_captured_bytes_through_original_aba() {
        let fixture = Fixture::new();
        let source = fixture.0.join("source.swift");
        fs::write(&source, b"approved source").unwrap();
        let input = HeldBuildInput::open(&source, 64).unwrap();
        let replacement = fixture.0.join("replacement.swift");
        fs::write(&replacement, b"different source").unwrap();
        fs::rename(&replacement, &source).unwrap();
        assert!(input.check().is_err());
        fs::write(&source, b"approved source").unwrap();
        assert!(input.check().is_err());
        assert_eq!(input.bytes(), b"approved source");
        fs::remove_file(&source).unwrap();
        std::os::unix::fs::symlink(fixture.0.join("extraction/magi-extract"), &source).unwrap();
        assert!(HeldBuildInput::open(&source, 64).is_err());
    }

    #[test]
    #[ignore = "Requires an explicitly signed captured-source helper fixture and existing verified provider inputs; no resource publication."]
    fn signed_captured_helper_runs_actual_noncopying_tauri_codegen() {
        let helper = PathBuf::from(
            std::env::var_os("MAGI_TEST_SIGNED_HELPER_ROOT").expect("explicit helper fixture"),
        );
        let target = PathBuf::from(
            std::env::var_os("MAGI_TEST_PROVIDER_TARGET").expect("explicit provider target"),
        );
        let manifest =
            PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("explicit manifest"));
        let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("explicit fixture output"));
        validate_configuration_override(std::env::var_os("TAURI_CONFIG").as_deref()).unwrap();
        verify_existing_authorities(
            &out,
            &manifest,
            &target,
            &helper,
            &manifest
                .parent()
                .unwrap()
                .join(".local/native-build/extraction"),
            |configuration| {
                tauri_build::try_build(
                    tauri_build::Attributes::default().config_path(configuration),
                )
                .map_err(|_| io::Error::other("Actual noncopying code generation failed"))
            },
        )
        .unwrap();
    }

    #[test]
    #[ignore = "Requires an explicit existing published resource root; verification must reject missing signed helper provenance without repair."]
    fn existing_generation_without_signed_helper_source_fails_before_codegen() {
        let out = std::env::var_os("MAGI_TEST_BUILD_OUT_DIR")
            .expect("explicit existing output directory");
        let error = verify_existing_resources(Path::new(&out)).unwrap_err();
        assert_eq!(
            error.to_string(),
            "Extraction source authority is missing or changed"
        );
        assert!(!Path::new(&out).join("verified-configuration").exists());
    }

    #[test]
    fn configured_helper_candidate_checks_complete_inventory_and_held_versions() {
        let fixture = Fixture::new();
        let candidate = fixture.0.join("helper-candidate");
        let admitted = fixture.0.join("helper-admitted");
        for directory in [&candidate, &admitted] {
            fs::create_dir(directory).unwrap();
            fs::write(directory.join("magi-extract"), b"signed helper bytes").unwrap();
            fs::write(directory.join("magi-extract.sha256"), b"checksum").unwrap();
        }
        let request = magi_provider::VerificationRequest::until(
            std::time::Instant::now() + std::time::Duration::from_secs(5),
        );
        let held = verify_configured_helper_inputs(&candidate, &admitted, &request).unwrap();
        fs::write(candidate.join("magi-extract"), b"helper candidate drift").unwrap();
        assert!(verify_configured_helper_inputs(&candidate, &admitted, &request).is_err());
        assert!(held.iter().any(|input| input.check().is_err()));
        fs::write(candidate.join("magi-extract"), b"signed helper bytes").unwrap();
        fs::write(candidate.join("extra"), b"unapproved helper input").unwrap();
        assert!(verify_configured_helper_inputs(&candidate, &admitted, &request).is_err());
        assert_eq!(
            fs::read(admitted.join("magi-extract")).unwrap(),
            b"signed helper bytes"
        );
    }

    #[test]
    fn verification_purpose_rejects_unknown_resources_and_unguarded_publication() {
        assert_eq!(BuildPurpose::parse(None).unwrap(), BuildPurpose::Verify);
        assert_eq!(
            BuildPurpose::parse(Some("verify")).unwrap(),
            BuildPurpose::Verify
        );
        assert!(BuildPurpose::parse(Some("publsih")).is_err());
        assert!(require_publication_authority().is_err());
        let injected =
            std::ffi::OsStr::new(r#"{"bundle":{"resources":{"attacker":"provider/codex-acp/"}}}"#);
        assert!(
            verify_existing_resources_with(
                Path::new("missing-output"),
                Path::new("missing-manifest"),
                Some(injected),
                |_| panic!("override must reject before Tauri resource copying")
            )
            .is_err()
        );

        validate_configuration_override(None).unwrap();
        assert!(
            validate_configuration_override(Some(std::ffi::OsStr::new(
                r#"{"bundle":{"resources":{"attacker":"provider/codex-acp/"}}}"#
            )))
            .is_err()
        );
        assert!(validate_configuration_override(Some(std::ffi::OsStr::new(""))).is_err());

        let mut config = serde_json::json!({"bundle":{"resources":{
            "../.local/provider-build/codex-http-ca-preserve-backend-v1-package/codex-acp-tauri-resources/":"provider/codex-acp/",
            "../.local/native-build/extraction/":"extraction/"
        }}});
        validate_resource_configuration(&config).unwrap();
        config["bundle"]["externalBin"] = serde_json::json!(["unverified-sidecar"]);
        assert!(validate_resource_configuration(&config).is_err());
        config["bundle"]
            .as_object_mut()
            .unwrap()
            .remove("externalBin");
        config["bundle"]["resources"]["unverified"] = serde_json::json!("provider/codex-acp/");
        assert!(validate_resource_configuration(&config).is_err());
    }

    #[test]
    fn failed_placeholder_removal_restores_exact_read_only_published_generation() {
        let fixture = Fixture::new();
        let destination = fixture.destination();
        fs::create_dir_all(&destination).unwrap();
        fs::write(destination.join("authority"), b"prior authority").unwrap();
        seal_generated_resources(&destination).unwrap();
        let inode = fs::metadata(&destination).unwrap().ino();
        let previous = destination
            .parent()
            .unwrap()
            .join("previous-codex-acp-resources");
        assert!(
            retire_resources_using(&destination, &previous, |_| {
                Err(io::Error::other("Injected placeholder retirement failure"))
            })
            .is_err()
        );
        assert_eq!(fs::metadata(&destination).unwrap().ino(), inode);
        assert_eq!(fs::metadata(&destination).unwrap().mode() & 0o777, 0o555);
        assert_eq!(
            fs::read(destination.join("authority")).unwrap(),
            b"prior authority"
        );
        assert!(!previous.exists());
    }

    #[test]
    fn signing_discovery_requires_one_valid_expected_team_identity() {
        let expected = "Developer ID Application: Public Fixture (95B7J2U49K)";
        assert_eq!(
            select_developer_identity(&format!("1) ABC \"{expected}\"\n 1 valid identities found"))
                .unwrap(),
            Some(expected.into())
        );
        assert!(
            select_developer_identity("0 valid identities found")
                .unwrap()
                .is_none()
        );
        assert!(
            select_developer_identity("1) ABC \"Developer ID Application: Other (OTHERTEAM)\"")
                .unwrap()
                .is_none()
        );
        assert!(
            select_developer_identity(&format!("1) ABC \"{expected}\"\n2) DEF \"{expected}\""))
                .is_err()
        );
    }

    #[test]
    fn writable_candidate_is_preserved_while_fresh_publication_is_read_only() {
        let fixture = Fixture::new();
        let source = fixture.0.join("candidate");
        fs::write(&source, b"signed executable fixture").unwrap();
        fs::set_permissions(&source, fs::Permissions::from_mode(0o755)).unwrap();
        let inode = fs::metadata(&source).unwrap().ino();
        publish_fresh_resources(&fixture.out(), || {
            fs::create_dir_all(fixture.destination())?;
            fs::copy(&source, fixture.destination().join("codex-acp"))?;
            fs::write(
                fixture.destination().join("build-manifest.json"),
                b"closed manifest fixture",
            )?;
            fs::create_dir_all(fixture.0.join("target/debug/extraction"))?;
            Ok(())
        })
        .unwrap();
        assert_eq!(
            fs::metadata(fixture.destination()).unwrap().mode() & 0o777,
            0o555
        );
        assert_eq!(
            fs::metadata(fixture.0.join("target/debug/extraction"))
                .unwrap()
                .mode()
                & 0o777,
            0o555
        );
        let published = fs::metadata(fixture.destination().join("codex-acp")).unwrap();
        assert_ne!(published.ino(), inode);
        assert_eq!(published.mode() & 0o777, 0o555);
        assert_eq!(published.nlink(), 1);
        assert_eq!(
            fs::metadata(fixture.destination().join("build-manifest.json"))
                .unwrap()
                .mode()
                & 0o777,
            0o444
        );
        assert_eq!(fs::metadata(&source).unwrap().ino(), inode);
        assert_eq!(fs::metadata(&source).unwrap().mode() & 0o777, 0o755);
        assert_eq!(fs::read(&source).unwrap(), b"signed executable fixture");
    }

    #[test]
    fn sealing_failure_restores_both_resource_trees_and_preserves_external_files() {
        let fixture = Fixture::new();
        let helper = fixture.0.join("target/debug/extraction");
        fs::create_dir_all(fixture.destination()).unwrap();
        fs::create_dir_all(&helper).unwrap();
        fs::write(fixture.destination().join("prior"), b"provider").unwrap();
        fs::write(helper.join("prior"), b"helper").unwrap();
        seal_generated_resources(&fixture.destination()).unwrap();
        seal_generated_resources(&helper).unwrap();
        let provider_root_inode = fs::metadata(fixture.destination()).unwrap().ino();
        let helper_root_inode = fs::metadata(&helper).unwrap().ino();
        let provider_inode = fs::metadata(fixture.destination().join("prior"))
            .unwrap()
            .ino();
        let helper_inode = fs::metadata(helper.join("prior")).unwrap().ino();
        let outside = fixture.0.join("outside-canary");
        fs::write(&outside, b"outside").unwrap();
        assert!(
            publish_fresh_resources(&fixture.out(), || {
                fs::create_dir_all(fixture.destination())?;
                fs::write(fixture.destination().join("fresh"), b"provider")?;
                fs::create_dir_all(&helper)?;
                std::os::unix::fs::symlink(&outside, helper.join("invalid"))?;
                Ok(())
            })
            .is_err()
        );
        assert_eq!(
            fs::metadata(fixture.destination().join("prior"))
                .unwrap()
                .ino(),
            provider_inode
        );
        assert_eq!(
            fs::metadata(helper.join("prior")).unwrap().ino(),
            helper_inode
        );
        assert_eq!(
            fs::metadata(fixture.destination()).unwrap().ino(),
            provider_root_inode
        );
        assert_eq!(fs::metadata(&helper).unwrap().ino(), helper_root_inode);
        assert_eq!(
            fs::metadata(fixture.destination()).unwrap().mode() & 0o777,
            0o555
        );
        assert_eq!(fs::metadata(&helper).unwrap().mode() & 0o777, 0o555);
        assert_eq!(fs::read(&outside).unwrap(), b"outside");
    }

    #[test]
    fn repeated_copy_preserves_immutable_source_and_publishes_fresh_inodes() {
        let fixture = Fixture::new();
        let source = fixture.0.join("source.pem");
        fs::write(&source, b"immutable public certificate fixture").unwrap();
        fs::set_permissions(&source, fs::Permissions::from_mode(0o444)).unwrap();
        let copy = || {
            fs::create_dir_all(fixture.destination())?;
            fs::copy(&source, fixture.destination().join("cacert.pem"))?;
            fs::create_dir_all(fixture.0.join("target/debug/extraction"))?;
            fs::copy(
                &source,
                fixture.0.join("target/debug/extraction/helper.sha256"),
            )?;
            Ok(())
        };
        publish_fresh_resources(&fixture.out(), copy).unwrap();
        let before = fs::metadata(fixture.destination().join("cacert.pem"))
            .unwrap()
            .ino();
        publish_fresh_resources(&fixture.out(), copy).unwrap();
        let after = fs::metadata(fixture.destination().join("cacert.pem")).unwrap();
        assert_ne!(before, after.ino());
        assert_eq!(after.mode() & 0o777, 0o444);
        assert_eq!(
            fs::read(&source).unwrap(),
            b"immutable public certificate fixture"
        );
        assert_eq!(fs::metadata(&source).unwrap().mode() & 0o777, 0o444);
    }

    #[test]
    fn failed_copy_restores_previous_inode_and_preserves_siblings() {
        let fixture = Fixture::new();
        fs::create_dir_all(fixture.destination()).unwrap();
        let prior = fixture.destination().join("previous.pem");
        fs::write(&prior, b"previous verified resource").unwrap();
        let inode = fs::metadata(&prior).unwrap().ino();
        let sibling = fixture
            .destination()
            .parent()
            .unwrap()
            .join("other-provider.txt");
        fs::write(&sibling, b"unrelated generated resource").unwrap();
        assert!(
            publish_fresh_resources(&fixture.out(), || {
                fs::create_dir_all(fixture.destination())?;
                fs::write(fixture.destination().join("partial"), b"partial")?;
                Err(io::Error::other("controlled build failure"))
            })
            .is_err()
        );
        assert_eq!(fs::metadata(prior).unwrap().ino(), inode);
        assert_eq!(fs::read(sibling).unwrap(), b"unrelated generated resource");
        assert!(!fixture.destination().join("partial").exists());
    }

    #[test]
    fn successful_callback_with_missing_or_invalid_output_restores_previous_resources() {
        for output in ["missing", "file", "symlink"] {
            let fixture = Fixture::new();
            fs::create_dir_all(fixture.destination()).unwrap();
            let prior = fixture.destination().join("previous.pem");
            fs::write(&prior, b"verified prior resource").unwrap();
            let inode = fs::metadata(&prior).unwrap().ino();
            let outside = fixture.0.join("outside");
            fs::create_dir(&outside).unwrap();
            fs::write(outside.join("keep"), b"outside canary").unwrap();
            assert!(
                publish_fresh_resources(&fixture.out(), || {
                    match output {
                        "file" => fs::write(fixture.destination(), b"invalid resource root")?,
                        "symlink" => std::os::unix::fs::symlink(&outside, fixture.destination())?,
                        _ => {}
                    }
                    Ok(())
                })
                .is_err()
            );
            assert_eq!(fs::metadata(&prior).unwrap().ino(), inode);
            assert_eq!(fs::read(&prior).unwrap(), b"verified prior resource");
            assert_eq!(fs::read(outside.join("keep")).unwrap(), b"outside canary");
        }
    }

    #[test]
    fn both_resource_trees_restore_when_helper_publication_is_invalid() {
        for invalid in ["missing", "file", "symlink"] {
            let fixture = Fixture::new();
            let helper = fixture.0.join("target/debug/extraction");
            fs::create_dir_all(fixture.destination()).unwrap();
            fs::create_dir_all(&helper).unwrap();
            let provider_prior = fixture.destination().join("prior");
            let helper_prior = helper.join("prior");
            fs::write(&provider_prior, b"provider authority").unwrap();
            fs::write(&helper_prior, b"helper authority").unwrap();
            seal_generated_resources(&fixture.destination()).unwrap();
            seal_generated_resources(&helper).unwrap();
            let provider_inode = fs::metadata(&provider_prior).unwrap().ino();
            let helper_inode = fs::metadata(&helper_prior).unwrap().ino();
            let outside = fixture.0.join("outside");
            fs::create_dir(&outside).unwrap();
            fs::write(outside.join("keep"), b"outside canary").unwrap();
            assert!(
                publish_fresh_resources(&fixture.out(), || {
                    fs::create_dir_all(fixture.destination())?;
                    fs::write(fixture.destination().join("new"), b"new provider")?;
                    match invalid {
                        "file" => fs::write(&helper, b"invalid helper root")?,
                        "symlink" => std::os::unix::fs::symlink(&outside, &helper)?,
                        _ => {}
                    }
                    Ok(())
                })
                .is_err()
            );
            assert_eq!(fs::metadata(&provider_prior).unwrap().ino(), provider_inode);
            assert_eq!(fs::metadata(&helper_prior).unwrap().ino(), helper_inode);
            assert_eq!(fs::read(&provider_prior).unwrap(), b"provider authority");
            assert_eq!(fs::read(&helper_prior).unwrap(), b"helper authority");
            assert_eq!(fs::read(outside.join("keep")).unwrap(), b"outside canary");
        }
    }

    #[test]
    fn symlinked_generated_paths_are_rejected_without_touching_external_data() {
        let fixture = Fixture::new();
        let outside = fixture.0.join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("keep"), b"outside canary").unwrap();
        fs::create_dir_all(fixture.destination().parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&outside, fixture.destination()).unwrap();
        assert!(publish_fresh_resources(&fixture.out(), || panic!("build must not run")).is_err());
        assert_eq!(fs::read(outside.join("keep")).unwrap(), b"outside canary");
        assert!(
            publish_fresh_resources(&fixture.0, || panic!("invalid output must not run")).is_err()
        );
    }
}
