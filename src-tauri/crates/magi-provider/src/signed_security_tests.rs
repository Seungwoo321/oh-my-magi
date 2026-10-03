use crate::{CodexAcpClient, CodexAcpLaunch, sandbox};
use std::{
    net::TcpListener,
    os::fd::AsRawFd,
    os::unix::{
        fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
        process::CommandExt,
    },
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

struct Fixture(PathBuf);

fn copy_signed_generation(source: &std::path::Path, destination: &std::path::Path) {
    let metadata = std::fs::symlink_metadata(source).unwrap();
    assert!(!metadata.file_type().is_symlink());
    if metadata.is_dir() {
        std::fs::create_dir(destination).unwrap();
        for entry in std::fs::read_dir(source).unwrap() {
            let entry = entry.unwrap();
            copy_signed_generation(&entry.path(), &destination.join(entry.file_name()));
        }
    } else {
        std::fs::copy(source, destination).unwrap();
    }
    let permissions = if metadata.is_dir() {
        std::fs::Permissions::from_mode(0o555)
    } else {
        metadata.permissions()
    };
    std::fs::set_permissions(destination, permissions).unwrap();
}

#[tokio::test]
#[ignore = "Requires an explicitly selected signed provider artifact; verifies authority only, with no authentication or inference."]
async fn signed_verification_service_single_flight_replacement_and_abort_are_fenced() {
    use crate::{RuntimeVerificationService, VerificationRequest};
    use std::sync::Arc;
    fn request() -> VerificationRequest {
        VerificationRequest::until(std::time::Instant::now() + Duration::from_secs(60))
    }
    let source = PathBuf::from(
        std::env::var_os("MAGI_TEST_PROVIDER_DIR").expect("explicit signed artifact required"),
    )
    .canonicalize()
    .unwrap();
    let fixture = Fixture(std::env::temp_dir().canonicalize().unwrap().join(format!(
        "verification-service-{}-{}", std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos())));
    std::fs::create_dir(&fixture.0).unwrap();
    let container = fixture.0.join("generation");
    std::fs::create_dir(&container).unwrap();
    let root = container.join("darwin-arm64");
    copy_signed_generation(&source, &root);
    let executable = root.join("codex-acp");
    let service = RuntimeVerificationService::controlled_fixture();
    let timer = tokio::spawn(async {
        tokio::time::sleep(Duration::from_millis(25)).await;
        true
    });
    let (first, second) = tokio::join!(
        service.verify(executable.clone(), request()),
        service.verify(executable.clone(), request())
    );
    let first = first.unwrap();
    let second = second.unwrap();
    assert!(
        timer.is_finished(),
        "blocking proof must not starve the runtime timer"
    );
    assert!(timer.await.unwrap());
    assert!(Arc::ptr_eq(&first, &second));
    let metrics = service.metrics();
    assert_eq!(metrics.proofs, 1);
    assert_eq!(metrics.reused, 1);
    let expected_bytes: u64 = [
        "codex-acp",
        "vendor/aarch64-apple-darwin/bin/codex",
        "vendor/aarch64-apple-darwin/codex-path/rg",
        "cacert.pem",
    ]
    .into_iter()
    .map(|relative| std::fs::metadata(root.join(relative)).unwrap().len())
    .sum();
    assert_eq!(metrics.hashed_bytes, expected_bytes);
    println!("verification_metrics={metrics:?}");
    let reused = service.verify(executable.clone(), request()).await.unwrap();
    assert!(Arc::ptr_eq(&first, &reused));
    assert_eq!(service.metrics().proofs, 1);
    std::fs::rename(&container, fixture.0.join("previous")).unwrap();
    std::fs::create_dir(&container).unwrap();
    copy_signed_generation(&source, &root);
    assert!(first.check(&request()).is_err());
    let replacement = service.verify(executable.clone(), request()).await.unwrap();
    assert!(!Arc::ptr_eq(&first, &replacement));
    assert_eq!(first.identity(), replacement.identity());
    assert_eq!(service.metrics().proofs, 2);
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(service.verify(executable.clone(), request()).await.is_err());
    assert!(!service.has_published_authority().await);
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o555)).unwrap();
    let abort_service = Arc::new(RuntimeVerificationService::controlled_fixture());
    let worker_service = abort_service.clone();
    let task = tokio::spawn(async move { worker_service.verify(executable, request()).await });
    while abort_service.metrics().proofs == 0 {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    task.abort();
    assert!(matches!(task.await, Err(error) if error.is_cancelled()));
    abort_service.wait_for_worker_settlement().await.unwrap();
    assert!(!abort_service.has_published_authority().await);
    println!("replacement_and_aborted_worker_fenced=true");
}

#[test]
#[ignore = "Requires an explicitly selected read-only signed provider artifact; no authentication or inference."]
fn signed_artifact_rejects_owner_writable_executables_and_manifest() {
    fn copy_tree(source: &std::path::Path, destination: &std::path::Path) {
        let metadata = std::fs::symlink_metadata(source).unwrap();
        assert!(!metadata.file_type().is_symlink());
        if metadata.is_dir() {
            std::fs::create_dir(destination).unwrap();
            for entry in std::fs::read_dir(source).unwrap() {
                let entry = entry.unwrap();
                copy_tree(&entry.path(), &destination.join(entry.file_name()));
            }
        } else {
            std::fs::copy(source, destination).unwrap();
            std::fs::set_permissions(destination, metadata.permissions()).unwrap();
            assert_ne!(
                metadata.ino(),
                std::fs::metadata(destination).unwrap().ino()
            );
        }
    }
    let source = PathBuf::from(std::env::var_os("MAGI_TEST_PROVIDER_DIR").unwrap());
    crate::verify_packaged_artifact(&source.join("codex-acp")).unwrap();
    let root = Fixture(std::env::temp_dir().canonicalize().unwrap().join(format!(
        "readonly-artifact-{}-{}", std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
    )));
    std::fs::create_dir(&root.0).unwrap();
    let destination = root.0.join("darwin-arm64");
    copy_tree(&source, &destination);
    crate::verify_packaged_artifact(&destination.join("codex-acp")).unwrap();
    for relative in [
        "codex-acp",
        "build-manifest.json",
        "vendor/aarch64-apple-darwin/bin/codex",
        "vendor/aarch64-apple-darwin/codex-path/rg",
    ] {
        let path = destination.join(relative);
        let original = std::fs::metadata(&path).unwrap().permissions();
        std::fs::set_permissions(
            &path,
            std::fs::Permissions::from_mode(original.mode() | 0o200),
        )
        .unwrap();
        assert!(crate::verify_packaged_artifact(&destination.join("codex-acp")).is_err());
        std::fs::set_permissions(&path, original).unwrap();
    }
    crate::verify_packaged_artifact(&destination.join("codex-acp")).unwrap();
}

#[test]
fn runtime_manifest_provenance_is_closed_and_preserves_explicit_legacy_policy() {
    let mut legacy = serde_json::Map::new();
    for key in [
        "provider",
        "package",
        "package_version",
        "package_integrity",
        "source_commit",
        "bundled_codex_package",
        "bundled_codex_version",
        "bundled_codex_integrity",
        "codex_platform_package",
        "codex_platform_version",
        "codex_platform_integrity",
        "codex_target_triple",
        "target",
        "upstream_artifact_sha256",
        "adapter_patch_id",
        "artifact_sha256",
        "codex_executable_sha256",
        "ripgrep_sha256",
        "public_ca_source_url",
        "public_ca_sha256",
        "status",
    ] {
        legacy.insert(key.into(), "shape-control".into());
    }
    legacy.insert("schema_version".into(), 4.into());
    let valid = |value: &serde_json::Map<String, serde_json::Value>| {
        sandbox::runtime_manifest_provenance_valid(&serde_json::to_vec(value).unwrap())
    };
    assert!(valid(&legacy));
    let provenance = [
        (
            "codex_source_commit",
            "b412ff32c417f855c2b2d1581b77058eed87c84b",
        ),
        (
            "codex_source_sha256",
            "1ac6a92e7318b8acf3d767170c5c5e6dceeffdc074c73b1c5d422b46f0de4daf",
        ),
        ("codex_source_patch_id", "codex-http-ca-preserve-backend-v1"),
        (
            "codex_source_patch_sha256",
            "b08f4099725b6394e5657691e10d2dc8d9696cd119623fa6155db28a70d76d54",
        ),
        (
            "codex_source_lock_sha256",
            "d722f05fc760bcd1f5749ec452452d81058458b788df3b765b80500d757eba4a",
        ),
    ];
    let mut patched = legacy.clone();
    patched.insert("schema_version".into(), 5.into());
    assert!(!valid(&patched));
    for (field, value) in provenance {
        patched.insert(field.into(), value.into());
    }
    assert!(valid(&patched));
    for (field, _) in provenance {
        for replacement in [serde_json::Value::Null, "unknown".into()] {
            let mut invalid = patched.clone();
            invalid.insert(field.into(), replacement);
            assert!(!valid(&invalid));
        }
        let mut invalid = patched.clone();
        invalid.remove(field);
        assert!(!valid(&invalid));
        let mut invalid = legacy.clone();
        invalid.insert(field.into(), serde_json::Value::Null);
        assert!(!valid(&invalid));
    }
    let mut invalid = patched.clone();
    invalid.insert("extra".into(), true.into());
    assert!(!valid(&invalid));
    let bytes = serde_json::to_string(&patched).unwrap();
    let duplicate = bytes.replacen('{', "{\"schema_version\":5,", 1);
    assert!(!sandbox::runtime_manifest_provenance_valid(
        duplicate.as_bytes()
    ));
    assert!(!sandbox::runtime_manifest_provenance_valid(&vec![
        b' ';
        65_537
    ]));
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fn writable_directories(path: &std::path::Path) {
            let Ok(metadata) = std::fs::symlink_metadata(path) else {
                return;
            };
            if metadata.is_dir() {
                let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755));
                if let Ok(entries) = std::fs::read_dir(path) {
                    for entry in entries.flatten() {
                        writable_directories(&entry.path());
                    }
                }
            }
        }
        writable_directories(&self.0);
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn prepared_fixture() -> (Fixture, sandbox::PreparedLaunch) {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = Fixture(
        std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("provider-security-{}-{nonce}", std::process::id())),
    );
    let runtime_home_id = format!("runtime-{nonce}");
    let home = root.0.join(&runtime_home_id);
    let role = root.0.join("role");
    for directory in [&root.0, &home, &role] {
        std::fs::create_dir(directory).unwrap();
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let artifact = PathBuf::from(
        std::env::var_os("MAGI_TEST_PROVIDER_DIR")
            .expect("explicit signed provider artifact required"),
    );
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(artifact.join("build-manifest.json")).unwrap())
            .unwrap();
    let prepared = sandbox::prepare(&CodexAcpLaunch {
        executable: artifact.join("codex-acp"),
        expected_executable_sha256: manifest["artifact_sha256"].as_str().unwrap().into(),
        provider_profile_id: "security-fixture".into(),
        profile_revision: 0,
        runtime_home_id,
        profile_home: home,
        role_workdir: role,
    })
    .unwrap();
    (root, prepared)
}

fn proxy_url(listener: &TcpListener) -> String {
    format!(
        "http://magi:{}@127.0.0.1:{}",
        "a".repeat(64),
        listener.local_addr().unwrap().port()
    )
}

async fn bounded_output(mut command: tokio::process::Command) -> std::process::Output {
    command.kill_on_drop(true);
    tokio::time::timeout(Duration::from_secs(15), command.output())
        .await
        .expect("signed security probe deadline")
        .expect("signed security probe spawn")
}

#[tokio::test]
#[ignore = "Requires an explicitly selected signed macOS provider artifact; no authentication or inference."]
async fn signed_runtime_denies_sibling_files_writes_and_proxy_bypass() {
    let (root, prepared) = prepared_fixture();
    let source = root.0.join("outside-source.txt");
    let sibling_home = root.0.join("sibling-home");
    let sibling_role = root.0.join("sibling-role");
    for directory in [&sibling_home, &sibling_role] {
        std::fs::create_dir(directory).unwrap();
    }
    let sibling_auth = sibling_home.join("auth.json");
    let sibling_context = sibling_role.join("context.txt");
    for file in [&source, &sibling_auth, &sibling_context] {
        std::fs::write(file, b"disposable security canary").unwrap();
    }
    let outside_write = root.0.join("outside-write.txt");
    let allowed_source = prepared.role_workdir.join("approved-context.txt");
    std::fs::write(&allowed_source, b"disposable allowed context").unwrap();
    let blocked_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let allowed_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let paths = serde_json::to_string(&[&source, &sibling_auth, &sibling_context]).unwrap();
    let script = r#"const fs=require('node:fs');const denied=[];for(const p of JSON.parse(process.env.TEST_PATHS)){try{fs.readFileSync(p);denied.push(false)}catch(e){denied.push(['EPERM','EACCES'].includes(e.code))}}const allowedReadable=fs.readFileSync(process.env.TEST_ALLOWED,'utf8')==='disposable allowed context';let writeDenied=false;try{fs.writeFileSync(process.env.TEST_WRITE,'disposable')}catch(e){writeDenied=['EPERM','EACCES'].includes(e.code)}let networkOpened=false,networkError=null,networkTimedOut=false;await new Promise(resolve=>{const timer=setTimeout(()=>{networkTimedOut=true;resolve()},3000);const fail=e=>{networkError={code:e.code,errno:e.errno};clearTimeout(timer);resolve()};Bun.connect({hostname:'127.0.0.1',port:Number(process.env.TEST_PORT),socket:{data(){},open(s){networkOpened=true;s.end();clearTimeout(timer);resolve()},error(s,e){fail(e)}}}).catch(fail)});const networkDenied=!networkOpened&&networkError!==null&&['EPERM','EACCES','ECONNREFUSED'].includes(networkError.code);console.log(JSON.stringify({denied,allowedReadable,writeDenied,networkDenied,networkOpened,networkError,networkTimedOut}));"#;
    let configure = |command: &mut tokio::process::Command, port: u16| {
        command
            .env("BUN_BE_BUN", "1")
            .env("TEST_PATHS", &paths)
            .env("TEST_ALLOWED", &allowed_source)
            .env("TEST_WRITE", &outside_write)
            .env("TEST_PORT", port.to_string())
            .args(["-e", script]);
    };
    let mut control = tokio::process::Command::new(&prepared.executable);
    control
        .env_clear()
        .env("HOME", &prepared.profile_home)
        .env("TMPDIR", &prepared.temporary_dir);
    configure(&mut control, blocked_listener.local_addr().unwrap().port());
    let control = bounded_output(control).await;
    assert!(control.status.success());
    let result: serde_json::Value = serde_json::from_slice(&control.stdout).unwrap();
    assert_eq!(result["denied"], serde_json::json!([false, false, false]));
    assert_eq!(result["writeDenied"], false);
    assert_eq!(result["networkDenied"], false);
    assert_eq!(result["networkOpened"], true);
    assert_eq!(result["networkTimedOut"], false);
    assert_eq!(result["allowedReadable"], true);
    blocked_listener.set_nonblocking(true).unwrap();
    assert!(blocked_listener.accept().is_ok());
    std::fs::remove_file(&outside_write).unwrap();
    let mut isolated = sandbox::isolated_command(
        &prepared,
        allowed_listener.local_addr().unwrap(),
        &proxy_url(&allowed_listener),
    )
    .unwrap();
    configure(&mut isolated, blocked_listener.local_addr().unwrap().port());
    let output = bounded_output(isolated).await;
    assert!(output.status.success(), "signed sandbox probe failed");
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["denied"], serde_json::json!([true, true, true]));
    assert_eq!(result["writeDenied"], true);
    assert_eq!(result["networkDenied"], true, "sandbox result: {result}");
    assert_eq!(result["networkOpened"], false);
    assert_eq!(result["networkTimedOut"], false);
    assert_eq!(result["allowedReadable"], true);
    assert!(!outside_write.exists());
    assert_eq!(
        blocked_listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    let mut allowed = sandbox::isolated_command(
        &prepared,
        allowed_listener.local_addr().unwrap(),
        &proxy_url(&allowed_listener),
    )
    .unwrap();
    configure(&mut allowed, allowed_listener.local_addr().unwrap().port());
    let output = bounded_output(allowed).await;
    assert!(output.status.success());
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["denied"], serde_json::json!([true, true, true]));
    assert_eq!(result["writeDenied"], true);
    assert_eq!(result["networkDenied"], false);
    assert_eq!(result["networkOpened"], true);
    assert_eq!(result["networkTimedOut"], false);
    assert_eq!(result["allowedReadable"], true);
    allowed_listener.set_nonblocking(true).unwrap();
    assert!(allowed_listener.accept().is_ok());
    assert!(!prepared.profile_home.join("auth.json").exists());
}

#[tokio::test]
#[ignore = "Requires an explicitly selected signed macOS provider artifact; no authentication or inference."]
async fn signed_adapter_denies_unapproved_child_executables() {
    let (_root, prepared) = prepared_fixture();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let manifest: serde_json::Value = serde_json::from_slice(
        &std::fs::read(prepared.artifact_dir.join("build-manifest.json")).unwrap(),
    )
    .unwrap();
    let baseline = CodexAcpClient::spawn_controlled_fixture(CodexAcpLaunch {
        executable: prepared.executable.clone(),
        expected_executable_sha256: manifest["artifact_sha256"].as_str().unwrap().into(),
        provider_profile_id: prepared.provider_profile_id.clone(),
        profile_revision: prepared.profile_revision,
        runtime_home_id: prepared.runtime_home_id.clone(),
        profile_home: prepared.profile_home.clone(),
        role_workdir: prepared.role_workdir.clone(),
    })
    .await
    .unwrap();
    let initialized = tokio::time::timeout(Duration::from_secs(20), baseline.initialize()).await;
    baseline.shutdown().await;
    assert_eq!(initialized.unwrap().unwrap().protocol_version, 1);
    drop(baseline);
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(prepared.profile_home.join(".magi-provider-operation.lock"))
        .unwrap();
    let metadata = lock.metadata().unwrap();
    assert!(metadata.is_file());
    assert_eq!(metadata.uid(), unsafe { libc::geteuid() });
    assert_eq!(metadata.mode() & 0o777, 0o600);
    assert_eq!(metadata.nlink(), 1);
    assert_eq!(
        unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    for executable in ["/bin/sh", "/usr/bin/open"] {
        let mut command = sandbox::isolated_command(
            &prepared,
            listener.local_addr().unwrap(),
            &proxy_url(&listener),
        )
        .unwrap();
        command
            .env("CODEX_PATH", executable)
            .env("MAGI_PROVIDER_HOME_LOCK_FD", "3");
        let descriptor = lock.as_raw_fd();
        unsafe {
            command.as_std_mut().pre_exec(move || {
                if libc::dup2(descriptor, 3) < 0 || libc::fcntl(3, libc::F_SETFD, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let output = bounded_output(command).await;
        assert!(!output.status.success());
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(
            stderr.contains("EPERM") && stderr.contains(executable),
            "unapproved child result: {stderr}"
        );
    }
    assert!(!prepared.profile_home.join("auth.json").exists());
}

fn ca_fixture() -> Fixture {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = Fixture(
        std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("provider-public-ca-{}-{nonce}", std::process::id())),
    );
    std::fs::create_dir(&root.0).unwrap();
    std::fs::set_permissions(&root.0, std::fs::Permissions::from_mode(0o700)).unwrap();
    root
}

#[test]
fn public_ca_rejects_missing_modified_writable_and_symlinked_files() {
    let root = ca_fixture();
    assert!(sandbox::verify_public_ca_bundle(&root.0).is_err());
    let path = root.0.join("cacert.pem");
    std::fs::write(&path, b"untrusted test certificate authority").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).unwrap();
    assert!(sandbox::verify_public_ca_bundle(&root.0).is_err());
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(sandbox::verify_public_ca_bundle(&root.0).is_err());
    std::fs::rename(&path, root.0.join("other.pem")).unwrap();
    std::os::unix::fs::symlink(root.0.join("other.pem"), &path).unwrap();
    assert!(sandbox::verify_public_ca_bundle(&root.0).is_err());
}

#[test]
#[ignore = "Requires an explicitly selected signed public-CA provider artifact; no authentication or inference."]
fn pinned_public_ca_and_manifest_authority_are_immutable() {
    let (_prepared_root, prepared) = prepared_fixture();
    assert!(sandbox::verify_public_ca_bundle(&prepared.artifact_dir).is_ok());
    let root = ca_fixture();
    let path = root.0.join("cacert.pem");
    std::fs::copy(&prepared.public_ca_bundle, &path).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).unwrap();
    assert!(sandbox::verify_public_ca_bundle(&root.0).is_ok());
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(sandbox::verify_public_ca_bundle(&root.0).is_err());
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).unwrap();
    std::fs::hard_link(&path, root.0.join("shared.pem")).unwrap();
    assert!(sandbox::verify_public_ca_bundle(&root.0).is_err());
    std::fs::remove_file(root.0.join("shared.pem")).unwrap();
    let target = root.0.join("darwin-arm64");
    std::fs::create_dir(&target).unwrap();
    std::fs::copy(&prepared.executable, target.join("codex-acp")).unwrap();
    let mut manifest: serde_json::Value = serde_json::from_slice(
        &std::fs::read(prepared.artifact_dir.join("build-manifest.json")).unwrap(),
    )
    .unwrap();
    for (source, relative) in [
        (
            &prepared.codex_executable,
            "vendor/aarch64-apple-darwin/bin/codex",
        ),
        (
            &prepared.ripgrep_executable,
            "vendor/aarch64-apple-darwin/codex-path/rg",
        ),
        (&prepared.public_ca_bundle, "cacert.pem"),
    ] {
        let destination = target.join(relative);
        std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
        std::fs::copy(source, &destination).unwrap();
    }
    std::fs::write(
        target.join("build-manifest.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    std::fs::set_permissions(
        target.join("build-manifest.json"),
        std::fs::Permissions::from_mode(0o444),
    )
    .unwrap();
    assert!(sandbox::verify_packaged_artifact(&target.join("codex-acp")).is_ok());
    for (field, value) in [
        ("schema_version", serde_json::json!(3)),
        (
            "public_ca_source_url",
            serde_json::json!("https://example.invalid/alternate-ca.pem"),
        ),
        ("public_ca_sha256", serde_json::json!("0".repeat(64))),
    ] {
        let original = manifest[field].clone();
        manifest[field] = value;
        std::fs::set_permissions(
            target.join("build-manifest.json"),
            std::fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        std::fs::write(
            target.join("build-manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        std::fs::set_permissions(
            target.join("build-manifest.json"),
            std::fs::Permissions::from_mode(0o444),
        )
        .unwrap();
        assert!(sandbox::verify_packaged_artifact(&target.join("codex-acp")).is_err());
        manifest[field] = original;
    }
}

#[tokio::test]
#[ignore = "Requires an explicitly selected signed artifact; launches local children without authentication, session or prompt requests."]
async fn signed_custody_attachment_binds_artifact_and_settles_three_real_clients() {
    use crate::resource_custody::{ArtifactNamespace, DevelopmentProfile, ResourceCustody};
    use crate::{RuntimeVerificationService, VerificationRequest};
    use std::sync::Arc;
    struct DeniedReader;
    impl crate::ClientFileReader for DeniedReader {
        fn read_text_file(
            &self,
            _: &std::path::Path,
            _: Option<u32>,
            _: Option<u32>,
        ) -> Result<String, crate::ClientFileReadError> {
            Err(crate::ClientFileReadError)
        }
    }
    let source = PathBuf::from(
        std::env::var_os("MAGI_TEST_PROVIDER_DIR").expect("explicit signed artifact required"),
    )
    .canonicalize()
    .unwrap();
    let fixture = Fixture(std::env::temp_dir().canonicalize().unwrap().join(format!(
            "custody-signed-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        )));
    std::fs::create_dir(&fixture.0).unwrap();
    let custody = ResourceCustody::bootstrap_empty(
        ArtifactNamespace::development(&fixture.0, DevelopmentProfile::Debug).unwrap(),
    )
    .unwrap();
    let parent = fixture.0.join("src-tauri/target/debug/provider/codex-acp");
    std::fs::create_dir_all(&parent).unwrap();
    let artifact_root = parent.join("darwin-arm64");
    copy_signed_generation(&source, &artifact_root);
    let root = VerificationRequest::until(std::time::Instant::now() + Duration::from_secs(120));
    let service = RuntimeVerificationService::with_custody(custody.clone());
    let artifact = service
        .verify(artifact_root.join("codex-acp"), root.clone())
        .await
        .unwrap();
    let expected_generation = artifact.identity().artifact_set_digest.as_ref().unwrap();
    let control = std::fs::read_dir(fixture.0.join(".local/artifact-authority"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let journal = rusqlite::Connection::open_with_flags(
        control.join("custody.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )
    .unwrap();
    let verified_bindings: u64 = journal
        .query_row(
            "SELECT count(*) FROM operations WHERE generation=?1",
            [expected_generation],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(verified_bindings, 1);
    drop(journal);
    assert!(service.close_custody().await.is_err());
    let mut clients = Vec::new();
    for index in 0..3 {
        let runtime_home_id = format!("custody-runtime-{index}");
        let home = fixture.0.join(&runtime_home_id);
        let role = fixture.0.join(format!("role-{index}"));
        for path in [&home, &role] {
            std::fs::create_dir(path).unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let client = CodexAcpClient::spawn_verified_network_closed_fixture(
            CodexAcpLaunch {
                executable: artifact.executable().to_owned(),
                expected_executable_sha256: artifact.identity().acp_executable_sha256.clone(),
                provider_profile_id: format!("custody-profile-{index}"),
                profile_revision: 0,
                runtime_home_id,
                profile_home: home,
                role_workdir: role,
            },
            Arc::new(DeniedReader),
            artifact.clone(),
            root.clone(),
        )
        .await
        .unwrap();
        assert_eq!(client.fixture_closed_network_counters(), (true, 0, 0, 1));
        clients.push(client);
    }
    assert!(custody.unresolved_operations().unwrap() >= 5);
    root.revoke();
    assert!(service.close_custody().await.is_err());
    for client in clients {
        client.shutdown().await;
        assert_eq!(client.fixture_closed_network_counters(), (true, 0, 0, 1));
        drop(client);
    }
    root.wait_for_settlement(std::time::Instant::now() + Duration::from_secs(15))
        .await
        .unwrap();
    drop(artifact);
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if service.close_custody().await.is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(custody.unresolved_operations().unwrap(), 0);
    assert!(root.settlement().is_settled());
}
