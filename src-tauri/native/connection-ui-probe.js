(async () => {
  const config = window.__magiConnectionProbe;
  const original = window.__TAURI_INTERNALS__.invoke;
  const invoke = (command, args) => original.call(window.__TAURI_INTERNALS__, command, args);
  let stage = "profile_query";
  let checkpointCount = 0;
  let modelMismatch = null;
  const failureCodes = new Set(["bounded_ui_wait", "profile_count", "profile_label", "saved_model", "catalog_projection_error", "authentication_time", "draft_return", "profile_save_readback", "invoke_replaced", "native_report_rejected"]);
  const deadline = performance.now() + config.remainingMs;
  const wait = async predicate => {
    while (performance.now() < deadline) {
      const value = predicate();
      if (value) return value;
      await new Promise(resolve => setTimeout(resolve, 25));
    }
    throw Error("bounded_ui_wait");
  };
  const button = (scope, text) => Array.from(scope.querySelectorAll("button"))
    .find(element => { const copy = element.cloneNode(true); copy.querySelectorAll('[aria-hidden="true"]').forEach(item => item.remove()); return copy.textContent.trim() === text; });
  const currentEditControl = profile => {
    const row = Array.from(document.querySelectorAll(".acp-profile-item")).find(item => item.isConnected && item.dataset.profileId === profile.providerProfileId && item.dataset.profileRevision === String(profile.revision));
    const control = row ? button(row, "편집") : null;
    return control?.isConnected && !control.disabled && control.dataset.profileEditId === profile.providerProfileId && control.dataset.profileEditRevision === String(profile.revision) ? control : null;
  };
  const currentProfileDialog = profile => {
    const dialog = document.querySelector(".acp-profile-dialog[open]");
    return dialog?.isConnected && dialog.dataset.profileId === (profile?.providerProfileId ?? "new") && dialog.dataset.profileRevision === String(profile?.revision ?? "new") ? dialog : null;
  };
  const openProfileEdit = async profile => {
    await phase("edit_control_wait");
    const control = await wait(() => currentEditControl(profile));
    await phase("edit_control_found");
    control.click();
    await phase("edit_clicked");
    const dialog = await wait(() => currentProfileDialog(profile));
    await phase("edit_dialog_found");
    await phase("edit_dialog_focus_wait");
    try {
      await wait(() => dialog.isConnected && dialog.contains(document.activeElement));
    } catch (error) {
      await phase("edit_focus_failed");
      throw error;
    }
    await phase("edit_dialog_focus_ready");
    return { control, dialog };
  };
  const setValue = (element, value) => {
    Object.getOwnPropertyDescriptor(element.tagName === "TEXTAREA" ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype, "value")
      .set.call(element, value);
    element.dispatchEvent(new Event("input", { bubbles: true }));
  };
  const report = async payload => {
    try {
      await invoke("connections_ui_probe_report", {
        input: { schemaVersion: 1, nonce: config.nonce, pid: config.pid, checkpoint: payload }
      });
      if (payload.kind !== "failed" && payload.kind !== "progress" && payload.kind !== "phase") checkpointCount += 1;
    } catch {
      throw Error("native_report_rejected");
    }
  };
  const progress = async next => { stage = next; await report({ kind: "progress", stage, checkpointCount }); };
  const phase = async value => report({ kind: "phase", stage, phase: value, checkpointCount });
  const digest = async text => Array.from(new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(text))))
    .map(byte => byte.toString(16).padStart(2, "0")).join("");
  const connections = async () => {
    await phase("home_wait");
    const home = await wait(() => button(document, "모델 연결"));
    await phase("home_ready");
    home.click();
    const list = await wait(() => document.querySelector(".acp-profile-list"));
    await phase("profile_list_ready");
    return list;
  };
  try {
    await phase("script_initialized");
    await progress("profile_query");
    const profiles = (await invoke("list_provider_profiles")).filter(profile => profile.authenticationMethod === "local_subscription");
    if (!profiles.length || profiles.length > 100) throw Error("profile_count");
    await progress("connections");
    const list = await connections();
    const elements = await wait(() => {
      const rows = Array.from(list.querySelectorAll(".acp-profile-item"));
      return rows.length === profiles.length ? rows : null;
    });
    await progress("row_projection");
    const rows = [];
    for (let index = 0; index < profiles.length; index += 1) {
      const profile = profiles[index];
      await phase("projection_pending");
      const element = await wait(() => {
        const row = Array.from(list.querySelectorAll(".acp-profile-item")).find(item => item.dataset.profileId === profile.providerProfileId && item.dataset.profileRevision === String(profile.revision));
        return row?.dataset.catalogProjection === "ready" || row?.dataset.catalogProjection === "error" ? row : null;
      });
      if (element.dataset.catalogProjection === "error") {
        await phase("projection_error");
        throw Error("catalog_projection_error");
      }
      await phase("projection_ready");
      if (element.querySelector(".acp-profile-copy strong").textContent !== profile.displayName) throw Error("profile_label");
      const catalog = await invoke("load_provider_catalog", { profileId: profile.providerProfileId, expectedRevision: profile.revision });
      const binding = catalog.modelSelection?.binding ?? null;
      const paragraphs = Array.from(element.querySelectorAll(":scope > p.field-help"));
      const modelParagraph = paragraphs.find(item => item.textContent.startsWith("저장 모델"));
      const codes = Array.from(modelParagraph.querySelectorAll("code")).map(item => item.textContent);
      const modelId = binding?.modelId ?? null, modeId = binding?.modeId ?? null;
      await phase("comparison");
      if (codes[0] !== (modelId ?? "선택하지 않음") || (codes[1] ?? null) !== modeId) {
        modelMismatch = { actualModelId: codes[0] ?? null, actualModeId: codes[1] ?? null,
          expectedModelId: modelId, expectedModeId: modeId };
        throw Error("saved_model");
      }
      const auth = paragraphs.find(item => item.textContent.startsWith("인증 상태"));
      if (!auth.textContent.includes("아직 확인하지 않음") || element.querySelector("time")) throw Error("authentication_time");
      rows.push({ profileId: profile.providerProfileId, revision: profile.revision, modelId, modeId, checkedAt: null, labelMatched: true });
    }
    await report({ kind: "rows", rows });

    await progress("draft_return");
    const coreBefore = await invoke("load_core_model_selections");
    (await wait(() => button(document, "원래 화면으로 돌아가기"))).click();
    (await wait(() => button(document, "새 심의 시작"))).click();
    const question = await wait(() => document.querySelector("#question-draft"));
    setValue(question, config.question);
    await connections();
    (await wait(() => button(document, "원래 화면으로 돌아가기"))).click();
    const returned = await wait(() => document.querySelector("#question-draft"));
    const coreAfter = await invoke("load_core_model_selections");
    if (returned.value !== config.question || JSON.stringify(coreBefore) !== JSON.stringify(coreAfter)) throw Error("draft_return");
    await report({ kind: "draft_return", questionDigest: await digest(returned.value), questionLength: returned.value.length, coresPreserved: true });

    await progress("modal_add");
    await connections();
    const addButton = await wait(() => {
      const container = document.querySelector(".acp-profile-add");
      const candidate = container ? button(container, "새 ACP 프로필") : null;
      return container?.dataset.addReadiness === "ready" && candidate && !candidate.disabled ? candidate : null;
    });
    addButton.click();
    let dialog = await wait(() => currentProfileDialog(null));
    const addInitialEmpty = dialog.querySelector("#acp-profile-alias").value === ""
      && dialog.querySelector("#acp-profile-credential-home").value === "";
    const addFocused = await wait(() => dialog.contains(document.activeElement));
    dialog.dispatchEvent(new Event("cancel", { cancelable: true }));
    await wait(() => !document.querySelector(".acp-profile-dialog"));
    const addFocusReturned = document.activeElement === addButton;

    await progress("modal_edit");
    let edit = await openProfileEdit(profiles[0]);
    dialog = edit.dialog;
    button(dialog, "취소").click();
    await wait(() => !document.querySelector(".acp-profile-dialog"));
    let editCancelled;
    try {
      editCancelled = await wait(() => edit.control.isConnected && document.activeElement === edit.control);
    } catch (error) {
      await phase("edit_focus_failed");
      throw error;
    }
    await phase("edit_trigger_focus_restored");
    edit = await openProfileEdit(profiles[0]);
    dialog = edit.dialog;
    const discardedAlias = dialog.querySelector("#acp-profile-alias");
    setValue(discardedAlias, profiles[0].displayName + " discarded");
    button(dialog, "취소").click();
    (await wait(() => button(dialog, "계속 편집"))).click();
    const discardContinueRetained = discardedAlias.value === profiles[0].displayName + " discarded";
    button(dialog, "취소").click();
    (await wait(() => button(dialog, "변경 버리기"))).click();
    await wait(() => !document.querySelector(".acp-profile-dialog"));
    const unchanged = (await invoke("list_provider_profiles")).find(profile => profile.providerProfileId === profiles[0].providerProfileId);
    const discardCommitted = unchanged?.revision === profiles[0].revision && unchanged.displayName === profiles[0].displayName;
    edit = await openProfileEdit(profiles[0]);
    dialog = edit.dialog;
    await progress("modal_save");
    const alias = dialog.querySelector("#acp-profile-alias"), home = dialog.querySelector("#acp-profile-credential-home");
    const initialName = alias.value, initialHome = home.value;
    const editInitialMatches = initialName === profiles[0].displayName && initialHome === profiles[0].credentialHome.displayPath;
    const editFocused = await wait(() => dialog.contains(document.activeElement));
    setValue(alias, initialName + " UI");
    setValue(home, config.unavailableHome);
    button(dialog, "프로필 저장").click();
    await wait(() => dialog.textContent.includes("프로필을 저장하지 못했습니다. 입력 내용은 유지했습니다."));
    const saveErrorRetained = alias.value === initialName + " UI" && home.value === config.unavailableHome;
    setValue(home, initialHome);
    button(dialog, "프로필 저장").click();
    await wait(() => !document.querySelector(".acp-profile-dialog"));
    const saved = (await invoke("list_provider_profiles")).find(profile => profile.providerProfileId === profiles[0].providerProfileId);
    if (!saved || saved.revision !== profiles[0].revision + 1 || saved.displayName !== initialName + " UI") throw Error("profile_save_readback");
    await report({ kind: "modal", profileId: saved.providerProfileId, previousRevision: profiles[0].revision, savedRevision: saved.revision,
      addInitialEmpty, addFocused, addFocusReturned, editInitialMatches, editFocused, editCancelled, discardContinueRetained, discardCommitted, saveErrorRetained,
      saveCommitted: true });
    await progress("ipc_integrity");
    if (window.__TAURI_INTERNALS__.invoke !== original) throw Error("invoke_replaced");
    await progress("complete");
    await report({ kind: "complete" });
  } catch (error) {
    const code = error instanceof Error && failureCodes.has(error.message) ? error.message : "unexpected_exception";
    await report({ kind: "failed", stage, code, checkpointCount, modelMismatch });
  }
})();
