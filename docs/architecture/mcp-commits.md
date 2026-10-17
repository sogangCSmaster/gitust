# Host-side MCP worktree commits

`gitust_worktree_commit` commits selected files in a managed agent worktree without
granting command sandboxes write access to shared Git administration. It is part of
the trusted single-user alpha implementation. It does not run
verification, grant human approval, merge a target branch or publish upstream.

The current full checks, architecture review and recorded macOS rehearsal passed.
The independent HTTP review completed a set of local checks, but further review
was interrupted by tool policy before its final verdict. See [review scope and
limits](../security/mcp-commit-review-2026-10-17.md). Actual IDE-agent sessions,
GitHub publication and Linux runtime behavior remain unverified; see
[alpha progress](../operations/alpha-progress-2026-10-17.md).

## Request and result

Call the tool through `tools/call` on the existing stdio service or HTTP `/mcp`
endpoint. Obtain the task identity and branch from `gitust_worktree_list`. A new
task starts at its returned `base_commit`; subsequent successful commits return
the next expected parent in `commit`. The list's `base_commit` is the creation
SHA, not a moving HEAD field. Inspect the actual checkout/branch after any
indeterminate result, and inspect changes before selecting paths. A read-only Git
command through `gitust_sandbox_exec` is ordinary execution and also invalidates
earlier verification.

```json
{
  "jsonrpc": "2.0",
  "id": 2,
  "method": "tools/call",
  "params": {
    "name": "gitust_worktree_commit",
    "arguments": {
      "worktree_id": "00000000-0000-4000-8000-000000000001",
      "expected_head": "0123456789abcdef0123456789abcdef01234567",
      "paths": ["src/lib.rs", "removed-file.txt"],
      "message": "Explain the change and why it is needed"
    }
  }
}
```

Replace the example UUID, SHA and paths with observed task values. All four
arguments are required; unknown fields are rejected. The tool supplies the branch
from the managed task, rather than accepting an arbitrary ref or checkout path.

| Argument | Enforced boundary |
| --- | --- |
| `worktree_id` | UUID of an active managed linked worktree with valid creation-target metadata. |
| `expected_head` | Exactly 40 ASCII hexadecimal characters, normalized to lowercase. The attached checkout HEAD and its managed `agent/*` branch must both still equal this commit. |
| `paths` | 1–128 unique literal repository-relative file paths; at most 4,096 UTF-8 bytes each and 65,536 bytes combined. Include a tracked file's path to record its deletion. |
| `message` | Nonblank UTF-8 text, at most 8,192 bytes and without NUL. Multiline text is retained. |

Path components cannot be empty, `.`, `..` or case-insensitive `.git`; absolute
paths, backslashes and NUL are forbidden. Paths are literal, so spaces, brackets,
tabs, newlines and leading `-` do not become pathspecs or command options.
Directory recursion and submodules are unsupported. Filesystem case/Unicode
aliases of another Git spelling are rejected; use exact tracked names. Case-only
renames require an operator's ordinary Git workflow.

The success payload is JSON text inside the standard MCP `content` result:

```json
{
  "worktree_id": "00000000-0000-4000-8000-000000000001",
  "branch": "agent/example/task",
  "commit": "89abcdef0123456789abcdef0123456789abcdef",
  "parent": "0123456789abcdef0123456789abcdef01234567",
  "status": "pending",
  "verification_required": true
}
```

The author and committer are `Gitust agent`, with the acting principal's user UUID
at `agents.gitust.local`. The tool does not accept a client-supplied identity.
An unchanged selected tree is an error. Unselected working-file edits remain in
the checkout and can still prevent subsequent clean-commit verification.

## Transport and authority

HTTP commits require `serve --trusted-single-user-sandbox`, an active SiteAdmin
credential and the persisted creator of the worktree. A different SiteAdmin does
not gain the creator's commit authority. Repository access is checked, and merged,
discarded or durably approved tasks cannot receive another agent commit. The
HTTP handler authenticates again after reading the bounded request body, and the
commit path rechecks the original credential and repository authority immediately
before updating the branch.

The actual stdio service runs under the local operator's authority. It enables
trusted execution and can continue an unmerged recovered task whose stored owner
was created by an earlier stdio process. It retains that owner record. This local
operator authority is distinct from an authenticated HTTP principal, even if both
have SiteAdmin role. Do not expose a stdio process as an unauthenticated remote
service.

One Gitust worktree service owns a repository at a time. Connect agents to the
running server's HTTP endpoint when `serve` owns the repository. Stop that service
before starting an independent stdio service on the same repository.

Source reads retain the execution sandbox's hidden-file policy, including private
auth state and hardlink aliases. The policy also checks ancestor `.gitattributes`,
the common repository config and `info/attributes` read by normalization. This path remains a trusted single-operator
facility; it does not establish multi-tenant isolation from arbitrary host writers.

## Content and Git normalization

The tool accepts ordinary files, executable files, symlink text and tracked-file
deletions. It records a symlink's target text without opening its destination.
Directory parents are opened without following symlinks; directories, devices and
FIFOs are rejected. Reads are bounded to 8 MiB per file and 32 MiB in total.
Normalized blobs have the same per-file and total limits. Host-side source reads
currently require a Unix platform.

The real worktree index must contain no staged or unmerged changes, and cannot
hide entries with `assume-unchanged` or `skip-worktree`. The tool locks that index
and constructs a separate index starting at `expected_head`. Only the requested
entries are changed. It checks the resulting tree's exact paths, modes and object
IDs before creating the commit. The real and prepared indexes are bounded to
32 MiB.

Builtin Git content normalization supports `text`, legacy `crlf`, `eol`, `working-tree-encoding`
and `ident`, together with the relevant repository `core.autocrlf`, `core.eol`,
`core.safecrlf`, `core.checkRoundtripEncoding`, `core.ignoreCase` and
`core.precomposeUnicode` settings. Enabled `config.worktree` builtin settings are
included. Nested `.gitattributes` and common `info/attributes` retain Git's
precedence. Missing tracked attribute files fall back to the expected parent's
tree, matching the index used for this commit. An attribute file selected for
deletion is removed from that private index and is not restored by fallback.
Files without an applicable
conversion retain their bytes, including binary content and explicit `-text`.
`core.fileMode=false` and `core.symlinks=false` use the parent index's Git rules
for executable modes and emulated symlinks. Converted executable files retain
their captured owner-execute mode when mode tracking is enabled.

Managed execution, verification, commit, approval and publication reject repositories
with `refs/replace` entries. Replacement views can make a SHA read different bytes
from its stored object, so they cannot be implicit authority for exact-SHA publication.
This limit applies to loose and packed replacement refs; remove the replacement
through the operator's ordinary Git workflow and review the actual stored content
before retrying. Gitust does not silently remove or apply replacements.

Conversion batches Git's ordinary staging against the parent index using private
copies of allowed configuration, attributes and captured source bytes. This
preserves existing-blob CRLF behavior for `text=auto` and `core.autocrlf`; a raw
`hash-object` conversion does not provide the same semantics. The tool rechecks
captured metadata before the branch update. Metadata snapshots total
at most 1 MiB; attribute path discovery is limited to 512 paths/256 KiB. Attribute
and configuration symlinks, changed inputs, invalid encodings and failed builtin
conversions are errors. Global configuration is excluded, and an external
`core.attributesFile` is refused; `/dev/null` is permitted.

Executable clean/process filters, including Git LFS filters, are unsupported and
rejected when applicable. Hooks and GPG/signing programs do not execute. The tool
does not sign commits. These restrictions preserve the host execution boundary;
do not remove a repository's required filters merely to make a commit succeed.

## State, cancellation and errors

After authority, ownership and the per-worktree operation guard are established,
starting the attempt clears `verified_commit` and `verified_checks_oid` and sets
Pending. A successful commit stays Pending; a later failure marks Failed. Invalid
JSON/argument shape, unauthorized callers and a busy worktree can fail before
this transition. A repository-busy response occurs after the transition, so it
also requires fresh verification. Old verification is never restored by a retry.

The blocking worker owns the worktree/service/index guards and final state
transition. An HTTP disconnect does not cancel its already-running commit. Final
checkout, index, attribute and credential checks precede a compare-and-swap of
the branch from the expected parent to the prepared SHA. Changed parents,
detached or replaced checkouts, missing origin metadata, index changes and stale
locks fail closed. Preparation can leave unreachable Git objects, but it does
not grant approval or publish them upstream.

Execution errors use MCP `isError: true`, with the mapped `http_status` and message
in the content payload; invalid JSON-RPC/argument shape uses JSON-RPC errors.
HTTP transport status 200 alone is not tool success. Inspect the MCP result and
the observed branch SHA. In particular:

- Worktree/repository busy is a conflict. Stop concurrent operations and inspect
  the current HEAD before retrying an operation that may have reached the server.
- A stale `expected_head` is rejected. Review the actual branch; never replace the
  expected SHA blindly to bypass that check.
- The branch update and prepared-index installation are separate disk mutations.
  If index installation fails after the update, the error identifies the created
  commit SHA and says index recovery is required. The commit may exist even
  though the tool returned an error; no approval was granted.
- On an indeterminate response, compare HEAD, branch ref and reflog before any
  retry. Do the same for an ambiguous upstream publication result using the
  approved SHA and actual upstream ref; see [publication recovery](outbound-sync.md).

## Interrupted commit and stale index-lock recovery

Ordinary failure/drop removes only the lock owned by that attempt. An uncatchable
kill such as `SIGKILL`, a machine crash or filesystem damage can leave
`<worktree Git administration directory>/index.lock` or an older index after the
branch has advanced. Automatic stale-lock removal is not implemented.

1. Stop new HTTP/MCP requests, agent editing, service auto-restart, Gitust services,
   ordinary Git processes and their child commands. Confirm every writer exited
   using the service manager and process inspection. A timeout or failed health
   check does not establish this. Keep all writers stopped throughout recovery.
2. Make a private, mode-preserving backup of the complete working files, including
   untracked/ignored edits, plus the affected index, lock and Git metadata. Preserve
   symlinks as links. The [stopped-installation backup procedure](../operations/backup-restore.md)
   describes the full layout. If a copy fails, resolve it before changing anything.
3. Resolve and inspect the worktree's `.git` pointer, administration directory and
   common-directory association. Record HEAD, the expected agent ref, reflog,
   index/lock file identities and any SHA returned in an error. Compare the
   parent/tree with the intended task edits. Stop if ownership or identity is
   unclear; do not invent approval or rewrite refs to force agreement.
4. Only after confirming no writer owns it, remove the exact stale regular
   `index.lock` in this verified worktree administration directory. A symlink,
   directory or otherwise unexpected lock requires separate inspection. Do not
   unlink the permanent Gitust service/auth lock files to bypass their OS locks.
5. Rebuild the index from the inspected current HEAD with `reset --mixed --no-refresh HEAD`.
   This preserves working files but replaces staging state, which is why the
   original index and all edits must already be backed up. A commit created
   before the crash remains in branch history. Review any remaining working-file
   differences manually or within the execution sandbox and commit the intended
   edits through a new checked request. Avoid host working-tree diff/status commands
   until the repository's executable filters and configuration have been reviewed.
6. Run Git integrity checks, restart the intended single service and obtain fresh
   explicit verification for the observed committed content. History or a prior
   Verified display cannot restore approval eligibility.

For step 3 and step 5, this shell helper removes inherited `GIT_*` selectors and
disables global/system configuration, hooks and filesystem-monitor programs.
Set the absolute checkout path only after the stopped backup is complete:

```sh
worktree=/absolute/path/to/managed-worktree-UUID
git_recovery() {
  env -i PATH="$PATH" HOME="$HOME" \
    GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null \
    git -c core.hooksPath=/dev/null -c core.fsmonitor=false \
      -c core.untrackedCache=false -c core.attributesFile=/dev/null \
      -C "$worktree" "$@"
}

git_recovery rev-parse --absolute-git-dir
git_recovery symbolic-ref HEAD
git_recovery rev-parse --verify 'HEAD^{commit}'
git_recovery reflog show --format='%H %gs' -10 HEAD
```

Privately inspect those results and compare the known agent branch with
`rev-parse --verify 'refs/heads/agent/<known-branch>^{commit}'`. The commands below
are for step 5 onward, **after** backing up and manually resolving the verified
stale lock:

```sh
git_recovery reset --mixed --no-refresh HEAD
git_recovery diff-tree --no-commit-id --name-status -r HEAD
git_recovery fsck --full --strict
```

Do not use a hard reset, clean operation, automatic lock deletion or ref rollback
as part of this procedure. `--no-refresh` avoids inspecting working files during
index recovery; the tree command inspects committed objects only. The helper does
not make arbitrary Git commands safe: working-tree commands can consult clean
filters from repository configuration. Keep recovery output private: file names and reflogs can
contain source and task details.

## Test and operational evidence

The Git regression modules `tests_worktree_commit*.rs` exercise selected-file
isolation, independent Git tree/SHA observations, executable and symlink modes,
deletions, staged/hidden-index rejection, exact-parent gates, drop cleanup, index
recovery, path aliases, builtin normalization and forbidden external programs.
Server `tests_mcp_commit*.rs` cover HTTP OAuth editing→commit→verification→human
approval→publication, role/owner/trusted-mode denial, hidden sources, busy paths,
verification invalidation, streamed-body revocation and stdio restart ownership.
These describe test coverage; a current pass requires the actual recorded run.

The [ten-task rehearsal](../operations/alpha-rehearsal.md) also checks returned
commit parents, branches, selected paths and Pending state with independent Git
commands, then tests explicit approval/publication and relocated restoration.
The latest schema-2 macOS report identifies the binary after the index-aware
normalization fixes and its runner hashes. Earlier reports remain historical
evidence. The remaining alpha evidence and independent review limits stay open.
