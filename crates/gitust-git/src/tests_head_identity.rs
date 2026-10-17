use super::{git, init_repo};
use crate::{GitEngine, GitEngineError};
use std::fs;
use tempfile::TempDir;

#[test]
fn head_identity_matches_git_for_attached_and_detached_commits() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().join("repo");
    init_repo(&repo);
    let engine = GitEngine::new(&repo);
    let expected = git(&repo, &["rev-parse", "HEAD"]);
    let identity = engine.inspect_head_identity().unwrap();
    assert_eq!(identity.head_sha.0, expected);
    assert_eq!(
        identity.branch_name,
        git(&repo, &["branch", "--show-current"])
    );
    let snapshot = engine.inspect_head_zero_copy().unwrap();
    assert_eq!(snapshot.head_sha, identity.head_sha);
    assert_eq!(snapshot.branch_name, identity.branch_name);
    git(&repo, &["checkout", "-q", "--detach", "HEAD"]);
    let identity = engine.inspect_head_identity().unwrap();
    assert_eq!(identity.head_sha.0, expected);
    assert_eq!(identity.branch_name, "detached");
}

#[test]
fn head_identity_does_not_read_the_root_tree_and_rejects_unborn_heads() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().join("repo");
    init_repo(&repo);
    let expected = git(&repo, &["rev-parse", "HEAD"]);
    let tree = git(&repo, &["rev-parse", "HEAD^{tree}"]);
    fs::remove_file(repo.join(".git/objects").join(&tree[..2]).join(&tree[2..])).unwrap();
    let engine = GitEngine::new(&repo);
    assert_eq!(engine.inspect_head_identity().unwrap().head_sha.0, expected);
    assert!(matches!(
        engine.inspect_head_zero_copy(),
        Err(GitEngineError::TreeTraversal(_))
    ));

    let unborn = tmp.path().join("unborn");
    fs::create_dir(&unborn).unwrap();
    git(&unborn, &["init", "-q", "-b", "main"]);
    assert!(matches!(
        GitEngine::new(&unborn).inspect_head_identity(),
        Err(GitEngineError::HeadResolution(_))
    ));
}
