(async () => {
  "use strict";
  const config = window.__MAGI_SAVED_UI_CONFIG;
  if (!config || typeof config.nonce !== "string" || typeof config.question !== "string") return;
  delete window.__MAGI_SAVED_UI_CONFIG;
  const deadline = performance.now() + 240000;
  let phase = "bridge_ready";
  let original = null;
  let finished = false;
  const report = async input => {
    if (finished) return;
    finished = true;
    if (!original) return;
    window.__TAURI_INTERNALS__.invoke = original;
    await original("saved_deliberation_ui_report", { input: { nonce: config.nonce, receipt: null, request: null, failure: null, ...input } });
  };
  const wait = async predicate => {
    while (performance.now() < deadline && !finished) {
      const result = predicate();
      if (result) return result;
      await new Promise(resolve => setTimeout(resolve, 100));
    }
    throw Error("bounded UI wait");
  };
  const button = labels => Array.from(document.querySelectorAll("button")).find(el => labels.some(label => el.textContent.trim().includes(label)));
  let captured = false;
  try {
    const bridge = await wait(() => typeof window.__TAURI_INTERNALS__?.invoke === "function" && window.__TAURI_INTERNALS__);
    original = bridge.invoke.bind(bridge);
    phase = "document_ready";
    await wait(() => document.readyState !== "loading" && document.body);
    bridge.invoke = async (command, args, options) => {
      const result = await original(command, args, options);
      if (command === "select_context_files" && result?.sources?.some(source => source.status === "captured")) captured = true;
      if (command === "start_deliberation") {
        const request = { ...args.input };
        delete request.admissionAuthority;
        await report({ receipt: result, request });
      }
      return result;
    };
    phase = "new_deliberation";
    (await wait(() => button(["새 심의 시작", "Start new deliberation"]))).click();
    phase = "question_writable";
    const textarea = await wait(() => {
      const field = document.getElementById("question-draft");
      return field instanceof HTMLTextAreaElement && !field.disabled && !field.readOnly ? field : null;
    });
    const setter = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value").set;
    setter.call(textarea, config.question);
    textarea.dispatchEvent(new Event("input", { bubbles: true }));
    phase = "sources_view";
    (await wait(() => button(["자료 보기", "View sources"]))).click();
    phase = "native_picker";
    (await wait(() => button(["파일 선택", "Select files"]))).click();
    // The owned native picker is completed separately using the approved policy file.
    phase = "capture_complete";
    await wait(() => captured);
    phase = "input_review";
    (await wait(() => button(["입력 확인", "Review input"]))).click();
    phase = "consent_ready";
    const consent = await wait(() => document.querySelector(".check-row input[type=checkbox]:not(:disabled)"));
    if (!consent.checked) consent.click();
    phase = "start_ready";
    (await wait(() => {
      const start = button(["이 동의로 심의 시작", "Start deliberation with this consent"]);
      return start && !start.disabled ? start : null;
    })).click();
    phase = "start_receipt";
    await wait(() => finished);
  } catch (_) {
    await report({ failure: `physical_ui_${phase}` });
  }
})();
