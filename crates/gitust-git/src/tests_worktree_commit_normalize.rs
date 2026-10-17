use super::*;

fn commit(
    engine: &GitEngine,
    m: &MaterializedWorktree,
    paths: &[&str],
) -> Result<crate::WorktreeCommit, GitEngineError> {
    let prepared = engine.prepare_worktree_commit(
        m.worktree.id,
        &m.worktree.branch,
        &m.worktree.base_commit,
        &paths
            .iter()
            .map(|path| (*path).to_owned())
            .collect::<Vec<_>>(),
        "normalize builtins",
        "operator@example.test",
    )?;
    engine.finish_worktree_commit(prepared)
}

#[test]
fn host_commit_normalizes_text_eol_and_keeps_binary_bytes() {
    let temp = TempDir::new().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    fs::write(
        repo.join(".gitattributes"),
        "*.txt text eol=crlf\n*.bin -text\n",
    )
    .unwrap();
    git(&repo, &["add", ".gitattributes"]);
    git(&repo, &["commit", "-qm", "attributes"]);
    let engine = GitEngine::new(&repo);
    let m = materialize(&engine, "agent/test/text-normalize").unwrap();
    fs::write(m.worktree_dir.join("line.txt"), b"first\r\nsecond\r\n").unwrap();
    fs::write(
        m.worktree_dir.join("data.bin"),
        b"binary\0first\r\nsecond\r\n",
    )
    .unwrap();
    let committed = commit(&engine, &m, &["line.txt", "data.bin"]).unwrap();
    let output = Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["show", &format!("{}:line.txt", committed.commit.0)])
        .output()
        .unwrap();
    assert_eq!(output.stdout, b"first\nsecond\n");
    let output = Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["show", &format!("{}:data.bin", committed.commit.0)])
        .output()
        .unwrap();
    assert_eq!(output.stdout, b"binary\0first\r\nsecond\r\n");
    assert_eq!(git(&m.worktree_dir, &["status", "--porcelain"]), "");
    assert_eq!(
        engine
            .clean_worktree_commit(m.worktree.id, &m.worktree.branch)
            .unwrap(),
        committed.commit
    );
    GitEngine::verify_git_fsck(&repo).unwrap();
}

#[test]
fn host_commit_honors_autocrlf_and_nested_attribute_overrides() {
    let temp = TempDir::new().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    git(&repo, &["config", "core.autocrlf", "true"]);
    let engine = GitEngine::new(&repo);
    let m = materialize(&engine, "agent/test/autocrlf").unwrap();
    fs::create_dir(m.worktree_dir.join("nested")).unwrap();
    fs::write(
        m.worktree_dir.join("nested/.gitattributes"),
        "*.txt -text\n",
    )
    .unwrap();
    fs::write(m.worktree_dir.join("line.txt"), b"first\r\nsecond\r\n").unwrap();
    fs::write(
        m.worktree_dir.join("nested/raw.txt"),
        b"first\r\nsecond\r\n",
    )
    .unwrap();
    commit(
        &engine,
        &m,
        &["line.txt", "nested/.gitattributes", "nested/raw.txt"],
    )
    .unwrap();
    assert_eq!(git(&m.worktree_dir, &["status", "--porcelain"]), "");
    let output = Command::new("git")
        .arg("-C")
        .arg(&m.worktree_dir)
        .args(["show", "HEAD:line.txt"])
        .output()
        .unwrap();
    assert_eq!(output.stdout, b"first\nsecond\n");
    let output = Command::new("git")
        .arg("-C")
        .arg(&m.worktree_dir)
        .args(["show", "HEAD:nested/raw.txt"])
        .output()
        .unwrap();
    assert_eq!(output.stdout, b"first\r\nsecond\r\n");
}

#[test]
fn host_commit_handles_ident_encoding_and_info_attribute_precedence() {
    let temp = TempDir::new().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    fs::write(
        repo.join(".gitattributes"),
        "*.txt -text\n*.utf16 text working-tree-encoding=UTF-16LE\nidentity ident\n",
    )
    .unwrap();
    git(&repo, &["add", ".gitattributes"]);
    git(&repo, &["commit", "-qm", "attributes"]);
    fs::write(repo.join(".git/info/attributes"), "*.txt text eol=crlf\n").unwrap();
    let engine = GitEngine::new(&repo);
    let m = materialize(&engine, "agent/test/encoding").unwrap();
    fs::write(m.worktree_dir.join("line.txt"), b"first\r\nsecond\r\n").unwrap();
    fs::write(m.worktree_dir.join("document.utf16"), b"a\0\r\0\n\0").unwrap();
    fs::write(m.worktree_dir.join("identity"), b"$Id: abcdef $\n").unwrap();
    commit(&engine, &m, &["line.txt", "document.utf16", "identity"]).unwrap();
    assert_eq!(git(&m.worktree_dir, &["show", "HEAD:document.utf16"]), "a");
    assert_eq!(git(&m.worktree_dir, &["show", "HEAD:identity"]), "$Id$");
    assert_eq!(git(&m.worktree_dir, &["status", "--porcelain"]), "");
    GitEngine::verify_git_fsck(&repo).unwrap();
}

#[test]
fn host_commit_attribute_fallback_matches_git_when_tracked_attributes_are_deleted() {
    let temp = TempDir::new().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    fs::write(repo.join(".gitattributes"), "*.txt text eol=crlf\n").unwrap();
    git(&repo, &["add", ".gitattributes"]);
    git(&repo, &["commit", "-qm", "attributes"]);
    let engine = GitEngine::new(&repo);
    let m = materialize(&engine, "agent/test/attr-fallback").unwrap();
    fs::remove_file(m.worktree_dir.join(".gitattributes")).unwrap();
    fs::write(m.worktree_dir.join("line.txt"), b"first\r\nsecond\r\n").unwrap();
    commit(&engine, &m, &["line.txt"]).unwrap();
    let output = Command::new("git")
        .arg("-C")
        .arg(&m.worktree_dir)
        .args(["show", "HEAD:line.txt"])
        .output()
        .unwrap();
    assert_eq!(output.stdout, b"first\nsecond\n");
    assert_eq!(
        git(&m.worktree_dir, &["status", "--porcelain"]),
        "D .gitattributes"
    );
}

#[cfg(unix)]
#[test]
fn host_commit_refuses_external_filters_in_worktree_config_without_running_them() {
    use std::os::unix::fs::PermissionsExt;
    let temp = TempDir::new().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    git(&repo, &["config", "extensions.worktreeConfig", "true"]);
    let engine = GitEngine::new(&repo);
    let m = materialize(&engine, "agent/test/worktree-filter").unwrap();
    let marker = temp.path().join("executed");
    let filter = temp.path().join("clean-filter");
    fs::write(
        &filter,
        format!("#!/bin/sh\ntouch '{}'\ncat\n", marker.display()),
    )
    .unwrap();
    fs::set_permissions(&filter, fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(
        m.worktree_admin_dir.join("config.worktree"),
        format!(
            "[filter \"evil\"]\nclean = {}\nprocess = {}\n",
            filter.display(),
            filter.display()
        ),
    )
    .unwrap();
    fs::write(
        m.worktree_dir.join(".gitattributes"),
        "line.txt filter=evil\n",
    )
    .unwrap();
    fs::write(m.worktree_dir.join("line.txt"), "first\r\n").unwrap();
    let error = commit(&engine, &m, &["line.txt"]).unwrap_err();
    assert!(matches!(error, GitEngineError::CredentialsInRepoConfig));
    assert!(!marker.exists());
    assert_eq!(
        git(&m.worktree_dir, &["rev-parse", "HEAD"]),
        m.worktree.base_commit.0
    );
}

#[cfg(unix)]
#[test]
fn host_commit_refuses_symlink_attributes_and_detects_attribute_configuration_changes() {
    use std::os::unix::fs::symlink;
    let temp = TempDir::new().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    let engine = GitEngine::new(&repo);
    let m = materialize(&engine, "agent/test/attribute-snapshot").unwrap();
    fs::write(m.worktree_dir.join("line.txt"), "line\r\n").unwrap();
    fs::write(temp.path().join("outside-attributes"), "*.txt text\n").unwrap();
    symlink(
        temp.path().join("outside-attributes"),
        m.worktree_dir.join(".gitattributes"),
    )
    .unwrap();
    assert!(commit(&engine, &m, &["line.txt"]).is_err());
    fs::remove_file(m.worktree_dir.join(".gitattributes")).unwrap();
    fs::write(m.worktree_dir.join(".gitattributes"), "*.txt text\n").unwrap();
    let prepare = || {
        engine
            .prepare_worktree_commit(
                m.worktree.id,
                &m.worktree.branch,
                &m.worktree.base_commit,
                &["line.txt".into()],
                "snapshot",
                "operator@example.test",
            )
            .unwrap()
    };
    let prepared = prepare();
    fs::write(m.worktree_dir.join(".gitattributes"), "*.txt -text\n").unwrap();
    let mut called = false;
    assert!(engine
        .finish_worktree_commit_with_gate(prepared, || {
            called = true;
            Ok(())
        })
        .is_err());
    assert!(!called);
    fs::write(m.worktree_dir.join(".gitattributes"), "*.txt text\n").unwrap();
    let prepared = prepare();
    git(&repo, &["config", "core.autocrlf", "true"]);
    assert!(engine.finish_worktree_commit(prepared).is_err());
    assert_eq!(
        git(&m.worktree_dir, &["rev-parse", "HEAD"]),
        m.worktree.base_commit.0
    );
    assert!(!m.worktree_admin_dir.join("index.lock").exists());
}

#[test]
fn host_commit_honors_enabled_worktree_builtin_configuration() {
    let temp = TempDir::new().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    use std::io::Write;
    fs::OpenOptions::new()
        .append(true)
        .open(repo.join(".git/config"))
        .unwrap()
        .write_all(b"[extensions]\nworktreeConfig\n[core]\nautocrlf=false\n")
        .unwrap();
    let engine = GitEngine::new(&repo);
    let m = materialize(&engine, "agent/test/worktree-builtins").unwrap();
    fs::write(
        m.worktree_admin_dir.join("config.worktree"),
        "[core]\nautocrlf = true\n",
    )
    .unwrap();
    fs::write(m.worktree_dir.join("line.txt"), "line\r\n").unwrap();
    commit(&engine, &m, &["line.txt"]).unwrap();
    let output = Command::new("git")
        .arg("-C")
        .arg(&m.worktree_dir)
        .args(["show", "HEAD:line.txt"])
        .output()
        .unwrap();
    assert_eq!(output.stdout, b"line\n");
    assert_eq!(git(&m.worktree_dir, &["status", "--porcelain"]), "");
}

#[test]
fn host_commit_uses_case_matching_policy_for_attribute_patterns() {
    let temp = TempDir::new().unwrap();
    let repo = temp.path().join("repo");
    init_repo(&repo);
    git(&repo, &["config", "core.ignorecase", "true"]);
    fs::write(repo.join(".gitattributes"), "*.TXT text eol=crlf\n").unwrap();
    git(&repo, &["add", ".gitattributes"]);
    git(&repo, &["commit", "-qm", "attributes"]);
    let engine = GitEngine::new(&repo);
    let m = materialize(&engine, "agent/test/attr-pattern-case").unwrap();
    fs::write(m.worktree_dir.join("line.txt"), "line\r\n").unwrap();
    commit(&engine, &m, &["line.txt"]).unwrap();
    let output = Command::new("git")
        .arg("-C")
        .arg(&m.worktree_dir)
        .args(["show", "HEAD:line.txt"])
        .output()
        .unwrap();
    assert_eq!(output.stdout, b"line\n");
    assert_eq!(git(&m.worktree_dir, &["status", "--porcelain"]), "");
}
