import { invoke } from "@tauri-apps/api/core";
import { useEffect, useRef, useState } from "react";
import { t } from "./lib/locale";
import { Button, Panel } from "./ui-controls";
import { safeEvidenceImage } from "./lib/evidence-selection";
import type { ContextSelectionSummary } from "./lib/desktop-api";
import type { RecordEvidenceLocator as EvidenceLocator, RecordEvidenceView as EvidenceView } from "./lib/desktop-api";

export function ContextPreview({ selection, onSelectionChange }: { selection: ContextSelectionSummary | null; onSelectionChange?: (selection: ContextSelectionSummary) => void }) {
  const [locator, setLocator] = useState<EvidenceLocator | null>(null);
  const [loaded, setView] = useState<{ draftId: string; revision: number; locator: EvidenceLocator; view: EvidenceView } | null>(null);
  const view = loaded && selection?.draftId === loaded.draftId && selection.revision === loaded.revision && locator === loaded.locator && selection.sources.some(source => source.status === "captured" && source.includedLocators?.some(entry => JSON.stringify(entry) === JSON.stringify(loaded.locator))) ? loaded.view : null;
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState(false);
  useEffect(() => { setLocator(null); setView(null); setError(false); }, [selection?.draftId, selection?.revision]);
  useEffect(() => {
    if (!selection || !locator) return;
    let disposed = false;
    setLoading(true); setView(null); setError(false);
    void invoke<EvidenceView>("load_context_evidence", { draftId: selection.draftId, contextRevision: selection.revision, locator }).then(response => { if (disposed) return; if (response.sourceId !== locator.source_id || response.objectDigest !== locator.object_digest || (response.page ?? null) !== (locator.page ?? null) || (response.width ?? null) !== (locator.width ?? null) || (response.height ?? null) !== (locator.height ?? null) || (locator.start_line !== null && (response.startLine !== locator.start_line || response.endLine !== locator.end_line || response.totalLines !== locator.total_lines))) throw new Error("context_evidence_authority_mismatch"); setView({ draftId: selection.draftId, revision: selection.revision, locator, view: response }); }).catch(() => { if (!disposed) setError(true); }).finally(() => { if (!disposed) setLoading(false); });
    return () => { disposed = true; };
  }, [selection?.draftId, selection?.revision, locator]);
  return <Panel title={t("실제 전달 캡처")} kicker="CAPTURED DELIVERY PREVIEW">
    <p>{t("승인 대상인 불변 캡처만 엽니다. 원본 파일을 다시 읽거나 모델에 전송하지 않습니다.")}</p>
    {(selection?.sources ?? []).filter(source => source.status === "captured").map(source => <div key={source.sourceId}><strong>{source.displayName}</strong>{onSelectionChange && source.includedLocators?.some(entry => entry.page) && <PdfPageRange key={`${selection?.draftId}:${source.sourceId}:${selection?.revision}`} selection={selection!} source={source} onChange={onSelectionChange} />}{(source.includedLocators ?? []).map((entry,index) => <Button key={index} disabled={loading} onClick={() => setLocator(entry)}>{entry.page ? t("페이지 {0}", [entry.page]) : entry.width ? t("이미지 {0} × {1}", [entry.width, entry.height]) : t("줄 {0}–{1} / 전체 {2}줄", [entry.start_line, entry.end_line, entry.total_lines])}</Button>)}{!source.includedLocators?.length && <p role="alert">{t("전달 캡처의 정확한 범위를 확인할 수 없습니다. 자료를 다시 접수하십시오.")}</p>}</div>)}
    {loading && <p role="status">{t("캡처본을 확인하고 있습니다.")}</p>}
    {error && <p role="alert">{t("전달 캡처를 열지 못했습니다. 자료 변경 상태와 접수 범위를 다시 확인하십시오.")}</p>}
    {view && <><p>{view.page ? t("캡처된 페이지 {0}", [view.page]) : view.sourceId}</p>{view.text !== null && <pre className="text-field evidence-source" tabIndex={0} aria-label={t("전송 전 캡처 원문")}>{view.text}</pre>}{safeEvidenceImage(view.dataUrl) && <img className="evidence-image" src={safeEvidenceImage(view.dataUrl)!} alt={t("선택한 원문 이미지 · {0} × {1}", [view.width, view.height])} />}{(view.warnings ?? []).map((warning,index) => <p key={index}>{warning}</p>)}</>}
  </Panel>;
}

function PdfPageRange({ selection, source, onChange }: { selection: ContextSelectionSummary; source: ContextSelectionSummary["sources"][number]; onChange: (selection: ContextSelectionSummary) => void }) {
  const pages = (source.includedLocators ?? []).flatMap(locator => locator.page ? [locator.page] : []);
  const [start, setStart] = useState(String(Math.min(...pages)));
  const [end, setEnd] = useState(String(Math.max(...pages)));
  const [busy, setBusy] = useState(false);
  const [failed, setFailed] = useState(false);
  const generation = useRef(0);
  useEffect(() => () => { ++generation.current; }, []);
  const [available, setAvailable] = useState<number | null>(null);
  useEffect(() => {
    let disposed = false;
    void invoke<number>("load_context_pdf_page_count", { draftId: selection.draftId, contextRevision: selection.revision, sourceId: source.sourceId }).then(count => { if (!disposed && Number.isSafeInteger(count) && count > 0) setAvailable(count); }).catch(() => { if (!disposed) setFailed(true); });
    return () => { disposed = true; };
  }, [selection.draftId, selection.revision, source.sourceId]);
  const valid = available !== null && Number.isSafeInteger(Number(start)) && Number.isSafeInteger(Number(end)) && Number(start) >= 1 && Number(end) >= Number(start) && Number(end) <= available;
  const apply = async () => {
    const current = ++generation.current;
    setBusy(true); setFailed(false);
    try { const next = await invoke<ContextSelectionSummary>("select_context_pdf_pages", { draftId: selection.draftId, contextRevision: selection.revision, sourceId: source.sourceId, startPage: Number(start), endPage: Number(end) }); if (current === generation.current) onChange(next); }
    catch { if (current === generation.current) setFailed(true); }
    finally { if (current === generation.current) setBusy(false); }
  };
  return <fieldset className="context-range-fields" disabled={busy}><legend>{t("PDF 전달 페이지 범위 · {0}", [source.displayName])}</legend>
    <p>{t("선택한 페이지만 캡처와 전송 범위에 포함됩니다. 범위를 바꾸면 전송 동의와 단계별 예산을 다시 확인합니다.")}</p>
    <label className="field"><span>{t("시작 페이지")}</span><input type="number" min={1} max={available ?? undefined} value={start} onChange={event => setStart(event.target.value)} /></label>
    <label className="field"><span>{t("끝 페이지")}</span><input type="number" min={1} max={available ?? undefined} value={end} onChange={event => setEnd(event.target.value)} /></label>
    <Button disabled={!valid || busy} onClick={() => void apply()}>{t("선택한 PDF 페이지 적용")}</Button>
    {failed && <p role="alert">{t("PDF 범위를 적용하지 못했습니다. 현재 접수 변경 번호와 페이지 범위를 확인하십시오.")}</p>}
  </fieldset>;
}
