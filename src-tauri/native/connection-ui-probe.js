(async () => {
  const config = window.__magiConnectionProbe;
  const original = window.__TAURI_INTERNALS__.invoke;
  const invoke = (command, args) => original.call(window.__TAURI_INTERNALS__, command, args);
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
  const setValue = (element, value) => {
    Object.getOwnPropertyDescriptor(element.tagName === "TEXTAREA" ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype, "value")
      .set.call(element, value);
    element.dispatchEvent(new Event("input", { bubbles: true }));
  };
  const report = payload => invoke("connections_ui_probe_report", {
    input: { schemaVersion: 1, nonce: config.nonce, pid: config.pid, checkpoint: payload }
  });
  const digest = async text => Array.from(new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(text))))
    .map(byte => byte.toString(16).padStart(2, "0")).join("");
  const connections = async () => {
    (await wait(() => button(document, "모델 연결"))).click();
    return wait(() => document.querySelector(".acp-profile-list"));
  };
  try {
    const profiles = (await invoke("list_provider_profiles")).filter(profile => profile.authenticationMethod === "local_subscription");
    if (!profiles.length || profiles.length > 100) throw Error("profile_count");
    const list = await connections();
    const elements = await wait(() => {
      const rows = Array.from(list.querySelectorAll(".acp-profile-item"));
      return rows.length === profiles.length ? rows : null;
    });
    const rows = [];
    for (let index = 0; index < profiles.length; index += 1) {
      const profile = profiles[index], element = elements[index];
      if (element.querySelector(".acp-profile-copy strong").textContent !== profile.displayName) throw Error("profile_label");
      const catalog = await invoke("load_provider_catalog", { profileId: profile.providerProfileId, expectedRevision: profile.revision });
      const binding = catalog.modelSelection?.binding ?? null;
      const paragraphs = Array.from(element.querySelectorAll(":scope > p.field-help"));
      const modelParagraph = paragraphs.find(item => item.textContent.startsWith("저장 모델"));
      const codes = Array.from(modelParagraph.querySelectorAll("code")).map(item => item.textContent);
      const modelId = binding?.modelId ?? null, modeId = binding?.modeId ?? null;
      if (codes[0] !== (modelId ?? "선택하지 않음") || (codes[1] ?? null) !== modeId) throw Error("saved_model");
      const auth = paragraphs.find(item => item.textContent.startsWith("인증 상태"));
      if (!auth.textContent.includes("아직 확인하지 않음") || element.querySelector("time")) throw Error("authentication_time");
      rows.push({ profileId: profile.providerProfileId, revision: profile.revision, modelId, modeId, checkedAt: null, labelMatched: true });
    }
    await report({ kind: "rows", rows });

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

    const modalList = await connections();
    const addButton = await wait(() => button(document, "새 ACP 프로필"));
    addButton.click();
    let dialog = await wait(() => document.querySelector(".acp-profile-dialog[open]"));
    const addInitialEmpty = dialog.querySelector("#acp-profile-alias").value === ""
      && dialog.querySelector("#acp-profile-credential-home").value === "";
    const addFocused = await wait(() => dialog.contains(document.activeElement));
    dialog.dispatchEvent(new Event("cancel", { cancelable: true }));
    await wait(() => !document.querySelector(".acp-profile-dialog[open]"));
    const addFocusReturned = document.activeElement === addButton;

    const editButton = button(modalList.querySelector(".acp-profile-item"), "편집");
    editButton.click();
    dialog = await wait(() => document.querySelector(".acp-profile-dialog[open]"));
    button(dialog, "취소").click();
    await wait(() => !document.querySelector(".acp-profile-dialog[open]"));
    const editCancelled = await wait(() => document.activeElement === editButton);
    editButton.click();
    dialog = await wait(() => document.querySelector(".acp-profile-dialog[open]"));
    const discardedAlias = dialog.querySelector("#acp-profile-alias");
    setValue(discardedAlias, profiles[0].displayName + " discarded");
    button(dialog, "취소").click();
    (await wait(() => button(dialog, "계속 편집"))).click();
    const discardContinueRetained = discardedAlias.value === profiles[0].displayName + " discarded";
    button(dialog, "취소").click();
    (await wait(() => button(dialog, "변경 버리기"))).click();
    await wait(() => !document.querySelector(".acp-profile-dialog[open]"));
    const unchanged = (await invoke("list_provider_profiles")).find(profile => profile.providerProfileId === profiles[0].providerProfileId);
    const discardCommitted = unchanged?.revision === profiles[0].revision && unchanged.displayName === profiles[0].displayName;
    editButton.click();
    dialog = await wait(() => document.querySelector(".acp-profile-dialog[open]"));
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
    await wait(() => !document.querySelector(".acp-profile-dialog[open]"));
    const saved = (await invoke("list_provider_profiles")).find(profile => profile.providerProfileId === profiles[0].providerProfileId);
    if (!saved || saved.revision !== profiles[0].revision + 1 || saved.displayName !== initialName + " UI") throw Error("profile_save_readback");
    await report({ kind: "modal", profileId: saved.providerProfileId, previousRevision: profiles[0].revision, savedRevision: saved.revision,
      addInitialEmpty, addFocused, addFocusReturned, editInitialMatches, editFocused, editCancelled, discardContinueRetained, discardCommitted, saveErrorRetained,
      saveCommitted: true });
    if (window.__TAURI_INTERNALS__.invoke !== original) throw Error("invoke_replaced");
    await report({ kind: "complete" });
  } catch {
    await report({ kind: "failed" });
  }
})();
