(() => {
  "use strict";
  const K = window.MagiKit;
  const screens = window.MagiScreens;
  const params = new URLSearchParams(location.search);
  const defaultQuestion = "새 기능을 제한 공개할까요?";
  const content = document.getElementById("screen-content");
  const input = document.getElementById("question-input");
  const dialog = document.getElementById("console-dialog");
  const announcement = document.getElementById("announcement");
  const primary = document.getElementById("agenda-action");
  const shell = document.getElementById("console-shell");
  const screenSelect = document.getElementById("atlas-screen");
  const variantSelect = document.getElementById("atlas-variant");
  const formState = { theme: "command", motion: matchMedia("(prefers-reduced-motion: reduce)").matches ? "reduced" : "full", sound: false, density: "normal", textScale: "100", language: "ko", preferenceRevision: 0, outcome: "default", selectedSource: "02" };
  try { Object.assign(formState, JSON.parse(localStorage.getItem("magi-console-preferences") || "{}")); } catch (_) { /* Storage denial preserves the session defaults. */ }
  let screen = screens[params.get("screen")] ? params.get("screen") : "input";
  let variant = params.get("variant") || "default";
  let question = defaultQuestion;
  let frozenQuestion = defaultQuestion;
  const clone = value => JSON.parse(JSON.stringify(value));
  const draftRoutes = new Set(["input", "intake", "confirmation", "roles", "provider", "connections", "settings"]);
  const initialSetup = window.MagiSetup.state({ formState });
  const fixtureSnapshot = clone({ runId: "R-041", question: defaultQuestion, sources: window.MagiSetup.selectedSources(initialSetup), roles: initialSetup.roles, assignments: initialSetup.assignments, providerName: initialSetup.providerName, providerModel: initialSetup.providerModel });
  let runSnapshot = clone(fixtureSnapshot);
  const recordSnapshots = new Map([[fixtureSnapshot.runId, clone(fixtureSnapshot)]]);
  let runSequence = 0;
  function displayedInput() {
    const setup = window.MagiSetup.state({ formState });
    return draftRoutes.has(screen) ? { question, sources: window.MagiSetup.selectedSources(setup), roles: setup.roles, assignments: setup.assignments, providerName: setup.providerName, providerModel: setup.providerModel } : runSnapshot;
  }
  function evidenceUnavailable() { return !draftRoutes.has(screen) && runSnapshot.evidenceAvailability === "unavailable"; }
  function loseEvidenceBodies() {
    runSnapshot.evidenceAvailability = "unavailable";
    runSnapshot.sources = runSnapshot.sources.map(({ text, delivered, ...reference }) => reference);
    recordSnapshots.set(runSnapshot.runId, clone(runSnapshot));
  }
  function useRecordSnapshot(recordQuestion) {
    const record = window.MagiRecords?.getRecord(formState.selectedRun);
    const recordId = formState.selectedRun || fixtureSnapshot.runId;
    runSnapshot = clone(recordSnapshots.get(recordId) || { ...fixtureSnapshot, runId: recordId, question: recordQuestion, sources: fixtureSnapshot.sources.slice(0, record?.sources ?? fixtureSnapshot.sources.length) });
    recordSnapshots.set(recordId, clone(runSnapshot));
    frozenQuestion = recordQuestion;
  }
  let runStage = ["input", "connections", "roles", "settings", "intake", "confirmation", "provider"].includes(screen) ? "input" : screen;
  let composing = false;
  let dialogOpener;
  let dialogAfter;
  let previous = { screen: "input", variant: "default" };
  let audioContext;
  let pendingNavigation;
  let dialogCallbacks = new Map();
  const activeStages = new Set(["independent", "review", "proposal", "sealed", "paused", "interrupted", "cancelling"]);
  const stageNames = { input: "안건 입력", independent: "독립 검토", review: "교차 검토", proposal: "결의문 작성", sealed: "봉인 표결", verdict: "표결 완료", paused: "일시정지", interrupted: "중단 · 확인 필요", cancelling: "취소 확인 중", cancelled: "취소됨", failed: "실패", "save-error": "저장 실패" };
  const notify = text => { announcement.textContent = text; document.getElementById("toast-text").textContent = text; document.getElementById("toast").hidden = false; };
  const context = (event, target) => ({ variant, runStage, isRunActive: activeStages.has(runStage), question: displayedInput().question, sourceCount: displayedInput().sources.length, sources: clone(displayedInput().sources), roles: clone(displayedInput().roles), escape: K.esc, topology: K.topology, formState, go, notify, openDialog, closeDialog, event, target, rerender: () => render(false), applyPreferences, closePopover: () => actions["close-popover"](), quit: () => actions.quit() });

  function companionStage(mode) {
    return ["majority", "unanimous", "rejected", "unresolved", "record"].includes(mode) ? "verdict" : mode === "empty" ? "input" : mode === "sealed" ? "sealed" : mode === "paused" ? "paused" : ["offline", "deleted"].includes(mode) ? "interrupted" : "review";
  }
  function storageBlockVariant() { return formState.storageBlocked === "migrating" ? "migrating" : "migration-error"; }
  function go(next, nextVariant = "default", options = {}) {
    if (next === "paused" && nextVariant === "timeout") { next = "interrupted"; }
    if (next === "independent" && formState.storageBlocked) { next = "maintenance"; nextVariant = storageBlockVariant(); }
    if (!screens[next]) { notify(`화면을 찾을 수 없습니다: ${next}`); return; }
    if (screen === "roles" && (formState.rolesDirty || formState.setup?.roleDirty) && next !== "roles" && !options.force) {
      pendingNavigation = { screen: next, variant: nextVariant, options };
      actions.discard();
      return;
    }
    if (options.clearDraft) question = "";
    if (formState.recordQuestion) { useRecordSnapshot(formState.recordQuestion); delete formState.recordQuestion; }
    previous = { screen, variant };
    screen = next;
    variant = screens[next].variants.some(item => item.id === nextVariant) ? nextVariant : screens[next].variants[0].id;
    if (screen === "evidence" && variant === "unavailable") loseEvidenceBodies();
    else if (screen === "evidence" && runSnapshot.evidenceAvailability === "unavailable") {
      if (options.atlas) { runSnapshot = clone(fixtureSnapshot); frozenQuestion = runSnapshot.question; }
      else variant = "unavailable";
    }
    if (screen === "independent" && (options.start || previous.screen === "confirmation")) {
      const setup = window.MagiSetup.state({ formState });
      frozenQuestion = question.trim();
      runSnapshot = clone({ runId: `local-run-${++runSequence}`, question: frozenQuestion, sources: setup.confirmedSources || window.MagiSetup.selectedSources(setup), roles: setup.confirmedRoles || setup.roles, assignments: setup.assignments, providerName: setup.providerName, providerModel: setup.providerModel });
      recordSnapshots.set(runSnapshot.runId, clone(runSnapshot));
      formState.started = true;
    }
    if (stageNames[screen] && !(screen === "input" && ["queued", "followup"].includes(variant) && activeStages.has(runStage))) runStage = screen;
    if (screen === "verdict") formState.outcome = variant;
    if (screen === "input" && variant === "queued") runStage = "review";
    if (screen === "companion") runStage = companionStage(variant);
    if (screen === "settings" && options.atlas && ["clear", "reduced"].includes(variant)) applyPreferences(variant === "clear" ? { theme: "clear" } : { motion: "reduced" });
    if (screen === "companion" && ["majority", "unanimous", "rejected", "unresolved"].includes(variant)) { formState.outcome = variant === "majority" ? "default" : variant; runStage = "verdict"; }
    if (options.atlas && screen === "input") {
      if (["empty", "no-sources"].includes(variant)) window.MagiSetup.state({ formState }).selected = [];
      else if (variant === "default") window.MagiSetup.state({ formState }).selected = fixtureSnapshot.sources.map(source => source.id);
      if (variant === "empty") question = "";
      else if (variant === "followup") question = "참여자의 철회 절차를 포함하면 공개할 수 있을까요?";
      else if (!question) question = defaultQuestion;
    }
    if (dialog.open) dialog.close();
    shell.hidden = false;
    document.getElementById("atlas-reopen").hidden = true;
    document.getElementById("toast").hidden = true;
    render();
    if (options.focus !== false) content.focus();
  }

  function render(updateUrl = true) {
    const view = screens[screen];
    if (!view.variants.some(item => item.id === variant)) variant = view.variants[0].id;
    document.body.dataset.screen = screen;
    document.body.dataset.variant = variant;
    document.body.dataset.profile = view.profile === "companion" || screen === "companion" ? "companion" : "desktop";
    content.dataset.screen = screen;
    input.readOnly = screen !== "input";
    input.value = displayedInput().question;
    const isDraft = screen === "input";
    primary.dataset.action = isDraft ? "confirm" : activeStages.has(runStage) ? "run-details" : "new";
    primary.innerHTML = `${isDraft ? "입력 확인" : activeStages.has(runStage) ? "안건 확인" : "새 안건"} <span aria-hidden="true">↗</span>`;
    primary.disabled = isDraft && variant === "queued";
    primary.setAttribute("aria-label", primary.disabled ? "현재 심의가 진행 중입니다. 초안은 보존됩니다." : isDraft ? "입력 확인" : activeStages.has(runStage) ? "안건 확인" : "새 안건");
    primary.classList.toggle("button-primary", isDraft || !activeStages.has(runStage));
    document.getElementById("footer-stage").textContent = `${stageNames[runStage] || "저장 기록"}${!stageNames[screen] ? ` · ${view.title}` : ""}`;
    document.getElementById("footer-save").textContent = screen === "save-error" ? "저장 실패" : "로컬 보존";
    content.innerHTML = view.render(context());
    const shownInput = displayedInput();
    document.getElementById("source-count-label").textContent = shownInput.sources.length ? `자료 ${shownInput.sources.length}개` : "자료 없음";
    content.querySelectorAll("[data-source-count]").forEach(label => { label.textContent = `선택한 자료 ${shownInput.sources.length}개`; });
    const displayedRoles = shownInput.roles;
    if (displayedRoles) content.querySelectorAll(".core-control").forEach(button => {
      const index = K.cores.findIndex(core => core.id === button.dataset.core);
      const role = displayedRoles[index];
      if (!role) return;
      button.querySelector(".core-role").textContent = role.name;
      button.setAttribute("aria-label", `${K.cores[index].name} · ${role.name} · ${button.querySelector(".core-status-kr").textContent} · 상세 열기`);
    });
    screenSelect.value = screen;
    variantSelect.innerHTML = view.variants.map(item => `<option value="${K.esc(item.id)}">${K.esc(item.label)}</option>`).join("");
    variantSelect.value = variant;
    applyPreferences();
    resizeQuestion();
    if (updateUrl) {
      const url = new URL(location.href);
      url.searchParams.set("screen", screen); url.searchParams.set("variant", variant);
      url.searchParams.delete("overlay");
      history.replaceState({ screen, variant }, "", url);
    }
    announcement.textContent = `${view.title} · ${view.variants.find(item => item.id === variant).label}`;
  }

  function applyPreferences(patch = {}) {
    Object.assign(formState, patch);
    if (Object.keys(patch).length) formState.preferenceRevision += 1;
    document.body.dataset.theme = formState.theme;
    document.body.dataset.motion = formState.motion;
    document.body.dataset.density = formState.density;
    document.body.dataset.textScale = formState.textScale;
    document.documentElement.style.fontSize = `${Number(formState.textScale) || 100}%`;
    document.querySelector('[data-action="sound"]').textContent = formState.sound ? "소리 켬" : "소리 끔";
    document.querySelector('[data-action="sound"]').setAttribute("aria-pressed", String(formState.sound));
    document.querySelector('[data-action="motion"]').setAttribute("aria-pressed", String(formState.motion !== "full"));
    try { localStorage.setItem("magi-console-preferences", JSON.stringify({ theme: formState.theme, motion: formState.motion, sound: formState.sound, density: formState.density, textScale: formState.textScale, language: formState.language, budget: formState.budget })); } catch (_) { /* Preferences remain active in this window. */ }
    const english = formState.language === "en";
    document.documentElement.lang = english ? "en" : "ko";
    document.querySelector('[data-action="history"]').textContent = english ? "History" : "기록";
    document.querySelector('[data-action="settings"]').textContent = english ? "Settings" : "설정";
    document.querySelector('[data-action="motion"]').textContent = english ? "Reduce motion" : "동작 줄임";
    document.querySelector('[data-action="sound"]').textContent = english ? formState.sound ? "Sound on" : "Sound off" : formState.sound ? "소리 켬" : "소리 끔";
    syncReadingLayout();
  }

  function playTone() {
    if (!formState.sound) return;
    try {
      audioContext ||= new (window.AudioContext || window.webkitAudioContext)(); audioContext.resume();
      const oscillator = audioContext.createOscillator(); const gain = audioContext.createGain();
      oscillator.frequency.value = 440; gain.gain.setValueAtTime(.05, audioContext.currentTime);
      gain.gain.exponentialRampToValueAtTime(.001, audioContext.currentTime + .12);
      oscillator.connect(gain); gain.connect(audioContext.destination); oscillator.start(); oscillator.stop(audioContext.currentTime + .13);
    } catch (_) { notify("이 환경에서 알림 소리를 재생할 수 없습니다."); }
  }

  function openDialog(title, html, buttons = [{ label: "닫기", action: "close-dialog" }]) {
    if (typeof title === "object") {
      const config = title;
      return openDialog(config.title, config.body, [{ label: "돌아가기", action: "close-dialog" }, { label: config.confirmLabel || "확인", action: config.onConfirm, danger: config.danger, primary: !config.danger }]);
    }
    if (!dialog.open) dialogOpener = document.activeElement;
    dialogCallbacks = new Map();
    document.getElementById("dialog-title").textContent = title;
    document.getElementById("dialog-content").innerHTML = html;
    document.getElementById("dialog-actions").innerHTML = buttons.map((item, index) => { const name = typeof item.action === "function" ? `dialog-callback-${index}` : item.action; if (typeof item.action === "function") dialogCallbacks.set(name, item.action); return `<button type="button" class="button${item.primary ? " button-primary" : ""}${item.danger ? " button-danger" : ""}" data-action="${K.esc(name)}">${K.esc(item.label)}</button>`; }).join("");
    if (!dialog.open) dialog.showModal();
    document.querySelector(".dialog-close").focus();
  }
  function closeDialog(after) { dialogAfter = after; dialog.close(); }
  dialog.addEventListener("close", () => { if (dialogAfter) { const after = dialogAfter; dialogAfter = null; after(); } else if (dialogOpener?.isConnected) dialogOpener.focus(); });
  dialog.addEventListener("cancel", () => { dialogAfter = null; if (screen === "roles") pendingNavigation = null; });
  dialog.addEventListener("keydown", event => {
    if (event.key !== "Tab") return;
    const items = [...dialog.querySelectorAll('button:not([disabled]),input:not([disabled]),select:not([disabled]),textarea:not([disabled]),a[href],[tabindex="0"]')].filter(item => item.getClientRects().length);
    if (event.shiftKey && document.activeElement === items[0]) { event.preventDefault(); items.at(-1)?.focus(); }
    if (!event.shiftKey && document.activeElement === items.at(-1)) { event.preventDefault(); items[0]?.focus(); }
  });

  function syncReadingLayout() {
    document.body.dataset.textExpanded = "false";
    const largeBody = parseFloat(getComputedStyle(document.body).fontSize) >= 24;
    const title = document.querySelector(".console-title");
    const overflow = document.body.dataset.profile !== "companion" && title.scrollWidth > title.clientWidth + 1;
    document.body.dataset.textExpanded = String(largeBody || overflow);
  }
  function resizeQuestion() { input.style.height = "auto"; input.style.height = `${input.scrollHeight + 1}px`; syncReadingLayout(); }
  function confirmInput() {
    if (formState.storageBlocked) { go("maintenance", storageBlockVariant()); return; }
    if (!question.trim()) { input.setCustomValidity("심의할 질문을 입력해 주세요."); input.reportValidity(); input.focus(); return; }
    if (variant === "queued") { notify("현재 심의가 진행 중입니다. 초안은 보존되며 자동 제출되지 않습니다."); return; }
    go("confirmation");
  }
  function coreDetails(id) {
    const core = K.cores.find(item => item.id === id); if (!core) return;
    const role = displayedInput().roles[K.cores.indexOf(core)];
    const revealed = runStage === "verdict" && !["sealed", "proposal", "independent", "review"].includes(screen);
    const vote = (K.outcomes[formState.outcome] || K.outcomes.default).votes[K.cores.indexOf(core)];
    let html = `<p class="confirmation-question">${K.esc(role?.challenge || core.question)}</p>`;
    if (screen === "sealed" || (screen === "companion" && variant === "sealed")) html += "<h3>표 제출 상태</h3><p>표의 방향과 이유는 세 표가 검증된 뒤 함께 공개됩니다.</p><p>다른 코어의 최종 표는 이 코어에 전달하지 않습니다.</p>";
    else if (revealed) html += `<h3>공개된 표와 판단 이유</h3><p class="core-detail-vote ${vote}">${{ support: "찬성", oppose: "반대", abstain: "기권" }[vote]}</p><p>${vote === "abstain" ? "판단에 필요한 참여자 자료가 부족합니다." : vote === "support" && core.id === "casper" ? "고정된 문안의 제한 공개와 명시된 조건을 지지합니다. 지원 범위 변경 시 다시 검토해야 합니다." : vote === "oppose" && core.id === "balthasar" ? "운영 담당자의 지원 부담을 감당할 근거가 부족하여 이 문안을 지지하지 않습니다." : core.summary}</p>`;
    else if (runStage === "input") html += "<h3>검토 기준</h3><p>같은 질문과 선택한 자료를 이 관점으로 읽습니다. 권한·표의 수·다른 코어의 역할은 바꾸지 않습니다.</p>";
    else html += `<h3>공개 판단 근거</h3><p>${core.summary}</p><p>자료: release-plan.md · 참조 위치 확인됨</p><p class="muted">이 판단은 최종 표가 아닙니다. 내부 사고 과정은 수집하지 않습니다.</p>`;
    if (evidenceUnavailable()) html += '<h3>근거 본문 열람 불가</h3><p>위 내용은 보존된 판단 이유입니다. 근거 본문이 없어 원문 대조와 인용 위치 재검증은 할 수 없습니다.</p>';
    openDialog(`${core.name} · ${role?.name || core.role}`, html, [{ label: "닫기", action: "close-dialog" }, { label: "근거 원문", action: "original" }]);
  }
  function followup(value = "참여자의 철회 절차를 포함하면 공개할 수 있을까요?") {
    question = value; formState.parentQuestion = frozenQuestion; go("input", activeStages.has(runStage) ? "queued" : "followup"); input.focus();
  }
  function hideShell(text) {
    shell.hidden = true; document.getElementById("atlas-reopen").hidden = false;
    document.getElementById("atlas-status").textContent = text;
  }
  function finishPendingNavigation() {
    const next = pendingNavigation || { screen: "input", variant: "default" };
    pendingNavigation = null;
    go(next.screen, next.variant, { ...next.options, force: true });
  }
  const actions = {
    confirm: confirmInput,
    "start-run": () => go("independent", "default", { start: true }),
    "run-details": () => openDialog("고정된 심의 입력", `<p class="confirmation-question">${K.esc(runSnapshot.question)}</p><h3>자료 ${runSnapshot.sources.length}개 · ${evidenceUnavailable() ? "참조만 보존 · 본문 열람 불가" : "수집본 고정"}</h3><p>${runSnapshot.sources.length ? runSnapshot.sources.map(source => K.esc(source.name)).join(" · ") : "추가 자료 없음"}</p><h3>관점과 모델</h3><ul>${runSnapshot.roles.map((role, i) => `<li>${K.cores[i].name} · ${K.esc(role.name)}</li>`).join("")}</ul><p>${K.esc(runSnapshot.providerName)} · ${K.esc(runSnapshot.providerModel)}</p><p>현재 설정을 바꾸어도 이 실행의 입력은 바뀌지 않습니다.</p>`),
    sources: () => draftRoutes.has(screen) ? go("intake") : actions["run-details"](), settings: () => go("settings"), connections: () => go("connections"), history: () => go("history"),
    original: () => {
      if (evidenceUnavailable()) {
        openDialog("근거 본문 열람 불가", `<h3>남아 있는 참조</h3><p>${runSnapshot.sources.map(source => `${K.esc(source.name)} · ${K.esc(source.hash || "식별자 없음")}`).join("<br>")}</p><p>보존된 근거 본문이 없어 원문·전달본·인용 위치를 열거나 재검증할 수 없습니다.</p><p>파일 재선택은 새 심의의 입력이며 기존 기록의 근거를 복구한 것으로 처리하지 않습니다.</p>`, [{ label: "닫기", action: "close-dialog" }, { label: "새 자료 선택", action: "reselect-evidence", primary: true }]);
        return;
      }
      const source = displayedInput().sources[0];
      if (!source) { openDialog("연결된 원문 없음", "<p>이 입력에는 추가 자료가 없습니다. 모델 지식·추론·가정과 사용자 입력만 구분해 검토합니다.</p>"); return; }
      openDialog(`${source.name} · 보존된 수집본`, `<p>근거 [${K.esc(formState.selectedSource)}] · ${K.esc(source.kind || "텍스트")}</p><pre class="source-original">${K.esc(source.text || source.delivered || "수집본 내용 없음")}</pre><h3>전달 범위</h3><p>${K.esc(source.locator || "선택한 범위")} · 입력 확인에서 승인한 수집본</p><p>현재 파일 변경은 이 보존된 수집본을 덮어쓰지 않습니다.</p>`);
    },
    issue: () => evidenceUnavailable() ? openDialog("보존된 쟁점 · 원문 대조 불가", "<p>참여자의 철회 방법이 명시되어 있는가?</p><p>당시 제기된 질문은 남아 있지만 근거 본문을 사용할 수 없어 자료와 다시 대조할 수 없습니다.</p>") : openDialog("교차 검토 쟁점", "<p class=\"confirmation-question\">참여자의 철회 방법이 명시되어 있는가?</p><h3>CASPER·3 → MELCHIOR·1</h3><p>범위를 제한하는 사실만으로 참여자의 선택권을 보장할 수 있는지 추가 근거를 요청합니다.</p><h3>참조 자료</h3><p>release-plan.md에는 공개 인원 제한이 있고 철회 절차는 없습니다.</p><h3>남은 질문</h3><p>철회 요청의 접수 경로와 반영 기한은 무엇인가?</p>"),
    "proposal-reader": () => openDialog("고정된 표결 문안", "<p class=\"confirmation-question\">지원 범위를 명시하고 소규모 공개한다.</p><h3>성립 조건</h3><p>대상 인원·지원 채널·운영 담당자를 명시한다.</p><h3>남은 이견</h3><p>철회 절차를 먼저 정해야 한다는 의견을 보존한다.</p><p>세 코어는 이 동일한 본문과 조건을 평가합니다.</p>"),
    "edit-proposal": () => openDialog("문안을 수정해 새 심의를 준비할까요?", "<p>현재 심의의 고정 입력을 덮어쓰지 않습니다. 진행 중인 심의를 보존한 채 후속 초안을 준비합니다.</p>", [{ label: "계속 읽기", action: "close-dialog" }, { label: "후속 초안", action: "dialog-followup", primary: true }]),
    "unavailable-opinion": () => coreDetails("casper"),
    "reselect-evidence": () => {
      const prepare = () => { question = runSnapshot.question; formState.parentQuestion = runSnapshot.question; window.MagiSetup.state({ formState }).selected = []; go("intake", "scanning"); };
      if (dialog.open) closeDialog(prepare); else prepare();
    },
    "dialog-followup": () => closeDialog(() => followup()), followup: () => followup(),
    cancel: () => openDialog("이 심의를 취소할까요?", `<p class="confirmation-question">${K.esc(frozenQuestion)}</p><p>새 호출을 차단하고 진행 중인 요청의 정지를 확인합니다. 이미 검증된 의견은 기록에 보존합니다.</p>`, [{ label: "계속 심의", action: "close-dialog" }, { label: "심의 취소", action: "confirm-cancel", danger: true }]),
    "confirm-cancel": () => closeDialog(() => go("cancelling")),
    "finish-cancel": () => { if (variant === "remote-unknown") notify("원격 완료 여부가 아직 확인되지 않았습니다. 취소 확인 상태를 유지합니다."); else { go("cancelled"); if (formState.quitPending) { formState.quitPending = false; hideShell("앱 종료 상태 · 소유 호출 정지 확인됨"); } } },
    "check-recovery": () => openDialog("정지 상태 확인됨", "<p>이전 소유 호출의 정지를 확인했습니다. 입력과 모델 경로가 일치하므로 저장 지점에서 재개할 수 있습니다.</p><p>재개는 사용자의 명시적인 요청으로 실행합니다.</p>", [{ label: "나중에", action: "close-dialog" }, { label: "심의 재개", action: "resume", primary: true }]),
    resume: () => closeDialog(() => go("independent")),
    "retry-save": () => { if (variant === "corrupt") { notify("객체 손상은 쓰기 재시도로 복구되지 않습니다. 검증된 백업 복원을 사용해 주세요."); return; } notify("저장 공간이 아직 부족합니다. 새 호출 차단을 유지합니다."); },
    diagnostics: () => go("maintenance", "diagnostics"),
    quit: () => openDialog("MAGI CONSOLE을 종료할까요?", activeStages.has(runStage) ? "<p>진행 중인 심의의 취소 의도를 저장하고 소유 호출의 정지를 확인한 뒤 종료합니다.</p><p>창만 닫으면 실행을 유지할 수 있습니다.</p>" : "<p>보존된 기록은 다음 실행에서 다시 열 수 있습니다.</p>", [{ label: "돌아가기", action: "close-dialog" }, { label: "앱 종료", action: "confirm-quit", danger: true }]),
    "confirm-quit": () => closeDialog(() => { if (activeStages.has(runStage)) { go("cancelling"); notify("종료를 위한 취소 확인 중입니다. 정지 확인 후 종료할 수 있습니다."); formState.quitPending = true; } else hideShell("앱 종료 상태. 검토 도구에서 콘솔을 다시 열 수 있습니다."); }),
    "close-popover": () => hideShell("팝오버 닫힘 · 실행과 콘솔 문맥은 유지됩니다."),
    "open-console": () => go(runStage === "verdict" ? "verdict" : stageNames[runStage] ? runStage : "input", runStage === "verdict" ? formState.outcome : "default"),
    "close-dialog": () => { if (screen === "roles") pendingNavigation = null; closeDialog(); }, back: () => go(previous.screen, previous.variant), close: () => go("input"),
    new: () => {
      if (screen === "input" && question.trim()) openDialog("새 안건을 작성할까요?", "<p>현재 초안을 비우고 새 안건을 작성합니다. 완료된 기록과 진행 중인 심의는 유지됩니다.</p>", [{ label: "계속 작성", action: "close-dialog" }, { label: "새 안건", action: "confirm-new", primary: true }]);
      else { go("input", "empty", { clearDraft: true }); input.focus(); }
    },
    "confirm-new": () => closeDialog(() => { go("input", "empty", { clearDraft: true }); input.focus(); }),
    "dismiss-toast": () => { document.getElementById("toast").hidden = true; },
    sound: () => { applyPreferences({ sound: !formState.sound }); playTone(); notify(formState.sound ? "알림 소리를 켰습니다." : "알림 소리를 껐습니다."); },
    motion: () => { applyPreferences({ motion: formState.motion === "full" ? "reduced" : "full" }); notify(formState.motion === "full" ? "동작 효과를 표시합니다." : "동작을 줄입니다."); },
    "source-01": () => { formState.selectedSource = "01"; render(); notify("근거 01 운영 범위를 선택했습니다."); },
    "source-02": () => { formState.selectedSource = "02"; render(); notify("근거 02 철회 절차를 선택했습니다."); },
    command: () => openDialog("명령 찾기", `<label class="field">명령 검색<input id="command-search" placeholder="기록, 역할, 연결…"/></label><div class="command-list">${[["새 안건", "input"], ["자료 접수", "intake"], ["모델 연결", "connections"], ["세 관점 편집", "roles"], ["심의 기록", "history"], ["설정", "settings"], ["데이터 관리", "data"], ["앱 정보", "maintenance", "about"]].map(([label, route, mode]) => K.button(label, route, mode || "default")).join("")}</div>`),
    discard: () => openDialog("저장하지 않은 관점 변경", "<p>관점 편집 내용이 아직 저장되지 않았습니다. 저장 후 이동하거나 변경을 버릴 수 있습니다. 계속 편집하면 현재 초안을 유지합니다.</p>", [{ label: "계속 편집", action: "close-dialog" }, { label: "변경 버리기", action: "confirm-discard", danger: true }, { label: "저장하고 이동", action: "save-and-leave", primary: true }]),
    "confirm-discard": () => closeDialog(() => {
      formState.rolesDirty = false;
      const setup = window.MagiSetup.state({ formState });
      setup.roleDirty = false; setup.roleDraft = clone(setup.roles);
      finishPendingNavigation();
    }),
    "save-and-leave": () => closeDialog(() => {
      if (!window.MagiSetup.saveRoles(context())) {
        pendingNavigation = null;
        go("roles", "invalid");
        document.querySelector("#setup-roles-form :invalid")?.focus();
        return;
      }
      formState.rolesDirty = false;
      finishPendingNavigation();
      notify("세 관점을 저장했습니다. 다음 심의부터 적용됩니다.");
    }),
    delete: () => openDialog("선택한 기록을 삭제할까요?", `<p class="confirmation-question">${K.esc(frozenQuestion)}</p><p>이 기록과 연결된 로컬 자료의 삭제 범위를 확인합니다. 외부 제공자가 이미 받은 자료는 별도로 관리됩니다.</p>`, [{ label: "돌아가기", action: "close-dialog" }, { label: "기록 삭제", action: "confirm-delete", danger: true }]),
    "confirm-delete": () => closeDialog(() => { formState.recordDeleted = true; go("history", "empty"); notify("선택한 기록을 삭제했습니다."); }),
    "clear-all": () => openDialog("모든 로컬 기록을 삭제할까요?", '<p>연결된 심의 기록과 수집본을 삭제합니다. 계속하려면 <strong>삭제</strong>를 입력해 주세요.</p><label class="field">확인 문구<input id="clear-confirm" autocomplete="off"/></label>', [{ label: "돌아가기", action: "close-dialog" }, { label: "전체 삭제", action: "confirm-clear-all", danger: true }]),
    "confirm-clear-all": () => { const field = document.getElementById("clear-confirm"); if (field?.value !== "삭제") { field.setCustomValidity("삭제를 정확히 입력해 주세요."); field.reportValidity(); field.focus(); return; } closeDialog(() => { formState.recordDeleted = true; go("history", "empty"); notify("로컬 기록과 수집본을 삭제했습니다."); }); },
    revoke: () => openDialog("이 자료의 접근 권한을 철회할까요?", "<p>앞으로 원본을 다시 읽는 권한을 철회합니다. 이미 보존된 수집본과 외부 전송본은 별도입니다.</p>", [{ label: "돌아가기", action: "close-dialog" }, { label: "접근 철회", action: "confirm-revoke", danger: true }]),
    "confirm-revoke": () => closeDialog(() => go("data", "revoked")),
    overwrite: () => openDialog("같은 이름의 내보내기 파일이 있습니다", "<p>magi-decision.png를 덮어쓰거나 다른 이름으로 저장할 수 있습니다.</p>", [{ label: "다른 이름", action: "export-rename" }, { label: "덮어쓰기", action: "confirm-overwrite", danger: true }]),
    "confirm-overwrite": () => closeDialog(() => go("share", "saved")), "export-rename": () => closeDialog(() => go("share")),
  };
  function dispatch(name, event, target) {
    if (dialogCallbacks.has(name)) { const callback = dialogCallbacks.get(name); closeDialog(() => callback(context(event, target))); return; }
    const custom = screens[screen].actions?.[name];
    if (custom) custom(context(event, target));
    else if (actions[name]) actions[name](event, target);
    else notify(`이 행동의 연결이 누락되었습니다: ${name}`);
  }
  const overlays = { core: () => coreDetails("casper"), source: actions.original, issue: actions.issue, cancel: actions.cancel, quit: actions.quit, command: actions.command, discard: actions.discard, delete: actions.delete, revoke: actions.revoke, overwrite: actions.overwrite };
  const groups = [...new Set(Object.values(screens).map(view => view.group))];
  screenSelect.innerHTML = groups.map(group => `<optgroup label="${K.esc(group)}">${Object.entries(screens).filter(([, view]) => view.group === group).map(([id, view]) => `<option value="${id}">${K.esc(view.title)}</option>`).join("")}</optgroup>`).join("");
  screenSelect.addEventListener("change", () => go(screenSelect.value, "default", { atlas: true, force: true }));
  variantSelect.addEventListener("change", () => go(screen, variantSelect.value, { atlas: true, force: true }));
  document.getElementById("atlas-overlay").addEventListener("change", event => { overlays[event.target.value]?.(); event.target.value = ""; });
  document.getElementById("atlas-reopen").addEventListener("click", () => { shell.hidden = false; document.getElementById("atlas-reopen").hidden = true; document.getElementById("atlas-status").textContent = ""; content.focus(); });
  document.addEventListener("click", event => {
    if (screen === "companion" && !dialog.open && !event.target.closest("#console-shell,.review-toolbar,#console-dialog,#toast")) actions["close-popover"]();
    const target = event.target.closest("button,a"); if (!target || target.disabled) return;
    if (target.dataset.go) { event.preventDefault(); go(target.dataset.go, target.dataset.variant || "default"); return; }
    if (target.dataset.core) { coreDetails(target.dataset.core); return; }
    if (target.dataset.action) dispatch(target.dataset.action, event, target);
  });
  function captureField(event) {
    const field = event.target;
    if (!(field instanceof HTMLInputElement || field instanceof HTMLSelectElement || field instanceof HTMLTextAreaElement)) return;
    if (field.type === "password" || field.hasAttribute("data-sensitive")) return;
    const name = field.dataset.field || field.name; if (name) formState[name] = field.type === "checkbox" ? field.checked : field.value;
  }
  document.addEventListener("input", event => {
    captureField(event);
    if (event.target === input) { question = input.value; input.setCustomValidity(""); resizeQuestion(); }
    if (event.target.id === "command-search") document.querySelectorAll(".command-list button").forEach(button => { button.hidden = !button.textContent.includes(event.target.value); });
    if (event.target.id === "clear-confirm") event.target.setCustomValidity("");
  });
  document.addEventListener("change", captureField);
  input.addEventListener("compositionstart", () => { composing = true; }); input.addEventListener("compositionend", () => { composing = false; });
  document.addEventListener("submit", event => {
    if (!(event.target instanceof HTMLFormElement)) return;
    event.preventDefault(); if (!event.target.reportValidity()) return;
    if (event.target.id === "followup-form") { followup(document.getElementById("followup-input").value); return; }
    event.target.querySelectorAll("input,textarea,select").forEach(field => captureField({ target: field }));
    if (event.target.dataset.action) { dispatch(event.target.dataset.action, event, event.target); return; }
    if (event.target.dataset.submitScreen) go(event.target.dataset.submitScreen, event.target.dataset.submitVariant || "default", { start: event.target.dataset.submitScreen === "independent" });
  });
  document.addEventListener("keydown", event => {
    if (composing || event.isComposing || event.keyCode === 229) return;
    const command = event.metaKey || event.ctrlKey;
    if (command && event.key === "Enter" && screen === "input" && !dialog.open) { event.preventDefault(); confirmInput(); }
    if (command && event.key.toLowerCase() === "k") { event.preventDefault(); actions.command(); }
    if (command && event.key.toLowerCase() === "n" && !dialog.open) { event.preventDefault(); actions.new(); }
    if (command && event.key.toLowerCase() === "q") { event.preventDefault(); actions.quit(); }
    if (command && ["1", "2", "3"].includes(event.key) && !dialog.open) { event.preventDefault(); coreDetails(K.cores[Number(event.key) - 1].id); }
    if (event.key === "Escape" && !dialog.open && screen === "companion") actions["close-popover"]();
  });
  window.addEventListener("popstate", () => { const query = new URLSearchParams(location.search); go(query.get("screen") || "input", query.get("variant") || "default", { force: true }); });
  window.addEventListener("resize", resizeQuestion); document.fonts.ready.then(resizeQuestion);
  document.body.dataset.capture = String(params.get("capture") === "1");
  if (screen === "input" && ["empty", "no-sources"].includes(variant)) initialSetup.selected = [];
  if (screen === "input" && variant === "empty") question = "";
  if (screen === "input" && variant === "followup") question = "참여자의 철회 절차를 포함하면 공개할 수 있을까요?";
  if (screen === "input" && variant === "queued") runStage = "review";
  if (screen === "evidence") runStage = "verdict";
  if (screen === "verdict") formState.outcome = variant;
  if (screen === "companion") runStage = companionStage(variant);
  if (screen === "settings" && ["clear", "reduced"].includes(variant)) applyPreferences(variant === "clear" ? { theme: "clear" } : { motion: "reduced" });
  window.MagiReview = { go, context, dispatch, openDialog, closeDialog, registry: screens,
    hasAction: name => !!(screens[screen].actions?.[name] || actions[name] || dialogCallbacks.has(name)),
    getState: () => JSON.parse(JSON.stringify({ screen, variant, runStage, question, frozenQuestion, runSnapshot, displayedInput: displayedInput(), formState })) };
  if (screen === "paused" && params.get("variant") === "timeout") { screen = "interrupted"; variant = "timeout"; runStage = "interrupted"; }
  if (screen === "evidence" && variant === "unavailable") loseEvidenceBodies();
  render(false);
  if (params.get("screen") && !screens[params.get("screen")]) document.getElementById("atlas-status").textContent = "알 수 없는 화면 주소여서 안건 입력을 표시합니다.";
  if (params.get("overlay")) overlays[params.get("overlay")]?.();
})();
