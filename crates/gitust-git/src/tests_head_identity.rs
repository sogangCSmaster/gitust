use super::{git, init_repo};
use crate::{GitEngine, GitEngineError, RepositoryHead};
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
    let (branch, head) = engine.inspect_repository_head().unwrap().into_parts();
    assert_eq!(branch, "detached");
    assert_eq!(head.unwrap().0, expected);
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

#[test]
fn repository_head_reports_only_the_missing_default_ref_without_inventing_a_commit() {
    let temp = TempDir::new().unwrap();
    for bare in [false, true] {
        let path = temp.path().join(if bare { "bare" } else { "checkout" });
        fs::create_dir(&path).unwrap();
        let args = if bare {
            vec!["init", "-q", "--bare", "-b", "main"]
        } else {
            vec!["init", "-q", "-b", "main"]
        };
        git(&path, &args);
        let engine = GitEngine::new(&path);
        assert!(matches!(engine.inspect_repository_head().unwrap(),
            RepositoryHead::Unborn { branch_name } if branch_name == "main"));
        assert!(engine.inspect_head_identity().is_err());
        assert!(engine.inspect_head_zero_copy().is_err());
    }
    let path = temp.path().join("other-branch-has-history");
    init_repo(&path);
    let expected = git(&path, &["rev-parse", "HEAD"]);
    git(&path, &["symbolic-ref", "HEAD", "refs/heads/topic/$plain"]);
    let engine = GitEngine::new(&path);
    assert!(matches!(engine.inspect_repository_head().unwrap(),
        RepositoryHead::Unborn { branch_name } if branch_name == "topic/$plain"));
    assert_eq!(git(&path, &["rev-parse", "main"]), expected);
    assert!(engine.inspect_head_identity().is_err());
}

#[test]
fn repository_head_does_not_turn_corrupt_or_nonbranch_heads_into_unborn_state() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("repo");
    init_repo(&path);
    let common = path.join(".git");
    let blob = git(&path, &["rev-parse", "HEAD:src/lib.rs"]);
    for head in [
        "malformed\n".to_owned(),
        format!("{}\n", "0".repeat(40)),
        format!("{}\n", "a".repeat(40)),
        format!("{blob}\n"),
        "ref: refs/tags/no-commit\n".to_owned(),
        "ref: refs/heads/normal/\n".to_owned(),
    ] {
        fs::write(common.join("HEAD"), head).unwrap();
        assert!(GitEngine::new(&path).inspect_repository_head().is_err());
    }
    fs::write(common.join("HEAD"), "ref: refs/heads/alias\n").unwrap();
    fs::write(common.join("refs/heads/alias"), "ref: refs/heads/missing\n").unwrap();
    assert!(GitEngine::new(&path).inspect_repository_head().is_err());
    fs::write(
        common.join("refs/heads/alias"),
        format!("{}\n", "a".repeat(40)),
    )
    .unwrap();
    assert!(GitEngine::new(&path).inspect_repository_head().is_err());
}
