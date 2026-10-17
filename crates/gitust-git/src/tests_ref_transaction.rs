//! Ordinary ref publication, denial, and cleanup without concurrent writers.
use super::{git, init_repo};
use crate::{ensure_reflog_identity, ref_transaction as tx, GitEngine, GitEngineError};
use gix::refs::{
    transaction::{Change, LogChange, PreviousValue, RefEdit, RefLog},
    Target,
};
use std::{fs, path::PathBuf};
use tempfile::TempDir;

fn fixture() -> (
    TempDir,
    PathBuf,
    gix::Repository,
    gix::ObjectId,
    gix::ObjectId,
) {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("repo");
    init_repo(&path);
    git(&path, &["config", "user.name", "Gitust Test"]);
    git(&path, &["config", "user.email", "test@company.internal"]);
    let first = gix::ObjectId::from_hex(git(&path, &["rev-parse", "HEAD"]).as_bytes()).unwrap();
    git(&path, &["commit", "--allow-empty", "-qm", "second"]);
    let second = gix::ObjectId::from_hex(git(&path, &["rev-parse", "HEAD"]).as_bytes()).unwrap();
    let mut repo = gix::open(&path).unwrap();
    ensure_reflog_identity(&mut repo).unwrap();
    (temp, path, repo, first, second)
}

fn matched(oid: gix::ObjectId) -> PreviousValue {
    PreviousValue::MustExistAndMatch(Target::Object(oid))
}

fn edit(name: &str, target: gix::ObjectId, expected: PreviousValue) -> RefEdit {
    RefEdit {
        name: name.try_into().unwrap(),
        deref: false,
        change: Change::Update {
            expected,
            new: Target::Object(target),
            log: LogChange {
                mode: RefLog::AndReference,
                force_create_reflog: false,
                message: "gitust test: publish references".into(),
            },
        },
    }
}

fn delete(name: &str, old: gix::ObjectId) -> RefEdit {
    RefEdit {
        name: name.try_into().unwrap(),
        deref: false,
        change: Change::Delete {
            expected: matched(old),
            log: RefLog::AndReference,
        },
    }
}

fn refs(path: &std::path::Path) -> String {
    git(path, &["for-each-ref", "--format=%(refname) %(objectname)"])
}

#[test]
fn prepared_transaction_preserves_packed_loose_tag_and_origin_semantics() {
    let (_temp, path, repo, first, second) = fixture();
    let first_hex = first.to_string();
    let second_hex = second.to_string();
    for name in ["packed-update", "packed-delete"] {
        git(&path, &["branch", name, &first_hex]);
    }
    git(&path, &["pack-refs", "--all", "--prune"]);
    assert!(!path.join(".git/refs/heads/packed-update").exists());
    assert!(!path.join(".git/refs/heads/packed-delete").exists());
    for name in ["loose-update", "loose-delete"] {
        git(&path, &["branch", name, &first_hex]);
        assert!(path.join(".git/refs/heads").join(name).is_file());
    }
    git(
        &path,
        &["tag", "-a", "original", "-m", "annotation", &first_hex],
    );
    let tag_hex = git(&path, &["rev-parse", "refs/tags/original"]);
    let tag = gix::ObjectId::from_hex(tag_hex.as_bytes()).unwrap();
    assert_eq!(git(&path, &["cat-file", "-t", &tag_hex]), "tag");

    tx::apply(
        &repo,
        vec![
            edit("refs/heads/packed-update", second, matched(first)),
            edit("refs/heads/loose-update", second, matched(first)),
            edit("refs/heads/created", first, PreviousValue::MustNotExist),
            edit("refs/tags/copied", tag, PreviousValue::MustNotExist),
            delete("refs/heads/packed-delete", first),
            delete("refs/heads/loose-delete", first),
        ],
    )
    .unwrap();
    for name in ["packed-update", "loose-update"] {
        assert_eq!(git(&path, &["rev-parse", name]), second_hex);
    }
    assert_eq!(git(&path, &["rev-parse", "created"]), first_hex);
    assert_eq!(git(&path, &["rev-parse", "refs/tags/copied"]), tag_hex);
    assert_eq!(git(&path, &["cat-file", "-t", "refs/tags/copied"]), "tag");
    assert!(git(
        &path,
        &[
            "for-each-ref",
            "--format=%(refname)",
            "refs/heads/packed-delete",
            "refs/heads/loose-delete",
        ],
    )
    .is_empty());
    assert!(!path.join(".git/logs/refs/heads/loose-delete").exists());

    let materialized =
        super::materialize(&GitEngine::new(&path), "agent/test/native-origin").unwrap();
    let log =
        fs::read_to_string(path.join(".git/logs/refs/heads/agent/test/native-origin")).unwrap();
    let (metadata, message) = log.lines().next().unwrap().split_once('\t').unwrap();
    let mut metadata = metadata.split_whitespace();
    let zero = "0".repeat(second_hex.len());
    assert_eq!(metadata.next(), Some(zero.as_str()));
    assert_eq!(metadata.next(), Some(second_hex.as_str()));
    assert_eq!(message, "gitust: create agent branch from main");
    assert_eq!(materialized.worktree.base_commit.0, second_hex);
    GitEngine::verify_git_fsck(&path).unwrap();
}

#[test]
fn prepared_gate_refusal_releases_locks_without_changing_ref_or_reflog() {
    let (_temp, path, repo, first, second) = fixture();
    let name = "refs/heads/agent/test/gated";
    git(
        &path,
        &["update-ref", "--create-reflog", name, &first.to_string()],
    );
    let reference = path.join(".git").join(name);
    let lock = path.join(".git").join(format!("{name}.lock"));
    let log = path.join(".git/logs").join(name);
    let before_ref = fs::read(&reference).unwrap();
    let before_log = fs::read(&log).unwrap();
    let mut checked = false;
    let result = tx::apply_with_gate(&repo, vec![edit(name, second, matched(first))], || {
        checked = true;
        assert!(
            lock.is_file(),
            "publication gate must run under the native ref lock"
        );
        assert_eq!(git(&path, &["rev-parse", name]), first.to_string());
        Err(GitEngineError::VerificationInvalid(
            "synthetic gate refusal".into(),
        ))
    });
    assert!(checked);
    assert!(
        matches!(result, Err(GitEngineError::VerificationInvalid(message)) if message == "synthetic gate refusal")
    );
    assert_eq!(fs::read(&reference).unwrap(), before_ref);
    assert_eq!(fs::read(&log).unwrap(), before_log);
    assert!(!lock.exists());
    assert!(!path.join(".git/packed-refs.lock").exists());

    tx::apply(&repo, vec![edit(name, second, matched(first))]).unwrap();
    assert_eq!(git(&path, &["rev-parse", name]), second.to_string());
    assert!(!lock.exists());
    GitEngine::verify_git_fsck(&path).unwrap();
}

#[test]
fn direct_and_dangling_symbolic_refs_are_not_replaced_by_object_updates() {
    let (_temp, path, repo, first, second) = fixture();
    let direct = "refs/heads/direct-alias";
    let dangling = "refs/heads/dangling-alias";
    git(&path, &["symbolic-ref", direct, "refs/heads/main"]);
    git(&path, &["symbolic-ref", dangling, "refs/heads/absent"]);
    for (name, expected, target) in [
        (direct, matched(second), "refs/heads/main"),
        (dangling, PreviousValue::MustNotExist, "refs/heads/absent"),
    ] {
        let reference = path.join(".git").join(name);
        let before = fs::read(&reference).unwrap();
        let mut checked = false;
        assert!(
            tx::apply_with_gate(&repo, vec![edit(name, first, expected)], || {
                checked = true;
                Ok(())
            })
            .is_err()
        );
        assert!(
            !checked,
            "a symbolic ref must be rejected before publication authority is checked"
        );
        assert_eq!(fs::read(&reference).unwrap(), before);
        assert_eq!(git(&path, &["symbolic-ref", "--no-recurse", name]), target);
        assert!(!path.join(".git").join(format!("{name}.lock")).exists());
    }
    assert_eq!(git(&path, &["rev-parse", "main"]), second.to_string());
    assert!(!path.join(".git/refs/heads/main.lock").exists());
    tx::apply(
        &repo,
        vec![edit(
            "refs/heads/after-alias-denial",
            first,
            PreviousValue::MustNotExist,
        )],
    )
    .unwrap();
    assert_eq!(
        git(&path, &["rev-parse", "after-alias-denial"]),
        first.to_string()
    );
}

#[test]
fn invalid_conflicting_and_oversized_batches_never_publish_a_partial_result() {
    let (_temp, path, repo, first, second) = fixture();
    let name = "refs/heads/kept";
    git(&path, &["branch", "kept", &first.to_string()]);
    let before = refs(&path);
    let invalid = vec![
        edit(name, second, matched(first)),
        edit(
            "refs/remotes/unsupported",
            first,
            PreviousValue::MustNotExist,
        ),
    ];
    let conflict = vec![
        edit(
            "refs/heads/would-create",
            first,
            PreviousValue::MustNotExist,
        ),
        edit(name, first, matched(second)),
    ];
    let mut oversized = Vec::with_capacity(100_001);
    oversized.push(edit(name, second, matched(first)));
    oversized.extend((0..100_000).map(|index| {
        edit(
            &format!("refs/heads/cap/{index}"),
            first,
            PreviousValue::MustNotExist,
        )
    }));
    for edits in [invalid, conflict, oversized] {
        let mut checked = false;
        assert!(tx::apply_with_gate(&repo, edits, || {
            checked = true;
            Ok(())
        })
        .is_err());
        assert!(!checked);
        assert_eq!(refs(&path), before);
        assert!(!path.join(".git/refs/heads/kept.lock").exists());
        assert!(!path.join(".git/refs/heads/would-create.lock").exists());
        assert!(!path.join(".git/packed-refs.lock").exists());
    }
    GitEngine::verify_git_fsck(&path).unwrap();
}
