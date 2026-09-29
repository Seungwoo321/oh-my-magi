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
  loadActiveProviderProfileSelection,
  saveProviderProfile,
  setActiveProviderProfile,
  validateProviderProfile,
  authenticateProviderProfile,
  startDeliberation,
  cancelDeliberation,
  loadRunDossier,
  listenRunUpdate,
  listRolePresets,
  loadActiveRolePresetSelection,
  setActiveRolePreset,
  cloneRolePreset,
  saveRolePreset,
  listRecentRuns,
  isDesktopApp,
  selectContextFiles,
  selectContextDirectory,
  getConsolePreferences,
  saveConsolePreferences,
  type ConsolePreferences,
  type ConsoleSnapshot,
  type ContextSelectionSummary,
  type ProviderProfileSummary,
  type ProviderAdmission,
  type RunDossierView,
  type RunUpdate,
  type StoredRolePreset,
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

type SafeRunProgress = Pick<RunUpdate, "runId" | "stage" | "coreId" | "state">;

function mapProviderAdmission(admission: ProviderAdmission): AcpProfile["admission"] {
  if (admission.state === "ready") {
    return {
      state: "admitted",
      profileId: admission.profileId,
      adapterId: admission.adapterId,
      rootBinding: admission.rootBinding,
      checkedAt: admission.checkedAt,
      modelId: admission.modelId,
    };
  }
  if (admission.state === "needs_auth") {
    return {
      state: "needs_auth",
      profileId: admission.profileId,
      adapterId: admission.adapterId,
      rootBinding: admission.rootBinding,
      checkedAt: admission.checkedAt,
    };
  }
  if (admission.state === "blocked") {
    return { state: "blocked", reason: admission.reason, checkedAt: admission.checkedAt };
  }
  return { state: "not_checked" };
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
  const [screen, setScreen] = useState<ScreenId>("home");
  const [question, setQuestion] = useState("");
  const [preferenceDefaults] = useState(defaultConsolePreferences);
  const [motion, setMotion] = useState<MotionSetting>(preferenceDefaults.motion);
  const [sound, setSound] = useState(preferenceDefaults.sound);
  const [theme, setTheme] = useState<ThemeSetting>(preferenceDefaults.theme);
  const [fontScale, setFontScale] = useState<ConsolePreferences["fontScale"]>(preferenceDefaults.fontScale);
  const [snapshot, setSnapshot] = useState<ConsoleSnapshot>(emptySnapshot);
  const [contextSelection, setContextSelection] = useState<ContextSelectionSummary | null>(null);
  const [homeData, setHomeData] = useState<HomeData>({ recordsState: "loading", recentRuns: [] });
  const [selectedRunId, setSelectedRunId] = useState<string | null>(null);
  const [selectedRunDossier, setSelectedRunDossier] = useState<RunDossierView | null>(null);
  const [selectedRunDossierState, setSelectedRunDossierState] = useState<"idle" | "loading" | "ready" | "error">("idle");
  const [acpAdaptersState, setAcpAdaptersState] = useState<"loading" | "ready" | "unavailable" | "error">("loading");
  const [acpAdapters, setAcpAdapters] = useState<AcpAdapterSummary[]>([]);
  const [acpProfilesState, setAcpProfilesState] = useState<AcpProfileStoreState>("loading");
  const [acpProfiles, setAcpProfiles] = useState<AcpProfile[]>([]);
  const [selectedAcpProfileId, setSelectedAcpProfileId] = useState<string | null>(null);
  const [authenticatingProfileId, setAuthenticatingProfileId] = useState<string | null>(null);
  const [validatingProfileId, setValidatingProfileId] = useState<string | null>(null);
  const [acpProfileDraft, setAcpProfileDraft] = useState<AcpProfileDraft | null>(null);
  const [acpProfileWriteState, setAcpProfileWriteState] = useState<AcpProfileWriteState>("idle");
  const [rolePresetsState, setRolePresetsState] = useState<RolePresetStoreState>("loading");
  const [rolePresets, setRolePresets] = useState<RolePreset[]>([]);
  const [selectedRolePresetId, setSelectedRolePresetId] = useState<string | null>(null);
  const [disclosureConfirmed, setDisclosureConfirmed] = useState(false);
  const [acceptedRunId, setAcceptedRunId] = useState<string | null>(null);
  const [runProgress, setRunProgress] = useState<SafeRunProgress | null>(null);
  const [runDossier, setRunDossier] = useState<RunDossierView | null>(null);
  const [runDossierState, setRunDossierState] = useState<"idle" | "loading" | "ready" | "error">("idle");
  const [runStartState, setRunStartState] = useState<"idle" | "starting" | "error">("idle");
  const [runStartError, setRunStartError] = useState("");
  const [cancelRequest, setCancelRequest] = useState<{ runId: string; state: "sending" | "sent" | "error"; message?: string } | null>(null);
  const [roleDraft, setRoleDraft] = useState<RolePresetDraft | null>(null);
  const [roleWriteState, setRoleWriteState] = useState<RolePresetWriteState>("idle");
  const [shellContext, setShellContext] = useState<ShellContext | null>(null);
  const [dialog, setDialog] = useState<DialogModel | null>(null);
  const [notice, setNotice] = useState("");
  const [invalidQuestion, setInvalidQuestion] = useState(false);
  const mainRef = useRef<HTMLElement>(null);
  const dialogRef = useRef<HTMLDialogElement>(null);
  const dialogInvoker = useRef<HTMLElement | null>(null);
  const lastEventSequence = useRef(-1);
  const preferencesRef = useRef<ConsolePreferences>(preferenceDefaults);
  const changedPreferenceFields = useRef(new Set<keyof ConsolePreferences>());
  const preferencesReady = useRef(false);
  const preferenceSaveQueue = useRef<Promise<void>>(Promise.resolve());
  const providerSelectionRevision = useRef<number | null>(null);
  const roleSelectionRevision = useRef<number | null>(null);
  const activeRunIdRef = useRef<string | null>(null);
  const selectedRunIdRef = useRef<string | null>(null);

  const activeRun = snapshot.activeRun;
  const currentRunId = activeRun?.id ?? acceptedRunId;
  const currentRunProgress = runProgress?.runId === currentRunId ? runProgress : null;
  const currentRunDossier = runDossier?.runId === currentRunId ? runDossier : null;
  const selectedProviderProfile = acpProfiles.find((profile) => profile.id === selectedAcpProfileId);
  const selectedRolePreset = rolePresets.find((preset) => preset.id === selectedRolePresetId);
  const shownQuestion = activeRun?.question ?? question;
  const isDraft = screen === "input" && !activeRun;

  useEffect(() => {
    if (!selectedRunId) {
      setSelectedRunDossier(null);
      setSelectedRunDossierState("idle");
      return;
    }
    let disposed = false;
    setSelectedRunDossierState("loading");
    void loadRunDossier(selectedRunId).then((dossier) => {
      if (disposed || dossier.runId !== selectedRunId) return;
      setSelectedRunDossier(dossier);
      setSelectedRunDossierState("ready");
    }).catch(() => {
      if (!disposed) setSelectedRunDossierState("error");
    });
    return () => { disposed = true; };
  }, [selectedRunId]);

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

  const announce = useCallback((message: string) => {
    setNotice(message);
    window.setTimeout(() => setNotice(""), 5000);
  }, []);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    const runtimeScreens: ScreenId[] = ["confirmation", "independent", "review", "proposal", "sealed", "verdict", "paused", "interrupted", "cancelling", "cancelled", "failed"];
    const showRunDossier = (dossier: RunDossierView) => {
      setRunDossier(dossier);
      setRunDossierState("ready");
      const next = routeForDossier(dossier);
      if (next) setScreen((current) => runtimeScreens.includes(current) ? next : current);
    };
    const refreshRunState = async (runId: string) => {
      try {
        const dossier = await loadRunDossier(runId);
        if (disposed || activeRunIdRef.current !== runId || dossier.runId !== runId) return;
        showRunDossier(dossier);
        if (["completed", "failed", "cancelled"].includes(dossier.status)) {
          const [nextSnapshot, recentRuns] = await Promise.all([getConsoleSnapshot(), listRecentRuns()]);
          if (disposed || activeRunIdRef.current !== runId) return;
          setSnapshot(nextSnapshot);
          setHomeData({ recordsState: "ready", recentRuns: recentRuns.map((run) => ({ id: run.runId, question: run.question, status: run.status, createdAt: run.createdAt })) });
        }
      } catch {
        if (!disposed) setRunDossierState("error");
      }
    };
    void listenRunUpdate((update) => {
      if (!activeRunIdRef.current || activeRunIdRef.current !== update.runId) return;
      setAcceptedRunId(update.runId);
      setRunProgress({ runId: update.runId, stage: update.stage, coreId: update.coreId, state: update.state });
      if (update.result?.runId === update.runId) {
        showRunDossier(update.result);
      } else if (update.state === "failed" || update.state === "cancelled" || update.state === "completed") {
        void refreshRunState(update.runId);
      }
      if (!update.result && (update.state === "started" || update.state === "streaming")) {
        const next = runtimeRoutes[update.stage];
        if (next && !["verdict", "failed", "cancelled"].includes(next)) {
          setScreen((current) => runtimeScreens.includes(current) ? next : current);
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
  }, [announce]);

  useEffect(() => {
    let disposed = false;
    void getShellContext().then((context) => {
      if (!disposed && context) setShellContext(context);
    }).catch(() => undefined);
    return () => { disposed = true; };
  }, []);

  useEffect(() => {
    if (!isDesktopApp()) {
      setHomeData({ recordsState: "unavailable", recentRuns: [] });
      setAcpAdaptersState("unavailable");
      setAcpProfilesState("unavailable");
      setRolePresetsState("unavailable");
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

    void Promise.all([
      listRolePresets(),
      loadActiveRolePresetSelection(),
    ]).then(([storedPresets, selection]) => {
      if (disposed) return;
      const presets = storedPresets.map(mapStoredRolePreset);
      setRolePresets(presets);
      const fallback = presets.find((preset) => preset.id === "factory.magi.default") ?? presets.find((preset) => preset.kind === "factory");
      setSelectedRolePresetId(selection?.presetId ?? fallback?.id ?? null);
      roleSelectionRevision.current = selection?.selectionRevision ?? null;
      setRolePresetsState("ready");
    }).catch(() => {
      if (!disposed) setRolePresetsState("error");
    });

    return () => { disposed = true; };
  }, [shellContext?.windowLabel]);

  useEffect(() => {
    if (!activeRun) return;
    activeRunIdRef.current = activeRun.id;
    setAcceptedRunId(activeRun.id);
    const next = runtimeRoutes[activeRun.status];
    const runtimeScreens: ScreenId[] = ["input", "independent", "review", "proposal", "sealed", "verdict", "paused", "interrupted", "cancelling", "cancelled", "failed", "save-error", "confirmation"];
    if (next && runtimeScreens.includes(screen) && screen !== next) {
      setScreen(next);
    }
  }, [activeRun?.id, activeRun?.status, screen]);

  useEffect(() => {
    if (!currentRunId) {
      setRunDossier(null);
      setRunDossierState("idle");
      return;
    }
    let disposed = false;
    setRunDossierState("loading");
    void loadRunDossier(currentRunId).then((dossier) => {
      if (disposed || dossier.runId !== currentRunId) return;
      setRunDossier(dossier);
      setRunDossierState("ready");
      const next = routeForDossier(dossier);
      const runtimeScreens: ScreenId[] = ["confirmation", "independent", "review", "proposal", "sealed", "verdict", "paused", "interrupted", "cancelling", "cancelled", "failed"];
      if (next) setScreen((current) => runtimeScreens.includes(current) ? next : current);
    }).catch(() => {
      if (!disposed) setRunDossierState("error");
    });
    return () => { disposed = true; };
  }, [currentRunId]);

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

  const navigate = useCallback((next: ScreenId) => {
    setScreen(next);
    setInvalidQuestion(false);
    setNotice("");
    setDialog(null);
    setTimeout(() => mainRef.current?.focus(), 0);
  }, []);

  const openRecentRun = useCallback((runId: string) => {
    selectedRunIdRef.current = runId;
    setSelectedRunId(runId);
    navigate("history");
  }, [navigate]);

  const beginAcpProfileCreate = useCallback(() => {
    setAcpProfileDraft({ profileId: null, expectedRevision: null, displayName: "", adapterId: "codex-acp" });
    setAcpProfileWriteState("idle");
  }, []);

  const beginAcpProfileEdit = useCallback((profileId: string) => {
    const profile = acpProfiles.find((item) => item.id === profileId);
    if (!profile) {
      announce("편집할 연결 프로필이 최신 목록에 없습니다. 프로필 목록을 새로 읽습니다.");
      void listProviderProfiles().then((items) => setAcpProfiles(items.map(mapProviderProfile))).catch(() => setAcpProfilesState("error"));
      return;
    }
    setAcpProfileDraft({ profileId: profile.id, expectedRevision: profile.revision, displayName: profile.displayName, adapterId: profile.adapterId });
    setAcpProfileWriteState("idle");
  }, [acpProfiles, announce]);

  const saveAcpProfileDraft = useCallback(async (draft: AcpProfileDraft) => {
    setAcpProfileWriteState("saving");
    try {
      const saved = await saveProviderProfile(draft);
      const profiles = await listProviderProfiles();
      setAcpProfiles(profiles.map(mapProviderProfile));
      setAcpProfileDraft({ profileId: saved.providerProfileId, expectedRevision: saved.revision, displayName: saved.displayName, adapterId: saved.providerId });
      setAcpProfileWriteState("saved");
      setDisclosureConfirmed(false);
      announce("연결 프로필과 별도 홈 바인딩을 저장했습니다. 런타임 검증 전까지 모델 호출은 차단됩니다.");
    } catch (error) {
      const message = String(error).toLowerCase();
      setAcpProfileWriteState(message.includes("revision") || message.includes("conflict") ? "conflict" : "error");
      if (message.includes("revision") || message.includes("conflict")) {
        void listProviderProfiles().then((items) => setAcpProfiles(items.map(mapProviderProfile))).catch(() => setAcpProfilesState("error"));
      }
      announce("연결 프로필을 저장하지 못했습니다. 입력 내용을 유지했습니다.");
    }
  }, [announce]);

  const selectAcpProfile = useCallback(async (profileId: string) => {
    const profile = acpProfiles.find((item) => item.id === profileId);
    if (!profile) return;
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
    setValidatingProfileId(profileId);
    try {
      const result = await validateProviderProfile(profileId);
      const admission = mapProviderAdmission(result);
      setAcpProfiles((profiles) => profiles.map((profile) => profile.id === profileId ? { ...profile, admission } : profile));
      announce(result.state === "ready" ? "프로필 인증·홈 바인딩·모델 검증이 확인되었습니다." : result.state === "needs_auth" ? "공식 로그인이 필요합니다. 로그인 후 프로필을 다시 확인합니다." : "프로필 실행 자격이 차단되었습니다. 표시된 사유를 확인하십시오.");
    } catch {
      setAcpProfiles((profiles) => profiles.map((profile) => profile.id === profileId ? { ...profile, admission: { state: "not_checked" } } : profile));
      announce("프로필 검증 결과를 받지 못했습니다. 이전 검증을 사용하지 않고 실행을 차단했습니다.");
    } finally {
      setValidatingProfileId((current) => current === profileId ? null : current);
    }
  }, [announce]);

  const authenticateAcpProfile = useCallback(async (profileId: string) => {
    const profile = acpProfiles.find((item) => item.id === profileId);
    if (profile?.authenticationMethod !== "local_subscription") {
      announce("공식 ChatGPT 로그인은 구독 연결 프로필에서만 사용할 수 있습니다.");
      return;
    }
    setAuthenticatingProfileId(profileId);
    try {
      const result = await authenticateProviderProfile(profileId);
      const admission = mapProviderAdmission(result);
      setAcpProfiles((profiles) => profiles.map((profile) => profile.id === profileId ? { ...profile, admission } : profile));
      announce(result.state === "ready" ? "공식 로그인과 프로필 실행 검증이 완료되었습니다." : result.state === "needs_auth" ? "공식 로그인이 아직 확인되지 않았습니다. 연결은 차단 상태입니다." : "공식 인증 경로가 실행 자격을 확인하지 못했습니다. 표시된 사유를 확인하십시오.");
    } catch {
      setAcpProfiles((profiles) => profiles.map((item) => item.id === profileId ? { ...item, admission: { state: "not_checked" } } : item));
      announce("공식 로그인 결과를 확인하지 못했습니다. 이전 검증을 사용하지 않고 실행을 차단했습니다.");
    } finally {
      setAuthenticatingProfileId((current) => current === profileId ? null : current);
    }
  }, [acpProfiles, announce]);

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

  const queuePreferenceSave = useCallback((preferences: ConsolePreferences) => {
    preferenceSaveQueue.current = preferenceSaveQueue.current
      .catch(() => undefined)
      .then(() => saveConsolePreferences(preferences))
      .catch(() => announce("콘솔 설정을 저장하지 못했습니다. 현재 화면의 선택은 유지됩니다."));
  }, [announce]);

  useEffect(() => {
    let disposed = false;
    void getConsolePreferences().then((saved) => {
      if (disposed) return;
      const current = preferencesRef.current;
      const resolved: ConsolePreferences = { ...preferenceDefaults, ...saved };
      if (resolved.language !== "ko") changedPreferenceFields.current.add("language");
      resolved.language = "ko";
      if (changedPreferenceFields.current.has("motion")) resolved.motion = current.motion;
      if (changedPreferenceFields.current.has("sound")) resolved.sound = current.sound;
      if (changedPreferenceFields.current.has("theme")) resolved.theme = current.theme;
      if (changedPreferenceFields.current.has("fontScale")) resolved.fontScale = current.fontScale;
      preferencesRef.current = resolved;
      setMotion(resolved.motion);
      setSound(resolved.sound);
      setTheme(resolved.theme);
      setFontScale(resolved.fontScale);
      preferencesReady.current = true;
      if (changedPreferenceFields.current.size > 0 || saved?.language !== "ko") queuePreferenceSave(resolved);
    }).catch(() => {
      if (disposed) return;
      preferencesReady.current = true;
      announce("저장된 콘솔 설정을 읽지 못했습니다. OS 동작 설정과 기본 테마를 사용합니다.");
      if (changedPreferenceFields.current.size > 0) queuePreferenceSave(preferencesRef.current);
    });
    return () => { disposed = true; };
  }, [announce, preferenceDefaults, queuePreferenceSave]);

  const changeMotion = useCallback((value: MotionSetting) => {
    changedPreferenceFields.current.add("motion");
    const next = { ...preferencesRef.current, motion: value };
    preferencesRef.current = next;
    setMotion(value);
    if (preferencesReady.current) queuePreferenceSave(next);
  }, [queuePreferenceSave]);

  const changeSound = useCallback((value: boolean) => {
    const wasEnabled = preferencesRef.current.sound;
    changedPreferenceFields.current.add("sound");
    const next = { ...preferencesRef.current, sound: value };
    preferencesRef.current = next;
    setSound(value);
    if (value && !wasEnabled) playSoundPreview();
    if (preferencesReady.current) queuePreferenceSave(next);
  }, [queuePreferenceSave]);

  const changeTheme = useCallback((value: ThemeSetting) => {
    changedPreferenceFields.current.add("theme");
    const next = { ...preferencesRef.current, theme: value };
    preferencesRef.current = next;
    setTheme(value);
    if (preferencesReady.current) queuePreferenceSave(next);
  }, [queuePreferenceSave]);

  const changeFontScale = useCallback((value: ConsolePreferences["fontScale"]) => {
    changedPreferenceFields.current.add("fontScale");
    const next = { ...preferencesRef.current, fontScale: value };
    preferencesRef.current = next;
    setFontScale(value);
    if (preferencesReady.current) queuePreferenceSave(next);
  }, [queuePreferenceSave]);

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

  const askToStart = async () => {
    if (runStartState === "starting") return;
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
    if (!selectedProviderProfile) {
      setRunStartError("먼저 ACP 연결 프로필을 선택하십시오.");
      setRunStartState("error");
      navigate("provider");
      return;
    }
    const selectedAdmission = selectedProviderProfile.admission;
    const selectedAdapter = acpAdapters.find((adapter) => adapter.id === selectedProviderProfile.adapterId);
    if (selectedAdmission.state !== "admitted"
      || selectedAdmission.profileId !== selectedProviderProfile.id
      || selectedAdmission.adapterId !== selectedProviderProfile.adapterId
      || selectedAdmission.rootBinding !== "verified"
      || selectedAdapter?.state !== "supported") {
      setRunStartError(selectedAdmission.state === "needs_auth" ? "선택한 프로필에서 공식 로그인을 완료하십시오." : "선택한 프로필의 런타임 검증을 완료하십시오.");
      setRunStartState("error");
      navigate("provider");
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
    if (existingRunActive) {
      setRunStartError("이미 진행 중인 심의가 있습니다. 해당 심의 상태를 확인하십시오.");
      setRunStartState("error");
      return;
    }
    if (snapshot.storage !== "ready") {
      setRunStartError("로컬 저장소 준비 상태가 확인되지 않아 실행을 차단했습니다.");
      setRunStartState("error");
      return;
    }

    setRunStartState("starting");
    setRunStartError("");
    try {
      const started = await startDeliberation({
        question,
        contextDraftId: contextSelection?.draftId ?? null,
        contextRevision: contextSelection?.revision ?? null,
        providerProfileId: selectedProviderProfile.id,
        rolePresetId: selectedRolePreset.id,
        roleRevision: selectedRolePreset.revision,
        disclosureConfirmed: true,
      });
      activeRunIdRef.current = started.runId;
      selectedRunIdRef.current = started.runId;
      setAcceptedRunId(started.runId);
      setSelectedRunId(started.runId);
      setRunProgress(null);
      setRunDossier(null);
      setRunDossierState("loading");
      setCancelRequest(null);
      setRunStartState("idle");
      setDisclosureConfirmed(false);
      announce("심의 시작 요청이 접수되었습니다. 실제 실행 단계 이벤트를 기다립니다.");
    } catch {
      setRunStartState("error");
      setRunStartError("심의 시작 요청이 거부되었습니다. 연결·역할·입력 revision을 다시 확인하십시오.");
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
    if (refreshCurrent) setRunDossierState("loading");
    if (selectedRunIdRef.current === runId) setSelectedRunDossierState("loading");
    try {
      const dossier = await loadRunDossier(runId);
      if (dossier.runId !== runId) throw new Error("Run dossier mismatch.");
      if (refreshCurrent) {
        setRunDossier(dossier);
        setRunDossierState("ready");
      }
      if (selectedRunIdRef.current === runId) {
        setSelectedRunDossier(dossier);
        setSelectedRunDossierState("ready");
      }
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
          setSnapshot(await getConsoleSnapshot());
        } catch {
          announce("심의 결과는 확인했지만 앱 상태 스냅샷을 새로 읽지 못했습니다.");
        }
      }
    } catch {
      if (refreshCurrent) setRunDossierState("error");
      if (selectedRunIdRef.current === runId) setSelectedRunDossierState("error");
      announce("저장된 심의 상태를 다시 읽지 못했습니다.");
    }
  };

  const openCore = (core: string) => {
    setDialog({
      title: `${core} · ${screen === "input" ? "관점" : "코어 정보"}`,
      body: "기본 관점과 역할 편집 화면을 확인할 수 있습니다. 실행 중인 모델 의견은 아직 없습니다.",
    });
  };

  const toggleSound = () => {
    changeSound(!sound);
    announce(sound ? "콘솔 음향을 껐습니다." : "콘솔 음향을 켰습니다. 짧은 미리보기만 재생합니다.");
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

  const connectionLabel = snapshot.connection === "ready"
    ? "연결 확인됨"
    : snapshot.connection === "blocked"
      ? "연결 차단됨"
      : "연결 미확인";
  const storageLabel = snapshot.storage === "ready"
    ? "로컬 기록 준비됨"
    : snapshot.storage === "error"
      ? "로컬 저장 오류"
      : "로컬 기록 미확인";
  const footerStage = currentRunProgress?.stage ?? activeRun?.stage ?? "안건 대기";

  if (shellContext?.windowLabel === "companion") {
    return <CompanionSurface snapshot={snapshot} runId={currentRunId} progress={currentRunProgress} dossier={currentRunDossier} cancelRequest={cancelRequest} onCancelRun={cancelRun} onRefreshRunStatus={refreshRunStatus} onOpenConsole={openCompanionConsole} onOpenSettings={openCompanionSettings} onClose={closeStatusCompanion} onRequestExit={requestExitConfirmation} />;
  }

  return (
    <div className="console-shell" data-theme={theme}>
      <a className="skip-link" href="#screen-content">
        본문으로 건너뛰기
      </a>
      <header className="console-header">
        <div className="window-control-safe-area" aria-hidden="true" />
        <h1 className="console-title">
          <span className="title-long">MAGI COMMAND CONSOLE</span>
          <span className="title-short">MAGI CONSOLE</span>
        </h1>
        <div className="console-edition" aria-label="팬 창작 데스크톱 앱">
          <span>MACOS</span>
          <span className="edition-divider" aria-hidden="true" />
          <span>LOCAL</span>
        </div>
        <nav className="header-actions" aria-label="콘솔 도구">
          <button type="button" onClick={() => navigate("history")}>
            기록
          </button>
          <button type="button" onClick={() => navigate("settings")}>
            설정
          </button>
          <button type="button" aria-pressed={sound} onClick={toggleSound}>
            {sound ? "소리 켬" : "소리 끔"}
          </button>
          <button type="button" aria-pressed={motion !== "full"} onClick={() => changeMotion(motion === "full" ? "reduced" : "full")}>
            {motion === "full" ? "동작 켬" : motion === "reduced" ? "동작 줄임" : "동작 끔"}
          </button>
        </nav>
      </header>

      {screen !== "home" && <section className={`agenda ${invalidQuestion ? "agenda-invalid" : ""}`} aria-label="현재 안건">
        <div className="agenda-marker">
          <strong>안건</strong>
          <span>AGENDA</span>
        </div>
        <div className="agenda-input-wrap">
          <label className="sr-only" htmlFor="question-draft">심의할 질문</label>
          <textarea
            id="question-draft"
            rows={2}
            value={shownQuestion}
            readOnly={!isDraft}
            placeholder={isDraft ? "결정이 필요한 질문을 입력하십시오…" : "안건 대기"}
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
            {invalidQuestion ? "심의할 질문을 입력해야 합니다." : "Enter는 줄바꿈입니다. Command와 Enter를 함께 눌러 입력 확인으로 이동합니다."}
          </p>
        </div>
        <div className="agenda-actions">
          <div className="agenda-source-status">
            <span className="source-symbol" aria-hidden="true">▤</span>
            <span>{activeRun ? `자료 ${activeRun.sourceCount}개` : contextSelection ? `자료 ${contextSelection.sources.filter((source) => source.status === "captured").length}개 접수` : "자료 0개"}</span>
            <button type="button" className="text-action" onClick={() => navigate("intake")}>자료 보기</button>
          </div>
          <button type="button" className="button button-primary agenda-primary" onClick={confirmQuestion} disabled={!isDraft}>
            {isDraft ? "입력 확인" : activeRun ? "진행 중" : "새 안건"}
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
          snapshot={snapshot}
          homeData={homeData}
          selectedRunId={selectedRunId}
          selectedRunDossier={selectedRunDossier}
          selectedRunDossierState={selectedRunDossierState}
          currentRunId={currentRunId}
          runProgress={currentRunProgress}
          runDossier={currentRunDossier}
          runDossierState={runDossierState}
          runStartState={runStartState}
          runStartError={runStartError}
          disclosureConfirmed={disclosureConfirmed}
          cancelRequest={cancelRequest}
          contextSelection={contextSelection}
          acpAdaptersState={acpAdaptersState}
          acpAdapters={acpAdapters}
          acpProfilesState={acpProfilesState}
          acpProfiles={acpProfiles}
          selectedAcpProfileId={selectedAcpProfileId}
          selectedProviderProfile={selectedProviderProfile ?? null}
          authenticatingProfileId={authenticatingProfileId}
          validatingProfileId={validatingProfileId}
          acpProfileDraft={acpProfileDraft}
          acpProfileWriteState={acpProfileWriteState}
          rolePresetsState={rolePresetsState}
          rolePresets={rolePresets}
          selectedRolePresetId={selectedRolePresetId}
          selectedRolePreset={selectedRolePreset ?? null}
          roleDraft={roleDraft}
          roleWriteState={roleWriteState}
          companionWindow={false}
          onNavigate={navigate}
          onOpenCore={openCore}
          onOpenRecentRun={openRecentRun}
          onBeginAcpProfileCreate={beginAcpProfileCreate}
          onBeginAcpProfileEdit={beginAcpProfileEdit}
          onAcpProfileDraftChange={setAcpProfileDraft}
          onSaveAcpProfile={(draft) => { void saveAcpProfileDraft(draft); }}
          onSelectAcpProfile={(profileId) => { void selectAcpProfile(profileId); }}
          onValidateAcpProfile={(profileId) => { void validateAcpProfile(profileId); }}
          onAuthenticateAcpProfile={(profileId) => { void authenticateAcpProfile(profileId); }}
          onSelectRolePreset={(presetId) => { void selectRolePreset(presetId); }}
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
        <span className="footer-value">{storageLabel}</span>
        <span className="footer-stage">{footerStage}</span>
        <button type="button" className="connection-state" onClick={() => navigate("connections")}>
          <span className={`status-dot status-${snapshot.connection}`} aria-hidden="true" />
          {connectionLabel}
        </button>
      </footer>

      <dialog className="magi-dialog" ref={dialogRef} aria-labelledby="dialog-title" onCancel={() => setDialog(null)} onClose={() => setDialog(null)}>
        {dialog && (
          <>
            <div className="dialog-header">
              <span>MAGI / CONTROL</span>
              <button type="button" className="icon-button" aria-label="대화상자 닫기" onClick={() => setDialog(null)}>×</button>
            </div>
            <h2 id="dialog-title">{dialog.title}</h2>
            <p className="dialog-copy">{dialog.body}</p>
            <div className="dialog-actions">
              <button type="button" className="button" onClick={() => setDialog(null)}>닫기</button>
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
