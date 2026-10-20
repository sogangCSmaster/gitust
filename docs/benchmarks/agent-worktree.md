# Benchmark: agent worktree materialization & Section 6 performance budgets

Measures `GitEngine::materialize_agent_worktree` using in-process `gix` tree materialization,
Git Index v2 (`DIRC`) generation, and Copy-on-Write (`clonefile(2)` on macOS APFS / `FICLONE` on
Linux) template cloning under `<common_git_dir>/gitust-cow-templates/<base_sha>`, as well as
Section 6 latency budgets for prepared sandbox startup, metadata API p99, Git Tree/Commit API p99,
and keyboard input feedback.

## How to run

```sh
cargo test -p gitust-git --release -- --ignored --nocapture bench_
cargo test -p gitust-server performance_budget_sandbox_startup_metadata_and_git_api_p99 -- --nocapture
npm --prefix packages/design-system test -- tests/modal-shortcuts.test.ts
```

Each worktree size runs 5 times (run 0 builds the cold per-commit CoW template; runs 1..4 clone
from the warm CoW template); the repository has files spread over 100 directories.

## Results (2026-10-20 — CoW `clonefile(2)` + in-process `gix` ref & index materialization)

Environment: Apple M4 Pro, 24 GB RAM, macOS 26.5.2, APFS (internal SSD), git 2.52.0,
rustc 1.92.0, release build.

| Tracked files | Warm CoW min | Median | Cold template max | Previous median (`git worktree add`) |
|---:|---:|---:|---:|---:|
| 10 | **0.925 ms** (`925 µs`) | **0.987 ms** (`987 µs`) | 3.84 ms | 18.9 ms |
| 1,000 | **6.04 ms** (`6,035 µs`) | **6.13 ms** (`6,125 µs`) | 101.6 ms | 121.8 ms |
| 10,000 | **71.7 ms** (`71,712 µs`) | **122.8 ms** (`122,773 µs`) | 1,167.6 ms | 694.0 ms |

### Section 6 End-to-End Performance Budget Measurements (2026-10-20)

| Budget Item | Target | Measured | Command / Suite |
|---|---|---|---|
| CoW worktree spawn (10 files, warm) | `< 1 ms` | **0.925 ms min / 0.987 ms median** | `bench_materialize_agent_worktree` |
| Prepared sandbox execution startup | `< 100–200 ms` | **23.47 ms min / 24.01 ms median / 28.66 ms p99** | `performance_budget_sandbox_startup_metadata_and_git_api_p99` |
| General metadata API (`/api/v1/repos`) p99 | `< 15 ms` | **0.182 ms min / 0.212 ms median / 0.632 ms p99** | `performance_budget_sandbox_startup_metadata_and_git_api_p99` |
| Git Tree/Commit API (`/tree`, `/commits`) p99 | `< 30 ms` | **0.236 ms min / 0.327 ms median / 0.673 ms p99** | `performance_budget_sandbox_startup_metadata_and_git_api_p99` |
| Keyboard input → DOM feedback (100 dispatches) | `< 16 ms` | **< 0.1 ms median / < 1 ms p99** (6 ms total for 100 dispatches) | `packages/design-system/tests/modal-shortcuts.test.ts` |
