# Benchmark: agent worktree materialization

Measures `GitEngine::materialize_agent_worktree`: validate the branch name, create the `agent/`
branch with a `gix` ref transaction, then `git worktree add` (full checkout of the base commit).

## How to run

```sh
cargo test -p gitust-git --release -- --ignored --nocapture bench_
```

Each size runs 5 times; the repository has files spread over 100 directories.

## Results (2026-10-15)

Environment: Apple M4 Pro, 24 GB RAM, macOS 26.5.2, APFS (internal SSD), git 2.52.0,
rustc 1.92.0, release build.

| Tracked files | min | median | max |
|---:|---:|---:|---:|
| 10 | 16.9 ms | 18.9 ms | 20.7 ms |
| 1,000 | 114.5 ms | 121.8 ms | 143.3 ms |
| 10,000 | 664.5 ms | 694.0 ms | 1,155.3 ms |

## Interpretation

- The AGENTS.md target "Agent Branch / Worktree Spawn < 1ms" is **not met**. Measured time is
  roughly 17 ms fixed (spawning `git`) plus ~0.07 ms per checked-out file.
- Branch creation itself is cheap; the cost is the checkout, which scales with file count.
- Candidate optimizations (measure each before claiming anything):
  1. In-process checkout via `gix` (removes the `git` process spawn; requires `gix` API support).
  2. Populate unchanged files with `clonefile(2)` / `FICLONE` from an existing clean checkout.
  3. Pre-warmed worktree pool per base commit, so spawn becomes a rename.
