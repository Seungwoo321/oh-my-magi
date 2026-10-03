import { BudgetPreview } from "./budget-preview";
import { ReviewedSharePanel, RestoreDataPanel } from "./record-actions";
import { ContextPreview } from "./context-preview";
import { SignedUpdatePanel } from "./native-settings";
import { RoleTransfer } from "./role-transfer";
import { getLocale, t, type Locale } from "./lib/locale";
import { Button, Panel } from "./ui-controls";
import { RunClarificationPanel } from "./clarification";
import { PdfRangeCapture } from "./pdf-range-capture";
import { ExternalReplayBrowser, RecordBackup, RecordBrowser, StoredReplay, RecordEvidenceBrowser, type EvidenceReadingTarget } from "./records";
export { Button, Panel } from "./ui-controls";
import { CoreBindingControls, type CoreBindingsController } from "./core-bindings";
import { useEffect, useRef, useState, type FormEvent, type ReactNode } from "react";
import type { ClarificationParent, ClarificationDraft, AuthProfileResult, CredentialHome, ConsoleRunSummary, ConsoleSnapshot, ContextSelectionSummary, LiveRunError, LiveRunSnapshot, ProviderAdmissionFailure, ProviderAuthProgress, ProviderCatalogSnapshot, ProviderSourceScope, ProviderValidationFailure, RoleStoreDiagnostic, RunDossierView, RunUpdate } from "./lib/desktop-api";

export const screenCatalog = [
  { id: "home", title: "시작 화면", english: "COMMAND CENTER", family: "home" },
  { id: "input", title: "안건 입력", english: "AGENDA INPUT", family: "console" },
  { id: "independent", title: "독립 검토", english: "INDEPENDENT REVIEW", family: "console" },
  { id: "review", title: "교차 검토", english: "CROSS REVIEW", family: "console" },
  { id: "proposal", title: "결의문 작성", english: "PROPOSAL DRAFT", family: "console" },
  { id: "sealed", title: "봉인 표결", english: "SEALED BALLOT", family: "console" },
  { id: "verdict", title: "심의 결의", english: "FINAL VERDICT", family: "console" },
  { id: "evidence", title: "결의와 근거", english: "DOSSIER / EVIDENCE", family: "console" },
  { id: "paused", title: "심의 일시정지", english: "DELIBERATION PAUSED", family: "recovery" },
  { id: "interrupted", title: "상태 확인", english: "RUN STATUS CHECK", family: "recovery" },
  { id: "cancelling", title: "취소 확인 중", english: "CANCELLATION CHECK", family: "recovery" },
  { id: "cancelled", title: "심의 취소", english: "RUN CANCELLED", family: "recovery" },
  { id: "failed", title: "심의 실패", english: "RUN FAILED", family: "recovery" },
  { id: "save-error", title: "저장 상태", english: "LOCAL STORAGE STATUS", family: "recovery" },
  { id: "connections", title: "모델 연결", english: "CONNECTION LEDGER", family: "work" },
  { id: "intake", title: "자료 접수", english: "SOURCE INTAKE", family: "work" },
  { id: "confirmation", title: "입력·전송 확인", english: "INPUT CONFIRMATION", family: "work" },
  { id: "roles", title: "세 관점 설정", english: "CORE ROLE EDITOR", family: "work" },
  { id: "settings", title: "콘솔 설정", english: "CONSOLE SETTINGS", family: "work" },
  { id: "history", title: "대화·기록", english: "LOCAL RECORDS", family: "work" },
  { id: "replay", title: "기록 재생", english: "READ-ONLY REPLAY", family: "work" },
  { id: "share", title: "공유 미리보기", english: "SHARE PREVIEW", family: "work" },
  { id: "data", title: "자료·보존", english: "DATA & RETENTION", family: "work" },
  { id: "maintenance", title: "유지관리", english: "MAINTENANCE", family: "work" },
  { id: "companion", title: "메뉴 막대 상태", english: "MAGI STATUS", family: "companion" },
] as const;

export type ScreenId = (typeof screenCatalog)[number]["id"] | "provider";
export type MotionSetting = "full" | "reduced" | "off";

export type RecentRunSummary = {
  id: string;
  question: string;
  status: string;
  createdAt: string;
};

export type HomeData = {
  recordsState: "loading" | "ready" | "unavailable" | "error";
  recentRuns: RecentRunSummary[];
};

export type AcpAdapterSummary = {
  id: string;
  displayName: string;
  state: "supported" | "blocked";
  reason?: string;
};

export type AcpAdapterState = "loading" | "ready" | "unavailable" | "error";
function profileSetupSupported(adapterId: string, adapters: AcpAdapterSummary[], adaptersState: AcpAdapterState): boolean {
  return adapterId === "codex-acp" || (adaptersState === "ready" && adapters.some(adapter => adapter.id === adapterId && adapter.id !== "openai-responses" && adapter.state === "supported"));
}
export type AcpProfileStoreState = "loading" | "ready" | "unavailable" | "error";
export type AcpProfileBlockReason = ProviderAdmissionFailure;

export type AcpProfileAdmission =
  | { state: "not_checked" }
  | { state: "blocked"; reason: AcpProfileBlockReason; checkedAt?: string }
  | { state: "admitted"; profileId: string; profileRevision: number; adapterId: string; rootBinding: "verified"; checkedAt: string };

export type AcpProfile = {
  id: string;
  displayName: string;
  accountAlias: string;
  adapterId: string;
  revision: number;
  authenticationMethod: "local_subscription" | "byok_api";
  admission: AcpProfileAdmission;
  credentialHome?: CredentialHome | null;
};

export type AcpProfileDraft = {
  profileId: string | null;
  expectedRevision: number | null;
  displayName: string;
  adapterId: string;
  credentialHomePath: string;
};

export type AcpProfileWriteState = "idle" | "saving" | "saved" | "conflict" | "error";

export type CoreRole = "melchior" | "balthasar" | "casper";
export type RoleOutputLanguage = "same" | "ko" | "en";

export type RoleDefinition = {
  core: CoreRole;
  label: string;
  perspective: string;
  criteria: string;
  challengeCondition: string;
  outputLanguage: RoleOutputLanguage;
};

export type RolePreset = {
  id: string;
  name: string;
  revision: number;
  kind: "factory" | "user";
  roles: RoleDefinition[];
};

export type RolePresetDraft = {
  presetId: string | null;
  expectedRevision: number | null;
  name: string;
  roles: RoleDefinition[];
};

export type RolePresetStoreState = "loading" | "ready" | "unavailable" | "error";
export type RolePresetWriteState = "idle" | "saving" | "saved" | "conflict" | "error";
const roleStoreStageLabels: Record<RoleStoreDiagnostic["stage"], string> = {
  command_access: "명령 접근 확인",
  open: "저장소 열기",
  list_presets: "프리셋 목록 조회",
  load_revision: "프리셋 revision 읽기",
  missing_revision: "프리셋 revision 확인",
  load_selection: "활성 선택 읽기",
  map_presets: "프리셋 응답 변환",
  unknown: "실패 단계 미확인",
};
export type RunProgressSummary = Pick<RunUpdate, "runId" | "stage" | "coreId" | "state" | "text">;
export type CancelRequestState = { runId: string; state: "sending" | "sent" | "error"; message?: string } | null;

export type ScreenProps = {
  locale: Locale;
  onLocaleChange: (locale: Locale) => void;
  screen: ScreenId;
  question: string;
  motion: MotionSetting;
  sound: boolean;
  theme: "command" | "clear";
  snapshot: ConsoleSnapshot;
  homeData: HomeData;
  evidenceReadingTarget?: EvidenceReadingTarget | null;
  onEvidenceReadingTargetChange?: (target: EvidenceReadingTarget) => void;
  selectedRunId: string | null;
  selectedRunDossier: RunDossierView | null;
  selectedRunDossierState: "idle" | "loading" | "ready" | "error";
  currentRunId: string | null;
  runProgress: RunProgressSummary | null;
  clarificationDraft?: ClarificationDraft | null;
  onClarificationParentVerified?: (parent: ClarificationParent | null) => void;
  onDiscardClarificationDraft?: (action: () => void) => void;
  onConfirmClarificationDraft?: (draft: ClarificationDraft) => void;
  onClarificationDraftChanged?: (draft: ClarificationDraft | null) => void;
  runDossier: RunDossierView | null;
  runDossierState: "idle" | "loading" | "ready" | "error";
  runStartState: "idle" | "starting" | "error";
  runStartError: string;
  admissionCancellation: "idle" | "pending" | "accepted" | "error";
  onCancelAdmission: () => void;
  disclosureConfirmed: boolean;
  cancelRequest: CancelRequestState;
  contextSelection: ContextSelectionSummary | null;
  onPdfCaptured: (selection: ContextSelectionSummary) => void;
  acpAdaptersState: AcpAdapterState;
  acpAdapters: AcpAdapterSummary[];
  acpProfilesState: AcpProfileStoreState;
  acpProfiles: AcpProfile[];
  selectedAcpProfileId: string | null;
  providerSourceScopes: ProviderSourceScope[];
  providerSourceScopesState: "loading" | "ready" | "error" | "unavailable";
  providerSourceScopePathInput: string;
  providerSourceScopeWriteState: "idle" | "adding" | "revoking";
  providerSourceScopeError: string;
  selectedProviderProfile: AcpProfile | null;
  authenticatingProfileId: string | null;
  providerAuthProgress: ProviderAuthProgress | null;
  cancellingProviderAuth: { profileId: string; profileRevision: number } | null;
  validatingProfile: { profileId: string; profileRevision: number } | null;
  providerValidationError: ProviderValidationFailure | null;
  authProfileResult: AuthProfileResult | null;
  authProfileError: LiveRunError | null;
  authProfileErrorBinding: { profileId: string; profileRevision: number } | null;
  liveCatalog: ProviderCatalogSnapshot | null;
  liveCatalogState: "idle" | "loading" | "ready" | "error";
  liveCatalogError: LiveRunError | null;
  coreBindings: CoreBindingsController;
  selectedLiveModelId: string;
  liveQuestion: string;
  liveRunId: string | null;
  liveRunSnapshot: LiveRunSnapshot | null;
  liveRunText: string;
  liveSnapshotError: LiveRunError | null;
  liveRequestState: "idle" | "starting" | "accepted" | "error";
  liveRequestError: LiveRunError | null;
  liveCancelRequest: { runId: string; state: "sending" | "accepted" | "error"; error?: LiveRunError } | null;
  acpProfileDraft: AcpProfileDraft | null;
  acpProfileWriteState: AcpProfileWriteState;
  rolePresetsState: RolePresetStoreState;
  roleStoreDiagnostic: RoleStoreDiagnostic | null;
  rolePresets: RolePreset[];
  selectedRolePresetId: string | null;
  selectedRolePreset: RolePreset | null;
  roleDraft: RolePresetDraft | null;
  roleWriteState: RolePresetWriteState;
  companionWindow: boolean;
  onNavigate: (screen: ScreenId) => void;
  onOpenCore: (core: string) => void;
  onOpenRecentRun: (runId: string) => void;
  onRecordDeleted: (runId: string) => void;
  onBeginAcpProfileCreate: () => void;
  onBeginAcpProfileEdit: (profileId: string) => void;
  onAcpProfileDraftChange: (draft: AcpProfileDraft | null) => void;
  onSaveAcpProfile: (draft: AcpProfileDraft) => void;
  onSelectAcpProfile: (profileId: string) => void;
  onValidateAcpProfile: (profileId: string) => void;
  onAuthenticateAcpProfile: (profileId: string) => void;
  onCancelProviderAuth: (profileId: string, profileRevision: number) => void;
  onProviderSourceScopePathInputChange: (value: string) => void;
  onPickProviderSourceDirectory: () => void;
  onAddProviderSourceScope: () => void;
  onRevokeProviderSourceScope: (grantId: string) => void;
  onRefreshLiveCatalog: () => void;
  onSelectedLiveModelIdChange: (modelId: string) => void;
  onLiveQuestionChange: (question: string) => void;
  onStartLiveRun: () => void;
  onCancelLiveRun: (runId: string) => void;
  onSelectRolePreset: (presetId: string) => void;
  onRetryRolePresets: () => void;
  onReturnFromRoles: () => void;
  onReturnFromConnections: () => void;
  onCheckConnection: (profileId: string) => void;
  onReturnFromSettings: () => void;
  onBeginRolePresetEdit: (presetId: string) => void;
  onCloneRolePreset: (presetId: string) => void;
  onEditRoleDraft: (draft: RolePresetDraft | null) => void;
  onSaveRolePreset: (input: { presetId: string | null; expectedRevision: number | null; draft: RolePresetDraft }) => void;
  onStartRealRun: () => void;
  onDisclosureConfirmedChange: (confirmed: boolean) => void;
  onCancelRun: (runId: string) => void;
  onRefreshRunStatus: (runId: string) => void;
  onNotice: (message: string) => void;
  onMotionChange: (value: MotionSetting) => void;
  onSoundChange: (value: boolean) => void;
  onThemeChange: (value: "command" | "clear") => void;
  fontScale: 100 | 125 | 150 | 200;
  onFontScaleChange: (value: 100 | 125 | 150 | 200) => void;
  commonContextBudgetRevision: number | null;
  commonContextTokenLimit: number;
  onCommonContextTokenLimitChange: (value: number) => void;
  onSelectContextFiles: () => void;
  onSelectContextDirectory: () => void;
  onOpenConsole: () => void;
  onOpenSettings: () => void;
  onCloseCompanion: () => void;
  onRequestExit: () => void;
};

type CoreId = "balthasar" | "casper" | "melchior";
type Vote = "support" | "oppose" | "abstain";

const cores: Array<{
  id: CoreId;
  voteIndex: number;
  number: string;
  name: string;
  role: string;
  focus: string;
  polygon: string;
}> = [
  { id: "balthasar", voteIndex: 1, number: "02", name: "BALTHASAR·2", role: "지속성과 돌봄", focus: "장기 영향·사람·운영", polygon: "320,10 1040,10 1120,100 1120,160 850,325 510,325 240,160 240,100" },
  { id: "casper", voteIndex: 2, number: "03", name: "CASPER·3", role: "주체성과 대안", focus: "선택권·반증·대안", polygon: "175,235 230,235 560,440 635,540 635,595 45,595 10,560 10,435" },
  { id: "melchior", voteIndex: 0, number: "01", name: "MELCHIOR·1", role: "근거와 실현 가능성", focus: "근거·구현·제약", polygon: "1185,235 1130,235 800,440 725,540 725,595 1315,595 1350,560 1350,435" },
];

const voteNames: Record<Vote, string> = { support: "찬성", oppose: "반대", abstain: "기권" };
const voteJapanese: Record<Vote, string> = { support: "賛成", oppose: "反対", abstain: "棄権" };

const stageLabels: Record<string, { english: string; korean: string; japanese: string; hub: string }> = {
  input: { english: "STANDBY", korean: "안건 대기", japanese: "待機", hub: "STANDBY" },
  independent: { english: "ISOLATED REVIEW", korean: "독립 검토", japanese: "独立審議", hub: "REVIEW" },
  review: { english: "CROSS REVIEW", korean: "교차 검토", japanese: "相互検討", hub: "CROSS" },
  proposal: { english: "SYNTHESIS", korean: "결의문 작성", japanese: "決議作成", hub: "PROPOSAL" },
  sealed: { english: "SEALED", korean: "표 봉인됨", japanese: "封印", hub: "SEALED" },
  verdict: { english: "RESOLVED", korean: "심의 종료", japanese: "可決", hub: "RESOLVED" },
  evidence: { english: "DOSSIER", korean: "결의와 근거", japanese: "記録", hub: "DOSSIER" },
  paused: { english: "PAUSED", korean: "일시정지", japanese: "待機", hub: "PAUSED" },
  interrupted: { english: "STATUS UNKNOWN", korean: "상태 확인 필요", japanese: "中断", hub: "CHECK" },
  cancelling: { english: "CANCELLING", korean: "취소 확인 중", japanese: "停止中", hub: "CANCEL" },
  cancelled: { english: "CANCELLED", korean: "심의 취소됨", japanese: "中止", hub: "STOPPED" },
  failed: { english: "FAILED", korean: "실패", japanese: "障害", hub: "FAILED" },
  "save-error": { english: "SAVE STATUS", korean: "저장 상태 확인", japanese: "保存状態", hub: "SAVE" },
};

const compactHubLabels: Record<string, string> = {
  input: "STBY",
  independent: "REVIEW",
  review: "X-REV",
  proposal: "DRAFT",
  sealed: "SEALED",
  verdict: "DONE",
  evidence: "RECORD",
  paused: "PAUSED",
  interrupted: "CHECK",
  cancelling: "CANCEL",
  cancelled: "STOP",
  failed: "FAILED",
  "save-error": "SAVE",
};

export function ScreenSurface(props: ScreenProps) {
  const {
    screen, question, motion, sound, theme, snapshot, contextSelection, onNavigate,
    onOpenCore, onStartRealRun, companionWindow,
    homeData, selectedRunId, acpAdaptersState, acpAdapters, acpProfilesState, acpProfiles,
    selectedAcpProfileId, acpProfileDraft, acpProfileWriteState,
    rolePresetsState, rolePresets, selectedRolePresetId, roleDraft, roleWriteState,
    onNotice, onMotionChange, onSoundChange, onThemeChange, onSelectContextFiles,
    fontScale, onFontScaleChange,
    onBeginAcpProfileCreate, onBeginAcpProfileEdit, onAcpProfileDraftChange,
    onSaveAcpProfile, onSelectAcpProfile,
    onSelectRolePreset, onBeginRolePresetEdit, onCloneRolePreset, onEditRoleDraft, onSaveRolePreset,
    onOpenConsole, onOpenSettings, onCloseCompanion, onRequestExit,
  } = props;
  if (screen === "home") return <HomePage data={homeData} snapshot={snapshot} onNavigate={onNavigate} onOpenCore={onOpenCore} onOpenRecentRun={props.onOpenRecentRun} />;
  const page = screenCatalog.find((item) => item.id === (screen === "provider" ? "connections" : screen))!;
  const run = snapshot.activeRun;
  const runId = props.currentRunId;
  const dossier = props.runDossier?.runId === runId ? props.runDossier : null;
  const progress = props.runProgress?.runId === runId ? props.runProgress : null;
  const runIsActive = Boolean(runId && (dossier ? !isTerminalRunStatus(dossier.status) : run ? !isTerminalRunStatus(run.status) : true));
  const runScreen = ["confirmation", "independent", "review", "proposal", "sealed", "verdict", "evidence", "paused", "interrupted", "cancelling", "cancelled", "failed"].includes(screen);
  const showRunProgress = runScreen && (screen !== "confirmation" || runIsActive || props.runStartState === "starting");

  return (
    <>
      <PageHeading page={page} screen={screen} />
      {showRunProgress && <RunProgressPanel runId={runId} progress={progress} dossier={dossier} dossierState={props.runDossierState} />}
      {screen === "input" && (
        <InputStage
          onNavigate={onNavigate}
          onOpenCore={onOpenCore}
          snapshot={snapshot}
          run={run}
        />
      )}
      {screen === "independent" && <DeliberationStage screen={screen} run={run} onOpenCore={onOpenCore} runId={runId} runIsActive={runIsActive} cancelRequest={props.cancelRequest} onCancelRun={props.onCancelRun} />}
      {screen === "review" && <DeliberationStage screen={screen} run={run} onOpenCore={onOpenCore} runId={runId} runIsActive={runIsActive} cancelRequest={props.cancelRequest} onCancelRun={props.onCancelRun} />}
      {screen === "proposal" && <ProposalStage run={run} dossier={dossier} runId={runId} runIsActive={runIsActive} cancelRequest={props.cancelRequest} onCancelRun={props.onCancelRun} onNavigate={onNavigate} />}
      {screen === "sealed" && <SealedStage run={run} dossier={dossier} onOpenCore={onOpenCore} runId={runId} runIsActive={runIsActive} cancelRequest={props.cancelRequest} onCancelRun={props.onCancelRun} />}
      {screen === "verdict" && <VerdictStage run={run} dossier={dossier} onNavigate={onNavigate} />}
      {screen === "evidence" && <EvidenceStage onChanged={props.onRefreshRunStatus} evidenceReadingTarget={props.evidenceReadingTarget} onEvidenceReadingTargetChange={props.onEvidenceReadingTargetChange} run={run} dossier={dossier} onOpenCore={onOpenCore} onNavigate={onNavigate} />}
      {["paused", "interrupted", "cancelling", "cancelled", "failed", "save-error"].includes(screen) && <RecoveryPage onClarificationParentVerified={props.onClarificationParentVerified} onDiscardClarificationDraft={props.onDiscardClarificationDraft} onConfirmClarificationDraft={props.onConfirmClarificationDraft} onClarificationDraftChanged={props.onClarificationDraftChanged} screen={screen} run={run} dossier={dossier} runId={runId} cancelRequest={props.cancelRequest} onCancelRun={props.onCancelRun} onRefreshRunStatus={props.onRefreshRunStatus} runDossierState={props.runDossierState} onNavigate={onNavigate} />}
      {(screen === "connections" || screen === "provider") && <ProviderPage
        onCheckConnection={props.onCheckConnection}
        onBack={props.onReturnFromConnections}
        adaptersState={acpAdaptersState}
        adapters={acpAdapters}
        profilesState={acpProfilesState}
        profiles={acpProfiles}
        selectedProfileId={selectedAcpProfileId}
        draft={acpProfileDraft}
        writeState={acpProfileWriteState}
        authenticatingProfileId={props.authenticatingProfileId}
        providerAuthProgress={props.providerAuthProgress}
        validatingProfile={props.validatingProfile}
        providerValidationError={props.providerValidationError}
        authProfileResult={props.authProfileResult}
        authProfileError={props.authProfileError}
        authProfileErrorBinding={props.authProfileErrorBinding}
        liveCatalog={props.liveCatalog}
        liveCatalogState={props.liveCatalogState}
        liveCatalogError={props.liveCatalogError}
        coreBindings={props.coreBindings}
        selectedLiveModelId={props.selectedLiveModelId}
        liveRequestState={props.liveRequestState}
        onBeginCreate={onBeginAcpProfileCreate}
        onBeginEdit={onBeginAcpProfileEdit}
        onDraftChange={onAcpProfileDraftChange}
        onSave={onSaveAcpProfile}
        onSelect={onSelectAcpProfile}
        onRefreshLiveCatalog={props.onRefreshLiveCatalog}
        onSelectedLiveModelIdChange={props.onSelectedLiveModelIdChange}
      />}
      {screen === "intake" && <IntakePage selection={contextSelection} onNavigate={onNavigate} onSelectContextFiles={onSelectContextFiles} onSelectContextDirectory={props.onSelectContextDirectory} onPdfCaptured={props.onPdfCaptured} />}
      {screen === "confirmation" && <ConfirmationPage commonContextBudgetRevision={props.commonContextBudgetRevision} clarificationDraft={props.clarificationDraft} coreBindings={props.coreBindings} question={question} run={run} selection={contextSelection} snapshot={snapshot} rolePreset={props.selectedRolePreset} profilesState={acpProfilesState} rolePresetsState={props.rolePresetsState} disclosureConfirmed={props.disclosureConfirmed} runStartState={props.runStartState} runStartError={props.runStartError} admissionCancellation={props.admissionCancellation} onCancelAdmission={props.onCancelAdmission} currentRunId={runId} dossier={dossier} dossierState={props.runDossierState} onDisclosureConfirmedChange={props.onDisclosureConfirmedChange} onCancelRun={props.onCancelRun} cancelRequest={props.cancelRequest} onNavigate={onNavigate} onStart={onStartRealRun} />}
      {screen === "roles" && <RolesPage presetsState={rolePresetsState} diagnostic={props.roleStoreDiagnostic} presets={rolePresets} selectedPresetId={selectedRolePresetId} draft={roleDraft} writeState={roleWriteState} onSelect={onSelectRolePreset} onBeginEdit={onBeginRolePresetEdit} onClone={onCloneRolePreset} onDraftChange={onEditRoleDraft} onSave={onSaveRolePreset} onRetry={props.onRetryRolePresets} onBack={props.onReturnFromRoles} />}
      {screen === "settings" && <SettingsPage commonContextTokenLimit={props.commonContextTokenLimit} onCommonContextTokenLimitChange={props.onCommonContextTokenLimitChange} locale={props.locale} onLocaleChange={props.onLocaleChange} onBack={props.onReturnFromSettings} motion={motion} sound={sound} theme={theme} fontScale={fontScale} onMotionChange={onMotionChange} onSoundChange={onSoundChange} onThemeChange={onThemeChange} onFontScaleChange={onFontScaleChange} onNavigate={onNavigate} onNotice={onNotice} />}
      {screen === "history" && <HistoryPage onClarificationParentVerified={props.onClarificationParentVerified} onDiscardClarificationDraft={props.onDiscardClarificationDraft} onClarificationDraftChanged={props.onClarificationDraftChanged} onConfirmClarificationDraft={props.onConfirmClarificationDraft} evidenceReadingTarget={props.evidenceReadingTarget} onEvidenceReadingTargetChange={props.onEvidenceReadingTargetChange} data={homeData} selectedRunId={selectedRunId} selectedDossier={props.selectedRunDossier?.runId === selectedRunId ? props.selectedRunDossier : null} dossierState={props.selectedRunDossierState} storageDiagnostic={snapshot.storageDiagnostic} onNavigate={onNavigate} onSelectRun={props.onOpenRecentRun} onDeleted={props.onRecordDeleted} onRefreshRunStatus={props.onRefreshRunStatus} />}
      {screen === "replay" && <ReplayPage runId={selectedRunId} dossier={props.selectedRunDossier?.runId === selectedRunId ? props.selectedRunDossier : null} onNavigate={onNavigate} />}
      {screen === "share" && <ReviewedSharePanel dossier={props.selectedRunDossier} />}
      {screen === "data" && <DataPage />}
      {screen === "maintenance" && <MaintenancePage onNavigate={onNavigate} />}
      {screen === "companion" && <CompanionPage run={run} snapshot={snapshot} nativeWindow={companionWindow} runId={runId} progress={progress} dossier={dossier} cancelRequest={props.cancelRequest} onCancelRun={props.onCancelRun} onRefreshRunStatus={props.onRefreshRunStatus} onNavigate={onNavigate} onOpenConsole={onOpenConsole} onOpenSettings={onOpenSettings} onClose={onCloseCompanion} onRequestExit={onRequestExit} />}
    </>
  );
}

function HomePage({ data, snapshot, onNavigate, onOpenCore, onOpenRecentRun }: {
  data: HomeData;
  snapshot: ConsoleSnapshot;
  onNavigate: ScreenProps["onNavigate"];
  onOpenCore: ScreenProps["onOpenCore"];
  onOpenRecentRun: ScreenProps["onOpenRecentRun"];
}) {
  const run = snapshot.activeRun;
  const runInProgress = Boolean(run && !["completed", "cancelled", "failed"].includes(run.status));
  const connection = runtimeAvailabilityLabel(snapshot.connection);
  const storage = snapshot.storage === "ready" ? "로컬 저장소 준비됨" : snapshot.storage === "error" ? "로컬 저장 오류" : "저장 상태 미확인";
  const recentRuns = data.recentRuns.filter((item) => item.id !== run?.id);

  return (
    <section className="home-landing" aria-labelledby="screen-title">
      <header className="home-heading">
        <div>
          <p className="page-kicker"><span>MAGI COMMAND</span><span className="kicker-divider" aria-hidden="true">/</span><span>00 · HOME</span></p>
          <h2 id="screen-title">{t("사령 콘솔")}</h2>
          <p className="home-intro">{t("안건을 열고, 세 관점의 심의와 실제 기록을 관리합니다.")}</p>
        </div>
        <div className="home-system-status" aria-label={t("현재 시스템 상태")}>
          <span className={`status-chip ${runtimeAvailabilityClass(snapshot.connection)}`}>{connection}</span>
          <span className={`status-chip ${snapshot.storage === "ready" ? "chip-support" : snapshot.storage === "error" ? "chip-oppose" : "chip-unknown"}`}>{t("저장 ·")}{storage}</span>
        </div>
      </header>
      <StorageUnavailableNotice diagnostic={snapshot.storageDiagnostic} />

      <div className="home-grid">
        <section className="home-launch panel" aria-labelledby="home-launch-title">
          <div className="home-launch-heading">
            <p className="panel-kicker">THREE CORES / ONE AGENDA</p>
            <h3 id="home-launch-title">{runInProgress ? t("진행 중인 심의") : run ? t("마지막 확인 심의") : t("새 심의를 준비하십시오")}</h3>
            <p>{run ? run.question : t("질문을 입력하고 자료·연결·역할을 확인한 뒤 심의를 시작합니다.")}</p>
            {run && <span className="home-run-state">{runStageLabel(run.stage)} · {runStatusLabel(run.status)}</span>}
          </div>
          <div className="home-topology"><CoreTopology screen={screenForRun(run)} run={run} onOpenCore={onOpenCore} compact /></div>
          <div className="home-launch-actions">
            <Button tone="primary" onClick={() => run ? onOpenRecentRun(run.id) : onNavigate("input")}>{runInProgress ? t("진행 심의 기록 열기") : run ? t("최근 심의 기록 열기") : t("새 심의 시작")}<span aria-hidden="true">↗</span></Button>
            {run && !runInProgress && <Button tone="primary" onClick={() => onNavigate("input")}>{t("새 심의 시작")}</Button>}
            <Button onClick={() => onNavigate("connections")}>{t("모델 연결 관리")}</Button>
            <Button onClick={() => onNavigate("roles")}>{t("세 관점 설정")}</Button>
          </div>
          <p className="home-run-caveat">{t("실제 실행은 연결·권한·자료 전송 확인과 저장소 상태가 준비되어야 시작됩니다.")}</p>
        </section>

        <section className="home-records panel" aria-labelledby="home-records-title">
          <div className="home-section-heading">
            <div><p className="panel-kicker">LOCAL RECORDS</p><h3 id="home-records-title">{t("최근 기록")}</h3></div>
            <Button onClick={() => onNavigate("history")}>{t("기록 전체 보기")}</Button>
          </div>
          {data.recordsState === "loading" && <EmptyState title={t("기록 확인 중")} text={t("로컬 저장소에서 실제 심의 기록을 읽고 있습니다.")} />}
          {data.recordsState === "unavailable" && <EmptyState title={t("기록을 사용할 수 없습니다")} text={t("기록 목록 서비스가 제공되지 않아 비어 있는 기록으로 간주하지 않습니다.")} />}
          {data.recordsState === "error" && <EmptyState title={t("기록을 읽지 못했습니다")} text={t("저장된 기록을 조회하지 못했습니다. 다시 확인하거나 저장소 상태를 확인하십시오.")} />}
          {data.recordsState === "ready" && recentRuns.length === 0 && <EmptyState title={run ? t("현재 심의 외에 추가 기록이 없습니다") : t("저장된 심의 기록이 없습니다")} text={run ? t("현재 또는 마지막으로 확인한 심의는 왼쪽에 표시합니다.") : t("새 심의를 시작하면 확인된 실행 기록이 이곳에 나타납니다.")} />}
          {data.recordsState === "ready" && recentRuns.length > 0 && <ul className="home-record-list">{recentRuns.map((item) => <li key={item.id}>
            <button type="button" className="home-record-row" onClick={() => onOpenRecentRun(item.id)}>
              <span className="home-record-copy"><strong>{item.question || t("안건 없음")}</strong><small>{formatRecordDate(item.createdAt)}</small></span>
              <span className="home-record-status">{runStatusLabel(item.status)}</span>
              <span className="home-record-arrow" aria-hidden="true">↗</span>
            </button>
          </li>)}</ul>}
        </section>
      </div>
    </section>
  );
}

function formatRecordDate(value: string): string {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return "시각 미확인";
  return new Intl.DateTimeFormat("ko-KR", { dateStyle: "medium", timeStyle: "short" }).format(date);
}

function runStatusLabel(status: string): string {
  const labels: Record<string, string> = {
    completed: "완료",
    running: "진행 중",
    preparing: "준비 중",
    awaiting_confirmation: "확인 대기",
    paused: "일시 정지",
    interrupted: "상태 미확인",
    cancelling: "취소 확인 중",
    cancelled: "취소됨",
    failed: "실패",
  };
  return labels[status] ? t(labels[status]) : `${t("상태")} · ${status}`;
}

function runStageLabel(stage: string): string {
  const labels: Record<string, string> = {
    preparing: "입력 준비",
    awaiting_confirmation: "입력 확인 대기",
    independent_review: "독립 검토",
    cross_review: "교차 검토",
    synthesis: "결의문 작성",
    balloting: "봉인 표결",
  };
  return t(labels[stage] ?? "단계 미확인");
}

function outcomeLabel(outcome: RunDossierView["outcome"]): string {
  const labels: Record<Exclude<RunDossierView["outcome"], null>, string> = {
    unanimous: "만장일치",
    majority: "다수 결의",
    rejected: "기각",
    unresolved: "미해결",
  };
  return t(outcome ? labels[outcome] : "결과 미기록");
}

function isTerminalRunStatus(status: string | undefined): boolean {
  return status === "completed" || status === "failed" || status === "cancelled";
}

function RunProgressPanel({ runId, progress, dossier, dossierState }: {
  runId: string | null;
  progress: RunProgressSummary | null;
  dossier: RunDossierView | null;
  dossierState: ScreenProps["runDossierState"];
}) {
  if (!runId) return null;
  const current = progress?.runId === runId ? progress : null;
  const currentDossier = dossier?.runId === runId ? dossier : null;
  const terminal = isTerminalRunStatus(currentDossier?.status);
  const coreNames: Record<NonNullable<RunProgressSummary["coreId"]>, string> = {
    "MELCHIOR-1": "MELCHIOR·1",
    "BALTHASAR-2": "BALTHASAR·2",
    "CASPER-3": "CASPER·3",
  };
  const eventLabel = current?.coreId
    ? current.state === "streaming" ? `${coreNames[current.coreId]} 응답 생성 중` : current.state === "started" ? `${coreNames[current.coreId]} 단계 시작` : current.state === "completed" ? `${coreNames[current.coreId]} 단계 완료 · 전체 심의 종료 여부 확인 중` : `${coreNames[current.coreId]} 단계 상태 확인 중`
    : current?.state === "streaming" ? "현재 단계 실행 중" : current?.state === "started" ? "실행 단계 시작" : current?.state === "completed" || current?.state === "failed" || current?.state === "cancelled" ? "저장된 최종 상태 확인 중" : "실행 요청 상태 확인 중";
  const stage = current?.stage ?? currentDossier?.stage;
  return (
    <Panel title={t("실행 계기")} kicker="LIVE RUN STATE">
      {dossierState === "loading" && <p className="field-help" role="status">{t("실제 저장 상태를 확인하고 있습니다.")}</p>}
      {dossierState === "error" && <p className="unavailable-reason" role="alert">{t("저장된 실행 상태를 읽지 못했습니다. 완료·실패·취소를 추정하지 않습니다.")}</p>}
      {stage && <p className="proposal-meta"><span>{t("현재 단계")}</span><strong>{runStageLabel(stage)} · {stage}</strong></p>}
      {current && <p role="status" aria-live="polite">{terminal ? `저장된 결과 상태 · ${runStatusLabel(currentDossier?.status ?? "unknown")}` : eventLabel}</p>}
      {current?.text && <section className="run-stream-preview" aria-label={t("실시간 공급자 응답")}>
        <p className="run-stream-label">{t("공급자가 반환한 실제 응답")}</p>
        <div className="run-stream-body" role="region" aria-label={t("실시간 응답 본문")} aria-live="off">{current.text}</div>
        <p className="field-help">{t("화면의 원문 텍스트는 서식이나 명령으로 실행하지 않습니다. 최종 응답과 상태는 저장된 실행 기록으로 확인합니다.")}</p>
      </section>}
      {currentDossier && <p className="field-help">{t("저장된 Run 상태 ·")}{runStatusLabel(currentDossier.status)}{terminal ? t(" · 최종 상태 확인됨") : t(" · 실행 중")}</p>}
      {!current && !currentDossier && dossierState === "ready" && <p className="field-help">{t("이 실행에 대한 진행 이벤트가 아직 도착하지 않았습니다.")}</p>}
      {!terminal && <p className="field-help">{t("코어별 단계 완료는 전체 심의 완료를 뜻하지 않습니다. 종료 여부는 저장된 dossier 상태로 확인합니다.")}</p>}
    </Panel>
  );
}

function InlineCancelControl({ runId, active, cancelRequest, onCancel }: {
  runId: string;
  active: boolean;
  cancelRequest: CancelRequestState;
  onCancel: (runId: string) => void;
}) {
  const [confirming, setConfirming] = useState(false);
  if (!active) return null;
  const request = cancelRequest?.runId === runId ? cancelRequest : null;
  return (
    <div className="inline-warning" aria-label={t("심의 취소")}>
      <strong>{t("진행 중인 심의")}</strong>
      {request?.state === "sending" && <p role="status">{t("취소 요청을 보내고 있습니다.")}</p>}
      {request?.state === "sent" && <p role="status">{t("취소 요청을 보냈습니다. 저장 상태가 취소로 바뀔 때까지 실행 상태를 확인합니다.")}</p>}
      {request?.state === "error" && <p role="alert">{request.message ?? t("취소 요청을 전달하지 못했습니다.")}</p>}
      {!confirming && (!request || request.state === "error") && <Button tone="danger" onClick={() => setConfirming(true)}>{t("심의 취소")}</Button>}
      {confirming && (!request || request.state === "error") && <>
        <p>{t("현재 Run의 남은 코어 작업을 취소하도록 요청합니다. 취소 완료는 저장 상태로 확인합니다.")}</p>
        <div className="page-actions"><Button onClick={() => setConfirming(false)}>{t("취소 안 함")}</Button><Button tone="danger" onClick={() => { setConfirming(false); onCancel(runId); }}>{t("취소 요청 보내기")}</Button></div>
      </>}
    </div>
  );
}

function screenForRun(run?: ConsoleRunSummary): string {
  if (!run) return "input";
  if (run.ballotState === "public") return "verdict";
  if (run.ballotState === "sealed" || run.stage === "balloting") return "sealed";
  const screens: Record<string, string> = {
    preparing: "input",
    awaiting_confirmation: "input",
    independent_review: "independent",
    cross_review: "review",
    synthesis: "proposal",
  };
  return screens[run.stage] ?? "interrupted";
}

function stageScreen(stage?: string, status?: string): string {
  if (status === "completed") return "verdict";
  if (status === "failed" || status === "cancelled") return status;
  const screens: Record<string, string> = {
    preparing: "input",
    awaiting_confirmation: "input",
    independent_review: "independent",
    cross_review: "review",
    synthesis: "proposal",
    balloting: "sealed",
  };
  if (stage) return screens[stage] ?? "interrupted";
  if (status === "paused" || status === "cancelling" || status === "interrupted") return status;
  return status ? "interrupted" : "input";
}

function PageHeading({ page, screen }: { page: (typeof screenCatalog)[number]; screen: ScreenId }) {
  return (
    <div className="page-heading">
      <div className="heading-copy">
        <p className="page-kicker"><span>{page.english}</span><span className="kicker-divider" aria-hidden="true">/</span><span>{screen === "input" ? "COMMAND 01" : `MAGI · ${String(screenCatalog.findIndex((item) => item.id === screen) + 1).padStart(2, "0")}`}</span></p>
        <h2 id="screen-title">{t(page.title)}</h2>
      </div>
      <div className="heading-state" aria-label={t("실제 실행 정보 미확인")}>
        <span className="status-chip chip-unknown">LOCAL CONSOLE</span>
        <span className="heading-core-mark" aria-hidden="true"><i /><i /><i /></span>
      </div>
    </div>
  );
}

function InputStage({ onNavigate, onOpenCore, snapshot, run }: {
  onNavigate: ScreenProps["onNavigate"];
  onOpenCore: (core: string) => void; snapshot: ConsoleSnapshot; run?: ConsoleRunSummary;
}) {
  const connection = runtimeAvailabilityLabel(snapshot.connection);
  return (
    <section className="stage-section" aria-label={t("세 코어 심의 무대")}>
      <div className="stage-meta stage-meta-left">
        <span className="instrument-rule" />
        <strong>MAGI CORE</strong>
        <span>{t("STANDBY / 안건 대기")}</span>
        <p>{t("질문·자료·역할을 확인한 뒤 심의를 시작합니다.")}</p>
      </div>
      <CoreTopology screen="input" run={run} onOpenCore={onOpenCore} />
      <div className="stage-meta stage-meta-right">
        <span className="instrument-rule" />
        <strong>{snapshot.connection === "runtime_available" ? "ACP RUNTIME AVAILABLE" : snapshot.connection === "blocked" ? "ACP RUNTIME BLOCKED" : "ACP RUNTIME UNVERIFIED"}</strong>
        <span>{connection}</span>
        <p>{t("연결·권한·입력 검증이 완료되기 전에는 실행하지 않습니다.")}</p>
      </div>
      <div className="stage-rail">
        <span className="rail-index">01</span>
        <div><strong>{t("질문에서 결의까지")}</strong><p>{t("세 관점이 같은 안건을 검토하도록 질문을 작성하십시오.")}</p></div>
        <div className="stage-actions">
          <Button onClick={() => onNavigate("connections")}>{t("연결 설정")}</Button>
          <Button onClick={() => onNavigate("roles")}>{t("세 관점 설정")}</Button>
        </div>
      </div>
    </section>
  );
}

function DeliberationStage({ screen, run, onOpenCore, runId, runIsActive, cancelRequest, onCancelRun }: {
  screen: "independent" | "review";
  run?: ConsoleRunSummary;
  onOpenCore: (core: string) => void;
  runId: string | null;
  runIsActive: boolean;
  cancelRequest: CancelRequestState;
  onCancelRun: ScreenProps["onCancelRun"];
}) {
  const stage = stageLabels[screen];
  const isReview = screen === "review";
  return (
    <section className="stage-section" aria-label={stage.korean}>
      <div className="stage-meta stage-meta-left">
        <span className="instrument-rule" /><strong>RUN STATE</strong>
        <span>{run ? runStatusLabel(run.status) : t("실행 상태 확인 중")}</span>
        <p>{t("단계와 종료 여부는 실제 실행 이벤트와 저장 상태로 확인합니다.")}</p>
      </div>
      <CoreTopology screen={screen} run={run} onOpenCore={onOpenCore} />
      <div className="stage-meta stage-meta-right">
        <span className="instrument-rule" /><strong>{stage.english}</strong>
        <span>{stage.japanese} · {stage.korean}</span>
        <p>{isReview ? t("교차 검토는 관점 간 공개된 쟁점만 표시합니다.") : t("독립 검토 의견은 표결 공개 전 서로 분리되어야 합니다.")}</p>
      </div>
      <div className="stage-rail">
        <span className="rail-index">{isReview ? "03" : "02"}</span>
        <div><strong>{isReview ? t("서로의 주장 검토") : t("같은 안건 · 독립된 관점")}</strong><p>{t("실제 실행은 검증된 입력과 연결 상태 확인 뒤에만 표시됩니다.")}</p></div>
        {runId && <div className="stage-actions"><InlineCancelControl runId={runId} active={runIsActive} cancelRequest={cancelRequest} onCancel={onCancelRun} /></div>}
      </div>
    </section>
  );
}

function ProposalStage({ run, dossier, runId, runIsActive, cancelRequest, onCancelRun, onNavigate }: {
  run?: ConsoleRunSummary;
  dossier: RunDossierView | null;
  runId: string | null;
  runIsActive: boolean;
  cancelRequest: CancelRequestState;
  onCancelRun: ScreenProps["onCancelRun"];
  onNavigate: ScreenProps["onNavigate"];
}) {
  return (
    <section className="stage-section stage-section-reading">
      <CoreTopology screen="proposal" run={run} />
      <div className="stage-rail proposal-rail">
        <span className="rail-index">04</span>
        <div><strong>{t("표결 대상 제안")}</strong><p>{dossier?.proposal ? t("저장된 제안 내용을 표시합니다.") : t("저장된 제안 내용이 아직 없습니다.")}</p></div>
        <div className="stage-actions">
          <Button onClick={() => onNavigate("evidence")}>{t("근거 검토")}</Button>
          {runId && <InlineCancelControl runId={runId} active={runIsActive} cancelRequest={cancelRequest} onCancel={onCancelRun} />}
        </div>
      </div>
      <Panel title={t("결의문 전문")} kicker="PROPOSAL CONTENT">
        {dossier?.proposal ? <ProposalContent proposal={dossier.proposal} /> : <EmptyState title={t("제안 미제공")} text={t("저장된 실행 기록에 제안 본문이 생기면 이곳에 표시합니다.")} />}
        <div className="proposal-meta"><span>{t("안건")}</span><strong>{dossier?.question ?? run?.question ?? t("안건 미확인")}</strong></div>
        {run && <div className="proposal-meta"><span>{t("자료 연결")}</span><strong>{run.sourceCount}{t("개")}</strong></div>}
      </Panel>
    </section>
  );
}

function SealedStage({ run, dossier, onOpenCore, runId, runIsActive, cancelRequest, onCancelRun }: { run?: ConsoleRunSummary; dossier: RunDossierView | null; onOpenCore: (core: string) => void; runId: string | null; runIsActive: boolean; cancelRequest: CancelRequestState; onCancelRun: ScreenProps["onCancelRun"] }) {
  return (
    <section className="stage-section">
      <div className="sealed-warning" role="note"><strong>{t("표 방향은 봉인되어 있습니다")}</strong><span>{t("세 표가 검증되기 전까지 색·문구·접근성 이름에 찬반 방향이 나타나지 않습니다.")}</span></div>
      <CoreTopology screen="sealed" run={run} onOpenCore={onOpenCore} />
      <Panel title={t("동일한 표결 대상")} kicker="PROPOSAL SNAPSHOT">
        {dossier?.proposal ? <ProposalContent proposal={dossier.proposal} /> : <p className="proposal-copy">{t("표결 대상 문안이 저장된 상태에 없습니다.")}</p>}
        <p className="sealed-status">{dossier ? `표 ${dossier.votes.length}개 접수 · 방향 봉인` : run?.ballotState === "sealed" ? `표 ${run.votes?.length ?? t("확인된 수 없음")}개 제출 · 방향 봉인` : t("표결 상태 미확인")}</p>
      </Panel>
      {runId && <InlineCancelControl runId={runId} active={runIsActive} cancelRequest={cancelRequest} onCancel={onCancelRun} />}
    </section>
  );
}

function VerdictStage({ run, dossier, onNavigate }: { run?: ConsoleRunSummary; dossier: RunDossierView | null; onNavigate: ScreenProps["onNavigate"] }) {
  const resultReady = dossier?.status === "completed";
  const votes = resultReady ? dossier.votes : undefined;
  const voteCounts = votes ? countVotes(votes) : undefined;
  const outcomeLabels: Record<NonNullable<RunDossierView["outcome"]>, string> = { unanimous: "전원 합의", majority: "다수 결의", rejected: "제안 기각", unresolved: "결론 미도출" };
  const summary = resultReady && dossier.outcome ? outcomeLabels[dossier.outcome] : "최종 결과 확인 전";
  return (
    <section className="stage-section verdict-stage">
      <div className="verdict-overline"><span>FINAL VERDICT</span><span>{resultReady ? "DOSSIER VERIFIED" : "RESULT NOT VERIFIED"}</span></div>
      <CoreTopology screen="verdict" run={run} />
      <div className="verdict-band">
        <div className="verdict-label"><span>{t("결의")}</span><small>VERDICT</small></div>
        <div className="verdict-main"><span className="verdict-english">{resultReady && dossier.outcome ? dossier.outcome.toUpperCase() : "UNVERIFIED"}</span><strong>{summary}</strong></div>
        <div className="vote-counts" aria-label={votes ? `찬성 ${voteCounts?.support ?? 0}, 반대 ${voteCounts?.oppose ?? 0}, 기권 ${voteCounts?.abstain ?? 0}` : t("표결 정보 미확인")}>
          {votes ? <><span className="count-support">{t("찬성")}{voteCounts?.support ?? 0}</span><span className="count-oppose">{t("반대")}{voteCounts?.oppose ?? 0}</span><span>{t("기권")}{voteCounts?.abstain ?? 0}</span></> : <span>{t("표결 기록 미확인")}</span>}
        </div>
      </div>
      <div className="verdict-details">
        <Panel title={t("승인된 제안")} kicker="PROPOSAL">
          {resultReady && dossier.proposal ? <ProposalContent proposal={dossier.proposal} /> : <EmptyState title={t("최종 결의문 미확인")} text={t("저장된 최종 dossier의 완료 상태가 확인되면 제안을 표시합니다.")} />}
        </Panel>
        <Panel title={t("소수 의견")} kicker="DISSENT · ALWAYS VISIBLE" className="dissent-panel">
          {resultReady ? <DissentContent dossier={dossier} /> : <EmptyState title={t("최종 상태 미확인")} text={t("코어 단계 완료만으로 전체 심의를 종료하지 않습니다. 저장된 dossier를 기다립니다.")} />}
          {dossier?.error && <p className="unavailable-reason" role="alert">{dossier.error.code} · {dossier.error.message}</p>}
        </Panel>
      </div>
      <div className="page-actions">
        <Button onClick={() => onNavigate("evidence")}>{t("근거와 이견 열기")}<span aria-hidden="true">↗</span></Button>
        <Button onClick={() => onNavigate("history")}>{t("기록 열기")}</Button>
      </div>
    </section>
  );
}

function EvidenceStage({ run, dossier, onOpenCore, onNavigate, evidenceReadingTarget, onEvidenceReadingTargetChange, onChanged }: { onChanged: ScreenProps["onRefreshRunStatus"]; evidenceReadingTarget?: EvidenceReadingTarget | null; onEvidenceReadingTargetChange?: (target: EvidenceReadingTarget) => void; run?: ConsoleRunSummary; dossier: RunDossierView | null; onOpenCore: (core: string) => void; onNavigate: ScreenProps["onNavigate"] }) {
  const resultReady = dossier?.status === "completed";
  const votes = resultReady ? dossier.votes : undefined;
  const voteCounts = votes ? countVotes(votes) : undefined;
  return (
    <section className="evidence-layout">
      <div className="evidence-topology"><CoreTopology screen="evidence" run={run} onOpenCore={onOpenCore} compact /></div>
      <div className="evidence-summary">
        <p className="panel-kicker">{resultReady ? "DOSSIER VERIFIED" : "DOSSIER STATUS UNKNOWN"}</p>
        <h3>{resultReady ? dossier.outcome ?? t("결론 미도출") : t("결의 기록 미확인")}</h3>
        <p>{votes ? `찬성 ${voteCounts?.support ?? 0} · 반대 ${voteCounts?.oppose ?? 0} · 기권 ${voteCounts?.abstain ?? 0}` : t("최종 저장 상태 확인 전에는 표결 방향을 표시하지 않습니다.")}</p>
      </div>
      <div className="evidence-sections">
        <Panel title={t("제안")} kicker="01 / PROPOSAL">{resultReady && dossier.proposal ? <ProposalContent proposal={dossier.proposal} /> : <EmptyState title={t("제안 미제공")} text={t("실제 dossier에 저장된 제안만 표시합니다.")} />}</Panel>
        <Panel title={t("표결")} kicker="02 / BALLOTS">{votes ? <VoteDetails votes={votes} /> : <p>{t("최종 완료 상태가 저장된 dossier로 확인되기 전까지 표의 방향을 공개하지 않습니다.")}</p>}</Panel>
        <Panel title={t("공통 근거와 의견 차이")} kicker="03 / EVIDENCE & DISSENT">
          {resultReady ? <DissentContent dossier={dossier} /> : <EmptyState title={t("최종 상태 미확인")} text={t("전체 심의가 완료됐다고 확인되기 전에는 표결과 이견을 확정하지 않습니다.")} />}
        </Panel>
        <RecordEvidenceBrowser runId={dossier?.runId ?? null} revision={dossier?.revision} onChanged={onChanged} target={evidenceReadingTarget} onTargetChange={onEvidenceReadingTargetChange} />
      </div>
      <div className="page-actions"><Button onClick={() => onNavigate("history")}>{t("기록으로")}</Button><Button tone="primary" onClick={() => onNavigate("share")}>{t("공유 미리보기")}<span aria-hidden="true">↗</span></Button></div>
    </section>
  );
}

function countVotes(votes: RunDossierView["votes"]) {
  return votes.reduce((counts, vote) => {
    counts[vote.choice] += 1;
    return counts;
  }, { support: 0, oppose: 0, abstain: 0 });
}

function coreDisplayName(coreId: RunDossierView["votes"][number]["coreId"]): string {
  const names: Record<RunDossierView["votes"][number]["coreId"], string> = {
    "MELCHIOR-1": "MELCHIOR·1",
    "BALTHASAR-2": "BALTHASAR·2",
    "CASPER-3": "CASPER·3",
  };
  return names[coreId];
}

function choiceLabel(choice: RunDossierView["votes"][number]["choice"]): string {
  return t(choice === "support" ? "찬성" : choice === "oppose" ? "반대" : "기권");
}

function VoteDetails({ votes }: { votes: RunDossierView["votes"] }) {
  return votes.length > 0 ? <ul className="contract-list">{votes.map((vote) => <li key={vote.coreId}><strong>{coreDisplayName(vote.coreId)} · {choiceLabel(vote.choice)}</strong><p>{vote.rationale}</p></li>)}</ul> : <EmptyState title={t("저장된 표결 없음")} text={t("최종 dossier에 공개 표결이 없습니다.")} />;
}

function ProposalContent({ proposal }: { proposal: NonNullable<RunDossierView["proposal"]> }) {
  return <>
    <p className="proposal-copy">{proposal.body}</p>
    {proposal.conditions.length > 0 && <><h4>{t("성립 조건")}</h4><ul className="contract-list">{proposal.conditions.map((item, index) => <li key={`${index}-${item}`}>{item}</li>)}</ul></>}
    {proposal.alternatives.length > 0 && <><h4>{t("대안")}</h4><ul className="contract-list">{proposal.alternatives.map((item, index) => <li key={`${index}-${item}`}>{item}</li>)}</ul></>}
  </>;
}

function DissentContent({ dossier }: { dossier: RunDossierView | null }) {
  const objections = dossier?.proposal?.openObjections ?? [];
  const votes = dossier?.votes ?? [];
  if (objections.length === 0 && votes.length === 0) return <EmptyState title={t("기록된 표결 근거·미해결 이의 없음")} text={t("완료된 dossier의 공개 기록에 이 항목이 없습니다.")} />;
  return <>
    {votes.length > 0 && <><h4>{t("코어별 표결 근거")}</h4><VoteDetails votes={votes} /></>}
    {objections.length > 0 && <><h4>{t("미해결 이의")}</h4><ul className="contract-list">{objections.map((objection) => <li key={objection.claimId}><strong>{objection.claimId}</strong><p>{objection.rationale}</p>{objection.requiredInformation.length > 0 && <><span>{t("필요 정보")}</span><ul>{objection.requiredInformation.map((item, index) => <li key={`${index}-${item}`}>{item}</li>)}</ul></>}</li>)}</ul></>}
  </>;
}

function PageHeadingAndLayout({ english, title, intro, children, aside }: { english: string; title: string; intro: string; children: ReactNode; aside?: ReactNode }) {
  return (
    <section className="work-layout">
      <div className="work-main">{children}</div>
      <aside className="work-aside">
        <div className="aside-instrument"><span className="aside-badge" aria-hidden="true">MAGI</span><span className="aside-index">CORE SYSTEM / {String(screenCatalog.findIndex((item) => item.english === english) + 1).padStart(2, "0")}</span></div>
        <p className="aside-kicker">{english}</p><strong>{t(title)}</strong><p>{t(intro)}</p>
        {aside ?? <div className="aside-core-list" aria-label={t("고정된 세 코어")}>
          <span><b>01</b> MELCHIOR·1 <small>{t("근거와 실현 가능성")}</small></span>
          <span><b>02</b> BALTHASAR·2 <small>{t("지속성과 돌봄")}</small></span>
          <span><b>03</b> CASPER·3 <small>{t("주체성과 대안")}</small></span>
        </div>}
        <p className="aside-footnote">{t("표시된 의견·결정 상태는 확인된 실행 기록만 반영합니다.")}</p>
      </aside>
    </section>
  );
}

function ProviderPage({ coreBindings, onCheckConnection, onBack, adaptersState, adapters, profilesState, profiles, selectedProfileId, draft, writeState, authenticatingProfileId, providerAuthProgress, validatingProfile, providerValidationError, authProfileResult, authProfileError, authProfileErrorBinding, liveCatalog, liveCatalogState, liveCatalogError, selectedLiveModelId, liveRequestState, onBeginCreate, onBeginEdit, onDraftChange, onSave, onSelect, onRefreshLiveCatalog, onSelectedLiveModelIdChange }: {
  onCheckConnection: (profileId: string) => void;
  onBack: () => void;
  adaptersState: AcpAdapterState;
  adapters: AcpAdapterSummary[];
  profilesState: AcpProfileStoreState;
  profiles: AcpProfile[];
  selectedProfileId: string | null;
  draft: AcpProfileDraft | null;
  writeState: AcpProfileWriteState;
  authenticatingProfileId: string | null;
  providerAuthProgress: ProviderAuthProgress | null;
  validatingProfile: ScreenProps["validatingProfile"];
  providerValidationError: ProviderValidationFailure | null;
  authProfileResult: AuthProfileResult | null;
  authProfileError: LiveRunError | null;
  authProfileErrorBinding: ScreenProps["authProfileErrorBinding"];
  liveCatalog: ProviderCatalogSnapshot | null;
  liveCatalogState: "idle" | "loading" | "ready" | "error";
  liveCatalogError: LiveRunError | null;
  coreBindings: CoreBindingsController;
  selectedLiveModelId: string;
  liveRequestState: "idle" | "starting" | "accepted" | "error";
  onBeginCreate: () => void;
  onBeginEdit: (profileId: string) => void;
  onDraftChange: (draft: AcpProfileDraft | null) => void;
  onSave: (draft: AcpProfileDraft) => void;
  onSelect: (profileId: string) => void;
  onRefreshLiveCatalog: () => void;
  onSelectedLiveModelIdChange: (modelId: string) => void;
}) {
  const [modeId, setModeId] = useState("");
  const profileDialogInvoker = useRef<HTMLButtonElement | null>(null);
  const subscriptionProfiles = profiles.filter(profile => profile.authenticationMethod === "local_subscription");
  const selectedProfile = subscriptionProfiles.find((profile) => profile.id === selectedProfileId);
  const selectedAdapter = selectedProfile && adapters.find((adapter) => adapter.id === selectedProfile.adapterId);
  const canRunSelectedProfile = Boolean(
    selectedProfile
      && adaptersState === "ready"
      && selectedAdapter?.state === "supported"
      && isProfileAdmissionValid(selectedProfile),
  );
  const selectedAuthResult = selectedProfile && authProfileResult
    && authProfileResult.profileId === selectedProfile.id
    && authProfileResult.profileRevision === selectedProfile.revision
    && authProfileResult.providerId === selectedProfile.adapterId
    ? authProfileResult
    : null;
  const selectedAuthError = selectedProfile && authProfileError
    && authProfileErrorBinding?.profileId === selectedProfile.id
    && authProfileErrorBinding.profileRevision === selectedProfile.revision
    ? authProfileError
    : null;
  const supportsLiveAcpProfile = selectedProfile?.authenticationMethod === "local_subscription";
  const selectedAuthState = supportsLiveAcpProfile ? selectedAuthResult?.state : "unsupported";
  const catalogMatchesProfile = Boolean(supportsLiveAcpProfile && selectedProfile && liveCatalog
    && liveCatalog.providerProfileId === selectedProfile.id
    && liveCatalog.profileRevision === selectedProfile.revision
    && liveCatalog.providerId === selectedProfile.adapterId
    && liveCatalog.adapterId === selectedProfile.adapterId
    && liveCatalog.acpMode === "acp");
  const restoredCatalog = selectedProfile ? coreBindings.catalogs[selectedProfile.id]?.catalog : null;
  const selectedCatalog = restoredCatalog ?? (catalogMatchesProfile ? liveCatalog : null);
  useEffect(() => { setModeId(""); }, [selectedProfile?.id, selectedProfile?.revision, selectedCatalog?.catalogSnapshotId, selectedCatalog?.catalogDigest]);
  const canRefreshCatalog = Boolean(supportsLiveAcpProfile
    && canRunSelectedProfile
    && selectedAuthResult?.state === "authenticated"
    && selectedAuthResult.method === "chat_gpt"
    && liveCatalogState !== "loading");

  return (
    <>
    <PageHeadingAndLayout english="MODEL CONNECTIONS" title={t("모델 연결")} intro={t("이름과 기존 구독 인증 홈으로 프로필을 만들고, 실제 연결과 모델을 확인합니다.")}>
      <Panel title={t("ACP 연결 프로필")} kicker="SAVED CONNECTIONS">
        {profilesState === "loading" && <EmptyState title={t("연결 프로필 확인 중")} text={t("저장된 ACP 프로필을 읽고 있습니다.")} />}
        {profilesState === "unavailable" && <EmptyState title={t("프로필 저장소를 사용할 수 없습니다")} text={t("이 앱에서 프로필 저장 기능이 제공되지 않아 프로필을 만들거나 검증할 수 없습니다.")} />}
        {profilesState === "error" && <EmptyState title={t("프로필을 읽지 못했습니다")} text={t("저장된 프로필을 확인하지 못했습니다. 저장소 오류 상태를 확인하십시오.")} />}
        {profilesState === "ready" && subscriptionProfiles.length === 0 && <EmptyState title={t("저장된 ACP 프로필이 없습니다")} text={t("프로필 추가를 눌러 이름과 기존 CLI 구독 인증 홈 경로를 직접 입력하십시오.")} />}
        {profilesState === "ready" && subscriptionProfiles.length > 0 && <ul className="acp-profile-list">{subscriptionProfiles.map((profile) => {
          const adapter = adapters.find((item) => item.id === profile.adapterId);
          const verified = isProfileAdmissionValid(profile);
          const stateLabel = profileAdmissionLabel(profile, verified);
          const savedState = coreBindings.catalogs[profile.id];
          const catalogProjection = coreBindings.profileCatalogProjection(profile);
          const savedModel = catalogProjection === "ready" ? savedState?.modelSelection?.binding : undefined;
          const savedModelStale = Boolean(savedModel && (savedModel.profileRevision !== profile.revision || savedState.selectionState !== "selected"));
          const authentication = coreBindings.verifiedAuthentication(profile);
          const checkState = coreBindings.profileConnectionState(profile);
          const authenticationLabel = checkState === "checking" ? t("확인 중")
            : checkState === "failed" ? t("확인 실패")
            : authentication ? t("기존 구독 확인됨")
            : checkState === "stale" ? t("프로필 변경 · 다시 확인 필요") : t("아직 확인하지 않음");
          const verifiedAt = authentication?.checkedAt && /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2})$/.test(authentication.checkedAt) && Number.isFinite(Date.parse(authentication.checkedAt)) ? authentication.checkedAt : null;

          const validationPendingForProfile = validatingProfile?.profileId === profile.id
            && validatingProfile.profileRevision === profile.revision;
          const authProgressMatches = providerAuthProgress?.profileId === profile.id && providerAuthProgress.profileRevision === profile.revision;
          const authProgressForProfile = authProgressMatches ? providerAuthProgress : null;
          const validationErrorForProfile = providerValidationError?.profileId === profile.id
            && providerValidationError.profileRevision === profile.revision ? providerValidationError : null;
          const authErrorForProfile = authProfileError && authProfileErrorBinding?.profileId === profile.id
            && authProfileErrorBinding.profileRevision === profile.revision ? authProfileError : null;
          const authProgressLabel = authProgressForProfile
            ? authProgressForProfile.stage === "failed" ? t("기존 인증 확인 실패") : authProgressForProfile.stage === "cancelled" ? t("인증 확인 취소됨") : t("기존 구독 인증 상태 확인 중")
            : null;
          return <li className={`acp-profile-item ${profile.id === selectedProfileId ? "selected" : ""}`} key={profile.id} data-catalog-projection={catalogProjection} data-profile-id={profile.id} data-profile-revision={profile.revision}>
            <button type="button" className="acp-profile-select" aria-pressed={profile.id === selectedProfileId} disabled={liveRequestState === "starting"} onClick={() => onSelect(profile.id)}>
              <span className="acp-profile-mark" aria-hidden="true">{verified ? "✓" : "◇"}</span>
              <span className="acp-profile-copy"><strong>{profile.displayName}</strong><small>{adapter?.displayName ?? profile.adapterId} · {t("기존 CLI 구독")}{profile.accountAlias ? ` · ${profile.accountAlias}` : ""}</small></span>
              <span className={`status-chip ${verified ? "chip-support" : "chip-unknown"}`}>{stateLabel}</span>
            </button>
            <p className="field-help">{t("인증 홈 ·")}<code>{profile.credentialHome?.displayPath ?? t("저장된 경로 없음 · 편집 필요")}</code></p>
            <p className="field-help">{t("저장 모델 ·")}<code>{catalogProjection === "pending" ? t("저장된 모델 확인 중…") : catalogProjection === "error" ? t("저장된 모델 확인 실패") : savedModel?.modelId ?? t("선택하지 않음")}</code>{savedModel?.modeId ? <>{t("· 모드")}<code>{savedModel.modeId}</code></> : null}{savedModelStale ? t(" · 저장한 모델 선택을 다시 확인하십시오") : ""}</p>
            <p className="field-help">{t("인증 상태 ·")}{authenticationLabel}</p>
            <p className="field-help">{t("마지막 인증 확인 ·")}{verifiedAt ? <time dateTime={verifiedAt}>{new Date(verifiedAt).toLocaleString(getLocale() === "en" ? "en-US" : "ko-KR")}</time> : authentication || checkState !== "not_checked" ? t("시간 미확인") : t("확인 기록 없음")}</p>

            <div className="acp-profile-actions">
              <Button data-profile-edit-id={profile.id} data-profile-edit-revision={profile.revision} onClick={(event) => { profileDialogInvoker.current = event.currentTarget; onBeginEdit(profile.id); }} disabled={liveRequestState === "starting" || profilesState !== "ready" || writeState === "saving"}>{t("편집")}</Button>
              <Button onClick={() => onCheckConnection(profile.id)} disabled={liveRequestState === "starting" || validationPendingForProfile || authenticatingProfileId !== null || liveCatalogState === "loading" || adaptersState !== "ready" || adapter?.state !== "supported" || writeState === "saving"}>{validationPendingForProfile || authenticatingProfileId === profile.id ? t("연결 확인 중…") : t("연결 확인")}</Button>
            </div>
            {validationPendingForProfile && <p className="field-help" role="status" aria-live="polite">{t("실행 환경을 확인하고 있습니다.")}</p>}
            {validationErrorForProfile?.code === "validation_command_failed" && <p className="live-acp-error" role="alert">{t("실행 환경을 확인하지 못했습니다. 연결 확인을 다시 시도하십시오.")}</p>}
            {authProgressLabel && <p className="field-help" role="status" aria-live="polite">{authProgressLabel}</p>}
            {authErrorForProfile && <p className="live-acp-error" role="alert">{authErrorForProfile.message}</p>}
          </li>;
        })}</ul>}
        {adaptersState !== "ready" && <p className="field-help" role="status">{adaptersState === "loading" ? t("실행 어댑터를 확인하고 있습니다. Codex ACP 프로필은 먼저 설정할 수 있습니다.") : t("실행 어댑터를 확인하지 못했습니다. Codex ACP 프로필 설정은 가능하지만 연결 확인과 실행은 차단됩니다.")}</p>}
        {profilesState === "ready" && <div className="acp-profile-add" data-adapters-state={adaptersState} data-add-readiness={profileSetupSupported("codex-acp", adapters, adaptersState) ? "ready" : "unavailable"}><Button tone="primary" data-profile-create="true" onClick={(event) => { profileDialogInvoker.current = event.currentTarget; onBeginCreate(); }} disabled={liveRequestState === "starting" || writeState === "saving"}>{t("새 ACP 프로필")}</Button></div>}
      </Panel>

      <Panel title={t("연결 상태")} kicker="CONNECTION STATUS">
        {!selectedProfile && <EmptyState title={t("프로필을 선택하십시오")} text={t("사용할 프로필을 선택하고 연결 확인을 누르십시오.")} />}
        {selectedProfile && <>
          <p><strong>{selectedProfile.displayName}</strong> · <code>{selectedProfile.credentialHome?.displayPath ?? t("인증 홈 경로 확인 필요")}</code></p>
          <ol className="acp-adapter-list" aria-label={t("연결 확인 단계")}>
            <li><strong>{t("실행 환경")}</strong><span>{validatingProfile?.profileId === selectedProfile.id ? t("확인 중") : canRunSelectedProfile ? t("확인됨") : t("확인 필요")}</span></li>
            <li><strong>{t("기존 구독 확인")}</strong><span>{authenticatingProfileId === selectedProfile.id && !validatingProfile ? t("확인 중") : selectedAuthState === "authenticated" ? t("확인됨") : selectedAuthState === "unauthenticated" ? t("인증 홈과 구독 상태 확인 필요") : t("확인 필요")}</span></li>
            <li><strong>{t("모델 목록")}</strong><span>{liveCatalogState === "loading" ? t("가져오는 중") : selectedCatalog ? `${selectedCatalog.models.length}개 확인됨` : t("확인 필요")}</span></li>
          </ol>
          {selectedProfile.accountAlias && selectedProfile.accountAlias !== selectedProfile.displayName && <p className="field-help">{t("계정 ·")}{selectedProfile.accountAlias}</p>}
          {!canRunSelectedProfile && <details><summary>{t("연결 문제 자세히 보기")}</summary><p>{runBlockExplanation(selectedProfile, selectedAdapter, adaptersState)}</p></details>}
        </>}
        {adaptersState === "loading" && <p role="status">{t("사용 가능한 연결 환경을 확인하고 있습니다.")}</p>}
        {(adaptersState === "error" || adaptersState === "unavailable") && <p role="alert">{t("연결 환경을 읽지 못했습니다. 앱을 다시 열거나 연결 확인을 다시 시도하십시오.")}</p>}
      </Panel>

      <Panel title={t("연결 확인과 모델")} kicker="AVAILABLE MODELS" className="live-acp-panel">
        {!selectedProfile && <EmptyState title={t("실행할 프로필을 선택하십시오")} text={t("프로필 인증과 전용 홈 검증을 마친 뒤, 해당 프로필이 반환한 모델을 선택할 수 있습니다.")} />}
        {selectedProfile && <>
          {selectedAuthError && <p className="live-acp-error" role="alert">{selectedAuthError.message}</p>}
          {selectedAuthState !== "authenticated" && <p className="field-help">{t("프로필의 연결 확인을 마치면 사용할 수 있는 모델이 표시됩니다.")}</p>}
          <div className="live-acp-actions">
            <Button onClick={onRefreshLiveCatalog} disabled={!canRefreshCatalog || liveRequestState === "starting"}>
              {liveCatalogState === "loading" ? t("모델 목록 가져오는 중…") : selectedCatalog ? t("모델 목록 새로고침") : t("모델 목록 가져오기")}
            </Button>
            {liveCatalogState === "loading" && <span className="field-help" role="status">{t("선택한 프로필에서 사용할 수 있는 모델을 확인하고 있습니다.")}</span>}
          </div>
          {liveCatalogError && <p className="live-acp-error" role="alert">{liveCatalogError.message}</p>}
          {selectedCatalog && <>
            {selectedCatalog.models.length === 0
              ? <EmptyState title={t("반환된 모델이 없습니다")} text={t("이 프로필의 최신 ACP 모델 목록이 비어 있습니다. 모델 요청은 시작할 수 없습니다.")} />
              : <label className="field live-acp-model-field" htmlFor="live-acp-model">
                <span className="field-label">{t("사용할 모델")}</span>
                <select id="live-acp-model" className="text-field" value={selectedLiveModelId} disabled={liveRequestState === "starting"} onChange={(event) => onSelectedLiveModelIdChange(event.target.value)}>
                  <option value="">{t("반환된 모델 중 하나를 선택하십시오")}</option>
                  {selectedCatalog.models.map((model) => <option key={model.modelId} value={model.modelId}>{model.name || model.modelId}</option>)}
                </select>
              </label>}
            {!selectedCatalog.negotiatedModes ? <p role="status">{t("연결 환경의 모드 지원 여부를 다시 확인해야 모델을 저장할 수 있습니다.")}</p> : selectedCatalog.negotiatedModes.modes.length > 0 && <label className="field" htmlFor="connection-model-mode">{t("사용할 모드")}<select id="connection-model-mode" className="text-field" value={modeId} onChange={event => setModeId(event.target.value)}><option value="">{t("모드 선택")}</option>{selectedCatalog.negotiatedModes.modes.map(mode => <option key={mode.modeId} value={mode.modeId}>{mode.name}</option>)}</select></label>}
            <Button disabled={!selectedCatalog.models.some(model => model.modelId === selectedLiveModelId) || !selectedCatalog.negotiatedModes || (selectedCatalog.negotiatedModes.modes.length > 0 && !selectedCatalog.negotiatedModes.modes.some(mode => mode.modeId === modeId)) || coreBindings.busy.length > 0} onClick={() => { void coreBindings.saveModel(selectedProfile.id, selectedLiveModelId, selectedCatalog.negotiatedModes?.modes.length ? modeId : null); }}>{t("모델 연결 저장")}</Button>
            {coreBindings.errors[selectedProfile.id] && <p role="alert">{coreBindings.errors[selectedProfile.id]}</p>}
            <p className="field-help">{t("저장된 모델 ·")}{coreBindings.catalogs[selectedProfile.id]?.modelSelection?.binding.modelId ?? t("선택 후 저장 필요")}</p>
            <p className="field-help">{t("마지막 모델 확인 ·")}{formatRecordDate(selectedCatalog.fetchedAt)}</p>
          </>}
        </>}
      </Panel>

      <Panel title={t("세 코어 할당")} kicker="THREE PERSPECTIVES"><CoreBindingControls controller={coreBindings} profiles={profiles} /></Panel>
      <div className="page-actions"><Button onClick={onBack}>{t("원래 화면으로 돌아가기")}</Button></div>
      {selectedProfile && !canRunSelectedProfile && <p className="unavailable-reason">{t("선택한 프로필의 연결 확인을 완료하십시오.")}</p>}
    </PageHeadingAndLayout>
    {draft && <AcpProfileEditorDialog key={`${draft.profileId ?? "new"}:${draft.expectedRevision ?? "new"}`} adaptersState={adaptersState} adapters={adapters} draft={draft} invoker={profileDialogInvoker.current} writeState={writeState} onDraftChange={onDraftChange} onBeginEdit={onBeginEdit} onSave={onSave} />}
    </>
  );
}

function profileDialogInvokerMatches(invoker: HTMLButtonElement | null, draft: AcpProfileDraft): boolean {
  return Boolean(invoker?.isConnected && !invoker.disabled && (draft.profileId
    ? invoker.dataset.profileEditId === draft.profileId && invoker.dataset.profileEditRevision === String(draft.expectedRevision)
    : invoker.dataset.profileCreate === "true"));
}

function AcpProfileEditorDialog({ adaptersState, adapters, draft, invoker, writeState, onDraftChange, onBeginEdit, onSave }: {
  adaptersState: AcpAdapterState;
  adapters: AcpAdapterSummary[];
  draft: AcpProfileDraft;
  invoker: HTMLButtonElement | null;
  writeState: AcpProfileWriteState;
  onDraftChange: (draft: AcpProfileDraft | null) => void;
  onBeginEdit: (profileId: string) => void;
  onSave: (draft: AcpProfileDraft) => void;
}) {
  const openingInvoker = useRef(invoker);
  const dialogRef = useRef<HTMLDialogElement>(null);
  const aliasRef = useRef<HTMLInputElement>(null);
  const discardRef = useRef<HTMLButtonElement>(null);
  const pathRef = useRef<HTMLInputElement>(null);
  const [pathError, setPathError] = useState("");
  const initialDraft = useRef(draft);
  const [validationError, setValidationError] = useState("");
  const [discardPrompt, setDiscardPrompt] = useState(false);
  const submitting = useRef(false);
  const canSubmit = profileSetupSupported(draft.adapterId, adapters, adaptersState) && writeState !== "saving" && writeState !== "conflict";

  useEffect(() => {
    const dialog = dialogRef.current;
    if (!dialog) return;
    if (!dialog.open) dialog.showModal();
    const focusFrame = requestAnimationFrame(() => aliasRef.current?.focus());
    return () => {
      cancelAnimationFrame(focusFrame);
      if (dialog.open) dialog.close();
      if (profileDialogInvokerMatches(openingInvoker.current, initialDraft.current)) openingInvoker.current?.focus({ preventScroll: true });
    };
  }, []);

  const close = () => {
    if (writeState === "saving" || submitting.current) return;
    if (draft.credentialHomePath !== initialDraft.current.credentialHomePath || draft.displayName !== initialDraft.current.displayName || draft.adapterId !== initialDraft.current.adapterId) {
      setDiscardPrompt(true);
      requestAnimationFrame(() => discardRef.current?.focus());
      return;
    }
    onDraftChange(null);
  };

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (writeState === "saving" || submitting.current) return;
    if (!draft.displayName.trim()) {
      setValidationError("프로필 별칭을 입력하십시오.");
      aliasRef.current?.focus();
      return;
    }
    const path = draft.credentialHomePath.trim();
    if (!path || !(path.startsWith("/") || path === "~" || path.startsWith("~/"))) {
      setPathError("기존 CLI 인증 홈의 절대 경로나 ~/로 시작하는 경로를 직접 입력하십시오.");
      pathRef.current?.focus();
      return;
    }
    if (canSubmit && !submitting.current) {
      submitting.current = true;
      setValidationError("");
      onSave({ ...draft, credentialHomePath: path });
    }
  };

  useEffect(() => {
    if (writeState !== "saving") submitting.current = false;
  }, [writeState]);

  return (
    <dialog ref={dialogRef} data-profile-id={draft.profileId ?? "new"} data-profile-revision={draft.expectedRevision ?? "new"} className="magi-dialog acp-profile-dialog" aria-labelledby="acp-profile-dialog-title" aria-describedby="acp-profile-dialog-description" onCancel={(event) => { event.preventDefault(); if (discardPrompt) { setDiscardPrompt(false); aliasRef.current?.focus(); } else close(); }}>
      <section className="acp-profile-editor">
        <div className="dialog-header">
          <span>PERSISTED ACP PROFILE</span>
          <button type="button" className="icon-button" aria-label={t("프로필 편집 닫기")} onClick={close} disabled={writeState === "saving"}>×</button>
        </div>
        <h2 id="acp-profile-dialog-title">{draft.profileId ? `프로필 편집 · ${draft.displayName}` : t("새 ACP 프로필")}</h2>
        <p id="acp-profile-dialog-description" className="dialog-copy">{draft.profileId ? t("저장된 이름과 기존 CLI 인증 홈 경로를 편집합니다. 어댑터는 변경할 수 없습니다.") : t("이름과 ACP 어댑터, 기존 CLI 인증 홈 경로를 입력합니다. 저장 뒤 연결을 검증하십시오.")}</p>
        <form className="acp-profile-editor-form" onSubmit={submit} noValidate>
          <label className="field" htmlFor="acp-profile-alias">
            <span className="field-label">{t("프로필 별칭")}<span aria-hidden="true">{t("· 필수")}</span></span>
            <input
              ref={aliasRef}
              id="acp-profile-alias"
              className="text-field"
              autoComplete="off"
              required
              aria-required="true"
              aria-invalid={Boolean(validationError)}
              aria-describedby="acp-profile-alias-help"
              value={draft.displayName}
              disabled={writeState === "saving"}
              onChange={(event) => { setValidationError(""); onDraftChange({ ...draft, displayName: event.target.value }); }}
              placeholder={t("예: 개인 구독")}
            />
            <small id="acp-profile-alias-help" className={validationError ? "live-acp-error" : "field-help"}>
              {validationError || t("저장된 프로필 목록에서 구분할 이름을 입력하십시오.")}
            </small>
          </label>
          <label className="field" htmlFor="acp-profile-adapter">
            <span className="field-label">{t("ACP 어댑터")}</span>
            <select id="acp-profile-adapter" className="text-field" value={draft.adapterId} disabled={Boolean(draft.profileId) || adaptersState !== "ready" || writeState === "saving"} onChange={(event) => onDraftChange({ ...draft, adapterId: event.target.value })}>
              <option value="">{t("어댑터 선택")}</option>
              <option value="codex-acp">{t("Codex ACP · 수동 프로필 설정")}</option>
              {adapters.filter(item => item.id !== "openai-responses" && item.id !== "codex-acp").map((item) => <option key={item.id} value={item.id} disabled={item.state !== "supported"}>{item.displayName}{item.state === "blocked" ? ` · 설정 차단${item.reason ? `: ${adapterReasonLabel(item.reason)}` : ""}` : t(" · 프로필 설정 가능")}</option>)}
            </select>
            {!profileSetupSupported(draft.adapterId, adapters, adaptersState) && <small className="unavailable-reason" role="status">{t("프로필 설정이 가능한 어댑터가 없어 이 프로필을 저장할 수 없습니다.")}</small>}
          </label>
          <label className="field" htmlFor="acp-profile-credential-home">
            <span className="field-label">{t("기존 CLI 인증 홈 경로")}<span aria-hidden="true">{t("· 필수")}</span></span>
            <input ref={pathRef} id="acp-profile-credential-home" className="text-field" autoComplete="off" spellCheck={false} required aria-required="true" aria-invalid={Boolean(pathError)} aria-describedby="acp-profile-credential-home-help" value={draft.credentialHomePath} disabled={writeState === "saving"} onChange={(event) => { setPathError(""); onDraftChange({ ...draft, credentialHomePath: event.target.value }); }} placeholder={t("~/… 또는 절대 경로")} />
            <small id="acp-profile-credential-home-help" className={pathError ? "live-acp-error" : "field-help"}>{pathError || t("이미 구독 인증된 CLI 홈의 경로를 직접 입력하십시오. 경로를 변경하면 연결과 모델을 다시 검증해야 합니다.")}</small>
          </label>
          {writeState === "conflict" && <div className="inline-warning" role="alert"><strong>{t("프로필 revision 충돌")}</strong><p>{t("저장된 버전이 바뀌었습니다. 최신 값을 다시 불러옵니다.")}</p>{draft.profileId && <Button onClick={() => onBeginEdit(draft.profileId!)}>{t("최신 버전 다시 읽기")}</Button>}</div>}
          {writeState === "error" && <p className="unavailable-reason" role="alert">{t("프로필을 저장하지 못했습니다. 입력 내용은 유지했습니다.")}</p>}
          {writeState === "saving" && <p className="field-help" role="status">{t("프로필을 저장하고 있습니다.")}</p>}
          {discardPrompt && <div className="inline-warning acp-profile-discard" role="alert">
            <strong>{t("저장하지 않은 변경 사항이 있습니다.")}</strong>
            <p>{t("변경을 버리거나 계속 편집할 수 있습니다.")}</p>
            <div className="dialog-actions">
              <Button onClick={() => { setDiscardPrompt(false); aliasRef.current?.focus(); }}>{t("계속 편집")}</Button>
              <button ref={discardRef} type="button" className="button button-danger" disabled={writeState === "saving"} onClick={() => { if (!submitting.current && writeState !== "saving") onDraftChange(null); }}>{t("변경 버리기")}</button>
            </div>
          </div>}
          <div className="dialog-actions">
            <Button onClick={close} disabled={writeState === "saving"}>{t("취소")}</Button>
            <Button tone="primary" type="submit" disabled={!canSubmit}>{writeState === "saving" ? t("저장 중…") : t("프로필 저장")}</Button>
          </div>
        </form>
      </section>
    </dialog>
  );
}

function isProfileAdmissionValid(profile: AcpProfile): boolean {
  return profile.admission.state === "admitted"
    && profile.admission.profileId === profile.id
    && profile.admission.profileRevision === profile.revision
    && profile.admission.adapterId === profile.adapterId
    && profile.admission.rootBinding === "verified";
}

function runBlockExplanation(profile: AcpProfile, adapter: AcpAdapterSummary | undefined, adaptersState: AcpAdapterState): string {
  if (adaptersState === "loading") return "프로필 설정 어댑터 상태를 확인 중입니다. 확인을 마칠 때까지 인증·자료 전달·모델 요청은 차단됩니다.";
  if (adaptersState === "unavailable") return "ACP 어댑터 상태를 확인할 수 없어 런타임 실행을 차단했습니다.";
  if (adaptersState === "error") return "ACP 어댑터 목록을 읽지 못해 런타임 실행을 차단했습니다.";
  if (!adapter) return "선택한 프로필의 어댑터가 현재 설정 목록에 없습니다. 프로필을 확인하고 다시 선택하십시오.";
  if (adapter.state === "blocked") return `선택한 어댑터가 프로필 설정 단계에서 차단되었습니다.${adapter.reason ? ` 사유: ${adapterReasonLabel(adapter.reason)}` : ""} 런타임 실행도 차단됩니다.`;
  return admissionExplanation(profile.admission);
}

function adapterReasonLabel(reason: string): string {
  const labels: Record<string, string> = {
    adapter_unavailable: "ACP 실행 어댑터를 사용할 수 없음",
    executable_missing: "ACP 실행 파일을 찾을 수 없음",
    home_override_unsupported: "프로필별 전용 홈 설정을 지원하지 않음",
    home_mismatch: "프로필 전용 홈과 실행 홈이 일치하지 않음",
    runtime_unavailable: "ACP 런타임을 사용할 수 없음",
    runtime_unsupported_platform: "현재 macOS 플랫폼 또는 아키텍처에서 지원하지 않음",
    runtime_resource_unavailable: "앱에 포함된 ACP 리소스를 찾거나 읽을 수 없음",
    runtime_resource_invalid: "앱에 포함된 ACP 리소스 구성이 유효하지 않음",
    runtime_manifest_invalid: "앱에 포함된 ACP 빌드 매니페스트를 읽을 수 없음",
    runtime_manifest_mismatch: "앱에 포함된 ACP 빌드가 현재 앱과 일치하지 않음",
    runtime_checksum_mismatch: "앱에 포함된 ACP 파일 체크섬이 일치하지 않음",
    runtime_artifact_verification_failed: "앱에 포함된 ACP 파일 검증 실패",
    attestation_missing: "프로필 실행 attestation을 받지 못함",
  };
  return labels[reason] ?? reason.replaceAll("_", " ");
}

const admissionMessages: Record<AcpProfileBlockReason, { label: string; explanation: string }> = {
    adapter_unsupported: { label: "지원되지 않는 어댑터", explanation: "선택한 어댑터는 프로필 격리 실행을 지원하지 않습니다." },
    home_override_unsupported: { label: "전용 홈 격리 미지원", explanation: "어댑터가 프로필별 전용 홈 격리를 지원하지 않아 실행이 차단됐습니다." },
    home_mismatch: { label: "프로필 홈 불일치", explanation: "실행기가 보고한 홈 바인딩이 선택한 프로필과 일치하지 않습니다." },
    attestation_missing: { label: "검증 증명 미확인", explanation: "어댑터에서 유효한 프로필 실행 증명을 받지 못했습니다." },
    other: { label: "검증 실패", explanation: "프로필 실행 검증에 실패했습니다. 설정을 확인한 뒤 다시 시도하십시오." },
    unsupported_platform: { label: "지원되지 않는 플랫폼", explanation: "이 macOS 플랫폼 또는 아키텍처에서 실행기를 지원하지 않습니다." },
    invalid_launch: { label: "실행 설정 오류", explanation: "프로필 실행 설정이 유효하지 않아 실행이 차단됐습니다." },
    artifact_verification_failed: { label: "실행 파일 검증 실패", explanation: "동봉 실행 파일의 서명·아키텍처·체크섬 검증에 실패했습니다." },
    profile_home_unavailable: { label: "프로필 홈 사용 불가", explanation: "프로필 전용 홈을 안전하게 확인하거나 준비하지 못했습니다." },
    role_workdir_unavailable: { label: "임시 작업 공간 사용 불가", explanation: "ACP 세션의 격리된 임시 작업 공간을 준비하지 못했습니다." },
    isolation_unavailable: { label: "운영체제 격리 사용 불가", explanation: "운영체제 보안 격리 경계를 설정하지 못했습니다." },
    sandbox_network_rule_rejected: { label: "로컬 연결 정책 거부", explanation: "macOS가 제한된 ACP 프록시 연결 규칙을 거부했습니다." },
    sandbox_profile_rejected: { label: "격리 정책 거부", explanation: "macOS가 제한된 ACP 실행 정책을 거부했습니다." },
    sandbox_policy_probe_timed_out: { label: "격리 정책 확인 시간 초과", explanation: "ACP 격리 정책 확인이 제한 시간을 넘었습니다." },
    process_start_failed: { label: "ACP 실행 실패", explanation: "동봉 ACP 프로세스를 시작하지 못했습니다." },
    process_closed: { label: "ACP 연결 종료", explanation: "ACP 프로세스가 검증 응답 전에 연결을 닫았습니다." },
    process_exited: { label: "ACP 실행 종료", explanation: "ACP 프로세스가 검증 중 예상보다 일찍 종료됐습니다." },
    protocol_error: { label: "ACP 프로토콜 오류", explanation: "ACP 초기화 프로토콜 교환이 실패했습니다." },
    remote_request_failed: { label: "Codex 초기화 거부", explanation: "Codex 실행기가 프로필 초기화 요청을 거부했습니다." },
    rpc_timeout: { label: "ACP 초기화 시간 초과", explanation: "ACP 프로필 초기화 응답을 제한 시간 안에 받지 못했습니다." },
    proxy_unavailable: { label: "연결 프록시 사용 불가", explanation: "공급자 연결 프록시를 시작하거나 완료하지 못했습니다." },
    invalid_response: { label: "초기화 응답 오류", explanation: "ACP 실행기의 초기화 응답이 프로토콜 형식과 일치하지 않습니다." },
    authentication_unsupported: { label: "인증 방식 미지원", explanation: "이 프로필에서 선택한 인증 방식을 실행기가 지원하지 않습니다." },
    unauthenticated: { label: "기존 구독 정보 확인 필요", explanation: "지정한 CLI 인증 홈에서 기존 구독 정보를 확인하지 못했습니다. 경로와 CLI 구독 상태를 확인하십시오." },
    session_already_created: { label: "세션 중복 생성", explanation: "이 ACP 연결에서 이미 세션이 생성되어 검증을 완료할 수 없습니다." },
    catalog_busy: { label: "모델 조회 중", explanation: "모델 카탈로그 요청이 이미 진행 중입니다." },
    model_unavailable: { label: "모델 사용 불가", explanation: "선택 모델을 실행기에서 확인할 수 없습니다." },
    session_unavailable: { label: "세션 사용 불가", explanation: "요청한 ACP 세션을 찾을 수 없습니다." },
    source_read_unavailable: { label: "자료 권한 저장소 사용 불가", explanation: "승인된 자료 권한을 안전하게 읽을 수 없습니다." },
    prompt_in_progress: { label: "요청 진행 중", explanation: "현재 ACP 세션에서 다른 프롬프트가 진행 중입니다." },
    tool_denied: { label: "도구 요청 차단", explanation: "실행기가 허용되지 않은 도구를 요청했습니다." },
    output_limit: { label: "응답 크기 제한", explanation: "실행기 응답이 허용된 크기를 초과했습니다." },
    input_limit: { label: "요청 크기 제한", explanation: "공급자 요청이 허용된 크기를 초과했습니다." },
    event_limit: { label: "이벤트 제한", explanation: "실행기 이벤트 수가 허용된 한도를 초과했습니다." },
    timeout: { label: "실행 시간 초과", explanation: "프로필 실행 검증이 제한 시간을 초과했습니다." },
    cancelled: { label: "검증 취소됨", explanation: "프로필 실행 검증이 취소됐습니다." },
};

function profileAdmissionLabel(profile: AcpProfile, valid: boolean): string {
  if (valid) return "실행 환경 확인됨";
  if (profile.admission.state === "not_checked") return "실행 환경 확인 필요";
  if (profile.admission.state === "admitted") return "검증 정보 불일치 · 실행 차단";
  return admissionMessages[profile.admission.reason].label;
}

function admissionExplanation(admission: AcpProfileAdmission): string {
  if (admission.state === "not_checked") return "런타임의 프로필별 검증 결과가 아직 없습니다. 검증을 실행하기 전까지 이 연결은 차단 상태입니다.";
  if (admission.state === "admitted") return "검증 응답의 프로필, revision 또는 어댑터가 선택한 연결과 일치하지 않습니다. 다시 검증하기 전까지 연결을 사용할 수 없습니다.";
  return admissionMessages[admission.reason].explanation;
}

function IntakePage({ selection, onNavigate, onSelectContextFiles, onSelectContextDirectory, onPdfCaptured }: { selection: ContextSelectionSummary | null; onNavigate: ScreenProps["onNavigate"]; onSelectContextFiles: () => void; onSelectContextDirectory: () => void; onPdfCaptured: (selection: ContextSelectionSummary) => void }) {
  const issueNames: Record<string, string> = {
    user_excluded: "사용자가 제외함", hidden_path: "숨김 경로", credential_path: "인증 정보 경로", symbolic_link: "심볼릭 링크",
    special_file: "지원하지 않는 파일 종류", cross_volume: "허용 범위 밖의 볼륨", unsupported_format: "지원하지 않는 형식",
    invalid_utf8: "텍스트 인코딩을 읽을 수 없음", file_too_large: "파일 크기 제한 초과", manifest_limit: "자료 수 제한 초과",
    access_denied: "접근 권한 없음", changed_during_capture: "읽는 동안 파일이 변경됨", revoked_grant: "접근 권한 철회됨",
    invalid_range: "선택 범위가 잘못됨", read_failure: "파일 읽기 실패",
  };
  const formatBytes = (bytes: number) => bytes < 1024 ? `${bytes} B` : bytes < 1_048_576 ? `${(bytes / 1024).toFixed(1)} KB` : `${(bytes / 1_048_576).toFixed(1)} MB`;
  return (
    <PageHeadingAndLayout english="SOURCE INTAKE" title={t("자료 접수")} intro={t("파일 접근 동의와 모델 전송 확인은 별도의 단계입니다.")}>
      <ContextPreview selection={selection} onSelectionChange={onPdfCaptured} />
      <div className="intake-grid">
        <Panel title={t("선택한 자료")} kicker="SOURCE MANIFEST">
          {selection ? <>
            <div className="manifest-summary"><span>{t("선택")}{selection.sources.length}{t("개")}</span><span>{t("접수")}{selection.sources.filter((source) => source.status === "captured").length}{t("개")}</span><span>{t("변경 번호")}{selection.revision}</span></div>
            <ul className="source-list">{selection.sources.map((source) => <li key={source.sourceId} className={`source-row source-${source.status}`}>
              <span className="source-row-mark" aria-hidden="true">{source.status === "captured" ? "✓" : source.status === "excluded" ? "−" : "!"}</span>
              <span className="source-row-copy"><strong>{source.displayName}</strong><small>{formatBytes(source.byteLength)} · {source.representation.replaceAll("_", " ")}</small>{source.issueCodes.length > 0 && <small>{source.issueCodes.map((code) => issueNames[code] ?? t("상세 사유 미확인")).join(" · ")}</small>}</span>
              <span className="status-chip">{source.status === "captured" ? t("접수됨") : source.status === "excluded" ? t("제외됨") : t("실패")}</span>
            </li>)}</ul>
          </> : <EmptyState title={t("선택된 자료 없음")} text={t("macOS 파일·폴더 선택기로 허용된 항목만 접수합니다. 경로와 원문은 화면에 표시하지 않습니다.")} />}
        </Panel>
        <Panel title={t("원문 / 전달 범위")} kicker="CAPTURE & DELIVERY">{selection ? <div className="source-preview-state"><strong>{t("파일 선택 요약 접수")}</strong><p>{t("파일 경로와 원문 내용은 이 화면에 노출되지 않습니다. 접수된 텍스트·첨부의 실제 전달 범위는 입력 확인 단계에서 별도로 확인합니다.")}</p><small>Manifest digest · {selection.manifestDigest.slice(0, 12)}…</small></div> : <p>{t("파일을 선택하면 접수 결과를 표시합니다. 선택은 모델 전송 동의가 아니며, 전달 범위는 다음 확인 화면에서 따로 확인합니다.")}</p>}</Panel>
      </div>
      <PdfRangeCapture selection={selection} onChange={onPdfCaptured} />
      <div className="page-actions"><Button onClick={onSelectContextFiles}>{t("파일 선택")}</Button><Button onClick={onSelectContextDirectory}>{t("폴더 선택")}</Button><Button tone="primary" onClick={() => onNavigate("confirmation")}>{t("입력 확인")}<span aria-hidden="true">↗</span></Button><p className="field-help">{t("파일 또는 폴더를 고르면 허용된 원문만 캡처합니다. 선택만으로 외부 전송은 시작되지 않습니다.")}</p></div>
    </PageHeadingAndLayout>
  );
}

function ConfirmationPage({ commonContextBudgetRevision, clarificationDraft, coreBindings, question, run, selection, snapshot, rolePreset, profilesState, rolePresetsState, disclosureConfirmed, runStartState, runStartError, admissionCancellation, onCancelAdmission, currentRunId, dossier, dossierState, onDisclosureConfirmedChange, onCancelRun, cancelRequest, onNavigate, onStart }: {
  commonContextBudgetRevision: number | null;
  clarificationDraft?: ClarificationDraft | null;
  coreBindings: CoreBindingsController;
  question: string;
  run?: ConsoleRunSummary;
  selection: ContextSelectionSummary | null;
  snapshot: ConsoleSnapshot;
  rolePreset: RolePreset | null;
  profilesState: AcpProfileStoreState;
  rolePresetsState: RolePresetStoreState;
  disclosureConfirmed: boolean;
  runStartState: ScreenProps["runStartState"];
  runStartError: string;
  admissionCancellation: ScreenProps["admissionCancellation"];
  onCancelAdmission: () => void;
  currentRunId: string | null;
  dossier: RunDossierView | null;
  dossierState: ScreenProps["runDossierState"];
  onDisclosureConfirmedChange: ScreenProps["onDisclosureConfirmedChange"];
  onCancelRun: ScreenProps["onCancelRun"];
  cancelRequest: CancelRequestState;
  onNavigate: ScreenProps["onNavigate"];
  onStart: () => void;
}) {
  const capturedCount = selection?.sources.filter((source) => source.status === "captured").length ?? 0;
  const providerReady = coreBindings.ready;
  const rolesReady = rolePresetsState === "ready" && Boolean(rolePreset && hasThreeRoleSlots(rolePreset.roles));
  const terminalDossier = Boolean(dossier && ["completed", "failed", "cancelled"].includes(dossier.status));
  const activeRun = Boolean(run && !["completed", "failed", "cancelled"].includes(run.status));
  const waitingForRunStatus = Boolean(currentRunId && (!terminalDossier || dossierState !== "ready"));
  const verifiedChildScope = Boolean(clarificationDraft && clarificationDraft.parent.runId === currentRunId && dossier?.runId === currentRunId && dossier.status === "paused" && dossierState === "ready" && selection?.draftId === clarificationDraft.context.draftId && selection.revision === clarificationDraft.context.revision && question === clarificationDraft.question);
  const hasActiveRun = (activeRun || waitingForRunStatus) && !verifiedChildScope;
  const canConfirmTransfer = Boolean(commonContextBudgetRevision !== null && question.trim() && providerReady && rolesReady && !hasActiveRun);
  const [budgetApproval, setBudgetApproval] = useState<{key: string; ready: boolean} | null>(null);
  const budgetRequestKey = JSON.stringify({expectedCommonContextBudgetRevision: commonContextBudgetRevision, question, contextDraftId: selection?.draftId ?? null, contextRevision: selection?.revision ?? null, coreBindings: coreBindings.destinations.map(item => item.reference), rolePresetId: rolePreset?.id ?? "", roleRevision: rolePreset?.revision ?? 0});
  const budgetReady = budgetApproval?.key === budgetRequestKey && budgetApproval.ready;
  const canStart = Boolean(budgetReady && canConfirmTransfer && providerReady && snapshot.storage === "ready" && disclosureConfirmed && runStartState !== "starting");
  const blockReason = hasActiveRun
    ? "현재 심의의 저장 상태를 확인한 뒤 새 심의를 시작할 수 있습니다."
    : profilesState !== "ready"
      ? "ACP 프로필 저장소 상태를 확인하지 못했습니다."
      : !providerReady
        ? `${coreBindings.destinations.filter(item => !item.ready).map(item => item.coreId).join(" · ")}의 저장된 모델 연결을 확인하십시오.`
        : !rolesReady
              ? "실행에 사용할 세 코어 역할 프리셋을 확인하십시오."
              : snapshot.storage !== "ready"
                ? "로컬 저장소 준비 상태가 확인되지 않아 실행을 차단했습니다."
                : !disclosureConfirmed
                  ? "전송 범위를 읽고 명시적으로 동의해야 시작할 수 있습니다."
                  : !budgetReady ? "단계별 전송 예산을 확인해야 시작할 수 있습니다." : "입력·전송 범위가 확인되었습니다.";
  const sourceStatus = (status: string) => status === "captured" ? "캡처됨" : status === "excluded" ? "제외됨" : "실패";
  return (
    <PageHeadingAndLayout english="INPUT CONFIRMATION" title={t("입력·전송 확인")} intro={t("이 화면에서 확인한 범위만 실행 요청에 포함됩니다.")}>
      <Panel title={t("질문")} kicker="AGENDA"><p className="confirmation-question">{question || t("질문이 입력되지 않았습니다.")}</p><Button onClick={() => onNavigate(verifiedChildScope ? "history" : "input")} disabled={hasActiveRun}>{t("질문 편집")}</Button></Panel>
      <Panel title={t("캡처된 자료")} kicker="CONTEXT MANIFEST">
        <p>{selection ? `접수 ${capturedCount}개 · 전체 항목 ${selection.sources.length}개 · revision ${selection.revision}` : t("자료 0개 · 질문과 역할 지침만 전달 대상입니다.")}</p>
        {selection && <ul className="source-list">{selection.sources.map((source) => <li className={`source-row source-${source.status}`} key={source.sourceId}><span className="source-row-copy"><strong>{source.displayName}</strong><small>{sourceStatus(source.status)} · {source.representation.replaceAll("_", " ")} · {source.byteLength} bytes</small></span><span className="status-chip">{sourceStatus(source.status)}</span></li>)}</ul>}
      </Panel>
      <Panel title={t("전송 목적지")} kicker="PROVIDER PROFILE">
        {coreBindings.destinations.map(item => <div className="confirmation-destination" key={item.coreId}><strong>{item.coreId}</strong><p>{item.ready ? `${item.profile?.displayName} · ${item.model?.modelId}${item.model?.modeId ? ` · ${item.model.modeId}` : ""}` : t("저장된 연결 확인 필요")}</p><p>{t("인증 홈 ·")}<code>{item.profile?.credentialHome?.displayPath ?? t("경로 확인 필요")}</code></p></div>)}
      </Panel>
      <Panel title={t("심의 역할")} kicker="ROLE PRESET">
        {rolePreset ? <><div className="confirmation-destination"><strong>{rolePreset.name}</strong><p>revision {rolePreset.revision} · {rolePreset.kind === "factory" ? t("기본 프리셋") : t("사용자 프리셋")}</p></div><ul className="contract-list">{rolePreset.roles.map((role) => <li key={role.core}><strong>{role.label}</strong> · {role.perspective}{t("· 출력")}{role.outputLanguage === "same" ? t("안건 언어와 동일") : role.outputLanguage === "ko" ? t("한국어") : t("영어")}</li>)}</ul></> : <EmptyState title={rolePresetsState === "loading" ? t("역할 확인 중") : t("역할 프리셋 미선택")} text={t("저장된 정확히 세 코어의 역할 revision을 확인해야 심의를 시작할 수 있습니다.")} />}
      </Panel>
      <BudgetPreview requestKey={budgetRequestKey} enabled={canConfirmTransfer} onResult={setBudgetApproval} />
      <Panel title={t("자료 전송에 대한 명시적 동의")} kicker="EXTERNAL TRANSFER">
        <label className="check-row"><input type="checkbox" checked={disclosureConfirmed} disabled={!canConfirmTransfer || runStartState === "starting"} onChange={(event) => onDisclosureConfirmedChange(event.target.checked)} /><span>{t("이 질문, 선택한 역할 지침, 캡처된 원문 자료")}{capturedCount}{t("개를 위의 선택 프로필로 전송하는 데 동의합니다. 외부 서비스의 처리와 보관은 해당 서비스 정책을 따릅니다.")}</span></label>
        <p className="field-help">{t("자료 선택만으로 전송하지 않습니다. 동의를 해제하거나 질문·역할·프로필·자료를 바꾸면 다시 확인해야 합니다.")}</p>
      </Panel>
      {runStartState === "starting" && <p className="field-help" role="status">{t("시작 요청을 등록하고 실제 저장 상태를 확인하고 있습니다.")}</p>}
      {admissionCancellation === "pending" && <p className="field-help" role="status">{t("취소 의도를 유지하고 있습니다. 실제 저장 응답을 기다립니다.")}</p>}
      {admissionCancellation === "accepted" && <p className="field-help" role="status">{t("취소 의도가 저장되었습니다. 코어 작업의 종료 여부는 저장된 심의 상태로 확인합니다.")}</p>}
      {admissionCancellation === "error" && <p className="unavailable-reason" role="alert">{t("취소 저장 응답을 확인하지 못했습니다. 취소 의도와 원래 요청을 유지합니다.")}</p>}
      {((runStartState === "starting" && admissionCancellation === "idle") || admissionCancellation === "error") && <Button tone="danger" onClick={onCancelAdmission}>{admissionCancellation === "error" ? t("취소 저장 다시 확인") : t("심의 시작 요청 취소")}</Button>}
      {runStartState === "error" && runStartError && <p className="unavailable-reason" role="alert">{runStartError}</p>}
      {runStartState !== "error" && !canStart && <p className="blocked-reason">{blockReason}</p>}
      {currentRunId && runStartState !== "starting" && <InlineCancelControl runId={currentRunId} active={hasActiveRun} cancelRequest={cancelRequest} onCancel={onCancelRun} />}
      <div className="confirmation-actions"><Button onClick={() => onNavigate("connections")}>{t("ACP 프로필 확인")}</Button><Button onClick={() => onNavigate("intake")}>{t("자료 범위 변경")}</Button><Button tone="primary" onClick={onStart} disabled={!canStart}>{runStartState === "starting" ? t("심의 시작 중…") : t("이 동의로 심의 시작")}<span aria-hidden="true">↗</span></Button></div>
    </PageHeadingAndLayout>
  );
}

function RolesPage({ presetsState, diagnostic, presets, selectedPresetId, draft, writeState, onSelect, onBeginEdit, onClone, onDraftChange, onSave, onRetry, onBack }: {
  presetsState: RolePresetStoreState;
  diagnostic: RoleStoreDiagnostic | null;
  presets: RolePreset[];
  selectedPresetId: string | null;
  draft: RolePresetDraft | null;
  writeState: RolePresetWriteState;
  onSelect: (presetId: string) => void;
  onBeginEdit: (presetId: string) => void;
  onClone: (presetId: string) => void;
  onDraftChange: (draft: RolePresetDraft | null) => void;
  onSave: ScreenProps["onSaveRolePreset"];
  onRetry: ScreenProps["onRetryRolePresets"];
  onBack: ScreenProps["onReturnFromRoles"];
}) {
  const [activeCore, setActiveCore] = useState<CoreRole>("melchior");
  const selected = presets.find((preset) => preset.id === selectedPresetId);
  const editableDraft = draft && (draft.presetId === null || draft.presetId === selected?.id) ? draft : null;
  const mismatchedDraft = Boolean(draft && draft.presetId !== null && draft.presetId !== selected?.id);
  const visibleRoles = editableDraft?.roles ?? selected?.roles ?? [];
  const activeRole = visibleRoles.find((role) => role.core === activeCore);
  const roleCoreOrder: CoreRole[] = ["melchior", "balthasar", "casper"];
  const roleCoreNames: Record<CoreRole, string> = {
    melchior: "MELCHIOR·1",
    balthasar: "BALTHASAR·2",
    casper: "CASPER·3",
  };
  const originalSelves: Record<CoreRole, string> = {
    melchior: "Scientist · 과학자",
    balthasar: "Mother · 어머니",
    casper: "Woman · 여성",
  };
  const validRoles = hasThreeRoleSlots(visibleRoles);
  const completeRoles = validRoles && visibleRoles.every((role) => role.label.trim() && role.perspective.trim() && role.criteria.trim() && role.challengeCondition.trim());
  const canEditDraft = Boolean(editableDraft && presetsState === "ready" && writeState !== "saving" && writeState !== "conflict");
  const canSaveDraft = Boolean(canEditDraft && editableDraft?.name.trim() && completeRoles);

  const updateRole = (core: CoreRole, update: Partial<RoleDefinition>) => {
    if (!editableDraft) return;
    onDraftChange({ ...editableDraft, roles: editableDraft.roles.map((role) => role.core === core ? { ...role, ...update } : role) });
  };

  return (
    <PageHeadingAndLayout english="CORE ROLE EDITOR" title={t("세 관점 설정")} intro={t("세 코어의 정체성은 고정하고, 판단 관점·기준·반증 조건과 답변 언어를 프리셋으로 관리합니다.")}>
      <RoleTransfer selected={selected} onChanged={onRetry} />
      <div className="role-editor">
        <nav className="role-tabs" aria-label={t("역할 프리셋 선택")}>
          {presetsState === "ready" && presets.map((preset) => <button type="button" className={preset.id === selectedPresetId ? "active" : ""} key={preset.id} aria-pressed={preset.id === selectedPresetId} onClick={() => onSelect(preset.id)}>
            <strong>{preset.name}</strong><span>{preset.kind === "factory" ? t("기본 프리셋") : t("사용자 프리셋")} · rev {preset.revision}</span>
          </button>)}
          {presetsState === "ready" && presets.filter((preset) => preset.kind === "factory").length === 0 && <EmptyState title={t("기본 코어 프리셋 미확인")} text={t("원작의 세 코어 역할을 불러오지 못해 사용자 프리셋을 만들 수 없습니다.")} />}
        </nav>

        <div className="role-detail">
          {mismatchedDraft && <div className="inline-warning" role="alert"><strong>{t("편집 중인 역할과 선택된 프리셋이 다릅니다")}</strong><p>{t("선택이 바뀐 프리셋에 편집 초안을 적용하지 않았습니다. 저장된 선택을 다시 확인하거나 초안을 닫으십시오.")}</p><Button onClick={() => onDraftChange(null)}>{t("초안 닫기")}</Button></div>}
          {presetsState === "loading" && <Panel title={t("역할 프리셋 확인 중")} kicker="ROLE STORE"><EmptyState title={t("저장된 프리셋을 읽고 있습니다")} text={t("기본 프리셋이나 사용자 역할을 임의로 만들지 않습니다.")} /></Panel>}
          {presetsState === "unavailable" && <Panel title={t("역할 저장소를 사용할 수 없습니다")} kicker="ROLE STORE"><EmptyState title={t("역할을 읽거나 저장할 수 없습니다")} text={t("역할 서비스가 제공되지 않아 이 화면에서 프리셋을 변경할 수 없습니다.")} /></Panel>}
          {presetsState === "error" && <Panel title={t("역할 프리셋을 읽지 못했습니다")} kicker="ROLE STORE">
            <EmptyState title={t("역할 상태 미확인")} text={t("저장된 역할과 기본 프리셋을 불러오지 못했습니다. 확인되지 않은 역할을 표시하지 않습니다.")} />
            <p className="field-help" role="status">{t("진단 코드")}<code>{diagnostic?.code ?? "role_store_unavailable"}</code>{t("· 단계")}<code>{roleStoreStageLabels[diagnostic?.stage ?? "unknown"]}</code>
            </p>
            <div className="page-actions">
              <Button tone="primary" onClick={onRetry}>{t("저장소 다시 확인")}</Button>
              <Button onClick={onBack}>{t("이전 화면으로 돌아가기")}</Button>
            </div>
          </Panel>}
          {presetsState === "ready" && presets.length === 0 && <Panel title={t("사용할 수 있는 역할 프리셋 없음")} kicker="ROLE STORE"><EmptyState title={t("기본 프리셋 미확인")} text={t("기본 코어 자아를 확인하지 못해 프리셋을 생성하거나 심의에 배정할 수 없습니다.")} /></Panel>}
          {presetsState === "ready" && selected && <>
            <Panel title={editableDraft ? editableDraft.presetId ? t("사용자 프리셋 편집") : t("새 사용자 프리셋") : selected.name} kicker={editableDraft ? editableDraft.presetId ? `USER PRESET / REVISION ${selected.revision}` : "NEW USER PRESET / CLONED TEMPLATE" : selected.kind === "factory" ? "FACTORY / READ ONLY" : `USER PRESET / REVISION ${selected.revision}`}>
              <div className="role-core-tabs" role="tablist" aria-label={t("편집할 MAGI 코어")}>
                {roleCoreOrder.map((core) => <button type="button" role="tab" aria-selected={activeCore === core} className={activeCore === core ? "active" : ""} key={core} onClick={() => setActiveCore(core)}>{roleCoreNames[core]}</button>)}
              </div>
              {editableDraft && <label className="field role-preset-name"><span className="field-label">{t("프리셋 이름")}</span><input className="text-field" value={editableDraft.name} disabled={writeState === "saving"} onChange={(event) => onDraftChange({ ...editableDraft, name: event.target.value })} /></label>}
              {activeRole && <RoleFields role={activeRole} originalSelf={originalSelves[activeRole.core]} readOnly={!editableDraft} disabled={writeState === "saving"} onChange={(update) => updateRole(activeRole.core, update)} />}
              {!activeRole && <EmptyState title={t("코어 역할을 확인하지 못했습니다")} text={t("선택된 프리셋에 세 개의 서로 다른 코어 역할이 있어야 합니다. 잘못된 프리셋은 저장하거나 실행할 수 없습니다.")} />}
              <p className="role-identity-note">{t("Scientist·Mother·Woman은 원작의 코어 자아를 가리킵니다. 판단 기준은 성별·모성·직업 고정관념을 강제하지 않습니다.")}</p>
              {selected.kind === "factory" && !editableDraft && <div className="page-actions"><Button tone="primary" onClick={() => onClone(selected.id)}>{t("복제하여 편집")}</Button></div>}
              {selected.kind === "user" && !editableDraft && <div className="page-actions"><Button tone="primary" onClick={() => onBeginEdit(selected.id)}>{t("편집 시작")}</Button></div>}
              {editableDraft && <div className="role-save-actions">
                <Button onClick={() => onDraftChange(null)} disabled={writeState === "saving"}>{t("편집 닫기")}</Button>
                <Button tone="primary" onClick={() => onSave({ presetId: editableDraft.presetId, expectedRevision: editableDraft.expectedRevision, draft: editableDraft })} disabled={!canSaveDraft}>{t("새 revision 저장")}</Button>
              </div>}
              {writeState === "saving" && <p className="field-help" role="status">{t("역할 revision을 저장하고 있습니다.")}</p>}
              {writeState === "saved" && <p className="field-help" role="status">{t("역할 프리셋이 저장되었습니다. 진행 중 심의는 시작 시 고정한 역할을 유지합니다.")}</p>}
              {writeState === "conflict" && <div className="inline-warning" role="alert"><strong>{t("역할 revision 충돌")}</strong><p>{t("열어 둔 revision 이후 저장 내용이 바뀌었습니다. 최신 사용자 프리셋을 다시 읽어 수정하십시오.")}</p>{editableDraft?.presetId && selected.kind === "user" && <Button onClick={() => onBeginEdit(selected.id)}>{t("최신 revision 다시 읽기")}</Button>}</div>}
              {writeState === "error" && <p className="unavailable-reason" role="alert">{t("역할 프리셋을 저장하지 못했습니다. 편집 내용은 유지했습니다.")}</p>}
              {editableDraft && !completeRoles && <p className="field-help">{t("세 코어의 이름·관점·기준·반증 조건을 모두 입력해야 저장할 수 있습니다.")}</p>}
            </Panel>
          </>}
          {presetsState === "ready" && !selected && presets.length > 0 && <Panel title={t("역할 프리셋을 선택하십시오")} kicker="CORE ROLE EDITOR"><EmptyState title={t("선택된 역할 없음")} text={t("기본 프리셋은 읽기 전용으로 확인하고, 사용자 프리셋은 선택해 revision을 편집합니다.")} /></Panel>}
        </div>
      </div>
    </PageHeadingAndLayout>
  );
}

function RoleFields({ role, originalSelf, readOnly, disabled, onChange }: {
  role: RoleDefinition;
  originalSelf: string;
  readOnly: boolean;
  disabled: boolean;
  onChange: (update: Partial<RoleDefinition>) => void;
}) {
  return <div className="role-fields">
    <div className="role-self"><span>CORE IDENTITY</span><strong>{originalSelf}</strong></div>
    <label className="field"><span className="field-label">{t("관점 이름")}</span><input className="text-field" value={role.label} readOnly={readOnly} disabled={disabled} onChange={(event) => onChange({ label: event.target.value })} /></label>
    <label className="field"><span className="field-label">{t("목적")}</span><textarea className="text-field" value={role.perspective} readOnly={readOnly} disabled={disabled} onChange={(event) => onChange({ perspective: event.target.value })} /></label>
    <label className="field"><span className="field-label">{t("우선 판단 기준")}</span><textarea className="text-field" value={role.criteria} readOnly={readOnly} disabled={disabled} onChange={(event) => onChange({ criteria: event.target.value })} /></label>
    <label className="field"><span className="field-label">{t("의문을 제기할 조건")}</span><textarea className="text-field" value={role.challengeCondition} readOnly={readOnly} disabled={disabled} onChange={(event) => onChange({ challengeCondition: event.target.value })} /></label>
    <label className="field"><span className="field-label">{t("출력 언어")}</span><select className="text-field" value={role.outputLanguage} disabled={readOnly || disabled} onChange={(event) => onChange({ outputLanguage: event.target.value as RoleOutputLanguage })}><option value="same">{t("안건 언어와 동일")}</option><option value="ko">{t("한국어")}</option><option value="en">{t("영어")}</option></select></label>
  </div>;
}

function hasThreeRoleSlots(roles: RoleDefinition[]): boolean {
  const expected: CoreRole[] = ["melchior", "balthasar", "casper"];
  return roles.length === expected.length && expected.every((core) => roles.filter((role) => role.core === core).length === 1);
}

function SettingsPage({ commonContextTokenLimit, onCommonContextTokenLimitChange, locale, onLocaleChange, onBack, motion, sound, theme, fontScale, onMotionChange, onSoundChange, onThemeChange, onFontScaleChange, onNavigate, onNotice }: { commonContextTokenLimit: number; onCommonContextTokenLimitChange: (value: number) => void; locale: Locale; onLocaleChange: (locale: Locale) => void; onBack: () => void; motion: MotionSetting; sound: boolean; theme: "command" | "clear"; fontScale: 100 | 125 | 150 | 200; onMotionChange: (value: MotionSetting) => void; onSoundChange: (value: boolean) => void; onThemeChange: (value: "command" | "clear") => void; onFontScaleChange: (value: 100 | 125 | 150 | 200) => void; onNavigate: ScreenProps["onNavigate"]; onNotice: (message: string) => void }) {
  const [commonLimitDraft, setCommonLimitDraft] = useState(String(commonContextTokenLimit));
  const [commonLimitEditing, setCommonLimitEditing] = useState(false);
  const [commonLimitDirty, setCommonLimitDirty] = useState(false);
  const parsedCommonLimit = Number(commonLimitDraft);
  const commonLimitValid = commonLimitDraft.trim() !== "" && Number.isSafeInteger(parsedCommonLimit) && parsedCommonLimit >= 1 && parsedCommonLimit <= 128000;
  useEffect(() => {
    if (!commonLimitEditing && !commonLimitDirty) {
      setCommonLimitDraft(String(commonContextTokenLimit));
    }
  }, [commonContextTokenLimit, commonLimitEditing, commonLimitDirty]);
  const commitCommonLimit = () => {
    setCommonLimitEditing(false);
    if (!commonLimitValid) return;
    setCommonLimitDirty(false);
    onCommonContextTokenLimitChange(parsedCommonLimit);
  };
  return (
    <PageHeadingAndLayout english="CONSOLE SETTINGS" title={t("콘솔 설정")} intro={t("시각 표현과 접근성 설정을 변경해도 현재 질문·근거 선택은 유지됩니다.")}>
      <Panel title={t("시각 표현")} kicker="DISPLAY">
        <fieldset className="setting-row"><legend>{t("테마")}</legend><label><input type="radio" name="theme" checked={theme === "command"} onChange={() => { onThemeChange("command"); onNotice(t("Command 테마를 적용했습니다.")); }} /> Command</label><label><input type="radio" name="theme" checked={theme === "clear"} onChange={() => { onThemeChange("clear"); onNotice(t("Clear 테마를 적용했습니다.")); }} /> Clear</label></fieldset>
        <label className="setting-row"><span><strong>{t("동작")}</strong><small>{t("OS 설정을 최초 표시에도 반영합니다.")}</small></span><select value={motion} onChange={(event) => onMotionChange(event.target.value as MotionSetting)}><option value="full">{t("전체 동작")}</option><option value="reduced">{t("동작 줄임")}</option><option value="off">{t("동작 끔")}</option></select></label>
        <label className="setting-row"><span><strong>{t("음향")}</strong><small>{t("켜면 짧은 직접 제작 미리보기만 한 번 재생합니다. 입력·응답 이벤트 음향은 재생하지 않습니다.")}</small></span><input type="checkbox" checked={sound} onChange={(event) => onSoundChange(event.target.checked)} /></label>
        <label className="setting-row"><span><strong>{t("글자 확대")}</strong><small>{t("화면의 글자와 내용이 함께 커지고, 필요한 영역은 다시 배치됩니다.")}</small></span><select aria-label={t("글자 확대")} value={fontScale} onChange={(event) => onFontScaleChange(Number(event.target.value) as 100 | 125 | 150 | 200)}><option value={100}>100%</option><option value={125}>125%</option><option value={150}>150%</option><option value={200}>200%</option></select></label>
      </Panel>
      <Panel title={locale === "en" ? "Common input budget" : "공통 입력 예산"} kicker="INPUT BUDGET">
        <label className="setting-row"><span><strong id="common-context-limit-label">{locale === "en" ? "Common context token limit" : "공통 문맥 토큰 상한"}</strong><small id="common-context-limit-range">1–128,000</small></span><input type="number" min={1} max={128000} step={1} value={commonLimitDraft} aria-invalid={!commonLimitValid} aria-labelledby="common-context-limit-label" aria-describedby={commonLimitValid ? "common-context-limit-range" : "common-context-limit-range common-context-limit-feedback"} onFocus={() => setCommonLimitEditing(true)} onChange={event => { setCommonLimitDraft(event.currentTarget.value); setCommonLimitDirty(true); }} onBlur={commitCommonLimit} onKeyDown={event => { if (event.key === "Enter") { event.preventDefault(); event.currentTarget.blur(); } }} /></label>
        {!commonLimitValid && <p id="common-context-limit-feedback" role="alert">{locale === "en" ? "Enter a whole number from 1 to 128,000." : "1부터 128,000까지의 정수를 입력하세요."}</p>}
      </Panel>
      <Panel title={t("언어")} kicker="LANGUAGE">
        <label className="setting-row"><span><strong>{t("콘솔 언어")}</strong><small>{t("화면 언어는 답변 언어와 별도로 저장됩니다.")}</small></span><select aria-label={t("콘솔 언어")} value={locale} onChange={event => onLocaleChange(event.target.value as Locale)}><option value="ko">{t("한국어")}</option><option value="en">English</option></select></label>
        <div className="setting-row"><span><strong>{t("답변 언어")}</strong><small>{t("답변 언어 설정은 각 역할 프로필에 속합니다.")}</small></span><Button onClick={() => onNavigate("roles")}>{t("역할 설정 열기")}</Button></div>
      </Panel>
      <Panel title={t("모델 연결")} kicker="CONNECTIONS"><p>{t("구독 프로필·연결 확인·모델 설정은 모델 연결에서 관리합니다.")}</p><Button onClick={() => onNavigate("connections")}>{t("모델 연결 관리")}</Button></Panel>
      <div className="page-actions"><Button onClick={onBack}>{t("원래 화면으로 돌아가기")}</Button></div>
      <p className="field-help" role="status">{t("테마·동작·음향·글자 확대는 변경할 때마다 이 Mac의 앱 로컬 설정으로 저장됩니다.")}</p>
    </PageHeadingAndLayout>
  );
}

function HistoryPage({ onClarificationParentVerified, onDiscardClarificationDraft, onClarificationDraftChanged, onConfirmClarificationDraft, evidenceReadingTarget, onEvidenceReadingTargetChange, data, selectedRunId, selectedDossier, dossierState, storageDiagnostic, onNavigate, onSelectRun, onDeleted, onRefreshRunStatus }: {
  evidenceReadingTarget?: EvidenceReadingTarget | null;
  onEvidenceReadingTargetChange?: (target: EvidenceReadingTarget) => void;
  data: HomeData;
  selectedRunId: string | null;
  onClarificationDraftChanged?: (draft: ClarificationDraft | null) => void;
  onClarificationParentVerified?: (parent: ClarificationParent | null) => void;
  onDiscardClarificationDraft?: (action: () => void) => void;
  onConfirmClarificationDraft?: (draft: ClarificationDraft) => void;
  selectedDossier: RunDossierView | null;
  dossierState: ScreenProps["selectedRunDossierState"];
  storageDiagnostic: ConsoleSnapshot["storageDiagnostic"];
  onNavigate: ScreenProps["onNavigate"];
  onSelectRun: ScreenProps["onOpenRecentRun"];
  onDeleted: ScreenProps["onRecordDeleted"];
  onRefreshRunStatus: ScreenProps["onRefreshRunStatus"];
}) {
  const selectedRun = data.recentRuns.find((run) => run.id === selectedRunId) ?? (selectedDossier ? { id: selectedDossier.runId, question: selectedDossier.question, status: selectedDossier.status, createdAt: "" } : undefined);
  const terminal = isTerminalRunStatus(selectedDossier?.status);
  return (
    <PageHeadingAndLayout english="LOCAL RECORDS" title={t("대화·기록")} intro={t("저장소가 실제로 제공한 안건·상태·시각만 표시합니다.")}>
      <StorageUnavailableNotice diagnostic={storageDiagnostic} />
      <div className="history-browser">
        <RecordBrowser selectedRunId={selectedRunId} onSelectRun={onSelectRun} onReplay={() => onNavigate("replay")} onDeleted={onDeleted} />
        <section className="history-summary panel" aria-labelledby="history-summary-title">
          <p className="panel-kicker">SAVED RUN SUMMARY</p>
          <h3 id="history-summary-title">{selectedRun ? t("선택한 심의") : t("기록 요약")}</h3>
          {selectedRun ? <>
            <dl className="history-summary-fields">
              <div><dt>{t("상태")}</dt><dd>{runStatusLabel(selectedRun.status)}</dd></div>
              <div><dt>{t("기록 시각")}</dt><dd>{formatRecordDate(selectedRun.createdAt)}</dd></div>
            </dl>
            <p className="history-summary-question">{selectedRun.question || t("안건 없음")}</p>
            {dossierState === "loading" && <p className="field-help" role="status">{t("선택한 심의의 실제 저장 상태를 확인하고 있습니다.")}</p>}
            {dossierState === "error" && <p className="unavailable-reason" role="alert">{t("심의 상세를 읽지 못했습니다. 목록의 실제 요약만 표시합니다.")}</p>}
            {dossierState === "ready" && !selectedDossier && <p className="unavailable-reason" role="alert">{t("선택한 심의의 저장 상세를 확인할 수 없습니다.")}</p>}
            {selectedDossier && <>
              <dl className="history-summary-fields">
                <div><dt>{t("저장 상태")}</dt><dd>{runStatusLabel(selectedDossier.status)}</dd></div>
                <div><dt>{t("저장 단계")}</dt><dd>{runStageLabel(selectedDossier.stage)}</dd></div>
              </dl>
              {selectedDossier.status === "paused" && <RunClarificationPanel onParentVerified={onClarificationParentVerified} onRequestDiscard={onDiscardClarificationDraft} onDraftChanged={onClarificationDraftChanged} onConfirmDraft={onConfirmClarificationDraft} runId={selectedDossier.runId} dossier={selectedDossier} onInspectParent={() => onNavigate("replay")} />}
              {selectedDossier.error && <div className="unavailable-reason" role="alert"><strong>{selectedDossier.error.code}</strong><p>{selectedDossier.error.message}</p></div>}
              {selectedDossier.proposal && <>
                <h4>{selectedDossier.status === "completed" ? t("저장된 결의안") : t("저장된 제안 초안")}</h4>
                <ProposalContent proposal={selectedDossier.proposal} />
              </>}
              {terminal && selectedDossier.status === "completed" && <>
                <h4>{t("최종 표결 ·")}{outcomeLabel(selectedDossier.outcome)}</h4>
                <VoteDetails votes={selectedDossier.votes} />
                <DissentContent dossier={selectedDossier} />
              </>}
              {terminal && selectedDossier.status !== "completed" && selectedDossier.votes.length > 0 && <>
                <h4>{t("종료 전에 저장된 의견")}</h4>
                <VoteDetails votes={selectedDossier.votes} />
              </>}
              {!terminal && selectedDossier.status !== "completed" && <p className="field-help">{t("심의가 종료되지 않았습니다. 최종 표결은 저장된 완료 상태가 확인될 때까지 공개하지 않습니다.")}</p>}
            </>}
            <div className="page-actions">
              <Button onClick={() => onRefreshRunStatus(selectedRun.id)} disabled={dossierState === "loading"}>{t("저장 상태 새로고침")}</Button>
            </div>
          </> : <EmptyState title={t("심의를 선택하십시오")} text={selectedRunId ? t("선택한 기록이 현재 목록에 없어 요약을 표시할 수 없습니다.") : t("기록 행을 선택하면 저장된 질문·상태·시각을 확인할 수 있습니다.")} />}
        </section>
      </div>
      <RecordEvidenceBrowser runId={selectedDossier?.runId === selectedRunId ? selectedRunId : null} revision={selectedDossier?.runId === selectedRunId ? selectedDossier.revision : undefined} onChanged={onRefreshRunStatus} target={evidenceReadingTarget} onTargetChange={onEvidenceReadingTargetChange} />
      <div className="page-actions"><Button tone="primary" onClick={() => onNavigate("input")}>{t("새 안건")}</Button><Button onClick={() => onNavigate("data")}>{t("자료·보존")}</Button></div>
    </PageHeadingAndLayout>
  );
}

function ReplayPage({ runId, dossier, onNavigate }: { runId: string | null; dossier: RunDossierView | null; onNavigate: ScreenProps["onNavigate"] }) {
  return (
    <PageHeadingAndLayout english="READ-ONLY REPLAY" title={t("기록 재생")} intro={t("저장된 공개 이벤트를 읽기 전용으로 재현합니다. 재생은 새 모델 호출을 만들지 않습니다.")}>
      <div className="replay-stamp"><strong>READ-ONLY REPLAY</strong><span>{t("실제 기록만 재생")}</span></div>
      <StoredReplay runId={runId} dossier={dossier} />
      <ExternalReplayBrowser />
      <div className="page-actions"><Button onClick={() => onNavigate("history")}>{t("기록 목록")}</Button><Button tone="primary" disabled={!dossier || dossier.status !== "completed"} onClick={() => onNavigate("share")}>{t("공유 미리보기")}<span aria-hidden="true">↗</span></Button></div>
    </PageHeadingAndLayout>
  );
}

function DataPage() {
  return (
    <PageHeadingAndLayout english="DATA & RETENTION" title={t("자료·보존")} intro={t("로컬 자료 권한·외부 전송 동의·수집본 보존은 서로 다른 상태입니다.")}>
      <Panel title={t("접근 권한")} kicker="SOURCE PERMISSIONS"><EmptyState title={t("권한 상태 미확인")} text={t("자료를 읽었다거나 권한을 철회했다는 상태를 추정하지 않습니다.")} /></Panel>
      <Panel title={t("기록 보존")} kicker="LOCAL RETENTION"><div className="data-metric"><strong>{t("미확인")}</strong><span>{t("로컬 기록 수")}</span><strong>{t("미확인")}</strong><span>{t("보존 원문 수")}</span></div></Panel>
      <RecordBackup />
      <RestoreDataPanel />
    </PageHeadingAndLayout>
  );
}

function MaintenancePage({ onNavigate }: { onNavigate: ScreenProps["onNavigate"] }) {
  return (
    <PageHeadingAndLayout english="MAINTENANCE" title={t("유지관리")} intro={t("업데이트·진단·앱 정보는 실제 서명과 저장소 상태를 검증한 뒤 제공합니다.")}>
      <SignedUpdatePanel />
      <Panel title={t("진단 정보")} kicker="LOCAL ONLY"><p>{t("질문·파일 내용·모델 응답·비밀은 진단에 포함하지 않습니다.")}</p><Button onClick={() => onNavigate("data")}>{t("자료·보존 설정")}</Button></Panel>
      <Panel title={t("앱 정보")} kicker="MAGI CONSOLE"><p>{t("무료 팬 창작 앱 · 코드와 문서 MIT")}</p><p>{t("원작 명칭·이미지·음원에 대한 권리는 코드 라이선스에 포함되지 않습니다.")}</p></Panel>
    </PageHeadingAndLayout>
  );
}

function CompanionPage({ run, snapshot, nativeWindow, runId, progress, dossier, cancelRequest, onCancelRun, onRefreshRunStatus, onNavigate, onOpenConsole, onOpenSettings, onClose, onRequestExit }: {
  run?: ConsoleRunSummary;
  snapshot: ConsoleSnapshot;
  nativeWindow: boolean;
  runId: string | null;
  progress: RunProgressSummary | null;
  dossier: RunDossierView | null;
  cancelRequest: CancelRequestState;
  onCancelRun: ScreenProps["onCancelRun"];
  onRefreshRunStatus: ScreenProps["onRefreshRunStatus"];
  onNavigate: ScreenProps["onNavigate"];
  onOpenConsole: () => void;
  onOpenSettings: () => void;
  onClose: () => void;
  onRequestExit: () => void;
}) {
  const currentRun = run && run.id === runId ? run : undefined;
  const currentDossier = dossier?.runId === runId ? dossier : null;
  const currentProgress = progress?.runId === runId ? progress : null;
  const actualStatus = currentDossier?.status ?? currentRun?.status;
  const terminal = isTerminalRunStatus(actualStatus);
  const running = Boolean(runId && !terminal);
  const stage = currentProgress?.stage ?? currentDossier?.stage ?? currentRun?.stage;
  const phase = stage ? runStageLabel(stage) : running ? "상태 확인 중" : snapshot.connection === "blocked" ? "ACP 실행 환경 차단됨" : snapshot.connection === "unknown" ? "상태 확인 중" : "안건 대기";
  const coreScreen = stage || actualStatus ? stageScreen(stage, actualStatus) : screenForRun(currentRun);
  const question = currentDossier?.question ?? currentRun?.question;
  const progressLabel = currentProgress?.coreId
    ? `${coreDisplayName(currentProgress.coreId)} · ${currentProgress.state === "streaming" ? "응답 생성 중" : currentProgress.state === "started" ? "단계 시작" : currentProgress.state === "completed" ? "단계 완료" : "상태 확인 중"}`
    : currentProgress ? `${phase} · ${currentProgress.state === "streaming" ? "실행 중" : "상태 확인 중"}` : null;
  return (
    <section className="companion-card" role="region" aria-label={t("MAGI 상태 팝오버")} tabIndex={0}>
      <header className="companion-header"><span className="companion-icon" aria-hidden="true">M</span><div><p>MAGI CONSOLE</p><strong>{phase}</strong></div><span className={`status-chip ${runtimeAvailabilityClass(snapshot.connection)}`}>{runtimeAvailabilityLabel(snapshot.connection)}</span></header>
      <CoreTopology screen={coreScreen} run={currentRun} onOpenCore={onOpenConsole} compact />
      <section className="companion-agenda"><span>{running ? t("현재 안건") : question ? t("최근 확인 안건") : t("현재 안건")}</span><strong>{question ?? (running ? t("실행 상태 확인 중") : t("안건 대기"))}</strong><p>{actualStatus ? runStatusLabel(actualStatus) : running ? phase : t("실행 중인 심의가 없습니다.")}</p>{progressLabel && <small role="status" aria-live="polite">{progressLabel}</small>}</section>
      {runId && running && <InlineCancelControl runId={runId} active={running} cancelRequest={cancelRequest} onCancel={onCancelRun} />}
      <div className="companion-actions">
        {runId && <Button onClick={() => onRefreshRunStatus(runId)}>{t("상태 새로고침")}</Button>}
        <Button tone="primary" onClick={nativeWindow ? onOpenConsole : () => onNavigate("input")}>{t("콘솔 열기")}</Button>
        {nativeWindow && <Button onClick={onOpenSettings}>{t("설정")}</Button>}
        {nativeWindow && <Button onClick={onClose}>{t("팝오버 닫기")}</Button>}
        {nativeWindow && <Button onClick={onRequestExit}>{t("앱 종료")}</Button>}
      </div>
      <p className="companion-note">{t("팝오버 표시·닫기는 모델 호출과 파일 접근을 만들지 않습니다.")}</p>
    </section>
  );
}

export function CompanionSurface({ snapshot, runId, progress, dossier, cancelRequest, onCancelRun, onRefreshRunStatus, onOpenConsole, onOpenSettings, onClose, onRequestExit }: {
  snapshot: ConsoleSnapshot;
  runId: string | null;
  progress: RunProgressSummary | null;
  dossier: RunDossierView | null;
  cancelRequest: CancelRequestState;
  onCancelRun: ScreenProps["onCancelRun"];
  onRefreshRunStatus: ScreenProps["onRefreshRunStatus"];
  onOpenConsole: () => void;
  onOpenSettings: () => void;
  onClose: () => void;
  onRequestExit: () => void;
}) {
  return <main className="companion-window" aria-label={t("MAGI 상태 창")}><CompanionPage run={snapshot.activeRun} snapshot={snapshot} nativeWindow runId={runId} progress={progress} dossier={dossier} cancelRequest={cancelRequest} onCancelRun={onCancelRun} onRefreshRunStatus={onRefreshRunStatus} onNavigate={() => undefined} onOpenConsole={onOpenConsole} onOpenSettings={onOpenSettings} onClose={onClose} onRequestExit={onRequestExit} /></main>;
}

function runtimeAvailabilityLabel(state: ConsoleSnapshot["connection"]): string {
  return state === "runtime_available" ? "ACP 실행 환경 사용 가능" : state === "blocked" ? "ACP 실행 환경 차단됨" : "ACP 실행 환경 미확인";
}

function runtimeAvailabilityClass(state: ConsoleSnapshot["connection"]): string {
  return state === "runtime_available" ? "chip-support" : state === "blocked" ? "chip-oppose" : "chip-unknown";
}

function StorageUnavailableNotice({ diagnostic }: { diagnostic: ConsoleSnapshot["storageDiagnostic"] }) {
  if (!diagnostic) return null;
  return (
    <div className="inline-warning" role="alert">
      <strong>{diagnostic.message}</strong>
      <p>{diagnostic.action}</p>
      <small>{t("진단 코드 ·")}{diagnostic.code}</small>
    </div>
  );
}

function RecoveryPage({ onClarificationParentVerified, onDiscardClarificationDraft, onConfirmClarificationDraft, onClarificationDraftChanged, screen, run, dossier, runId, cancelRequest, onCancelRun, onRefreshRunStatus, runDossierState, onNavigate }: {
  onClarificationParentVerified?: (parent: ClarificationParent | null) => void;
  onDiscardClarificationDraft?: (action: () => void) => void;
  onConfirmClarificationDraft?: (draft: ClarificationDraft) => void;
  onClarificationDraftChanged?: (draft: ClarificationDraft | null) => void;
  screen: string;
  run?: ConsoleRunSummary;
  dossier: RunDossierView | null;
  runId: string | null;
  cancelRequest: CancelRequestState;
  onCancelRun: ScreenProps["onCancelRun"];
  onRefreshRunStatus: ScreenProps["onRefreshRunStatus"];
  runDossierState: ScreenProps["runDossierState"];
  onNavigate: ScreenProps["onNavigate"];
}) {
  const title = screenCatalog.find((item) => item.id === screen)?.title ?? "상태 확인";
  const currentDossier = dossier?.runId === runId ? dossier : null;
  const actualStatus = currentDossier?.status ?? run?.status;
  const terminal = isTerminalRunStatus(actualStatus);
  const active = Boolean(runId && !terminal);
  const stage = currentDossier?.stage ?? run?.stage;
  const question = currentDossier?.question ?? run?.question;
  return (
    <PageHeadingAndLayout english={screenCatalog.find((item) => item.id === screen)?.english ?? "RUN RECOVERY"} title={title} intro={t("실패·중단·취소는 실제 저장 상태로 표시하며 서로 바꾸어 추정하지 않습니다.")}>
      <div className={`recovery-panel recovery-${screen}`}><span className="recovery-symbol" aria-hidden="true">{actualStatus === "failed" ? "!" : "◇"}</span><div><p className="panel-kicker">{actualStatus ? `RUN STATUS · ${actualStatus}` : "RUN STATUS · UNVERIFIED"}</p><h3>{actualStatus ? runStatusLabel(actualStatus) : t("실행 상태를 확인하지 못했습니다")}</h3><p>{stage ? `마지막 저장 단계: ${runStageLabel(stage)} · ${stage}` : t("저장된 실행 상태를 불러오지 못했습니다. 완료·실패·취소를 추정하지 않습니다.")}</p></div></div>
      {runDossierState === "loading" && <p className="field-help" role="status">{t("저장된 dossier를 다시 확인하고 있습니다.")}</p>}
      {runDossierState === "error" && <p className="unavailable-reason" role="alert">{t("저장된 실행 상세를 읽지 못했습니다. 마지막으로 확인된 요약 상태만 표시합니다.")}</p>}
      {actualStatus === "paused" && runId && <RunClarificationPanel onParentVerified={onClarificationParentVerified} onRequestDiscard={onDiscardClarificationDraft} onConfirmDraft={onConfirmClarificationDraft} onDraftChanged={onClarificationDraftChanged} runId={runId} dossier={currentDossier} onInspectParent={() => onNavigate("evidence")} />}
      {currentDossier?.error && <div className="unavailable-reason" role="alert"><strong>{currentDossier.error.code}</strong><p>{currentDossier.error.message}</p></div>}
      <Panel title={t("보존된 입력과 부분 결과")} kicker="RECOVERY CHECKPOINT">
        <p>{question ?? t("저장 상태 미확인")}</p>
        <ul className="contract-list"><li>{t("마지막 확인 단계:")}{stage ? `${runStageLabel(stage)} · ${stage}` : t("미확인")}</li>{run && <li>{t("캡처 자료 수:")}{run.sourceCount}{t("개")}</li>}<li>{t("동일 요청 자동 재전송: 하지 않음")}</li></ul>
        {currentDossier?.proposal && <><h4>{currentDossier.status === "completed" ? t("저장된 결의안") : t("저장된 제안 초안")}</h4><ProposalContent proposal={currentDossier.proposal} /></>}
        {currentDossier && ["failed", "cancelled"].includes(currentDossier.status) && currentDossier.votes.length > 0 && <><h4>{t("종료 전에 저장된 의견")}</h4><VoteDetails votes={currentDossier.votes} /></>}
      </Panel>
      {runId && <InlineCancelControl runId={runId} active={active} cancelRequest={cancelRequest} onCancel={onCancelRun} />}
      <div className="page-actions"><Button onClick={() => onNavigate("history")}>{t("기록 목록")}</Button><Button onClick={() => onNavigate("connections")}>{t("연결 설정")}</Button><Button tone="primary" onClick={() => runId && onRefreshRunStatus(runId)} disabled={!runId || runDossierState === "loading"}>{t("상태 다시 확인")}</Button></div>
    </PageHeadingAndLayout>
  );
}

function EmptyState({ title, text }: { title: string; text: string }) {
  return <div className="empty-state"><span className="empty-glyph" aria-hidden="true">◇</span><div><strong>{title}</strong><p>{text}</p></div></div>;
}

function CoreTopology({ screen, run, onOpenCore, compact = false }: { screen: string; run?: ConsoleRunSummary; onOpenCore?: (core: string) => void; compact?: boolean }) {
  const hasPublicVotes = run?.ballotState === "public" && run.votes?.length === 3;
  const publicResult = ["verdict", "evidence"].includes(screen) && hasPublicVotes === true;
  const sealed = screen === "sealed";
  const stage = stageLabels[screen] ?? stageLabels.input;
  const hubStatus = publicResult
    ? compact ? "3/3" : "3/3 VOTES"
    : sealed || run?.ballotState === "sealed"
      ? "SEALED"
      : compact ? compactHubLabels[screen] ?? stage.hub : stage.hub;
  const votes = publicResult ? run?.votes : undefined;
  const coreRole = (id: CoreId) => {
    if (!run) return undefined;
    const keys: Record<CoreId, string[]> = {
      melchior: ["MELCHIOR-1", "MELCHIOR·1", "melchior"],
      balthasar: ["BALTHASAR-2", "BALTHASAR·2", "balthasar"],
      casper: ["CASPER-3", "CASPER·3", "casper"],
    };
    const match = run.roles.find((roleItem) => keys[id].includes(roleItem.core));
    return match?.label;
  };
  return (
    <div className={`core-topology ${compact ? "topology-compact" : ""} ${publicResult ? "topology-result" : ""}`} data-stage={screen} aria-describedby="topology-summary">
      <div className="topology-art" aria-hidden="true">
        <svg className="topology-svg" viewBox="0 0 1360 600" focusable="false">
          {cores.map((core) => <polygon key={core.id} className={`core-face core-${core.id}`} data-vote={publicResult ? votes?.[core.voteIndex] : undefined} points={core.polygon} />)}
          <path className={`core-circuit ${screen === "independent" ? "circuit-isolated" : ""}`} d="M240 175 L500 337 L606 337 L628 325 L732 325 L754 337 L860 337 L1120 175 M230 220 L566 427 L650 540 L650 600 M1130 220 L794 427 L710 540 L710 600 M680 490 L680 555 L651 580 M680 555 L709 580" />
          {screen === "review" && <path className="cross-signal" d="M256 189 L508 346 L625 346 M1104 189 L852 346 L735 346 M680 491 L680 588" />}
          <polygon className="topology-hub-shape" points="630,342 730,342 765,385 720,465 680,490 640,465 595,385" />
          <polygon className="topology-hub-inner" points="634,351 726,351 754,386 713,458 680,478 647,458 606,386" />
        </svg>
        <div className="topology-hub"><strong>MAGI</strong><span>{hubStatus}</span></div>
      </div>
      {cores.map((core) => {
        const vote = votes?.[core.voteIndex] ?? "abstain";
        const dispatch = run?.dispatchProjection?.runId === run?.id ? run?.dispatchProjection : undefined;
        const coreSlots = dispatch?.coreDispatches.filter(slot => slot.bindingCoreId === ["MELCHIOR-1", "BALTHASAR-2", "CASPER-3"][core.voteIndex]);
        const currentSlot = coreSlots?.find(slot => slot.stage === dispatch?.stage);
        const slotStatus = !currentSlot ? t("요청 상태 미확인") : currentSlot.state === "settled" ? currentSlot.resultRef ? t("공개 산출물 저장됨") : t("요청 종료 · 산출물 미확인") : currentSlot.state === "active" ? t("실행 중") : currentSlot.state === "reserved" ? t("실행 대기열") : currentSlot.state === "released" ? t("요청 해제됨") : t("요청 상태 미확인");
          const status = publicResult ? voteNames[vote] : sealed || run?.ballotState === "sealed" ? t("표 봉인됨") : screen === "input" && !run ? t("안건 대기") : run ? slotStatus : stage.korean;
        const role = coreRole(core.id) ?? core.role;
        return (
          <div key={core.id} className={`core-slot core-slot-${core.id}`}>
            <span className="core-index" aria-hidden="true"><small>CORE</small>{core.number}</span>
            <button type="button" className={`core-control ${publicResult ? `vote-${vote}` : ""}`} onClick={() => onOpenCore?.(core.name)} disabled={!onOpenCore} aria-label={`${core.name}, ${role}, ${sealed ? t("표 방향 비공개, 봉인됨") : publicResult ? voteNames[vote] : status}, ${onOpenCore ? compact ? t("콘솔 열기") : t("상세 열기") : t("상세 정보 미제공")}`}>
              <span className="core-name">{core.name}</span>
              <span className="core-status">
                <span className="core-status-jp" lang="ja" aria-hidden="true">{publicResult ? voteJapanese[vote] : sealed ? "封印" : stage.japanese}</span>
                <strong>{status}</strong>
              </span>
              <span className="core-role">{role}</span>
              {run && <span className="core-focus">{run.dispatchProjection?.runId === run.id ? run.dispatchProjection.coreDispatches.filter(slot => slot.bindingCoreId === ["MELCHIOR-1", "BALTHASAR-2", "CASPER-3"][core.voteIndex]).map(slot => <span key={slot.slotOrdinal}>{slot.slotOrdinal} · {slot.stage} · {slot.state === "settled" ? slot.resultRef ? t("공개 산출물 저장됨") : t("요청 종료 · 산출물 미확인") : slot.state === "reserved" ? slot.stage === run.dispatchProjection?.stage ? t("실행 대기열") : t("후속 단계 대기") : slot.state === "active" ? t("실행 중") : slot.state === "released" ? t("요청 해제됨") : t("요청 상태 미확인")} </span>) : t("저장된 코어 요청 상태를 확인하지 못했습니다.")}</span>}
              {!compact && <span className="core-focus">{screen === "input" ? core.focus : t("공개된 의견 없음")}</span>}
              <span className="core-open">{onOpenCore ? compact ? t("콘솔에서 보기") : screen === "input" ? t("관점 열기") : t("상세 보기") : t("상세 정보 미제공")}{onOpenCore && " ↗"}</span>
            </button>
          </div>
        );
      })}
      <span id="topology-summary" className="sr-only">{t("위쪽 BALTHASAR 2, 왼쪽 아래 CASPER 3, 오른쪽 아래 MELCHIOR 1의 고정된 삼각 배치입니다. 현재 단계:")}{stage.korean}.{sealed || run?.ballotState === "sealed" ? t(" 세 표의 방향은 봉인되어 있습니다.") : publicResult ? t(" 검증된 공개 표결을 표시합니다.") : ""}</span>
    </div>
  );
}
