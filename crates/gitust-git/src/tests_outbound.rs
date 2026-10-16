use crate::{GitEngine, GitEngineError};
use gitust_core::CommitSha;
use std::{fs, path::Path, process::Command};

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.test")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.test")
        .output()
        .expect("git");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout)
        .expect("utf8")
        .trim()
        .to_owned()
}

#[test]
fn outbound_sends_exact_approved_commit_without_private_refs_or_config_rewrites() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let remote = temp.path().join("remote.git");
    let trap = temp.path().join("trap.git");
    for dir in [&source, &remote, &trap] {
        fs::create_dir(dir).unwrap();
    }
    git(&source, &["init", "-b", "main"]);
    git(&remote, &["init", "--bare", "-b", "main"]);
    git(&trap, &["init", "--bare", "-b", "main"]);
    git(&source, &["commit", "--allow-empty", "-m", "approved"]);
    let approved = CommitSha(git(&source, &["rev-parse", "HEAD"]));
    git(&source, &["tag", "-a", "private-tag", "-m", "private"]);
    git(&source, &["checkout", "-b", "agent/test/private"]);
    fs::write(source.join("secret.txt"), "private experiment").unwrap();
    git(&source, &["add", "."]);
    git(&source, &["commit", "-m", "not approved"]);
    let private = git(&source, &["rev-parse", "HEAD"]);
    git(&source, &["branch", "-f", "main", &private]);
    git(&source, &["config", "push.followTags", "true"]);
    git(
        &source,
        &[
            "config",
            &format!("url.{}.pushInsteadOf", trap.display()),
            remote.to_str().unwrap(),
        ],
    );
    let engine = GitEngine::new(&source);
    engine
        .push_approved_commit(remote.to_str().unwrap(), "main", &approved)
        .unwrap();
    assert_eq!(
        git(&remote, &["show-ref"]),
        format!("{} refs/heads/main", approved.0)
    );
    assert_eq!(git(&trap, &["for-each-ref"]), "");
    let private_object = Command::new("git")
        .arg("-C")
        .arg(&remote)
        .args(["cat-file", "-e", &private])
        .output()
        .unwrap();
    assert!(!private_object.status.success());
    // Retry is safe even if the acknowledgement of the first push was lost.
    engine
        .push_approved_commit(remote.to_str().unwrap(), "main", &approved)
        .unwrap();
    GitEngine::verify_git_fsck(&remote).unwrap();
    GitEngine::verify_git_fsck(&source).unwrap();

    for bad in [
        "agent/test/private",
        "Agent/test/private",
        "refs/heads/agent/test/private",
        "-x",
        "main:other",
    ] {
        assert!(engine
            .push_approved_commit(remote.to_str().unwrap(), bad, &approved)
            .is_err());
    }
    assert!(engine
        .push_approved_commit("ext::false", "main", &approved)
        .is_err());
    assert!(engine
        .push_approved_commit(remote.to_str().unwrap(), "main", &CommitSha("HEAD".into()))
        .is_err());
}

#[test]
fn outbound_rejects_non_fast_forward_without_overwriting_remote() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let remote = temp.path().join("remote.git");
    fs::create_dir(&source).unwrap();
    fs::create_dir(&remote).unwrap();
    git(&source, &["init", "-b", "main"]);
    git(&remote, &["init", "--bare", "-b", "main"]);
    git(&source, &["commit", "--allow-empty", "-m", "base"]);
    let base = git(&source, &["rev-parse", "HEAD"]);
    git(
        &source,
        &["commit", "--allow-empty", "-m", "remote advance"],
    );
    let advanced = CommitSha(git(&source, &["rev-parse", "HEAD"]));
    let engine = GitEngine::new(&source);
    engine
        .push_approved_commit(remote.to_str().unwrap(), "main", &advanced)
        .unwrap();
    git(&source, &["checkout", "--detach", &base]);
    git(&source, &["commit", "--allow-empty", "-m", "diverged"]);
    let diverged = CommitSha(git(&source, &["rev-parse", "HEAD"]));
    assert!(engine
        .push_approved_commit(remote.to_str().unwrap(), "main", &diverged)
        .is_err());
    git(&source, &["branch", "-f", "main", &diverged.0]);
    assert!(matches!(
        engine.push_approved_commit(remote.to_str().unwrap(), "main", &diverged),
        Err(GitEngineError::GitCommand { .. })
    ));
    assert_eq!(git(&remote, &["rev-parse", "main"]), advanced.0);
}
