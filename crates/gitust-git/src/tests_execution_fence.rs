use std::{fs, path::Path, process::Command};

use gitust_core::{AgentKind, RepoId, WorktreeId};
use tempfile::TempDir;

use crate::{GitEngine, GitEngineError, MaterializedWorktree};

fn git(path: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Fence Test")
        .env("GIT_AUTHOR_EMAIL", "fence@company.internal")
        .env("GIT_COMMITTER_NAME", "Fence Test")
        .env("GIT_COMMITTER_EMAIL", "fence@company.internal")
        .output()
        .unwrap();
    assert!(output.status.success(), "git command failed");
    String::from_utf8(output.stdout).unwrap().trim().into()
}

fn fixture() -> (TempDir, GitEngine, MaterializedWorktree) {
    let temp = TempDir::new().unwrap();
    let repo = temp.path().join("repo");
    fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    fs::write(repo.join("README.md"), "fixture\n").unwrap();
    git(&repo, &["add", "README.md"]);
    git(&repo, &["commit", "-q", "-m", "fixture"]);
    let engine = GitEngine::new(&repo);
    let worktree = engine
        .materialize_agent_worktree(
            WorktreeId::new(),
            RepoId::new(),
            AgentKind::CodexSwarm,
            "agent/test/fence",
            "main",
            "fence test",
        )
        .unwrap();
    (temp, engine, worktree)
}

fn markers(worktree: &MaterializedWorktree) -> Vec<std::path::PathBuf> {
    fs::read_dir(worktree.common_git_dir.join("gitust-active-executions"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect()
}

#[test]
fn unresolved_execution_quarantines_cached_mutations_and_blocks_a_fresh_owner() {
    let (_temp, engine, worktree) = fixture();
    let service = engine.lock_worktree_service().unwrap();
    let fence = engine.begin_execution_fence(&worktree.worktree).unwrap();
    drop(fence);
    assert!(engine.lock_worktree_service().is_err());
    assert!(engine
        .remove_agent_worktree(
            &worktree.worktree_dir,
            &worktree.worktree.branch,
            &worktree.worktree.base_commit,
        )
        .is_err());
    assert!(worktree.worktree_dir.is_dir());
    assert!(engine
        .merge_agent_branch(&worktree.worktree.branch, "main")
        .is_err());
    assert!(engine.smart_http_receive_pack(b"").is_err());
    assert!(engine
        .recover_execution_fences_after_operator_confirmation()
        .is_err());
    drop(service);
    let restarted = GitEngine::new(engine.repo_path());
    assert!(restarted.lock_worktree_service().is_err());
    assert_eq!(markers(&worktree).len(), 1);
    assert_eq!(
        restarted
            .recover_execution_fences_after_operator_confirmation()
            .unwrap(),
        1
    );
    assert!(restarted.lock_worktree_service().is_ok());
    assert!(markers(&worktree).is_empty());
}

#[test]
fn active_fences_coexist_and_completion_removes_only_its_own_private_marker() {
    let (_temp, engine, worktree) = fixture();
    let _service = engine.lock_worktree_service().unwrap();
    let first = engine.begin_execution_fence(&worktree.worktree).unwrap();
    let second = engine.begin_execution_fence(&worktree.worktree).unwrap();
    assert_eq!(markers(&worktree).len(), 2);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let directory = worktree.common_git_dir.join("gitust-active-executions");
        assert_eq!(
            fs::metadata(directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
        for path in markers(&worktree) {
            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
    first.complete().unwrap();
    assert_eq!(markers(&worktree).len(), 1);
    assert!(engine.lock_worktree_service().is_ok());
    second.complete().unwrap();
    assert!(markers(&worktree).is_empty());
    assert!(engine.lock_worktree_service().is_ok());
}

#[test]
fn prepared_commit_cannot_publish_after_an_unresolved_execution() {
    let (_temp, engine, worktree) = fixture();
    let _service = engine.lock_worktree_service().unwrap();
    fs::write(worktree.worktree_dir.join("README.md"), "change\n").unwrap();
    let prepared = engine
        .prepare_worktree_commit(
            worktree.worktree.id,
            &worktree.worktree.branch,
            &worktree.worktree.base_commit,
            &["README.md".into()],
            "prepared change",
            "fence@company.internal",
        )
        .unwrap();
    drop(engine.begin_execution_fence(&worktree.worktree).unwrap());
    assert!(engine.finish_worktree_commit(prepared).is_err());
    assert_eq!(
        git(&worktree.worktree_dir, &["rev-parse", "HEAD"]),
        worktree.worktree.base_commit.0
    );
}

#[test]
fn operator_recovery_validates_every_marker_before_deleting_any() {
    let (_temp, engine, worktree) = fixture();
    let service = engine.lock_worktree_service().unwrap();
    let first = engine.begin_execution_fence(&worktree.worktree).unwrap();
    let second = engine.begin_execution_fence(&worktree.worktree).unwrap();
    drop((first, second, service));
    let paths = markers(&worktree);
    fs::write(&paths[1], b"corrupt").unwrap();
    let restarted = GitEngine::new(engine.repo_path());
    assert!(restarted
        .recover_execution_fences_after_operator_confirmation()
        .is_err());
    assert_eq!(markers(&worktree).len(), 2);
    assert!(restarted.lock_worktree_service().is_err());
}

#[cfg(unix)]
#[test]
fn symlink_metadata_and_failed_completion_remain_quarantined() {
    use std::os::unix::fs::symlink;
    let (_temp, engine, worktree) = fixture();
    let _service = engine.lock_worktree_service().unwrap();
    let fence = engine.begin_execution_fence(&worktree.worktree).unwrap();
    let path = markers(&worktree).remove(0);
    let outside = worktree.worktree_dir.join("outside.json");
    fs::rename(&path, &outside).unwrap();
    symlink(&outside, &path).unwrap();
    assert!(fence.complete().is_err());
    assert!(engine.lock_worktree_service().is_err());
    assert!(path.is_symlink());
    assert!(outside.is_file());
}

#[cfg(unix)]
#[test]
fn begin_storage_failure_quarantines_existing_service_without_following_directory_links() {
    use std::os::unix::fs::symlink;
    let (_temp, engine, worktree) = fixture();
    let service = engine.lock_worktree_service().unwrap();
    let outside = worktree.worktree_dir.join("outside");
    fs::create_dir(&outside).unwrap();
    symlink(
        &outside,
        worktree.common_git_dir.join("gitust-active-executions"),
    )
    .unwrap();
    assert!(engine.begin_execution_fence(&worktree.worktree).is_err());
    assert!(engine.lock_worktree_service().is_err());
    assert!(fs::read_dir(&outside).unwrap().next().is_none());
    drop(service);
    let restarted = GitEngine::new(engine.repo_path());
    assert!(restarted.lock_worktree_service().is_err());
    assert!(restarted
        .recover_execution_fences_after_operator_confirmation()
        .is_err());
}

#[test]
fn operator_recovery_refuses_a_second_service_and_never_changes_git_refs() {
    let (_temp, engine, worktree) = fixture();
    let service = engine.lock_worktree_service().unwrap();
    drop(engine.begin_execution_fence(&worktree.worktree).unwrap());
    let rival = GitEngine::new(engine.repo_path());
    assert!(matches!(
        rival.recover_execution_fences_after_operator_confirmation(),
        Err(GitEngineError::WorktreeServiceBusy(_))
    ));
    drop(service);
    rival
        .recover_execution_fences_after_operator_confirmation()
        .unwrap();
    assert_eq!(
        git(engine.repo_path(), &["rev-parse", "main"]),
        worktree.worktree.base_commit.0
    );
    assert_eq!(
        git(
            engine.repo_path(),
            &["rev-parse", &worktree.worktree.branch]
        ),
        worktree.worktree.base_commit.0
    );
}

#[test]
fn execution_fences_do_not_require_manual_verification_target_metadata() {
    let (_temp, engine, mut worktree) = fixture();
    worktree.worktree.base_branch.clear();
    worktree.worktree.base_commit.0.clear();
    engine
        .begin_execution_fence(&worktree.worktree)
        .unwrap()
        .complete()
        .unwrap();
    assert!(markers(&worktree).is_empty());
}

#[test]
fn concurrent_fence_completion_preserves_other_active_executions() {
    let (_temp, engine, worktree) = fixture();
    let _service = engine.lock_worktree_service().unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
    let threads: Vec<_> = (0..8)
        .map(|_| {
            let engine = engine.clone();
            let worktree = worktree.worktree.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let fence = engine.begin_execution_fence(&worktree).unwrap();
                barrier.wait();
                fence.complete().unwrap();
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
    assert!(markers(&worktree).is_empty());
    assert!(engine.lock_worktree_service().is_ok());
}

#[cfg(unix)]
#[test]
fn recovery_rejects_oversized_or_nonprivate_marker_without_removing_it() {
    use std::os::unix::fs::PermissionsExt;
    let (_temp, engine, worktree) = fixture();
    drop(engine.begin_execution_fence(&worktree.worktree).unwrap());
    let path = markers(&worktree).remove(0);
    let original = fs::read(&path).unwrap();
    fs::write(&path, vec![b'x'; 4097]).unwrap();
    assert!(engine
        .recover_execution_fences_after_operator_confirmation()
        .is_err());
    assert!(path.is_file());
    fs::write(&path, original).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(engine
        .recover_execution_fences_after_operator_confirmation()
        .is_err());
    assert!(path.is_file());
}

#[test]
fn marker_count_limit_fails_closed_without_deleting_evidence() {
    let (_temp, engine, worktree) = fixture();
    drop(engine.begin_execution_fence(&worktree.worktree).unwrap());
    let directory = worktree.common_git_dir.join("gitust-active-executions");
    for count in 0..1024 {
        fs::write(directory.join(format!("unexpected-{count}")), b"").unwrap();
    }
    assert!(engine.lock_worktree_service().is_err());
    assert!(engine
        .recover_execution_fences_after_operator_confirmation()
        .is_err());
    assert_eq!(fs::read_dir(directory).unwrap().count(), 1025);
}
