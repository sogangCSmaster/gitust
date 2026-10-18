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
3. **Finish Phase 3 (Implemented).** Durable automatic-run scheduling and lifecycle
   records, restart recovery, Checks/Actions views, stable repository identity and
   routing, repository registration/creation/import/listing, runtime lifecycle
   administration (`POST /api/v1/admin/repos`, `GET/PATCH /api/v1/repos/{id}/settings`),
   SSH Git transport (`git-upload-pack` and `git-receive-pack` with Ed25519 and ECDSA
   NIST P-256/384/521 keys), and full pull-request collaboration with branch protection
   are implemented and verified. Final-candidate restoration and external deployment
   validation remain tracked under the alpha acceptance gates.
4. **Finish Phase 4 (Implemented).** Source-of-truth operation (`github_primary` /
   `gitust_primary`) with upstream backup mirror pushes and CAS-verified rollback;
   GitHub Actions-compatible workflow/job dependencies (`needs`), matrices, encrypted
   secrets, artifacts and caches; stronger execution isolation (`host_os` /
   `container_vm`), multi-node runner scheduling and SHA-256 shared build caches;
   enterprise SSO (`oidc` / `ldap`), RFC 6238 TOTP two-factor authentication and a
   tamper-evident SHA-256 hash-chained audit trail; and commit-pinned repository-authorized
   trigram code and symbol search (`GET /api/v1/repos/{id}/search`) are implemented and
   verified across the Rust backend and web workbench.

Repository CI is a development verification tool, distinct from the in-product
GitHub Actions-compatible workflow engine inside Gitust. MCP OAuth remains agent
authorization, while enterprise account SSO (`oidc`/`ldap`) and RFC 6238 TOTP protect
human sign-in and emit hash-chained audit records. Optional [SSH hosting](ssh-read-only.md)
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
- Checkpoint completed slices on `main` with detailed Korean commit messages and
  push them. The user authorized unsigned commits; disable signing per command
  rather than changing their global 1Password or Git configuration. Preserve the
  pre-push verification hook.
- Keep test installations and fixture remotes separate from user repositories.
  Never publish credentials or private installation state in logs or artifacts.
- Do not mark the overall goal complete until every phase's listed capability and
  relevant acceptance condition has current implementation and runtime evidence.
  Missing platform, external integration or deployment evidence remains open.
