use std::fs;
use std::path::Path;
use std::process::Command;

use gitust_core::{AgentKind, RepoId, WorktreeId};
use tempfile::TempDir;

use crate::GitEngine;

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Gitust Test")
        .env("GIT_AUTHOR_EMAIL", "test@company.internal")
        .env("GIT_COMMITTER_NAME", "Gitust Test")
        .env("GIT_COMMITTER_EMAIL", "test@company.internal")
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .status()
        .expect("git command");
    assert!(status.success(), "git {args:?} failed");
}

#[test]
fn cow_template_directory_is_populated_and_warm_worktree_spawn_is_fast_and_isolated() {
    let tmp = TempDir::new().expect("tempdir");
    let repo_dir = tmp.path().join("repo");
    fs::create_dir_all(&repo_dir).expect("mkdir");
    git(&repo_dir, &["init", "-q", "-b", "main"]);

    for i in 0..10 {
        let sub = repo_dir.join(format!("pkg/mod_{i}"));
        fs::create_dir_all(&sub).expect("mkdir");
        fs::write(
            sub.join("lib.rs"),
            format!("pub const VAL_{i}: usize = {i};\n"),
        )
        .expect("write");
    }
    git(&repo_dir, &["add", "."]);
    git(&repo_dir, &["commit", "-q", "-m", "ten modules"]);

    // Add an untracked file in the main working tree to prove CoW template materialization
    // reads strictly from the committed Git tree.
    fs::write(repo_dir.join("UNTRACKED_SECRET.txt"), "must not copy\n").expect("untracked");

    let engine = GitEngine::new(&repo_dir);
    let repo_id = RepoId::new();

    let m1 = engine
        .materialize_agent_worktree(
            WorktreeId::new(),
            repo_id,
            AgentKind::ClaudeCode,
            "agent/claude/cow-1",
            "main",
            "first cow worktree",
        )
        .expect("materialize m1");

    // Verify CoW template directory was created under `<common_git_dir>/gitust-cow-templates/<base_sha>`.
    let template_dir = m1
        .common_git_dir
        .join("gitust-cow-templates")
        .join(&m1.worktree.base_commit.0);
    assert!(
        template_dir.join("tree").is_dir() && template_dir.join("index").is_file(),
        "expected CoW template tree and index at {}",
        template_dir.display()
    );
    assert!(!m1.worktree_dir.join("UNTRACKED_SECRET.txt").exists());

    // Second worktree hits the warm CoW template via `clonefile` / reflink.
    let m2 = engine
        .materialize_agent_worktree(
            WorktreeId::new(),
            repo_id,
            AgentKind::CursorAgent,
            "agent/cursor/cow-2",
            "main",
            "second warm cow worktree",
        )
        .expect("materialize m2");

    assert!(
        m2.worktree.materialize_micros < 5_000,
        "warm CoW worktree spawn for ~10 files should be under 5ms (got {} us)",
        m2.worktree.materialize_micros
    );

    // Both worktrees must pass `clean_worktree_commit`, `list_agent_worktrees`, `recover_agent_worktrees`, and `git fsck`.
    assert_eq!(
        engine
            .clean_worktree_commit(m1.worktree.id, &m1.worktree.branch)
            .expect("m1 clean"),
        m1.worktree.base_commit
    );
    assert_eq!(
        engine
            .clean_worktree_commit(m2.worktree.id, &m2.worktree.branch)
            .expect("m2 clean"),
        m2.worktree.base_commit
    );
    assert_eq!(engine.list_agent_worktrees().expect("list").len(), 2);
    assert_eq!(
        engine
            .recover_agent_worktrees(repo_id)
            .expect("recover")
            .len(),
        2
    );

    // Mutate a cloned file in m1 and verify Copy-on-Write isolation (m2 and template remain untouched).
    fs::write(
        m1.worktree_dir.join("pkg/mod_0/lib.rs"),
        "pub const VAL_0: usize = 999;\n",
    )
    .expect("mutate m1");
    assert_eq!(
        fs::read_to_string(m2.worktree_dir.join("pkg/mod_0/lib.rs")).expect("read m2"),
        "pub const VAL_0: usize = 0;\n"
    );
    assert_eq!(
        fs::read_to_string(template_dir.join("tree/pkg/mod_0/lib.rs")).expect("read template"),
        "pub const VAL_0: usize = 0;\n"
    );

    // Commit in m1 via host-side MCP commit plumbing and verify m1 advances while m2 remains clean at base.
    let prepared = engine
        .prepare_worktree_commit(
            m1.worktree.id,
            &m1.worktree.branch,
            &m1.worktree.base_commit,
            &[String::from("pkg/mod_0/lib.rs")],
            "feat: update mod_0 in cow worktree",
            "agent@company.internal",
        )
        .expect("prepare commit in m1");
    let commit_res = engine
        .finish_worktree_commit(prepared)
        .expect("commit in m1");
    assert_ne!(commit_res.commit, m1.worktree.base_commit);
    assert_eq!(
        engine
            .clean_worktree_commit(m1.worktree.id, &m1.worktree.branch)
            .expect("m1 clean after commit"),
        commit_res.commit
    );
    assert_eq!(
        engine
            .clean_worktree_commit(m2.worktree.id, &m2.worktree.branch)
            .expect("m2 still clean at base"),
        m2.worktree.base_commit
    );

    GitEngine::verify_git_fsck(&repo_dir).expect("repo fsck");
    GitEngine::verify_git_fsck(&m1.worktree_dir).expect("m1 fsck");
    GitEngine::verify_git_fsck(&m2.worktree_dir).expect("m2 fsck");
}

#[test]
#[cfg(unix)]
fn tree_with_symlink_or_gitattributes_falls_back_to_git_worktree_add_without_cow_template() {
    let tmp = TempDir::new().expect("tempdir");
    let repo_dir = tmp.path().join("repo");
    fs::create_dir_all(&repo_dir).expect("mkdir");
    git(&repo_dir, &["init", "-q", "-b", "main"]);
    fs::write(repo_dir.join("README.md"), "# hello\n").expect("write");
    std::os::unix::fs::symlink("README.md", repo_dir.join("README.link")).expect("symlink");
    git(&repo_dir, &["add", "."]);
    git(&repo_dir, &["commit", "-q", "-m", "with symlink"]);

    let engine = GitEngine::new(&repo_dir);
    let m = engine
        .materialize_agent_worktree(
            WorktreeId::new(),
            RepoId::new(),
            AgentKind::ClaudeCode,
            "agent/claude/symlink-fallback",
            "main",
            "fallback checkout",
        )
        .expect("materialize with symlink fallback");

    let template_dir = m
        .common_git_dir
        .join("gitust-cow-templates")
        .join(&m.worktree.base_commit.0);
    assert!(
        !template_dir.exists(),
        "trees containing symlinks must not materialize a CoW template inside common_git_dir"
    );
    assert!(fs::symlink_metadata(m.worktree_dir.join("README.link"))
        .expect("link meta")
        .file_type()
        .is_symlink());
    GitEngine::verify_git_fsck(&repo_dir).expect("repo fsck");
    GitEngine::verify_git_fsck(&m.worktree_dir).expect("worktree fsck");
}

#[test]
#[cfg(unix)]
fn cow_index_v2_modulo_8_padding_executable_modes_empty_blobs_and_incomplete_template_recovery() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = TempDir::new().expect("tempdir");
    let repo_dir = tmp.path().join("repo");
    fs::create_dir_all(&repo_dir).expect("mkdir");
    git(&repo_dir, &["init", "-q", "-b", "main"]);

    // 1. Create filenames covering every modulo-8 NUL-padding boundary (lengths 1..=9 and 63..=65)
    for len in [1usize, 2, 3, 4, 5, 6, 7, 8, 9, 63, 64, 65] {
        let name = "x".repeat(len);
        fs::write(repo_dir.join(&name), format!("len={len}\n")).expect("write");
    }
    // 2. 0-byte empty file
    fs::write(repo_dir.join("empty.bin"), b"").expect("write empty");
    // 3. Executable script (100755)
    let script_path = repo_dir.join("run.sh");
    fs::write(&script_path, "#!/bin/sh\necho ok\n").expect("write script");
    fs::set_permissions(&script_path, fs::Permissions::from_mode(0o755)).expect("chmod 755");
    // 4. Deep nested directory hierarchy
    let deep_dir = repo_dir.join("a/b/c/d/e");
    fs::create_dir_all(&deep_dir).expect("mkdir deep");
    fs::write(deep_dir.join("leaf.rs"), "pub fn leaf() {}\n").expect("write leaf");

    git(&repo_dir, &["add", "."]);
    git(&repo_dir, &["commit", "-q", "-m", "edge case tree"]);

    let engine = GitEngine::new(&repo_dir);
    let head_sha = gitust_core::CommitSha(
        engine
            .resolve_branch_oid("main")
            .expect("head sha")
            .expect("main exists"),
    );
    let common_git_dir = engine.repository_common_directory().expect("common dir");

    // 5. Simulate an interrupted/crashed previous CoW template directory (missing `ready` marker)
    let stale_template = common_git_dir
        .join("gitust-cow-templates")
        .join(&head_sha.0);
    fs::create_dir_all(stale_template.join("tree")).expect("mkdir stale");
    fs::write(
        stale_template.join("tree/CORRUPTED_LEAK.txt"),
        "stale crash debris",
    )
    .expect("write debris");
    fs::write(stale_template.join("index"), b"corrupted-index").expect("write bad index");

    let m = engine
        .materialize_agent_worktree(
            WorktreeId::new(),
            RepoId::new(),
            AgentKind::ClaudeCode,
            "agent/claude/edge-cow",
            "main",
            "edge case cow verification",
        )
        .expect("materialize after stale template recovery");

    assert!(
        stale_template.join("ready").is_file(),
        "stale template must be replaced with a valid ready template"
    );
    assert!(
        !m.worktree_dir.join("CORRUPTED_LEAK.txt").exists(),
        "crash debris from incomplete template must never appear in worktree"
    );
    assert_eq!(
        fs::metadata(m.worktree_dir.join("empty.bin"))
            .expect("empty meta")
            .len(),
        0
    );
    let exec_mode = fs::metadata(m.worktree_dir.join("run.sh"))
        .expect("run.sh meta")
        .permissions()
        .mode();
    assert_ne!(
        exec_mode & 0o111,
        0,
        "executable bit (100755) must be preserved across CoW clone"
    );
    assert_eq!(
        engine
            .clean_worktree_commit(m.worktree.id, &m.worktree.branch)
            .expect("clean commit check"),
        head_sha
    );
    GitEngine::verify_git_fsck(&repo_dir).expect("repo fsck");
    GitEngine::verify_git_fsck(&m.worktree_dir).expect("worktree fsck");

    // 6. Symlinked `gitust-cow-templates` root must fail closed
    let templates_root = common_git_dir.join("gitust-cow-templates");
    fs::remove_dir_all(&templates_root).expect("remove templates root");
    let external_dir = tmp.path().join("external-templates");
    fs::create_dir_all(&external_dir).expect("mkdir external");
    std::os::unix::fs::symlink(&external_dir, &templates_root).expect("symlink templates root");
    assert!(
        engine
            .materialize_agent_worktree(
                WorktreeId::new(),
                RepoId::new(),
                AgentKind::ClaudeCode,
                "agent/claude/symlink-templates-root",
                "main",
                "must fail closed on symlinked templates root",
            )
            .is_err(),
        "symlinked gitust-cow-templates root must be rejected fail-closed"
    );
}
