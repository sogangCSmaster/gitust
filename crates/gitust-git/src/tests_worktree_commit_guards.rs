use super::*;

#[test]
fn host_commit_rejects_partial_updates_when_git_ignores_an_explicit_path() {
    let temp = TempDir::new().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    git(&repo, &["config", "core.protectNTFS", "true"]);
    git(&repo, &["config", "core.protectHFS", "true"]);
    let engine = GitEngine::new(&repo);
    for (index, name) in ["git~1", ".git.", ".git\u{200d}"].iter().enumerate() {
        let m = materialize(&engine, &format!("agent/test/ignored-{index}")).unwrap();
        fs::write(m.worktree_dir.join("normal.txt"), "normal\n").unwrap();
        fs::write(m.worktree_dir.join(name), "must not be silently omitted\n").unwrap();
        let original = fs::read(m.worktree_admin_dir.join("index")).unwrap();
        let prepared = engine.prepare_worktree_commit(
            m.worktree.id,
            &m.worktree.branch,
            &m.worktree.base_commit,
            &["normal.txt".into(), (*name).into()],
            "must be all or nothing",
            "operator@example.test",
        );
        assert!(
            prepared.is_err(),
            "Git ignored requested path {name:?}, but preparation succeeded"
        );
        assert_eq!(
            git(&m.worktree_dir, &["rev-parse", "HEAD"]),
            m.worktree.base_commit.0
        );
        assert_eq!(
            fs::read(m.worktree_admin_dir.join("index")).unwrap(),
            original
        );
        assert!(!m.worktree_admin_dir.join("index.lock").exists());
    }
}

#[cfg(unix)]
#[test]
fn host_commit_uses_owner_executable_bit_like_git() {
    use std::os::unix::fs::PermissionsExt;
    let temp = TempDir::new().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    git(&repo, &["config", "core.fileMode", "true"]);
    let engine = GitEngine::new(&repo);
    let m = materialize(&engine, "agent/test/owner-mode").unwrap();
    fs::write(m.worktree_dir.join("not-owner-executable"), "data\n").unwrap();
    fs::set_permissions(
        m.worktree_dir.join("not-owner-executable"),
        fs::Permissions::from_mode(0o654),
    )
    .unwrap();
    let prepared = engine
        .prepare_worktree_commit(
            m.worktree.id,
            &m.worktree.branch,
            &m.worktree.base_commit,
            &["not-owner-executable".into()],
            "preserve Git mode semantics",
            "operator@example.test",
        )
        .unwrap();
    engine.finish_worktree_commit(prepared).unwrap();
    assert!(git(
        &m.worktree_dir,
        &["ls-tree", "HEAD", "not-owner-executable"]
    )
    .starts_with("100644 blob "));
    assert_eq!(git(&m.worktree_dir, &["status", "--porcelain"]), "");
    GitEngine::verify_git_fsck(&repo).unwrap();
}

#[cfg(unix)]
#[test]
fn verification_rejects_executable_linked_worktree_config_without_running_filter() {
    use std::os::unix::fs::PermissionsExt;
    let temp = TempDir::new().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    git(&repo, &["config", "extensions.worktreeConfig", "true"]);
    let engine = GitEngine::new(&repo);
    let m = materialize(&engine, "agent/test/verify-worktree-config").unwrap();
    let marker = temp.path().join("executed");
    let filter = temp.path().join("filter");
    fs::write(
        &filter,
        format!("#!/bin/sh\ntouch '{}'\ncat\n", marker.display()),
    )
    .unwrap();
    fs::set_permissions(&filter, fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(
        m.worktree_admin_dir.join("config.worktree"),
        format!("[filter \"evil\"]\nclean = {}\n", filter.display()),
    )
    .unwrap();
    fs::write(
        m.worktree_dir.join(".gitattributes"),
        "src/lib.rs filter=evil\n",
    )
    .unwrap();
    let source = m.worktree_dir.join("src/lib.rs");
    let length = fs::read(&source).unwrap().len();
    fs::write(&source, vec![b'x'; length]).unwrap();
    fs::File::options()
        .write(true)
        .open(&source)
        .unwrap()
        .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1))
        .unwrap();
    // Independent oracle: the host status command itself executes this active driver.
    let output = crate::process::run_capped(
        {
            let mut command = crate::git_command(&m.worktree_dir);
            command.args([
                "status",
                "--porcelain=v1",
                "-z",
                "--untracked-files=normal",
                "--ignore-submodules=none",
            ]);
            command
        },
        std::time::Duration::from_secs(10),
        4096,
    )
    .unwrap();
    assert!(!output.0.is_empty());
    assert!(marker.exists());
    fs::remove_file(&marker).unwrap();
    assert!(matches!(
        engine.clean_worktree_commit(m.worktree.id, &m.worktree.branch),
        Err(GitEngineError::CredentialsInRepoConfig)
    ));
    assert!(
        !marker.exists(),
        "verification ran a linked worktree filter on the host"
    );
}

#[cfg(unix)]
#[test]
fn worktree_configuration_guard_rejects_oversized_symlink_and_fifo_files() {
    use std::os::unix::fs::symlink;
    let temp = TempDir::new().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    let engine = GitEngine::new(&repo);
    let m = materialize(&engine, "agent/test/bounded-config").unwrap();
    engine
        .ensure_worktree_config_has_no_credentials(&m.worktree_dir)
        .unwrap();
    let config = m.worktree_admin_dir.join("config.worktree");
    fs::write(&config, vec![b'x'; 1024 * 1024 + 1]).unwrap();
    assert!(engine
        .ensure_worktree_config_has_no_credentials(&m.worktree_dir)
        .is_err());
    fs::remove_file(&config).unwrap();
    let outside = temp.path().join("outside-config");
    fs::write(&outside, "[core]\nautocrlf=false\n").unwrap();
    symlink(&outside, &config).unwrap();
    assert!(engine
        .ensure_worktree_config_has_no_credentials(&m.worktree_dir)
        .is_err());
    fs::remove_file(&config).unwrap();
    let path = std::ffi::CString::new(config.to_str().unwrap()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
    assert!(engine
        .ensure_worktree_config_has_no_credentials(&m.worktree_dir)
        .is_err());
    fs::remove_file(&config).unwrap();
    engine
        .ensure_worktree_config_has_no_credentials(&m.worktree_dir)
        .unwrap();
}
