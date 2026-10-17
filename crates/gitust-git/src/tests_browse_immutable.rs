use std::fs;
use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

use super::{git, init_repo};
use crate::{DiffLineKind, GitEngine, GitEngineError};

fn commit_content(repo: &Path, value: u32, extra: &str) -> String {
    fs::write(
        repo.join("src/lib.rs"),
        format!("pub fn answer() -> u32 {{ {value} }}\n"),
    )
    .unwrap();
    fs::write(repo.join(extra), format!("{value}\n")).unwrap();
    git(repo, &["add", "."]);
    git(repo, &["commit", "-q", "-m", extra]);
    git(repo, &["rev-parse", "--verify", "HEAD"])
}

fn assert_missing_object(repo: &Path, oid: &str) {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["cat-file", "-e", oid])
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "object unexpectedly exists: {oid}"
    );
}

#[test]
fn immutable_full_oid_is_not_replaced_by_same_named_branch_or_tag() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().join("repo");
    init_repo(&repo);
    let base = git(&repo, &["rev-parse", "HEAD"]);
    let reviewed = commit_content(&repo, 43, "reviewed.txt");
    let newer = commit_content(&repo, 44, "newer.txt");
    let branch = format!("refs/heads/{reviewed}");
    let tag = format!("refs/tags/{reviewed}");
    git(&repo, &["update-ref", &branch, &newer]);
    git(&repo, &["update-ref", &tag, &base]);
    git(
        &repo,
        &["update-ref", &format!("refs/heads/{branch}"), &base],
    );
    git(&repo, &["update-ref", &format!("refs/heads/{tag}"), &newer]);

    assert_eq!(git(&repo, &["cat-file", "-t", &reviewed]), "commit");
    assert_eq!(git(&repo, &["rev-parse", "--verify", &reviewed]), reviewed);
    assert_eq!(git(&repo, &["rev-parse", "--verify", &branch]), newer);
    assert_eq!(git(&repo, &["rev-parse", "--verify", &tag]), base);
    let expected_blob = git(
        &repo,
        &["cat-file", "blob", &format!("{reviewed}:src/lib.rs")],
    );
    let engine = GitEngine::new(&repo);
    for oid in [&reviewed, &reviewed.to_uppercase()] {
        assert_eq!(
            engine.list_commits(Some(oid), 1).unwrap().head_sha.0,
            reviewed
        );
        let tree = engine.inspect_tree(Some(oid), None, true).unwrap();
        assert_eq!(tree.commit_sha.0, reviewed);
        assert!(tree
            .entries
            .iter()
            .any(|entry| entry.path == "reviewed.txt"));
        assert!(!tree.entries.iter().any(|entry| entry.path == "newer.txt"));
        let blob = engine.read_blob(Some(oid), "src/lib.rs").unwrap();
        assert_eq!(blob.commit_sha.0, reviewed);
        assert_eq!(blob.content.unwrap().trim(), expected_blob);
        let diff = engine
            .inspect_diff(Some(&base), Some(oid), Some("src/lib.rs"))
            .unwrap();
        assert_eq!(
            (diff.base_sha.0, diff.head_sha.0),
            (base.clone(), reviewed.clone())
        );
        assert!(diff
            .lines
            .iter()
            .any(|line| { line.kind == DiffLineKind::Add && line.content == expected_blob }));
    }

    // A fully qualified ref remains an explicit request for mutable reference lookup.
    assert_eq!(
        engine.list_commits(Some(&branch), 1).unwrap().head_sha.0,
        newer
    );
    assert_eq!(engine.list_commits(Some(&tag), 1).unwrap().head_sha.0, base);
    git(&repo, &["update-ref", "-d", &branch]);
    assert!(matches!(
        engine.list_commits(Some(&branch), 1),
        Err(GitEngineError::RevisionNotFound(_))
    ));
    assert_eq!(
        engine.list_commits(Some(&reviewed), 1).unwrap().head_sha.0,
        reviewed
    );
    let diff = engine
        .inspect_diff(Some(&reviewed), Some(&newer), Some("src/lib.rs"))
        .unwrap();
    assert_eq!((diff.base_sha.0, diff.head_sha.0), (reviewed, newer));
}

#[test]
fn immutable_missing_or_non_commit_oid_never_falls_back_to_named_reference() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().join("repo");
    init_repo(&repo);
    let head = git(&repo, &["rev-parse", "HEAD"]);
    let missing = "1111111111111111111111111111111111111111";
    assert_missing_object(&repo, missing);
    let blob = git(&repo, &["rev-parse", "HEAD:src/lib.rs"]);
    assert_eq!(git(&repo, &["cat-file", "-t", &blob]), "blob");
    let engine = GitEngine::new(&repo);
    for anchor in [missing, &blob] {
        for namespace in ["heads", "tags"] {
            let reference = format!("refs/{namespace}/{anchor}");
            git(&repo, &["update-ref", &reference, &head]);
            assert_eq!(git(&repo, &["rev-parse", &reference]), head);
            assert!(matches!(
                engine.list_commits(Some(anchor), 1),
                Err(GitEngineError::RevisionNotFound(_))
            ));
            assert!(matches!(
                engine.inspect_tree(Some(anchor), None, false),
                Err(GitEngineError::RevisionNotFound(_))
            ));
            assert!(matches!(
                engine.read_blob(Some(anchor), "src/lib.rs"),
                Err(GitEngineError::RevisionNotFound(_))
            ));
            assert!(matches!(
                engine.inspect_diff(None, Some(anchor), None),
                Err(GitEngineError::RevisionNotFound(_))
            ));
            assert!(matches!(
                engine.inspect_diff(Some(anchor), None, None),
                Err(GitEngineError::RevisionNotFound(_))
            ));
            assert_eq!(
                engine.list_commits(Some(&reference), 1).unwrap().head_sha.0,
                head
            );
            git(&repo, &["update-ref", "-d", &reference]);
        }
    }
    assert_missing_object(&repo, missing);
}

#[test]
fn immutable_oid_ancestry_and_existing_named_revision_semantics_are_preserved() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().join("repo");
    init_repo(&repo);
    let base = git(&repo, &["rev-parse", "HEAD"]);
    let middle = commit_content(&repo, 43, "middle.txt");
    let head = commit_content(&repo, 44, "head.txt");
    git(&repo, &["branch", &head, &base]);
    git(&repo, &["branch", &head[..7], &middle]);
    git(&repo, &["tag", "-a", "release", "-m", "release", &middle]);
    let engine = GitEngine::new(&repo);
    for (revision, expected) in [
        (None, &head),
        (Some("HEAD"), &head),
        (Some("main"), &head),
        (Some("main^"), &middle),
        (Some("release"), &middle),
        (Some(&base[..7]), &base),
        // Short hexadecimal names retain the existing branch-first lookup contract.
        (Some(&head[..7]), &middle),
        (Some(&format!("{head}~1")), &middle),
        (Some(&format!("{head}^")), &middle),
        (Some(&format!("{head}^0")), &head),
        (Some(&format!("{head}~2")), &base),
        (Some(&format!("refs/heads/{head}^0")), &base),
    ] {
        let resolved = engine.list_commits(revision, 1).unwrap();
        assert_eq!(resolved.head_sha.0, *expected, "revision: {revision:?}");
    }
    let git_parent = git(&repo, &["rev-parse", &format!("{head}^~1")]);
    assert_eq!(git_parent, base);
    assert_eq!(
        engine
            .list_commits(Some(&format!("{head}^~1")), 1)
            .unwrap()
            .head_sha
            .0,
        git_parent
    );
}
