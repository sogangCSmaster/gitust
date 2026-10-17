use std::fs;
use std::path::Path;
use std::process::Command;

use gitust_core::{AgentKind, RepoId, WorktreeId, WorktreeStatus};
use tempfile::TempDir;

#[path = "tests_approvals.rs"]
mod tests_approvals;
#[path = "tests_browse_immutable.rs"]
mod tests_browse_immutable;
#[path = "tests_checks.rs"]
mod tests_checks;
#[path = "tests_head_identity.rs"]
mod tests_head_identity;
#[path = "tests_review_security.rs"]
mod tests_review_security;
#[path = "tests_worktree_commit.rs"]
mod tests_worktree_commit;
#[path = "tests_worktree_commit_aliases.rs"]
mod tests_worktree_commit_aliases;
#[path = "tests_worktree_commit_guards.rs"]
mod tests_worktree_commit_guards;
#[path = "tests_worktree_commit_modes.rs"]
mod tests_worktree_commit_modes;
#[path = "tests_worktree_commit_normalize.rs"]
mod tests_worktree_commit_normalize;
#[path = "tests_worktree_commit_normalize_index.rs"]
mod tests_worktree_commit_normalize_index;
#[path = "tests_worktree_labels.rs"]
mod tests_worktree_labels;
#[path = "tests_worktree_origin.rs"]
mod tests_worktree_origin;
#[path = "tests_worktree_replacements.rs"]
mod tests_worktree_replacements;

use crate::config_guard::config_contains_credentials;
use crate::{GitEngine, GitEngineError, MaterializedWorktree};

fn git(dir: &Path, args: &[&str]) -> String {
    let mut command = Command::new("git");
    crate::process::sanitize_git_environment(&mut command);
    let out = command
        .arg("-C")
        .arg(dir)
        .args(["-c", "commit.gpgsign=false", "-c", "tag.gpgsign=false"])
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Gitust Test")
        .env("GIT_AUTHOR_EMAIL", "test@company.internal")
        .env("GIT_COMMITTER_NAME", "Gitust Test")
        .env("GIT_COMMITTER_EMAIL", "test@company.internal")
        .output()
        .expect("spawn git");
    assert!(
        out.status.success(),
        "git {:?} failed in {}: {}",
        args,
        dir.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

fn init_repo(dir: &Path) {
    fs::create_dir_all(dir.join("src")).expect("mkdir");
    git(dir, &["init", "-q", "-b", "main"]);
    fs::write(dir.join("src/lib.rs"), "pub fn answer() -> u32 { 42 }\n").expect("write");
    git(dir, &["add", "."]);
    git(dir, &["commit", "-q", "-m", "initial commit"]);
}

fn materialize(engine: &GitEngine, branch: &str) -> Result<MaterializedWorktree, GitEngineError> {
    engine.materialize_agent_worktree(
        WorktreeId::new(),
        RepoId::new(),
        AgentKind::ClaudeCode,
        branch,
        "main",
        "test",
    )
}

#[test]
fn gate_agent_worktree_is_clean_checkout_create_only_and_shares_refs() {
    let tmp = TempDir::new().expect("tempdir");
    let repo_dir = tmp.path().join("repo");
    init_repo(&repo_dir);
    let base_commit = git(&repo_dir, &["rev-parse", "main"]);
    git(&repo_dir, &["branch", "release/1.0"]);

    // Dirty main working tree: uncommitted change + untracked secret.
    fs::write(
        repo_dir.join("src/lib.rs"),
        "pub fn answer() -> u32 { 0 }\n",
    )
    .expect("write");
    fs::write(repo_dir.join(".env"), "SECRET=1\n").expect("write");

    let engine = GitEngine::new(&repo_dir);

    // Namespace and traversal guards.
    assert!(matches!(
        materialize(&engine, "release/1.0"),
        Err(GitEngineError::BranchNotAgentScoped(_))
    ));
    assert!(matches!(
        materialize(&engine, "agent/../../etc/passwd"),
        Err(GitEngineError::InvalidBranchName(_))
    ));
    assert!(matches!(
        materialize(&engine, "agent/"),
        Err(GitEngineError::InvalidBranchName(_) | GitEngineError::BranchNotAgentScoped(_))
    ));
    assert_eq!(git(&repo_dir, &["rev-parse", "release/1.0"]), base_commit);

    let m = materialize(&engine, "agent/claude/task-1.2").expect("materialize");
    assert_eq!(m.worktree.status, WorktreeStatus::Pending);
    assert_eq!(m.worktree.base_commit.0, base_commit);

    // Clean checkout of the base commit: no uncommitted changes, no untracked files.
    assert_eq!(
        fs::read_to_string(m.worktree_dir.join("src/lib.rs")).expect("read"),
        "pub fn answer() -> u32 { 42 }\n"
    );
    assert!(!m.worktree_dir.join(".env").exists());

    // Create-only: the same branch cannot be created twice.
    assert!(matches!(
        materialize(&engine, "agent/claude/task-1.2"),
        Err(GitEngineError::BranchAlreadyExists(_))
    ));

    // Reflog written for the agent branch.
    assert!(repo_dir
        .join(".git/logs/refs/heads/agent/claude/task-1.2")
        .is_file());

    // Edits in the worktree do not touch the main working tree; commits share refs/objects.
    fs::write(m.worktree_dir.join("src/agent.rs"), "// agent\n").expect("write");
    git(&m.worktree_dir, &["add", "."]);
    git(&m.worktree_dir, &["commit", "-q", "-m", "agent change"]);
    assert!(!repo_dir.join("src/agent.rs").exists());
    let wt_head = git(&m.worktree_dir, &["rev-parse", "HEAD"]);
    assert_eq!(
        git(&repo_dir, &["rev-parse", "agent/claude/task-1.2"]),
        wt_head
    );

    // GC in the main repo is worktree-aware: nothing the worktree needs is pruned.
    git(&repo_dir, &["gc", "-q", "--prune=now"]);
    GitEngine::verify_git_fsck(&m.worktree_dir).expect("worktree fsck after gc");
    GitEngine::verify_git_fsck(&repo_dir).expect("main fsck after gc");

    // Listing and removal.
    let listed = engine.list_agent_worktrees().expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].worktree_dir, m.worktree_dir);
    assert_eq!(listed[0].branch, "agent/claude/task-1.2");

    assert!(matches!(
        engine.remove_agent_worktree(&repo_dir, "agent/claude/task-1.2", &m.worktree.base_commit),
        Err(GitEngineError::WorktreeNotManaged(_))
    ));
    // The branch has an agent commit: the checkout goes, the branch (the work) stays.
    let deleted = engine
        .remove_agent_worktree(
            &m.worktree_dir,
            "agent/claude/task-1.2",
            &m.worktree.base_commit,
        )
        .expect("remove");
    assert!(!deleted);
    assert!(!m.worktree_dir.exists());
    assert!(engine.list_agent_worktrees().expect("list").is_empty());
    assert_eq!(
        git(&repo_dir, &["rev-parse", "agent/claude/task-1.2"]),
        wt_head
    );

    // An untouched worktree: both checkout and branch are removed.
    let untouched = materialize(&engine, "agent/claude/untouched").expect("materialize");
    let deleted = engine
        .remove_agent_worktree(
            &untouched.worktree_dir,
            "agent/claude/untouched",
            &untouched.worktree.base_commit,
        )
        .expect("remove");
    assert!(deleted);
    let branches = git(&repo_dir, &["branch", "--list", "agent/claude/untouched"]);
    assert!(
        branches.is_empty(),
        "untouched branch must be deleted: {branches}"
    );
    // Removing again is a no-op, not an error.
    assert!(!engine
        .remove_agent_worktree(
            &untouched.worktree_dir,
            "agent/claude/untouched",
            &untouched.worktree.base_commit,
        )
        .expect("idempotent remove"));
    GitEngine::verify_git_fsck(&repo_dir).expect("main fsck after remove");
}

#[test]
fn gate_agent_worktree_from_bare_mirror() {
    let tmp = TempDir::new().expect("tempdir");
    let src = tmp.path().join("src-repo");
    init_repo(&src);
    let bare = tmp.path().join("mirror.git");
    git(
        tmp.path(),
        &[
            "clone",
            "-q",
            "--bare",
            src.to_str().expect("utf8"),
            bare.to_str().expect("utf8"),
        ],
    );

    let engine = GitEngine::new(&bare);
    let m = materialize(&engine, "agent/cursor/bare").expect("materialize from bare");
    assert!(m.main_workdir.is_none());
    assert_eq!(
        fs::read_to_string(m.worktree_dir.join("src/lib.rs")).expect("read"),
        "pub fn answer() -> u32 { 42 }\n"
    );
    GitEngine::verify_git_fsck(&m.worktree_dir).expect("fsck");
}

#[test]
fn recover_agent_worktrees_reconstructs_id_base_commit_and_prunes_stale() {
    let tmp = TempDir::new().expect("tempdir");
    let repo_dir = tmp.path().join("repo");
    init_repo(&repo_dir);
    let engine = GitEngine::new(&repo_dir);

    let untouched = materialize(&engine, "agent/claude/untouched").expect("materialize");
    let committed = materialize(&engine, "agent/cursor/committed").expect("materialize");
    let stale = materialize(&engine, "agent/codex/stale").expect("materialize");
    fs::write(committed.worktree_dir.join("src/new.rs"), "// work\n").expect("write");
    git(&committed.worktree_dir, &["add", "."]);
    git(&committed.worktree_dir, &["commit", "-q", "-m", "work"]);
    // Simulate an ungraceful crash where a worktree dir was deleted externally.
    fs::remove_dir_all(&stale.worktree_dir).expect("rm stale");

    let recovered = GitEngine::new(&repo_dir)
        .recover_agent_worktrees(RepoId::new())
        .expect("recover");
    assert_eq!(recovered.len(), 2);

    let rec_untouched = recovered
        .iter()
        .find(|w| w.worktree.branch == "agent/claude/untouched")
        .expect("untouched");
    assert_eq!(rec_untouched.worktree.id, untouched.worktree.id);
    assert_eq!(
        rec_untouched.worktree.base_commit,
        untouched.worktree.base_commit
    );
    assert_eq!(rec_untouched.worktree.base_branch, "main");
    assert_eq!(rec_untouched.worktree.agent, AgentKind::ClaudeCode);

    let rec_committed = recovered
        .iter()
        .find(|w| w.worktree.branch == "agent/cursor/committed")
        .expect("committed");
    assert_eq!(rec_committed.worktree.id, committed.worktree.id);
    assert_eq!(
        rec_committed.worktree.base_commit,
        committed.worktree.base_commit
    );
    assert_eq!(rec_committed.worktree.agent, AgentKind::CursorAgent);
}

#[test]
fn gate_refuses_repos_with_credentials_in_config() {
    let tmp = TempDir::new().expect("tempdir");
    let repo_dir = tmp.path().join("repo");
    init_repo(&repo_dir);
    git(
        &repo_dir,
        &[
            "remote",
            "add",
            "origin",
            "https://x-access-token:ghp_secret@github.com/acme/app.git",
        ],
    );
    let engine = GitEngine::new(&repo_dir);
    assert!(matches!(
        materialize(&engine, "agent/claude/creds"),
        Err(GitEngineError::CredentialsInRepoConfig)
    ));
    // Nothing was created.
    assert!(git(&repo_dir, &["branch", "--list", "agent/*"]).is_empty());

    // Also reject credentials placed inside `.git/config.worktree`.
    git(
        &repo_dir,
        &[
            "remote",
            "set-url",
            "origin",
            "https://github.com/acme/app.git",
        ],
    );
    fs::write(
        repo_dir.join(".git/config.worktree"),
        "[http]\n\textraHeader = Authorization: Bearer ghp_worktree_secret\n",
    )
    .expect("write config.worktree");
    assert!(matches!(
        materialize(&engine, "agent/claude/creds-wt"),
        Err(GitEngineError::CredentialsInRepoConfig)
    ));
}

#[test]
fn credential_detection_cases() {
    assert!(config_contains_credentials(
        "[remote \"o\"]\n\turl = https://user:pw@host/r.git\n"
    ));
    assert!(config_contains_credentials(
        "[remote \"o\"]\n\turl = https://\"user:pw@host\"/r.git\n"
    ));
    assert!(config_contains_credentials(
        "[remote \"o\"]\n\turl = \"https://user:pw@\"host/r.git\n"
    ));
    assert!(config_contains_credentials(
        "[remote \"o\"]\n\turl = https://ghp_token@github.com/r.git\n"
    ));
    assert!(config_contains_credentials(
        "[http]\n\textraHeader = Authorization: Bearer x\n"
    ));
    assert!(config_contains_credentials(
        "[url \"https://tok@github.com/\"]\n\tinsteadOf = https://github.com/\n"
    ));
    assert!(config_contains_credentials(
        "[credential]\n\thelper = store --file .git/creds\n"
    ));
    assert!(config_contains_credentials(
        "[credential]\n\thelper = \"st\"\"ore --file .git/creds\"\n"
    ));
    assert!(config_contains_credentials(
        "[credential]\n\thelper = \"store --file .git/creds\"\n"
    ));
    assert!(config_contains_credentials(
        "[credential]\n\thelper = !f() { echo password=ghp_secret; }; f\n"
    ));
    assert!(config_contains_credentials(
        "[credential \"https://tok@github.com\"]\n\thelper = osxkeychain\n"
    ));
    assert!(config_contains_credentials(
        "[include]\n\tpath = ~/.gitconfig-secrets\n"
    ));
    assert!(config_contains_credentials(
        "[include\t]\n\tpath = ~/.gitconfig-secrets\n"
    ));
    assert!(config_contains_credentials(
        "[credential]\n\thelper = \\\n\tstore --file .git/creds\n"
    ));
    assert!(config_contains_credentials(
        "[includeIf \"gitdir:~/work/\"]\n\tpath = ~/.gitconfig-work\n"
    ));
    assert!(!config_contains_credentials(
        "[credential]\n\thelper = osxkeychain\n"
    ));
    assert!(!config_contains_credentials(
        "[remote \"o\"]\n\turl = https://github.com/acme/r.git\n"
    ));
    assert!(!config_contains_credentials(
        "[remote \"o\"]\n\turl = git@github.com:acme/r.git\n"
    ));
    assert!(!config_contains_credentials(
        "[remote \"o\"]\n\turl = ssh://git@github.com/acme/r.git\n"
    ));
}

#[test]
fn idempotent_remove_already_deleted_dir_and_detached_head_recovery() {
    let tmp = TempDir::new().expect("tempdir");
    let repo_dir = tmp.path().join("repo");
    init_repo(&repo_dir);
    let engine = GitEngine::new(&repo_dir);

    let m1 = materialize(&engine, "agent/claude/idempotent-1").expect("m1");
    let m2 = materialize(&engine, "agent/claude/detached-2").expect("m2");

    // Simulate crash after m1's directory was deleted on disk while m2 keeps `worktrees_base`
    // alive; pass a non-canonical path through the tempdir root.
    fs::remove_dir_all(&m1.worktree_dir).expect("delete m1 dir");
    let non_canonical_m1 = repo_dir
        .parent()
        .expect("parent")
        .join(m1.worktrees_base.file_name().expect("base name"))
        .join(m1.worktree_dir.file_name().expect("leaf name"));
    let deleted = engine
        .remove_agent_worktree(
            &non_canonical_m1,
            &m1.worktree.branch,
            &m1.worktree.base_commit,
        )
        .expect("idempotent remove should succeed even when dir was already gone");
    assert!(deleted, "unchanged branch should still be cleaned up");

    // Detach HEAD inside m2 and verify list/recover still see it.
    git(&m2.worktree_dir, &["checkout", "-q", "--detach", "HEAD"]);
    let listed = engine.list_agent_worktrees().expect("list");
    assert_eq!(listed.len(), 1, "detached worktree must remain visible");
    let recovered = engine
        .recover_agent_worktrees(m2.worktree.repo_id)
        .expect("recover");
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].worktree.id, m2.worktree.id);
}

#[test]
fn gate_7_4_git_integrity_clone_commit_push_fetch_roundtrip_and_fsck() {
    let tmp = TempDir::new().expect("create tempdir");
    let server_repo = tmp.path().join("server.git");
    let seed_client = tmp.path().join("seed-client");
    let clone_a = tmp.path().join("client-a");
    let clone_b = tmp.path().join("client-b");

    fs::create_dir_all(&server_repo).expect("mkdir server");
    fs::create_dir_all(&seed_client).expect("mkdir seed");

    git(&server_repo, &["init", "-q", "--bare", "-b", "main"]);

    git(&seed_client, &["init", "-q", "-b", "main"]);
    fs::write(seed_client.join("README.md"), "# Gitust Integrity Gate\n").expect("write");
    git(&seed_client, &["add", "README.md"]);
    git(&seed_client, &["commit", "-q", "-m", "seed commit"]);
    git(
        &seed_client,
        &["remote", "add", "origin", server_repo.to_str().expect("p")],
    );
    git(&seed_client, &["push", "-q", "-u", "origin", "main"]);

    git(
        tmp.path(),
        &[
            "clone",
            "-q",
            server_repo.to_str().expect("p"),
            clone_a.to_str().expect("p"),
        ],
    );
    fs::create_dir_all(clone_a.join("crates")).expect("mkdir crates");
    fs::write(clone_a.join("crates/engine.rs"), "pub const A: u64 = 1;\n").expect("write");
    git(&clone_a, &["add", "."]);
    git(&clone_a, &["commit", "-q", "-m", "feat: add engine module"]);
    git(&clone_a, &["push", "-q", "origin", "main"]);

    git(
        tmp.path(),
        &[
            "clone",
            "-q",
            server_repo.to_str().expect("p"),
            clone_b.to_str().expect("p"),
        ],
    );
    fs::write(
        clone_a.join("crates/engine.rs"),
        "pub const A: u64 = 1;\npub const B: bool = true;\n",
    )
    .expect("update");
    git(&clone_a, &["commit", "-q", "-am", "feat: second change"]);
    git(&clone_a, &["push", "-q", "origin", "main"]);

    git(&clone_b, &["fetch", "-q", "origin", "main"]);
    git(&clone_b, &["merge", "-q", "--ff-only", "origin/main"]);

    let server_head = GitEngine::new(&server_repo)
        .inspect_head_zero_copy()
        .expect("inspect server head");
    let client_b_head = GitEngine::new(&clone_b)
        .inspect_head_zero_copy()
        .expect("inspect client_b head");

    assert_eq!(server_head.head_sha, client_b_head.head_sha);
    assert!(server_head
        .root_entries
        .iter()
        .any(|e| e.filename == "crates" && e.is_dir));

    GitEngine::verify_git_fsck(&server_repo).expect("server git fsck");
    GitEngine::verify_git_fsck(&clone_a).expect("client A git fsck");
    GitEngine::verify_git_fsck(&clone_b).expect("client B git fsck");
}

#[test]
fn worktree_service_lock_rejects_second_concurrent_owner() {
    let tmp = TempDir::new().expect("tempdir");
    let repo_dir = tmp.path().join("repo");
    fs::create_dir_all(&repo_dir).expect("mkdir");
    git(&repo_dir, &["init", "-q", "-b", "main"]);
    fs::write(repo_dir.join("README.md"), "# Lock Test\n").expect("write");
    git(&repo_dir, &["add", "README.md"]);
    git(&repo_dir, &["commit", "-q", "-m", "initial"]);

    let engine_a = GitEngine::new(&repo_dir);
    let engine_b = GitEngine::new(&repo_dir);

    let guard_a = engine_a
        .lock_worktree_service()
        .expect("first service lock must succeed");
    let err = engine_b
        .lock_worktree_service()
        .expect_err("second concurrent service lock must be rejected");
    assert!(
        matches!(err, GitEngineError::WorktreeServiceBusy(_)),
        "expected WorktreeServiceBusy, got: {err:?}"
    );

    drop(guard_a);
    let _guard_b = engine_b
        .lock_worktree_service()
        .expect("service lock must succeed after first owner releases it");
}

#[test]
fn recover_worktrees_and_mirror_refs_fail_closed_on_corrupt_entries() {
    let tmp = TempDir::new().expect("tempdir");
    let upstream = tmp.path().join("upstream");
    let mirror = tmp.path().join("mirror");
    init_repo(&upstream);
    git(&upstream, &["branch", "feature"]);

    let engine = GitEngine::new(&mirror);
    let initial = engine
        .sync_inbound_read_only(upstream.to_str().expect("utf8"))
        .expect("initial sync");
    assert_eq!(initial.synced_branches_count, 2);

    // 1. Corrupt a managed worktree's `.git` pointer: `recover_agent_worktrees` must fail closed
    //    instead of silently omitting the worktree from the recovered list.
    let m = materialize(&engine, "agent/claude/corrupt-pointer").expect("materialize");
    fs::write(m.worktree_dir.join(".git"), "corrupted-pointer\n").expect("corrupt .git");
    assert!(
        engine.recover_agent_worktrees(RepoId::new()).is_err(),
        "corrupted worktree .git pointer must fail closed during recovery"
    );
    fs::remove_dir_all(&m.worktree_dir).expect("remove corrupt worktree dir");

    // 2. Replace local `refs/heads/feature` with a dangling symbolic ref (`ref: refs/heads/missing`)
    //    and advance upstream `feature`: `apply_inbound_mirror_refs` must record `feature` in
    //    `skipped_branches` rather than treating it as non-existent (`MustNotExist`).
    fs::write(
        upstream.join("src/lib.rs"),
        "pub fn answer() -> u32 { 99 }\n",
    )
    .expect("write");
    git(&upstream, &["checkout", "-q", "feature"]);
    git(&upstream, &["add", "."]);
    git(&upstream, &["commit", "-q", "-m", "advance feature"]);
    fs::write(
        mirror.join("refs/heads/feature"),
        "ref: refs/heads/missing-target\n",
    )
    .expect("write dangling symref");

    let second = engine
        .sync_inbound_read_only(upstream.to_str().expect("utf8"))
        .expect("second sync");
    assert!(
        second.skipped_branches.iter().any(|(b, _)| b == "feature"),
        "unreadable local ref must be recorded in skipped_branches: {:?}",
        second.skipped_branches
    );
}

#[path = "tests_browse_bench.rs"]
mod tests_browse_bench;
#[path = "tests_mirror.rs"]
mod tests_mirror;
