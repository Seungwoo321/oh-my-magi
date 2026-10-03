import { invoke } from "@tauri-apps/api/core";
import { t } from "./lib/locale";
import { projectRecordedReplay } from "./lib/replay-state";
import type { RunDossierView } from "./lib/desktop-api";
import { SourceFreshness } from "./source-freshness";
import { useEffect, useRef, useState } from "react";
import { Button, Panel } from "./ui-controls";
import { listRecordEvidence, loadRecordEvidence, type RecordEvidenceList, type RecordEvidenceLocator, type RecordEvidenceView, createLocalBackup, deleteRecord, importSharedReplay, listExternalReplays, listRecords, loadExternalReplay, loadRecordReplay, type ExternalReplay, type ExternalReplaySummary, type RecordHistoryCursor, type RecordReplayPage, type StoredRecordSummary } from "./lib/desktop-api";

type LoadState = "loading" | "ready" | "error";
const recordStatusLabels: Record<string, string> = { preparing: "준비", awaiting_confirmation: "입력 확인 대기", independent_review: "독립 검토", cross_review: "교차 검토", synthesis: "결의문 작성", balloting: "봉인 표결", paused: "일시정지", interrupted: "상태 미확인", cancelling: "취소 확인 중", completed: "완료", cancelled: "취소", failed: "실패" };
const statusLabel = (status: string) => t(recordStatusLabels[status] ?? "저장 상태 미확인");
const eventLabels: Record<string, string> = { run_created: "심의 저장", confirmation_required: "입력 확인 대기", run_started: "심의 시작", assessment_accepted: "의견 저장", phase_advanced: "단계 전환", proposal_frozen: "제안 확정", ballot_sealed: "표결 봉인", ballots_revealed: "표결 공개", run_completed: "심의 완료", run_cancelled: "취소 확인", run_failed: "심의 실패", run_paused: "일시정지", run_interrupted: "외부 상태 미확인", cancel_requested: "취소 요청", phase_entered: "단계 진입", assessment_available: "공개 의견", run_ended: "재생 종료" };
const eventLabel = (kind: string) => t(eventLabels[kind] ?? "저장된 이벤트");
const voteLabel = (choice: string) => t(({ support: "동의", oppose: "반대", abstain: "기권" })[choice as "support" | "oppose" | "abstain"] ?? "표결 미확인");

export function RecordBrowser({ selectedRunId, onSelectRun, onReplay, onDeleted }: { selectedRunId: string | null; onSelectRun: (id: string) => void; onReplay: () => void; onDeleted: (id: string) => void }) {
  const [query, setQuery] = useState("");
  const [status, setStatus] = useState("");
  const [rows, setRows] = useState<StoredRecordSummary[]>([]);
  const [cursor, setCursor] = useState<RecordHistoryCursor | null>(null);
  const [state, setState] = useState<LoadState>("loading");
  const [refresh, setRefresh] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const [deleting, setDeleting] = useState(false);
  const [confirming, setConfirming] = useState(false);
  const request = useRef(0);
  const mutation = useRef(false);
  const selected = rows.find((row) => row.runId === selectedRunId);

  useEffect(() => {
    const id = ++request.current;
    setState("loading"); setError(null); setConfirming(false);
    listRecords(query, status).then((page) => {
      if (request.current !== id) return;
      setRows(page.items); setCursor(page.nextCursor); setState("ready");
    }).catch(() => { if (request.current === id) { setState("error"); setError("기록을 읽지 못했습니다."); } });
    return () => { request.current++; };
  }, [query, status, refresh]);

  async function more() {
    if (!cursor || state === "loading") return;
    const id = ++request.current;
    setState("loading");
    try {
      const page = await listRecords(query, status, cursor);
      if (request.current !== id) return;
      setRows((previous) => [...previous, ...page.items.filter((row) => !previous.some((old) => old.runId === row.runId))]);
      setCursor(page.nextCursor); setState("ready");
    } catch { if (request.current === id) { setState("error"); setError("기록 페이지를 읽지 못했습니다. 다시 읽으면 현재 저장 상태부터 확인합니다."); } }
  }

  async function remove() {
    if (!selected || mutation.current) return;
    mutation.current = true; setDeleting(true); setError(null);
    try {
      await deleteRecord(selected.runId, selected.revision);
      onDeleted(selected.runId);
      setConfirming(false); setRefresh((value) => value + 1);
    } catch { setError("기록을 삭제하지 못했습니다. 실행 상태나 저장 버전이 바뀌었을 수 있습니다. 다시 읽고 확인하십시오."); }
    finally { mutation.current = false; setDeleting(false); }
  }

  return <div className="record-authority-browser">
    <Panel title={t("저장된 심의")} kicker="LOCAL RECORDS">
      <div className="record-filters">
        <label>{t("안건 검색")}<input value={query} disabled={deleting} onChange={(event) => setQuery(event.target.value)} /></label>
        <label>{t("상태")}<select aria-label={t("상태")} value={status} disabled={deleting} onChange={(event) => setStatus(event.target.value)}><option value="">{t("모든 상태")}</option>{Object.entries(recordStatusLabels).map(([value, label]) => <option key={value} value={value}>{label}</option>)}</select></label>
        <Button onClick={() => setRefresh((value) => value + 1)} disabled={state === "loading" || deleting}>{t("다시 읽기")}</Button>
      </div>
      {state === "loading" && <p role="status">{t("저장된 기록을 확인하고 있습니다.")}</p>}
      {error && <p role="alert" className="unavailable-reason">{error}</p>}
      {state === "ready" && rows.length === 0 && <p>{query || status ? t("검색 조건에 맞는 기록이 없습니다.") : t("저장된 심의 기록이 없습니다.")}</p>}
      <ul className="history-record-list">{rows.map((row) => <li key={row.runId}><button type="button" className="history-record-row" aria-pressed={row.runId === selectedRunId} disabled={deleting} onClick={() => { setConfirming(false); onSelectRun(row.runId); }}><span><strong>{row.question}</strong><small>{row.createdAt}</small></span><span>{statusLabel(row.status)}</span></button></li>)}</ul>
      {cursor && <Button onClick={() => { void more(); }} disabled={state === "loading" || deleting}>{t("다음 기록")}</Button>}
    </Panel>
    {selected && <Panel title={t("선택한 기록")}>
      <p>{selected.question}</p><p>{statusLabel(selected.status)} · {selected.updatedAt}</p>
      <div className="page-actions"><Button onClick={onReplay} disabled={deleting}>{t("공개 기록 재생")}</Button><Button tone="danger" onClick={() => setConfirming(true)} disabled={deleting}>{t("기록 삭제 확인")}</Button></div>
      {confirming && <div className="record-delete-confirm" role="group" aria-label={t("선택한 기록 삭제 확인")}><p>{t("이 안건의 질문·의견·결과와 사용되지 않는 근거를 삭제합니다. 다른 기록에서 사용하는 근거는 유지합니다. 진행 중이거나 외부 요청 상태가 확인되지 않은 실행은 삭제할 수 없습니다.")}</p><Button tone="danger" onClick={() => { void remove(); }} disabled={deleting}>{deleting ? t("삭제 중") : t("선택한 기록 삭제")}</Button><Button onClick={() => setConfirming(false)} disabled={deleting}>{t("계속 보관")}</Button></div>}
    </Panel>}
  </div>;
}

export function StoredReplay({ runId, dossier }: { runId: string | null; dossier?: RunDossierView | null }) {
  const [loaded, setLoaded] = useState<{ runId: string; page: RecordReplayPage } | null>(null);
  const page = loaded?.runId === runId ? loaded.page : null;
  const [state, setState] = useState<LoadState>("loading");
  const [position, setPosition] = useState(0);
  const generation = useRef(0);
  useEffect(() => {
    const id = ++generation.current;
    setLoaded(null); setPosition(0); setState("loading");
    if (!runId) { setState("ready"); return; }
    loadRecordReplay(runId).then((next) => { if (generation.current === id) { setLoaded({ runId, page: next }); setState("ready"); } }).catch(() => { if (generation.current === id) setState("error"); });
    return () => { generation.current++; };
  }, [runId]);
  async function more() {
    if (!page || page.complete || !runId || state === "loading") return;
    const id = ++generation.current; setState("loading");
    try { const next = await loadRecordReplay(runId, page.nextCursor); if (generation.current === id) { setLoaded({ runId, page: { ...next, events: [...page.events, ...next.events] } }); setState("ready"); } }
    catch { if (generation.current === id) setState("error"); }
  }
  const event = page?.events[position];
  const projection = projectRecordedReplay(page?.events ?? [], position);
  const matchingDossier = dossier?.runId === runId ? dossier : null;
  return <Panel title={t("저장된 공개 이벤트")} kicker="READ-ONLY REPLAY">
    {!runId && <p>{t("기록에서 재생할 심의를 선택하십시오.")}</p>}
    {state === "loading" && runId && <p role="status">{t("저장된 이벤트를 읽고 있습니다.")}</p>}
    {state === "error" && <p role="alert">{t("재생을 읽지 못했습니다. 기록에서 다시 선택하십시오.")}</p>}
    {event && <><p>{event.createdAt} · {eventLabel(event.kind)} {event.phase ? statusLabel(event.phase) : ""}</p><div className="page-actions"><Button onClick={() => setPosition(0)} disabled={position === 0}>{t("처음")}</Button><Button onClick={() => setPosition((value) => Math.max(0, value - 1))} disabled={position === 0}>{t("이전")}</Button><Button onClick={() => setPosition((value) => value + 1)} disabled={position >= (page?.events.length ?? 0) - 1}>{t("다음")}</Button></div>{event.votes && <ul>{event.votes.map((vote) => <li key={vote.coreId}><strong>{vote.coreId} · {voteLabel(vote.choice)}</strong><p>{vote.rationale}</p></li>)}</ul>}{event.outcome && <p>{{ unanimous: t("전원 동의"), majority: t("다수 동의"), rejected: t("기각"), unresolved: t("미결") }[event.outcome]}</p>}</>}
    {event && <section aria-label={t("재생 시점의 공개 상태")}><p>{projection.phase ? statusLabel(projection.phase) : t("단계 미확인")} · {projection.status ? statusLabel(projection.status) : t("진행 중")}</p>{matchingDossier?.assessments?.filter(assessment => projection.assessments.has(assessment.assessmentId)).map(assessment => <p key={assessment.assessmentId}>{assessment.coreId} · {assessment.positionSummary}</p>)}{projection.proposalVisible && matchingDossier?.proposal && <><h4>{t("이 시점에 공개된 결의문")}</h4><p>{matchingDossier.proposal.body}</p></>}{projection.votes.length > 0 ? <ul>{projection.votes.map(vote => <li key={vote.coreId}>{vote.coreId} · {voteLabel(vote.choice)} · {vote.rationale}</li>)}</ul> : <p>{t("이 시점의 표 방향은 비공개입니다.")}</p>}</section>}
    {page && !page.complete && <Button onClick={() => { void more(); }} disabled={state === "loading"}>{t("다음 공개 이벤트 읽기")}</Button>}
  </Panel>;
}

export function ExternalReplayBrowser() {
  const [items, setItems] = useState<ExternalReplaySummary[]>([]);
  const [selected, setSelected] = useState<ExternalReplay | null>(null);
  const [state, setState] = useState<LoadState>("loading");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [position, setPosition] = useState(0);
  const request = useRef(0);
  const mutation = useRef(false);
  useEffect(() => {
    const id = ++request.current;
    listExternalReplays().then((rows) => { if (request.current === id) { setItems(rows); setState("ready"); } }).catch(() => { if (request.current === id) setState("error"); });
    return () => { request.current++; };
  }, []);
  async function importFile() {
    if (mutation.current) return;
    mutation.current = true; setBusy(true); setError(null);
    const id = ++request.current;
    try {
      const replay = await importSharedReplay();
      if (request.current !== id || !replay) return;
      const rows = await listExternalReplays();
      if (request.current !== id) return;
      setItems(rows); setSelected(replay); setPosition(0); setState("ready");
    } catch { if (request.current === id) setError("공유 파일을 가져오지 못했습니다. 형식·용량·참조 검증에 실패한 입력은 기록에 섞지 않습니다."); }
    finally { mutation.current = false; setBusy(false); }
  }
  async function select(id: string) {
    const generation = ++request.current; setBusy(true); setError(null);
    try { const replay = await loadExternalReplay(id); if (request.current === generation) { setSelected(replay); setPosition(0); } }
    catch { if (request.current === generation) setError("외부 재생을 읽지 못했습니다."); }
    finally { if (request.current === generation) setBusy(false); }
  }
  async function more() {
    const last = items.at(-1); if (!last || busy) return;
    const id = ++request.current; setBusy(true);
    try { const next = await listExternalReplays(last); if (request.current === id) setItems((old) => [...old, ...next.filter((row) => !old.some((item) => item.externalReplayId === row.externalReplayId))]); }
    catch { if (request.current === id) setError("다음 외부 기록을 읽지 못했습니다."); }
    finally { if (request.current === id) setBusy(false); }
  }
  const event = selected?.data.events[position];
  const revealed = selected?.data.events.slice(0, position + 1).some((item) => item.type === "ballots_revealed");
  const proposalVisible = selected?.data.events.slice(0, position + 1).some((item) => item.type === "proposal_frozen");
  return <Panel title={t("외부 공유본")} kicker="EXTERNAL REPLAY">
    <p>{t("실행 진위 미확인 · 가져오기와 재생은 모델 호출·자료 접근 권한·전송 동의를 만들지 않습니다.")}</p>
    <Button onClick={() => { void importFile(); }} disabled={busy}>{t("공유 파일 선택")}</Button>
    {state === "loading" && <p role="status">{t("외부 기록 목록을 읽고 있습니다.")}</p>}
    {(state === "error" || error) && <p role="alert">{error ?? t("외부 기록 목록을 읽지 못했습니다.")}</p>}
    {state === "ready" && items.length === 0 && <p>{t("가져온 외부 공유본이 없습니다.")}</p>}
    <ul className="history-record-list">{items.map((item) => <li key={item.externalReplayId}><button type="button" className="history-record-row" disabled={busy} aria-pressed={item.externalReplayId === selected?.externalReplayId} onClick={() => { void select(item.externalReplayId); }}><span><strong>{item.title}</strong><small>{new Date(item.importedAtEpochMs).toLocaleString()}</small></span><span>EXTERNAL</span></button></li>)}</ul>
    {items.length >= 30 && <Button disabled={busy} onClick={() => { void more(); }}>{t("다음 외부 기록")}</Button>}
    {selected && <section aria-label={t("선택한 외부 재생")}><h4>{selected.data.title}</h4><p>{selected.data.question}</p><p>{selected.data.attribution}</p>{event && <><p>{event.sequence} · {event.offset_ms}ms · {eventLabel(event.type)}</p><div className="page-actions"><Button disabled={position === 0 || busy} onClick={() => setPosition(0)}>{t("처음")}</Button><Button disabled={position === 0 || busy} onClick={() => setPosition((value) => value - 1)}>{t("이전")}</Button><Button disabled={position >= selected.data.events.length - 1 || busy} onClick={() => setPosition((value) => value + 1)}>{t("다음")}</Button></div></>}{proposalVisible && <p>{selected.data.proposal.body}</p>}{revealed && <ul>{selected.data.ballots.map((ballot) => <li key={ballot.core_id}><strong>{ballot.core_id} · {voteLabel(ballot.vote)}</strong><p>{ballot.rationale}</p></li>)}</ul>}</section>}
  </Panel>;
}

export function RecordBackup() {
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState("");
  const guard = useRef(false);
  async function backup() {
    if (guard.current) return;
    guard.current = true; setBusy(true); setMessage("");
    try { const result = await createLocalBackup(); if (result) setMessage(`백업 검증 완료 · 이벤트 ${result.eventHighWater} · 수집 객체 ${result.objects.length}개`); }
    catch { setMessage("백업을 만들지 못했습니다. 기존 저장소를 유지합니다."); }
    finally { guard.current = false; setBusy(false); }
  }
  return <Panel title={t("로컬 백업")} kicker="CAPTURED RECORDS"><p>{t("질문·의견·수집한 원문을 포함합니다. 로그인 비밀과 실행 중 요청의 권한은 백업하지 않습니다. 저장할 폴더를 직접 선택하십시오.")}</p><Button onClick={() => { void backup(); }} disabled={busy}>{busy ? t("백업 중") : t("백업 폴더 선택")}</Button>{message && <p role="status">{message}</p>}</Panel>;
}

export type EvidenceReadingTarget = { runId: string; sourceId: string; locatorIndex: number };

function evidenceLocation(locator: RecordEvidenceLocator): string {
  if (locator.page != null) return `${locator.page}쪽${locator.width != null ? ` · ${locator.width}×${locator.height}` : ""}`;
  return locator.start_line != null ? `${locator.start_line}–${locator.end_line}행` : "선택한 전체 범위";
}

function matchesEvidence(view: RecordEvidenceView, locator: RecordEvidenceLocator, representation: string): boolean {
  if (view.sourceId !== locator.source_id || view.objectDigest !== locator.object_digest) return false;
  if (view.evidenceUnavailable) return view.text == null && view.dataUrl == null;
  if (view.representationKind !== representation || (view.page ?? null) !== (locator.page ?? null)
    || (view.width ?? null) !== (locator.width ?? null) || (view.height ?? null) !== (locator.height ?? null)) return false;
  if (locator.start_line != null && (view.startLine !== locator.start_line || view.endLine !== locator.end_line || view.totalLines !== locator.total_lines)) return false;
  if (representation === "utf8_text" || representation === "pdf_text") return typeof view.text === "string" && view.dataUrl == null;
  return view.text == null && typeof view.dataUrl === "string" && /^data:image\/png;base64,iVBORw0KGgo[A-Za-z0-9+/]+={0,2}$/.test(view.dataUrl)
    && Number.isSafeInteger(view.width) && Number.isSafeInteger(view.height) && (view.width ?? 0) > 0 && (view.height ?? 0) > 0;
}

export function RecordEvidenceBrowser({ runId, revision, target, onTargetChange, onChanged }: {
  runId: string | null; revision?: number; target?: EvidenceReadingTarget | null;
  onTargetChange?: (target: EvidenceReadingTarget) => void; onChanged?: (runId: string) => void;
}) {
  const [metadata, setMetadata] = useState<RecordEvidenceList | null>(null);
  const [state, setState] = useState<LoadState>("loading");
  const [view, setView] = useState<RecordEvidenceView | null>(null);
  const [reading, setReading] = useState<LoadState>("ready");
  const [localTarget, setLocalTarget] = useState<EvidenceReadingTarget | null>(null);
  const [refresh, setRefresh] = useState(0);
  const listGeneration = useRef(0);
  const readGeneration = useRef(0);
  const readRevision = useRef<number | null>(null);
  const selection = target === undefined ? localTarget : target;
  const currentMetadata = metadata?.runId === runId && (revision == null || metadata.revision === revision) ? metadata : null;
  const [deletion, setDeletion] = useState<{ runId: string; sourceId: string; revision: number; affectedRunIds: string[]; objectDigests: string[] } | null>(null);
  const [reviewed, setReviewed] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [deletionError, setDeletionError] = useState(false);
  const mutation = useRef(false);
  const source = currentMetadata && selection?.runId === runId ? currentMetadata.sources.find((item) => item.sourceId === selection.sourceId) : undefined;
  const locator = source && selection ? source.locators[selection.locatorIndex] : undefined;

  useEffect(() => {
    const generation = ++listGeneration.current;
    readGeneration.current++; setDeletion(null); setReviewed(false); setMetadata(null); setView(null); setState("loading");
    if (!runId) { setState("ready"); return; }
    listRecordEvidence(runId).then((next) => {
      if (listGeneration.current !== generation) return;
      if (next.runId !== runId || !Number.isSafeInteger(next.revision) || next.revision < 0 || (revision != null && next.revision !== revision)
        || !Array.isArray(next.sources) || new Set(next.sources.map((item) => item.sourceId)).size !== next.sources.length
        || next.sources.some((item) => !item.sourceId || item.availability !== "not_checked" || !Array.isArray(item.locators) || item.locators.some((value) => value.source_id !== item.sourceId))) throw new Error("evidence_authority_mismatch");
      setMetadata(next); setState("ready");
    }).catch(() => { if (listGeneration.current === generation) setState("error"); });
    return () => { listGeneration.current++; readGeneration.current++; };
  }, [runId, revision, refresh]);

  useEffect(() => {
    const generation = ++readGeneration.current;
    setView(null); setReading("ready");
    if (!runId || !locator || !source || !metadata || state !== "ready") return;
    setReading("loading");
    loadRecordEvidence(runId, locator).then((next) => {
      if (readGeneration.current !== generation) return;
      if (!matchesEvidence(next, locator, source.representation)) throw new Error("evidence_read_mismatch");
      readRevision.current = metadata.revision; setView(next); setReading("ready");
    }).catch(() => { if (readGeneration.current === generation) setReading("error"); });
    return () => { readGeneration.current++; };
  }, [runId, locator, source, state, metadata]);

  function select(sourceId: string, locatorIndex: number) {
    if (!runId) return;
    readGeneration.current++; setView(null); setDeletion(null); setReviewed(false);
    const next = { runId, sourceId, locatorIndex };
    setLocalTarget(next); onTargetChange?.(next);
  }

  async function deleteEvidence(commit: boolean) {
    if (!runId || !source || !currentMetadata || mutation.current) return;
    const captured = { runId, sourceId: source.sourceId, revision: currentMetadata.revision };
    const generation = listGeneration.current;
    mutation.current = true; setDeleting(true); setDeletionError(false);
    try {
      if (commit) {
        if (!reviewed || !deletion || deletion.runId !== runId || deletion.sourceId !== source.sourceId || deletion.revision !== currentMetadata.revision) return;
        await invoke("delete_record_evidence", { runId, sourceId: source.sourceId, expectedRevision: deletion.revision, expectedAffectedRunIds: deletion.affectedRunIds });
        if (listGeneration.current !== generation) return;
        setDeletion(null); setReviewed(false); setView(null); setRefresh(value => value + 1); onChanged?.(runId);
      } else {
        const result = await invoke<{ runId: string; sourceId: string; objectDigests: string[]; affectedRunIds: string[] }>("preview_evidence_deletion", { runId, sourceId: source.sourceId });
        if (listGeneration.current !== generation) return;
        if (result.runId !== captured.runId || result.sourceId !== captured.sourceId || !Array.isArray(result.affectedRunIds) || !result.affectedRunIds.includes(runId) || !Array.isArray(result.objectDigests)) throw new Error("deletion_authority_mismatch");
        setDeletion({ ...result, revision: captured.revision }); setReviewed(false);
      }
    } catch { if (listGeneration.current === generation) { setDeletion(null); setReviewed(false); setDeletionError(true); } }
    finally { mutation.current = false; setDeleting(false); }
  }
  const renderedGeneration = readGeneration.current;
  return <Panel title={t("수집한 근거 읽기")} kicker="CAPTURED EVIDENCE">
    {!runId && <p>{t("저장된 심의를 선택하십시오.")}</p>}
    {runId && state === "loading" && <p role="status">{t("수집한 자료 범위를 확인하고 있습니다.")}</p>}
    {state === "error" && <p role="alert">{t("근거 목록을 읽지 못했습니다. 저장 상태와 자료 권한을 다시 확인하십시오.")}</p>}
    {runId && <Button disabled={state === "loading"} onClick={() => setRefresh((value) => value + 1)}>{t("근거 다시 읽기")}</Button>}
    {state === "ready" && currentMetadata && currentMetadata.sources.length === 0 && <p>{t("이 심의에 수집한 근거가 없습니다.")}</p>}
    {state === "ready" && currentMetadata && currentMetadata.sources.length > 0 && <>
      <label>{t("수집한 자료")}<select disabled={deleting} value={source?.sourceId ?? ""} onChange={(event) => select(event.target.value, 0)}><option value="" disabled>{t("자료 선택")}</option>{currentMetadata.sources.map((item) => <option key={item.sourceId} value={item.sourceId}>{item.displayName}</option>)}</select></label>
      {selection?.runId === runId && !locator && <p role="alert">{t("선택했던 자료 범위가 현재 기록에 없습니다. 목록에서 다시 선택하십시오.")}</p>}
      {source && <><Button tone="danger" disabled={deleting} onClick={() => void deleteEvidence(false)}>{t("근거 삭제 영향 확인")}</Button>{deletion && deletion.runId === runId && deletion.sourceId === source.sourceId && deletion.revision === currentMetadata.revision && <div role="group" aria-label={t("선택한 근거 삭제 확인")}><p>{t("이 자료를 사용하는 모든 기록의 수집 내용을 삭제합니다. 질문·의견·결과와 자료의 출처는 유지합니다.")}</p><ul>{deletion.affectedRunIds.map(id => <li key={id}>{id}</li>)}</ul><label><input type="checkbox" disabled={deleting} checked={reviewed} onChange={event => setReviewed(event.target.checked)} />{t("영향받는 기록을 확인했습니다.")}</label><Button tone="danger" disabled={deleting || !reviewed} onClick={() => void deleteEvidence(true)}>{t("선택한 근거 삭제")}</Button></div>}{deletionError && <p role="alert">{t("근거를 삭제하지 못했습니다. 저장 버전과 영향받는 기록을 다시 확인하십시오.")}</p>}</>}
      {source && <SourceFreshness key={`${runId}:${source.sourceId}`} runId={runId!} sourceId={source.sourceId} />}
      {source && <div className="record-evidence-reference"><p>{source.displayName} · {source.byteLength}{t("바이트")}</p><p>{t("수집 시각 ·")}{source.capturedAtEpochMs == null ? t("시각 미제공") : new Date(source.capturedAtEpochMs).toLocaleString()}</p><p>{t("원본 상태 ·")}{{ unchanged: t("변경 없음"), changed: t("변경됨"), missing: t("찾을 수 없음"), unreadable: t("읽을 수 없음"), unchecked: t("확인하지 않음") }[(readRevision.current === currentMetadata.revision ? view?.freshness : undefined) ?? source.freshness]}</p>
        <label>{t("수집한 위치")}<select value={locator ? selection?.locatorIndex : ""} onChange={(event) => select(source.sourceId, Number(event.target.value))}>{source.locators.map((item, index) => <option key={index} value={index}>{evidenceLocation(item)}</option>)}</select></label>
      </div>}
      {reading === "loading" && <p role="status">{t("선택한 수집 범위를 읽고 있습니다.")}</p>}
      {reading === "error" && <p role="alert">{t("선택한 근거를 읽지 못했습니다. 자료와 위치를 유지합니다. 다시 읽어 확인하십시오.")}</p>}
      {readRevision.current === currentMetadata.revision && locator && view?.evidenceUnavailable && <p role="status">{t("수집한 내용을 사용할 수 없습니다. 자료 이름·수집 시각·위치는 기록에 유지됩니다.")}</p>}
      {readRevision.current === currentMetadata.revision && locator && view && matchesEvidence(view, locator, source?.representation ?? "") && !view.evidenceUnavailable && (view.text != null ? <pre className="record-evidence-text" tabIndex={0} aria-label={t("선택한 수집 내용")}>{view.text}</pre> : view.dataUrl && <figure className="record-evidence-image"><img src={view.dataUrl} onError={() => { if (readGeneration.current === renderedGeneration) { setView(null); setReading("error"); } }} width={view.width ?? undefined} height={view.height ?? undefined} alt={`${source?.displayName ?? t("수집한 자료")} · ${locator ? evidenceLocation(locator) : t("선택한 위치")}`} /><figcaption>{locator && evidenceLocation(locator)}</figcaption></figure>)}
    </>}
  </Panel>;
}
