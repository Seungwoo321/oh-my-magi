import { isTauri, invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type ConnectionState = "unknown" | "runtime_available" | "blocked";
export type StorageState = "unknown" | "ready" | "error";
export type StorageUnavailableDiagnostic = {
  code: "app_data_unavailable" | "migration_checksum_mismatch" | "unsupported_schema_version" | "store_integrity_check_failed" | "store_access_denied" | "store_already_open" | "store_initialization_failed";
  message: string;
  action: string;
};
export type BallotChoice = "support" | "oppose" | "abstain";

export type RunDossierView = {
  runId: string;
  question: string;
  status: string;
  stage: string;
  proposal: {
    body: string;
    conditions: string[];
    alternatives: string[];
    openObjections: Array<{ claimId: string; rationale: string; requiredInformation: string[] }>;
  } | null;
  votes: Array<{
    coreId: "MELCHIOR-1" | "BALTHASAR-2" | "CASPER-3";
    choice: BallotChoice;
    rationale: string;
  }>;
  outcome: "unanimous" | "majority" | "rejected" | "unresolved" | null;
  error?: { code: string; message: string; profileBinding?: ProfileAuthenticationBinding };
};

export type RunUpdate = {
  runId: string;
  stage: string;
  coreId?: "MELCHIOR-1" | "BALTHASAR-2" | "CASPER-3";
  state: "started" | "streaming" | "completed" | "failed" | "cancelled";
  text?: string;
  result?: RunDossierView;
};

export type ConsoleRunSummary = {
  id: string;
  question: string;
  stage: string;
  status: string;
  sourceCount: number;
  roles: Array<{ core: string; label: string }>;
  ballotState: "none" | "sealed" | "public";
  votes?: BallotChoice[];
  outcome?: string;
};

export type ConsoleSnapshot = {
  schemaVersion: 1;
  connection: ConnectionState;
  storage: StorageState;
  storageDiagnostic?: StorageUnavailableDiagnostic;
  activeRun?: ConsoleRunSummary;
  confirmedAt?: string;
  eventSequence?: number;
};

export type CapturedSourceSummary = {
  sourceId: string;
  displayName: string;
  status: "captured" | "excluded" | "failed";
  byteLength: number;
  representation: "utf8_text" | "pdf_text" | "pdf_raster" | "image" | "unsupported" | "unknown";
  digest?: string;
  issueCodes: string[];
};

export type ContextSelectionSummary = {
  draftId: string;
  manifestId: string;
  manifestDigest: string;
  revision: number;
  sources: CapturedSourceSummary[];
};

export type PdfRangePreview = {
  selectionToken: string;
  draftId: string;
  contextRevision: number | null;
  displayName: string;
  capturedDigest: string;
  totalPages: number;
  expiresAtEpochMs: number;
};

export type ContextSelectionRequest = {
  draftId: string;
  expectedRevision: number;
};

export type RecentRunSummary = {
  runId: string;
  question: string;
  status: string;
  createdAt: string;
};

export type CredentialHome = {
  displayPath: string;
};

export type ProviderProfileSummary = {
  providerProfileId: string;
  revision: number;
  providerId: string;
  displayName: string;
  accountAlias: string;
  authenticationMethod: "local_subscription" | "byok_api";
  credentialConfigured: boolean;
  credentialHome?: CredentialHome | null;
  digest: string;
  updatedAt: string;
};

export type ProviderSourceScope = {
  grantId: string;
  providerProfileId: string;
  path: string;
  available: boolean;
  createdAt: string;
};

export type ActiveProviderProfileSelection = {
  providerId: string;
  providerProfileId: string;
  selectionRevision: number;
  updatedAt: string;
};

export type ProviderProfileDraft = {
  profileId: string | null;
  expectedRevision: number | null;
  displayName: string;
  adapterId: string;
  credentialHomePath: string;
};

export type ProviderAdmissionFailure =
  | "adapter_unsupported"
  | "home_override_unsupported"
  | "home_mismatch"
  | "attestation_missing"
  | "other"
  | "unsupported_platform"
  | "invalid_launch"
  | "artifact_verification_failed"
  | "profile_home_unavailable"
  | "role_workdir_unavailable"
  | "isolation_unavailable"
  | "sandbox_network_rule_rejected"
  | "sandbox_profile_rejected"
  | "sandbox_policy_probe_timed_out"
  | "process_start_failed"
  | "process_closed"
  | "process_exited"
  | "protocol_error"
  | "remote_request_failed"
  | "rpc_timeout"
  | "proxy_unavailable"
  | "invalid_response"
  | "authentication_unsupported"
  | "unauthenticated"
  | "session_already_created"
  | "catalog_busy"
  | "model_unavailable"
  | "session_unavailable"
  | "source_read_unavailable"
  | "prompt_in_progress"
  | "tool_denied"
  | "output_limit"
  | "input_limit"
  | "event_limit"
  | "timeout"
  | "cancelled";

export type ProviderAdmission =
  | { state: "ready"; profileId: string; profileRevision: number; adapterId: string; rootBinding: "verified"; checkedAt: string }
  | { state: "blocked"; profileId: string; profileRevision: number; adapterId: string; rootBinding: "unverified"; reason: ProviderAdmissionFailure; checkedAt: string };

export type ProviderValidationFailure = {
  profileId: string;
  profileRevision: number;
  code: "validation_command_failed";
};

export type AuthProfileResult = {
  providerId: string;
  profileId: string;
  profileRevision: number;
  state: "authenticated" | "unauthenticated" | "unsupported";
  method?: "chat_gpt";
  checkedAt: string;
  remediationCategory?: string;
};

export type ProviderAuthProgressStage =
  | "opening"
  | "checking_status"
  | "starting_login"
  | "browser_opening"
  | "launch_requested"
  | "launcher_accepted"
  | "callback_complete"
  | "failed"
  | "cancelled";

export type ProviderAuthProgress = {
  profileId: string;
  profileRevision: number;
  stage: ProviderAuthProgressStage;
};

export type ProviderCatalogSnapshot = {
  schemaVersion: number;
  catalogSnapshotId: string;
  catalogDigest: string;
  providerId: string;
  acpMode: "acp";
  providerProfileId: string;
  profileRevision: number;
  adapterId: string;
  adapterVersion: string;
  adapterDigest: string;
  artifactSetDigest?: string;
  fetchedAt: string;
  negotiatedModes?: { currentModeId: string | null; modes: Array<{ modeId: string; name: string; description: string | null }> };
  models: Array<{
    modelId: string;
    name?: string | null;
    description?: string | null;
    contextWindowTokens?: number | null;
    maxOutputTokens?: number | null;
  }>;
};

export type AcpModelBindingInput = Pick<ProviderCatalogSnapshot,
  "schemaVersion" | "catalogSnapshotId" | "catalogDigest" | "providerId" | "acpMode"
  | "providerProfileId" | "profileRevision" | "adapterId" | "adapterVersion" | "adapterDigest" | "artifactSetDigest"
> & { modelId: string; modeId?: string | null };

export type AcpModelBindingSnapshot = AcpModelBindingInput & { bindingDigest: string };

export function catalogHasExecutionAuthority(catalog: ProviderCatalogSnapshot | null | undefined): catalog is ProviderCatalogSnapshot & { artifactSetDigest: string; negotiatedModes: NonNullable<ProviderCatalogSnapshot["negotiatedModes"]> } {
  if (catalog?.schemaVersion !== 3 || typeof catalog.artifactSetDigest !== "string"
    || !/^[a-f0-9]{64}$/.test(catalog.artifactSetDigest)) return false;
  const modes = catalog.negotiatedModes;
  if (!modes || !Array.isArray(modes.modes)) return false;
  const ids = modes.modes.map(mode => mode?.modeId);
  return ids.every(id => typeof id === "string" && id.trim().length > 0 && !/[\u0000-\u001f\u007f]/.test(id))
    && new Set(ids).size === ids.length
    && (modes.currentModeId === null || typeof modes.currentModeId === "string" && ids.includes(modes.currentModeId));
}

export function executionValuesEqual(left: unknown, right: unknown): boolean {
  const canonical = (value: unknown): unknown => Array.isArray(value) ? value.map(canonical)
    : value !== null && typeof value === "object" ? Object.fromEntries(Object.entries(value).sort(([a], [b]) => a.localeCompare(b)).map(([key, item]) => [key, canonical(item)])) : value;
  return JSON.stringify(canonical(left)) === JSON.stringify(canonical(right));
}

export function catalogsHaveEquivalentExecutionAuthority(left: ProviderCatalogSnapshot | null | undefined, right: ProviderCatalogSnapshot | null | undefined): boolean {
  if (!catalogHasExecutionAuthority(left) || !catalogHasExecutionAuthority(right)) return false;
  const authority = (catalog: ProviderCatalogSnapshot) => {
    const { catalogSnapshotId: _id, fetchedAt: _time, catalogDigest: _digest, ...semantics } = catalog;
    return semantics;
  };
  return executionValuesEqual(authority(left), authority(right));
}

export type LiveRunProviderOutcome = "not_started" | "pending" | "confirmed" | "unknown";
export type LiveRunRemediationCategory = "reauthenticate" | "refresh_catalog" | "review_request" | "retry_later";
export type LiveRunStatus = "queued" | "claimed" | "session_creation_intent" | "running" | "cancelling" | "cancelled" | "interrupted" | "unknown" | "completed" | "failed";

export type ProfileAuthenticationBinding = { providerProfileId: string; profileRevision: number };

export function authenticationFailureBinding(error: unknown): ProfileAuthenticationBinding | undefined {
  if (!error || typeof error !== "object") return undefined;
  const failure = error as { code?: unknown; profileBinding?: unknown };
  if (!["authentication_required", "authentication_status_rpc_failed", "authentication_unsupported"].includes(String(failure.code))
    || !failure.profileBinding || typeof failure.profileBinding !== "object") return undefined;
  const binding = failure.profileBinding as Partial<ProfileAuthenticationBinding>;
  return typeof binding.providerProfileId === "string" && binding.providerProfileId.length > 0
    && typeof binding.profileRevision === "number" && Number.isSafeInteger(binding.profileRevision) && binding.profileRevision >= 0
    ? { providerProfileId: binding.providerProfileId, profileRevision: binding.profileRevision } : undefined;
}

export type LiveRunFailure = {
  code: string;
  detail: string;
  externalEffectUnknown: boolean;
  profileBinding?: ProfileAuthenticationBinding;
  remediationCategory?: LiveRunRemediationCategory;
};

export type LiveRunEventCursor = {
  storeId: string;
  storeGeneration: number;
  runId: string;
  afterSequence: number;
  highWaterSequence: number;
  complete: boolean;
};

export type LiveRunSnapshot = {
  schemaVersion: number;
  storeId: string;
  storeGeneration: number;
  runId: string;
  question: string;
  status: LiveRunStatus;
  revision: number;
  modelBinding: AcpModelBindingSnapshot;
  queue: {
    admissionSequence: number;
    state: LiveRunStatus;
    position: number | null;
    admittedCount: number;
    capacity: number;
  };
  eventCursor: LiveRunEventCursor;
  events: Array<{
    sequence: number;
    storeGeneration: number;
    runRevision: number;
    claimGeneration: number;
    kind: "status_changed" | "text_delta" | "security_violation";
    status?: LiveRunStatus | null;
    textDelta?: string | null;
    failure?: LiveRunFailure | null;
    createdAt: string;
  }>;
  cancellation?: {
    requestedAt: string;
    providerOutcome: LiveRunProviderOutcome;
  } | null;
  result?: {
    finalText: string;
    stopReason: string;
    usage?: {
      totalTokens?: number | null;
      inputTokens?: number | null;
      cachedReadTokens?: number | null;
      outputTokens?: number | null;
      thoughtTokens?: number | null;
    } | null;
    contentDigest: string;
    contentByteLength: number;
  } | null;
  failure?: LiveRunFailure | null;
  createdAt: string;
  updatedAt: string;
};

export type LiveRunReceipt = {
  commandId: string;
  runId: string;
  acceptedRevision: number;
  queue: LiveRunSnapshot["queue"];
  eventCursor: LiveRunEventCursor;
};

export type CancelLiveRunInput = {
  commandId: string;
  idempotencyKey: string;
  runId: string;
  expectedRevision: number;
};

export type LiveRunCancellationReceipt = {
  commandId: string;
  runId: string;
  acceptedRevision: number;
  status: LiveRunStatus;
  eventCursor: LiveRunEventCursor;
  providerOutcome: LiveRunProviderOutcome;
};

export type LiveRunError = {
  code: string;
  message: string;
  remediationCategory?: LiveRunRemediationCategory;
  retryable: boolean;
  capacity?: number;
  admittedCount?: number;
  profileBinding?: ProfileAuthenticationBinding;
};

export type LiveRunChanged = Pick<LiveRunSnapshot, "runId" | "revision"> & {
  eventCursor: LiveRunEventCursor;
};

export type StoredCoreRole = {
  coreId: "MELCHIOR-1" | "BALTHASAR-2" | "CASPER-3";
  profileId: string;
  displayName: string;
  reviewPurpose: string;
  evaluationCriteria: string[];
  falsificationQuestions: string[];
  responseLanguage: string;
};

export type StoredRolePreset = {
  presetId: string;
  revision: number;
  displayName: string;
  roles: [StoredCoreRole, StoredCoreRole, StoredCoreRole];
  digest: string;
  source: "factory" | "user";
};

export type RoleStoreDiagnostic = {
  code: "role_store_unavailable" | "role_store_read_failed" | "role_store_inconsistent" | "role_store_response_invalid";
  stage: "command_access" | "open" | "list_presets" | "load_revision" | "missing_revision" | "load_selection" | "map_presets" | "unknown";
};

const roleStoreDiagnosticCodes = new Set<RoleStoreDiagnostic["code"]>([
  "role_store_unavailable",
  "role_store_read_failed",
  "role_store_inconsistent",
  "role_store_response_invalid",
]);
const roleStoreDiagnosticStages = new Set<RoleStoreDiagnostic["stage"]>([
  "command_access",
  "open",
  "list_presets",
  "load_revision",
  "missing_revision",
  "load_selection",
  "map_presets",
  "unknown",
]);

export function normalizeRoleStoreDiagnostic(
  error: unknown,
  fallbackStage: RoleStoreDiagnostic["stage"] = "unknown",
): RoleStoreDiagnostic {
  let payload: unknown = error;
  if (typeof payload === "string") {
    try {
      payload = JSON.parse(payload) as unknown;
    } catch {
      payload = null;
    }
  }
  if (typeof payload === "object" && payload !== null && "error" in payload) {
    payload = payload.error;
  }
  if (typeof payload === "object" && payload !== null && "code" in payload && "stage" in payload) {
    const { code, stage } = payload;
    if (typeof code === "string" && roleStoreDiagnosticCodes.has(code as RoleStoreDiagnostic["code"])
      && typeof stage === "string" && roleStoreDiagnosticStages.has(stage as RoleStoreDiagnostic["stage"])) {
      return { code: code as RoleStoreDiagnostic["code"], stage: stage as RoleStoreDiagnostic["stage"] };
    }
  }
  return { code: fallbackStage === "map_presets" ? "role_store_response_invalid" : "role_store_unavailable", stage: fallbackStage };
}

export type ActiveRolePresetSelection = {
  presetId: string;
  selectionRevision: number;
  updatedAt: string;
};

export type RolePresetDraftInput = {
  presetId: string | null;
  expectedRevision: number | null;
  displayName: string;
  roles: Array<{
    core: "melchior" | "balthasar" | "casper";
    label: string;
    perspective: string;
    criteria: string[];
    challengeCondition: string[];
    outputLanguage: "same" | "ko" | "en";
  }>;
};

export type ConsolePreferences = {
  motion: "full" | "reduced" | "off";
  sound: boolean;
  theme: "command" | "clear";
  fontScale: 100 | 125 | 150 | 200;
  language: "ko" | "en";
};

export type ShellContext = {
  windowLabel: "main" | "companion";
  platform: string;
};

export type ShellEventName = "magi:open-settings" | "magi:exit-requested" | "magi:preferences-changed";

const unavailableSnapshot: ConsoleSnapshot = {
  schemaVersion: 1,
  connection: "blocked",
  storage: "unknown",
};

export function isDesktopApp(): boolean {
  return isTauri();
}

export async function getConsoleSnapshot(): Promise<ConsoleSnapshot> {
  if (!isDesktopApp()) return unavailableSnapshot;
  return invoke<ConsoleSnapshot>("get_console_snapshot");
}

export async function selectContextFiles(
  request?: ContextSelectionRequest,
): Promise<ContextSelectionSummary | null> {
  if (!isDesktopApp()) return null;
  return invoke<ContextSelectionSummary | null>("select_context_files", request ? { request } : {});
}

export async function selectContextDirectory(
  request?: ContextSelectionRequest,
): Promise<ContextSelectionSummary | null> {
  if (!isDesktopApp()) return null;
  return invoke<ContextSelectionSummary | null>("select_context_directory", request ? { request } : {});
}

export async function preparePdfRangeCapture(selection: ContextSelectionSummary | null): Promise<PdfRangePreview | null> {
  if (!isDesktopApp()) throw new Error("Native PDF selection is unavailable.");
  const result = await invoke<PdfRangePreview | null>("prepare_pdf_range_capture", { draftId: selection?.draftId ?? null, contextRevision: selection?.revision ?? null });
  if (result && (!result.selectionToken || !result.draftId || !result.displayName || (result.contextRevision !== null && (!Number.isSafeInteger(result.contextRevision) || result.contextRevision < 0)) || (!selection && result.contextRevision !== null) || (selection && (result.draftId !== selection.draftId || result.contextRevision !== selection.revision)) || !Number.isSafeInteger(result.totalPages) || result.totalPages < 1 || result.totalPages > 1000 || !Number.isSafeInteger(result.expiresAtEpochMs) || !/^[a-f0-9]{64}$/.test(result.capturedDigest))) {
    if (typeof result.selectionToken === "string" && result.selectionToken) await discardPdfRangeCapture(result.selectionToken).catch(() => undefined);
    throw new Error("Native PDF selection could not be verified.");
  }
  return result;
}

export async function applyPdfRangeCapture(preview: PdfRangePreview, startPage: number, endPage: number): Promise<ContextSelectionSummary> {
  if (!isDesktopApp()) throw new Error("Native PDF selection is unavailable.");
  return invoke<ContextSelectionSummary>("apply_pdf_range_capture", { selectionToken: preview.selectionToken, draftId: preview.draftId, contextRevision: preview.contextRevision, startPage, endPage });
}

export async function discardPdfRangeCapture(selectionToken: string): Promise<void> {
  if (!isDesktopApp()) return;
  await invoke<boolean>("discard_pdf_range_capture", { selectionToken });
}

export async function listRecentRuns(): Promise<RecentRunSummary[]> {
  if (!isDesktopApp()) return [];
  return invoke<RecentRunSummary[]>("list_recent_runs");
}

export async function listAcpAdapters(): Promise<Array<{ id: string; displayName: string; state: "supported" | "blocked"; reason?: string }>> {
  if (!isDesktopApp()) return [];
  return invoke("list_acp_adapters");
}

export async function listProviderProfiles(): Promise<ProviderProfileSummary[]> {
  if (!isDesktopApp()) return [];
  return invoke<ProviderProfileSummary[]>("list_provider_profiles");
}

export async function listProviderSourceScopes(profileId: string): Promise<ProviderSourceScope[]> {
  if (!isDesktopApp()) return [];
  return invoke<ProviderSourceScope[]>("list_provider_source_scopes", { profileId });
}

export async function pickProviderSourceDirectory(): Promise<string | null> {
  if (!isDesktopApp()) return null;
  return invoke<string | null>("pick_provider_source_directory");
}

export async function addProviderSourceScope(profileId: string, pathInput: string): Promise<ProviderSourceScope> {
  if (!isDesktopApp()) throw new Error("Provider source permissions are available only in the desktop app.");
  return invoke<ProviderSourceScope>("add_provider_source_scope", { profileId, pathInput });
}

export async function revokeProviderSourceScope(profileId: string, grantId: string): Promise<boolean> {
  if (!isDesktopApp()) throw new Error("Provider source permissions are available only in the desktop app.");
  return invoke<boolean>("revoke_provider_source_scope", { profileId, grantId });
}

export async function loadActiveProviderProfileSelection(providerId: string): Promise<ActiveProviderProfileSelection | null> {
  if (!isDesktopApp()) return null;
  return invoke("load_active_provider_profile_selection", { providerId });
}

export async function saveProviderProfile(draft: ProviderProfileDraft): Promise<ProviderProfileSummary> {
  if (!isDesktopApp()) throw new Error("Provider profiles are available only in the desktop app.");
  return invoke("save_provider_profile", { draft });
}

export async function setActiveProviderProfile(
  providerId: string,
  profileId: string,
  expectedSelectionRevision: number | null,
): Promise<ActiveProviderProfileSelection> {
  if (!isDesktopApp()) throw new Error("Provider profiles are available only in the desktop app.");
  return invoke("set_active_provider_profile", { providerId, profileId, expectedSelectionRevision });
}

export async function validateProviderProfile(profileId: string): Promise<ProviderAdmission> {
  if (!isDesktopApp()) {
    return {
      state: "blocked",
      profileId,
      profileRevision: 0,
      adapterId: "codex-acp",
      rootBinding: "unverified",
      reason: "other",
      checkedAt: new Date().toISOString(),
    };
  }
  return invoke("validate_provider_profile", { profileId });
}

export async function authenticateProviderProfile(profileId: string, expectedRevision: number): Promise<AuthProfileResult> {
  if (!isDesktopApp()) {
    return {
      providerId: "codex-acp",
      profileId,
      profileRevision: expectedRevision,
      state: "unsupported",
      checkedAt: new Date().toISOString(),
    };
  }
  return invoke("authenticate_provider_profile", { profileId, expectedRevision });
}

export async function cancelProviderAuthentication(profileId: string, expectedRevision: number): Promise<boolean> {
  if (!isDesktopApp()) return false;
  return invoke("cancel_provider_authentication", { profileId, expectedRevision });
}

export async function listenProviderAuthProgress(
  onProgress: (progress: ProviderAuthProgress) => void,
): Promise<UnlistenFn> {
  if (!isDesktopApp()) return () => undefined;
  return listen<ProviderAuthProgress>("magi:provider-auth-progress", ({ payload }) => onProgress(payload));
}

export async function refreshProviderModelCatalog(profileId: string, expectedRevision: number): Promise<ProviderCatalogSnapshot> {
  if (!isDesktopApp()) throw new Error("Provider catalogs are available only in the desktop app.");
  return invoke("refresh_provider_model_catalog", { profileId, expectedRevision });
}

export type CoreId = "MELCHIOR-1" | "BALTHASAR-2" | "CASPER-3";
export type ProviderModelSelection = { binding: AcpModelBindingSnapshot; selectionRevision: number; updatedAt: string };
export type ProviderCatalogState = { providerProfileId: string; profileRevision: number; catalog: ProviderCatalogSnapshot | null; modelSelection: ProviderModelSelection | null; modelSelectionRevision: number | null; selectionState: "selected" | "unselected" | "stale" };
export type CoreModelSelection = { coreId: CoreId; providerProfileId: string; profileRevision: number; modelSelectionRevision: number; selectionRevision: number; updatedAt: string };
export type CoreModelSelectionState = { coreId: CoreId; selection: CoreModelSelection | null; selectionRevision: number | null; selectionState: "selected" | "unselected" | "stale" };
export type CoreBindingReference = { coreId: CoreId; providerProfileId: string; profileRevision: number; modelSelectionRevision: number; coreSelectionRevision: number };
export type CatalogExecutionWitness = { binding: AcpModelBindingSnapshot; originalCatalog: ProviderCatalogSnapshot; freshCatalog: ProviderCatalogSnapshot };
export type CoreExecutionWitnesses = { schemaVersion: 1; cores: Array<{ coreBindingReference: CoreBindingReference; catalogExecutionWitness: CatalogExecutionWitness }> };
const executionCoreOrder: CoreId[] = ["MELCHIOR-1", "BALTHASAR-2", "CASPER-3"];
const exactKeys = (value: unknown, keys: string[]): value is Record<string, unknown> => value !== null && typeof value === "object" && !Array.isArray(value)
  && Object.keys(value).length === keys.length && keys.every(key => Object.hasOwn(value, key));
const witnessText = (value: unknown): value is string => typeof value === "string" && value.trim().length > 0 && !/[\u0000-\u001f\u007f]/.test(value);
const witnessRevision = (value: unknown): value is number => typeof value === "number" && Number.isSafeInteger(value) && value >= 0;
const witnessDigest = (value: unknown): value is string => typeof value === "string" && /^[a-f0-9]{64}$/.test(value);
const witnessNullableText = (value: unknown) => value === null || typeof value === "string";
const witnessCommonKeys = ["schemaVersion", "catalogSnapshotId", "catalogDigest", "providerId", "acpMode", "providerProfileId", "profileRevision", "adapterId", "adapterVersion", "adapterDigest", "artifactSetDigest"];
function witnessCommon(value: Record<string, unknown>): boolean {
  return value.schemaVersion === 3 && value.acpMode === "acp" && witnessRevision(value.profileRevision)
    && ["catalogSnapshotId", "providerId", "providerProfileId", "adapterId", "adapterVersion"].every(key => witnessText(value[key]))
    && ["catalogDigest", "adapterDigest", "artifactSetDigest"].every(key => witnessDigest(value[key]));
}
function validWitnessCatalog(value: unknown): value is ProviderCatalogSnapshot {
  if (!exactKeys(value, [...witnessCommonKeys, "fetchedAt", "models", "negotiatedModes"]) || !witnessCommon(value)
    || typeof value.fetchedAt !== "string" || !Number.isFinite(Date.parse(value.fetchedAt)) || !Array.isArray(value.models) || value.models.length === 0
    || !exactKeys(value.negotiatedModes, ["currentModeId", "modes"]) || !Array.isArray(value.negotiatedModes.modes)) return false;
  if (!value.models.every(model => exactKeys(model, ["modelId", "name", "description", "contextWindowTokens", "maxOutputTokens"])
    && witnessText(model.modelId) && witnessNullableText(model.name) && witnessNullableText(model.description)
    && [model.contextWindowTokens, model.maxOutputTokens].every(limit => limit === null || witnessRevision(limit) && limit > 0))) return false;
  if (new Set(value.models.map(model => model.modelId)).size !== value.models.length) return false;
  const modes = value.negotiatedModes;
  const items = modes.modes;
  if (!Array.isArray(items)) return false;
  return items.length <= 128 && items.every(mode => exactKeys(mode, ["modeId", "name", "description"])
    && witnessText(mode.modeId) && witnessText(mode.name) && witnessNullableText(mode.description))
    && new Set(items.map(mode => mode.modeId)).size === items.length
    && (modes.currentModeId === null || witnessText(modes.currentModeId) && items.some(mode => mode.modeId === modes.currentModeId));
}
function validWitnessBinding(value: unknown): value is AcpModelBindingSnapshot {
  if (value === null || typeof value !== "object" || Array.isArray(value)) return false;
  const record = value as Record<string, unknown>;
  return exactKeys(record, [...witnessCommonKeys, "modelId", "bindingDigest", ...(Object.hasOwn(record, "modeId") ? ["modeId"] : [])])
    && witnessCommon(record) && witnessText(record.modelId) && witnessDigest(record.bindingDigest)
    && (record.modeId === undefined || record.modeId === null || witnessText(record.modeId));
}
export async function loadCoreExecutionWitnesses(coreBindings: CoreBindingReference[]): Promise<CoreExecutionWitnesses> {
  const result: unknown = await invoke("load_core_execution_witnesses", { input: { coreBindings } });
  if (!exactKeys(result, ["schemaVersion", "cores"]) || result.schemaVersion !== 1 || !Array.isArray(result.cores) || result.cores.length !== 3) throw new Error("Invalid execution witness");
  for (const [index, entry] of result.cores.entries()) {
    if (!exactKeys(entry, ["coreBindingReference", "catalogExecutionWitness"])) throw new Error("Invalid execution witness");
    const reference = entry.coreBindingReference;
    const expected = coreBindings.find(ref => ref.coreId === executionCoreOrder[index]);
    if (!exactKeys(reference, ["coreId", "providerProfileId", "profileRevision", "modelSelectionRevision", "coreSelectionRevision"]) || !expected || !witnessText(reference.providerProfileId) || ![reference.profileRevision, reference.modelSelectionRevision, reference.coreSelectionRevision].every(witnessRevision)
      || Object.entries(expected).some(([key, value]) => reference[key] !== value)) throw new Error("Invalid execution witness reference");
    const witness = entry.catalogExecutionWitness;
    if (!exactKeys(witness, ["binding", "originalCatalog", "freshCatalog"])) throw new Error("Invalid execution witness");
    if (!validWitnessCatalog(witness.originalCatalog) || !validWitnessCatalog(witness.freshCatalog) || !validWitnessBinding(witness.binding)) throw new Error("Invalid execution witness fields");
    const original = witness.originalCatalog;
    const fresh = witness.freshCatalog;
    const binding = witness.binding;
    if (!catalogsHaveEquivalentExecutionAuthority(original, fresh) || !binding || binding.schemaVersion !== 3
      || original.providerProfileId !== expected.providerProfileId || original.profileRevision !== expected.profileRevision
      || binding.providerProfileId !== expected.providerProfileId || binding.profileRevision !== expected.profileRevision
      || binding.catalogSnapshotId !== original.catalogSnapshotId || binding.catalogDigest !== original.catalogDigest
      || binding.artifactSetDigest !== original.artifactSetDigest || binding.adapterId !== original.adapterId
      || binding.adapterDigest !== original.adapterDigest || binding.adapterVersion !== original.adapterVersion
      || binding.providerId !== original.providerId || binding.acpMode !== original.acpMode
      || !original.models.some(model => model.modelId === binding.modelId)
      || (original.negotiatedModes!.modes.length ? !original.negotiatedModes!.modes.some(mode => mode.modeId === binding.modeId) : binding.modeId != null)) throw new Error("Invalid execution witness authority");
  }
  return result as CoreExecutionWitnesses;
}
export async function loadProviderCatalog(profileId: string, expectedRevision: number): Promise<ProviderCatalogState> {
  if (!isDesktopApp()) throw new Error("Desktop connection required");
  return invoke("load_provider_catalog", { profileId, expectedRevision });
}
export async function refreshProviderCatalog(profileId: string, expectedRevision: number): Promise<ProviderCatalogState> {
  if (!isDesktopApp()) throw new Error("Desktop connection required");
  return invoke("refresh_provider_catalog", { profileId, expectedRevision });
}
export async function selectProviderModel(input: { providerProfileId: string; profileRevision: number; catalogSnapshotId: string; catalogDigest: string; modelId: string; modeId: string | null; expectedSelectionRevision: number | null }): Promise<ProviderModelSelection> {
  if (!isDesktopApp()) throw new Error("Desktop connection required");
  return invoke("select_provider_model", { input });
}
export async function loadCoreModelSelections(): Promise<CoreModelSelectionState[]> {
  if (!isDesktopApp()) throw new Error("Desktop connection required");
  return invoke("load_core_model_selections");
}
export async function selectCoreModel(input: { coreId: CoreId; providerProfileId: string; profileRevision: number; modelSelectionRevision: number; expectedSelectionRevision: number | null }): Promise<CoreModelSelection> {
  if (!isDesktopApp()) throw new Error("Desktop connection required");
  return invoke("select_core_model", { input });
}

export type StartLiveRunInput = {
  commandId: string;
  idempotencyKey: string;
  question: string;
  modelBinding: AcpModelBindingInput;
};

export async function startLiveRun(input: StartLiveRunInput): Promise<LiveRunReceipt> {
  if (!isDesktopApp()) throw new Error("Live provider requests are available only in the desktop app.");
  return invoke("start_live_run", input);
}

export async function getLiveRunSnapshot(runId: string, afterSequence: number): Promise<LiveRunSnapshot> {
  if (!isDesktopApp()) throw new Error("Live run records are available only in the desktop app.");
  return invoke("get_live_run_snapshot", { runId, afterSequence });
}

export async function cancelLiveRun(input: CancelLiveRunInput): Promise<LiveRunCancellationReceipt> {
  if (!isDesktopApp()) throw new Error("Live provider requests are available only in the desktop app.");
  return invoke("cancel_live_run", input);
}

function isLiveRunRemediationCategory(value: unknown): value is LiveRunRemediationCategory {
  return value === "reauthenticate" || value === "refresh_catalog" || value === "review_request" || value === "retry_later";
}

export function normalizeLiveRunError(error: unknown): LiveRunError {
  if (error && typeof error === "object") {
    const candidate = error as Partial<LiveRunError>;
    if (typeof candidate.code === "string" && typeof candidate.message === "string" && typeof candidate.retryable === "boolean") {
      return {
        code: candidate.code,
        profileBinding: authenticationFailureBinding(candidate),
        message: candidate.message,
        remediationCategory: isLiveRunRemediationCategory(candidate.remediationCategory) ? candidate.remediationCategory : undefined,
        retryable: candidate.retryable,
        capacity: typeof candidate.capacity === "number" ? candidate.capacity : undefined,
        admittedCount: typeof candidate.admittedCount === "number" ? candidate.admittedCount : undefined,
      };
    }
  }
  return {
    code: "request_failed",
    message: "요청 결과를 확인하지 못했습니다. 저장된 실행 상태를 다시 확인하십시오.",
    retryable: true,
  };
}

export type StartDeliberationInput = {
  commandId: string;
  idempotencyKey: string;
  question: string;
  contextDraftId: string | null;
  contextRevision: number | null;
  coreBindings: CoreBindingReference[];
  rolePresetId: string;
  roleRevision: number;
  disclosureConfirmed: true;
};

export type AdmissionAuthority = { token: string; processEpoch: string };
export type IssuedAdmissionAuthority = AdmissionAuthority & { expiresAt: string };
export type RegisterDeliberationReceipt =
  | { kind: "registered"; admissionAuthority: IssuedAdmissionAuthority }
  | { kind: "replayed"; receipt: { runId: string } };
export type CancelDeliberationRequestInput = {
  request: StartDeliberationInput;
  commandId: string;
  idempotencyKey: string;
  admissionAuthority?: AdmissionAuthority;
};
export type CancelDeliberationRequestReceipt = {
  requestCancellation: {
    schemaVersion: 1;
    commandId: string;
    idempotencyKey: string;
    request: { commandId: string; idempotencyKey: string; intentDigest: string };
    acceptedAt: string;
    admittedRunId: string | null;
  };
  runCancellation: LiveRunCancellationReceipt | null;
};

export async function registerDeliberationRequest(input: StartDeliberationInput): Promise<RegisterDeliberationReceipt> {
  if (!isDesktopApp()) throw new Error("Deliberation is available only in the desktop app.");
  const receipt = await invoke<RegisterDeliberationReceipt>("register_deliberation_request", { input });
  return validateRegistrationReceipt(receipt);
}

function validateRegistrationReceipt(receipt: RegisterDeliberationReceipt): RegisterDeliberationReceipt {
  const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
  if (receipt?.kind === "replayed" && typeof receipt.receipt?.runId === "string" && receipt.receipt.runId.length > 0) return receipt;
  if (receipt?.kind === "registered" && uuid.test(receipt.admissionAuthority?.token ?? "")
    && uuid.test(receipt.admissionAuthority?.processEpoch ?? "")
    && typeof receipt.admissionAuthority?.expiresAt === "string" && Number.isFinite(Date.parse(receipt.admissionAuthority.expiresAt))) return receipt;
  throw new Error("The native registration response could not be verified.");
}

export type ClarificationStartInput = { parent: ClarificationParent; contextDraftId: string; contextDraftRevision: number; request: StartDeliberationInput };
export async function registerClarificationRequest(input: ClarificationStartInput): Promise<RegisterDeliberationReceipt> {
  return validateRegistrationReceipt(await invoke("register_clarification_request", { input }));
}
export async function startClarification(input: ClarificationStartInput, admissionAuthority: AdmissionAuthority): Promise<{ runId: string }> {
  return invoke("start_clarification", { input: { ...input, request: { ...input.request, admissionAuthority } } });
}
export async function cancelClarificationRequest(input: { request: ClarificationStartInput; commandId: string; idempotencyKey: string; admissionAuthority?: AdmissionAuthority }): Promise<CancelDeliberationRequestReceipt> {
  return invoke("cancel_clarification_request", { input });
}

export async function cancelDeliberationRequest(input: CancelDeliberationRequestInput): Promise<CancelDeliberationRequestReceipt> {
  if (!isDesktopApp()) throw new Error("Deliberation is available only in the desktop app.");
  return invoke("cancel_deliberation_request", { input });
}

export async function startDeliberation(input: StartDeliberationInput & { admissionAuthority: AdmissionAuthority }): Promise<{ runId: string }> {
  if (!isDesktopApp()) throw new Error("Deliberation is available only in the desktop app.");
  return invoke("start_deliberation", { input });
}

export async function cancelDeliberation(runId: string): Promise<void> {
  if (!isDesktopApp()) throw new Error("Deliberation is available only in the desktop app.");
  await invoke("cancel_deliberation", { runId });
}

export type RunClarification = {
  schemaVersion: 1;
  runId: string;
  runRevision: number;
  inputDigest: string;
  runGeneration: number;
  reason: "needs_input";
  resumeStage: "independent_review" | "cross_review" | "synthesis" | "balloting";
  gaps: Array<{
    attemptId: string;
    coreId: "MELCHIOR-1" | "BALTHASAR-2" | "CASPER-3";
    stage: "independent_review" | "cross_review";
    gapIndex: number;
    missingInformation: string;
    impact: string;
    essential: boolean;
  }>;
};

export async function loadRunClarification(runId: string): Promise<RunClarification | null> {
  if (!isDesktopApp()) throw new Error("Clarification is available only in the desktop app.");
  const result: unknown = await invoke("load_run_clarification", { runId });
  if (result === null) return null;
  const closed = (value: unknown, keys: string[]): value is Record<string, unknown> => typeof value === "object" && value !== null && !Array.isArray(value) && Object.keys(value).length === keys.length && keys.every(key => Object.hasOwn(value, key));
  const integer = (value: unknown) => Number.isSafeInteger(value) && typeof value === "number" && value >= 0;
  const text = (value: unknown): value is string => typeof value === "string" && value.trim().length > 0 && value.length <= 262144;
  const invalid = () => new Error("The saved clarification could not be verified.");
  if (!closed(result, ["schemaVersion", "runId", "runRevision", "inputDigest", "runGeneration", "reason", "resumeStage", "gaps"])
    || result.schemaVersion !== 1 || result.runId !== runId || !text(result.runId)
    || !integer(result.runRevision) || !integer(result.runGeneration)
    || typeof result.inputDigest !== "string" || !/^[a-f0-9]{64}$/.test(result.inputDigest)
    || result.reason !== "needs_input" || !["independent_review", "cross_review", "synthesis", "balloting"].includes(String(result.resumeStage))
    || !Array.isArray(result.gaps) || result.gaps.length === 0 || result.gaps.length > 4096) throw invalid();
  const identities = new Set<string>();
  for (const gap of result.gaps) {
    if (!closed(gap, ["attemptId", "coreId", "stage", "gapIndex", "missingInformation", "impact", "essential"])
      || !text(gap.attemptId) || !["MELCHIOR-1", "BALTHASAR-2", "CASPER-3"].includes(String(gap.coreId))
      || !["independent_review", "cross_review"].includes(String(gap.stage)) || !integer(gap.gapIndex)
      || !text(gap.missingInformation) || !text(gap.impact) || typeof gap.essential !== "boolean") throw invalid();
    const identity = JSON.stringify([gap.attemptId, gap.coreId, gap.stage, gap.gapIndex]);
    if (identities.has(identity)) throw invalid();
    identities.add(identity);
  }
  if (!result.gaps.some(gap => gap.essential === true)) throw invalid();
  return result as RunClarification;
}

export type ClarificationParent = { runId: string; revision: number; inputDigest: string; generation: number };
export type ClarificationDraft = { schemaVersion: 1; parent: ClarificationParent; question: string; context: ContextSelectionSummary };

function validateClarificationDraft(value: unknown, parent?: ClarificationParent, draftId?: string): ClarificationDraft {
  const closed = (value: unknown, keys: string[]): value is Record<string, unknown> => typeof value === "object" && value !== null && !Array.isArray(value) && Object.keys(value).length === keys.length && keys.every(key => Object.hasOwn(value, key));
  const integer = (value: unknown) => typeof value === "number" && Number.isSafeInteger(value) && value >= 0;
  const id = (value: unknown): value is string => typeof value === "string" && value.trim().length > 0 && value.length <= 4096;
  const digest = (value: unknown) => typeof value === "string" && /^[a-f0-9]{64}$/.test(value);
  const invalid = () => new Error("The clarification draft could not be verified.");
  if (!closed(value, ["schemaVersion", "parent", "question", "context"]) || value.schemaVersion !== 1 || typeof value.question !== "string" || value.question.length > 262144
    || !closed(value.parent, ["runId", "revision", "inputDigest", "generation"]) || !id(value.parent.runId) || !integer(value.parent.revision) || !integer(value.parent.generation) || !digest(value.parent.inputDigest)
    || !closed(value.context, ["draftId", "manifestId", "manifestDigest", "revision", "sources"]) || !id(value.context.draftId) || !id(value.context.manifestId) || !digest(value.context.manifestDigest) || !integer(value.context.revision) || !Array.isArray(value.context.sources) || value.context.sources.length > 1000) throw invalid();
  const ids = new Set<string>();
  for (const source of value.context.sources) {
    if (typeof source !== "object" || source === null || Array.isArray(source)
      || Object.keys(source).some(key => !["sourceId", "displayName", "status", "byteLength", "representation", "digest", "issueCodes"].includes(key))
      || !id(source.sourceId) || !id(source.displayName) || !["captured", "excluded", "failed"].includes(source.status) || !integer(source.byteLength)
      || !["utf8_text", "pdf_text", "pdf_raster", "image", "unsupported", "unknown"].includes(source.representation)
      || (source.digest !== undefined && !digest(source.digest)) || !Array.isArray(source.issueCodes) || !source.issueCodes.every(id) || ids.has(source.sourceId)) throw invalid();
    ids.add(source.sourceId);
  }
  const result = value as ClarificationDraft;
  if (parent && (result.parent.runId !== parent.runId || result.parent.revision !== parent.revision || result.parent.inputDigest !== parent.inputDigest || result.parent.generation !== parent.generation)) throw invalid();
  if (draftId && result.context.draftId !== draftId) throw invalid();
  return result;
}

export async function createClarificationDraft(parent: ClarificationParent): Promise<ClarificationDraft> {
  return validateClarificationDraft(await invoke("create_clarification_draft", { input: { parent } }), parent);
}
export async function listClarificationDrafts(parent: ClarificationParent): Promise<ClarificationDraft[]> {
  const value: unknown = await invoke("list_clarification_drafts", { parent });
  if (!Array.isArray(value) || value.length > 100) throw new Error("The clarification drafts could not be verified.");
  const drafts = value.map(item => validateClarificationDraft(item, parent));
  if (new Set(drafts.map(item => item.context.draftId)).size !== drafts.length) throw new Error("The clarification drafts could not be verified.");
  return drafts;
}
export async function loadClarificationDraft(draftId: string): Promise<ClarificationDraft> {
  return validateClarificationDraft(await invoke("load_clarification_draft", { draftId }), undefined, draftId);
}
export async function saveClarificationQuestion(draft: ClarificationDraft, question: string): Promise<ClarificationDraft> {
  const saved = validateClarificationDraft(await invoke("save_clarification_question", { input: { draftId: draft.context.draftId, expectedRevision: draft.context.revision, question } }), draft.parent, draft.context.draftId);
  if (saved.context.revision !== draft.context.revision + 1 || saved.question !== question) throw new Error("The saved clarification revision could not be verified.");
  return saved;
}
export async function discardClarificationDraft(draft: ClarificationDraft): Promise<void> {
  await invoke("discard_clarification_draft", { input: { draftId: draft.context.draftId, expectedRevision: draft.context.revision } });
}

export async function loadRunDossier(runId: string): Promise<RunDossierView> {
  if (!isDesktopApp()) throw new Error("Run records are available only in the desktop app.");
  return invoke("load_run_dossier", { runId });
}

export async function listRolePresets(): Promise<StoredRolePreset[]> {
  if (!isDesktopApp()) return [];
  return invoke("list_role_presets");
}

export async function loadActiveRolePresetSelection(): Promise<ActiveRolePresetSelection | null> {
  if (!isDesktopApp()) return null;
  return invoke("load_active_role_preset_selection");
}

export async function setActiveRolePreset(
  presetId: string,
  expectedSelectionRevision: number,
): Promise<ActiveRolePresetSelection> {
  if (!isDesktopApp()) throw new Error("Role profiles are available only in the desktop app.");
  return invoke("set_active_role_preset", { presetId, expectedSelectionRevision });
}

export async function cloneRolePreset(sourcePresetId: string, sourceRevision: number, displayName: string): Promise<StoredRolePreset> {
  if (!isDesktopApp()) throw new Error("Role profiles are available only in the desktop app.");
  return invoke("clone_role_preset", { sourcePresetId, sourceRevision, displayName });
}

export async function saveRolePreset(draft: RolePresetDraftInput): Promise<StoredRolePreset> {
  if (!isDesktopApp()) throw new Error("Role profiles are available only in the desktop app.");
  return invoke("save_role_preset", { draft });
}

export async function getConsolePreferences(): Promise<ConsolePreferences | null> {
  if (!isDesktopApp()) return null;
  return invoke<ConsolePreferences | null>("get_console_preferences");
}

export async function saveConsolePreferences(preferences: ConsolePreferences): Promise<void> {
  if (!isDesktopApp()) return;
  await invoke("save_console_preferences", { preferences });
}

export async function listenConsoleSnapshotChanged(
  onSnapshot: (snapshot: ConsoleSnapshot) => void,
): Promise<UnlistenFn> {
  if (!isDesktopApp()) return () => undefined;
  return listen<ConsoleSnapshot>("console_snapshot_changed", ({ payload }) => {
    onSnapshot(payload);
  });
}

export async function listenRunUpdate(
  onUpdate: (update: RunUpdate) => void,
): Promise<UnlistenFn> {
  if (!isDesktopApp()) return () => undefined;
  return listen<RunUpdate>("magi:run-update", ({ payload }) => onUpdate(payload));
}

export async function listenLiveRunChanged(
  onChange: (change: LiveRunChanged) => void,
): Promise<UnlistenFn> {
  if (!isDesktopApp()) return () => undefined;
  return listen<LiveRunChanged>("magi:live-run-changed", ({ payload }) => onChange(payload));
}

export async function listenShellEvent(
  eventName: ShellEventName,
  callback: () => void,
): Promise<UnlistenFn> {
  if (!isDesktopApp()) return () => undefined;
  return listen(eventName, callback);
}

export async function getShellContext(): Promise<ShellContext | null> {
  if (!isDesktopApp()) return null;
  return invoke<ShellContext>("shell_context");
}

export async function openConsoleFromCompanion(): Promise<void> {
  if (!isDesktopApp()) return;
  await invoke("shell_open_console");
}

export async function openSettingsFromCompanion(): Promise<void> {
  if (!isDesktopApp()) return;
  await invoke("shell_open_settings");
}

export async function closeCompanion(): Promise<void> {
  if (!isDesktopApp()) return;
  await invoke("shell_close_companion");
}

export async function requestApplicationExit(): Promise<void> {
  if (!isDesktopApp()) return;
  await invoke("shell_request_exit");
}

export async function confirmApplicationExit(): Promise<void> {
  if (!isDesktopApp()) return;
  await invoke("shell_confirm_exit");
}

export type RecordHistoryCursor = { store_id: string; store_generation: number; membership_generation: number; filter_digest: string; last_created_at: string; last_run_id: string };
export type RecordEventCursor = { store_id: string; store_generation: number; run_id: string; after_sequence: number; high_water_sequence: number };
export type StoredRecordSummary = { runId: string; conversationId: string; question: string; status: string; revision: number; createdAt: string; updatedAt: string };
export type RecordHistoryPage = { items: StoredRecordSummary[]; nextCursor: RecordHistoryCursor | null };
export type RecordReplayEvent = { sequence: number; createdAt: string; kind: string; phase?: string; assessmentId?: string; coreId?: string; votes?: RunDossierView["votes"]; outcome?: RunDossierView["outcome"] };
export type RecordReplayPage = { events: RecordReplayEvent[]; nextCursor: RecordEventCursor; complete: boolean };
export type RecordDeletionReceipt = { runId: string; deleted: boolean; collectedObjects: number };

export type RecordEvidenceLocator = { source_id: string; object_digest: string; start_line: number | null; end_line: number | null; total_lines: number | null; page?: number; width?: number; height?: number };
export type RecordEvidenceSource = { sourceId: string; displayName: string; representation: "utf8_text" | "pdf_text" | "pdf_raster" | "image"; byteLength: number; capturedAtEpochMs: number | null; locators: RecordEvidenceLocator[]; freshness: "unchanged" | "changed" | "missing" | "unreadable" | "unchecked"; availability: "not_checked" };
export type RecordEvidenceList = { runId: string; revision: number; sources: RecordEvidenceSource[] };
export type RecordEvidenceView = { sourceId: string; objectDigest: string; startLine: number; endLine: number; totalLines: number; text: string | null; evidenceUnavailable: boolean; freshness: RecordEvidenceSource["freshness"]; representationKind: RecordEvidenceSource["representation"] | null; page: number | null; width: number | null; height: number | null; dataUrl: string | null; warnings: string[] };

export async function listRecordEvidence(runId: string): Promise<RecordEvidenceList> {
  if (!isDesktopApp()) throw new Error("records_unavailable");
  return invoke("list_record_evidence", { runId });
}

export async function loadRecordEvidence(runId: string, locator: RecordEvidenceLocator): Promise<RecordEvidenceView> {
  if (!isDesktopApp()) throw new Error("records_unavailable");
  return invoke("load_evidence", { runId, locator });
}

export async function listRecords(query: string, status: string, cursor: RecordHistoryCursor | null = null): Promise<RecordHistoryPage> {
  if (!isDesktopApp()) throw new Error("records_unavailable");
  return invoke("list_records", { request: { filter: { question_query: query.trim() || null, statuses: status ? [status] : [], created_after: null, created_before: null }, cursor, page_size: 30 } });
}

export async function loadRecordReplay(runId: string, cursor: RecordEventCursor | null = null): Promise<RecordReplayPage> {
  if (!isDesktopApp()) throw new Error("records_unavailable");
  return invoke("load_record_replay", { runId, cursor, limit: 100 });
}

export async function deleteRecord(runId: string, expectedRevision: number): Promise<RecordDeletionReceipt> {
  if (!isDesktopApp()) throw new Error("records_unavailable");
  return invoke("delete_record", { runId, expectedRevision });
}

export type ExternalReplaySummary = { externalReplayId: string; fileDigest: string; schemaVersion: number; importedAtEpochMs: number; title: string; question: string; external: boolean };
export type ExternalReplay = Omit<ExternalReplaySummary, "title" | "question"> & { data: { title: string; question: string; attribution: string; proposal: { body: string }; ballots: Array<{ core_id: string; vote: BallotChoice; rationale: string }>; events: Array<{ sequence: number; offset_ms: number; type: string; payload: Record<string, string> }> } };
export type LocalBackupResult = { format: string; schemaVersion: number; storeId: string; generation: number; eventHighWater: number; databaseDigest: string; objects: Array<{ digest: string; byte_length: number }>; containsPrivateSources: boolean };

export async function importSharedReplay(): Promise<ExternalReplay | null> {
  if (!isDesktopApp()) throw new Error("records_unavailable");
  return invoke("import_shared_replay");
}
export async function loadExternalReplay(externalReplayId: string): Promise<ExternalReplay> {
  if (!isDesktopApp()) throw new Error("records_unavailable");
  return invoke("load_external_replay", { externalReplayId });
}
export async function listExternalReplays(before: ExternalReplaySummary | null = null): Promise<ExternalReplaySummary[]> {
  if (!isDesktopApp()) throw new Error("records_unavailable");
  return invoke("list_external_replays", { beforeImportedAtEpochMs: before?.importedAtEpochMs ?? null, beforeExternalReplayId: before?.externalReplayId ?? null, limit: 30 });
}
export async function createLocalBackup(): Promise<LocalBackupResult | null> {
  if (!isDesktopApp()) throw new Error("records_unavailable");
  return invoke("create_local_backup");
}
