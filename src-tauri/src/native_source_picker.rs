#[cfg(target_os = "macos")]
use objc2::ClassType;
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PickerOutcome {
    Selected(Vec<PathBuf>),
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PickerError {
    Unavailable,
    TimedOut,
    InvalidSelection,
}

impl PickerError {
    pub(crate) fn message(&self) -> &'static str {
        match self {
            Self::Unavailable => "The native source picker is unavailable. No source was captured.",
            Self::TimedOut => "The native source picker timed out. No source was captured.",
            Self::InvalidSelection => {
                "The native source picker returned an invalid selection. No source was captured."
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PickerFilter {
    pub(crate) label: String,
    pub(crate) extensions: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PickerRequest {
    SingleFile {
        filters: Vec<PickerFilter>,
    },
    MultipleFiles {
        filters: Vec<PickerFilter>,
    },
    Directory,
    SaveFile {
        default_name: String,
        filters: Vec<PickerFilter>,
    },
}
impl PickerRequest {
    fn filters(&self) -> &[PickerFilter] {
        match self {
            Self::SingleFile { filters }
            | Self::MultipleFiles { filters }
            | Self::SaveFile { filters, .. } => filters,
            Self::Directory => &[],
        }
    }
    fn validate(&self) -> Result<(), PickerError> {
        if self.filters().len() > 16
            || self.filters().iter().any(|filter| {
                filter.label.trim().is_empty()
                    || filter.label.len() > 128
                    || filter.label.chars().any(char::is_control)
                    || filter.extensions.is_empty()
                    || filter.extensions.len() > 32
                    || filter.extensions.iter().any(|extension| {
                        extension.is_empty()
                            || extension.len() > 32
                            || !extension.bytes().all(|byte| byte.is_ascii_alphanumeric())
                    })
            })
        {
            return Err(PickerError::InvalidSelection);
        }
        if let Self::SaveFile { default_name, .. } = self
            && (default_name.trim().is_empty()
                || default_name.len() > 256
                || default_name == "."
                || default_name == ".."
                || default_name
                    .chars()
                    .any(|character| character.is_control() || matches!(character, '/' | '\\')))
        {
            return Err(PickerError::InvalidSelection);
        }
        Ok(())
    }
    fn validate_paths(&self, paths: Vec<PathBuf>) -> Result<PickerOutcome, PickerError> {
        let multiple = matches!(self, Self::MultipleFiles { .. });
        if paths.is_empty()
            || paths.len() > if multiple { 1000 } else { 1 }
            || paths.iter().any(|path| {
                !path.is_absolute()
                    || (!self.filters().is_empty()
                        && !path
                            .extension()
                            .and_then(|extension| extension.to_str())
                            .is_some_and(|extension| {
                                self.filters()
                                    .iter()
                                    .flat_map(|filter| &filter.extensions)
                                    .any(|allowed| extension.eq_ignore_ascii_case(allowed))
                            }))
            })
        {
            return Err(PickerError::InvalidSelection);
        }
        Ok(PickerOutcome::Selected(paths))
    }
}
#[cfg(any(not(target_os = "macos"), test))]
struct CompletionLifetime(Arc<AtomicBool>);
#[cfg(any(not(target_os = "macos"), test))]
impl Drop for CompletionLifetime {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

pub(crate) async fn select(
    window: &tauri::WebviewWindow,
    directory: bool,
) -> Result<PickerOutcome, PickerError> {
    select_request(
        window,
        if directory {
            PickerRequest::Directory
        } else {
            PickerRequest::MultipleFiles {
                filters: Vec::new(),
            }
        },
    )
    .await
}

fn validate_title(title: Option<&str>) -> Result<(), PickerError> {
    if title.is_some_and(|title| {
        title.trim().is_empty()
            || title.chars().count() > 1024
            || title.chars().any(char::is_control)
    }) {
        return Err(PickerError::InvalidSelection);
    }
    Ok(())
}
pub(crate) async fn select_request(
    window: &tauri::WebviewWindow,
    request: PickerRequest,
) -> Result<PickerOutcome, PickerError> {
    select_request_inner(window, request, None).await
}
pub(crate) async fn select_request_with_title(
    window: &tauri::WebviewWindow,
    request: PickerRequest,
    title: String,
) -> Result<PickerOutcome, PickerError> {
    select_request_inner(window, request, Some(title)).await
}

struct Completion {
    deadline: Instant,
    closed: Arc<AtomicBool>,
    sender: Option<tokio::sync::oneshot::Sender<Result<PickerOutcome, PickerError>>>,
}

impl Completion {
    fn finish(mut self, result: Result<PickerOutcome, PickerError>) {
        if !self.closed.swap(true, Ordering::SeqCst)
            && let Some(sender) = self.sender.take()
        {
            let result = if Instant::now() >= self.deadline {
                Err(PickerError::TimedOut)
            } else {
                result
            };
            let _ = sender.send(result);
        }
    }
}

async fn wait(
    receiver: tokio::sync::oneshot::Receiver<Result<PickerOutcome, PickerError>>,
    closed: &AtomicBool,
    deadline: Instant,
) -> Result<PickerOutcome, PickerError> {
    match tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), receiver).await {
        Ok(Ok(result)) => {
            if Instant::now() >= deadline {
                closed.store(true, Ordering::SeqCst);
                Err(PickerError::TimedOut)
            } else {
                result
            }
        }
        Ok(Err(_)) => {
            closed.store(true, Ordering::SeqCst);
            Err(PickerError::Unavailable)
        }
        Err(_) => {
            closed.store(true, Ordering::SeqCst);
            Err(PickerError::TimedOut)
        }
    }
}

#[cfg(any(target_os = "macos", test))]
fn required_panel<T>(panel: Option<T>) -> Result<T, PickerError> {
    panel.ok_or(PickerError::Unavailable)
}

#[cfg(target_os = "macos")]
#[derive(Clone)]
enum NativePanel {
    Open(objc2::rc::Retained<objc2_app_kit::NSOpenPanel>),
    Save(objc2::rc::Retained<objc2_app_kit::NSSavePanel>),
}
#[cfg(target_os = "macos")]
impl NativePanel {
    fn sheet(&self) -> &objc2_app_kit::NSSavePanel {
        match self {
            Self::Open(panel) => panel.as_super(),
            Self::Save(panel) => panel,
        }
    }
    fn paths(&self) -> Result<Vec<PathBuf>, PickerError> {
        let urls: Vec<_> = match self {
            Self::Open(panel) => {
                let urls = panel.URLs();
                if urls.count() == 0 || urls.count() > 1000 {
                    return Err(PickerError::InvalidSelection);
                }
                urls.iter().collect()
            }
            Self::Save(panel) => vec![panel.URL().ok_or(PickerError::InvalidSelection)?],
        };
        if urls.is_empty() || urls.len() > 1000 {
            return Err(PickerError::InvalidSelection);
        }
        urls.into_iter()
            .map(|url| {
                if !url.isFileURL() {
                    return Err(PickerError::InvalidSelection);
                }
                url.path()
                    .map(|path| PathBuf::from(path.to_string()))
                    .ok_or(PickerError::InvalidSelection)
            })
            .collect()
    }
}
#[cfg(target_os = "macos")]
thread_local! {
    static PANELS: std::cell::RefCell<std::collections::HashMap<uuid::Uuid, NativePanel>> = std::cell::RefCell::new(std::collections::HashMap::new());
}
#[cfg(target_os = "macos")]
struct PanelLifetime {
    window: tauri::WebviewWindow,
    token: uuid::Uuid,
    closed: Arc<AtomicBool>,
}
#[cfg(target_os = "macos")]
impl Drop for PanelLifetime {
    fn drop(&mut self) {
        self.closed.store(true, Ordering::SeqCst);
        let token = self.token;
        // A dropped IPC future closes only its own sheet; a late callback cannot capture.
        let _ = self.window.run_on_main_thread(move || {
            let panel = PANELS.with(|panels| panels.borrow_mut().remove(&token));
            if let Some(panel) = panel {
                unsafe { panel.sheet().cancel(None) };
            }
        });
    }
}
#[cfg(target_os = "macos")]
async fn select_request_inner(
    window: &tauri::WebviewWindow,
    request: PickerRequest,
    title: Option<String>,
) -> Result<PickerOutcome, PickerError> {
    use block2::RcBlock;
    use objc2::{MainThreadMarker, msg_send, rc::Retained};
    use objc2_app_kit::{
        NSModalResponseCancel, NSModalResponseOK, NSOpenPanel, NSSavePanel, NSWindow,
    };
    use objc2_foundation::{NSArray, NSString};
    use objc2_uniform_type_identifiers::UTType;
    use std::cell::RefCell;
    let deadline = Instant::now() + Duration::from_secs(120);
    request.validate()?;
    validate_title(title.as_deref())?;
    let token = uuid::Uuid::new_v4();
    let closed = Arc::new(AtomicBool::new(false));
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let _lifetime = PanelLifetime {
        window: window.clone(),
        token,
        closed: closed.clone(),
    };
    let completion = Completion {
        deadline,
        closed: closed.clone(),
        sender: Some(sender),
    };
    let owned_window = window.clone();
    window.run_on_main_thread(move || {
        let Some(_main) = MainThreadMarker::new() else { completion.finish(Err(PickerError::Unavailable)); return; };
        if completion.closed.load(Ordering::SeqCst) { return; }
        if Instant::now() >= completion.deadline { completion.finish(Err(PickerError::TimedOut)); return; }
        if PANELS.with(|panels| !panels.borrow().is_empty()) { completion.finish(Err(PickerError::Unavailable)); return; }
        #[cfg(feature = "native-live-probe")]
        {
            let bundle = objc2_foundation::NSBundle::mainBundle().bundleIdentifier();
            let host_matches = bundle.as_ref().is_some_and(|id| id.to_string() == "local.magi.installed.probe");
            let application = objc2_app_kit::NSApplication::sharedApplication(_main);
            let activation_policy: isize = unsafe { msg_send![&*application, activationPolicy] };
            eprintln!("Native source picker host: bundle_present={}, installed_probe_identity={}, activation_policy={}", bundle.is_some(), host_matches, activation_policy);
        }
        let parent = owned_window.ns_window().ok().and_then(|pointer| {
            // Retain the actual Tauri parent on its owning thread for sheet presentation.
            unsafe { Retained::<NSWindow>::retain(pointer.cast()) }
        });
        let Some(parent) = parent else { completion.finish(Err(PickerError::Unavailable)); return; };
        // AppKit factories may return nil when the view service cannot create a panel.
        // Generated nonnullable factory bindings would panic on the owning thread.
        let panel = if matches!(request, PickerRequest::SaveFile { .. }) {
            let panel: Option<Retained<NSSavePanel>> = unsafe { msg_send![NSSavePanel::class(), savePanel] };
            panel.map(NativePanel::Save)
        } else {
            let panel: Option<Retained<NSOpenPanel>> = unsafe { msg_send![NSOpenPanel::class(), openPanel] };
            panel.map(NativePanel::Open)
        };
        let panel = match required_panel(panel) { Ok(panel) => panel, Err(error) => { completion.finish(Err(error)); return; } };
        if let Some(title) = &title { panel.sheet().setTitle(Some(&NSString::from_str(title))); }
        if let NativePanel::Open(open) = &panel {
            let directory = matches!(request, PickerRequest::Directory);
            open.setCanChooseFiles(!directory);
            open.setCanChooseDirectories(directory);
            open.setAllowsMultipleSelection(matches!(request, PickerRequest::MultipleFiles { .. }));
        }
        if let PickerRequest::SaveFile { default_name, .. } = &request {
            panel.sheet().setNameFieldStringValue(&NSString::from_str(default_name));
            panel.sheet().setAllowsOtherFileTypes(false);
        }
        let types = request.filters().iter().flat_map(|filter| &filter.extensions)
            .map(|extension| UTType::typeWithFilenameExtension(&NSString::from_str(extension)).ok_or(PickerError::InvalidSelection))
            .collect::<Result<Vec<_>, _>>();
        let types = match types { Ok(types) => types, Err(error) => { completion.finish(Err(error)); return; } };
        panel.sheet().setAllowedContentTypes(&NSArray::from_retained_slice(&types));
        let completion = RefCell::new(Some(completion));
        let callback = RcBlock::new(move |response: isize| {
            let panel = PANELS.with(|panels| panels.borrow_mut().remove(&token));
            let Some(completion) = completion.borrow_mut().take() else { return; };
            if completion.closed.load(Ordering::SeqCst) { return; }
            if Instant::now() >= completion.deadline { completion.finish(Err(PickerError::TimedOut)); return; }
            let result = if response == NSModalResponseCancel { Ok(PickerOutcome::Cancelled) }
                else if response != NSModalResponseOK { Err(PickerError::Unavailable) }
                else if let Some(panel) = panel { panel.paths().and_then(|paths| request.validate_paths(paths)) }
                else { Err(PickerError::Unavailable) };
            completion.finish(result);
        });
        let presentation = panel.clone();
        PANELS.with(|panels| panels.borrow_mut().insert(token, panel));
        // Do not retain a registry borrow while AppKit can synchronously complete the sheet.
        presentation.sheet().beginSheetModalForWindow_completionHandler(&parent, &callback);
    }).map_err(|_| PickerError::Unavailable)?;
    wait(receiver, &closed, deadline).await
}
#[cfg(not(target_os = "macos"))]
async fn select_request_inner(
    window: &tauri::WebviewWindow,
    request: PickerRequest,
    title: Option<String>,
) -> Result<PickerOutcome, PickerError> {
    use tauri_plugin_dialog::DialogExt;
    let deadline = Instant::now() + Duration::from_secs(120);
    request.validate()?;
    validate_title(title.as_deref())?;
    let closed = Arc::new(AtomicBool::new(false));
    let _lifetime = CompletionLifetime(closed.clone());
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let completion = Completion {
        deadline,
        closed: closed.clone(),
        sender: Some(sender),
    };
    let mut picker = window.dialog().file().set_parent(window);
    if let Some(title) = title {
        picker = picker.set_title(title);
    }
    for filter in request.filters() {
        let extensions: Vec<_> = filter.extensions.iter().map(String::as_str).collect();
        picker = picker.add_filter(&filter.label, &extensions);
    }
    let one_request = request.clone();
    let one = move |path: Option<tauri_plugin_dialog::FilePath>| match path {
        None => Ok(PickerOutcome::Cancelled),
        Some(path) => path
            .into_path()
            .map_err(|_| PickerError::InvalidSelection)
            .and_then(|path| one_request.validate_paths(vec![path])),
    };
    match request.clone() {
        PickerRequest::Directory => picker.pick_folder(move |path| completion.finish(one(path))),
        PickerRequest::SingleFile { .. } => {
            picker.pick_file(move |path| completion.finish(one(path)))
        }
        PickerRequest::SaveFile { default_name, .. } => picker
            .set_file_name(&default_name)
            .save_file(move |path| completion.finish(one(path))),
        PickerRequest::MultipleFiles { .. } => picker.pick_files(move |paths| {
            let result = match paths {
                None => Ok(PickerOutcome::Cancelled),
                Some(paths) if paths.is_empty() || paths.len() > 1000 => {
                    Err(PickerError::InvalidSelection)
                }
                Some(paths) => paths
                    .into_iter()
                    .map(|path| path.into_path().map_err(|_| PickerError::InvalidSelection))
                    .collect::<Result<Vec<_>, _>>()
                    .and_then(|paths| request.validate_paths(paths)),
            };
            completion.finish(result);
        }),
    }
    wait(receiver, &closed, deadline).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_source_title_is_bounded_and_preserved_without_control_characters() {
        assert!(validate_title(Some("Select the current file for selected-source.pdf")).is_ok());
        assert!(validate_title(None).is_ok());
        for title in [
            "".to_owned(),
            "source\0name".to_owned(),
            "source\nname".to_owned(),
            "가".repeat(1025),
        ] {
            assert_eq!(
                validate_title(Some(&title)),
                Err(PickerError::InvalidSelection)
            );
        }
        assert!(validate_title(Some(&"가".repeat(1024))).is_ok());
    }

    #[test]
    fn nil_factory_is_unavailable_and_cannot_publish_a_selected_path() {
        let (sender, mut receiver) = tokio::sync::oneshot::channel();
        let result = required_panel::<()>(None)
            .map(|_| PickerOutcome::Selected(vec![PathBuf::from("/tmp/source")]));
        Completion {
            deadline: Instant::now() + Duration::from_secs(120),
            closed: Arc::new(AtomicBool::new(false)),
            sender: Some(sender),
        }
        .finish(result);
        assert_eq!(receiver.try_recv().unwrap(), Err(PickerError::Unavailable));
    }

    #[test]
    fn single_save_and_filtered_selections_preserve_cardinality_and_type() {
        let filter = PickerFilter {
            label: "JSON".into(),
            extensions: vec!["json".into()],
        };
        for request in [
            PickerRequest::SingleFile {
                filters: vec![filter.clone()],
            },
            PickerRequest::SaveFile {
                default_name: "magi-roles.json".into(),
                filters: vec![filter],
            },
        ] {
            assert!(request.validate().is_ok());
            assert!(
                request
                    .validate_paths(vec![PathBuf::from("/tmp/magi.JSON")])
                    .is_ok()
            );
            for paths in [
                vec![],
                vec![PathBuf::from("relative.json")],
                vec![PathBuf::from("/tmp/file.pdf")],
                vec![
                    PathBuf::from("/tmp/one.json"),
                    PathBuf::from("/tmp/two.json"),
                ],
            ] {
                assert_eq!(
                    request.validate_paths(paths),
                    Err(PickerError::InvalidSelection)
                );
            }
        }
        let multiple = PickerRequest::MultipleFiles {
            filters: Vec::new(),
        };
        assert!(
            multiple
                .validate_paths(vec![PathBuf::from("/tmp/a"), PathBuf::from("/tmp/b")])
                .is_ok()
        );
        assert!(
            multiple
                .validate_paths(vec![PathBuf::from("/tmp/a"); 1001])
                .is_err()
        );
        assert!(
            PickerRequest::Directory
                .validate_paths(vec![PathBuf::from("/tmp/a"), PathBuf::from("/tmp/b")])
                .is_err()
        );
    }
    #[test]
    fn caller_filter_and_default_name_are_bounded_without_path_authority() {
        for extension in ["", "*.json", "../json", "json/pdf", "j\nson"] {
            assert!(
                PickerRequest::SingleFile {
                    filters: vec![PickerFilter {
                        label: "JSON".into(),
                        extensions: vec![extension.into()]
                    }]
                }
                .validate()
                .is_err()
            );
        }
        for name in [
            "",
            ".",
            "..",
            "/tmp/file.json",
            "parent\\file.json",
            "file\n.json",
        ] {
            assert!(
                PickerRequest::SaveFile {
                    default_name: name.into(),
                    filters: Vec::new()
                }
                .validate()
                .is_err()
            );
        }
        assert!(
            PickerRequest::SaveFile {
                default_name: "a".repeat(257),
                filters: Vec::new()
            }
            .validate()
            .is_err()
        );
        assert!(
            PickerRequest::SingleFile {
                filters: vec![PickerFilter {
                    label: " ".into(),
                    extensions: vec!["json".into()]
                }]
            }
            .validate()
            .is_err()
        );
    }
    #[test]
    fn cancelled_future_closes_callback_before_late_selected_publication() {
        let closed = Arc::new(AtomicBool::new(false));
        let lifetime = CompletionLifetime(closed.clone());
        let (sender, mut receiver) = tokio::sync::oneshot::channel();
        let completion = Completion {
            deadline: Instant::now() + Duration::from_secs(120),
            closed,
            sender: Some(sender),
        };
        drop(lifetime);
        completion.finish(Ok(PickerOutcome::Selected(vec![PathBuf::from(
            "/tmp/late",
        )])));
        assert_eq!(
            receiver.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Closed)
        );
    }

    #[test]
    fn completion_distinguishes_unavailable_from_real_user_cancel() {
        let deadline = Instant::now() + Duration::from_secs(1);
        for result in [Err(PickerError::Unavailable), Ok(PickerOutcome::Cancelled)] {
            let closed = Arc::new(AtomicBool::new(false));
            let (sender, mut receiver) = tokio::sync::oneshot::channel();
            let expected = result.clone();
            Completion {
                deadline,
                closed,
                sender: Some(sender),
            }
            .finish(result);
            assert_eq!(receiver.try_recv().unwrap(), expected);
        }
    }

    #[tokio::test]
    async fn bounded_wait_rejects_late_selection_and_dropped_callback() {
        let deadline = Instant::now() + Duration::from_secs(1);
        let closed = Arc::new(AtomicBool::new(false));
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let completion = Completion {
            deadline,
            closed: closed.clone(),
            sender: Some(sender),
        };
        assert_eq!(
            wait(receiver, &closed, Instant::now() - Duration::from_millis(1)).await,
            Err(PickerError::TimedOut)
        );
        completion.finish(Ok(PickerOutcome::Selected(vec![PathBuf::from("late")])));
        assert!(closed.load(Ordering::SeqCst));
        let (sender, mut late_receiver) = tokio::sync::oneshot::channel();
        Completion {
            deadline,
            closed: closed.clone(),
            sender: Some(sender),
        }
        .finish(Ok(PickerOutcome::Selected(vec![PathBuf::from("late")])));
        assert_eq!(
            late_receiver.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Closed)
        );
        let (sender, receiver) = tokio::sync::oneshot::channel();
        drop(sender);
        assert_eq!(
            wait(receiver, &closed, deadline).await,
            Err(PickerError::Unavailable)
        );
    }
    #[tokio::test]
    async fn ready_selection_after_absolute_deadline_never_reaches_capture() {
        let deadline = Instant::now() - Duration::from_millis(1);
        let closed = Arc::new(AtomicBool::new(false));
        let (sender, receiver) = tokio::sync::oneshot::channel();
        sender
            .send(Ok(PickerOutcome::Selected(vec![PathBuf::from("expired")])))
            .unwrap();
        let result = wait(receiver, &closed, deadline).await;
        let captures = usize::from(matches!(result, Ok(PickerOutcome::Selected(_))));
        assert_eq!(result, Err(PickerError::TimedOut));
        assert_eq!(captures, 0);
        let closed = Arc::new(AtomicBool::new(false));
        let (sender, mut receiver) = tokio::sync::oneshot::channel();
        Completion {
            deadline,
            closed,
            sender: Some(sender),
        }
        .finish(Ok(PickerOutcome::Selected(vec![PathBuf::from("expired")])));
        assert_eq!(receiver.try_recv().unwrap(), Err(PickerError::TimedOut));
    }
}
