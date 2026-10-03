use crate::commands::{ContextSelectionSummary, DesktopState, context_summary};
use cap_std::fs::{MetadataExt, OpenOptions, OpenOptionsExt};
use magi_context::{
    CaptureLimits, EvidenceLocator, ManifestSource, ManifestSourceState, SourceCaptureManifest,
};
use magi_domain::Digest;
use magi_storage::Storage;
use serde::Serialize;
use std::{
    collections::HashMap,
    io::{ErrorKind, Read},
    path::Path,
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Manager, State, WebviewWindow};
use tauri_plugin_dialog::DialogExt;

const TOKEN_TTL: Duration = Duration::from_secs(600);
const MAX_PENDING: usize = 4;
static CAPTURE_CAPACITY: AtomicUsize = AtomicUsize::new(0);
struct CaptureSlot;
impl CaptureSlot {
    fn acquire() -> Result<Self, String> {
        CAPTURE_CAPACITY
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < MAX_PENDING).then_some(count + 1)
            })
            .map_err(|_| {
                "Complete or discard an existing PDF selection before selecting another.".to_owned()
            })?;
        Ok(Self)
    }
}
impl Drop for CaptureSlot {
    fn drop(&mut self) {
        CAPTURE_CAPACITY.fetch_sub(1, Ordering::AcqRel);
    }
}
static PENDING: OnceLock<Mutex<HashMap<String, PendingPdf>>> = OnceLock::new();
type SelectedFileIdentity = (u64, u64, i64, i64);
type SelectedPdfBytes = (Vec<u8>, SelectedFileIdentity);

struct PendingPdf {
    capacity: Option<CaptureSlot>,
    draft_id: String,
    store_identity: magi_storage::StoreIdentity,
    revision: Option<u64>,
    resource_root: std::path::PathBuf,
    bytes: Vec<u8>,
    _selected_identity: SelectedFileIdentity,
    digest: Digest,
    display_name: String,
    total_pages: u32,
    created: Instant,
    applied: Option<(u32, u32, magi_storage::ContextDraft)>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PdfRangePreview {
    selection_token: String,
    draft_id: String,
    context_revision: Option<u64>,
    display_name: String,
    captured_digest: Digest,
    total_pages: u32,
    expires_at_epoch_ms: u64,
}
fn cache() -> &'static Mutex<HashMap<String, PendingPdf>> {
    PENDING.get_or_init(|| Mutex::new(HashMap::new()))
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
fn main_only(window: &WebviewWindow) -> Result<(), String> {
    if window.label() == "main" {
        Ok(())
    } else {
        Err("PDF source selection is available only in the main console.".into())
    }
}
fn check_revision(storage: &Storage, draft_id: &str, revision: Option<u64>) -> Result<(), String> {
    if draft_id.trim().is_empty() || draft_id.len() > 256 {
        return Err("The context draft identifier is invalid.".into());
    }
    if storage
        .load_context_draft(draft_id)
        .map_err(|_| "The PDF selection could not be processed safely.")?
        .as_ref()
        .map(|draft| draft.revision)
        != revision
    {
        return Err("The context draft changed. Select the PDF again.".into());
    }
    Ok(())
}
fn read_selected(path: &Path) -> Result<SelectedPdfBytes, String> {
    read_selected_with_observer(path, || {})
}
fn read_selected_with_observer(
    path: &Path,
    after_parent_pinned: impl FnOnce(),
) -> Result<SelectedPdfBytes, String> {
    let resolved = path
        .canonicalize()
        .map_err(|_| "The selected PDF is unavailable.")?;
    if magi_context::classify_selected_path(path).is_some()
        || magi_context::classify_selected_path(&resolved).is_some()
    {
        return Err("The selected path is not an allowed source.".into());
    }
    let (parent, name) = crate::commands::pin_selected_parent(path)
        .map_err(|_| "The selected PDF path changed or is not an allowed source.")?;
    after_parent_pinned();
    let before = parent
        .symlink_metadata(&name)
        .map_err(|_| "The selected PDF is unavailable.")?;
    let maximum = CaptureLimits::default_policy().max_file_bytes;
    if !before.is_file() || before.len() > maximum {
        return Err("The PDF exceeds the permitted file size or is not a regular file.".into());
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = parent.open_with(&name, &options).map_err(|error| {
        if error.kind() == ErrorKind::PermissionDenied {
            "The selected PDF is unreadable."
        } else {
            "The selected PDF could not be opened safely."
        }
    })?;
    let opened = file
        .metadata()
        .map_err(|_| "PDF metadata is unavailable.")?;
    #[cfg(unix)]
    {
        if opened.dev() != before.dev()
            || opened.ino() != before.ino()
            || opened.len() != before.len()
            || opened.ctime() != before.ctime()
            || opened.ctime_nsec() != before.ctime_nsec()
        {
            return Err("The selected PDF changed before capture.".into());
        }
    }
    let mut bytes = Vec::new();
    (&file)
        .take(maximum + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "The selected PDF could not be read.")?;
    let after = file
        .metadata()
        .map_err(|_| "PDF metadata is unavailable.")?;
    #[cfg(unix)]
    {
        let path_after = parent
            .symlink_metadata(&name)
            .map_err(|_| "The selected PDF changed during capture.")?;
        if path_after.file_type().is_symlink()
            || opened.dev() != path_after.dev()
            || opened.ino() != path_after.ino()
            || opened.len() != path_after.len()
            || opened.ctime() != path_after.ctime()
            || opened.ctime_nsec() != path_after.ctime_nsec()
            || opened.ctime() != after.ctime()
            || opened.ctime_nsec() != after.ctime_nsec()
        {
            return Err("The selected PDF changed during capture.".into());
        }
    }
    if bytes.len() as u64 > maximum
        || bytes.len() as u64 != opened.len()
        || opened.len() != after.len()
        || opened.modified().ok() != after.modified().ok()
    {
        return Err("The selected PDF changed during capture.".into());
    }
    Ok((
        bytes,
        (
            opened.dev(),
            opened.ino(),
            opened.ctime(),
            opened.ctime_nsec(),
        ),
    ))
}

#[tauri::command]
pub(crate) async fn prepare_pdf_range_capture(
    window: WebviewWindow,
    app: AppHandle,
    state: State<'_, DesktopState>,
    draft_id: Option<String>,
    context_revision: Option<u64>,
) -> Result<Option<PdfRangePreview>, String> {
    main_only(&window)?;
    cache()
        .lock()
        .map_err(|_| "PDF selection state is unavailable.")?
        .retain(|_, pending| pending.created.elapsed() < TOKEN_TTL);
    let capacity = CaptureSlot::acquire()?;
    if draft_id.is_some() != context_revision.is_some() {
        return Err("A stored draft ID and revision must be provided together.".into());
    }
    let storage = state.storage()?;
    let draft_id = draft_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    check_revision(&storage, &draft_id, context_revision)?;
    let (sender, receiver) = mpsc::sync_channel(1);
    app.dialog()
        .file()
        .add_filter("PDF", &["pdf"])
        .pick_file(move |path| {
            let _ = sender.send(path);
        });
    let selected = tauri::async_runtime::spawn_blocking(move || receiver.recv())
        .await
        .map_err(|_| "The PDF selection could not be processed safely.")?
        .map_err(|_| "The PDF selection could not be processed safely.")?;
    let Some(selected) = selected else {
        return Ok(None);
    };
    let path = selected
        .into_path()
        .map_err(|_| "The selected PDF is unavailable.")?;
    let resource_root = app
        .path()
        .resource_dir()
        .map_err(|_| "The extraction resources are unavailable.")?;
    tauri::async_runtime::spawn_blocking(move || {
        crate::commands::extraction_helper::ensure(&resource_root)?;
        let (bytes, selected_identity) = read_selected(&path)?;
        let total_pages = magi_context::captured_pdf_page_count(&bytes)?;
        check_revision(&storage, &draft_id, context_revision)?;
        let digest = Digest::from_bytes(&bytes);
        let display_name: String = path
            .file_name()
            .map(|name| {
                name.to_string_lossy()
                    .chars()
                    .filter(|c| !c.is_control())
                    .take(160)
                    .collect()
            })
            .unwrap_or_else(|| "Selected PDF".into());
        let token = uuid::Uuid::new_v4().to_string();
        let preview = PdfRangePreview {
            selection_token: token.clone(),
            draft_id: draft_id.clone(),
            context_revision,
            display_name: display_name.clone(),
            captured_digest: digest.clone(),
            total_pages,
            expires_at_epoch_ms: now() + TOKEN_TTL.as_millis() as u64,
        };
        let mut cache = cache()
            .lock()
            .map_err(|_| "PDF selection state is unavailable.")?;
        cache.retain(|_, pending| pending.created.elapsed() < TOKEN_TTL);
        if cache
            .values()
            .filter(|pending| pending.applied.is_none())
            .count()
            >= MAX_PENDING
        {
            return Err(
                "Too many pending PDF selections. Complete a selection or wait for its expiration."
                    .into(),
            );
        }
        if cache.len() >= 64 {
            return Err(
                "PDF selection receipt capacity reached. Wait for existing selections to expire."
                    .into(),
            );
        }
        cache.insert(
            token,
            PendingPdf {
                capacity: Some(capacity),
                draft_id,
                store_identity: storage.identity().clone(),
                revision: context_revision,
                resource_root,
                bytes,
                _selected_identity: selected_identity,
                digest,
                display_name,
                total_pages,
                created: Instant::now(),
                applied: None,
            },
        );
        Ok(Some(preview))
    })
    .await
    .map_err(|_| "The PDF selection could not be processed safely.")?
}

fn apply_pending(
    storage: &Storage,
    pending: &mut PendingPdf,
    draft_id: &str,
    revision: Option<u64>,
    start_page: u32,
    end_page: u32,
) -> Result<ContextSelectionSummary, String> {
    if storage.identity() != &pending.store_identity
        || pending.created.elapsed() >= TOKEN_TTL
        || pending.draft_id != draft_id
        || pending.revision != revision
    {
        return Err("The PDF selection expired or differs from the current draft.".into());
    }
    if let Some((start, end, saved)) = &pending.applied {
        if (*start, *end) != (start_page, end_page) {
            return Err(
                "The completed PDF selection cannot be retried with a different range.".into(),
            );
        }
        return context_summary(saved.clone());
    }
    if start_page == 0
        || end_page < start_page
        || end_page > pending.total_pages
        || end_page - start_page + 1 > 200
    {
        return Err("Select an explicit PDF range of at most 200 pages.".into());
    }
    check_revision(storage, draft_id, revision)?;
    crate::commands::extraction_helper::ensure(&pending.resource_root)?;
    let extracted = magi_context::extract_pdf_page_selection(&pending.bytes, start_page, end_page)?;
    if extracted
        .pages
        .iter()
        .filter_map(|page| page.text.as_deref())
        .any(magi_context::contains_sensitive_content)
    {
        return Err("The selected PDF text contains a credential-sensitive pattern.".into());
    }
    let representation = serde_json::to_vec(&extracted)
        .map_err(|_| "The PDF selection could not be processed safely.")?;
    let source_id = uuid::Uuid::new_v4().to_string();
    let source = ManifestSource {
        source_id: source_id.clone(),
        display_name: pending.display_name.clone(),
        state: ManifestSourceState::Captured,
        byte_length: Some(pending.bytes.len() as u64),
        mime_type: Some("application/pdf".into()),
        object_digest: Some(pending.digest.clone()),
        derived_digest: Some(Digest::from_bytes(&representation)),
        representation_kind: Some(extracted.kind),
        extractor_id: Some("macos-native-bounded".into()),
        extractor_version: Some("1".into()),
        included_locators: extracted
            .pages
            .iter()
            .map(|page| EvidenceLocator {
                source_id: source_id.clone(),
                object_digest: pending.digest.clone(),
                start_line: None,
                end_line: None,
                total_lines: None,
                page: Some(page.page),
                width: page.width,
                height: page.height,
            })
            .collect(),
        omission: None,
        captured_at_epoch_ms: Some(now()),
        secret_pattern_findings: Vec::new(),
        secret_scan_incomplete: true,
    };
    let mut sources = storage
        .load_context_draft(draft_id)
        .map_err(|_| "The PDF selection could not be processed safely.")?
        .map(|draft| draft.manifest.content.sources)
        .unwrap_or_default();
    let limits = CaptureLimits::default_policy();
    if sources.len() >= limits.max_items
        || sources
            .iter()
            .filter_map(|source| source.byte_length)
            .sum::<u64>()
            .saturating_add(pending.bytes.len() as u64)
            > limits.max_manifest_bytes
    {
        return Err("The selected PDF exceeds the context draft item or byte limit.".into());
    }
    sources.push(source);
    let manifest = SourceCaptureManifest::draft(sources, now())
        .map_err(|_| "The PDF selection could not be processed safely.")?;
    for bytes in [&pending.bytes, &representation] {
        let stored = storage
            .put_source_object(bytes)
            .map_err(|_| "The captured PDF could not be stored safely.")?;
        if stored.digest != Digest::from_bytes(bytes) || stored.byte_length != bytes.len() as u64 {
            return Err("The captured PDF failed local integrity verification.".into());
        }
    }
    let saved = storage
        .save_context_draft(draft_id, revision, &manifest, now())
        .map_err(|_| "The context draft changed or could not be saved safely.")?;
    let result = context_summary(saved.clone())?;
    pending.applied = Some((start_page, end_page, saved));
    pending.capacity.take();
    pending.bytes.clear();
    pending.bytes.shrink_to_fit();
    Ok(result)
}

#[tauri::command]
pub(crate) async fn apply_pdf_range_capture(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    selection_token: String,
    draft_id: String,
    context_revision: Option<u64>,
    start_page: u32,
    end_page: u32,
) -> Result<ContextSelectionSummary, String> {
    main_only(&window)?;
    let storage = state.storage()?;
    tauri::async_runtime::spawn_blocking(move || {
        let mut cache = cache()
            .lock()
            .map_err(|_| "PDF selection state is unavailable.")?;
        cache.retain(|_, pending| pending.created.elapsed() < TOKEN_TTL);
        let pending = cache
            .get_mut(&selection_token)
            .ok_or("The PDF selection is unavailable or expired.")?;
        let result = apply_pending(
            &storage,
            pending,
            &draft_id,
            context_revision,
            start_page,
            end_page,
        )?;
        Ok(result)
    })
    .await
    .map_err(|_| "The PDF selection could not be processed safely.")?
}

#[tauri::command]
pub(crate) async fn discard_pdf_range_capture(
    window: WebviewWindow,
    selection_token: String,
) -> Result<bool, String> {
    main_only(&window)?;
    tauri::async_runtime::spawn_blocking(move || {
        let removed = cache()
            .lock()
            .map_err(|_| "PDF selection state is unavailable.")?
            .remove(&selection_token);
        Ok(removed.is_some())
    })
    .await
    .map_err(|_| "PDF selection state is unavailable.".to_owned())?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    #[test]
    fn pending_pdf_capacity_is_reserved_before_reading_and_released_on_discard() {
        let slots = (0..MAX_PENDING)
            .map(|_| CaptureSlot::acquire().unwrap())
            .collect::<Vec<_>>();
        assert!(CaptureSlot::acquire().is_err());
        drop(slots);
        assert!(CaptureSlot::acquire().is_ok());
    }
    #[test]
    #[ignore = "Requires explicit owned signed installation in a dedicated process."]
    fn signed_pdf_range_freezes_bytes_and_fences_draft_and_retry() {
        let mut installation =
            crate::commands::resource_quiescence_tests::SignedInstallationFixture::open();
        let resource_root = installation.publication.clone();
        let root = installation.create_capture_root("pdf-range");
        let path = root.join("Selected.pdf");
        let original = many_page_pdf(3);
        fs::write(&path, &original).unwrap();
        let (bytes, selected_identity) = read_selected(&path).unwrap();
        let storage = Storage::open_or_create(root.join("store")).unwrap();
        let mut pending = PendingPdf {
            capacity: None,
            draft_id: "range-draft".into(),
            store_identity: storage.identity().clone(),
            revision: None,
            resource_root,
            digest: Digest::from_bytes(&bytes),
            total_pages: magi_context::captured_pdf_page_count(&bytes).unwrap(),
            bytes,
            _selected_identity: selected_identity,
            display_name: "Selected.pdf".into(),
            created: Instant::now(),
            applied: None,
        };
        assert_eq!(pending.total_pages, 3);
        assert!(storage.load_context_draft("range-draft").unwrap().is_none());
        fs::write(&path, b"replacement after selected snapshot").unwrap();
        assert!(apply_pending(&storage, &mut pending, "wrong-draft", None, 2, 2).is_err());
        assert!(apply_pending(&storage, &mut pending, "range-draft", Some(0), 2, 2).is_err());
        assert!(apply_pending(&storage, &mut pending, "range-draft", None, 0, 2).is_err());
        assert!(apply_pending(&storage, &mut pending, "range-draft", None, 1, 4).is_err());
        assert!(storage.load_context_draft("range-draft").unwrap().is_none());
        apply_pending(&storage, &mut pending, "range-draft", None, 2, 2).unwrap();
        let saved = storage.load_context_draft("range-draft").unwrap().unwrap();
        let source = &saved.manifest.content.sources[0];
        assert_eq!(saved.manifest.schema_version, 2);
        assert_eq!(source.included_locators.len(), 1);
        assert_eq!(source.included_locators[0].page, Some(2));
        assert_eq!(
            storage
                .read_source_object(source.object_digest.as_ref().unwrap())
                .unwrap(),
            original
        );
        let derived = String::from_utf8(
            storage
                .read_source_object(source.derived_digest.as_ref().unwrap())
                .unwrap(),
        )
        .unwrap();
        assert!(derived.contains("private-page-2"));
        assert!(!derived.contains("private-page-1"));
        assert!(!derived.contains("private-page-3"));
        apply_pending(&storage, &mut pending, "range-draft", None, 2, 2).unwrap();
        assert_eq!(
            storage
                .load_context_draft("range-draft")
                .unwrap()
                .unwrap()
                .revision,
            saved.revision
        );
        assert!(apply_pending(&storage, &mut pending, "range-draft", None, 1, 1).is_err());
        assert!(pending.bytes.is_empty());
        pending.created = Instant::now() - TOKEN_TTL;
        assert!(apply_pending(&storage, &mut pending, "range-draft", None, 2, 2).is_err());
        std::os::unix::fs::symlink(&path, root.join("Linked.pdf")).unwrap();
        assert!(read_selected(&root.join("Linked.pdf")).is_err());
        drop(pending);
        drop(storage);
        installation.finish();
        assert!(!root.exists());
    }
    #[test]
    fn pinned_parent_preserves_selected_inode_despite_ancestor_namespace_replacement() {
        let root = std::env::temp_dir().join(format!("pdf-parent-{}", uuid::Uuid::new_v4()));
        let selected = root.join("Public");
        let credential = root.join(".codex");
        fs::create_dir_all(&selected).unwrap();
        fs::create_dir_all(&credential).unwrap();
        let original = many_page_pdf(1);
        fs::write(selected.join("Selected.pdf"), &original).unwrap();
        fs::write(credential.join("Selected.pdf"), b"private_token_canary").unwrap();
        let path = selected.join("Selected.pdf");
        let (bytes, _) = read_selected_with_observer(&path, || {
            fs::rename(&selected, root.join("Original")).unwrap();
            std::os::unix::fs::symlink(&credential, &selected).unwrap();
        })
        .unwrap();
        assert_eq!(bytes, original);
        assert!(read_selected(&path).is_err());
        assert_eq!(
            fs::read(credential.join("Selected.pdf")).unwrap(),
            b"private_token_canary"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn stale_stored_draft_and_oversized_range_cannot_publish_selected_bytes() {
        let root = std::env::temp_dir().join(format!("pdf-fence-{}", uuid::Uuid::new_v4()));
        let storage = Storage::open_or_create(&root).unwrap();
        let bytes = many_page_pdf(3);
        let mut pending = PendingPdf {
            capacity: None,
            draft_id: "draft".into(),
            store_identity: storage.identity().clone(),
            revision: None,
            resource_root: root.join("unavailable-extraction-resources"),
            digest: Digest::from_bytes(&bytes),
            total_pages: 201,
            bytes,
            _selected_identity: (0, 0, 0, 0),
            display_name: "Selected.pdf".into(),
            created: Instant::now(),
            applied: None,
        };
        assert!(apply_pending(&storage, &mut pending, "draft", None, 1, 201).is_err());
        pending.store_identity.generation += 1;
        assert!(apply_pending(&storage, &mut pending, "draft", None, 2, 2).is_err());
        pending.store_identity = storage.identity().clone();
        let manifest = SourceCaptureManifest::draft(Vec::new(), now()).unwrap();
        let existing = storage
            .save_context_draft("draft", None, &manifest, now())
            .unwrap();
        assert!(apply_pending(&storage, &mut pending, "draft", None, 2, 2).is_err());
        let unchanged = storage.load_context_draft("draft").unwrap().unwrap();
        assert_eq!(unchanged.revision, existing.revision);
        assert!(unchanged.manifest.content.sources.is_empty());
        assert!(!pending.bytes.is_empty());
        drop(storage);
        fs::remove_dir_all(root).unwrap();
    }

    const PNG: [u8; 69] = [
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 2,
        0, 0, 0, 144, 119, 83, 222, 0, 0, 0, 12, 73, 68, 65, 84, 120, 156, 99, 248, 207, 192, 0, 0,
        3, 1, 1, 0, 201, 254, 146, 239, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
    ];
    #[test]
    #[ignore = "Requires explicit owned signed installation in a dedicated process."]
    fn signed_native_image_capture_persists_raw_and_derived_objects() {
        let mut installation =
            crate::commands::resource_quiescence_tests::SignedInstallationFixture::open();
        let root = installation.create_capture_root("image-capture");
        let path = root.join("Selected.png");
        fs::write(&path, PNG).unwrap();
        let storage = std::sync::Arc::new(Storage::open_or_create(root.join("store")).unwrap());
        crate::commands::capture_context_files(
            storage.clone(),
            "image-draft".into(),
            None,
            vec![path],
            &installation.publication,
        )
        .unwrap();
        let draft = storage.load_context_draft("image-draft").unwrap().unwrap();
        let source = &draft.manifest.content.sources[0];
        assert_eq!(
            source.representation_kind,
            Some(magi_context::RepresentationKind::Image)
        );
        assert_eq!(source.included_locators[0].width, Some(1));
        assert_eq!(source.included_locators[0].height, Some(1));
        assert_eq!(
            storage
                .read_source_object(source.object_digest.as_ref().unwrap())
                .unwrap(),
            PNG
        );
        let derived = storage
            .read_source_object(source.derived_digest.as_ref().unwrap())
            .unwrap();
        assert_ne!(derived, PNG);
        assert_eq!(
            Digest::from_bytes(&derived),
            *source.derived_digest.as_ref().unwrap()
        );
        let parsed: magi_context::NativeExtraction = serde_json::from_slice(&derived).unwrap();
        assert!(parsed.image_base64.is_some());
        assert!(parsed.pages.is_empty());
        drop(storage);
        installation.finish();
        assert!(!root.exists());
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
}

fn frozen_pdf_representation(
    storage: &Storage,
    draft_id: &str,
    revision: u64,
    source_id: &str,
) -> Result<
    (
        magi_storage::ContextDraft,
        usize,
        magi_context::NativeExtraction,
    ),
    String,
> {
    let draft = storage
        .load_context_draft(draft_id)
        .map_err(|_| "pdf_context_unavailable")?
        .ok_or("pdf_context_unavailable")?;
    if draft.revision != revision {
        return Err("pdf_context_revision_changed".into());
    }
    let index = draft
        .manifest
        .content
        .sources
        .iter()
        .position(|source| source.source_id == source_id)
        .ok_or("pdf_source_unavailable")?;
    let source = &draft.manifest.content.sources[index];
    if source.state != ManifestSourceState::Captured
        || source.mime_type.as_deref() != Some("application/pdf")
    {
        return Err("pdf_source_not_captured".into());
    }
    let digest = source
        .derived_digest
        .as_ref()
        .ok_or("pdf_representation_unavailable")?;
    let bytes = storage
        .read_source_object(digest)
        .map_err(|_| "pdf_representation_unavailable")?;
    if Digest::from_bytes(&bytes) != *digest {
        return Err("pdf_representation_digest_mismatch".into());
    }
    let extracted: magi_context::NativeExtraction =
        serde_json::from_slice(&bytes).map_err(|_| "pdf_representation_invalid")?;
    if extracted.schema_version != 1
        || extracted.mime_type != "application/pdf"
        || extracted.pages.is_empty()
        || extracted.pages.len() != source.included_locators.len()
        || extracted.kind
            != source
                .representation_kind
                .ok_or("pdf_representation_invalid")?
    {
        return Err("pdf_representation_invalid".into());
    }
    let mut previous = 0;
    for (page, locator) in extracted.pages.iter().zip(&source.included_locators) {
        if page.page <= previous
            || locator.page != Some(page.page)
            || locator.source_id != source_id
            || Some(&locator.object_digest) != source.object_digest.as_ref()
            || locator.width != page.width
            || locator.height != page.height
        {
            return Err("pdf_page_locator_mismatch".into());
        }
        previous = page.page;
    }
    Ok((draft, index, extracted))
}

#[tauri::command]
pub(crate) async fn load_context_pdf_page_count(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    draft_id: String,
    context_revision: u64,
    source_id: String,
) -> Result<u32, String> {
    main_only(&window)?;
    let storage = state.storage()?;
    tauri::async_runtime::spawn_blocking(move || {
        let (_, _, extracted) =
            frozen_pdf_representation(&storage, &draft_id, context_revision, &source_id)?;
        // The stored worker metadata is authoritative; no original/live PDF is read here.
        extracted
            .total_pages
            .ok_or("pdf_total_page_count_unavailable".into())
    })
    .await
    .map_err(|_| "pdf_page_count_worker_failed".to_owned())?
}

fn shrink_pdf_selection(
    storage: &Storage,
    draft_id: &str,
    revision: u64,
    source_id: &str,
    start_page: u32,
    end_page: u32,
) -> Result<ContextSelectionSummary, String> {
    let (draft, index, mut extracted) =
        frozen_pdf_representation(storage, draft_id, revision, source_id)?;
    if start_page == 0 || end_page < start_page || end_page - start_page + 1 > 200 {
        return Err("pdf_page_range_invalid".into());
    }
    if (start_page..=end_page).any(|number| !extracted.pages.iter().any(|page| page.page == number))
    {
        return Err("pdf_range_expansion_requires_explicit_recapture".into());
    }
    let selected = extracted
        .pages
        .iter()
        .filter(|page| page.page >= start_page && page.page <= end_page)
        .count();
    if selected == extracted.pages.len() {
        return context_summary(draft);
    }
    extracted
        .pages
        .retain(|page| page.page >= start_page && page.page <= end_page);
    let bytes = serde_json::to_vec(&extracted).map_err(|_| "pdf_representation_invalid")?;
    let digest = Digest::from_bytes(&bytes);
    let stored = storage
        .put_source_object(&bytes)
        .map_err(|_| "pdf_representation_save_failed")?;
    if stored.digest != digest || stored.byte_length != bytes.len() as u64 {
        return Err("pdf_representation_digest_mismatch".into());
    }
    let mut sources = draft.manifest.content.sources;
    sources[index].derived_digest = Some(digest);
    sources[index].included_locators.retain(|locator| {
        locator
            .page
            .is_some_and(|page| page >= start_page && page <= end_page)
    });
    let manifest =
        SourceCaptureManifest::draft(sources, now()).map_err(|_| "pdf_manifest_invalid")?;
    let saved = storage
        .save_context_draft(draft_id, Some(revision), &manifest, now())
        .map_err(|_| "pdf_context_revision_changed")?;
    context_summary(saved)
}

#[tauri::command]
pub(crate) async fn select_context_pdf_pages(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    draft_id: String,
    context_revision: u64,
    source_id: String,
    start_page: u32,
    end_page: u32,
) -> Result<ContextSelectionSummary, String> {
    main_only(&window)?;
    let storage = state.storage()?;
    tauri::async_runtime::spawn_blocking(move || {
        shrink_pdf_selection(
            &storage,
            &draft_id,
            context_revision,
            &source_id,
            start_page,
            end_page,
        )
    })
    .await
    .map_err(|_| "pdf_page_selection_worker_failed".to_owned())?
}

#[cfg(test)]
mod retained_pdf_range_tests {
    use super::*;
    #[test]
    fn retained_pages_shrink_with_cas_and_cannot_expand_without_recapture() {
        let root = std::env::temp_dir().join(format!("magi-pdf-shrink-{}", uuid::Uuid::new_v4()));
        let storage = Storage::open_or_create(&root).unwrap();
        let original = storage
            .put_source_object(b"immutable selected original")
            .unwrap();
        let extraction = magi_context::NativeExtraction {
            schema_version: 1,
            kind: magi_context::RepresentationKind::PdfText,
            mime_type: "application/pdf".into(),
            width: None,
            height: None,
            image_base64: None,
            warnings: Vec::new(),
            total_pages: Some(9),
            pages: (2..=3)
                .map(|page| magi_context::ExtractedPage {
                    page,
                    text: Some(format!("page-{page}")),
                    mime_type: None,
                    image_base64: None,
                    width: None,
                    height: None,
                })
                .collect(),
        };
        let derived = storage
            .put_source_object(&serde_json::to_vec(&extraction).unwrap())
            .unwrap();
        let source = ManifestSource {
            source_id: "pdf-source".into(),
            display_name: "selected.pdf".into(),
            state: ManifestSourceState::Captured,
            byte_length: Some(original.byte_length),
            mime_type: Some("application/pdf".into()),
            object_digest: Some(original.digest.clone()),
            derived_digest: Some(derived.digest),
            representation_kind: Some(extraction.kind),
            extractor_id: Some("macos-native-bounded".into()),
            extractor_version: Some("1".into()),
            included_locators: (2..=3)
                .map(|page| EvidenceLocator {
                    source_id: "pdf-source".into(),
                    object_digest: original.digest.clone(),
                    start_line: None,
                    end_line: None,
                    total_lines: None,
                    page: Some(page),
                    width: None,
                    height: None,
                })
                .collect(),
            omission: None,
            captured_at_epoch_ms: Some(now()),
            secret_pattern_findings: Vec::new(),
            secret_scan_incomplete: true,
        };
        let manifest = SourceCaptureManifest::draft(vec![source], now()).unwrap();
        let draft = storage
            .save_context_draft("pdf-draft", None, &manifest, now())
            .unwrap();
        assert!(
            shrink_pdf_selection(&storage, "pdf-draft", draft.revision, "pdf-source", 1, 3)
                .is_err()
        );
        shrink_pdf_selection(&storage, "pdf-draft", draft.revision, "pdf-source", 2, 2).unwrap();
        let current = storage.load_context_draft("pdf-draft").unwrap().unwrap();
        assert_eq!(current.revision, draft.revision + 1);
        assert_eq!(
            current.manifest.content.sources[0].object_digest,
            Some(original.digest.clone())
        );
        let bytes = storage
            .read_source_object(
                current.manifest.content.sources[0]
                    .derived_digest
                    .as_ref()
                    .unwrap(),
            )
            .unwrap();
        let stored: magi_context::NativeExtraction = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(stored.pages.len(), 1);
        assert_eq!(stored.pages[0].page, 2);
        assert_eq!(stored.total_pages, Some(9));
        assert!(
            shrink_pdf_selection(&storage, "pdf-draft", draft.revision, "pdf-source", 2, 2)
                .is_err()
        );
        assert!(
            shrink_pdf_selection(&storage, "pdf-draft", current.revision, "pdf-source", 2, 3)
                .is_err()
        );
        assert_eq!(
            storage.read_source_object(&original.digest).unwrap(),
            b"immutable selected original"
        );
        drop(storage);
        std::fs::remove_dir_all(root).unwrap();
    }
}
