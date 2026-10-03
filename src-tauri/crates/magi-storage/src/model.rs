use serde::{Deserialize, Serialize};

use magi_context::{FreshnessObservation, SourceCaptureManifest};
use magi_domain::{
    AcpModelBindingSnapshot, CoreId, CoreRoleDefinition, Digest, EventPosition, LiveProviderUsage,
    LiveRunEventCursor, LiveRunQueueProjection, ProviderAuthenticationMethod,
    ProviderProfileRevision, RunSnapshot, RunStage, RunStatus,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoreIdentity {
    pub store_id: String,
    pub generation: u64,
    pub schema_version: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdmissionExecutionAuthority {
    pub lineage_id: String,
    pub store_generation: u64,
    pub active: bool,
}

/// Holds native request revocation and completed worker cleanup through the store
/// switch. Validation must not wait for worker cleanup while a DB transaction is held.
pub trait AdmissionActivationPermission {
    fn validate(&self, previous: &AdmissionExecutionAuthority) -> Result<(), crate::StorageError>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentObjectRef {
    pub digest: Digest,
    pub byte_length: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextDraft {
    pub draft_id: String,
    pub revision: u64,
    pub manifest: SourceCaptureManifest,
    pub updated_at_epoch_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContextDraftSummary {
    pub draft_id: String,
    pub revision: u64,
    pub manifest_id: String,
    pub source_count: u64,
    pub captured_source_count: u64,
    pub updated_at_epoch_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RolePresetInput {
    pub preset_id: String,
    pub display_name: String,
    pub roles: [CoreRoleDefinition; 3],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RolePresetSource {
    Factory,
    User,
}

impl RolePresetSource {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Factory => "factory",
            Self::User => "user",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "factory" => Some(Self::Factory),
            "user" => Some(Self::User),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RolePresetSummary {
    pub preset_id: String,
    pub display_name: String,
    pub revision: u64,
    pub digest: Digest,
    pub source: RolePresetSource,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActiveRolePresetSelection {
    pub preset_id: String,
    pub selection_revision: u64,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActiveProviderProfileSelection {
    pub provider_id: String,
    pub provider_profile_id: String,
    pub selection_revision: u64,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderProfileSummary {
    pub provider_profile_id: String,
    pub revision: u64,
    pub provider_id: String,
    pub display_name: String,
    pub account_alias: String,
    pub authentication_method: ProviderAuthenticationMethod,
    pub credential_configured: bool,
    #[serde(default, skip_serializing)]
    pub credential_home: Option<magi_domain::ProviderCredentialHome>,
    pub digest: Digest,
    pub updated_at: String,
}

impl ProviderProfileSummary {
    pub(crate) fn from_revision(revision: ProviderProfileRevision, updated_at: String) -> Self {
        Self {
            provider_profile_id: revision.provider_profile_id,
            revision: revision.revision,
            provider_id: revision.provider_id,
            display_name: revision.display_name,
            account_alias: revision.account_alias,
            authentication_method: revision.authentication_method,
            credential_configured: revision.credential_home.is_some(),
            credential_home: revision.credential_home,
            digest: revision.digest,
            updated_at,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderSourceScope {
    pub grant_id: String,
    pub provider_profile_id: String,
    pub canonical_path: String,
    pub root_device: u64,
    pub root_inode: u64,
    pub created_at: String,
    pub revoked_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LiveRunAdmissionRequest {
    pub command_id: String,
    pub idempotency_key: String,
    pub question: String,
    pub model_binding: AcpModelBindingSnapshot,
}

pub type CoreBindingReference = magi_domain::FrozenCoreSelection;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdmissionRequestIntent {
    pub command_id: String,
    pub idempotency_key: String,
    pub question: String,
    pub core_bindings: Vec<CoreBindingReference>,
    pub request_provenance: magi_domain::DeliberationRequestProvenance,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub common_context_budget: Option<magi_domain::CommonContextBudgetPolicy>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClarificationParentReference {
    pub run_id: String,
    pub revision: u64,
    pub input_digest: Digest,
    pub generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClarificationDraft {
    pub parent: ClarificationParentReference,
    pub context: ContextDraft,
    pub question: String,
    pub execution_authority: AdmissionExecutionAuthority,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClarificationAdmissionIntent {
    pub parent: ClarificationParentReference,
    pub context_draft_id: String,
    pub context_draft_revision: u64,
    pub request: AdmissionRequestIntent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdmissionRequestBinding {
    pub command_id: String,
    pub idempotency_key: String,
    pub intent_digest: Digest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AdmissionRequestCancellationReceipt {
    pub schema_version: u32,
    pub command_id: String,
    pub idempotency_key: String,
    pub request: AdmissionRequestBinding,
    pub accepted_at: String,
    pub admitted_run_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LiveRunReceipt {
    pub command_id: String,
    pub run_id: String,
    pub accepted_revision: u64,
    pub queue: LiveRunQueueProjection,
    pub event_cursor: LiveRunEventCursor,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case", deny_unknown_fields)]
pub enum LiveRunAdmissionOutcome {
    Accepted {
        receipt: LiveRunReceipt,
        duplicate: bool,
    },
    QueueFull {
        capacity: u8,
        admitted_count: u8,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LiveRunClaim {
    pub run_id: String,
    pub admission_sequence: u64,
    pub claim_generation: u64,
    pub claim_owner: String,
    pub question: String,
    pub model_binding: AcpModelBindingSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LiveProviderResultInput {
    pub final_text: String,
    pub stop_reason: String,
    pub usage: Option<LiveProviderUsage>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LiveRunChange {
    pub run_id: String,
    pub revision: u64,
    pub event_cursor: LiveRunEventCursor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatusFilter {
    Preparing,
    AwaitingConfirmation,
    IndependentReview,
    CrossReview,
    Synthesis,
    Balloting,
    Paused,
    Interrupted,
    Cancelling,
    Completed,
    Cancelled,
    Failed,
}

impl RunStatusFilter {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Preparing => "preparing",
            Self::AwaitingConfirmation => "awaiting_confirmation",
            Self::IndependentReview => "independent_review",
            Self::CrossReview => "cross_review",
            Self::Synthesis => "synthesis",
            Self::Balloting => "balloting",
            Self::Paused => "paused",
            Self::Interrupted => "interrupted",
            Self::Cancelling => "cancelling",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct RunHistoryFilter {
    pub question_query: Option<String>,
    pub statuses: Vec<RunStatusFilter>,
    pub created_after: Option<String>,
    pub created_before: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunHistoryCursor {
    pub store_id: String,
    pub store_generation: u64,
    pub membership_generation: u64,
    pub filter_digest: Digest,
    pub last_created_at: String,
    pub last_run_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunHistoryRequest {
    pub filter: RunHistoryFilter,
    pub cursor: Option<RunHistoryCursor>,
    pub page_size: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunSummary {
    pub run_id: String,
    pub conversation_id: String,
    pub question: String,
    pub status: RunStatus,
    pub revision: u64,
    pub input_digest: Digest,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunHistoryPage {
    pub items: Vec<RunSummary>,
    pub next_cursor: Option<RunHistoryCursor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunEventCursor {
    pub store_id: String,
    pub store_generation: u64,
    pub run_id: String,
    pub after_sequence: u64,
    pub high_water_sequence: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunEventPage {
    pub events: Vec<StoredEvent>,
    pub next_cursor: RunEventCursor,
    pub complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DispatchEventCursor {
    pub store_id: String,
    pub store_generation: u64,
    pub run_id: String,
    pub after_sequence: u64,
    pub high_water_sequence: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DispatchEventPage {
    pub events: Vec<DispatchEvent>,
    pub next_cursor: DispatchEventCursor,
    pub complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceFreshnessRecord {
    pub run_id: String,
    pub source_id: String,
    pub captured_digest: Digest,
    pub observation: FreshnessObservation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunDossier {
    pub snapshot: RunSnapshot,
    pub capture_manifest: Option<SourceCaptureManifest>,
    pub source_freshness: Vec<SourceFreshnessRecord>,
    pub dispatches: Vec<DispatchRecord>,
    pub decision_dossier_digest: Option<Digest>,
    pub replay_cursor: RunEventCursor,
    pub dispatch_replay_cursor: DispatchEventCursor,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveDispatchProjection {
    pub schema_version: u16,
    pub run_id: String,
    pub run_revision: u64,
    pub input_digest: Digest,
    pub run_generation: u64,
    pub dispatches: Vec<LiveDispatchRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveDispatchState {
    Reserved,
    Active,
    Settled,
    Released,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveDispatchRecord {
    pub slot_ordinal: u8,
    pub state: LiveDispatchState,
    pub stage: RunStage,
    pub core_id: Option<CoreId>,
    pub binding_core_id: CoreId,
    pub provider_profile_id: String,
    pub profile_revision: u64,
    pub binding_digest: Digest,
    pub run_generation: u64,
    pub result_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedDispatch {
    pub dispatch_id: String,
    pub run_id: String,
    pub slot_id: String,
    pub stage: RunStage,
    pub core_id: Option<CoreId>,
    pub attempt_generation: u64,
    pub input_digest: Digest,
    pub binding_digest: Digest,
    pub payload_digest: Digest,
    pub created_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DispatchState {
    Prepared,
    Dispatched,
    Settled,
    Unknown,
    AbortedBeforeDispatch,
    ReconciledNoEffect,
}

impl DispatchState {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Prepared => "prepared",
            Self::Dispatched => "dispatched",
            Self::Settled => "settled",
            Self::Unknown => "unknown",
            Self::AbortedBeforeDispatch => "aborted_before_dispatch",
            Self::ReconciledNoEffect => "reconciled_no_effect",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DispatchIdentity {
    pub store_id: String,
    pub store_generation: u64,
    pub dispatch_id: String,
    pub batch_key: String,
    pub batch_ordinal: u32,
    pub run_id: String,
    pub run_generation: u64,
    pub slot_id: String,
    pub stage: RunStage,
    pub core_id: Option<CoreId>,
    pub input_digest: Digest,
    pub binding_digest: Digest,
    pub payload_digest: Digest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DispatchRecord {
    pub identity: DispatchIdentity,
    pub state: DispatchState,
    pub provider_request_id: Option<String>,
    pub result_ref: Option<ContentObjectRef>,
    pub reconciliation_reference: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DispatchTransitionDisposition {
    Applied,
    Reconciled,
    Quarantined,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DispatchEventKind {
    Prepared,
    Dispatched,
    ProviderRequestIdentified,
    Settled,
    Unknown,
    AbortedBeforeDispatch,
    ReconciledNoEffect,
    LateCallbackQuarantined,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DispatchEvent {
    pub sequence: u64,
    pub identity: DispatchIdentity,
    pub event_kind: DispatchEventKind,
    pub previous_state: Option<DispatchState>,
    pub state: DispatchState,
    pub disposition: DispatchTransitionDisposition,
    pub provider_request_id: Option<String>,
    pub result_ref: Option<ContentObjectRef>,
    pub reconciliation_reference: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DispatchTransitionResult {
    pub dispatch: DispatchRecord,
    pub disposition: DispatchTransitionDisposition,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DispatchBatchReceipt {
    pub batch_key: String,
    pub command_id: String,
    pub run_id: String,
    pub dispatch_count: u32,
    pub dispatches_digest: Digest,
}

impl PreparedDispatch {
    pub fn validate(
        &self,
        run_id: &str,
        input_digest: &Digest,
        generation: u64,
    ) -> Result<(), String> {
        if self.dispatch_id.trim().is_empty() || self.slot_id.trim().is_empty() {
            return Err("dispatch and slot IDs must be non-empty".to_owned());
        }
        if self.run_id != run_id || &self.input_digest != input_digest {
            return Err("dispatch must use the target run's frozen input".to_owned());
        }
        if self.attempt_generation != generation {
            return Err("dispatch attempt generation is stale".to_owned());
        }
        if !self.input_digest.is_valid()
            || !self.binding_digest.is_valid()
            || !self.payload_digest.is_valid()
        {
            return Err("dispatch digests must be SHA-256".to_owned());
        }
        if self.created_at.trim().is_empty() {
            return Err("dispatch timestamp is missing".to_owned());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommitResult {
    pub receipt: magi_domain::CommandReceipt,
    pub event_position: Option<EventPosition>,
    pub dispatch_batch: DispatchBatchReceipt,
    pub duplicate: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredEvent {
    pub position: EventPosition,
    pub event: magi_domain::DomainEvent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsoleSnapshot {
    pub run: RunSnapshot,
    pub high_water: EventPosition,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversationSummary {
    pub conversation_id: String,
    pub title: String,
    pub revision: u64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryFinding {
    pub run_id: String,
    pub status: String,
    pub external_effect_unknown: bool,
    pub changed_to_interrupted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryReport {
    pub previous_generation: u64,
    pub current_generation: u64,
    pub findings: Vec<RecoveryFinding>,
}

#[cfg(test)]
mod profile_summary_privacy_tests {
    use super::*;

    #[test]
    fn serialized_profile_summary_omits_private_authority() {
        let summary = ProviderProfileSummary {
            provider_profile_id: "profile-fixture".into(),
            revision: 0,
            provider_id: "codex-acp".into(),
            display_name: "Fixture".into(),
            account_alias: "Fixture".into(),
            authentication_method: ProviderAuthenticationMethod::LocalSubscription,
            credential_configured: true,
            credential_home: Some(magi_domain::ProviderCredentialHome {
                authority_id: "private-authority".into(),
                canonical_path: "/Users/private/.codex".into(),
                device: 1,
                inode: 2,
                credential_store: magi_domain::ProviderCredentialStore::File,
                account_digest: Some(Digest::from_bytes(b"private-account")),
            }),
            digest: Digest::from_bytes(b"fixture-profile"),
            updated_at: "2026-10-01T00:00:00Z".into(),
        };
        let encoded = serde_json::to_value(summary).unwrap();
        assert!(encoded.get("credential_home").is_none());
        assert_eq!(encoded["credential_configured"], true);
        let json = encoded.to_string();
        assert!(!json.contains("private-authority"));
        assert!(!json.contains("/Users/private"));
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderModelSelection {
    pub binding: AcpModelBindingSnapshot,
    pub selection_revision: u64,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CoreModelSelection {
    pub core_id: CoreId,
    pub provider_profile_id: String,
    pub profile_revision: u64,
    pub model_selection_revision: u64,
    pub selection_revision: u64,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderModelSelectionInput {
    pub provider_profile_id: String,
    pub profile_revision: u64,
    pub catalog_snapshot_id: String,
    pub catalog_digest: Digest,
    pub model_id: String,
    pub mode_id: Option<String>,
    pub expected_selection_revision: Option<u64>,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CoreModelSelectionInput {
    pub core_id: CoreId,
    pub provider_profile_id: String,
    pub profile_revision: u64,
    pub model_selection_revision: u64,
    pub expected_selection_revision: Option<u64>,
    pub updated_at: String,
}
