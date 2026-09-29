# 설계 근거와 적용 범위

이 문서는 외부 자료가 뒷받침하는 사실과 프로젝트가 선택한 설계를 구분한다. 아래 링크의 최신 내용은 공급자 연결·의존성 갱신·배포 시 해당 버전과 함께 확인한다. 출처가 존재한다는 사실만으로 구현 적합성을 증명하지 않는다.

## 1. 제품 문서의 설계 원칙

| 설계 원칙 | 이 제품에서 정의하는 계약 |
|---|---|
| 제품·연동·보안·디자인 시스템이 각자 계약의 권위를 소유한다. 디자인 토큰과 실제 렌더 증거도 구분한다. | [문서 소유권](../README.md#설계-정본), [디자인 시스템](DESIGN_SYSTEM.md) |
| 명령 수락, 관찰된 상태, 외부 효과를 구분한다. 재시도와 복구는 멱등성·세대 검증으로 안전하게 만든다. | [아키텍처](ARCHITECTURE.md), [운영](OPERATIONS.md) |
| 화면별로 사용자에게 맞는 언어와 안내를 제공하고 연결 확인·늦은 응답·한글 입력·창 reflow를 일관되게 처리한다. | [콘솔 화면](UI_UX.md) |
| 규범 스냅샷·설계 결정·실행 증거의 역할을 분리하고 사용자 흐름을 수용 시나리오에 연결한다. | [제품 설계](DESIGN.md), [수용 시나리오](SCENARIOS.md) |

각 링크는 이 제품이 해당 계약을 정의하는 정본을 가리킨다.

<a id="related-work"></a>
## 2. 관련 프로젝트와 연구

| 자료 | 확인되는 접근 | 이 설계에 주는 의미 |
|---|---|---|
| [lordpba/AI_Magi](https://github.com/lordpba/AI_Magi) | 역할별 에이전트, 답변 취합, Gradio·CrewAI 기반 인터페이스, API·로컬 모델 경로 | 세 역할과 합성만으로 차별성을 주장할 근거가 되지 않음 |
| [fshiori/magi](https://github.com/fshiori/magi) | 다중 모델의 교차 비판, 이견 보존, NERV풍 화면과 실행 기록·재생 | 팬 콘솔과 반대 의견도 이미 존재하는 접근; 실제 로컬 자료 흐름과 계약의 완성도로 비교 |
| [Improving Factuality and Reasoning in Language Models through Multiagent Debate](https://arxiv.org/abs/2305.14325) | 연구에서 정한 과제·모델·설정에서 다중 모델 토론을 평가 | 교차 검토를 탐구할 근거; 임의의 모델 조합·현실 문제에 보편적 정확도 향상을 보장하지 않음 |

별 개수와 README의 성능 수치는 제품 품질의 독립 평가가 아니다. 기존 프로젝트의 인지도 원인을 코드 검증이나 사용자 조사 없이 단정하지 않는다. 여기서 채택하는 차별화 방향은 **허용한 로컬 원문 → 검토 가능한 주장 → 동일 문안 표결 → 이견과 출처를 가진 결의**를 하나의 강한 데스크톱 경험으로 연결하는 것이다. 이는 설계 가설이며 [사용자 관찰과 평가](SCENARIOS.md#acceptance)로 확인한다.

## 3. 통신·실행·저장

| 공식 자료 | 적용 계약 |
|---|---|
| [ACP 개요](https://agentclientprotocol.com/protocol/v1/overview), [초기화](https://agentclientprotocol.com/protocol/v1/initialization), [세션 설정](https://agentclientprotocol.com/protocol/v1/session-setup) | 프로토콜·기능을 협상하고 실제 지원 기능으로 연결을 허용한다. 작업 디렉터리나 프롬프트를 OS 격리로 간주하지 않는다 |
| [Codex ACP v1.13.1 초기화 구현](https://github.com/agentclientprotocol/codex-acp/blob/v1.13.1/src/CodexAcpClient.ts#L2420-L2450), [Codex 설정 loader](https://github.com/openai/codex/blob/main/codex-rs/config/src/loader/mod.rs) | Codex ACP 초기화 응답은 유효 home 경로를 보고하지 않는다. Codex 런타임은 `CODEX_HOME` 환경변수로 설정 루트를 선택하므로 앱은 이를 자체 실행 경계에서 지정·검증한다 |
| [Claude Code 법률·준수 문서](https://code.claude.com/docs/en/legal-and-compliance), [Agent SDK 개요](https://code.claude.com/docs/en/agent-sdk/overview) | 공식 클라이언트·사용자 인증·제삼자 앱 인증의 조건을 경로별로 확인한다. ACP를 지원한다는 이유만으로 구독 인증 재사용이 허용된다고 단정하지 않는다 |
| [Tauri 프로세스 모델](https://v2.tauri.app/concept/process-model/), [Capabilities](https://v2.tauri.app/security/capabilities/) | Rust host가 권한과 상태를 소유하고 WebView는 좁은 명령 인터페이스를 사용한다. Tauri 권한 설정이 외부 에이전트 전체를 격리한다는 추론은 하지 않는다 |
| [SQLite 원자적 커밋](https://www.sqlite.org/atomiccommit.html), [synchronous](https://www.sqlite.org/pragma.html#pragma_synchronous), [백업 API](https://www.sqlite.org/backup.html) | 로컬 단일 쓰기 권위, 영속 이벤트와 상태의 원자적 갱신, 일관된 백업을 설계한다 |
| [macOS 서명](https://v2.tauri.app/distribute/sign/macos/), [Tauri updater](https://v2.tauri.app/plugin/updater/) | 앱 코드 서명·공증·업데이트 서명을 구분하고 각 검증 결과를 배포 근거로 보관한다 |

공식 프로토콜·프레임워크 문서는 가능한 기능을 정의한다. 특정 바이너리의 모델 선택, 도구 차단, 인증, 취소, 사용량 보고를 보증하지 않는다. 연결 자격은 [adapter 계약](ARCHITECTURE.md)과 해당 경로의 실행 증거로 판정한다.

<a id="canonical-json"></a>
## 4. 내용 식별

[RFC 8785: JSON Canonicalization Scheme](https://www.rfc-editor.org/rfc/rfc8785)는 JSON의 결정적 직렬화를 정의한다. 결의문은 이 직렬화와 SHA-256으로 내용 식별자를 만든다. 어떤 필드를 포함하는지는 [ProposalSnapshot](DELIBERATION.md#outputs)이 소유한다. hash가 같다는 것은 같은 표결 대상을 가리킨다는 뜻이며 주장이 참이라는 증거는 아니다.

## 5. 화면·접근성·원작 시각 참고

| 자료 | 적용 범위 |
|---|---|
| [WCAG 2.2](https://www.w3.org/TR/WCAG22/) | Level AA를 웹 렌더링 콘텐츠의 접근성 기준으로 적용하고 macOS 키보드·VoiceOver 동작을 함께 확인 |
| [DTCG Format Module 2025.10](https://www.designtokens.org/tr/2025.10/format/) | primitive → semantic → component 토큰의 형식·별칭에 참고. W3C Recommendation과 구분되는 Community Group Report |
| [WAI-ARIA 모달 대화상자 패턴](https://www.w3.org/WAI/ARIA/apg/patterns/dialog-modal/) | 이름·초점 진입·내부 순환·닫은 뒤 복귀 동작 |
| [Apple MenuBarExtra](https://developer.apple.com/documentation/swiftui/menubarextra), [Tauri System Tray](https://v2.tauri.app/learn/system-tray/) | 메뉴 막대 항목과 팝오버형 표현의 기술 선택. SwiftUI의 window 스타일과 Tauri의 tray API를 구분하고 Tauri의 풍부한 상태 창은 별도 구성 |
| [Norton × Evangelion 공식 협업 페이지](https://japan.norton.com/evangelion_cp/) | 공개된 MAGI 장면의 삼각 배치·상태 색면·기계 표제에 대한 시각 참고. 이미지의 복제·재배포 허가로 사용하지 않음 |

서체의 출처와 라이선스는 [디자인 시스템](DESIGN_SYSTEM.md)이 소유한다. [콘솔 레이아웃 검토](visuals/console/index.html)는 공통 셸·평면 도형·조작 위치를 확인하는 HTML·SVG 자료다. 모델·권한·영속 실행의 증거를 대신하지 않는다.

<a id="rights"></a>
## 6. 코드 라이선스와 팬 표현

[MIT License](https://opensource.org/license/mit)는 코드 재사용 조건의 근거다. [OSI FAQ](https://opensource.org/faq)는 오픈소스와 상업적 사용의 관계를 설명한다. 앱을 무료로 제공하는 정책은 MIT 코드의 상업 재사용을 금지하지 않는다.

[Khara 팬 창작 가이드라인](https://www.khara.co.jp/guideline/)은 팬 창작의 조건과 다루는 공개 형식을 제시한다. 무료라는 사실만으로 모든 소프트웨어·명칭·자산 사용이 허용된다고 확대 해석하지 않는다. 앱 배포에 적용하는 자산별 확인은 [제품의 배포 계약](DESIGN.md#distribution)과 [권리 고지](../NOTICE.md)를 따른다.
