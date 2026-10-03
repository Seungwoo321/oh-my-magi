import { t } from "./lib/locale";
import { useEffect, useRef, useState } from "react";
import { Button, Panel } from "./ui-controls";
import { applyPdfRangeCapture, discardPdfRangeCapture, isDesktopApp, preparePdfRangeCapture, type ContextSelectionSummary, type PdfRangePreview } from "./lib/desktop-api";

function selectionKey(selection: ContextSelectionSummary | null): string {
  return selection ? `${selection.draftId}:${selection.revision}:${selection.manifestDigest}` : "empty";
}

export function PdfRangeCapture({ selection, onChange }: { selection: ContextSelectionSummary | null; onChange: (selection: ContextSelectionSummary) => void }) {
  const [preview, setPreview] = useState<PdfRangePreview | null>(null);
  const [start, setStart] = useState("1");
  const [end, setEnd] = useState("1");
  const [phase, setPhase] = useState<"idle" | "selecting" | "applying">("idle");
  const [error, setError] = useState(false);
  const [expired, setExpired] = useState(false);
  const generation = useRef(0);
  const busy = useRef(false);
  const mounted = useRef(true);
  const pending = useRef<PdfRangePreview | null>(null);
  const currentKey = useRef(selectionKey(selection));
  currentKey.current = selectionKey(selection);
  const selectAnchor = useRef<HTMLDivElement>(null);
  const startInput = useRef<HTMLInputElement>(null);
  const key = selectionKey(selection);

  const release = (value: PdfRangePreview | null) => {
    if (value) void discardPdfRangeCapture(value.selectionToken).catch(() => undefined);
  };
  useEffect(() => {
    mounted.current = true;
    ++generation.current;
    release(pending.current);
    pending.current = null;
    busy.current = false;
    setPreview(null);
    setExpired(false);
    setPhase("idle");
    return () => {
      mounted.current = false;
      ++generation.current;
      release(pending.current);
      pending.current = null;
    };
  }, [key]);
  useEffect(() => {
    if (!preview) return;
    startInput.current?.focus();
    const timer = window.setTimeout(() => setExpired(true), Math.max(0, preview.expiresAtEpochMs - Date.now()));
    return () => window.clearTimeout(timer);
  }, [preview]);

  const current = (request: number, capturedKey: string) => mounted.current && request === generation.current && capturedKey === currentKey.current;
  const cancel = () => {
    if (busy.current) return;
    ++generation.current;
    release(pending.current);
    pending.current = null;
    setPreview(null);
    setExpired(false);
    setError(false);
    selectAnchor.current?.querySelector("button")?.focus();
  };
  const prepare = async () => {
    if (busy.current) return;
    busy.current = true;
    const request = ++generation.current;
    const capturedKey = currentKey.current;
    release(pending.current);
    pending.current = null;
    setPreview(null);
    setExpired(false);
    setPhase("selecting");
    setError(false);
    try {
      const result = await preparePdfRangeCapture(selection);
      if (!current(request, capturedKey)) { release(result); return; }
      if (!result) return;
      pending.current = result;
      setPreview(result);
      setStart("1");
      setEnd(String(Math.min(200, result.totalPages)));
    } catch {
      if (current(request, capturedKey)) setError(true);
    } finally {
      if (current(request, capturedKey)) { busy.current = false; setPhase("idle"); }
    }
  };
  const valid = Boolean(preview && !expired && Date.now() < preview.expiresAtEpochMs && Number.isSafeInteger(Number(start)) && Number.isSafeInteger(Number(end)) && Number(start) >= 1 && Number(end) >= Number(start) && Number(end) <= preview.totalPages && Number(end) - Number(start) + 1 <= 200);
  const apply = async () => {
    if (!preview || !valid || busy.current) return;
    busy.current = true;
    const request = ++generation.current;
    const capturedKey = currentKey.current;
    setPhase("applying");
    setError(false);
    try {
      const result = await applyPdfRangeCapture(preview, Number(start), Number(end));
      if (current(request, capturedKey)) onChange(result);
    } catch {
      if (current(request, capturedKey)) setError(true);
    } finally {
      if (current(request, capturedKey)) { busy.current = false; setPhase("idle"); }
    }
  };
  return <Panel title={t("PDF 페이지를 골라 접수")} kicker="PDF PAGE SELECTION">
    <p>{t("먼저 파일과 전체 페이지 수만 확인합니다. 선택한 최대 200페이지를 접수하며, 적용 전에는 초안을 바꾸거나 모델로 전송하지 않습니다.")}</p>
    <div ref={selectAnchor} className="page-actions"><Button disabled={phase !== "idle" || !isDesktopApp()} onClick={() => void prepare()}>{t("페이지 범위를 정할 PDF 선택")}</Button></div>
    {phase !== "idle" && <p role="status">{phase === "selecting" ? t("파일 선택과 페이지 수를 확인하는 중…") : t("선택한 페이지를 접수하는 중…")}</p>}
    {preview && <>
      <fieldset disabled={phase !== "idle" || expired}><legend>{preview.displayName}</legend><p>{t("전체")}{preview.totalPages}{t("페이지 · 한 번에 최대 200페이지")}</p><p className="field-help">{t("수집본 digest ·")}{preview.capturedDigest.slice(0, 12)}…</p>
        <label className="field"><span>{t("시작 페이지")}</span><input ref={startInput} type="number" min={1} max={preview.totalPages} value={start} onChange={event => setStart(event.target.value)} /></label>
        <label className="field"><span>{t("끝 페이지")}</span><input type="number" min={1} max={preview.totalPages} value={end} onChange={event => setEnd(event.target.value)} /></label>
        <Button tone="primary" disabled={!valid || phase !== "idle"} onClick={() => void apply()}>{t("이 페이지 범위만 접수")}</Button>
      </fieldset>
      <div className="page-actions"><Button disabled={phase !== "idle"} onClick={cancel}>{t("PDF 선택 취소")}</Button></div>
    </>}
    {expired && <p role="alert">{t("PDF 선택 권한이 만료되었습니다. 현재 초안은 유지됩니다. 파일을 다시 선택하십시오.")}</p>}
    {error && <p role="alert">{t("PDF 범위를 접수하지 못했습니다. 입력한 페이지와 기존 초안은 유지됩니다. 다시 확인하고 재시도하십시오.")}</p>}
    {!isDesktopApp() && <p className="field-help">{t("PDF 선택은 데스크톱 앱에서 사용할 수 있습니다.")}</p>}
  </Panel>;
}
