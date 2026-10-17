# Offline backup and restore

Gitust currently stores authentication in a JSON snapshot and task state beside Git's own data. A Git clone or bundle alone does not preserve a deployment: it omits uncommitted work, linked-worktree administration, owners, approvals, and execution history.

This procedure is a cold copy for a trusted single-operator deployment. Stop every writer before copying, restore into a new private directory, and validate the restored copy before using it. It is not an online snapshot or an automatic disaster-recovery service.

## Repeatable backup tool

For the supported layout below, prefer the repository's Python 3 utility. It holds the
Gitust service lock and auth lock while copying, preserves dirty/ignored files and symlinks,
and records streaming SHA-256 checksums. It refuses an existing destination, unsupported
external worktrees/object alternates, busy locks and incomplete or damaged snapshots.
Stop ordinary Git processes and editors too: they do not cooperate with Gitust's locks.

```sh
python3 scripts/backup.py create \
  --repo /srv/gitust/project \
  --auth-db /srv/gitust/private/auth-db.json \
  --output /srv/private-backups/gitust-snapshot

python3 scripts/backup.py restore \
  --backup /srv/private-backups/gitust-snapshot \
  --destination /srv/gitust-restore-rehearsal
```

Parents must already exist; both output directories must be absent. The restored repository
is `gitust-restore-rehearsal/repository`, its auth file is `auth/auth-db.json`, and a normal
checkout's managed worktrees are `.repository.gitust-worktrees/`. A bare repository retains
its internal `repository/gitust-worktrees/` layout. The tool reconnects both worktree pointer
directions **inside the restored copy** and runs Git integrity checks. No manual `repair`
is needed after successful scripted restoration. The backup is sensitive, unencrypted
operator data; its checksums detect corruption, not a maliciously replaced manifest.
Use only backups you control. An interrupted operation can leave a partial directory;
do not reuse it or treat it as complete. A completed restore contains a `RESTORED` marker.

The tool does not start a server, verify accounts through login, restore outbound SSH
credentials, or infer fresh verification authority. Complete sections 4 and 5 against the
restored service. The manual procedure below explains the same storage and recovery model.

## What must be preserved

Resolve the repository and auth database from the actual service command. `--auth-db` defaults to `.gitust/auth-db.json` relative to the process working directory; it is not necessarily inside `--repo`.

| Location | Content and recovery meaning |
|---|---|
| Entire main repository directory | Working files, including ignored/untracked files, and the complete Git common directory: objects, refs, configuration, reflogs, index, and linked-worktree administration. |
| Default managed worktree directory | For a normal checkout named `project`, the sibling `.project.gitust-worktrees/`; for a bare repository, `<common Git directory>/gitust-worktrees/`. Preserve every UUID directory, including its `.git` file and unfinished edits. |
| `<common>/worktrees/<admin name>/gitust-origin.json` | Immutable worktree ID, agent branch, original target branch, creation SHA and an optional task display label. Preserve the complete file and its association with the surrounding Git worktree administration directory. |
| `<common>/gitust-owners/<UUID>.json` | Server-created worktree ownership by auth user ID. The matching auth snapshot is required to retain account identity. |
| `<common>/gitust-approvals/<UUID>.json` | Durable human approval, exact approved SHA, target branch, and last publication destination. These records can restore `Merged`, subject to validation. |
| `<common>/gitust-check-runs/<UUID>.json` | Up to five recent completed runs per worktree, with bounded command output. Historical success does not restore verification authority. |
| The configured auth JSON file | Users and password hashes, invite/reset-token state, sessions, PATs, registered SSH public keys, repository records, and security policy. Tokens are hashed, but the snapshot remains sensitive. |
| Service configuration, separately | Binary version, startup working directory, `--repo`, `--auth-db`, `--sync-github`, sandbox flag, bind address, and any timeout settings. These settings are not reconstructed from task history. |

The permanent `<common>/gitust-worktree-service.lock` file and `<auth-db>.lock` coordinate running processes. Their presence does not prove that a process is running or stopped. Copying a lock file does not copy an active OS lock; never unlink a live deployment's lock file to bypass ownership.

New tasks retain their supplied display label in the origin record, bounded to 4,096
UTF-8 bytes within a 32 KiB record. Older records without that optional field retain a
generic recovered label. Restore with the same or a compatible newer binary: older
binaries reject the new field, so downgrading a labelled installation is unsupported.
Labels never restore verification authority. CLI sandbox tasks use a fixed display
label rather than persisting their raw shell command as an origin label.

The examples below cover a main normal checkout with a real `.git` directory, or a main bare repository, using the default managed-worktree layout. Stop if the main path is itself a linked worktree, Git uses an external common directory, managed worktrees use a custom base, Git metadata/storage contains symlinks, the repository is a partial clone, or `objects/info/alternates` references external object storage. Those layouts need a separately reviewed backup of all dependencies. Ordinary linked worktrees outside the managed base must also be included and reconnected explicitly; the examples do not cover them. Submodules and Git LFS require their own completeness checks.

## 1. Quiesce and inventory

1. Stop new web/MCP requests, agent editing, scheduled Git operations, and service auto-restart.
2. Let active commands finish, then stop `gitust serve`, stdio `gitust mcp`, and any CLI or ordinary Git process using this repository or auth database. Confirm the processes and child commands have exited using the service manager and process inspection. A health-check failure alone is insufficient.
3. Keep the writers stopped through the entire copy and its validation. A filesystem copy does not acquire Gitust's locks or freeze another editor.
4. Record the binary version, actual startup settings, expected worktree IDs/branches, and expected pending/merged tasks in a private operator record. Do not paste auth JSON, passwords, tokens, or command output containing source code into a public report.

Set absolute paths for your deployment and a **new**, nonexisting backup directory beneath an operator-owned private parent. The backup must be outside the source repository and managed worktree tree. Do not enable shell tracing.

```sh
set -eu
umask 077
repo=/srv/gitust/project
auth_db=/srv/gitust/private/auth-db.json
backup=/srv/private-backups/gitust-2026-10-17

test -d "$repo"
test -f "$auth_db"
test ! -L "$auth_db"
test ! -e "$backup"
repo=$(cd "$repo" && pwd -P)
repo_name=$(basename "$repo")
repo_parent=$(dirname "$repo")
common=$(git -C "$repo" rev-parse --path-format=absolute --git-common-dir)
git -C "$repo" worktree list --porcelain > /dev/null
git -C "$repo" fsck --full --strict
```

Inspect `git worktree list --porcelain` privately to establish the complete checkout inventory. For a normal checkout, verify that `common` is its `.git` directory. For a bare repository it should be the repository directory itself. Confirm no external object alternates or worktrees are omitted before proceeding.

## 2. Copy the stopped deployment

The private parent directories in these examples must already exist and be owned by the operator. Preserve modes and symlinks without following symlink targets. Use a copy tool appropriate to the local filesystem; `cp -pR` below is intended for macOS/Linux. A failed command leaves an incomplete backup: do not use it for restoration or mark it complete.

```sh
mkdir -m 700 "$backup"
mkdir -m 700 "$backup/data" "$backup/auth"
cp -pR "$repo" "$backup/data/$repo_name"

managed="$repo_parent/.$repo_name.gitust-worktrees"
if test -d "$managed"; then
  test ! -L "$managed"
  cp -pR "$managed" "$backup/data/.$repo_name.gitust-worktrees"
fi

cp -p "$auth_db" "$backup/auth/auth-db.json"
chmod 600 "$backup/auth/auth-db.json"
git -C "$repo" show-ref > "$backup/refs.before"
git -C "$repo" worktree list --porcelain > "$backup/worktrees.before"
gitust --version > "$backup/version.txt"
```

For a bare repository, its default `gitust-worktrees/` is already inside the copied repository; do not substitute a sibling directory. A repository without any refs needs an explicit empty-ref inventory instead of treating a failed `show-ref` as a successful backup. Record the actual service command privately beside `version.txt`, excluding credentials.

Compare the stopped source with the backup before restarting production. Compare repository and managed-worktree file contents and symlink targets, and compare the auth file with `cmp`; keep comparison output private because filenames can be sensitive. Establish checksums with your platform's checksum tool if the backup will be transferred, and verify them after transfer. Checksums detect copy damage; they do not authenticate an untrusted backup.

Do not use `git clone`, `git bundle`, or `git archive` as a replacement for the full copy. Do not store the backup in Git or a publicly accessible directory. Encrypt backups when storing or transferring them outside the operator's protected storage. Preserve a verified prior backup until the new one has passed a restore rehearsal.

## 3. Restore into a separate directory

Use the same Gitust binary version first. Choose a new directory outside both the source and backup. Keep the repository's leaf name unchanged so the default sibling worktree directory still matches it. Do not overwrite an existing deployment and do not run Gitust against the copied data before reconnecting every linked worktree.

```sh
restore=/srv/gitust-restore-rehearsal
test ! -e "$restore"
mkdir -m 700 "$restore"
mkdir -m 700 "$restore/private-auth"
cp -pR "$backup/data/$repo_name" "$restore/$repo_name"

if test -d "$backup/data/.$repo_name.gitust-worktrees"; then
  cp -pR "$backup/data/.$repo_name.gitust-worktrees" \
    "$restore/.$repo_name.gitust-worktrees"
fi

cp -p "$backup/auth/auth-db.json" "$restore/private-auth/auth-db.json"
chmod 600 "$restore/private-auth/auth-db.json"
restored_repo="$restore/$repo_name"
```

Restore as the intended service account, or use an administrator-approved ownership transfer. Retain executable bits and private state modes; do not apply a blanket recursive `chmod` to source files. In particular, `gitust-check-runs/` must remain `0700` on Unix. Protect all ancestor directories. Copying does not supply the GitHub SSH private key, SSH agent, or trusted `known_hosts`; restore/reconfigure outbound credentials separately under the operator's control.

### Reconnect linked worktrees

A copied worktree's `.git` file and its common-directory `worktrees/<admin name>/gitdir` back-pointer can still contain source paths. **Before running Git inside a restored linked worktree**, run repair from the restored main repository and pass only the new copied worktree paths:

```sh
# Normal checkout example; replace each UUID with an actual copied directory.
git -c core.hooksPath=/dev/null -C "$restored_repo" worktree repair -- \
  "$restore/.$repo_name.gitust-worktrees/<worktree-UUID>"
```

For a bare repository the copied path is `$restored_repo/gitust-worktrees/<worktree-UUID>`. Include every copied linked worktree, as separate quoted arguments or separate repair commands. Do not pass original/source paths and do not run repair from the original repository. Keep Gitust's UUID directory names and `agent/*` branch names unchanged. Do not delete/recreate origin, owner, approval, or history records to make a partial restore start.

After repair, use `git -C "$restored_repo" worktree list --porcelain` and confirm **every** listed path belongs to the restored tree. For each restored linked worktree, `git rev-parse --path-format=absolute --git-common-dir` must resolve to the restored common directory; its Git administration directory must also be inside that directory's `worktrees/`. Stop if any pointer still references the source or backup. Recheck that source worktree pointers are unchanged during a rehearsal. Do not prune broken registrations before accounting for them: pruning can hide an incomplete restore, and Gitust startup itself performs a prune before recovery.

## 4. Validate data before starting

1. Compare `git show-ref` in the restored main repository with `refs.before`; all ref names and object IDs must match. Check the expected target and agent branch SHAs independently with `git rev-parse`.
2. Run `git fsck --full --strict` on the restored main repository and each linked worktree. Any missing/corrupt object or nonzero result blocks acceptance. Dangling-object notices require inspection but are not automatically corruption.
3. Compare main-checkout and linked-worktree files, including ignored/untracked work, with the stopped snapshot. Allow only the `.git` pointer and Git `gitdir` back-pointer changes needed for relocation.
4. Compare the restored auth JSON and Gitust origin/owner/approval/history files byte-for-byte with the backup. Confirm worktree counts, UUIDs, branch names, and account IDs agree with the private inventory. Git's integrity check does not validate these JSON records.
5. Keep the backup unchanged. If a command fails or data is missing, leave production untouched and investigate the isolated restore. Do not repair by discarding a broken record or inventing an approval.

## 5. Start locally and revalidate authority

Start the restored deployment on an unused loopback port with explicit absolute paths and the recorded upstream setting. Leave the production service routing and DNS unchanged. Initially omit the trusted-execution flag while inspecting the data:

```sh
gitust serve --bind 127.0.0.1:3300 \
  --repo "$restored_repo" \
  --auth-db "$restore/private-auth/auth-db.json" \
  --sync-github '<recorded-upstream-URL>'
```

Do not invoke inbound synchronization or publication during the initial inspection. Rehearsals must avoid publishing real changes; use a separate disposable upstream for publication tests and record that changed destination. A changed remote intentionally clears the UI's previous destination-specific publication indication.

Verify these observable results through a fresh login and the real web/API:

- Existing users retain their IDs, roles, and active/disabled state. Check a permitted login and rejection of a known disabled account. Use actual saved credentials privately; never print the auth snapshot. Restoring an older snapshot also restores its older revocations/passwords, so account and credential changes since the snapshot must be reconciled before exposing the service.
- Managed worktree UUIDs, agent branches, creation targets, and server-created owners match the snapshot. Legacy tasks with no owner remain administrator-managed. Repository runtime IDs and display timestamps are not stable backup identifiers.
- Tasks with valid durable approval records recover as `Merged`, and their exact approved SHAs remain on the recorded target branch. Publication status applies only to the same configured destination; it is not a fresh check of the remote's current refs.
- Unmerged tasks recover as `Pending`, with no `verified_commit` or `verified_checks_oid`. A past passing run must **not** allow immediate approval. Re-run explicit verification after enabling execution deliberately in the isolated trusted deployment, then review before approving.
- The execution-history view returns the saved recent completed runs, with the same command/results and truncation notices. The current history is bounded diagnostics, not a full audit log; it does not record every interrupted execution.
- A detached checkout or a legacy task without trustworthy creation-target metadata cannot gain approval through restore. Corrupt origin/owner/approval data blocks startup; corrupt history is surfaced when history is read. Stop on those errors instead of deleting evidence.

Sessions and PAT hashes are present in the auth snapshot; expired or incompatible sessions can still be rejected by normal authentication. OAuth client registrations and pending authorization codes are in memory and do not survive restart. Reconnect MCP clients as needed. New verification evidence must always come from a new execution, never from restored history.

For acceptance, record the backup identifier, binary/Git/OS versions, copied paths, ref comparison, both Git integrity checks, account/ownership checks, history check, approval-denial-before-reverification, and the final re-verification result. Keep that evidence private and separate from credentials. A written procedure or a successful cold copy alone is not proof that the deployment can be recovered.

## Current limits

- There is no online point-in-time snapshot spanning Git refs, auth JSON, and approval/history files. All writers must remain stopped during a manual backup.
- Git mutations and approval-record writes are separate operations. An already inconsistent source snapshot stays inconsistent; the backup does not manufacture missing human approval evidence. See [approval and publication recovery](../architecture/outbound-sync.md).
- Restoring a snapshot does not undo changes already accepted by GitHub. Compare upstream refs before any later synchronization or publication; never force-push to make them match the backup.
- Backup encryption, retention, off-host storage, credential rotation, and service-manager shutdown are operator responsibilities. This runbook does not claim a completed macOS/Linux deployment recovery rehearsal.

The pointer-reconnection example was checked with a disposable normal repository and a linked worktree using Git 2.52.0: the copied layout resolved to its copied common directory, original pointers were unchanged, and main/linked `git fsck --full --strict` passed. That check validates the Git relocation step only, not full Gitust or cross-platform recovery.

## Interrupted execution precondition

The backup command also rejects unresolved execution fences, even if the crashed
server's OS lock has already been released. Stop every prior command and follow
the [explicit recovery procedure](execution-recovery.md) before taking a new
snapshot. Restoring or relocating an installation must preserve these fences;
it must not automatically clear evidence of a possibly live command.
