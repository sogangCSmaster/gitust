# AGENTS.md — Gitust 작업 지침

## 1. 적용 범위와 문서 역할

이 문서는 이 저장소에서 작업하는 코딩 에이전트의 구현·검증·리뷰·Git 작업 규칙이다.
Gitust는 Git 호스팅, 에이전트 워크트리, 로컬 명령 실행, 사람의 승인과 원격 발행을 연결하는 Rust 프로젝트다.

- 제품 소개와 실행 방법은 [README.md](README.md)를 참고한다.
- 설계 맥락은 [프로젝트 비전](docs/vision.md), [아키텍처](docs/architecture/overview.md)를 참고한다.
- 단계별 범위는 [로드맵](docs/roadmap.md), 사업 구상은 [Open-Core 방향](docs/business/open-core.md)에 둔다.
- 문서의 계획을 구현 사실이나 작업 허가로 해석하지 않는다. 현재 동작은 코드와 실행 결과로 확인한다.
- 문서를 수정할 때 제품 설명은 README/docs에, 에이전트의 행동 규칙은 이 파일에 둔다.

## 2. 작업 범위와 변경 원칙

- 요청과 관련된 구현·호출부·상태 전이만 필요한 만큼 확인하고, 원인을 공통 경로에서 고친다.
- 무관한 변경과 다른 사람의 작업을 보존한다. 기능 변경에 큰 리팩터링을 섞지 않는다.
- 구현·문서·UI에서 계획된 기능, 데모 화면, 실서버 기능을 구분한다.
- 오류를 성공이나 권한 허용으로 바꾸지 않는다. 인증·영속화·Git·샌드박스·동기화는 fail-closed로 처리한다.
- 비밀정보를 소스·Git·로그·테스트 출력에 남기지 않는다. 실제 외부 발행과 운영 변경은 사용자가 허용한 범위에서만 수행한다.

## 3. 구현 원칙

1. **성능과 UI**
   - 인터랙션은 즉시 피드백하고, 변경 요청에는 낙관적 UI와 오류 복원을 적용한다.
   - 낙관적 화면 표시를 실제 검증·병합·발행 성공의 근거로 쓰지 않는다. 승인 가능 여부는 서버의 검증 증거로 판단한다.
   - 6장의 성능 예산을 목표로 하되 측정하지 않은 값을 달성한 사실처럼 표시하지 않는다.
2. **디자인 시스템 우선**
   - UI 컴포넌트는 `packages/design-system/src/components/ui`의 solid-ui를 사용하고, Tailwind 의미 토큰과 `src/styles.css`의 테마 변수를 사용한다.
   - 프로덕션 UI의 유니코드 이모지는 금지한다. `<Icon name="..." />` SVG 스프라이트를 사용한다.
3. **Rust 구현**
   - 코어와 CLI/서버는 Rust로 구현한다. Gitea/Forgejo 또는 Dagger 같은 외부 서버를 제품 엔진으로 묶어 대체하지 않는다.
   - 기존 `gix`, `axum`, `tokio` 등과 표준 기능을 우선 사용한다. 현재 Git CLI 호출을 없애려고 요청과 무관한 재작성을 하지 않는다.
   - `Newtype`, `enum`, `Result<T, E>`로 상태와 실패를 표현한다. 프로덕션 경로의 `unwrap()`/`expect()`를 금지한다.
   - 블로킹 I/O·프로세스 실행·CPU 작업은 비동기 실행 스레드를 막지 않게 분리한다. 취소 시 자원과 상태를 정리한다.
4. **검증 우선**
   - 기능은 7.4 검증과 7.5 독립 리뷰를 거쳐야 완료로 본다. 테스트 성공만으로 리뷰를 대신하지 않는다.

## 4. 코드 소유 경계

- `crates/gitust-cli`: CLI 인자와 운영 명령.
- `crates/gitust-server`: HTTP/MCP, 인증 경계, 작업 상태와 작업 간 조정.
- `crates/gitust-core`: 공유 도메인 타입과 상태 전이.
- `crates/gitust-git`: Git 객체·refs·워크트리·검증·승인 기록·발행.
- `crates/gitust-sandbox`: 로컬 OS 샌드박스 실행과 자원 정리.
- `crates/gitust-sync`: 동기화 설정과 공개 ref 범위 정책.
- `crates/gitust-db`: 인증·권한 상태와 영속화.
- `packages/design-system`: SolidJS UI, 토큰, 실서버 클라이언트와 별도 데모 상태.

계획에만 있는 패키지나 서비스를 이미 존재한다고 가정하지 않는다. 새 의존성·추상화·설정은 요청에 필요한 경우에만 추가한다.

## 5. 단계와 인증 규칙

- [로드맵](docs/roadmap.md)의 순서에 따라 기능을 쌓고, 이전 단계의 관련 7.4 검증을 마친 뒤 다음 단계로 진행한다.
- 현재 Phase 3의 기반 기능을 구현 중이다. 기존 구현과 미완료 범위를 확인한 뒤 작업을 고른다.
- CI/CD 순서는 `sandbox run` → 저장소 Checks → 자동 실행 → GitHub Actions 호환이다.
- SSO/OIDC/LDAP·2FA, 멀티 노드, 검색 등 뒤 단계의 기능을 요청 없이 앞당겨 구현하지 않는다.
- 현재 OS 샌드박스는 신뢰하는 단일 사용자용이다. HTTP 실행의 SiteAdmin·명시적 활성화 조건을 유지하고, 다중 테넌트 격리로 광고하지 않는다.

### 인증 규칙 (Phase 2~3)

- 공개 회원가입과 "가입 후 승인" 방식은 두지 않습니다. 계정은 **관리자가 발급한 초대 링크**로만 생성합니다.
- 초대/비밀번호 재설정 링크는 메일 서버 없이 관리자가 메신저 등으로 직접 전달합니다.
- 링크 토큰은 충분히 긴 암호학적 랜덤 값이며 DB에는 해시만 저장합니다. 1회 사용, 만료(기본 72시간), 관리자 취소를 지원합니다.
- 비밀번호는 argon2로 해시하고, 로그인 시도 횟수를 제한합니다.
- 현재 Git HTTP 인증에는 개인 액세스 토큰을 사용합니다. SSH 호스팅을 구현할 때 등록된 SSH 키를 사용해야 합니다. 계정 비활성화 시 세션·SSH 키·토큰이 모두 즉시 무효화되어야 합니다.
- 첫 관리자는 CLI(`gitust admin create`)로만 생성합니다. 웹에 "첫 가입자 = 관리자" 경로를 만들지 않습니다.

## 6. 성능 예산과 측정 규칙

아래는 구현 목표이며 보장 수치가 아니다. 벤치마크로 측정하고 환경·명령·결과를 함께 기록해야 달성으로 인정한다.

| 항목 | 목표 |
|---|---|
| CoW 워크트리 생성 | 1ms 미만 |
| 준비된 샌드박스 실행 시작 | 100~200ms 미만 |
| 일반 메타데이터 API p99 | 15ms 미만 |
| Git Tree/Commit API p99 | 30ms 미만 |
| 키보드 입력 → 화면 피드백 | 16ms 미만 |
| 초기 JavaScript 번들 gzip | 50KB 미만 |

- CoW 수치는 reflink/clonefile 지원 파일시스템이 전제다. 미지원 환경의 대체 경로는 별도 측정한다.
- 샌드박스 시작 시간에 빌드·테스트 자체 시간을 섞지 않는다.
- 현재 워크트리는 Git linked worktree checkout이다. [기록된 벤치마크](docs/benchmarks/agent-worktree.md)는 1ms 목표를 달성하지 못했다.
- 보안·데이터 무결성 검사를 약화해 성능 목표를 맞추지 않는다.

## 7. Coding Conventions & Workflow Instructions

### 7.1 Rust Guidelines
- 모든 커밋 전 `cargo fmt --all -- --check`와 `cargo clippy --workspace --all-targets -- -D warnings`를 통과해야 합니다.
- 루프 내부에서 DB 쿼리를 호출하는 N+1 문제를 절대 만들지 마십시오.

### 7.2 Design System & Frontend Guidelines
- 화면은 solid-ui 컴포넌트와 Tailwind 의미 토큰으로 구성하십시오. 기존 `ds-*` 스타일이나 별도 원시 컴포넌트를 다시 만들지 마십시오. 가상 뷰어의 동적 위치·크기는 예외입니다.
- 프로덕션 UI에는 절대 유니코드 이모지를 쓰지 말고 `<Icon name="..." />` 벡터 스프라이트 프리미티브를 사용하십시오.

### 7.3 Git Branch & Commit Workflow (Single-Developer Trunk-Based `main`)
- 이 프로젝트는 1인 개발 체제로 진행되므로 **불필요한 피처 브랜치(Feature Branch)나 PR을 분리하지 않고 항상 `main` 브랜치에서 직접 작업**합니다.
- **사용자가 직접적으로 `git commit` 또는 `git push`를 요청하지 않는 한 절대 임의로 커밋·푸시하지 않습니다.**
- 각 Step의 개발이 끝나면 7.4의 검증과 7.5의 리뷰 에이전트(Subagent) 감사를 거쳐 지적 사항을 모두 해결한 상태로 워킹 트리에 유지하고, **사용자의 명시적인 요청이 있을 때만 `git commit` / `git push`를 수행**합니다.
- **"커밋/푸시해줘" 요청은 게이트 생략 허가가 아닙니다.** 요청을 받으면 반드시 다음 순서로 진행합니다.
  1. `scripts/verify.sh` 실행 (종료 코드 0 필수, `grep` 파이프로 실패를 숨기지 않음).
  2. 아직 7.5 Gate A를 통과하지 않은 변경이 있으면 독립 리뷰 Subagent를 먼저 구동하고, P0/P1을 모두 해결한 뒤 결과(Verdict)를 사용자에게 보고.
  3. 그 다음에만 commit/push. 게이트를 생략해야 할 사정이 있으면 push 전에 사용자에게 명시적으로 묻습니다.
- 저장소 pre-push hook(`.githooks/pre-push` → `scripts/verify.sh`)을 사용합니다. 새 클론에서는 `git config core.hooksPath .githooks`로 활성화하며, `--no-verify` 사용을 금지합니다.

### 7.4 Verification Gates (기능 완료 조건)
기능을 구현하기 전에 통과해야 할 테스트를 먼저 작성하고, 아래 해당 항목을 통과해야 완료로 봅니다.
- **Git 데이터 무결성**: 실제 `git` 클라이언트로 clone → commit → push → fetch 왕복 통합 테스트 후 서버·클라이언트 양쪽 `git fsck`가 통과해야 합니다.
- **권한**: 허용 경로뿐 아니라 거부 경로(다른 팀 저장소 접근, 비활성 계정, 만료·재사용된 초대 링크, 승인 없는 Outbound 동기화)가 반드시 막히는지 테스트합니다.
- **샌드박스 격리**: 샌드박스 밖 파일 읽기/쓰기, 호스트 시크릿·환경 변수 접근, 허용되지 않은 네트워크 접근, 프로세스 탈출 시도가 실패하는지 테스트합니다.
- **동기화**: 미러 결과가 원본 GitHub 저장소의 ref·커밋과 일치하는지 비교 검증합니다.
- **백업/복구**: Phase 3부터 백업에서 실제로 복구하는 리허설 테스트를 둡니다.
- **성능**: 6장 수치를 주장하는 변경은 벤치마크 결과를 함께 남깁니다.
- **리뷰 PoC의 회귀 테스트화**: 독립 리뷰에서 `[Verified]`로 보고된 모든 P0/P1은 해당 PoC를 저장소 테스트로 옮겨, 수정 전 코드에서 실패하고 수정 후 통과함을 확인합니다. 같은 버그 유형의 다른 위치도 함께 찾아 고칩니다(`adversarial-audit` 2.5 Fix-Sweep Rule).
- **Fail-closed**: 인증·영속화·샌드박스·Git·동기화 경로에서 오류를 "변경 없음/허용/건너뜀"으로 바꾸지 않습니다. `scripts/audit-fail-open.sh`가 이를 기계적으로 검사합니다.

### 7.5 Mandatory Per-Step Independent Review Gate (코드 품질·성능·아키텍처 감사)
각 Step이나 작업 개발이 끝날 때마다 **`git commit` / `git push` 전에 `adversarial-audit` 스킬을 적용한 독립 Subagent로 Gate A 감사**를 수행하고, 발견된 P0/P1 이슈를 모두 수정해야 합니다. 형식적인 고무도장(Rubber-stamp) 통과를 엄격히 금지합니다.

- **Gate A — 코드 품질·성능·아키텍처 감사 ([`adversarial-audit` 스킬](.agents/skills/adversarial-audit/SKILL.md))**:
  1. **속도 (Blazing Speed)**: 락 경합(Lock Contention), 비동기 런타임 내 블로킹 I/O 및 CPU 병목, N+1 쿼리, 불필요한 메모리 할당/복사 여부.
  2. **안정성 (Stability & Edge Cases)**: `unwrap()`/`expect()` 제거, `spawn_blocking` 취소 시 가드/리소스 누수, 경계값 및 엣지케이스 처리 여부.
  3. **스파게티 코드 방지 (Clean Architecture & Hard `<600` Line Gate)**: 모든 Rust/TS/TSX 소스 및 테스트 파일은 **단일 파일 600줄 미만(`wc -l < 600`)을 강제(Hard P1 Gate)** 하며, 다중 맵 락 순서 조합(`AppState` 등)은 반드시 단일 상태 전이 메서드로 캡슐화하고, 실서버 API 클라이언트와 쇼케이스 데모 상태를 한 파일에 섞지 않으며, 상태 이원화(Split-Brain) 및 허위 측정치를 엄격히 금지합니다.
- **Gate B 중단**: 2026-10-17 사용자 지시에 따라 Gate B와 `redteam-pentest` Subagent를 자동 실행하지 않습니다. 이후 사용자가 별도로 요청할 때만 수행합니다. 7.4의 기능·권한·격리·실패 경로 검증과 Gate A는 계속 유지하며, 과거 Gate B 미완료 기록을 통과로 바꾸지 않습니다.
