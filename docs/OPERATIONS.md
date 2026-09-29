# 설치·실행·보존·출시 계약

이 문서는 macOS 배포, 실행 수명주기, 고정 운영 한도, 복구와 검증 가능한 품질 기준을 정의한다. 표의 성능 수치는 수용 목표이며 측정 결과가 아니다. 심의 의미는 [심의 프로토콜](DELIBERATION.md), 실행 데이터는 [실행 구조](ARCHITECTURE.md), 정보 공개는 [보안](SECURITY.md)을 따른다.

## 1. 설치와 실행 준비

배포물은 서명·공증된 설치형 macOS 앱과 필요한 앱 소유 helper다. 웹 배포물과 모바일 앱은 제품 출시 대상이 아니다. LLM 추론 서버·사용자 계정 서버·원격 DB를 운영 전제로 요구하지 않는다. 릴리스 manifest는 지원 macOS 범위·CPU architecture·앱·helper·schema·adapter 호환성·asset 출처를 고정한다. 확인되지 않은 OS·architecture를 설치 가능하다는 이유만으로 지원으로 표시하지 않는다.

최초 실행은 콘솔을 열고 무료 체험용 로컬 replay 자료를 보여줄 수 있다. replay는 실제 모델 호출·새 심의·실시간 결과로 표시하지 않는다. 실제 시작은 provider 연결, 3개 역할의 binding, 원문 공개 범위와 예상 호출 예산을 확인한 뒤 가능하다. 모델이 없어도 저장 기록 열람·역할 편집·자료 준비·replay는 동작한다.

agent 실행물은 사용자가 설치한 공식 경로 또는 검증된 배포 manifest의 파일만 연결한다. 실행물·adapter의 version·digest·architecture·라이선스·인증 조건을 확인한다. arbitrary URL·model 출력이 제공한 설치 명령을 자동 실행하지 않는다. 자동 설치를 제공하는 경로도 다운로드 대상과 필요한 외부 인증을 사용자에게 표시한다.

<a id="lifecycle"></a>
## 2. 창·앱·agent 수명주기

앱은 메인 콘솔과 메뉴 막대 아이콘을 함께 제공한다. 창의 초기·최소 크기와 사용 가능한 화면 영역의 크기 제한은 [UI/UX §2.1](UI_UX.md)이 소유한다. DesktopShell은 현재 디스플레이의 배율을 반영한 논리 좌표로 크기와 위치를 복원한다.

| 사용자 또는 장애 동작 | 계약 |
|---|---|
| 창 최소화 | macOS의 Dock 최소화 동작을 따른다. 앱과 실행을 유지하며 메뉴 막대 팝오버 전환으로 대체하지 않는다 |
| 마지막 메인 창 닫기 | 닫기 사건을 창 숨김으로 처리한다. 앱과 실행을 유지하고 메뉴 막대와 Dock에서 콘솔을 복원할 수 있다 |
| 메뉴 막대 아이콘 클릭 | 아이콘 위치에 고정한 상태 팝오버를 토글한다. 상태 조회 외의 모델 호출·실행 생성·자료 읽기를 수행하지 않는다 |
| 팝오버 바깥 클릭·Escape | 팝오버만 닫고 메인 창·Run·입력 초안을 보존한다. 취소·앱 종료 명령으로 해석하지 않는다 |
| 팝오버의 콘솔 열기·Dock 복원 | 메인 창의 숨김·최소화를 해제하고 같은 Run과 읽던 위치를 표시한다. 재심의나 모델 재호출을 발생시키지 않는다 |
| 다른 Space·화면 잠금 | 실행은 코어가 유지한다. 보이지 않는 창의 시각 효과를 중지한다 |
| 디스플레이·Space·창 크기 변경 | 현재 디스플레이의 사용 가능한 화면 영역에서 창 크기·위치를 다시 계산한다. 팝오버는 메뉴 막대 아이콘 위치를 다시 확인해 배치한다 |
| `⌘Q`·앱 메뉴 또는 메뉴 막대의 종료 | 활성 실행이 있으면 실행에 미치는 영향을 표시하고 종료 확인을 받는다. 확인 후 새 dispatch 차단, durable 취소 의도, 활성 호출 취소·소유 프로세스 종료 확인 순서로 종료한다 |
| 메인 또는 팝오버 WebView crash·reload | 코어·Run과 다른 창을 유지하고 해당 화면만 snapshot·event cursor로 복원한다 |
| 코어 crash·OS 강제 종료 | 앱 소유 실행 guard가 parent 연결 소실을 감지해 소유 agent를 정지한다. 재실행은 복구 검사로 시작한다 |
| OS 재부팅 | 살아있는 세션으로 간주하지 않는다. 저장 checkpoint·지원되는 provider 조회로 영향 Run을 조정한다 |
| sleep/wake·네트워크 변경 | 진행·응답의 신선도를 다시 확인한다. 깨어났다는 이유로 미확인 prompt를 재전송하지 않는다 |

메뉴 막대 아이콘·팝오버·Dock은 동일 앱 인스턴스의 진입점이다. 콘솔 복원 요청이 겹쳐도 두 번째 Coordinator나 같은 Run의 별도 실행을 만들지 않는다. 제거된 디스플레이의 좌표를 그대로 재사용해 창이 화면 밖에 남지 않도록 한다. 팝오버의 화면 경계·포커스·닫기 동작은 DesktopShell이 처리하며, 메인 창과 실행의 수명을 바꾸지 않는다.

종료 확인을 닫거나 취소하면 앱과 실행을 유지한다. 모든 명시적 종료 진입점은 같은 종료 절차를 사용한다. 앱 재시작·로그인 복구·창 복원은 조회 가능한 상태와 입력을 되살리며, 중단된 실행은 [재개 조건](ARCHITECTURE.md#run-state)과 사용자의 명시적 재개 명령 없이는 모델을 다시 호출하지 않는다.

agent process group은 실행 직전에 소유권·시작 정체성·Run·slot·generation을 기록한다. 시작은 예약 의도 저장, gated child 준비, identity 저장, 실행 허용 순서다. 실행 허용 이전에 부모와 연결이 끊기면 child는 종료한다. 이미 실행된 child의 parent-liveness guard는 heartbeat 손실 뒤 새 도구 실행을 막고 소유 process group을 정지한다. PID만 저장하거나 PID가 같다는 이유로 임의 프로세스를 종료하지 않는다.

guard의 관측·종료는 OS 스케줄링·sleep·crash의 영향을 받는다. 강제 종료된 앱의 모든 클라우드 추론이 즉시 멈춘다고 보장하지 않는다. 재실행 시 정지 여부가 불확실한 슬롯은 `interrupted`로 보존하고 새 호출을 막는다. 외부의 사용자 소유 터미널 세션을 인수하거나 종료하지 않는다.

취소는 `cancelling`을 먼저 저장한 뒤 공급자 cancel을 요청하고 확인한다. 유예 후 앱이 소유한 process group만 종료할 수 있다. 정지·새 dispatch 차단을 확인해야 `cancelled`로 끝낸다. 이미 확정된 completed 기록에 취소를 덮어쓰지 않는다. late 결과는 [fencing](ARCHITECTURE.md)에 따라 현재 Run 결과에 적용하지 않는다.

## 3. 저장·복구

사용자 데이터는 OS가 지정한 앱 전용 Application Support 아래에 둔다. `state/`는 DB와 버전 필드가 있는 `console-preferences.json`을 소유한다. 환경설정 파일은 테마·모션·음향·글자 확대·화면 언어만 담는다. 구버전 필드는 명시된 기본값으로 읽고 다음 저장에서 새 버전으로 기록한다. 파일은 임시 경로에 완전히 쓴 뒤 원자적으로 교체하며 소유자 읽기·쓰기 권한으로 저장한다. `objects/`는 원문·파생 객체, `runs/`는 세션 scratch, `logs/`는 진단, `tmp/`는 미완 객체, `backups/`는 명시적 백업을 소유한다. 앱 디렉터리는 소유자 전용으로 생성한다. 실행 데이터를 코드 저장소에 쓰지 않는다.

SQLite는 bundled 버전을 lock하고 WAL·`synchronous=FULL`·외래키 검사를 사용한다. OS lock을 가진 코어만 writer가 된다. 클라우드 동기화·network filesystem을 활성 DB 위치로 사용하지 않는다. WAL만 임의 삭제하거나 lock 파일을 지워 중복 writer를 시작하지 않는다. 내구성 설정의 의미는 [SQLite synchronous](https://www.sqlite.org/pragma.html#pragma_synchronous)를 따른다.

복구는 store identity·schema·DB 무결성·참조 객체를 확인하고 미완 outbox·실행 identity·generation·예산을 대조한다. 새 코어 세대를 발급한 뒤 이전 세대 이벤트를 격리한다. 원문·역할·질문·provider binding이 같고 미확인 호출이 해소된 경우만 같은 Run 재개를 허용한다. 모델 문맥이나 입력 경계를 보존할 수 없으면 자식 Run으로 이어간다.

디스크 부족·DB 쓰기 오류·객체 손상 시 새 dispatch를 차단한다. 저장되지 않은 모델 결과를 completed로 표시하지 않는다. 손상된 data root를 보존하고 진단·검증된 export·별도 root 복구를 제공한다. 읽을 수 없는 객체를 빈 내용으로 대체해 유효한 근거처럼 사용하지 않는다.

<a id="limits"></a>
## 4. 실행 예산과 한도

아래 값은 제품 기본 정책이며 Run의 실행 계약에 고정한다. 공급자 한도·모델 context 한도·현재 권한이 더 좁으면 그 값을 적용한다. 늘리는 설정은 하드 상한 안에서 새 Run에만 반영하며 예산이 남아 있다는 사실은 권한이 아니다.

| 항목 | 기본값 | 하드 상한·판정 |
|---|---|---|
| Conversation당 활성 Run | 1 | 1; 다른 Conversation은 전역 공급자 큐를 공정하게 공유 |
| 앱 전체 동시 모델 호출 | 3 | 3; 계정·공급자의 더 좁은 제한을 함께 적용 |
| 한 Run의 동시 모델 호출 | 3 | 3; 공급자 제한에 맞춰 직렬 수행해도 1차 결과 봉인은 유지 |
| 심의 사이클 | 1 | 1; 추가 심의는 자식 Run |
| 기본 앱 턴 요청 | 10 | 독립 3 + 교차 3 + 서기 1 + 표결 3; provider 내부 추론 횟수와 구분 |
| malformed 출력 교정 | 슬롯당 1회 | 한 Run의 전체 앱 턴 요청 20회; 사용자가 승인한 예산이 더 작으면 그 값 적용 |
| 파일당 원문 크기 | 20 MiB | 100 MiB; context 투입 가능량과 별개 |
| manifest 원문 합계 | 100 MiB | 500 MiB; 디스크·추출 한도 추가 적용 |
| manifest 항목 수 | 200 | 1,000; 빠진 항목은 제외 이유 표시 |
| PDF 페이지·이미지 픽셀 | 파일당 200페이지·40MP | 1,000페이지·100MP; 초과 파일은 범위 선택 필요 |
| 추출 worker | 동시 2개·각 512 MiB | 각각 1 GiB; 파일당 60초, 사용자가 고른 큰 범위는 최대 300초 |
| 공통 원문 문맥 | 32,000 추정 token | 각 단계의 system·역할·누적 평가·제안·출력 예약량을 뺀 공통 최소값 이하; 설정 상한 128,000 |
| 슬롯 공개 출력 | 4,096 token 요청 | 8,192 token 요청; parser 결과 256 KiB, 깊이 32 |
| host 근거 읽기 | 호출당 64 KiB, 슬롯당 16회 | 슬롯 합계 1 MiB; manifest 범위·model context 상한을 함께 검사 |
| 모델 호출 기한 | 10분 | 30분; 시간 초과는 투표의 abstain으로 바꾸지 않음 |
| Run 활성 실행 누적 시간 | 45분 | 120분; 사용자·인증 대기는 별도 기록, 남은 호출 예산은 유지 |
| 생존 관측 | 5초 간격 | 15초 이상 신선한 근거가 없으면 상태 unknown·조정 |
| 정상 취소 유예 | 10초 | 이후 소유 process group 정지; 추가 5초 안에 관측되지 않으면 interrupted |
| 내부 이벤트 frame | 256 KiB | 큰 근거·결의는 object 참조로 전달 |
| 공유 replay JSON | 10 MiB | 실제 입력 bytes 기준, parse 이전 상한 집행; 압축 archive 미지원 |
| 공유 replay 이벤트 | 10,000개 | 단일 묶음 총합, sequence·참조 검증 |
| 공유 replay text 필드·깊이 | 필드당 UTF-8 1 MiB·깊이 32 | export·import에 같은 제한; 중첩으로 상한 우회 금지 |
| 미완 임시 export·추출 | 종료 후 24시간 | 활성 참조·복구 대상은 제거 대상에서 제외 |

token 추정치는 확정 과금이나 남은 구독 사용량이 아니다. provider가 실제 usage를 주면 입력·출력·캐시·추론 항목과 단위를 보존하고, 없으면 unknown으로 표시한다. 0·무제한·임의 백분율을 대신 표시하지 않는다. 출력 token 설정은 지원되는 모델에만 전달하며 반환 크기는 adapter와 host가 별도로 제한한다.

앱 턴 요청 상한은 Coordinator의 dispatch 수를 제한한다. native agent 안에서 발생한 모델 요청·도구 재시도는 provider가 관측 정보를 제공한 범위에서만 집계한다. 시간·host 도구 호출·출력 한도를 초과하면 취소하며, 그 취소가 이미 소비한 provider 사용량을 되돌린다고 표시하지 않는다.

context 예산은 토크나이저를 지원하면 그 결과, 없으면 검증한 보수적 추정으로 계산한다. model 최대 문맥을 확인할 수 없는 binding은 무제한으로 통과시키지 않는다. context overflow 뒤 자동 truncation·역할별 서로 다른 자료를 허용하지 않는다. 사용자는 명시적 범위 선택·새 모델 binding으로 자식 Run을 만든다.

preflight는 단계별 `source + question/role/system + prior_artifacts_max + output_reserve <= model_context_limit`를 검사한다. `prior_artifacts_max`에는 교차 검토의 최초 평가 3개, 서기의 최초·교차 평가 6개, 표결의 자기 평가·전체 교차 평가·제안을 각 산출물 상한으로 반영한다. 이미지 token 비용과 tool 결과 여유도 해당 binding의 검증된 계산법에 포함한다. 모든 슬롯이 수용하는 최소 원문 예산을 사용자에게 제시하므로 최초 검토만 겨우 들어가고 후반 단계가 필연적으로 넘치는 구성을 시작하지 않는다.

실제 provider 계산이 보수적 추정을 넘어가면 `paused(validation)`으로 정지하고 부족한 단계·량을 표시한다. 이미 검토한 자료나 다른 코어의 반론을 숨겨 잘라내지 않는다. model 또는 공통 자료를 바꾸는 해결은 새 확인과 자식 Run을 요구한다.

## 5. 재시도·인증·quota

조회·연결 재시도와 추론 재호출은 별개다. 실패 종류·발행 여부·provider request ID·실제 사용량·다음 재시도 시각을 저장한다. 앱 재시작은 예산과 횟수를 초기화하지 않는다.

| 상황 | 처리 |
|---|---|
| 인증 만료·로그아웃 | `paused(auth)`와 공식 재인증 경로. 갱신 뒤 [사용자 재개 명령](ARCHITECTURE.md#run-state)을 기다리며 다른 계정으로 자동 교체하지 않음 |
| provider quota·429 | `paused(quota)`; 실제 알려진 reset 또는 Retry-After를 표시. 회복 뒤 [사용자 재개 명령](ARCHITECTURE.md#run-state)을 기다림 |
| 요청이 발행되지 않았음이 확인된 일시 오류 | 최초 포함 3회, 1초·2초 기준 지연과 jitter, 최대 30초·기한 안에서 재시도 |
| 전송 후 응답 유실·timeout | 상태 조회로 조정. 수락·과금 불명은 interrupted, 자동 재호출 금지 |
| malformed 구조화 출력 | 슬롯당 한 번 교정; 고정 입력·역할·제안 digest를 보존하고 추가 호출 소비 |
| 의미·인용·제안 digest 불일치 | `paused(validation)` 또는 failed. 유효한 표로 치환하지 않음 |
| 추가 근거 필요 | `paused(needs_input)`; 새 manifest 확인 후 자식 Run |

readiness는 로그인·지원 기능·binding·입력 예산·출력 예약량·디스크·grant를 검사한다. 구독 잔량을 API로 알 수 없으면 시작 전에 unknown임을 표시하고 실행 중 제한을 정상 상태로 처리한다. 무료 한도 소진을 유료 API·더 저렴한 모델·다른 계정으로 자동 우회하지 않는다.

<a id="retention"></a>
## 6. 보존·삭제·백업

| 데이터 | 보존 계약 |
|---|---|
| Conversation·Run·구조화 평가·표·결의 | 사용자가 삭제할 때까지 로컬 보관 |
| 원문·추출 객체 | 참조하는 기록이 유지되는 동안 보관; 사용자가 근거만 삭제할 수 있음 |
| 진단 로그 | 14일 또는 50 MiB 중 먼저 도달한 한도; 원문·prompt·응답·비밀 제외 |
| provider 원시 stream | 기본 영속 저장하지 않음; 필요한 구조화 결과와 관측만 보관 |
| 중단 세션 scratch | 정지·필요 객체 이관을 확인한 뒤 24시간 이내 정리 |
| 명시적 백업 | 사용자가 선택한 보존·삭제. 자동 cloud sync 없음 |

기록 삭제는 참조와 진행 중 실행을 먼저 확인한다. 여러 Run이 공유하는 원문 객체는 마지막 참조가 사라져야 GC한다. 사용자가 원문만 삭제하면 영향 기록과 잃는 감사 범위를 미리 표시하고 참조를 결손 상태로 바꾼다. 저장 한도 경고 때문에 결의·근거를 자동 삭제하지 않는다.

앱 데이터의 원문·결의는 OS 파일 권한으로 보호되는 로컬 데이터다. 별도 앱 암호화가 적용되지 않은 데이터를 암호화됐다고 표시하지 않는다. Keychain 비밀은 일반 데이터 export·backup에 포함하지 않는다. 사용자가 만든 backup에 민감한 원문이 포함되는 경우 보호 수준과 범위를 생성 전에 표시한다.

백업은 [SQLite online backup](https://www.sqlite.org/backup.html) 또는 읽기 snapshot으로 DB와 event high-water를 고정하고, 참조 객체를 pin한 뒤 복사·해시 검증한다. manifest에는 store ID·schema·객체 digest·포함 범위가 들어간다. 모두 검증된 뒤 complete marker를 설치하며 partial은 복구 가능한 백업으로 표시하지 않는다.

복원은 새 data root에서 수행하고 검증 후 사용자 선택으로 전환한다. 새 store generation을 발급하며 실행 lease·권한 동의·secret·provider 로그인·살아있는 세션을 백업에서 재활성화하지 않는다. 복원한 기록은 열람 가능하지만 새 호출은 readiness와 공개 범위 확인을 거친다. 기존 data root를 검증 전 덮어쓰지 않는다.

## 7. 업데이트와 배포 서명

앱·helper의 Apple Developer ID 서명과 notarization, Tauri updater 서명, 사용자 Keychain 비밀은 목적이 다르다. updater 키는 이 앱 전용으로 관리하며 private key를 앱·저장소·진단에 넣지 않는다. Apple 서명이 updater key를 대체하지 않는다. [macOS 서명](https://v2.tauri.app/distribute/sign/macos/)과 [Tauri updater](https://v2.tauri.app/plugin/updater/)의 검증 경계를 각각 적용한다.

업데이트는 서명·digest·OS·schema 호환성을 확인한 새 배포물을 준비하고 활성 Run의 종료 또는 취소 확인 뒤 적용한다. 실행 중 adapter나 모델 설정을 몰래 교체하지 않는다. migration 전에 일관된 backup과 여유 공간을 확보하고 migration journal·목표 schema를 기록한다.

migration은 단일 writer에서 멱등적으로 수행하며 중간 실패 상태에서는 새 심의를 시작하지 않는다. rollback binary가 현재 schema를 읽고 쓸 수 있을 때만 되돌린다. 호환되지 않으면 별도 root에 검증된 backup을 복원한다. 단순 앱 버전 교체로 DB·동의·사용량 기록을 과거 상태로 덮어쓰지 않는다.

공개 릴리스의 팬 콘텐츠·라이선스·상표 사용 조건은 [제품 경계](DESIGN.md)와 [출처](REFERENCES.md)를 따른다. 무료 배포라는 사실을 제3자 자산·공급자 조건 검토의 대체물로 사용하지 않는다.

## 8. 관측과 적합성

진단은 Run·slot·command ID, 단계, 오류 code, 대기 이유, 지연, event sequence, adapter 버전과 실제 사용량의 제공 여부를 기록한다. 파일명·로컬 경로·원문·질문·결의·계정 이메일·credential 원문은 기본 로그에 넣지 않는다. 사용자가 진단을 공유하기 전에 포함 항목과 redaction 결과를 검토한다.

지원 provider 조합은 OS·CPU·앱·agent·adapter·model·인증 경로·capability manifest·조건 검토 근거·시험 결과·시각을 가진 호환성 기록으로 발행한다. 검증 시점 이후 실행물 digest가 바뀌거나 필수 capability가 달라지면 지원을 재평가한다. 연결 성공 하나를 전체 지원으로 표시하지 않는다.

| 품질 대상 | 수용 목표·증거 |
|---|---|
| 콘솔 입력 | 실제 3개 stream 중에도 로컬 조작 응답 p95 100ms 이내; 모델 대기 시간과 별도 측정 |
| 화면 연출 | 지원 기준 장비의 60Hz 화면에서 활성 장면 frame p95 16.7ms 이내; 본문 DOM 가독성·입력 유지 |
| 비활성 상태 | 최소화·가려진 창의 연속 장식 animation 중지; 모델 heartbeat와 분리 |
| 메뉴 막대 | 팝오버 반복 열기·닫기에서 새 모델 호출 0회, 같은 Run·revision 표시, 콘솔 복원 시 입력·근거 위치 보존 |
| 데스크톱 창 | 지원 디스플레이·배율·Space 변경 후 창과 팝오버가 사용 가능한 화면 영역 안에 있으며 Dock 최소화·숨김·명시적 종료가 구분됨 |
| 재시작 | 원문 재추출 없이 최근 1,000개 Run 목록·선택 기록을 2초 이내 표시하는 목표; 외부 로그인 시간 제외 |
| 스트림 폭주 | 3개 동시 출력·느린 renderer에도 코어 저장·취소 가능, 미리보기 누락은 표시하고 최종 결과는 객체로 복구 |
| 공급자 격리 | 범위 밖 파일·셸·다른 코어의 봉인된 결과·secret 접근 시도 거부 증거 |
| 장애 복구 | dispatch 전·후, 평가 수락, 3번째 표 저장, 결과 commit 경계 crash 후 중복 표·가짜 완료가 없음 |
| 동시성 | 취소와 마지막 결과 경합, 메인 창·Companion의 같은 명령, stale revision·generation 결과를 결정적으로 처리 |
| 입력 완전성 | PDF 추출 손실·한도 초과·삭제·파일 교체·인용 범위 오류가 manifest·UI에 드러남 |
| 데이터 이관 | backup 누락·digest 손상·schema 불일치·migration 중단을 검출하고 원본 보존 |

기준 장비·OS·화면 크기·배율·데이터 fixture·네트워크 조건·측정 횟수는 릴리스 검증 manifest에 기록한다. 성능 숫자만 통과시키려고 MAGI 장면을 일반 카드 UI로 대체하거나 의미 있는 상태·근거를 생략하지 않는다. 심의 기능과 화면 품질은 [수용 시나리오](SCENARIOS.md) 및 [디자인 시스템](DESIGN_SYSTEM.md)으로 함께 판정한다.
