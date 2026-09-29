// TypeScript companion to src-tauri/schemas/contracts.schema.json.
// Keep wire names and optionality aligned with the JSON Schema definitions.

export type OpaqueId = string;
export type Digest = string;
export type Timestamp = string;
export type CoreId = "MELCHIOR-1" | "BALTHASAR-2" | "CASPER-3";
export type QuestionKind = "answer" | "recommendation";

export interface QuestionSnapshot {
  schema_version: 1;
  question_id: OpaqueId;
  kind: QuestionKind;
  prompt: string;
  conditions?: string[];
  evaluation_criteria?: string[];
  selected_prior_dossier_ids?: OpaqueId[];
  digest: Digest;
}

export interface ModelBindingSnapshot {
  provider_profile_id: OpaqueId;
  revision: number;
  adapter_id: string;
  adapter_version: string;
  adapter_digest: Digest;
  model_id: string;
  context_window_tokens?: number | null;
  maximum_output_tokens?: number | null;
}

export interface CoreRoleProfile {
  core_id: CoreId;
  profile_id: OpaqueId;
  revision: number;
  display_name: string;
  review_purpose: string;
  evaluation_criteria: string[];
  falsification_questions: string[];
  response_language: string;
  binding: ModelBindingSnapshot;
}

export interface RoleSetSnapshot {
  schema_version: 1;
  role_set_id: OpaqueId;
  roles: [
    CoreRoleProfile & { core_id: "MELCHIOR-1" },
    CoreRoleProfile & { core_id: "BALTHASAR-2" },
    CoreRoleProfile & { core_id: "CASPER-3" },
  ];
  digest: Digest;
}

export interface SourceSnapshot {
  source_id: OpaqueId;
  object_digest: Digest;
  allowed_locators?: string[];
}

export interface ContextManifest {
  schema_version: 1;
  manifest_id: OpaqueId;
  sources: SourceSnapshot[];
  digest: Digest;
}

export interface InputSnapshot {
  schema_version: 1;
  question: QuestionSnapshot;
  context_manifest: ContextManifest;
  role_set: RoleSetSnapshot;
  policy_digest: Digest;
  protocol_version: 1;
  input_digest: Digest;
}

export interface EvidenceRef {
  source_id: OpaqueId;
  object_digest: Digest;
  locator: string;
}

export type ClaimKind =
  | "source_fact"
  | "model_knowledge"
  | "inference"
  | "preference"
  | "assumption";

export interface Claim {
  claim_id: OpaqueId;
  kind: ClaimKind;
  text: string;
  evidence_refs?: EvidenceRef[];
  limitations?: string[];
}

export type AssessmentStage = "independent_review" | "cross_review";
export type ClaimResponseKind = "agree" | "challenge" | "needs_evidence";

export interface ClaimResponse {
  target_claim_id: OpaqueId;
  response: ClaimResponseKind;
  rationale: string;
  evidence_refs?: EvidenceRef[];
}

export interface PositionChange {
  claim_id: OpaqueId;
  influenced_by_claim_ids: OpaqueId[];
  rationale: string;
  evidence_refs?: EvidenceRef[];
}

export interface InformationGap {
  missing_information: string;
  impact: string;
  essential: boolean;
}

export interface Counterargument {
  target_claim_id?: OpaqueId | null;
  rationale: string;
  evidence_refs?: EvidenceRef[];
}

export interface RoleAssessment {
  schema_version: 1;
  run_id: OpaqueId;
  attempt_id: OpaqueId;
  core_id: CoreId;
  stage: AssessmentStage;
  input_digest: Digest;
  attempt_generation: number;
  position_summary: string;
  claims: Claim[];
  assumptions?: string[];
  information_gaps?: InformationGap[];
  counterarguments?: Counterargument[];
  claim_responses?: ClaimResponse[];
  position_changes?: PositionChange[];
  created_at: Timestamp;
}

export interface ProposalClaim {
  claim_id: OpaqueId;
  kind: ClaimKind;
  text: string;
  evidence_refs?: EvidenceRef[];
  limitations?: string[];
}

export interface OpenObjection {
  claim_id: OpaqueId;
  rationale: string;
  required_information?: string[];
}

export interface ProposalSnapshot {
  schema_version: 1;
  proposal_id: OpaqueId;
  run_id: OpaqueId;
  question_digest: Digest;
  context_digest: Digest;
  roles_digest: Digest;
  kind: QuestionKind;
  body: string;
  claims: ProposalClaim[];
  conditions?: string[];
  alternatives?: string[];
  open_objections?: OpenObjection[];
  digest: Digest;
  created_at: Timestamp;
}

export type RunStage =
  | "independent_review"
  | "cross_review"
  | "synthesis"
  | "balloting";
export type PauseReason = "auth" | "quota" | "needs_input" | "validation";
export type Outcome = "unanimous" | "majority" | "rejected" | "unresolved";

export type RunStatus =
  | { status: "preparing" }
  | { status: "awaiting_confirmation" }
  | { status: "independent_review" }
  | { status: "cross_review" }
  | { status: "synthesis" }
  | { status: "balloting" }
  | { status: "paused"; reason: PauseReason; detail: string; resume_stage: RunStage }
  | {
      status: "interrupted";
      resume_stage: RunStage;
      detail: string;
      cancellation_requested: boolean;
    }
  | { status: "cancelling" }
  | { status: "completed"; outcome: Outcome }
  | { status: "cancelled" }
  | { status: "failed"; reason_code: string };

export interface Run {
  schema_version: 1;
  run_id: OpaqueId;
  conversation_id: OpaqueId;
  parent_run_id?: OpaqueId | null;
  question_id: OpaqueId;
  context_manifest_id: OpaqueId;
  role_set_id: OpaqueId;
  input_digest: Digest;
  protocol_version: 1;
  status: RunStatus;
  revision: number;
  generation: number;
  created_at: Timestamp;
  updated_at: Timestamp;
}

export type VoteValue = "support" | "oppose" | "abstain";

export interface Ballot {
  schema_version: 1;
  run_id: OpaqueId;
  attempt_id: OpaqueId;
  attempt_generation: number;
  core_id: CoreId;
  input_digest: Digest;
  proposal_id: OpaqueId;
  proposal_digest: Digest;
  vote: VoteValue;
  rationale: string;
  objection_refs?: OpaqueId[];
  created_at: Timestamp;
}

export interface VoteCounts {
  support: number;
  oppose: number;
  abstain: number;
}

export type Tally = {
  run_id: OpaqueId;
  input_digest: Digest;
  proposal_id: OpaqueId;
  proposal_digest: Digest;
} & (
  | { counts: { support: 3; oppose: 0; abstain: 0 }; outcome: "unanimous" }
  | {
      counts:
        | { support: 2; oppose: 1; abstain: 0 }
        | { support: 2; oppose: 0; abstain: 1 };
      outcome: "majority";
    }
  | {
      counts:
        | { support: 1; oppose: 2; abstain: 0 }
        | { support: 0; oppose: 3; abstain: 0 }
        | { support: 0; oppose: 2; abstain: 1 };
      outcome: "rejected";
    }
  | {
      counts:
        | { support: 1; oppose: 1; abstain: 1 }
        | { support: 1; oppose: 0; abstain: 2 }
        | { support: 0; oppose: 1; abstain: 2 }
        | { support: 0; oppose: 0; abstain: 3 };
      outcome: "unresolved";
    }
);

export type CommandKind =
  | "create_run"
  | "confirm_run"
  | "pause_run"
  | "resume_run"
  | "cancel_run"
  | "add_assessment"
  | "freeze_proposal"
  | "add_ballot"
  | "delete_run"
  | "create_child_run";

export interface CommandEnvelope {
  command_id: OpaqueId;
  idempotency_key: OpaqueId;
  command_kind: CommandKind;
  target_id: OpaqueId;
  expected_revision: number;
  payload_digest: Digest;
}

export interface EventPosition {
  store_id: OpaqueId;
  generation: number;
  sequence: number;
}

export interface CommandReceipt {
  command_id: OpaqueId;
  command_kind: CommandKind;
  target_id: OpaqueId;
  payload_digest: Digest;
  accepted_revision: number;
  event_position?: EventPosition | null;
  accepted_at: Timestamp;
}

export type EventKind =
  | "run_created"
  | "confirmation_required"
  | "run_started"
  | "phase_advanced"
  | "assessment_accepted"
  | "essential_information_needed"
  | "proposal_frozen"
  | "ballot_sealed"
  | "ballots_revealed"
  | "run_paused"
  | "run_interrupted"
  | "cancellation_requested"
  | "run_cancelled"
  | "run_failed"
  | "attempt_fenced";

export type EventPayload =
  | { type: "run_created"; data: { run_id: OpaqueId; input_digest: Digest } }
  | { type: "confirmation_required"; data: { run_id: OpaqueId } }
  | { type: "run_started"; data: { run_id: OpaqueId; stage: RunStage } }
  | { type: "phase_advanced"; data: { from: RunStage; to: RunStage } }
  | {
      type: "assessment_accepted";
      data: {
        assessment_id: OpaqueId;
        core_id: CoreId;
        stage: AssessmentStage;
        accepted_count: number;
      };
    }
  | {
      type: "essential_information_needed";
      data: { gap_count: number; resume_stage: RunStage };
    }
  | { type: "proposal_frozen"; data: { proposal_id: OpaqueId; proposal_digest: Digest } }
  | { type: "ballot_sealed"; data: { submitted_count: number } }
  | { type: "ballots_revealed"; data: { ballots: Ballot[]; tally: Tally } }
  | {
      type: "run_paused";
      data: { reason: PauseReason; detail: string; resume_stage: RunStage };
    }
  | {
      type: "run_interrupted";
      data: {
        detail: string;
        resume_stage: RunStage;
        cancellation_requested: boolean;
        external_effect_unknown: boolean;
      };
    }
  | { type: "cancellation_requested"; data: { generation: number } }
  | { type: "run_cancelled" }
  | { type: "run_failed"; data: { reason_code: string } }
  | { type: "attempt_fenced"; data: { generation: number } };

export interface DomainEvent {
  run_id: OpaqueId;
  run_revision: number;
  generation: number;
  event_type: EventKind;
  payload: EventPayload;
  created_at: Timestamp;
}

export interface RunCheckpoint {
  run_id: OpaqueId;
  input_digest: Digest;
  status: RunStatus;
  resume_stage?: RunStage | null;
  generation: number;
  completed_assessment_attempt_ids: OpaqueId[];
  pending_dispatch_ids: OpaqueId[];
  essential_input_pending: boolean;
  external_effect_unknown: boolean;
  latest_event_sequence: number;
  revision: number;
  updated_at: Timestamp;
}

export interface RunSnapshot {
  run: Run;
  input: InputSnapshot;
  assessments: RoleAssessment[];
  proposal?: ProposalSnapshot | null;
  submitted_ballot_count: number;
  ballots_revealed?: Ballot[] | null;
  tally?: Tally | null;
  essential_input_pending: boolean;
  external_effect_unknown: boolean;
  latest_event_sequence: number;
}

export interface RunPersistenceState {
  run: Run;
  input: InputSnapshot;
  assessments: RoleAssessment[];
  proposal?: ProposalSnapshot | null;
  sealed_ballots: Ballot[];
  tally?: Tally | null;
  essential_input_pending: boolean;
  external_effect_unknown: boolean;
  cancel_resume_stage?: RunStage | null;
  event_drafts: DomainEvent[];
}

export interface CoreRoleDefinition {
  core_id: CoreId;
  profile_id: OpaqueId;
  display_name: string;
  review_purpose: string;
  evaluation_criteria: string[];
  falsification_questions: string[];
  response_language: string;
}

export interface RolePresetInput {
  preset_id: OpaqueId;
  display_name: string;
  roles: [
    CoreRoleDefinition & { core_id: "MELCHIOR-1" },
    CoreRoleDefinition & { core_id: "BALTHASAR-2" },
    CoreRoleDefinition & { core_id: "CASPER-3" },
  ];
}

export interface RolePresetRevision extends RolePresetInput {
  schema_version: 1;
  revision: number;
  digest: Digest;
}

export type RolePresetSource = "factory" | "user";

export interface RolePresetSummary {
  preset_id: OpaqueId;
  display_name: string;
  revision: number;
  digest: Digest;
  source: RolePresetSource;
  updated_at: Timestamp;
}

export interface ActiveRolePresetSelection {
  preset_id: OpaqueId;
  selection_revision: number;
  updated_at: Timestamp;
}

export type ProviderAuthenticationMethod = "local_subscription" | "byok_api";

export interface ProviderProfileSummary {
  provider_profile_id: OpaqueId;
  revision: number;
  provider_id: OpaqueId;
  display_name: string;
  account_alias: string;
  authentication_method: ProviderAuthenticationMethod;
  credential_configured: boolean;
  digest: Digest;
  updated_at: Timestamp;
}

export interface ActiveProviderProfileSelection {
  provider_id: OpaqueId;
  provider_profile_id: OpaqueId;
  selection_revision: number;
  updated_at: Timestamp;
}

export interface StoreIdentity {
  store_id: OpaqueId;
  generation: number;
  schema_version: number;
}

export interface ContentObjectRef {
  digest: Digest;
  byte_length: number;
}

export interface Recipient {
  provider_id: OpaqueId;
  account_profile_id: OpaqueId;
}

export type DisclosureState = "draft" | "approved";
export type ManifestSourceState = "captured" | "excluded" | "failed";
export type RepresentationKind = "utf8_text";
export type SourceOmissionCode =
  | "user_excluded"
  | "hidden_path"
  | "credential_path"
  | "symbolic_link"
  | "special_file"
  | "cross_volume"
  | "unsupported_format"
  | "invalid_utf8"
  | "file_too_large"
  | "manifest_limit"
  | "access_denied"
  | "changed_during_capture"
  | "revoked_grant"
  | "invalid_range"
  | "read_failure";
export type SecretPatternKind =
  | "credential_assignment"
  | "token_prefix"
  | "private_key_block";

export interface EvidenceLocator {
  source_id: OpaqueId;
  object_digest: Digest;
  start_line: number | null;
  end_line: number | null;
  total_lines: number | null;
}

export interface SourceOmission {
  code: SourceOmissionCode;
  detail: string;
}

export interface SecretPatternFinding {
  line: number;
  kind: SecretPatternKind;
}

export interface ManifestSource {
  source_id: OpaqueId;
  display_name: string;
  state: ManifestSourceState;
  byte_length: number | null;
  mime_type: string | null;
  object_digest: Digest | null;
  derived_digest: Digest | null;
  representation_kind: RepresentationKind | null;
  extractor_id: string | null;
  extractor_version: string | null;
  included_locators: EvidenceLocator[];
  omission: SourceOmission | null;
  captured_at_epoch_ms: number | null;
  secret_pattern_findings: SecretPatternFinding[];
  secret_scan_incomplete: boolean;
}

export interface SourceCaptureManifestContent {
  sources: ManifestSource[];
  recipients: Recipient[];
}

export interface SourceCaptureManifest {
  schema_version: 1;
  manifest_id: OpaqueId;
  created_at_epoch_ms: number;
  content: SourceCaptureManifestContent;
  disclosure_state: DisclosureState;
  digest: Digest;
}

export interface ContextDraft {
  draft_id: OpaqueId;
  revision: number;
  manifest: SourceCaptureManifest;
  updated_at_epoch_ms: number;
}

export interface ContextDraftSummary {
  draft_id: OpaqueId;
  revision: number;
  manifest_id: OpaqueId;
  source_count: number;
  captured_source_count: number;
  updated_at_epoch_ms: number;
}

export type RunStatusFilter =
  | "preparing"
  | "awaiting_confirmation"
  | "independent_review"
  | "cross_review"
  | "synthesis"
  | "balloting"
  | "paused"
  | "interrupted"
  | "cancelling"
  | "completed"
  | "cancelled"
  | "failed";

export interface RunHistoryFilter {
  question_query?: string | null;
  statuses: RunStatusFilter[];
  created_after?: string | null;
  created_before?: string | null;
}

export interface RunHistoryCursor {
  store_id: OpaqueId;
  store_generation: number;
  membership_generation: number;
  filter_digest: Digest;
  last_created_at: Timestamp;
  last_run_id: OpaqueId;
}

export interface RunHistoryRequest {
  filter: RunHistoryFilter;
  cursor: RunHistoryCursor | null;
  page_size: number;
}

export interface RunSummary {
  run_id: OpaqueId;
  conversation_id: OpaqueId;
  question: string;
  status: RunStatus;
  revision: number;
  input_digest: Digest;
  created_at: Timestamp;
  updated_at: Timestamp;
}

export interface RunHistoryPage {
  items: RunSummary[];
  next_cursor: RunHistoryCursor | null;
}

export interface RunEventCursor {
  store_id: OpaqueId;
  store_generation: number;
  run_id: OpaqueId;
  after_sequence: number;
  high_water_sequence: number;
}

export interface StoredEvent {
  position: EventPosition;
  event: DomainEvent;
}

export interface RunEventPage {
  events: StoredEvent[];
  next_cursor: RunEventCursor;
  complete: boolean;
}

export type FreshnessStatus =
  | "unchanged"
  | "changed"
  | "missing"
  | "unreadable"
  | "unchecked";

export interface FreshnessObservation {
  status: FreshnessStatus;
  observed_at_epoch_ms: number;
  observed_digest: Digest | null;
}

export interface SourceFreshnessRecord {
  run_id: OpaqueId;
  source_id: OpaqueId;
  captured_digest: Digest;
  observation: FreshnessObservation;
}

export type DispatchState =
  | "prepared"
  | "dispatched"
  | "settled"
  | "unknown"
  | "aborted_before_dispatch"
  | "reconciled_no_effect";
export type DispatchTransitionDisposition = "applied" | "reconciled" | "quarantined";
export type DispatchEventKind =
  | DispatchState
  | "provider_request_identified"
  | "late_callback_quarantined";

export interface DispatchIdentity {
  store_id: OpaqueId;
  store_generation: number;
  dispatch_id: OpaqueId;
  batch_key: OpaqueId;
  batch_ordinal: number;
  run_id: OpaqueId;
  run_generation: number;
  slot_id: OpaqueId;
  stage: RunStage;
  core_id: CoreId | null;
  input_digest: Digest;
  binding_digest: Digest;
  payload_digest: Digest;
}

export interface PreparedDispatch {
  dispatch_id: OpaqueId;
  run_id: OpaqueId;
  slot_id: OpaqueId;
  stage: RunStage;
  core_id: CoreId | null;
  attempt_generation: number;
  input_digest: Digest;
  binding_digest: Digest;
  payload_digest: Digest;
  created_at: Timestamp;
}

export interface DispatchRecord {
  identity: DispatchIdentity;
  state: DispatchState;
  provider_request_id: string | null;
  result_ref: ContentObjectRef | null;
  reconciliation_reference: string | null;
  created_at: Timestamp;
  updated_at: Timestamp;
}

export interface DispatchEvent {
  sequence: number;
  identity: DispatchIdentity;
  event_kind: DispatchEventKind;
  previous_state: DispatchState | null;
  state: DispatchState;
  disposition: DispatchTransitionDisposition;
  provider_request_id: string | null;
  result_ref: ContentObjectRef | null;
  reconciliation_reference: string | null;
  created_at: Timestamp;
}

export interface DispatchTransitionResult {
  dispatch: DispatchRecord;
  disposition: DispatchTransitionDisposition;
}

export interface DispatchBatchReceipt {
  batch_key: OpaqueId;
  command_id: OpaqueId;
  run_id: OpaqueId;
  dispatch_count: number;
  dispatches_digest: Digest;
}

export interface DispatchEventCursor {
  store_id: OpaqueId;
  store_generation: number;
  run_id: OpaqueId;
  after_sequence: number;
  high_water_sequence: number;
}

export interface DispatchEventPage {
  events: DispatchEvent[];
  next_cursor: DispatchEventCursor;
  complete: boolean;
}

export interface CommitResult {
  receipt: CommandReceipt;
  event_position: EventPosition | null;
  dispatch_batch: DispatchBatchReceipt;
  duplicate: boolean;
}

export interface ConversationSummary {
  conversation_id: OpaqueId;
  title: string;
  revision: number;
  created_at: Timestamp;
  updated_at: Timestamp;
}

export interface ConsoleSnapshot {
  run: RunSnapshot;
  high_water: EventPosition;
}

export interface RunDossier {
  snapshot: RunSnapshot;
  capture_manifest: SourceCaptureManifest | null;
  source_freshness: SourceFreshnessRecord[];
  dispatches: DispatchRecord[];
  decision_dossier_digest: Digest | null;
  replay_cursor: RunEventCursor;
  dispatch_replay_cursor: DispatchEventCursor;
}

export interface RecoveryFinding {
  run_id: OpaqueId;
  status: string;
  external_effect_unknown: boolean;
  changed_to_interrupted: boolean;
}

export interface RecoveryReport {
  previous_generation: number;
  current_generation: number;
  findings: RecoveryFinding[];
}
