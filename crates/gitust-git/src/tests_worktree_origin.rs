use super::*;

fn release_worktree(path: &Path) -> (GitEngine, MaterializedWorktree) {
    init_repo(path);
    git(path, &["checkout", "-b", "release/stable"]);
    fs::write(path.join("release-only"), "release content").unwrap();
    git(path, &["add", "."]);
    git(path, &["commit", "-m", "release"]);
    git(path, &["checkout", "main"]);
    let engine = GitEngine::new(path);
    let m = engine
        .materialize_agent_worktree(
            WorktreeId::new(),
            RepoId::new(),
            AgentKind::CodexSwarm,
            "agent/codex/release",
            "release/stable",
            "review",
        )
        .unwrap();
    git(&m.worktree_dir, &["commit", "--allow-empty", "-m", "agent"]);
    (engine, m)
}

#[test]
fn reflog_expiry_preserves_the_creation_target_and_cannot_merge_release_into_main() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("repo");
    let (engine, m) = release_worktree(&path);
    let main = git(&path, &["rev-parse", "main"]);
    git(&path, &["reflog", "expire", "--expire=now", "--all"]);
    let recovered = engine
        .recover_agent_worktrees(RepoId::new())
        .unwrap()
        .remove(0);
    assert_eq!(recovered.worktree.base_branch, "release/stable");
    assert_eq!(recovered.worktree.base_commit, m.worktree.base_commit);
    let checked = engine
        .clean_worktree_commit(m.worktree.id, &m.worktree.branch)
        .unwrap();
    assert!(engine
        .approve_verified_worktree(m.worktree.id, &m.worktree.branch, "main", &checked)
        .is_err());
    engine
        .approve_verified_worktree(
            m.worktree.id,
            &m.worktree.branch,
            "release/stable",
            &checked,
        )
        .unwrap();
    assert_eq!(git(&path, &["rev-parse", "main"]), main);
    assert!(!path.join("release-only").exists());
    GitEngine::verify_git_fsck(&path).unwrap();
    GitEngine::verify_git_fsck(&m.worktree_dir).unwrap();
}

#[test]
fn legacy_valid_reflog_is_migrated_before_git_maintenance_removes_it() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("repo");
    let (engine, m) = release_worktree(&path);
    let origin = m.worktree_admin_dir.join("gitust-origin.json");
    if origin.exists() {
        fs::remove_file(&origin).unwrap();
    }
    engine.recover_agent_worktrees(RepoId::new()).unwrap();
    assert!(
        origin.is_file(),
        "legacy creation evidence must become durable"
    );
    git(&path, &["reflog", "expire", "--expire=now", "--all"]);
    let recovered = engine.recover_agent_worktrees(RepoId::new()).unwrap();
    assert_eq!(recovered[0].worktree.base_branch, "release/stable");
}

#[test]
fn unknown_legacy_base_never_defaults_to_main_and_cannot_be_approved() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("repo");
    let (engine, m) = release_worktree(&path);
    let origin = m.worktree_admin_dir.join("gitust-origin.json");
    if origin.exists() {
        fs::remove_file(&origin).unwrap();
    }
    git(&path, &["reflog", "expire", "--expire=now", "--all"]);
    let recovered = engine.recover_agent_worktrees(RepoId::new()).unwrap();
    assert!(recovered[0].worktree.base_branch.is_empty());
    let before = git(&path, &["rev-parse", "main"]);
    assert!(engine
        .approve_managed_worktree(m.worktree.id, &m.worktree.branch, "main")
        .is_err());
    assert_eq!(git(&path, &["rev-parse", "main"]), before);
    assert!(engine
        .load_worktree_approval(m.worktree.id)
        .unwrap()
        .is_none());
}

#[test]
fn corrupt_origin_is_not_silently_replaced_with_reflog_or_main() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("repo");
    let (engine, m) = release_worktree(&path);
    fs::write(m.worktree_admin_dir.join("gitust-origin.json"), "broken").unwrap();
    assert!(engine.recover_agent_worktrees(RepoId::new()).is_err());
}

#[test]
fn origin_is_bounded_identity_checked_immutable_and_rejects_symlinks() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("repo");
    let (engine, m) = release_worktree(&path);
    let origin = m.worktree_admin_dir.join("gitust-origin.json");
    let saved = fs::read(&origin).unwrap();
    assert!(crate::worktree_origin::persist(
        &m.worktree_admin_dir,
        m.worktree.id,
        &m.worktree.branch,
        "main",
        &m.worktree.base_commit,
        None
    )
    .is_err());
    assert_eq!(fs::read(&origin).unwrap(), saved);
    for (field, value) in [
        ("version", serde_json::json!(999)),
        ("worktree_id", serde_json::json!(WorktreeId::new())),
        ("branch", serde_json::json!("agent/codex/other")),
        ("base_branch", serde_json::json!("../main")),
        ("base_commit", serde_json::json!("invalid")),
    ] {
        let mut json: serde_json::Value = serde_json::from_slice(&saved).unwrap();
        json[field] = value;
        fs::write(&origin, serde_json::to_vec(&json).unwrap()).unwrap();
        assert!(
            engine.recover_agent_worktrees(RepoId::new()).is_err(),
            "{field}"
        );
    }
    fs::write(&origin, vec![b'x'; 8193]).unwrap();
    assert!(engine.recover_agent_worktrees(RepoId::new()).is_err());
    #[cfg(unix)]
    {
        fs::remove_file(&origin).unwrap();
        let outside = tmp.path().join("outside");
        fs::write(&outside, &saved).unwrap();
        std::os::unix::fs::symlink(&outside, &origin).unwrap();
        assert!(engine.recover_agent_worktrees(RepoId::new()).is_err());
        assert_eq!(fs::read(&outside).unwrap(), saved);
    }
}

#[test]
fn legacy_first_entry_survives_a_large_reflog_and_partial_staging_write() {
    use std::io::Write;
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("repo");
    let (engine, m) = release_worktree(&path);
    let origin = m.worktree_admin_dir.join("gitust-origin.json");
    fs::remove_file(origin).unwrap();
    let log = m
        .common_git_dir
        .join("logs/refs/heads")
        .join(&m.worktree.branch);
    let mut file = fs::OpenOptions::new().append(true).open(&log).unwrap();
    file.write_all(&vec![b'\n'; 16384]).unwrap();
    fs::write(m.worktree_admin_dir.join("gitust-origin.tmp"), "partial").unwrap();
    let recovered = engine.recover_agent_worktrees(RepoId::new()).unwrap();
    assert_eq!(recovered[0].worktree.base_branch, "release/stable");
    assert!(!m.worktree_admin_dir.join("gitust-origin.tmp").exists());
}
