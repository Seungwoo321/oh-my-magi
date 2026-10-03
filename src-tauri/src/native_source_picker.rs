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

#[cfg(target_os = "macos")]
thread_local! {
    static PANELS: std::cell::RefCell<std::collections::HashMap<uuid::Uuid, objc2::rc::Retained<objc2_app_kit::NSOpenPanel>>> = std::cell::RefCell::new(std::collections::HashMap::new());
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
        // A dropped IPC future must close only its own sheet; a late callback cannot capture.
        let _ = self.window.run_on_main_thread(move || {
            let panel = PANELS.with(|panels| panels.borrow_mut().remove(&token));
            if let Some(panel) = panel {
                unsafe { panel.cancel(None) };
            }
        });
    }
}

#[cfg(target_os = "macos")]
pub(crate) async fn select(
    window: &tauri::WebviewWindow,
    directory: bool,
) -> Result<PickerOutcome, PickerError> {
    use block2::RcBlock;
    use objc2::{ClassType, MainThreadMarker, msg_send, rc::Retained};
    use objc2_app_kit::{NSModalResponseCancel, NSModalResponseOK, NSOpenPanel, NSWindow};
    use std::cell::RefCell;
    let deadline = Instant::now() + Duration::from_secs(120);
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
        let Some(_main) = MainThreadMarker::new() else {
            completion.finish(Err(PickerError::Unavailable));
            return;
        };
        if completion.closed.load(Ordering::SeqCst) {
            return;
        }
        if Instant::now() >= completion.deadline {
            completion.finish(Err(PickerError::TimedOut));
            return;
        }
        if PANELS.with(|panels| !panels.borrow().is_empty()) {
            completion.finish(Err(PickerError::Unavailable));
            return;
        }
        #[cfg(feature = "native-live-probe")]
        {
            let bundle = objc2_foundation::NSBundle::mainBundle().bundleIdentifier();
            let host_matches = bundle.as_ref().is_some_and(|id| {
                id.to_string() == "local.magi.installed.probe"
            });
            let application = objc2_app_kit::NSApplication::sharedApplication(_main);
            let activation_policy: isize = unsafe { msg_send![&*application, activationPolicy] };
            eprintln!("Native source picker host: bundle_present={}, installed_probe_identity={}, activation_policy={}", bundle.is_some(), host_matches, activation_policy);
        }

        let parent = owned_window.ns_window().ok().and_then(|pointer| {
            // Retain the actual Tauri parent on its owning thread for the sheet presentation.
            unsafe { Retained::<NSWindow>::retain(pointer.cast()) }
        });
        let Some(parent) = parent else {
            completion.finish(Err(PickerError::Unavailable));
            return;
        };
        // AppKit can return nil when its view service cannot create a panel.
        // Its generated nonnullable binding would panic across the main-thread callback.
        let panel: Option<Retained<NSOpenPanel>> =
            unsafe { msg_send![NSOpenPanel::class(), openPanel] };
        let Some(panel) = panel else {
            completion.finish(Err(PickerError::Unavailable));
            return;
        };
        panel.setCanChooseFiles(!directory);
        panel.setCanChooseDirectories(directory);
        panel.setAllowsMultipleSelection(!directory);
        let completion = RefCell::new(Some(completion));
        let callback = RcBlock::new(move |response: isize| {
            let panel = PANELS.with(|panels| panels.borrow_mut().remove(&token));
            let Some(completion) = completion.borrow_mut().take() else {
                return;
            };
            if completion.closed.load(Ordering::SeqCst) {
                return;
            }
            let result = if response == NSModalResponseCancel {
                Ok(PickerOutcome::Cancelled)
            } else if response != NSModalResponseOK {
                Err(PickerError::Unavailable)
            } else if let Some(panel) = panel {
                let urls = panel.URLs();
                let mut paths = Vec::new();
                let mut valid = urls.count() > 0 && urls.count() <= 1000;
                if valid {
                    for url in urls.iter() {
                        if !url.isFileURL() {
                            valid = false;
                            break;
                        }
                        match url.path() {
                            Some(path) => paths.push(PathBuf::from(path.to_string())),
                            None => {
                                valid = false;
                                break;
                            }
                        }
                    }
                }
                if valid && (!directory || paths.len() == 1) {
                    Ok(PickerOutcome::Selected(paths))
                } else {
                    Err(PickerError::InvalidSelection)
                }
            } else {
                Err(PickerError::Unavailable)
            };
            completion.finish(result);
        });
        PANELS.with(|panels| panels.borrow_mut().insert(token, panel.clone()));
        // AppKit copies the block and retains the sheet while it is presented.
        panel.beginSheetModalForWindow_completionHandler(&parent, &callback);
    })
    .map_err(|_| PickerError::Unavailable)?;
    wait(receiver, &closed, deadline).await
}

#[cfg(not(target_os = "macos"))]
pub(crate) async fn select(
    window: &tauri::WebviewWindow,
    directory: bool,
) -> Result<PickerOutcome, PickerError> {
    use tauri_plugin_dialog::DialogExt;
    let deadline = Instant::now() + Duration::from_secs(120);
    let closed = Arc::new(AtomicBool::new(false));
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let completion = Completion {
        deadline,
        closed: closed.clone(),
        sender: Some(sender),
    };
    let picker = window.dialog().file().set_parent(window);
    if directory {
        picker.pick_folder(move |path| {
            let result = match path {
                None => Ok(PickerOutcome::Cancelled),
                Some(path) => path
                    .into_path()
                    .map(|path| PickerOutcome::Selected(vec![path]))
                    .map_err(|_| PickerError::InvalidSelection),
            };
            completion.finish(result);
        });
    } else {
        picker.pick_files(move |paths| {
            let result = match paths {
                None => Ok(PickerOutcome::Cancelled),
                Some(paths) if !paths.is_empty() && paths.len() <= 1000 => paths
                    .into_iter()
                    .map(|path| path.into_path().map_err(|_| PickerError::InvalidSelection))
                    .collect::<Result<Vec<_>, _>>()
                    .map(PickerOutcome::Selected),
                Some(_) => Err(PickerError::InvalidSelection),
            };
            completion.finish(result);
        });
    }
    wait(receiver, &closed, deadline).await
}

#[cfg(test)]
mod tests {
    use super::*;

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
