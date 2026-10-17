use super::*;

fn create_replacement(repo: &Path, packed: bool) -> String {
    let original = git(repo, &["rev-parse", "HEAD:src/lib.rs"]);
    let mut command = crate::git_command(repo);
    command.args(["hash-object", "-w", "--stdin"]);
    let (replacement, _) = crate::process::run_capped_with_input(
        command,
        std::time::Duration::from_secs(10),
        128,
        Some(b"replacement content must never be approved as the original SHA\n".to_vec()),
    )
    .unwrap();
    let replacement = std::str::from_utf8(&replacement).unwrap().trim();
    git(repo, &["replace", &original, replacement]);
    if packed {
        git(repo, &["pack-refs", "--all", "--prune"]);
    }
    // Independent Git oracle proves the physical object and replacement view differ.
    assert_eq!(
        git(repo, &["cat-file", "blob", &original]),
        "replacement content must never be approved as the original SHA"
    );
    let physical = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["cat-file", "blob", &original])
        .env("GIT_NO_REPLACE_OBJECTS", "1")
        .output()
        .unwrap();
    assert_eq!(physical.stdout, b"pub fn answer() -> u32 { 42 }\n");
    original
}

fn replacement_error<T>(result: Result<T, GitEngineError>) -> bool {
    matches!(result, Err(GitEngineError::VerificationInvalid(message)) if message.contains("refs/replace"))
}

#[test]
fn managed_workflows_reject_loose_and_packed_replacements_and_resume_after_removal() {
    for packed in [false, true] {
        let temporary = TempDir::new().unwrap();
        let repo = temporary.path().join("repo");
        init_repo(&repo);
        let engine = GitEngine::new(&repo);
        let m = materialize(&engine, "agent/test/replacements").unwrap();
        let original = create_replacement(&repo, packed);
        let index = fs::read(m.worktree_admin_dir.join("index")).unwrap();
        let guarded = replacement_error(engine.ensure_config_has_no_credentials());
        let verified =
            replacement_error(engine.clean_worktree_commit(m.worktree.id, &m.worktree.branch));
        fs::write(m.worktree_dir.join("new.txt"), "new\n").unwrap();
        let prepared = replacement_error(engine.prepare_worktree_commit(
            m.worktree.id,
            &m.worktree.branch,
            &m.worktree.base_commit,
            &["new.txt".into()],
            "explicit source",
            "operator@example.test",
        ));
        let approved = replacement_error(engine.approve_verified_worktree(
            m.worktree.id,
            &m.worktree.branch,
            "main",
            &m.worktree.base_commit,
        ));
        assert_eq!(
            git(&m.worktree_dir, &["rev-parse", "HEAD"]),
            m.worktree.base_commit.0
        );
        assert_eq!(fs::read(m.worktree_admin_dir.join("index")).unwrap(), index);
        assert!(!m.worktree_admin_dir.join("index.lock").exists());
        assert!(guarded && verified && prepared && approved, "replacement policy was not enforced: guard={guarded} verification={verified} prepare={prepared} approval={approved}");
        git(&repo, &["replace", "-d", &original]);
        engine
            .ensure_worktree_config_has_no_credentials(&m.worktree_dir)
            .unwrap();
        let prepared = engine
            .prepare_worktree_commit(
                m.worktree.id,
                &m.worktree.branch,
                &m.worktree.base_commit,
                &["new.txt".into()],
                "after replacement removal",
                "operator@example.test",
            )
            .unwrap();
        let committed = engine.finish_worktree_commit(prepared).unwrap();
        assert_eq!(
            engine
                .clean_worktree_commit(m.worktree.id, &m.worktree.branch)
                .unwrap(),
            committed.commit
        );
        engine
            .approve_verified_worktree(m.worktree.id, &m.worktree.branch, "main", &committed.commit)
            .unwrap();
        let original = create_replacement(&repo, packed);
        let remote = temporary.path().join("remote.git");
        fs::create_dir(&remote).unwrap();
        git(&remote, &["init", "--bare", "--quiet", "-b", "main"]);
        assert!(replacement_error(engine.push_approved_commit(
            remote.to_str().unwrap(),
            "main",
            &committed.commit
        )));
        assert_eq!(git(&remote, &["for-each-ref", "refs/heads/"]), "");
        git(&repo, &["replace", "-d", &original]);
        engine
            .publish_managed_worktree(m.worktree.id, remote.to_str().unwrap())
            .unwrap();
        assert_eq!(git(&remote, &["rev-parse", "main"]), committed.commit.0);
        GitEngine::verify_git_fsck(&repo).unwrap();
        GitEngine::verify_git_fsck(&remote).unwrap();
    }
}

#[test]
fn malformed_replacement_references_fail_closed() {
    let temporary = TempDir::new().unwrap();
    let repo = temporary.path().join("repo");
    init_repo(&repo);
    let engine = GitEngine::new(&repo);
    let m = materialize(&engine, "agent/test/broken-replacement").unwrap();
    let original = git(&repo, &["rev-parse", "HEAD:src/lib.rs"]);
    let directory = repo.join(".git/refs/replace");
    fs::create_dir(&directory).unwrap();
    fs::write(directory.join(&original), "not a valid reference\n").unwrap();
    assert!(engine.ensure_config_has_no_credentials().is_err());
    assert!(engine
        .clean_worktree_commit(m.worktree.id, &m.worktree.branch)
        .is_err());
    fs::remove_file(directory.join(original)).unwrap();
    engine.ensure_config_has_no_credentials().unwrap();
}

#[test]
fn cached_replacement_view_requires_restart_after_ref_removal() {
    let temporary = TempDir::new().unwrap();
    let repo = temporary.path().join("repo");
    init_repo(&repo);
    let original = create_replacement(&repo, false);
    git(&repo, &["config", "core.useReplaceRefs", "false"]);
    let engine = GitEngine::new(&repo);
    let cached = engine.acquire_repo().unwrap();
    assert_eq!(cached.objects.store_ref().replacements().count(), 1);
    let object = cached
        .find_blob(gix::ObjectId::from_hex(original.as_bytes()).unwrap())
        .unwrap();
    assert!(object.data.starts_with(b"replacement content"));
    drop(object);
    git(&repo, &["replace", "-d", &original]);
    assert!(replacement_error(engine.ensure_config_has_no_credentials()));
    drop(cached);
    drop(engine);
    let restarted = GitEngine::new(&repo);
    restarted.ensure_config_has_no_credentials().unwrap();
    let m = materialize(&restarted, "agent/test/restarted-view").unwrap();
    assert_eq!(
        restarted
            .clean_worktree_commit(m.worktree.id, &m.worktree.branch)
            .unwrap(),
        m.worktree.base_commit
    );
}
