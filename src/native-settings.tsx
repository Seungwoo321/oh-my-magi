import { invoke } from "@tauri-apps/api/core";
import { useState } from "react";
import { t } from "./lib/locale";
import { Button, Panel } from "./ui-controls";

type UpdateStatus = { status: "up_to_date" | "available" | "blocked" | "downloading" | "ready_to_restart"; currentVersion: string; version?: string; notes?: string; reason?: string; candidateId?: string };
export function SignedUpdatePanel() {
  const [result, setResult] = useState<UpdateStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const [confirmed, setConfirmed] = useState(false);
  const [error, setError] = useState(false);
  const action = async (command: "check_for_update" | "install_update" | "restart_after_update") => {
    setBusy(true); setError(false);
    if (command === "check_for_update") setResult(null);
    try { const response = await invoke<UpdateStatus>(command, command === "install_update" ? { expectedCandidateId: result?.candidateId } : undefined); if (command !== "restart_after_update") { setResult(response); setConfirmed(false); } }
    catch { setError(true); if (command === "install_update") { setResult(null); setConfirmed(false); } }
    finally { setBusy(false); }
  };
  return <Panel title={t("앱 업데이트")} kicker="SIGNED RELEASE">
    <p>{t("서명과 호환성이 검증된 업데이트만 적용합니다. 활성 심의가 종료되거나 취소 확인되기 전에는 설치하지 않습니다.")}</p>
    <Button disabled={busy} onClick={() => void action("check_for_update")}>{t("업데이트 상태 확인")}</Button>
    {result && <><p>{t("현재 버전: {0}", [result.currentVersion])} · {result.version}</p><p role="status">{result.status === "up_to_date" ? t("최신 버전입니다.") : result.status === "blocked" ? t("업데이트가 차단되었습니다.") : result.status === "ready_to_restart" ? t("검증된 업데이트 설치 완료 · 재시작 필요") : result.status === "downloading" ? t("업데이트 다운로드 중") : t("새 업데이트가 있습니다.")}</p>{result.reason && <p>{result.reason}</p>}{result.notes && <pre className="evidence-source">{result.notes}</pre>}
      {result.status === "available" && <><label className="check-row"><input type="checkbox" checked={confirmed} disabled={busy} onChange={event => setConfirmed(event.target.checked)} />{t("릴리스 정보를 확인했으며 업데이트 설치에 동의합니다.")}</label><Button disabled={!confirmed || busy || !result.candidateId} onClick={() => void action("install_update")}>{t("검증 후 업데이트 설치")}</Button></>}
      {result.status === "ready_to_restart" && <Button disabled={busy} onClick={() => void action("restart_after_update")}>{t("업데이트를 적용하고 재시작")}</Button>}
    </>}
    {error && <p role="alert">{t("업데이트 작업을 완료하지 못했습니다. 기존 기록과 설치를 유지합니다.")}</p>}
  </Panel>;
}
