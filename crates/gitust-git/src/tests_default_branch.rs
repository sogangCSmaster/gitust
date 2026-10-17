use super::{git, init_repo};
use crate::{CommitSha, GitEngine, GitEngineError};
use gitust_core::{AgentKind, RepoId, WorktreeId};
use std::fs;
fn fixture() -> (tempfile::TempDir, GitEngine, CommitSha) {
    let temp = tempfile::tempdir().unwrap();
    let seed = temp.path().join("seed");
    init_repo(&seed);
    git(&seed, &["checkout", "-qb", "topic"]);
    git(&seed, &["commit", "--allow-empty", "-qm", "topic"]);
    let tip = CommitSha(git(&seed, &["rev-parse", "HEAD"]));
    let bare = temp.path().join("bare");
    git(
        temp.path(),
        &[
            "clone",
            "-q",
            "--bare",
            seed.to_str().unwrap(),
            bare.to_str().unwrap(),
        ],
    );
    git(&bare, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    (temp, GitEngine::new(bare), tip)
}
#[test]
fn changing_bare_default_preserves_refs_and_old_worktree_merge_target() {
    let (_temp, engine, tip) = fixture();
    let _lease = engine.lock_worktree_service().unwrap();
    let wt = engine
        .materialize_agent_worktree(
            WorktreeId::new(),
            RepoId::new(),
            AgentKind::CodexSwarm,
            "agent/old",
            "main",
            "old target",
        )
        .unwrap();
    let origin = fs::read(wt.worktree_admin_dir.join("gitust-origin.json")).unwrap();
    fs::write(wt.worktree_dir.join("change.txt"), "change\n").unwrap();
    git(&wt.worktree_dir, &["add", "."]);
    git(&wt.worktree_dir, &["commit", "-qm", "old target change"]);
    let refs = git(engine.repo_path(), &["show-ref"]);
    let changed = engine
        .change_default_branch_with_gate("refs/heads/topic", "refs/heads/main", &tip, || Ok(()))
        .unwrap();
    assert_eq!(changed.head_commit, tip.clone());
    assert_eq!(
        git(engine.repo_path(), &["symbolic-ref", "HEAD"]),
        "refs/heads/topic"
    );
    assert_eq!(git(engine.repo_path(), &["show-ref"]), refs);
    assert_eq!(
        fs::read(wt.worktree_admin_dir.join("gitust-origin.json")).unwrap(),
        origin
    );
    let approved = engine
        .approve_managed_worktree(
            wt.worktree.id,
            &wt.worktree.branch,
            &wt.worktree.base_branch,
        )
        .unwrap();
    assert_eq!(git(engine.repo_path(), &["rev-parse", "main"]), approved.0);
    assert_eq!(git(engine.repo_path(), &["rev-parse", "topic"]), tip.0);
    git(engine.repo_path(), &["fsck", "--full", "--strict"]);
}
#[test]
fn unborn_default_can_select_a_committed_topic_and_clone_advertises_new_default() {
    let (temp, engine, tip) = fixture();
    git(
        engine.repo_path(),
        &["symbolic-ref", "HEAD", "refs/heads/unborn"],
    );
    assert!(engine.committed_default_branch().is_err());
    engine
        .change_default_branch_with_gate("refs/heads/topic", "refs/heads/unborn", &tip, || Ok(()))
        .unwrap();
    assert_eq!(engine.committed_default_branch().unwrap(), "topic");
    let advertised = git(
        temp.path(),
        &[
            "ls-remote",
            "--symref",
            engine.repo_path().to_str().unwrap(),
            "HEAD",
        ],
    );
    assert!(advertised.starts_with("ref: refs/heads/topic\tHEAD"));
    let client = temp.path().join("clone");
    git(
        temp.path(),
        &[
            "clone",
            "-q",
            engine.repo_path().to_str().unwrap(),
            client.to_str().unwrap(),
        ],
    );
    assert_eq!(git(&client, &["branch", "--show-current"]), "topic");
    assert_eq!(git(&client, &["rev-parse", "HEAD"]), tip.0);
    git(&client, &["fsck", "--full", "--strict"]);
}
#[test]
fn stale_targets_and_invalid_or_unsupported_states_never_mutate_head() {
    let (temp, engine, tip) = fixture();
    let before = fs::read(engine.repo_path().join("HEAD")).unwrap();
    for (target, current, sha) in [
        ("refs/heads/agent/private", "refs/heads/main", tip.clone()),
        ("refs/tags/topic", "refs/heads/main", tip.clone()),
        ("refs/heads/HEAD", "refs/heads/main", tip.clone()),
        (
            "refs/heads/refs/heads/topic",
            "refs/heads/main",
            tip.clone(),
        ),
        (
            "refs/heads/non\u{a0}breaking",
            "refs/heads/main",
            tip.clone(),
        ),
        ("refs/heads/topic", "refs/heads/stale", tip.clone()),
        (
            "refs/heads/topic",
            "refs/heads/main",
            CommitSha("a".repeat(40)),
        ),
        ("refs/heads/missing", "refs/heads/main", tip.clone()),
        (
            "refs/heads/topic",
            "refs/heads/main",
            CommitSha("0".repeat(40)),
        ),
    ] {
        assert!(engine
            .change_default_branch_with_gate(target, current, &sha, || Ok(()))
            .is_err());
        assert_eq!(fs::read(engine.repo_path().join("HEAD")).unwrap(), before);
    }
    let blob = git(engine.repo_path(), &["rev-parse", "main:src/lib.rs"]);
    fs::write(
        engine.repo_path().join("refs/heads/blob"),
        format!("{blob}\n"),
    )
    .unwrap();
    assert!(engine
        .change_default_branch_with_gate(
            "refs/heads/blob",
            "refs/heads/main",
            &CommitSha(blob),
            || Ok(())
        )
        .is_err());
    fs::remove_file(engine.repo_path().join("refs/heads/blob")).unwrap();
    git(
        engine.repo_path(),
        &["symbolic-ref", "refs/heads/alias", "refs/heads/topic"],
    );
    assert!(engine
        .change_default_branch_with_gate("refs/heads/alias", "refs/heads/main", &tip, || Ok(()))
        .is_err());
    assert!(engine
        .change_default_branch_with_gate("refs/heads/topic", "refs/heads/main", &tip, || Err(
            GitEngineError::DefaultBranchUnsupported
        ))
        .is_err());
    assert_eq!(fs::read(engine.repo_path().join("HEAD")).unwrap(), before);
    let normal = GitEngine::new(temp.path().join("seed"));
    assert!(normal
        .inspect_changeable_default_branch()
        .unwrap()
        .is_none());
    assert!(normal
        .change_default_branch_with_gate("refs/heads/topic", "refs/heads/topic", &tip, || Ok(()))
        .is_err());
    fs::write(engine.repo_path().join("HEAD"), format!("{}\n", tip.0)).unwrap();
    assert!(engine
        .inspect_changeable_default_branch()
        .unwrap()
        .is_none());
    assert!(engine.committed_default_branch().is_err());
}
#[test]
fn gate_time_target_move_and_competing_default_change_are_rejected_by_exact_checks() {
    let (_temp, engine, tip) = fixture();
    let main = git(engine.repo_path(), &["rev-parse", "main"]);
    assert!(engine
        .change_default_branch_with_gate("refs/heads/topic", "refs/heads/main", &tip, || {
            git(
                engine.repo_path(),
                &["update-ref", "refs/heads/topic", &main],
            );
            Ok(())
        })
        .is_err());
    assert_eq!(
        git(engine.repo_path(), &["symbolic-ref", "HEAD"]),
        "refs/heads/main"
    );
    git(
        engine.repo_path(),
        &["update-ref", "refs/heads/topic", &tip.0],
    );
    assert!(engine
        .change_default_branch_with_gate("refs/heads/topic", "refs/heads/main", &tip, || {
            git(
                engine.repo_path(),
                &["symbolic-ref", "HEAD", "refs/heads/other"],
            );
            Ok(())
        })
        .is_err());
    assert_eq!(
        git(engine.repo_path(), &["symbolic-ref", "HEAD"]),
        "refs/heads/other"
    );
}
#[test]
fn default_compare_reads_the_current_symbolic_value_after_head_lock_is_acquired() {
    let (_temp, engine, tip) = fixture();
    let mut writer = None;
    let result =
        engine.change_default_branch_with_gate("refs/heads/topic", "refs/heads/main", &tip, || {
            let path = engine.repo_path().to_path_buf();
            fs::write(path.join("HEAD.lock"), "ref: refs/heads/other\n").unwrap();
            writer = Some(std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(20));
                fs::rename(path.join("HEAD.lock"), path.join("HEAD")).unwrap();
            }));
            Ok(())
        });
    writer.unwrap().join().unwrap();
    assert!(
        result.is_err(),
        "a competing symbolic HEAD update must not be overwritten"
    );
    assert_eq!(
        git(engine.repo_path(), &["symbolic-ref", "HEAD"]),
        "refs/heads/other"
    );
}
