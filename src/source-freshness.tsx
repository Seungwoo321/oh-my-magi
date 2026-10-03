import { invoke } from "@tauri-apps/api/core";
import { useEffect, useRef, useState } from "react";
import { Button } from "./ui-controls";
import { getLocale, t } from "./lib/locale";

type FreshnessView = { runId: string; sourceId: string; capturedDigest: string; observedDigest: string | null; status: "unchanged" | "changed" | "missing" | "unreadable" | "unchecked"; observedAtEpochMs: number; canRecheck: boolean; reason: string | null };

export function SourceFreshness({ runId, sourceId }: { runId: string; sourceId: string }) {
  const [view, setView] = useState<FreshnessView | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState(false);
  const generation = useRef(0);
  useEffect(() => () => { ++generation.current; }, []);
  const request = async (command: string) => {
    const current = ++generation.current;
    setBusy(true); setError(false);
    try {
      const result = await invoke<FreshnessView | null>(command, { runId, sourceId });
      if (current !== generation.current) return;
      if (result && (result.runId !== runId || result.sourceId !== sourceId)) throw new Error("Source observation mismatch.");
      if (result) setView(result);
    } catch { if (current === generation.current) setError(true); }
    finally { if (current === generation.current) setBusy(false); }
  };
  const labels = { unchanged: t("캡처본과 같음"), changed: t("캡처 후 변경됨"), missing: t("현재 파일 없음"), unreadable: t("현재 파일을 읽을 수 없음"), unchecked: t("현재 파일 미확인") };
  return <section aria-label={t("현재 파일과 캡처본 비교")}>
    <h4>{t("현재 파일과 캡처본 비교")}</h4>
    <p>{t("사용자가 선택한 현재 파일만 비교합니다. 보존한 원문은 바뀌지 않으며 로컬 접근 권한은 이 앱 실행 동안만 유지됩니다.")}</p>
    <p role="status">{labels[view?.status ?? "unchecked"]}</p>
    {view && <><p>{t("관찰 시각: {0}", [new Date(view.observedAtEpochMs).toLocaleString(getLocale() === "en" ? "en-US" : "ko-KR")])}</p><p>{t("캡처 digest: {0}", [view.capturedDigest])}</p>{view.observedDigest && <p>{t("현재 digest: {0}", [view.observedDigest])}</p>}{view.status === "changed" && <p>{t("새 내용을 심의하려면 부모 기록에서 새 질문을 만들고 자료를 다시 접수하십시오.")}</p>}</>}
    <Button disabled={busy} onClick={() => void request("select_source_freshness_file")}>{t("비교할 현재 파일 선택")}</Button>
    <Button disabled={busy || !view?.canRecheck} onClick={() => void request("recheck_source_freshness")}>{t("허용한 파일 상태 다시 확인")}</Button>
    <Button disabled={busy || !view?.canRecheck} onClick={() => void request("revoke_source_freshness_access")}>{t("현재 파일 접근 철회")}</Button>
    {error && <p role="alert">{t("현재 파일을 비교하지 못했습니다. 파일을 다시 선택하십시오.")}</p>}
  </section>;
}
