use crate::{AuthenticationStatus, CodexAcpClient, CodexAcpLaunch};
use sha2::{Digest, Sha256};
use std::{
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use zeroize::{Zeroize, Zeroizing};

fn broker_snapshot_digest(broker: &crate::subscription::SubscriptionBroker) -> [u8; 32] {
    fn clear(value: &mut serde_json::Value) {
        match value {
            serde_json::Value::String(value) => value.zeroize(),
            serde_json::Value::Array(values) => values.iter_mut().for_each(clear),
            serde_json::Value::Object(values) => values.values_mut().for_each(clear),
            _ => {}
        }
    }
    let mut parameters = broker
        .connect_parameters()
        .expect("broker snapshot unavailable");
    let bytes =
        Zeroizing::new(serde_json::to_vec(&parameters).expect("broker snapshot unavailable"));
    let digest = Sha256::digest(&*bytes).into();
    clear(&mut parameters);
    digest
}

fn copy_public_artifact(source: &std::path::Path, destination: &std::path::Path) {
    let metadata = std::fs::symlink_metadata(source).expect("artifact metadata");
    assert!(
        !metadata.file_type().is_symlink(),
        "artifact symlink rejected"
    );
    if metadata.is_dir() {
        std::fs::create_dir(destination).expect("fresh artifact directory");
        for entry in std::fs::read_dir(source).expect("artifact entries") {
            let entry = entry.expect("artifact entry");
            copy_public_artifact(&entry.path(), &destination.join(entry.file_name()));
        }
    } else {
        assert!(metadata.is_file(), "regular artifact required");
        std::fs::copy(source, destination).expect("fresh artifact inode");
    }
}

struct DiagnosticFileReader;

impl crate::ClientFileReader for DiagnosticFileReader {
    fn read_text_file(
        &self,
        _: &Path,
        _: Option<u32>,
        _: Option<u32>,
    ) -> Result<String, crate::ClientFileReadError> {
        Err(crate::ClientFileReadError)
    }
}

#[tokio::test]
#[ignore = "Requires explicit signed resources and subscription; compares official authentication status without inference."]
async fn official_client_certificate_backend_differential() {
    let required =
        |name| PathBuf::from(std::env::var_os(name).expect("explicit diagnostic input required"));
    let artifact = required("MAGI_TEST_PROVIDER_DIR");
    let selected = required("MAGI_TEST_CREDENTIAL_HOME");
    let root = required("MAGI_TEST_DIAGNOSTIC_ROOT");
    assert!(
        root.is_absolute() && !root.exists(),
        "fresh diagnostic root required"
    );
    std::fs::create_dir(&root).expect("fresh diagnostic root");
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    let root = root.canonicalize().unwrap();
    let manifest: serde_json::Value = serde_json::from_slice(
        &std::fs::read(artifact.join("build-manifest.json")).expect("public manifest"),
    )
    .expect("public manifest shape");
    let digest = manifest["artifact_sha256"]
        .as_str()
        .expect("artifact pin")
        .to_owned();
    let broker = Arc::new(
        crate::subscription::SubscriptionBroker::open(
            crate::subscription::CredentialHome::inspect(&selected).expect("selected authority"),
        )
        .expect("selected account pin"),
    );
    let snapshot = Zeroizing::new(broker_snapshot_digest(&broker));
    let mut observations = Vec::new();
    for (name, platform) in [("configured", false), ("platform_roots", true)] {
        assert!(
            broker_snapshot_digest(&broker) == *snapshot,
            "snapshot_rotated"
        );
        let case = root.join(name);
        let home = case.join("runtime-diagnostic");
        let role = case.join("role");
        for directory in [&case, &home, &role] {
            std::fs::create_dir(directory).unwrap();
            std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let launch = CodexAcpLaunch {
            executable: artifact.join("codex-acp"),
            expected_executable_sha256: digest.clone(),
            provider_profile_id: "certificate-diagnostic".into(),
            profile_revision: 0,
            runtime_home_id: "runtime-diagnostic".into(),
            profile_home: home,
            role_workdir: role,
        };
        let reader: Arc<dyn crate::ClientFileReader> = Arc::new(DiagnosticFileReader);
        let deadline = tokio::time::Instant::now() + Duration::from_secs(45);
        let client = tokio::time::timeout_at(deadline, async {
            if platform {
                CodexAcpClient::spawn_platform_roots_diagnostic(launch, reader).await
            } else {
                CodexAcpClient::spawn_with_client_file_reader(launch, reader).await
            }
        })
        .await
        .expect("diagnostic spawn deadline")
        .expect("verified diagnostic spawn");
        let mut injection_acknowledged = false;
        let outcome = tokio::time::timeout_at(deadline, async {
            client.initialize().await?;
            client.connect_existing_subscription(broker.clone()).await?;
            injection_acknowledged = true;
            client.authentication_status().await
        })
        .await;
        let status = match &outcome {
            Ok(Ok(AuthenticationStatus::Authenticated { .. })) => "authenticated",
            Ok(Ok(_)) => "authentication_required",
            Ok(Err(_)) => "rpc_failed",
            Err(_) => "deadline",
        };
        let diagnostic = client.rpc_failure_diagnostic().await;
        client.shutdown().await;
        assert!(
            broker_snapshot_digest(&broker) == *snapshot,
            "snapshot_rotated"
        );
        observations.push(serde_json::json!({"case":name,"injectionAcknowledged":injection_acknowledged,"status":status,"diagnostic":diagnostic}));
        std::fs::write(
            root.join("observations.json"),
            serde_json::to_vec_pretty(&observations).unwrap(),
        )
        .expect("durable closed observation");
    }
    assert!(
        observations
            .iter()
            .all(|value| value["status"] != "deadline"),
        "diagnostic_deadline"
    );
}

#[tokio::test]
#[ignore = "Requires explicit signed artifacts and selected subscription; compares authentication status only."]
async fn signed_codex_authentication_status_differential() {
    let required =
        |name| PathBuf::from(std::env::var_os(name).expect("explicit diagnostic input required"));
    let selected = required("MAGI_TEST_CREDENTIAL_HOME");
    let artifact = required("MAGI_TEST_PROVIDER_DIR");
    let upstream = required("MAGI_TEST_UPSTREAM_CODEX");
    let root = required("MAGI_TEST_DIAGNOSTIC_ROOT");
    assert!(
        root.is_absolute() && !root.exists(),
        "fresh diagnostic root required"
    );
    std::fs::create_dir(&root).expect("diagnostic root");
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    let root = root.canonicalize().unwrap();
    let broker = Arc::new(
        crate::subscription::SubscriptionBroker::open(
            crate::subscription::CredentialHome::inspect(&selected).expect("selected authority"),
        )
        .expect("selected account pin"),
    );
    let snapshot = Zeroizing::new(broker_snapshot_digest(&broker));
    let vendor_relative = "vendor/aarch64-apple-darwin/bin/codex";
    let upstream_sha = "0196e89fe5a7598f816ee54232c3d7c26d75e502ab5cfe2c9240e81d90f7255a";
    let resigned_sha = "f88be5286083a3d34da175b6a271c6a0d10af8265b14ce094d92d85a64d5bacd";
    let digest_file = |path: &std::path::Path| {
        format!(
            "{:x}",
            Sha256::digest(std::fs::read(path).expect("public artifact"))
        )
    };
    assert_eq!(
        digest_file(&upstream),
        upstream_sha,
        "official artifact pin"
    );
    assert_eq!(
        digest_file(&artifact.join(vendor_relative)),
        resigned_sha,
        "resigned artifact pin"
    );
    let mut observations = Vec::new();
    for (name, official) in [("official", true), ("resigned", false)] {
        assert!(
            broker_snapshot_digest(&broker) == *snapshot,
            "snapshot_rotated"
        );
        let case = root.join(name);
        std::fs::create_dir(&case).unwrap();
        std::fs::set_permissions(&case, std::fs::Permissions::from_mode(0o700)).unwrap();
        let copied = case.join("darwin-arm64");
        copy_public_artifact(&artifact, &copied);
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(copied.join("build-manifest.json")).unwrap())
                .unwrap();
        if official {
            std::fs::remove_file(copied.join(vendor_relative)).unwrap();
            std::fs::copy(&upstream, copied.join(vendor_relative)).unwrap();
            manifest["codex_executable_sha256"] = upstream_sha.into();
            std::fs::write(
                copied.join("build-manifest.json"),
                serde_json::to_vec_pretty(&manifest).unwrap(),
            )
            .unwrap();
        }
        let home = case.join("runtime-signing-control");
        let role = case.join("role");
        for directory in [&home, &role] {
            std::fs::create_dir(directory).unwrap();
            std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let client = CodexAcpClient::spawn(CodexAcpLaunch {
            executable: copied.join("codex-acp"),
            expected_executable_sha256: manifest["artifact_sha256"].as_str().unwrap().into(),
            provider_profile_id: "signing-diagnostic".into(),
            profile_revision: 0,
            runtime_home_id: "runtime-signing-control".into(),
            profile_home: home,
            role_workdir: role,
        })
        .await
        .expect("strict signed diagnostic spawn");
        let outcome = tokio::time::timeout(Duration::from_secs(180), async {
            client.initialize().await?;
            client.connect_existing_subscription(broker.clone()).await?;
            client.authentication_status().await
        })
        .await;
        let authenticated = matches!(outcome, Ok(Ok(AuthenticationStatus::Authenticated { .. })));
        let completed = outcome.is_ok();
        client.shutdown().await;
        let diagnostic = client.rpc_failure_diagnostic().await;
        assert!(
            broker_snapshot_digest(&broker) == *snapshot,
            "snapshot_rotated"
        );
        observations.push(serde_json::json!({"case":name,"completed":completed,"authenticated":authenticated,"diagnostic":diagnostic}));
        eprintln!(
            "signing control {name}: completed={completed} authenticated={authenticated} diagnostic={diagnostic:?}"
        );
    }
    std::fs::write(
        root.join("observations.json"),
        serde_json::to_vec_pretty(&observations).unwrap(),
    )
    .unwrap();
    assert!(
        observations.iter().all(|case| case["completed"] == true),
        "diagnostic_deadline"
    );
}

struct FixtureDirectory(PathBuf);

impl Drop for FixtureDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[tokio::test]
#[ignore = "Requires an explicitly selected subscription and signed provider; performs real source-free inference."]
async fn signed_provider_infers_with_selected_existing_subscription() {
    let evidence_path = std::env::var_os("MAGI_TEST_INFERENCE_EVIDENCE_PATH").map(PathBuf::from);
    let evidence_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .unwrap()
        .join(".local");
    if let Some(path) = &evidence_path {
        validate_inference_evidence_path(&evidence_root, path)
            .expect("safe fresh inference evidence path");
    }
    let selected = PathBuf::from(
        std::env::var_os("MAGI_TEST_CREDENTIAL_HOME")
            .expect("explicit selected authority required"),
    );
    let authority = crate::subscription::CredentialHome::inspect(&selected)
        .expect("selected authority inspection");
    let broker = Arc::new(
        crate::subscription::SubscriptionBroker::open(authority).expect("selected account pin"),
    );
    let artifact = PathBuf::from(
        std::env::var_os("MAGI_TEST_PROVIDER_DIR").expect("explicit signed artifact required"),
    );
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(artifact.join("build-manifest.json")).unwrap())
            .unwrap();
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = FixtureDirectory(std::env::temp_dir().canonicalize().unwrap().join(format!(
        "magi-native-inference-{}-{nonce}",
        std::process::id()
    )));
    let runtime_home_id = format!("runtime-{nonce}");
    let home = root.0.join(&runtime_home_id);
    let role = root.0.join("role");
    for directory in [&root.0, &home, &role] {
        std::fs::create_dir(directory).unwrap();
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let client = CodexAcpClient::spawn(CodexAcpLaunch {
        executable: artifact.join("codex-acp"),
        expected_executable_sha256: manifest["artifact_sha256"].as_str().unwrap().into(),
        provider_profile_id: "native-inference-fixture".into(),
        profile_revision: 0,
        runtime_home_id,
        profile_home: home,
        role_workdir: role,
    })
    .await
    .expect("verified sandboxed provider spawn");
    let result = tokio::time::timeout(Duration::from_secs(240), async {
        client.initialize().await?;
        client.connect_existing_subscription(broker).await?;
        assert!(matches!(
            client.authentication_status().await?,
            AuthenticationStatus::Authenticated { .. }
        ));
        let session = client.discover_session().await?;
        let selected_id = std::env::var("MAGI_TEST_MODEL_ID")
            .unwrap_or_else(|_| session.current_model_id.clone());
        let model = session
            .available_models
            .iter()
            .find(|model| model.model_id == selected_id)
            .expect("selected model must exist in actual provider catalog");
        eprintln!("native inference: authenticated catalog model selected");
        let pending = client
            .begin_model_configuration(&session.session_id, &model.model_id)
            .await?;
        client.finish_model_configuration(pending).await?;
        let selected_mode = std::env::var("MAGI_TEST_MODE_ID")
            .ok()
            .or_else(|| session.current_mode_id.clone());
        if let Some(mode) = selected_mode {
            assert!(
                session
                    .available_modes
                    .iter()
                    .any(|available| available.mode_id == mode),
                "selected mode must exist in actual provider modes"
            );
            client.select_mode(&session.session_id, &mode).await?;
        }
        let confirmed = client.confirmed_session_info(&session.session_id).await?;
        let response = client
            .prompt(
                &session.session_id,
                "Without tools or files, calculate 17 plus 25. Reply with only the integer.".into(),
            )
            .await?
            .finish()
            .await?;
        assert_eq!(response.final_text.trim(), "42");
        if let Some(path) = &evidence_path {
            let evidence = serde_json::json!({"modelId":confirmed.current_model_id,"modeId":confirmed.current_mode_id,"adapterArtifactSha256":manifest["artifact_sha256"],"body":"42"});
            write_inference_evidence(&evidence_root, path, &evidence).expect("verified inference evidence publication");
        }
        eprintln!("native inference: source-free model result verified");
        Ok::<(), crate::ProviderError>(())
    })
    .await;
    if !matches!(result, Ok(Ok(()))) {
        eprintln!(
            "native inference safe RPC diagnostic: {:?}",
            client.rpc_failure_diagnostic().await
        );
    }
    client.shutdown().await;
    result
        .expect("bounded source-free inference")
        .expect("real source-free provider inference");
}

fn validate_inference_evidence_path(root: &Path, path: &Path) -> std::io::Result<PathBuf> {
    let root = root.canonicalize()?;
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("Evidence parent is required"))?
        .canonicalize()?;
    if !path.is_absolute()
        || !parent.starts_with(&root)
        || path.file_name().is_none()
        || std::fs::symlink_metadata(path).is_ok()
    {
        return Err(std::io::Error::other(
            "Evidence must be a fresh file under the local artifact directory",
        ));
    }
    Ok(parent.join(path.file_name().unwrap()))
}

fn write_inference_evidence(
    root: &Path,
    path: &Path,
    evidence: &serde_json::Value,
) -> std::io::Result<()> {
    let path = validate_inference_evidence_path(root, path)?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?;
    file.write_all(&serde_json::to_vec_pretty(evidence)?)?;
    file.sync_all()
}

#[test]
fn verified_inference_evidence_rejects_overwrite_escape_and_symlink_targets() {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = FixtureDirectory(
        std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("inference-evidence-{}-{nonce}", std::process::id())),
    );
    std::fs::create_dir(&root.0).unwrap();
    let local = root.0.join("local");
    std::fs::create_dir(&local).unwrap();
    let evidence = serde_json::json!({"modelId":"actual-test-model","modeId":null,"adapterArtifactSha256":"a".repeat(64),"body":"42"});
    let path = local.join("verified.json");
    write_inference_evidence(&local, &path, &evidence).unwrap();
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(write_inference_evidence(&local, &path, &evidence).is_err());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&std::fs::read(path).unwrap()).unwrap(),
        evidence
    );
    let outside = root.0.join("outside.json");
    assert!(write_inference_evidence(&local, &outside, &evidence).is_err());
    std::fs::write(&outside, b"outside canary").unwrap();
    let symlink = local.join("link.json");
    std::os::unix::fs::symlink(&outside, &symlink).unwrap();
    assert!(write_inference_evidence(&local, &symlink, &evidence).is_err());
    assert_eq!(std::fs::read(outside).unwrap(), b"outside canary");
}

#[test]
fn actual_child_spawn_and_revocation_are_linearized_in_both_orders() {
    use std::{
        process::{Command, Stdio},
        sync::{
            Barrier,
            atomic::{AtomicBool, Ordering},
        },
        time::Instant,
    };
    fn child() -> std::io::Result<std::process::Child> {
        Command::new("/usr/bin/true")
            .env_clear()
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
    }
    let revoked = crate::VerificationRequest::until(Instant::now() + Duration::from_secs(2));
    revoked.revoke();
    let called = AtomicBool::new(false);
    assert!(matches!(
        revoked.publish(|| {
            called.store(true, Ordering::SeqCst);
            child()
        }),
        Err(crate::ProviderError::Cancelled)
    ));
    assert!(!called.load(Ordering::SeqCst));

    let request = crate::VerificationRequest::until(Instant::now() + Duration::from_secs(2));
    let entered = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    let starting = request.clone();
    let started = entered.clone();
    let permitted = release.clone();
    let launch = std::thread::spawn(move || {
        starting
            .publish(|| {
                let child = child().unwrap();
                started.wait();
                permitted.wait();
                child
            })
            .unwrap()
    });
    entered.wait();
    let cancelling = request.clone();
    let cancellation = std::thread::spawn(move || cancelling.revoke());
    release.wait();
    let mut owned_child = launch.join().unwrap();
    cancellation.join().unwrap();
    assert!(owned_child.wait().unwrap().success());
    assert!(matches!(
        request.publish(child),
        Err(crate::ProviderError::Cancelled)
    ));
}
