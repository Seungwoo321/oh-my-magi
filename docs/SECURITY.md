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

공개 목록은 원문뿐 아니라 질문·역할·선택한 이전 기록·교차 검토에 공유되는 중간 평가·서기 제안도 포함한다. 원문 파일, 추출 텍스트와 파생 이미지는 서로 다른 콘텐츠 종류로 고정한다. PDF 페이지를 렌더링하거나 이미지를 변환한 바이트는 파생 이미지이며 원문 파일로 표시하지 않는다. 실제 전송 표현·범위가 manifest와 동의에 포함되지 않으면 보내지 않는다. 서로 다른 공급자를 배정하면 한 공급자에서 생성된 공개 평가가 다른 공급자의 입력으로 전달된다는 사실을 최초 확인에 표시한다. 사용자 동의는 단계별 콘텐츠 종류와 수신자를 고정하며, 새로운 수신자를 추가하는 변경은 기존 동의로 처리하지 않는다.

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

macOS 공급자 sandbox는 `system.sb`의 플랫폼 기본 규칙 위에 TLS 인증서 신뢰 검증에 필요한 공급자 전용 추가 IPC로 정확한 `com.apple.SecurityServer`의 `mach-lookup`을 허용한다. 이 권한은 인증서 신뢰 서비스 접근이며 네트워크 목적지나 모델 도구 권한을 확장하지 않는다. 공급자 전용 wildcard Mach 서비스나 지정되지 않은 서비스 예외를 추가하지 않는다. 사용자 파일·credential store 읽기, 범위 밖 파일 쓰기, proxy를 우회한 외부 연결은 허용하지 않는다. 플랫폼 IPC와 네트워크·파일·도구 권한은 각각 독립된 allowlist로 집행하고 검증한다.

<a id="provider-tls"></a>
### 공급자 TLS 신뢰

Codex의 HTTP 호출은 서명·출처가 검증된 고정 런타임의 호출자가 선택한 플랫폼 TLS backend를 사용한다. 검증된 public CA bundle의 모든 인증서를 기존 신뢰 설정에 추가하며, CA 설정을 이유로 HTTP backend를 자동 교체하지 않는다. WebSocket의 rustls 경로는 플랫폼의 native root와 같은 검증된 public CA 전체를 함께 사용한다. 두 경로 모두 인증서 체인과 hostname을 엄격히 검증하며 미확인 인증서 허용, 검증 해제, 실패 시 다른 TLS 경로로 자동 우회하지 않는다.

CA bundle의 출처·digest·정규 파일 정체성·권한을 실행 전 검증한다. `CODEX_CA_CERTIFICATE`는 이 검증된 bundle만 가리킨다. TLS 신뢰 추가는 기존 proxy 목적지 allowlist와 모델 도구의 네트워크 권한을 넓히지 않는다. HTTP·WebSocket·인증 확인의 성공 여부는 별도로 관측하며 한 경로의 성공으로 다른 경로나 모델 추론의 성공을 표시하지 않는다. 실행물의 발행과 전체 정체성 검사는 [공급자 실행물 계약](ARCHITECTURE.md#provider-artifacts)을 따른다.

agent의 공식 인증 런타임이 provider credential store를 사용하는 것과 모델의 파일 도구가 비밀을 읽는 것은 구분한다. 모델 도구에서 credential store·환경 비밀·다른 앱 문맥 접근을 차단할 수 없는 조합은 사용할 수 없다. agent·adapter 버전 또는 전체 artifact-set 정체성이 바뀌면 기존 카탈로그·binding·readiness와 호환성 근거를 자동 승계하지 않는다. 새 실행은 변경된 전체 실행물 정체성을 다시 검증한다.

<a id="provider-verification"></a>
### 실행물 검증 권위와 수명

빌드 출처의 검증 입력은 실행물 세대에 묶인 고정 소스 압축본과 패치된 의존성 lockfile이다. 앱 빌드 검증은 실제 내용 해시를 다시 확인한다. 전체 소스 복제본과 컴파일 산출물은 마지막 빌드 소비 직후 제거하고, 검증 입력은 해당 실행물 세대와 이후 앱 빌드 소비자가 종료하면 회수한다. 출처 기록의 주장만으로 입력 바이트 검증을 대체하지 않는다.

검증 권위는 완전한 파일 내용·서명·출처·CA 검증이 성공한 불변 generation과 열린 파일 descriptor에 결합한다. 경로·파일 종류·소유권·쓰기 권한·link 수·inode·파일 version을 사용 경계에서 확인한다. 경로 교체, link 또는 권한 변경, descriptor의 파일 version 변경은 보유 권위를 무효화하며 새로운 전체 검증이 끝날 때까지 외부 효과를 차단한다. 파일 크기·수정 시각 같은 metadata만으로 완전한 검증 결과를 만들거나 교체된 파일에 승계하지 않는다.

검증 worker와 서명·아키텍처 검사 subprocess는 명시적 deadline을 가지며 만료된 subprocess는 종료하고 회수한다. deadline·취소·Run generation 변경은 결과 발행 전에 확인하고, admission과 dispatch는 외부 효과 직전에 같은 fencing 권위를 다시 확인한다. 늦게 끝난 검증은 취소된 요청을 Ready로 바꾸거나 새 요청의 권위를 덮어쓰지 않는다. deadline 또는 취소는 atomic admission의 commit 이전과 provider 효과 직전에 요청 권위를 철회한다. 이미 admission이 완료되었으면 durable cancellation을 접수하여 아직 시작하지 않은 슬롯을 해제하고 진행 중 효과의 결과를 기존 취소·generation 규칙으로 정리한다. blocking worker의 중단 요청만으로 admission 또는 외부 효과 취소가 완료되었다고 표시하지 않는다. 카탈로그·admission·dispatch의 전체 artifact-set 일치 검사와 엄격한 인증서·hostname 검증은 보유 권위를 재사용해도 유지한다.

접수 권위의 등록·만료·영속 취소 fence는 [명령 수명 계약](ARCHITECTURE.md#admission-requests)을 따른다. 취소와 commit은 같은 guarded 권위로 순서를 결정하며 deadline은 guard 안에서 평가한다. Provider 실행·인증·session·prompt 전송도 취소와 효과 발행의 순서를 결정하는 경계를 공유한다. 검사 후 별도 호출 사이에 권위를 철회할 수 있는 check-then-send 구조를 허용하지 않는다. Commit 후 취소는 guard를 해제한 뒤 정확한 Run의 영속 취소를 접수하여 저장 lock과 효과 정리의 교착을 피한다.

로컬 제어 연결은 소유 프로세스와 같은 UID, 제한된 메시지 크기, 연결 전체의 절대 deadline을 검증한다. 조각마다 갱신되는 read timeout만으로 연결 수명을 제한하지 않는다. 진단용 프로세스 제어는 그 프로세스가 등록한 현재 요청에만 적용되며 UI의 정확한 요청 token을 검증하는 취소 경로를 대신하지 않는다.

## 6. Host Context Reader

Context Reader는 필요한 경우 세션 한정 stdio MCP로 노출하며 `list_sources`, `read_evidence`, `resolve_citation` 같은 읽기 계약만 가진다. capability는 Run·role·slot·generation·manifest에 묶이며 현재 stage의 공개 경계를 매번 검사한다. 문자열 경로를 인수로 받지 않고 승인된 source ID와 locator를 받는다.

초기 심의에서는 다른 코어의 출력 ID가 알려져도 읽을 수 없다. 교차 검토의 공개 snapshot은 Coordinator가 세 결과를 검증한 뒤 발행한다. 서기는 공개된 평가와 manifest만 받고 개인 세션·credential·숨은 추론에 접근하지 않는다. 표결 세션은 다른 코어의 비공개 Ballot을 읽을 수 없다.

`readOnlyHint`·`destructiveHint` 같은 도구 메타데이터는 안내이며 보안 제어가 아니다. authorization은 host의 현재 grant·fence·manifest·범위 검사로 결정한다. 알 수 없는 source ID·범위 초과·재사용된 generation·철회된 grant는 거부하고 내용이 없는 오류를 반환한다.

ACP permission 요청도 같은 정책에 연결한다. 이미 승인된 manifest 읽기는 해당 슬롯 범위 안에서 처리하고 미등록 도구·shell·쓰기·범위 확대는 거부한다. 자료 부족에 해당하면 `paused(needs_input)`으로 표시한다. permission 대화에서 광범위한 상시 허용을 선택해 제품 정책을 우회하지 않는다. 사용자의 자료 추가는 새 manifest와 Run 생성 경로를 따른다.

<a id="provider-auth"></a>
## 7. 공급자 인증과 비밀

앱은 사용자가 지정한 기존 CLI·ACP 홈의 구독 인증만 사용한다. 각 프로필은 사용자 지정 이름과 사용자가 직접 입력한 기존 CLI 인증 홈 경로를 반드시 저장하며, 선택한 인증 권위와 revision을 고정한다. 기본·커스텀 경로 모드, 폴더 찾아보기, 자동 경로 설정을 제공하지 않는다. 경로는 절대 경로 또는 사용자 홈을 가리키는 `~/` 형식을 허용하며 native 경계에서 정규화·존재·권한·인증 구조를 검증한다. 다른 홈이나 계정으로 자동 fallback하지 않는다. 앱 내 OAuth·브라우저 로그인·API 키 연결을 제공하지 않는다.

기존 인증 홈은 읽기 전용 권위다. 인증 확인과 갱신은 공급자 공식 런타임의 구독 계약을 따르고, native 인증 broker는 선택한 홈에 묶인 인증 참조를 제한된 로컬 IPC의 메모리로만 전달한다. adapter는 인증 파일을 쓰거나 토큰을 로그·argv·DB·백업·내보내기에 기록하지 않는다. 기존 홈의 인증 정보와 설정을 앱 실행 루트에 복사하지 않으며 다른 공급자 API 요청에 전용하지 않는다. renderer에는 인증 상태와 사용자가 선택한 경로 표시만 제공하고 비밀 원문은 제공하지 않는다.

프로필의 실행·작업 공간은 인증 홈과 분리한다. 실행 환경과 OS sandbox는 선택된 인증 권위와 검증된 실행 파일 및 허용된 입력만 사용할 수 있게 제한한다. ACP 초기화 응답을 홈 경로 attestation으로 취급하지 않는다. 인증 없음·읽기 실패·만료·홈 변경·adapter 미지원·sandbox 검증 실패는 실행을 차단하고 기존 인증 연결 확인 조치를 표시한다. 앱이 새 로그인을 유도하거나 다른 계정으로 전환하지 않는다. 홈 변경은 profile revision과 모델 binding 및 반출 동의를 다시 검증하게 한다.

인증 상태 RPC의 실패만으로 credential이 잘못되었다고 판정하지 않는다. workspace discovery·TLS·전송·상태 확인의 원인을 확정할 수 없는 오류는 연결 확인 실패로 표시하며 재로그인 조치를 붙이지 않는다. 실제 인증 없음·지원되지 않는 인증 권위가 확인된 경우에만 해당 고정 profile ID와 revision에 묶인 인증 실패를 전달한다. 다른 profile의 readiness나 저장한 모델 선택을 함께 폐기하지 않는다.

인증 경로는 메인 창의 연결 설정에서 사용자가 선택한 값을 확인하는 용도로만 표시한다. 실행 snapshot·모델 입력·공유 기록에는 개인 절대 경로를 복제하지 않는다. 앱 내부 비밀은 OS secret store에서 관리하고 저장 산출물에는 참조만 둔다.

공급자 profile은 개인·회사 등 사용자가 구분한 계정별로 분리한다. 프로세스에는 필요한 환경 변수 allowlist와 인증 설정만 전달하며 앱의 전체 환경을 상속하지 않는다. 인증 실패 시 다른 profile을 자동 선택하지 않는다. 앱 연결 해제는 앱의 세션·권한을 폐기하며 다른 앱이 사용하는 공급자 로그인 자체를 자동 삭제하지 않는다.

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
