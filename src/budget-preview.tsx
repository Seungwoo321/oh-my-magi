import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import { Panel } from "./ui-controls";
import { t } from "./lib/locale";
import type { CommonContextBudgetPolicy } from "./lib/desktop-api";

type Budget = { textTokenUpperBound: number; imageCount: number; visionTokenUpperBound: number | null; priorInputReserveTokenUpperBound: number; futureArtifactReinputByteBudget: number; futureArtifactCount: number; outputReserveTokens: number; contextLimitTokens: number; requiresProviderTokenAttestation: boolean; outputRequestTargetTokens: number; providerEnforcedOutputLimitTokens: number | null; acceptedPublicOutputTokenLimitTokens: number; parserByteLimit: number; textBudgetBasis: "utf8_byte_upper_bound" };
type Preview = { commonContextBudget: CommonContextBudgetPolicy; inputFingerprint: string; ready: boolean; futureArtifactsKnown: false; scope: "serialized_base_inputs_conservative"; warnings: string[]; slots: Array<{ slot: number; stage: string; coreId: string; baseInputBytes: number; budget: Budget | null; blockedReason: string | null }> };
function previewReady(preview: Preview): boolean { return preview.ready && preview.slots.every(slot => slot.budget !== null && slot.blockedReason === null); }
export function BudgetPreview({ requestKey, enabled, onResult }: { requestKey: string; enabled: boolean; onResult: (result: { key: string; ready: boolean }) => void }) {
  const [view, setView] = useState<{ key: string; preview: Preview } | null>(null);
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    let disposed = false;
    setView(null); setFailed(false); onResult({ key: requestKey, ready: false });
    if (!enabled) return;
    const input = JSON.parse(requestKey);
    void invoke<Preview>("preview_deliberation_budget", { input }).then(preview => {
      if (!disposed) { if (!preview.commonContextBudget || Object.keys(preview.commonContextBudget).sort().join() !== "schemaVersion,settingsFieldRevision,tokenLimit" || preview.commonContextBudget.schemaVersion !== 1 || !Number.isSafeInteger(preview.commonContextBudget.tokenLimit) || preview.commonContextBudget.tokenLimit < 1 || preview.commonContextBudget.tokenLimit > 128000 || !Number.isSafeInteger(preview.commonContextBudget.settingsFieldRevision) || preview.commonContextBudget.settingsFieldRevision < 0 || preview.commonContextBudget.settingsFieldRevision !== input.expectedCommonContextBudgetRevision || !/^[a-f0-9]{64}$/.test(preview.inputFingerprint) || preview.scope !== "serialized_base_inputs_conservative" || preview.futureArtifactsKnown !== false || preview.slots.length !== 10) throw new Error("budget_preview_authority_mismatch"); setView({ key: requestKey, preview }); onResult({ key: requestKey, ready: previewReady(preview) }); }
    }).catch(() => { if (!disposed) setFailed(true); });
    return () => { disposed = true; };
  }, [requestKey, enabled, onResult]);
  const preview = view?.key === requestKey ? view.preview : null;
  const stage = { independent_review: t("독립 검토"), cross_review: t("교차 검토"), synthesis: t("결의 문안"), balloting: t("표결") };
  return <Panel title={t("단계별 전송 예산")} kicker="ACTUAL PROMPT PREFLIGHT">
    <p>{t("현재 직렬화한 입력과 표시된 미래 재입력 보수 예약을 기준으로 확인합니다. 이후 실제 입력이 상한을 넘으면 원문을 보존하고 일시정지합니다. 확인만으로 모델을 호출하거나 권한을 만들지 않습니다.")}</p>
    {!enabled && <p>{t("세 코어의 연결과 저장한 역할을 먼저 확인하십시오.")}</p>}
    {enabled && !preview && !failed && <p role="status">{t("단계별 전송 예산 확인 중")}</p>}
    {failed && <p role="alert">{t("단계별 예산을 확인하지 못해 시작할 수 없습니다. 연결과 입력을 다시 확인하십시오.")}</p>}
    {preview && <><p role="status">{previewReady(preview) ? t("모든 단계의 사전 예산 확인됨") : t("단계별 예산 제한으로 시작 차단됨")}</p><p>{t("실제 직렬화한 현재 입력과 표시된 미래 재입력 예약만 검증합니다. 미래 응답은 아직 알 수 없으며 실제 전송마다 다시 검사합니다.")}</p><code>{preview.inputFingerprint}</code>{preview.warnings.map((warning,index) => <p role="status" key={index}>{warning}</p>)}
      <ul className="contract-list">{preview.slots.map((slot,index) => <li key={index}><strong>{stage[slot.stage as keyof typeof stage] ?? slot.stage} · {slot.coreId ?? t("서기")}</strong>{slot.budget && <><p>{t("텍스트 상한 {0} · 이미지 {1}개 · 시각 상한 {2}", [slot.budget.textTokenUpperBound, slot.budget.imageCount, slot.budget.visionTokenUpperBound ?? t("제공자 확인 필요")])}</p><p>{t("이전 입력 상한 예약 {0} · 이번 출력 예약 {1} · 연결 문맥 상한 {2}", [slot.budget.priorInputReserveTokenUpperBound, slot.budget.outputReserveTokens, slot.budget.contextLimitTokens])}</p><p>{t("미래 공개 산출물 {0}개 · 각 재입력 byte 예산 {1}", [slot.budget.futureArtifactCount, slot.budget.futureArtifactReinputByteBudget])}</p><p>{t("출력 요청 목표 {0} · 제공자 강제 상한 {1} · 공개 출력 수용 상한 {2} · 파서 byte 상한 {3}", [slot.budget.outputRequestTargetTokens, slot.budget.providerEnforcedOutputLimitTokens ?? t("확인되지 않음"), slot.budget.acceptedPublicOutputTokenLimitTokens, slot.budget.parserByteLimit])}</p><p>{t("텍스트 예산은 UTF-8 byte 기반 보수 상한입니다. 제공자 전체 사용량이나 숨은 추론량을 뜻하지 않습니다.")}</p>{slot.budget.requiresProviderTokenAttestation && <p>{t("실제 전송 전 제공자의 입력 토큰 확인이 필요합니다.")}</p>}</>}{slot.blockedReason && <p role="alert">{slot.blockedReason}</p>}</li>)}</ul></>}
  </Panel>;
}
