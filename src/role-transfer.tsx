import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import { t } from "./lib/locale";
import { Button, Panel } from "./ui-controls";
import type { RolePreset } from "./screens";
import type { RolePresetRevision } from "./generated/contracts";
type ImportPreview = { previewToken: string; fileDigest: string; preset: RolePresetRevision; duplicatePerspective: boolean };
export function RoleTransfer({ selected, onChanged }: { selected: RolePreset | undefined; onChanged: () => void }) {
  const [preview, setPreview] = useState<ImportPreview | null>(null);
  const [reviewed, setReviewed] = useState(false);
  const [replace, setReplace] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState(false);
  const [saved, setSaved] = useState(false);
  useEffect(() => { setReplace(false); setReviewed(false); }, [selected?.id, selected?.revision]);
  const open = async () => {
    setPreview(null); setReviewed(false); setReplace(false); setError(false); setSaved(false); setBusy(true);
    try { setPreview(await invoke<ImportPreview | null>("preview_role_preset_import")); }
    catch { setError(true); }
    finally { setBusy(false); }
  };
  const apply = async () => {
    if (!preview || !reviewed) return;
    setBusy(true); setError(false);
    try { await invoke("apply_role_preset_import", { previewToken: preview.previewToken, targetPresetId: replace && selected?.kind === "user" ? selected.id : null, expectedRevision: replace && selected?.kind === "user" ? selected.revision : null }); setPreview(null); setReviewed(false); setSaved(true); onChanged(); }
    catch { setError(true); }
    finally { setBusy(false); }
  };
  const exportPreset = async () => {
    if (!selected) return;
    setBusy(true); setError(false);
    try { await invoke("export_role_preset", { presetId: selected.id, revision: selected.revision }); }
    catch { setError(true); }
    finally { setBusy(false); }
  };
  return <Panel title={t("역할 프리셋 가져오기·내보내기")} kicker="VALIDATED ROLE DATA">
    <p>{t("외부 역할 파일은 검증된 내용만 미리 보고 사용자 프리셋으로 저장합니다. 모델 배정과 전송 권한은 가져오지 않으며 자동 활성화하지 않습니다.")}</p>
    <Button disabled={busy} onClick={() => void open()}>{t("역할 파일 선택 및 검증")}</Button><Button disabled={busy || !selected} onClick={() => void exportPreset()}>{t("선택한 저장 revision 내보내기")}</Button>
    {preview && <><h4>{preview.preset.display_name}</h4><p>SHA-256 · {preview.fileDigest}</p>{preview.preset.roles.map(role => <section key={role.core_id}><h4>{role.core_id} · {role.display_name}</h4><p>{role.review_purpose}</p><ul>{role.evaluation_criteria.map((text,index) => <li key={index}>{text}</li>)}</ul><ul>{role.falsification_questions.map((text,index) => <li key={index}>{text}</li>)}</ul><p>{role.response_language}</p></section>)}
      {preview.duplicatePerspective && <p role="status">{t("일부 관점이 중복됩니다. 세 코어가 서로 다른 판단 기준을 사용하는지 확인하십시오.")}</p>}
      {selected?.kind === "user" && <label className="check-row"><input type="checkbox" checked={replace} disabled={busy} onChange={event => { setReplace(event.target.checked); setReviewed(false); }} />{t("선택한 사용자 프리셋의 revision {0}을 기준으로 갱신합니다.", [selected.revision])}</label>}
      <label className="check-row"><input type="checkbox" checked={reviewed} disabled={busy} onChange={event => setReviewed(event.target.checked)} />{t("세 역할의 실제 내용과 적용 대상을 확인했습니다.")}</label><Button disabled={busy || !reviewed} onClick={() => void apply()}>{replace ? t("확인한 revision에 가져오기 적용") : t("새 사용자 프리셋으로 저장")}</Button>
    </>}
    {saved && <p role="status">{t("역할을 저장했습니다. 사용할 프리셋은 목록에서 명시적으로 선택하십시오.")}</p>}
    {error && <p role="alert">{t("역할 파일 작업에 실패했습니다. 파일 검증과 최신 revision을 다시 확인하십시오.")}</p>}
  </Panel>;
}
