# AGENTS.md — Gitust (Gitea in Rust + Superhuman-Grade UX)

## 1. Project Vision & Identity

**Gitust**는 불특정 다수를 위한 퍼블릭 클라우드 SaaS가 아니라, **기업이나 조직이 보안·망분리·컴플라이언스·자체 코드 자산 보호를 위해 자사 온프레미스(On-Premise) 서버나 프라이빗 클라우드에 가장 쉽고 빠르게 구축하는 "기업 전용 초고속 프라이빗 GitHub (Self-Hosted Enterprise Git Platform)"**입니다.

오픈소스 셀프호스팅 Git 서비스인 [Gitea](https://github.com/go-gitea/gitea)의 모든 엔터프라이즈 도메인 기능(자체 회원가입/로그인 제어, 사내 조직/팀 권한 관리, 사이트 관리자(Site Admin) 시스템, 내장 CI/CD Actions, 단일 바이너리 배포)을 완벽히 계승하면서, 다음 **두 가지 핵심 차별점**으로 새롭게 구축합니다.

1. **Design System First & "Speed is a Feature" (Superhuman-Grade UI/UX)**
   - Gitea의 기존 레거시 템플릿/UI(Fomantic-UI + jQuery/Vue 혼용)를 복제하지 않습니다.
   - 본격적인 화면 개발에 앞서 **우리만의 독자적인 디자인 시스템(Design System)**을 가장 먼저 구축합니다.
   - 이메일 클라이언트 **Superhuman**이 증명했듯 **"속도는 곧 핵심 기능(Speed is a Feature)"**입니다. 모든 인터랙션은 **100ms(이상적으로는 16ms/1프레임) 이내**에 반응해야 하며, 키보드 퍼스트(Keyboard-First) 네비게이션, 낙관적 업데이트(Optimistic UI), 프리페칭(Prefetching), 제로 레이아웃 시프트(Zero CLS)를 기본 원칙으로 삼습니다.
2. **Written in Idiomatic Rust (Zero-Cost, Memory-Safe, Blazing Fast)**
   - Go 언어로 작성된 Gitea를 **100% Rust**로 재창조합니다.
   - Go 코드를 기계적으로 1:1 번역(Line-by-line port)하는 것을 엄격히 금지합니다. Gitea의 도메인 모델, 엣지 케이스, Git 처리 방식만 참고하되, 설계와 구현은 **Rust의 타입 시스템, 소유권(Ownership), 비동기 런타임(`tokio`), 제로 코스트 추상화**를 극대화하는 **Idiomatic Rust** 방식으로 작성합니다.

---

## 2. Core Principles for AI Agents

이 저장소에서 작업하는 모든 AI 에이전트는 코드를 작성하거나 아키텍처를 제안할 때 반드시 아래의 5대 원칙을 준수해야 합니다.

### Principle 1: Speed is the Ultimate Feature (< 100ms Rule)
- **Instant Feedback**: 사용자의 클릭이나 키 입력 후 네트워크 응답을 기다리며 스피너(Spinner)를 보여주는 것은 실패한 UX입니다. 가능한 모든 상태 변경(이슈 생성/닫기, 라벨 변경, 코멘트 작성, 사용자 권한 변경, CI 재실행 등)은 **Optimistic UI** 또는 **Local-First 캐시**로 즉시(0~16ms 내) 화면에 반영되어야 합니다.
- **Keyboard-First Everything**: 마우스 없이 모든 워크플로우(레포지토리 이동, 코드 검색, PR 리뷰, 파일 트리 탐색, 브랜치 전환, 이슈 필터링, CI/CD 로그 탐색, 관리자 콘솔 제어)를 완수할 수 있어야 합니다. 전역 Command Palette (`Cmd/Ctrl + K`)와 시퀀스 단축키(`g i` 이슈, `g d` 디프, `g a` Actions CI/CD, `g s` Admin/보안 등)를 디자인 시스템 코어에 내장합니다.
- **Zero Bloat**: 무거운 런타임, 불필요한 리렌더링, 거대한 번들 사이즈를 거부합니다. 프론트엔드와 백엔드 모두 가장 가볍고 빠른 선택지를 우선합니다.

### Principle 2: Design System First (No Ad-hoc UI)
- **선(先) 디자인 시스템, 후(後) 페이지 개발**: 개별 페이지(이슈 목록, PR 디프 뷰, CI/CD 파이프라인, 관리자 콘솔 등)를 만들 때 일회성 스타일이나 하드코딩된 색상/간격을 절대 사용하지 마십시오.
- 모든 UI 컴포넌트는 `packages/design-system`(또는 해당 워크스페이스)에 **디자인 토큰(Tokens) -> 기본 프리미티브(Primitives) -> 복합 패턴(Patterns)** 순서로 먼저 정의되고 검증되어야 합니다.
- **고밀도(High-Density) & 고가독성**: 개발자 도구 및 엔터프라이즈 콘솔 특성상 한 화면에 많은 정보(코드 라인, 커밋 해시, CI 파이프라인 상태, 빌드 로그, 사내 사용자/권한 테이블)가 표시되어야 합니다. Linear 및 Superhuman처럼 정교한 타이포그래피와 높은 정보 밀도를 가진 UI를 지향합니다.

### Principle 3: Idiomatic Rust over Go Translation
- Gitea의 Go 코드는 **"무엇을(What) 해결해야 하는가"**에 대한 레퍼런스일 뿐, **"어떻게(How) 작성할 것인가"**의 정답이 아닙니다.
- Go의 `interface{}`, 암묵적 nil 체크, 전역 상태 패턴을 Rust로 그대로 가져오지 마십시오.
- 유효하지 않은 상태를 컴파일 타임에 방지(Make invalid states unrepresentable)하기 위해 Rust의 `enum`, Newtype 패턴(예: `RepoId(i64)`, `CommitSha(String)`), `Result<T, E>`를 적극 활용하십시오.
- `unwrap()` 및 `expect()`는 테스트 코드나 초기화 시점의 불변 조건 검증 외에는 프로덕션 경로에서 절대 사용을 금지합니다.

### Principle 4: Zero-Copy & High-Performance Git Operations
- Git 호스팅 서비스의 병목은 대개 `git` 서브프로세스 스폰과 대용량 Diff/Blob 파싱에서 발생합니다.
- 순수 Rust 기반의 [`gitoxide` (`gix`)](https://github.com/Byron/gitoxide)를 최우선으로 활용하여 인프로세스(In-process)에서 제로 카피로 Git 객체(Commit, Tree, Blob, Tag, Packfile)를 읽고 처리합니다.
- `gix`에서 아직 지원하지 않는 복잡한 쓰기 작업(예: 특정 머지 전략, 훅 실행을 동반한 receive-pack 등)에 한해서만 격리된 비동기 `git` CLI 프로세스 풀 또는 `git2`를 제한적으로 사용하며, 이를 추상화 계층(`crates/gitust-git`) 뒤에 숨깁니다.

### Principle 5: Enterprise Self-Hosted Ready & Single-Binary Heritage
- **간편한 사내 구축**: 기업 보안팀이나 인프라팀이 외부 의존성 없이 단일 바이너리(또는 단일 컨테이너) 하나로 사내 서버에 즉시 구축할 수 있어야 합니다.
- **엔터프라이즈 필수 통제권 기본 내장**:
  1. **Auth & 가입 정책 제어**: 사내 정책에 따른 회원가입 허용/차단(Invite-only), 허용 이메일 도메인 화이트리스트(`@company.com`), 2FA/Passkey, SSO(OIDC/OAuth2/LDAP), SSH/GPG 키 및 PAT 관리.
  2. **Site Admin Console**: 전체 사용자/조직/레포지토리 통제, 셀프호스팅 러너(Runner) 관리, 보안 감사 로그(Audit Log) 조회.
  3. **Built-in CI/CD (`Gitust Actions`)**: 별도의 Jenkins나 외부 CI 툴을 구축할 필요 없이, GitHub Actions / Gitea Actions 워크플로우(`.github/workflows/*.yml`)와 100% 호환되는 내장 CI/CD 파이프라인 및 실시간 로그 스트리밍을 제공합니다.

---

## 3. Recommended Tech Stack & Architecture

아키텍처 세부 사항이 확정되지 않은 모듈을 새로 생성할 때는 아래의 **초고속·초경량 권장 스택**을 기준으로 설계합니다.

### 3.1 Backend (Rust Workspace)
| 영역 | 권장 기술 스택 | 선정 이유 |
| :--- | :--- | :--- |
| **Async Runtime** | `tokio` | 산업 표준 고성능 비동기 런타임 |
| **Web Framework** | `axum` + `tower` | 제로 코스트 라우팅, 뛰어난 타입 안정성, 미들웨어 생태계 |
| **Database / Query** | `sqlx` (또는 다중 DB 추상화 시 `sea-orm`) | 컴파일 타임 SQL 검증, 비동기 커넥션 풀링, 최소한의 런타임 오버헤드 (PostgreSQL / SQLite 지원) |
| **Git Engine** | `gix` (gitoxide) + async `git` CLI fallback | 순수 Rust 기반 최고 속도의 Git 객체 탐색 및 메모리 안전성 |
| **SSH Server** | `russh` | 외부 OpenSSH 데몬 의존 없이 단일 바이너리 내장 비동기 SSH Git 서버 구현 |
| **Serialization** | `serde` + `sonic-rs` (또는 `serde_json`) | 초고속 JSON 직렬화/역직렬화 |
| **Search / Indexing** | `tantivy` | Lucene을 대체하는 순수 Rust 초고속 전문 검색(Full-text & Code search) 엔진 내장 |
| **Error Handling** | `thiserror` (라이브러리/도메인) + `anyhow` (바이너리/CLI) | 명확하고 구조화된 에러 전파 |
| **Observability** | `tracing` + `tracing-subscriber` | 마이크로초 단위의 레이턴시 추적 및 병목 지점 프로파일링 |

### 3.2 Frontend & Design System ("Superhuman" Speed Stack — 확정)
Superhuman 수준의 체감 속도를 달성하기 위해 무거운 가상 DOM(VDOM) 오버헤드를 피하고 **Fine-grained Reactivity(세밀한 반응성)**와 **Local-First 캐싱**을 결합합니다.

- **UI Runtime (확정 표준)**:
  - **SolidJS + TypeScript + Vite**: 가상 DOM(VDOM) 없이 실제 DOM 노드를 핀포인트 업데이트하여 프레임 드랍 없는 120fps 인터랙션을 보장하고, 초기 JS 번들을 20KB(Gzip) 내외로 유지합니다.
- **Icon System (확정 표준)**:
  - **Zero-Dependency SVG `<symbol>` Sprite (`<Icon />`)**: 이모지나 무거운 외부 아이콘 컴포넌트 런타임을 사용하지 않고, `<defs><symbol>` 스프라이트와 `<use href="#ds-icon-..." />` GPU 인스턴싱을 사용하여 수천 개의 파일 트리/이슈 행에서도 DOM 노드 증가 없이 즉시 렌더링합니다.
- **Sync & Data Layer**:
  - 단순 REST 호출의 나열이 아닌, **In-Memory Normalized Cache + WebSocket/SSE Delta Sync** (또는 Local-First Sync Engine 패턴)를 통해 화면 전환 시 네트워크 대기 시간을 **0ms**로 수렴시킵니다.
  - 마우스가 링크 위에 올라가는 순간(`pointerenter` / `focus`) 데이터를 백그라운드에서 즉시 프리페치(Prefetch)합니다.
- **Styling & Design Tokens**:
  - **Zero-Runtime CSS**: 런타임 CSS-in-JS(styled-components, Emotion 등)는 성능 저하를 유발하므로 **엄격히 금지**합니다.
  - **CSS Custom Properties (Design Tokens) + Vanilla CSS** 등 빌드 타임에 완전히 컴파일되는 제로 런타임 스타일링만 허용합니다.
- **Code & Diff Viewer**:
  - 수만 줄의 PR Diff나 소스 코드를 60~120fps로 스크롤하기 위해 **가상 스크롤링(Virtualization)**과 백그라운드 **Web Worker / WASM 기반 신택스 하이라이팅(`tree-sitter` 또는 `syntect` WASM)**을 기본 적용합니다. 메인 스레드 블로킹은 절대 허용되지 않습니다.

---

## 4. Proposed Repository Structure (Monorepo)

프로젝트는 Rust Cargo Workspace와 프론트엔드/디자인 시스템 패키지가 결합된 구조를 따릅니다.

```text
gitust/
├── AGENTS.md                  # AI 에이전트 핵심 지침서 (현재 문서)
├── Cargo.toml                 # Rust Cargo Workspace 루트
├── crates/
│   ├── gitust-server/         # 메인 실행 바이너리 (Axum HTTP/WebSocket 서버, SSH 서버 부트스트랩)
│   ├── gitust-api/            # REST / RPC / Real-time Sync 핸들러, 미들웨어, DTO
│   ├── gitust-core/           # 순수 도메인 모델 및 비즈니스 로직 (User, Org, Repo, Issue, PR, Action)
│   ├── gitust-db/             # 데이터베이스 스키마, 마이그레이션, 리포지토리 구현체
│   ├── gitust-git/            # gitoxide(gix) 래퍼, Git 프로토콜(Smart HTTP, SSH), Diff/Blame 엔진
│   ├── gitust-search/         # Tantivy 기반 코드/이슈 초고속 인덱싱 및 검색 엔진
│   ├── gitust-actions/        # Gitea Actions 호환 CI/CD 러너 및 워크플로우 파서/스케줄러
│   └── gitust-packages/       # 패키지 레지스트리 (Cargo, npm, Container, Maven 등)
├── packages/
│   ├── design-system/         # [Phase 1 최우선] 토큰, 프리미티브 컴포넌트, 키보드 네비게이션 엔진, 아이콘
│   ├── sync-client/           # Optimistic UI, 인메모리 캐시, 프리페칭, 단축키 바인딩 코어 엔진
│   └── web/                   # Gitust 웹 애플리케이션 (디자인 시스템 위에서 조립되는 뷰)
└── docs/
    ├── architecture/          # ADR (Architecture Decision Records) 및 시스템 설계 문서
    ├── design-system/         # 디자인 토큰 명세, 인터랙션 가이드라인, 단축키 맵
    └── gitea-parity/          # Gitea 기능 대비 구현 현황 매트릭스
```

---

## 5. Development Roadmap & Phase Rules

에이전트는 작업을 수행할 때 아래의 **단계적 우선순위(Phased Execution)**를 반드시 지켜야 합니다.

### Phase 1: Design System & Interaction Engine Foundation (최우선 순위)
비즈니스 화면을 찍어내기 전에 **우리만의 디자인 시스템과 속도 엔진**을 먼저 완성합니다.
1. **Design Tokens 정의**: Color(Dark/Light 고대비 테마), Typography(UI 폰트 및 고정폭 코드 폰트 스케일), Spacing(고밀도 4px/2px 그리드), Elevation, Motion(즉각적인 마이크로 트랜지션 <= 100ms).
2. **Keyboard & Command Engine**:
   - 전역 단축키 매니저(Chord shortcuts: `g i`, `g p`, `c` 등, Vim 스타일 리스트 탐색 `j`/`k`/`o`/`x`).
   - 초고속 Command Palette (`Cmd+K`) 컴포넌트.
3. **Core UI Primitives**:
   - Icon(SVG `<symbol>` Sprite), Kbd, Button, Input, Badge, Avatar, Tooltip(지연 시간 제어), Popover, Dialog, Tabs, SplitPane, Toast.
   - 모든 컴포넌트는 키보드 포커스 링(Focus Ring)과 스크린 리더 접근성(ARIA)을 완벽히 지원해야 합니다.
4. **Domain-Specific High-Performance Primitives**:
   - **Virtualized File Tree**: 수만 개의 파일이 있는 모노레포도 지연 없이 열리고 검색되는 트리 뷰.
   - **Virtualized Code & Diff Viewer**: Unified / Split 뷰, 인라인 코멘트 스레드, 구문 강조를 16ms 프레임 예산 내에서 렌더링하는 컴포넌트.
   - **Instant Filter Bar**: 이슈/PR 목록을 타이핑과 동시에 로컬 필터링/자동완성하는 슈퍼휴먼 스타일 검색 바.
   - **Gitust Actions CI/CD Pipeline & Virtualized Log Streamer**: 워크플로우 DAG/스텝 상태 시각화 및 수만 줄의 실시간 빌드 콘솔 로그를 120fps로 스트리밍/검색하는 가상화 로그 뷰어.
   - **Enterprise Auth, SSO & Site Admin Patterns**: 사내 로그인/회원가입 제어(Invite-only, 도메인 제한, SSO), 사용자/권한 매트릭스, 셀프호스팅 러너 상태, 보안 감사 로그(Audit Log)를 0ms Optimistic UI로 제어하는 엔터프라이즈 관리 패턴.

### Phase 2: Rust Core & Git Plumbing Engine
1. Workspace 초기화 및 `gitust-core`, `gitust-git`, `gitust-db` 스캐폴딩.
2. Git Smart HTTP (`git-upload-pack`, `git-receive-pack`) 및 내장 SSH 서버(`russh`) 구현.
3. `gix` 기반의 초고속 Commit Log, Tree, Blob, Blame, Diff 추출 API 구축 및 벤치마킹.
4. 인증/인가(Session, PAT, OAuth2, SSH Key, GPG/SSH 서명 검증) 코어 구현.

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
