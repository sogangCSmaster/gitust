//! Publication authority is checked after approval validation and before any merge.
use std::{fs, path::Path};

use gitust_core::{AgentKind, CommitSha, RepoId, WorktreeId};

use crate::{GitEngine, GitEngineError};

fn git(path: &Path, args: &[&str]) -> String {
    let output = crate::process::git_command(path)
        .args(["-c", "commit.gpgsign=false", "-c", "tag.gpgsign=false"])
        .args(args)
        .env("GIT_AUTHOR_NAME", "Gitust Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.test")
        .env("GIT_COMMITTER_NAME", "Gitust Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.test")
        .output()
        .unwrap();
    assert!(output.status.success(), "synthetic Git command failed");
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

#[test]
fn approval_gate_denial_preserves_bare_and_checked_out_targets_and_can_retry() {
    for bare in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        fs::create_dir(&source).unwrap();
        git(&source, &["init", "-q", "-b", "main"]);
        fs::write(source.join("file.txt"), "original\n").unwrap();
        git(&source, &["add", "."]);
        git(&source, &["commit", "-qm", "initial"]);
        let path = if bare {
            let path = temp.path().join("bare.git");
            git(
                temp.path(),
                &[
                    "clone",
                    "--bare",
                    source.to_str().unwrap(),
                    path.to_str().unwrap(),
                ],
            );
            path
        } else {
            source
        };
        let before = git(&path, &["rev-parse", "refs/heads/main"]);
        let engine = GitEngine::new(&path);
        let managed = engine
            .materialize_agent_worktree(
                WorktreeId::new(),
                RepoId::new(),
                AgentKind::ClaudeCode,
                "agent/test/approval-gate",
                "main",
                "approval publication gate",
            )
            .unwrap();
        fs::write(managed.worktree_dir.join("file.txt"), "reviewed\n").unwrap();
        git(&managed.worktree_dir, &["add", "."]);
        git(&managed.worktree_dir, &["commit", "-qm", "reviewed"]);
        let reviewed = CommitSha(git(&managed.worktree_dir, &["rev-parse", "HEAD"]));
        let index = fs::read(managed.worktree_admin_dir.join("index")).unwrap();
        let common = if bare {
            path.clone()
        } else {
            path.join(".git")
        };
        let mut checked = false;
        let denied = engine.approve_verified_worktree_with_gate(
            managed.worktree.id,
            &managed.worktree.branch,
            "main",
            &reviewed,
            || {
                checked = true;
                assert_eq!(git(&path, &["rev-parse", "refs/heads/main"]), before);
                if bare {
                    assert!(common.join("refs/heads/main.lock").is_file());
                }
                Err(GitEngineError::VerificationInvalid(
                    "synthetic approval authority revoked".into(),
                ))
            },
        );
        assert!(checked);
        assert!(matches!(
            denied,
            Err(GitEngineError::VerificationInvalid(_))
        ));
        assert_eq!(git(&path, &["rev-parse", "refs/heads/main"]), before);
        assert_eq!(
            git(&path, &["rev-parse", &managed.worktree.branch]),
            reviewed.0
        );
        assert_eq!(
            fs::read(managed.worktree_admin_dir.join("index")).unwrap(),
            index
        );
        assert!(engine
            .load_worktree_approval(managed.worktree.id)
            .unwrap()
            .is_none());
        assert!(!common.join("refs/heads/main.lock").exists());
        if !bare {
            assert_eq!(fs::read(path.join("file.txt")).unwrap(), b"original\n");
            assert!(git(&path, &["status", "--porcelain"]).is_empty());
        }
        let mut accepted = false;
        let approved = engine
            .approve_verified_worktree_with_gate(
                managed.worktree.id,
                &managed.worktree.branch,
                "main",
                &reviewed,
                || {
                    accepted = true;
                    Ok(())
                },
            )
            .unwrap();
        assert!(accepted);
        assert_eq!(approved, reviewed);
        assert_eq!(git(&path, &["rev-parse", "refs/heads/main"]), reviewed.0);
        assert_eq!(
            engine
                .load_worktree_approval(managed.worktree.id)
                .unwrap()
                .unwrap()
                .approved_commit,
            reviewed
        );
        if !bare {
            assert_eq!(fs::read(path.join("file.txt")).unwrap(), b"reviewed\n");
        }
        GitEngine::verify_git_fsck(&path).unwrap();
        GitEngine::verify_git_fsck(&managed.worktree_dir).unwrap();
    }
}
