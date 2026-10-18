use crate::policy::resolve_rules;
use crate::{
    await_pipe_drain, LocalSandboxExecutor, PathAccess, PathRule, SandboxError, SandboxSpec,
};
use gitust_core::WorktreeId;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

#[test]
fn linked_worktree_policy_hides_fixed_ssh_host_key_directory() {
    let common = std::path::Path::new("/fixture/repository.git");
    let spec = SandboxSpec::for_linked_worktree(
        std::path::Path::new("/fixture/worktrees/own"),
        &common.join("worktrees/own"),
        common,
        None,
        std::path::Path::new("/fixture/worktrees"),
        std::path::Path::new("/fixture/scratch"),
    );
    assert!(spec.rules.iter().any(|rule| {
        rule.path == common.join("gitust-ssh") && rule.access == PathAccess::Hidden
    }));
}

#[test]
fn missing_readonly_child_fails_instead_of_inheriting_writable_parent() {
    let temp = tempfile::tempdir().unwrap();
    for access in [PathAccess::ReadOnly, PathAccess::ReadWrite] {
        let missing = temp.path().join(".git");
        let error = resolve_rules(vec![
            PathRule::new(temp.path(), PathAccess::ReadWrite),
            PathRule::new(&missing, access),
        ])
        .unwrap_err();
        assert!(matches!(error, SandboxError::UnsupportedPath(_)));
        assert!(!missing.exists());
    }
}

#[tokio::test]
async fn missing_git_pointer_refuses_execution_before_command_can_recreate_it() {
    let temp = tempfile::tempdir().unwrap();
    let workdir = temp.path().join("worktree");
    std::fs::create_dir(&workdir).unwrap();
    let spec = SandboxSpec {
        workdir: workdir.clone(),
        scratch_base: temp.path().join("scratch"),
        rules: vec![
            PathRule::new(&workdir, PathAccess::ReadWrite),
            PathRule::new(workdir.join(".git"), PathAccess::ReadOnly),
        ],
    };
    let error = LocalSandboxExecutor::new(1)
        .execute(WorktreeId::new(), &spec, "printf replaced > .git")
        .await
        .unwrap_err();
    assert!(matches!(error, SandboxError::UnsupportedPath(_)));
    assert!(!workdir.join(".git").exists());
}

#[tokio::test]
async fn command_starting_with_dash_is_a_script_instead_of_shell_options() {
    let temp = tempfile::tempdir().unwrap();
    let spec = SandboxSpec {
        workdir: temp.path().to_path_buf(),
        scratch_base: temp.path().join("scratch"),
        rules: vec![PathRule::new(temp.path(), PathAccess::ReadWrite)],
    };
    let executor = LocalSandboxExecutor::new(1);
    for required in [vec![], vec!["touch must-not-run".into()]] {
        let result = executor
            .execute_with_checks(WorktreeId::new(), &spec, "-s", &required)
            .await
            .unwrap();
        assert_eq!(result.exit_code, 127, "{}", result.stderr);
        assert!(!temp.path().join("must-not-run").exists());
    }
}

#[tokio::test]
async fn drain_timeout_aborts_the_reader_instead_of_detaching_it() {
    struct MarkDropped(Arc<AtomicBool>);
    impl Drop for MarkDropped {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    let dropped = Arc::new(AtomicBool::new(false));
    let guard = MarkDropped(Arc::clone(&dropped));
    let reader = tokio::spawn(async move {
        let _guard = guard;
        std::future::pending::<(Vec<u8>, bool)>().await
    });
    let partial = Arc::new(Mutex::new(b"captured".to_vec()));
    let (bytes, truncated) = await_pipe_drain(reader, &partial).await;
    assert_eq!(bytes, b"captured");
    assert!(truncated);
    tokio::task::yield_now().await;
    assert!(dropped.load(Ordering::SeqCst));
}

#[tokio::test]
async fn auth_state_in_bare_common_directory_stays_hidden_when_git_data_is_readable() {
    let temp = tempfile::tempdir().unwrap();
    let common = temp.path().join("bare.git");
    let admin = common.join("worktrees/own");
    let auth = common.join(".gitust");
    let base = temp.path().join("worktrees");
    let workdir = base.join("own");
    for dir in [&admin, &auth, &workdir] {
        std::fs::create_dir_all(dir).unwrap();
    }
    std::fs::write(common.join("public"), "visible-git-data").unwrap();
    std::fs::write(auth.join("auth-db.json"), "private-auth-state").unwrap();
    std::fs::write(workdir.join(".git"), "gitdir: unused\n").unwrap();
    let spec = SandboxSpec::for_linked_worktree(
        &workdir,
        &admin,
        &common,
        None,
        &base,
        &temp.path().join("scratch"),
    );
    let result = LocalSandboxExecutor::new(1)
        .execute(
            WorktreeId::new(),
            &spec,
            &format!(
                "cat '{}/public'; cat '{}/auth-db.json'; printf stolen > '{}/auth-db.json.lock'",
                common.display(),
                auth.display(),
                auth.display()
            ),
        )
        .await
        .unwrap();
    assert_ne!(result.exit_code, 0);
    assert!(result.stdout.contains("visible-git-data"));
    assert!(!result.stdout.contains("private-auth-state"));
    assert!(!auth.join("auth-db.json.lock").exists());
}

#[tokio::test]
async fn cancelling_pipe_drain_aborts_its_reader_instead_of_detaching_it() {
    struct MarkDropped(Arc<AtomicBool>);
    impl Drop for MarkDropped {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    let dropped = Arc::new(AtomicBool::new(false));
    let guard = MarkDropped(Arc::clone(&dropped));
    let reader = tokio::spawn(async move {
        let _guard = guard;
        std::future::pending::<(Vec<u8>, bool)>().await
    });
    let partial = Arc::new(Mutex::new(Vec::new()));
    let cancelled = tokio::time::timeout(
        std::time::Duration::from_millis(5),
        await_pipe_drain(reader, &partial),
    )
    .await;
    assert!(cancelled.is_err());
    tokio::task::yield_now().await;
    assert!(dropped.load(Ordering::SeqCst));
}
