# AGENTS.md — Gitust (Gitea in Rust + Superhuman-Grade UX)

## 1. Project Vision & Identity

**Gitust**는 과거처럼 "사람이 마우스로 버튼 누르고 하루에 PR 몇 개 올리는 시대"의 GitHub 클론이 아닙니다.
**Gitust는 기업이 자체 서버(On-Premise / Private Cloud)에 구축하여, 바이브 코딩 에이전트(Claude Code, Cursor, Codex, 사내 AI 에이전트 등)들이 수백 개의 작업 브랜치를 동시에 생성하고 초고속으로 빌드·검증(Sandbox Execution)하며, 인간 개발자는 이를 Superhuman 속도로 지휘·승인(Triage & Merge)할 수 있도록 최적화된 "Agent-Native Enterprise Git + 초경량 샌드박스 실행 엔진"**입니다.

오픈소스 셀프호스팅 Git 서비스인 [Gitea](https://github.com/go-gitea/gitea)의 모든 엔터프라이즈 도메인 기능(자체 회원가입/로그인 제어, 사내 조직/팀 권한 관리, 사이트 관리자(Site Admin) 시스템, 내장 CI/CD Actions, 단일 바이너리 배포)을 완벽히 계승하면서, 다음 **세 가지 핵심 차별점**으로 완전히 새로운 시장을 선점합니다.

1. **Agent-Native Git & Sub-100ms Sandbox Execution Engine (핵심 게임 체인저)**
   - **Zero-Copy Mass Branching (`< 1ms` 스냅샷 포크)**: AI 에이전트 수백 개가 동시에 포크/브랜치를 생성해도 디스크 복사가 일어나지 않도록, `gitoxide`(`gix`) 인메모리 객체 공유 및 Copy-on-Write(CoW) 워크트리 스냅샷을 통해 `< 1ms` 만에 격리된 에이전트 작업 브랜치를 수백 개 스폰합니다.
   - **Ultra-Fast Sandbox Verification (`crates/gitust-sandbox`)**: 무거운 레거시 CI 컨테이너 Cold-boot(수 분 소요) 대신, 에이전트가 수정한 코드를 즉시 컴파일·테스트·정적분석하고 구조화된 피드백(AST 에러/로그)을 돌려주는 초경량 샌드박스 실행 엔진을 내장합니다.
   - **Agent-First Protocol (MCP & Batch Git RPC)**: 웹 UI뿐 아니라 Claude Code, Cursor 등이 직접 연결되는 내장 **MCP(Model Context Protocol) 서버** 및 **단일 왕복(Single-Roundtrip) 인메모리 커밋·검증 RPC**를 기본 탑재합니다.
2. **Design System First & "Speed is a Feature" (Superhuman-Grade Triage UX)**
   - Gitea의 기존 레거시 템플릿/UI(Fomantic-UI + jQuery/Vue 혼용)를 복제하지 않고, **독자적인 초고속 디자인 시스템(`@gitust/design-system`)**을 가장 먼저 구축합니다.
   - 수백 개의 에이전트가 동시에 쏟아내는 브랜치와 PR을 인간 개발자가 지치지 않고 검수·지휘하려면 **"속도가 곧 핵심 기능(Speed is a Feature)"**이어야 합니다. 모든 인터랙션은 **16ms(1프레임) ~ 100ms 이내**에 반응하며, 키보드 퍼스트(Keyboard-First) 네비게이션, 낙관적 업데이트(Optimistic UI), 프리페칭(Prefetching), 제로 레이아웃 시프트(Zero CLS)를 기본 원칙으로 삼습니다.
3. **Written in Idiomatic Rust (Zero-Cost, Memory-Safe, Blazing Fast)**
   - Go 언어로 작성된 Gitea를 **100% Rust**로 재창조합니다.
   - Go 코드를 기계적으로 1:1 번역(Line-by-line port)하는 것을 엄격히 금지합니다. Gitea의 도메인 모델, 엣지 케이스, Git 처리 방식만 참고하되, 설계와 구현은 **Rust의 타입 시스템, 소유권(Ownership), 비동기 런타임(`tokio`), 제로 코스트 추상화**를 극대화하는 **Idiomatic Rust** 방식으로 작성합니다.

---

## 2. Core Principles for AI Agents

이 저장소에서 작업하는 모든 AI 에이전트는 코드를 작성하거나 아키텍처를 제안할 때 반드시 아래의 5대 원칙을 준수해야 합니다.

### Principle 1: Speed is the Ultimate Feature (< 100ms / 16ms Frame Rule)
- **Instant Feedback**: 사용자의 클릭이나 키 입력 후 네트워크 응답을 기다리며 스피너(Spinner)를 보여주는 것은 실패한 UX입니다. 가능한 모든 상태 변경(에이전트 브랜치 승인/머지, 샌드박스 재실행, 이슈 생성/닫기, 사용자 권한 변경 등)은 **Optimistic UI** 또는 **Local-First 캐시**로 즉시(0~16ms 내) 화면에 반영되어야 합니다.
- **Keyboard-First Everything**: 마우스 없이 모든 워크플로우(에이전트 스웜 지휘, 레포지토리 이동, 코드 검색, PR 리뷰, 파일 트리 탐색, CI/CD 로그 탐색, 관리자 콘솔 제어)를 완수할 수 있어야 합니다. 전역 Command Palette (`Cmd/Ctrl + K`)와 시퀀스 단축키(`g w` 에이전트 스웜/샌드박스, `g i` 이슈, `g d` 디프, `g a` Actions CI/CD, `g s` Admin/보안 등)를 디자인 시스템 코어에 내장합니다.
- **Zero Bloat**: 무거운 런타임, 불필요한 리렌더링, 거대한 번들 사이즈를 거부합니다. 프론트엔드와 백엔드 모두 가장 가볍고 빠른 선택지를 우선합니다.

### Principle 2: Design System First (No Ad-hoc UI)
- **선(先) 디자인 시스템, 후(後) 페이지 개발**: 개별 페이지를 만들 때 일회성 스타일이나 하드코딩된 색상/간격을 절대 사용하지 마십시오.
- 모든 UI 컴포넌트는 `packages/design-system`에 **디자인 토큰(Tokens) -> 기본 프리미티브(Primitives) -> 복합 패턴(Patterns)** 순서로 먼저 정의되고 검증되어야 합니다.
- **고밀도(High-Density) & 고가독성**: 개발자 도구 및 에이전트 관제 콘솔 특성상 한 화면에 많은 정보(수백 개 에이전트 브랜치 상태, 샌드박스 리소스/통과율, 코드 라인, 커밋 해시, 빌드 로그, 사내 권한 테이블)가 표시되어야 합니다. Linear 및 Superhuman처럼 정교한 타이포그래피와 높은 정보 밀도를 가진 UI를 지향합니다.

### Principle 3: Idiomatic Rust over Go Translation
- Gitea의 Go 코드는 **"무엇을(What) 해결해야 하는가"**에 대한 레퍼런스일 뿐, **"어떻게(How) 작성할 것인가"**의 정답이 아닙니다.
- Go의 `interface{}`, 암묵적 nil 체크, 전역 상태 패턴을 Rust로 그대로 가져오지 마십시오.
- 유효하지 않은 상태를 컴파일 타임에 방지(Make invalid states unrepresentable)하기 위해 Rust의 `enum`, Newtype 패턴(예: `RepoId(i64)`, `SandboxId(Uuid)`, `CommitSha(String)`), `Result<T, E>`를 적극 활용하십시오.
- `unwrap()` 및 `expect()`는 테스트 코드나 초기화 시점의 불변 조건 검증 외에는 프로덕션 경로에서 절대 사용을 금지합니다.

### Principle 4: Zero-Copy Git & High-Concurrency Agent Sandbox Engine
- 수백 개의 코딩 에이전트가 동시에 브랜치를 따고 커밋을 밀어넣을 때 병목이 발생하지 않도록, 순수 Rust 기반의 [`gitoxide` (`gix`)](https://github.com/Byron/gitoxide)를 최우선으로 활용하여 인프로세스(In-process)에서 제로 카피로 Git 객체를 읽고 멀티 브랜치 트랜잭션을 처리합니다.
- 에이전트 작업 검증을 위해 무거운 VM 부팅 대신 **네임스페이스/cgroups/WASM/Firecracker 또는 캐시된 CoW 워크트리 기반의 초경량 샌드박스 실행기(`crates/gitust-sandbox`)**를 통해 수백 개의 빌드·테스트를 병렬 실행합니다.

### Principle 5: Enterprise Self-Hosted Ready & Open-Core Monetization
- **간편한 사내 구축**: 기업 보안팀이나 인프라팀이 외부 의존성 없이 단일 바이너리(또는 단일 컨테이너) 하나로 사내 서버에 즉시 구축할 수 있어야 합니다.
- **엔터프라이즈 필수 통제권 기본 내장**:
  1. **Auth & 가입 정책 제어**: 사내 정책에 따른 회원가입 허용/차단(Invite-only), 허용 이메일 도메인 화이트리스트(`@company.com`), 2FA/Passkey, SSO(OIDC/OAuth2/LDAP), 에이전트 전용 API 키/스코프 토큰 관리.
  2. **Site Admin & Agent Governance Console**: 전체 사용자/조직/레포지토리 통제, 에이전트 스웜 동시 실행 쿼터(Quota) 및 보안 정책(AI 코드 반출 방지, 허용 명령어 정책), 보안 감사 로그(Audit Log) 조회.
  3. **Built-in CI/CD (`Gitust Actions`) & Agent Sandbox Pool**: `.github/workflows/*.yml`과 100% 호환되는 CI/CD 파이프라인 및 에이전트 전용 초고속 샌드박스 워커 풀을 제공합니다.

---

## 3. Recommended Tech Stack & Architecture

### 3.1 Backend (Rust Workspace)
| 영역 | 권장 기술 스택 | 선정 이유 |
| :--- | :--- | :--- |
| **Async Runtime** | `tokio` | 수백 개 에이전트 동시 세션을 처리하는 산업 표준 고성능 비동기 런타임 |
| **Web / RPC / MCP** | `axum` + `tower` + JSON-RPC/MCP | 제로 코스트 라우팅, WebSocket 스트리밍, Claude Code/Cursor용 내장 MCP 서버 |
| **Database / Query** | `sqlx` (PostgreSQL / SQLite) | 컴파일 타임 SQL 검증, 비동기 커넥션 풀링, 최소한의 런타임 오버헤드 |
| **Git Engine** | `gix` (gitoxide) + CoW Snapshot | 순수 Rust 기반 최고 속도의 Git 객체 탐색 및 `<1ms` 멀티 브랜치 스폰 |
| **Sandbox Engine** | `crates/gitust-sandbox` | 에이전트 브랜치별 격리 빌드/테스트 실행, 캐시 마운트, 실시간 리소스/로그 제어 |
| **SSH Server** | `russh` | 외부 OpenSSH 데몬 의존 없이 단일 바이너리 내장 비동기 SSH Git 서버 구현 |
| **Serialization** | `serde` + `sonic-rs` (또는 `serde_json`) | 초고속 JSON 직렬화/역직렬화 |
| **Search / Indexing** | `tantivy` | 에이전트가 코드베이스 전체를 `<5ms` 내에 검색할 수 있는 내장 전문/심볼 검색 엔진 |
| **Error Handling** | `thiserror` (라이브러리/도메인) + `anyhow` (바이너리/CLI) | 명확하고 구조화된 에러 전파 |
| **Observability** | `tracing` + `tracing-subscriber` | 마이크로초 단위의 레이턴시 추적 및 에이전트 샌드박스 리소스 프로파일링 |

### 3.2 Frontend & Design System ("Superhuman" Speed Stack — 확정)
Superhuman 수준의 체감 속도를 달성하기 위해 무거운 가상 DOM(VDOM) 오버헤드를 피하고 **Fine-grained Reactivity(세밀한 반응성)**와 **Local-First 캐싱**을 결합합니다.

- **UI Runtime (확정 표준)**:
  - **SolidJS + TypeScript + Vite**: 가상 DOM(VDOM) 없이 실제 DOM 노드를 핀포인트 업데이트하여 프레임 드랍 없는 120fps 인터랙션을 보장하고, 초기 JS 번들을 20~25KB(Gzip) 내외로 유지합니다.
- **Icon System (확정 표준)**:
  - **Zero-Dependency SVG `<symbol>` Sprite (`<Icon />`)**: 이모지나 무거운 외부 아이콘 컴포넌트 런타임을 사용하지 않고, `<defs><symbol>` 스프라이트와 `<use href="#ds-icon-..." />` GPU 인스턴싱을 사용하여 수천 개의 파일 트리/에이전트 브랜치 행에서도 DOM 노드 증가 없이 즉시 렌더링합니다.
- **Sync & Data Layer**:
  - 단순 REST 호출의 나열이 아닌, **In-Memory Normalized Cache + WebSocket/SSE Delta Sync**를 통해 화면 전환 시 네트워크 대기 시간을 **0ms**로 수렴시킵니다.
- **Styling & Design Tokens**:
  - **Zero-Runtime CSS**: 런타임 CSS-in-JS는 엄격히 금지하며, **CSS Custom Properties (Design Tokens) + Vanilla CSS**만 허용합니다.
- **Code & Diff Viewer**:
  - 수만 줄의 PR Diff나 에이전트 생성 패치를 60~120fps로 스크롤하기 위해 **가상 스크롤링(Virtualization)**과 백그라운드 신택스 하이라이팅을 기본 적용합니다.

---

## 4. Proposed Repository Structure (Monorepo)

```text
gitust/
├── AGENTS.md                  # AI 에이전트 핵심 지침서 (현재 문서)
├── Cargo.toml                 # Rust Cargo Workspace 루트
├── crates/
│   ├── gitust-server/         # 메인 실행 바이너리 (Axum HTTP/WS 서버, MCP 서버, SSH 서버 부트스트랩)
│   ├── gitust-api/            # REST / Batch Git RPC / MCP 핸들러, 미들웨어, DTO
│   ├── gitust-core/           # 순수 도메인 모델 (User, Org, Repo, AgentSession, SandboxRun, Issue, PR)
│   ├── gitust-db/             # 데이터베이스 스키마, 마이그레이션, 리포지토리 구현체
│   ├── gitust-git/            # gitoxide(gix) 래퍼, <1ms CoW 멀티 브랜치 스냅샷, Diff/Blame 엔진
│   ├── gitust-sandbox/        # [핵심] 코딩 에이전트 전용 초경량 병렬 샌드박스 빌드/검증 실행 엔진
│   ├── gitust-search/         # Tantivy 기반 코드/심볼/이슈 초고속 인덱싱 및 에이전트 컨텍스트 검색
│   ├── gitust-actions/        # Gitea Actions / GitHub Actions 호환 CI/CD 러너 및 워크플로우 스케줄러
│   └── gitust-packages/       # 패키지 레지스트리 (Cargo, npm, Container, Maven 등)
├── packages/
│   ├── design-system/         # [Phase 1] 토큰, 프리미티브 컴포넌트, 키보드 엔진, 에이전트 스웜/샌드박스 뷰
│   ├── sync-client/           # Optimistic UI, 인메모리 캐시, 프리페칭, 실시간 스트리밍 클라이언트
│   └── web/                   # Gitust 웹 애플리케이션 (디자인 시스템 위에서 조립되는 뷰)
└── docs/
    ├── architecture/          # ADR 및 Agent-Native Sandbox 설계 문서
    ├── design-system/         # 디자인 토큰 명세, 인터랙션 가이드라인, 단축키 맵
    └── business/              # 엔터프라이즈 수익화(Open-Core) 및 라이선스 정책
```

---

## 5. Development Roadmap & Phase Rules

### Phase 1: Design System & Interaction Engine Foundation (최우선 순위 — 완료/진행 중)
1. **Design Tokens 정의**: Color(Dark/Light 고대비 테마), Typography, Spacing, Motion(<= 100ms).
2. **Keyboard & Command Engine**: 전역 단축키 매니저(`g w`, `g i`, `g d`, `g a`, `g s` 등) 및 Command Palette (`Cmd+K`).
3. **Core UI Primitives**: Icon(SVG `<symbol>` Sprite), Kbd, Button, Input, Badge, Avatar, Dialog, Tabs.
4. **Domain-Specific High-Performance Primitives**:
   - **Agent Swarm & Sandbox Fleet Control (`g w`)**: Claude Code, Cursor 등 수백 개의 병렬 작업 브랜치 상태, `<1ms` 스냅샷 생성, 실시간 샌드박스 빌드/테스트 검증 결과, 키보드 기반 초고속 승인/머지(`a`) 패턴.
   - **Virtualized File Tree (`g t`)** & **Virtualized Code & Diff Viewer (`g d`)**.
   - **Instant Filter Bar & Issue/PR List (`g i`)**.
   - **Gitust Actions CI/CD Pipeline & Virtualized Log Streamer (`g a`)**.
   - **Enterprise Auth, SSO & Site Admin Patterns (`g s`)**.

### Phase 2: Rust Core, Zero-Copy Git & Agent Sandbox Engine
1. Workspace 초기화 및 `gitust-core`, `gitust-git`, `gitust-sandbox`, `gitust-db`, `gitust-server` 스캐폴딩.
2. `gix` 기반 `<1ms` 에이전트 병렬 브랜치/스냅샷 생성 및 Commit/Tree/Diff 추출 API 구축.
3. 코딩 에이전트용 초경량 샌드박스 실행기(`gitust-sandbox`) 및 MCP(Model Context Protocol) / Batch RPC 연동.
4. 엔터프라이즈 인증/인가(Invite-only, 도메인 제한, SSO, 에이전트 스코프 토큰, Site Admin) 구현.

### Phase 3: Core Git Hosting & Collaboration (Gitea Feature Parity)
1. **Repository UI & Code Browsing**: 브랜치/태그 관리, 커밋 그래프, 코드 탐색, 초고속 파일 파인더(`t` 단축키).
2. **Issues & Pull Requests**: 마크다운 에디터, 라벨, 마일스톤, 프로젝트 보드(칸반), PR 코드 리뷰, 머지 전략(Merge, Rebase, Squash).
3. **Organizations & Teams**: 세분화된 권한 모델(Owner, Admin, Write, Read, Triage).

### Phase 4: Ecosystem & Advanced Features
1. **Gitust Actions**: Gitea Actions / GitHub Actions 호환 워크플로우 실행 및 실시간 로그 스트리밍.
2. **Package Registry & Releases**: 컨테이너, Cargo, npm 등 패키지 호스팅 및 릴리스 자산 관리.
3. **Migration Tools**: GitHub, GitLab, 기존 Gitea 인스턴스로부터의 무중단 고속 마이그레이션 도구.

---

## 6. Performance Budgets & Quality Gates (성능 예산)

에이전트가 작성하는 모든 기능은 **항상 Superhuman 수준의 극한 속도(Blazing Speed)를 최우선으로 지향**하며 아래의 정량적 성능 기준을 충족해야 합니다.

- **API 응답 시간 (p99)**:
  - 일반 메타데이터 조회(이슈 목록, 레포 정보 등): **< 15ms** (로컬/동일 리전 기준)
  - Git Tree / Commit 목록 조회 (`gix` 활용): **< 30ms**
- **UI 인터랙션 레이턴시**:
  - 키보드 단축키 입력 -> 화면 피드백: **< 16ms (1 프레임)**
  - 캐시된 라우트 간 페이지 전환: **< 30ms**
  - Command Palette (`Cmd+K`) 오픈 및 검색 필터링: **< 16ms**
- **프론트엔드 번들 사이즈**:
  - 초기 로드 JS (Gzip/Brotli 압축 기준): **< 50KB**를 목표로 하며, 무거운 기능(예: 차트, 복잡한 에디터)은 지연 로딩(Lazy chunk)하되 유휴 시간에 백그라운드 프리로드합니다.

---

## 7. Coding Conventions & Workflow Instructions

### 7.1 Rust Guidelines
- **Formatting & Linting**: 모든 커밋 전 `cargo fmt --all -- --check`와 `cargo clippy --workspace --all-targets -- -D warnings`를 통과해야 합니다.
- **Unsafe Code**: 성능 크리티컬한 제로 카피 파싱 외에는 `unsafe` 블록 사용을 자제하며, 불가피하게 사용할 경우 반드시 `// SAFETY: ...` 주석으로 불변 조건을 증명해야 합니다.
- **SQL & N+1 방지**: 루프 내부에서 DB 쿼리를 호출하는 N+1 문제를 절대 만들지 마십시오. `JOIN` 또는 배치 로더(DataLoader 패턴)를 사용하여 단일 라운드트립으로 데이터를 가져오십시오.
- **Testing**: 도메인 로직(`gitust-core`)과 Git 엔진(`gitust-git`)은 반드시 단위 테스트 및 통합 테스트를 동반해야 합니다.

### 7.2 Design System & Frontend Guidelines
- **Token Enforcement**: 컴포넌트 CSS에 `#ffffff`, `14px` 같은 매직 넘버를 직접 쓰지 말고, 반드시 `--ds-bg-primary`, `--ds-font-size-sm` 등의 디자인 토큰 변수를 사용하십시오.
- **No Emojis in UI**: 프로덕션 UI 및 컴포넌트에는 절대 유니코드 이모지(`📄`, `💬` 등)를 쓰지 말고, 반드시 GPU 인스턴싱 기반의 `<Icon name="..." />` 벡터 스프라이트 프리미티브를 사용하십시오.
- **No Layout Shift**: 데이터 로딩 전후로 레이아웃이 흔들리는 현상(CLS)이 없도록 정확한 높이 계산 또는 고정 레이아웃 골격을 유지하십시오.
- **Shortcut Registration**: 새로운 뷰나 액션을 추가할 때는 반드시 "이 기능을 마우스 없이 키보드만으로 어떻게 1초 안에 실행할 수 있는가?"를 먼저 정의하고 단축키 레지스트리에 등록하십시오.

### 7.3 How to Reference Gitea (Upstream)
- Gitea의 비즈니스 규칙(예: PR 머지 가능 여부 판단 로직, 브랜치 보호 규칙, 웹훅 페이로드 포맷, API 호환성)을 확인할 때는 Gitea의 Go 소스코드(`models/`, `services/`, `modules/git/`, `routers/`)를 참조하십시오.
- 단, 참조한 후에는 **"이 도메인 요구사항을 가장 빠르고 안전한 Rust 코드로 표현하려면 어떻게 해야 하는가?"**를 기준으로 처음부터 깔끔하게 설계하십시오.

### 7.4 Git Branch & Commit Workflow (Single-Developer Trunk-Based `main`)
- 이 프로젝트는 1인 개발 체제로 진행되므로 **불필요한 피처 브랜치(Feature Branch)나 PR을 분리하지 않고 항상 `main` 브랜치에서 직접 작업, 커밋, 푸시(`git push origin main`)**합니다.
- 모든 주요 단계(Phase)나 기능 단위가 검증되면 타입 체크 및 빌드를 확인한 뒤 즉시 `main` 브랜치에 깔끔한 커밋 메시지로 반영하십시오.


---

## 8. Business & Monetization Architecture (Open-Core + Enterprise Agent Infrastructure)

아키텍처 설계 시 오픈소스 코어(Community Edition)와 유료 수익 모델(Enterprise / Cloud Edition)의 경계를 명확히 고려하여 모듈화합니다.

1. **Community Edition (Open Source — 시장 장악 및 표준 선점)**
   - 단일 노드 셀프호스팅 Git 서버, Superhuman 디자인 시스템 UI, 기본 `Gitust Actions`, 소규모 에이전트 동시 브랜치/샌드박스 실행(예: 동시 4~8개 샌드박스 워커).
2. **Enterprise On-Premise License (핵심 수익원 #1 — Seat + Agent Concurrency Node 과금)**
   - **High-Density Agent Sandbox Cluster Orchestrator**: 수백~수천 개의 Claude Code/Cursor 에이전트 브랜치를 다중 워커 노드에 분산 스케줄링하고 공유 빌드 캐시(Remote Build/Crate/npm Cache)를 제공하는 클러스터 엔진.
   - **Enterprise Governance & Security Policy**: SAML/OIDC/LDAP 그룹 동기화, 에이전트 전용 RBAC(특정 디렉토리/시크릿 접근 차단), AI 생성 코드 감사 로그(Audit Trail) 및 보안 취약점 게이트키핑.
3. **Managed Dedicated Sandbox Cloud (핵심 수익원 #2 — Usage-based Compute 과금)**
   - 자체 빌드 클러스터를 운영하기 부담스러운 중견/스타트업을 위해, 코드는 사내에 두거나 프라이빗 볼륨에 두고 **초고속 Rust/Firecracker 샌드박스 연산 시간(vCPU-초)만 종량제로 과금**하는 하이브리드 클라우드 러너.
