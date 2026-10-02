# ADR 0004 — 공급자 런타임의 TLS와 전체 실행물 정체성

- 상태: 채택
- 결정일: 2026-10-02
- 현재 계약: [TLS 신뢰](../SECURITY.md#provider-tls), [공급자 실행물](../ARCHITECTURE.md#provider-artifacts)

## 배경

같은 공식 ACP client, 인증 broker, sandbox와 상태 확인 요청을 유지한 비교에서 CA 설정이 HTTP backend를 바꾸면 workspace discovery 오류가 발생했고, 호출자의 플랫폼 TLS backend를 유지한 경로에서는 인증 상태 확인이 성공했다. 인증 상태 RPC 실패는 credential 자체의 오류를 입증하지 않았다. 이 비교는 인증 확인만 수행했으며 prompt, 모델 추론, 전체 심의의 성공을 입증하지 않는다.

HTTP와 WebSocket은 서로 다른 TLS client 경로를 가진다. 한 경로의 신뢰 설정이나 성공 결과를 다른 경로에 그대로 적용하면 인증 확인 이후의 연결 실패를 숨길 수 있다. 또한 ACP 실행 파일이 같아도 함께 배포한 Codex·보조 실행 파일·CA·소스 패치가 달라지면 실제 실행 권위가 달라진다.

## 검토한 대안

| 대안 | 기술적 효과 | 실패 경계 |
|---|---|---|
| public CA 설정 시 HTTP를 rustls로 강제 | TLS 구현을 하나로 통일한다 | 호출자가 선택한 플랫폼 신뢰 동작을 바꾸며, 같은 인증 권위에서 확인된 backend별 차이를 보존하지 못한다 |
| 호출자의 HTTP backend를 유지하고 검증된 public CA를 추가 | 플랫폼 동작과 추가 CA를 함께 사용한다 | HTTP와 WebSocket의 신뢰 구성·검증을 각각 책임져야 한다 |
| ACP 실행 파일 SHA만 binding에 고정 | adapter 교체를 식별한다 | Codex·보조 실행 파일·CA 또는 출처만 바뀐 실행물을 구분하지 못한다 |
| 전체 manifest의 artifact-set digest를 별도로 고정 | 모든 구성물과 출처를 같은 실행 권위로 검증한다 | 카탈로그·admission·dispatch가 같은 정체성을 보존하고 검사해야 한다 |

## 선택과 근거

HTTP는 고정·서명된 공식 런타임의 호출자가 선택한 플랫폼 TLS backend를 유지하고 검증된 bundle의 모든 public CA를 추가한다. WebSocket의 rustls 경로는 native root와 같은 public CA 전체를 사용한다. 인증서 체인과 hostname 검증은 두 경로에서 모두 유지한다.

최소 권한 원칙에 따라 TLS 수정은 인증서 신뢰 구성에 한정하며 proxy 목적지·원문 읽기·모델 도구 권한을 넓히지 않는다. fail-closed 원칙에 따라 bundle·서명·출처가 맞지 않거나 binding의 전체 정체성과 실제 실행물이 다르면 외부 호출을 막는다. 인증 없음과 상태 확인 불가를 구분하여 연결 장애가 다른 profile의 인증 실패로 전파되는 것도 막는다.

전체 artifact-set 정체성을 ACP SHA와 분리한다. 단일 진실 원칙에 따라 같은 검증 결과가 카탈로그·저장된 선택·고정 슬롯·실제 dispatch를 연결한다. 별도 source 빌드의 고정 patch는 CA 등록 시 호출자의 backend를 교체하는 동작만 제거하며 dependency 출처와 lock의 검증을 유지한다.

## 결과와 검증

빌드는 새 불변 generation에서 수행하며 검증된 출력과 앱 resource를 함께 발행한다. 실패한 발행은 기존 실행물을 보존한다. 인증 확인 성공을 모델 추론이나 심의 완료의 증거로 사용하지 않는다.

검증은 동일 공식 client의 인증 상태 비교와 별도로, CA 전체 파싱·엄격한 hostname·잘못된 인증서 거부, 구성물별 digest 변경, 서명·출처 불일치, 발행 rollback, admission·dispatch 정체성 불일치를 확인한다. 진단은 고정 오류 분류와 제한된 수치만 반환하며 credential·응답 원문·개인 경로를 보존하지 않는다.

## 재검토 조건

고정 upstream의 HTTP·WebSocket TLS 구성이나 플랫폼 신뢰 요구가 달라지면 같은 호출자·인증·sandbox를 유지한 비교와 보안 음성 대조군으로 재검토한다. 인증서 검증 해제나 자동 계정 전환은 backend 선택의 대안으로 취급하지 않는다.
