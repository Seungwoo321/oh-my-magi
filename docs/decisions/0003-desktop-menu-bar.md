# ADR 0003 — macOS 콘솔과 메뉴 막대 Companion

- 상태: 채택
- 결정일: 2026-09-28
- 현재 계약: [실행 구조](../ARCHITECTURE.md), [콘솔 화면](../UI_UX.md), [수명주기](../OPERATIONS.md#lifecycle)

## 배경

제품의 실행 환경은 설치형 macOS 앱이다. 사용자는 콘솔을 닫거나 다른 앱을 사용하는 동안에도 메뉴 막대 아이콘에서 심의 상태를 확인하고 같은 안건의 콘솔로 돌아가기를 원한다. 메뉴 막대 팝오버는 별도 실행기가 아니라 현재 심의의 작은 표현이다.

메인 콘솔은 세 코어의 고정 SVG 기하, 시맨틱 토큰, 한국어 본문과 검토 동작을 공유한다. 이 표현을 메뉴 막대에 추가할 때 상태·표결·권한의 정본이 둘로 나뉘지 않아야 한다.

## 검토한 대안

| 대안 | 제품에 주는 효과 | 구조와 검증의 부담 |
|---|---|---|
| SwiftUI + MenuBarExtra | 메뉴 막대 진입점과 팝오버형 창을 OS의 scene·control 체계로 표현한다. macOS 포커스·메뉴·접근성 연결을 네이티브 UI 안에서 다룬다 | MAGI의 SVG 기하·타이포·상태 표현을 SwiftUI로 옮기거나 별도 WebView와 연결해야 한다. Rust 심의 코어를 유지하면 Swift와 Rust 사이의 상태·명령 경계를 명시해야 한다 |
| Tauri 2 + Rust + React·SVG | 메인 창과 Companion이 같은 SVG·토큰·DTO를 소비하고 단일 Rust Coordinator에 명령을 보낸다. 화면 추가가 심의 상태나 권한 소유권을 바꾸지 않는다 | 사용자 지정 팝오버 창의 배치·포커스·바깥 클릭·Space·창 수명은 DesktopShell이 명시적으로 처리하고 macOS에서 검증해야 한다 |

Apple의 [MenuBarExtra](https://developer.apple.com/documentation/swiftui/menubarextra)는 시스템 메뉴 막대의 지속적인 진입점을 제공한다. [window 스타일](https://developer.apple.com/documentation/swiftui/menubarextrastyle/window)은 사용자 지정 내용을 팝오버형 창에 배치한다. SwiftUI는 이 OS 통합을 직접 표현할 수 있다는 장점이 있다.

Tauri의 [프로세스 모델](https://v2.tauri.app/concept/process-model/)은 전역 상태와 창 관리·IPC를 코어에 모은다. [System Tray API](https://v2.tauri.app/learn/system-tray/)는 아이콘·메뉴·클릭 사건을 제공한다. 사용자 지정 상태 팝오버는 이 사건에 연결한 별도 WebView 창으로 구성하며, tray 기능을 `NSPopover` 자동 생성 기능으로 해석하지 않는다.

## 선택과 근거

**Tauri 2 + Rust + React·SVG를 채택하고 macOS 통합을 DesktopShell 어댑터에 둔다.**

- **Single source of truth:** 두 화면이 동일 Run·revision·표결 객체를 읽고 같은 명령 경로를 사용한다. 팝오버의 로컬 상태가 이미 완료된 심의를 진행 중으로 되돌리거나 중복 호출을 만드는 실패를 막는다.
- **관심사 분리와 단방향 의존:** 도메인은 OS 창 객체를 모르고 DesktopShell이 macOS 사건을 앱 명령과 표현 동작으로 변환한다. Space·포커스 수정이 표결 규칙이나 공급자 연결에 전파되지 않는다.
- **공통 표현 계약:** 메인 콘솔과 Companion은 같은 기하·토큰·역할 의미를 공유한다. 서로 다른 렌더러가 코어 위치·찬반 표시·접근 가능한 이름을 다르게 구현하는 위험을 줄인다.
- **최소 권한:** Companion은 필요한 조회·명령 capability만 가지며 파일·제공자·비밀 권한은 host에 남는다. 작은 창을 추가하는 일이 새 권한 경로를 만드는 근거가 되지 않는다.

이 선택의 부담은 macOS 창 동작을 DesktopShell에서 직접 책임진다는 점이다. SwiftUI의 네이티브 메뉴 막대 scene을 사용할 때 얻는 OS 통합과 비교해, 사용자 지정 WebView 팝오버의 포커스·키보드·화면 경계·VoiceOver를 별도로 입증해야 한다. 결정 근거는 공유되는 제품 표현과 단일 상태·권한 경계다.

## 결과

웹 서비스와 모바일 앱은 배포 대상에서 제외한다. 브라우저에서 열 수 있는 콘솔 설계 참조는 WebView 표현과 상호작용을 확인하는 산출물이며 설치형 앱의 창·프로세스·권한 검증을 대신하지 않는다.

메뉴 막대 팝오버는 같은 Run의 상태를 읽으며 열기·닫기로 새 모델 호출을 만들지 않는다. 창 최소화는 Dock 동작을 따르고, 마지막 메인 창 닫기는 숨김으로 처리한다. `⌘Q`와 명시적 종료는 확인과 실행 정리 절차를 따른다. 구체적인 화면 규격과 실행 의미는 현재 계약 문서가 소유한다.

## 재검토 조건

지원 macOS에서 DesktopShell의 포커스·Space·VoiceOver·화면 배치 계약을 충족하지 못하거나 공통 SVG·토큰의 표현 경계를 유지할 수 없으면 호스트 선택을 재검토한다. 먼저 실패한 경계와 재현 가능한 증거를 확인하고, OS 통합 경계의 교체와 전체 표현 계층 교체를 각각 평가한다.
