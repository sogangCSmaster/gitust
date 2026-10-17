use super::*;

fn prepare(
    engine: &GitEngine,
    m: &MaterializedWorktree,
    paths: &[&str],
) -> Result<crate::PreparedWorktreeCommit, GitEngineError> {
    engine.prepare_worktree_commit(
        m.worktree.id,
        &m.worktree.branch,
        &m.worktree.base_commit,
        &paths.iter().map(|p| (*p).to_owned()).collect::<Vec<_>>(),
        "agent change\n\nDetails stay intact.\n",
        "operator@example.test",
    )
}

#[test]
fn host_commit_records_only_explicit_files_and_keeps_unselected_edits() {
    let temp = TempDir::new().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    let engine = GitEngine::new(&repo);
    let m = materialize(&engine, "agent/test/commit").unwrap();
    fs::write(m.worktree_dir.join("src/lib.rs"), "changed\n").unwrap();
    fs::write(m.worktree_dir.join("selected [x] \n.txt"), "selected\n").unwrap();
    fs::write(m.worktree_dir.join("not-selected.txt"), "left alone\n").unwrap();
    let result = engine
        .finish_worktree_commit(
            prepare(&engine, &m, &["src/lib.rs", "selected [x] \n.txt"]).unwrap(),
        )
        .unwrap();
    assert_eq!(result.worktree_id, m.worktree.id);
    assert_eq!(result.parent, m.worktree.base_commit);
    assert_eq!(
        git(&m.worktree_dir, &["rev-parse", "HEAD"]),
        result.commit.0
    );
    assert_eq!(
        git(&m.worktree_dir, &["show", "HEAD:src/lib.rs"]),
        "changed"
    );
    assert_eq!(
        git(&m.worktree_dir, &["show", "HEAD:selected [x] \n.txt"]),
        "selected"
    );
    assert_eq!(
        git(&m.worktree_dir, &["status", "--porcelain"]),
        "?? not-selected.txt"
    );
    assert_eq!(
        git(&m.worktree_dir, &["show", "-s", "--format=%ae"]),
        "operator@example.test"
    );
    assert_eq!(git(&repo, &["rev-parse", "main"]), m.worktree.base_commit.0);
    assert!(engine
        .load_worktree_approval(m.worktree.id)
        .unwrap()
        .is_none());
    GitEngine::verify_git_fsck(&repo).unwrap();
    GitEngine::verify_git_fsck(&m.worktree_dir).unwrap();
}

#[test]
fn host_commit_refuses_staged_changes_hidden_index_flags_and_no_changes() {
    let temp = TempDir::new().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    let engine = GitEngine::new(&repo);
    let m = materialize(&engine, "agent/test/index").unwrap();
    assert!(prepare(&engine, &m, &["src/lib.rs"]).is_err());
    fs::write(m.worktree_dir.join("src/lib.rs"), "updated\n").unwrap();
    git(&m.worktree_dir, &["add", "src/lib.rs"]);
    let index = fs::read(m.worktree_admin_dir.join("index")).unwrap();
    assert!(prepare(&engine, &m, &["src/lib.rs"]).is_err());
    assert_eq!(fs::read(m.worktree_admin_dir.join("index")).unwrap(), index);
    git(&m.worktree_dir, &["reset", "--quiet"]);
    for flag in ["--assume-unchanged", "--skip-worktree"] {
        git(&m.worktree_dir, &["update-index", flag, "src/lib.rs"]);
        assert!(prepare(&engine, &m, &["src/lib.rs"]).is_err());
        git(
            &m.worktree_dir,
            &[
                "update-index",
                "--no-assume-unchanged",
                "--no-skip-worktree",
                "src/lib.rs",
            ],
        );
    }
    assert!(!m.worktree_admin_dir.join("index.lock").exists());
}

#[test]
fn host_commit_final_gate_and_parent_cas_never_publish_on_denial() {
    let temp = TempDir::new().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    let engine = GitEngine::new(&repo);
    let m = materialize(&engine, "agent/test/cas").unwrap();
    fs::write(m.worktree_dir.join("new.txt"), "new\n").unwrap();
    let prepared = prepare(&engine, &m, &["new.txt"]).unwrap();
    assert!(m.worktree_admin_dir.join("index.lock").exists());
    let before = git(&m.worktree_dir, &["rev-parse", "HEAD"]);
    let index = fs::read(m.worktree_admin_dir.join("index")).unwrap();
    assert!(engine
        .finish_worktree_commit_with_gate(prepared, || Err(GitEngineError::VerificationInvalid(
            "revoked".into()
        )))
        .is_err());
    assert_eq!(git(&m.worktree_dir, &["rev-parse", "HEAD"]), before);
    assert_eq!(fs::read(m.worktree_admin_dir.join("index")).unwrap(), index);
    assert!(!m.worktree_admin_dir.join("index.lock").exists());
    let prepared = prepare(&engine, &m, &["new.txt"]).unwrap();
    git(
        &repo,
        &["commit", "--allow-empty", "-m", "advance other branch"],
    );
    let other = git(&repo, &["rev-parse", "HEAD"]);
    git(
        &repo,
        &[
            "update-ref",
            &format!("refs/heads/{}", m.worktree.branch),
            &other,
        ],
    );
    let mut gate_called = false;
    assert!(engine
        .finish_worktree_commit_with_gate(prepared, || {
            gate_called = true;
            Ok(())
        })
        .is_err());
    assert!(!gate_called);
    assert_eq!(git(&m.worktree_dir, &["rev-parse", "HEAD"]), other);
    assert_eq!(fs::read(m.worktree_admin_dir.join("index")).unwrap(), index);
}

#[cfg(unix)]
#[test]
fn host_commit_records_deletions_exec_mode_and_link_text_without_following_links() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let temp = TempDir::new().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    let engine = GitEngine::new(&repo);
    let m = materialize(&engine, "agent/test/modes").unwrap();
    fs::remove_file(m.worktree_dir.join("src/lib.rs")).unwrap();
    fs::write(m.worktree_dir.join("run.sh"), "#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(
        m.worktree_dir.join("run.sh"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    fs::write(temp.path().join("secret"), "must not be recorded").unwrap();
    symlink(temp.path().join("secret"), m.worktree_dir.join("link")).unwrap();
    let result = engine
        .finish_worktree_commit(prepare(&engine, &m, &["src/lib.rs", "run.sh", "link"]).unwrap())
        .unwrap();
    assert_eq!(
        git(&m.worktree_dir, &["show", "HEAD:link"]),
        temp.path().join("secret").to_str().unwrap()
    );
    assert!(git(&m.worktree_dir, &["ls-tree", "HEAD"]).contains("100755 blob"));
    assert!(git(&m.worktree_dir, &["ls-tree", "HEAD"]).contains("120000 blob"));
    assert!(!git(&m.worktree_dir, &["ls-tree", "-r", "HEAD"]).contains("src/lib.rs"));
    assert_eq!(git(&m.worktree_dir, &["status", "--porcelain"]), "");
    assert_eq!(
        engine
            .clean_worktree_commit(m.worktree.id, &m.worktree.branch)
            .unwrap(),
        result.commit
    );
    GitEngine::verify_git_fsck(&repo).unwrap();
}

#[cfg(unix)]
#[test]
fn host_commit_rejects_metadata_parent_symlinks_directories_fifos_and_untracked_deletions() {
    use std::os::unix::fs::symlink;
    let temp = TempDir::new().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    let engine = GitEngine::new(&repo);
    let m = materialize(&engine, "agent/test/paths").unwrap();
    fs::write(temp.path().join("secret"), "outside secret").unwrap();
    symlink(temp.path(), m.worktree_dir.join("outside")).unwrap();
    let fifo = std::ffi::CString::new(m.worktree_dir.join("pipe").to_str().unwrap()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    for path in [
        ".git",
        ".git/config",
        "src/.GiT/config",
        "../secret",
        "/etc/passwd",
        "outside/secret",
        "src",
        "pipe",
        "missing.txt",
        "src//lib.rs",
        "src/./lib.rs",
    ] {
        assert!(prepare(&engine, &m, &[path]).is_err(), "accepted {path:?}");
        assert!(!m.worktree_admin_dir.join("index.lock").exists());
    }
    assert_eq!(
        git(&m.worktree_dir, &["rev-parse", "HEAD"]),
        m.worktree.base_commit.0
    );
    assert_eq!(
        fs::read_to_string(temp.path().join("secret")).unwrap(),
        "outside secret"
    );
}

#[test]
fn host_commit_prepare_drop_and_existing_locks_preserve_repository_state() {
    let temp = TempDir::new().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    let engine = GitEngine::new(&repo);
    let m = materialize(&engine, "agent/test/drop").unwrap();
    fs::write(m.worktree_dir.join("new.txt"), "new\n").unwrap();
    let original = fs::read(m.worktree_admin_dir.join("index")).unwrap();
    let prepared = prepare(&engine, &m, &["new.txt"]).unwrap();
    assert!(prepare(&engine, &m, &["new.txt"]).is_err());
    assert!(m.worktree_admin_dir.join("index.lock").exists());
    let rival = GitEngine::new(&repo);
    assert!(matches!(
        rival.lock_worktree_service(),
        Err(GitEngineError::WorktreeServiceBusy(_))
    ));
    drop(prepared);
    assert_eq!(
        fs::read(m.worktree_admin_dir.join("index")).unwrap(),
        original
    );
    assert!(!m.worktree_admin_dir.join("index.lock").exists());
    assert!(rival.lock_worktree_service().is_ok());
    assert_eq!(
        git(&m.worktree_dir, &["rev-parse", "HEAD"]),
        m.worktree.base_commit.0
    );
}

#[test]
fn host_commit_validates_origin_attachment_identity_and_approval() {
    let temp = TempDir::new().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    let engine = GitEngine::new(&repo);
    let m = materialize(&engine, "agent/test/origin").unwrap();
    fs::write(m.worktree_dir.join("new.txt"), "new\n").unwrap();
    let origin = m.worktree_admin_dir.join("gitust-origin.json");
    let bytes = fs::read(&origin).unwrap();
    fs::write(&origin, b"{}").unwrap();
    assert!(prepare(&engine, &m, &["new.txt"]).is_err());
    fs::write(&origin, bytes).unwrap();
    git(&m.worktree_dir, &["checkout", "--detach", "--quiet"]);
    assert!(prepare(&engine, &m, &["new.txt"]).is_err());
    git(
        &m.worktree_dir,
        &["checkout", "--quiet", &m.worktree.branch],
    );
    let result = engine
        .finish_worktree_commit(prepare(&engine, &m, &["new.txt"]).unwrap())
        .unwrap();
    engine
        .approve_verified_worktree(m.worktree.id, &m.worktree.branch, "main", &result.commit)
        .unwrap();
    fs::write(m.worktree_dir.join("new.txt"), "after approval\n").unwrap();
    assert!(engine
        .prepare_worktree_commit(
            m.worktree.id,
            &m.worktree.branch,
            &result.commit,
            &["new.txt".into()],
            "should fail",
            "operator@example.test"
        )
        .is_err());
    assert_eq!(git(&repo, &["rev-parse", "main"]), result.commit.0);
}

#[cfg(unix)]
#[test]
fn host_commit_never_runs_hooks_signing_or_filters() {
    use std::os::unix::fs::PermissionsExt;
    let temp = TempDir::new().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    let engine = GitEngine::new(&repo);
    let m = materialize(&engine, "agent/test/hooks").unwrap();
    let marker = temp.path().join("executed");
    let program = format!("#!/bin/sh\ntouch '{}'\nexit 1\n", marker.display());
    for hook in [
        "pre-commit",
        "commit-msg",
        "post-commit",
        "reference-transaction",
    ] {
        let path = m.common_git_dir.join("hooks").join(hook);
        fs::write(&path, &program).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let signer = temp.path().join("signer");
    fs::write(&signer, &program).unwrap();
    fs::set_permissions(&signer, fs::Permissions::from_mode(0o755)).unwrap();
    git(&repo, &["config", "commit.gpgsign", "true"]);
    git(&repo, &["config", "gpg.program", signer.to_str().unwrap()]);
    fs::write(m.worktree_dir.join("new.txt"), "exact\r\nbytes\n").unwrap();
    let result = engine
        .finish_worktree_commit(prepare(&engine, &m, &["new.txt"]).unwrap())
        .unwrap();
    assert!(!marker.exists());
    let object = Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["show", &format!("{}:new.txt", result.commit.0)])
        .output()
        .unwrap();
    assert!(object.status.success());
    assert_eq!(object.stdout, b"exact\r\nbytes\n");
    git(
        &repo,
        &["config", "filter.evil.clean", signer.to_str().unwrap()],
    );
    fs::write(
        m.worktree_dir.join(".gitattributes"),
        "new.txt filter=evil\n",
    )
    .unwrap();
    assert!(engine
        .prepare_worktree_commit(
            m.worktree.id,
            &m.worktree.branch,
            &result.commit,
            &["new.txt".into()],
            "filtered",
            "operator@example.test"
        )
        .is_err());
    assert!(!marker.exists());
}

#[test]
fn host_commit_rejects_oversized_requests_and_files_before_ref_update() {
    let temp = TempDir::new().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    let engine = GitEngine::new(&repo);
    let m = materialize(&engine, "agent/test/limits").unwrap();
    let parent = &m.worktree.base_commit;
    for paths in [
        vec![],
        vec!["x".into(); 129],
        vec!["x".repeat(4097)],
        (0..17)
            .map(|n| format!("{n}{}", "x".repeat(4000)))
            .collect(),
    ] {
        assert!(engine
            .prepare_worktree_commit(
                m.worktree.id,
                &m.worktree.branch,
                parent,
                &paths,
                "message",
                "operator@example.test"
            )
            .is_err());
    }
    for message in [String::new(), "x".repeat(8193), "contains\0NUL".into()] {
        assert!(engine
            .prepare_worktree_commit(
                m.worktree.id,
                &m.worktree.branch,
                parent,
                &["new.txt".into()],
                &message,
                "operator@example.test"
            )
            .is_err());
    }
    let file = fs::File::create(m.worktree_dir.join("large")).unwrap();
    file.set_len(8 * 1024 * 1024 + 1).unwrap();
    assert!(prepare(&engine, &m, &["large"]).is_err());
    for n in 0..5 {
        let file = fs::File::create(m.worktree_dir.join(format!("source-{n}"))).unwrap();
        file.set_len(8 * 1024 * 1024).unwrap();
    }
    assert!(prepare(
        &engine,
        &m,
        &["source-0", "source-1", "source-2", "source-3", "source-4"]
    )
    .is_err());
    assert_eq!(git(&m.worktree_dir, &["rev-parse", "HEAD"]), parent.0);
    assert!(!m.worktree_admin_dir.join("index.lock").exists());
}

#[test]
fn host_commit_reports_created_sha_when_index_installation_fails() {
    let temp = TempDir::new().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    let engine = GitEngine::new(&repo);
    let m = materialize(&engine, "agent/test/recovery").unwrap();
    fs::write(m.worktree_dir.join("new.txt"), "new\n").unwrap();
    let original = fs::read(m.worktree_admin_dir.join("index")).unwrap();
    let prepared = prepare(&engine, &m, &["new.txt"]).unwrap();
    // Inject a filesystem failure immediately after all validation, at the CAS boundary.
    let error = engine
        .finish_worktree_commit_with_gate(prepared, || {
            fs::remove_file(m.worktree_admin_dir.join("index.lock")).unwrap();
            fs::create_dir(m.worktree_admin_dir.join("index.lock")).unwrap();
            Ok(())
        })
        .unwrap_err();
    let created = git(&m.worktree_dir, &["rev-parse", "HEAD"]);
    assert_ne!(created, m.worktree.base_commit.0);
    assert!(error.to_string().contains(&created));
    assert!(error.to_string().contains("index recovery is required"));
    assert_eq!(
        fs::read(m.worktree_admin_dir.join("index")).unwrap(),
        original
    );
    assert!(engine
        .load_worktree_approval(m.worktree.id)
        .unwrap()
        .is_none());
    assert!(engine
        .clean_worktree_commit(m.worktree.id, &m.worktree.branch)
        .is_err());
    fs::remove_dir(m.worktree_admin_dir.join("index.lock")).unwrap();
    git(&m.worktree_dir, &["reset", "--mixed", "HEAD"]);
    assert_eq!(
        engine
            .clean_worktree_commit(m.worktree.id, &m.worktree.branch)
            .unwrap()
            .0,
        created
    );
    GitEngine::verify_git_fsck(&repo).unwrap();
}

#[test]
fn host_commit_refuses_noncooperating_index_changes_and_unmerged_entries() {
    let temp = TempDir::new().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    let engine = GitEngine::new(&repo);
    let m = materialize(&engine, "agent/test/index-cas").unwrap();
    fs::write(m.worktree_dir.join("new.txt"), "new\n").unwrap();
    let prepared = prepare(&engine, &m, &["new.txt"]).unwrap();
    fs::write(m.worktree_admin_dir.join("index"), "replaced externally").unwrap();
    let mut gate_called = false;
    assert!(engine
        .finish_worktree_commit_with_gate(prepared, || {
            gate_called = true;
            Ok(())
        })
        .is_err());
    assert!(!gate_called);
    assert_eq!(
        git(&m.worktree_dir, &["rev-parse", "HEAD"]),
        m.worktree.base_commit.0
    );
    assert_eq!(
        fs::read_to_string(m.worktree_admin_dir.join("index")).unwrap(),
        "replaced externally"
    );
    fs::remove_file(m.worktree_admin_dir.join("index")).unwrap();
    git(&m.worktree_dir, &["reset", "--mixed", "HEAD"]);
    let blob = git(&m.worktree_dir, &["rev-parse", "HEAD:src/lib.rs"]);
    let mut command = crate::git_command(&m.worktree_dir);
    command.args(["update-index", "--index-info"]);
    crate::process::run_capped_with_input(command, std::time::Duration::from_secs(10), 0,
        Some(format!("0 {}\tsrc/lib.rs\n100644 {blob} 1\tsrc/lib.rs\n100644 {blob} 2\tsrc/lib.rs\n100644 {blob} 3\tsrc/lib.rs\n", "0".repeat(40)).into_bytes())).unwrap();
    assert!(!git(&m.worktree_dir, &["ls-files", "--unmerged"]).is_empty());
    let unmerged = fs::read(m.worktree_admin_dir.join("index")).unwrap();
    assert!(prepare(&engine, &m, &["new.txt"]).is_err());
    assert_eq!(
        fs::read(m.worktree_admin_dir.join("index")).unwrap(),
        unmerged
    );
}
