use super::tests_worktree_commit_normalize_index::index_oracle;
use super::*;

fn assert_oracle(engine: &GitEngine, m: &MaterializedWorktree, paths: &[&str]) {
    let expected = index_oracle(&m.worktree_dir, &m.worktree.base_commit.0, paths);
    let prepared = engine
        .prepare_worktree_commit(
            m.worktree.id,
            &m.worktree.branch,
            &m.worktree.base_commit,
            &paths
                .iter()
                .map(|path| (*path).to_owned())
                .collect::<Vec<_>>(),
            "preserve Git entry semantics",
            "operator@example.test",
        )
        .unwrap();
    let result = engine.finish_worktree_commit(prepared).unwrap();
    assert_eq!(
        git(&m.worktree_dir, &["rev-parse", "HEAD^{tree}"]),
        expected
    );
    assert_eq!(git(&m.worktree_dir, &["status", "--porcelain"]), "");
    assert_eq!(
        engine
            .clean_worktree_commit(m.worktree.id, &m.worktree.branch)
            .unwrap(),
        result.commit
    );
    GitEngine::verify_git_fsck(&m.worktree_dir).unwrap();
}

#[cfg(unix)]
#[test]
fn host_commit_mode_policy_honors_disabled_filemode_in_common_and_worktree_configs() {
    use std::os::unix::fs::PermissionsExt;
    for worktree_config in [false, true] {
        let temporary = TempDir::new().unwrap();
        let repo = temporary.path().join("repo");
        init_repo(&repo);
        fs::write(repo.join("script.sh"), "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(repo.join("script.sh"), fs::Permissions::from_mode(0o755)).unwrap();
        git(&repo, &["add", "script.sh"]);
        git(&repo, &["commit", "-qm", "executable parent"]);
        git(&repo, &["config", "extensions.worktreeConfig", "true"]);
        if !worktree_config {
            git(&repo, &["config", "core.fileMode", "false"]);
        }
        let engine = GitEngine::new(&repo);
        let m = materialize(&engine, "agent/test/filemode-policy").unwrap();
        if worktree_config {
            fs::write(
                m.worktree_admin_dir.join("config.worktree"),
                "[core]\nfileMode = false\n",
            )
            .unwrap();
        }
        fs::set_permissions(
            m.worktree_dir.join("script.sh"),
            fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        fs::write(m.worktree_dir.join("script.sh"), "#!/bin/sh\nexit 1\n").unwrap();
        fs::write(m.worktree_dir.join("new.sh"), "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(
            m.worktree_dir.join("new.sh"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        assert_oracle(&engine, &m, &["script.sh", "new.sh"]);
        assert!(git(&m.worktree_dir, &["ls-tree", "HEAD", "script.sh"]).starts_with("100755 "));
        assert!(git(&m.worktree_dir, &["ls-tree", "HEAD", "new.sh"]).starts_with("100644 "));
    }
}

#[test]
fn host_commit_mode_policy_honors_legacy_crlf_attributes() {
    for attribute in ["crlf", "crlf=input", "-crlf"] {
        let temporary = TempDir::new().unwrap();
        let repo = temporary.path().join("repo");
        init_repo(&repo);
        fs::write(repo.join(".gitattributes"), format!("*.txt {attribute}\n")).unwrap();
        git(&repo, &["add", ".gitattributes"]);
        git(&repo, &["commit", "-qm", "legacy attributes"]);
        let engine = GitEngine::new(&repo);
        let m = materialize(&engine, "agent/test/legacy-crlf").unwrap();
        fs::write(m.worktree_dir.join("new.txt"), b"new text\r\n").unwrap();
        assert_oracle(&engine, &m, &["new.txt"]);
        let output = Command::new("git")
            .arg("-C")
            .arg(&m.worktree_dir)
            .args(["show", "HEAD:new.txt"])
            .output()
            .unwrap();
        let expected: &[u8] = if attribute == "-crlf" {
            b"new text\r\n"
        } else {
            b"new text\n"
        };
        assert_eq!(output.stdout, expected);
    }
}

#[cfg(unix)]
#[test]
fn host_commit_mode_policy_preserves_emulated_symlinks() {
    use std::os::unix::fs::symlink;
    let temporary = TempDir::new().unwrap();
    let repo = temporary.path().join("repo");
    init_repo(&repo);
    symlink("old-target", repo.join("link.txt")).unwrap();
    git(&repo, &["add", "link.txt"]);
    git(&repo, &["commit", "-qm", "tracked link"]);
    git(&repo, &["config", "core.symlinks", "false"]);
    let engine = GitEngine::new(&repo);
    let m = materialize(&engine, "agent/test/emulated-link").unwrap();
    assert!(fs::symlink_metadata(m.worktree_dir.join("link.txt"))
        .unwrap()
        .is_file());
    fs::write(m.worktree_dir.join("link.txt"), "new-target").unwrap();
    assert_oracle(&engine, &m, &["link.txt"]);
    assert!(git(&m.worktree_dir, &["ls-tree", "HEAD", "link.txt"]).starts_with("120000 "));
}

#[cfg(unix)]
#[test]
fn host_commit_mode_policy_keeps_actual_file_symlink_type_transitions() {
    use std::os::unix::fs::symlink;
    let temporary = TempDir::new().unwrap();
    let repo = temporary.path().join("repo");
    init_repo(&repo);
    fs::write(repo.join("file.txt"), "regular\n").unwrap();
    symlink("old-target", repo.join("link.txt")).unwrap();
    git(&repo, &["add", "file.txt", "link.txt"]);
    git(&repo, &["commit", "-qm", "both types"]);
    git(&repo, &["config", "core.fileMode", "false"]);
    let engine = GitEngine::new(&repo);
    let m = materialize(&engine, "agent/test/type-change").unwrap();
    fs::remove_file(m.worktree_dir.join("file.txt")).unwrap();
    symlink("new-target", m.worktree_dir.join("file.txt")).unwrap();
    fs::remove_file(m.worktree_dir.join("link.txt")).unwrap();
    fs::write(m.worktree_dir.join("link.txt"), "now regular\n").unwrap();
    assert_oracle(&engine, &m, &["file.txt", "link.txt"]);
    assert!(git(&m.worktree_dir, &["ls-tree", "HEAD", "file.txt"]).starts_with("120000 "));
    assert!(git(&m.worktree_dir, &["ls-tree", "HEAD", "link.txt"]).starts_with("100644 "));
}

#[cfg(unix)]
#[test]
fn host_commit_mode_policy_keeps_executable_bit_with_builtin_conversions() {
    use std::os::unix::fs::PermissionsExt;
    for attribute in ["text eol=lf", "ident", "crlf"] {
        let temporary = TempDir::new().unwrap();
        let repo = temporary.path().join("repo");
        init_repo(&repo);
        git(&repo, &["config", "core.fileMode", "true"]);
        fs::write(repo.join(".gitattributes"), format!("*.sh {attribute}\n")).unwrap();
        git(&repo, &["add", ".gitattributes"]);
        git(&repo, &["commit", "-qm", "script attributes"]);
        let engine = GitEngine::new(&repo);
        let m = materialize(&engine, "agent/test/converted-executable").unwrap();
        fs::write(
            m.worktree_dir.join("new.sh"),
            b"#!/bin/sh\r\necho '$Id: old $'\r\n",
        )
        .unwrap();
        fs::set_permissions(
            m.worktree_dir.join("new.sh"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        assert_oracle(&engine, &m, &["new.sh"]);
        assert!(git(&m.worktree_dir, &["ls-tree", "HEAD", "new.sh"]).starts_with("100755 "));
    }
}
