# Full roadmap execution plan

On 2026-10-17 the user expanded the active goal to the **entire product roadmap**,
including Phase 4. The alpha is the first release checkpoint, not the end of the
overall goal. Preserve the [product vision](../vision.md) and the scope of every
phase in the [roadmap](../roadmap.md); an implemented subset does not complete a
phase or replace its remaining requirements.

## Sequence and exit evidence

1. **Complete the adopted workbench interface.** Cover repository selection and
   overview, worktree creation, file/diff review, verification, history, approval,
   publication and account tools. Exercise compact desktop, tablet and mobile
   layouts, errors and session transitions. Keep previews labelled. Verify actual
   repository state independently of optimistic UI. Track the initial bundle gap
   without treating code splitting as proof of a latency target.
2. **Close the alpha checkpoint.** Retain all [A1–A9 acceptance
   conditions](alpha-acceptance.md). Add repository CI that runs the actual gates
   and operational rehearsal on Linux/Bubblewrap and macOS/Seatbelt. Collect
   intended agent-client sessions, authorized GitHub publication, clean-install,
   restart and relocated recovery evidence against a fixed candidate. Source
   pushes, local bare remotes and protocol-only clients do not substitute for the
   corresponding external integration evidence.
3. **Finish Phase 3 (in progress).** Implemented portions include durable automatic-run
   scheduling/recovery, Checks views, repository identity/routing, registration,
   creation/import/listing, online administration, SSH Git transport and pull-request
   collaboration with branch protection. RSA user-key authentication, final-candidate
   restoration, external deployment and alpha acceptance evidence remain open.
4. **Finish Phase 4 (in progress).** Source-of-truth topology, backup mirror/rollback,
   JSON workflow DAGs/matrices, commit-pinned search, TOTP and hash-chained audit
   foundations exist. Workflow YAML and `uses` actions are unsupported. Caches are
   local and key-addressed; artifacts are bounded text previews. Runner registration
   and heartbeats are metadata only: remote execution and `container_vm` requests
   are rejected. Native OIDC/LDAP, remote dispatch, container/microVM isolation,
   general artifact handling and shared multi-node caches remain planned. Existing
   installations require the [security upgrade procedure](security-upgrade.md).

Repository CI is a development verification tool, distinct from the in-product
JSON workflow subset inside Gitust. MCP OAuth remains agent authorization. Human
SSO currently uses a custom HMAC-signed assertion bridge; `oidc`/`ldap` provider labels
do not establish native protocol compatibility. RFC 6238 TOTP and audit records
are separate account-security features. Optional [SSH hosting](ssh-read-only.md)
implements Ed25519 and ECDSA (`nistp256`/`nistp384`/`nistp521`) clone/fetch and authenticated
push (`git-upload-pack` and `git-receive-pack`) when `--ssh-bind` is configured; key registration
alone does not enable it. Metadata-only AuthStore repository records do not create
Git services unless registered or created via online repository administration.

## Work protocol

- Choose cohesive implementation slices and preserve their dependencies. Record
  implemented behavior, remaining work and validation limits separately.
- Run the relevant functional, denial, failure, concurrency and recovery checks,
  then `scripts/verify.sh` and independent Gate A review. Resolve actionable
  P0/P1/P2 before closing the slice; preserve failing-before regression evidence
  for reproduced defects. Automatic Gate B remains disabled.
- Keep completed slices on `main` in the working tree until the user explicitly
  requests a commit or push, following the current AGENTS.md gates. When requested,
  use detailed Korean commit messages and preserve the pre-push verification hook.
  Historical execution-plan authorization does not grant a new commit or push.
- Keep test installations and fixture remotes separate from user repositories.
  Never publish credentials or private installation state in logs or artifacts.
- Do not mark the overall goal complete until every phase's listed capability and
  relevant acceptance condition has current implementation and runtime evidence.
  Missing platform, external integration or deployment evidence remains open.
