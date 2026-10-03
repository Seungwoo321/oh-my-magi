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
    await original("saved_deliberation_ui_report", { input: { nonce: config.nonce, ...input } });
  };
  const mark = async next => {
    phase = next;
    if (original && !finished) await original("saved_deliberation_ui_progress", { input: { nonce: config.nonce, phase: next } });
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
  try {
    const bridge = await wait(() => typeof window.__TAURI_INTERNALS__?.invoke === "function" && window.__TAURI_INTERNALS__);
    original = bridge.invoke.bind(bridge);
    await mark("bridge_ready");
    await mark("document_ready");
    await wait(() => document.readyState !== "loading" && document.body);
    await mark("new_deliberation");
    (await wait(() => document.querySelector("button.console-brand-link"))).click();
    (await wait(() => button(["새 심의 시작", "Start new deliberation"]))).click();
    await mark("question_writable");
    const textarea = await wait(() => {
      const field = document.getElementById("question-draft");
      return field instanceof HTMLTextAreaElement && !field.disabled && !field.readOnly ? field : null;
    });
    const setter = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value").set;
    setter.call(textarea, config.question);
    textarea.dispatchEvent(new Event("input", { bubbles: true }));
    await mark("sources_view");
    (await wait(() => button(["자료 보기", "View sources"]))).click();
    await mark("native_picker");
    (await wait(() => button(["파일 선택", "Select files"]))).click();
    // The owned native picker is completed separately using the approved policy file.
    await mark("capture_complete");
    await wait(() => Array.from(document.querySelectorAll(".source-row-mark")).some(mark => mark.textContent.trim() === "✓"));
    await mark("input_review");
    (await wait(() => button(["입력 확인", "Review input"]))).click();
    await mark("consent_ready");
    const consent = await wait(() => document.querySelector(".check-row input[type=checkbox]:not(:disabled)"));
    if (!consent.checked) consent.click();
    await mark("start_ready");
    (await wait(() => {
      const start = button(["이 동의로 심의 시작", "Start deliberation with this consent"]);
      return start && !start.disabled ? start : null;
    })).click();
  } catch (_) {
    await report({ failure: `physical_ui_${phase}` });
  }
})();
