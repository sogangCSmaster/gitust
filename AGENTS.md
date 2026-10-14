# AGENTS.md — Gitust (The All-in-One Rust Git + Container-Use + Microsandbox Platform)

## 1. Project Vision & Identity ("The Smartphone Moment for Agent-Native Git")

오늘날 AI 코딩 에이전트(Claude Code, Cursor, Codex 등) 시대에 기업과 개발팀이 겪는 가장 큰 문제는 **도구들의 파편화**와 **클라우드 코드 유출(Privacy) 딜레마**입니다.

1. **Cursor Origin / OpenAI Cloud Agents의 치명적 한계**:
   - 클라우드 백그라운드 에이전트를 쓰려면 **'Privacy Mode'를 강제로 꺼야 하고, 기업의 핵심 소스 코드 전체가 외부 클라우드 VM으로 전송**되어야 합니다. 또한 클라우드 VM 분당 과금과 동시 실행 할당량(Quota) 제한 때문에 사내 유휴 서버가 있어도 마음껏 에이전트를 돌리지 못합니다.
2. **오픈소스 생태계의 파편화와 언어적 한계 (Go/Dagger의 무거움)**:
   - 기존에는 **① Gitea/Forgejo (Go 언어 기반 사내 Git 서버)**, **② Container Use (Go/Dagger 기반 에이전트 컨테이너 격리 MCP)**, **③ Microsandbox (격리 실행기)**가 전부 따로 놀았을 뿐 아니라, Go GC나 무거운 컨테이너 데몬 때문에 우리가 원하는 극한의 속도(`<1ms` 브랜치 포크, `<16ms` 응답)를 내지 못합니다.

**Gitust는 외부 Go 바이너리(Gitea/Forgejo, Dagger 등)를 절대 가져다 쓰지 않고, 이 세 가지 도메인(Git 서버 + 에이전트 워크트리 격리 + 초고속 샌드박스 CI)을 처음부터 끝까지 100% 순수 Rust(`gitust` 단일 바이너리)와 Superhuman급 16ms UI로 직접 재작성하여 세계 최고의 속도를 내는 올인원 플랫폼입니다.**

---

## 2. The 4 Pillars of Gitust (핵심 4대 기둥)

### Pillar 1: 100% Pure Rust From-Scratch Engine (타협 없는 최고 속도)
Gitea/Forgejo나 Container-Use의 개념과 장점만 흡수하되, 모든 코어 엔진은 **100% 순수 Rust로 직접 작성**하여 극한의 성능을 뽑아냅니다.
- **Pure Rust Git Server (`crates/gitust-git`)**: Go 기반 Gitea를 완전히 대체하여, 순수 Rust(`gix` + `russh` + OS 네이티브 CoW 시스템 콜)로 자체 Git 서버와 `<1ms` 제로 카피 멀티 브랜치/워크트리 스냅샷 엔진을 직접 구현합니다.
- **Pure Rust Agent Isolation & MCP (`crates/gitust-sandbox`)**: 무거운 Dagger/Docker 데몬 없이, Claude Code나 Cursor가 내장 Rust MCP/CLI로 붙는 순간 에이전트마다 완벽히 격리된 CoW 브랜치와 워크트리 환경을 `<1ms`에 자동 할당합니다.
- **Sub-100ms Rust Sandbox & CI Runner (`crates/gitust-sandbox` & `crates/gitust-actions`)**: 코드가 푸시되거나 에이전트가 검증을 요청하면 레거시 CI Cold-boot 없이 Rust 네이티브 격리 런타임에서 `<100ms` 내에 빌드·테스트·정적분석을 즉시 수행합니다.

### Pillar 2: 100% On-Premise Privacy & Unlimited Local Quota (Zero Cloud Leakage)
- **Privacy Mode 100% 유지**: 회사 소스 코드가 단 1바이트도 외부 클라우드 VM으로 나가지 않습니다.
- **무제한 에이전트 스웜 실행**: 클라우드 할당량 제한이나 분당 과금 걱정 없이, 사내 서버(Linux 서버, Mac Studio 클러스터 등) 자원을 활용해 수백 개의 에이전트 브랜치와 샌드박스를 무제한 병렬 구동합니다.

### Pillar 3: Bi-Directional Real-Time GitHub Sync (기존 GitHub과 실시간 양방향 동기화)
- **도입 장벽 제로(Zero Migration Friction)**: 기업이 기존 GitHub 저장소를 버릴 필요가 없습니다.
- **Two-Way Live Mirroring (`crates/gitust-sync`)**:
  - **Inbound**: 기존 GitHub 저장소의 커밋/PR/브랜치 변경사항을 실시간 Webhook/스트리밍으로 사내 Gitust에 `<100ms` 내 동기화합니다.
  - **Outbound (Noise-Free)**: 사내에서 에이전트 수백 개가 생성한 수천 개의 실험/실패 브랜치는 사내 Gitust에만 두고(GitHub 오염 및 노출 방지), **인간 개발자가 최종 승인(`a` 키 머지)한 검증된 커밋이나 PR만 기존 GitHub 저장소로 실시간 양방향 푸시/동기화**합니다.

### Pillar 4: Unified Single-Binary CLI (`gitust`) + Superhuman-Grade 16ms UX
- **Built-in CLI (`crates/gitust-cli`)**: 서버 데몬과 개발자/에이전트용 CLI가 단일 바이너리에 통합됩니다.
  - `gitust serve`: 사내 올인원 서버(Git + Web UI + Microsandbox + MCP + GitHub Sync) 즉시 기동
  - `gitust mcp`: Claude Code / Cursor에 즉시 연결되는 에이전트 격리 MCP 서버 (`container-use` 완벽 대체)
  - `gitust sandbox run "<cmd>"`: 현재 작업 트리를 `<1ms` CoW 스냅샷으로 따서 Microsandbox에서 즉시 빌드/테스트
  - `gitust sync <github-url>`: 기존 GitHub 저장소와 실시간 양방향 미러링 연결
  - `gitust triage`: 터미널/웹에서 에이전트 브랜치를 초고속 검수·머지
- **Superhuman-Grade Design System (`@gitust/design-system`)**:
  - **SolidJS + Zero-Runtime CSS Tokens + GPU-Instanced SVG Sprite (`<Icon />`)** 기반으로 초기 번들 **~25KB(Gzip)**, 모든 키보드 인터랙션(`Cmd+K`, `g w`, `j`/`k`, `a` 승인, `x` 폐기)을 **16ms(1프레임)** 내에 처리합니다.

---

## 3. Core Principles for AI Agents

이 저장소에서 작업하는 모든 AI 에이전트는 코드를 작성하거나 아키텍처를 제안할 때 반드시 아래의 원칙을 준수해야 합니다.

1. **Speed is the Ultimate Feature (< 16ms Frame / < 100ms Interaction)**
   - 스피너(Spinner)를 보여주는 것은 실패한 UX입니다. 에이전트 브랜치 승인/머지, GitHub 양방향 싱크 트리거, 샌드박스 재실행, 이슈 상태 변경, 관리자 권한 변경은 모두 **Optimistic UI (0ms)**로 즉시 반영되어야 합니다.
2. **Design System First (No Ad-hoc UI, No Emojis)**
   - 모든 UI 컴포넌트는 `packages/design-system`에 먼저 정의되어야 하며, 매직 넘버 대신 `--ds-*` CSS 토큰만 사용합니다.
   - 프로덕션 UI에 유니코드 이모지(`📄`, `💬` 등) 사용을 엄격히 금지하며, 반드시 GPU 인스턴싱 기반 `<Icon name="..." />` 스프라이트 프리미티브를 사용합니다.
3. **Idiomatic Rust & Open-Source Core Composition**
   - Go 코드를 기계적으로 1:1 번역하지 않고, Rust의 타입 시스템(`Newtype`, `enum`, `Result<T, E>`)과 `tokio` 비동기 런타임으로 설계합니다. 프로덕션 경로에서 `unwrap()`/`expect()` 사용을 금지합니다.
   - `gitoxide`(`gix`), `microsandbox`(`msb`), `axum`, `sqlx`, `russh`, `tantivy`, `clap` 등 검증된 Rust 크레이트를 유기적으로 결합합니다.
4. **Enterprise Self-Hosted Ready (Auth, Admin, Actions) — 단계적 제공**
   - 사내 구축을 위한 **로그인 제어**, **Site Admin 콘솔**, **내장 CI/CD (`Gitust Actions`)**는 아래 로드맵(5장)의 단계 순서대로 제공합니다.
   - 인증은 **관리자 초대 링크(Invite-only)**로 시작하고, SSO/OIDC/LDAP·2FA는 엔터프라이즈 단계에서 추가합니다.
   - CI/CD는 `sandbox run` → **Checks** → **GitHub Actions 호환** 순서로 쌓습니다.
   - 현재 단계에 없는 항목을 앞당겨 구현하지 않습니다.
5. **Verification First (검증이 완료 조건)**
   - 이 프로젝트는 바이브코딩으로 개발되므로, 자동 검증이 유일한 품질 보증입니다. 기능은 7.4의 검증을 통과해야 완료로 봅니다.
   - 측정하지 않은 성능 수치를 코드 주석·README·UI 문구에 사실처럼 쓰지 않습니다.

---

## 4. Proposed Repository Structure (Monorepo)

```text
gitust/
├── AGENTS.md                  # AI 에이전트 핵심 지침서 (현재 문서)
├── Cargo.toml                 # Rust Cargo Workspace 루트
├── crates/
│   ├── gitust-cli/            # 단일 바이너리 엔트리포인트 (`gitust serve | mcp | sandbox | sync | triage`)
│   ├── gitust-server/         # Axum HTTP/WS 서버, 내장 MCP 게이트웨이, russh SSH 서버 부트스트랩
│   ├── gitust-core/           # 순수 도메인 모델 (User, Org, Repo, AgentWorktree, SandboxRun, SyncMirror, Issue, PR)
│   ├── gitust-git/            # gitoxide(gix) 래퍼, <1ms CoW 멀티 브랜치/워크트리 스냅샷, Diff/Blame 엔진
│   ├── gitust-sandbox/        # [핵심] Microsandbox + Container-Use 통합 에이전트 격리 실행 엔진
│   ├── gitust-sync/           # [핵심] 기존 GitHub/GitLab 저장소와의 실시간 양방향 동기화(Two-Way Live Sync) 엔진
│   ├── gitust-actions/        # Checks(Phase 3) → GitHub Actions 호환 CI/CD 파이프라인 & 스케줄러(Phase 4)
│   ├── gitust-db/             # SQLx 기반 데이터베이스 스키마, 마이그레이션, DataLoader 리포지토리
│   └── gitust-search/         # Tantivy 기반 코드/심볼/이슈 <5ms 초고속 검색 엔진
├── packages/
│   ├── design-system/         # [Phase 1 완료] 토큰, 아이콘 스프라이트, 키보드 엔진, 에이전트 스웜/싱크/CLI/Admin 뷰
│   ├── sync-client/           # Optimistic UI, 인메모리 캐시, 프리페칭, 실시간 스트리밍 클라이언트
│   └── web/                   # Gitust 웹 애플리케이션 (디자인 시스템 위에서 조립되는 뷰)
└── docs/
    ├── architecture/          # ADR 및 Gitea + Container-Use + Microsandbox 통합 설계 문서
    ├── design-system/         # 디자인 토큰 명세, 인터랙션 가이드라인, 단축키 맵
    └── business/              # 엔터프라이즈 수익화(Open-Core) 및 라이선스 정책
```

---

## 5. Development Roadmap & Phase Rules

### Phase 1: Design System & Interaction Engine Foundation (완료)
1. **Design Tokens & Zero-Dependency SVG `<symbol>` Icon Sprite (`<Icon />`)**.
2. **Keyboard & Command Engine (`ShortcutManager`, `Cmd+K`, Chord shortcuts `g w`, `g i`, `g d`, `g a`, `g s`, `g t`, `g p`)**.
3. **Domain-Specific High-Performance Patterns**:
   - **Agent Swarm, Microsandbox & GitHub Bi-Directional Sync View (`g w`)**: Gitea + Container-Use + Microsandbox 올인원 관제, 100% On-Premise Privacy HUD, GitHub 실시간 양방향 미러링, `gitust` CLI & MCP 연동 패널.
   - **Virtualized Code & Diff Viewer (10,000 Lines, `g d`)** & **Virtualized File Tree (5,000 Files, `g t`)**.
   - **Superhuman Issue & PR List (`g i`)**.
   - **Gitust Actions CI/CD Pipeline & 10,000-Line Live Log Streamer (`g a`)**.
   - **Enterprise Self-Hosted Auth, SSO & Site Admin Console (`g s`)**.

Phase 2부터는 **사내 GitHub로 실제 사용**하는 것을 목표로, GitHub를 안전망으로 두고 단계적으로 도입합니다. 각 Phase는 이전 Phase가 7.4의 검증을 통과한 뒤에 시작합니다.

### Phase 2: Agent Workspace + Read-Only Mirror (MVP — 원본은 GitHub)
1. Cargo Workspace 초기화 및 `gitust-cli`, `gitust-core`, `gitust-git`, `gitust-sandbox`, `gitust-sync`, `gitust-server` 구축. (뼈대 완료)
2. `gitoxide`(`gix`) 기반 CoW 에이전트 워크트리 격리(`container-use` 패턴) 및 `microsandbox` 실행 연동 (`gitust sandbox run`).
3. 내장 MCP 서버(`gitust mcp`): Claude Code / Cursor가 붙으면 에이전트별 워크트리 할당 + `sandbox run` 호출.
4. GitHub → Gitust **단방향 읽기 전용 미러**(`gitust sync`)와 웹 코드/커밋/Diff 보기.
5. 인증: **관리자 초대 링크** 방식 (아래 "인증 규칙" 참고).
6. 목표: Gitust를 Gitust로 개발(dogfooding)할 수 있는 상태.

### Phase 3: Internal GitHub (에이전트 브랜치는 사내, 승인분만 GitHub로)
1. HTTPS/SSH(`russh`) Git 호스팅(clone/push/fetch)과 조직·팀·저장소 단위 권한.
2. 에이전트 브랜치는 Gitust에만 push, 사람이 승인한 커밋/PR만 GitHub로 Outbound 동기화.
3. PR 리뷰·머지, `gitust triage`.
4. **Checks**: 저장소 설정 파일에 적힌 명령(예: `cargo fmt --check`, `cargo test`)을 브랜치/PR 갱신 시 자동 실행하고 통과/실패를 표시.
5. Site Admin 콘솔(멤버 초대·비활성화, 저장소 관리)과 백업/복구.

### Phase 4: Source of Truth 전환 + Enterprise
1. 안정성이 확인된 저장소부터 Gitust를 원본으로 전환하고 GitHub는 백업 미러로 운영.
2. **Gitust Actions**: GitHub Actions YAML 호환 워크플로 엔진(표현식, matrix, needs, secrets, artifacts, cache, 마켓플레이스 액션 실행).
3. SSO/OIDC/LDAP, 2FA, 감사 로그(Audit Trail).
4. 멀티 노드 샌드박스 클러스터, Tantivy 검색.

### 인증 규칙 (Phase 2~3)
- 공개 회원가입과 "가입 후 승인" 방식은 두지 않습니다. 계정은 **관리자가 발급한 초대 링크**로만 생성합니다.
- 초대/비밀번호 재설정 링크는 메일 서버 없이 관리자가 메신저 등으로 직접 전달합니다.
- 링크 토큰은 충분히 긴 암호학적 랜덤 값이며 DB에는 해시만 저장합니다. 1회 사용, 만료(기본 72시간), 관리자 취소를 지원합니다.
- 비밀번호는 argon2로 해시하고, 로그인 시도 횟수를 제한합니다.
- Git 접근은 SSH 키와 개인 액세스 토큰(HTTPS)으로 합니다. 계정 비활성화 시 세션·SSH 키·토큰이 모두 즉시 무효화되어야 합니다.
- 첫 관리자는 CLI(`gitust admin create`)로만 생성합니다. 웹에 "첫 가입자 = 관리자" 경로를 만들지 않습니다.

---

## 6. Performance Budgets & Quality Gates (성능 예산)

에이전트가 작성하는 모든 기능은 **항상 Superhuman 수준의 극한 속도(Blazing Speed)를 최우선으로 지향**하며 아래의 정량적 성능 기준을 목표로 합니다.

- 아래 수치는 **목표치**입니다. 벤치마크 스크립트로 측정한 결과만 달성으로 인정하며, 문서·UI에는 측정 환경과 함께 측정값을 인용합니다.
- CoW 워크트리 수치는 reflink/clonefile을 지원하는 파일시스템(APFS, btrfs, XFS)이 전제입니다. 지원하지 않는 파일시스템(ext4 등)의 대체 경로와 그 성능을 따로 측정합니다.
- 샌드박스 수치는 실행 환경 기동 시간이며, 빌드·테스트 자체의 소요 시간은 포함하지 않습니다.

- **Agent Branch / Worktree Spawn (`gix` CoW)**: **< 1ms**
- **Microsandbox Warm Execution Start**: **< 100ms ~ 200ms**
- **API 응답 시간 (p99)**: 일반 메타데이터 **< 15ms**, Git Tree/Commit 조회 **< 30ms**
- **UI 인터랙션 레이턴시**: 키보드 입력 -> 화면 피드백 **< 16ms (1 프레임)**
- **프론트엔드 번들 사이즈**: 초기 로드 JS (Gzip 기준) **< 50KB**

---

## 7. Coding Conventions & Workflow Instructions

### 7.1 Rust Guidelines
- 모든 커밋 전 `cargo fmt --all -- --check`와 `cargo clippy --workspace --all-targets -- -D warnings`를 통과해야 합니다.
- 루프 내부에서 DB 쿼리를 호출하는 N+1 문제를 절대 만들지 마십시오.

### 7.2 Design System & Frontend Guidelines
- 매직 넘버를 쓰지 말고 반드시 `--ds-*` 디자인 토큰 변수를 사용하십시오.
- 프로덕션 UI에는 절대 유니코드 이모지를 쓰지 말고 `<Icon name="..." />` 벡터 스프라이트 프리미티브를 사용하십시오.

### 7.3 Git Branch & Commit Workflow (Single-Developer Trunk-Based `main`)
- 이 프로젝트는 1인 개발 체제로 진행되므로 **불필요한 피처 브랜치(Feature Branch)나 PR을 분리하지 않고 항상 `main` 브랜치에서 직접 작업, 커밋, 푸시(`git push origin main`)**합니다.

### 7.4 Verification Gates (기능 완료 조건)
기능을 구현하기 전에 통과해야 할 테스트를 먼저 작성하고, 아래 해당 항목을 통과해야 완료로 봅니다.
- **Git 데이터 무결성**: 실제 `git` 클라이언트로 clone → commit → push → fetch 왕복 통합 테스트 후 서버·클라이언트 양쪽 `git fsck`가 통과해야 합니다.
- **권한**: 허용 경로뿐 아니라 거부 경로(다른 팀 저장소 접근, 비활성 계정, 만료·재사용된 초대 링크, 승인 없는 Outbound 동기화)가 반드시 막히는지 테스트합니다.
- **샌드박스 격리**: 샌드박스 밖 파일 읽기/쓰기, 호스트 시크릿·환경 변수 접근, 허용되지 않은 네트워크 접근, 프로세스 탈출 시도가 실패하는지 테스트합니다.
- **동기화**: 미러 결과가 원본 GitHub 저장소의 ref·커밋과 일치하는지 비교 검증합니다.
- **백업/복구**: Phase 3부터 백업에서 실제로 복구하는 리허설 테스트를 둡니다.
- **성능**: 6장 수치를 주장하는 변경은 벤치마크 결과를 함께 남깁니다.

---

## 8. Business & Monetization Architecture (Open-Core + Enterprise Agent Infrastructure)

1. **Community Edition (Open Source — GitHub Trending & 개발자 표준 선점)**
   - 단일 바이너리(`gitust`) 올인원 패키지: 사내 Git 서버 + GitHub 양방향 싱크 + `gitust` CLI/MCP + 기본 Microsandbox 워커(동시 4개).
2. **Enterprise On-Premise License (핵심 수익원 #1 — Concurrent Agent Sandbox Cluster 과금)**
   - **Unlimited Multi-Node Microsandbox Cluster**: 사내 다중 서버에 수백 개의 에이전트 샌드박스를 분산 스케줄링하고 공유 빌드 캐시(Remote Cargo/sccache/npm Cache)를 제공하는 엔터프라이즈 클러스터 엔진.
   - **Enterprise Security & AI Governance**: SAML/OIDC/LDAP SSO, 에이전트 전용 디렉토리/시크릿 접근 차단(Guardrails), GitHub 아웃바운드 싱크 승인 게이트, AI 생성 코드 감사 로그(Audit Trail).
3. **Hybrid Zero-Trust Burst Runner (핵심 수익원 #2 — Usage-based Compute 과금)**
   - 코드는 사내 Gitust에 100% 보관하되, 순간적으로 수백 개의 대규모 샌드박스 빌드가 필요할 때만 암호화 메모리 볼륨으로 연산(vCPU-초)을 대여하는 종량제 버스트 클라우드.
