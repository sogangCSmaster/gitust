# Alpha installation and platform validation

Gitust's alpha is a trusted single-operator installation. A successful macOS
run does not establish Linux Bubblewrap support, and generated Bubblewrap
arguments are not evidence that a kernel permits the required namespaces.
Windows sandbox execution is not implemented.

## Repeatable acceptance command

Run from a source checkout on each advertised operating system:

```sh
python3 scripts/alpha-platform.py
```

Prerequisites are the README build tools (Rust, Git, Node.js, pnpm 10), Python 3,
`curl`, and an executable OS sandbox backend. Linux requires `/usr/bin/bwrap`
or `/usr/local/bin/bwrap` with permission to create the namespaces used by
Bubblewrap. Rustup must be installed under the operator's home so that the
sandboxed Rust-toolchain test executes instead of silently skipping.

The default is **offline**. Dependencies must already be in the host's Cargo
and pnpm caches. An existing pnpm store can be selected explicitly:

```sh
python3 scripts/alpha-platform.py --store-dir /absolute/path/to/pnpm/store/v10
```

On a newly provisioned validation host, an operator can explicitly permit
dependency downloads with `--allow-downloads`. This flag does not fetch or
publish an application repository, launch a container, configure namespaces,
or change the host's system configuration.

The runner:

1. Copies tracked and non-ignored source files, including current local edits,
   into a new temporary directory. It excludes Git metadata, local Gitust state,
   environment files other than examples, tool caches, and previous build
   output. Symlink source entries are rejected. File hashes record the exact
   snapshot and changes detected during copying fail the run.
2. Installs the locked frontend dependencies and builds the production web UI.
3. Builds the release CLI with a locked Cargo dependency graph and an initially
   empty target directory.
4. Executes the sandbox crate's tests serially. The required isolation and Rust
   toolchain tests must appear as successfully executed tests. These cover
   allowed edits, denied host/sibling reads and writes, hidden Git state,
   stripped environment, a real host-loopback network denial, timeouts,
   background-process cleanup, command sequencing, and bounded output.
5. Executes the real Smart HTTP clone/commit/push/fetch permission regression,
   including client and server `git fsck`. A missing or renamed test fails
   acceptance instead of allowing Cargo's zero-matching-tests success.
   The upstream and client repositories are temporary local fixtures; this
   does not validate github.com connectivity, credentials, or publication.
6. Copies only the release binary to a separate runtime directory, gives it a
   PATH containing Git alone, and temporarily moves the source web bundle out
   of its compiled location. It bootstraps a synthetic administrator, starts a
   loopback-only server, checks every embedded HTML/JS/CSS response byte for
   byte, and logs in. Node.js, pnpm, Vite, and a static-file directory are not
   available through that runtime PATH.

`summary.json`, `source-manifest.json`, `commands.log`, the source snapshot,
and the synthetic runtime fixture remain under the printed temporary evidence
directory. That directory is mode `0700`. The runner does not print passwords
or session tokens. Delete the evidence directory when no longer needed; release
build output can occupy substantial disk space.

The result describes a fresh **source/output directory using existing host
toolchains and caches**, not a freshly installed machine. It does not replace
`scripts/verify.sh`, independent Gate A review, the ten-task operating
rehearsal, backup restoration, a real agent-client connection, or GitHub
authentication/publication validation.

## Observed environment and evidence (2026-10-17)

The available host is Apple Silicon, macOS 26.5.2 (Darwin 25.5.0), with Rust
1.92.0, Cargo 1.92.0, Node.js 24.18.1, pnpm 10.0.0, Git 2.52.0, Python 3.9.6,
and `/usr/bin/sandbox-exec`.

The first offline source-snapshot run installed dependencies and built the web
UI and release binary successfully. Its sandbox tests failed with
`sandbox-exec: sandbox_apply: Operation not permitted` because the tool's outer
sandbox disallowed applying Seatbelt. This is a failed execution result, not a
successful isolation test. Normal terminal execution, or an approved tool run
outside that outer sandbox, is required to evaluate Seatbelt itself.

An approved run outside the tool's outer sandbox subsequently passed with
network dependency downloads disabled. Its 219-file source snapshot had SHA-256
`96083f1c66a0cfb1d2de9a4f27af0f4997dcbf2fd229fe094c7d5b3df2304e08`.
The observed checks were:

| Check | Observed result |
|---|---|
| Locked dependency installation from the existing pnpm cache | Passed, 1.341 s |
| Production web build | Passed, 3.428 s |
| Release CLI build with an initially empty target directory | Passed, 24.855 s |
| Sandbox suite, including real Seatbelt execution | 20 passed, 0 failed, 0 ignored |
| Smart HTTP Git roundtrip, RBAC and both `git fsck` checks | 1 passed |
| Copied release binary with only Git on PATH and source assets moved away | 8 embedded responses matched; administrator login passed |

These are one-host observations using warm dependency caches, not build-time or
runtime latency guarantees. The ephemeral evidence directory printed by this
run ends in `gitust-alpha-platform-iu9ta2fe`; it contains the summary, full
command output and per-file snapshot hashes.

The final runner was repeated after adding required-test execution checks and
process-group cleanup on timeout/cancellation. This second successful run ends
in `gitust-alpha-platform-uwm_0xpg`, with 221 files and snapshot SHA-256
`47926e6586202109b5de2cc50d972d76f1a34daf59fd2b6a36d58590874b2373`.
It passed the same 20 sandbox tests, one Git roundtrip and eight embedded
responses plus login. Separate negative probes confirmed that Git metadata,
local state and environment files are excluded, source symlinks are rejected,
and a zero-matching-tests Cargo-style result cannot satisfy a required test.

Linux execution is **unverified** in this environment. Docker's CLI is present,
but both `docker info` and `docker image ls` fail to connect to the configured
`desktop-linux` socket because the socket is absent. No Podman, Colima, Lima,
or local Bubblewrap executable is available. No VM was started, packages
downloaded, namespace policy changed, or remote host accessed for this check.

To close Linux acceptance, execute the command above on an actual Linux host
and retain a `status: passed` summary with its command log and snapshot hashes.
An ordinary container without usable user/mount/PID/network namespaces may fail
correctly; do not bypass isolation checks or substitute host execution to make
the result green.
