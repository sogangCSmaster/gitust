use std::ffi::OsStr;
use std::fs;
use tempfile::TempDir;

use super::*;
use crate::process::run_git;

fn init_repo_with_main() -> (TempDir, GitEngine, CommitSha) {
    let dir = TempDir::new().expect("tempdir");
    run_git(
        dir.path(),
        [OsStr::new("init"), OsStr::new("-b"), OsStr::new("main")],
    )
    .expect("git init");
    run_git(
        dir.path(),
        [
            OsStr::new("config"),
            OsStr::new("user.name"),
            OsStr::new("Test User"),
        ],
    )
    .expect("git config name");
    run_git(
        dir.path(),
        [
            OsStr::new("config"),
            OsStr::new("user.email"),
            OsStr::new("test@company.internal"),
        ],
    )
    .expect("git config email");
    fs::write(dir.path().join("README.md"), "line 1\n").expect("write readme");
    run_git(dir.path(), [OsStr::new("add"), OsStr::new("README.md")]).expect("git add");
    run_git(
        dir.path(),
        [OsStr::new("commit"), OsStr::new("-m"), OsStr::new("init")],
    )
    .expect("git commit");
    let engine = GitEngine::new(dir.path());
    let head = engine.inspect_head_identity().expect("head").head_sha;
    (dir, engine, head)
}

#[test]
fn preview_detects_clean_merge_and_conflicting_files_and_gated_merge_updates_base() {
    let (dir, engine, base_sha) = init_repo_with_main();
    run_git(
        dir.path(),
        [
            OsStr::new("checkout"),
            OsStr::new("-b"),
            OsStr::new("feature"),
        ],
    )
    .expect("checkout feature");
    fs::write(dir.path().join("feature.txt"), "hello\n").expect("write feature");
    run_git(dir.path(), [OsStr::new("add"), OsStr::new("feature.txt")]).expect("add feature");
    run_git(
        dir.path(),
        [
            OsStr::new("commit"),
            OsStr::new("-m"),
            OsStr::new("feature commit"),
        ],
    )
    .expect("commit feature");
    let head_sha = engine
        .inspect_head_identity()
        .expect("feature sha")
        .head_sha;
    run_git(dir.path(), [OsStr::new("checkout"), OsStr::new("main")]).expect("checkout main");

    let preview = engine
        .preview_branch_merge("main", "feature")
        .expect("preview should succeed");
    assert!(preview.can_merge);
    assert!(preview.fast_forward);
    assert!(preview.conflicting_files.is_empty());
    assert_eq!(preview.base_sha, base_sha);
    assert_eq!(preview.head_sha, head_sha);

    let merged_sha = engine
        .merge_branches_with_gate(
            "main",
            "feature",
            &base_sha,
            &head_sha,
            "Merge PR #1",
            "Maintainer",
            "maint@company.internal",
            || Ok(()),
        )
        .expect("gated merge should succeed");
    assert_ne!(merged_sha, base_sha);
    let new_main = engine
        .resolve_checks_ref("refs/heads/main")
        .expect("resolve main");
    assert_eq!(new_main, merged_sha);
    assert!(dir.path().join("feature.txt").exists());
}

#[test]
fn preview_reports_conflicting_files_and_merge_rejects_conflict() {
    let (dir, engine, base_sha) = init_repo_with_main();
    run_git(
        dir.path(),
        [
            OsStr::new("checkout"),
            OsStr::new("-b"),
            OsStr::new("conflict-branch"),
        ],
    )
    .expect("checkout conflict-branch");
    fs::write(dir.path().join("README.md"), "conflict branch change\n").expect("write");
    run_git(dir.path(), [OsStr::new("add"), OsStr::new("README.md")]).expect("add");
    run_git(
        dir.path(),
        [
            OsStr::new("commit"),
            OsStr::new("-m"),
            OsStr::new("branch edit"),
        ],
    )
    .expect("commit");
    let head_sha = engine.inspect_head_identity().expect("head").head_sha;

    run_git(dir.path(), [OsStr::new("checkout"), OsStr::new("main")]).expect("checkout main");
    fs::write(dir.path().join("README.md"), "main branch change\n").expect("write");
    run_git(dir.path(), [OsStr::new("add"), OsStr::new("README.md")]).expect("add");
    run_git(
        dir.path(),
        [
            OsStr::new("commit"),
            OsStr::new("-m"),
            OsStr::new("main edit"),
        ],
    )
    .expect("commit");
    let new_base_sha = engine.inspect_head_identity().expect("head").head_sha;
    assert_ne!(new_base_sha, base_sha);

    let preview = engine
        .preview_branch_merge("main", "conflict-branch")
        .expect("preview");
    assert!(!preview.can_merge);
    assert!(!preview.fast_forward);
    assert_eq!(preview.conflicting_files, vec!["README.md".to_owned()]);

    let err = engine
        .merge_branches_with_gate(
            "main",
            "conflict-branch",
            &new_base_sha,
            &head_sha,
            "Merge conflict",
            "Maintainer",
            "maint@company.internal",
            || Ok(()),
        )
        .expect_err("merge should fail on conflict");
    assert!(matches!(err, GitEngineError::MergeConflict(_)));
}
