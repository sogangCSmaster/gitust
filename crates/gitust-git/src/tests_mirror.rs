use std::fs;
use std::process::Command;

use tempfile::TempDir;

use super::{git, init_repo, materialize};
use crate::ancestry::{classify_branch_relation, BranchRelation};
use crate::{GitEngine, GitEngineError};

#[test]
fn competing_mirror_process_cannot_replace_an_owned_services_staging_refs() {
    let tmp = TempDir::new().expect("tempdir");
    let upstream = tmp.path().join("upstream");
    let foreign = tmp.path().join("foreign");
    let mirror = tmp.path().join("mirror");
    init_repo(&upstream);
    git(
        tmp.path(),
        &[
            "clone",
            upstream.to_str().unwrap(),
            foreign.to_str().unwrap(),
        ],
    );
    fs::write(
        foreign.join("foreign.txt"),
        "not from the configured upstream\n",
    )
    .unwrap();
    git(&foreign, &["add", "."]);
    git(&foreign, &["commit", "-m", "foreign source"]);
    git(&foreign, &["branch", "foreign-only"]);
    let owner = GitEngine::new(&mirror);
    owner
        .sync_inbound_read_only(upstream.to_str().unwrap())
        .unwrap();
    let service = owner.lock_worktree_service().unwrap();
    owner
        .fetch_inbound_mirror_staging(upstream.to_str().unwrap())
        .unwrap();
    let status = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "tests::tests_mirror::competing_mirror_process_probe",
        ])
        .env("GITUST_MIRROR_PROBE_REPO", &mirror)
        .env("GITUST_MIRROR_PROBE_REMOTE", &foreign)
        .status()
        .unwrap();
    assert!(
        status.success(),
        "competing process must reject the busy service"
    );
    let result = owner
        .apply_inbound_mirror_refs(upstream.to_str().unwrap(), std::time::Instant::now())
        .unwrap();
    assert_eq!(result.github_remote_url, upstream.to_str().unwrap());
    assert_eq!(
        git(&mirror, &["rev-parse", "refs/heads/main"]),
        git(&upstream, &["rev-parse", "main"])
    );
    assert!(!owner
        .list_non_agent_refs()
        .unwrap()
        .contains_key("refs/heads/foreign-only"));
    drop(service);
}

#[test]
fn competing_mirror_process_probe() {
    let Some(repo) = std::env::var_os("GITUST_MIRROR_PROBE_REPO") else {
        return;
    };
    let remote = std::env::var("GITUST_MIRROR_PROBE_REMOTE").unwrap();
    assert!(matches!(
        GitEngine::new(std::path::PathBuf::from(repo)).sync_inbound_read_only(&remote),
        Err(GitEngineError::WorktreeServiceBusy(_))
    ));
}

#[test]
fn first_bare_mirror_initialization_respects_the_service_lock() {
    let tmp = TempDir::new().unwrap();
    let upstream = tmp.path().join("upstream");
    let mirror = tmp.path().join("mirror");
    init_repo(&upstream);
    fs::create_dir(&mirror).unwrap();
    let lock = fs::File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(mirror.join("gitust-worktree-service.lock"))
        .unwrap();
    lock.try_lock().unwrap();
    let engine = GitEngine::new(&mirror);
    assert!(matches!(
        engine.sync_inbound_read_only(upstream.to_str().unwrap()),
        Err(GitEngineError::WorktreeServiceBusy(_))
    ));
    assert!(!mirror.join("HEAD").exists());
    lock.unlock().unwrap();
    drop(lock);
    engine
        .sync_inbound_read_only(upstream.to_str().unwrap())
        .unwrap();
    assert_eq!(
        git(&mirror, &["rev-parse", "main"]),
        git(&upstream, &["rev-parse", "main"])
    );
}

#[test]
fn gate_7_4_inbound_mirror_sync_matches_upstream_refs_and_preserves_private_agent_branches() {
    let tmp = TempDir::new().expect("tempdir");
    let upstream_dir = tmp.path().join("upstream");
    let mirror_dir = tmp.path().join("mirror");

    init_repo(&upstream_dir);
    git(&upstream_dir, &["branch", "release/1.0"]);
    git(
        &upstream_dir,
        &["tag", "-a", "v1.0.0", "-m", "annotated release v1.0.0"],
    );
    let upstream_annotated_tag_oid = git(&upstream_dir, &["rev-parse", "refs/tags/v1.0.0"]);
    let upstream_head_commit_oid = git(&upstream_dir, &["rev-parse", "HEAD"]);
    assert_ne!(
        upstream_annotated_tag_oid, upstream_head_commit_oid,
        "annotated tag OID must differ from underlying commit OID"
    );

    let upstream_engine = GitEngine::new(&upstream_dir);
    let mirror_engine = GitEngine::new(&mirror_dir);

    // 1. Reject unsafe remote URLs (flag injection, ext:: helper, embedded credentials, missing dir)
    for bad_url in [
        "",
        "--upload-pack=/bin/sh",
        "ext::sh -c touch% /tmp/pwn",
        "https://user:ghp_secret@github.com/acme/repo.git",
        "/nonexistent/path/that/does/not/exist",
    ] {
        assert!(
            matches!(
                mirror_engine.sync_inbound_read_only(bad_url),
                Err(GitEngineError::InvalidRemoteUrl(_))
            ),
            "bad remote URL must be rejected: {bad_url}"
        );
    }

    // 2. Initial sync into a not-yet-created mirror directory initializes a bare mirror & syncs all refs
    let first_sync = mirror_engine
        .sync_inbound_read_only(upstream_dir.to_str().expect("utf8"))
        .expect("initial inbound mirror sync");
    assert_eq!(first_sync.synced_branches_count, 2);
    assert_eq!(first_sync.synced_tags_count, 1);
    assert_eq!(first_sync.private_agent_branches_filtered, 0);
    assert!(first_sync.last_synced_commit.is_some());
    assert_eq!(
        upstream_engine
            .list_non_agent_refs()
            .expect("upstream refs"),
        mirror_engine.list_non_agent_refs().expect("mirror refs")
    );
    // P1 #1 (annotated tag): Verify via independent `git cat-file` that `refs/tags/v1.0.0` in the
    // mirror points to the annotated `tag` object (NOT peeled to `commit`) and preserves its message.
    assert_eq!(
        git(&mirror_dir, &["rev-parse", "refs/tags/v1.0.0"]),
        upstream_annotated_tag_oid,
        "annotated tag object OID must be preserved in mirror without peeling to commit"
    );
    assert_eq!(
        git(&mirror_dir, &["cat-file", "-t", "refs/tags/v1.0.0"]),
        "tag",
        "refs/tags/v1.0.0 in mirror must remain an annotated tag object"
    );
    assert!(
        git(&mirror_dir, &["cat-file", "-p", "refs/tags/v1.0.0"])
            .contains("annotated release v1.0.0"),
        "annotated tag message must be preserved in mirror"
    );

    // 3. Spawn a private on-premise agent worktree on the mirror and commit to it
    let agent_wt = materialize(&mirror_engine, "agent/claude/private-exp")
        .expect("materialize agent worktree on mirror");
    fs::write(
        agent_wt.worktree_dir.join("src/private.rs"),
        "// on-prem only\n",
    )
    .expect("write");
    git(&agent_wt.worktree_dir, &["add", "."]);
    git(
        &agent_wt.worktree_dir,
        &["commit", "-q", "-m", "private agent experiment"],
    );
    let private_agent_sha = git(&agent_wt.worktree_dir, &["rev-parse", "HEAD"]);

    // 4. Advance upstream: add new commit on `main`, fast-forward `release/1.0`, create `feat/sync-v2`, add tag `v1.1.0`
    fs::write(
        upstream_dir.join("src/lib.rs"),
        "pub fn answer() -> u32 { 42 }\npub fn synced() -> bool { true }\n",
    )
    .expect("write upstream");
    git(
        &upstream_dir,
        &["commit", "-q", "-am", "feat: upstream v1.1"],
    );
    git(&upstream_dir, &["branch", "-f", "release/1.0", "HEAD"]);
    git(&upstream_dir, &["branch", "feat/sync-v2"]);
    git(&upstream_dir, &["tag", "v1.1.0"]);
    // Also create a branch under `agent/` in upstream to verify upstream cannot overwrite on-prem `agent/*`
    git(
        &upstream_dir,
        &["branch", "agent/claude/private-exp", "HEAD"],
    );

    // 5. Re-run `sync_inbound_read_only` and verify Gate 7.4 ref equality + private agent preservation
    let second_sync = mirror_engine
        .sync_inbound_read_only(upstream_dir.to_str().expect("utf8"))
        .expect("second inbound mirror sync");
    assert_eq!(second_sync.private_agent_branches_filtered, 1);
    assert_eq!(second_sync.synced_branches_count, 3);
    assert_eq!(second_sync.synced_tags_count, 1);
    assert_eq!(
        upstream_engine
            .list_non_agent_refs()
            .expect("upstream refs"),
        mirror_engine.list_non_agent_refs().expect("mirror refs")
    );
    assert_eq!(
        git(
            &mirror_dir,
            &["rev-parse", "refs/heads/agent/claude/private-exp"]
        ),
        private_agent_sha,
        "local private agent/* branch must never be overwritten or pruned by upstream sync"
    );
    // P2 #1: Verify `^refs/heads/agent/*` negative refspec prevented upstream's `agent/*` branch
    // from ever being fetched into `refs/remotes/upstream/agent/*`.
    let verify_staging = Command::new("git")
        .arg("-C")
        .arg(&mirror_dir)
        .args([
            "rev-parse",
            "--verify",
            "-q",
            "refs/remotes/upstream/agent/claude/private-exp",
        ])
        .output()
        .expect("git rev-parse staging");
    assert!(
        !verify_staging.status.success(),
        "P2 #1: ^refs/heads/agent/* must exclude upstream agent/* refs at fetch time"
    );
    GitEngine::verify_git_fsck(&mirror_dir).expect("mirror git fsck");
}

#[test]
fn regression_p0_1_and_p1_3_4_preserve_local_branches_approved_merges_and_linked_worktrees() {
    let tmp = TempDir::new().expect("tempdir");
    let upstream_dir = tmp.path().join("upstream");
    let local_dir = tmp.path().join("local-clone");

    init_repo(&upstream_dir);
    let init_sha = git(&upstream_dir, &["rev-parse", "HEAD"]);
    git(&upstream_dir, &["branch", "aaa"]);
    git(&upstream_dir, &["branch", "feature"]);
    git(&upstream_dir, &["branch", "linked-branch"]);
    git(&upstream_dir, &["tag", "v1.0.0"]);

    git(
        tmp.path(),
        &[
            "clone",
            "-q",
            upstream_dir.to_str().expect("utf8"),
            local_dir.to_str().expect("utf8"),
        ],
    );
    git(&local_dir, &["branch", "aaa", &init_sha]);
    git(&local_dir, &["branch", "linked-branch", &init_sha]);

    git(&local_dir, &["checkout", "-q", "-b", "feature", &init_sha]);
    fs::write(local_dir.join("feature.txt"), "unpushed feature work\n").expect("write feature");
    git(&local_dir, &["add", "feature.txt"]);
    git(&local_dir, &["commit", "-q", "-m", "unpushed feature work"]);
    let feature_unpushed_sha = git(&local_dir, &["rev-parse", "HEAD"]);

    git(
        &local_dir,
        &["checkout", "-q", "-b", "my-local-wip", &init_sha],
    );
    fs::write(local_dir.join("wip.txt"), "unpushed WIP\n").expect("write wip");
    git(&local_dir, &["add", "wip.txt"]);
    git(&local_dir, &["commit", "-q", "-m", "unpushed WIP"]);
    let wip_unpushed_sha = git(&local_dir, &["rev-parse", "HEAD"]);

    git(&local_dir, &["checkout", "-q", "aaa"]);
    fs::write(local_dir.join("aaa-local.txt"), "local aaa\n").expect("write aaa local");
    git(&local_dir, &["add", "aaa-local.txt"]);
    git(&local_dir, &["commit", "-q", "-m", "local aaa commit"]);
    let aaa_local_sha = git(&local_dir, &["rev-parse", "HEAD"]);

    git(&local_dir, &["checkout", "-q", "main"]);

    let local_engine = GitEngine::new(&local_dir);
    let sync1 = local_engine
        .sync_inbound_read_only(upstream_dir.to_str().expect("utf8"))
        .expect("sync on local clone must preserve ahead and local-only branches");

    assert_eq!(
        git(&local_dir, &["rev-parse", "refs/heads/feature"]),
        feature_unpushed_sha,
        "P0 #1: unpushed commit on `feature` must NOT be overwritten by upstream sync"
    );
    assert_eq!(
        git(&local_dir, &["rev-parse", "refs/heads/my-local-wip"]),
        wip_unpushed_sha,
        "P0 #1: local-only branch `my-local-wip` must NOT be deleted by upstream sync"
    );
    assert!(
        sync1
            .local_only_branches
            .iter()
            .any(|b| b == "my-local-wip"),
        "P2 #2: local-only branch `my-local-wip` must be reported in local_only_branches: {:?}",
        sync1.local_only_branches
    );

    let agent_wt =
        materialize(&local_engine, "agent/claude/approve-kept").expect("materialize agent");
    fs::write(agent_wt.worktree_dir.join("approved.rs"), "// approved\n").expect("write approved");
    git(&agent_wt.worktree_dir, &["add", "approved.rs"]);
    git(
        &agent_wt.worktree_dir,
        &["commit", "-q", "-m", "feat: approved agent work"],
    );
    let merged_sha = local_engine
        .merge_agent_branch("agent/claude/approve-kept", "main")
        .expect("merge_agent_branch");

    local_engine
        .sync_inbound_read_only(upstream_dir.to_str().expect("utf8"))
        .expect("sync after approve");
    assert_eq!(
        git(&local_dir, &["rev-parse", "refs/heads/main"]),
        merged_sha.0,
        "P1 #3: approved merge on `main` must NOT be overwritten by sync"
    );

    let external_engine = GitEngine::new(&local_dir);
    let held_lock = external_engine
        .lock_worktree_service()
        .expect("acquire external lock");
    let busy_err = local_engine
        .sync_inbound_read_only(upstream_dir.to_str().expect("utf8"))
        .expect_err("sync while another service holds lock must fail");
    assert!(
        matches!(busy_err, GitEngineError::WorktreeServiceBusy(_)),
        "expected WorktreeServiceBusy, got: {busy_err:?}"
    );
    drop(held_lock);

    let linked_wt_dir = tmp.path().join("linked-wt");
    git(
        &local_dir,
        &[
            "worktree",
            "add",
            "-q",
            linked_wt_dir.to_str().expect("utf8"),
            "linked-branch",
        ],
    );
    git(&upstream_dir, &["checkout", "-q", "linked-branch"]);
    fs::write(upstream_dir.join("upstream-linked.txt"), "advance\n").expect("write");
    git(&upstream_dir, &["add", "upstream-linked.txt"]);
    git(&upstream_dir, &["commit", "-q", "-m", "advance linked"]);

    git(&upstream_dir, &["checkout", "-q", "aaa"]);
    fs::write(upstream_dir.join("aaa-upstream.txt"), "upstream force\n").expect("write");
    git(&upstream_dir, &["add", "aaa-upstream.txt"]);
    git(&upstream_dir, &["commit", "-q", "-m", "upstream force aaa"]);
    let zzz_target_sha = git(&upstream_dir, &["rev-parse", "HEAD"]);
    git(&upstream_dir, &["branch", "zzz", &zzz_target_sha]);
    git(&upstream_dir, &["tag", "-f", "v1.0.0", &zzz_target_sha]);
    git(&upstream_dir, &["checkout", "-q", "main"]);

    let partial_sync = local_engine
        .sync_inbound_read_only(upstream_dir.to_str().expect("utf8"))
        .expect("P1 #1: diverging aaa and linked-branch must be skipped without aborting zzz");

    assert_eq!(
        git(&local_dir, &["rev-parse", "refs/heads/aaa"]),
        aaa_local_sha,
        "diverged local branch `aaa` must remain untouched"
    );
    assert_eq!(
        git(&local_dir, &["rev-parse", "refs/heads/linked-branch"]),
        init_sha,
        "branch checked out in linked worktree must remain untouched"
    );
    assert_eq!(
        git(&local_dir, &["rev-parse", "refs/heads/zzz"]),
        zzz_target_sha,
        "P1 #1: branch `zzz` alphabetically after `aaa` must still be created!"
    );
    assert!(
        partial_sync
            .skipped_branches
            .iter()
            .any(|(b, reason)| b == "aaa" && reason.contains("non-fast-forward")),
        "expected `aaa` in skipped_branches: {:?}",
        partial_sync.skipped_branches
    );
    assert!(
        partial_sync
            .skipped_branches
            .iter()
            .any(|(b, reason)| b == "linked-branch" && reason.contains("linked worktree")),
        "expected `linked-branch` in skipped_branches: {:?}",
        partial_sync.skipped_branches
    );
    assert!(
        partial_sync.skipped_tags.iter().any(|(t, _)| t == "v1.0.0"),
        "P2 #2: moved tag `v1.0.0` must be reported in skipped_tags: {:?}",
        partial_sync.skipped_tags
    );

    let broken_dir = tmp.path().join("broken-non-repo");
    fs::create_dir_all(&broken_dir).expect("mkdir broken");
    fs::write(broken_dir.join("important.txt"), "do not init bare here\n").expect("write");
    let broken_engine = GitEngine::new(&broken_dir);
    let open_err = broken_engine
        .sync_inbound_read_only(upstream_dir.to_str().expect("utf8"))
        .expect_err("non-empty non-repo dir must fail OpenRepo without git init --bare");
    assert!(
        matches!(open_err, GitEngineError::OpenRepo { .. }),
        "expected OpenRepo error, got: {open_err:?}"
    );
    assert!(
        !broken_dir.join("HEAD").exists(),
        "must not scatter bare repo files into non-empty non-repo directory"
    );
}

#[test]
fn ancestry_handles_clock_skew_and_distinguishes_indeterminate_from_diverged() {
    use std::io::Write;
    use std::process::Stdio;

    let tmp = TempDir::new().expect("tempdir");
    let repo_dir = tmp.path().join("skew-repo");
    fs::create_dir_all(&repo_dir).expect("mkdir");
    git(&repo_dir, &["init", "--bare", "-q", "-b", "main"]);

    // Construct a commit chain where child commit timestamps are OLDER than their parents
    // (`parent.time > child.time`):
    // c1 (ts = 2_000_000_000) <- c2 (ts = 1_500_000_000) <- c3 (ts = 1_000_000_000)
    // and a diverged branch `div` from c1 with ts = 1_000_000_000.
    let mut fi = Command::new("git")
        .arg("-C")
        .arg(&repo_dir)
        .args(["fast-import", "--quiet"])
        .stdin(Stdio::piped())
        .spawn()
        .expect("spawn fast-import");
    {
        let stdin = fi.stdin.as_mut().expect("stdin");
        writeln!(stdin, "blob\nmark :1\ndata 6\nhello\n").expect("blob");
        writeln!(
            stdin,
            "commit refs/heads/base\nmark :2\ncommitter Skew <s@c> 2000000000 +0000\ndata 3\nc1\nM 100644 :1 README.md\n"
        )
        .expect("c1");
        writeln!(
            stdin,
            "commit refs/heads/mid\nmark :3\ncommitter Skew <s@c> 1500000000 +0000\ndata 3\nc2\nfrom :2\n"
        )
        .expect("c2");
        writeln!(
            stdin,
            "commit refs/heads/tip\nmark :4\ncommitter Skew <s@c> 1000000000 +0000\ndata 3\nc3\nfrom :3\n"
        )
        .expect("c3");
        writeln!(
            stdin,
            "commit refs/heads/div\nmark :5\ncommitter Skew <s@c> 1000000000 +0000\ndata 4\ndiv\nfrom :2\n"
        )
        .expect("div");
    }
    assert!(fi.wait().expect("wait fast-import").success());

    let c1: gix::ObjectId = git(&repo_dir, &["rev-parse", "refs/heads/base"])
        .parse()
        .expect("oid");
    let c3: gix::ObjectId = git(&repo_dir, &["rev-parse", "refs/heads/tip"])
        .parse()
        .expect("oid");
    let div: gix::ObjectId = git(&repo_dir, &["rev-parse", "refs/heads/div"])
        .parse()
        .expect("oid");
    let missing: gix::ObjectId = "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef"
        .parse()
        .expect("oid");

    // 1. Test WITHOUT commit-graph (pure 2-color DAG walk under severe clock skew)
    let repo = gix::open(&repo_dir).expect("open");
    assert_eq!(
        classify_branch_relation(&repo, c1, c3),
        BranchRelation::FastForward,
        "clock-skewed descendant c3 must still be recognized as FastForward from c1"
    );
    assert_eq!(
        classify_branch_relation(&repo, c3, c1),
        BranchRelation::LocalAhead,
        "clock-skewed descendant c3 must still be recognized as LocalAhead of c1"
    );
    assert_eq!(
        classify_branch_relation(&repo, c3, div),
        BranchRelation::Diverged,
        "diverged branches must be classified as Diverged"
    );
    assert!(
        matches!(
            classify_branch_relation(&repo, c1, missing),
            BranchRelation::Indeterminate(_)
        ),
        "missing/unreadable commit object must return Indeterminate, never Diverged"
    );

    // 2. Write commit-graph and verify topological generation numbers also classify accurately
    git(&repo_dir, &["commit-graph", "write", "--reachable"]);
    let repo_cg = gix::open(&repo_dir).expect("open with commit-graph");
    assert_eq!(
        classify_branch_relation(&repo_cg, c1, c3),
        BranchRelation::FastForward
    );
    assert_eq!(
        classify_branch_relation(&repo_cg, c3, c1),
        BranchRelation::LocalAhead
    );
    assert_eq!(
        classify_branch_relation(&repo_cg, c3, div),
        BranchRelation::Diverged
    );
}
