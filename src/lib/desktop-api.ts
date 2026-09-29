import { isTauri, invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type ConnectionState = "unknown" | "ready" | "blocked";
export type StorageState = "unknown" | "ready" | "error";
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
  error?: { code: string; message: string };
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
  activeRun?: ConsoleRunSummary;
  confirmedAt?: string;
  eventSequence?: number;
};

export type CapturedSourceSummary = {
  sourceId: string;
  displayName: string;
  status: "captured" | "excluded" | "failed";
  byteLength: number;
  representation: "utf8_text" | "image" | "unsupported" | "unknown";
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

export type ProviderProfileSummary = {
  providerProfileId: string;
  revision: number;
  providerId: string;
  displayName: string;
  accountAlias: string;
  authenticationMethod: "local_subscription" | "byok_api";
  credentialConfigured: boolean;
  digest: string;
  updatedAt: string;
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
};

export type ProviderAdmission =
  | { state: "ready"; profileId: string; adapterId: string; rootBinding: "verified"; authentication: "authenticated"; checkedAt: string; modelId?: string }
  | { state: "needs_auth"; profileId: string; adapterId: string; rootBinding: "verified"; authentication: "required"; checkedAt: string }
  | { state: "blocked"; reason: "adapter_unsupported" | "home_override_unsupported" | "home_mismatch" | "attestation_missing" | "other"; checkedAt: string }
  | { state: "not_checked" };

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

export type ShellEventName = "magi:open-settings" | "magi:exit-requested";

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
  if (!isDesktopApp()) return { state: "blocked", reason: "other", checkedAt: new Date().toISOString() };
  return invoke("validate_provider_profile", { profileId });
}

export async function authenticateProviderProfile(profileId: string): Promise<ProviderAdmission> {
  if (!isDesktopApp()) return { state: "blocked", reason: "other", checkedAt: new Date().toISOString() };
  return invoke("authenticate_provider_profile", { profileId });
}

export type StartDeliberationInput = {
  question: string;
  contextDraftId: string | null;
  contextRevision: number | null;
  providerProfileId: string;
  rolePresetId: string;
  roleRevision: number;
  disclosureConfirmed: true;
};

export async function startDeliberation(input: StartDeliberationInput): Promise<{ runId: string }> {
  if (!isDesktopApp()) throw new Error("Deliberation is available only in the desktop app.");
  return invoke("start_deliberation", input);
}

export async function cancelDeliberation(runId: string): Promise<void> {
  if (!isDesktopApp()) throw new Error("Deliberation is available only in the desktop app.");
  await invoke("cancel_deliberation", { runId });
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
