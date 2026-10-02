import { useEffect, useRef, useState } from "react";
import { createClarificationDraft, listClarificationDrafts, loadClarificationDraft, saveClarificationQuestion, discardClarificationDraft, selectContextFiles, loadRunClarification, type ClarificationDraft, type ClarificationParent, type RunClarification, type RunDossierView } from "./lib/desktop-api";
import { Button, Panel } from "./ui-controls";

export function RunClarificationPanel({ runId, dossier, onInspectParent, onDraftChanged, onConfirmDraft, onRequestDiscard, onParentVerified }: { runId: string; dossier: RunDossierView | null; onInspectParent: () => void; onDraftChanged?: (draft: ClarificationDraft | null) => void; onConfirmDraft?: (draft: ClarificationDraft) => void; onRequestDiscard?: (action: () => void) => void; onParentVerified?: (parent: ClarificationParent | null) => void }) {
  const [result, setResult] = useState<{ runId: string; source: RunDossierView | null; value: RunClarification | null; state: "loading" | "ready" | "error" } | null>(null);
  const parentObserver = useRef(onParentVerified);
  parentObserver.current = onParentVerified;
  useEffect(() => {
    let disposed = false;
    parentObserver.current?.(null);
    setResult({ runId, source: dossier, value: null, state: "loading" });
    void loadRunClarification(runId).then(value => {
      if (!disposed) {
        parentObserver.current?.(value ? { runId: value.runId, revision: value.runRevision, inputDigest: value.inputDigest, generation: value.runGeneration } : null);
        setResult({ runId, source: dossier, value, state: "ready" });
      }
    }).catch(() => {
      if (!disposed) setResult({ runId, source: dossier, value: null, state: "error" });
    });
    return () => { disposed = true; };
  }, [runId, dossier]);
  const current = result?.runId === runId && result.source === dossier ? result : null;
  if (!current || current.state === "loading") return <p role="status" className="field-help">저장된 추가 입력 요청을 확인하고 있습니다.</p>;
  if (current.state === "error") return <p role="alert" className="unavailable-reason">추가 입력 요청을 검증하지 못했습니다. 같은 요청을 다시 실행하지 않습니다.</p>;
  if (!current.value) return null;
  const clarification = current.value;
  return <Panel title="심의를 계속하려면 필요한 입력" kicker="REQUIRED CLARIFICATION">
    <p>필수 정보가 부족해 심의가 멈췄습니다. 기존 검토는 보존되며 같은 입력을 자동으로 다시 보내지 않습니다.</p>
    <ul className="contract-list">{clarification.gaps.map(gap => <li key={JSON.stringify([gap.attemptId, gap.coreId, gap.stage, gap.gapIndex])}>
      <strong>{gap.essential ? "필수" : "추가 정보"} · {gap.coreId}</strong><p>{gap.missingInformation}</p><p>영향: {gap.impact}</p>
    </li>)}</ul>
    <p className="field-help">새 답변과 자료는 별도 자식 심의의 입력으로 확인해야 합니다. 부모 검토와 자료 권한은 자동으로 공유되지 않습니다.</p>
    <ClarificationDraftEditor key={JSON.stringify([clarification.runId, clarification.runRevision, clarification.inputDigest, clarification.runGeneration])} parentQuestion={dossier?.question ?? null} parent={{ runId: clarification.runId, revision: clarification.runRevision, inputDigest: clarification.inputDigest, generation: clarification.runGeneration }} onDraftChanged={onDraftChanged} onConfirmDraft={onConfirmDraft} onRequestDiscard={onRequestDiscard} />
    <Button onClick={onInspectParent}>보존된 부모 검토 확인</Button>
  </Panel>;
}

function ClarificationDraftEditor({ parent, parentQuestion, onDraftChanged, onConfirmDraft, onRequestDiscard }: { parent: ClarificationParent; parentQuestion: string | null; onDraftChanged?: (draft: ClarificationDraft | null) => void; onConfirmDraft?: (draft: ClarificationDraft) => void; onRequestDiscard?: (action: () => void) => void }) {
  const [drafts, setDrafts] = useState<ClarificationDraft[]>([]);
  const [draft, setDraft] = useState<ClarificationDraft | null>(null);
  const [question, setQuestion] = useState("");
  const [pending, setPending] = useState(false);
  const [error, setError] = useState("");
  const alive = useRef(true);
  const busy = useRef(false);
  const change = useRef(onDraftChanged);
  change.current = onDraftChanged;
  const parentKey = JSON.stringify(parent);
  useEffect(() => {
    alive.current = true;
    let disposed = false;
    void listClarificationDrafts(parent).then(saved => { if (!disposed) setDrafts(saved); }).catch(() => { if (!disposed) setError("저장된 자식 초안을 확인하지 못했습니다."); });
    return () => { disposed = true; alive.current = false; };
  }, [parentKey]);
  async function perform(action: () => Promise<ClarificationDraft | null>, preserveQuestion = false) {
    if (busy.current || !alive.current) return;
    busy.current = true; setPending(true); setError("");
    try {
      const next = await action();
      if (!alive.current) return;
      if (next && (next.parent.runId !== parent.runId || next.parent.revision !== parent.revision || next.parent.inputDigest !== parent.inputDigest || next.parent.generation !== parent.generation)) throw new Error("Parent changed.");
      setDraft(next);
      if (!preserveQuestion) setQuestion(next?.question ?? "");
      setDrafts(previous => next ? [next, ...previous.filter(item => item.context.draftId !== next.context.draftId)] : previous.filter(item => item.context.draftId !== draft?.context.draftId));
      change.current?.(next);
    } catch {
      if (alive.current) setError("초안을 저장하거나 확인하지 못했습니다. 작성한 질문은 유지했습니다. 부모와 자료 revision을 다시 확인하십시오.");
    } finally {
      busy.current = false;
      if (alive.current) setPending(false);
    }
  }
  return <section aria-label="자식 심의 초안">
    <h4>새 입력으로 자식 심의 준비</h4>
    <p>부모 검토는 보존됩니다. 자식 초안은 자료와 전송 권한 없이 시작합니다.</p>
    {!draft && <><Button disabled={pending} onClick={() => void perform(() => createClarificationDraft(parent))}>자식 초안 만들기</Button>
      {drafts.map(saved => <Button key={saved.context.draftId} disabled={pending} onClick={() => void perform(() => loadClarificationDraft(saved.context.draftId))}>저장된 자식 초안 열기 · revision {saved.context.revision}</Button>)}</>}
    {draft && <>
      <label className="field"><span className="field-label">보충 답변을 포함한 새 질문</span><textarea className="text-field" rows={5} value={question} disabled={pending} onChange={event => setQuestion(event.target.value)} /></label>
      <p className="field-help">저장된 revision {draft.context.revision} · 자식 자료 {draft.context.sources.length}개</p>
      <Button disabled={pending || !question.trim() || question === draft.question} onClick={() => void perform(() => saveClarificationQuestion(draft, question))}>질문 저장</Button>
      <Button disabled={pending} onClick={() => void perform(async () => { const selected = await selectContextFiles({ draftId: draft.context.draftId, expectedRevision: draft.context.revision }); return selected ? loadClarificationDraft(draft.context.draftId) : draft; }, true)}>새 자료 선택</Button>
      <Button disabled={pending} onClick={() => void perform(() => loadClarificationDraft(draft.context.draftId), true)}>저장 revision 다시 확인</Button>
      <Button disabled={pending || !onRequestDiscard} onClick={() => onRequestDiscard?.(() => void perform(async () => { await discardClarificationDraft(draft); return null; }))}>자식 초안 삭제</Button>
      {onConfirmDraft && <Button disabled={pending || question !== draft.question || !question.trim() || parentQuestion === null || (question.trim() === parentQuestion.trim() && draft.context.sources.length === 0)} onClick={() => onConfirmDraft(draft)}>자식 입력과 전송 확인</Button>}
      <p className="field-help">자식 심의 시작은 질문·자료와 별도 전송 확인을 완료한 뒤에만 허용됩니다.</p>
    </>}
    {pending && <p role="status" className="field-help">자식 초안의 저장 상태를 확인하고 있습니다.</p>}
    {error && <p role="alert" className="unavailable-reason">{error}</p>}
  </section>;
}
