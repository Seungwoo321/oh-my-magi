(() => {
  "use strict";

  const S = window.MagiScreens;
  const K = () => window.MagiKit;
  const question = "새 기능을 제한 공개할까요?";
  const proposal = "지원 범위를 명시하고 소규모 공개한다.";
  const objection = "참여자의 철회 방법을 먼저 명시해야 합니다.";
  const rows = [
    { id: "R-041", title: question, date: "오늘 14:32", status: "majority", label: "다수 지지", parent: "R-037", sources: 3 },
    { id: "R-042", title: "참여자의 철회 절차를 포함하면 공개할 수 있을까요?", date: "오늘 14:40", status: "running", label: "교차 검토", parent: "R-041", sources: 3 },
    { id: "R-037", title: "지원 범위가 정해지기 전에 공개해도 될까요?", date: "어제 18:05", status: "rejected", label: "제안 부결", parent: null, sources: 2 },
    { id: "R-035", title: "이번 공개에서 무엇을 먼저 결정해야 하나요?", date: "어제 17:18", status: "unresolved", label: "결론 미도출", parent: null, sources: 1 },
  ];
  const memory = {
    search: "", filter: "all", selected: "R-041", deleted: new Set(), revoked: false,
    disclosureRevoked: false, restored: false, backupSaved: false, shareTitle: "제한 공개에 대한 세 관점의 결의",
    replayStep: 0, replaySpeed: 1, playing: false, replayMode: "default", timer: null,
    archiveRun: "R-041", shareRedacted: false, companionTarget: "R-042", shareFormat: "replay", includeEvidence: false, includeSources: true, includeSettings: true,
  };
  const stages = ["입력 확인", "독립 검토", "교차 검토", "결의문 작성", "봉인 표결", "표결 공개"];
  const outcomes = {
    majority: { name: "다수 지지", mark: "可決", votes: ["support", "oppose", "support"], why: objection },
    unanimous: { name: "만장일치 지지", mark: "可決", votes: ["support", "support", "support"], why: "세 관점이 동일 문안을 지지합니다. 지지는 사실 정확도의 보증이 아닙니다." },
    rejected: { name: "제안 부결", mark: "否決", votes: ["oppose", "oppose", "support"], why: "지원 책임과 철회 절차를 명시한 대안을 다시 검토해야 합니다." },
    unresolved: { name: "결론 미도출", mark: "保留", votes: ["support", "oppose", "abstain"], why: "기권 사유: 지원 인력과 공개 범위를 판단할 자료가 부족합니다." },
  };
  const esc = value => K().esc(value);
  const nav = (label, screen, variant = "default", tone = "") => K().button(label, screen, variant, tone);
  const action = (label, name, tone = "") => K().action(label, name, tone);
  const panel = (title, html, kicker = "") => K().panel(title, html, kicker);
  const notice = (title, text, tone = "info") => K().notice(title, text, tone);
  const page = value => K().page(value);
  const field = (label, id, value = "", type = "text") => K().field(label, id, value, type);
  const select = (label, id, options, selected) => K().select(label, id, options, selected);
  const variants = values => values.map(([id, label]) => ({ id, label }));
  const buttonRow = html => `<div class="page-actions records-actions">${html}</div>`;
  const badge = (label, tone = "") => `<span class="status-pill records-status ${esc(tone)}">${esc(label)}</span>`;
  const readInput = (id, fallback = "") => document.getElementById(id)?.value ?? fallback;
  const current = () => rows.find(row => row.id === memory.selected) || rows[0];
  const archived = () => rows.find(row => row.id === memory.archiveRun) || rows[0];
  const archivedOutcome = () => outcomes[archived().status] || outcomes.majority;
  const go = (ctx, screen, variant = "default") => ctx.go(screen, variant);
  function confirm(ctx, title, body, label, onConfirm, danger = false) {
    ctx.openDialog(title, body, [{ label: "돌아가기", action: "close-dialog" }, { label, action: onConfirm, primary: !danger, danger }]);
  }
  function count(votes) {
    return ["support", "oppose", "abstain"].map(vote => votes.filter(value => value === vote).length);
  }
  function voteLine(votes) {
    const [yes, no, abstain] = count(votes);
    return `<p class="records-votes">찬성 <b>${yes}</b> <span>/</span> 반대 <b>${no}</b> <span>/</span> 기권 <b>${abstain}</b></p>`;
  }
  function rowsTable(data) {
    return `<div class="records-ledger" role="list" aria-label="심의 기록">${data.map(row => `<button type="button" class="records-ledger-row${memory.selected === row.id ? " is-selected" : ""}" data-action="records-select-${row.id}" aria-pressed="${memory.selected === row.id}"><span class="records-ledger-id">${esc(row.id)}<small>${esc(row.date)}</small></span><span class="records-ledger-title">${esc(row.title)}<small>${row.parent ? `이전 심의 ${esc(row.parent)}에서 이어짐` : "새 대화"} · 자료 ${row.sources}개</small></span><span class="records-ledger-state ${row.status === "rejected" ? "records-negative" : ""}">${esc(row.label)} <span aria-hidden="true">↗</span></span></button>`).join("")}</div>`;
  }
  function history(ctx) {
    const v = ctx.variant;
    let data = rows.filter(row => !memory.deleted.has(row.id));
    if (v === "empty") data = [];
    else data = data.filter(row => row.title.includes(memory.search) || row.id.toLowerCase().includes(memory.search.toLowerCase())).filter(row => memory.filter === "all" || row.status === memory.filter);
    if (v === "no-results") data = [];
    const filters = `<div class="records-filter"><div>${field("기록 검색", "records-search", memory.search)}</div><div>${select("결과", "records-filter", [{ value: "all", label: "모든 결과" }, { value: "running", label: "진행 중" }, { value: "majority", label: "다수 지지" }, { value: "rejected", label: "제안 부결" }, { value: "unresolved", label: "결론 미도출" }], memory.filter)}</div>${action("검색 적용", "records-search", "primary")}</div>`;
    let body = filters + `<p class="records-tally" role="status">${data.length}개 기록 <span>LOCAL ARCHIVE / 로컬 보관</span></p>` + rowsTable(data);
    if (v === "loading") body = notice("기록을 읽고 있습니다", "저장된 목록을 확인합니다. 원문을 다시 읽거나 모델을 호출하지 않습니다.") + buttonRow(action("목록 확인", "records-loaded"));
    else if (v === "error") body = notice("기록 목록을 읽지 못했습니다", "저장소를 변경하지 않았습니다. 다시 확인하거나 진단 정보를 열 수 있습니다.", "error") + buttonRow(action("다시 확인", "records-loaded") + nav("진단", "maintenance", "diagnostics"));
    else if (v === "empty") body = notice("아직 보관된 심의가 없습니다", "첫 안건을 작성하거나 연결 없이 예제 심의를 재생하세요.") + buttonRow(nav("새 안건", "input") + nav("예제 재생", "replay", "example"));
    else if (!data.length) body += notice("검색 결과가 없습니다", "다른 단어를 입력하거나 결과 필터를 해제하세요.") + buttonRow(action("검색 초기화", "records-clear"));
    if (v === "branches") body = `<ol class="records-branch"><li><span>R-037</span><h3>지원 범위 없이 공개하기</h3>${badge("제안 부결", "records-negative")}<p>지원 책임과 참여자의 선택권을 먼저 명시해야 합니다.</p></li><li><span>R-041 · R-037의 후속 질문</span><h3>${question}</h3>${badge("다수 지지")}<p>지원 범위를 추가했습니다. 철회 방법에 대한 반론은 남아 있습니다.</p></li><li><span>R-042 · R-041의 후속 질문</span><h3>${rows[1].title}</h3>${badge("교차 검토")}<p>앞선 표를 재사용하지 않고 새 자료와 질문을 심의합니다.</p></li></ol>`;
    const row = current();
    const aside = ["empty", "loading", "error"].includes(v) || memory.deleted.size === rows.length ? panel("로컬 기록", "<p>저장된 심의를 선택하면 당시 자료와 결과를 확인할 수 있습니다.</p>", "ARCHIVE") : panel("선택한 기록", `<p class="records-id">${esc(row.id)}</p><h3>${esc(row.title)}</h3>${badge(row.label)}<dl class="records-facts"><div><dt>자료</dt><dd>${row.sources}개 · 당시 수집본</dd></div><div><dt>연결</dt><dd>${row.parent || "상위 심의 없음"}</dd></div><div><dt>원문 접근</dt><dd>${memory.revoked ? "철회됨 · 보존한 수집본은 별도" : "선택한 범위만 허용"}</dd></div></dl>${buttonRow(action("선택한 기록 열기", "records-open", "primary") + nav("대화 분기", "history", "branches") + (row.status === "running" ? '<p class="field-help">진행 중인 심의는 공개된 의견을 먼저 확인하세요.</p>' : action("기록 재생", "records-replay") + action("공유 준비", "records-share")))}`, "SELECTED RECORD");
    return page({ kicker: "記録庫 / ARCHIVE", title: "심의 기록", intro: "질문, 판단, 반론과 근거를 당시 상태 그대로 읽습니다.", body, aside, actions: nav("공유 재생 열기", "import") + nav("데이터 관리", "data") });
  }
  const historyActions = {
    "records-search": ctx => { memory.search = readInput("records-search").trim(); memory.filter = readInput("records-filter", "all"); go(ctx, "history"); },
    "records-clear": ctx => { memory.search = ""; memory.filter = "all"; go(ctx, "history"); },
    "records-loaded": ctx => go(ctx, "history"),
    "records-replay": ctx => { memory.archiveRun = current().id; memory.replayStep = 0; stopReplay(); go(ctx, "replay"); },
    "records-share": ctx => { memory.archiveRun = current().id; memory.shareTitle = current().title; go(ctx, "share"); },
    "records-open": ctx => { ctx.formState.recordQuestion = current().title; ctx.formState.selectedRun = current().id; const outcome = current().status === "majority" ? "default" : current().status; ctx.formState.outcome = outcome; go(ctx, current().status === "running" ? "review" : "verdict", current().status === "running" ? "default" : outcome); },
  };
  rows.forEach(row => { historyActions[`records-select-${row.id}`] = ctx => { memory.selected = row.id; go(ctx, "history"); }; });
  S.history = { title: "심의 기록", group: "기록", variants: variants([["default", "목록"], ["empty", "기록 없음"], ["loading", "불러오기"], ["no-results", "검색 없음"], ["error", "읽기 실패"], ["branches", "대화 분기"]]), render: history, actions: historyActions };

  function stopReplay() { clearInterval(memory.timer); memory.timer = null; memory.playing = false; }
  function replay(ctx) {
    const v = ctx.variant;
    if (v === "complete") { memory.replayStep = stages.length - 1; stopReplay(); }
    if (v === "paused") stopReplay();
    if (["default", "example", "external"].includes(v)) memory.replayMode = v;
    const external = memory.replayMode === "external";
    const replayResult = external || memory.replayMode === "example" ? outcomes.majority : archivedOutcome();
    const label = external ? "EXTERNAL REPLAY / 외부 공유본" : memory.replayMode === "example" ? "DEMO / 예제" : "REPLAY / 기록 재생";
    const at = memory.replayStep;
    const body = `<div class="records-replay" data-records-replay="true"><div class="records-replay-banner">${esc(label)}<span>원래 기록 시각 · 2026.09.28 14:32</span></div>${external ? notice("수정 가능한 공유 자료", "원래 모델 실행은 확인되지 않았습니다. 포함된 정제 텍스트만 읽으며 원문·모델·외부 주소에 접근하지 않습니다.", "warning") : ""}<ol class="records-steps">${stages.map((stage, index) => `<li class="${index === at ? "is-current" : index < at ? "is-past" : ""}"${index === at ? ' aria-current="step"' : ""}><button type="button" data-action="replay-step-${index}"><b>${String(index + 1).padStart(2, "0")}</b>${stage}</button></li>`).join("")}</ol><section class="records-replay-stage" aria-live="polite"><span class="records-large-number">${String(at + 1).padStart(2, "0")}</span><div><p class="page-kicker">STORED PUBLIC EVENT</p><h2>${stages[at]}</h2><p>${["질문과 자료 범위가 확인됐습니다.", "세 관점의 검증된 의견을 보관했습니다.", objection, proposal, "세 코어의 표를 봉인합니다. 방향은 공개되지 않았습니다.", "같은 결의문에 대한 세 유효 표를 함께 공개했습니다."][at]}</p>${at === 5 ? voteLine(replayResult.votes) : badge(at === 4 ? "봉인 · 방향 비공개" : "저장된 단계")}</div></section>${buttonRow(action("이전 단계", "replay-prev") + action(memory.playing ? "일시 정지" : "재생", "replay-toggle", "primary") + action("다음 단계", "replay-next") + action(`속도 ${memory.replaySpeed}×`, "replay-speed"))}<p class="muted">${memory.playing ? "재생 중" : at === 5 ? "재생 완료" : "일시 정지"} · ${at + 1} / ${stages.length} 단계 · 모델 호출 0회</p></div>`;
    return page({ kicker: "再生 / PUBLIC EVENT REPLAY", title: "심의 다시 보기", intro: "기록된 공개 사건을 재생합니다. 새 의견이나 새 표를 만들지 않습니다.", body, aside: panel("재생 범위", `<p>${proposal}</p><p class="records-negative">CASPER 반론: ${objection}</p><p>원문 경로·인증 비밀·봉인된 중간 표는 재생 자료에 없습니다.</p>`, label), actions: nav("기록으로", "history") + nav("공유 준비", "share") });
  }
  function replayAdvance(ctx, delta) { memory.replayStep = Math.max(0, Math.min(stages.length - 1, memory.replayStep + delta)); go(ctx, "replay", memory.replayMode); }
  function play(ctx) {
    if (memory.playing) { stopReplay(); go(ctx, "replay", memory.replayMode); return; }
    if (memory.replayStep === stages.length - 1) memory.replayStep = 0;
    memory.playing = true;
    go(ctx, "replay", memory.replayMode);
    memory.timer = setInterval(() => {
      if (!document.querySelector("[data-records-replay]")) { stopReplay(); return; }
      if (memory.replayStep >= stages.length - 1) { stopReplay(); go(ctx, "replay", "complete"); return; }
      replayAdvance(ctx, 1);
    }, 2200 / memory.replaySpeed);
  }
  const replayActions = {
    "replay-prev": ctx => { stopReplay(); replayAdvance(ctx, -1); },
    "replay-next": ctx => { stopReplay(); replayAdvance(ctx, 1); },
    "replay-toggle": play,
    "replay-speed": ctx => { const wasPlaying = memory.playing; stopReplay(); memory.replaySpeed = memory.replaySpeed === 1 ? 2 : memory.replaySpeed === 2 ? 0.5 : 1; if (wasPlaying) play(ctx); else go(ctx, "replay", memory.replayMode); },
  };
  stages.forEach((_, index) => { replayActions[`replay-step-${index}`] = ctx => { stopReplay(); memory.replayStep = index; go(ctx, "replay", memory.replayMode); }; });
  S.replay = { title: "기록 재생", group: "기록", variants: variants([["default", "기록 재생"], ["example", "DEMO 예제"], ["external", "외부 공유본"], ["paused", "일시 정지"], ["complete", "재생 완료"]]), render: replay, actions: replayActions };

  function captureShare() {
    memory.shareTitle = readInput("share-title", memory.shareTitle).trim() || "제한 공개에 대한 결의";
    memory.shareFormat = readInput("share-format", memory.shareFormat);
    memory.includeEvidence = document.getElementById("share-evidence")?.checked ?? memory.includeEvidence;
  }
  const exportName = () => memory.shareFormat === "image" ? "magi-resolution.png + magi-resolution.txt" : memory.shareFormat === "archive" ? "magi-private-record.json" : "magi-resolution.json";
  function share(ctx) {
    const v = ctx.variant;
    const redacted = v === "redacted" || memory.shareRedacted;
    const result = archivedOutcome();
    let warning = "";
    if (v === "sensitive") warning = notice("민감한 내용 확인이 필요합니다", "연락처와 개인 경로가 본문에서 발견됐습니다. 정제 결과를 확인하기 전에는 공유 파일을 만들 수 없습니다.", "warning");
    if (v === "saving") warning = notice("내보내기 결과 확인 중", "완성한 파일과 내용을 확인한 후에만 저장 완료를 표시합니다.") + buttonRow(action("저장 결과 확인", "share-saved"));
    if (v === "saved") warning = notice("내보내기 완료", `Downloads / ${exportName()} · 결의문·표·반대 의견${memory.includeEvidence ? "·정제된 근거 발췌" : ""} 포함 · 자동 업로드 없음`, "success");
    if (v === "error") warning = notice("파일을 내보내지 못했습니다", "저장 위치에 쓸 수 없습니다. 기존 파일은 변경되지 않았습니다.", "error") + buttonRow(action("저장 위치 다시 확인", "share-save"));
    if (v === "overwrite") warning = notice("같은 이름의 파일이 있습니다", `Downloads / ${exportName()}의 정확한 대상을 확인한 뒤 교체를 선택하세요.`, "warning");
    const archive = v === "archive";
    const form = `<div class="form-grid">${field("공유 제목", "share-title", memory.shareTitle)}${select("출력 형식", "share-format", [{ value: "replay", label: "공개 이벤트 재생 JSON" }, { value: "image", label: "정적 결의 이미지와 텍스트" }, { value: "archive", label: "개인 보관용 결의 묶음" }], archive ? "archive" : memory.shareFormat)}</div><fieldset class="records-checks"><legend>포함 범위</legend><label><input type="checkbox" checked disabled> 표결한 결의문·세 표·반대와 기권 사유</label><label><input type="checkbox" id="share-evidence"${memory.includeEvidence || archive ? " checked" : ""}> 정제한 근거 발췌</label><label><input type="checkbox" checked disabled> 정제 여부·REPLAY 표시</label></fieldset>`;
    const preview = `<article class="records-share-preview"><p class="page-kicker">MAGI COMMAND CONSOLE · REPLAY${redacted ? " · 정제됨" : ""}</p><h2>${esc(memory.shareTitle)}</h2><p class="records-decision" data-outcome="${esc(archived().status)}">${result.name}</p>${voteLine(result.votes)}<h3>표결한 결의문</h3><p>${proposal}</p><h3 class="records-negative">CASPER의 반대</h3><p>${objection}</p>${result.votes.includes("abstain") ? `<h3>기권 사유</h3><p>${esc(result.why)}</p>` : ""}${memory.includeEvidence ? "<h3>공개 근거 발췌</h3><p>[02] 지원 범위와 대상 인원을 제한한다. 철회 절차는 문서에서 확인되지 않았습니다.</p>" : ""}<small>비공식 팬 프로젝트 · 원본 실행 결과의 표시용 사본</small></article>`;
    const aside = panel("공개 범위", `<dl class="records-facts"><div><dt>기본 제외</dt><dd>원문·로컬 경로·계정·인증 비밀</dd></div><div><dt>유지</dt><dd>표 조합·소수 의견·미확인 조건</dd></div><div><dt>게시</dt><dd>로컬 내보내기만 · 자동 업로드 없음</dd></div></dl>${redacted ? notice("정제된 문안", "개인 식별 정보를 제거했습니다. 표는 원본 결과이며 정제 문안에 다시 표결한 결과가 아닙니다.") : ""}${archive ? notice("개인 보관용", "원문을 포함하면 민감한 정보가 남을 수 있습니다. 공개 공유물과 별도로 보관하세요.", "warning") : ""}`, "DISCLOSURE");
    return page({ kicker: "共有 / EXPORT", title: "공유 전 확인", intro: "무엇을 내보내는지와 무엇을 제거했는지를 출력 그대로 확인합니다.", body: warning + panel("내보낼 내용", form) + preview, aside, actions: action("미리보기 갱신", "share-preview") + action("민감 내용 정제", "share-redact") + (v === "sensitive" ? '<button class="button button-primary" disabled aria-describedby="share-blocked">내보내기 불가</button><span id="share-blocked" class="field-help">정제 결과를 먼저 확인하세요.</span>' : action(v === "overwrite" ? "대상 파일 교체 확인" : "내보내기 확인", v === "overwrite" ? "share-overwrite" : "share-save", "primary")) });
  }
  const shareActions = {
    "share-preview": ctx => { captureShare(); go(ctx, "share", memory.shareFormat === "archive" ? "archive" : "default"); },
    "share-redact": ctx => { captureShare(); memory.shareRedacted = true; memory.shareTitle = "제한 공개에 대한 세 관점의 결의"; go(ctx, "share", "redacted"); },
    "share-save": ctx => { captureShare(); confirm(ctx, "공유 파일 내보내기", `<p><b>${esc(exportName())}</b>에 결의문·세 표·소수 의견을 내보냅니다.</p><p>근거 발췌: ${memory.includeEvidence ? "정제한 범위 포함" : "제외"}. 개인 경로·인증 비밀은 제외하며 자동 게시하지 않습니다.</p>`, "내보내기 확인", () => go(ctx, "share", "saved")); },
    "share-overwrite": ctx => { captureShare(); confirm(ctx, "기존 파일 교체", `<p>대상: Downloads / ${esc(exportName())}</p><p>검증된 새 내용으로 표시한 대상만 교체합니다. 다른 파일은 변경하지 않습니다.</p>`, "표시한 대상 교체", () => go(ctx, "share", "saved"), true); },
    "share-saved": ctx => go(ctx, "share", "saved"),
  };
  S.share = { title: "공유와 내보내기", group: "기록", variants: variants([["default", "공개 미리보기"], ["redacted", "정제된 본문"], ["sensitive", "민감 내용"], ["saving", "저장 확인"], ["saved", "저장 결과"], ["error", "저장 실패"], ["overwrite", "덮어쓰기"], ["archive", "개인 보관"]]), render: share, actions: shareActions };

  S.import = {
    title: "공유 재생 열기", group: "기록", variants: variants([["default", "파일 선택"], ["checking", "형식 검사"], ["invalid", "유효하지 않음"]]),
    render: ctx => page({ kicker: "受信 / EXTERNAL REPLAY", title: "공유 재생 열기", intro: "공유받은 기록은 내 실제 심의와 분리해 열람합니다.", body: panel("재생 파일", `<p class="records-file-icon" aria-hidden="true">{ }</p><h2>공개 이벤트 JSON</h2><p>최대 10 MiB · 압축 파일·원격 자산·실행 가능한 내용은 받지 않습니다.</p>${buttonRow(action("파일 선택", "import-choose", "primary"))}`) + (ctx.variant === "checking" ? notice("파일을 확인합니다", "버전·크기·이벤트 순서·제안 참조·정제 표시를 검사합니다.") + buttonRow(action("검사 결과 보기", "import-valid")) : ctx.variant === "invalid" ? notice("이 공유 파일을 열 수 없습니다", "제안과 표의 참조가 일치하지 않습니다. 기존 기록은 변경되지 않았고 일부 결과도 가져오지 않았습니다.", "error") + buttonRow(action("다른 파일 선택", "import-choose")) : ""), aside: panel("독립된 외부 보기", `<p>EXTERNAL REPLAY 표시는 항상 유지됩니다.</p><p>형식 검사는 실제 모델 실행·작성자·원문이 사실임을 보증하지 않습니다.</p><p>원본 경로를 찾거나 모델에 전송하지 않습니다.</p>`, "UNTRUSTED DATA"), actions: nav("기록으로", "history") }),
    actions: { "import-choose": ctx => go(ctx, "import", "checking"), "import-valid": ctx => { memory.replayStep = 0; memory.replayMode = "external"; go(ctx, "replay", "external"); } },
  };

  function data(ctx) {
    const v = ctx.variant;
    let body;
    if (v === "backup") body = panel("보존할 범위", `<div class="records-checks"><label><input type="checkbox" checked disabled> 대화·결의·표·공개 이벤트</label><label><input type="checkbox" id="backup-sources"${memory.includeSources ? " checked" : ""}> 참조하는 원문과 추출 객체</label><label><input type="checkbox" id="backup-settings"${memory.includeSettings ? " checked" : ""}> 역할·연결 프로필의 비밀 없는 설정</label></div>${notice("포함하지 않는 항목", "Keychain 비밀·공급자 로그인·실행 권한·살아 있는 세션은 백업하지 않습니다.")}${notice("보호 수준", "원문을 포함한 백업은 민감한 로컬 데이터입니다. 이 앱이 별도 암호화를 적용했다고 표시하지 않습니다.", "warning")}${memory.backupSaved ? notice("백업 결과 확인", `기록 ${rows.length - memory.deleted.size}개 · 원문 ${memory.includeSources ? 3 : 0}개 · 객체 해시 확인 · 인증 비밀 제외`, "success") : ""}${buttonRow(action("백업 범위 확인", "data-backup", "primary"))}`, "LOCAL BACKUP");
    else if (v === "restore" || v === "restore-error") body = panel("별도 저장소로 복원", `<p class="records-id">magi-backup.json</p><p>기록 4개 · 원문 3개 · 저장 형식 확인</p>${v === "restore-error" ? notice("백업 검증 실패", "객체 하나의 해시가 일치하지 않습니다. 현재 저장소를 유지하며 복원본으로 전환하지 않습니다.", "error") : notice("기존 저장소를 먼저 변경하지 않습니다", "새 저장소에서 객체와 참조를 검증한 뒤 전환합니다. 로그인·동의·실행 세션은 복원하지 않습니다.")}${memory.restored ? notice("복원 결과 확인", "복원된 기록을 열람할 수 있습니다. 모델 연결과 전송 범위를 다시 확인하기 전에는 심의를 시작하지 않습니다.", "success") : ""}${buttonRow(v === "restore-error" ? nav("다른 백업 확인", "data", "restore") : action("검증된 복원본으로 전환 확인", "data-restore", "primary"))}`, "NEW DATA STORE");
    else if (v === "delete") body = panel("삭제 영향 확인", `<h2>${esc(current().title)}</h2><p>${esc(current().id)} · 결의·공개 평가·표·연결된 기록</p><dl class="records-facts"><div><dt>함께 확인</dt><dd>다른 기록이 참조하는 원문은 별도 보존</dd></div><div><dt>원문만 삭제</dt><dd>결의는 유지되지만 근거가 삭제됨으로 표시</dd></div><div><dt>공급자 자료</dt><dd>이미 전송한 내용의 외부 삭제를 보장하지 않음</dd></div></dl>${buttonRow(action("이 심의 기록 삭제", "data-delete", "danger") + nav("삭제하지 않고 돌아가기", "history"))}`, "DELETE SCOPE");
    else body = `<div class="records-permission-grid">${panel("로컬 자료 읽기", `<h3>프로젝트 자료 · 파일 3개</h3><p>release-plan.md 외 2개</p>${badge(memory.revoked || v === "revoked" ? "읽기 권한 철회됨" : "선택한 범위 허용")}${buttonRow(memory.revoked || v === "revoked" ? nav("자료 다시 선택", "intake") : action("앞으로의 읽기 철회", "data-revoke", "danger"))}`, "SOURCE GRANT")}${panel("모델에 전송", `<h3>현재 심의의 확인된 자료</h3><p>새 공급자·새 자료에 자동으로 이어지지 않습니다.</p>${badge(memory.disclosureRevoked ? "후속 전송 차단" : "확인한 범위에서만 전송")}${buttonRow(memory.disclosureRevoked ? nav("새 입력 확인", "confirmation") : action("후속 전송 동의 철회", "data-disclosure", "danger"))}`, "DISCLOSURE GRANT")}</div>${memory.revoked || v === "revoked" ? notice("권한 철회와 삭제는 다릅니다", "새 원문 읽기를 막았습니다. 이미 수집한 사본과 이전 결의는 별도 보존하며, 외부로 보낸 내용을 회수한 것은 아닙니다.") : ""}`;
    return page({ kicker: "資料管理 / LOCAL DATA", title: "자료와 보관", intro: "파일 읽기, 모델 전송, 보관된 사본과 삭제를 따로 관리합니다.", body, aside: panel("데이터 경계", `<p>앱의 로컬 기록은 사용자 기기에 보관합니다.</p><p>접근 철회만으로 원문 사본을 지우거나 외부 자료를 회수하지 않습니다.</p>${buttonRow(nav("읽기·전송 권한", "data") + nav("기록 삭제", "data", "delete") + nav("백업", "data", "backup") + nav("복원", "data", "restore"))}`, "CONTROL"), actions: nav("기록으로", "history") });
  }
  S.data = {
    title: "자료와 보관", group: "관리", variants: variants([["default", "읽기·전송 권한"], ["revoked", "권한 철회"], ["delete", "삭제 영향"], ["backup", "백업"], ["restore", "복원"], ["restore-error", "복원 실패"]]), render: data,
    actions: {
      "data-revoke": ctx => confirm(ctx, "앞으로의 자료 읽기 철회", "<p>선택한 파일 3개의 새 읽기와 새 수집을 막습니다. 보관한 사본과 이전 결의는 삭제하지 않습니다.</p>", "읽기 권한 철회", () => { memory.revoked = true; go(ctx, "data", "revoked"); }, true),
      "data-disclosure": ctx => confirm(ctx, "후속 모델 전송 차단", "<p>아직 보내지 않은 요청과 후속 전송을 막고 진행 중 요청의 취소를 확인합니다. 이미 보낸 자료는 회수할 수 없습니다.</p>", "전송 동의 철회", () => { memory.disclosureRevoked = true; go(ctx, "data"); }, true),
      "data-delete": ctx => {
        const target = current();
        if (target.status === "running") {
          confirm(ctx, "진행 중인 심의는 먼저 취소합니다", `<p><b>${esc(target.id)} · ${esc(target.title)}</b></p><p>정지 여부를 확인하기 전에는 기록을 삭제하지 않습니다. 취소 화면에서 이 심의의 요청을 확인하세요.</p>`, "취소 확인으로", () => { ctx.formState.selectedRun = target.id; ctx.formState.recordQuestion = target.title; go(ctx, "cancelling"); }, true);
          return;
        }
        confirm(ctx, "심의 기록 삭제", `<p><b>${esc(target.id)} · ${esc(target.title)}</b></p><p>이 기록만 삭제합니다. 다른 기록의 공유 원문은 유지합니다.</p>`, "이 기록 삭제", () => { memory.deleted.add(target.id); memory.selected = rows.find(row => !memory.deleted.has(row.id))?.id || "R-041"; go(ctx, "history", memory.deleted.size === rows.length ? "empty" : "default"); }, true);
      },
      "data-backup": ctx => {
        memory.includeSources = document.getElementById("backup-sources")?.checked ?? true;
        memory.includeSettings = document.getElementById("backup-settings")?.checked ?? true;
        confirm(ctx, "백업 범위 확인", `<p>심의 기록 포함 · 원문 ${memory.includeSources ? "포함" : "제외"} · 비밀 없는 설정 ${memory.includeSettings ? "포함" : "제외"}.</p><p>인증 비밀·로그인·권한·실행 세션은 제외합니다.</p>`, "백업 결과 확인", () => { memory.backupSaved = true; go(ctx, "data", "backup"); });
      },
      "data-restore": ctx => confirm(ctx, "검증된 복원본으로 전환", "<p>새 저장소를 사용합니다. 기존 저장소는 검증이 끝날 때까지 보존하며, 이전 로그인·전송 동의·실행을 재활성화하지 않습니다.</p>", "복원 결과 확인", () => { memory.restored = true; memory.deleted.clear(); memory.revoked = true; memory.disclosureRevoked = true; ctx.formState.storageBlocked = false; if (ctx.formState.setup) { ctx.formState.setup.connectionReady = false; ctx.formState.setup.consent = false; } go(ctx, "data", "restore"); }),
    },
  };

  function maintenance(ctx) {
    const v = ctx.variant;
    if (v === "migration-error" || v === "migrating") ctx.formState.storageBlocked = v;
    let body;
    if (v === "migration-error") body = panel("저장 형식 변환을 완료하지 못했습니다", `${notice("새 심의가 차단되었습니다", "변환한 저장소의 참조 검증이 실패했습니다. 입력 중인 안건과 기존 저장소는 보존하며 불완전한 복원본으로 전환하지 않습니다.", "error")}<dl class="records-facts"><div><dt>기존 기록</dt><dd>원본 저장소 보존 · 자동 덮어쓰기 없음</dd></div><div><dt>읽기·실행</dt><dd>검증된 기록만 읽기 가능 · 새 심의와 재개 차단</dd></div><div><dt>복구 범위</dt><dd>호환되는 백업을 별도 저장소에서 검증한 뒤 전환</dd></div></dl>${buttonRow(nav("백업으로 별도 저장소 복원", "data", "restore", "primary") + nav("진단 정보 보기", "maintenance", "diagnostics"))}`, "STORAGE RECOVERY");
    else if (v === "about") body = panel("MAGI CONSOLE", `<p class="records-wordmark">MAGI<br>COMMAND CONSOLE</p><p>무료 비공식 팬 창작. 기여 코드·문서는 MIT이며 원작 표현에 관한 권리와 구분합니다.</p><dl class="records-facts"><div><dt>배포 자산</dt><dd>자체 도형·서체·효과음의 출처 기록</dd></div><div><dt>서체</dt><dd>Barlow Condensed · Noto Sans KR · IBM Plex Mono / SIL OFL 1.1</dd></div><div><dt>원작 자산</dt><dd>공식 영상·로고·음악을 추출해 포함하지 않음</dd></div></dl>${buttonRow(action("라이선스 고지", "maintenance-licenses"))}`, "ABOUT / LICENSES");
    else if (v === "diagnostics") body = panel("내보낼 진단 정보", `<ul class="records-check-list"><li>실행 단계·오류 코드·지연·공급자 연결 버전</li><li>사용량 제공 여부와 상태 이벤트 순서</li><li>질문·답변·원문·파일명·개인 경로·계정·비밀 제외</li></ul><pre class="records-log">stage: cross_review\nerror: provider_response_unconfirmed\nsource_content: excluded\naccount_identity: excluded\ncredential: excluded</pre>${notice("자동 전송하지 않습니다", "아래 확인에서 실제 포함 범위를 검토한 뒤 로컬 진단 묶음의 결과를 확인합니다.")}${buttonRow(action("진단 묶음 확인", "maintenance-diagnostics", "primary"))}`, "REDACTED DIAGNOSTICS");
    else {
      const statuses = { default: ["설치된 앱을 확인합니다", "업데이트 조회는 모델 추론과 별개이며 질문·원문을 전송하지 않습니다."], available: ["검증할 업데이트가 있습니다", "앱 서명·업데이트 서명·파일 해시·운영체제·저장 형식을 확인합니다."], downloading: ["업데이트 파일 수신", "수신된 파일의 검증이 완료될 때까지 설치하지 않습니다."], blocked: ["심의가 진행 중입니다", "진행 중인 실행의 완료 또는 명시적인 취소 확인 뒤 업데이트를 적용합니다."], invalid: ["업데이트 서명을 확인하지 못했습니다", "새 배포물을 적용하지 않습니다. 설치된 앱과 기록은 유지됩니다."], migrating: ["저장 형식 변환 중", "일관된 백업과 여유 공간을 확인했습니다. 변환 중에는 새 심의를 시작하지 않습니다."] };
      const [title, description] = statuses[v] || statuses.default;
      body = panel(title, `<p>${description}</p><dl class="records-facts"><div><dt>macOS 코드 서명·공증</dt><dd>${v === "invalid" ? "검증 보류" : "배포물 식별 확인"}</dd></div><div><dt>업데이트 서명·해시</dt><dd>${v === "invalid" ? "불일치 · 설치 차단" : v === "default" ? "조회 전" : "검증 대상 고정"}</dd></div><div><dt>기존 기록</dt><dd>보존 · 검증 전 덮어쓰지 않음</dd></div></dl>${v === "invalid" ? notice("설치를 진행할 수 없습니다", "서명을 통과하지 못한 파일을 우회 설치하지 않습니다.", "error") : ""}${buttonRow(v === "blocked" ? nav("진행 중 심의 보기", "review") : v === "invalid" ? action("배포 정보 다시 확인", "maintenance-check") : v === "migrating" ? action("변환 결과 확인", "maintenance-finished") : v === "downloading" ? action("수신·검증 결과 확인", "maintenance-download") : v === "available" ? action("업데이트 적용 확인", "maintenance-install", "primary") : action("업데이트 확인", "maintenance-check", "primary"))}`, "UPDATE VERIFICATION");
    }
    return page({ kicker: "保守 / SYSTEM", title: v === "diagnostics" ? "진단 정보" : v === "about" ? "앱과 라이선스" : "앱 업데이트", intro: "앱과 기록을 검증된 상태로 유지합니다.", body, aside: panel("관리", buttonRow(nav("업데이트", "maintenance") + nav("진단 정보", "maintenance", "diagnostics") + nav("앱과 라이선스", "maintenance", "about") + nav("백업·복원", "data", "backup")), "SYSTEM MENU"), actions: nav("콘솔로", "input") });
  }
  S.maintenance = {
    title: "앱 관리", group: "관리", variants: variants([["default", "업데이트"], ["available", "새 배포물"], ["downloading", "수신·검증"], ["blocked", "실행 중 차단"], ["invalid", "서명 실패"], ["migrating", "저장 형식 변환"], ["migration-error", "저장 형식 변환 실패"], ["diagnostics", "진단 정보"], ["about", "앱·라이선스"]]), render: maintenance,
    actions: {
      "maintenance-check": ctx => go(ctx, "maintenance", "available"),
      "maintenance-download": ctx => go(ctx, "maintenance", "available"),
      "maintenance-install": ctx => {
        if (ctx.isRunActive) { go(ctx, "maintenance", "blocked"); return; }
        confirm(ctx, "업데이트 적용 확인", "<p>백업·서명·저장 형식 호환성을 확인한 뒤 적용합니다.</p>", "변환 화면 확인", () => go(ctx, "maintenance", "migrating"));
      },
      "maintenance-finished": ctx => { ctx.formState.storageBlocked = false; ctx.notify("저장 형식 변환이 완료됐습니다. 보존된 기록을 확인할 수 있습니다."); go(ctx, "maintenance"); },
      "maintenance-diagnostics": ctx => confirm(ctx, "진단 묶음 포함 범위", "<p>상태·오류·연결 버전·이벤트 순서만 포함합니다. 질문·원문·경로·계정·비밀은 제외하며 자동 업로드하지 않습니다.</p>", "정제된 결과 확인", () => ctx.notify("정제된 진단 묶음을 준비했습니다. 자동으로 전송하지 않았습니다.")),
      "maintenance-licenses": ctx => confirm(ctx, "라이선스와 팬 창작 고지", "<p>기여 코드·문서: MIT.</p><p>Barlow Condensed, Noto Sans KR, IBM Plex Mono: SIL OFL 1.1.</p><p>원작의 명칭·상표·이미지·음원 권리는 코드 라이선스와 별개입니다.</p>", "확인", () => {}),
    },
  };

  function companion(ctx) {
    const requestedVariant = ctx.variant;
    const requestedTarget = requestedVariant === "record" || outcomes[requestedVariant] ? "R-041" : memory.companionTarget;
    const v = memory.deleted.has(requestedTarget) ? "deleted" : requestedVariant;
    const outcome = outcomes[v === "record" ? "majority" : v];
    const empty = v === "empty";
    const unavailable = v === "offline" || v === "deleted";
    const cancelled = ctx.formState.companionStage === "cancelled" && !outcome && v !== "empty";
    const phase = cancelled ? "심의 취소됨" : outcome ? "표결 완료" : ({ sealed: "봉인 표결", paused: "일시 중단 · 인증 필요", offline: "상태 확인 중", deleted: "기록 삭제됨", empty: "안건 대기", display: "화면 조정 필요" }[v] || "교차 검토");
    const votes = outcome?.votes;
    const topologyState = cancelled ? "cancelled" : votes ? "verdict" : v === "sealed" ? "sealed" : v === "paused" ? "paused" : unavailable ? "interrupted" : empty ? "input" : "review";
    const smallCore = ctx.topology(topologyState, true, v === "record" || v === "majority" ? "default" : v);
    const target = requestedTarget;
    const title = target === "R-041" ? question : rows[1].title;
    return `<section class="records-companion-stage" data-companion-open="true"><div class="records-tray-anchor" aria-hidden="true">▲</div><div class="records-companion" aria-label="메뉴 막대 상태 팝오버"><header class="records-companion-header"><h1>MAGI CONSOLE</h1><button class="records-icon-button" type="button" data-action="companion-close" aria-label="팝오버 닫기">×</button></header><div class="records-companion-body"><div class="records-companion-status"><strong>${phase}</strong><span>${unavailable ? "마지막 확인 14:40" : "현재 저장 상태"}</span></div>${smallCore}<p class="records-companion-target">${v === "record" ? "기록 · 2026.09.28 14:32" : empty ? "새 안건을 기다립니다" : `표시 중 ${target} · 대상 고정`}</p><h2>${empty ? "무엇을 함께 판단할까요?" : esc(title)}</h2>${outcome ? `<div class="records-companion-verdict" data-outcome="${esc(v)}"><strong>${outcome.name}</strong>${voteLine(votes)}<p>${proposal}</p><p class="${v === "unanimous" ? "muted" : "records-negative"}">${outcome.why}</p></div>` : v === "sealed" ? '<p class="records-seal">표 2 / 3 접수 · 방향 비공개</p><p>세 표를 검증한 뒤 함께 공개합니다.</p>' : v === "paused" ? notice("연결 인증이 필요합니다", "콘솔에서 연결을 복구하고 명시적으로 재개하세요.", "warning") : unavailable ? notice(phase, "이 상태에서는 심의를 취소할 수 없습니다. 확인할 수 있는 기록만 유지합니다.", "warning") : v === "display" ? notice("메인 콘솔의 작업 영역이 부족합니다", "1024×720 콘텐츠와 창 장식을 담을 수 있도록 화면 배율을 조정하거나 디스플레이를 연결하세요. 심의는 보존됩니다.", "warning") : cancelled ? '<p>부분 기록을 보존했습니다. 최종 판정은 없습니다.</p>' : empty ? '<p>안건 작성과 자료 선택은 메인 콘솔에서 진행합니다.</p>' : `<p>현재 쟁점: ${objection}</p>`}${v === "multiple" ? `<div class="records-companion-other"><p>다른 진행 중 심의 2건</p>${action("다른 실행 확인", "companion-other")}</div>` : ""}</div><footer class="records-companion-footer">${v === "display" ? '<button class="button button-primary" disabled aria-describedby="companion-display-help">화면 조정 후 콘솔 열기</button><span id="companion-display-help" class="field-help">심의와 마지막 유효 위치는 보존됩니다.</span>' : action("콘솔 열기", "companion-console", "primary")}<div>${nav("설정", "settings")}${!empty && !outcome && !cancelled ? unavailable ? '<button class="button button-danger" disabled>상태 확인 전 취소 불가</button>' : action("심의 취소", "companion-cancel", "danger") : ""}${action("앱 종료", "companion-quit")}</div></footer></div><p class="records-companion-context">메뉴 막대 보조 창 · 440 × 560</p></section>`;
  }
  S.companion = {
    title: "메뉴 막대", group: "메뉴 막대", profile: "companion", variants: variants([["default", "진행 중"], ["empty", "안건 대기"], ["sealed", "봉인 표결"], ["majority", "다수 지지"], ["unanimous", "만장일치"], ["rejected", "제안 부결"], ["unresolved", "결론 미도출"], ["paused", "일시 중단"], ["offline", "연결 단절"], ["record", "과거 기록"], ["deleted", "기록 삭제"], ["multiple", "여러 실행"], ["display", "작업 영역 부족"]]), render: companion,
    actions: {
      "companion-close": ctx => ctx.closePopover(),
      "companion-console": ctx => {
        const ended = !!outcomes[ctx.variant] || ctx.variant === "record";
        const target = ended ? "R-041" : memory.companionTarget;
        if (ctx.variant === "deleted" || memory.deleted.has(target)) { go(ctx, "history"); return; }
        ctx.formState.selectedRun = target;
        ctx.formState.recordQuestion = ended ? question : rows[1].title;
        ctx.formState.outcome = ctx.variant === "record" || ctx.variant === "majority" ? "default" : ctx.variant;
        const route = ended ? "verdict" : ctx.variant === "empty" ? "input" : ctx.variant === "paused" ? "paused" : ctx.variant === "offline" ? "interrupted" : ctx.variant === "deleted" ? "history" : ctx.variant === "sealed" ? "sealed" : ctx.formState.companionStage === "cancelled" ? "cancelled" : "review";
        go(ctx, route, ended ? ctx.formState.outcome : ctx.variant === "paused" ? "auth" : "default");
      },
      "companion-other": ctx => go(ctx, "history"),
      "companion-cancel": ctx => {
        const target = memory.companionTarget;
        if (["offline", "deleted"].includes(ctx.variant) || memory.deleted.has(target)) return;
        ctx.formState.selectedRun = target;
        ctx.formState.recordQuestion = rows[1].title;
        confirm(ctx, "표시 중인 안건 취소", `<p><b>${esc(target)} · ${esc(rows[1].title)}</b></p><p>이 심의의 새 요청을 막고 진행 중 요청의 정지를 확인합니다. 다른 심의는 유지합니다.</p>`, "이 심의 취소", () => go(ctx, "cancelling"), true);
      },
      "companion-quit": ctx => ctx.quit(),
    },
  };
  window.MagiRecords = Object.freeze({ getRecord: id => { const row = rows.find(item => item.id === id && !memory.deleted.has(id)); return row ? Object.freeze({ ...row }) : null; } });
})();
