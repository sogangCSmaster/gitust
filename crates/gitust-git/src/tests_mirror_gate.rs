//! Gate failure is an error, including after slow main-worktree inspection and partial phases.
use super::{git, init_repo};
use crate::{GitEngine, GitEngineError};
use std::time::Instant;
fn denied() -> GitEngineError {
    GitEngineError::VerificationInvalid("synthetic authority revoked".into())
}
#[test]
fn mirror_gate_denial_after_main_status_never_merges_the_worktree() {
    let temp = tempfile::tempdir().unwrap();
    let primary = temp.path().join("primary");
    init_repo(&primary);
    let upstream = temp.path().join("upstream");
    git(
        temp.path(),
        &[
            "clone",
            "--no-local",
            primary.to_str().unwrap(),
            upstream.to_str().unwrap(),
        ],
    );
    let before = git(&primary, &["rev-parse", "HEAD"]);
    let contents = std::fs::read(primary.join("src/lib.rs")).unwrap();
    std::fs::write(upstream.join("src/lib.rs"), "pub fn changed() {}\n").unwrap();
    git(&upstream, &["commit", "-qam", "advance main"]);
    let engine = GitEngine::new(&primary);
    engine
        .fetch_inbound_mirror_staging(upstream.to_str().unwrap())
        .unwrap();
    let mut gates = 0;
    assert!(engine
        .apply_inbound_mirror_refs_with_gate(upstream.to_str().unwrap(), Instant::now(), || {
            gates += 1;
            if gates == 2 {
                Err(denied())
            } else {
                Ok(())
            }
        })
        .is_err());
    assert_eq!(gates, 2);
    assert_eq!(git(&primary, &["rev-parse", "HEAD"]), before);
    assert_eq!(std::fs::read(primary.join("src/lib.rs")).unwrap(), contents);
    engine
        .apply_inbound_mirror_refs(upstream.to_str().unwrap(), Instant::now())
        .unwrap();
    assert_eq!(
        git(&primary, &["rev-parse", "HEAD"]),
        git(&upstream, &["rev-parse", "HEAD"])
    );
    git(&primary, &["fsck", "--full"]);
}
#[test]
fn mirror_gate_denial_before_batched_heads_and_tags_is_not_a_skipped_success() {
    let temp = tempfile::tempdir().unwrap();
    let upstream = temp.path().join("upstream");
    init_repo(&upstream);
    let bare = temp.path().join("bare");
    let engine = GitEngine::new(&bare);
    engine
        .sync_inbound_read_only(upstream.to_str().unwrap())
        .unwrap();
    let before = git(
        &bare,
        &[
            "for-each-ref",
            "--format=%(refname) %(objectname)",
            "refs/heads",
            "refs/tags",
        ],
    );
    git(&upstream, &["branch", "new-public"]);
    git(&upstream, &["tag", "-a", "new-tag", "-m", "annotated"]);
    engine
        .fetch_inbound_mirror_staging(upstream.to_str().unwrap())
        .unwrap();
    let mut gates = 0;
    assert!(engine
        .apply_inbound_mirror_refs_with_gate(upstream.to_str().unwrap(), Instant::now(), || {
            gates += 1;
            if gates == 2 {
                Err(denied())
            } else {
                Ok(())
            }
        })
        .is_err());
    assert_eq!(gates, 2);
    assert_eq!(
        git(
            &bare,
            &[
                "for-each-ref",
                "--format=%(refname) %(objectname)",
                "refs/heads",
                "refs/tags"
            ]
        ),
        before
    );
    engine
        .apply_inbound_mirror_refs(upstream.to_str().unwrap(), Instant::now())
        .unwrap();
    assert_eq!(
        git(&bare, &["rev-parse", "refs/tags/new-tag"]),
        git(&upstream, &["rev-parse", "refs/tags/new-tag"])
    );
    git(&bare, &["fsck", "--full"]);
}
#[test]
fn late_head_gate_denial_propagates_without_reversing_earlier_authorized_refs() {
    let temp = tempfile::tempdir().unwrap();
    let upstream = temp.path().join("upstream");
    init_repo(&upstream);
    let bare = temp.path().join("bare");
    let engine = GitEngine::new(&bare);
    engine
        .fetch_inbound_mirror_staging(upstream.to_str().unwrap())
        .unwrap();
    git(&bare, &["symbolic-ref", "HEAD", "refs/heads/unborn"]);
    let mut gates = 0;
    let result = engine.apply_inbound_mirror_refs_with_gate(
        upstream.to_str().unwrap(),
        Instant::now(),
        || {
            gates += 1;
            if gates == 4 {
                Err(denied())
            } else {
                Ok(())
            }
        },
    );
    assert!(result.is_err());
    assert_eq!(gates, 4);
    assert_eq!(git(&bare, &["symbolic-ref", "HEAD"]), "refs/heads/unborn");
    // The batch was authorized before the later HEAD gate failed; it is not rolled back.
    assert_eq!(
        git(&bare, &["rev-parse", "refs/heads/main"]),
        git(&upstream, &["rev-parse", "HEAD"])
    );
    git(&bare, &["fsck", "--full"]);
}
