mod commands;
mod pdf_capture;
mod preferences;
mod profiles;
mod run_projection;

use std::os::unix::fs::PermissionsExt;
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU8, Ordering},
    },
    time::{Duration, Instant},
};
use tauri::Manager;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProbePurpose {
    Deliberation,
    CatalogDiagnostic,
    RecoveryCancel,
    PdfUi,
}

impl ProbePurpose {
    fn parse(value: Option<&str>) -> Result<Self, &'static str> {
        match value {
            None | Some("deliberation") => Ok(Self::Deliberation),
            Some("catalog-diagnostic") => Ok(Self::CatalogDiagnostic),
            Some("pdf-ui") => Ok(Self::PdfUi),
            Some("recovery-cancel") => Ok(Self::RecoveryCancel),
            _ => Err("invalid native probe purpose"),
        }
    }

    fn profile_count(self) -> usize {
        match self {
            Self::Deliberation => 3,
            Self::CatalogDiagnostic => 1,
            Self::PdfUi | Self::RecoveryCancel => 0,
        }
    }

    fn permits_admission(self) -> bool {
        self == Self::Deliberation
    }
}

#[derive(Default)]
struct PdfUiProbeState {
    metadata: Mutex<Option<(String, u32)>>,
    rendered: Mutex<Option<(String, u64)>>,
    failed: AtomicU8,
}

#[derive(serde::Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum PdfUiReport {
    Metadata {
        draft_id: String,
        context_revision: Option<u64>,
        total_pages: u32,
    },
    Rendered {
        draft_id: String,
        revision: u64,
    },
    Failed,
}

#[tauri::command]
fn pdf_ui_probe_report(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, commands::DesktopState>,
    report: tauri::State<'_, PdfUiProbeState>,
    input: PdfUiReport,
) -> Result<(), &'static str> {
    if window.label() != "main" {
        return Err("native probe report requires main window");
    }
    match input {
        PdfUiReport::Metadata {
            draft_id,
            context_revision,
            total_pages,
        } => {
            if draft_id.is_empty()
                || draft_id.len() > 256
                || context_revision.is_some()
                || !(2..=1000).contains(&total_pages)
            {
                return Err("invalid PDF metadata proof");
            }
            let storage = state
                .storage()
                .map_err(|_| "PDF probe storage unavailable")?;
            if !storage
                .list_context_drafts(1)
                .map_err(|_| "PDF draft query failed")?
                .is_empty()
            {
                return Err("metadata selection mutated a draft");
            }
            *report
                .metadata
                .lock()
                .map_err(|_| "PDF probe state unavailable")? = Some((draft_id, total_pages));
        }
        PdfUiReport::Rendered { draft_id, revision } => {
            if revision != 0
                || report
                    .metadata
                    .lock()
                    .map_err(|_| "PDF probe state unavailable")?
                    .as_ref()
                    .is_none_or(|(id, _)| id != &draft_id)
            {
                return Err("PDF capture differs from selected metadata");
            }
            *report
                .rendered
                .lock()
                .map_err(|_| "PDF probe state unavailable")? = Some((draft_id, revision));
        }
        PdfUiReport::Failed => {
            report.failed.store(1, Ordering::Release);
        }
    }
    Ok(())
}

async fn pdf_ui_probe(
    app: &tauri::AppHandle,
    window: &tauri::WebviewWindow,
    root: &std::path::Path,
    monitor: &ProbeMonitor,
) -> Result<(), String> {
    monitor.mark("pdf_ui");
    window.set_focus().map_err(|_| "PDF console focus failed")?;
    window.eval(r#"(async()=>{
      const wait=async fn=>{for(let i=0;i<600;i++){const value=fn();if(value)return value;await new Promise(r=>setTimeout(r,50));}throw Error('bounded UI wait');};
      const button=text=>Array.from(document.querySelectorAll('button')).find(el=>el.textContent.trim().includes(text));
      const original=window.__TAURI_INTERNALS__.invoke.bind(window.__TAURI_INTERNALS__);
      window.__TAURI_INTERNALS__.invoke=async(cmd,args,options)=>{const result=await original(cmd,args,options);
        if(cmd==='prepare_pdf_range_capture'&&result)await original('pdf_ui_probe_report',{input:{kind:'metadata',draftId:result.draftId,contextRevision:result.contextRevision,totalPages:result.totalPages}});
        if(cmd==='apply_pdf_range_capture')window.__pdfCaptureResult=result;
        return result;
      };
      try{
        (await wait(()=>button('새 심의 시작'))).click();
        const question=await wait(()=>document.querySelector('#question-draft'));
        Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype,'value').set.call(question,'Selected page approval');question.dispatchEvent(new Event('input',{bubbles:true}));
        (await wait(()=>button('자료 보기'))).click();
        (await wait(()=>button('페이지 범위를 정할 PDF 선택'))).click();
        const fields=await wait(()=>{const inputs=document.querySelectorAll('fieldset input[type=number]');return inputs.length===2?inputs:null});
        for(const field of fields){Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value').set.call(field,'2');field.dispatchEvent(new Event('input',{bubbles:true}));}
        (await wait(()=>{const el=button('이 페이지 범위만 접수');return el&&!el.disabled?el:null})).click();
        const result=await wait(()=>window.__pdfCaptureResult);
        await wait(()=>document.querySelector('.source-row.source-captured')&&!document.querySelector('fieldset input[type=number]'));
        if(document.querySelector('#question-draft').value!=='Selected page approval')throw Error('draft question changed');
        await original('pdf_ui_probe_report',{input:{kind:'rendered',draftId:result.draftId,revision:result.revision}});
      }catch{await original('pdf_ui_probe_report',{input:{kind:'failed'}});}
    })()"#).map_err(|_| "PDF WebView script failed")?;
    let started = Instant::now();
    loop {
        let report = app.state::<PdfUiProbeState>();
        if report.failed.load(Ordering::Acquire) != 0 {
            return Err("PDF rendered native flow failed".into());
        }
        let rendered = report
            .rendered
            .lock()
            .map_err(|_| "PDF probe state unavailable")?
            .clone();
        if let Some((draft_id, revision)) = rendered {
            let storage = app.state::<commands::DesktopState>().storage()?;
            let draft = storage
                .load_context_draft(&draft_id)
                .map_err(|_| "PDF persisted draft failed")?
                .ok_or("PDF persisted draft missing")?;
            if draft.revision != revision
                || draft.manifest.content.sources.len() != 1
                || storage
                    .has_queued_live_runs()
                    .map_err(|_| "PDF queue check failed")?
            {
                return Err("PDF draft or no-dispatch proof failed".into());
            }
            let source = &draft.manifest.content.sources[0];
            if source.included_locators.is_empty()
                || source
                    .included_locators
                    .iter()
                    .any(|locator| locator.page != Some(2))
            {
                return Err("PDF capture includes unapproved page".into());
            }
            let digest = source
                .derived_digest
                .as_ref()
                .ok_or("PDF derived object missing")?;
            let bytes = storage
                .read_source_object(digest)
                .map_err(|_| "PDF derived object unreadable")?;
            let text = std::str::from_utf8(&bytes).map_err(|_| "PDF derived text invalid")?;
            if !text.contains("CONTROLLED PAGE TWO") || text.contains("UNSELECTED PAGE ONE") {
                return Err("PDF immutable selected text differs".into());
            }
            let proof = serde_json::json!({"kind":"native_webview_pdf_capture","draftId":draft_id,"revision":revision,"manifestDigest":draft.manifest.digest,"derivedDigest":digest,"page":2,"metadataOnlyBeforeApply":true,"rendered":true,"modelDispatches":0});
            std::fs::write(
                root.join("pdf-ui-capture-proof.json"),
                serde_json::to_vec_pretty(&proof).map_err(|_| "PDF proof encoding failed")?,
            )
            .map_err(|_| "PDF proof write failed")?;
            println!(
                "native WebView selected PDF page two persisted and rendered without model dispatch"
            );
            return Ok(());
        }
        if started.elapsed() > Duration::from_secs(240) {
            return Err("PDF native UI deadline exceeded".into());
        }
        tauri::async_runtime::spawn_blocking(|| std::thread::sleep(Duration::from_millis(100)))
            .await
            .map_err(|_| "PDF probe poll failed")?;
    }
}

struct ProbeMonitor {
    outcome: AtomicU8,
    phase: Mutex<(&'static str, Instant)>,
    active_request: Mutex<Option<profiles::CancelDeliberationRequestInputDto>>,
    started: Instant,
    lifecycle: commands::AdmissionRequestLifecycle,
}

impl commands::AdmissionRequestLifecycle {
    pub(crate) fn revoke_with_cancellation_lease(
        &self,
    ) -> Option<commands::AdmissionOperationLease> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.terminal {
            self.verification.revoke();
            return None;
        }
        self.operations.fetch_add(1, Ordering::SeqCst);
        state.revoked = true;
        self.verification.revoke();
        Some(commands::AdmissionOperationLease(self.operations.clone()))
    }
}
impl commands::AdmissionRequestRegistry {
    pub(crate) fn supervised_capability(
        &self,
        lifecycle: &commands::AdmissionRequestLifecycle,
        command_id: &str,
        key: &str,
    ) -> Option<commands::AdmissionAuthorityDto> {
        self.requests
            .lock()
            .ok()?
            .iter()
            .find_map(|(token, request)| {
                (Arc::ptr_eq(&request.state, &lifecycle.state)
                    && request.binding.command_id == command_id
                    && request.binding.idempotency_key == key)
                    .then(|| commands::AdmissionAuthorityDto {
                        token: token.clone(),
                        process_epoch: self.epoch.as_ref().clone(),
                    })
            })
    }
}
impl ProbeMonitor {
    fn ensure_running(&self) -> Result<(), String> {
        if self.outcome.load(Ordering::Acquire) != 0 {
            return Err("supervised probe permission revoked".into());
        }
        self.lifecycle
            .check()
            .map_err(|_| "supervised probe permission revoked".into())
    }

    fn revoke_for_timeout(&self) -> Option<commands::AdmissionOperationLease> {
        self.lifecycle.revoke_with_cancellation_lease()
    }
    fn mark_verified(&self, app: &tauri::AppHandle, phase: &'static str, root: &std::path::Path) {
        self.mark(phase);
        let Ok(service) = app.state::<commands::DesktopState>().runtime_service() else {
            return;
        };
        let metrics = service.metrics();
        eprintln!("native verification phase metrics: {phase}; {metrics:?}");
        let value = serde_json::json!({"phase":phase,"elapsedMicros":self.started.elapsed().as_micros(),"calls":metrics.calls,"proofs":metrics.proofs,"reused":metrics.reused,"hashedBytes":metrics.hashed_bytes,"hashMicros":metrics.hash_micros,"subprocessMicros":metrics.subprocess_micros,"queueMicros":metrics.queue_micros,"heldChecks":metrics.held_checks,"heldCheckMicros":metrics.held_check_micros});
        if let Ok(bytes) = serde_json::to_vec_pretty(&value) {
            let _ = std::fs::write(root.join(format!("verification-{phase}.json")), bytes);
        }
    }

    async fn record_application_shutdown(
        &self,
        app: &tauri::AppHandle,
        root: &std::path::Path,
    ) -> bool {
        let deadline = Instant::now() + Duration::from_secs(30);
        let monitored = self.record_terminal_settlement_until(root, deadline).await;
        let state = app.state::<commands::DesktopState>();
        let global = state.freeze_resource_consumers(deadline, None).await;
        let observed_closed = global.is_ok();
        let timed_out = matches!(global, Err(magi_provider::ProviderError::Timeout));
        let evidence = serde_json::json!({
            "monitoredSettled": monitored,
            "artifactConsumersObservedClosed": observed_closed,
            "deadlineReached": timed_out,
            "unknownCleanup": !observed_closed
        });
        let written = serde_json::to_vec_pretty(&evidence)
            .ok()
            .and_then(|bytes| std::fs::write(root.join("application-shutdown.json"), bytes).ok())
            .is_some();
        monitored && observed_closed && written
    }

    async fn record_terminal_settlement_until(
        &self,
        root: &std::path::Path,
        deadline: Instant,
    ) -> bool {
        let mut revoked = matches!(
            self.lifecycle.verification.check(),
            Err(magi_provider::ProviderError::Cancelled)
        );
        while !revoked && Instant::now() < deadline {
            if let Ok(mut state) = self.lifecycle.state.try_lock() {
                state.revoked = true;
                self.lifecycle.verification.revoke();
                revoked = true;
                break;
            }
            if self
                .lifecycle
                .verification
                .wait_for_stream_checkpoint(deadline)
                .await
                .is_err()
            {
                break;
            }
        }
        let provider_wait = if revoked {
            self.lifecycle
                .verification
                .wait_for_settlement(deadline)
                .await
        } else {
            Err(magi_provider::ProviderError::Timeout)
        };
        while self.lifecycle.operations.load(Ordering::SeqCst) != 0 && Instant::now() < deadline {
            if self
                .lifecycle
                .verification
                .wait_for_stream_checkpoint(deadline)
                .await
                .is_err()
            {
                break;
            }
        }
        let snapshot = self.lifecycle.verification.settlement();
        let native_operations = self.lifecycle.operations.load(Ordering::SeqCst);
        let settled = provider_wait.is_ok() && snapshot.is_settled() && native_operations == 0;
        let evidence = serde_json::json!({
            "revoked": revoked, "settled": settled,
            "queuedVerifications": snapshot.queued_verifications,
            "verificationWorkers": snapshot.verification_workers,
            "providerOperations": snapshot.provider_operations,
            "unresolvedCleanup": snapshot.unresolved_cleanup,
            "nativeOperations": native_operations,
            "deadlineReached": !settled && Instant::now() >= deadline
        });
        eprintln!("native terminal settlement: {evidence}");
        let written = serde_json::to_vec_pretty(&evidence)
            .ok()
            .and_then(|bytes| std::fs::write(root.join("terminal-settlement.json"), bytes).ok())
            .is_some();
        settled && written
    }

    fn mark(&self, phase: &'static str) {
        *self.phase.lock().unwrap() = (phase, Instant::now());
        eprintln!(
            "native live probe phase: {phase}; elapsed_micros={}",
            self.started.elapsed().as_micros()
        );
    }
}

fn valid_recovery_run(run: &str) -> bool {
    run.strip_prefix("run-").is_some_and(|digest| {
        digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}

fn main() {
    let configured_purpose = match std::env::var("MAGI_TEST_PROBE_PURPOSE") {
        Ok(value) => Some(value),
        Err(std::env::VarError::NotPresent) => None,
        Err(std::env::VarError::NotUnicode(_)) => panic!("probe purpose must be valid UTF-8"),
    };
    let purpose =
        ProbePurpose::parse(configured_purpose.as_deref()).expect("invalid probe purpose");
    let root = PathBuf::from(
        std::env::var_os("MAGI_TEST_DATA_ROOT").expect("explicit disposable data root required"),
    );
    let recovery_run = if purpose == ProbePurpose::RecoveryCancel {
        let run = std::env::var("MAGI_TEST_CANCEL_RUN_ID").expect("explicit recovery run required");
        assert!(valid_recovery_run(&run), "invalid recovery run reference");
        Some(run)
    } else {
        None
    };
    if purpose == ProbePurpose::Deliberation {
        let model = std::env::var("MAGI_TEST_MODEL_ID").expect("explicit observed model required");
        let mode = std::env::var("MAGI_TEST_MODE_ID")
            .expect("explicit observed mode or explicit empty absent facility required");
        let source_digest = std::env::var("MAGI_TEST_POLICY_SOURCE_DIGEST")
            .expect("explicit approved canonical source digest required");
        assert!(
            source_digest.len() == 64 && source_digest.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "valid approved canonical source digest required"
        );
        assert!(
            !model.is_empty()
                && model.len() <= 256
                && mode.len() <= 128
                && !model.chars().chain(mode.chars()).any(char::is_control),
            "valid explicit observed selection required"
        );
    }
    let selected = if matches!(purpose, ProbePurpose::PdfUi | ProbePurpose::RecoveryCancel) {
        String::new()
    } else {
        std::env::var("MAGI_TEST_CREDENTIAL_HOME").expect("explicit selected authority required")
    };
    if purpose == ProbePurpose::RecoveryCancel {
        assert!(
            root.is_dir() && root.join("state/magi.sqlite").is_file(),
            "existing recovery store required"
        );
    } else {
        assert!(!root.exists(), "probe data root must be fresh");
        std::fs::create_dir_all(&root).expect("create disposable data root");
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))
            .expect("private disposable data root");
    }
    let root = root.canonicalize().expect("canonical disposable data root");
    let mut context = tauri::generate_context!();
    if purpose == ProbePurpose::RecoveryCancel {
        context.config_mut().build.dev_url = None;
        for window in &mut context.config_mut().app.windows {
            window.url =
                tauri::WebviewUrl::External("about:blank".parse().expect("inert recovery URL"));
            window.visible = false;
        }
    } else if cfg!(debug_assertions) {
        let frontend = std::env::var("MAGI_TEST_FRONTEND_URL")
            .expect("explicit child frontend URL required for debug probe");
        assert_eq!(
            frontend, "http://127.0.0.1:1427",
            "probe debug frontend must be its dedicated local server"
        );
        context.config_mut().build.dev_url = Some(frontend.parse().expect("child frontend URL"));
    }
    context.config_mut().identifier = format!("local.magi.probe.p{}", std::process::id());
    context.config_mut().app.app_directories_override = Some(
        tauri::utils::config::AppDirectoriesOverride::Root(root.clone()),
    );
    let monitor = Arc::new(ProbeMonitor {
        outcome: AtomicU8::new(0),
        phase: Mutex::new(("initializing", Instant::now())),
        active_request: Mutex::new(None),
        started: Instant::now(),
        lifecycle: commands::AdmissionRequestLifecycle::until(
            Instant::now() + Duration::from_secs(300),
        ),
    });
    let setup_monitor = monitor.clone();
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            pdf_ui_probe_report,
            crate::pdf_capture::prepare_pdf_range_capture,
            crate::pdf_capture::apply_pdf_range_capture,
            crate::pdf_capture::discard_pdf_range_capture,
            get_console_snapshot,
            crate::commands::load_run_dossier,
            crate::commands::select_context_files,
            crate::commands::select_context_directory,
            crate::preferences::get_console_preferences,
            crate::preferences::save_console_preferences,
            crate::commands::shell_context,
            crate::commands::shell_open_console,
            crate::commands::shell_open_settings,
            crate::commands::shell_close_companion,
            crate::commands::shell_request_exit,
            crate::commands::shell_confirm_exit,
            crate::profiles::list_recent_runs,
            crate::profiles::list_acp_adapters,
            crate::profiles::list_provider_profiles,
            crate::profiles::list_provider_source_scopes,
            crate::profiles::pick_provider_source_directory,
            crate::profiles::add_provider_source_scope,
            crate::profiles::revoke_provider_source_scope,
            crate::profiles::load_active_provider_profile_selection,
            crate::profiles::save_provider_profile,
            crate::profiles::set_active_provider_profile,
            crate::profiles::validate_provider_profile,
            crate::profiles::authenticate_provider_profile,
            crate::profiles::cancel_provider_authentication,
            crate::profiles::refresh_provider_model_catalog,
            crate::profiles::load_provider_catalog,
            crate::profiles::refresh_provider_catalog,
            crate::profiles::select_provider_model,
            crate::profiles::load_core_model_selections,
            crate::profiles::load_core_execution_witnesses,
            crate::profiles::select_core_model,
            crate::profiles::register_deliberation_request,
            crate::profiles::register_clarification_request,
            crate::profiles::start_clarification,
            crate::profiles::cancel_clarification_request,
            crate::profiles::start_deliberation,
            crate::profiles::load_run_clarification,
            crate::commands::create_clarification_draft,
            crate::commands::load_clarification_draft,
            crate::commands::list_clarification_drafts,
            crate::commands::save_clarification_question,
            crate::commands::discard_clarification_draft,
            crate::profiles::start_live_run,
            crate::profiles::get_live_run_snapshot,
            crate::profiles::cancel_live_run,
            crate::profiles::cancel_deliberation,
            crate::profiles::cancel_deliberation_request,
            crate::profiles::list_role_presets,
            crate::profiles::load_active_role_preset_selection,
            crate::profiles::set_active_role_preset,
            crate::profiles::clone_role_preset,
            crate::profiles::save_role_preset
        ])
        .setup(move |app| {
            setup_monitor.mark("setup");
            app.manage(commands::ExitAuthorization::new());
            app.manage(if purpose == ProbePurpose::RecoveryCancel {
                commands::DesktopState::open_quiet(app.handle())
            } else {
                commands::DesktopState::open(app.handle())
            });
            app.manage(PdfUiProbeState::default());
            let handle = app.handle().clone();
            let selected = selected.clone();
            let root = root.clone();
            if purpose == ProbePurpose::Deliberation {
                start_probe_control_bridge(handle.clone(), setup_monitor.clone(), root.clone())?;
            }
            let watchdog_root = root.clone();
            let watchdog_handle = handle.clone();
            let watchdog_monitor = setup_monitor.clone();
            std::thread::spawn(move || {
                let started = Instant::now();
                loop {
                    std::thread::sleep(Duration::from_secs(1));
                    if watchdog_monitor.outcome.load(Ordering::Acquire) != 0 {
                        break;
                    }
                    let (phase, since) = *watchdog_monitor.phase.lock().unwrap();
                    let phase_limit = if phase == "dispatching" { 1800 } else { 300 };
                    if since.elapsed() > Duration::from_secs(phase_limit)
                        || started.elapsed() > Duration::from_secs(2100)
                    {
                        if watchdog_monitor
                            .outcome
                            .compare_exchange(0, 2, Ordering::AcqRel, Ordering::Acquire)
                            .is_ok()
                        {
                            eprintln!("native live probe failed: bounded phase deadline ({phase})");
                            let cleanup = watchdog_monitor.revoke_for_timeout();
                            let cancelled =
                                cancel_probe_request(&watchdog_handle, &watchdog_monitor, cleanup);
                            let _ = std::fs::write(
                                watchdog_root.join("deadline-cancellation.json"),
                                serde_json::to_vec_pretty(&cancelled).unwrap_or_default(),
                            );
                            tauri::async_runtime::block_on(
                                watchdog_monitor
                                    .record_application_shutdown(&watchdog_handle, &watchdog_root),
                            );
                            watchdog_handle.exit(1);
                        }
                        break;
                    }
                }
            });
            let task_monitor = setup_monitor.clone();
            tauri::async_runtime::spawn(async move {
                let result = probe(
                    &handle,
                    selected,
                    root.clone(),
                    purpose,
                    recovery_run,
                    &task_monitor,
                )
                .await;
                let settled = task_monitor
                    .record_application_shutdown(&handle, &root)
                    .await;
                match result {
                    Ok(()) if settled => {
                        if task_monitor
                            .outcome
                            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
                            .is_ok()
                        {
                            handle.exit(0);
                        }
                    }
                    Ok(()) => {
                        task_monitor.outcome.store(2, Ordering::Release);
                        eprintln!(
                            "native live probe failed: terminal settlement remains unresolved"
                        );
                        handle.exit(1);
                    }
                    Err(error) => {
                        task_monitor.outcome.store(2, Ordering::Release);
                        eprintln!("native live probe failed: {error}");
                        handle.exit(1);
                    }
                }
            });
            Ok(())
        })
        .build(context)
        .expect("real native application initialization");
    let exit_code = app.run_return(|app, event| {
        if let tauri::RunEvent::ExitRequested {
            code: None, api, ..
        } = event
            && !app.state::<commands::ExitAuthorization>().is_authorized()
        {
            api.prevent_exit();
        }
    });
    if monitor.outcome.load(Ordering::Acquire) != 1 || exit_code != 0 {
        eprintln!("native live probe failed: event loop ended without verified completion");
        std::process::exit(1);
    }
}

async fn probe(
    app: &tauri::AppHandle,
    selected: String,
    root: PathBuf,
    purpose: ProbePurpose,
    recovery_run: Option<String>,
    monitor: &ProbeMonitor,
) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or("production main window missing")?;
    if purpose == ProbePurpose::RecoveryCancel {
        monitor.mark("recovery_cancelling");
        let run_id = recovery_run.ok_or("explicit recovery run missing")?;
        let storage = app.state::<commands::DesktopState>().storage()?;
        let before = storage
            .get_live_run_snapshot(&run_id, 0)
            .map_err(|_| "recovery authority snapshot")?;
        let before_dispatches = storage
            .load_live_dispatch_projection(&run_id)
            .map_err(|_| "recovery authority dispatch projection")?;
        std::fs::write(root.join("recovery-before-cancellation.json"), serde_json::to_vec_pretty(&serde_json::json!({"runId":run_id,"status":before.status,"revision":before.revision,"cancellation":before.cancellation,"dispatches":before_dispatches})).map_err(|_| "recovery authority encoding")?).map_err(|_| "recovery authority publication")?;
        let cancellation_result =
            profiles::cancel_deliberation(window, app.clone(), app.state(), run_id.clone()).await;
        let cancellation_accepted = cancellation_result.is_ok();
        let storage = app.state::<commands::DesktopState>().storage()?;
        let live = storage
            .get_live_run_snapshot(&run_id, 0)
            .map_err(|_| "recovered cancellation snapshot")?;
        let dispatches = storage
            .load_live_dispatch_projection(&run_id)
            .map_err(|_| "recovered dispatch projection")?;
        std::fs::write(root.join("recovery-cancellation.json"), serde_json::to_vec_pretty(&serde_json::json!({"purpose":"recovery-cancel","cancellationAccepted":cancellation_accepted,"errorCategory":if cancellation_accepted {None} else {Some("production_recovery_cancellation_rejected")},"runId":run_id,"status":live.status,"revision":live.revision,"cancellation":live.cancellation,"dispatches":dispatches,"inferenceAcceptance":false,"renderedUiAcceptance":false})).map_err(|_| "recovery receipt encoding")?).map_err(|_| "recovery receipt publication")?;
        if !cancellation_accepted {
            return Err("production recovery cancellation rejected; post-state preserved".into());
        }
        if live.cancellation.is_none() {
            return Err("durable cancellation receipt missing; post-state preserved".into());
        }
        monitor.mark("recovery_cancelled_or_interrupted");
        return Ok(());
    }
    if purpose == ProbePurpose::PdfUi {
        return pdf_ui_probe(app, &window, &root, monitor).await;
    }
    let mut core_bindings = Vec::new();
    let mut observed_selections = Vec::new();
    monitor
        .lifecycle
        .bind_execution(
            app.state::<commands::DesktopState>()
                .storage()
                .map_err(|_| "execution store unavailable")?,
        )
        .map_err(|_| "execution authority unavailable")?;
    monitor.ensure_running()?;
    let _probe_operation = monitor
        .lifecycle
        .lease()
        .map_err(|_| "supervised probe operation revoked")?;
    for (index, core_id) in magi_domain::CoreId::ALL
        .into_iter()
        .enumerate()
        .take(purpose.profile_count())
    {
        monitor.ensure_running()?;
        let draft = serde_json::from_value(serde_json::json!({"credentialHomePath":selected,"profileId":null,"expectedRevision":null,"displayName":format!("Subscription Review {}", index + 1),"adapterId":"codex-acp"})).map_err(|_| "profile draft encoding")?;
        monitor.mark("saving_profile");
        let profile =
            profiles::save_provider_profile(window.clone(), app.clone(), app.state(), draft)
                .await?;
        monitor.ensure_running()?;
        let profile = serde_json::to_value(profile).map_err(|_| "profile receipt encoding")?;
        let profile_id = profile["providerProfileId"]
            .as_str()
            .ok_or("profile id missing")?
            .to_owned();
        let revision = profile["revision"]
            .as_u64()
            .ok_or("profile revision missing")?;
        monitor.mark("reading_catalog");
        let catalog = profiles::refresh_provider_model_catalog_inner(
            window.clone(),
            app.clone(),
            app.state(),
            profile_id.clone(),
            revision,
            Some(&monitor.lifecycle),
        )
        .await
        .map_err(|e| format!("catalog: {e:?}"))?;
        monitor.ensure_running()?;
        if catalog.schema_version != magi_domain::ARTIFACT_ATTESTED_CATALOG_SCHEMA_VERSION
            || catalog.artifact_set_digest.is_none()
        {
            return Err("catalog lacks verified artifact set authority".into());
        }
        if purpose == ProbePurpose::CatalogDiagnostic {
            std::fs::write(root.join("catalog-diagnostic.json"), serde_json::to_vec_pretty(&serde_json::json!({"purpose":"catalog-diagnostic","providerProfileId":profile_id,"profileRevision":revision,"catalog":catalog})).map_err(|_| "catalog diagnostic encoding")?).map_err(|_| "catalog diagnostic output")?;
            monitor.mark("catalog_diagnostic_completed");
            return Ok(());
        }
        let requested =
            std::env::var("MAGI_TEST_MODEL_ID").map_err(|_| "explicit observed model required")?;
        let model = catalog
            .models
            .iter()
            .find(|model| model.model_id == requested)
            .ok_or("explicit model is not in actual catalog")?;
        let facility = catalog
            .negotiated_modes
            .as_ref()
            .ok_or("actual mode evidence missing")?;
        let requested_mode =
            std::env::var("MAGI_TEST_MODE_ID").map_err(|_| "explicit observed mode required")?;
        let mode_id = if facility.modes.is_empty() {
            if !requested_mode.is_empty() {
                return Err("mode requested for attested absent facility".into());
            }
            None
        } else {
            Some(
                facility
                    .modes
                    .iter()
                    .find(|mode| mode.mode_id == requested_mode)
                    .ok_or("explicit mode is not in actual catalog")?
                    .mode_id
                    .clone(),
            )
        };
        let selection_input = serde_json::from_value(serde_json::json!({"providerProfileId":profile_id,"profileRevision":revision,"catalogSnapshotId":catalog.catalog_snapshot_id,"catalogDigest":catalog.catalog_digest,"modelId":model.model_id,"modeId":mode_id,"expectedSelectionRevision":null})).map_err(|_| "model selection input encoding")?;
        let selected_model =
            profiles::select_provider_model(window.clone(), app.state(), selection_input)
                .map_err(|_| "explicit provider model selection")?;
        let selection_input = serde_json::from_value(serde_json::json!({"coreId":core_id,"providerProfileId":profile_id,"profileRevision":revision,"modelSelectionRevision":selected_model.selection_revision,"expectedSelectionRevision":null})).map_err(|_| "core selection input encoding")?;
        let selected_core =
            profiles::select_core_model(window.clone(), app.state(), selection_input)
                .map_err(|_| "independent core selection")?;
        observed_selections.push(serde_json::json!({"coreId":core_id,"providerProfileId":profile_id,"catalogSnapshotId":catalog.catalog_snapshot_id,"artifactSetDigest":catalog.artifact_set_digest,"acpExecutableSha256":catalog.adapter_digest,"modelId":model.model_id,"modeId":mode_id,"modelPolicy":"explicit_environment_member","modePolicy":"explicit_environment_member"}));
        core_bindings.push(magi_storage::CoreBindingReference {
            core_id,
            provider_profile_id: profile_id,
            profile_revision: revision,
            model_selection_revision: selected_model.selection_revision,
            core_selection_revision: selected_core.selection_revision,
        });
    }
    if core_bindings
        .iter()
        .map(|binding| &binding.provider_profile_id)
        .collect::<std::collections::BTreeSet<_>>()
        .len()
        != 3
    {
        return Err("distinct profile routing required".into());
    }
    std::fs::write(
        root.join("observed-selections.json"),
        serde_json::to_vec_pretty(&observed_selections)
            .map_err(|_| "observed selection encoding")?,
    )
    .map_err(|_| "observed selection output")?;
    if !purpose.permits_admission() {
        return Err("catalog diagnostic cannot admit a deliberation".into());
    }
    let witnesses = profiles::load_core_execution_witnesses(
        window.clone(),
        app.state(),
        serde_json::from_value(serde_json::json!({"coreBindings":core_bindings}))
            .map_err(|_| "witness input encoding")?,
    )
    .map_err(|_| "fresh execution witness unavailable")?;
    std::fs::write(
        root.join("core-execution-witnesses.json"),
        serde_json::to_vec_pretty(&witnesses).map_err(|_| "witness output encoding")?,
    )
    .map_err(|_| "witness output")?;
    let storage = app.state::<commands::DesktopState>().storage()?;
    let role = storage
        .list_role_presets(100)
        .map_err(|_| "role list")?
        .into_iter()
        .next()
        .ok_or("factory role missing")?;
    let command_id = uuid::Uuid::new_v4().to_string();
    monitor.ensure_running()?;
    monitor.mark("capturing_sources");
    let draft_id = uuid::Uuid::new_v4().to_string();
    let capture_storage = storage.clone();
    let capture_draft = draft_id.clone();
    let approved_source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../docs/SECURITY.md");
    let expected_source_digest = std::env::var("MAGI_TEST_POLICY_SOURCE_DIGEST")
        .map_err(|_| "explicit approved canonical source digest required")?;
    if expected_source_digest.len() != 64
        || !expected_source_digest
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("invalid approved canonical source digest".into());
    }
    let capture_lease = monitor
        .lifecycle
        .lease()
        .map_err(|_| "source capture revoked")?;
    let capture_operation = app
        .state::<commands::DesktopState>()
        .resource_coordinator
        .enter(Some(monitor.lifecycle.verification.clone()))
        .map_err(|_| "source capture resources frozen")?;
    let approved_digest = expected_source_digest.clone();
    let resource_root = app
        .path()
        .resource_dir()
        .map_err(|_| "source capture resources")?;
    let captured = tauri::async_runtime::spawn_blocking(move || {
        let _capture_lease = capture_lease;
        let _capture_operation = capture_operation;
        _capture_operation
            .check()
            .map_err(|_| "source capture resources frozen".to_owned())?;
        let bytes =
            std::fs::read(&approved_source).map_err(|_| "approved canonical source unavailable")?;
        if magi_domain::Digest::from_bytes(&bytes).as_str() != expected_source_digest {
            return Err("approved canonical source changed".to_owned());
        }
        commands::capture_context_files(
            capture_storage,
            capture_draft,
            None,
            vec![approved_source],
            &resource_root,
        )
    })
    .await
    .map_err(|_| "source capture worker")??;
    monitor.ensure_running()?;
    let captured = serde_json::to_value(captured).map_err(|_| "source capture summary")?;
    if captured["sources"].as_array().map(Vec::len) != Some(1)
        || captured["sources"][0]["status"] != "captured"
        || captured["sources"][0]["digest"].as_str() != Some(approved_digest.as_str())
    {
        return Err("the approved canonical source was not fully captured".into());
    }
    let context_revision = captured["revision"]
        .as_u64()
        .ok_or("source capture revision")?;
    let draft = storage
        .load_context_draft(&draft_id)
        .map_err(|_| "source capture durable read")?
        .ok_or("source capture durable draft missing")?;
    let source = draft
        .manifest
        .content
        .sources
        .first()
        .ok_or("source capture durable entry missing")?;
    if draft.revision != context_revision
        || draft.manifest.content.sources.len() != 1
        || source
            .object_digest
            .as_ref()
            .map(magi_domain::Digest::as_str)
            != captured["sources"][0]["digest"].as_str()
        || source.included_locators.is_empty()
    {
        return Err("source capture durable authority mismatch".into());
    }
    let capture_evidence = serde_json::json!({"capture":captured,"canonicalSourceDigest":source.object_digest,"manifestDigest":draft.manifest.digest,"includedLocators":source.included_locators});
    std::fs::write(
        root.join("approved-policy-source-capture.json"),
        serde_json::to_vec_pretty(&capture_evidence)
            .map_err(|_| "source capture evidence encoding")?,
    )
    .map_err(|_| "source capture evidence")?;
    let question = "Using the explicitly captured product security contract as the factual basis, evaluate the explicit-confirmation policy for this desktop product. Users manually select local subscription profiles; inference is performed by the selected external provider, not locally merely because the app is local. Source grants restrict explicitly selected representations. Disclosure includes the question, roles and authorized sources, followed by permitted sharing of reviews and the common proposal. App diagnostics exclude body contents and there is no app-operator LLM gateway. Decide the confirmation contents and when a scope change requires reconfirmation. Treat unknown provider retention/training/deletion policy as an explicit limitation, not an assumed guarantee. Do not assume a file-type distribution, user-test results or jurisdiction-specific compliance. This is an application safeguard policy decision, not a declaration that any particular provider/account is safe for all data. Preserve legitimate meaning-changing essential gaps; distinguish conditional deployment limitations from facts needed to decide the stated application policy.";
    let mut request = serde_json::json!({"commandId":command_id,"idempotencyKey":command_id,"question":question,"contextDraftId":draft_id,"contextRevision":context_revision,"coreBindings":core_bindings,"rolePresetId":role.preset_id,"roleRevision":role.revision,"disclosureConfirmed":true});
    monitor.ensure_running()?;
    *monitor.active_request.lock().map_err(|_| "request reference lock")? = Some(serde_json::from_value(serde_json::json!({"request":request,"commandId":uuid::Uuid::new_v4().to_string(),"idempotencyKey":uuid::Uuid::new_v4().to_string()})).map_err(|_| "cancellation intent encoding")?);
    let registration = profiles::register_deliberation_request_inner(
        window.clone(),
        app.clone(),
        app.state(),
        serde_json::from_value(request.clone()).map_err(|_| "registration request encoding")?,
        Some(&monitor.lifecycle),
    )
    .await
    .map_err(|_| "request registration")?;
    monitor.ensure_running()?;
    let registration = serde_json::to_value(registration).map_err(|_| "registration output")?;
    if registration["kind"] != "registered" {
        return Err("fresh probe request unexpectedly replayed".into());
    }
    let authority = &registration["admissionAuthority"];
    request["admissionAuthority"] =
        serde_json::json!({"token":authority["token"],"processEpoch":authority["processEpoch"]});
    let input =
        serde_json::from_value(request.clone()).map_err(|_| "deliberation input encoding")?;
    request
        .as_object_mut()
        .ok_or("request object")?
        .remove("admissionAuthority");
    if let Some(reference) = monitor
        .active_request
        .lock()
        .map_err(|_| "request reference lock")?
        .as_mut()
    {
        reference.admission_authority = Some(serde_json::from_value(serde_json::json!({"token":authority["token"],"processEpoch":authority["processEpoch"]})).map_err(|_| "cancellation authority encoding")?);
    }
    monitor.ensure_running()?;
    monitor.mark_verified(app, "admitting", &root);
    let receipt = profiles::start_deliberation(window.clone(), app.clone(), app.state(), input)
        .await
        .map_err(|e| format!("admission: {e:?}"))?;
    let receipt = serde_json::to_value(receipt).map_err(|_| "admission receipt")?;
    let run_id = receipt["runId"].as_str().ok_or("run id missing")?;
    std::fs::write(root.join("admission.json"), serde_json::to_vec_pretty(&serde_json::json!({"runId":run_id,"observedSelections":observed_selections,"coreBindings":core_bindings})).map_err(|_| "admission evidence encoding")?).map_err(|_| "admission evidence output")?;
    eprintln!("native live probe: production deliberation admitted; run {run_id}");
    monitor.mark_verified(app, "dispatching", &root);
    let started = Instant::now();
    loop {
        let dossier = storage
            .load_run_dossier(run_id)
            .map_err(|_| "durable dossier read")?;
        if dossier.snapshot.submitted_ballot_count < 3
            && dossier.snapshot.ballots_revealed.is_some()
        {
            return Err("partial sealed ballots exposed".into());
        }
        if matches!(
            dossier.snapshot.run.status,
            magi_domain::RunStatus::Paused {
                reason: magi_domain::PauseReason::NeedsInput,
                ..
            }
        ) {
            let clarification = crate::run_projection::clarification_view(&dossier.snapshot)
                .map_err(|_| "paused clarification integrity")?
                .ok_or("paused clarification missing")?;
            std::fs::write(
                root.join("paused-clarification.json"),
                serde_json::to_vec_pretty(&clarification).map_err(|_| "clarification encoding")?,
            )
            .map_err(|_| "clarification evidence")?;
            return Err(
                "production run paused awaiting essential information; consensus not completed"
                    .into(),
            );
        }
        if dossier.snapshot.run.status.is_terminal() {
            if !matches!(
                dossier.snapshot.run.status,
                magi_domain::RunStatus::Completed { .. }
            ) {
                return Err(format!(
                    "production run ended: {:?}",
                    dossier.snapshot.run.status
                ));
            }
            if dossier.snapshot.ballots_revealed.as_ref().map(Vec::len) != Some(3)
                || dossier.decision_dossier_digest.is_none()
            {
                return Err("completed durable ten-turn consensus invariant failed".into());
            }
            let proposal = dossier
                .snapshot
                .proposal
                .as_ref()
                .ok_or("completed proposal missing")?;
            let ballots = dossier
                .snapshot
                .ballots_revealed
                .as_ref()
                .ok_or("ballots missing")?;
            let mut cores = std::collections::BTreeSet::new();
            for ballot in ballots {
                ballot
                    .validate_for(
                        run_id,
                        &dossier.snapshot.run.input_digest,
                        dossier.snapshot.run.generation,
                        proposal,
                    )
                    .map_err(|_| "ballot binding mismatch")?;
                if !cores.insert(format!("{:?}", ballot.core_id)) {
                    return Err("duplicate ballot core".into());
                }
            }
            let execution = storage
                .load_live_dispatch_projection(run_id)
                .map_err(|_| "durable live dispatch projection")?;
            if execution.run_revision != dossier.snapshot.run.revision
                || execution.input_digest != dossier.snapshot.run.input_digest
                || execution.run_generation != dossier.snapshot.run.generation
                || execution.dispatches.len() != 10
            {
                return Err("execution projection snapshot mismatch".into());
            }
            for dispatch in &execution.dispatches {
                let reference = core_bindings
                    .iter()
                    .find(|reference| reference.core_id == dispatch.binding_core_id)
                    .ok_or("execution core reference missing")?;
                let frozen = dossier
                    .snapshot
                    .input
                    .role_set
                    .roles
                    .iter()
                    .find(|role| role.core_id == dispatch.binding_core_id)
                    .and_then(|role| role.catalog_binding.as_ref())
                    .ok_or("execution frozen binding missing")?;
                if dispatch.provider_profile_id != reference.provider_profile_id
                    || dispatch.profile_revision != reference.profile_revision
                    || dispatch.binding_digest != frozen.binding_digest
                {
                    return Err("execution routed profile mismatch".into());
                }
            }
            let frozen_refs = dossier
                .snapshot
                .input
                .role_set
                .frozen_core_selections
                .as_ref()
                .ok_or("frozen core selections missing")?;
            for reference in &core_bindings {
                if !frozen_refs.iter().any(|frozen| {
                    frozen.core_id == reference.core_id
                        && frozen.provider_profile_id == reference.provider_profile_id
                        && frozen.profile_revision == reference.profile_revision
                        && frozen.model_selection_revision == reference.model_selection_revision
                        && frozen.core_selection_revision == reference.core_selection_revision
                }) {
                    return Err("frozen core selection mismatch".into());
                }
            }
            if execution.dispatches.iter().any(|dispatch| {
                dispatch.state != magi_storage::LiveDispatchState::Settled
                    || dispatch.result_ref.is_none()
            }) {
                return Err("unsettled production dispatch".into());
            }
            let reader = magi_storage::StorageReader::open_read_only(
                app.path()
                    .app_data_dir()
                    .map_err(|_| "app data authority")?,
            )
            .map_err(|_| "reopen durable reader")?;
            let reopened = reader
                .load_run_dossier(run_id)
                .map_err(|_| "reopened durable dossier")?;
            if reopened != dossier {
                return Err("reopened durable projection mismatch".into());
            }
            if reader
                .load_live_dispatch_projection(run_id)
                .map_err(|_| "reopened live dispatch projection")?
                != execution
            {
                return Err("reopened execution projection mismatch".into());
            }
            std::fs::write(
                root.join("completed-live-dispatches.json"),
                serde_json::to_vec_pretty(&execution).map_err(|_| "execution evidence encoding")?,
            )
            .map_err(|_| "execution evidence output")?;
            let output = serde_json::to_vec_pretty(&dossier).map_err(|_| "dossier encoding")?;
            std::fs::write(root.join("completed-dossier.json"), output)
                .map_err(|_| "durable evidence output")?;
            let public = commands::load_run_dossier(window.clone(), app.state(), run_id.to_owned())
                .map_err(|_| "production UI dossier command")?;
            let public = serde_json::to_value(public).map_err(|_| "UI dossier value encoding")?;
            std::fs::write(
                root.join("completed-ui-dossier.json"),
                serde_json::to_vec_pretty(&public).map_err(|_| "UI dossier encoding")?,
            )
            .map_err(|_| "UI dossier evidence output")?;
            let public_run_id = public
                .get("runId")
                .and_then(serde_json::Value::as_str)
                .ok_or("UI dossier run identity missing")?;
            if public_run_id != run_id {
                return Err("UI dossier run identity mismatch".into());
            }
            if public["status"] != "completed"
                || public["votes"].as_array().map(Vec::len) != Some(3)
                || public["proposal"]["body"] != proposal.body
            {
                return Err("UI dossier does not match completed durable consensus".into());
            }
            for ballot in ballots {
                let expected_core =
                    serde_json::to_value(ballot.core_id).map_err(|_| "ballot core encoding")?;
                let expected_choice =
                    serde_json::to_value(ballot.vote).map_err(|_| "ballot choice encoding")?;
                if !public["votes"]
                    .as_array()
                    .ok_or("UI votes missing")?
                    .iter()
                    .any(|vote| {
                        vote["coreId"] == expected_core
                            && vote["choice"] == expected_choice
                            && vote["rationale"] == ballot.rationale
                    })
                {
                    return Err("UI ballot does not match frozen durable ballot".into());
                }
            }
            if let magi_domain::RunStatus::Completed { outcome } = &dossier.snapshot.run.status
                && public["outcome"]
                    != serde_json::to_value(outcome).map_err(|_| "durable outcome encoding")?
            {
                return Err("UI outcome does not match durable consensus".into());
            }
            eprintln!(
                "native live probe: durable ten-turn run and three revealed ballots verified; run {run_id}"
            );
            monitor.mark_verified(app, "verified", &root);
            return Ok(());
        }
        if started.elapsed() > Duration::from_secs(1800) {
            return Err("bounded production deliberation deadline".into());
        }
        tauri::async_runtime::spawn_blocking(|| std::thread::sleep(Duration::from_millis(250)))
            .await
            .map_err(|_| "poll worker")?;
    }
}

#[cfg(test)]
mod purpose_tests {
    use super::{PdfUiReport, ProbePurpose, valid_recovery_run};

    #[test]
    fn catalog_diagnostic_is_single_profile_and_cannot_admit_inference() {
        let purpose = ProbePurpose::parse(Some("catalog-diagnostic")).unwrap();
        assert_eq!(purpose.profile_count(), 1);
        assert!(!purpose.permits_admission());
        let normal = ProbePurpose::parse(None).unwrap();
        assert_eq!(normal.profile_count(), 3);
        assert!(normal.permits_admission());
        for invalid in ["", "catalog", "automatic", "42"] {
            assert!(ProbePurpose::parse(Some(invalid)).is_err());
        }
    }

    #[test]
    fn recovery_cancellation_has_no_profiles_or_admission_and_requires_exact_run_reference() {
        let purpose = ProbePurpose::parse(Some("recovery-cancel")).unwrap();
        assert_eq!(purpose.profile_count(), 0);
        assert!(!purpose.permits_admission());
        assert!(valid_recovery_run(&format!("run-{}", "a".repeat(64))));
        for invalid in ["", "run-private_canary", "run-../../private", "automatic"] {
            assert!(!valid_recovery_run(invalid));
        }
    }

    #[test]
    fn pdf_ui_purpose_has_no_profiles_or_admission_and_closed_reports() {
        let purpose = ProbePurpose::parse(Some("pdf-ui")).unwrap();
        assert_eq!(purpose.profile_count(), 0);
        assert!(!purpose.permits_admission());
        assert!(serde_json::from_value::<PdfUiReport>(serde_json::json!({"kind":"metadata","draftId":"draft","contextRevision":null,"totalPages":2})).is_ok());
        for value in [
            serde_json::json!({"kind":"metadata","draftId":"draft","contextRevision":null,"totalPages":2,"path":"private_canary"}),
            serde_json::json!({"kind":"private_canary"}),
            serde_json::json!({"kind":"rendered","draftId":"draft","revision":-1}),
        ] {
            assert!(serde_json::from_value::<PdfUiReport>(value).is_err());
        }
    }
}

#[tauri::command]
async fn get_console_snapshot(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::commands::DesktopState>,
) -> Result<crate::commands::ConsoleSnapshot, String> {
    crate::commands::verified_console_snapshot(window, app, state).await
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ProbeControlMessage {
    action: ProbeControlAction,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum ProbeControlAction {
    Cancel,
}

fn cancel_probe_request(
    app: &tauri::AppHandle,
    monitor: &ProbeMonitor,
    cleanup: Option<commands::AdmissionOperationLease>,
) -> serde_json::Value {
    cancel_probe_request_with_cleanup(
        app,
        monitor,
        Instant::now() + Duration::from_secs(60),
        cleanup,
    )
}

fn cancel_probe_request_until(
    app: &tauri::AppHandle,
    monitor: &ProbeMonitor,
    deadline: Instant,
) -> serde_json::Value {
    cancel_probe_request_with_cleanup(app, monitor, deadline, None)
}

fn cancel_probe_request_with_cleanup(
    app: &tauri::AppHandle,
    monitor: &ProbeMonitor,
    deadline: Instant,
    cleanup: Option<commands::AdmissionOperationLease>,
) -> serde_json::Value {
    let reference = match monitor.active_request.try_lock() {
        Ok(value) => value.clone(),
        Err(_) => return serde_json::json!({"accepted":false,"code":"control_busy"}),
    };
    let Some(reference) = reference else {
        return serde_json::json!({"accepted":false,"code":"request_not_registered"});
    };
    let Some(window) = app.get_webview_window("main") else {
        return serde_json::json!({"accepted":false,"code":"window_unavailable"});
    };
    let app = app.clone();
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    let lifecycle = monitor.lifecycle.clone();
    tauri::async_runtime::spawn(async move {
        let _cleanup = cleanup.or_else(|| lifecycle.revoke_with_cancellation_lease());
        let mut reference = reference;
        if reference.admission_authority.is_none() {
            reference.admission_authority = app
                .state::<commands::DesktopState>()
                .admission_requests
                .supervised_capability(
                    &lifecycle,
                    &reference.request.command_id,
                    &reference.request.idempotency_key,
                );
        }
        let accepted =
            profiles::cancel_deliberation_request(window, app.clone(), app.state(), reference)
                .await
                .is_ok();
        let _ = sender.send(accepted);
    });
    wait_probe_cancellation(receiver, deadline)
}

fn wait_probe_cancellation(
    receiver: std::sync::mpsc::Receiver<bool>,
    deadline: Instant,
) -> serde_json::Value {
    match receiver.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        Ok(true) => {
            serde_json::json!({"accepted":true,"authority":"production_request_cancellation"})
        }
        Ok(false) => {
            serde_json::json!({"accepted":false,"code":"production_cancellation_rejected"})
        }
        Err(_) => {
            serde_json::json!({"accepted":false,"code":"cancellation_pending"})
        }
    }
}

fn serve_probe_control(
    mut stream: std::os::unix::net::UnixStream,
    cancellation: impl FnOnce(Instant) -> serde_json::Value,
) -> std::io::Result<()> {
    use std::{
        io::{Read, Write},
        os::fd::AsRawFd,
    };
    let mut uid = 0;
    let mut gid = 0;
    if unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) } != 0
        || uid != unsafe { libc::geteuid() }
    {
        return Err(std::io::Error::other("Control peer authority rejected"));
    }
    stream.set_nonblocking(false)?;
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut bytes = Vec::new();
    let remaining = || {
        deadline
            .checked_duration_since(Instant::now())
            .filter(|value| !value.is_zero())
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::TimedOut, "Control deadline expired")
            })
    };
    while bytes.len() < 257 {
        stream.set_read_timeout(Some(remaining()?))?;
        let mut buffer = [0_u8; 257];
        let count = stream.read(&mut buffer[..257 - bytes.len()])?;
        if count == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
    remaining()?;
    let message = if bytes.len() <= 256 {
        serde_json::from_slice::<ProbeControlMessage>(&bytes).ok()
    } else {
        None
    };
    let result = match message {
        Some(ProbeControlMessage {
            action: ProbeControlAction::Cancel,
        }) => cancellation(deadline),
        None => serde_json::json!({"accepted":false,"code":"invalid_control_message"}),
    };
    let output = serde_json::to_vec(&result)
        .map_err(|_| std::io::Error::other("Control result encoding failed"))?;
    let mut written = 0;
    while written < output.len() {
        stream.set_write_timeout(Some(remaining()?))?;
        let count = stream.write(&output[written..])?;
        if count == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::WriteZero,
                "Control response unavailable",
            ));
        }
        written += count;
    }
    Ok(())
}

fn start_probe_control_bridge(
    app: tauri::AppHandle,
    monitor: Arc<ProbeMonitor>,
    root: PathBuf,
) -> std::io::Result<()> {
    use std::os::unix::net::UnixListener;
    let directory = PathBuf::from("/tmp")
        .canonicalize()?
        .join(format!("magi-control-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir(&directory)?;
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
    let socket = directory.join("cancel.sock");
    let listener = UnixListener::bind(&socket)?;
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;
    std::fs::write(root.join("control-reference.json"), serde_json::to_vec_pretty(&serde_json::json!({"socketPath":socket,"processId":std::process::id(),"action":"cancel","scope":"owned_live_process"})).map_err(|_| std::io::Error::other("Control reference encoding failed"))?)?;
    std::thread::spawn(move || {
        while monitor.outcome.load(Ordering::Acquire) == 0 {
            match listener.accept() {
                Ok((stream, _)) => {
                    let _ = serve_probe_control(stream, |deadline| {
                        cancel_probe_request_until(&app, &monitor, deadline)
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(25))
                }
                Err(_) => break,
            }
        }
        drop(listener);
        let _ = std::fs::remove_file(socket);
        let _ = std::fs::remove_dir(directory);
    });
    Ok(())
}

#[cfg(test)]
mod probe_control_bridge_tests {
    fn native_binding(id: &str) -> magi_storage::AdmissionRequestBinding {
        magi_storage::AdmissionRequestBinding {
            command_id: id.into(),
            idempotency_key: id.into(),
            intent_digest: magi_domain::Digest::from_bytes(id.as_bytes()),
        }
    }
    #[test]
    fn supervised_timeout_before_issuance_cannot_mint_fresh_authority() {
        let registry = commands::AdmissionRequestRegistry::default();
        let lifecycle =
            commands::AdmissionRequestLifecycle::until(Instant::now() + Duration::from_secs(30));
        let cleanup = lifecycle.revoke_with_cancellation_lease().unwrap();
        assert!(
            registry
                .issue_from_lifecycle(native_binding("supervised"), Some(&lifecycle), None)
                .is_err()
        );
        assert!(registry.requests.lock().unwrap().is_empty());
        assert!(
            lifecycle
                .verification
                .with_deadline(Instant::now() + Duration::from_secs(60))
                .check()
                .is_err()
        );
        assert_eq!(lifecycle.operations.load(Ordering::SeqCst), 1);
        drop(cleanup);
        assert_eq!(lifecycle.operations.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn supervised_issued_timeout_holds_cleanup_against_retirement() {
        let registry = commands::AdmissionRequestRegistry::default();
        let binding = native_binding("supervised-issued");
        let lifecycle =
            commands::AdmissionRequestLifecycle::until(Instant::now() + Duration::from_secs(30));
        let issued = registry
            .issue_from_lifecycle(binding.clone(), Some(&lifecycle), None)
            .unwrap();
        let capability = registry
            .supervised_capability(&lifecycle, &binding.command_id, &binding.idempotency_key)
            .unwrap();
        let authority = registry.resolve(&capability, &binding).unwrap();
        let cleanup = lifecycle.revoke_with_cancellation_lease().unwrap();
        authority.state.lock().unwrap().terminal = true;
        registry
            .issue(native_binding("retire-trigger-held"))
            .unwrap();
        assert!(
            registry
                .requests
                .lock()
                .unwrap()
                .contains_key(&issued.token)
        );
        assert!(authority.check().is_err());
        assert!(authority.effect_request().is_err());
        assert_eq!(issued.expires_at, lifecycle.expires_at);
        drop(cleanup);
        registry
            .issue(native_binding("retire-trigger-settled"))
            .unwrap();
        assert!(
            !registry
                .requests
                .lock()
                .unwrap()
                .contains_key(&issued.token)
        );
        assert!(lifecycle.revoke_with_cancellation_lease().is_none());
        assert_eq!(lifecycle.operations.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn supervised_original_deadline_and_admitted_lifetime_are_distinct() {
        let registry = commands::AdmissionRequestRegistry::default();
        let expired =
            commands::AdmissionRequestLifecycle::until(Instant::now() - Duration::from_millis(1));
        assert!(
            registry
                .issue_from_lifecycle(native_binding("expired-supervised"), Some(&expired), None)
                .is_err()
        );
        let lifecycle =
            commands::AdmissionRequestLifecycle::until(Instant::now() + Duration::from_secs(30));
        let binding = native_binding("admitted-supervised");
        registry
            .issue_from_lifecycle(binding.clone(), Some(&lifecycle), None)
            .unwrap();
        lifecycle.state.lock().unwrap().admitted_run = Some("owned-run".into());
        assert!(lifecycle.check().is_ok());
        let effects: Vec<_> = (0..3)
            .map(|_| {
                lifecycle
                    .verification
                    .with_deadline(Instant::now() + Duration::from_secs(60))
            })
            .collect();
        let cleanup = lifecycle.revoke_with_cancellation_lease().unwrap();
        assert!(lifecycle.check().is_err());
        assert!(effects.iter().all(|effect| effect.check().is_err()));
        drop(cleanup);
    }

    #[test]
    fn contested_authority_does_not_extend_control_processing_deadline_or_claim_revocation() {
        let registry = super::commands::AdmissionRequestRegistry::default();
        let authority = registry.reference("contested-control").unwrap();
        let permission = authority.state.lock().unwrap();
        let pending_authority = authority.clone();
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        let worker = std::thread::spawn(move || {
            pending_authority.revoke();
            let _ = sender.send(true);
        });
        let started = std::time::Instant::now();
        let result = super::wait_probe_cancellation(
            receiver,
            started + std::time::Duration::from_millis(80),
        );
        assert_eq!(
            result,
            serde_json::json!({"accepted":false,"code":"cancellation_pending"})
        );
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
        assert!(!permission.revoked);
        drop(permission);
        worker.join().unwrap();
        assert!(authority.state.lock().unwrap().revoked);
    }
    #[test]
    fn slow_fragmented_control_cannot_extend_absolute_connection_deadline() {
        use std::io::Write;
        let (server, mut client) = std::os::unix::net::UnixStream::pair().unwrap();
        let started = std::time::Instant::now();
        let worker = std::thread::spawn(move || {
            super::serve_probe_control(server, |_| panic!("Incomplete request cannot cancel"))
        });
        for _ in 0..5 {
            if client.write_all(b" ").is_err() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(800));
        }
        assert!(worker.join().unwrap().is_err());
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }
    use super::*;
    use std::{
        os::unix::net::UnixListener,
        process::{Command, Stdio},
        sync::atomic::AtomicUsize,
    };

    #[test]
    fn owned_cross_process_delayed_fragmented_cancel_reaches_closed_control_authority() {
        for (message, expected) in [
            ("{\"action\":\"cancel\"}", true),
            (
                "{\"action\":\"cancel\",\"private_canary\":\"private_value_canary\"}",
                false,
            ),
            ("{\"action\":\"other\"}", false),
        ] {
            let directory = PathBuf::from("/tmp").canonicalize().unwrap().join(format!(
                "magi-control-test-{}",
                uuid::Uuid::new_v4().simple()
            ));
            std::fs::create_dir(&directory).unwrap();
            std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).unwrap();
            let path = directory.join("cancel.sock");
            let listener = UnixListener::bind(&path).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
            listener.set_nonblocking(true).unwrap();
            let calls = Arc::new(AtomicUsize::new(0));
            let observed = calls.clone();
            let server = std::thread::spawn(move || {
                let stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(5))
                        }
                        Err(_) => panic!("Owned control accept failed"),
                    }
                };
                serve_probe_control(stream, |_| {
                    observed.fetch_add(1, Ordering::SeqCst);
                    serde_json::json!({"accepted":true})
                })
            });
            let script = "import socket,sys,time,json
s=socket.socket(socket.AF_UNIX)
s.connect(sys.argv[1])
time.sleep(0.1)
p=sys.argv[2].encode()
s.sendall(p[:5])
time.sleep(0.05)
s.sendall(p[5:])
s.shutdown(socket.SHUT_WR)
data=b''
while True:
 chunk=s.recv(4096)
 if not chunk: break
 data+=chunk
assert b'private_value_canary' not in data and b'private_canary' not in data
assert json.loads(data)['accepted']==(sys.argv[3]=='true')
";
            let status = Command::new("/usr/bin/python3")
                .args([
                    "-c",
                    script,
                    path.to_str().unwrap(),
                    message,
                    if expected { "true" } else { "false" },
                ])
                .env_clear()
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .unwrap();
            assert!(
                status.success(),
                "Owned cross-process control assertion failed"
            );
            assert!(server.join().unwrap().is_ok());
            assert_eq!(calls.load(Ordering::SeqCst), usize::from(expected));
            std::fs::remove_file(path).unwrap();
            std::fs::remove_dir(directory).unwrap();
        }
    }
}

#[cfg(test)]
mod terminal_stream_settlement_tests {
    use super::*;

    async fn contested_admission_state_cannot_extend_settlement_capture_deadline() {
        let monitor = Arc::new(ProbeMonitor {
            outcome: AtomicU8::new(2),
            phase: Mutex::new(("terminal", Instant::now())),
            active_request: Mutex::new(None),
            started: Instant::now(),
            lifecycle: commands::AdmissionRequestLifecycle::until(
                Instant::now() + Duration::from_secs(2),
            ),
        });
        let provider_lease = monitor
            .lifecycle
            .verification
            .track_stream_operation()
            .unwrap();
        let native_lease = monitor.lifecycle.lease().unwrap();
        let (locked_sender, locked) = std::sync::mpsc::channel();
        let (release, held) = std::sync::mpsc::channel();
        let worker_monitor = monitor.clone();
        let worker = std::thread::spawn(move || {
            let _provider_lease = provider_lease;
            let _native_lease = native_lease;
            let _guard = worker_monitor.lifecycle.state.lock().unwrap();
            locked_sender.send(()).unwrap();
            held.recv().unwrap();
        });
        locked.recv_timeout(Duration::from_secs(1)).unwrap();
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../.local/verification")
            .join(format!(
                "contested-terminal-settlement-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
        std::fs::create_dir_all(&root).unwrap();
        let started = Instant::now();
        assert!(
            !monitor
                .record_terminal_settlement_until(&root, started + Duration::from_millis(20))
                .await
        );
        assert!(started.elapsed() < Duration::from_millis(250));
        let evidence: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.join("terminal-settlement.json")).unwrap())
                .unwrap();
        assert_eq!(evidence["settled"], false);
        assert_eq!(evidence["revoked"], false);
        assert_eq!(evidence["providerOperations"], 1);
        assert_eq!(evidence["nativeOperations"], 1);
        release.send(()).unwrap();
        worker.join().unwrap();
        assert!(
            monitor
                .record_terminal_settlement_until(&root, Instant::now() + Duration::from_secs(1))
                .await
        );
        assert!(monitor.lifecycle.verification.check().is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    async fn aborted_delivery_caller_cannot_publish_zero_settlement_before_worker_exit() {
        let monitor = ProbeMonitor {
            outcome: AtomicU8::new(2),
            phase: Mutex::new(("terminal", Instant::now())),
            active_request: Mutex::new(None),
            started: Instant::now(),
            lifecycle: commands::AdmissionRequestLifecycle::until(
                Instant::now() + Duration::from_secs(2),
            ),
        };
        let provider_lease = monitor
            .lifecycle
            .verification
            .track_stream_operation()
            .unwrap();
        let native_lease = monitor.lifecycle.revoke_with_cancellation_lease().unwrap();
        let (release, mut held) = tauri::async_runtime::channel::<()>(1);
        let worker = tauri::async_runtime::spawn(async move {
            let _provider_lease = provider_lease;
            let _native_lease = native_lease;
            let _ = held.recv().await;
        });
        let caller = tauri::async_runtime::spawn(async move {
            let _ = worker.await;
        });
        caller.abort();
        let _ = caller.await;
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../.local/verification")
            .join(format!(
                "terminal-stream-settlement-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
        std::fs::create_dir_all(&root).unwrap();
        assert!(
            !monitor
                .record_terminal_settlement_until(&root, Instant::now() + Duration::from_millis(20))
                .await
        );
        let evidence: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.join("terminal-settlement.json")).unwrap())
                .unwrap();
        assert_eq!(evidence["settled"], false);
        assert_eq!(evidence["providerOperations"], 1);
        assert_eq!(evidence["nativeOperations"], 1);
        assert!(monitor.lifecycle.verification.check().is_err());
        release.send(()).await.unwrap();
        assert!(
            monitor
                .record_terminal_settlement_until(&root, Instant::now() + Duration::from_secs(1))
                .await
        );
        let evidence: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.join("terminal-settlement.json")).unwrap())
                .unwrap();
        assert_eq!(evidence["settled"], true);
        for field in [
            "queuedVerifications",
            "verificationWorkers",
            "providerOperations",
            "unresolvedCleanup",
            "nativeOperations",
        ] {
            assert_eq!(evidence[field], 0);
        }
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn contested_admission_state_capture_control() {
        tauri::async_runtime::block_on(
            contested_admission_state_cannot_extend_settlement_capture_deadline(),
        );
    }
    #[test]
    fn aborted_delivery_caller_settlement_control() {
        tauri::async_runtime::block_on(
            aborted_delivery_caller_cannot_publish_zero_settlement_before_worker_exit(),
        );
    }
}
