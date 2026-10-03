(() => {
  "use strict";

  const S = window.MagiScreens;
  const K = window.MagiKit;
  const e = K.esc;
  const coreNames = ["BALTHASAR·2", "CASPER·3", "MELCHIOR·1"];
  const initialRoles = [
    { name: "지속성과 돌봄", purpose: "공개 이후 사람과 운영이 감당할 수 있는 선택을 찾는다.", criteria: "지원 책임, 운영 부담, 장기적인 영향", challenge: "지원 담당자 또는 중단 절차가 없을 때", language: "ko" },
    { name: "주체성과 대안", purpose: "참여자의 선택권과 놓친 대안을 검토한다.", criteria: "자발적 참여, 철회 가능성, 대안의 다양성", challenge: "참여자가 불이익 없이 철회할 수 없을 때", language: "ko" },
    { name: "근거와 실현 가능성", purpose: "자료의 근거와 실제 실행 조건을 확인한다.", criteria: "확인 가능한 사실, 구현 조건, 반증 가능성", challenge: "핵심 주장을 뒷받침하는 근거가 없을 때", language: "ko" },
  ];
  const sourceCatalog = [
    { id: "release", name: "release-plan.md", kind: "Markdown", size: "2.4 KB", hash: "73a9b1cf", status: "포함", locator: "1–12행", text: "# 제한 공개 계획\n\n목표: 실제 사용 환경에서 기능의 유용성을 확인한다.\n대상: 자발적으로 참여한 사용자 20명 이내.\n기간: 2주.\n지원 범위와 대상 인원을 제한한다.\n문의는 전담 채널로 접수한다.\n담당자가 하루 한 번 문의를 확인한다.\n문제가 확인되면 신규 참여를 중단한다.\n\n참여 철회 절차는 별도 검토한다.\n결과를 검토한 뒤 공개 범위를 결정한다.", delivered: "목표: 실제 사용 환경에서 기능의 유용성을 확인한다.\n대상: 자발적으로 참여한 사용자 20명 이내.\n기간: 2주.\n지원 범위와 대상 인원을 제한한다.\n문의는 전담 채널로 접수한다.\n참여 철회 절차는 별도 검토한다." },
    { id: "support", name: "support-scope.md", kind: "Markdown", size: "1.1 KB", hash: "ad71028f", status: "포함", locator: "전체 8행", text: "# 지원 범위\n\n담당: 제품 운영 담당자\n지원 시간: 평일 10:00–17:00\n응답 목표: 다음 영업일\n장애 접수는 전담 채널을 사용한다.\n자동 복구가 불가능하면 공개를 중단한다.\n문의 기록은 담당자만 열람한다." },
    { id: "feedback", name: "feedback.md", kind: "Markdown", size: "1.8 KB", hash: "c914e80a", status: "포함", locator: "전체 6행", text: "# 참여자 의견\n\n기능이 어떤 문제를 해결하는지 먼저 알고 싶습니다.\n문제가 생겼을 때 연락할 곳이 필요합니다.\n참여를 끝내는 방법을 쉽게 찾고 싶습니다.\n제한 공개가 끝난 뒤 결과를 알려주세요." },
    { id: "secret", name: ".env.local", kind: "환경 설정", size: "0.4 KB", status: "제외", reason: "비밀 후보 · 내용 수집 안 함" },
    { id: "large", name: "research-recording.mov", kind: "동영상", size: "124 MB", status: "제외", reason: "크기 초과 · 이 자료 형식은 지원하지 않음" },
    { id: "archive", name: "planning.key", kind: "Keynote", size: "4.2 MB", status: "제외", reason: "미지원 형식 · PDF 또는 텍스트로 변환 필요" },
    { id: "broken", name: "research-scan.pdf", kind: "PDF", size: "3.6 MB", status: "제외", reason: "추출 실패 · 암호화된 문서" },
    { id: "link", name: "linked-notes", kind: "심볼릭 링크", size: "—", status: "제외", reason: "선택 범위 밖 링크 · 대상 자동 접근 안 함" },
  ];
  let currentState;
  const clone = value => JSON.parse(JSON.stringify(value));
  function state(ctx) {
    currentState = ctx.formState;
    if (!currentState.setup) currentState.setup = { selected: ["release", "support", "feedback"], sourceId: "release", sourceView: "original", sourceRange: "selected", roles: clone(initialRoles), roleDraft: clone(initialRoles), roleDirty: false, assignments: ["personal", "personal", "personal"], providerName: "개인 ACP", providerModel: "선택한 프론티어 모델", profiles: [{ id: "personal", name: "개인 구독", path: "~/.codex", revision: 1 }], connectionReady: true, consent: false, sourceRevision: 1, roleRevision: 1 };
    return currentState.setup;
  }
  const variants = entries => entries.map(([id, label]) => ({ id, label }));
  const btn = (label, screen, variant = "default", tone = "") => K.button(label, screen, variant, tone);
  const act = (label, action, tone = "") => K.action(label, action, tone);
  const badge = (text, tone = "") => `<span class="status-pill ${tone ? `setup-status-${tone}` : ""}">${e(text)}</span>`;
  const fact = (label, value) => `<div class="setup-fact"><dt>${e(label)}</dt><dd>${e(value)}</dd></div>`;
  function field(label, id, value, { area = false, help = "", type = "text", required = false, path = "", min, max } = {}) {
    const attributes = `id="${e(id)}" name="${e(id)}"${path ? ` data-setup-field="${e(path)}"` : ""}${required ? " required" : ""}${help ? ` aria-describedby="${e(id)}-help"` : ""}`;
    return `<label class="field" for="${e(id)}"><span class="field-label">${e(label)}</span>${area ? `<textarea ${attributes} rows="3">${e(value ?? "")}</textarea>` : `<input ${attributes} type="${e(type)}" value="${type === "password" ? "" : e(value ?? "")}"${type === "password" ? ' data-sensitive="true" autocomplete="off"' : ""}${min !== undefined ? ` min="${min}"` : ""}${max !== undefined ? ` max="${max}"` : ""}>`}${help ? `<span class="field-help" id="${e(id)}-help">${e(help)}</span>` : ""}</label>`;
  }
  function select(label, id, value, options, path = "") {
    return `<label class="field" for="${e(id)}"><span class="field-label">${e(label)}</span><select id="${e(id)}" name="${e(id)}"${path ? ` data-setup-field="${e(path)}"` : ""}>${options.map(([v, text]) => `<option value="${e(v)}"${String(v) === String(value) ? " selected" : ""}>${e(text)}</option>`).join("")}</select></label>`;
  }
  function ledger(rows) { return `<dl class="setup-ledger">${rows.map(([a, b]) => fact(a, b)).join("")}</dl>`; }
  function aside(ctx, title, body, stateName = "input") {
    return `<div class="setup-core-map">${ctx.topology(stateName, true)}</div>${K.panel(title, body, "MAGI / THREE PERSPECTIVES")}`;
  }
  function page(ctx, { title, kicker, intro, body, side = "", actions = "" }) {
    state(ctx);
    return `<div class="setup-screen">${K.page({ title, kicker, intro, body, aside: side, actions })}</div>`;
  }
  function persistInput(target) {
    if (!currentState || !target.dataset.setupField || target.type === "password") return;
    const parts = target.dataset.setupField.split(".");
    let object = currentState.setup;
    for (const part of parts.slice(0, -1)) object = object[part];
    object[parts.at(-1)] = target.type === "checkbox" ? target.checked : target.value;
    if (parts[0] === "roleDraft") currentState.setup.roleDirty = true;
    if (["selected", "sourceRange", "roleDraft", "assignments"].includes(parts[0])) currentState.setup.consent = false;
  }
  document.addEventListener("input", event => persistInput(event.target));
  document.addEventListener("change", event => persistInput(event.target));

  const connectionStates = {
    default: ["연결 준비됨", "공급자 연결과 세 관점의 할당을 확인했습니다.", "success"],
    empty: ["연결된 제공자가 없습니다", "질문과 자료를 준비할 수 있습니다. 심의를 시작하려면 본인 연결을 추가하세요.", "info"],
    checking: ["필수 기능 확인", "실행 파일부터 모델까지 확인 단계를 순서대로 살펴봅니다.", "info"],
    auth: ["기존 구독 인증 확인 필요", "개인 ACP의 인증 상태를 확인할 수 없습니다. 연결을 확인한 뒤 직접 심의를 시작하세요.", "warning"],
    quota: ["제공자 사용 한도", "개인 ACP가 사용량 제한을 알렸습니다. 초기화 시각은 제공되지 않았습니다.", "warning"],
    unsupported: ["필수 기능 미지원", "범위 밖 파일 접근 차단과 호출 정지를 확인할 수 없어 이 연결로 시작할 수 없습니다.", "error"],
    missing: ["실행 파일을 찾을 수 없습니다", "연결에 지정한 에이전트 경로가 이동했거나 접근할 수 없습니다. 실행 파일을 다시 선택하세요.", "error"],
    policy: ["연결 조건 확인 필요", "이 배포 형태에서 사용할 수 있는 인증 경로와 제공자 조건이 확인되지 않았습니다.", "warning"],
  };
  S.connections = {
    title: "모델 연결", group: "설정", variants: variants(Object.entries(connectionStates).map(([id, a]) => [id, a[0]])),
    render(ctx) {
      const s = state(ctx), v = ctx.variant === "default" && !s.connectionReady ? "checking" : ctx.variant || "default", status = connectionStates[v] || connectionStates.default;
      if (v !== "default") s.connectionReady = false;
      const checks = ["실행 파일", "통신", "기존 구독 인증", "자료·도구 격리", "취소·정지 관측", "모델 확인"];
      const blocker = { missing: 0, auth: 2, unsupported: 3, policy: 2, checking: 3, empty: 0 }[v];
      const admission = `<ol class="setup-check-list">${checks.map((name, i) => `<li><span class="setup-check-number">${String(i + 1).padStart(2, "0")}</span><span>${name}</span>${badge(v === "empty" ? "연결 없음" : blocker === undefined || i < blocker ? "확인됨" : i === blocker ? (v === "checking" ? "확인 중" : "확인 필요") : "대기", blocker !== undefined && i >= blocker ? "pending" : "ready")}</li>`).join("")}</ol>`;
      const assignments = `<div class="setup-assignment-list">${coreNames.map((name, i) => `<div class="setup-assignment"><strong>${name}</strong>${select("할당할 연결", `setup-assignment-${i}`, s.assignments[i], [...s.profiles.map(p => [p.id, `${p.name} · ${s.providerModel}`]), ["none", "연결 선택 필요"]], `assignments.${i}`)}</div>`).join("")}</div>${act("할당 적용", "setup-assign", "primary")}`;
      const profiles = v === "empty" ? `<p>저장된 프로필이 없습니다. 이름과 기존 CLI 인증 홈 경로를 입력해 추가하십시오.</p>` : s.profiles.map(p => `<div class="setup-provider-head"><div><h3>${e(p.name)}</h3><p><code>${e(p.path)}</code> · revision ${p.revision}</p></div>${act("편집", "setup-profile-edit").replace('data-action="setup-profile-edit"', `data-action="setup-profile-edit" data-profile-id="${e(p.id)}"`)}</div>`).join("");
      return page(ctx, { kicker: "MODEL CONNECTIONS", title: "모델 연결", intro: "프로필·연결 확인·실제 모델·세 코어 할당을 한 화면에서 관리합니다.", body: `${K.notice(...status)}${K.panel("연결 프로필", profiles + `<div class="page-actions">${act("프로필 추가", "setup-profile-add", "primary")}${act("연결 확인", "setup-check-done")}</div>` + admission, "01 / PROFILES")}${K.panel("세 코어 모델 할당", assignments, "02 / CORE BINDING")}`, side: aside(ctx, "다음 실행에 적용", K.notice("같은 모델을 세 관점으로 사용", "모델 중복은 통계적 독립성이나 정확도 향상을 보장하지 않습니다. 진행 중 심의는 고정 입력을 유지합니다.")), actions: act("원래 화면으로 돌아가기", "setup-return-connections") });
    },
    actions: {
      "setup-profile-add": ctx => editProfile(ctx, false),
      "setup-profile-edit": ctx => editProfile(ctx, true),
      "setup-profile-save": ctx => saveProvider(ctx),
      "setup-return-connections": ctx => ctx.go(ctx.formState.connectionReturn?.screen || "input", ctx.formState.connectionReturn?.variant || "default", { returning: true }),
      "setup-assign": ctx => { const s = state(ctx); s.consent = false; ctx.notify(s.assignments.includes("none") ? "할당되지 않은 코어가 있습니다. 연결을 선택해 주세요." : "세 관점의 연결을 다음 실행에 적용했습니다."); ctx.go("connections", "default"); },
      "setup-check-done": ctx => { state(ctx).connectionReady = true; ctx.go("connections", "default"); },
      "setup-usage": ctx => show(ctx, "제공자 사용량", "남은 사용량과 초기화 시각은 제공되지 않았습니다. 연결이 회복되어도 심의는 자동 재개하지 않습니다."),
      "setup-policy": ctx => show(ctx, "연결 조건", "공식 인증 소유자, 앱 배포 형태의 허용 조건, 도구 격리와 취소 관측을 각각 확인합니다. 토큰을 추출하거나 다른 계정의 인증을 재사용하지 않습니다."),
    },
  };

  function selectedSources(s) {
    const sources = sourceCatalog.map(source => s.refreshed && source.id === "release" ? { ...source, text: `${source.text}\n참여자는 전담 채널에서 언제든 철회를 요청할 수 있다.`, delivered: `${source.delivered}\n참여자는 전담 채널에서 언제든 철회를 요청할 수 있다.`, hash: "38ba29ed", locator: "1–13행" } : source);
    if (s.pasted) sources.push({ id: "paste", name: s.pasteName || "붙여넣은 자료.txt", kind: "텍스트", size: `${s.pasted.length}자`, text: s.pasted, locator: "전체", status: "포함", hash: "직접 입력" });
    return sources.filter(source => s.selected.includes(source.id));
  }
  function sourceNavigation(s, excluded) {
    const rows = excluded ? [...selectedSources(s), ...sourceCatalog.filter(x => x.reason)] : selectedSources(s);
    return `<div class="setup-source-list" aria-label="자료 목록">${rows.map(source => `<button type="button" class="setup-source-row${s.sourceId === source.id ? " selected" : ""}" data-action="setup-source" data-source-id="${source.id}" aria-pressed="${s.sourceId === source.id}"><span class="setup-file-type">${e(source.kind)}</span><strong>${e(source.name)}</strong><span>${e(source.reason || `${source.size} · ${source.locator}`)}</span><span class="setup-source-state">${source.reason ? "제외" : "포함"}</span></button>`).join("")}</div>`;
  }
  S.intake = {
    title: "자료 접수", group: "입력", variants: variants([["default", "자료 확인"], ["empty", "자료 없음"], ["scanning", "수집 목록 확인"], ["permission", "접근 권한"], ["excluded", "제외 자료"], ["changed", "자료 변경"], ["paste", "붙여넣기"]]),
    render(ctx) {
      const s = state(ctx);
      let v = ctx.variant || "default";
      if (v === "empty") s.selected = [];
      if (v === "default" && !s.selected.length) v = "empty";
      const source = [...selectedSources(s), ...sourceCatalog].find(x => x.id === s.sourceId) || sourceCatalog[0];
      const notice = { scanning: ["수집 대상 확인", "선택 범위의 자료 목록을 확인합니다. 제외된 파일은 모델에 전달되지 않습니다."], permission: ["원문 접근 권한이 필요합니다", "보관한 수집본은 남아 있습니다. 현재 파일을 다시 읽으려면 자료를 재선택하세요.", "warning"], excluded: ["자료 5개가 제외되었습니다", "비밀 후보·크기 초과·미지원·추출 실패·범위 밖 링크를 각각 확인할 수 있습니다.", "warning"], changed: ["현재 파일이 수집본과 다릅니다", "기존 실행의 원문은 변경하지 않습니다. 새 수집본을 선택하면 입력 확인을 다시 받습니다.", "warning"] }[v];
      const catalog = `<fieldset class="setup-catalog"><legend>추가할 자료 선택</legend>${sourceCatalog.filter(x => !x.reason).map(x => `<label><input type="checkbox" name="setup-catalog" value="${x.id}"${s.selected.includes(x.id) ? " checked" : ""}><span><strong>${x.name}</strong><small>${x.kind} · ${x.size}</small></span></label>`).join("")}</fieldset>${act("선택한 자료 추가", "setup-add-sources", "primary")}`;
      const previewText = source.reason ? source.reason : s.sourceView === "delivered" && s.sourceRange === "selected" && source.delivered ? source.delivered : source.text;
      const preview = `<div class="setup-source-workspace">${sourceNavigation(s, v === "excluded")}<section class="setup-document" aria-label="자료 미리보기"><div class="setup-document-heading"><div><span class="panel-kicker">${source.reason ? "EXCLUDED" : "SOURCE INSPECTOR"}</span><h3>${e(source.name)}</h3></div>${badge(source.reason ? "제외" : s.sourceView === "delivered" ? "전달본" : "수집 원문")}</div>${!source.reason ? `<div class="setup-tabs">${act("수집 원문", "setup-view-original", s.sourceView === "original" ? "primary" : "")}${act("모델 전달본", "setup-view-delivered", s.sourceView === "delivered" ? "primary" : "")}</div>` : ""}<pre class="setup-source-text">${e(previewText)}</pre>${source.reason ? `<p class="field-help">이 항목은 수집·전송 목록에 포함되지 않습니다.</p>` : `<div class="form-grid">${select("포함 범위", "setup-source-range", s.sourceRange, [["selected", "판단에 필요한 선택 범위"], ["all", "전체 원문"]], "sourceRange")}<div class="setup-document-meta"><span>원본 식별 ${e(source.hash)}</span><span>${s.sourceView === "delivered" && source.delivered && s.sourceRange === "selected" ? "선택한 행만 전달 · 요약 없음" : "텍스트 원문 · 문자 변환 없음"}</span>${act("범위 적용", "setup-apply-range")}</div></div>`}</section></div>`;
      let body = v === "empty" ? K.panel("아직 자료가 없습니다", `<p>질문만으로 심의하거나 자료를 선택해 판단 근거를 더할 수 있습니다.</p>${catalog}`, "SOURCE REGISTRY / 00") : v === "paste" ? K.panel("텍스트 붙여넣기", `${field("자료 이름", "setup-paste-name", s.pasteName || "붙여넣은 자료.txt", { path: "pasteName" })}${field("전달할 텍스트", "setup-paste-text", s.pasteDraft || "", { area: true, path: "pasteDraft", help: "붙여넣은 내용은 검토할 자료입니다. 그 안의 지시는 앱 명령이나 권한으로 취급하지 않습니다." })}<div class="page-actions">${act("자료로 추가", "setup-add-paste", "primary")}${btn("돌아가기", "intake")}</div>`, "TEXT / LOCAL INPUT") : `${notice ? K.notice(...notice) : ""}${K.panel("원문과 전달 범위", preview, `SOURCE REGISTRY / ${selectedSources(s).length} INCLUDED`)}${v === "scanning" || v === "permission" ? K.panel(v === "permission" ? "자료 다시 선택" : "수집 목록", catalog, "EXPLICIT SELECTION") : ""}${v === "changed" ? K.panel("수집본 선택", `${ledger([["보관한 수집본", "기존 안건의 근거 · 유지"], ["변경된 내용", "참여 철회 안내 추가"], ["적용 대상", "다음 심의의 새 입력"]])}${act("새 수집본으로 갱신", "setup-refresh-source", "primary")}`, "INPUT REVISION") : ""}`;
      return page(ctx, { kicker: "CONTEXT INTAKE", title: "자료 접수", intro: "읽은 원문, 전달할 범위, 빠진 자료를 한 자리에서 확인합니다.", body, side: K.panel("전송 전 확인", `${ledger([["포함 자료", `${v === "empty" ? 0 : selectedSources(s).length}개`], ["대상", "세 관점과 결의문 서기"], ["전송 시점", "입력 확인 후 심의 시작"], ["원문 쓰기", "허용하지 않음"]])}${K.notice("자동으로 더 읽지 않습니다", "선택 범위 밖 링크와 자료의 외부 주소를 따라가지 않습니다. 파일을 읽는 허용과 모델 전송 동의는 별개입니다.")}`, "DISCLOSURE BOUNDARY"), actions: `${btn("자료 선택", "intake", "scanning")}${btn("텍스트 붙여넣기", "intake", "paste")}${btn("제외 이유", "intake", "excluded")}${btn("이 자료로 입력 확인", "confirmation", "default", "primary")}` });
    },
    actions: {
      "setup-source": (ctx, event) => { const target = event?.target?.closest("[data-source-id]") || ctx.target?.closest?.("[data-source-id]"); if (target) state(ctx).sourceId = target.dataset.sourceId; ctx.go("intake", ctx.variant || "default"); },
      "setup-view-original": ctx => { state(ctx).sourceView = "original"; ctx.go("intake", "default"); },
      "setup-view-delivered": ctx => { state(ctx).sourceView = "delivered"; ctx.go("intake", "default"); },
      "setup-apply-range": ctx => { state(ctx).consent = false; state(ctx).sourceView = "delivered"; ctx.go("intake", "default"); },
      "setup-add-sources": ctx => { const s = state(ctx); s.selected = Array.from(document.querySelectorAll('[name="setup-catalog"]:checked'), x => x.value); s.sourceId = s.selected[0] || "release"; s.consent = false; s.sourceRevision++; ctx.go("intake", s.selected.length ? "default" : "empty"); },
      "setup-add-paste": ctx => { const s = state(ctx); if (!s.pasteDraft?.trim()) { ctx.notify("전달할 텍스트를 입력해 주세요."); document.getElementById("setup-paste-text")?.focus(); return; } s.pasted = s.pasteDraft; s.selected = [...new Set([...s.selected, "paste"])]; s.sourceId = "paste"; s.consent = false; s.sourceRevision++; ctx.go("intake", "default"); },
      "setup-refresh-source": ctx => { const s = state(ctx); s.sourceRevision++; s.consent = false; s.sourceView = "original"; s.refreshed = true; ctx.notify("새 수집본을 다음 심의의 입력으로 선택했습니다."); ctx.go("confirmation", "changed"); },
    },
  };

  S.confirmation = {
    title: "고정 입력 확인", group: "입력", variants: variants([["default", "시작 준비"], ["blocked", "연결 차단"], ["changed", "입력 변경"], ["oversize", "문맥 한도 초과"]]),
    render(ctx) {
      const s = state(ctx), v = ctx.variant || "default";
      const notice = { blocked: ["연결 확인이 필요합니다", "필수 기능을 확인하지 못한 연결이 있습니다. 현재 입력을 보존하고 연결 화면에서 확인하세요.", "error"], changed: ["입력이 변경되어 동의를 다시 받습니다", "자료 또는 역할의 범위가 바뀌었습니다. 전달할 내용을 확인하고 다시 동의하세요.", "warning"], oversize: ["세 관점의 공통 문맥 한도를 초과합니다", "자동으로 내용을 잘라내지 않습니다. 자료 범위를 줄이거나 검증된 다른 모델을 선택하세요.", "error"] }[v];
      const roles = `<ol class="setup-role-summary">${s.roles.map((r, i) => `<li><strong>${coreNames[i]}</strong><div><h4>${e(r.name)}</h4><p>${e(r.purpose)}</p><small>${e(`${s.profiles.find(p => p.id === s.assignments[i])?.name || "미할당"} · ${s.providerModel}`)}</small></div></li>`).join("")}</ol>`;
      const sources = selectedSources(s);
      return page(ctx, { kicker: "INPUT CONFIRMATION", title: "이 입력으로 심의합니다", intro: "질문·자료·세 관점·전송 대상을 고정합니다. 이후 변경은 새 심의로 이어집니다.", body: `${notice ? K.notice(...notice) : ""}${K.panel("판단할 질문", `<blockquote class="setup-question">${e(ctx.question)}</blockquote>${ledger([["목표", "제한 공개의 조건과 미해결 사항 판단"], ["제약", "참여자 20명 · 2주 · 지정된 지원 범위"]])}${btn("질문 수정", "input")}`, "01 / AGENDA")}${K.panel("세 관점과 모델", roles + btn("역할 편집", "roles"), "02 / ROLE SET")}${K.panel("전달할 내용", `<div class="setup-disclosure"><h3>선택한 제공자의 클라우드</h3><p>질문, 역할 지침, 아래 자료, 공개된 검토 의견과 결의문이 단계별 입력으로 전달됩니다. 서로 다른 제공자를 선택하면 공개 평가가 다른 제공자에게도 전달됩니다.</p>${sources.length ? `<ul>${sources.map(x => `<li><strong>${e(x.name)}</strong><span>${e(s.sourceRange === "all" ? "전체 원문" : x.locator)} · ${e(x.kind)}</span></li>`).join("")}</ul>` : "<p>추가 자료 없음 · 질문과 역할만 사용합니다.</p>"}</div><div class="page-actions">${btn("원문·범위 다시 보기", "intake")}${btn("제공자 확인", "connections")}</div>`, "03 / DISCLOSURE")}`, side: aside(ctx, "호출과 사용량", `${ledger([["심의", "독립 3 → 교차 3 → 서기 1 → 표결 3"], ["기본 앱 턴 요청", "10회"], ["구조 교정 포함 상한", `최대 ${ctx.formState.budget || 20}회`], ["제공자 내부 추론", "요청 수와 다를 수 있음"], ["제공자 잔여 사용량", "제공되지 않음"]])}${K.notice("실패해도 자동 우회하지 않습니다", "다른 계정·유료 API로 바꾸지 않습니다. 중단한 심의는 같은 입력을 확인한 뒤 직접 재개합니다.")}`), actions: `<div class="setup-start"><label class="setup-consent"><input type="checkbox" id="setup-consent" data-setup-field="consent"${s.consent && v === "default" ? " checked" : ""}><span>위 질문·역할·자료 범위를 표시된 제공자에게 전달하는 데 동의합니다.</span></label>${v === "blocked" ? btn("연결 확인", "connections", "unsupported", "primary") : v === "oversize" ? btn("자료 범위 조정", "intake", "default", "primary") : `${act("이 입력으로 심의 시작", "setup-start-run", "primary")}<p class="field-help" id="setup-start-help">명시적으로 시작하기 전에는 모델을 호출하지 않습니다.</p>`}</div>` });
    },
    actions: {
      "setup-start-run": ctx => { const s = state(ctx); if (!document.getElementById("setup-consent")?.checked) { ctx.notify("전달 범위를 확인하고 동의해 주세요."); document.getElementById("setup-consent")?.focus(); return; } if (!ctx.question.trim()) { ctx.notify("판단할 질문을 먼저 입력하세요."); ctx.go("input", "empty"); return; } if (!s.connectionReady || s.assignments.includes("none")) { ctx.go("confirmation", "blocked"); return; } s.confirmedQuestion = ctx.question; s.confirmedSources = clone(selectedSources(s)); s.confirmedRoles = clone(s.roles); s.consent = false; ctx.go("independent", "default"); },
    },
  };

  function roleFields(s) {
    return s.roleDraft.map((role, i) => `<section class="setup-role-editor"><div class="setup-role-title"><span>${String([2, 3, 1][i]).padStart(2, "0")}</span><div><h3>${coreNames[i]}</h3><p>위치와 표의 수는 바뀌지 않습니다.</p></div></div><div class="form-grid">${field("관점 이름", `setup-role-${i}-name`, role.name, { path: `roleDraft.${i}.name`, required: true })}${select("답변 언어", `setup-role-${i}-language`, role.language, [["ko", "한국어"], ["en", "English"]], `roleDraft.${i}.language`)}${field("이 관점의 목적", `setup-role-${i}-purpose`, role.purpose, { area: true, path: `roleDraft.${i}.purpose`, required: true })}${field("우선하는 판단 기준", `setup-role-${i}-criteria`, role.criteria, { area: true, path: `roleDraft.${i}.criteria`, required: true })}${field("의문을 제기할 조건", `setup-role-${i}-challenge`, role.challenge, { area: true, path: `roleDraft.${i}.challenge`, required: true })}</div></section>`).join("");
  }
  S.roles = {
    title: "세 관점 편집", group: "설정", variants: variants([["default", "역할 편집"], ["invalid", "입력 오류"], ["unsaved", "저장하지 않은 변경"], ["import", "프리셋 가져오기"]]),
    render(ctx) {
      const s = state(ctx), v = ctx.variant || "default";
      const imported = JSON.stringify({ schema_version: 1, roles: initialRoles }, null, 2);
      const body = v === "import" ? K.panel("역할 프리셋", `${field("프리셋 JSON", "setup-role-import", s.importDraft ?? imported, { area: true, path: "importDraft", help: "정확히 세 역할의 이름·목적·기준·반증 조건·언어만 가져옵니다. 파일 권한·명령·외부 연결은 허용하지 않습니다." })}${s.importError ? K.notice("가져오기 거부", s.importError, "error") : ""}<div class="page-actions">${act("검증하고 가져오기", "setup-import-roles", "primary")}${btn("편집으로 돌아가기", "roles")}</div>`, "PRESET / DATA ONLY") : `${v === "invalid" ? K.notice("역할의 필수 항목을 입력하세요", s.roleError || "세 관점 모두 이름, 목적, 기준, 의문을 제기할 조건이 필요합니다.", "error") : ""}${v === "unsaved" ? K.panel("변경한 역할을 저장할까요?", `<p>저장 전 변경은 다음 실행에 반영되지 않습니다. 현재 진행 중인 심의는 고정한 역할을 유지합니다.</p><div class="page-actions">${act("변경 저장", "setup-save-roles", "primary")}${act("변경 버리고 돌아가기", "setup-discard-roles", "danger")}${btn("계속 편집", "roles")}</div>`, "UNSAVED CHANGES") : ""}<form id="setup-roles-form">${roleFields(s)}</form>`;
      return page(ctx, { kicker: "PERSONALITY CONFIGURATION", title: "세 관점 편집", intro: "같은 질문을 바라보는 기준을 정합니다. 관점이 권한이나 심의 규칙을 바꾸지는 않습니다.", body, side: aside(ctx, "다음 실행에 적용", `${ledger([["코어", "정확히 3개"], ["표결 권한", "각 코어 1표"], ["특별 거부권", "없음"], ["적용", "저장 후 시작하는 새 심의"]])}${K.notice("진행 중 입력은 고정됩니다", "역할을 저장해도 이미 진행 중인 심의의 관점이나 모델은 변경되지 않습니다.")}`), actions: `${act("역할 저장", "setup-save-roles", "primary")}${btn("프리셋 가져오기", "roles", "import")}${act("프리셋 내보내기", "setup-export-roles")}${act("돌아가기", "setup-leave-roles")}` });
    },
    actions: {
      "setup-save-roles": ctx => { if (!saveRoles(ctx)) { ctx.go("roles", "invalid"); return; } ctx.notify("역할을 저장했습니다. 다음 심의부터 적용됩니다."); ctx.go("roles", "default"); },
      "setup-leave-roles": ctx => ctx.go(state(ctx).roleDirty ? "roles" : "input", state(ctx).roleDirty ? "unsaved" : "default"),
      "setup-discard-roles": ctx => { const s = state(ctx); s.roleDraft = clone(s.roles); s.roleDirty = false; ctx.go("input", "default"); },
      "setup-import-roles": ctx => { const s = state(ctx); try { const value = JSON.parse(document.getElementById("setup-role-import").value); if (value.schema_version !== 1 || Object.keys(value).some(k => !["schema_version", "roles"].includes(k)) || !Array.isArray(value.roles) || value.roles.length !== 3) throw new Error("지원하는 버전의 역할 3개가 필요합니다."); const keys = ["name", "purpose", "criteria", "challenge", "language"]; value.roles.forEach(r => { if (typeof r !== "object" || r === null || Object.keys(r).some(k => !keys.includes(k)) || keys.some(k => typeof r[k] !== "string" || !r[k].trim()) || !["ko", "en"].includes(r.language)) throw new Error("역할 필드 또는 답변 언어가 올바르지 않습니다. 실행 명령·추가 권한 필드는 허용되지 않습니다."); }); s.roleDraft = clone(value.roles); s.roleDirty = true; s.importError = ""; ctx.notify("프리셋을 편집 초안으로 가져왔습니다. 저장하면 다음 실행에 적용됩니다."); ctx.go("roles", "default"); } catch (error) { s.importError = error instanceof SyntaxError ? "JSON 형식을 확인하세요. 기존 역할은 유지됩니다." : error.message; ctx.go("roles", "import"); } },
      "setup-export-roles": ctx => { const s = state(ctx); const text = JSON.stringify({ schema_version: 1, roles: s.roles }, null, 2); show(ctx, "역할 프리셋 내보내기", `<p>저장한 세 관점만 포함합니다. 모델 연결·자료·인증 정보는 포함하지 않습니다.</p><textarea class="setup-export-text" rows="12" readonly aria-label="내보낼 역할 프리셋">${e(text)}</textarea><div class="page-actions">${act("JSON 파일 저장", "setup-download-roles", "primary")}</div>`, true); },
      "setup-download-roles": ctx => { download("magi-role-preset.json", JSON.stringify({ schema_version: 1, roles: state(ctx).roles }, null, 2)); ctx.notify("역할 프리셋 JSON을 저장했습니다."); },
    },
  };

  S.settings = {
    title: "콘솔 설정", group: "설정", variants: variants([["default", "Command"], ["clear", "Clear"], ["reduced", "동작 줄임"], ["budget", "실행 한도"]]),
    render(ctx) {
      const s = state(ctx), prefs = ctx.formState;
      const revision = prefs.preferenceRevision || 0;
      if (!s.preferencesDraft || s.preferenceRevision !== revision) {
        s.preferencesDraft = { sound: prefs.sound ?? false, motion: prefs.motion || "full", theme: prefs.theme || "command", density: prefs.density || "normal", language: prefs.language || "ko", textScale: prefs.textScale || "100", budget: prefs.budget || "20" };
        s.preferenceRevision = revision;
      }
      const settings = s.preferencesDraft;
      const t = (ko, en) => prefs.language === "en" ? en : ko;
      const appearance = `<div class="form-grid">
        ${select(t("표현", "Appearance"), "setup-theme", settings.theme, [["command", "Command"], ["clear", "Clear"]], "preferencesDraft.theme")}
        ${select(t("동작", "Motion"), "setup-motion", settings.motion, [["full", t("전체 동작", "Full motion")], ["reduced", t("동작 줄임", "Reduced motion")], ["off", t("장식 동작 끔", "Decorative motion off")]], "preferencesDraft.motion")}
        ${select(t("음향", "Sound"), "setup-sound", String(settings.sound), [["false", t("소리 끔", "Sound off")], ["true", t("소리 켬", "Sound on")]], "preferencesDraft.sound")}
        ${select(t("정보 간격", "Density"), "setup-density", settings.density, [["normal", t("기본", "Normal")], ["comfortable", t("여유", "Comfortable")]], "preferencesDraft.density")}
        ${select(t("앱 언어", "Application language"), "setup-language", settings.language, [["ko", "한국어"], ["en", "English"]], "preferencesDraft.language")}
        ${select(t("본문 확대", "Text scale"), "setup-text-scale", settings.textScale, [["100", "100%"], ["125", "125%"], ["150", "150%"], ["200", "200%"]], "preferencesDraft.textScale")}
      </div><p class="field-help">${t("음향은 명령·의견 도착·표 공개에만 반응합니다. 글자마다 소리를 내거나 지난 알림을 반복하지 않습니다.", "Sounds accompany commands, assessments, and ballot disclosure. Text streaming and past events do not trigger repeated sounds.")}</p>`;
      const limits = `${field(t("앱 턴 요청 상한", "Maximum application turns"), "setup-budget", settings.budget, { type: "number", min: 10, max: 20, path: "preferencesDraft.budget", help: t("기본 사이클은 10회입니다. 구조 교정을 포함해 최대 20회로 제한합니다. 제공자 내부 추론 횟수와 구분합니다.", "A normal cycle uses 10 application turns, with up to 20 including format corrections. Provider internal inference may differ.") })}${ledger([[t("앱 전체 동시 호출", "Concurrent calls"), t("최대 3개", "Up to 3")], [t("한 심의 사이클", "Deliberation cycles"), "1"], [t("한도 변경 적용", "Budget applies to"), t("다음 심의부터", "New deliberations")], [t("제공자 잔여 토큰", "Provider token balance"), t("제공된 경우에만 표시", "Shown only when reported")]])}`;
      return page(ctx, {
        kicker: "CONSOLE PREFERENCES", title: t("콘솔 설정", "Console preferences"),
        intro: t("강한 형상과 읽기 편한 본문. 메인 콘솔과 상태 팝오버에 같은 설정을 적용합니다.", "A shared visual language for the command console and its status companion."),
        body: `${ctx.variant === "budget" ? K.notice(t("다음 심의에 적용되는 한도", "Budget for the next deliberation"), t("진행 중인 심의의 예산은 고정됩니다. 저장한 한도는 다음 입력 확인에서 표시됩니다.", "An active deliberation keeps its approved budget. New limits appear in the next input confirmation.")) : ""}<form id="setup-settings-form">${K.panel(t("화면과 동작", "Presentation"), appearance, "01 / PRESENTATION")}${K.panel(t("다음 심의의 실행 한도", "Limits for new deliberations"), limits, "02 / RUN LIMITS")}</form>`,
        side: aside(ctx, t("설정의 적용 범위", "Shared preferences"), `${K.notice(t("안건과 읽던 위치를 유지합니다", "Your agenda and reading position are preserved"), t("설정은 세 코어의 위치·표 의미·근거를 숨기거나 바꾸지 않습니다.", "Preferences preserve the core positions, ballot meaning, and supporting evidence."))}${ledger([["Command / Clear", t("동일한 기하와 의미", "Same topology and meaning")], [t("동작 줄임", "Reduced motion"), t("실제 상태는 즉시 표시", "State is shown immediately")], [t("앱 언어", "Application language"), t("답변 언어와 별도", "Separate from response language")], [t("메뉴 막대", "Menu bar"), t("같은 설정 공유", "Shared preferences")]])}`),
        actions: `${btn("모델 연결 관리", "connections")}${act("원래 화면으로 돌아가기", "setup-return-settings")}${act(t("설정 적용", "Apply preferences"), "setup-apply-settings", "primary")}${act(t("기본 설정 복원", "Restore defaults"), "setup-reset-settings")}${btn(t("콘솔로 돌아가기", "Return to console"), "input")}`,
      });
    },
    actions: {
      "setup-return-settings": ctx => ctx.go(ctx.formState.settingsReturn?.screen || "input", ctx.formState.settingsReturn?.variant || "default", { returning: true }),
      "setup-apply-settings": ctx => applySettings(ctx, false),
      "setup-reset-settings": ctx => applySettings(ctx, true),
    },
  };

  function show(ctx, title, body, html = false) {
    ctx.openDialog(title, html ? body : `<p>${e(body)}</p>`);
  }
  function editProfile(ctx, editing) {
    const s = state(ctx), profile = editing ? s.profiles.find(p => p.id === ctx.target?.dataset.profileId) || s.profiles[0] : null;
    s.providerDraft = profile ? { ...profile } : { id: null, name: "", path: "", revision: 0 };
    ctx.openDialog(profile ? "연결 프로필 편집" : "연결 프로필 추가", `<form id="setup-provider-form"><p>이름과 기존 CLI 구독 인증 홈 경로를 직접 입력합니다.</p>${field("프로필 이름", "setup-provider-name", s.providerDraft.name, { required: true, path: "providerDraft.name" })}${field("인증 홈 경로", "setup-provider-path", s.providerDraft.path, { required: true, path: "providerDraft.path", help: "홈 내부 경로는 ~/.codex처럼 표시합니다." })}<p>어댑터 · Codex ACP${editing ? " · 변경 불가" : ""}</p></form>`, [{ label: "취소", action: "close-dialog" }, { label: "프로필 저장", action: "setup-profile-save", primary: true }]);
    document.getElementById("setup-provider-form")?.addEventListener("submit", event => { event.preventDefault(); saveProvider(ctx); });
    document.getElementById("setup-provider-name")?.focus();
  }
  function saveProvider(ctx) {
    const s = state(ctx), form = document.getElementById("setup-provider-form");
    if (!form.reportValidity()) return;
    const draft = s.providerDraft;
    if (!draft.name.trim() || !/^(~\/|\/)/.test(draft.path.trim())) {
      const field = document.getElementById("setup-provider-path");
      field.setCustomValidity("절대 경로나 ~/로 시작하는 인증 홈 경로를 입력하십시오."); field.reportValidity(); field.setCustomValidity(""); return;
    }
    const profile = { ...draft, id: draft.id || `profile-${s.profiles.length + 1}`, name: draft.name.trim(), path: draft.path.trim(), revision: draft.revision + 1 };
    const index = s.profiles.findIndex(p => p.id === profile.id);
    if (index < 0) s.profiles.push(profile); else s.profiles[index] = profile;
    s.providerName = profile.name; s.connectionReady = false; s.consent = false;
    ctx.closeDialog(() => { ctx.rerender(); document.querySelector('[data-action="setup-profile-add"]')?.focus(); });
    ctx.notify("프로필을 저장했습니다. 연결 확인이 필요합니다.");
  }
  function download(name, content) {
    const url = URL.createObjectURL(new Blob([content], { type: "application/json" }));
    const link = document.createElement("a"); link.href = url; link.download = name; link.click();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
  }
  function applySettings(ctx, reset) {
    const s = state(ctx);
    const draft = reset ? { sound: false, motion: "full", theme: "command", density: "normal", language: "ko", textScale: "100", budget: "20" } : s.preferencesDraft;
    const budget = Number(draft.budget);
    if (!Number.isInteger(budget) || budget < 10 || budget > 20) { ctx.notify("앱 턴 요청 상한은 10–20 사이 정수로 입력하세요."); document.getElementById("setup-budget")?.focus(); return; }
    const values = { ...draft, sound: draft.sound === true || draft.sound === "true", budget };
    Object.assign(ctx.formState, values);
    s.preferencesDraft = { ...values };
    if (ctx.applyPreferences) ctx.applyPreferences(values);
    ctx.notify(reset ? "콘솔 기본 설정을 복원했습니다." : "화면과 다음 심의의 한도를 적용했습니다.");
    ctx.go("settings", values.theme === "clear" ? "clear" : values.motion !== "full" ? "reduced" : "default");
  }
  function saveRoles(ctx) {
    const s = state(ctx);
    const bad = s.roleDraft.findIndex(role => ["name", "purpose", "criteria", "challenge"].some(key => !role[key]?.trim()));
    if (bad !== -1) { s.roleError = `${coreNames[bad]}의 비어 있는 필수 항목을 입력하세요.`; return false; }
    s.roles = clone(s.roleDraft);
    s.roleDirty = false;
    s.roleError = "";
    s.roleRevision++;
    s.consent = false;
    return true;
  }
  window.MagiSetup = { state, selectedSources, saveRoles };
})();
