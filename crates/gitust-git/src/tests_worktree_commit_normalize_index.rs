use super::*;

pub(super) fn index_oracle(dir: &Path, parent: &str, paths: &[&str]) -> String {
    let temporary = TempDir::new().unwrap();
    let index = temporary.path().join("index");
    let run = |args: &[&str]| {
        let output = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args([
                "--literal-pathspecs",
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "core.fsmonitor=false",
            ])
            .args(args)
            .env("GIT_INDEX_FILE", &index)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    };
    run(&["read-tree", parent]);
    let mut args = vec!["add", "--force", "--"];
    args.extend_from_slice(paths);
    run(&args);
    run(&["write-tree"])
}

fn raw_crlf_parent(repo: &Path, attributes: &str) {
    init_repo(repo);
    fs::write(repo.join(".gitattributes"), "*.txt -text\n").unwrap();
    fs::write(repo.join("a.txt"), "old\r\n").unwrap();
    git(repo, &["add", ".gitattributes", "a.txt"]);
    git(repo, &["commit", "-qm", "raw CRLF blob"]);
    fs::write(repo.join(".gitattributes"), attributes).unwrap();
    git(repo, &["add", ".gitattributes"]);
    git(
        repo,
        &[
            "commit",
            "-qm",
            "change attributes without renormalizing old blob",
        ],
    );
}

fn commit_and_assert_oracle(engine: &GitEngine, m: &MaterializedWorktree, paths: &[&str]) {
    let expected_tree = index_oracle(&m.worktree_dir, &m.worktree.base_commit.0, paths);
    let prepared = engine
        .prepare_worktree_commit(
            m.worktree.id,
            &m.worktree.branch,
            &m.worktree.base_commit,
            &paths
                .iter()
                .map(|path| (*path).to_owned())
                .collect::<Vec<_>>(),
            "match ordinary git add",
            "operator@example.test",
        )
        .unwrap();
    let committed = engine.finish_worktree_commit(prepared).unwrap();
    assert_eq!(
        git(&m.worktree_dir, &["rev-parse", "HEAD^{tree}"]),
        expected_tree
    );
    let output = Command::new("git")
        .arg("-C")
        .arg(&m.worktree_dir)
        .args(["show", "HEAD:a.txt"])
        .output()
        .unwrap();
    assert_eq!(output.stdout, b"new text\r\n");
    assert_eq!(git(&m.worktree_dir, &["status", "--porcelain"]), "");
    assert_eq!(
        engine
            .clean_worktree_commit(m.worktree.id, &m.worktree.branch)
            .unwrap(),
        committed.commit
    );
    GitEngine::verify_git_fsck(&m.worktree_dir).unwrap();
}

#[test]
fn host_commit_index_aware_text_auto_preserves_existing_raw_crlf_blob() {
    for (attributes, autocrlf) in [
        ("*.txt text=auto\n", None),
        ("", Some("true")),
        ("", Some("input")),
    ] {
        let temporary = TempDir::new().unwrap();
        let repo = temporary.path().join("repo");
        raw_crlf_parent(&repo, attributes);
        if let Some(value) = autocrlf {
            git(&repo, &["config", "core.autocrlf", value]);
        }
        let engine = GitEngine::new(&repo);
        let m = materialize(&engine, "agent/test/index-aware-auto").unwrap();
        fs::write(m.worktree_dir.join("a.txt"), "new text\r\n").unwrap();
        commit_and_assert_oracle(&engine, &m, &["a.txt"]);
    }
}

#[test]
fn host_commit_index_aware_selected_attribute_deletion_changes_fallback() {
    let temporary = TempDir::new().unwrap();
    let repo = temporary.path().join("repo");
    raw_crlf_parent(&repo, "*.txt text eol=crlf\n");
    let engine = GitEngine::new(&repo);
    let m = materialize(&engine, "agent/test/delete-attributes").unwrap();
    fs::remove_file(m.worktree_dir.join(".gitattributes")).unwrap();
    fs::write(m.worktree_dir.join("a.txt"), "new text\r\n").unwrap();
    commit_and_assert_oracle(&engine, &m, &[".gitattributes", "a.txt"]);
}
