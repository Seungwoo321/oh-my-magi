import { invoke } from "@tauri-apps/api/core";
import { useEffect, useRef, useState } from "react";
import { Button, Panel } from "./ui-controls";
import { t } from "./lib/locale";

type StoreState = { storeId: string; storeGeneration: number; locationKind: "default" | "restored"; locationLabel: string; pendingRestart: boolean; pendingSelection: PendingSelection | null };
type PendingSelection = { selectionToken: string; commandId: string; targetStoreId: string; targetStoreGeneration: number; locationLabel: string; schemaVersion: number; activated: true };
type Selection = { selectionToken: string; targetStoreId: string; targetStoreGeneration: number; locationLabel: string; schemaVersion: number; recordCount: number; requiresReauthorization: true };

export function StoreSelection() {
  const [current, setCurrent] = useState<StoreState | null>(null);
  const [selected, setSelected] = useState<Selection | PendingSelection | null>(null);
  const [reviewed, setReviewed] = useState(false);
  const [activated, setActivated] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const operation = useRef(0);
  const command = useRef<string | null>(null);
  useEffect(() => {
    const request = ++operation.current;
    void invoke<StoreState>("get_store_selection_state").then(state => { if (operation.current === request) { setCurrent(state); if (state.pendingSelection) { setSelected(state.pendingSelection); command.current = state.pendingSelection.commandId; setActivated(true); } } }).catch(() => { if (operation.current === request) setError(t("저장소 작업을 완료하지 못했습니다. 현재 저장소와 선택을 다시 확인하십시오.")); });
    return () => { ++operation.current; };
  }, []);
  const act = async (action: "select" | "activate" | "restart" | "cancel") => {
    const request = ++operation.current;
    setBusy(true); setError("");
    try {
      if (action === "select") {
        const result = await invoke<Selection | null>("select_restored_store");
        if (request !== operation.current) return;
        if (result) { setSelected(result); setReviewed(false); setActivated(false); command.current = crypto.randomUUID(); }
      } else if (selected) {
        if (action === "activate" && current && reviewed) {
          const result = await invoke<{ targetStoreId: string; restartRequired: true; requiresReauthorization: true }>("activate_restored_store", { selectionToken: selected.selectionToken, expectedStoreId: current.storeId, expectedStoreGeneration: current.storeGeneration, commandId: command.current });
          if (request !== operation.current) return;
          if (result.targetStoreId !== selected.targetStoreId || !result.restartRequired || !result.requiresReauthorization) throw new Error("Store selection authority mismatch.");
          setActivated(true);
        } else if (action === "cancel") {
          await invoke("cancel_store_selection", { selectionToken: selected.selectionToken });
          const state = await invoke<StoreState>("get_store_selection_state");
          if (request !== operation.current) return;
          setCurrent(state); setSelected(null); setReviewed(false); setActivated(false); command.current = null;
        } else if (action === "restart" && activated) await invoke("restart_into_selected_store", { selectionToken: selected.selectionToken });
      }
    } catch (cause) { if (request === operation.current) setError(t("저장소 작업을 완료하지 못했습니다. 현재 저장소와 선택을 다시 확인하십시오.")); }
    finally { if (request === operation.current) setBusy(false); }
  };
  return <Panel title={t("복원 저장소 열기")} kicker="EXPLICIT STORE SELECTION">
    {current && <p>{t("현재 저장소: {0} · {1} · 세대 {2}", [current.locationLabel, current.storeId, current.storeGeneration])}</p>}
    <p>{t("복원된 별도 폴더를 선택하고 검토한 뒤 앱을 재시작합니다. 기존 저장소를 덮어쓰지 않습니다.")}</p>
    <Button disabled={busy || !current || activated} onClick={() => void act("select")}>{t("복원 저장소 폴더 선택")}</Button>
    {selected && <>
      <p>{t("선택한 저장소: {0} · {1} · 세대 {2}", [selected.locationLabel, selected.targetStoreId, selected.targetStoreGeneration])}</p>
      {"recordCount" in selected && <p>{t("기록 {0}개 · 저장소 스키마 {1}", [selected.recordCount, selected.schemaVersion])}</p>}
      {!activated && <label className="check-row"><input type="checkbox" checked={reviewed} disabled={busy || activated} onChange={event => setReviewed(event.target.checked)} />{t("검토한 복원 저장소로 심의 실행 권한을 이전하고, 제공자 연결과 자료 전송 동의를 다시 확인할 것을 승인합니다.")}</label>}
      {!activated && <Button disabled={busy || !reviewed} onClick={() => void act("activate")}>{t("검토한 저장소로 실행 권한 이전 준비")}</Button>}
      {activated && <><p role="status">{t("재시작 전까지 기존 저장소를 사용합니다. 새 심의는 차단되어 있습니다.")}</p><Button tone="primary" disabled={busy} onClick={() => void act("restart")}>{t("선택한 저장소로 앱 재시작")}</Button></>}
      <Button disabled={busy} onClick={() => void act("cancel")}>{t("저장소 선택 취소")}</Button>
    </>}
    {current?.pendingRestart && !selected && <p role="alert">{t("대기 중인 저장소 전환이 있습니다. 검증된 복원 폴더를 다시 선택해 전환 상태를 확인하십시오.")}</p>}
    {busy && <p role="status">{t("저장소 작업 중")}</p>}{error && <p role="alert">{error}</p>}
  </Panel>;
}
