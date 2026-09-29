(() => {
  "use strict";
  const esc = value => String(value ?? "").replace(/[&<>"']/g, char => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[char]);
  const toneClass = tone => tone === "primary" ? " button-primary" : ["danger", "error"].includes(tone) ? " button-danger" : "";
  const button = (label, screen, variant = "default", tone = "") => `<button type="button" class="button${toneClass(tone)}" data-go="${esc(screen)}" data-variant="${esc(variant)}">${esc(label)} <span aria-hidden="true">↗</span></button>`;
  const action = (label, command, tone = "") => `<button type="button" class="button${toneClass(tone)}" data-action="${esc(command)}">${esc(label)}</button>`;
  const panel = (title, html, kicker = "") => `<section class="panel">${kicker ? `<p class="panel-kicker">${esc(kicker)}</p>` : ""}<h3 class="panel-heading">${esc(title)}</h3>${html}</section>`;
  const notice = (title, text, tone = "info") => `<section class="notice notice-${esc(tone)}"><strong>${esc(title)}</strong><p>${esc(text)}</p></section>`;
  const field = (label, id, value = "", type = "text") => `<label class="field" for="${esc(id)}"><span class="field-label">${esc(label)}</span>${type === "textarea" ? `<textarea id="${esc(id)}" name="${esc(id)}" rows="4">${esc(value)}</textarea>` : `<input id="${esc(id)}" name="${esc(id)}" type="${esc(type)}" value="${esc(value)}"/>`}</label>`;
  const select = (label, id, options, selected) => `<label class="field" for="${esc(id)}"><span class="field-label">${esc(label)}</span><select id="${esc(id)}" name="${esc(id)}">${options.map(option => `<option value="${esc(option.value)}"${option.value === selected ? " selected" : ""}>${esc(option.label)}</option>`).join("")}</select></label>`;
  const page = ({ kicker, title, intro = "", body, aside = "", actions = "" }) => `<section class="design-page"><header class="page-heading"><div><p class="page-kicker">${esc(kicker)}</p><h2>${esc(title)}</h2>${intro ? `<p class="page-intro">${esc(intro)}</p>` : ""}</div><span class="page-emblem" aria-hidden="true">MAGI<br>三賢人</span></header><div class="page-layout${aside ? "" : " page-layout-wide"}"><div class="page-main">${body}</div>${aside ? `<aside class="page-aside">${aside}</aside>` : ""}</div>${actions ? `<footer class="page-actions">${actions}</footer>` : ""}</section>`;
  const cores = [
    { id: "balthasar", name: "BALTHASAR·2", number: "02", role: "지속성과 돌봄", points: "320,10 1040,10 1120,100 1120,160 850,325 510,325 240,160 240,100", summary: "지원 범위와 담당자가 명확하다면 제한 공개의 운영 부담을 관리할 수 있습니다.", question: "공개 이후 참여자를 지속적으로 지원할 수 있는가?" },
    { id: "casper", name: "CASPER·3", number: "03", role: "주체성과 대안", points: "175,235 230,235 560,440 635,540 635,595 45,595 10,560 10,435", summary: "참여자의 철회 방법을 먼저 명시해야 합니다.", question: "참여자가 스스로 선택하고 철회할 수 있는가?" },
    { id: "melchior", name: "MELCHIOR·1", number: "01", role: "근거와 실현 가능성", points: "1185,235 1130,235 800,440 725,540 725,595 1315,595 1350,560 1350,435", summary: "공개 범위와 대상 인원 제한이 문서에 명시되어 있어 소규모 공개를 검토할 수 있습니다.", question: "문서에 명시된 제한을 실제로 적용할 수 있는가?" },
  ];
  const outcomes = {
    default: { title: "다수 지지", english: "MAJORITY", japanese: "可決", votes: ["support", "oppose", "support"] },
    unanimous: { title: "만장일치 지지", english: "UNANIMOUS", japanese: "全会一致", votes: ["support", "support", "support"] },
    rejected: { title: "제안 부결", english: "REJECTED", japanese: "否決", votes: ["oppose", "oppose", "support"] },
    unresolved: { title: "결론 미도출", english: "UNRESOLVED", japanese: "未決", votes: ["support", "oppose", "abstain"] },
    abstention: { title: "다수 지지 · 기권 1", english: "MAJORITY", japanese: "可決", votes: ["support", "abstain", "support"] },
  };
  const phases = {
    input: ["입력 대기", "入力待機", "READY"], independent: ["독립 검토", "独立審議", "ISOLATED"], review: ["교차 검토", "相互検討", "REVIEW"],
    proposal: ["결의문 작성", "決議作成", "DRAFT"], sealed: ["표 봉인됨", "封印", "SEALED"], paused: ["심의 일시정지", "待機", "PAUSED"],
    interrupted: ["상태 확인 필요", "中断", "UNKNOWN"], cancelling: ["취소 확인 중", "停止中", "STOPPING"], cancelled: ["심의 취소됨", "中止", "STOPPED"], failed: ["심의 실패", "障害", "FAILED"], "save-error": ["저장 실패", "保存障害", "UNSAVED"],
  };
  function topology(state = "input", compact = false, variant = "default") {
    const revealed = ["verdict", "evidence", "completed"].includes(state);
    const outcome = outcomes[variant] || outcomes.default;
    const phase = phases[state] || phases.input;
    const voteName = vote => ({ support: "찬성", oppose: "반대", abstain: "기권" })[vote];
    const voteJapanese = vote => ({ support: "賛成", oppose: "反対", abstain: "棄権" })[vote];
    return `<div class="topology${compact ? " compact" : ""}" data-component="CoreTopology" data-geometry="1360x600"><svg class="topology-svg" viewBox="0 0 1360 600" aria-hidden="true" focusable="false">${cores.map((core, i) => `<polygon class="core-face" data-core="${core.id}"${revealed ? ` data-vote="${outcome.votes[i]}"` : ""} points="${core.points}"/>`).join("")}<path class="core-circuit${state === "independent" ? " circuit-isolated" : ""}" d="M240 175 L500 337 L606 337 L628 325 L732 325 L754 337 L860 337 L1120 175 M230 220 L566 427 L650 540 L650 600 M1130 220 L794 427 L710 540 L710 600 M680 490 L680 555 L651 580 M680 555 L709 580"/>${state === "review" ? '<path class="cross-signal" d="M256 189 L508 346 L625 346 M1104 189 L852 346 L735 346 M680 491 L680 588"/>' : ""}<polygon class="hub" points="630,342 730,342 765,385 720,465 680,490 640,465 595,385"/><polygon class="hub-inner" points="634,351 726,351 754,386 713,458 680,478 647,458 606,386"/></svg>${cores.map((core, i) => `<span class="core-number" data-core="${core.id}" aria-hidden="true"><small>MAGI CORE</small>${core.number}</span><button class="core-control" type="button" data-core="${core.id}"${revealed ? ` data-vote="${outcome.votes[i]}"` : ""} aria-label="${core.name} · ${core.role} · ${revealed ? voteName(outcome.votes[i]) : state === "sealed" && i === 2 ? "표 제출 대기" : phase[0]} · 상세 열기"><span class="core-name">${core.name}</span><span class="core-status"><span class="core-status-jp" aria-hidden="true" lang="ja">${revealed ? voteJapanese(outcome.votes[i]) : phase[1]}</span><span class="core-status-kr">${revealed ? voteName(outcome.votes[i]) : state === "sealed" && i === 2 ? "표 제출 대기" : phase[0]}</span></span><span class="core-meta"><span class="core-role">${core.role}</span><span class="core-open">${state === "input" ? "관점 열기" : "상세 열기"} ↗</span></span></button>`).join("")}<div class="topology-hub" aria-hidden="true"><div class="hub-name">MAGI</div><div class="hub-state">${revealed ? "3 / 3" : phase[2]}</div></div></div>`;
  }
  const voteCount = variant => {
    const votes = (outcomes[variant] || outcomes.default).votes;
    return `<p class="vote-count">찬성 ${votes.filter(v => v === "support").length} <span aria-hidden="true">/</span> 반대 ${votes.filter(v => v === "oppose").length} <span aria-hidden="true">/</span> 기권 ${votes.filter(v => v === "abstain").length}</p>`;
  };
  window.MagiScreens = {};
  window.MagiKit = { esc, button, action, panel, notice, field, select, page, cores, outcomes, phases, topology, voteCount };
})();
