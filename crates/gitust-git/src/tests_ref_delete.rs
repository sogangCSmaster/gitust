use super::*;
use crate::tests::tests_ref_concurrency::{bare_fixture, child_commit, Writer};

#[test]
fn delete_ref_cas_preserves_a_concurrent_branch_advance() {
    let (_temp, bare, base) = bare_fixture();
    let git = |args: &[&str]| {
        let output = crate::process::git_command(&bare)
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success());
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    };
    git(&["branch", "agent/test/delete-cas"]);
    git(&["pack-refs", "--all", "--prune"]);
    let competing = child_commit(&bare, &base, "concurrent branch writer");
    let repo = gix::open(&bare).unwrap();
    let writer = Writer::prepare(
        &bare,
        "refs/heads/agent/test/delete-cas",
        &competing,
        Some(&base),
    )
    .delayed();
    // Preserve the caller's observed expectation even if this thread is descheduled.
    let expected = gix::ObjectId::from_hex(base.as_bytes()).unwrap();
    let result = delete_branch_ref(
        &repo,
        "refs/heads/agent/test/delete-cas",
        PreviousValue::MustExistAndMatch(Target::Object(expected)),
    );
    writer.join().unwrap();
    assert!(result.is_err());
    assert_eq!(git(&["rev-parse", "agent/test/delete-cas"]), competing);
    GitEngine::verify_git_fsck(&bare).unwrap();
}
