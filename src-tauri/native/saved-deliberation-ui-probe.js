(async () => {
  "use strict";
  const config = window.__MAGI_SAVED_UI_CONFIG;
  if (!config || typeof config.nonce !== "string" || typeof config.question !== "string") return;
  delete window.__MAGI_SAVED_UI_CONFIG;
  const deadline = performance.now() + 240000;
  let phase = "bridge_ready";
  let original = null;
  let finished = false;
  const report = input => {
    if (finished) return;
    finished = true;
    if (!original) return;
    try { Promise.resolve(original("saved_deliberation_ui_report", { input: { nonce: config.nonce, ...input } })).catch(() => {}); } catch (_) {}
  };
  const mark = async next => {
    phase = next;
    if (original && !finished) await bounded(() => original("saved_deliberation_ui_progress", { input: { nonce: config.nonce, phase: next } }));
  };
  const bounded = async operation => {
    const remaining = deadline - performance.now();
    if (remaining <= 0 || finished) throw Error("bounded UI deadline");
    let timer;
    try {
      const result = await Promise.race([Promise.resolve().then(() => {
        if (performance.now() >= deadline || finished) throw Error("bounded UI deadline");
        return operation();
      }), new Promise((_, reject) => { timer = setTimeout(() => reject(Error("bounded UI deadline")), remaining); })]);
      if (performance.now() >= deadline || finished) throw Error("bounded UI deadline");
      return result;
    }
    finally { clearTimeout(timer); }
  };
  const wait = async predicate => {
    while (performance.now() < deadline && !finished) {
      const result = await bounded(predicate);
      if (result) return result;
      await bounded(() => new Promise(resolve => setTimeout(resolve, Math.min(100, deadline - performance.now()))));
    }
    throw Error("bounded UI wait");
  };
  const usable = el => Boolean(el && el.isConnected && !el.disabled && el.getClientRects().length && getComputedStyle(el).visibility === "visible" && getComputedStyle(el).display !== "none");
  const click = el => { if (!usable(el)) throw Error("control unavailable"); el.scrollIntoView({ block: "center" }); el.click(); };
  const button = labels => Array.from(document.querySelectorAll("button")).find(el => usable(el) && labels.some(label => el.textContent.trim().includes(label)));
  const readonly = (command, args) => bounded(() => original(command, args));
  const connectionStep = (profileIndex, step) => bounded(() => original("saved_deliberation_ui_progress", { input: { nonce: config.nonce, phase, connectionStep: { profileIndex, step } } }));
  const coreStep = (coreIndex, step, metadata) => bounded(() => original("saved_deliberation_ui_progress", { input: { nonce: config.nonce, phase, coreStep: { coreIndex, step, ...(metadata ? { metadata } : {}) } } }));
  try {
    const bridge = await wait(() => typeof window.__TAURI_INTERNALS__?.invoke === "function" && window.__TAURI_INTERNALS__);
    original = bridge.invoke.bind(bridge);
    await mark("bridge_ready");
    await mark("document_ready");
    await wait(() => document.readyState !== "loading" && document.body);
    await mark("new_deliberation");
    click((await wait(() => document.querySelector("button.console-brand-link"))));
    click((await wait(() => button(["새 심의 시작", "Start new deliberation"]))));
    await mark("question_writable");
    const textarea = await wait(() => {
      const field = document.getElementById("question-draft");
      return field instanceof HTMLTextAreaElement && usable(field) && !field.readOnly ? field : null;
    });
    const setter = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value").set;
    textarea.scrollIntoView({ block: "center" });
    setter.call(textarea, config.question);
    textarea.dispatchEvent(new Event("input", { bubbles: true }));
    await mark("connection_verification");
    if (!Array.isArray(config.selections) || config.selections.length !== 3) throw Error("missing fixed selections");
    click((await wait(() => Array.from(document.querySelectorAll("button")).find(el => usable(el) && ["모델 연결", "Model connections"].includes(el.textContent.trim())))));
    const rendered = () => bounded(() => new Promise(resolve => setTimeout(resolve, 100)));
    const changeSelect = (field, value) => {
      if (!usable(field)) throw Error("select unavailable");
      field.scrollIntoView({ block: "center" });
      if (!Array.from(field.options).some(option => option.value === value && !option.disabled)) throw Error("saved choice unavailable");
      Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, "value").set.call(field, value);
      field.dispatchEvent(new Event("change", { bubbles: true }));
    };
    for (const [profileIndex, choice] of config.selections.entries()) {
      const row = () => Array.from(document.querySelectorAll("li[data-profile-id]")).find(el => el.dataset.profileId === choice.providerProfileId);
      await connectionStep(profileIndex, "selection_requested");
      click((await wait(() => row()?.querySelector(".acp-profile-select:not(:disabled)"))));
      await wait(() => { const selected = row()?.querySelector(".acp-profile-select"); return usable(selected) && selected.getAttribute("aria-pressed") === "true"; });
      await connectionStep(profileIndex, "selection_confirmed");
      const check = await wait(() => Array.from(row()?.querySelectorAll("button") ?? []).find(el => usable(el) && ["연결 확인", "Check connection"].includes(el.textContent.trim())));
      await connectionStep(profileIndex, "check_requested");
      click(check);
      await rendered();
      await wait(() => row()?.textContent.includes("기존 구독 확인됨") || row()?.textContent.includes("Existing subscription verified"));
      await connectionStep(profileIndex, "authentication_confirmed");
      const model = await wait(() => { const el = document.getElementById("live-acp-model"); return usable(el) ? el : null; });
      await connectionStep(profileIndex, "model_control_ready");
      changeSelect(model, choice.modelId);
      await rendered();
      if (choice.modeId !== null) changeSelect(await wait(() => { const el = document.getElementById("connection-model-mode"); return usable(el) ? el : null; }), choice.modeId);
      await rendered();
      const profileRevision = Number(row().dataset.profileRevision);
      const before = await readonly("load_provider_catalog", { profileId: choice.providerProfileId, expectedRevision: profileRevision });
      const beforeRevision = before.modelSelectionRevision;
      if (!Number.isSafeInteger(beforeRevision)) throw Error("missing prior model revision");
      await mark("model_reconfirmation");
      click((await wait(() => { const el = button(["모델 연결 저장", "Save model connection"]); return el && !el.disabled ? el : null; })));
      await rendered();
      await wait(async () => {
        const after = await readonly("load_provider_catalog", { profileId: choice.providerProfileId, expectedRevision: profileRevision });
        if (after.modelSelectionRevision > beforeRevision + 1) throw Error("unexpected model revision");
        const binding = after.modelSelection?.binding;
        return after.selectionState === "selected" && after.modelSelectionRevision === beforeRevision + 1 && after.modelSelection?.selectionRevision === beforeRevision + 1 && binding?.providerProfileId === choice.providerProfileId && binding.profileRevision === profileRevision && binding.modelId === choice.modelId && (binding.modeId ?? null) === choice.modeId;
      });
      await wait(() => !Array.from(document.querySelectorAll("button")).some(el => ["저장 중…", "Saving…"].includes(el.textContent.trim())) && row()?.textContent.includes(choice.modelId) && !row()?.textContent.includes("저장한 모델 선택을 다시 확인하십시오"));
    }
    await mark("core_reconfirmation");
    for (const [coreIndex, choice] of config.selections.entries()) {
      await coreStep(coreIndex, "selection_requested");
      const select = await wait(() => { const el = document.getElementById(`core-${choice.coreId}`); return usable(el) ? el : null; });
      changeSelect(select, choice.providerProfileId);
      await wait(() => usable(select) && select.value === choice.providerProfileId);
      await coreStep(coreIndex, "selection_confirmed");
      const field = select.closest(".field");
      const save = await wait(() => Array.from(field.querySelectorAll("button")).find(el => usable(el) && ["코어 연결 저장", "Save core connection"].includes(el.textContent.trim())));
      const beforeRows = await readonly("load_core_model_selections", {});
      const beforeCore = beforeRows.find(item => item.coreId === choice.coreId);
      if (!Number.isSafeInteger(beforeCore?.selectionRevision)) throw Error("missing prior core revision");
      const profileRevision = Number(Array.from(document.querySelectorAll("li[data-profile-id]")).find(el => el.dataset.profileId === choice.providerProfileId)?.dataset.profileRevision);
      const selectedModel = await readonly("load_provider_catalog", { profileId: choice.providerProfileId, expectedRevision: profileRevision });
      await coreStep(coreIndex, "save_requested");
      click(save);
      await coreStep(coreIndex, "save_read_requested");
      let lastReadMetadata = null;
      await wait(async () => {
        const rows = await readonly("load_core_model_selections", {});
        const after = rows.find(item => item.coreId === choice.coreId);
        const selection = after?.selection;
        const metadata = {
          beforeRevision: beforeCore.selectionRevision,
          expectedRevision: beforeCore.selectionRevision + 1,
          afterRevision: Number.isSafeInteger(after?.selectionRevision) && after.selectionRevision >= 0 ? after.selectionRevision : null,
          selectionState: ["selected", "stale", "unselected"].includes(after?.selectionState) ? after.selectionState : "unknown",
          revisionMatches: after?.selectionRevision === beforeCore.selectionRevision + 1 && selection?.selectionRevision === after?.selectionRevision,
          profileMatches: selection?.providerProfileId === choice.providerProfileId,
          profileRevisionMatches: selection?.profileRevision === profileRevision,
          modelRefMatches: selection?.modelSelectionRevision === selectedModel.modelSelectionRevision,
        };
        const summary = JSON.stringify(metadata);
        if (summary !== lastReadMetadata) {
          await coreStep(coreIndex, "save_read_received", metadata);
          lastReadMetadata = summary;
        }
        if (after?.selectionRevision > beforeCore.selectionRevision + 1) throw Error("unexpected core revision");
        return after?.selectionState === "selected" && after.selectionRevision === beforeCore.selectionRevision + 1 && selection?.selectionRevision === after.selectionRevision && selection.providerProfileId === choice.providerProfileId && selection.profileRevision === profileRevision && selection.modelSelectionRevision === selectedModel.modelSelectionRevision;
      });
      await coreStep(coreIndex, "save_acknowledged");
      await rendered();
      await wait(() => !field.textContent.includes("저장된 모델 선택을 다시 확인하십시오.") && !field.textContent.includes("Recheck the saved model selection.") && !field.textContent.includes("저장 중…") && !field.textContent.includes("Saving…"));
      await coreStep(coreIndex, "field_settled");
    }
    await mark("draft_restored");
    click((await wait(() => button(["원래 화면으로 돌아가기", "Return to the previous screen"]))));
    await wait(() => document.getElementById("question-draft")?.value === config.question);
    if (config.sourceMode !== "file" && config.sourceMode !== "question-only") throw Error("invalid source mode");
    await mark("sources_view");
    click((await wait(() => button(["자료 보기", "View sources"]))));
    if (config.sourceMode === "file") {
      await mark("native_picker");
      click((await wait(() => button(["파일 선택", "Select files"]))));
      // The owned native picker is completed separately using the approved policy file.
      await mark("capture_complete");
      await wait(() => Array.from(document.querySelectorAll(".source-row-mark")).some(mark => mark.textContent.trim() === "✓"));
    }
    await mark("input_review");
    click((await wait(() => button(["입력 확인", "Review input"]))));
    await mark("consent_ready");
    let lastConfirmationSummary = null;
    const consent = await wait(async () => {
      const checkbox = document.querySelector(".check-row input[type=checkbox]");
      const reason = document.querySelector(".blocked-reason")?.textContent.trim() ?? "";
      const exactReasons = [
        ["active_run", ["현재 심의의 저장 상태를 확인한 뒤 새 심의를 시작할 수 있습니다.", "Verify the current deliberation's saved state before starting another."]],
        ["profile_store", ["ACP 프로필 저장소 상태를 확인하지 못했습니다.", "Could not verify the ACP profile store."]],
        ["roles", ["실행에 사용할 세 코어 역할 프리셋을 확인하십시오.", "Verify the three-core role preset to use for execution."]],
        ["storage", ["로컬 저장소 준비 상태가 확인되지 않아 실행을 차단했습니다.", "Execution is blocked because local storage readiness is unverified."]],
        ["disclosure", ["전송 범위를 읽고 명시적으로 동의해야 시작할 수 있습니다.", "Read the transfer scope and give explicit consent before starting."]],
        ["budget", ["단계별 전송 예산을 확인해야 시작할 수 있습니다."]],
      ];
      const needsCheck = Array.from(document.querySelectorAll(".confirmation-destination p")).filter(el => ["저장된 연결 확인 필요", "Saved connection verification required"].includes(el.textContent.trim())).length;
      const category = exactReasons.find(([, labels]) => labels.includes(reason))?.[0]
        ?? (/^(MELCHIOR-1|BALTHASAR-2|CASPER-3)( · (MELCHIOR-1|BALTHASAR-2|CASPER-3))*의 저장된 모델 연결을 확인하십시오\.$/.test(reason) ? "core_connection" : reason ? "unknown" : "none");
      const metadata = {
        checkboxPresent: Boolean(checkbox), checkboxDisabled: Boolean(checkbox?.disabled),
        headingIsConfirmation: Array.from(document.querySelectorAll("h1,h2")).some(el => ["입력·전송 확인", "Input and transfer confirmation"].includes(el.textContent.trim())),
        blockReasonCategory: category, destinationNeedsCheckCount: Math.min(needsCheck, 3),
      };
      const summary = JSON.stringify(metadata);
      if (summary !== lastConfirmationSummary) {
        await bounded(() => original("saved_deliberation_ui_progress", { input: { nonce: config.nonce, phase, confirmationDiagnostic: metadata } }));
        lastConfirmationSummary = summary;
      }
      const el = document.querySelector(".check-row input[type=checkbox]:not(:disabled)");
      return usable(el) ? el : null;
    });
    if (!consent.checked) click(consent);
    await mark("start_ready");
    click(await wait(() => {
      const start = button(["이 동의로 심의 시작", "Start deliberation with this consent"]);
      return start && !start.disabled ? start : null;
    }));
  } catch (_) {
    report({ failure: `physical_ui_${phase}` });
  }
})();
