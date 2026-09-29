import { useState, type ReactNode } from "react";
import type { ConsoleRunSummary, ConsoleSnapshot, ContextSelectionSummary, RunDossierView, RunUpdate } from "./lib/desktop-api";

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
  { id: "provider", title: "연결 프로필", english: "PROVIDER PROFILE", family: "work" },
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

export type ScreenId = (typeof screenCatalog)[number]["id"];
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
export type AcpProfileStoreState = "loading" | "ready" | "unavailable" | "error";
export type AcpProfileBlockReason = "adapter_unsupported" | "home_override_unsupported" | "home_mismatch" | "attestation_missing" | "other";

export type AcpProfileAdmission =
  | { state: "not_checked" }
  | { state: "needs_auth"; profileId: string; adapterId: string; rootBinding: "verified"; checkedAt: string }
  | { state: "blocked"; reason: AcpProfileBlockReason; checkedAt?: string }
  | { state: "admitted"; profileId: string; adapterId: string; rootBinding: "verified"; checkedAt: string; modelId?: string };

export type AcpProfile = {
  id: string;
  displayName: string;
  accountAlias: string;
  adapterId: string;
  revision: number;
  authenticationMethod: "local_subscription" | "byok_api";
  admission: AcpProfileAdmission;
};

export type AcpProfileDraft = {
  profileId: string | null;
  expectedRevision: number | null;
  displayName: string;
  adapterId: string;
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
export type RunProgressSummary = Pick<RunUpdate, "runId" | "stage" | "coreId" | "state">;
export type CancelRequestState = { runId: string; state: "sending" | "sent" | "error"; message?: string } | null;

export type ScreenProps = {
  screen: ScreenId;
  question: string;
  motion: MotionSetting;
  sound: boolean;
  theme: "command" | "clear";
  snapshot: ConsoleSnapshot;
  homeData: HomeData;
  selectedRunId: string | null;
  selectedRunDossier: RunDossierView | null;
  selectedRunDossierState: "idle" | "loading" | "ready" | "error";
  currentRunId: string | null;
  runProgress: RunProgressSummary | null;
  runDossier: RunDossierView | null;
  runDossierState: "idle" | "loading" | "ready" | "error";
  runStartState: "idle" | "starting" | "error";
  runStartError: string;
  disclosureConfirmed: boolean;
  cancelRequest: CancelRequestState;
  contextSelection: ContextSelectionSummary | null;
  acpAdaptersState: AcpAdapterState;
  acpAdapters: AcpAdapterSummary[];
  acpProfilesState: AcpProfileStoreState;
  acpProfiles: AcpProfile[];
  selectedAcpProfileId: string | null;
  selectedProviderProfile: AcpProfile | null;
  authenticatingProfileId: string | null;
  validatingProfileId: string | null;
  acpProfileDraft: AcpProfileDraft | null;
  acpProfileWriteState: AcpProfileWriteState;
  rolePresetsState: RolePresetStoreState;
  rolePresets: RolePreset[];
  selectedRolePresetId: string | null;
  selectedRolePreset: RolePreset | null;
  roleDraft: RolePresetDraft | null;
  roleWriteState: RolePresetWriteState;
  companionWindow: boolean;
  onNavigate: (screen: ScreenId) => void;
  onOpenCore: (core: string) => void;
  onOpenRecentRun: (runId: string) => void;
  onBeginAcpProfileCreate: () => void;
  onBeginAcpProfileEdit: (profileId: string) => void;
  onAcpProfileDraftChange: (draft: AcpProfileDraft | null) => void;
  onSaveAcpProfile: (draft: AcpProfileDraft) => void;
  onSelectAcpProfile: (profileId: string) => void;
  onValidateAcpProfile: (profileId: string) => void;
  onAuthenticateAcpProfile: (profileId: string) => void;
  onSelectRolePreset: (presetId: string) => void;
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

function Button({ children, onClick, tone = "secondary", disabled = false, type = "button", title }: {
  children: ReactNode;
  onClick?: () => void;
  tone?: "primary" | "secondary" | "danger";
  disabled?: boolean;
  type?: "button" | "submit";
  title?: string;
}) {
  return <button className={`button button-${tone}`} type={type} onClick={onClick} disabled={disabled} title={title}>{children}</button>;
}

function Panel({ title, kicker, children, className = "" }: { title: string; kicker?: string; children: ReactNode; className?: string }) {
  return (
    <section className={`panel ${className}`}>
      {kicker && <p className="panel-kicker">{kicker}</p>}
      <h3>{title}</h3>
      <div className="panel-content">{children}</div>
    </section>
  );
}

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
    onSaveAcpProfile, onSelectAcpProfile, onValidateAcpProfile,
    onSelectRolePreset, onBeginRolePresetEdit, onCloneRolePreset, onEditRoleDraft, onSaveRolePreset,
    onOpenConsole, onOpenSettings, onCloseCompanion, onRequestExit,
  } = props;
  if (screen === "home") return <HomePage data={homeData} snapshot={snapshot} onNavigate={onNavigate} onOpenCore={onOpenCore} onOpenRecentRun={props.onOpenRecentRun} />;
  const page = screenCatalog.find((item) => item.id === screen)!;
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
      {screen === "evidence" && <EvidenceStage run={run} dossier={dossier} onOpenCore={onOpenCore} onNavigate={onNavigate} />}
      {["paused", "interrupted", "cancelling", "cancelled", "failed", "save-error"].includes(screen) && <RecoveryPage screen={screen} run={run} dossier={dossier} runId={runId} cancelRequest={props.cancelRequest} onCancelRun={props.onCancelRun} onRefreshRunStatus={props.onRefreshRunStatus} runDossierState={props.runDossierState} onNavigate={onNavigate} />}
      {screen === "connections" && <ConnectionsPage snapshot={snapshot} onNavigate={onNavigate} />}
      {screen === "provider" && <ProviderPage adaptersState={acpAdaptersState} adapters={acpAdapters} profilesState={acpProfilesState} profiles={acpProfiles} selectedProfileId={selectedAcpProfileId} draft={acpProfileDraft} writeState={acpProfileWriteState} authenticatingProfileId={props.authenticatingProfileId} validatingProfileId={props.validatingProfileId} onBeginCreate={onBeginAcpProfileCreate} onBeginEdit={onBeginAcpProfileEdit} onDraftChange={onAcpProfileDraftChange} onSave={onSaveAcpProfile} onSelect={onSelectAcpProfile} onValidate={onValidateAcpProfile} onAuthenticate={props.onAuthenticateAcpProfile} />}
      {screen === "intake" && <IntakePage selection={contextSelection} onNavigate={onNavigate} onSelectContextFiles={onSelectContextFiles} onSelectContextDirectory={props.onSelectContextDirectory} />}
      {screen === "confirmation" && <ConfirmationPage question={question} run={run} selection={contextSelection} snapshot={snapshot} provider={props.selectedProviderProfile} rolePreset={props.selectedRolePreset} adapters={acpAdapters} profilesState={acpProfilesState} rolePresetsState={props.rolePresetsState} disclosureConfirmed={props.disclosureConfirmed} runStartState={props.runStartState} runStartError={props.runStartError} currentRunId={runId} dossier={dossier} dossierState={props.runDossierState} onDisclosureConfirmedChange={props.onDisclosureConfirmedChange} onCancelRun={props.onCancelRun} cancelRequest={props.cancelRequest} onNavigate={onNavigate} onStart={onStartRealRun} />}
      {screen === "roles" && <RolesPage presetsState={rolePresetsState} presets={rolePresets} selectedPresetId={selectedRolePresetId} draft={roleDraft} writeState={roleWriteState} onSelect={onSelectRolePreset} onBeginEdit={onBeginRolePresetEdit} onClone={onCloneRolePreset} onDraftChange={onEditRoleDraft} onSave={onSaveRolePreset} />}
      {screen === "settings" && <SettingsPage motion={motion} sound={sound} theme={theme} fontScale={fontScale} onMotionChange={onMotionChange} onSoundChange={onSoundChange} onThemeChange={onThemeChange} onFontScaleChange={onFontScaleChange} onNavigate={onNavigate} onNotice={onNotice} />}
      {screen === "history" && <HistoryPage data={homeData} selectedRunId={selectedRunId} selectedDossier={props.selectedRunDossier?.runId === selectedRunId ? props.selectedRunDossier : null} dossierState={props.selectedRunDossierState} onNavigate={onNavigate} onSelectRun={props.onOpenRecentRun} onRefreshRunStatus={props.onRefreshRunStatus} />}
      {screen === "replay" && <ReplayPage onNavigate={onNavigate} />}
      {screen === "share" && <SharePage />}
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
  const connection = snapshot.connection === "ready" ? "연결 확인됨" : snapshot.connection === "blocked" ? "연결 차단됨" : "연결 미확인";
  const storage = snapshot.storage === "ready" ? "로컬 저장소 준비됨" : snapshot.storage === "error" ? "로컬 저장 오류" : "저장 상태 미확인";
  const recentRuns = data.recentRuns.filter((item) => item.id !== run?.id);

  return (
    <section className="home-landing" aria-labelledby="screen-title">
      <header className="home-heading">
        <div>
          <p className="page-kicker"><span>MAGI COMMAND</span><span className="kicker-divider" aria-hidden="true">/</span><span>00 · HOME</span></p>
          <h2 id="screen-title">사령 콘솔</h2>
          <p className="home-intro">안건을 열고, 세 관점의 심의와 실제 기록을 관리합니다.</p>
        </div>
        <div className="home-system-status" aria-label="현재 시스템 상태">
          <span className={`status-chip ${snapshot.connection === "ready" ? "chip-support" : "chip-unknown"}`}>연결 · {connection}</span>
          <span className={`status-chip ${snapshot.storage === "ready" ? "chip-support" : snapshot.storage === "error" ? "chip-oppose" : "chip-unknown"}`}>저장 · {storage}</span>
        </div>
      </header>

      <div className="home-grid">
        <section className="home-launch panel" aria-labelledby="home-launch-title">
          <div className="home-launch-heading">
            <p className="panel-kicker">THREE CORES / ONE AGENDA</p>
            <h3 id="home-launch-title">{runInProgress ? "진행 중인 심의" : run ? "마지막 확인 심의" : "새 심의를 준비하십시오"}</h3>
            <p>{run ? run.question : "질문을 입력하고 자료·연결·역할을 확인한 뒤 심의를 시작합니다."}</p>
            {run && <span className="home-run-state">{runStageLabel(run.stage)} · {runStatusLabel(run.status)}</span>}
          </div>
          <div className="home-topology"><CoreTopology screen={screenForRun(run)} run={run} onOpenCore={onOpenCore} compact /></div>
          <div className="home-launch-actions">
            <Button tone="primary" onClick={() => run ? onOpenRecentRun(run.id) : onNavigate("input")}>{runInProgress ? "진행 심의 기록 열기" : run ? "최근 심의 기록 열기" : "새 심의 시작"}<span aria-hidden="true">↗</span></Button>
            <Button onClick={() => onNavigate("provider")}>ACP 연결 프로필</Button>
            <Button onClick={() => onNavigate("roles")}>세 관점 설정</Button>
          </div>
          <p className="home-run-caveat">실제 실행은 연결·권한·자료 전송 확인과 저장소 상태가 준비되어야 시작됩니다.</p>
        </section>

        <section className="home-records panel" aria-labelledby="home-records-title">
          <div className="home-section-heading">
            <div><p className="panel-kicker">LOCAL RECORDS</p><h3 id="home-records-title">최근 기록</h3></div>
            <Button onClick={() => onNavigate("history")}>기록 전체 보기</Button>
          </div>
          {data.recordsState === "loading" && <EmptyState title="기록 확인 중" text="로컬 저장소에서 실제 심의 기록을 읽고 있습니다." />}
          {data.recordsState === "unavailable" && <EmptyState title="기록을 사용할 수 없습니다" text="기록 목록 서비스가 제공되지 않아 비어 있는 기록으로 간주하지 않습니다." />}
          {data.recordsState === "error" && <EmptyState title="기록을 읽지 못했습니다" text="저장된 기록을 조회하지 못했습니다. 다시 확인하거나 저장소 상태를 확인하십시오." />}
          {data.recordsState === "ready" && recentRuns.length === 0 && <EmptyState title={run ? "현재 심의 외에 추가 기록이 없습니다" : "저장된 심의 기록이 없습니다"} text={run ? "현재 또는 마지막으로 확인한 심의는 왼쪽에 표시합니다." : "새 심의를 시작하면 확인된 실행 기록이 이곳에 나타납니다."} />}
          {data.recordsState === "ready" && recentRuns.length > 0 && <ul className="home-record-list">{recentRuns.map((item) => <li key={item.id}>
            <button type="button" className="home-record-row" onClick={() => onOpenRecentRun(item.id)}>
              <span className="home-record-copy"><strong>{item.question || "안건 없음"}</strong><small>{formatRecordDate(item.createdAt)}</small></span>
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
  return labels[status] ?? `상태 · ${status}`;
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
  return labels[stage] ?? "단계 미확인";
}

function outcomeLabel(outcome: RunDossierView["outcome"]): string {
  const labels: Record<Exclude<RunDossierView["outcome"], null>, string> = {
    unanimous: "만장일치",
    majority: "다수 결의",
    rejected: "기각",
    unresolved: "미해결",
  };
  return outcome ? labels[outcome] : "결과 미기록";
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
    <Panel title="실행 계기" kicker="LIVE RUN STATE">
      {dossierState === "loading" && <p className="field-help" role="status">실제 저장 상태를 확인하고 있습니다.</p>}
      {dossierState === "error" && <p className="unavailable-reason" role="alert">저장된 실행 상태를 읽지 못했습니다. 완료·실패·취소를 추정하지 않습니다.</p>}
      {stage && <p className="proposal-meta"><span>현재 단계</span><strong>{runStageLabel(stage)} · {stage}</strong></p>}
      {current && <p role="status" aria-live="polite">{terminal ? `저장된 결과 상태 · ${runStatusLabel(currentDossier?.status ?? "unknown")}` : eventLabel}</p>}
      {currentDossier && <p className="field-help">저장된 Run 상태 · {runStatusLabel(currentDossier.status)}{terminal ? " · 최종 상태 확인됨" : " · 실행 중"}</p>}
      {!current && !currentDossier && dossierState === "ready" && <p className="field-help">이 실행에 대한 진행 이벤트가 아직 도착하지 않았습니다.</p>}
      {!terminal && <p className="field-help">코어별 단계 완료는 전체 심의 완료를 뜻하지 않습니다. 종료 여부는 저장된 dossier 상태로 확인합니다.</p>}
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
    <div className="inline-warning" aria-label="심의 취소">
      <strong>진행 중인 심의</strong>
      {request?.state === "sending" && <p role="status">취소 요청을 보내고 있습니다.</p>}
      {request?.state === "sent" && <p role="status">취소 요청을 보냈습니다. 저장 상태가 취소로 바뀔 때까지 실행 상태를 확인합니다.</p>}
      {request?.state === "error" && <p role="alert">{request.message ?? "취소 요청을 전달하지 못했습니다."}</p>}
      {!confirming && (!request || request.state === "error") && <Button tone="danger" onClick={() => setConfirming(true)}>심의 취소</Button>}
      {confirming && (!request || request.state === "error") && <>
        <p>현재 Run의 남은 코어 작업을 취소하도록 요청합니다. 취소 완료는 저장 상태로 확인합니다.</p>
        <div className="page-actions"><Button onClick={() => setConfirming(false)}>취소 안 함</Button><Button tone="danger" onClick={() => { setConfirming(false); onCancel(runId); }}>취소 요청 보내기</Button></div>
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
        <h2 id="screen-title">{page.title}</h2>
      </div>
      <div className="heading-state" aria-label="실제 실행 정보 미확인">
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
  const connection = snapshot.connection === "ready" ? "연결 확인됨" : snapshot.connection === "blocked" ? "연결 차단됨" : "연결 미확인";
  return (
    <section className="stage-section" aria-label="세 코어 심의 무대">
      <div className="stage-meta stage-meta-left">
        <span className="instrument-rule" />
        <strong>MAGI CORE</strong>
        <span>STANDBY / 안건 대기</span>
        <p>질문·자료·역할을 확인한 뒤 심의를 시작합니다.</p>
      </div>
      <CoreTopology screen="input" run={run} onOpenCore={onOpenCore} />
      <div className="stage-meta stage-meta-right">
        <span className="instrument-rule" />
        <strong>{snapshot.connection === "ready" ? "CONNECTION CHECKED" : "CONNECTION UNVERIFIED"}</strong>
        <span>{connection}</span>
        <p>연결·권한·입력 검증이 완료되기 전에는 실행하지 않습니다.</p>
      </div>
      <div className="stage-rail">
        <span className="rail-index">01</span>
        <div><strong>질문에서 결의까지</strong><p>세 관점이 같은 안건을 검토하도록 질문을 작성하십시오.</p></div>
        <div className="stage-actions">
          <Button onClick={() => onNavigate("connections")}>연결 설정</Button>
          <Button onClick={() => onNavigate("roles")}>세 관점 설정</Button>
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
        <span>{run ? runStatusLabel(run.status) : "실행 상태 확인 중"}</span>
        <p>단계와 종료 여부는 실제 실행 이벤트와 저장 상태로 확인합니다.</p>
      </div>
      <CoreTopology screen={screen} run={run} onOpenCore={onOpenCore} />
      <div className="stage-meta stage-meta-right">
        <span className="instrument-rule" /><strong>{stage.english}</strong>
        <span>{stage.japanese} · {stage.korean}</span>
        <p>{isReview ? "교차 검토는 관점 간 공개된 쟁점만 표시합니다." : "독립 검토 의견은 표결 공개 전 서로 분리되어야 합니다."}</p>
      </div>
      <div className="stage-rail">
        <span className="rail-index">{isReview ? "03" : "02"}</span>
        <div><strong>{isReview ? "서로의 주장 검토" : "같은 안건 · 독립된 관점"}</strong><p>실제 실행은 검증된 입력과 연결 상태 확인 뒤에만 표시됩니다.</p></div>
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
        <div><strong>표결 대상 제안</strong><p>{dossier?.proposal ? "저장된 제안 내용을 표시합니다." : "저장된 제안 내용이 아직 없습니다."}</p></div>
        <div className="stage-actions">
          <Button onClick={() => onNavigate("evidence")}>근거 검토</Button>
          {runId && <InlineCancelControl runId={runId} active={runIsActive} cancelRequest={cancelRequest} onCancel={onCancelRun} />}
        </div>
      </div>
      <Panel title="결의문 전문" kicker="PROPOSAL CONTENT">
        {dossier?.proposal ? <ProposalContent proposal={dossier.proposal} /> : <EmptyState title="제안 미제공" text="저장된 실행 기록에 제안 본문이 생기면 이곳에 표시합니다." />}
        <div className="proposal-meta"><span>안건</span><strong>{dossier?.question ?? run?.question ?? "안건 미확인"}</strong></div>
        {run && <div className="proposal-meta"><span>자료 연결</span><strong>{run.sourceCount}개</strong></div>}
      </Panel>
    </section>
  );
}

function SealedStage({ run, dossier, onOpenCore, runId, runIsActive, cancelRequest, onCancelRun }: { run?: ConsoleRunSummary; dossier: RunDossierView | null; onOpenCore: (core: string) => void; runId: string | null; runIsActive: boolean; cancelRequest: CancelRequestState; onCancelRun: ScreenProps["onCancelRun"] }) {
  return (
    <section className="stage-section">
      <div className="sealed-warning" role="note"><strong>표 방향은 봉인되어 있습니다</strong><span>세 표가 검증되기 전까지 색·문구·접근성 이름에 찬반 방향이 나타나지 않습니다.</span></div>
      <CoreTopology screen="sealed" run={run} onOpenCore={onOpenCore} />
      <Panel title="동일한 표결 대상" kicker="PROPOSAL SNAPSHOT">
        {dossier?.proposal ? <ProposalContent proposal={dossier.proposal} /> : <p className="proposal-copy">표결 대상 문안이 저장된 상태에 없습니다.</p>}
        <p className="sealed-status">{dossier ? `표 ${dossier.votes.length}개 접수 · 방향 봉인` : run?.ballotState === "sealed" ? `표 ${run.votes?.length ?? "확인된 수 없음"}개 제출 · 방향 봉인` : "표결 상태 미확인"}</p>
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
        <div className="verdict-label"><span>결의</span><small>VERDICT</small></div>
        <div className="verdict-main"><span className="verdict-english">{resultReady && dossier.outcome ? dossier.outcome.toUpperCase() : "UNVERIFIED"}</span><strong>{summary}</strong></div>
        <div className="vote-counts" aria-label={votes ? `찬성 ${voteCounts?.support ?? 0}, 반대 ${voteCounts?.oppose ?? 0}, 기권 ${voteCounts?.abstain ?? 0}` : "표결 정보 미확인"}>
          {votes ? <><span className="count-support">찬성 {voteCounts?.support ?? 0}</span><span className="count-oppose">반대 {voteCounts?.oppose ?? 0}</span><span>기권 {voteCounts?.abstain ?? 0}</span></> : <span>표결 기록 미확인</span>}
        </div>
      </div>
      <div className="verdict-details">
        <Panel title="승인된 제안" kicker="PROPOSAL">
          {resultReady && dossier.proposal ? <ProposalContent proposal={dossier.proposal} /> : <EmptyState title="최종 결의문 미확인" text="저장된 최종 dossier의 완료 상태가 확인되면 제안을 표시합니다." />}
        </Panel>
        <Panel title="소수 의견" kicker="DISSENT · ALWAYS VISIBLE" className="dissent-panel">
          {resultReady ? <DissentContent dossier={dossier} /> : <EmptyState title="최종 상태 미확인" text="코어 단계 완료만으로 전체 심의를 종료하지 않습니다. 저장된 dossier를 기다립니다." />}
          {dossier?.error && <p className="unavailable-reason" role="alert">{dossier.error.code} · {dossier.error.message}</p>}
        </Panel>
      </div>
      <div className="page-actions">
        <Button onClick={() => onNavigate("evidence")}>근거와 이견 열기 <span aria-hidden="true">↗</span></Button>
        <Button onClick={() => onNavigate("history")}>기록 열기</Button>
      </div>
    </section>
  );
}

function EvidenceStage({ run, dossier, onOpenCore, onNavigate }: { run?: ConsoleRunSummary; dossier: RunDossierView | null; onOpenCore: (core: string) => void; onNavigate: ScreenProps["onNavigate"] }) {
  const resultReady = dossier?.status === "completed";
  const votes = resultReady ? dossier.votes : undefined;
  const voteCounts = votes ? countVotes(votes) : undefined;
  return (
    <section className="evidence-layout">
      <div className="evidence-topology"><CoreTopology screen="evidence" run={run} onOpenCore={onOpenCore} compact /></div>
      <div className="evidence-summary">
        <p className="panel-kicker">{resultReady ? "DOSSIER VERIFIED" : "DOSSIER STATUS UNKNOWN"}</p>
        <h3>{resultReady ? dossier.outcome ?? "결론 미도출" : "결의 기록 미확인"}</h3>
        <p>{votes ? `찬성 ${voteCounts?.support ?? 0} · 반대 ${voteCounts?.oppose ?? 0} · 기권 ${voteCounts?.abstain ?? 0}` : "최종 저장 상태 확인 전에는 표결 방향을 표시하지 않습니다."}</p>
      </div>
      <div className="evidence-sections">
        <Panel title="제안" kicker="01 / PROPOSAL">{resultReady && dossier.proposal ? <ProposalContent proposal={dossier.proposal} /> : <EmptyState title="제안 미제공" text="실제 dossier에 저장된 제안만 표시합니다." />}</Panel>
        <Panel title="표결" kicker="02 / BALLOTS">{votes ? <VoteDetails votes={votes} /> : <p>최종 완료 상태가 저장된 dossier로 확인되기 전까지 표의 방향을 공개하지 않습니다.</p>}</Panel>
        <Panel title="공통 근거와 의견 차이" kicker="03 / EVIDENCE & DISSENT">
          {resultReady ? <DissentContent dossier={dossier} /> : <EmptyState title="최종 상태 미확인" text="전체 심의가 완료됐다고 확인되기 전에는 표결과 이견을 확정하지 않습니다." />}
        </Panel>
        <Panel title="자료 범위와 신선도" kicker="04 / SOURCES"><p>{run ? `실행 기록의 연결 자료 ${run.sourceCount}개` : "이 화면에서 자료 수를 확인하지 못했습니다."}</p><p>파일별 인용 위치와 freshness는 dossier 응답에 포함되지 않습니다.</p><Button onClick={() => onNavigate("intake")}>자료 접수 열기</Button></Panel>
      </div>
      <div className="page-actions"><Button onClick={() => onNavigate("history")}>기록으로</Button><Button tone="primary" onClick={() => onNavigate("share")}>공유 미리보기 <span aria-hidden="true">↗</span></Button></div>
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
  return choice === "support" ? "찬성" : choice === "oppose" ? "반대" : "기권";
}

function VoteDetails({ votes }: { votes: RunDossierView["votes"] }) {
  return votes.length > 0 ? <ul className="contract-list">{votes.map((vote) => <li key={vote.coreId}><strong>{coreDisplayName(vote.coreId)} · {choiceLabel(vote.choice)}</strong><p>{vote.rationale}</p></li>)}</ul> : <EmptyState title="저장된 표결 없음" text="최종 dossier에 공개 표결이 없습니다." />;
}

function ProposalContent({ proposal }: { proposal: NonNullable<RunDossierView["proposal"]> }) {
  return <>
    <p className="proposal-copy">{proposal.body}</p>
    {proposal.conditions.length > 0 && <><h4>성립 조건</h4><ul className="contract-list">{proposal.conditions.map((item, index) => <li key={`${index}-${item}`}>{item}</li>)}</ul></>}
    {proposal.alternatives.length > 0 && <><h4>대안</h4><ul className="contract-list">{proposal.alternatives.map((item, index) => <li key={`${index}-${item}`}>{item}</li>)}</ul></>}
  </>;
}

function DissentContent({ dossier }: { dossier: RunDossierView | null }) {
  const objections = dossier?.proposal?.openObjections ?? [];
  const votes = dossier?.votes ?? [];
  if (objections.length === 0 && votes.length === 0) return <EmptyState title="기록된 표결 근거·미해결 이의 없음" text="완료된 dossier의 공개 기록에 이 항목이 없습니다." />;
  return <>
    {votes.length > 0 && <><h4>코어별 표결 근거</h4><VoteDetails votes={votes} /></>}
    {objections.length > 0 && <><h4>미해결 이의</h4><ul className="contract-list">{objections.map((objection) => <li key={objection.claimId}><strong>{objection.claimId}</strong><p>{objection.rationale}</p>{objection.requiredInformation.length > 0 && <><span>필요 정보</span><ul>{objection.requiredInformation.map((item, index) => <li key={`${index}-${item}`}>{item}</li>)}</ul></>}</li>)}</ul></>}
  </>;
}

function PageHeadingAndLayout({ english, title, intro, children, aside }: { english: string; title: string; intro: string; children: ReactNode; aside?: ReactNode }) {
  return (
    <section className="work-layout">
      <div className="work-main">{children}</div>
      <aside className="work-aside">
        <div className="aside-instrument"><span className="aside-badge" aria-hidden="true">MAGI</span><span className="aside-index">CORE SYSTEM / {String(screenCatalog.findIndex((item) => item.english === english) + 1).padStart(2, "0")}</span></div>
        <p className="aside-kicker">{english}</p><strong>{title}</strong><p>{intro}</p>
        {aside ?? <div className="aside-core-list" aria-label="고정된 세 코어">
          <span><b>01</b> MELCHIOR·1 <small>근거와 실현 가능성</small></span>
          <span><b>02</b> BALTHASAR·2 <small>지속성과 돌봄</small></span>
          <span><b>03</b> CASPER·3 <small>주체성과 대안</small></span>
        </div>}
        <p className="aside-footnote">표시된 의견·결정 상태는 확인된 실행 기록만 반영합니다.</p>
      </aside>
    </section>
  );
}

function ConnectionsPage({ snapshot, onNavigate }: { snapshot: ConsoleSnapshot; onNavigate: ScreenProps["onNavigate"] }) {
  const connectionState = snapshot.connection === "ready" ? "확인됨" : snapshot.connection === "blocked" ? "차단됨" : "미확인";
  return (
    <PageHeadingAndLayout english="CONNECTION LEDGER" title="모델 연결" intro="실제 자격·기능·사용량은 연결 검증 결과로만 표시합니다.">
      <Panel title="연결 프로필" kicker="USER-OWNED PROVIDER">
        <ConnectionRow name="사용자 로컬 연결" detail="프로필·모델 목록은 이 상태 요약에서 제공되지 않습니다." state={connectionState} />
      </Panel>
      <Panel title="세 코어 할당" kicker="ROLE ASSIGNMENT">
        <CoreAssignment name="MELCHIOR·1" role="근거와 실현 가능성" state="할당 미확인" />
        <CoreAssignment name="BALTHASAR·2" role="지속성과 돌봄" state="할당 미확인" />
        <CoreAssignment name="CASPER·3" role="주체성과 대안" state="할당 미확인" />
      </Panel>
      <div className="page-actions"><Button onClick={() => onNavigate("roles")}>역할 확인</Button><Button disabled title="연결 상태 재조회 기능이 연결되지 않았습니다">연결 상태 확인</Button><Button tone="primary" onClick={() => onNavigate("provider")}>연결 프로필 추가 <span aria-hidden="true">↗</span></Button><p className="unavailable-reason">연결 상태는 마지막으로 확인된 스냅샷만 표시합니다. 현재 상태 재조회 기능이 연결되지 않았습니다.</p></div>
    </PageHeadingAndLayout>
  );
}

function ConnectionRow({ name, detail, state }: { name: string; detail: string; state: string }) {
  return <div className="ledger-row"><span className="connection-glyph" aria-hidden="true">◇</span><div><strong>{name}</strong><small>{detail}</small></div><span className="status-chip chip-unknown">{state}</span></div>;
}

function CoreAssignment({ name, role, state }: { name: string; role: string; state: string }) {
  return <div className="assignment-row"><strong>{name}</strong><span>{role}</span><span className="assignment-state">{state}</span></div>;
}

function ProviderPage({ adaptersState, adapters, profilesState, profiles, selectedProfileId, draft, writeState, authenticatingProfileId, validatingProfileId, onBeginCreate, onBeginEdit, onDraftChange, onSave, onSelect, onValidate, onAuthenticate }: {
  adaptersState: AcpAdapterState;
  adapters: AcpAdapterSummary[];
  profilesState: AcpProfileStoreState;
  profiles: AcpProfile[];
  selectedProfileId: string | null;
  draft: AcpProfileDraft | null;
  writeState: AcpProfileWriteState;
  authenticatingProfileId: string | null;
  validatingProfileId: string | null;
  onBeginCreate: () => void;
  onBeginEdit: (profileId: string) => void;
  onDraftChange: (draft: AcpProfileDraft | null) => void;
  onSave: (draft: AcpProfileDraft) => void;
  onSelect: (profileId: string) => void;
  onValidate: (profileId: string) => void;
  onAuthenticate: (profileId: string) => void;
}) {
  const selectedProfile = profiles.find((profile) => profile.id === selectedProfileId);
  const selectedAdapter = selectedProfile && adapters.find((adapter) => adapter.id === selectedProfile.adapterId);
  const canRunSelectedProfile = Boolean(
    selectedProfile
      && adaptersState === "ready"
      && selectedAdapter?.state === "supported"
      && isProfileAdmissionValid(selectedProfile),
  );
  const draftAdapter = draft ? adapters.find((adapter) => adapter.id === draft.adapterId) : undefined;
  const canSaveDraft = Boolean(draft && draft.displayName.trim() && draftAdapter?.state === "supported" && adaptersState === "ready" && profilesState === "ready" && writeState !== "saving" && writeState !== "conflict");

  return (
    <PageHeadingAndLayout english="PROVIDER PROFILE" title="ACP 연결 프로필" intro="각 프로필은 앱이 관리하는 전용 홈에 바인딩됩니다. 경로는 화면에 노출하지 않습니다.">
      <Panel title="ACP 연결 프로필" kicker="LOCAL ACP ADMISSION">
        {profilesState === "loading" && <EmptyState title="연결 프로필 확인 중" text="저장된 ACP 프로필을 읽고 있습니다." />}
        {profilesState === "unavailable" && <EmptyState title="프로필 저장소를 사용할 수 없습니다" text="이 앱에서 프로필 저장 기능이 제공되지 않아 프로필을 만들거나 검증할 수 없습니다." />}
        {profilesState === "error" && <EmptyState title="프로필을 읽지 못했습니다" text="저장된 프로필을 확인하지 못했습니다. 저장소 오류 상태를 확인하십시오." />}
        {profilesState === "ready" && profiles.length === 0 && <EmptyState title="저장된 ACP 프로필이 없습니다" text="프로필을 만들면 앱이 별도 연결 홈을 준비합니다. 기존 전역 프로필이나 HOME 설정은 가져오지 않습니다." />}
        {profilesState === "ready" && profiles.length > 0 && <ul className="acp-profile-list">{profiles.map((profile) => {
          const adapter = adapters.find((item) => item.id === profile.adapterId);
          const verified = isProfileAdmissionValid(profile);
          const stateLabel = profileAdmissionLabel(profile, verified);
          return <li className={`acp-profile-item ${profile.id === selectedProfileId ? "selected" : ""}`} key={profile.id}>
            <button type="button" className="acp-profile-select" aria-pressed={profile.id === selectedProfileId} onClick={() => onSelect(profile.id)}>
              <span className="acp-profile-mark" aria-hidden="true">{verified ? "✓" : "◇"}</span>
              <span className="acp-profile-copy"><strong>{profile.displayName}</strong><small>{adapter?.displayName ?? profile.adapterId} · {profile.authenticationMethod === "local_subscription" ? "ChatGPT 구독 계정" : "BYOK API"}{profile.accountAlias ? ` · ${profile.accountAlias}` : ""} · revision {profile.revision}</small></span>
              <span className={`status-chip ${verified ? "chip-support" : "chip-unknown"}`}>{stateLabel}</span>
            </button>
            <div className="acp-profile-actions">
              <Button onClick={() => onBeginEdit(profile.id)} disabled={profilesState !== "ready" || writeState === "saving"}>편집</Button>
              {profile.authenticationMethod === "local_subscription" && <Button onClick={() => onAuthenticate(profile.id)} disabled={authenticatingProfileId === profile.id || validatingProfileId === profile.id || adaptersState !== "ready" || adapter?.state !== "supported" || writeState === "saving"}>{authenticatingProfileId === profile.id ? "공식 로그인 확인 중…" : "공식 ChatGPT 로그인"}</Button>}
              <Button onClick={() => onValidate(profile.id)} disabled={validatingProfileId === profile.id || authenticatingProfileId === profile.id || adaptersState !== "ready" || adapter?.state !== "supported" || writeState === "saving"}>{validatingProfileId === profile.id ? "검증 중…" : profile.admission.state === "admitted" ? "다시 검증" : "프로필 검증"}</Button>
            </div>
          </li>;
        })}</ul>}
        {profilesState === "ready" && <div className="acp-profile-add"><Button tone="primary" onClick={onBeginCreate} disabled={writeState === "saving" || adaptersState !== "ready" || !adapters.some((adapter) => adapter.state === "supported")}>새 ACP 프로필</Button></div>}
      </Panel>

      <Panel title="선택 프로필의 실행 자격" kicker="PROFILE-BOUND ATTESTATION">
        {!selectedProfile && <EmptyState title="프로필을 선택하십시오" text="연결 프로필을 선택하고 어댑터 검증을 마쳐야 실행 자격을 확인할 수 있습니다." />}
        {selectedProfile && <div className={`acp-admission ${canRunSelectedProfile ? "admission-ready" : "admission-blocked"}`}>
          <strong>{canRunSelectedProfile ? "프로필 실행 자격 확인됨" : `실행 차단 · ${profileAdmissionLabel(selectedProfile, false)}`}</strong>
          <p>{canRunSelectedProfile ? `${selectedProfile.displayName}의 프로필 ID·어댑터·전용 홈 attestation이 일치합니다.` : runBlockExplanation(selectedProfile, selectedAdapter, adaptersState)}</p>
          <small>현재 선택: {selectedProfile.displayName}{selectedAdapter ? ` · ${selectedAdapter.displayName}` : " · 어댑터 이름 미확인"}</small>
          {selectedProfile.accountAlias && <small>계정 별칭 · {selectedProfile.accountAlias}</small>}
          <small>인증 방식 · {selectedProfile.authenticationMethod === "local_subscription" ? "ChatGPT 구독 계정" : "BYOK API"}</small>
          {selectedProfile.admission.state === "admitted" && selectedProfile.admission.modelId && <small>검증된 모델 · {selectedProfile.admission.modelId}</small>}
          {selectedProfile.admission.state === "admitted" && <small>마지막 검증 · {formatRecordDate(selectedProfile.admission.checkedAt)}</small>}
          {selectedAdapter?.reason && <small className="acp-adapter-reason">어댑터 사유 · {adapterReasonLabel(selectedAdapter.reason)}</small>}
        </div>}
        {adaptersState === "loading" && <p className="field-help">프로필 설정이 가능한 ACP 어댑터를 확인하고 있습니다.</p>}
        {adaptersState === "unavailable" && <p className="unavailable-reason">ACP 어댑터 기능을 사용할 수 없어 인증·자료 전송·모델 요청을 시작할 수 없습니다.</p>}
        {adaptersState === "error" && <p className="unavailable-reason">ACP 어댑터 목록을 읽지 못했습니다. 실행 자격을 추정하지 않습니다.</p>}
        {adaptersState === "ready" && adapters.length === 0 && <EmptyState title="프로필 설정 어댑터 없음" text="사용 가능한 ACP 설정 어댑터가 없어 프로필을 만들 수 없습니다." />}
        {adaptersState === "ready" && adapters.length > 0 && <>
          <ul className="acp-adapter-list">{adapters.map((adapter) => <li key={adapter.id}>
            <span><strong>{adapter.displayName}</strong><small>{adapter.state === "supported" ? "프로필 설정 가능" : "설정 차단"}{adapter.reason ? ` · ${adapterReasonLabel(adapter.reason)}` : ""}</small></span>
            <span className={`status-chip ${adapter.state === "supported" ? "chip-unknown" : "chip-oppose"}`}>{adapter.state === "supported" ? "설정 가능" : "차단됨"}</span>
          </li>)}</ul>
          <p className="field-help">프로필 설정 가능 표시는 ACP 런타임 실행 가능을 뜻하지 않습니다. 인증·자료 전달·모델 요청은 선택 프로필·어댑터·전용 홈이 일치하는 runtime attestation이 있을 때만 허용됩니다.</p>
        </>}
      </Panel>

      {draft && <Panel title={draft.profileId ? "프로필 편집" : "새 프로필 등록"} kicker="PERSISTED ACP PROFILE" className="acp-profile-editor">
        <label className="field"><span className="field-label">프로필 별칭</span><input className="text-field" autoComplete="off" value={draft.displayName} disabled={writeState === "saving"} onChange={(event) => onDraftChange({ ...draft, displayName: event.target.value })} placeholder="예: 개인 구독" /></label>
        <label className="field"><span className="field-label">ACP 어댑터</span><select className="text-field" value={draft.adapterId} disabled={Boolean(draft.profileId) || adaptersState !== "ready" || writeState === "saving"} onChange={(event) => onDraftChange({ ...draft, adapterId: event.target.value })}>
          <option value="">어댑터 선택</option>
          {adapters.map((adapter) => <option key={adapter.id} value={adapter.id} disabled={adapter.state !== "supported"}>{adapter.displayName}{adapter.state === "blocked" ? ` · 설정 차단${adapter.reason ? `: ${adapterReasonLabel(adapter.reason)}` : ""}` : " · 프로필 설정 가능"}</option>)}
        </select></label>
        {adaptersState === "ready" && !adapters.some((adapter) => adapter.state === "supported") && <p className="unavailable-reason">프로필 설정이 가능한 어댑터가 없어 이 프로필을 저장할 수 없습니다.</p>}
        <p className="field-help">{draft.profileId ? "저장된 프로필의 어댑터와 전용 홈은 변경할 수 없습니다. 다른 어댑터를 사용하려면 새 프로필을 만드십시오." : "전용 홈은 앱이 프로필별로 관리합니다. 경로를 입력하거나 다른 계정의 홈으로 대체할 수 없습니다."}</p>
        {writeState === "conflict" && <div className="inline-warning" role="alert"><strong>프로필 revision 충돌</strong><p>현재 저장된 버전이 바뀌었습니다. 최신 프로필을 다시 열어 확인한 뒤 수정하십시오.</p>{draft.profileId && <Button onClick={() => onBeginEdit(draft.profileId!)}>최신 버전 다시 읽기</Button>}</div>}
        {writeState === "error" && <p className="unavailable-reason" role="alert">프로필을 저장하지 못했습니다. 입력 내용은 유지했습니다.</p>}
        {writeState === "saved" && <p className="field-help" role="status">프로필이 저장되었습니다. 변경된 연결은 다시 검증해야 사용할 수 있습니다.</p>}
        <div className="page-actions"><Button onClick={() => onDraftChange(null)} disabled={writeState === "saving"}>편집 닫기</Button><Button tone="primary" onClick={() => onSave(draft)} disabled={!canSaveDraft}>프로필 저장</Button></div>
        {writeState === "saving" && <p className="field-help" role="status">프로필을 저장하고 있습니다.</p>}
      </Panel>}

      {selectedProfile && !canRunSelectedProfile && <p className="unavailable-reason">현재 선택된 프로필은 실행할 수 없습니다. 프로필에 고정된 어댑터와 앱 전용 홈이 검증되기 전에는 인증·자료 전송·모델 요청을 시작하지 않습니다.</p>}
    </PageHeadingAndLayout>
  );
}

function isProfileAdmissionValid(profile: AcpProfile): boolean {
  return profile.admission.state === "admitted"
    && profile.admission.profileId === profile.id
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
    attestation_missing: "프로필 실행 attestation을 받지 못함",
  };
  return labels[reason] ?? reason.replaceAll("_", " ");
}

function profileAdmissionLabel(profile: AcpProfile, valid: boolean): string {
  if (valid) return "프로필 홈 attestation 확인";
  if (profile.admission.state === "not_checked") return "실행 attestation 미확인";
  if (profile.admission.state === "needs_auth") return "공식 로그인 필요";
  if (profile.admission.state === "admitted") return "검증 정보 불일치 · 실행 차단";
  const labels: Record<AcpProfileBlockReason, string> = {
    adapter_unsupported: "지원되지 않는 어댑터",
    home_override_unsupported: "전용 홈 격리 미지원",
    home_mismatch: "프로필 홈 불일치",
    attestation_missing: "검증 증명 미확인",
    other: "검증 실패",
  };
  return labels[profile.admission.reason];
}

function admissionExplanation(admission: AcpProfileAdmission): string {
  if (admission.state === "not_checked") return "런타임의 프로필별 검증 결과가 아직 없습니다. 검증을 실행하기 전까지 이 연결은 차단 상태입니다.";
  if (admission.state === "needs_auth") return "ACP 런타임이 공식 로그인을 요구했습니다. 공식 로그인 후 다시 검증해 ready 상태를 받아야 자료 전송과 모델 호출을 할 수 있습니다.";
  if (admission.state === "admitted") return "검증 응답의 프로필 또는 어댑터 식별이 선택한 저장 프로필과 일치하지 않습니다. 다시 검증하기 전까지 연결을 사용할 수 없습니다.";
  if (admission.reason === "home_override_unsupported") return "어댑터가 프로필별 전용 홈 격리를 지원하지 않습니다. 다른 프로필이나 전역 홈으로 전환하지 않습니다.";
  if (admission.reason === "home_mismatch") return "런타임이 보고한 홈 바인딩이 선택 프로필과 일치하지 않습니다. 인증과 모델 요청은 차단됩니다.";
  if (admission.reason === "adapter_unsupported") return "선택한 어댑터는 프로필 격리 실행에 지원되지 않습니다.";
  if (admission.reason === "attestation_missing") return "어댑터에서 유효한 프로필 검증 증명을 받지 못했습니다.";
  return "런타임 검증을 통과하지 못했습니다. 원인을 확인하기 전까지 실행할 수 없습니다.";
}

function IntakePage({ selection, onNavigate, onSelectContextFiles, onSelectContextDirectory }: { selection: ContextSelectionSummary | null; onNavigate: ScreenProps["onNavigate"]; onSelectContextFiles: () => void; onSelectContextDirectory: () => void }) {
  const issueNames: Record<string, string> = {
    user_excluded: "사용자가 제외함", hidden_path: "숨김 경로", credential_path: "인증 정보 경로", symbolic_link: "심볼릭 링크",
    special_file: "지원하지 않는 파일 종류", cross_volume: "허용 범위 밖의 볼륨", unsupported_format: "지원하지 않는 형식",
    invalid_utf8: "텍스트 인코딩을 읽을 수 없음", file_too_large: "파일 크기 제한 초과", manifest_limit: "자료 수 제한 초과",
    access_denied: "접근 권한 없음", changed_during_capture: "읽는 동안 파일이 변경됨", revoked_grant: "접근 권한 철회됨",
    invalid_range: "선택 범위가 잘못됨", read_failure: "파일 읽기 실패",
  };
  const formatBytes = (bytes: number) => bytes < 1024 ? `${bytes} B` : bytes < 1_048_576 ? `${(bytes / 1024).toFixed(1)} KB` : `${(bytes / 1_048_576).toFixed(1)} MB`;
  return (
    <PageHeadingAndLayout english="SOURCE INTAKE" title="자료 접수" intro="파일 접근 동의와 모델 전송 확인은 별도의 단계입니다.">
      <div className="intake-grid">
        <Panel title="선택한 자료" kicker="SOURCE MANIFEST">
          {selection ? <>
            <div className="manifest-summary"><span>선택 {selection.sources.length}개</span><span>접수 {selection.sources.filter((source) => source.status === "captured").length}개</span><span>변경 번호 {selection.revision}</span></div>
            <ul className="source-list">{selection.sources.map((source) => <li key={source.sourceId} className={`source-row source-${source.status}`}>
              <span className="source-row-mark" aria-hidden="true">{source.status === "captured" ? "✓" : source.status === "excluded" ? "−" : "!"}</span>
              <span className="source-row-copy"><strong>{source.displayName}</strong><small>{formatBytes(source.byteLength)} · {source.representation.replaceAll("_", " ")}</small>{source.issueCodes.length > 0 && <small>{source.issueCodes.map((code) => issueNames[code] ?? "상세 사유 미확인").join(" · ")}</small>}</span>
              <span className="status-chip">{source.status === "captured" ? "접수됨" : source.status === "excluded" ? "제외됨" : "실패"}</span>
            </li>)}</ul>
          </> : <EmptyState title="선택된 자료 없음" text="macOS 파일·폴더 선택기로 허용된 항목만 접수합니다. 경로와 원문은 화면에 표시하지 않습니다." />}
        </Panel>
        <Panel title="원문 / 전달 범위" kicker="CAPTURE & DELIVERY">{selection ? <div className="source-preview-state"><strong>파일 선택 요약 접수</strong><p>파일 경로와 원문 내용은 이 화면에 노출되지 않습니다. 접수된 텍스트·첨부의 실제 전달 범위는 입력 확인 단계에서 별도로 확인합니다.</p><small>Manifest digest · {selection.manifestDigest.slice(0, 12)}…</small></div> : <p>파일을 선택하면 접수 결과를 표시합니다. 선택은 모델 전송 동의가 아니며, 전달 범위는 다음 확인 화면에서 따로 확인합니다.</p>}</Panel>
      </div>
      <div className="page-actions"><Button onClick={onSelectContextFiles}>파일 선택</Button><Button onClick={onSelectContextDirectory}>폴더 선택</Button><Button tone="primary" onClick={() => onNavigate("confirmation")}>입력 확인 <span aria-hidden="true">↗</span></Button><p className="field-help">파일 또는 폴더를 고르면 허용된 원문만 캡처합니다. 선택만으로 외부 전송은 시작되지 않습니다.</p></div>
    </PageHeadingAndLayout>
  );
}

function ConfirmationPage({ question, run, selection, snapshot, provider, rolePreset, adapters, profilesState, rolePresetsState, disclosureConfirmed, runStartState, runStartError, currentRunId, dossier, dossierState, onDisclosureConfirmedChange, onCancelRun, cancelRequest, onNavigate, onStart }: {
  question: string;
  run?: ConsoleRunSummary;
  selection: ContextSelectionSummary | null;
  snapshot: ConsoleSnapshot;
  provider: AcpProfile | null;
  rolePreset: RolePreset | null;
  adapters: AcpAdapterSummary[];
  profilesState: AcpProfileStoreState;
  rolePresetsState: RolePresetStoreState;
  disclosureConfirmed: boolean;
  runStartState: ScreenProps["runStartState"];
  runStartError: string;
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
  const selectedAdapter = provider && adapters.find((adapter) => adapter.id === provider.adapterId);
  const providerReady = Boolean(provider && selectedAdapter?.state === "supported" && isProfileAdmissionValid(provider));
  const rolesReady = rolePresetsState === "ready" && Boolean(rolePreset && hasThreeRoleSlots(rolePreset.roles));
  const terminalDossier = Boolean(dossier && ["completed", "failed", "cancelled"].includes(dossier.status));
  const activeRun = Boolean(run && !["completed", "failed", "cancelled"].includes(run.status));
  const waitingForRunStatus = Boolean(currentRunId && (!terminalDossier || dossierState !== "ready"));
  const hasActiveRun = activeRun || waitingForRunStatus;
  const canConfirmTransfer = Boolean(question.trim() && provider && rolesReady && !hasActiveRun);
  const canStart = Boolean(canConfirmTransfer && providerReady && snapshot.storage === "ready" && disclosureConfirmed && runStartState !== "starting");
  const blockReason = hasActiveRun
    ? "현재 심의의 저장 상태를 확인한 뒤 새 심의를 시작할 수 있습니다."
    : profilesState !== "ready"
      ? "ACP 프로필 저장소 상태를 확인하지 못했습니다."
      : !provider
        ? "실행에 사용할 ACP 프로필을 선택하십시오."
        : provider.admission.state === "needs_auth"
          ? "선택 프로필에서 공식 로그인을 완료한 뒤 다시 검증하십시오."
          : !providerReady
            ? "선택 프로필의 실제 런타임 검증과 모델 상태 확인이 필요합니다."
            : !rolesReady
              ? "실행에 사용할 세 코어 역할 프리셋을 확인하십시오."
              : snapshot.storage !== "ready"
                ? "로컬 저장소 준비 상태가 확인되지 않아 실행을 차단했습니다."
                : !disclosureConfirmed
                  ? "전송 범위를 읽고 명시적으로 동의해야 시작할 수 있습니다."
                  : "입력·전송 범위가 확인되었습니다.";
  const sourceStatus = (status: string) => status === "captured" ? "캡처됨" : status === "excluded" ? "제외됨" : "실패";
  return (
    <PageHeadingAndLayout english="INPUT CONFIRMATION" title="입력·전송 확인" intro="이 화면에서 확인한 범위만 실행 요청에 포함됩니다.">
      <Panel title="질문" kicker="AGENDA"><p className="confirmation-question">{question || "질문이 입력되지 않았습니다."}</p><Button onClick={() => onNavigate("input")} disabled={hasActiveRun}>질문 편집</Button></Panel>
      <Panel title="캡처된 자료" kicker="CONTEXT MANIFEST">
        <p>{selection ? `접수 ${capturedCount}개 · 전체 항목 ${selection.sources.length}개 · revision ${selection.revision}` : "자료 0개 · 질문과 역할 지침만 전달 대상입니다."}</p>
        {selection && <ul className="source-list">{selection.sources.map((source) => <li className={`source-row source-${source.status}`} key={source.sourceId}><span className="source-row-copy"><strong>{source.displayName}</strong><small>{sourceStatus(source.status)} · {source.representation.replaceAll("_", " ")} · {source.byteLength} bytes</small></span><span className="status-chip">{sourceStatus(source.status)}</span></li>)}</ul>}
      </Panel>
      <Panel title="전송 목적지" kicker="PROVIDER PROFILE">
        {provider ? <div className="confirmation-destination"><strong>{provider.displayName}</strong><p>{selectedAdapter?.displayName ?? provider.adapterId} · {provider.authenticationMethod === "local_subscription" ? "ChatGPT 구독 계정" : "BYOK API"}{provider.accountAlias ? ` · ${provider.accountAlias}` : ""}</p><p>실행 자격 · {profileAdmissionLabel(provider, providerReady)}{provider.admission.state === "admitted" && provider.admission.modelId ? ` · ${provider.admission.modelId}` : ""}</p></div> : <EmptyState title="전송 프로필 미선택" text="ACP 프로필 목록에서 사용할 연결을 선택하십시오." />}
        {provider?.admission.state === "needs_auth" && <p className="unavailable-reason">공식 로그인이 필요합니다. <Button onClick={() => onNavigate("provider")}>공식 로그인 열기</Button></p>}
      </Panel>
      <Panel title="심의 역할" kicker="ROLE PRESET">
        {rolePreset ? <><div className="confirmation-destination"><strong>{rolePreset.name}</strong><p>revision {rolePreset.revision} · {rolePreset.kind === "factory" ? "기본 프리셋" : "사용자 프리셋"}</p></div><ul className="contract-list">{rolePreset.roles.map((role) => <li key={role.core}><strong>{role.label}</strong> · {role.perspective} · 출력 {role.outputLanguage === "same" ? "안건 언어와 동일" : role.outputLanguage === "ko" ? "한국어" : "영어"}</li>)}</ul></> : <EmptyState title={rolePresetsState === "loading" ? "역할 확인 중" : "역할 프리셋 미선택"} text="저장된 정확히 세 코어의 역할 revision을 확인해야 심의를 시작할 수 있습니다." />}
      </Panel>
      <Panel title="자료 전송에 대한 명시적 동의" kicker="EXTERNAL TRANSFER">
        <label className="check-row"><input type="checkbox" checked={disclosureConfirmed} disabled={!canConfirmTransfer || runStartState === "starting"} onChange={(event) => onDisclosureConfirmedChange(event.target.checked)} /><span>이 질문, 선택한 역할 지침, 캡처된 원문 자료 {capturedCount}개를 위의 선택 프로필로 전송하는 데 동의합니다. 외부 서비스의 처리와 보관은 해당 서비스 정책을 따릅니다.</span></label>
        <p className="field-help">자료 선택만으로 전송하지 않습니다. 동의를 해제하거나 질문·역할·프로필·자료를 바꾸면 다시 확인해야 합니다.</p>
      </Panel>
      {runStartState === "starting" && <p className="field-help" role="status">실제 심의 시작 요청을 보내고 저장된 Run 상태를 확인하고 있습니다.</p>}
      {runStartState === "error" && runStartError && <p className="unavailable-reason" role="alert">{runStartError}</p>}
      {runStartState !== "error" && !canStart && <p className="blocked-reason">{blockReason}</p>}
      {currentRunId && runStartState !== "starting" && <InlineCancelControl runId={currentRunId} active={hasActiveRun} cancelRequest={cancelRequest} onCancel={onCancelRun} />}
      <div className="confirmation-actions"><Button onClick={() => onNavigate("provider")}>ACP 프로필 확인</Button><Button onClick={() => onNavigate("intake")}>자료 범위 변경</Button><Button tone="primary" onClick={onStart} disabled={!canStart}>{runStartState === "starting" ? "심의 시작 중…" : "이 동의로 심의 시작"}<span aria-hidden="true">↗</span></Button></div>
    </PageHeadingAndLayout>
  );
}

function RolesPage({ presetsState, presets, selectedPresetId, draft, writeState, onSelect, onBeginEdit, onClone, onDraftChange, onSave }: {
  presetsState: RolePresetStoreState;
  presets: RolePreset[];
  selectedPresetId: string | null;
  draft: RolePresetDraft | null;
  writeState: RolePresetWriteState;
  onSelect: (presetId: string) => void;
  onBeginEdit: (presetId: string) => void;
  onClone: (presetId: string) => void;
  onDraftChange: (draft: RolePresetDraft | null) => void;
  onSave: ScreenProps["onSaveRolePreset"];
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
    <PageHeadingAndLayout english="CORE ROLE EDITOR" title="세 관점 설정" intro="세 코어의 정체성은 고정하고, 판단 관점·기준·반증 조건과 답변 언어를 프리셋으로 관리합니다.">
      <div className="role-editor">
        <nav className="role-tabs" aria-label="역할 프리셋 선택">
          {presetsState === "ready" && presets.map((preset) => <button type="button" className={preset.id === selectedPresetId ? "active" : ""} key={preset.id} aria-pressed={preset.id === selectedPresetId} onClick={() => onSelect(preset.id)}>
            <strong>{preset.name}</strong><span>{preset.kind === "factory" ? "기본 프리셋" : "사용자 프리셋"} · rev {preset.revision}</span>
          </button>)}
          {presetsState === "ready" && presets.filter((preset) => preset.kind === "factory").length === 0 && <EmptyState title="기본 코어 프리셋 미확인" text="원작의 세 코어 역할을 불러오지 못해 사용자 프리셋을 만들 수 없습니다." />}
        </nav>

        <div className="role-detail">
          {mismatchedDraft && <div className="inline-warning" role="alert"><strong>편집 중인 역할과 선택된 프리셋이 다릅니다</strong><p>선택이 바뀐 프리셋에 편집 초안을 적용하지 않았습니다. 저장된 선택을 다시 확인하거나 초안을 닫으십시오.</p><Button onClick={() => onDraftChange(null)}>초안 닫기</Button></div>}
          {presetsState === "loading" && <Panel title="역할 프리셋 확인 중" kicker="ROLE STORE"><EmptyState title="저장된 프리셋을 읽고 있습니다" text="기본 프리셋이나 사용자 역할을 임의로 만들지 않습니다." /></Panel>}
          {presetsState === "unavailable" && <Panel title="역할 저장소를 사용할 수 없습니다" kicker="ROLE STORE"><EmptyState title="역할을 읽거나 저장할 수 없습니다" text="역할 서비스가 제공되지 않아 이 화면에서 프리셋을 변경할 수 없습니다." /></Panel>}
          {presetsState === "error" && <Panel title="역할 프리셋을 읽지 못했습니다" kicker="ROLE STORE"><EmptyState title="역할 상태 미확인" text="저장된 역할과 기본 프리셋을 불러오지 못했습니다. 확인되지 않은 역할을 표시하지 않습니다." /></Panel>}
          {presetsState === "ready" && presets.length === 0 && <Panel title="사용할 수 있는 역할 프리셋 없음" kicker="ROLE STORE"><EmptyState title="기본 프리셋 미확인" text="기본 코어 자아를 확인하지 못해 프리셋을 생성하거나 심의에 배정할 수 없습니다." /></Panel>}
          {presetsState === "ready" && selected && <>
            <Panel title={editableDraft ? editableDraft.presetId ? "사용자 프리셋 편집" : "새 사용자 프리셋" : selected.name} kicker={editableDraft ? editableDraft.presetId ? `USER PRESET / REVISION ${selected.revision}` : "NEW USER PRESET / CLONED TEMPLATE" : selected.kind === "factory" ? "FACTORY / READ ONLY" : `USER PRESET / REVISION ${selected.revision}`}>
              <div className="role-core-tabs" role="tablist" aria-label="편집할 MAGI 코어">
                {roleCoreOrder.map((core) => <button type="button" role="tab" aria-selected={activeCore === core} className={activeCore === core ? "active" : ""} key={core} onClick={() => setActiveCore(core)}>{roleCoreNames[core]}</button>)}
              </div>
              {editableDraft && <label className="field role-preset-name"><span className="field-label">프리셋 이름</span><input className="text-field" value={editableDraft.name} disabled={writeState === "saving"} onChange={(event) => onDraftChange({ ...editableDraft, name: event.target.value })} /></label>}
              {activeRole && <RoleFields role={activeRole} originalSelf={originalSelves[activeRole.core]} readOnly={!editableDraft} disabled={writeState === "saving"} onChange={(update) => updateRole(activeRole.core, update)} />}
              {!activeRole && <EmptyState title="코어 역할을 확인하지 못했습니다" text="선택된 프리셋에 세 개의 서로 다른 코어 역할이 있어야 합니다. 잘못된 프리셋은 저장하거나 실행할 수 없습니다." />}
              <p className="role-identity-note">Scientist·Mother·Woman은 원작의 코어 자아를 가리킵니다. 판단 기준은 성별·모성·직업 고정관념을 강제하지 않습니다.</p>
              {selected.kind === "factory" && !editableDraft && <div className="page-actions"><Button tone="primary" onClick={() => onClone(selected.id)}>복제하여 편집</Button></div>}
              {selected.kind === "user" && !editableDraft && <div className="page-actions"><Button tone="primary" onClick={() => onBeginEdit(selected.id)}>편집 시작</Button></div>}
              {editableDraft && <div className="role-save-actions">
                <Button onClick={() => onDraftChange(null)} disabled={writeState === "saving"}>편집 닫기</Button>
                <Button tone="primary" onClick={() => onSave({ presetId: editableDraft.presetId, expectedRevision: editableDraft.expectedRevision, draft: editableDraft })} disabled={!canSaveDraft}>새 revision 저장</Button>
              </div>}
              {writeState === "saving" && <p className="field-help" role="status">역할 revision을 저장하고 있습니다.</p>}
              {writeState === "saved" && <p className="field-help" role="status">역할 프리셋이 저장되었습니다. 진행 중 심의는 시작 시 고정한 역할을 유지합니다.</p>}
              {writeState === "conflict" && <div className="inline-warning" role="alert"><strong>역할 revision 충돌</strong><p>열어 둔 revision 이후 저장 내용이 바뀌었습니다. 최신 사용자 프리셋을 다시 읽어 수정하십시오.</p>{editableDraft?.presetId && selected.kind === "user" && <Button onClick={() => onBeginEdit(selected.id)}>최신 revision 다시 읽기</Button>}</div>}
              {writeState === "error" && <p className="unavailable-reason" role="alert">역할 프리셋을 저장하지 못했습니다. 편집 내용은 유지했습니다.</p>}
              {editableDraft && !completeRoles && <p className="field-help">세 코어의 이름·관점·기준·반증 조건을 모두 입력해야 저장할 수 있습니다.</p>}
            </Panel>
          </>}
          {presetsState === "ready" && !selected && presets.length > 0 && <Panel title="역할 프리셋을 선택하십시오" kicker="CORE ROLE EDITOR"><EmptyState title="선택된 역할 없음" text="기본 프리셋은 읽기 전용으로 확인하고, 사용자 프리셋은 선택해 revision을 편집합니다." /></Panel>}
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
    <label className="field"><span className="field-label">관점 이름</span><input className="text-field" value={role.label} readOnly={readOnly} disabled={disabled} onChange={(event) => onChange({ label: event.target.value })} /></label>
    <label className="field"><span className="field-label">목적</span><textarea className="text-field" value={role.perspective} readOnly={readOnly} disabled={disabled} onChange={(event) => onChange({ perspective: event.target.value })} /></label>
    <label className="field"><span className="field-label">우선 판단 기준</span><textarea className="text-field" value={role.criteria} readOnly={readOnly} disabled={disabled} onChange={(event) => onChange({ criteria: event.target.value })} /></label>
    <label className="field"><span className="field-label">의문을 제기할 조건</span><textarea className="text-field" value={role.challengeCondition} readOnly={readOnly} disabled={disabled} onChange={(event) => onChange({ challengeCondition: event.target.value })} /></label>
    <label className="field"><span className="field-label">출력 언어</span><select className="text-field" value={role.outputLanguage} disabled={readOnly || disabled} onChange={(event) => onChange({ outputLanguage: event.target.value as RoleOutputLanguage })}><option value="same">안건 언어와 동일</option><option value="ko">한국어</option><option value="en">영어</option></select></label>
  </div>;
}

function hasThreeRoleSlots(roles: RoleDefinition[]): boolean {
  const expected: CoreRole[] = ["melchior", "balthasar", "casper"];
  return roles.length === expected.length && expected.every((core) => roles.filter((role) => role.core === core).length === 1);
}

function SettingsPage({ motion, sound, theme, fontScale, onMotionChange, onSoundChange, onThemeChange, onFontScaleChange, onNavigate, onNotice }: { motion: MotionSetting; sound: boolean; theme: "command" | "clear"; fontScale: 100 | 125 | 150 | 200; onMotionChange: (value: MotionSetting) => void; onSoundChange: (value: boolean) => void; onThemeChange: (value: "command" | "clear") => void; onFontScaleChange: (value: 100 | 125 | 150 | 200) => void; onNavigate: ScreenProps["onNavigate"]; onNotice: (message: string) => void }) {
  return (
    <PageHeadingAndLayout english="CONSOLE SETTINGS" title="콘솔 설정" intro="시각 표현과 접근성 설정을 변경해도 현재 질문·근거 선택은 유지됩니다.">
      <Panel title="시각 표현" kicker="DISPLAY">
        <fieldset className="setting-row"><legend>테마</legend><label><input type="radio" name="theme" checked={theme === "command"} onChange={() => { onThemeChange("command"); onNotice("Command 테마를 적용했습니다."); }} /> Command</label><label><input type="radio" name="theme" checked={theme === "clear"} onChange={() => { onThemeChange("clear"); onNotice("Clear 테마를 적용했습니다."); }} /> Clear</label></fieldset>
        <label className="setting-row"><span><strong>동작</strong><small>OS 설정을 최초 표시에도 반영합니다.</small></span><select value={motion} onChange={(event) => onMotionChange(event.target.value as MotionSetting)}><option value="full">전체 동작</option><option value="reduced">동작 줄임</option><option value="off">동작 끔</option></select></label>
        <label className="setting-row"><span><strong>음향</strong><small>켜면 짧은 직접 제작 미리보기만 한 번 재생합니다. 입력·응답 이벤트 음향은 재생하지 않습니다.</small></span><input type="checkbox" checked={sound} onChange={(event) => onSoundChange(event.target.checked)} /></label>
        <label className="setting-row"><span><strong>글자 확대</strong><small>화면의 글자와 내용이 함께 커지고, 필요한 영역은 다시 배치됩니다.</small></span><select aria-label="글자 확대" value={fontScale} onChange={(event) => onFontScaleChange(Number(event.target.value) as 100 | 125 | 150 | 200)}><option value={100}>100%</option><option value={125}>125%</option><option value={150}>150%</option><option value={200}>200%</option></select></label>
      </Panel>
      <Panel title="언어" kicker="LANGUAGE">
        <label className="setting-row"><span><strong>콘솔 언어</strong><small id="ui-language-status">현재 한국어로 표시됩니다. 영어 UI는 준비되지 않아 선택할 수 없습니다.</small></span><select aria-label="콘솔 언어" aria-describedby="ui-language-status" value="ko" disabled><option value="ko">한국어</option></select></label>
        <div className="setting-row"><span><strong>답변 언어</strong><small>답변 언어 설정은 각 역할 프로필에 속합니다.</small></span><Button onClick={() => onNavigate("roles")}>역할 설정 열기</Button></div>
      </Panel>
      <p className="field-help" role="status">테마·동작·음향·글자 확대는 변경할 때마다 이 Mac의 앱 로컬 설정으로 저장됩니다.</p>
    </PageHeadingAndLayout>
  );
}

function HistoryPage({ data, selectedRunId, selectedDossier, dossierState, onNavigate, onSelectRun, onRefreshRunStatus }: {
  data: HomeData;
  selectedRunId: string | null;
  selectedDossier: RunDossierView | null;
  dossierState: ScreenProps["selectedRunDossierState"];
  onNavigate: ScreenProps["onNavigate"];
  onSelectRun: ScreenProps["onOpenRecentRun"];
  onRefreshRunStatus: ScreenProps["onRefreshRunStatus"];
}) {
  const selectedRun = data.recentRuns.find((run) => run.id === selectedRunId);
  const terminal = isTerminalRunStatus(selectedDossier?.status);
  return (
    <PageHeadingAndLayout english="LOCAL RECORDS" title="대화·기록" intro="저장소가 실제로 제공한 안건·상태·시각만 표시합니다.">
      <div className="history-browser">
        <section className="history-records panel" aria-labelledby="history-list-title">
          <div className="home-section-heading"><div><p className="panel-kicker">LOCAL RUN INDEX</p><h3 id="history-list-title">저장된 심의</h3></div></div>
          {data.recordsState === "loading" && <EmptyState title="기록 확인 중" text="로컬 저장소에서 실제 심의 요약을 읽고 있습니다." />}
          {data.recordsState === "unavailable" && <EmptyState title="기록 목록을 사용할 수 없습니다" text="기록 조회 기능이 제공되지 않아 빈 목록으로 판단하지 않습니다." />}
          {data.recordsState === "error" && <EmptyState title="기록을 읽지 못했습니다" text="저장소 오류로 실제 기록을 확인할 수 없습니다." />}
          {data.recordsState === "ready" && data.recentRuns.length === 0 && <EmptyState title="저장된 심의 기록이 없습니다" text="기록 조회가 완료되어 비어 있음을 확인했습니다." />}
          {data.recordsState === "ready" && data.recentRuns.length > 0 && <ul className="history-record-list">{data.recentRuns.map((run) => <li key={run.id}>
            <button type="button" className="history-record-row" aria-pressed={run.id === selectedRunId} onClick={() => onSelectRun(run.id)}>
              <span><strong>{run.question || "안건 없음"}</strong><small>{formatRecordDate(run.createdAt)}</small></span>
              <span className="home-record-status">{runStatusLabel(run.status)}</span>
              <span className="home-record-arrow" aria-hidden="true">↗</span>
            </button>
          </li>)}</ul>}
        </section>
        <section className="history-summary panel" aria-labelledby="history-summary-title">
          <p className="panel-kicker">SAVED RUN SUMMARY</p>
          <h3 id="history-summary-title">{selectedRun ? "선택한 심의" : "기록 요약"}</h3>
          {selectedRun ? <>
            <dl className="history-summary-fields">
              <div><dt>상태</dt><dd>{runStatusLabel(selectedRun.status)}</dd></div>
              <div><dt>기록 시각</dt><dd>{formatRecordDate(selectedRun.createdAt)}</dd></div>
            </dl>
            <p className="history-summary-question">{selectedRun.question || "안건 없음"}</p>
            {dossierState === "loading" && <p className="field-help" role="status">선택한 심의의 실제 저장 상태를 확인하고 있습니다.</p>}
            {dossierState === "error" && <p className="unavailable-reason" role="alert">심의 상세를 읽지 못했습니다. 목록의 실제 요약만 표시합니다.</p>}
            {dossierState === "ready" && !selectedDossier && <p className="unavailable-reason" role="alert">선택한 심의의 저장 상세를 확인할 수 없습니다.</p>}
            {selectedDossier && <>
              <dl className="history-summary-fields">
                <div><dt>저장 상태</dt><dd>{runStatusLabel(selectedDossier.status)}</dd></div>
                <div><dt>저장 단계</dt><dd>{runStageLabel(selectedDossier.stage)}</dd></div>
              </dl>
              {selectedDossier.error && <div className="unavailable-reason" role="alert"><strong>{selectedDossier.error.code}</strong><p>{selectedDossier.error.message}</p></div>}
              {selectedDossier.proposal && <>
                <h4>{selectedDossier.status === "completed" ? "저장된 결의안" : "저장된 제안 초안"}</h4>
                <ProposalContent proposal={selectedDossier.proposal} />
              </>}
              {terminal && selectedDossier.status === "completed" && <>
                <h4>최종 표결 · {outcomeLabel(selectedDossier.outcome)}</h4>
                <VoteDetails votes={selectedDossier.votes} />
                <DissentContent dossier={selectedDossier} />
              </>}
              {terminal && selectedDossier.status !== "completed" && selectedDossier.votes.length > 0 && <>
                <h4>종료 전에 저장된 의견</h4>
                <VoteDetails votes={selectedDossier.votes} />
              </>}
              {!terminal && selectedDossier.status !== "completed" && <p className="field-help">심의가 종료되지 않았습니다. 최종 표결은 저장된 완료 상태가 확인될 때까지 공개하지 않습니다.</p>}
            </>}
            <div className="page-actions">
              <Button onClick={() => onRefreshRunStatus(selectedRun.id)} disabled={dossierState === "loading"}>저장 상태 새로고침</Button>
            </div>
          </> : <EmptyState title="심의를 선택하십시오" text={selectedRunId ? "선택한 기록이 현재 목록에 없어 요약을 표시할 수 없습니다." : "기록 행을 선택하면 저장된 질문·상태·시각을 확인할 수 있습니다."} />}
        </section>
      </div>
      <div className="page-actions"><Button tone="primary" onClick={() => onNavigate("input")}>새 안건</Button></div>
    </PageHeadingAndLayout>
  );
}

function ReplayPage({ onNavigate }: { onNavigate: ScreenProps["onNavigate"] }) {
  return (
    <PageHeadingAndLayout english="READ-ONLY REPLAY" title="기록 재생" intro="저장된 공개 이벤트를 읽기 전용으로 재현합니다. 재생은 새 모델 호출을 만들지 않습니다.">
      <div className="replay-stamp"><strong>READ-ONLY REPLAY</strong><span>실제 기록만 재생</span></div>
      <Panel title="저장된 재생을 불러오지 못했습니다" kicker="PUBLIC EVENT STREAM">
        <EmptyState title="실제 기록 재생 미확인" text="저장소에서 공개 이벤트를 확인하기 전에는 진행 단계나 결과를 표시하지 않습니다." />
        <p className="unavailable-reason">저장된 Run을 선택한 뒤 재생할 수 있습니다. 기록 조회 기능이 연결되지 않았습니다.</p>
      </Panel>
      <div className="page-actions"><Button onClick={() => onNavigate("history")}>기록 목록</Button><Button tone="primary" disabled title="재생할 실제 기록이 없습니다">공유 미리보기 <span aria-hidden="true">↗</span></Button></div>
    </PageHeadingAndLayout>
  );
}

function SharePage() {
  return (
    <PageHeadingAndLayout english="SHARE PREVIEW" title="공유 미리보기" intro="로컬에서 만들 파생본을 확인합니다. 자동 게시·업로드는 하지 않습니다.">
      <div className="share-grid"><Panel title="출력 형식" kicker="EXPORT FORMAT"><fieldset className="radio-stack" disabled aria-describedby="share-unavailable"><legend className="sr-only">공유 형식</legend>{["결의 문서", "정적 결의 이미지", "공개 재생 묶음"].map((label) => <label key={label}><input type="radio" name="export" disabled />{label}</label>)}</fieldset><label className="check-row"><input type="checkbox" disabled />근거 인용 위치 포함</label><label className="check-row"><input type="checkbox" disabled />개인 경로와 비밀 후보 정제</label></Panel><Panel title="실제 출력 미리보기" kicker="SANITIZED DERIVATIVE"><EmptyState title="선택된 기록 없음" text="기록을 선택하고 정제 범위를 확인한 뒤에만 공유본을 미리 볼 수 있습니다." /></Panel></div>
      <p className="unavailable-reason" id="share-unavailable">기록 조회·정제·내보내기 서비스가 연결되지 않아 공유 설정을 사용할 수 없습니다.</p>
      <div className="page-actions"><Button tone="primary" disabled title="내보내기 서비스가 연결되지 않았습니다">로컬 내보내기</Button></div>
    </PageHeadingAndLayout>
  );
}

function DataPage() {
  return (
    <PageHeadingAndLayout english="DATA & RETENTION" title="자료·보존" intro="로컬 자료 권한·외부 전송 동의·수집본 보존은 서로 다른 상태입니다.">
      <Panel title="접근 권한" kicker="SOURCE PERMISSIONS"><EmptyState title="권한 상태 미확인" text="자료를 읽었다거나 권한을 철회했다는 상태를 추정하지 않습니다." /></Panel>
      <Panel title="기록 보존" kicker="LOCAL RETENTION"><div className="data-metric"><strong>미확인</strong><span>로컬 기록 수</span><strong>미확인</strong><span>보존 원문 수</span></div></Panel>
      <Panel title="백업과 복원" kicker="BACKUP / RESTORE"><p>복원은 무결성을 확인한 별도 저장소를 대상으로 합니다. 로그인·권한 동의·진행 중 요청을 자동 복원하지 않습니다.</p><div className="inline-actions"><Button disabled title="백업 조회 기능이 연결되지 않았습니다">백업 범위</Button><Button disabled title="복원 검증 기능이 연결되지 않았습니다">복원 파일 선택</Button></div><p className="unavailable-reason">백업·복원은 실제 저장소 검증 기능이 연결될 때까지 사용할 수 없습니다.</p></Panel>
    </PageHeadingAndLayout>
  );
}

function MaintenancePage({ onNavigate }: { onNavigate: ScreenProps["onNavigate"] }) {
  return (
    <PageHeadingAndLayout english="MAINTENANCE" title="유지관리" intro="업데이트·진단·앱 정보는 실제 서명과 저장소 상태를 검증한 뒤 제공합니다.">
      <Panel title="앱 업데이트" kicker="SIGNED RELEASE"><p>현재 버전·배포 서명·업데이트 상태를 확인하지 못했습니다. 검증되지 않은 실행 파일을 설치하지 않습니다.</p><Button disabled title="서명된 업데이트 확인 서비스가 연결되지 않았습니다">업데이트 상태 확인</Button><p className="unavailable-reason">앱 업데이트 확인은 서명 검증 서비스가 연결될 때까지 사용할 수 없습니다.</p></Panel>
      <Panel title="진단 정보" kicker="LOCAL ONLY"><p>질문·파일 내용·모델 응답·비밀은 진단에 포함하지 않습니다.</p><Button onClick={() => onNavigate("data")}>자료·보존 설정</Button></Panel>
      <Panel title="앱 정보" kicker="MAGI CONSOLE"><p>무료 팬 창작 앱 · 코드와 문서 MIT</p><p>원작 명칭·이미지·음원에 대한 권리는 코드 라이선스에 포함되지 않습니다.</p></Panel>
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
  const phase = stage ? runStageLabel(stage) : running ? "상태 확인 중" : snapshot.connection === "blocked" ? "연결 차단됨" : snapshot.connection === "unknown" ? "상태 확인 중" : "안건 대기";
  const coreScreen = stage || actualStatus ? stageScreen(stage, actualStatus) : screenForRun(currentRun);
  const question = currentDossier?.question ?? currentRun?.question;
  const progressLabel = currentProgress?.coreId
    ? `${coreDisplayName(currentProgress.coreId)} · ${currentProgress.state === "streaming" ? "응답 생성 중" : currentProgress.state === "started" ? "단계 시작" : currentProgress.state === "completed" ? "단계 완료" : "상태 확인 중"}`
    : currentProgress ? `${phase} · ${currentProgress.state === "streaming" ? "실행 중" : "상태 확인 중"}` : null;
  return (
    <section className="companion-card" role="region" aria-label="MAGI 상태 팝오버" tabIndex={0}>
      <header className="companion-header"><span className="companion-icon" aria-hidden="true">M</span><div><p>MAGI CONSOLE</p><strong>{phase}</strong></div><span className="status-chip chip-unknown">{connectionLabel(snapshot.connection)}</span></header>
      <CoreTopology screen={coreScreen} run={currentRun} onOpenCore={onOpenConsole} compact />
      <section className="companion-agenda"><span>{running ? "현재 안건" : question ? "최근 확인 안건" : "현재 안건"}</span><strong>{question ?? (running ? "실행 상태 확인 중" : "안건 대기")}</strong><p>{actualStatus ? runStatusLabel(actualStatus) : running ? phase : "실행 중인 심의가 없습니다."}</p>{progressLabel && <small role="status" aria-live="polite">{progressLabel}</small>}</section>
      {runId && running && <InlineCancelControl runId={runId} active={running} cancelRequest={cancelRequest} onCancel={onCancelRun} />}
      <div className="companion-actions">
        {runId && <Button onClick={() => onRefreshRunStatus(runId)}>상태 새로고침</Button>}
        <Button tone="primary" onClick={nativeWindow ? onOpenConsole : () => onNavigate("input")}>콘솔 열기</Button>
        {nativeWindow && <Button onClick={onOpenSettings}>설정</Button>}
        {nativeWindow && <Button onClick={onClose}>팝오버 닫기</Button>}
        {nativeWindow && <Button onClick={onRequestExit}>앱 종료</Button>}
      </div>
      <p className="companion-note">팝오버 표시·닫기는 모델 호출과 파일 접근을 만들지 않습니다.</p>
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
  return <main className="companion-window" aria-label="MAGI 상태 창"><CompanionPage run={snapshot.activeRun} snapshot={snapshot} nativeWindow runId={runId} progress={progress} dossier={dossier} cancelRequest={cancelRequest} onCancelRun={onCancelRun} onRefreshRunStatus={onRefreshRunStatus} onNavigate={() => undefined} onOpenConsole={onOpenConsole} onOpenSettings={onOpenSettings} onClose={onClose} onRequestExit={onRequestExit} /></main>;
}

function connectionLabel(state: ConsoleSnapshot["connection"]): string {
  return state === "ready" ? "확인됨" : state === "blocked" ? "차단됨" : "미확인";
}

function RecoveryPage({ screen, run, dossier, runId, cancelRequest, onCancelRun, onRefreshRunStatus, runDossierState, onNavigate }: {
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
    <PageHeadingAndLayout english={screenCatalog.find((item) => item.id === screen)?.english ?? "RUN RECOVERY"} title={title} intro="실패·중단·취소는 실제 저장 상태로 표시하며 서로 바꾸어 추정하지 않습니다.">
      <div className={`recovery-panel recovery-${screen}`}><span className="recovery-symbol" aria-hidden="true">{actualStatus === "failed" ? "!" : "◇"}</span><div><p className="panel-kicker">{actualStatus ? `RUN STATUS · ${actualStatus}` : "RUN STATUS · UNVERIFIED"}</p><h3>{actualStatus ? runStatusLabel(actualStatus) : "실행 상태를 확인하지 못했습니다"}</h3><p>{stage ? `마지막 저장 단계: ${runStageLabel(stage)} · ${stage}` : "저장된 실행 상태를 불러오지 못했습니다. 완료·실패·취소를 추정하지 않습니다."}</p></div></div>
      {runDossierState === "loading" && <p className="field-help" role="status">저장된 dossier를 다시 확인하고 있습니다.</p>}
      {runDossierState === "error" && <p className="unavailable-reason" role="alert">저장된 실행 상세를 읽지 못했습니다. 마지막으로 확인된 요약 상태만 표시합니다.</p>}
      {currentDossier?.error && <div className="unavailable-reason" role="alert"><strong>{currentDossier.error.code}</strong><p>{currentDossier.error.message}</p></div>}
      <Panel title="보존된 입력과 부분 결과" kicker="RECOVERY CHECKPOINT">
        <p>{question ?? "저장 상태 미확인"}</p>
        <ul className="contract-list"><li>마지막 확인 단계: {stage ? `${runStageLabel(stage)} · ${stage}` : "미확인"}</li>{run && <li>캡처 자료 수: {run.sourceCount}개</li>}<li>동일 요청 자동 재전송: 하지 않음</li></ul>
        {currentDossier?.proposal && <><h4>{currentDossier.status === "completed" ? "저장된 결의안" : "저장된 제안 초안"}</h4><ProposalContent proposal={currentDossier.proposal} /></>}
        {currentDossier && ["failed", "cancelled"].includes(currentDossier.status) && currentDossier.votes.length > 0 && <><h4>종료 전에 저장된 의견</h4><VoteDetails votes={currentDossier.votes} /></>}
      </Panel>
      {runId && <InlineCancelControl runId={runId} active={active} cancelRequest={cancelRequest} onCancel={onCancelRun} />}
      <div className="page-actions"><Button onClick={() => onNavigate("history")}>기록 목록</Button><Button onClick={() => onNavigate("connections")}>연결 설정</Button><Button tone="primary" onClick={() => runId && onRefreshRunStatus(runId)} disabled={!runId || runDossierState === "loading"}>상태 다시 확인</Button></div>
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
          const status = publicResult ? voteNames[vote] : sealed || run?.ballotState === "sealed" ? "표 봉인됨" : screen === "input" && !run ? "안건 대기" : run ? runStageLabel(run.stage) : stage.korean;
        const role = coreRole(core.id) ?? core.role;
        return (
          <div key={core.id} className={`core-slot core-slot-${core.id}`}>
            <span className="core-index" aria-hidden="true"><small>CORE</small>{core.number}</span>
            <button type="button" className={`core-control ${publicResult ? `vote-${vote}` : ""}`} onClick={() => onOpenCore?.(core.name)} disabled={!onOpenCore} aria-label={`${core.name}, ${role}, ${sealed ? "표 방향 비공개, 봉인됨" : publicResult ? voteNames[vote] : status}, ${onOpenCore ? compact ? "콘솔 열기" : "상세 열기" : "상세 정보 미제공"}`}>
              <span className="core-name">{core.name}</span>
              <span className="core-status">
                <span className="core-status-jp" lang="ja" aria-hidden="true">{publicResult ? voteJapanese[vote] : sealed ? "封印" : stage.japanese}</span>
                <strong>{status}</strong>
              </span>
              <span className="core-role">{role}</span>
              {!compact && <span className="core-focus">{screen === "input" ? core.focus : "공개된 의견 없음"}</span>}
              <span className="core-open">{onOpenCore ? compact ? "콘솔에서 보기" : screen === "input" ? "관점 열기" : "상세 보기" : "상세 정보 미제공"}{onOpenCore && " ↗"}</span>
            </button>
          </div>
        );
      })}
      <span id="topology-summary" className="sr-only">위쪽 BALTHASAR 2, 왼쪽 아래 CASPER 3, 오른쪽 아래 MELCHIOR 1의 고정된 삼각 배치입니다. 현재 단계: {stage.korean}.{sealed || run?.ballotState === "sealed" ? " 세 표의 방향은 봉인되어 있습니다." : publicResult ? " 검증된 공개 표결을 표시합니다." : ""}</span>
    </div>
  );
}
