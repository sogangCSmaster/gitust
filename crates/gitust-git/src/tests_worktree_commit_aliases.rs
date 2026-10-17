use super::*;
use gitust_core::CommitSha;

fn setup() -> (TempDir, GitEngine, MaterializedWorktree) {
    let temp = TempDir::new().unwrap();
    init_repo(temp.path());
    fs::write(temp.path().join("a.txt"), "original\n").unwrap();
    fs::write(temp.path().join("é.txt"), "unicode original\n").unwrap();
    git(temp.path(), &["add", "a.txt", "é.txt"]);
    git(temp.path(), &["commit", "-qm", "alias fixture"]);
    let engine = GitEngine::new(temp.path());
    let task = materialize(&engine, "agent/test/path-alias").unwrap();
    (temp, engine, task)
}

fn commit(
    engine: &GitEngine,
    task: &MaterializedWorktree,
    paths: &[&str],
) -> Result<crate::WorktreeCommit, GitEngineError> {
    let expected = CommitSha(git(&task.worktree_dir, &["rev-parse", "HEAD"]));
    let prepared = engine.prepare_worktree_commit(
        task.worktree.id,
        &task.worktree.branch,
        &expected,
        &paths.iter().map(|path| (*path).into()).collect::<Vec<_>>(),
        "Explicit literal paths",
        "test@example.invalid",
    )?;
    engine.finish_worktree_commit(prepared)
}

#[test]
fn host_commit_rejects_case_alias_and_case_only_rename_on_insensitive_filesystems() {
    let (_temp, engine, task) = setup();
    let directory = &task.worktree_dir;
    fs::write(directory.join("a.txt"), "changed\n").unwrap();
    if !directory.join("A.txt").exists() {
        // A genuine second path is legal on this filesystem; exercise it below instead.
        fs::write(directory.join("A.txt"), "separate\n").unwrap();
        commit(&engine, &task, &["a.txt", "A.txt"]).unwrap();
        assert_eq!(git(directory, &["show", "HEAD:a.txt"]), "changed");
        assert_eq!(git(directory, &["show", "HEAD:A.txt"]), "separate");
        assert!(git(directory, &["status", "--porcelain"]).is_empty());
        return;
    }
    let original = git(directory, &["rev-parse", "HEAD"]);
    assert!(commit(&engine, &task, &["A.txt"]).is_err());
    assert_eq!(git(directory, &["rev-parse", "HEAD"]), original);
    fs::rename(directory.join("a.txt"), directory.join("A.txt")).unwrap();
    assert!(commit(&engine, &task, &["A.txt"]).is_err());
    assert!(commit(&engine, &task, &["a.txt", "A.txt"]).is_err());
    assert_eq!(git(directory, &["rev-parse", "HEAD"]), original);
    fs::rename(directory.join("A.txt"), directory.join("a.txt")).unwrap();
    commit(&engine, &task, &["a.txt"]).unwrap();
    assert!(git(directory, &["status", "--porcelain"]).is_empty());
    GitEngine::verify_git_fsck(directory).unwrap();
}

#[test]
fn host_commit_detects_unicode_aliases_and_preserves_real_unicode_paths() {
    let (_temp, engine, task) = setup();
    let directory = &task.worktree_dir;
    fs::write(directory.join("é.txt"), "unicode changed\n").unwrap();
    let decomposed = "e\u{301}.txt";
    if directory.join(decomposed).exists() {
        let original = git(directory, &["rev-parse", "HEAD"]);
        assert!(commit(&engine, &task, &[decomposed]).is_err());
        assert_eq!(git(directory, &["rev-parse", "HEAD"]), original);
        commit(&engine, &task, &["é.txt"]).unwrap();
    } else {
        fs::write(directory.join(decomposed), "genuinely separate\n").unwrap();
        commit(&engine, &task, &["é.txt", decomposed]).unwrap();
    }
    assert!(git(directory, &["status", "--porcelain"]).is_empty());
    GitEngine::verify_git_fsck(directory).unwrap();
}

#[cfg(unix)]
#[test]
fn host_commit_allows_two_actual_case_distinct_hardlink_names() {
    let (_temp, engine, task) = setup();
    let directory = &task.worktree_dir;
    if directory.join("A.txt").exists() {
        return;
    }
    fs::hard_link(directory.join("a.txt"), directory.join("A.txt")).unwrap();
    fs::write(directory.join("a.txt"), "same inode two real names\n").unwrap();
    commit(&engine, &task, &["a.txt", "A.txt"]).unwrap();
    assert!(git(directory, &["status", "--porcelain"]).is_empty());
    GitEngine::verify_git_fsck(directory).unwrap();
}
