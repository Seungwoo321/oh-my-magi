# ADR 0005 — 보유된 실행물 검증 권위

- 상태: 채택
- 결정일: 2026-10-02
- 현재 계약: [실행물 검증 서비스](../ARCHITECTURE.md#provider-artifacts), [검증 권위 수명](../SECURITY.md#provider-verification)

## 배경

실행물의 큰 파일 hash와 서명 검사를 UI event loop에서 동기 수행하면 취소와 deadline 처리가 검증 종료까지 지연된다. 같은 호출 경로에서 완전한 검증을 반복하면 이 지연을 늘린다. 경로 또는 metadata만 저장하는 cache는 검증 후 파일 교체를 구분하는 실행 권위를 제공하지 못한다.

## 검토한 대안

| 대안 | 기술적 효과 | 실패 경계 |
|---|---|---|
| 각 소비자가 동기 전체 검증 | 소비자별 완전한 검사 | UI 처리와 취소가 blocking 작업에 묶이며 같은 실행물을 반복 읽는다 |
| metadata cache로 검사 생략 | 파일 읽기를 줄인다 | 교체된 실행물에 검증 결과를 승계할 수 있다 |
| blocking 서비스가 완전히 검증한 파일 권위를 보유 | 동시 요청이 single-flight 결과와 열린 파일 권위를 공유한다 | 파일 권위 무효화와 deadline fencing을 모든 효과 경계에서 집행해야 한다 |

## 선택과 근거

완전한 검증을 제한된 blocking worker로 옮기고 검증한 불변 generation의 파일 descriptor와 정체성을 서비스가 보유한다. 단일 진실 원칙에 따라 카탈로그·admission·dispatch는 같은 검증 권위를 전달받는다. fail-closed 원칙에 따라 파일 교체와 권한·link·version 변경은 전체 재검증 전까지 효과를 차단한다. metadata는 권위 변경을 감지하는 조건이며 내용·서명 검증을 대신하지 않는다.

취소와 deadline은 결과 발행 및 외부 효과 이전에 generation fence로 확인한다. subprocess 종료와 회수까지 서비스가 소유하여 늦은 검증이 새 요청을 활성화하지 못하게 한다. atomic admission commit 이전과 provider 효과 직전에 요청 권위를 철회한다. admission 이후에는 durable cancellation을 접수하고 슬롯·진행 중 결과를 기존 취소 규칙으로 정리한다. worker 중단만으로 이 취소를 대신하지 않는다. 전체 artifact-set 일치와 엄격한 TLS 신뢰 계약은 바꾸지 않는다.

## 결과와 검증

검증은 동시 요청의 단일 전체 검사, 보유 inode 재사용, 경로 교체·쓰기 권한·link·version 변경 시 전체 재검증, subprocess deadline 종료·회수, 취소 후 늦은 결과 거부와 효과 없음으로 확인한다. UI는 검증 중에도 입력과 취소를 처리하며 확인 중 상태를 표시한다. 성능 수치는 실제 측정으로 별도 검증한다.
