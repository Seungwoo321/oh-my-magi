import { invoke } from "@tauri-apps/api/core";
import { useEffect, useRef, useState, type ReactNode } from "react";
import type { RunDossierView } from "./lib/desktop-api";
import { Button, Panel } from "./ui-controls";
import { t } from "./lib/locale";
import { StoreSelection } from "./store-selection";
export type PublicContent = {
  title: string;
  question: string;
  assessments: Array<{ assessment_id: string; core_id: "MELCHIOR-1" | "BALTHASAR-2" | "CASPER-3"; position_summary: string }>;
  proposal: { kind: "answer" | "recommendation"; body: string; conditions: string[]; alternatives: string[]; open_objections: string[] };
  ballot_rationales: string[];
};
type SharedReplay = {
  externalReplayId: string;
  fileDigest: string;
  external: true;
  data: {
    title: string;
    question: string;
    assessments: PublicContent["assessments"];
    proposal: PublicContent["proposal"] & { proposal_digest: string };
    ballots: Array<{ core_id: string; vote: string; rationale: string }>;
    events: Array<{ sequence: number; offset_ms: number; type: string; payload: unknown }>;
    redactions: Array<{ field: string; reason: string }>;
    redacted: boolean;
    attribution: string;
  };
};

const previewSharedReplay = (runId: string, publicContent: PublicContent) => invoke<SharedReplay>("preview_shared_replay", { runId, publicContent });
const exportSharedReplay = (runId: string, publicContent: PublicContent, expectedDigest: string) => invoke<SharedReplay | null>("export_shared_replay", { runId, publicContent, expectedDigest });
function ActionPage({english,title,intro,children}: {english:string;title:string;intro:string;children:ReactNode}) { return <section><p className="panel-kicker">{english}</p><h3>{title}</h3><p>{intro}</p>{children}</section>; }
function ErrorNotice({error}:{error:string}) { return error ? <p role="alert">{error}</p> : null; }
export function RestoreDataPanel() {
 const [busy,setBusy]=useState(false);const [confirmed,setConfirmed]=useState(false);const [message,setMessage]=useState("");const [error,setError]=useState(false);
 async function restore() {if(!confirmed||busy)return;setBusy(true);setError(false);setMessage("");try {const result=await invoke<{restoredObjects:number}|null>("restore_local_backup");if(result)setMessage(t("별도 저장소 복원 완료 · 객체 {0}개. 기존 저장소는 변경되지 않았습니다. 인증과 권한은 다시 확인하십시오.",[result.restoredObjects]));}catch{setError(true);}finally{setBusy(false);}}
 return <><Panel title={t("별도 저장소 복원")} kicker="INTEGRITY VERIFIED"><p>{t("복원은 빈 별도 폴더에만 수행합니다. 모든 객체 digest와 데이터베이스 참조를 확인합니다.")}</p><label className="check-row"><input type="checkbox" checked={confirmed} disabled={busy} onChange={event=>setConfirmed(event.target.checked)} />{t("기존 데이터와 분리된 새 폴더로 복원할 것을 확인합니다.")}</label><Button disabled={busy||!confirmed} onClick={()=>void restore()}>{t("백업과 복원 대상 폴더 선택")}</Button>{message&&<p role="status">{message}</p>}{error&&<p role="alert">{t("저장소 복원에 실패했습니다. 기존 저장소는 유지됩니다.")}</p>}</Panel><StoreSelection /></>;
}
function publicContentFor(dossier: RunDossierView): PublicContent | null {
  if (!dossier.proposal?.kind || !dossier.assessments || !Number.isSafeInteger(dossier.revision)) return null;
  return {
    title: dossier.question,
    question: dossier.question,
    assessments: dossier.assessments.map((assessment) => ({ assessment_id: assessment.assessmentId, core_id: assessment.coreId, position_summary: assessment.positionSummary })),
    proposal: { kind: dossier.proposal.kind, body: dossier.proposal?.body ?? "", conditions: dossier.proposal?.conditions ?? [], alternatives: dossier.proposal?.alternatives ?? [], open_objections: dossier.proposal?.openObjections.map((objection) => objection.rationale) ?? [] },
    ballot_rationales: dossier.votes.map((vote) => vote.rationale),
  };
}

export function ReviewedSharePanel({ dossier }: { dossier: RunDossierView | null }) {
  const operation = useRef(0);
  const [draft, setDraft] = useState<PublicContent | null>(null);
  const [preview, setPreview] = useState<SharedReplay | null>(null);
  const [confirmed, setConfirmed] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [saved, setSaved] = useState(false);
  useEffect(() => { ++operation.current; setDraft(dossier ? publicContentFor(dossier) : null); setPreview(null); setConfirmed(false); setBusy(false); setError(""); setSaved(false); return () => { ++operation.current; }; }, [dossier?.runId, dossier?.revision]);
  const edit = (next: PublicContent) => { ++operation.current; setDraft(next); setPreview(null); setConfirmed(false); setSaved(false); };
  const buildPreview = async () => {
    if (!dossier || !draft || busy) return;
    const generation = ++operation.current;
    setBusy(true); setError(""); setPreview(null); setConfirmed(false);
    try { const result = await previewSharedReplay(dossier.runId, draft); if (generation === operation.current) setPreview(result); }
    catch { if (generation === operation.current) setError(t("공유본을 만들지 못했습니다. 공개 범위와 기록을 다시 확인하십시오.")); }
    finally { if (generation === operation.current) setBusy(false); }
  };
  const exportPreview = async () => {
    if (!dossier || !draft || !preview || !confirmed || busy) return;
    const generation = ++operation.current;
    setBusy(true); setError("");
    try { const result = await exportSharedReplay(dossier.runId, draft, preview.fileDigest); if (generation === operation.current) setSaved(result !== null); }
    catch { if (generation === operation.current) setError(t("공유본을 만들지 못했습니다. 공개 범위와 기록을 다시 확인하십시오.")); }
    finally { if (generation === operation.current) setBusy(false); }
  };
  return <ActionPage english="SHARE PREVIEW" title={t("공유 미리보기")} intro={t("원본 기록과 분리된 공개 파생본을 검토합니다. 파일과 자격증명은 포함하지 않으며 자동 업로드하지 않습니다.")}>
    <Panel title={t("공개 범위 편집")} kicker="PUBLIC DERIVATIVE">
      {!dossier || !draft ? <p>{t("기록 화면에서 심의를 선택하십시오.")}</p> : <>
        <p>{t("내용을 삭제하거나 수정하면 파생본의 결의문 digest를 새로 계산합니다. 원본 기록은 변경되지 않습니다.")}</p>
        <PublicTextField label={t("공유 제목")} value={draft.title} disabled={busy} onChange={(title) => edit({ ...draft, title })} />
        <PublicTextField label={t("공개 질문")} value={draft.question} disabled={busy} onChange={(question) => edit({ ...draft, question })} />
        <PublicTextField label={t("공개 결의문")} value={draft.proposal.body} disabled={busy} onChange={(body) => edit({ ...draft, proposal: { ...draft.proposal, body } })} />
        {(["conditions", "alternatives", "open_objections"] as const).map((field) => <PublicTextField key={field} label={{ conditions: t("조건 · 한 줄에 하나"), alternatives: t("대안 · 한 줄에 하나"), open_objections: t("미해결 이견 · 한 줄에 하나") }[field]} value={draft.proposal[field].join("\n")} disabled={busy} onChange={(value) => edit({ ...draft, proposal: { ...draft.proposal, [field]: value.split("\n").filter(Boolean) } })} />)}
        {draft.assessments.map((assessment, index) => <PublicTextField key={assessment.assessment_id} label={t("{0} · 공개 의견 {1}", [assessment.core_id, index + 1])} value={assessment.position_summary} disabled={busy} onChange={(position_summary) => edit({ ...draft, assessments: draft.assessments.map((item, position) => position === index ? { ...item, position_summary } : item) })} />)}
        {draft.ballot_rationales.map((rationale, index) => <PublicTextField key={index} label={t("표결 사유 {0}", [index + 1])} value={rationale} disabled={busy} onChange={(value) => edit({ ...draft, ballot_rationales: draft.ballot_rationales.map((item, position) => position === index ? value : item) })} />)}
        <Button disabled={busy || dossier.status !== "completed"} onClick={() => void buildPreview()}>{t("정제된 실제 출력 확인")}</Button>
        {dossier.status !== "completed" && <p>{t("완료된 기록만 공유할 수 있습니다.")}</p>}
      </>}
    </Panel>
    <ErrorNotice error={error} />
    {preview && <Panel title={t("내보낼 실제 파일 내용")} kicker="SANITIZED PREVIEW">
      <p>{t("원본 파생 ·")}{preview.data.redacted ? t("정제됨") : t("자동 정제 항목 없음")}</p>
      <h4>{preview.data.title}</h4><p>{preview.data.question}</p><p>{preview.data.proposal.body}</p>
      <ul className="contract-list">{[...preview.data.proposal.conditions, ...preview.data.proposal.alternatives, ...preview.data.proposal.open_objections].map((text, index) => <li key={index}>{text}</li>)}</ul>
      {preview.data.assessments.map((assessment) => <div key={assessment.assessment_id}><h4>{assessment.core_id}</h4><p>{assessment.position_summary}</p></div>)}
      {preview.data.ballots.map((ballot) => <p key={ballot.core_id}><strong>{ballot.core_id} · {ballot.vote}</strong> {ballot.rationale}</p>)}
      <p>{preview.data.attribution}</p><p>{t("공개 이벤트")}{preview.data.events.length}{t("개 · 출력 확인값")}<code>{preview.fileDigest}</code></p>
      <details><summary>{t("정제 내역과 파일 전문")}</summary><pre className="text-field evidence-source" tabIndex={0}>{JSON.stringify(preview.data, null, 2)}</pre></details>
      <label className="check-row"><input type="checkbox" checked={confirmed} onChange={(event) => setConfirmed(event.target.checked)} />{t("질문·이견·표결 사유와 개인정보 공개 범위를 직접 확인했습니다.")}</label>
      <Button tone="primary" disabled={!confirmed || busy} onClick={() => void exportPreview()}>{t("확인한 파일 로컬 내보내기")}</Button>
    </Panel>}
    {saved && <p role="status">{t("확인한 공유본을 저장했습니다.")}</p>}
  </ActionPage>;
}

function PublicTextField({ label, value, disabled, onChange }: { label: string; value: string; disabled: boolean; onChange: (value: string) => void }) {
  return <label className="field"><span>{label}</span><textarea className="text-field" rows={3} value={value} disabled={disabled} onChange={(event) => onChange(event.target.value)} /></label>;
}

