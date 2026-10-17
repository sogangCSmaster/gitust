# Repository continuous integration

`.github/workflows/ci.yml` validates Gitust's own source on pushes to `main`,
pull requests, and manual runs. It is separate from the planned GitHub Actions
compatibility engine inside Gitust. A workflow file alone is not execution or
platform-support evidence; inspect the completed run for the exact revision.

Both `ubuntu-22.04` and `macos-15` run the same required gates:

1. Install Rust 1.92.0 with Clippy/rustfmt, Node.js 22, and the pnpm version
   declared by `packageManager`. Install locked frontend/Cargo dependencies.
2. Require the host's actual Bubblewrap or Seatbelt executable to run. Linux
   installs Bubblewrap without changing AppArmor or namespace policy. An
   unavailable or forbidden backend fails the job; there is no host-execution
   fallback. A home-directory Rustup installation is required so the sandboxed
   Rust-toolchain regression cannot silently skip.
3. Build the production UI and reject any difference from the committed bundle,
   including newly generated untracked chunks. Run `scripts/verify.sh` unchanged:
   line limits, fail-open audit, Python regressions, formatting, frontend
   typechecks/tests, bundle freshness, Clippy and all Rust workspace tests. The
   sandbox suite includes real host/sibling access denials, stripped secrets,
   loopback network denial and process cleanup.
4. Require a clean, full-history checkout, build the CLI and execute the existing
   ten-task rehearsal against temporary local repositories. The checkout uses
   `fetch-depth: 0`, and an explicit shallow-repository check prevents a future
   change from reintroducing incomplete fixture history. Shallow clones retain
   their boundary through the rehearsal's local clones and cannot satisfy the
   mirror import's complete commit-history contract. The rehearsal exercises
   concurrent MCP clients, exact-SHA Checks,
   approval/publication separation, restart, relocated backup restoration, and
   actual Smart HTTP clone/commit/push/fetch plus `git fsck`. On detached PR
   checkouts, only the ephemeral local `main` fixture ref is set to the tested
   revision before cloning it. Neither upstream GitHub publication nor real
   agent-client integration is established by this local rehearsal.
5. Require ten distinct passed tasks, an unchanged source checkout, the matching
   tested source SHA, and the expected `linux_bubblewrap` or `macos_seatbelt`
   backend in the report. The job summary
   contains only an allowlist of public revision/hash/platform/result fields.
6. Kill a disposable fixture service during an actual automatic Check and require
   a fresh service to refuse the unresolved execution fence. Stop only the
   fixture's authenticated process groups, require explicit operator recovery,
   and prove ordinary execution resumes without restoring verification authority.
   This runs on both hosts; Linux process death behavior is observed rather than
   inferred from the macOS result.

The checkout token has only `contents: read` and is not persisted in Git
configuration. The workflow uses `pull_request`, does not consume repository
secrets, does not interpolate PR-controlled text into shell commands, and does
not restore or publish shared build caches. Official actions are pinned to full
commit SHAs verified against their release tags on 2026-10-17:

- [actions/checkout v7.0.1](https://github.com/actions/checkout/releases/tag/v7.0.1)
- [actions/setup-node v7.1.0](https://github.com/actions/setup-node/releases/tag/v7.1.0)
- [pnpm/action-setup v6.1.0](https://github.com/pnpm/action-setup/releases/tag/v6.1.0)

Source snapshots, scratch repositories, auth databases, server logs and tokens
are never uploaded as artifacts. Rehearsal scratch state is removed by its
existing cleanup handler; its JSON report stays in the disposable runner's
temporary directory. The standard Actions log and public summary remain the
CI evidence. This workflow does not deploy, publish release binaries, replace
independent Gate A review, or run Gate B. The separate
[clean source/platform validator](platform-validation.md) remains necessary for
standalone-binary installation evidence.

The [local history regression evidence](evidence/ci-history-2026-10-17.json)
records the real checkout → clone → fixture commit → bare clone → Gitust sync
sequence. A depth-one checkout failed; full history preserved all three commits,
matched the independent upstream SHA and passed both `git fsck` checks. The
committed operating rehearsal and explicit shallow guard retain this regression
in the CI execution path. This local result does not establish a successful
GitHub-hosted run; record that run's exact revision and outcome separately.
