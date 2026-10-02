use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};

use cap_std::ambient_authority;
use cap_std::fs::{Dir, File, Metadata, MetadataExt, OpenOptions, OpenOptionsExt};
use magi_domain::Digest;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::manifest::{
    EvidenceLocator, ManifestSource, ManifestSourceState, RepresentationKind, SecretPatternKind,
    SourceCaptureManifest, SourceOmission, SourceOmissionCode,
};

pub const DEFAULT_FILE_BYTES: u64 = 20 * 1024 * 1024;
pub const DEFAULT_MANIFEST_BYTES: u64 = 100 * 1024 * 1024;
pub const DEFAULT_MANIFEST_ITEMS: usize = 200;
pub const MAX_FILE_BYTES: u64 = 100 * 1024 * 1024;
pub const MAX_MANIFEST_BYTES: u64 = 500 * 1024 * 1024;
pub const MAX_MANIFEST_ITEMS: usize = 1_000;
const EXTRACTOR_ID: &str = "utf8-text";
const EXTRACTOR_VERSION: &str = "1";
const MAX_CONTEXT_READ_BYTES: u64 = 20 * 1024 * 1024;
const MAX_CONTEXT_READ_RESPONSE_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptureLimits {
    pub max_items: usize,
    pub max_file_bytes: u64,
    pub max_manifest_bytes: u64,
}

impl CaptureLimits {
    pub const fn default_policy() -> Self {
        Self {
            max_items: DEFAULT_MANIFEST_ITEMS,
            max_file_bytes: DEFAULT_FILE_BYTES,
            max_manifest_bytes: DEFAULT_MANIFEST_BYTES,
        }
    }

    pub fn user_confirmed(
        max_items: usize,
        max_file_bytes: u64,
        max_manifest_bytes: u64,
    ) -> Result<Self, CaptureError> {
        let limits = Self {
            max_items,
            max_file_bytes,
            max_manifest_bytes,
        };
        limits.validate()?;
        Ok(limits)
    }

    fn validate(self) -> Result<(), CaptureError> {
        if self.max_items == 0
            || self.max_items > MAX_MANIFEST_ITEMS
            || self.max_file_bytes == 0
            || self.max_file_bytes > MAX_FILE_BYTES
            || self.max_manifest_bytes == 0
            || self.max_manifest_bytes > MAX_MANIFEST_BYTES
        {
            return Err(CaptureError::InvalidLimits);
        }
        Ok(())
    }
}

impl Default for CaptureLimits {
    fn default() -> Self {
        Self::default_policy()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineRange {
    pub start_line: u64,
    pub end_line: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureDirective {
    pub source_id: String,
    pub line_range: Option<LineRange>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CandidateSummary {
    pub source_id: String,
    pub display_name: String,
    pub byte_length: Option<u64>,
    pub mime_type: Option<String>,
    pub omission: Option<SourceOmission>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EnumerationReport {
    pub grant_id: String,
    pub candidates: Vec<CandidateSummary>,
    pub complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CaptureProblem {
    pub source_id: String,
    pub state: ManifestSourceState,
    pub omission: SourceOmission,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CaptureReport {
    pub captured_source_ids: Vec<String>,
    pub problems: Vec<CaptureProblem>,
    pub total_original_bytes: u64,
}

pub struct CaptureBatch {
    pub manifest: SourceCaptureManifest,
    pub objects: Vec<CapturedObject>,
    pub report: CaptureReport,
    pub enumeration_complete: bool,
}

pub struct CapturedObject {
    pub source_id: String,
    pub object_digest: Digest,
    pub derived_digest: Digest,
    pub byte_length: u64,
    raw_bytes: Vec<u8>,
    pub representation: ContentRepresentation,
}

impl CapturedObject {
    pub fn original_bytes(&self) -> &[u8] {
        &self.raw_bytes
    }

    pub fn into_original_bytes(self) -> Vec<u8> {
        self.raw_bytes
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct ContentRepresentation {
    pub kind: RepresentationKind,
    pub text: String,
    pub locator: EvidenceLocator,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum CaptureError {
    #[error("source permission has been revoked")]
    RevokedGrant,
    #[error("the path is not a valid approved source directory")]
    InvalidSourceScope,
    #[error("the requested file is outside the approved source scope or unavailable")]
    SourceScopeDenied,
    #[error("the requested source could not be read as bounded UTF-8 text")]
    SourceTextUnavailable,
    #[error("capture limit is outside the supported range")]
    InvalidLimits,
    #[error("selection contains an unknown or duplicate source")]
    InvalidSelection,
    #[error("the selected directory could not be read")]
    DirectoryReadFailed,
    #[error("the selected file could not be inspected")]
    FileMetadataFailed,
    #[error("context manifest could not be finalized")]
    ManifestFailed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceScopeRoot {
    canonical_path: PathBuf,
    device: u64,
    inode: u64,
}

impl SourceScopeRoot {
    pub fn from_user_input(input: &str, home: &Path) -> Result<Self, CaptureError> {
        if input.is_empty() || input.len() > 4096 || input.contains('\0') {
            return Err(CaptureError::InvalidSourceScope);
        }
        let canonical_home = home
            .canonicalize()
            .map_err(|_| CaptureError::InvalidSourceScope)?;
        let path = if input == "~" {
            canonical_home.clone()
        } else if let Some(suffix) = input.strip_prefix("~/") {
            canonical_home.join(suffix)
        } else if input.starts_with('~') {
            return Err(CaptureError::InvalidSourceScope);
        } else {
            PathBuf::from(input)
        };
        if !path.is_absolute() {
            return Err(CaptureError::InvalidSourceScope);
        }
        let canonical_path = normalize_absolute_path(&path)?;
        if input.starts_with('~') && !canonical_path.starts_with(&canonical_home) {
            return Err(CaptureError::InvalidSourceScope);
        }
        if classify_selected_path(&canonical_path).is_some() {
            return Err(CaptureError::InvalidSourceScope);
        }
        Self::open_verified(canonical_path, None)
    }

    pub fn from_persisted(
        canonical_path: PathBuf,
        device: u64,
        inode: u64,
    ) -> Result<Self, CaptureError> {
        if !canonical_path.is_absolute()
            || normalize_absolute_path(&canonical_path)? != canonical_path
            || classify_selected_path(&canonical_path).is_some()
        {
            return Err(CaptureError::InvalidSourceScope);
        }
        Self::open_verified(canonical_path, Some((device, inode)))
    }

    fn open_verified(
        canonical_path: PathBuf,
        expected_identity: Option<(u64, u64)>,
    ) -> Result<Self, CaptureError> {
        let root = open_absolute_directory_nofollow(&canonical_path)
            .map_err(|_| CaptureError::InvalidSourceScope)?;
        let metadata = root
            .dir_metadata()
            .map_err(|_| CaptureError::InvalidSourceScope)?;
        if !metadata.is_dir()
            || canonical_path.canonicalize().ok().as_deref() != Some(canonical_path.as_path())
            || expected_identity
                .is_some_and(|(device, inode)| metadata.dev() != device || metadata.ino() != inode)
        {
            return Err(CaptureError::InvalidSourceScope);
        }
        Ok(Self {
            canonical_path,
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }

    pub fn canonical_path(&self) -> &Path {
        &self.canonical_path
    }

    pub fn device(&self) -> u64 {
        self.device
    }

    pub fn inode(&self) -> u64 {
        self.inode
    }

    pub fn read_text_file(
        &self,
        requested_path: &Path,
        line: Option<u32>,
        limit: Option<u32>,
    ) -> Result<String, CaptureError> {
        let relative = requested_path
            .strip_prefix(&self.canonical_path)
            .map_err(|_| CaptureError::SourceScopeDenied)?;
        if relative.as_os_str().is_empty() || classify_selected_path(relative).is_some() {
            return Err(CaptureError::SourceScopeDenied);
        }
        let components = safe_components(relative).map_err(|_| CaptureError::SourceScopeDenied)?;
        if components.is_empty() {
            return Err(CaptureError::SourceScopeDenied);
        }
        let root = open_absolute_directory_nofollow(&self.canonical_path)
            .map_err(|_| CaptureError::SourceScopeDenied)?;
        let root_metadata = root
            .dir_metadata()
            .map_err(|_| CaptureError::SourceScopeDenied)?;
        if root_metadata.dev() != self.device || root_metadata.ino() != self.inode {
            return Err(CaptureError::SourceScopeDenied);
        }
        let before = metadata_within_nofollow(&root, relative)
            .map_err(|_| CaptureError::SourceScopeDenied)?;
        if before.file_type().is_symlink()
            || !before.is_file()
            || before.dev() != self.device
            || before.len() > MAX_CONTEXT_READ_BYTES
        {
            return Err(CaptureError::SourceScopeDenied);
        }
        let mut file = open_file_within_nofollow(&root, relative)
            .map_err(|_| CaptureError::SourceScopeDenied)?;
        let opened = file
            .metadata()
            .map_err(|_| CaptureError::SourceTextUnavailable)?;
        let expected = FileIdentity::from_metadata(&before);
        if !expected.same_file_and_version(FileIdentity::from_metadata(&opened)) {
            return Err(CaptureError::SourceScopeDenied);
        }
        let mut bytes = Vec::with_capacity(opened.len() as usize);
        (&mut file)
            .take(MAX_CONTEXT_READ_BYTES.saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(|_| CaptureError::SourceTextUnavailable)?;
        let after = file
            .metadata()
            .map_err(|_| CaptureError::SourceTextUnavailable)?;
        let current = metadata_within_nofollow(&root, relative)
            .map_err(|_| CaptureError::SourceScopeDenied)?;
        if bytes.len() as u64 > MAX_CONTEXT_READ_BYTES
            || !expected.same_file_and_version(FileIdentity::from_metadata(&after))
            || !expected.same_file_and_version(FileIdentity::from_metadata(&current))
        {
            return Err(CaptureError::SourceScopeDenied);
        }
        let text = std::str::from_utf8(&bytes).map_err(|_| CaptureError::SourceTextUnavailable)?;
        if text.as_bytes().contains(&0) {
            return Err(CaptureError::SourceTextUnavailable);
        }
        let start = line.unwrap_or(1);
        if start == 0 {
            return Err(CaptureError::SourceTextUnavailable);
        }
        let count = limit.map_or(usize::MAX, |value| value as usize);
        let start_index = start as usize;
        let mut line_count = 0usize;
        let mut selected = String::new();
        for segment in text.split_inclusive('\n') {
            line_count = line_count.saturating_add(1);
            if count == 0 || line_count < start_index {
                continue;
            }
            if selected.len().saturating_add(segment.len()) > MAX_CONTEXT_READ_RESPONSE_BYTES {
                return Err(CaptureError::SourceTextUnavailable);
            }
            selected.push_str(segment);
            if line_count.saturating_sub(start_index).saturating_add(1) >= count {
                break;
            }
        }
        if start_index > line_count.saturating_add(1) || contains_sensitive_content(&selected) {
            return Err(CaptureError::SourceTextUnavailable);
        }
        Ok(selected)
    }
}

pub struct SourceGrant {
    grant_id: String,
    scope: Option<GrantedScope>,
    candidates: BTreeMap<String, Candidate>,
    enumeration_complete: bool,
}

enum GrantedScope {
    Directory {
        root: Dir,
        root_device: u64,
        display_name: String,
    },
    SelectedFiles {
        files: BTreeMap<String, GrantedFile>,
    },
}

struct GrantedFile {
    file: File,
    identity: FileIdentity,
}

#[derive(Clone)]
struct Candidate {
    id: String,
    relative_path: Option<PathBuf>,
    display_name: String,
    identity: Option<FileIdentity>,
    byte_length: Option<u64>,
    mime_type: Option<String>,
    omission: Option<SourceOmission>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileIdentity {
    device: u64,
    inode: u64,
    length: u64,
    modified_seconds: i64,
    modified_nanoseconds: i64,
    changed_seconds: i64,
    changed_nanoseconds: i64,
}

impl FileIdentity {
    fn from_metadata(metadata: &Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            length: metadata.len(),
            modified_seconds: metadata.mtime(),
            modified_nanoseconds: metadata.mtime_nsec(),
            changed_seconds: metadata.ctime(),
            changed_nanoseconds: metadata.ctime_nsec(),
        }
    }

    fn same_file_and_version(self, other: Self) -> bool {
        self.device == other.device
            && self.inode == other.inode
            && self.length == other.length
            && self.modified_seconds == other.modified_seconds
            && self.modified_nanoseconds == other.modified_nanoseconds
            && self.changed_seconds == other.changed_seconds
            && self.changed_nanoseconds == other.changed_nanoseconds
    }
}

impl SourceGrant {
    /// The caller passes read-only handles acquired from the native file picker.
    /// Absolute paths never enter this crate's public data model or manifest.
    pub fn selected_files(files: Vec<(File, String)>) -> Result<Self, CaptureError> {
        if files.is_empty() || files.len() > MAX_MANIFEST_ITEMS {
            return Err(CaptureError::InvalidSelection);
        }
        let mut candidates = BTreeMap::new();
        let mut granted_files = BTreeMap::new();
        for (file, supplied_name) in files {
            let metadata = file
                .metadata()
                .map_err(|_| CaptureError::FileMetadataFailed)?;
            let identity = FileIdentity::from_metadata(&metadata);
            let classification = classify_selected_path(Path::new(&supplied_name));
            let hidden = classification == Some(SourceOmissionCode::HiddenPath);
            let credential = classification == Some(SourceOmissionCode::CredentialPath);
            let private_name = hidden || credential;
            let display_name = if hidden {
                "hidden item".to_owned()
            } else if credential {
                "credential-sensitive item".to_owned()
            } else {
                selected_file_display_name(&supplied_name)
            };
            let mime_type = if private_name {
                None
            } else {
                mime_type(&display_name)
            };
            let omission = if hidden {
                Some(omission(
                    SourceOmissionCode::HiddenPath,
                    "hidden items are excluded from automatic capture",
                ))
            } else if credential {
                Some(omission(
                    SourceOmissionCode::CredentialPath,
                    "credential-sensitive files are excluded from automatic capture",
                ))
            } else if !metadata.is_file() {
                Some(omission(
                    SourceOmissionCode::SpecialFile,
                    "selected item is not a regular file",
                ))
            } else if mime_type.is_none() {
                Some(omission(
                    SourceOmissionCode::UnsupportedFormat,
                    "this format has no verified text extractor",
                ))
            } else {
                None
            };
            let id = new_source_id();
            candidates.insert(
                id.clone(),
                Candidate {
                    id: id.clone(),
                    relative_path: None,
                    display_name,
                    identity: Some(identity),
                    byte_length: (!private_name).then_some(metadata.len()),
                    mime_type,
                    omission,
                },
            );
            granted_files.insert(id, GrantedFile { file, identity });
        }
        Ok(Self {
            grant_id: Uuid::new_v4().to_string(),
            scope: Some(GrantedScope::SelectedFiles {
                files: granted_files,
            }),
            candidates,
            enumeration_complete: true,
        })
    }

    /// Convenience for a single item selected through the native file picker.
    pub fn selected_file(
        file: File,
        display_name: impl Into<String>,
    ) -> Result<Self, CaptureError> {
        Self::selected_files(vec![(file, display_name.into())])
    }

    /// The caller supplies a directory handle granted by the OS file picker.
    /// All subsequent names are opened relative to this capability.
    pub fn selected_directory(
        root: Dir,
        display_name: impl Into<String>,
    ) -> Result<Self, CaptureError> {
        let metadata = root
            .metadata(".")
            .map_err(|_| CaptureError::DirectoryReadFailed)?;
        if !metadata.is_dir() {
            return Err(CaptureError::DirectoryReadFailed);
        }
        Ok(Self {
            grant_id: Uuid::new_v4().to_string(),
            scope: Some(GrantedScope::Directory {
                root,
                root_device: metadata.dev(),
                display_name: selected_file_display_name(&display_name.into()),
            }),
            candidates: BTreeMap::new(),
            enumeration_complete: false,
        })
    }

    pub fn grant_id(&self) -> &str {
        &self.grant_id
    }

    pub fn revoke(&mut self) {
        self.scope.take();
        self.candidates.clear();
        self.enumeration_complete = false;
    }

    pub fn enumerate(&mut self, limits: CaptureLimits) -> Result<EnumerationReport, CaptureError> {
        limits.validate()?;
        let scope = self.scope.as_ref().ok_or(CaptureError::RevokedGrant)?;
        let mut candidates = BTreeMap::new();
        let mut complete = true;
        match scope {
            GrantedScope::SelectedFiles { files } => {
                for (source_id, original) in &self.candidates {
                    let mut candidate = original.clone();
                    let granted = files.get(source_id).ok_or(CaptureError::InvalidSelection)?;
                    let current = granted
                        .file
                        .metadata()
                        .map_err(|_| CaptureError::FileMetadataFailed)?;
                    if !granted
                        .identity
                        .same_file_and_version(FileIdentity::from_metadata(&current))
                        && candidate.omission.is_none()
                    {
                        candidate.omission = Some(omission(
                            SourceOmissionCode::ChangedDuringCapture,
                            "the selected item changed after it was chosen; select it again",
                        ));
                    }
                    candidates.insert(source_id.clone(), candidate);
                }
            }
            GrantedScope::Directory {
                root,
                root_device,
                display_name,
            } => {
                let mut walked = 0usize;
                let policy = DirectoryWalkPolicy {
                    root,
                    root_device: *root_device,
                    root_label: display_name,
                    max_items: limits.max_items,
                };
                walk_directory(
                    &policy,
                    Path::new(""),
                    &mut walked,
                    &mut candidates,
                    &mut complete,
                )?;
            }
        }
        self.candidates = candidates;
        self.enumeration_complete = complete;
        let mut summaries: Vec<_> = self
            .candidates
            .values()
            .map(|candidate| CandidateSummary {
                source_id: candidate.id.clone(),
                display_name: candidate.display_name.clone(),
                byte_length: candidate.byte_length,
                mime_type: candidate.mime_type.clone(),
                omission: candidate.omission.clone(),
            })
            .collect();
        summaries.sort_by(|left, right| left.display_name.cmp(&right.display_name));
        Ok(EnumerationReport {
            grant_id: self.grant_id.clone(),
            candidates: summaries,
            complete,
        })
    }

    pub fn capture_selected(
        &mut self,
        directives: &[CaptureDirective],
        limits: CaptureLimits,
        captured_at_epoch_ms: u64,
    ) -> Result<CaptureBatch, CaptureError> {
        limits.validate()?;
        if self.scope.is_none() {
            return Err(CaptureError::RevokedGrant);
        }
        if self.candidates.is_empty() && !directives.is_empty() {
            return Err(CaptureError::InvalidSelection);
        }
        let mut selections = BTreeMap::new();
        for directive in directives {
            if !self.candidates.contains_key(&directive.source_id)
                || selections
                    .insert(directive.source_id.as_str(), directive.line_range.clone())
                    .is_some()
            {
                return Err(CaptureError::InvalidSelection);
            }
        }

        let mut manifest_sources = Vec::with_capacity(self.candidates.len() + 1);
        let mut objects = Vec::new();
        let mut problems = Vec::new();
        let mut used_bytes = 0u64;
        let mut captured_source_ids = Vec::new();
        let keys: Vec<String> = self.candidates.keys().cloned().collect();
        for (item_index, source_id) in keys.into_iter().enumerate() {
            let candidate = self
                .candidates
                .get(&source_id)
                .ok_or(CaptureError::InvalidSelection)?;
            if item_index >= limits.max_items {
                let reason = omission(
                    SourceOmissionCode::ManifestLimit,
                    "item exceeds the configured manifest item limit",
                );
                problems.push(CaptureProblem {
                    source_id: source_id.clone(),
                    state: ManifestSourceState::Excluded,
                    omission: reason.clone(),
                });
                manifest_sources.push(omitted_source(
                    candidate,
                    reason,
                    ManifestSourceState::Excluded,
                ));
                continue;
            }
            let Some(line_range) = selections.get(source_id.as_str()).cloned() else {
                let reason = candidate.omission.clone().unwrap_or_else(|| {
                    omission(
                        SourceOmissionCode::UserExcluded,
                        "the user did not include this item in the capture",
                    )
                });
                problems.push(CaptureProblem {
                    source_id: source_id.clone(),
                    state: ManifestSourceState::Excluded,
                    omission: reason.clone(),
                });
                manifest_sources.push(omitted_source(
                    candidate,
                    reason,
                    ManifestSourceState::Excluded,
                ));
                continue;
            };
            if let Some(reason) = candidate.omission.clone() {
                problems.push(CaptureProblem {
                    source_id: source_id.clone(),
                    state: ManifestSourceState::Excluded,
                    omission: reason.clone(),
                });
                manifest_sources.push(omitted_source(
                    candidate,
                    reason,
                    ManifestSourceState::Excluded,
                ));
                continue;
            }
            let Some(mime) = candidate.mime_type.clone() else {
                let reason = omission(
                    SourceOmissionCode::UnsupportedFormat,
                    "this format has no verified text extractor",
                );
                problems.push(CaptureProblem {
                    source_id: source_id.clone(),
                    state: ManifestSourceState::Excluded,
                    omission: reason.clone(),
                });
                manifest_sources.push(omitted_source(
                    candidate,
                    reason,
                    ManifestSourceState::Excluded,
                ));
                continue;
            };
            let expected_length = candidate.byte_length.unwrap_or(0);
            let Some(remaining_bytes) = limits.max_manifest_bytes.checked_sub(used_bytes) else {
                let reason = omission(
                    SourceOmissionCode::ManifestLimit,
                    "manifest byte limit reached",
                );
                problems.push(CaptureProblem {
                    source_id: source_id.clone(),
                    state: ManifestSourceState::Excluded,
                    omission: reason.clone(),
                });
                manifest_sources.push(omitted_source(
                    candidate,
                    reason,
                    ManifestSourceState::Excluded,
                ));
                continue;
            };
            let size_omission = if expected_length > limits.max_file_bytes {
                Some((
                    SourceOmissionCode::FileTooLarge,
                    "item exceeds the selected per-file byte limit",
                ))
            } else if expected_length > remaining_bytes {
                Some((
                    SourceOmissionCode::ManifestLimit,
                    "item exceeds the remaining manifest byte limit",
                ))
            } else {
                None
            };
            if let Some((code, detail)) = size_omission {
                let reason = omission(code, detail);
                problems.push(CaptureProblem {
                    source_id: source_id.clone(),
                    state: ManifestSourceState::Excluded,
                    omission: reason.clone(),
                });
                manifest_sources.push(omitted_source(
                    candidate,
                    reason,
                    ManifestSourceState::Excluded,
                ));
                continue;
            }
            let result = self.read_candidate(candidate, expected_length, limits.max_file_bytes);
            let (raw_bytes, actual_identity) = match result {
                Ok(value) => value,
                Err(code) => {
                    let reason = omission(code, omission_text(code));
                    problems.push(CaptureProblem {
                        source_id: source_id.clone(),
                        state: ManifestSourceState::Failed,
                        omission: reason.clone(),
                    });
                    manifest_sources.push(omitted_source(
                        candidate,
                        reason,
                        ManifestSourceState::Failed,
                    ));
                    continue;
                }
            };
            if raw_bytes.len() as u64 != expected_length
                || candidate
                    .identity
                    .is_some_and(|before| !before.same_file_and_version(actual_identity))
            {
                let reason = omission(
                    SourceOmissionCode::ChangedDuringCapture,
                    "the selected item changed while it was being captured; select it again",
                );
                problems.push(CaptureProblem {
                    source_id: source_id.clone(),
                    state: ManifestSourceState::Failed,
                    omission: reason.clone(),
                });
                manifest_sources.push(omitted_source(
                    candidate,
                    reason,
                    ManifestSourceState::Failed,
                ));
                continue;
            }
            let object_digest = Digest::from_bytes(&raw_bytes);
            if matches!(
                mime.as_str(),
                "application/pdf" | "image/png" | "image/jpeg"
            ) {
                if line_range.is_some() {
                    let reason = omission(
                        SourceOmissionCode::InvalidRange,
                        "binary documents require page or image selection rather than text line ranges",
                    );
                    manifest_sources.push(omitted_source(
                        candidate,
                        reason.clone(),
                        ManifestSourceState::Failed,
                    ));
                    problems.push(CaptureProblem {
                        source_id: source_id.clone(),
                        state: ManifestSourceState::Failed,
                        omission: reason,
                    });
                    continue;
                }
                match crate::extraction::extract_native(&raw_bytes, &mime) {
                    Ok(extracted) => {
                        if extracted
                            .pages
                            .iter()
                            .filter_map(|page| page.text.as_deref())
                            .any(contains_sensitive_content)
                        {
                            let reason = omission(
                                SourceOmissionCode::AccessDenied,
                                "selected content matches a credential pattern and is not sent to a provider",
                            );
                            manifest_sources.push(omitted_source(
                                candidate,
                                reason.clone(),
                                ManifestSourceState::Excluded,
                            ));
                            problems.push(CaptureProblem {
                                source_id: source_id.clone(),
                                state: ManifestSourceState::Excluded,
                                omission: reason,
                            });
                            continue;
                        }
                        let representation_text = serde_json::to_string(&extracted)
                            .map_err(|_| CaptureError::InvalidSelection)?;
                        let derived_digest = Digest::from_bytes(representation_text.as_bytes());
                        let mut locators: Vec<_> = extracted
                            .pages
                            .iter()
                            .map(|p| EvidenceLocator {
                                source_id: source_id.clone(),
                                object_digest: object_digest.clone(),
                                start_line: None,
                                end_line: None,
                                total_lines: None,
                                page: Some(p.page),
                                width: p.width,
                                height: p.height,
                            })
                            .collect();
                        if locators.is_empty() {
                            locators.push(EvidenceLocator {
                                source_id: source_id.clone(),
                                object_digest: object_digest.clone(),
                                start_line: None,
                                end_line: None,
                                total_lines: None,
                                page: None,
                                width: extracted.width,
                                height: extracted.height,
                            });
                        }
                        let locator = locators[0].clone();
                        manifest_sources.push(ManifestSource {
                            source_id: source_id.clone(),
                            display_name: safe_manifest_display(&candidate.display_name),
                            state: ManifestSourceState::Captured,
                            byte_length: Some(raw_bytes.len() as u64),
                            mime_type: Some(mime.clone()),
                            object_digest: Some(object_digest.clone()),
                            derived_digest: Some(derived_digest.clone()),
                            representation_kind: Some(extracted.kind),
                            extractor_id: Some("macos-native-bounded".into()),
                            extractor_version: Some("1".into()),
                            included_locators: locators,
                            omission: None,
                            captured_at_epoch_ms: Some(captured_at_epoch_ms),
                            secret_pattern_findings: Vec::new(),
                            secret_scan_incomplete: true,
                        });
                        objects.push(CapturedObject {
                            source_id: source_id.clone(),
                            object_digest,
                            derived_digest,
                            byte_length: raw_bytes.len() as u64,
                            raw_bytes,
                            representation: ContentRepresentation {
                                kind: extracted.kind,
                                text: representation_text,
                                locator,
                            },
                        });
                        used_bytes = used_bytes.saturating_add(expected_length);
                        captured_source_ids.push(source_id);
                        continue;
                    }
                    Err(error) => {
                        let reason = omission(
                            SourceOmissionCode::ReadFailure,
                            &format!("bounded native extraction failed: {error}"),
                        );
                        manifest_sources.push(omitted_source(
                            candidate,
                            reason.clone(),
                            ManifestSourceState::Failed,
                        ));
                        problems.push(CaptureProblem {
                            source_id: source_id.clone(),
                            state: ManifestSourceState::Failed,
                            omission: reason,
                        });
                        continue;
                    }
                }
            }
            let text = match std::str::from_utf8(&raw_bytes) {
                Ok(value) if !value.as_bytes().contains(&0) => value,
                Ok(_) => {
                    let reason = omission(
                        SourceOmissionCode::UnsupportedFormat,
                        "text contains binary NUL bytes and is not passed to a model",
                    );
                    problems.push(CaptureProblem {
                        source_id: source_id.clone(),
                        state: ManifestSourceState::Excluded,
                        omission: reason.clone(),
                    });
                    manifest_sources.push(omitted_source(
                        candidate,
                        reason,
                        ManifestSourceState::Excluded,
                    ));
                    continue;
                }
                Err(_) => {
                    let reason = omission(
                        SourceOmissionCode::InvalidUtf8,
                        "text is not valid UTF-8; no replacement decoding is applied",
                    );
                    problems.push(CaptureProblem {
                        source_id: source_id.clone(),
                        state: ManifestSourceState::Excluded,
                        omission: reason.clone(),
                    });
                    manifest_sources.push(omitted_source(
                        candidate,
                        reason,
                        ManifestSourceState::Excluded,
                    ));
                    continue;
                }
            };
            let line_count = text.split_inclusive('\n').count() as u64;
            let (start, end, included_text) = match select_lines(text, &line_range) {
                Ok(value) => value,
                Err(_) => {
                    let reason = omission(
                        SourceOmissionCode::InvalidRange,
                        "selected line range is outside the captured text",
                    );
                    problems.push(CaptureProblem {
                        source_id: source_id.clone(),
                        state: ManifestSourceState::Failed,
                        omission: reason.clone(),
                    });
                    manifest_sources.push(omitted_source(
                        candidate,
                        reason,
                        ManifestSourceState::Failed,
                    ));
                    continue;
                }
            };
            if contains_sensitive_content(included_text) {
                let reason = omission(
                    SourceOmissionCode::AccessDenied,
                    "selected content matches a credential pattern and is not sent to a provider",
                );
                problems.push(CaptureProblem {
                    source_id: source_id.clone(),
                    state: ManifestSourceState::Excluded,
                    omission: reason.clone(),
                });
                manifest_sources.push(omitted_source(
                    candidate,
                    reason,
                    ManifestSourceState::Excluded,
                ));
                continue;
            }
            let derived_digest = Digest::from_bytes(included_text.as_bytes());
            let representation_text = included_text.to_owned();
            let locator = EvidenceLocator {
                source_id: source_id.clone(),
                object_digest: object_digest.clone(),
                start_line: start,
                end_line: end,
                total_lines: line_range.as_ref().map(|_| line_count),
                page: None,
                width: None,
                height: None,
            };
            manifest_sources.push(ManifestSource {
                source_id: source_id.clone(),
                display_name: candidate.display_name.clone(),
                state: ManifestSourceState::Captured,
                byte_length: Some(raw_bytes.len() as u64),
                mime_type: Some(mime.clone()),
                object_digest: Some(object_digest.clone()),
                derived_digest: Some(derived_digest.clone()),
                representation_kind: Some(RepresentationKind::Utf8Text),
                extractor_id: Some(EXTRACTOR_ID.to_owned()),
                extractor_version: Some(EXTRACTOR_VERSION.to_owned()),
                included_locators: vec![locator.clone()],
                omission: None,
                captured_at_epoch_ms: Some(captured_at_epoch_ms),
                secret_pattern_findings: Vec::new(),
                secret_scan_incomplete: true,
            });
            objects.push(CapturedObject {
                source_id: source_id.clone(),
                object_digest,
                derived_digest,
                byte_length: raw_bytes.len() as u64,
                raw_bytes,
                representation: ContentRepresentation {
                    kind: RepresentationKind::Utf8Text,
                    text: representation_text,
                    locator,
                },
            });
            used_bytes = used_bytes.saturating_add(expected_length);
            captured_source_ids.push(source_id);
        }

        if !self.enumeration_complete {
            let id = new_source_id();
            let reason = omission(
                SourceOmissionCode::ManifestLimit,
                "folder enumeration stopped at the configured item limit; additional items may be missing",
            );
            problems.push(CaptureProblem {
                source_id: id.clone(),
                state: ManifestSourceState::Excluded,
                omission: reason.clone(),
            });
            manifest_sources.push(ManifestSource {
                source_id: id,
                display_name: "items beyond enumeration limit".to_owned(),
                state: ManifestSourceState::Excluded,
                byte_length: None,
                mime_type: None,
                object_digest: None,
                derived_digest: None,
                representation_kind: None,
                extractor_id: None,
                extractor_version: None,
                included_locators: Vec::new(),
                omission: Some(reason),
                captured_at_epoch_ms: None,
                secret_pattern_findings: Vec::new(),
                secret_scan_incomplete: true,
            });
        }
        let manifest = SourceCaptureManifest::draft(manifest_sources, captured_at_epoch_ms)
            .map_err(|_| CaptureError::ManifestFailed)?;
        Ok(CaptureBatch {
            manifest,
            objects,
            report: CaptureReport {
                captured_source_ids,
                problems,
                total_original_bytes: used_bytes,
            },
            enumeration_complete: self.enumeration_complete,
        })
    }

    fn read_candidate(
        &self,
        candidate: &Candidate,
        max_expected_length: u64,
        max_file_bytes: u64,
    ) -> Result<(Vec<u8>, FileIdentity), SourceOmissionCode> {
        let mut file = match self
            .scope
            .as_ref()
            .ok_or(SourceOmissionCode::RevokedGrant)?
        {
            GrantedScope::SelectedFiles { files } => files
                .get(&candidate.id)
                .ok_or(SourceOmissionCode::ReadFailure)?
                .file
                .try_clone()
                .map_err(|_| SourceOmissionCode::ReadFailure)?,
            GrantedScope::Directory { root, .. } => {
                let path = candidate
                    .relative_path
                    .as_ref()
                    .ok_or(SourceOmissionCode::ReadFailure)?;
                let before = metadata_within_nofollow(root, path)
                    .map_err(|_| SourceOmissionCode::AccessDenied)?;
                if before.file_type().is_symlink() {
                    return Err(SourceOmissionCode::SymbolicLink);
                }
                if !before.is_file() {
                    return Err(SourceOmissionCode::SpecialFile);
                }
                let expected = candidate.identity.ok_or(SourceOmissionCode::ReadFailure)?;
                if !expected.same_file_and_version(FileIdentity::from_metadata(&before)) {
                    return Err(SourceOmissionCode::ChangedDuringCapture);
                }
                let opened = open_file_within_nofollow(root, path)
                    .map_err(|_| SourceOmissionCode::AccessDenied)?;
                let opened_identity = opened
                    .metadata()
                    .map(|metadata| FileIdentity::from_metadata(&metadata))
                    .map_err(|_| SourceOmissionCode::ReadFailure)?;
                if !expected.same_file_and_version(opened_identity) {
                    return Err(SourceOmissionCode::ChangedDuringCapture);
                }
                opened
            }
        };
        let before = FileIdentity::from_metadata(
            &file
                .metadata()
                .map_err(|_| SourceOmissionCode::ReadFailure)?,
        );
        if before.length > max_file_bytes || before.length > max_expected_length {
            return Err(SourceOmissionCode::FileTooLarge);
        }
        file.seek(SeekFrom::Start(0))
            .map_err(|_| SourceOmissionCode::ReadFailure)?;
        let mut bytes = Vec::with_capacity(before.length as usize);
        (&mut file)
            .take(max_file_bytes.saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(|_| SourceOmissionCode::ReadFailure)?;
        if bytes.len() as u64 > max_file_bytes {
            return Err(SourceOmissionCode::FileTooLarge);
        }
        let after = FileIdentity::from_metadata(
            &file
                .metadata()
                .map_err(|_| SourceOmissionCode::ReadFailure)?,
        );
        if !before.same_file_and_version(after) {
            return Err(SourceOmissionCode::ChangedDuringCapture);
        }
        if let Some(path) = candidate.relative_path.as_ref() {
            let GrantedScope::Directory { root, .. } = self
                .scope
                .as_ref()
                .ok_or(SourceOmissionCode::RevokedGrant)?
            else {
                return Err(SourceOmissionCode::ReadFailure);
            };
            let current = metadata_within_nofollow(root, path)
                .map_err(|_| SourceOmissionCode::ChangedDuringCapture)?;
            if current.file_type().is_symlink()
                || !FileIdentity::from_metadata(&current).same_file_and_version(after)
            {
                return Err(SourceOmissionCode::ChangedDuringCapture);
            }
        }
        Ok((bytes, after))
    }
}

fn safe_components(path: &Path) -> io::Result<Vec<OsString>> {
    path.components()
        .map(|component| match component {
            Component::Normal(name) => Ok(name.to_os_string()),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "source paths must contain only normal relative components",
            )),
        })
        .collect()
}

fn normalize_absolute_path(path: &Path) -> Result<PathBuf, CaptureError> {
    if !path.is_absolute() {
        return Err(CaptureError::InvalidSourceScope);
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::RootDir => normalized.push("/"),
            Component::Normal(name) => normalized.push(name),
            Component::CurDir => {}
            Component::ParentDir => {
                if normalized != Path::new("/") {
                    normalized.pop();
                }
            }
            Component::Prefix(_) => return Err(CaptureError::InvalidSourceScope),
        }
    }
    if normalized == Path::new("/") {
        return Err(CaptureError::InvalidSourceScope);
    }
    Ok(normalized)
}

#[cfg(unix)]
fn open_absolute_directory_nofollow(path: &Path) -> io::Result<Dir> {
    let relative = path.strip_prefix("/").map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "source scope root must be absolute",
        )
    })?;
    let components = safe_components(relative)?;
    if components.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "filesystem root cannot be granted as a source scope",
        ));
    }
    let filesystem_root = Dir::open_ambient_dir("/", ambient_authority())?;
    open_directory_chain(&filesystem_root, &components)
}

#[cfg(not(unix))]
fn open_absolute_directory_nofollow(_path: &Path) -> io::Result<Dir> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "source scopes require no-follow directory handles",
    ))
}

#[cfg(unix)]
fn open_directory_chain(root: &Dir, components: &[OsString]) -> io::Result<Dir> {
    let mut directory = root.try_clone()?;
    for component in components {
        let mut options = OpenOptions::new();
        options
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW);
        let opened = directory.open_with(component, &options)?;
        directory = Dir::from_std_file(opened.into_std());
    }
    Ok(directory)
}

#[cfg(unix)]
fn open_directory_within_nofollow(root: &Dir, path: &Path) -> io::Result<Dir> {
    let components = safe_components(path)?;
    open_directory_chain(root, &components)
}

#[cfg(not(unix))]
fn open_directory_within_nofollow(_root: &Dir, _path: &Path) -> io::Result<Dir> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "directory capture requires no-follow open support",
    ))
}

#[cfg(unix)]
fn open_file_within_nofollow(root: &Dir, path: &Path) -> io::Result<File> {
    let mut components = safe_components(path)?;
    let file_name = components
        .pop()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "source path is empty"))?;
    let parent = open_directory_chain(root, &components)?;
    let mut options = OpenOptions::new();
    options.read(true).custom_flags(libc::O_NOFOLLOW);
    parent.open_with(file_name, &options)
}

#[cfg(not(unix))]
fn open_file_within_nofollow(_root: &Dir, _path: &Path) -> io::Result<File> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "directory capture requires no-follow open support",
    ))
}

#[cfg(unix)]
fn metadata_within_nofollow(root: &Dir, path: &Path) -> io::Result<Metadata> {
    let mut components = safe_components(path)?;
    let name = components
        .pop()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "source path is empty"))?;
    let parent = open_directory_chain(root, &components)?;
    parent.symlink_metadata(name)
}

#[cfg(not(unix))]
fn metadata_within_nofollow(_root: &Dir, _path: &Path) -> io::Result<Metadata> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "directory capture requires no-follow metadata support",
    ))
}

struct DirectoryWalkPolicy<'a> {
    root: &'a Dir,
    root_device: u64,
    root_label: &'a str,
    max_items: usize,
}

fn walk_directory(
    policy: &DirectoryWalkPolicy<'_>,
    prefix: &Path,
    visited: &mut usize,
    candidates: &mut BTreeMap<String, Candidate>,
    complete: &mut bool,
) -> Result<(), CaptureError> {
    let DirectoryWalkPolicy {
        root,
        root_device,
        root_label,
        max_items,
    } = *policy;
    let directory = open_directory_within_nofollow(root, prefix)
        .map_err(|_| CaptureError::DirectoryReadFailed)?;
    let mut entries: Vec<cap_std::fs::DirEntry> = directory
        .entries()
        .map_err(|_| CaptureError::DirectoryReadFailed)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| CaptureError::DirectoryReadFailed)?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        if *visited >= max_items {
            *complete = false;
            break;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            *visited += 1;
            let candidate = candidate_without_path(
                "non-UTF-8 file name".to_owned(),
                SourceOmissionCode::UnsupportedFormat,
                "file name cannot be represented safely in the manifest",
            );
            candidates.insert(candidate.id.clone(), candidate);
            continue;
        };
        if name == "." || name == ".." {
            continue;
        }
        let relative = prefix.join(&name);
        let visible_name = display_relative_path(root_label, &relative);
        *visited += 1;
        let file_type = entry
            .file_type()
            .map_err(|_| CaptureError::DirectoryReadFailed)?;
        if path_has_hidden_component(&relative) {
            insert_omission(
                candidates,
                "hidden item".to_owned(),
                &relative,
                SourceOmissionCode::HiddenPath,
                "hidden items are excluded from automatic capture",
            );
            continue;
        }
        if is_credential_name(&relative.to_string_lossy()) {
            insert_omission(
                candidates,
                "credential-sensitive item".to_owned(),
                &relative,
                SourceOmissionCode::CredentialPath,
                "credential-sensitive paths are excluded from automatic capture",
            );
            continue;
        }
        if file_type.is_symlink() {
            insert_omission(
                candidates,
                visible_name,
                &relative,
                SourceOmissionCode::SymbolicLink,
                "symbolic links are not followed",
            );
            continue;
        }
        let metadata = metadata_within_nofollow(root, &relative)
            .map_err(|_| CaptureError::DirectoryReadFailed)?;
        if metadata.dev() != root_device {
            insert_omission(
                candidates,
                visible_name,
                &relative,
                SourceOmissionCode::CrossVolume,
                "items on a different mounted volume are excluded",
            );
            continue;
        }
        if metadata.is_dir() {
            let child = open_directory_within_nofollow(root, &relative)
                .map_err(|_| CaptureError::DirectoryReadFailed)?;
            let child_metadata = child
                .dir_metadata()
                .map_err(|_| CaptureError::DirectoryReadFailed)?;
            let path_metadata = metadata_within_nofollow(root, &relative)
                .map_err(|_| CaptureError::DirectoryReadFailed)?;
            if path_metadata.file_type().is_symlink()
                || !FileIdentity::from_metadata(&child_metadata)
                    .same_file_and_version(FileIdentity::from_metadata(&path_metadata))
            {
                insert_omission(
                    candidates,
                    visible_name,
                    &relative,
                    SourceOmissionCode::ChangedDuringCapture,
                    "directory changed while its contents were being listed",
                );
                continue;
            }
            walk_directory(policy, &relative, visited, candidates, complete)?;
        } else if metadata.is_file() {
            let identity = FileIdentity::from_metadata(&metadata);
            let mut candidate = candidate_for(visible_name, Some(relative), identity, true, None);
            if candidate
                .byte_length
                .is_some_and(|size| size > MAX_FILE_BYTES)
            {
                candidate.omission = Some(omission(
                    SourceOmissionCode::FileTooLarge,
                    "item exceeds the hard per-file byte limit",
                ));
            }
            candidates.insert(candidate.id.clone(), candidate);
        } else {
            insert_omission(
                candidates,
                visible_name,
                &relative,
                SourceOmissionCode::SpecialFile,
                "devices, sockets, pipes, and other special files are not captured",
            );
        }
        if !*complete {
            break;
        }
    }
    Ok(())
}

fn candidate_for(
    display_name: String,
    relative_path: Option<PathBuf>,
    identity: FileIdentity,
    is_regular_file: bool,
    preexisting_omission: Option<SourceOmission>,
) -> Candidate {
    let display_name = sanitize_display_name(&display_name);
    let id = new_source_id();
    let mut source_omission = preexisting_omission;
    if !is_regular_file && source_omission.is_none() {
        source_omission = Some(omission(
            SourceOmissionCode::SpecialFile,
            "selected item is not a regular file",
        ));
    }
    let mime_type = mime_type(&display_name);
    if mime_type.is_none() && source_omission.is_none() {
        source_omission = Some(omission(
            SourceOmissionCode::UnsupportedFormat,
            "this format has no verified text extractor",
        ));
    }
    Candidate {
        id,
        relative_path,
        display_name,
        identity: Some(identity),
        byte_length: Some(identity.length),
        mime_type,
        omission: source_omission,
    }
}

fn candidate_without_path(
    display_name: String,
    code: SourceOmissionCode,
    detail: &str,
) -> Candidate {
    Candidate {
        id: new_source_id(),
        relative_path: None,
        display_name,
        identity: None,
        byte_length: None,
        mime_type: None,
        omission: Some(omission(code, detail)),
    }
}

fn insert_omission(
    candidates: &mut BTreeMap<String, Candidate>,
    display_name: String,
    relative_path: &Path,
    code: SourceOmissionCode,
    detail: &str,
) {
    let candidate = candidate_without_path(display_name, code, detail);
    candidates.insert(
        candidate.id.clone(),
        Candidate {
            relative_path: Some(relative_path.to_path_buf()),
            ..candidate
        },
    );
}

fn omitted_source(
    candidate: &Candidate,
    reason: SourceOmission,
    state: ManifestSourceState,
) -> ManifestSource {
    ManifestSource {
        source_id: candidate.id.clone(),
        display_name: safe_manifest_display(&candidate.display_name),
        state,
        byte_length: candidate.byte_length,
        mime_type: candidate.mime_type.clone(),
        object_digest: None,
        derived_digest: None,
        representation_kind: None,
        extractor_id: None,
        extractor_version: None,
        included_locators: Vec::new(),
        omission: Some(reason),
        captured_at_epoch_ms: None,
        secret_pattern_findings: Vec::new(),
        secret_scan_incomplete: true,
    }
}

fn select_lines<'a>(
    text: &'a str,
    range: &Option<LineRange>,
) -> Result<(Option<u64>, Option<u64>, &'a str), ()> {
    let Some(range) = range else {
        return Ok((None, None, text));
    };
    if range.start_line == 0 || range.end_line < range.start_line {
        return Err(());
    }
    let mut line_number = 0u64;
    let mut offset = 0usize;
    let mut byte_start = None;
    let mut byte_end = None;
    for segment in text.split_inclusive('\n') {
        line_number = line_number.saturating_add(1);
        if line_number == range.start_line {
            byte_start = Some(offset);
        }
        offset = offset.saturating_add(segment.len());
        if line_number == range.end_line {
            byte_end = Some(offset);
            break;
        }
    }
    let (Some(byte_start), Some(byte_end)) = (byte_start, byte_end) else {
        return Err(());
    };
    Ok((
        Some(range.start_line),
        Some(range.end_line),
        &text[byte_start..byte_end],
    ))
}

pub fn contains_sensitive_content(text: &str) -> bool {
    text.lines().any(|line| secret_pattern_kind(line).is_some())
}

fn secret_pattern_kind(line: &str) -> Option<SecretPatternKind> {
    let lower = line.to_ascii_lowercase();
    if lower.contains("-----begin ") && lower.contains("private key-----") {
        Some(SecretPatternKind::PrivateKeyBlock)
    } else if contains_credential_token(&lower, "sk-", 20)
        || contains_credential_token(&lower, "ghp_", 20)
        || contains_credential_token(&lower, "github_pat_", 20)
        || contains_credential_token(&lower, "xoxb-", 20)
        || contains_credential_token(&lower, "akia", 16)
        || contains_credential_token(&lower, "authorization: bearer ", 8)
    {
        Some(SecretPatternKind::TokenPrefix)
    } else if contains_credential_assignment(&lower) {
        Some(SecretPatternKind::CredentialAssignment)
    } else {
        None
    }
}

fn contains_credential_token(lower: &str, prefix: &str, minimum_length: usize) -> bool {
    lower.match_indices(prefix).any(|(start, _)| {
        let left_boundary = lower[..start]
            .chars()
            .next_back()
            .is_none_or(|character| !character.is_ascii_alphanumeric() && character != '_');
        let token = lower[start + prefix.len()..]
            .chars()
            .take_while(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '_' | '-')
            })
            .count();
        let full_token = &lower[start..start + prefix.len() + token];
        left_boundary && token >= minimum_length && !is_known_placeholder_token(prefix, full_token)
    })
}

fn is_known_placeholder_token(prefix: &str, full_token: &str) -> bool {
    matches!(
        full_token,
        "authorization: bearer your-token-here"
            | "authorization: bearer your-token"
            | "authorization: bearer example-token"
            | "authorization: bearer placeholder-token"
            | "authorization: bearer redacted-token"
            | "akiaiosfodnn7example"
    ) || (prefix == "authorization: bearer "
        && matches!(
            full_token,
            "authorization: bearer example"
                | "authorization: bearer placeholder"
                | "authorization: bearer redacted"
        ))
}

fn contains_credential_assignment(lower: &str) -> bool {
    [
        "password",
        "passwd",
        "api_key",
        "apikey",
        "client_secret",
        "access_token",
        "refresh_token",
    ]
    .iter()
    .any(|key| {
        lower.match_indices(key).any(|(start, _)| {
            let before_is_key = lower[..start]
                .chars()
                .next_back()
                .is_some_and(|character| character.is_ascii_alphanumeric() || character == '_');
            let after_key = &lower[start + key.len()..];
            let after_is_key = after_key
                .chars()
                .next()
                .is_some_and(|character| character.is_ascii_alphanumeric() || character == '_');
            if before_is_key || after_is_key {
                return false;
            }
            let separator = after_key.trim_start();
            let Some(value) = separator
                .strip_prefix('=')
                .or_else(|| separator.strip_prefix(':'))
            else {
                return false;
            };
            let value = value
                .trim_start()
                .trim_matches(|character| matches!(character, '\'' | '"' | '`'));
            let token = value
                .chars()
                .take_while(|character| {
                    character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '/')
                })
                .collect::<String>();
            token.len() >= 8
                && ![
                    "placeholder",
                    "redacted",
                    "changeme",
                    "example",
                    "your_api_key",
                    "your-api-key",
                    "none",
                    "null",
                    "false",
                    "true",
                ]
                .iter()
                .any(|placeholder| token.starts_with(placeholder))
        })
    })
}

fn path_has_hidden_component(path: &Path) -> bool {
    path.components().any(|component| match component {
        Component::Normal(name) => name.to_string_lossy().starts_with('.'),
        _ => false,
    })
}

pub fn classify_selected_path(path: &Path) -> Option<SourceOmissionCode> {
    let normalized = path.to_string_lossy().replace('\\', "/");
    if path_has_hidden_component(path)
        || normalized
            .split('/')
            .any(|component| component.starts_with('.'))
    {
        Some(SourceOmissionCode::HiddenPath)
    } else if is_credential_name(&normalized) {
        Some(SourceOmissionCode::CredentialPath)
    } else {
        None
    }
}

fn is_credential_name(value: &str) -> bool {
    let normalized = value.replace('\\', "/").to_ascii_lowercase();
    let components: Vec<_> = normalized.split('/').collect();
    let filename = components.last().copied().unwrap_or_default();
    let credential_components = [
        "credentials",
        "credential",
        "secrets",
        "secret",
        "keychain",
        "keychains",
        "login data",
        "cookies",
        "web data",
        "local state",
    ];
    let credential_files = [
        "id_rsa",
        "id_ed25519",
        "id_ecdsa",
        "authorized_keys",
        "known_hosts",
        "npmrc",
        ".pypirc",
        "credentials.json",
        "token.json",
        "logins.json",
        "key3.db",
        "key4.db",
        "cookies.sqlite",
        "cookies.binarycookies",
        "system.keychain",
    ];
    components
        .iter()
        .any(|part| credential_components.contains(part))
        || credential_files.contains(&filename)
        || filename.starts_with(".env")
        || [
            ".pem",
            ".p12",
            ".pfx",
            ".key",
            ".mobileprovision",
            ".keychain-db",
            ".jks",
            ".keystore",
            ".kdbx",
        ]
        .iter()
        .any(|suffix| filename.ends_with(suffix))
}

fn mime_type(name: &str) -> Option<String> {
    let extension = Path::new(name).extension()?.to_str()?.to_ascii_lowercase();
    let mime = match extension.as_str() {
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "txt" | "log" => "text/plain",
        "md" | "markdown" | "mdx" => "text/markdown",
        "json" | "jsonl" => "application/json",
        "csv" => "text/csv",
        "rs" => "text/x-rust",
        "ts" | "tsx" => "text/typescript",
        "js" | "jsx" => "text/javascript",
        "html" | "xml" => "text/html",
        "css" => "text/css",
        "toml" => "application/toml",
        "yaml" | "yml" => "application/yaml",
        "py" => "text/x-python",
        "go" => "text/x-go",
        "java" | "kt" => "text/x-java-source",
        "swift" => "text/x-swift",
        "sh" | "zsh" => "text/x-shellscript",
        "c" | "h" | "cpp" | "hpp" => "text/x-c",
        "sql" => "application/sql",
        "diff" | "patch" => "text/x-diff",
        "ini" | "conf" | "properties" => "text/plain",
        _ => return None,
    };
    Some(mime.to_owned())
}

fn display_relative_path(root_label: &str, relative: &Path) -> String {
    sanitize_display_name(&format!("{root_label}/{}", relative.to_string_lossy()))
}

fn selected_file_display_name(value: &str) -> String {
    let normalized = value.replace('\\', "/");
    let is_absolute = normalized.starts_with('/') || normalized.as_bytes().get(1) == Some(&b':');
    let display_value = if is_absolute {
        normalized.rsplit('/').next().unwrap_or_default()
    } else {
        &normalized
    };
    sanitize_display_name(display_value)
}

fn sanitize_display_name(value: &str) -> String {
    let path = Path::new(value);
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        "selected item".to_owned()
    } else {
        let safe = value.replace('\\', "/");
        if safe.is_empty()
            || safe
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..")
        {
            "selected item".to_owned()
        } else {
            safe
        }
    }
}

fn safe_manifest_display(value: &str) -> String {
    if value == "credential-sensitive item"
        || value == "hidden item"
        || value.starts_with("items beyond enumeration limit")
        || (!value.is_empty()
            && !Path::new(value).is_absolute()
            && Path::new(value)
                .components()
                .all(|component| matches!(component, Component::Normal(_))))
    {
        value.to_owned()
    } else {
        "selected item".to_owned()
    }
}

fn new_source_id() -> String {
    Uuid::new_v4().to_string()
}

fn omission(code: SourceOmissionCode, detail: &str) -> SourceOmission {
    SourceOmission {
        code,
        detail: detail.to_owned(),
    }
}

fn omission_text(code: SourceOmissionCode) -> &'static str {
    match code {
        SourceOmissionCode::UserExcluded => "the user did not include this item in the capture",
        SourceOmissionCode::HiddenPath => "hidden items are excluded from automatic capture",
        SourceOmissionCode::CredentialPath => {
            "credential-sensitive paths are excluded from automatic capture"
        }
        SourceOmissionCode::SymbolicLink => "symbolic links are not followed",
        SourceOmissionCode::SpecialFile => "special files are not captured",
        SourceOmissionCode::CrossVolume => "items on a different mounted volume are excluded",
        SourceOmissionCode::UnsupportedFormat => "this format has no verified text extractor",
        SourceOmissionCode::InvalidUtf8 => "text is not valid UTF-8",
        SourceOmissionCode::FileTooLarge => "item exceeds the selected byte limit",
        SourceOmissionCode::ManifestLimit => "manifest item or byte limit was reached",
        SourceOmissionCode::AccessDenied => "the source could not be read within its grant",
        SourceOmissionCode::ChangedDuringCapture => {
            "the source changed while it was being captured"
        }
        SourceOmissionCode::RevokedGrant => "source permission has been revoked",
        SourceOmissionCode::InvalidRange => "selected line range is outside the captured text",
        SourceOmissionCode::ReadFailure => "the selected source could not be captured",
    }
}

#[cfg(test)]
mod traversal_policy_tests {
    use super::*;

    #[test]
    fn recursive_enumeration_preserves_denials_and_shared_item_budget() {
        let root = std::env::temp_dir().join(format!("magi-context-policy-{}", Uuid::new_v4()));
        std::fs::create_dir_all(root.join("nested")).unwrap();
        std::fs::write(root.join("nested/allowed.txt"), "public fixture").unwrap();
        std::fs::write(root.join(".hidden.txt"), "hidden fixture").unwrap();
        std::fs::write(root.join("credentials.json"), "excluded fixture").unwrap();
        std::os::unix::fs::symlink(root.join("nested/allowed.txt"), root.join("linked.txt"))
            .unwrap();
        let directory = Dir::open_ambient_dir(&root, ambient_authority()).unwrap();
        let mut grant = SourceGrant::selected_directory(directory, "fixture").unwrap();
        let report = grant.enumerate(CaptureLimits::default()).unwrap();
        assert!(report.complete);
        assert!(
            report
                .candidates
                .iter()
                .any(
                    |candidate| candidate.display_name == "fixture/nested/allowed.txt"
                        && candidate.omission.is_none()
                )
        );
        for code in [
            SourceOmissionCode::HiddenPath,
            SourceOmissionCode::CredentialPath,
            SourceOmissionCode::SymbolicLink,
        ] {
            assert!(report.candidates.iter().any(|candidate| {
                candidate
                    .omission
                    .as_ref()
                    .is_some_and(|omission| omission.code == code)
            }));
        }
        let limited = grant
            .enumerate(
                CaptureLimits::user_confirmed(2, DEFAULT_FILE_BYTES, DEFAULT_MANIFEST_BYTES)
                    .unwrap(),
            )
            .unwrap();
        assert!(!limited.complete);
        std::fs::remove_dir_all(root).unwrap();
    }
}
