# 로컬 자료·권한·공급자 보안 계약

이 문서는 원문 접근, 공급자 공개, 에이전트 실행, 비밀과 내보내기의 신뢰 경계를 정의한다. 상태·저장은 [실행 구조](ARCHITECTURE.md), 운영 한도·삭제·복구는 [운영](OPERATIONS.md)을 따른다.

## 1. 신뢰 모델

사용자 입력, 원문 파일, 가져온 역할 preset, LLM 출력과 도구 결과는 비신뢰 데이터다. Coordinator의 결정적 권한 검사와 도메인 검증을 통과해야 제품 상태에 반영한다. 모델이 읽은 문서의 지시나 다른 코어의 의견은 사용자 승인으로 승격되지 않는다.

신뢰 기반은 서명·버전이 검증된 앱 코어, 승인한 adapter와 공식 agent 실행물, OS 자격증명 저장소다. 같은 OS 사용자 권한으로 실행되는 악성 프로그램·장치 관리자·침해된 공급자 런타임까지 방어한다고 주장하지 않는다. 프로세스 분리는 실패 경계이며 그 자체가 파일·네트워크 sandbox는 아니다.

| 자원·행위 | 권한 주체 | 실제 검사 위치 |
|---|---|---|
| 선택한 로컬 자료 읽기 | 사용자 Source Grant | Reader의 파일 열기 직전 |
| 모델에 내용 전송 | ContextManifest에 묶인 Disclosure Grant | dispatch 직전 |
| manifest 안의 추가 근거 읽기 | 해당 Run·슬롯의 Context Reader Grant | host tool 매 호출 |
| 모델 세션·프로세스 시작 | 검증된 binding + 사용자 심의 시작 | Coordinator·adapter |
| 결의·근거 export | 사용자 선택 대상·내용·저장 위치 | Exporter commit 직전 |
| 원래 프로젝트 수정·임의 shell 실행 | 심의 기능에 부여하지 않음 | 미지원 capability로 거부 |

<a id="source-access"></a>
## 2. 원문 파일 접근

Source Grant는 사용자 선택을 통해 생성하며 허용된 파일 또는 디렉터리 정체성, 읽기 동작, Run 범위, 생성 시각, 철회 상태를 가진다. macOS 파일 선택·drag-and-drop에서 받은 경로를 문자열 prefix 검사만으로 신뢰하지 않는다. 재연결할 수 없는 이동·권한 변경은 재선택을 요구한다.

Reader는 realpath·파일 정체성·symlink·mount 경계를 확인한다. 가능한 handle-relative 열기로 확인한 파일을 직접 읽고, 경로 교체로 허용 범위를 벗어나는 접근을 막는다. symlink 대상이 선택 범위 밖이면 별도 선택 없이는 따라가지 않는다. device·socket·FIFO·실행 가능한 플러그인·압축 archive의 자동 실행을 지원하지 않는다.

폴더를 선택하면 캡처 대상 목록과 제외 이유를 보여준다. 숨김 credential 파일, 환경 비밀 파일, SSH·브라우저·OS 자격증명 경로는 자동 수집하지 않는다. 비밀 패턴 검사는 보조 검사이며 탐지 실패 가능성을 설명하고 전송할 실제 표현을 검토할 수 있게 한다. 파일을 읽었다는 사실이 provider 전송 허용이 되지 않는다.

Source Grant 철회는 이후 읽기·새 캡처를 막는다. 이미 만든 원문 snapshot은 별도 보존 자원이며 삭제를 선택할 수 있다. source grant를 철회했다고 이미 외부 모델에 보낸 내용을 회수했다고 표시하지 않는다. 문맥 삭제의 처리와 기록에 남는 결손은 [보존](OPERATIONS.md#retention)을 따른다.

## 3. 추출과 문맥

PDF·이미지·복잡한 문서 파서는 자원 제한이 있는 별도 worker에서 동작한다. 파일에 포함된 스크립트·매크로·외부 링크·원격 글꼴을 실행하거나 자동 조회하지 않는다. 파서가 만든 텍스트도 신뢰 등급이 올라가지 않는다. worker timeout·메모리·압축 해제·이미지 픽셀 한도를 넘으면 실패 항목을 manifest에 기록한다.

파일의 자연어 지시, `AGENTS.md`, `CLAUDE.md`, 시스템 프롬프트처럼 보이는 문자열은 해당 자료의 내용이다. 역할·권한·공급자 설정을 바꾸는 명령으로 실행하지 않는다. 추출 컨텍스트는 원문 provenance를 포함한 데이터 영역으로 전달하고 사용자 질문·제품 정책·역할 지침과 구분한다.

인용 locator가 승인된 객체·범위에 속하는지는 결정적으로 확인한다. 문서에 실제로 존재하는 문장이 사실이라는 판정은 별도의 의미 검토다. 세 모델이 같은 잘못된 자료를 읽고 같은 결론을 냈다는 이유로 출처의 신뢰도를 올리지 않는다.

## 4. 외부 추론과 공개 동의

로컬에서 파일을 읽고 보관하는 것과 모델 추론 위치는 별개다. 시작 화면은 각 코어와 서기의 실제 공급자, 선택 model ID, cloud/local 실행 구분, 전송하는 source 표현·범위, 이미 외부로 전달된 여부를 표시한다. cloud 연결을 로컬 추론 또는 완전 오프라인으로 표시하지 않는다.

Disclosure Grant는 Run, ContextManifest digest, 공급자·계정 profile, 포함 콘텐츠 종류, 유효 기간과 호출 예산에 묶인다. 같은 동의가 별도 계정·새 공급자·추가 자료·다른 대화에 적용되지 않는다. 실제 dispatch 직전에 현재 철회·binding·digest를 다시 확인한다. 미래의 모든 파일을 포함하는 포괄 동의를 기본값으로 만들지 않는다.

공개 목록은 원문뿐 아니라 질문·역할·선택한 이전 기록·교차 검토에 공유되는 중간 평가·서기 제안도 포함한다. 서로 다른 공급자를 배정하면 한 공급자에서 생성된 공개 평가가 다른 공급자의 입력으로 전달된다는 사실을 최초 확인에 표시한다. 사용자 동의는 단계별 콘텐츠 종류와 수신자를 고정하며, 새로운 수신자를 추가하는 변경은 기존 동의로 처리하지 않는다.

업데이트 조회·진단 전송과 추론 공개는 다른 동작이다. 원문·질문·답변·파일명·역할 내용을 앱 운영 서버에 보내지 않는다. 공급자 요청은 선택한 공급자에게 직접 전달되며 앱 개발자의 LLM 게이트웨이를 경유하지 않는다. 오류 신고는 사용자가 검토한 명시적 진단 묶음만 전달한다.

동의 철회는 새 dispatch를 차단하고 진행 중 요청의 취소를 시도한다. 이미 전송된 prompt·응답의 보존·학습 사용·삭제 조건은 해당 공급자와 계정 정책에 따른다. 앱의 기록 삭제나 프로세스 종료가 공급자 삭제 또는 과금 취소를 보장하지 않는다.

<a id="agent-isolation"></a>
## 5. 로컬 agent의 실제 격리

ACP의 파일·terminal capability는 클라이언트가 제공하는 기능의 협상이다. 이를 광고하지 않아도 agent에 자체 shell·파일 도구가 있으면 그 도구가 자동 제거되는 것은 아니다. [ACP session의 작업 디렉터리](https://agentclientprotocol.com/protocol/v1/session-setup)는 세션 문맥이며 OS sandbox의 증거로 쓰지 않는다.

심의 admission에는 다음 조건이 모두 필요하다.

1. agent가 원문 디렉터리 대신 앱 소유의 빈 작업 디렉터리에서 시작한다.
2. 세 코어의 세션·메모리·임시 파일이 분리되고 다른 코어의 1차 결과를 읽을 수 없다.
3. 모델이 사용할 수 있는 자료 접근은 고정 manifest의 host Context Reader에 한정한다.
4. native shell·원문 쓰기·임의 network tool·범위 밖 파일 읽기를 검증된 공급자 도구 정책 또는 OS 경계로 차단한다.
5. 전역 instruction·memory·hook·plugin·자동 MCP 설정이 다른 자료·명령·권한을 들여오지 못한다.
6. 앱이 요청한 취소·프로세스 정지와 세대 폐기를 관측할 수 있다.

adapter는 각 조건의 집행 위치를 `host_enforced`, `provider_enforced`, `os_enforced`로 기록한다. 프롬프트만으로 제한하는 상태는 admission을 충족하지 못한다. provider 정책의 신뢰 경계와 OS sandbox의 보호 범위는 같은 배지로 표시하지 않는다. 요구 조건이 미확인·미지원이면 해당 binding을 실행하지 않는다.

Tauri capability는 WebView의 native 명령 접근을 제한한다. [공식 capability 설명](https://v2.tauri.app/security/capabilities/)에 맞춰 창별 명령을 명시적으로 등록하고, 모든 창에 임의 fs·shell·HTTP 권한을 합쳐 주지 않는다. 이 설정이 외부 agent 프로세스에 sandbox를 씌운다고 설명하지 않는다.

agent의 공식 인증 런타임이 provider credential store를 사용하는 것과 모델의 파일 도구가 비밀을 읽는 것은 구분한다. 모델 도구에서 credential store·환경 비밀·다른 앱 문맥 접근을 차단할 수 없는 조합은 사용할 수 없다. agent 또는 adapter 버전이 바뀌면 기존 호환성 근거를 자동 승계하지 않는다.

## 6. Host Context Reader

Context Reader는 필요한 경우 세션 한정 stdio MCP로 노출하며 `list_sources`, `read_evidence`, `resolve_citation` 같은 읽기 계약만 가진다. capability는 Run·role·slot·generation·manifest에 묶이며 현재 stage의 공개 경계를 매번 검사한다. 문자열 경로를 인수로 받지 않고 승인된 source ID와 locator를 받는다.

초기 심의에서는 다른 코어의 출력 ID가 알려져도 읽을 수 없다. 교차 검토의 공개 snapshot은 Coordinator가 세 결과를 검증한 뒤 발행한다. 서기는 공개된 평가와 manifest만 받고 개인 세션·credential·숨은 추론에 접근하지 않는다. 표결 세션은 다른 코어의 비공개 Ballot을 읽을 수 없다.

`readOnlyHint`·`destructiveHint` 같은 도구 메타데이터는 안내이며 보안 제어가 아니다. authorization은 host의 현재 grant·fence·manifest·범위 검사로 결정한다. 알 수 없는 source ID·범위 초과·재사용된 generation·철회된 grant는 거부하고 내용이 없는 오류를 반환한다.

ACP permission 요청도 같은 정책에 연결한다. 이미 승인된 manifest 읽기는 해당 슬롯 범위 안에서 처리하고 미등록 도구·shell·쓰기·범위 확대는 거부한다. 자료 부족에 해당하면 `paused(needs_input)`으로 표시한다. permission 대화에서 광범위한 상시 허용을 선택해 제품 정책을 우회하지 않는다. 사용자의 자료 추가는 새 manifest와 Run 생성 경로를 따른다.

<a id="provider-auth"></a>
## 7. 공급자 인증과 비밀

사용자는 자기 공급자의 공식 인증 흐름으로 로그인하거나 자기 API 키를 등록한다. 구독 로그인은 지원되는 공식 agent가 소유하고 OAuth token의 저장·갱신도 그 런타임에 둔다. 앱이 브라우저 cookie·구독 access/refresh token을 추출·복제하거나 별도 API 호출에 전용하지 않는다.

각 `ProviderProfile`은 앱이 관리하는 별도의 빈 home/config 루트에서 실행한다. 같은 공급자와 실행 파일도 계정 프로필마다 다른 루트를 사용한다. Codex ACP는 프로필 루트를 실행 환경의 `CODEX_HOME`과 `HOME`으로 지정한다. ACP 초기화 응답은 home 경로를 attestation하지 않으므로 경로 증거로 사용하지 않는다. 앱은 지정한 실행 환경과 OS sandbox 정책을 확인해 선택 프로필의 파일 경계를 강제한다. ACP가 일반 프로필 경로를 표준화한다고 가정하지 않는다.

전역 `$HOME`·`~/.codex`·다른 `ProviderProfile`을 추론하거나 fallback으로 사용하지 않는다. 기존 전역 프로필 파일·인증 정보를 앱 루트로 복사하지 않는다. 루트가 없거나 접근 불가·adapter 미지원·실행 환경 또는 OS sandbox 검증 실패면 해당 프로필은 실행 불가다. profile revision에 루트 식별을 묶고, 경로 변경은 연결과 binding 재검증을 요구한다. UI·로그·진단·내보내기에 절대 경로와 인증 데이터를 포함하지 않는다.

BYOK 키와 앱이 발급한 내부 비밀은 macOS Keychain에 보관한다. DB·설정·로그·역할 preset에는 secret reference만 둔다. renderer는 저장한 키를 다시 읽을 수 없고 등록 화면의 일시 입력을 native 저장 경계로 전달한 뒤 제거한다. 키 원문을 URL·argv·Markdown export·오류 메시지에 넣지 않는다.

공급자 profile은 개인·회사 등 사용자가 구분한 계정별로 분리한다. 프로세스에는 필요한 환경 변수 allowlist와 인증 설정만 전달하며 앱의 전체 환경을 상속하지 않는다. 인증 실패 시 다른 profile을 자동 선택하지 않는다. 앱 연결 해제는 앱의 세션·권한을 폐기하며 다른 앱이 사용하는 공급자 로그인 자체를 자동 삭제하지 않는다.

BYOK 요청 목적지는 검증한 공급자 profile의 HTTPS endpoint로 고정하고 TLS 검증을 생략하지 않는다. model 출력·원문 URL이 endpoint나 인증 헤더를 바꿀 수 없다. 자격증명을 다른 origin으로 redirect해 보내지 않는다. local 추론 endpoint는 별도로 검증된 local binding만 사용할 수 있으며 화면에 cloud 연결과 구분한다.

공급자 조건과 기술 호환성은 별도 확인이다. Claude의 공식 문서는 사용자 본인이 수정되지 않은 공식 binary에 로그인하는 경로와 제3자 앱의 로그인·credential 중개를 구분한다. 실제 배포 형태는 [Claude Code 조건](https://code.claude.com/docs/en/legal-and-compliance)과 [Agent SDK 인증 조건](https://code.claude.com/docs/en/agent-sdk/overview)을 함께 충족해야 한다. 특정 adapter가 작동한다는 사실만으로 제품 배포의 허용을 선언하지 않는다.

지원 binding은 조건 검토 근거·검토 일시·적용 배포 형태·인증 소유자와 적합성 증거를 가진다. 확인할 수 없는 경로는 비활성 상태와 이유를 표시한다. 사용량·가격·무료 티어·구독 포함 여부는 공급자의 실제 계정 정책을 따르며 정적 제품 문구로 무제한을 약속하지 않는다.

## 8. 화면·링크·내보내기

외부 HTML·SVG·스크립트를 콘솔에 직접 주입하지 않는다. 원문·모델 출력은 plain text 또는 허용된 Markdown 노드로 렌더링하며 raw HTML·JavaScript URL·iframe·원격 이미지 자동 로딩을 차단한다. 출처 클릭은 host가 해석한 snapshot viewer로 연결한다. 외부 링크는 주소를 드러내고 명시적 사용자 동작으로 시스템 브라우저에서 연다.

MAGI 연출의 성공·거부·경고 색상은 Coordinator의 검증된 상태에만 반응한다. 모델 텍스트의 `APPROVED`, ANSI escape, 가짜 시스템 메시지로 결의 배지·권한 화면을 바꿀 수 없다. 앱의 확인 UI는 원문·모델 생성 텍스트 영역과 시각·접근성 계층에서 구분한다.

내보내기는 종류·Run·포함 source·본문·개인 경로 제거 여부·대상 파일을 미리 보여준다. 결의 문서, 원문 포함 bundle, 공유용 화면은 각각 다른 공개 범위다. 공유용 화면은 원문과 로컬 경로를 기본으로 포함하지 않으며 사용자가 선택한 내용만 넣는다. 모델은 export 명령의 승인 주체가 아니다.

export는 사용자 지정 경로에서 기존 파일 존재·정체성을 확인하고 temp 파일을 완성한 뒤 원자 교체한다. 이미 존재하는 파일 덮어쓰기는 정확한 대상에 대한 승인을 요구한다. 실패한 export가 기존 파일을 절반 내용으로 남기지 않아야 한다. 완료는 모델의 선언 대신 Exporter receipt와 실제 파일 digest로 확인한다.

외부 공유 재생은 [ExternalReplay 계약](ARCHITECTURE.md#external-replay)의 plain JSON만 받는다. [공유 입력 한도](OPERATIONS.md#limits)를 실제 bytes를 읽는 동안 검사하고 크기를 초과하면 JSON parse 전에 중단한다. parse는 UTF-8·깊이·중복 key·문자열·배열 한도를 집행하며, 알 수 없는 형식 버전·enum·field·깨진 참조는 거부한다. 원격 자산·raw HTML 렌더링 필드·파일 경로·archive·URL을 통한 보조 파일 로딩은 허용하지 않는다.

공유본의 문자열은 inert plain text로 표시한다. 문자열이 URL·시스템 명령·HTML처럼 보여도 링크 자동 실행·HTML 해석·로컬 명령으로 처리하지 않는다. 묶음의 내부 digest가 맞아도 신뢰한 실행 자료로 승격하지 않으며 source grant·disclosure grant를 만들지 않는다. 유효성 검사 실패 시 기존 기록을 덮어쓰거나 일부 이벤트만 실제 결과처럼 가져오지 않는다.

## 9. 삭제·침해 대응

Source Grant·Disclosure Grant 철회와 Run·원문 객체 삭제는 별도 명령이다. 삭제는 참조 관계를 검사하고 현재 실행의 취소·dispatch 차단을 먼저 적용한다. 삭제된 근거를 참조하는 기록은 `evidence_unavailable`을 표시하며 감사 가능하다는 배지를 유지하지 않는다.

adapter 침해·예기치 않은 범위 밖 도구 접근을 감지하면 해당 binding을 격리하고 새 호출을 차단한다. 현재 Run은 확정할 수 있는 사실만 보존하고 interrupted 또는 failed로 처리한다. 비밀 노출 가능성이 있는 경우 영향 credential·전송 대상·관측 근거를 사용자에게 보여주며 무관한 계정·파일을 자동 변경하지 않는다.

로컬 파일 권한, Keychain, 공급자 도구 정책은 서로 다른 보호다. 암호화된 디스크 사용 여부와 원문이 앱 데이터에 존재하는 사실을 숨기지 않는다. SSD·파일시스템 snapshot·외부 백업·이미 전송된 공급자 자료의 물리적 소거는 앱의 삭제 완료 보장에 포함하지 않는다.
