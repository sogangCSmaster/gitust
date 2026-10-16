use super::*;
use std::{fs, process::Command};

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
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

#[test]
fn failed_outbound_keeps_merge_and_explicit_sha_retry_publishes_only_approval() {
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    let remote = temp.path().join("temporarily-unavailable.git");
    fs::create_dir(&repo).unwrap();
    fs::create_dir(&remote).unwrap();
    git(&repo, &["init", "-b", "main"]);
    git(&repo, &["commit", "--allow-empty", "-m", "initial"]);
    git(&repo, &["checkout", "-b", "agent/test/approved"]);
    git(&repo, &["commit", "--allow-empty", "-m", "approved"]);
    let approved = git(&repo, &["rev-parse", "HEAD"]);
    git(&repo, &["checkout", "main"]);
    assert!(run_triage_approve(
        &repo,
        "agent/test/approved",
        "main",
        Some(remote.to_str().unwrap())
    )
    .is_err());
    assert_eq!(git(&repo, &["rev-parse", "main"]), approved);
    git(
        &repo,
        &["commit", "--allow-empty", "-m", "later local work"],
    );
    git(&remote, &["init", "--bare", "-b", "main"]);
    for _ in 0..2 {
        run_triage_publish(&repo, "main", &approved, remote.to_str().unwrap()).unwrap();
        assert_eq!(
            git(&remote, &["show-ref"]),
            format!("{approved} refs/heads/main")
        );
    }
    assert!(run_triage_publish(&repo, "main", "HEAD", remote.to_str().unwrap()).is_err());
    GitEngine::verify_git_fsck(&repo).unwrap();
    GitEngine::verify_git_fsck(&remote).unwrap();
}

#[test]
fn managed_approval_retry_restores_publication_record_across_restart() {
    use gitust_core::{AgentKind, WorktreeId, WorktreeStatus};
    let temp = tempfile::tempdir().unwrap();
    let repo = temp.path().join("repo");
    let remote = temp.path().join("temporarily-unavailable.git");
    fs::create_dir(&repo).unwrap();
    fs::create_dir(&remote).unwrap();
    git(&repo, &["init", "-b", "main"]);
    git(&repo, &["commit", "--allow-empty", "-m", "initial"]);
    let engine = GitEngine::new(&repo);
    let managed = engine
        .materialize_agent_worktree(
            WorktreeId::new(),
            RepoId::new(),
            AgentKind::ClaudeCode,
            "agent/test/managed-retry",
            "main",
            "publication retry".to_owned(),
        )
        .unwrap();
    git(
        &managed.worktree_dir,
        &["commit", "--allow-empty", "-m", "approved"],
    );
    let approved = git(&managed.worktree_dir, &["rev-parse", "HEAD"]);
    assert!(run_triage_approve(
        &repo,
        &managed.worktree.branch,
        "main",
        Some(remote.to_str().unwrap())
    )
    .is_err());
    git(
        &repo,
        &["commit", "--allow-empty", "-m", "later local work"],
    );
    git(&remote, &["init", "--bare", "-b", "main"]);
    run_triage_publish(&repo, "main", &approved, remote.to_str().unwrap()).unwrap();
    let recovered = GitEngine::new(&repo)
        .recover_agent_worktrees(RepoId::new())
        .unwrap();
    assert_eq!(recovered[0].worktree.status, WorktreeStatus::Merged);
    assert_eq!(recovered[0].worktree.base_commit.0, approved);
    let destination = GitEngine::validate_mirror_remote_url(remote.to_str().unwrap()).unwrap();
    assert!(recovered[0]
        .approval
        .as_ref()
        .unwrap()
        .published_to(&destination));
    assert_eq!(
        git(&remote, &["show-ref"]),
        format!("{approved} refs/heads/main")
    );
    GitEngine::verify_git_fsck(&repo).unwrap();
    GitEngine::verify_git_fsck(&remote).unwrap();
}
