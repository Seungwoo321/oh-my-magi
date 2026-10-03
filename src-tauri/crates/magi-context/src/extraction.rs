use crate::RepresentationKind;
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Condvar, Mutex, OnceLock},
    time::{Duration, Instant},
};
static HELPER: OnceLock<Arc<dyn ExtractionHelperAuthority>> = OnceLock::new();
static WORKERS: (Mutex<usize>, Condvar) = (Mutex::new(0), Condvar::new());

#[derive(Clone)]
pub struct ExtractionInvocation {
    identity: Arc<()>,
    deadline: Instant,
}
impl ExtractionInvocation {
    pub fn deadline(&self) -> Instant {
        self.deadline
    }
}

pub struct ExtractionCompletionProof {
    invocation: ExtractionInvocation,
}
impl ExtractionCompletionProof {
    pub fn matches(&self, invocation: &ExtractionInvocation) -> bool {
        Arc::ptr_eq(&self.invocation.identity, &invocation.identity)
    }
}

/// Holds execution custody until the context supervisor proves the owned child
/// and all input/output workers have stopped. Dropping a lease is not that proof.
pub trait ExtractionOperationLease: Send {
    fn check(&self) -> Result<(), String>;
    fn spawn(&self, command: &mut Command) -> Result<std::process::Child, String>;
    fn finish(self: Box<Self>, proof: ExtractionCompletionProof) -> Result<(), String>;
}

pub trait ExtractionHelperAuthority: Send + Sync {
    fn path(&self) -> &Path;
    fn begin(
        &self,
        invocation: ExtractionInvocation,
    ) -> Result<Box<dyn ExtractionOperationLease>, String>;
    fn close(&self) -> Result<(), String>;
}
pub fn close_extraction_helper_authority() -> Result<(), String> {
    HELPER
        .get()
        .ok_or("native extraction helper unavailable")?
        .close()?;
    if *WORKERS.0.lock().map_err(|_| "worker lock poisoned")? != 0 {
        return Err("Native extraction supervisors remain active.".into());
    }
    Ok(())
}

pub fn configure_extraction_helper_authority(
    authority: Arc<dyn ExtractionHelperAuthority>,
) -> Result<(), String> {
    if !authority.path().is_absolute() {
        return Err("extraction helper authority is invalid".into());
    }
    HELPER
        .set(authority)
        .map_err(|_| "extraction helper already configured".into())
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtractedPage {
    pub page: u32,
    pub text: Option<String>,
    pub mime_type: Option<String>,
    pub image_base64: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeExtraction {
    pub schema_version: u16,
    pub kind: RepresentationKind,
    pub mime_type: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub image_base64: Option<String>,
    pub pages: Vec<ExtractedPage>,
    pub warnings: Vec<String>,
    #[serde(default)]
    pub total_pages: Option<u32>,
}
struct Permit;
impl Permit {
    fn acquire(deadline: Instant, lease: &dyn ExtractionOperationLease) -> Result<Self, String> {
        let (active, cv) = (&WORKERS.0, &WORKERS.1);
        let mut count = active.lock().map_err(|_| "worker lock poisoned")?;
        while *count >= 2 {
            lease.check()?;
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .ok_or("extraction worker deadline exceeded")?;
            count = cv
                .wait_timeout(count, remaining.min(Duration::from_millis(10)))
                .map_err(|_| "worker lock poisoned")?
                .0;
        }
        lease.check()?;
        *count += 1;
        Ok(Self)
    }
}
impl Drop for Permit {
    fn drop(&mut self) {
        if let Ok(mut c) = WORKERS.0.lock() {
            *c = c.saturating_sub(1);
            WORKERS.1.notify_one();
        }
    }
}
pub fn extract_native(bytes: &[u8], mime: &str) -> Result<NativeExtraction, String> {
    extract_native_request(bytes, mime, None, false)
}

pub(crate) fn extract_native_request(
    bytes: &[u8],
    mime: &str,
    range: Option<(u32, u32)>,
    metadata_only: bool,
) -> Result<NativeExtraction, String> {
    if range.is_some_and(|(start, end)| {
        start == 0 || end < start || end > 1000 || end - start + 1 > 200
    }) || (mime != "application/pdf" && (metadata_only || range.is_some()))
        || (metadata_only && range.is_some())
    {
        return Err("invalid bounded PDF extraction request".into());
    }
    if bytes.len() > 100 * 1024 * 1024 {
        return Err("file byte limit exceeded".into());
    }
    let helper = HELPER
        .get()
        .ok_or("native extraction helper unavailable")?
        .clone();
    let deadline = Instant::now() + Duration::from_secs(60);
    let invocation = ExtractionInvocation {
        identity: Arc::new(()),
        deadline,
    };
    let path = helper.path().to_owned();
    let bytes = bytes.to_vec();
    let mime = mime.to_owned();
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("native-extraction-supervisor".into())
        .spawn(move || {
            let result = helper.begin(invocation.clone()).and_then(|lease| {
                supervise_extraction(
                    ExtractionWork {
                        bytes,
                        mime,
                        range,
                        metadata_only,
                        helper: path,
                        invocation,
                    },
                    lease,
                )
            });
            let _ = sender.send(result);
        })
        .map_err(|_| "extraction supervisor could not start")?;
    receiver
        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .map_err(|_| "extraction cleanup is unresolved")?
}

struct ExtractionWork {
    bytes: Vec<u8>,
    mime: String,
    range: Option<(u32, u32)>,
    metadata_only: bool,
    helper: PathBuf,
    invocation: ExtractionInvocation,
}
fn supervise_extraction(
    work: ExtractionWork,
    lease: Box<dyn ExtractionOperationLease>,
) -> Result<NativeExtraction, String> {
    let ExtractionWork {
        bytes,
        mime,
        range,
        metadata_only,
        helper,
        invocation,
    } = work;
    let deadline = invocation.deadline();
    let mime = mime.as_str();
    let _permit = Permit::acquire(deadline, lease.as_ref())?;
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (helper, mime, bytes, invocation, range, metadata_only);
        return Err("native extraction requires macOS".into());
    }
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::process::CommandExt;
        let escaped = helper
            .to_str()
            .ok_or("helper path encoding invalid")?
            .replace('\\', "\\\\")
            .replace('"', "\\\"");
        let policy = format!(
            "(version 1)(deny default)(allow process-exec (literal \"{escaped}\"))(allow file-read* (subpath \"/System\") (subpath \"/usr/lib\") (subpath \"/private/var/db/dyld\") (literal \"{escaped}\"))(allow file-read-metadata)(allow file-read-data (require-not (vnode-type REGULAR-FILE)))(allow sysctl-read)(allow mach-lookup)(allow signal (target self))"
        );
        let mut command = Command::new("/usr/bin/sandbox-exec");
        command
            .args(["-p", &policy])
            .arg(&helper)
            .arg(mime)
            .env_clear()
            .env("LANG", "en_US.UTF-8")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if metadata_only {
            command.arg("--metadata");
        } else if let Some((start, end)) = range {
            command.args([start.to_string(), end.to_string()]);
        }
        // Resource limits apply before the untrusted document parser is loaded.
        unsafe {
            command.pre_exec(|| {
                if libc::setpgid(0, 0) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                let cpu = libc::rlimit {
                    rlim_cur: 60,
                    rlim_max: 60,
                };
                if libc::setrlimit(libc::RLIMIT_CPU, &cpu) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        lease.check()?;
        let child = lease.spawn(&mut command)?;
        let group = child.id() as i32;
        let mut child = OwnedExtractionChild {
            child,
            group,
            observed_stopped: false,
        };
        let mut stdin = child.stdin.take().ok_or("worker input unavailable")?;
        let stdout = child.stdout.take().ok_or("worker output unavailable")?;
        let stderr = child
            .stderr
            .take()
            .ok_or("worker error channel unavailable")?;
        let input = bytes;
        let writer = std::thread::spawn(move || stdin.write_all(&input));
        let reader = std::thread::spawn(move || {
            let mut output = Vec::new();
            stdout
                .take(100 * 1024 * 1024 + 1)
                .read_to_end(&mut output)
                .map(|_| output)
        });
        let errors = std::thread::spawn(move || {
            let mut output = Vec::new();
            stderr.take(4097).read_to_end(&mut output).map(|_| output)
        });
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None)
                    if Instant::now() < deadline
                        && lease.check().is_ok()
                        && worker_memory(child.id())
                            .is_some_and(|bytes| bytes <= 512 * 1024 * 1024) =>
                {
                    std::thread::sleep(Duration::from_millis(10))
                }
                _ => {
                    unsafe {
                        libc::kill(-group, libc::SIGKILL);
                    }
                    let reaped = child.wait().is_ok();
                    let _writer = writer.join();
                    let _reader = reader.join();
                    let _errors = errors.join();
                    if reaped && process_group_absent(group) {
                        child.observed_stopped = true;
                        lease.finish(ExtractionCompletionProof { invocation })?;
                    }
                    return Err("extraction timeout or process failure".into());
                }
            }
        };
        if !process_group_absent(group) {
            unsafe {
                libc::kill(-group, libc::SIGKILL);
            }
        }
        let writer_result = writer.join();
        let output_result = reader.join();
        let error_result = errors.join();
        if !process_group_absent(group) {
            return Err("extraction cleanup is unresolved".into());
        }
        child.observed_stopped = true;
        lease.finish(ExtractionCompletionProof { invocation })?;
        writer_result
            .map_err(|_| "worker input thread failed")?
            .map_err(|_| "worker input write failed")?;
        let output = output_result
            .map_err(|_| "worker output thread failed")?
            .map_err(|_| "worker output read failed")?;
        let error = error_result
            .map_err(|_| "worker error thread failed")?
            .map_err(|_| "worker error read failed")?;
        if !status.success() {
            return Err(worker_failure_code(&error).into());
        }
        if output.len() > 100 * 1024 * 1024 {
            return Err("derived output byte limit exceeded".into());
        }
        let extracted: NativeExtraction =
            serde_json::from_slice(&output).map_err(|_| "invalid native extraction response")?;
        validate_extraction(&extracted, mime, range, metadata_only)?;
        Ok(extracted)
    }
}

#[cfg(target_os = "macos")]
fn process_group_absent(group: i32) -> bool {
    unsafe {
        libc::kill(-group, 0) == -1
            && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
    }
}

#[cfg(target_os = "macos")]
struct OwnedExtractionChild {
    child: std::process::Child,
    group: i32,
    observed_stopped: bool,
}
#[cfg(target_os = "macos")]
impl std::ops::Deref for OwnedExtractionChild {
    type Target = std::process::Child;
    fn deref(&self) -> &Self::Target {
        &self.child
    }
}
#[cfg(target_os = "macos")]
impl std::ops::DerefMut for OwnedExtractionChild {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.child
    }
}
#[cfg(target_os = "macos")]
impl Drop for OwnedExtractionChild {
    fn drop(&mut self) {
        if !self.observed_stopped {
            unsafe {
                libc::kill(-self.group, libc::SIGKILL);
            }
            let _ = self.child.wait();
        }
    }
}

fn worker_failure_code(bytes: &[u8]) -> &'static str {
    match bytes {
        b"file_byte_limit" => "file_byte_limit",
        b"invalid_worker_request" => "invalid_worker_request",
        b"pdf_invalid_or_encrypted" => "pdf_invalid_or_encrypted",
        b"pdf_hard_page_limit" => "pdf_hard_page_limit",
        b"pdf_invalid_page_range" => "pdf_invalid_page_range",
        b"pdf_page_limit_select_range" => "pdf_page_limit_select_range",
        b"pdf_page_unavailable" => "pdf_page_unavailable",
        b"page_text_limit" => "page_text_limit",
        b"pdf_raster_pixel_limit" => "pdf_raster_pixel_limit",
        b"pdf_raster_allocation" => "pdf_raster_allocation",
        b"pdf_raster_failed" => "pdf_raster_failed",
        b"pdf_raster_encoder" => "pdf_raster_encoder",
        b"image_invalid_or_multiframe" => "image_invalid_or_multiframe",
        b"image_pixel_limit" => "image_pixel_limit",
        b"image_decode_failed" => "image_decode_failed",
        b"unsupported_worker_mime" => "unsupported_worker_mime",
        b"derived_byte_limit" => "derived_byte_limit",
        b"worker_output_failed" => "worker_output_failed",
        _ => "native_extraction_failed",
    }
}
fn valid_dimensions(width: Option<u32>, height: Option<u32>) -> bool {
    width.zip(height).is_some_and(|(w, h)| {
        w > 0 && h > 0 && w <= 10000 && h <= 10000 && u64::from(w) * u64::from(h) <= 40_000_000
    })
}
fn valid_base64(value: Option<&str>) -> bool {
    value.is_some_and(|value| {
        let bytes = value.as_bytes();
        let padding = bytes.iter().rev().take_while(|byte| **byte == b'=').count();
        !bytes.is_empty()
            && bytes.len() % 4 == 0
            && padding <= 2
            && bytes[..bytes.len() - padding]
                .iter()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/'))
    })
}
fn validate_extraction(
    extracted: &NativeExtraction,
    mime: &str,
    range: Option<(u32, u32)>,
    metadata_only: bool,
) -> Result<(), String> {
    let valid_kind = match mime {
        "application/pdf" => {
            matches!(
                extracted.kind,
                RepresentationKind::PdfText | RepresentationKind::PdfRaster
            ) && extracted.width.is_none()
                && extracted.height.is_none()
                && extracted.image_base64.is_none()
        }
        "image/png" | "image/jpeg" => {
            extracted.kind == RepresentationKind::Image
                && extracted.pages.is_empty()
                && extracted.total_pages.is_none()
                && valid_base64(extracted.image_base64.as_deref())
                && extracted
                    .width
                    .zip(extracted.height)
                    .is_some_and(|(width, height)| {
                        width > 0
                            && height > 0
                            && width <= 10000
                            && height <= 10000
                            && u64::from(width) * u64::from(height) <= 40_000_000
                    })
        }
        _ => false,
    };
    if !valid_kind
        || extracted.schema_version != 1
        || extracted.pages.len() > 200
        || extracted.mime_type != mime
        || (mime == "application/pdf"
            && !extracted
                .total_pages
                .is_some_and(|count| (1..=1000).contains(&count)))
        || (metadata_only && (!extracted.pages.is_empty() || extracted.image_base64.is_some()))
        || (!metadata_only
            && mime == "application/pdf"
            && extracted.pages.len()
                != range.map_or(extracted.total_pages.unwrap_or(0), |(start, end)| {
                    end - start + 1
                }) as usize)
    {
        return Err("native extraction contract mismatch".into());
    }
    let mut raster_pages = false;
    for (i, page) in extracted.pages.iter().enumerate() {
        let text_page = page
            .text
            .as_ref()
            .is_some_and(|text| !text.trim().is_empty() && text.len() <= 1024 * 1024)
            && page.mime_type.is_none()
            && page.image_base64.is_none()
            && page.width.is_none()
            && page.height.is_none();
        let raster_page = page.text.is_none()
            && page.mime_type.as_deref() == Some("image/png")
            && valid_base64(page.image_base64.as_deref())
            && valid_dimensions(page.width, page.height);
        raster_pages |= raster_page;
        if page.page != i as u32 + range.map_or(1, |(start, _)| start)
            || !text_page && !raster_page
            || page.page > extracted.total_pages.unwrap_or(0)
        {
            return Err("native extraction page contract mismatch".into());
        }
    }
    if !metadata_only
        && mime == "application/pdf"
        && (raster_pages != (extracted.kind == RepresentationKind::PdfRaster))
    {
        return Err("native extraction representation mismatch".into());
    }
    Ok(())
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    struct TestHelperAuthority(PathBuf);
    struct TestLease(ExtractionInvocation);
    impl ExtractionHelperAuthority for TestHelperAuthority {
        fn path(&self) -> &Path {
            &self.0
        }
        fn begin(
            &self,
            invocation: ExtractionInvocation,
        ) -> Result<Box<dyn ExtractionOperationLease>, String> {
            Ok(Box::new(TestLease(invocation)))
        }
        fn close(&self) -> Result<(), String> {
            Ok(())
        }
    }
    impl ExtractionOperationLease for TestLease {
        fn check(&self) -> Result<(), String> {
            Ok(())
        }
        fn spawn(&self, command: &mut Command) -> Result<std::process::Child, String> {
            command
                .spawn()
                .map_err(|_| "owned test helper could not start".into())
        }
        fn finish(self: Box<Self>, proof: ExtractionCompletionProof) -> Result<(), String> {
            if proof.matches(&self.0) {
                Ok(())
            } else {
                Err("wrong helper invocation".into())
            }
        }
    }

    #[test]
    fn extraction_completion_proof_is_bound_to_exact_context_invocation() {
        let deadline = Instant::now() + Duration::from_secs(1);
        let first = ExtractionInvocation {
            identity: Arc::new(()),
            deadline,
        };
        let second = ExtractionInvocation {
            identity: Arc::new(()),
            deadline,
        };
        let proof = ExtractionCompletionProof {
            invocation: first.clone(),
        };
        assert!(proof.matches(&first));
        assert!(!proof.matches(&second));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn stalled_helper_deadline_reaps_owned_group_before_completion_proof() {
        use std::{
            os::unix::process::CommandExt,
            sync::atomic::{AtomicBool, AtomicU32, Ordering},
        };
        struct StallLease {
            invocation: ExtractionInvocation,
            pid: Arc<AtomicU32>,
            finished: Arc<AtomicBool>,
        }
        impl ExtractionOperationLease for StallLease {
            fn check(&self) -> Result<(), String> {
                Ok(())
            }
            fn spawn(&self, _: &mut Command) -> Result<std::process::Child, String> {
                let mut command = Command::new("/bin/sleep");
                command
                    .arg("30")
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped());
                unsafe {
                    command.pre_exec(|| {
                        if libc::setpgid(0, 0) == 0 {
                            Ok(())
                        } else {
                            Err(std::io::Error::last_os_error())
                        }
                    });
                }
                let child = command
                    .spawn()
                    .map_err(|_| "owned stalled fixture could not start")?;
                self.pid.store(child.id(), Ordering::SeqCst);
                Ok(child)
            }
            fn finish(self: Box<Self>, proof: ExtractionCompletionProof) -> Result<(), String> {
                assert!(proof.matches(&self.invocation));
                assert!(process_group_absent(self.pid.load(Ordering::SeqCst) as i32));
                self.finished.store(true, Ordering::SeqCst);
                Ok(())
            }
        }
        let invocation = ExtractionInvocation {
            identity: Arc::new(()),
            deadline: Instant::now() + Duration::from_secs(1),
        };
        let pid = Arc::new(AtomicU32::new(0));
        let finished = Arc::new(AtomicBool::new(false));
        let lease = StallLease {
            invocation: invocation.clone(),
            pid: pid.clone(),
            finished: finished.clone(),
        };
        let result = supervise_extraction(
            ExtractionWork {
                bytes: Vec::new(),
                mime: "image/png".into(),
                range: None,
                metadata_only: false,
                helper: PathBuf::from("/owned-stalled-fixture"),
                invocation,
            },
            Box::new(lease),
        );
        assert!(result.is_err());
        assert!(pid.load(Ordering::SeqCst) > 0);
        assert!(finished.load(Ordering::SeqCst));
        assert!(process_group_absent(pid.load(Ordering::SeqCst) as i32));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn abandoned_helper_caller_cannot_finish_before_actual_held_reader_exits() {
        use std::{
            os::fd::{FromRawFd, OwnedFd},
            os::unix::process::CommandExt,
            sync::atomic::{AtomicBool, Ordering},
        };
        struct HeldReaderLease {
            invocation: ExtractionInvocation,
            held_writer: Arc<Mutex<Vec<OwnedFd>>>,
            child: std::sync::mpsc::SyncSender<u32>,
            finished: Arc<AtomicBool>,
        }
        impl ExtractionOperationLease for HeldReaderLease {
            fn check(&self) -> Result<(), String> {
                Ok(())
            }
            fn spawn(&self, _: &mut Command) -> Result<std::process::Child, String> {
                let mut command = Command::new("/bin/sleep");
                command
                    .arg("0.02")
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped());
                unsafe {
                    command.pre_exec(|| {
                        if libc::setpgid(0, 0) == 0 {
                            Ok(())
                        } else {
                            Err(std::io::Error::last_os_error())
                        }
                    });
                }
                let mut child = command.spawn().map_err(|_| "owned fixture spawn failed")?;
                let mut descriptors = [0; 2];
                assert_eq!(unsafe { libc::pipe(descriptors.as_mut_ptr()) }, 0);
                child.stdout = Some(std::process::ChildStdout::from(unsafe {
                    OwnedFd::from_raw_fd(descriptors[0])
                }));
                self.held_writer
                    .lock()
                    .unwrap()
                    .push(unsafe { OwnedFd::from_raw_fd(descriptors[1]) });
                assert_eq!(unsafe { libc::pipe(descriptors.as_mut_ptr()) }, 0);
                child.stderr = Some(std::process::ChildStderr::from(unsafe {
                    OwnedFd::from_raw_fd(descriptors[0])
                }));
                self.held_writer
                    .lock()
                    .unwrap()
                    .push(unsafe { OwnedFd::from_raw_fd(descriptors[1]) });
                self.child.send(child.id()).unwrap();
                Ok(child)
            }
            fn finish(self: Box<Self>, proof: ExtractionCompletionProof) -> Result<(), String> {
                assert!(proof.matches(&self.invocation));
                self.finished.store(true, Ordering::SeqCst);
                Ok(())
            }
        }
        let invocation = ExtractionInvocation {
            identity: Arc::new(()),
            deadline: Instant::now() + Duration::from_secs(2),
        };
        let held_writer = Arc::new(Mutex::new(Vec::new()));
        let finished = Arc::new(AtomicBool::new(false));
        let (child_sender, child_receiver) = std::sync::mpsc::sync_channel(1);
        let lease = HeldReaderLease {
            invocation: invocation.clone(),
            held_writer: held_writer.clone(),
            child: child_sender,
            finished: finished.clone(),
        };
        let (result_sender, abandoned_receiver) = std::sync::mpsc::sync_channel(1);
        let worker = std::thread::spawn(move || {
            let result = supervise_extraction(
                ExtractionWork {
                    bytes: Vec::new(),
                    mime: "image/png".into(),
                    range: None,
                    metadata_only: false,
                    helper: PathBuf::from("/owned-fixture"),
                    invocation,
                },
                Box::new(lease),
            );
            let _ = result_sender.send(result);
        });
        let pid = child_receiver.recv_timeout(Duration::from_secs(1)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        while unsafe { libc::kill(pid as i32, 0) } == 0 {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        drop(abandoned_receiver);
        assert!(!finished.load(Ordering::SeqCst));
        drop(held_writer.lock().unwrap().pop());
        assert!(!finished.load(Ordering::SeqCst));
        held_writer.lock().unwrap().clear();
        worker.join().unwrap();
        assert!(finished.load(Ordering::SeqCst));
    }
    #[test]
    fn worker_private_stderr_and_malformed_page_responses_fail_closed() {
        assert_eq!(
            worker_failure_code(b"private_token_canary"),
            "native_extraction_failed"
        );
        assert_eq!(
            worker_failure_code(b"pdf_invalid_or_encrypted private_token_canary"),
            "native_extraction_failed"
        );
        assert_eq!(
            worker_failure_code(b"pdf_invalid_or_encrypted"),
            "pdf_invalid_or_encrypted"
        );
        let text = ExtractedPage {
            page: 1,
            text: Some("public".into()),
            mime_type: None,
            image_base64: None,
            width: None,
            height: None,
        };
        let valid = NativeExtraction {
            schema_version: 1,
            kind: RepresentationKind::PdfText,
            mime_type: "application/pdf".into(),
            width: None,
            height: None,
            image_base64: None,
            pages: vec![text],
            warnings: vec![],
            total_pages: Some(1),
        };
        validate_extraction(&valid, "application/pdf", None, false).unwrap();
        let mut wrong = valid.clone();
        wrong.pages[0].image_base64 = Some("AAAA".into());
        assert!(validate_extraction(&wrong, "application/pdf", None, false).is_err());
        let mut wrong = valid.clone();
        wrong.pages[0].text = None;
        assert!(validate_extraction(&wrong, "application/pdf", None, false).is_err());
        let mut wrong = valid.clone();
        wrong.pages[0].width = Some(1);
        assert!(validate_extraction(&wrong, "application/pdf", None, false).is_err());
        let mut wrong = valid.clone();
        wrong.kind = RepresentationKind::PdfRaster;
        assert!(validate_extraction(&wrong, "application/pdf", None, false).is_err());
        let mut wrong = valid.clone();
        wrong.pages[0] = ExtractedPage {
            page: 1,
            text: None,
            mime_type: Some("image/png".into()),
            image_base64: Some("invalid%%%".into()),
            width: Some(1),
            height: Some(1),
        };
        wrong.kind = RepresentationKind::PdfRaster;
        assert!(validate_extraction(&wrong, "application/pdf", None, false).is_err());
        wrong.pages[0].image_base64 = Some("AAAA".into());
        validate_extraction(&wrong, "application/pdf", None, false).unwrap();
        wrong.pages[0].height = None;
        assert!(validate_extraction(&wrong, "application/pdf", None, false).is_err());
    }

    fn helper() {
        static INIT: OnceLock<()> = OnceLock::new();
        INIT.get_or_init(|| {
            let output = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../../.local/runtime-probes")
                .join(format!("magi-extract-test-{}", std::process::id()));
            std::fs::create_dir_all(output.parent().unwrap()).unwrap();
            let source =
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../native/extract.swift");
            let status = Command::new("xcrun")
                .args(["swiftc", "-O"])
                .arg(source)
                .arg("-o")
                .arg(&output)
                .status()
                .unwrap();
            assert!(status.success());
            configure_extraction_helper_authority(Arc::new(TestHelperAuthority(
                output.canonicalize().unwrap(),
            )))
            .unwrap();
        });
    }
    #[test]
    fn captures_png_without_ocr_and_rejects_bad_pdf() {
        helper();
        let bytes: &[u8] = &PNG;
        let image = extract_native(bytes, "image/png").unwrap();
        assert_eq!(image.kind, RepresentationKind::Image);
        assert_eq!(image.width, Some(1));
        assert_eq!(image.height, Some(1));
        assert!(image.image_base64.is_some());
        assert!(image.pages.is_empty());
        assert!(extract_native(b"invalid pdf", "application/pdf").is_err());
        assert!(extract_native(bytes, "image/jpeg").is_err());
        let text = extract_native(&pdf(true), "application/pdf").unwrap();
        assert_eq!(text.kind, RepresentationKind::PdfText);
        assert_eq!(text.pages.len(), 1);
        assert!(
            text.pages[0]
                .text
                .as_ref()
                .unwrap()
                .contains("Public PDF fixture")
        );
        let scanned = extract_native(&pdf(false), "application/pdf").unwrap();
        assert_eq!(scanned.kind, RepresentationKind::PdfRaster);
        assert!(scanned.pages[0].text.is_none());
        assert!(scanned.pages[0].image_base64.is_some());
    }

    #[test]
    fn binary_capture_preserves_raw_and_derived_authority_and_rejects_line_selection() {
        helper();
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../.local/runtime-probes")
            .join(format!("capture-{}.png", uuid::Uuid::new_v4()));
        std::fs::write(&path, PNG).unwrap();
        let file = cap_std::fs::File::from_std(std::fs::File::open(&path).unwrap());
        let mut grant =
            crate::SourceGrant::selected_files(vec![(file, "fixture.png".into())]).unwrap();
        let enumeration = grant
            .enumerate(crate::CaptureLimits::default_policy())
            .unwrap();
        let id = enumeration.candidates[0].source_id.clone();
        let batch = grant
            .capture_selected(
                &[crate::CaptureDirective {
                    source_id: id.clone(),
                    line_range: None,
                }],
                crate::CaptureLimits::default_policy(),
                1,
            )
            .unwrap();
        assert_eq!(batch.manifest.schema_version, 2);
        assert_eq!(batch.objects.len(), 1);
        assert_eq!(batch.objects[0].original_bytes(), PNG);
        assert_eq!(
            batch.objects[0].object_digest,
            crate::Digest::from_bytes(&PNG)
        );
        assert_ne!(
            batch.objects[0].object_digest,
            batch.objects[0].derived_digest
        );
        assert_eq!(batch.objects[0].representation.locator.width, Some(1));
        batch.manifest.validate().unwrap();
        let rejected = grant
            .capture_selected(
                &[crate::CaptureDirective {
                    source_id: id,
                    line_range: Some(crate::LineRange {
                        start_line: 1,
                        end_line: 1,
                    }),
                }],
                crate::CaptureLimits::default_policy(),
                2,
            )
            .unwrap();
        assert!(rejected.objects.is_empty());
        assert_eq!(
            rejected.manifest.content.sources[0]
                .omission
                .as_ref()
                .unwrap()
                .code,
            crate::SourceOmissionCode::InvalidRange
        );
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn large_pdf_requires_explicit_range_and_extracts_only_original_page_numbers() {
        helper();
        let bytes = many_page_pdf(201);
        assert!(extract_native(&bytes, "application/pdf").is_err());
        let metadata = extract_native_request(&bytes, "application/pdf", None, true).unwrap();
        assert_eq!(metadata.total_pages, Some(201));
        assert!(metadata.pages.is_empty());
        let selected =
            extract_native_request(&bytes, "application/pdf", Some((201, 201)), false).unwrap();
        assert_eq!(selected.pages.len(), 1);
        assert_eq!(selected.pages[0].page, 201);
        assert!(
            selected.pages[0]
                .text
                .as_ref()
                .unwrap()
                .contains("private-page-201")
        );
        let wire = serde_json::to_string(&selected).unwrap();
        assert!(!wire.contains("private-page-200"));
        assert!(extract_native_request(&bytes, "application/pdf", Some((1, 201)), false).is_err());
        assert!(
            extract_native_request(&bytes, "application/pdf", Some((202, 202)), false).is_err()
        );
        assert!(extract_native_request(&bytes, "application/pdf", Some((0, 1)), false).is_err());
        let maximum = many_page_pdf(1000);
        assert_eq!(
            extract_native_request(&maximum, "application/pdf", None, true)
                .unwrap()
                .total_pages,
            Some(1000)
        );
        let excessive = many_page_pdf(1001);
        assert!(extract_native_request(&excessive, "application/pdf", None, true).is_err());
        assert!(
            extract_native_request(&excessive, "application/pdf", Some((1, 1)), false).is_err()
        );
    }

    fn many_page_pdf(count: usize) -> Vec<u8> {
        let mut objects = vec![
            String::from("<< /Type /Catalog /Pages 2 0 R >>"),
            String::new(),
        ];
        let mut kids = Vec::new();
        for page in 1..=count {
            let object = objects.len() + 1;
            kids.push(format!("{object} 0 R"));
            let stream = format!("BT /F1 12 Tf 20 100 Td (private-page-{page}) Tj ET");
            objects.push(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Resources << /Font << /F1 {} 0 R >> >> /Contents {} 0 R >>", count * 2 + 3, object + 1));
            objects.push(format!(
                "<< /Length {} >>\nstream\n{}\nendstream",
                stream.len(),
                stream
            ));
        }
        objects[1] = format!(
            "<< /Type /Pages /Kids [{}] /Count {count} >>",
            kids.join(" ")
        );
        objects.push("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into());
        let mut bytes = b"%PDF-1.4\n".to_vec();
        let mut offsets = vec![0];
        for (index, object) in objects.iter().enumerate() {
            offsets.push(bytes.len());
            bytes
                .extend_from_slice(format!("{} 0 obj\n{}\nendobj\n", index + 1, object).as_bytes());
        }
        let xref = bytes.len();
        bytes.extend_from_slice(
            format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len()).as_bytes(),
        );
        for offset in offsets.iter().skip(1) {
            bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        bytes.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                offsets.len()
            )
            .as_bytes(),
        );
        bytes
    }
    fn pdf(text: bool) -> Vec<u8> {
        let stream = if text {
            "BT /F1 12 Tf 72 700 Td (Public PDF fixture) Tj ET"
        } else {
            "0.8 0.1 0.1 rg 20 20 100 100 re f"
        };
        let objects=["<< /Type /Catalog /Pages 2 0 R >>".to_string(),"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".into(),"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>".into(),"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),format!("<< /Length {} >>\nstream\n{}\nendstream",stream.len(),stream)];
        let mut out = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (i, object) in objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend(format!("{} 0 obj\n{}\nendobj\n", i + 1, object).as_bytes());
        }
        let xref = out.len();
        out.extend(b"xref\n0 6\n0000000000 65535 f \n");
        for offset in offsets {
            out.extend(format!("{offset:010} 00000 n \n").as_bytes());
        }
        out.extend(
            format!("trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
        );
        out
    }
    const PNG: [u8; 69] = [
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 2,
        0, 0, 0, 144, 119, 83, 222, 0, 0, 0, 12, 73, 68, 65, 84, 120, 156, 99, 248, 207, 192, 0, 0,
        3, 1, 1, 0, 201, 254, 146, 239, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
    ];
}

#[cfg(target_os = "macos")]
fn worker_memory(pid: u32) -> Option<u64> {
    #[repr(C)]
    struct Usage {
        uuid: [u8; 16],
        user: u64,
        system: u64,
        idle: u64,
        interrupt: u64,
        pageins: u64,
        wired: u64,
        resident: u64,
        footprint: u64,
        start: u64,
        exit: u64,
    }
    #[link(name = "proc")]
    unsafe extern "C" {
        fn proc_pid_rusage(
            pid: libc::c_int,
            flavor: libc::c_int,
            buffer: *mut libc::c_void,
        ) -> libc::c_int;
    }
    let mut usage = std::mem::MaybeUninit::<Usage>::zeroed();
    let result = unsafe { proc_pid_rusage(pid as libc::c_int, 0, usage.as_mut_ptr().cast()) };
    if result == 0 {
        Some(unsafe { usage.assume_init() }.resident)
    } else {
        None
    }
}
