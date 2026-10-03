mod error;
mod model;
mod replay;
mod store;

pub use error::StorageError;

pub struct DisclosureTurnRequest<'a> {
    pub slot_ordinal: u8,
    pub attempt_id: &'a str,
    pub request_digest: &'a magi_domain::Digest,
    pub content_kinds: &'a std::collections::BTreeSet<magi_context::DisclosureContentKind>,
}

pub use magi_domain::{
    AcpMode, AcpModelBindingSnapshot, Digest, LiveProviderResult, LiveProviderUsage, LiveRunEvent,
    LiveRunEventCursor, LiveRunEventKind, LiveRunFailure, LiveRunQueueProjection, LiveRunSnapshot,
    LiveRunStatus, OpaqueId, ProviderCatalogModel, ProviderCatalogSnapshot,
};
pub use model::{
    ActiveProviderProfileSelection, ActiveRolePresetSelection, AdmissionActivationPermission,
    AdmissionExecutionAuthority, AdmissionRequestBinding, AdmissionRequestCancellationReceipt,
    AdmissionRequestIntent, ClarificationAdmissionIntent, ClarificationDraft,
    ClarificationParentReference, CommitResult, ConsoleSnapshot, ContentObjectRef, ContextDraft,
    ContextDraftSummary, ConversationSummary, CoreBindingReference, CoreModelSelection,
    CoreModelSelectionInput, DispatchBatchReceipt, DispatchEvent, DispatchEventCursor,
    DispatchEventKind, DispatchEventPage, DispatchIdentity, DispatchRecord, DispatchState,
    DispatchTransitionDisposition, DispatchTransitionResult, LiveDispatchProjection,
    LiveDispatchRecord, LiveDispatchState, LiveProviderResultInput, LiveRunAdmissionOutcome,
    LiveRunAdmissionRequest, LiveRunChange, LiveRunClaim, LiveRunReceipt, PreparedDispatch,
    ProviderModelSelection, ProviderModelSelectionInput, ProviderProfileSummary,
    ProviderSourceScope, RecoveryFinding, RecoveryReport, RolePresetInput, RolePresetSource,
    RolePresetSummary, RunDossier, RunEventCursor, RunEventPage, RunHistoryCursor,
    RunHistoryFilter, RunHistoryPage, RunHistoryRequest, RunStatusFilter, RunSummary,
    SourceFreshnessRecord, StoreIdentity, StoredEvent,
};
pub use store::{
    ActiveClockBudget, BackupManifest, DeletionReceipt, EvidenceDeletionPreview, EvidenceView,
    GrantReservation, LIVE_RUN_DISPATCH_CAPACITY, LIVE_RUN_QUEUE_CAPACITY, LiveRunPausePermission,
    MAX_OBJECT_BYTES, RestoreReceipt, Storage, StorageReader,
};

pub use replay::{
    ExternalReplay, ExternalReplaySummary, MAX_REPLAY_BYTES, PublicAssessment, PublicBallot,
    PublicProposalContent, Redaction, ReplayEnd, ReplayEvent, ReplayPayload, ReplayPhase,
    ReplayPublicContent, SharedProposal, SharedReplay, import_replay,
};
