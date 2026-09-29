use std::collections::BTreeSet;

use magi_domain::{
    ContextManifest as RunContextManifest, Digest, DomainError, SourceSnapshot, ValidationIssue,
    canonical_json,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

pub const SOURCE_CAPTURE_SCHEMA_VERSION: u16 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recipient {
    pub provider_id: String,
    pub account_profile_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceCaptureManifest {
    pub schema_version: u16,
    pub manifest_id: String,
    pub created_at_epoch_ms: u64,
    pub content: SourceCaptureManifestContent,
    pub disclosure_state: DisclosureState,
    pub digest: Digest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceCaptureManifestContent {
    pub sources: Vec<ManifestSource>,
    pub recipients: Vec<Recipient>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisclosureState {
    Draft,
    Approved,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestSource {
    pub source_id: String,
    pub display_name: String,
    pub state: ManifestSourceState,
    pub byte_length: Option<u64>,
    pub mime_type: Option<String>,
    pub object_digest: Option<Digest>,
    pub derived_digest: Option<Digest>,
    pub representation_kind: Option<RepresentationKind>,
    pub extractor_id: Option<String>,
    pub extractor_version: Option<String>,
    pub included_locators: Vec<EvidenceLocator>,
    pub omission: Option<SourceOmission>,
    pub captured_at_epoch_ms: Option<u64>,
    pub secret_pattern_findings: Vec<SecretPatternFinding>,
    pub secret_scan_incomplete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SafeSourceSummary {
    pub source_id: String,
    pub display_name: String,
    pub result: ManifestSourceState,
    pub byte_length: Option<u64>,
    pub representation_kind: Option<RepresentationKind>,
    pub object_digest: Option<Digest>,
    pub derived_digest: Option<Digest>,
    pub warning_codes: Vec<SecretPatternKind>,
    pub secret_scan_incomplete: bool,
    pub omission_code: Option<SourceOmissionCode>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManifestSourceState {
    Captured,
    Excluded,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepresentationKind {
    Utf8Text,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceLocator {
    pub source_id: String,
    pub object_digest: Digest,
    pub start_line: Option<u64>,
    pub end_line: Option<u64>,
    pub total_lines: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceOmission {
    pub code: SourceOmissionCode,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceOmissionCode {
    UserExcluded,
    HiddenPath,
    CredentialPath,
    SymbolicLink,
    SpecialFile,
    CrossVolume,
    UnsupportedFormat,
    InvalidUtf8,
    FileTooLarge,
    ManifestLimit,
    AccessDenied,
    ChangedDuringCapture,
    RevokedGrant,
    InvalidRange,
    ReadFailure,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretPatternFinding {
    pub line: u64,
    pub kind: SecretPatternKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecretPatternKind {
    CredentialAssignment,
    TokenPrefix,
    PrivateKeyBlock,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FreshnessStatus {
    Unchanged,
    Changed,
    Missing,
    Unreadable,
    Unchecked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FreshnessObservation {
    pub status: FreshnessStatus,
    pub observed_at_epoch_ms: u64,
    pub observed_digest: Option<Digest>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisclosureRequest {
    pub run_id: String,
    pub manifest_digest: Digest,
    pub recipient: Recipient,
    pub content_kinds: BTreeSet<DisclosureContentKind>,
    pub issued_at_epoch_ms: u64,
    pub expires_at_epoch_ms: u64,
    pub max_app_turns: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisclosureContentKind {
    Question,
    RoleProfile,
    SourceOriginal,
    SourceDerivedText,
    PriorAssessment,
    Proposal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisclosureGrant {
    pub grant_id: String,
    pub run_id: String,
    pub manifest_digest: Digest,
    pub recipient: Recipient,
    pub content_kinds: BTreeSet<DisclosureContentKind>,
    pub issued_at_epoch_ms: u64,
    pub expires_at_epoch_ms: u64,
    pub max_app_turns: u16,
    pub revoked_at_epoch_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DispatchDisclosure {
    pub run_id: String,
    pub manifest_digest: Digest,
    pub recipient: Recipient,
    pub content_kinds: BTreeSet<DisclosureContentKind>,
    pub app_turn_number: u16,
    pub now_epoch_ms: u64,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum DisclosureError {
    #[error("disclosure requires at least one content kind")]
    EmptyContent,
    #[error("disclosure grant expiry must follow its issue time")]
    InvalidLifetime,
    #[error("disclosure grant has no positive app-turn budget")]
    EmptyBudget,
    #[error("the requested disclosure is not covered by the current grant")]
    NotCovered,
    #[error("the disclosure grant has expired or was revoked")]
    ExpiredOrRevoked,
    #[error("the app-turn budget has been exhausted")]
    BudgetExhausted,
    #[error("manifest validation failed: {0:?}")]
    InvalidManifest(Vec<ValidationIssue>),
    #[error("canonical manifest digest could not be calculated")]
    DigestFailure,
    #[error("approved capture manifest could not be converted to a run manifest")]
    RunManifestFailure,
}

impl SourceCaptureManifest {
    pub fn draft(
        sources: Vec<ManifestSource>,
        created_at_epoch_ms: u64,
    ) -> Result<Self, DisclosureError> {
        let mut manifest = Self {
            schema_version: SOURCE_CAPTURE_SCHEMA_VERSION,
            manifest_id: Uuid::new_v4().to_string(),
            created_at_epoch_ms,
            content: SourceCaptureManifestContent {
                sources,
                recipients: Vec::new(),
            },
            disclosure_state: DisclosureState::Draft,
            digest: Digest::from_bytes(b""),
        };
        manifest.digest = manifest.calculate_digest()?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn confirm_disclosure(
        &self,
        recipients: Vec<Recipient>,
        confirmed_digest: &Digest,
        confirmed_at_epoch_ms: u64,
    ) -> Result<Self, DisclosureError> {
        self.validate()?;
        if self.disclosure_state != DisclosureState::Draft || &self.digest != confirmed_digest {
            return Err(DisclosureError::NotCovered);
        }
        let mut confirmed = Self {
            schema_version: self.schema_version,
            manifest_id: Uuid::new_v4().to_string(),
            created_at_epoch_ms: confirmed_at_epoch_ms,
            content: SourceCaptureManifestContent {
                sources: self.content.sources.clone(),
                recipients,
            },
            disclosure_state: DisclosureState::Approved,
            digest: Digest::from_bytes(b""),
        };
        confirmed.digest = confirmed.calculate_digest()?;
        confirmed.validate()?;
        Ok(confirmed)
    }

    pub fn calculate_digest(&self) -> Result<Digest, DisclosureError> {
        canonical_json(&(
            self.schema_version,
            &self.manifest_id,
            self.created_at_epoch_ms,
            self.disclosure_state,
            &self.content,
        ))
        .map(|bytes| Digest::from_bytes(&bytes))
        .map_err(|_: DomainError| DisclosureError::DigestFailure)
    }

    pub fn safe_source_summaries(&self) -> Result<Vec<SafeSourceSummary>, DisclosureError> {
        self.validate()?;
        Ok(self
            .content
            .sources
            .iter()
            .map(|source| SafeSourceSummary {
                source_id: source.source_id.clone(),
                display_name: source.display_name.clone(),
                result: source.state,
                byte_length: source.byte_length,
                representation_kind: source.representation_kind,
                object_digest: source.object_digest.clone(),
                derived_digest: source.derived_digest.clone(),
                warning_codes: source
                    .secret_pattern_findings
                    .iter()
                    .map(|finding| finding.kind)
                    .collect(),
                secret_scan_incomplete: source.secret_scan_incomplete,
                omission_code: source.omission.as_ref().map(|omission| omission.code),
            })
            .collect())
    }

    pub fn run_manifest_for_recipient(
        &self,
        recipient: &Recipient,
    ) -> Result<RunContextManifest, DisclosureError> {
        self.validate()?;
        if self.disclosure_state != DisclosureState::Approved
            || !self.content.recipients.contains(recipient)
        {
            return Err(DisclosureError::NotCovered);
        }
        let sources = self
            .content
            .sources
            .iter()
            .filter(|source| source.state == ManifestSourceState::Captured)
            .map(|source| {
                let object_digest = source
                    .object_digest
                    .clone()
                    .ok_or(DisclosureError::RunManifestFailure)?;
                let allowed_locators = source
                    .included_locators
                    .iter()
                    .map(|locator| evidence_locator_id(locator, source.derived_digest.as_ref()))
                    .collect();
                Ok(SourceSnapshot {
                    source_id: source.source_id.clone(),
                    object_digest,
                    allowed_locators,
                })
            })
            .collect::<Result<Vec<_>, DisclosureError>>()?;
        RunContextManifest::new(self.manifest_id.clone(), sources)
            .map_err(|_| DisclosureError::RunManifestFailure)
    }

    pub fn validate(&self) -> Result<(), DisclosureError> {
        let mut issues = Vec::new();
        if self.schema_version != SOURCE_CAPTURE_SCHEMA_VERSION {
            issues.push(ValidationIssue::new(
                "schema_version",
                "unsupported_version",
                "unsupported context manifest schema version",
            ));
        }
        if self.manifest_id.is_empty() || self.manifest_id.len() > 80 {
            issues.push(ValidationIssue::new(
                "manifest_id",
                "invalid_id",
                "manifest ID is empty or exceeds its supported length",
            ));
        }
        if self.disclosure_state == DisclosureState::Approved && self.content.recipients.is_empty()
        {
            issues.push(ValidationIssue::new(
                "recipients",
                "recipient_required",
                "a finalized manifest must name at least one recipient",
            ));
        }
        if self.disclosure_state == DisclosureState::Draft && !self.content.recipients.is_empty() {
            issues.push(ValidationIssue::new(
                "recipients",
                "draft_has_recipients",
                "draft manifests do not contain approved recipients",
            ));
        }
        let mut source_ids = BTreeSet::new();
        let mut recipient_ids = BTreeSet::new();
        for (index, source) in self.content.sources.iter().enumerate() {
            if source.source_id.is_empty() || !source_ids.insert(source.source_id.as_str()) {
                issues.push(ValidationIssue::new(
                    format!("sources[{index}].source_id"),
                    "duplicate_or_empty_source_id",
                    "source IDs must be non-empty and unique",
                ));
            }
            if source.display_name.is_empty()
                || is_absolute_or_parent_path(&source.display_name)
                || (source.display_name != "hidden item"
                    && source.display_name != "credential-sensitive item"
                    && crate::classify_selected_path(std::path::Path::new(&source.display_name))
                        .is_some())
            {
                issues.push(ValidationIssue::new(
                    format!("sources[{index}].display_name"),
                    "unsafe_display_name",
                    "source display names must be safe relative names without hidden or credential-sensitive components",
                ));
            }
            if source.state == ManifestSourceState::Captured {
                if source.object_digest.is_none()
                    || source.derived_digest.is_none()
                    || source.byte_length.is_none()
                    || source.representation_kind.is_none()
                    || source.included_locators.is_empty()
                {
                    issues.push(ValidationIssue::new(
                        format!("sources[{index}]"),
                        "captured_object_missing",
                        "captured sources require original and derived digests, size, representation, and a locator",
                    ));
                }
                if source.omission.is_some() {
                    issues.push(ValidationIssue::new(
                        format!("sources[{index}].omission"),
                        "captured_source_omitted",
                        "captured sources cannot have an omission reason",
                    ));
                }
            } else if source.omission.is_none() {
                issues.push(ValidationIssue::new(
                    format!("sources[{index}].omission"),
                    "omission_reason_required",
                    "excluded and failed sources require a visible reason",
                ));
            } else if source.object_digest.is_some()
                || source.derived_digest.is_some()
                || source.representation_kind.is_some()
                || !source.included_locators.is_empty()
            {
                issues.push(ValidationIssue::new(
                    format!("sources[{index}]"),
                    "omitted_source_has_content",
                    "excluded and failed sources cannot expose captured object metadata",
                ));
            }
            for locator in &source.included_locators {
                if locator.source_id != source.source_id
                    || locator.object_digest
                        != source
                            .object_digest
                            .clone()
                            .unwrap_or_else(|| Digest::from_bytes(b""))
                {
                    issues.push(ValidationIssue::new(
                        format!("sources[{index}].included_locators"),
                        "locator_source_mismatch",
                        "each included locator must refer to the source's captured object",
                    ));
                }
                if let (Some(start), Some(end), Some(total)) =
                    (locator.start_line, locator.end_line, locator.total_lines)
                {
                    if start == 0 || end < start || end > total {
                        issues.push(ValidationIssue::new(
                            format!("sources[{index}].included_locators"),
                            "invalid_line_range",
                            "included line ranges must be one-based and within the captured text",
                        ));
                    }
                } else if locator.start_line.is_some()
                    || locator.end_line.is_some()
                    || locator.total_lines.is_some()
                {
                    issues.push(ValidationIssue::new(
                        format!("sources[{index}].included_locators"),
                        "partial_line_range",
                        "line ranges must include start, end, and total line count",
                    ));
                }
            }
        }
        for (index, recipient) in self.content.recipients.iter().enumerate() {
            if recipient.provider_id.is_empty()
                || recipient.account_profile_id.is_empty()
                || !recipient_ids.insert((&recipient.provider_id, &recipient.account_profile_id))
            {
                issues.push(ValidationIssue::new(
                    format!("recipients[{index}]"),
                    "duplicate_or_empty_recipient",
                    "each recipient needs a unique provider and account profile",
                ));
            }
        }
        if !issues.is_empty() {
            return Err(DisclosureError::InvalidManifest(issues));
        }
        let calculated = self.calculate_digest()?;
        if calculated != self.digest {
            return Err(DisclosureError::InvalidManifest(vec![
                ValidationIssue::new(
                    "digest",
                    "manifest_digest_mismatch",
                    "manifest content does not match its stored digest",
                ),
            ]));
        }
        Ok(())
    }
}

impl DisclosureGrant {
    pub fn after_user_confirmation(
        manifest: &SourceCaptureManifest,
        request: DisclosureRequest,
    ) -> Result<Self, DisclosureError> {
        manifest.validate()?;
        if manifest.disclosure_state != DisclosureState::Approved
            || request.manifest_digest != manifest.digest
            || !manifest.content.recipients.contains(&request.recipient)
        {
            return Err(DisclosureError::NotCovered);
        }
        if request.content_kinds.is_empty() {
            return Err(DisclosureError::EmptyContent);
        }
        if request.expires_at_epoch_ms <= request.issued_at_epoch_ms {
            return Err(DisclosureError::InvalidLifetime);
        }
        if request.max_app_turns == 0 {
            return Err(DisclosureError::EmptyBudget);
        }
        Ok(Self {
            grant_id: Uuid::new_v4().to_string(),
            run_id: request.run_id,
            manifest_digest: request.manifest_digest,
            recipient: request.recipient,
            content_kinds: request.content_kinds,
            issued_at_epoch_ms: request.issued_at_epoch_ms,
            expires_at_epoch_ms: request.expires_at_epoch_ms,
            max_app_turns: request.max_app_turns,
            revoked_at_epoch_ms: None,
        })
    }

    pub fn revoke(&mut self, at_epoch_ms: u64) {
        self.revoked_at_epoch_ms = Some(at_epoch_ms);
    }

    pub fn authorize(&self, dispatch: &DispatchDisclosure) -> Result<(), DisclosureError> {
        if dispatch.now_epoch_ms >= self.expires_at_epoch_ms || self.revoked_at_epoch_ms.is_some() {
            return Err(DisclosureError::ExpiredOrRevoked);
        }
        if dispatch.app_turn_number == 0 || dispatch.app_turn_number > self.max_app_turns {
            return Err(DisclosureError::BudgetExhausted);
        }
        if dispatch.run_id != self.run_id
            || dispatch.manifest_digest != self.manifest_digest
            || dispatch.recipient != self.recipient
            || !dispatch.content_kinds.is_subset(&self.content_kinds)
        {
            return Err(DisclosureError::NotCovered);
        }
        Ok(())
    }
}

fn evidence_locator_id(locator: &EvidenceLocator, derived_digest: Option<&Digest>) -> String {
    let derived_digest = derived_digest
        .map(ToString::to_string)
        .unwrap_or_else(|| "missing-digest".to_owned());
    match (locator.start_line, locator.end_line, locator.total_lines) {
        (Some(start), Some(end), Some(total)) => {
            format!("utf8-text-v1:{derived_digest}:lines:{start}-{end}:of-{total}")
        }
        _ => format!("utf8-text-v1:{derived_digest}:all"),
    }
}

fn is_absolute_or_parent_path(value: &str) -> bool {
    let normalized = value.replace('\\', "/");
    normalized.starts_with('/')
        || normalized.as_bytes().get(1) == Some(&b':')
        || normalized
            .split('/')
            .any(|component| component.is_empty() || component == "." || component == "..")
}

pub fn freshness_from_digests(
    original: &Digest,
    observed: Option<&Digest>,
    status: FreshnessStatus,
    observed_at_epoch_ms: u64,
) -> FreshnessObservation {
    let resolved = match status {
        FreshnessStatus::Missing => FreshnessStatus::Missing,
        FreshnessStatus::Unreadable => FreshnessStatus::Unreadable,
        FreshnessStatus::Unchecked => FreshnessStatus::Unchecked,
        FreshnessStatus::Unchanged | FreshnessStatus::Changed => match observed {
            Some(digest) if digest == original => FreshnessStatus::Unchanged,
            Some(_) => FreshnessStatus::Changed,
            None => FreshnessStatus::Unchecked,
        },
    };
    FreshnessObservation {
        status: resolved,
        observed_at_epoch_ms,
        observed_digest: observed.cloned(),
    }
}
