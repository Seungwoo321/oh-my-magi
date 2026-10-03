import { DossierPublicationFence } from "./lib/run-sync";
import { t, setLocale, useLocale, type Locale } from "./lib/locale";
import type { EvidenceReadingTarget } from "./records";
import { useCoreBindings } from "./core-bindings";
import { useCallback, useEffect, useRef, useState } from "react";
import {
  getConsoleSnapshot,
  getShellContext,
  listenConsoleSnapshotChanged,
  listenShellEvent,
  openConsoleFromCompanion,
  openSettingsFromCompanion,
  closeCompanion,
  confirmApplicationExit,
  listAcpAdapters,
  listProviderProfiles,
  listProviderSourceScopes,
  pickProviderSourceDirectory,
  addProviderSourceScope,
  revokeProviderSourceScope,
  loadActiveProviderProfileSelection,
  saveProviderProfile,
  setActiveProviderProfile,
  validateProviderProfile,
  authenticateProviderProfile,
  cancelProviderAuthentication,
  listenProviderAuthProgress,
  refreshProviderCatalog,
  startLiveRun,
  cancelLiveRun,
  getLiveRunSnapshot,
  listenLiveRunChanged,
  normalizeLiveRunError,
  authenticationFailureBinding,
  startClarification,
  registerClarificationRequest,
  cancelClarificationRequest,
  startDeliberation,
  registerDeliberationRequest,
  cancelDeliberationRequest,
  cancelDeliberation,
  loadRunDossier,
  loadRunCoreDispatches,
  validCoreDispatchProjection,
  type CoreDispatchProjection,
  listenRunUpdate,
  listenCoreDispatches,
  listRolePresets,
  loadActiveRolePresetSelection,
  normalizeRoleStoreDiagnostic,
  setActiveRolePreset,
  cloneRolePreset,
  saveRolePreset,
  listRecentRuns,
  isDesktopApp,
  catalogHasExecutionAuthority,
  selectContextFiles,
  selectContextDirectory,
  getConsolePreferences,
  saveConsolePreferences,
  type ConsolePreferences,
  type PreferenceField,
  type SettingsSnapshot,
  type SettingsCommand,
  type SettingsReceipt,
  type ConsoleSnapshot,
  type ActiveRolePresetSelection,
  type ContextSelectionSummary,
  type ProviderProfileSummary,
  type ProviderSourceScope,
  type ProviderAdmission,
  type ProviderValidationFailure,
  type AcpModelBindingInput,
  type ProviderAuthProgress,
  type LiveRunError,
  type LiveRunSnapshot,
  type LiveRunStatus,
  type ProviderCatalogSnapshot,
  type CancelLiveRunInput,
  type StartLiveRunInput,
  type StartDeliberationInput,
  type AdmissionAuthority,
  type CancelDeliberationRequestInput,
  type CancelDeliberationRequestReceipt,
  type ClarificationParent,
  type ClarificationStartInput,
  type ClarificationDraft,
  type RunDossierView,
  type RunUpdate,
  type StoredRolePreset,
  type RoleStoreDiagnostic,
  type RolePresetDraftInput,
  type ShellContext,
} from "./lib/desktop-api";
import {
  CompanionSurface,
  ScreenSurface,
  type AcpAdapterSummary,
  type AcpProfile,
  type AcpProfileDraft,
  type AcpProfileStoreState,
  type AcpProfileWriteState,
  type HomeData,
  type RoleDefinition,
  type RolePreset,
  type RolePresetDraft,
  type RolePresetStoreState,
  type RolePresetWriteState,
  type ScreenId,
} from "./screens";

type MotionSetting = ConsolePreferences["motion"];
type ThemeSetting = ConsolePreferences["theme"];

function defaultConsolePreferences(): ConsolePreferences {
  return {
    motion: window.matchMedia("(prefers-reduced-motion: reduce)").matches ? "reduced" : "full",
    sound: false,
    theme: "command",
    fontScale: 100,
    language: "ko",
    commonContextTokenLimit: 32000,
  };
}

function playSoundPreview() {
  const AudioContextConstructor = window.AudioContext;
  if (!AudioContextConstructor) return;

  const context = new AudioContextConstructor();
  const master = context.createGain();
  const start = context.currentTime;
  master.gain.setValueAtTime(0.035, start);
  master.connect(context.destination);

  [659.25, 880].forEach((frequency, index) => {
    const oscillator = context.createOscillator();
    const envelope = context.createGain();
    const noteStart = start + index * 0.14;
    oscillator.type = "sine";
    oscillator.frequency.setValueAtTime(frequency, noteStart);
    envelope.gain.setValueAtTime(0, noteStart);
    envelope.gain.linearRampToValueAtTime(1, noteStart + 0.01);
    envelope.gain.exponentialRampToValueAtTime(0.001, noteStart + 0.09);
    oscillator.connect(envelope);
    envelope.connect(master);
    oscillator.start(noteStart);
    oscillator.stop(noteStart + 0.1);
  });

  void context.resume().catch(() => undefined);
  window.setTimeout(() => void context.close().catch(() => undefined), 500);
}

const emptySnapshot: ConsoleSnapshot = {
  schemaVersion: 1,
  connection: "unknown",
  storage: "unknown",
};

const liveRunStorageKey = "magi.last-live-run-id";

const runtimeRoutes: Record<string, ScreenId> = {
  preparing: "confirmation",
  awaiting_confirmation: "confirmation",
  independent_review: "independent",
  cross_review: "review",
  synthesis: "proposal",
  balloting: "sealed",
  paused: "paused",
  interrupted: "interrupted",
  cancelling: "cancelling",
  completed: "verdict",
  cancelled: "cancelled",
  failed: "failed",
};

function routeForDossier(dossier: RunDossierView): ScreenId | undefined {
  if (["completed", "failed", "cancelled"].includes(dossier.status)) return runtimeRoutes[dossier.status];
  const terminalRoutes: ScreenId[] = ["verdict", "failed", "cancelled"];
  const statusRoute = runtimeRoutes[dossier.status];
  if (statusRoute && !terminalRoutes.includes(statusRoute)) return statusRoute;
  const stageRoute = runtimeRoutes[dossier.stage];
  return stageRoute && !terminalRoutes.includes(stageRoute) ? stageRoute : undefined;
}

type SafeRunProgress = Pick<RunUpdate, "runId" | "stage" | "coreId" | "state" | "text">;
type LiveCatalogState = "idle" | "loading" | "ready" | "error";
type LiveRequestState = "idle" | "starting" | "accepted" | "error";
type LiveRunTextDelta = { sequence: number; text: string };
type LiveCancelRequestState = { runId: string; state: "sending" | "accepted" | "error"; error?: LiveRunError } | null;

function isLiveRunTerminal(status: LiveRunStatus | undefined): boolean {
  return status === "completed" || status === "failed" || status === "cancelled";
}

function isLiveRunCancelable(status: LiveRunStatus | undefined): boolean {
  return status === "queued" || status === "claimed" || status === "session_creation_intent" || status === "running" || status === "unknown";
}

function mapProviderAdmission(admission: ProviderAdmission): AcpProfile["admission"] {
  if (admission.state === "ready") {
    return {
      state: "admitted",
      profileId: admission.profileId,
      profileRevision: admission.profileRevision,
      adapterId: admission.adapterId,
      rootBinding: admission.rootBinding,
      checkedAt: admission.checkedAt,
    };
  }
  return { state: "blocked", reason: admission.reason, checkedAt: admission.checkedAt };
}

function hasVerifiedProfileHome(profile: AcpProfile | undefined): profile is AcpProfile & {
  admission: Extract<AcpProfile["admission"], { state: "admitted" }>;
} {
  return Boolean(profile
    && profile.admission.state === "admitted"
    && profile.admission.profileId === profile.id
    && profile.admission.profileRevision === profile.revision
    && profile.admission.adapterId === profile.adapterId
    && profile.admission.rootBinding === "verified");
}

type DialogModel = {
  title: string;
  body: string;
  confirm?: { label: string; action: () => void; danger?: boolean };
};

const roleCoreIds: Record<StoredRolePreset["roles"][number]["coreId"], RoleDefinition["core"]> = {
  "MELCHIOR-1": "melchior",
  "BALTHASAR-2": "balthasar",
  "CASPER-3": "casper",
};

function mapStoredRolePreset(stored: StoredRolePreset): RolePreset {
  return {
    id: stored.presetId,
    name: stored.displayName,
    revision: stored.revision,
    kind: stored.source,
    roles: stored.roles.map((role) => ({
      core: roleCoreIds[role.coreId],
      label: role.displayName,
      perspective: role.reviewPurpose,
      criteria: role.evaluationCriteria.join("\n"),
      challengeCondition: role.falsificationQuestions.join("\n"),
      outputLanguage: role.responseLanguage === "ko" || role.responseLanguage === "en" ? role.responseLanguage : "same",
    })),
  };
}

function mapProviderProfile(profile: ProviderProfileSummary): AcpProfile {
  return {
    id: profile.providerProfileId,
    displayName: profile.displayName,
    accountAlias: profile.accountAlias,
    adapterId: profile.providerId,
    revision: profile.revision,
    authenticationMethod: profile.authenticationMethod,
    credentialHome: profile.credentialHome,
    admission: { state: "not_checked" },
  };
}

function roleDraftInput(draft: RolePresetDraft): RolePresetDraftInput {
  const toLines = (value: string) => value.split(/\r?\n/).map((line) => line.trim()).filter(Boolean);
  return {
    presetId: draft.presetId,
    expectedRevision: draft.expectedRevision,
    displayName: draft.name,
    roles: draft.roles.map((role) => ({
      core: role.core,
      label: role.label,
      perspective: role.perspective,
      criteria: toLines(role.criteria),
      challengeCondition: toLines(role.challengeCondition),
      outputLanguage: role.outputLanguage,
    })),
  };
}

function toRolePresetDraft(preset: RolePreset): RolePresetDraft {
  return {
    presetId: preset.id,
    expectedRevision: preset.revision,
    name: preset.name,
    roles: preset.roles.map((role) => ({ ...role })),
  };
}

export default function App() {
  const locale = useLocale();
  const [screen, setScreen] = useState<ScreenId>("home");
  const [question, setQuestion] = useState("");
  const [preferenceDefaults] = useState(defaultConsolePreferences);
  const [motion, setMotion] = useState<MotionSetting>(preferenceDefaults.motion);
  const [sound, setSound] = useState(preferenceDefaults.sound);
  const [theme, setTheme] = useState<ThemeSetting>(preferenceDefaults.theme);
  const [commonContextTokenLimit, setCommonContextTokenLimit] = useState(preferenceDefaults.commonContextTokenLimit);
  const [fontScale, setFontScale] = useState<ConsolePreferences["fontScale"]>(preferenceDefaults.fontScale);
  const [snapshot, setSnapshot] = useState<ConsoleSnapshot>(emptySnapshot);
  const [contextSelection, setContextSelection] = useState<ContextSelectionSummary | null>(null);
  const [homeData, setHomeData] = useState<HomeData>({ recordsState: "loading", recentRuns: [] });
  const [evidenceReadingTarget, setEvidenceReadingTarget] = useState<EvidenceReadingTarget | null>(null);
  const [selectedRunId, setSelectedRunId] = useState<string | null>(null);
  const [selectedRunDossier, setSelectedRunDossier] = useState<RunDossierView | null>(null);
  const [selectedRunDossierState, setSelectedRunDossierState] = useState<"idle" | "loading" | "ready" | "error">("idle");
  const [acpAdaptersState, setAcpAdaptersState] = useState<"loading" | "ready" | "unavailable" | "error">("loading");
  const [acpAdapters, setAcpAdapters] = useState<AcpAdapterSummary[]>([]);
  const [acpProfilesState, setAcpProfilesState] = useState<AcpProfileStoreState>("loading");
  const [acpProfiles, setAcpProfiles] = useState<AcpProfile[]>([]);
  const coreBindings = useCoreBindings(acpProfiles, acpProfilesState === "ready");
  const [selectedAcpProfileId, setSelectedAcpProfileId] = useState<string | null>(null);
  const [providerSourceScopes, setProviderSourceScopes] = useState<ProviderSourceScope[]>([]);
  const [providerSourceScopesState, setProviderSourceScopesState] = useState<"loading" | "ready" | "error" | "unavailable">("loading");
  const [providerSourceScopePathInput, setProviderSourceScopePathInput] = useState("");
  const [providerSourceScopeWriteState, setProviderSourceScopeWriteState] = useState<"idle" | "adding" | "revoking">("idle");
  const [providerSourceScopeError, setProviderSourceScopeError] = useState("");
  const [authenticatingProfileId, setAuthenticatingProfileId] = useState<string | null>(null);
  const [providerAuthProgress, setProviderAuthProgress] = useState<ProviderAuthProgress | null>(null);
  const [cancellingProviderAuth, setCancellingProviderAuth] = useState<{ profileId: string; profileRevision: number } | null>(null);
  const [validatingProfile, setValidatingProfile] = useState<{ profileId: string; profileRevision: number } | null>(null);
  const [providerValidationError, setProviderValidationError] = useState<ProviderValidationFailure | null>(null);
  const [authProfileError, setAuthProfileError] = useState<LiveRunError | null>(null);
  const [authProfileErrorBinding, setAuthProfileErrorBinding] = useState<{ profileId: string; profileRevision: number } | null>(null);
  const [liveCatalog, setLiveCatalog] = useState<ProviderCatalogSnapshot | null>(null);
  const [liveCatalogState, setLiveCatalogState] = useState<LiveCatalogState>("idle");
  const [liveCatalogError, setLiveCatalogError] = useState<LiveRunError | null>(null);

  useEffect(() => {
    setProviderValidationError((failure) => {
      if (!failure) return failure;
      const profile = acpProfiles.find((item) => item.id === failure.profileId);
      return profile?.revision === failure.profileRevision ? failure : null;
    });
  }, [acpProfiles]);
  const [selectedLiveModelId, setSelectedLiveModelId] = useState("");
  const [liveQuestion, setLiveQuestion] = useState("");
  const [liveRunId, setLiveRunId] = useState<string | null>(null);
  const [liveRunSnapshot, setLiveRunSnapshot] = useState<LiveRunSnapshot | null>(null);
  const [liveRunTextDeltas, setLiveRunTextDeltas] = useState<LiveRunTextDelta[]>([]);
  const [liveSnapshotError, setLiveSnapshotError] = useState<LiveRunError | null>(null);
  const [liveRequestState, setLiveRequestState] = useState<LiveRequestState>("idle");
  const [liveRequestError, setLiveRequestError] = useState<LiveRunError | null>(null);
  const [liveCancelRequest, setLiveCancelRequest] = useState<LiveCancelRequestState>(null);
  const [acpProfileDraft, setAcpProfileDraft] = useState<AcpProfileDraft | null>(null);
  const [acpProfileWriteState, setAcpProfileWriteState] = useState<AcpProfileWriteState>("idle");
  const [rolePresetsState, setRolePresetsState] = useState<RolePresetStoreState>("loading");
  const [roleStoreDiagnostic, setRoleStoreDiagnostic] = useState<RoleStoreDiagnostic | null>(null);
  const rolePresetLoad = useRef<Promise<[RolePreset[], ActiveRolePresetSelection | null]> | null>(null);
  const [rolePresets, setRolePresets] = useState<RolePreset[]>([]);
  const [selectedRolePresetId, setSelectedRolePresetId] = useState<string | null>(null);
  const [disclosureConfirmed, setDisclosureConfirmed] = useState(false);
  const confirmedBindingsKey = JSON.stringify(coreBindings.destinations.map(item => item.reference));
  useEffect(() => { setDisclosureConfirmed(false); }, [confirmedBindingsKey]);
  const [acceptedRunId, setAcceptedRunId] = useState<string | null>(null);
  const [runProgress, setRunProgress] = useState<SafeRunProgress | null>(null);
  const [verifiedClarificationParent, setVerifiedClarificationParent] = useState<ClarificationParent | null>(null);
  const [clarificationDraft, setClarificationDraft] = useState<ClarificationDraft | null>(null);
  const clarificationDraftRef = useRef<ClarificationDraft | null>(null);
  clarificationDraftRef.current = clarificationDraft;
  const [runDossier, setRunDossier] = useState<RunDossierView | null>(null);
  const [runDossierState, setRunDossierState] = useState<"idle" | "loading" | "ready" | "error">("idle");
  const [runStartState, setRunStartState] = useState<"idle" | "starting" | "error">("idle");
  const [runStartError, setRunStartError] = useState("");
  const [admissionCancellation, setAdmissionCancellation] = useState<"idle" | "pending" | "accepted" | "error">("idle");
  const [cancelRequest, setCancelRequest] = useState<{ runId: string; state: "sending" | "sent" | "error"; message?: string } | null>(null);
  const [roleDraft, setRoleDraft] = useState<RolePresetDraft | null>(null);
  const [roleWriteState, setRoleWriteState] = useState<RolePresetWriteState>("idle");
  const [shellContext, setShellContext] = useState<ShellContext | null>(null);
  const [dialog, setDialog] = useState<DialogModel | null>(null);
  const [notice, setNotice] = useState("");
  const [invalidQuestion, setInvalidQuestion] = useState(false);
  const mainRef = useRef<HTMLElement>(null);
  const screenRef = useRef<ScreenId>(screen);
  const connectionCheckSequence = useRef(0);
  const automaticConnectionChecks = useRef(new Set<string>());
  const [automaticCheckStep, setAutomaticCheckStep] = useState(0);
  const profileRevisionsRef = useRef(acpProfiles);
  profileRevisionsRef.current = acpProfiles;
  const roleReturnScreen = useRef<ScreenId>("home");
  const connectionReturnScreen = useRef<ScreenId>("home");
  const settingsReturnScreen = useRef<ScreenId>("home");
  const screenScrollPositions = useRef<Partial<Record<ScreenId, { top: number; left: number }>>>({});
  const dialogRef = useRef<HTMLDialogElement>(null);
  const dialogInvoker = useRef<HTMLElement | null>(null);
  const lastEventSequence = useRef(-1);
  const preferencesRef = useRef<ConsolePreferences>(preferenceDefaults);
  const changedPreferenceFields = useRef(new Set<keyof ConsolePreferences>());
  const preferencesReady = useRef(false);
  const preferenceReadGeneration = useRef(0);
  const reloadPreferences = useRef<(() => Promise<void>) | null>(null);
  const preferenceSaveQueue = useRef<Promise<void>>(Promise.resolve());
  const preferenceAuthority = useRef<SettingsSnapshot | null>(null);
  const [commonContextBudgetRevision, setCommonContextBudgetRevision] = useState<number | null>(null);
  const preferenceEditGenerations = useRef<Record<PreferenceField, number>>({ motion: 0, sound: 0, theme: 0, fontScale: 0, language: 0, commonContextTokenLimit: 0 });
  type PreferenceOperation = { promise: Promise<SettingsReceipt>; retry: () => Promise<SettingsReceipt>; state: "pending" | "committed" | "conflict" | "unresolved" };
  const preferencePredecessors = useRef<Partial<Record<PreferenceField, PreferenceOperation>>>({});
  const profileSavePending = useRef(false);
  const providerSelectionRevision = useRef<number | null>(null);
  const activeProviderAuthentication = useRef<{ profileId: string; profileRevision: number } | null>(null);
  const roleSelectionRevision = useRef<number | null>(null);
  const activeRunIdRef = useRef<string | null>(null);
  const selectedRunIdRef = useRef<string | null>(null);
  const pendingLiveRunCommand = useRef<StartLiveRunInput | null>(null);
  const pendingDeliberationCommand = useRef<StartDeliberationInput | null>(null);
  const admissionFlow = useRef<{
    request: StartDeliberationInput;
    clarification: ClarificationStartInput | null;
    authority: AdmissionAuthority | null;
    cancellation: CancelDeliberationRequestInput | null;
    receipt: CancelDeliberationRequestReceipt | null;
    cancellationInFlight: Promise<void> | null;
    registrationPending: boolean;
    inFlight: boolean;
  } | null>(null);
  const pendingLiveCancellation = useRef<CancelLiveRunInput | null>(null);
  const liveCancellationInFlight = useRef<string | null>(null);
  const liveCancelFence = useRef<{ runId: string; storeId: string; storeGeneration: number; afterSequence: number } | null>(null);
  const liveRunCursor = useRef<{
    runId: string;
    storeId: string | null;
    storeGeneration: number | null;
    afterSequence: number;
    revision: number;
  } | null>(null);
  const liveRunRefreshQueue = useRef<Promise<LiveRunSnapshot | null>>(Promise.resolve(null));
  const activeLiveRunIdRef = useRef<string | null>(null);

  const activeRun = snapshot.activeRun;
  const latestRunRef = useRef(activeRun);
  latestRunRef.current = activeRun;
  const [newDraftFromRunId, setNewDraftFromRunId] = useState<string | null>(null);
  const newDraftRunRef = useRef<string | null>(null);
  const retainsNewDraft = (runId: string, status: string, route: ScreenId) => newDraftRunRef.current === runId && ["completed", "cancelled", "failed"].includes(status) && ["input", "intake", "confirmation", "connections", "settings"].includes(route);
  const currentRunId = activeRun?.id ?? acceptedRunId;
  const [dispatchProjection, setDispatchProjection] = useState<CoreDispatchProjection | null>(null);
  const dispatchRequest = useRef(0);
  const dispatchAuthority = useRef<CoreDispatchProjection | null>(null);
  const dispatchRun = useRef(currentRunId);
  dispatchRun.current = currentRunId;
  const refreshCoreDispatches = useCallback(async (runId: string) => {
    const request = ++dispatchRequest.current;
    try {
      const next = await loadRunCoreDispatches(runId);
      if (request !== dispatchRequest.current || dispatchRun.current !== runId) return;
      const previous = dispatchAuthority.current;
      if (previous?.runId === runId && (next.generation < previous.generation || next.runRevision < previous.runRevision || (next.generation === previous.generation && next.inputDigest !== previous.inputDigest))) return;
      dispatchAuthority.current = next; setDispatchProjection(next);
    } catch { if (request === dispatchRequest.current && dispatchRun.current === runId) { setDispatchProjection(null); } }
  }, []);
  useEffect(() => {
    let disposed = false;
    let stop: (() => void) | undefined;
    ++dispatchRequest.current; dispatchAuthority.current = null; setDispatchProjection(null);
    if (currentRunId) void (async () => {
      try {
        stop = await listenCoreDispatches(next => {
          if (disposed || dispatchRun.current !== currentRunId || next?.runId !== currentRunId) return;
          if (!validCoreDispatchProjection(next, currentRunId)) { ++dispatchRequest.current; setDispatchProjection(null); return; }
          void refreshCoreDispatches(currentRunId);
        });
        if (disposed) { stop(); return; }
        await refreshCoreDispatches(currentRunId);
      } catch { if (!disposed) setDispatchProjection(null); }
    })();
    return () => { disposed = true; ++dispatchRequest.current; stop?.(); };
  }, [currentRunId, refreshCoreDispatches]);
  const currentDispatchProjection = dispatchProjection?.runId === currentRunId && (runDossier?.runId !== currentRunId || (dispatchProjection.runRevision >= runDossier.revision && dispatchProjection.generation >= runDossier.generation)) ? dispatchProjection : null;
  const presentedSnapshot: ConsoleSnapshot = { ...snapshot, activeRun: activeRun ? { ...activeRun, dispatchProjection: currentDispatchProjection ?? undefined } : undefined };
  const currentRunProgress = runProgress?.runId === currentRunId ? runProgress : null;
  const currentRunDossier = runDossier?.runId === currentRunId ? runDossier : null;
  const selectedProviderProfile = acpProfiles.find((profile) => profile.id === selectedAcpProfileId);
  const observedAuthenticationFailures = useRef(new Set<string>());
  const observeAuthenticationFailure = useCallback((key: string, failure: unknown) => {
    const binding = authenticationFailureBinding(failure);
    if (!binding) return;
    const identity = `${key}:${JSON.stringify(binding)}`;
    if (observedAuthenticationFailures.current.has(identity)) return;
    observedAuthenticationFailures.current.add(identity);
    coreBindings.invalidateAuthentication(binding);
  }, [coreBindings.invalidateAuthentication]);
  const dossierFence = useRef(new DossierPublicationFence());
  const publishDossier = useCallback((dossier: RunDossierView, request?: number): boolean => {
    const current = dispatchRun.current === dossier.runId;
    const selected = selectedRunIdRef.current === dossier.runId;
    if ((!current && !selected) || !dossierFence.current.accept(dossier, request)) return false;
    observeAuthenticationFailure(`deliberation:${dossier.runId}:${dossier.generation}:${dossier.revision}:${dossier.error?.code}`, dossier.error);
    if (current) { setRunDossier(dossier); setRunDossierState("ready"); }
    if (selected) { setSelectedRunDossier(dossier); setSelectedRunDossierState("ready"); }
    return true;
  }, [observeAuthenticationFailure]);
  const selectedAuthProfileResult = coreBindings.verifiedAuthentication(selectedProviderProfile);
  const selectedLiveCatalog = liveCatalog && liveCatalog.providerProfileId === selectedProviderProfile?.id
    && liveCatalog.profileRevision === selectedProviderProfile?.revision
    && liveCatalog.providerId === selectedProviderProfile?.adapterId
    && liveCatalog.adapterId === selectedProviderProfile?.adapterId
    && liveCatalog.acpMode === "acp"
    ? liveCatalog
    : null;
  const currentLiveRunSnapshot = liveRunSnapshot?.runId === liveRunId ? liveRunSnapshot : null;
  const currentLiveRunText = currentLiveRunSnapshot?.status === "completed" && typeof currentLiveRunSnapshot.result?.finalText === "string"
    ? currentLiveRunSnapshot.result.finalText
    : liveRunTextDeltas.map((delta) => delta.text).join("");
  const selectedRolePreset = rolePresets.find((preset) => preset.id === selectedRolePresetId);
  const currentClarificationDraft = clarificationDraft?.parent.runId === currentRunId && currentRunDossier?.status === "paused" && verifiedClarificationParent?.runId === clarificationDraft.parent.runId && verifiedClarificationParent.revision === clarificationDraft.parent.revision && verifiedClarificationParent.inputDigest === clarificationDraft.parent.inputDigest && verifiedClarificationParent.generation === clarificationDraft.parent.generation ? clarificationDraft : null;
  const retainedDraftIdentity = Boolean(activeRun && newDraftFromRunId === activeRun.id && ["completed", "cancelled", "failed"].includes(activeRun.status));
  const newDraftView = retainedDraftIdentity && ["input", "intake", "confirmation"].includes(screen);
  const shownQuestion = currentClarificationDraft && ["input", "confirmation", "intake"].includes(screen) ? currentClarificationDraft.question : retainedDraftIdentity ? question : activeRun?.question ?? question;
  const isDraft = screen === "input" && (!activeRun || newDraftView);

  useEffect(() => {
    if (!selectedRunId) {
      setSelectedRunDossier(null);
      setSelectedRunDossierState("idle");
      return;
    }
    let disposed = false;
    setSelectedRunDossierState("loading");
    const request = dossierFence.current.begin(selectedRunId);
    void loadRunDossier(selectedRunId).then((dossier) => {
      if (disposed || selectedRunIdRef.current !== selectedRunId || dossier.runId !== selectedRunId) return;
      publishDossier(dossier, request);
    }).catch(() => {
      if (!disposed && selectedRunIdRef.current === selectedRunId && dossierFence.current.isCurrent(selectedRunId, request)) setSelectedRunDossierState("error");
    });
    return () => { disposed = true; };
  }, [selectedRunId, publishDossier]);

  useEffect(() => {
    document.documentElement.dataset.motion = motion;
  }, [motion]);

  useEffect(() => {
    document.documentElement.dataset.theme = theme;
  }, [theme]);

  useEffect(() => {
    document.documentElement.style.fontSize = `${fontScale}%`;
    document.documentElement.dataset.fontScale = String(fontScale);
    const factor = fontScale / 100;
    const fluidFonts = {
      "--fluid-font-cqw-0-8": 0.8,
      "--fluid-font-cqw-0-9": 0.9,
      "--fluid-font-cqw-0-95": 0.95,
      "--fluid-font-cqw-1-25": 1.25,
      "--fluid-font-cqw-1-15": 1.15,
      "--fluid-font-cqw-1-5": 1.5,
      "--fluid-font-cqw-2-5": 2.5,
      "--fluid-font-cqw-3-1": 3.1,
      "--fluid-font-cqw-3-5": 3.5,
      "--fluid-font-cqw-4": 4,
      "--fluid-font-cqw-4-2": 4.2,
      "--fluid-font-cqw-4-45": 4.45,
      "--fluid-font-cqw-5-15": 5.15,
      "--fluid-font-vw-2-3": 2.3,
      "--fluid-font-vw-3-4": 3.4,
      "--fluid-font-vw-5-2": 5.2,
    };
    Object.entries(fluidFonts).forEach(([name, value]) => {
      const unit = name.includes("-vw-") ? "vw" : "cqw";
      document.documentElement.style.setProperty(name, `${value * factor}${unit}`);
    });
  }, [fontScale]);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    const acceptSnapshot = (next: ConsoleSnapshot) => {
      const sequence = next.eventSequence ?? -1;
      if (sequence < lastEventSequence.current) return;
      lastEventSequence.current = sequence;
      if (next.activeRun) activeRunIdRef.current = next.activeRun.id;
      setSnapshot(next);
    };

    void (async () => {
      try {
        unlisten = await listenConsoleSnapshotChanged(acceptSnapshot);
        if (disposed) {
          unlisten();
          return;
        }
        acceptSnapshot(await getConsoleSnapshot());
      } catch {
        if (!disposed) setSnapshot((current) => ({ ...current, connection: "unknown", storage: "unknown" }));
      }
    })();

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void listenProviderAuthProgress((progress) => {
      const active = activeProviderAuthentication.current;
      if (!active || active.profileId !== progress.profileId || active.profileRevision !== progress.profileRevision) return;
      setProviderAuthProgress(progress);
    }).then((stopListening) => {
      if (disposed) stopListening();
      else unlisten = stopListening;
    }).catch(() => undefined);
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  const announce = useCallback((message: string) => {
    setNotice(message);
    window.setTimeout(() => setNotice(""), 5000);
  }, []);

  const refreshLiveRunSnapshot = useCallback((runId: string): Promise<LiveRunSnapshot | null> => {
    const refresh = liveRunRefreshQueue.current.catch(() => undefined).then(async (): Promise<LiveRunSnapshot | null> => {
      if (activeLiveRunIdRef.current !== runId) return null;
      let cursor = liveRunCursor.current;
      if (cursor?.runId !== runId) {
        cursor = { runId, storeId: null, storeGeneration: null, afterSequence: 0, revision: -1 };
        liveRunCursor.current = cursor;
      }

      try {
        let latestSnapshot: LiveRunSnapshot | null = null;
        let caughtUp = false;
        while (!caughtUp) {
          if (activeLiveRunIdRef.current !== runId) return null;
          const currentCursor = liveRunCursor.current;
          if (!currentCursor || currentCursor.runId !== runId) return null;
          const next = await getLiveRunSnapshot(runId, currentCursor.afterSequence);
          if (activeLiveRunIdRef.current !== runId || next.runId !== runId) return null;
          if (next.eventCursor.runId !== runId
            || next.eventCursor.storeId !== next.storeId
            || next.eventCursor.storeGeneration !== next.storeGeneration) {
            setLiveSnapshotError({
              code: "live_run_cursor_mismatch",
              message: "저장된 실행 cursor가 현재 snapshot과 일치하지 않습니다. 새 상태를 확인하십시오.",
              retryable: true,
            });
            return null;
          }
          const storeChanged = currentCursor.storeId !== null
            && (currentCursor.storeId !== next.storeId || currentCursor.storeGeneration !== next.storeGeneration);
          if (storeChanged) {
            currentCursor.storeId = next.storeId;
            currentCursor.storeGeneration = next.storeGeneration;
            currentCursor.afterSequence = 0;
            currentCursor.revision = -1;
            liveCancelFence.current = null;
            setLiveRunSnapshot(null);
            setLiveRunTextDeltas([]);
            continue;
          }
          currentCursor.storeId = next.storeId;
          currentCursor.storeGeneration = next.storeGeneration;
          if (next.revision < currentCursor.revision) return latestSnapshot;

          latestSnapshot = next;
          const previousSequence = currentCursor.afterSequence;
          const lastReturnedSequence = next.events.reduce((highest, event) => Math.max(highest, event.sequence), previousSequence);
          currentCursor.afterSequence = next.eventCursor.complete
            ? Math.max(previousSequence, next.eventCursor.highWaterSequence)
            : lastReturnedSequence;
          currentCursor.revision = next.revision;
          observeAuthenticationFailure(`live:${next.runId}:${next.revision}`, next.failure);
          setLiveRunSnapshot(next);
          setLiveRunTextDeltas((existing) => {
            const deltas = new Map(existing.map((delta) => [delta.sequence, delta]));
            const cancellationFence = liveCancelFence.current;
            next.events.forEach((event) => {
              const afterCancellationFence = cancellationFence?.runId === runId
                && cancellationFence.storeId === next.storeId
                && cancellationFence.storeGeneration === next.storeGeneration
                && event.sequence > cancellationFence.afterSequence;
              if (event.kind === "text_delta" && typeof event.textDelta === "string" && !afterCancellationFence) {
                deltas.set(event.sequence, { sequence: event.sequence, text: event.textDelta });
              }
            });
            return [...deltas.values()].sort((left, right) => left.sequence - right.sequence);
          });
          if (isLiveRunTerminal(next.status)) pendingLiveCancellation.current = null;
          setLiveRequestState("accepted");
          setLiveRequestError(null);
          setLiveSnapshotError(null);
          caughtUp = next.eventCursor.complete || currentCursor.afterSequence <= previousSequence;
        }
        return latestSnapshot;
      } catch (error) {
        if (activeLiveRunIdRef.current !== runId) return null;
        setLiveSnapshotError(normalizeLiveRunError(error));
        return null;
      }
    });
    liveRunRefreshQueue.current = refresh;
    return refresh;
  }, [observeAuthenticationFailure]);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;

    void (async () => {
      try {
        unlisten = await listenLiveRunChanged(({ runId }) => {
          if (activeLiveRunIdRef.current === runId) void refreshLiveRunSnapshot(runId);
        });
        if (disposed) {
          unlisten();
          return;
        }
        let storedRunId: string | null = null;
        try {
          storedRunId = window.localStorage.getItem(liveRunStorageKey);
        } catch {
          storedRunId = null;
        }
        if (storedRunId) {
          activeLiveRunIdRef.current = storedRunId;
          liveRunCursor.current = { runId: storedRunId, storeId: null, storeGeneration: null, afterSequence: 0, revision: -1 };
          setLiveRunId(storedRunId);
          setLiveRequestState("accepted");
          void refreshLiveRunSnapshot(storedRunId);
        }
      } catch {
        if (!disposed) setLiveSnapshotError(normalizeLiveRunError(undefined));
      }
    })();

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [refreshLiveRunSnapshot]);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    const runtimeScreens: ScreenId[] = ["confirmation", "independent", "review", "proposal", "sealed", "verdict", "paused", "interrupted", "cancelling", "cancelled", "failed"];
    const showRunDossier = (dossier: RunDossierView, request?: number) => {
      if (!publishDossier(dossier, request)) return false;
      const next = routeForDossier(dossier);
      if (next) setScreen((current) => runtimeScreens.includes(current) && !retainsNewDraft(dossier.runId, dossier.status, current) && !(clarificationDraftRef.current?.parent.runId === dossier.runId && dossier.status === "paused" && ["input", "confirmation"].includes(current)) ? next : current);
      return true;
    };
    const refreshRunState = async (runId: string) => {
      const request = dossierFence.current.begin(runId);
      try {
        const dossier = await loadRunDossier(runId);
        if (disposed || activeRunIdRef.current !== runId || dossier.runId !== runId) return;
        if (!showRunDossier(dossier, request)) return;
        if (["completed", "failed", "cancelled"].includes(dossier.status)) {
          const [nextSnapshot, recentRuns] = await Promise.all([getConsoleSnapshot(), listRecentRuns()]);
          if (disposed || activeRunIdRef.current !== runId || !dossierFence.current.isCurrent(runId, request) || (nextSnapshot.eventSequence ?? -1) < lastEventSequence.current) return;
          lastEventSequence.current = nextSnapshot.eventSequence ?? -1;
          setSnapshot(nextSnapshot);
          setHomeData({ recordsState: "ready", recentRuns: recentRuns.map((run) => ({ id: run.runId, question: run.question, status: run.status, createdAt: run.createdAt })) });
        }
      } catch {
        if (!disposed && dispatchRun.current === runId && dossierFence.current.isCurrent(runId, request)) setRunDossierState("error");
      }
    };
    void listenRunUpdate((update) => {
      if (!activeRunIdRef.current || activeRunIdRef.current !== update.runId) return;
      if (update.result && (update.result.runId !== update.runId || !showRunDossier(update.result))) return;
      if (!update.dispatchProjection || validCoreDispatchProjection(update.dispatchProjection, update.runId)) void refreshCoreDispatches(update.runId);
      else { ++dispatchRequest.current; setDispatchProjection(null); }
      setAcceptedRunId(update.runId);
      setRunProgress((current) => {
        const previousText = current?.runId === update.runId ? current.text ?? "" : "";
        return {
          runId: update.runId,
          stage: update.stage,
          coreId: update.coreId,
          state: update.state,
          text: update.text === undefined
            ? previousText || undefined
            : update.state === "streaming"
              ? `${previousText}${update.text}`
              : update.text,
        };
      });
      if (!update.result && (update.state === "failed" || update.state === "cancelled" || update.state === "completed")) {
        void refreshRunState(update.runId);
      }
      if (!update.result && (update.state === "started" || update.state === "streaming")) {
        const next = runtimeRoutes[update.stage];
        if (next && !["verdict", "failed", "cancelled"].includes(next)) {
          setScreen((current) => runtimeScreens.includes(current) && !(clarificationDraftRef.current?.parent.runId === update.runId && next === "paused" && ["input", "confirmation"].includes(current)) ? next : current);
        }
      }
    }).then((stop) => {
      unlisten = stop;
      if (disposed) unlisten();
    }).catch(() => {
      if (!disposed) announce("실행 이벤트 연결을 시작하지 못했습니다. 저장된 실행 상태를 다시 확인하십시오.");
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [announce, publishDossier, refreshCoreDispatches]);

  useEffect(() => {
    let disposed = false;
    void getShellContext().then((context) => {
      if (!disposed && context) setShellContext(context);
    }).catch(() => undefined);
    return () => { disposed = true; };
  }, []);

  const reloadRolePresets = useCallback(async (isCurrent: () => boolean = () => true) => {
    setRolePresetsState("loading");
    setRoleStoreDiagnostic(null);
    if (!rolePresetLoad.current) {
      rolePresetLoad.current = (async (): Promise<[RolePreset[], ActiveRolePresetSelection | null]> => {
        let storedPresets: StoredRolePreset[];
        try {
          storedPresets = await listRolePresets();
        } catch (error) {
          throw normalizeRoleStoreDiagnostic(error, "list_presets");
        }

        let selection: ActiveRolePresetSelection | null;
        try {
          selection = await loadActiveRolePresetSelection();
        } catch (error) {
          throw normalizeRoleStoreDiagnostic(error, "load_selection");
        }

        let presets: RolePreset[];
        try {
          presets = storedPresets.map(mapStoredRolePreset);
        } catch {
          throw normalizeRoleStoreDiagnostic(null, "map_presets");
        }
        return [presets, selection];
      })();
    }
    const pending = rolePresetLoad.current;
    try {
      const [presets, selection] = await pending;
      if (!isCurrent()) return;
      setRolePresets(presets);
      const fallback = presets.find((preset) => preset.id === "factory.magi.default") ?? presets.find((preset) => preset.kind === "factory");
      setSelectedRolePresetId(selection?.presetId ?? fallback?.id ?? null);
      roleSelectionRevision.current = selection?.selectionRevision ?? null;
      setRolePresetsState("ready");
    } catch (error) {
      if (!isCurrent()) return;
      setRoleStoreDiagnostic(normalizeRoleStoreDiagnostic(error));
      setRolePresetsState("error");
    } finally {
      if (rolePresetLoad.current === pending) rolePresetLoad.current = null;
    }
  }, []);

  useEffect(() => {
    if (!isDesktopApp()) {
      setHomeData({ recordsState: "unavailable", recentRuns: [] });
      setAcpAdaptersState("unavailable");
      setAcpProfilesState("unavailable");
      setRolePresetsState("unavailable");
      setRoleStoreDiagnostic(null);
      return;
    }
    if (shellContext?.windowLabel !== "main") return;
    let disposed = false;

    void listRecentRuns().then((runs) => {
      if (!disposed) setHomeData({
        recordsState: "ready",
        recentRuns: runs.map((run) => ({ id: run.runId, question: run.question, status: run.status, createdAt: run.createdAt })),
      });
    }).catch(() => {
      if (!disposed) setHomeData({ recordsState: "error", recentRuns: [] });
    });

    void listAcpAdapters().then((adapters) => {
      if (!disposed) {
        setAcpAdapters(adapters);
        setAcpAdaptersState("ready");
      }
    }).catch(() => {
      if (!disposed) setAcpAdaptersState("error");
    });

    void Promise.all([
      listProviderProfiles(),
      loadActiveProviderProfileSelection("codex-acp"),
    ]).then(([profiles, selection]) => {
      if (disposed) return;
      setAcpProfiles(profiles.map(mapProviderProfile));
      setSelectedAcpProfileId(selection?.providerProfileId ?? null);
      providerSelectionRevision.current = selection?.selectionRevision ?? null;
      setAcpProfilesState("ready");
    }).catch(() => {
      if (!disposed) setAcpProfilesState("error");
    });

    void reloadRolePresets(() => !disposed);

    return () => { disposed = true; };
  }, [reloadRolePresets, shellContext?.windowLabel]);

  useEffect(() => {
    if (!isDesktopApp()) {
      setProviderSourceScopes([]);
      setProviderSourceScopesState("unavailable");
      return;
    }
    if (!selectedAcpProfileId) {
      setProviderSourceScopes([]);
      setProviderSourceScopesState("ready");
      setProviderSourceScopeError("");
      return;
    }
    let disposed = false;
    setProviderSourceScopesState("loading");
    setProviderSourceScopeError("");
    void listProviderSourceScopes(selectedAcpProfileId).then((scopes) => {
      if (disposed) return;
      setProviderSourceScopes(scopes);
      setProviderSourceScopesState("ready");
    }).catch(() => {
      if (disposed) return;
      setProviderSourceScopes([]);
      setProviderSourceScopesState("error");
      setProviderSourceScopeError("저장된 자료 접근 권한을 읽지 못했습니다.");
    });
    return () => { disposed = true; };
  }, [selectedAcpProfileId, shellContext?.windowLabel]);

  useEffect(() => {
    if (!activeRun) return;
    activeRunIdRef.current = activeRun.id;
    setAcceptedRunId(activeRun.id);
    const next = runtimeRoutes[activeRun.status];
    const runtimeScreens: ScreenId[] = ["input", "independent", "review", "proposal", "sealed", "verdict", "paused", "interrupted", "cancelling", "cancelled", "failed", "save-error", "confirmation"];
    if (next && runtimeScreens.includes(screen) && screen !== next && !retainsNewDraft(activeRun.id, activeRun.status, screen) && !(currentClarificationDraft && ["input", "confirmation"].includes(screen))) {
      setScreen(next);
    }
  }, [activeRun?.id, activeRun?.status, screen, currentClarificationDraft]);

  useEffect(() => {
    if (!currentRunId) {
      setRunDossier(null);
      setRunDossierState("idle");
      return;
    }
    let disposed = false;
    setRunDossierState("loading");
    const request = dossierFence.current.begin(currentRunId);
    void loadRunDossier(currentRunId).then((dossier) => {
      if (disposed || dossier.runId !== currentRunId) return;
      if (!publishDossier(dossier, request)) return;
      const next = routeForDossier(dossier);
      const runtimeScreens: ScreenId[] = ["confirmation", "independent", "review", "proposal", "sealed", "verdict", "paused", "interrupted", "cancelling", "cancelled", "failed"];
      if (next) setScreen((current) => runtimeScreens.includes(current) && !retainsNewDraft(dossier.runId, dossier.status, current) && !(clarificationDraftRef.current?.parent.runId === dossier.runId && dossier.status === "paused" && ["input", "confirmation"].includes(current)) ? next : current);
    }).catch(() => {
      if (!disposed && dispatchRun.current === currentRunId && dossierFence.current.isCurrent(currentRunId, request)) setRunDossierState("error");
    });
    return () => { disposed = true; };
  }, [currentRunId, publishDossier]);

  useEffect(() => {
    const node = dialogRef.current;
    if (!node) return;
    if (dialog && !node.open) {
      dialogInvoker.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
      node.showModal();
    } else if (!dialog && node.open) {
      node.close();
    }
  }, [dialog]);

  useEffect(() => {
    const node = dialogRef.current;
    if (!node) return;
    const restore = () => {
      const invoker = dialogInvoker.current;
      if (invoker?.isConnected) invoker.focus();
      dialogInvoker.current = null;
    };
    node.addEventListener("close", restore);
    return () => node.removeEventListener("close", restore);
  }, []);

  const navigate = useCallback((requested: ScreenId, returning = false) => {
    const next = requested === "provider" ? "connections" : requested;
    const current = screenRef.current;
    const latest = latestRunRef.current;
    if (next === "input" && latest && ["completed", "cancelled", "failed"].includes(latest.status)) {
      newDraftRunRef.current = latest.id;
      setNewDraftFromRunId(latest.id);
    }
    screenScrollPositions.current[current] = { top: window.scrollY, left: window.scrollX };
    if (!returning && next === "connections" && current !== "connections") connectionReturnScreen.current = current;
    if (!returning && next === "settings" && current !== "settings") settingsReturnScreen.current = current;
    if (next === "roles" && screenRef.current !== "roles") roleReturnScreen.current = screenRef.current;
    screenRef.current = next;
    setScreen(next);
    setInvalidQuestion(false);
    setNotice("");
    setDialog(null);
    setTimeout(() => {
      mainRef.current?.focus({ preventScroll: true });
      const position = screenScrollPositions.current[next];
      window.scrollTo(position?.left ?? 0, position?.top ?? 0);
    }, 0);
  }, []);

  useEffect(() => { screenRef.current = screen; }, [screen]);

  const returnFromRoles = useCallback(() => navigate(roleReturnScreen.current), [navigate]);

  const openRecentRun = useCallback((runId: string) => {
    selectedRunIdRef.current = runId;
    setSelectedRunId(runId);
    navigate("history");
  }, [navigate]);

  const beginAcpProfileCreate = useCallback(() => {
    setAcpProfileDraft({ profileId: null, expectedRevision: null, displayName: "", adapterId: "codex-acp", credentialHomePath: "" });
    setAcpProfileWriteState("idle");
  }, []);

  const beginAcpProfileEdit = useCallback((profileId: string) => {
    const profile = acpProfiles.find((item) => item.id === profileId);
    if (!profile) {
      announce("편집할 연결 프로필이 최신 목록에 없습니다. 프로필 목록을 새로 읽습니다.");
      void listProviderProfiles().then((items) => setAcpProfiles(items.map(mapProviderProfile))).catch(() => setAcpProfilesState("error"));
      return;
    }
    setAcpProfileDraft({ profileId: profile.id, expectedRevision: profile.revision, displayName: profile.displayName, adapterId: profile.adapterId, credentialHomePath: profile.credentialHome?.displayPath ?? "" });
    setAcpProfileWriteState("idle");
  }, [acpProfiles, announce]);

  const saveAcpProfileDraft = useCallback(async (draft: AcpProfileDraft) => {
    if (profileSavePending.current) return;
    profileSavePending.current = true;
    setAcpProfileWriteState("saving");
    let saved: Awaited<ReturnType<typeof saveProviderProfile>>;
    try {
      saved = await saveProviderProfile(draft);
    } catch (error) {
      const message = String(error).toLowerCase();
      setAcpProfileWriteState(message.includes("revision") || message.includes("conflict") ? "conflict" : "error");
      if (message.includes("revision") || message.includes("conflict")) {
        void listProviderProfiles().then((items) => setAcpProfiles(items.map(mapProviderProfile))).catch(() => setAcpProfilesState("error"));
      }
      profileSavePending.current = false;
      announce("연결 프로필을 저장하지 못했습니다. 입력 내용을 유지했습니다.");
      return;
    }

    setAcpProfiles((profiles) => {
      const mapped = mapProviderProfile(saved);
      return profiles.some((profile) => profile.id === mapped.id)
        ? profiles.map((profile) => profile.id === mapped.id ? mapped : profile)
        : [...profiles, mapped];
    });
    profileSavePending.current = false;
    setAcpProfileDraft(null);
    setAcpProfileWriteState("saved");
    setDisclosureConfirmed(false);
    announce(`연결 프로필 ${saved.displayName}을(를) 저장했습니다. 런타임 검증 전까지 모델 호출은 차단됩니다.`);

    try {
      const profiles = await listProviderProfiles();
      setAcpProfiles(profiles.map(mapProviderProfile));
      setAcpProfilesState("ready");
    } catch {
      setAcpProfilesState("error");
      announce("프로필은 저장됐지만 목록을 다시 읽지 못했습니다. 저장된 값은 보존되어 있습니다.");
    }
  }, [announce]);

  const refreshProviderSourceScopes = useCallback(async (profileId: string) => {
    const scopes = await listProviderSourceScopes(profileId);
    if (profileId === selectedAcpProfileId) {
      setProviderSourceScopes(scopes);
      setProviderSourceScopesState("ready");
    }
  }, [selectedAcpProfileId]);

  const chooseProviderSourceDirectory = useCallback(async () => {
    setProviderSourceScopeError("");
    try {
      const path = await pickProviderSourceDirectory();
      if (path) setProviderSourceScopePathInput(path);
    } catch {
      setProviderSourceScopeError("자료 접근 폴더 선택을 완료하지 못했습니다.");
    }
  }, []);

  const addProviderSourceRoot = useCallback(async () => {
    const profileId = selectedAcpProfileId;
    const pathInput = providerSourceScopePathInput.trim();
    if (!profileId || !pathInput) return;
    setProviderSourceScopeWriteState("adding");
    setProviderSourceScopeError("");
    try {
      await addProviderSourceScope(profileId, pathInput);
      await refreshProviderSourceScopes(profileId);
      setProviderSourceScopePathInput("");
      announce("이 프로필의 자료 읽기 권한을 저장했습니다. 이후 실행은 같은 폴더 권한을 사용합니다.");
    } catch {
      setProviderSourceScopeError("자료 폴더 권한을 저장하지 못했습니다. 경로와 폴더 접근 상태를 확인하십시오.");
    } finally {
      setProviderSourceScopeWriteState("idle");
    }
  }, [announce, providerSourceScopePathInput, refreshProviderSourceScopes, selectedAcpProfileId]);

  const revokeProviderSourceRoot = useCallback(async (grantId: string) => {
    const profileId = selectedAcpProfileId;
    if (!profileId) return;
    setProviderSourceScopeWriteState("revoking");
    setProviderSourceScopeError("");
    try {
      await revokeProviderSourceScope(profileId, grantId);
      await refreshProviderSourceScopes(profileId);
      announce("자료 폴더 권한을 회수했습니다. 이후 ACP 읽기 요청에는 이 권한이 적용되지 않습니다.");
    } catch {
      setProviderSourceScopeError("자료 폴더 권한을 회수하지 못했습니다. 권한 목록을 다시 읽어 확인하십시오.");
    } finally {
      setProviderSourceScopeWriteState("idle");
    }
  }, [announce, refreshProviderSourceScopes, selectedAcpProfileId]);

  const selectAcpProfile = useCallback(async (profileId: string) => {
    const profile = acpProfiles.find((item) => item.id === profileId);
    if (!profile) return;
    setAuthProfileError(null);
    setLiveCatalog(null);
    setLiveCatalogState("idle");
    setLiveCatalogError(null);
    setSelectedLiveModelId("");
    setLiveRequestError(null);
    setLiveRequestState("idle");
    pendingLiveRunCommand.current = null;
    try {
      const selection = await setActiveProviderProfile(profile.adapterId, profile.id, providerSelectionRevision.current);
      providerSelectionRevision.current = selection.selectionRevision;
      setSelectedAcpProfileId(selection.providerProfileId);
      setDisclosureConfirmed(false);
      announce(`${profile.displayName} 프로필을 선택했습니다. 실행 검증은 별도로 필요합니다.`);
    } catch {
      try {
        const selection = await loadActiveProviderProfileSelection(profile.adapterId);
        providerSelectionRevision.current = selection?.selectionRevision ?? null;
        setSelectedAcpProfileId(selection?.providerProfileId ?? null);
        setDisclosureConfirmed(false);
      } catch {
        setAcpProfilesState("error");
      }
      announce("프로필 선택이 동시에 변경되었습니다. 최신 선택을 다시 읽었습니다.");
    }
  }, [acpProfiles, announce]);

  const validateAcpProfile = useCallback(async (profileId: string) => {
    const profile = acpProfiles.find((item) => item.id === profileId);
    if (!profile) return;
    const binding = { profileId, profileRevision: profile.revision };
    setProviderValidationError(null);
    setValidatingProfile(binding);
    setAcpProfiles((profiles) => profiles.map((profile) => profile.id === binding.profileId && profile.revision === binding.profileRevision
      ? { ...profile, admission: { state: "not_checked" } }
      : profile));
    if (selectedProviderProfile?.id === profileId) {
      setAuthProfileError(null);
      setAuthProfileErrorBinding(null);
      setLiveCatalog(null);
      setLiveCatalogState("idle");
      setLiveCatalogError(null);
      setSelectedLiveModelId("");
      setLiveRequestError(null);
      setLiveRequestState("idle");
      pendingLiveRunCommand.current = null;
    }
    try {
      const result = await validateProviderProfile(profileId);
      if (result.profileId !== binding.profileId
        || result.profileRevision !== binding.profileRevision
        || result.adapterId !== profile.adapterId) {
        throw new Error("profile_validation_binding_mismatch");
      }
      const admission = mapProviderAdmission(result);
      setAcpProfiles((profiles) => profiles.map((current) => current.id === profileId && current.revision === binding.profileRevision
        ? { ...current, admission }
        : current));
      setProviderValidationError(null);
      announce(result.state === "ready" ? "프로필 홈 attestation이 확인되었습니다. 기존 CLI 구독 인증 상태는 별도로 확인하십시오." : "프로필 실행 자격이 차단되었습니다. 표시된 사유를 확인하십시오.");
    } catch {
      setAcpProfiles((profiles) => profiles.map((current) => current.id === profileId && current.revision === binding.profileRevision
        ? { ...current, admission: { state: "not_checked" } }
        : current));
      setProviderValidationError({ ...binding, code: "validation_command_failed" });
      announce("프로필 검증 결과를 받지 못했습니다. 이전 검증을 사용하지 않고 실행을 차단했습니다.");
    } finally {
      setValidatingProfile((current) => current?.profileId === binding.profileId && current.profileRevision === binding.profileRevision ? null : current);
    }
  }, [acpProfiles, announce, selectedProviderProfile?.id]);

  const authenticateAcpProfile = useCallback(async (profileId: string) => {
    const profile = acpProfiles.find((item) => item.id === profileId);
    if (!profile || !hasVerifiedProfileHome(profile)
      || (validatingProfile?.profileId === profileId && validatingProfile.profileRevision === profile.revision)) {
      announce("프로필 홈 검증을 통과한 뒤 기존 CLI 구독 인증을 확인하십시오.");
      return;
    }
    if (profile.authenticationMethod !== "local_subscription") {
      announce("기존 CLI 구독 인증 확인은 구독 연결 프로필에서만 사용할 수 있습니다.");
      return;
    }
    if (activeProviderAuthentication.current) return;
    const check = coreBindings.beginConnectionCheck(profile.id, profile.revision);
    const binding = { profileId: profile.id, profileRevision: profile.revision };
    activeProviderAuthentication.current = binding;
    setAuthenticatingProfileId(profileId);
    setProviderAuthProgress({ ...binding, stage: "checking_status" });
    setCancellingProviderAuth(null);
    setAuthProfileError(null);
    setAuthProfileErrorBinding(binding);
    try {
      const result = await authenticateProviderProfile(profileId, profile.revision);
      if (result.profileId !== profile.id || result.profileRevision !== profile.revision || result.providerId !== profile.adapterId) {
        throw { code: "profile_revision_mismatch", message: "프로필 버전이 바뀌었습니다. 최신 프로필에서 인증 상태를 다시 확인하십시오.", retryable: false };
      }
      setAuthProfileError(null);
      setAuthProfileErrorBinding(null);
      coreBindings.finishConnectionCheck(check, false);
      setProviderAuthProgress((current) => current?.profileId === profile.id && current.profileRevision === profile.revision
        ? { ...binding, stage: result.state === "authenticated" ? "callback_complete" : "failed" }
        : current);
      announce(result.state === "authenticated" ? "선택한 CLI 홈의 기존 구독 인증 상태가 확인되었습니다." : result.state === "unauthenticated" ? "선택한 CLI 홈에서 기존 구독 인증을 확인하지 못했습니다. 저장한 인증 홈 경로와 CLI 인증 상태를 확인하십시오." : "이 프로필은 기존 CLI 구독 인증 상태 확인을 지원하지 않습니다.");
    } catch (error) {
      coreBindings.finishConnectionCheck(check, false);
      const normalizedError = normalizeLiveRunError(error);
      setAuthProfileError(normalizedError);
      setAuthProfileErrorBinding(binding);
      setProviderAuthProgress((current) => current?.profileId === profile.id && current.profileRevision === profile.revision
        ? { ...binding, stage: normalizedError.code === "authentication_cancelled" ? "cancelled" : "failed" }
        : current);
      announce("기존 CLI 구독 인증 상태를 확인하지 못했습니다. 이전 결과로 실행을 허용하지 않습니다.");
    } finally {
      if (activeProviderAuthentication.current?.profileId === binding.profileId
        && activeProviderAuthentication.current.profileRevision === binding.profileRevision) {
        activeProviderAuthentication.current = null;
      }
      setCancellingProviderAuth((current) => current?.profileId === binding.profileId && current.profileRevision === binding.profileRevision ? null : current);
      setAuthenticatingProfileId((current) => current === profileId ? null : current);
    }
  }, [acpProfiles, announce, validatingProfile, coreBindings.beginConnectionCheck, coreBindings.finishConnectionCheck]);

  const cancelAcpAuthentication = useCallback(async (profileId: string, profileRevision: number) => {
    const active = activeProviderAuthentication.current;
    if (!active || active.profileId !== profileId || active.profileRevision !== profileRevision) return;
    setCancellingProviderAuth({ profileId, profileRevision });
    try {
      const cancelled = await cancelProviderAuthentication(profileId, profileRevision);
      if (!cancelled) {
        setCancellingProviderAuth((current) => current?.profileId === profileId && current.profileRevision === profileRevision ? null : current);
        setAuthProfileError({ code: "authentication_not_active", message: "인증 확인 작업이 이미 종료되어 취소할 수 없습니다. 현재 상태를 다시 확인하십시오.", retryable: true });
        setAuthProfileErrorBinding({ profileId, profileRevision });
      }
    } catch (error) {
      setCancellingProviderAuth((current) => current?.profileId === profileId && current.profileRevision === profileRevision ? null : current);
      setAuthProfileError(normalizeLiveRunError(error));
      setAuthProfileErrorBinding({ profileId, profileRevision });
    }
  }, []);

  const refreshLiveCatalog = useCallback(async () => {
    const profile = selectedProviderProfile;
    const auth = selectedAuthProfileResult;
    const check = profile ? coreBindings.beginConnectionCheck(profile.id, profile.revision) : null;
    if (!hasVerifiedProfileHome(profile)
      || (validatingProfile?.profileId === profile.id && validatingProfile.profileRevision === profile.revision)) {
      setLiveCatalog(null);
      setLiveCatalogState("error");
      if (check) coreBindings.finishConnectionCheck(check, false);
      setLiveCatalogError({ code: "profile_not_ready", message: "프로필별 전용 홈 검증을 마친 뒤 모델 목록을 가져올 수 있습니다.", retryable: false });
      return;
    }
    if (profile.authenticationMethod !== "local_subscription") {
      setLiveCatalog(null);
      setLiveCatalogState("error");
      if (check) coreBindings.finishConnectionCheck(check, false);
      setLiveCatalogError({ code: "acp_profile_unsupported", message: "BYOK API 프로필은 공식 Codex App Server ACP 모델 요청을 지원하지 않습니다. 이 경로는 ChatGPT 구독 프로필만 사용합니다.", retryable: false });
      setSelectedLiveModelId("");
      setLiveRequestError(null);
      setLiveRequestState("idle");
      pendingLiveRunCommand.current = null;
      return;
    }
    if (!auth || auth.profileId !== profile.id || auth.profileRevision !== profile.revision
      || auth.providerId !== profile.adapterId || auth.state !== "authenticated" || auth.method !== "chat_gpt") {
      setLiveCatalog(null);
      setLiveCatalogState("error");
      if (check) coreBindings.finishConnectionCheck(check, false);
      setLiveCatalogError({ code: "authentication_required", message: "선택한 CLI 홈의 기존 구독 인증 상태를 확인한 뒤 모델 목록을 가져오십시오.", retryable: false });
      return;
    }
    setLiveCatalogState("loading");
    setLiveCatalogError(null);
    setLiveCatalog(null);
    setSelectedLiveModelId("");
    setLiveRequestError(null);
    setLiveRequestState("idle");
    pendingLiveRunCommand.current = null;
    try {
      const state = await refreshProviderCatalog(profile.id, profile.revision);
      const catalog = state.catalog;
      if (!catalogHasExecutionAuthority(catalog)) throw new Error("catalog_missing_runtime_authority");
      if (catalog.providerProfileId !== profile.id
        || catalog.profileRevision !== profile.revision
        || catalog.adapterId !== profile.adapterId
        || catalog.providerId !== profile.adapterId
        || catalog.acpMode !== "acp") {
        throw { code: "catalog_binding_mismatch", message: "새 모델 목록이 선택한 프로필 버전과 일치하지 않습니다. 목록을 다시 가져오십시오.", retryable: true };
      }
      setLiveCatalog(catalog);
      setLiveCatalogState("ready");
      await coreBindings.reload();
      if (check) coreBindings.finishConnectionCheck(check, true, auth, catalog);
    } catch (error) {
      if (check) coreBindings.finishConnectionCheck(check, false);
      setLiveCatalog(null);
      setLiveCatalogState("error");
      setLiveCatalogError(normalizeLiveRunError(error));
    }
  }, [selectedAuthProfileResult, selectedProviderProfile, validatingProfile, coreBindings.reload, coreBindings.beginConnectionCheck, coreBindings.finishConnectionCheck]);

  const checkConnection = useCallback(async (profileId: string) => {
    const profile = profileRevisionsRef.current.find(item => item.id === profileId);
    if (!profile || profile.authenticationMethod !== "local_subscription" || activeProviderAuthentication.current) return;
    automaticConnectionChecks.current.add(`${profile.id}:${profile.revision}`);
    const check = coreBindings.beginConnectionCheck(profileId, profile.revision);
    const request = ++connectionCheckSequence.current;
    const binding = { profileId, profileRevision: profile.revision };
    const current = () => connectionCheckSequence.current === request && profileRevisionsRef.current.some(item => item.id === profileId && item.revision === profile.revision);
    activeProviderAuthentication.current = binding;
    setAuthenticatingProfileId(profileId);
    setValidatingProfile(binding);
    setProviderValidationError(null);
    setAcpProfiles(previous => previous.map(item => item.id === profileId && item.revision === profile.revision ? { ...item, admission: { state: "not_checked" } } : item));
    setAuthProfileError(null);
    setAuthProfileErrorBinding(binding);
    setLiveCatalog(null);
    setLiveCatalogState("idle");
    setLiveCatalogError(null);
    setSelectedLiveModelId("");
    try {
      const validated = await validateProviderProfile(profileId);
      if (!current()) return;
      if (validated.profileId !== profileId || validated.profileRevision !== profile.revision || validated.adapterId !== profile.adapterId || validated.state !== "ready") throw new Error("profile_validation_failed");
      setAcpProfiles(previous => previous.map(item => item.id === profileId && item.revision === profile.revision ? { ...item, admission: mapProviderAdmission(validated) } : item));
      setValidatingProfile(null);
      setProviderAuthProgress({ ...binding, stage: "checking_status" });
      const auth = await authenticateProviderProfile(profileId, profile.revision);
      if (!current()) return;
      if (auth.profileId !== profileId || auth.profileRevision !== profile.revision || auth.providerId !== profile.adapterId || auth.state !== "authenticated" || auth.method !== "chat_gpt") throw new Error("subscription_authentication_failed");
      setProviderAuthProgress({ ...binding, stage: "callback_complete" });
      setLiveCatalogState("loading");
      const result = await refreshProviderCatalog(profileId, profile.revision);
      const catalog = result.catalog;
      if (!catalogHasExecutionAuthority(catalog)) throw new Error("catalog_missing_runtime_authority");
      if (!current()) return;
      if (catalog.providerProfileId !== profileId || catalog.profileRevision !== profile.revision || catalog.adapterId !== profile.adapterId || catalog.providerId !== profile.adapterId || catalog.acpMode !== "acp") throw new Error("catalog_binding_mismatch");
      setLiveCatalog(catalog);
      setLiveCatalogState("ready");
      setAuthProfileErrorBinding(null);
      await coreBindings.reload();
      if (!current()) return;
      coreBindings.finishConnectionCheck(check, true, auth, catalog);
      announce("기존 구독 인증과 실제 모델 목록을 확인했습니다.");
    } catch (error) {
      if (!current()) return;
      coreBindings.finishConnectionCheck(check, false);
      const failure = normalizeLiveRunError(error);
      setAuthProfileError(failure);
      setAuthProfileErrorBinding(binding);
      setProviderAuthProgress({ ...binding, stage: "failed" });
      setLiveCatalog(null);
      setLiveCatalogState("error");
      setLiveCatalogError(failure);
      announce("연결 확인에 실패했습니다. 프로필 경로와 표시된 오류를 확인하십시오.");
    } finally {
      if (connectionCheckSequence.current === request) {
        activeProviderAuthentication.current = null;
        setAuthenticatingProfileId(null);
        setValidatingProfile(null);
      }
    }
  }, [announce, coreBindings.reload, coreBindings.beginConnectionCheck, coreBindings.finishConnectionCheck]);

  useEffect(() => {
    if (acpProfilesState !== "ready" || activeProviderAuthentication.current) return;
    const profile = coreBindings.cores
      .map(core => profileRevisionsRef.current.find(item => item.id === core.selection?.providerProfileId))
      .find(item => item?.authenticationMethod === "local_subscription" && !automaticConnectionChecks.current.has(`${item.id}:${item.revision}`));
    if (!profile) return;
    void checkConnection(profile.id).finally(() => setAutomaticCheckStep(step => step + 1));
  }, [acpProfilesState, acpProfiles, coreBindings.cores, checkConnection, authenticatingProfileId, automaticCheckStep]);

  const changeSelectedLiveModelId = useCallback((modelId: string) => {
    setSelectedLiveModelId(modelId);
    setLiveRequestError(null);
    setLiveRequestState((current) => current === "starting" ? current : "idle");
  }, []);

  const changeLiveQuestion = useCallback((question: string) => {
    setLiveQuestion(question);
    setLiveRequestError(null);
    setLiveRequestState((current) => current === "starting" ? current : "idle");
  }, []);

  const startLiveProviderRun = useCallback(async () => {
    const profile = selectedProviderProfile;
    const catalog = liveCatalog;
    const auth = selectedAuthProfileResult;
    const prompt = liveQuestion.trim();
    const model = catalog?.models.find((entry) => entry.modelId === selectedLiveModelId);
    if (profile?.authenticationMethod === "byok_api") {
      pendingLiveRunCommand.current = null;
      setLiveRequestError({ code: "acp_profile_unsupported", message: "이 요청은 기존 CLI 구독 인증 홈을 가진 ACP 프로필을 요구합니다. 구독 연결 프로필을 선택하십시오.", retryable: false });
      setLiveRequestState("error");
      return;
    }
    const admitted = hasVerifiedProfileHome(profile)
      && !(profile && validatingProfile?.profileId === profile.id && validatingProfile.profileRevision === profile.revision);
    const catalogMatches = Boolean(profile && catalog
      && catalog.providerProfileId === profile.id
      && catalog.profileRevision === profile.revision
      && catalog.providerId === profile.adapterId
      && catalog.adapterId === profile.adapterId
      && catalog.acpMode === "acp");
    const authMatches = Boolean(profile && auth
      && auth.profileId === profile.id
      && auth.profileRevision === profile.revision
      && auth.providerId === profile.adapterId
      && auth.state === "authenticated"
      && auth.method === "chat_gpt");

    const savedBinding = profile ? coreBindings.catalogs[profile.id]?.modelSelection?.binding : undefined;
    const selectedMode = savedBinding?.catalogSnapshotId === catalog?.catalogSnapshotId
      && savedBinding?.catalogDigest === catalog?.catalogDigest
      && savedBinding?.artifactSetDigest === catalog?.artifactSetDigest
      && savedBinding?.modelId === model?.modelId ? savedBinding?.modeId : undefined;
    const modeReady = catalog?.negotiatedModes && (catalog.negotiatedModes.modes.length
      ? catalog.negotiatedModes.modes.some(mode => mode.modeId === selectedMode)
      : catalog.negotiatedModes.currentModeId === null && selectedMode == null);
    if (!profile || !catalogHasExecutionAuthority(catalog) || !admitted || !catalogMatches || !authMatches || !model || !prompt || !modeReady) {
      setLiveRequestError({ code: "request_precondition_failed", message: "인증 상태, 프로필 검증, 최신 모델 선택, 질문을 확인하십시오.", retryable: false });
      setLiveRequestState("error");
      return;
    }

    const modelBinding: AcpModelBindingInput = {
      schemaVersion: catalog.schemaVersion,
      catalogSnapshotId: catalog.catalogSnapshotId,
      catalogDigest: catalog.catalogDigest,
      providerId: catalog.providerId,
      acpMode: catalog.acpMode,
      providerProfileId: catalog.providerProfileId,
      profileRevision: catalog.profileRevision,
      adapterId: catalog.adapterId,
      adapterVersion: catalog.adapterVersion,
      adapterDigest: catalog.adapterDigest,
      artifactSetDigest: catalog.artifactSetDigest,
      modelId: model.modelId,
      modeId: selectedMode ?? null,
    };
    const previousCommand = pendingLiveRunCommand.current;
    const canReplay = previousCommand
      && previousCommand.question === prompt
      && previousCommand.modelBinding.catalogSnapshotId === modelBinding.catalogSnapshotId
      && previousCommand.modelBinding.catalogDigest === modelBinding.catalogDigest
      && previousCommand.modelBinding.artifactSetDigest === modelBinding.artifactSetDigest
      && previousCommand.modelBinding.modeId === modelBinding.modeId
      && previousCommand.modelBinding.modelId === modelBinding.modelId;
    const command: StartLiveRunInput = canReplay ? previousCommand : {
      commandId: crypto.randomUUID(),
      idempotencyKey: crypto.randomUUID(),
      question: prompt,
      modelBinding,
    };
    pendingLiveRunCommand.current = command;
    setLiveRequestState("starting");
    setLiveRequestError(null);
    try {
      const receipt = await startLiveRun(command);
      pendingLiveRunCommand.current = null;
      pendingLiveCancellation.current = null;
      liveCancelFence.current = null;
      setLiveCancelRequest(null);
      activeLiveRunIdRef.current = receipt.runId;
      liveRunCursor.current = { runId: receipt.runId, storeId: null, storeGeneration: null, afterSequence: 0, revision: -1 };
      setLiveRunId(receipt.runId);
      setLiveRunSnapshot(null);
      setLiveRunTextDeltas([]);
      setLiveSnapshotError(null);
      setLiveRequestState("accepted");
      setLiveRequestError(null);
      try {
        window.localStorage.setItem(liveRunStorageKey, receipt.runId);
      } catch {
        // The durable run remains available through its returned ID and the CLI query command.
      }
      void refreshLiveRunSnapshot(receipt.runId);
    } catch (error) {
      const safeError = normalizeLiveRunError(error);
      if (!safeError.retryable) pendingLiveRunCommand.current = null;
      setLiveRequestError(safeError);
      setLiveRequestState("error");
    }
  }, [coreBindings.catalogs, selectedAuthProfileResult, liveCatalog, liveQuestion, refreshLiveRunSnapshot, selectedLiveModelId, selectedProviderProfile, validatingProfile]);

  const cancelLiveProviderRun = useCallback(async (runId: string) => {
    if (liveCancellationInFlight.current === runId
      || (liveCancelRequest?.runId === runId && liveCancelRequest.state !== "error")) return;

    liveCancellationInFlight.current = runId;
    setLiveCancelRequest({ runId, state: "sending" });
    try {
      const snapshot = liveRunSnapshot?.runId === runId
        ? liveRunSnapshot
        : await getLiveRunSnapshot(runId, 0);
      if (snapshot.runId !== runId) {
        throw { code: "live_run_id_mismatch", message: "저장된 요청 상태가 선택한 실행과 일치하지 않습니다.", retryable: true };
      }
      if (!isLiveRunCancelable(snapshot.status)) {
        setLiveCancelRequest({
          runId,
          state: "error",
          error: { code: "live_run_not_cancelable", message: "저장된 실행 상태가 이미 취소 요청을 받았거나 종료되었습니다. 최신 상태를 확인하십시오.", retryable: false },
        });
        void refreshLiveRunSnapshot(runId);
        return;
      }

      const existing = pendingLiveCancellation.current;
      const command: CancelLiveRunInput = existing?.runId === runId ? existing : {
        commandId: crypto.randomUUID(),
        idempotencyKey: crypto.randomUUID(),
        runId,
        expectedRevision: snapshot.revision,
      };
      pendingLiveCancellation.current = command;
      const receipt = await cancelLiveRun(command);
      if (receipt.runId !== runId || receipt.eventCursor.runId !== runId) {
        throw { code: "live_run_cancel_receipt_mismatch", message: "취소 접수 결과가 선택한 실행과 일치하지 않습니다. 저장 상태를 다시 확인하십시오.", retryable: true };
      }
      liveCancelFence.current = {
        runId,
        storeId: receipt.eventCursor.storeId,
        storeGeneration: receipt.eventCursor.storeGeneration,
        afterSequence: receipt.eventCursor.highWaterSequence,
      };
      setLiveCancelRequest({ runId, state: "accepted" });
      announce(receipt.status === "completed"
        ? "취소보다 먼저 실행이 완료되었습니다. 저장된 결과를 다시 확인합니다."
        : `취소 명령을 저장했습니다. provider 상태 · ${receipt.providerOutcome === "confirmed" ? "정지 확인됨" : receipt.providerOutcome === "pending" ? "정지 확인 중" : receipt.providerOutcome === "not_started" ? "provider 요청 전 취소" : "미확인"}.`);
      void refreshLiveRunSnapshot(runId);
    } catch (error) {
      setLiveCancelRequest({ runId, state: "error", error: normalizeLiveRunError(error) });
    } finally {
      if (liveCancellationInFlight.current === runId) liveCancellationInFlight.current = null;
    }
  }, [announce, liveCancelRequest, liveRunSnapshot, refreshLiveRunSnapshot]);

  const selectRolePreset = useCallback(async (presetId: string) => {
    try {
      const selection = await setActiveRolePreset(presetId, roleSelectionRevision.current ?? 0);
      roleSelectionRevision.current = selection.selectionRevision;
      setSelectedRolePresetId(selection.presetId);
      setRoleDraft(null);
      setDisclosureConfirmed(false);
      announce("활성 역할 프로필을 로컬 저장소에 반영했습니다.");
    } catch {
      try {
        const selection = await loadActiveRolePresetSelection();
        roleSelectionRevision.current = selection?.selectionRevision ?? null;
        setSelectedRolePresetId(selection?.presetId ?? null);
        setDisclosureConfirmed(false);
      } catch {
        setRolePresetsState("error");
      }
      announce("역할 선택이 동시에 바뀌었습니다. 최신 선택을 다시 읽었습니다.");
    }
  }, [announce]);

  const beginRolePresetEdit = useCallback((presetId: string) => {
    const preset = rolePresets.find((item) => item.id === presetId);
    if (!preset || preset.kind !== "user") return;
    setRoleDraft(toRolePresetDraft(preset));
    setRoleWriteState("idle");
  }, [rolePresets]);

  const cloneRolePresetForEditing = useCallback(async (presetId: string) => {
    const source = rolePresets.find((item) => item.id === presetId);
    if (!source) return;
    try {
      const cloned = await cloneRolePreset(source.id, source.revision, `${source.name} · 사용자 복사본`);
      const selection = await setActiveRolePreset(cloned.presetId, roleSelectionRevision.current ?? 0);
      roleSelectionRevision.current = selection.selectionRevision;
      const refreshed = await listRolePresets();
      const presets = refreshed.map(mapStoredRolePreset);
      setRolePresets(presets);
      const clone = presets.find((preset) => preset.id === cloned.presetId);
      if (clone) setRoleDraft(toRolePresetDraft(clone));
      setSelectedRolePresetId(cloned.presetId);
      setDisclosureConfirmed(false);
      setRoleWriteState("idle");
      announce("복사본을 만들고 편집 대상으로 선택했습니다. 원본 MAGI 역할은 변경되지 않습니다.");
    } catch {
      announce("역할 복사본을 만들거나 활성 프로필로 선택하지 못했습니다.");
    }
  }, [announce, rolePresets]);

  const saveRolePresetDraft = useCallback(async (input: { presetId: string | null; expectedRevision: number | null; draft: RolePresetDraft }) => {
    setRoleWriteState("saving");
    try {
      const saved = await saveRolePreset(roleDraftInput({
        ...input.draft,
        presetId: input.presetId,
        expectedRevision: input.expectedRevision,
      }));
      const presets = (await listRolePresets()).map(mapStoredRolePreset);
      setRolePresets(presets);
      const nextDraft = toRolePresetDraft(mapStoredRolePreset(saved));
      setRoleDraft(nextDraft);
      setDisclosureConfirmed(false);
      if (input.presetId === null) {
        const selection = await setActiveRolePreset(saved.presetId, roleSelectionRevision.current ?? 0);
        roleSelectionRevision.current = selection.selectionRevision;
        setSelectedRolePresetId(selection.presetId);
      }
      setRoleWriteState("saved");
      announce("새 역할 프로필 revision을 저장했습니다. 이미 시작된 심의 내용은 바뀌지 않습니다.");
    } catch (error) {
      const message = String(error).toLowerCase();
      setRoleWriteState(message.includes("revision") || message.includes("conflict") ? "conflict" : "error");
      void listRolePresets().then((items) => setRolePresets(items.map(mapStoredRolePreset))).catch(() => setRolePresetsState("error"));
      announce("역할 프로필을 저장하지 못했습니다. 입력 내용은 유지했습니다.");
    }
  }, [announce]);

  const displayPreferences = useCallback((snapshot: SettingsSnapshot) => {
    if (preferenceAuthority.current && snapshot.revision < preferenceAuthority.current.revision) return;
    preferenceAuthority.current = snapshot;
    setCommonContextBudgetRevision(snapshot.fieldRevisions.commonContextTokenLimit);
    const resolved = { ...snapshot.preferences };
    for (const field of changedPreferenceFields.current) Object.assign(resolved, { [field]: preferencesRef.current[field] });
    preferencesRef.current = resolved;
    setMotion(resolved.motion); setSound(resolved.sound); setTheme(resolved.theme); setFontScale(resolved.fontScale); setLocale(resolved.language); setCommonContextTokenLimit(resolved.commonContextTokenLimit);
  }, []);

  const queuePreferenceSave = useCallback((preferences: ConsolePreferences, onlyField?: PreferenceField) => {
    const baseline = preferenceAuthority.current;
    if (!baseline) return;
    const fields = onlyField ? [onlyField] : [...changedPreferenceFields.current];
    if (!fields.length) return;
    const patch: Partial<ConsolePreferences> = {};
    const generations = { ...preferenceEditGenerations.current };
    const dependencies = fields.map(field => ({ field, revision: baseline.fieldRevisions[field], predecessor: preferencePredecessors.current[field] }));
    for (const field of fields) Object.assign(patch, { [field]: preferences[field] });
    let issued: SettingsCommand | null = null;
    let operation: PreferenceOperation;
    const commit = async (): Promise<SettingsReceipt> => {
      if (!issued) {
        const expectedFieldRevisions: SettingsCommand["expectedFieldRevisions"] = {};
        for (const dependency of dependencies) {
          let expected = dependency.revision;
          if (dependency.predecessor && dependency.predecessor.state !== "conflict") {
            const previous = dependency.predecessor.state === "unresolved" ? await dependency.predecessor.retry() : await dependency.predecessor.promise;
            expected = previous.snapshot.fieldRevisions[dependency.field];
          }
          if ((preferenceAuthority.current?.fieldRevisions[dependency.field] ?? -1) > expected) throw new Error("Console preference field revision conflict.");
          expectedFieldRevisions[dependency.field] = expected;
        }
        issued = { schemaVersion: 1, commandId: crypto.randomUUID(), idempotencyKey: crypto.randomUUID(), target: "console_preferences", patch, expectedFieldRevisions };
      }
      let result;
      try { result = await saveConsolePreferences(issued); }
      catch (error) {
        if (String(error).includes("revision conflict") || String(error).includes("identity conflict")) throw error;
        result = await saveConsolePreferences(issued);
      }
      operation.state = "committed";
      for (const field of fields) if (preferenceEditGenerations.current[field] === generations[field]) changedPreferenceFields.current.delete(field);
      displayPreferences(result.receipt.snapshot);
      if (result.notification.state === "pending") announce("설정은 저장했습니다. 다른 창의 알림 전달은 대기 중입니다.");
      return result.receipt;
    };
    const retry = async () => {
      try { return await commit(); }
      catch (error) { operation.state = String(error).includes("conflict") ? "conflict" : "unresolved"; throw error; }
    };
    operation = { state: "pending", retry, promise: preferenceSaveQueue.current.catch(() => undefined).then(retry) };
    for (const field of fields) preferencePredecessors.current[field] = operation;
    preferenceSaveQueue.current = operation.promise.then(() => undefined).catch(async () => {
      announce("콘솔 설정을 저장하지 못했습니다. 입력은 유지했습니다. 충돌한 설정은 다시 확인해 주세요.");
      await reloadPreferences.current?.();
    });
  }, [announce, displayPreferences]);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    async function reload() {
      const generation = ++preferenceReadGeneration.current;
      const initial = !preferencesReady.current;
      try {
        const snapshot = await getConsolePreferences(preferenceDefaults);
        if (disposed || generation !== preferenceReadGeneration.current || snapshot === null) return;
        displayPreferences(snapshot);
        preferencesReady.current = true;
        if (initial && changedPreferenceFields.current.size > 0) queuePreferenceSave(preferencesRef.current);
      } catch {
        if (disposed || generation !== preferenceReadGeneration.current) return;
        announce("저장된 콘솔 설정을 읽지 못했습니다. 현재 화면의 선택은 유지됩니다.");
      }
    }
    reloadPreferences.current = reload;
    void (async () => {
      try { const stop = await listenShellEvent("magi:preferences-changed", () => { if (!disposed) void reload(); }); if (disposed) stop(); else unlisten = stop; }
      catch { if (!disposed) announce("다른 창의 설정 변경을 확인하지 못했습니다."); }
      if (!disposed) await reload();
    })();
    return () => { disposed = true; preferenceReadGeneration.current++; if (reloadPreferences.current === reload) reloadPreferences.current = null; unlisten?.(); };
  }, [announce, displayPreferences, preferenceDefaults, queuePreferenceSave]);

  const changeMotion = useCallback((value: MotionSetting) => {
    changedPreferenceFields.current.add("motion"); preferenceEditGenerations.current.motion++;
    const next = { ...preferencesRef.current, motion: value }; preferencesRef.current = next; setMotion(value);
    if (preferencesReady.current) queuePreferenceSave(next, "motion");
  }, [queuePreferenceSave]);
  const changeSound = useCallback((value: boolean) => {
    const wasEnabled = preferencesRef.current.sound;
    changedPreferenceFields.current.add("sound"); preferenceEditGenerations.current.sound++;
    const next = { ...preferencesRef.current, sound: value }; preferencesRef.current = next; setSound(value);
    if (value && !wasEnabled) playSoundPreview();
    if (preferencesReady.current) queuePreferenceSave(next, "sound");
  }, [queuePreferenceSave]);
  const changeTheme = useCallback((value: ThemeSetting) => {
    changedPreferenceFields.current.add("theme"); preferenceEditGenerations.current.theme++;
    const next = { ...preferencesRef.current, theme: value }; preferencesRef.current = next; setTheme(value);
    if (preferencesReady.current) queuePreferenceSave(next, "theme");
  }, [queuePreferenceSave]);
  const changeFontScale = useCallback((value: ConsolePreferences["fontScale"]) => {
    changedPreferenceFields.current.add("fontScale"); preferenceEditGenerations.current.fontScale++;
    const next = { ...preferencesRef.current, fontScale: value }; preferencesRef.current = next; setFontScale(value);
    if (preferencesReady.current) queuePreferenceSave(next, "fontScale");
  }, [queuePreferenceSave]);

  const changeCommonContextTokenLimit = useCallback((value: number) => {
    if (!Number.isSafeInteger(value) || value < 1 || value > 128000) return;
    if (value === preferencesRef.current.commonContextTokenLimit) {
      const pending = preferencePredecessors.current.commonContextTokenLimit;
      if (pending?.state === "unresolved") {
        pending.state = "pending";
        pending.promise = preferenceSaveQueue.current.catch(() => undefined).then(() => pending.retry());
        preferenceSaveQueue.current = pending.promise.then(() => undefined).catch(async () => {
          announce("콘솔 설정을 저장하지 못했습니다. 입력은 유지했습니다. 충돌한 설정은 다시 확인해 주세요.");
          await reloadPreferences.current?.();
        });
      }
      return;
    }
    changedPreferenceFields.current.add("commonContextTokenLimit"); preferenceEditGenerations.current.commonContextTokenLimit++;
    const next = { ...preferencesRef.current, commonContextTokenLimit: value }; preferencesRef.current = next; setCommonContextTokenLimit(value);
    if (preferencesReady.current) queuePreferenceSave(next, "commonContextTokenLimit");
  }, [announce, queuePreferenceSave]);

  const changeLocale = useCallback((value: Locale) => {
    changedPreferenceFields.current.add("language"); preferenceEditGenerations.current.language++;
    const next = { ...preferencesRef.current, language: value }; preferencesRef.current = next; setLocale(value);
    if (preferencesReady.current) queuePreferenceSave(next, "language");
  }, [queuePreferenceSave]);
  useEffect(() => { document.documentElement.lang = locale; }, [locale]);

  const confirmQuestion = () => {
    if (!question.trim()) {
      setInvalidQuestion(true);
      document.getElementById("question-draft")?.focus();
      announce("심의할 질문을 입력해 주세요.");
      return;
    }
    setInvalidQuestion(false);
    navigate("confirmation");
  };

  const sendAdmissionCancellation = async (flow: NonNullable<typeof admissionFlow.current>): Promise<void> => {
    if (!flow.cancellation || flow.receipt) return;
    if (flow.cancellationInFlight) {
      const pending = flow.cancellationInFlight;
      await pending;
      if (flow.receipt) return;
      if (flow.cancellationInFlight === pending) flow.cancellationInFlight = null;
      if (flow.cancellationInFlight) return sendAdmissionCancellation(flow);
    }
    const authority = flow.authority;
    const input = { ...flow.cancellation, ...(authority ? { admissionAuthority: authority } : {}) };
    const operation = (async () => {
      try {
        const receipt = flow.clarification ? await cancelClarificationRequest({ ...input, request: flow.clarification }) : await cancelDeliberationRequest(input);
        const fence = receipt.requestCancellation;
        if (fence.schemaVersion !== 1 || fence.commandId !== input.commandId || fence.idempotencyKey !== input.idempotencyKey
          || fence.request.commandId !== flow.request.commandId || fence.request.idempotencyKey !== flow.request.idempotencyKey
          || !/^[0-9a-f]{64}$/i.test(fence.request.intentDigest) || !Number.isFinite(Date.parse(fence.acceptedAt))
          || (receipt.runCancellation && receipt.runCancellation.runId !== fence.admittedRunId)) {
          throw new Error("Cancellation receipt does not match the original request.");
        }
        flow.receipt = receipt;
        if (admissionFlow.current !== flow) return;
        setAdmissionCancellation("accepted");
        setRunStartState("idle");
        announce("취소 의도가 저장되었습니다. 이미 시작된 심의의 종료 여부는 저장 상태로 확인합니다.");
        if (receipt.runCancellation) setCancelRequest({ runId: receipt.runCancellation.runId, state: "sent" });
      } catch {
        if (admissionFlow.current !== flow) return;
        setAdmissionCancellation(flow.registrationPending ? "pending" : "error");
        announce(flow.registrationPending ? "취소 의도를 유지하고 있습니다. 등록 결과와 실제 저장 응답을 기다립니다." : "취소 저장 응답을 확인하지 못했습니다. 같은 요청으로 다시 확인하십시오.");
      }
    })();
    flow.cancellationInFlight = operation;
    await operation;
    if (flow.cancellationInFlight === operation) flow.cancellationInFlight = null;
  };

  const requestAdmissionCancellation = () => {
    const flow = admissionFlow.current;
    if (!flow || flow.receipt) return;
    setDialog({
      title: "심의 시작 요청 취소",
      body: "이 시작 요청의 취소 의도를 저장합니다. 이미 시작된 코어 작업의 중단 여부는 실제 실행 상태로 확인합니다.",
      confirm: { label: "취소 의도 저장", danger: true, action: () => {
        if (admissionFlow.current !== flow) return;
        flow.cancellation ??= { request: flow.request, commandId: crypto.randomUUID(), idempotencyKey: crypto.randomUUID() };
        setAdmissionCancellation("pending");
        void sendAdmissionCancellation(flow);
      } },
    });
  };

  const askToStart = async () => {
    if (runStartState === "starting" || admissionFlow.current?.inFlight) return;
    if (admissionFlow.current?.cancellation && !admissionFlow.current.receipt) {
      setRunStartError("원래 요청의 취소 저장 응답을 먼저 확인하십시오. 취소 의도를 새 요청으로 덮어쓰지 않습니다.");
      return;
    }
    if (clarificationDraft && !currentClarificationDraft) {
      setRunStartError("자식 초안의 정확한 일시정지 부모 상태를 다시 확인하십시오.");
      return;
    }
    if (currentClarificationDraft && (question !== currentClarificationDraft.question || contextSelection?.draftId !== currentClarificationDraft.context.draftId || contextSelection.revision !== currentClarificationDraft.context.revision)) {
      setRunStartError("자식 질문과 자료의 같은 저장 revision을 다시 확인하십시오.");
      return;
    }
    if (!question.trim()) {
      setRunStartError("심의할 질문을 입력하십시오.");
      setRunStartState("error");
      return;
    }
    if (!disclosureConfirmed) {
      setRunStartError("질문·역할·선택 자료를 선택한 프로필로 보내는 동의를 확인하십시오.");
      setRunStartState("error");
      return;
    }
    if (!coreBindings.canStart()) {
      setRunStartError("세 코어의 저장된 프로필·모델 연결을 확인하십시오.");
      setRunStartState("error");
      navigate("connections");
      return;
    }
    if (!selectedRolePreset || rolePresetsState !== "ready") {
      setRunStartError("사용할 역할 프리셋을 저장소에서 확인하십시오.");
      setRunStartState("error");
      navigate("roles");
      return;
    }
    const currentRunTerminal = currentRunDossier && ["completed", "cancelled", "failed"].includes(currentRunDossier.status);
    const existingRunActive = Boolean(
      (activeRun && !["completed", "cancelled", "failed"].includes(activeRun.status))
        || (currentRunId && !currentRunTerminal),
    );
    if (existingRunActive && !currentClarificationDraft) {
      setRunStartError("이미 진행 중인 심의가 있습니다. 해당 심의 상태를 확인하십시오.");
      setRunStartState("error");
      return;
    }
    if (snapshot.storage !== "ready") {
      setRunStartError("로컬 저장소 준비 상태가 확인되지 않아 실행을 차단했습니다.");
      setRunStartState("error");
      return;
    }

    if (!preferencesReady.current || !preferenceAuthority.current) {
      setRunStartError("저장된 콘솔 설정을 확인한 뒤 다시 시작하십시오.");
      return;
    }
    const budgetRevision = preferenceAuthority.current.fieldRevisions.commonContextTokenLimit;
    const bindings = coreBindings.destinations.flatMap(item => item.reference ? [item.reference] : []);
    const previousCommand = pendingDeliberationCommand.current;
    const reusableCommand = previousCommand
      && previousCommand.question === question
      && previousCommand.contextDraftId === (contextSelection?.draftId ?? null)
      && previousCommand.contextRevision === (contextSelection?.revision ?? null)
      && previousCommand.rolePresetId === selectedRolePreset.id
      && previousCommand.roleRevision === selectedRolePreset.revision
      && JSON.stringify(previousCommand.coreBindings) === JSON.stringify(bindings);
    const command: StartDeliberationInput = reusableCommand ? previousCommand : {
      commandId: crypto.randomUUID(), idempotencyKey: crypto.randomUUID(), question,
      contextDraftId: contextSelection?.draftId ?? null, contextRevision: contextSelection?.revision ?? null,
      rolePresetId: selectedRolePreset.id, roleRevision: selectedRolePreset.revision,
      coreBindings: bindings, disclosureConfirmed: true,
      expectedCommonContextBudgetRevision: budgetRevision,
    };
    if (admissionFlow.current?.request === command && admissionFlow.current.cancellation) {
      setRunStartError("이 시작 요청의 취소 의도가 유지되고 있습니다. 취소 응답과 저장 상태를 확인하십시오.");
      return;
    }
    pendingDeliberationCommand.current = command;
    const clarification: ClarificationStartInput | null = currentClarificationDraft ? { parent: currentClarificationDraft.parent, contextDraftId: currentClarificationDraft.context.draftId, contextDraftRevision: currentClarificationDraft.context.revision, request: command } : null;
    const flow = { request: command, clarification, authority: null as AdmissionAuthority | null, cancellation: null as CancelDeliberationRequestInput | null, receipt: null as CancelDeliberationRequestReceipt | null, cancellationInFlight: null as Promise<void> | null, registrationPending: true, inFlight: true };
    admissionFlow.current = flow;
    setAdmissionCancellation("idle");
    setRunStartState("starting");
    setRunStartError("");
    try {
      const registration = clarification ? await registerClarificationRequest(clarification) : await registerDeliberationRequest(command);
      flow.registrationPending = false;
      if (registration.kind === "registered") {
        flow.authority = { token: registration.admissionAuthority.token, processEpoch: registration.admissionAuthority.processEpoch };
      }
      if (admissionFlow.current !== flow) {
        flow.cancellation ??= { request: flow.request, commandId: crypto.randomUUID(), idempotencyKey: crypto.randomUUID() };
        await sendAdmissionCancellation(flow);
        return;
      }
      if (flow.cancellation) {
        await sendAdmissionCancellation(flow);
        if (registration.kind !== "replayed") return;
      }
      if (admissionFlow.current !== flow) return;
      const started = registration.kind === "replayed" ? registration.receipt
        : clarification ? await startClarification(clarification, flow.authority!) : await startDeliberation({ ...command, admissionAuthority: flow.authority! });
      if (flow.cancellation) await sendAdmissionCancellation(flow);
      if (admissionFlow.current !== flow) return;
      if (!flow.cancellation) pendingDeliberationCommand.current = null;
      activeRunIdRef.current = started.runId;
      selectedRunIdRef.current = started.runId;
      if (clarification) setClarificationDraft(null);
      setAcceptedRunId(started.runId);
      setSelectedRunId(started.runId);
      setRunProgress(null);
      setRunDossier(null);
      setRunDossierState("loading");
      if (!flow.cancellation) setCancelRequest(null);
      setRunStartState("idle");
      setDisclosureConfirmed(false);
      announce(flow.cancellation ? "실제 실행 기록이 확인되었습니다. 취소 저장 응답과 종료 상태를 확인합니다." : "심의 시작 요청이 접수되었습니다. 실제 실행 단계 이벤트를 기다립니다.");
    } catch (error) {
      flow.registrationPending = false;
      if (admissionFlow.current !== flow) return;
      if (flow.cancellation) {
        await sendAdmissionCancellation(flow);
        if (admissionFlow.current !== flow) return;
        if (!flow.receipt) setRunStartState("error");
        setRunStartError(flow.receipt ? "취소 의도가 저장되어 이 시작 요청은 다시 실행하지 않습니다." : "취소 의도를 유지하고 있습니다. 취소 저장 응답을 다시 확인하십시오.");
        return;
      }
      const binding = authenticationFailureBinding(error);
      if (binding) coreBindings.invalidateAuthentication(binding);
      setRunStartState("error");
      const code = normalizeLiveRunError(error).code;
      const authorityErrors: Record<string, string> = {
        admission_authority_expired: "시작 권한이 만료되었습니다. 원래 요청으로 등록 결과를 다시 확인하십시오.",
        admission_authority_invalid: "시작 권한과 원래 요청이 일치하지 않습니다. 실행하지 않고 요청을 유지합니다.",
        admission_request_cancelled: "취소 의도가 저장된 요청입니다. 이 요청은 다시 실행하지 않습니다.",
      };
      setRunStartError(authorityErrors[code] ?? "심의 시작 요청이 거부되었습니다. 연결·역할·입력 revision을 다시 확인하십시오.");
    } finally {
      flow.registrationPending = false;
      flow.inFlight = false;
    }
  };

  const cancelRun = async (runId: string) => {
    if (cancelRequest?.runId === runId && cancelRequest.state !== "error") return;
    if (runDossier?.runId === runId && ["completed", "failed", "cancelled"].includes(runDossier.status)) {
      announce("이미 종료된 심의는 취소할 수 없습니다.");
      return;
    }
    setCancelRequest({ runId, state: "sending" });
    try {
      await cancelDeliberation(runId);
      setCancelRequest({ runId, state: "sent" });
      announce("취소 요청을 보냈습니다. 완료 여부는 저장된 실행 상태로 확인합니다.");
    } catch {
      setCancelRequest({ runId, state: "error", message: "취소 요청을 전달하지 못했습니다. 현재 심의는 계속 상태 확인 중입니다." });
    }
  };

  const refreshRunStatus = async (runId: string) => {
    const refreshCurrent = runId === currentRunId;
    const request = dossierFence.current.begin(runId);
    if (refreshCurrent) setRunDossierState("loading");
    if (selectedRunIdRef.current === runId) setSelectedRunDossierState("loading");
    try {
      const dossier = await loadRunDossier(runId);
      if (dossier.runId !== runId) throw new Error("Run dossier mismatch.");
      if (!publishDossier(dossier, request)) return;
      try {
        const recentRuns = await listRecentRuns();
        setHomeData({
          recordsState: "ready",
          recentRuns: recentRuns.map((item) => ({ id: item.runId, question: item.question, status: item.status, createdAt: item.createdAt })),
        });
      } catch {
        setHomeData((current) => ({ ...current, recordsState: "error" }));
      }
      if (activeRunIdRef.current === runId && ["completed", "failed", "cancelled"].includes(dossier.status)) {
        try {
          const nextSnapshot = await getConsoleSnapshot();
          if (activeRunIdRef.current === runId && dossierFence.current.isCurrent(runId, request) && (nextSnapshot.eventSequence ?? -1) >= lastEventSequence.current) { lastEventSequence.current = nextSnapshot.eventSequence ?? -1; setSnapshot(nextSnapshot); }
        } catch {
          announce("심의 결과는 확인했지만 앱 상태 스냅샷을 새로 읽지 못했습니다.");
        }
      }
    } catch {
      if (dossierFence.current.isCurrent(runId, request)) {
        if (dispatchRun.current === runId) setRunDossierState("error");
        if (selectedRunIdRef.current === runId) setSelectedRunDossierState("error");
      }
      announce("저장된 심의 상태를 다시 읽지 못했습니다.");
    }
  };

  const openCore = (core: string) => {
    setDialog({
      title: `${core} · ${screen === "input" ? "관점" : "코어 정보"}`,
      body: currentDispatchProjection ? currentDispatchProjection.coreDispatches.filter(slot => slot.bindingCoreId.replace("-", "·") === core || slot.bindingCoreId === core).map(slot => `${slot.slotOrdinal} · ${slot.stage} · ${slot.state}${slot.resultRef ? ` · ${slot.resultRef}` : ""}`).join("\n") + (currentRunDossier?.assessments?.filter(assessment => assessment.coreId.replace("-", "·") === core || assessment.coreId === core).map(assessment => `\n${assessment.positionSummary}`).join("") ?? "") || t("현재 코어의 저장된 요청이 없습니다.") : t("저장된 코어 요청 상태를 확인하지 못했습니다."),
    });
  };

  const toggleSound = () => {
    changeSound(!sound);
    announce(sound ? "콘솔 음향을 껐습니다." : "콘솔 음향을 켰습니다. 짧은 미리보기만 재생합니다.");
  };

  const acceptPdfCapture = (next: ContextSelectionSummary) => {
    setContextSelection(next);
    setDisclosureConfirmed(false);
    announce("선택한 PDF 페이지를 접수했습니다. 모델 전송은 시작되지 않았습니다.");
  };

  const chooseContextFiles = async () => {
    try {
      const request = contextSelection
        ? { draftId: contextSelection.draftId, expectedRevision: contextSelection.revision }
        : undefined;
      const next = await selectContextFiles(request);
      if (!next) return;
      setContextSelection(next);
      setDisclosureConfirmed(false);
      const captured = next.sources.filter((source) => source.status === "captured").length;
      const excluded = next.sources.length - captured;
      announce(`선택 자료 ${next.sources.length}개 · 접수 ${captured}개${excluded ? ` · 제외 또는 실패 ${excluded}개` : ""}. 모델 전송은 시작되지 않았습니다.`);
    } catch {
      announce("파일을 접수하지 못했습니다. 현재 입력과 이전 선택은 유지됩니다.");
    }
  };

  const chooseContextDirectory = async () => {
    try {
      const request = contextSelection
        ? { draftId: contextSelection.draftId, expectedRevision: contextSelection.revision }
        : undefined;
      const next = await selectContextDirectory(request);
      if (!next) return;
      setContextSelection(next);
      setDisclosureConfirmed(false);
      const captured = next.sources.filter((source) => source.status === "captured").length;
      const excluded = next.sources.length - captured;
      announce(`폴더 접수 요약 · 허용 파일 ${captured}개${excluded ? ` · 제외 또는 실패 ${excluded}개` : ""}. 모델 전송은 시작되지 않았습니다.`);
    } catch {
      announce("폴더를 접수하지 못했습니다. 현재 입력과 이전 선택은 유지됩니다.");
    }
  };

  const openCompanionConsole = () => {
    void openConsoleFromCompanion().catch(() => announce("콘솔 창을 열지 못했습니다."));
  };
  const openCompanionSettings = () => {
    void openSettingsFromCompanion().catch(() => announce("설정 화면을 열지 못했습니다."));
  };
  const closeStatusCompanion = useCallback(() => {
    void closeCompanion().catch(() => announce("상태 팝오버를 닫지 못했습니다."));
  }, [announce]);
  const requestExitConfirmation = useCallback(() => {
    setDialog({
      title: "MAGI CONSOLE 종료",
      body: activeRun ? `현재 안건 “${activeRun.question}”이 ${activeRun.stage} 상태입니다. 앱 종료를 확인해 주세요.` : "MAGI CONSOLE을 종료하시겠습니까?",
      confirm: {
        label: "앱 종료",
        danger: true,
        action: () => { void confirmApplicationExit().catch(() => announce("앱 종료 요청을 완료하지 못했습니다.")); },
      },
    });
  }, [activeRun?.question, activeRun?.stage, announce]);

  useEffect(() => {
    if (shellContext?.windowLabel !== "main") return;
    let disposed = false;
    let settingsUnlisten: (() => void) | undefined;
    let exitUnlisten: (() => void) | undefined;
    const cleanup = () => {
      const listeners = [settingsUnlisten, exitUnlisten];
      settingsUnlisten = undefined;
      exitUnlisten = undefined;
      listeners.forEach((unlisten) => unlisten?.());
    };
    void (async () => {
      try {
        settingsUnlisten = await listenShellEvent("magi:open-settings", () => navigate("settings"));
        if (disposed) {
          cleanup();
          return;
        }
        exitUnlisten = await listenShellEvent("magi:exit-requested", requestExitConfirmation);
        if (disposed) cleanup();
      } catch {
        cleanup();
        if (!disposed) announce("macOS 메뉴 이벤트를 연결하지 못했습니다.");
      }
    })();
    return () => {
      disposed = true;
      cleanup();
    };
  }, [shellContext?.windowLabel, navigate, requestExitConfirmation, announce]);

  useEffect(() => {
    if (shellContext?.windowLabel !== "companion") return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        closeStatusCompanion();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [shellContext?.windowLabel, closeStatusCompanion]);

  const connectionLabel = snapshot.connection === "runtime_available"
    ? "실행 환경 확인됨"
    : snapshot.connection === "blocked"
      ? "실행 환경 차단됨"
      : "실행 환경 미확인";
  const storageLabel = snapshot.storage === "ready"
    ? "로컬 기록 준비됨"
    : snapshot.storage === "error"
      ? "로컬 저장 오류"
      : "로컬 기록 미확인";
  const footerStage = currentRunProgress?.stage ?? activeRun?.stage ?? "안건 대기";

  if (shellContext?.windowLabel === "companion") {
    return <CompanionSurface snapshot={presentedSnapshot} runId={currentRunId} progress={currentRunProgress} dossier={currentRunDossier} cancelRequest={cancelRequest} onCancelRun={cancelRun} onRefreshRunStatus={refreshRunStatus} onOpenConsole={openCompanionConsole} onOpenSettings={openCompanionSettings} onClose={closeStatusCompanion} onRequestExit={requestExitConfirmation} />;
  }

  return (
    <div className="console-shell" data-theme={theme}>
      <a className="skip-link" href="#screen-content">{t("본문으로 건너뛰기")}</a>
      <header className="console-header">
        <h1 className="console-title">
          <button type="button" className="console-brand-link" aria-label={t("MAGI COMMAND CONSOLE 홈으로 이동")} onClick={() => navigate("home")}>
            <span className="title-long">MAGI COMMAND CONSOLE</span>
            <span className="title-short">MAGI CONSOLE</span>
          </button>
        </h1>
        <div className="console-edition" aria-label={t("팬 창작 데스크톱 앱")}>
          <span>MACOS</span>
          <span className="edition-divider" aria-hidden="true" />
          <span>LOCAL</span>
        </div>
        <nav className="header-actions" aria-label={t("콘솔 도구")}>
          <button type="button" onClick={() => navigate("history")}>{t("기록")}</button>
          <button type="button" onClick={() => navigate("connections")}>{t("모델 연결")}</button>
          <button type="button" onClick={() => navigate("settings")}>{t("설정")}</button>
          <button type="button" aria-pressed={sound} onClick={toggleSound}>
            {sound ? t("소리 켬") : t("소리 끔")}
          </button>
          <button type="button" aria-pressed={motion !== "full"} onClick={() => changeMotion(motion === "full" ? "reduced" : "full")}>
            {motion === "full" ? t("동작 켬") : motion === "reduced" ? t("동작 줄임") : t("동작 끔")}
          </button>
        </nav>
      </header>

      {screen !== "home" && <section className={`agenda ${invalidQuestion ? "agenda-invalid" : ""}`} aria-label={t("현재 안건")}>
        <div className="agenda-marker">
          <strong>{t("안건")}</strong>
          <span>AGENDA</span>
        </div>
        <div className="agenda-input-wrap">
          <label className="sr-only" htmlFor="question-draft">{t("심의할 질문")}</label>
          <textarea
            id="question-draft"
            rows={2}
            value={shownQuestion}
            readOnly={!isDraft}
            placeholder={isDraft ? t("결정이 필요한 질문을 입력하십시오…") : t("안건 대기")}
            aria-describedby={invalidQuestion ? "question-error" : "question-help"}
            aria-invalid={invalidQuestion}
            onChange={(event) => { setQuestion(event.target.value); setDisclosureConfirmed(false); setRunStartState("idle"); setRunStartError(""); setInvalidQuestion(false); }}
            onKeyDown={(event) => {
              if (event.nativeEvent.isComposing || event.nativeEvent.keyCode === 229) return;
              if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
                event.preventDefault();
                confirmQuestion();
              }
            }}
          />
          <p className="sr-only" id={invalidQuestion ? "question-error" : "question-help"}>
            {invalidQuestion ? t("심의할 질문을 입력해야 합니다.") : t("Enter는 줄바꿈입니다. Command와 Enter를 함께 눌러 입력 확인으로 이동합니다.")}
          </p>
        </div>
        <div className="agenda-actions">
          <div className="agenda-source-status">
            <span className="source-symbol" aria-hidden="true">▤</span>
            <span>{activeRun && !retainedDraftIdentity ? `자료 ${activeRun.sourceCount}개` : contextSelection ? `자료 ${contextSelection.sources.filter((source) => source.status === "captured").length}개 접수` : t("자료 0개")}</span>
            <button type="button" className="text-action" onClick={() => navigate("intake")}>{t("자료 보기")}</button>
          </div>
          <button type="button" className="button button-primary agenda-primary" onClick={confirmQuestion} disabled={!isDraft}>
            {isDraft || retainedDraftIdentity ? t("입력 확인") : activeRun && !["completed", "cancelled", "failed"].includes(activeRun.status) ? t("진행 중") : t("새 안건")}
            <span aria-hidden="true">↗</span>
          </button>
        </div>
      </section>}

      <main id="screen-content" className="screen-content" ref={mainRef} tabIndex={-1} aria-labelledby="screen-title">
        <ScreenSurface
          screen={screen}
          question={shownQuestion}
          motion={motion}
          sound={sound}
          theme={theme}
          snapshot={presentedSnapshot}
          homeData={homeData}
          evidenceReadingTarget={evidenceReadingTarget}
          onEvidenceReadingTargetChange={setEvidenceReadingTarget}
          selectedRunId={selectedRunId}
          selectedRunDossier={selectedRunDossier}
          selectedRunDossierState={selectedRunDossierState}
          currentRunId={currentRunId}
          runProgress={currentRunProgress}
          onClarificationParentVerified={setVerifiedClarificationParent}
          onDiscardClarificationDraft={(action) => setDialog({ title: t("자식 초안 삭제"), body: t("저장된 자식 질문과 초안을 삭제합니다. 부모 심의와 검토는 유지됩니다. 작성 중인 변경도 버립니다."), confirm: { label: t("초안 삭제"), danger: true, action } })}
          clarificationDraft={currentClarificationDraft}
          onClarificationDraftChanged={(draft) => { setClarificationDraft(draft); if (draft) { setQuestion(draft.question); setContextSelection(draft.context); } else { setQuestion(currentRunDossier?.question ?? ""); setContextSelection(null); } setDisclosureConfirmed(false); }}
          onConfirmClarificationDraft={(draft) => { if (draft.parent.runId !== currentRunId || currentRunDossier?.status !== "paused") { setRunStartError(t("자식 초안의 부모 상태를 현재 저장 기록에서 확인하십시오.")); return; } setClarificationDraft(draft); setQuestion(draft.question); setContextSelection(draft.context); setDisclosureConfirmed(false); navigate("confirmation"); }}
          runDossier={currentRunDossier}
          runDossierState={runDossierState}
          runStartState={runStartState}
          admissionCancellation={admissionCancellation}
          onCancelAdmission={requestAdmissionCancellation}
          runStartError={runStartError}
          disclosureConfirmed={disclosureConfirmed}
          cancelRequest={cancelRequest}
          contextSelection={contextSelection}
          acpAdaptersState={acpAdaptersState}
          acpAdapters={acpAdapters}
          acpProfilesState={acpProfilesState}
          acpProfiles={acpProfiles}
          selectedAcpProfileId={selectedAcpProfileId}
          providerSourceScopes={providerSourceScopes}
          providerSourceScopesState={providerSourceScopesState}
          providerSourceScopePathInput={providerSourceScopePathInput}
          providerSourceScopeWriteState={providerSourceScopeWriteState}
          providerSourceScopeError={providerSourceScopeError}
          selectedProviderProfile={selectedProviderProfile ?? null}
          authenticatingProfileId={authenticatingProfileId}
          providerAuthProgress={providerAuthProgress}
          cancellingProviderAuth={cancellingProviderAuth}
          validatingProfile={validatingProfile}
          providerValidationError={providerValidationError}
          authProfileResult={selectedAuthProfileResult}
          authProfileError={authProfileError}
          authProfileErrorBinding={authProfileErrorBinding}
          liveCatalog={selectedLiveCatalog}
          liveCatalogState={liveCatalogState}
          liveCatalogError={liveCatalogError}
          coreBindings={coreBindings}
          selectedLiveModelId={selectedLiveModelId}
          liveQuestion={liveQuestion}
          liveRunId={liveRunId}
          liveRunSnapshot={currentLiveRunSnapshot}
          liveRunText={currentLiveRunText}
          liveSnapshotError={liveSnapshotError}
          liveRequestState={liveRequestState}
          liveRequestError={liveRequestError}
          liveCancelRequest={liveCancelRequest}
          acpProfileDraft={acpProfileDraft}
          acpProfileWriteState={acpProfileWriteState}
          rolePresetsState={rolePresetsState}
          roleStoreDiagnostic={roleStoreDiagnostic}
          rolePresets={rolePresets}
          selectedRolePresetId={selectedRolePresetId}
          selectedRolePreset={selectedRolePreset ?? null}
          roleDraft={roleDraft}
          roleWriteState={roleWriteState}
          companionWindow={false}
          onNavigate={navigate}
          onOpenCore={openCore}
          onOpenRecentRun={openRecentRun}
          onRecordDeleted={(runId) => {
            setEvidenceReadingTarget((target) => target?.runId === runId ? null : target);
            setHomeData((previous) => ({ ...previous, recentRuns: previous.recentRuns.filter((run) => run.id !== runId) }));
            if (selectedRunIdRef.current !== runId) return;
            selectedRunIdRef.current = null;
            setSelectedRunId(null);
            setSelectedRunDossier(null);
            setSelectedRunDossierState("idle");
            announce(t("선택한 기록을 삭제했습니다."));
          }}
          onBeginAcpProfileCreate={beginAcpProfileCreate}
          onBeginAcpProfileEdit={beginAcpProfileEdit}
          onAcpProfileDraftChange={setAcpProfileDraft}
          onSaveAcpProfile={(draft) => { void saveAcpProfileDraft(draft); }}
          onSelectAcpProfile={(profileId) => { void selectAcpProfile(profileId); }}
          onValidateAcpProfile={(profileId) => { void validateAcpProfile(profileId); }}
          onAuthenticateAcpProfile={(profileId) => { void authenticateAcpProfile(profileId); }}
          onCancelProviderAuth={(profileId, profileRevision) => { void cancelAcpAuthentication(profileId, profileRevision); }}
          onProviderSourceScopePathInputChange={setProviderSourceScopePathInput}
          onPickProviderSourceDirectory={() => { void chooseProviderSourceDirectory(); }}
          onAddProviderSourceScope={() => { void addProviderSourceRoot(); }}
          onRevokeProviderSourceScope={(grantId) => { void revokeProviderSourceRoot(grantId); }}
          onRefreshLiveCatalog={() => { void refreshLiveCatalog(); }}
          onSelectedLiveModelIdChange={changeSelectedLiveModelId}
          onLiveQuestionChange={changeLiveQuestion}
          onStartLiveRun={() => { void startLiveProviderRun(); }}
          onCancelLiveRun={(runId) => { void cancelLiveProviderRun(runId); }}
          onSelectRolePreset={(presetId) => { void selectRolePreset(presetId); }}
          onRetryRolePresets={() => { void reloadRolePresets(); }}
          onCheckConnection={(profileId) => { void checkConnection(profileId); }}
          onReturnFromConnections={() => navigate(connectionReturnScreen.current, true)}
          onReturnFromSettings={() => navigate(settingsReturnScreen.current, true)}
          onReturnFromRoles={returnFromRoles}
          onBeginRolePresetEdit={beginRolePresetEdit}
          onCloneRolePreset={(presetId) => { void cloneRolePresetForEditing(presetId); }}
          onEditRoleDraft={setRoleDraft}
          onSaveRolePreset={(input) => { void saveRolePresetDraft(input); }}
          onStartRealRun={askToStart}
          onDisclosureConfirmedChange={setDisclosureConfirmed}
          onCancelRun={cancelRun}
          onRefreshRunStatus={refreshRunStatus}
          onNotice={announce}
          onMotionChange={changeMotion}
          onSoundChange={changeSound}
          onThemeChange={changeTheme}
          onSelectContextFiles={chooseContextFiles}
          onSelectContextDirectory={chooseContextDirectory}
          onPdfCaptured={acceptPdfCapture}
          locale={locale}
          onLocaleChange={changeLocale}
          commonContextBudgetRevision={commonContextBudgetRevision}
          commonContextTokenLimit={commonContextTokenLimit}
          onCommonContextTokenLimitChange={changeCommonContextTokenLimit}
          fontScale={fontScale}
          onFontScaleChange={changeFontScale}
          onOpenConsole={openCompanionConsole}
          onOpenSettings={openCompanionSettings}
          onCloseCompanion={closeStatusCompanion}
          onRequestExit={requestExitConfirmation}
        />
      </main>

      <footer className="console-footer">
        <span className="footer-system">MAGI SYSTEM <span aria-hidden="true">///</span></span>
        <span className="footer-rule" aria-hidden="true" />
        <span className="footer-value">{t(storageLabel)}</span>
        <span className="footer-stage">{t(footerStage)}</span>
        <button type="button" className="connection-state" onClick={() => navigate("connections")}>
          <span className={`status-dot status-${snapshot.connection}`} aria-hidden="true" />
          {t(connectionLabel)}
        </button>
      </footer>

      <dialog className="magi-dialog" ref={dialogRef} aria-labelledby="dialog-title" onCancel={() => setDialog(null)} onClose={() => setDialog(null)}>
        {dialog && (
          <>
            <div className="dialog-header">
              <span>MAGI / CONTROL</span>
              <button type="button" className="icon-button" aria-label={t("대화상자 닫기")} onClick={() => setDialog(null)}>×</button>
            </div>
            <h2 id="dialog-title">{dialog.title}</h2>
            <p className="dialog-copy">{dialog.body}</p>
            <div className="dialog-actions">
              <button type="button" className="button" onClick={() => setDialog(null)}>{t("닫기")}</button>
              {dialog.confirm && (
                <button type="button" className={`button ${dialog.confirm.danger ? "button-danger" : "button-primary"}`} onClick={() => { const action = dialog.confirm?.action; setDialog(null); action?.(); }}>
                  {dialog.confirm.label}
                </button>
              )}
            </div>
          </>
        )}
      </dialog>

      <div className="live-region sr-only" role="status" aria-live="polite" aria-atomic="true">{notice}</div>
    </div>
  );
}
