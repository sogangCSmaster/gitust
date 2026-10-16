use super::*;

#[test]
fn merging_into_main_refuses_index_flags_that_hide_local_work() {
    for flag in ["--assume-unchanged", "--skip-worktree"] {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("repo");
        init_repo(&path);
        let engine = GitEngine::new(&path);
        let m = materialize(&engine, "agent/test/main-index").unwrap();
        fs::write(m.worktree_dir.join("src/lib.rs"), "approved update\n").unwrap();
        git(&m.worktree_dir, &["commit", "-am", "approved"]);
        git(&path, &["update-index", flag, "src/lib.rs"]);
        fs::write(path.join("src/lib.rs"), "local work must survive\n").unwrap();
        assert!(engine
            .approve_managed_worktree(m.worktree.id, &m.worktree.branch, "main")
            .is_err());
        assert_eq!(
            fs::read_to_string(path.join("src/lib.rs")).unwrap(),
            "local work must survive\n"
        );
    }
}

#[test]
fn durable_approval_recovers_exact_sha_and_publication_destination() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("repo");
    init_repo(&path);
    let engine = GitEngine::new(&path);
    let m = materialize(&engine, "agent/test/durable").unwrap();
    git(
        &m.worktree_dir,
        &["commit", "--allow-empty", "-m", "reviewed"],
    );
    let approved = engine
        .approve_managed_worktree(m.worktree.id, &m.worktree.branch, "main")
        .unwrap();
    git(&path, &["commit", "--allow-empty", "-m", "later change"]);
    drop(engine);
    let restarted = GitEngine::new(&path);
    let recovered = restarted.recover_agent_worktrees(RepoId::new()).unwrap();
    assert_eq!(recovered[0].worktree.status, WorktreeStatus::Merged);
    assert_eq!(recovered[0].worktree.base_commit, approved);
    let remote = temp.path().join("remote.git");
    fs::create_dir(&remote).unwrap();
    git(&remote, &["init", "--bare", "-b", "main"]);
    restarted
        .publish_managed_worktree(m.worktree.id, remote.to_str().unwrap())
        .unwrap();
    assert_eq!(
        git(&remote, &["show-ref"]),
        format!("{} refs/heads/main", approved.0)
    );
    let receipt = GitEngine::new(&path)
        .load_worktree_approval(m.worktree.id)
        .unwrap()
        .unwrap();
    assert_eq!(receipt.approved_commit, approved);
    assert!(receipt
        .published_to(&GitEngine::validate_mirror_remote_url(remote.to_str().unwrap()).unwrap()));
    assert!(!receipt.published_to("git@github.com:other/repo.git"));
    GitEngine::verify_git_fsck(&remote).unwrap();
    GitEngine::verify_git_fsck(&path).unwrap();
}

#[test]
fn invalid_approval_records_fail_closed_on_recovery() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("repo");
    init_repo(&path);
    let engine = GitEngine::new(&path);
    let m = materialize(&engine, "agent/test/durable").unwrap();
    git(
        &m.worktree_dir,
        &["commit", "--allow-empty", "-m", "reviewed"],
    );
    engine
        .approve_managed_worktree(m.worktree.id, &m.worktree.branch, "main")
        .unwrap();
    let record = path
        .join(".git/gitust-approvals")
        .join(format!("{}.json", m.worktree.id.0));
    let valid = fs::read(&record).unwrap();
    for invalid in [b"broken".to_vec(), vec![b'x'; 8193]] {
        fs::write(&record, invalid).unwrap();
        assert!(engine.recover_agent_worktrees(RepoId::new()).is_err());
    }
    let mut json: serde_json::Value = serde_json::from_slice(&valid).unwrap();
    json["version"] = 999.into();
    fs::write(&record, serde_json::to_vec(&json).unwrap()).unwrap();
    assert!(engine.recover_agent_worktrees(RepoId::new()).is_err());
    fs::write(&record, &valid).unwrap();
    git(&path, &["reset", "--hard", "HEAD~1"]);
    assert!(engine.recover_agent_worktrees(RepoId::new()).is_err());
}

#[cfg(unix)]
#[test]
fn approval_state_refuses_symlink_and_failed_storage_cannot_report_success() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("repo");
    init_repo(&path);
    let engine = GitEngine::new(&path);
    let m = materialize(&engine, "agent/test/durable").unwrap();
    git(
        &m.worktree_dir,
        &["commit", "--allow-empty", "-m", "reviewed"],
    );
    let outside = temp.path().join("outside");
    fs::create_dir(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, path.join(".git/gitust-approvals")).unwrap();
    assert!(engine
        .approve_managed_worktree(m.worktree.id, &m.worktree.branch, "main")
        .is_err());
    assert_eq!(fs::read_dir(outside).unwrap().count(), 0);
}

#[cfg(unix)]
#[test]
fn remote_success_followed_by_record_write_failure_can_retry_exact_commit() {
    use std::os::unix::fs::PermissionsExt;
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("repo");
    init_repo(&path);
    let engine = GitEngine::new(&path);
    let m = materialize(&engine, "agent/test/retry").unwrap();
    git(
        &m.worktree_dir,
        &["commit", "--allow-empty", "-m", "reviewed"],
    );
    let approved = engine
        .approve_managed_worktree(m.worktree.id, &m.worktree.branch, "main")
        .unwrap();
    let remote = temp.path().join("remote.git");
    fs::create_dir(&remote).unwrap();
    git(&remote, &["init", "--bare", "-b", "main"]);
    let directory = path.join(".git/gitust-approvals");
    let record = directory.join(format!("{}.json", m.worktree.id.0));
    assert_eq!(
        fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(&record).unwrap().permissions().mode() & 0o777,
        0o600
    );
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o500)).unwrap();
    let result = engine.publish_managed_worktree(m.worktree.id, remote.to_str().unwrap());
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(
        result.is_err(),
        "publication must not report success when its receipt cannot be saved"
    );
    assert_eq!(git(&remote, &["rev-parse", "main"]), approved.0);
    assert!(!engine
        .load_worktree_approval(m.worktree.id)
        .unwrap()
        .unwrap()
        .published_to(&GitEngine::validate_mirror_remote_url(remote.to_str().unwrap()).unwrap()));
    engine
        .publish_managed_worktree(m.worktree.id, remote.to_str().unwrap())
        .unwrap();
    let target = temp.path().join("outside.json");
    fs::write(&target, fs::read(&record).unwrap()).unwrap();
    fs::remove_file(&record).unwrap();
    std::os::unix::fs::symlink(&target, &record).unwrap();
    assert!(engine.load_worktree_approval(m.worktree.id).is_err());
}
