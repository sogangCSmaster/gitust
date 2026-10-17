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
3. **Finish Phase 3.** Implement durable automatic-run scheduling and lifecycle
   records, restart recovery and a real Checks/Actions view. Establish stable
   repository identity and routing before binding durable cross-repository state
   to it. Implement real repository registration/creation/import/listing and
   lifecycle administration, then actual SSH Git transport and full pull-request
   collaboration. Verify permission denial, concurrency, partial failure and
   recovery for each vertical slice. Complete the final-candidate restoration,
   deployment/platform validation and independent review requirements already
   listed in the roadmap.
4. **Finish Phase 4.** Implement source-of-truth operation with an upstream backup
   and verified rollback; GitHub Actions-compatible workflow/job dependencies,
   matrices, secret handling, artifacts and caches; stronger execution isolation,
   multi-node scheduling and shared build caches; SSO/OIDC/LDAP, two-factor
   authentication and a durable audit trail; and repository-authorized code and
   symbol search. Domain models and preview controls alone do not complete these
   capabilities.

Repository CI is a development verification tool. It does not implement the
GitHub Actions-compatible execution engine inside Gitust. Likewise, current MCP
OAuth is agent authorization, not enterprise account SSO; SSH key registration
is not an SSH Git server; and AuthStore repository metadata is not an operational
multi-repository registry.

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
