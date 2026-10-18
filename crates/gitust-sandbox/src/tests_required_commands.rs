use crate::{LocalSandboxExecutor, PathAccess, PathRule, SandboxSpec};
use gitust_core::WorktreeId;

#[tokio::test]
async fn required_commands_preserve_quotes_stop_on_failure_and_accept_large_requests() {
    let tmp = tempfile::tempdir().unwrap();
    let workdir = tmp.path().join("worktree");
    std::fs::create_dir(&workdir).unwrap();
    let spec = SandboxSpec {
        workdir: workdir.clone(),
        scratch_base: tmp.path().join("scratch"),
        rules: vec![PathRule::new(&workdir, PathAccess::ReadWrite)],
    };
    let executor = LocalSandboxExecutor::new(1).with_timeout_secs(10);
    let result = executor
        .execute_with_checks(
            WorktreeId::new(),
            &spec,
            "printf '%s' \"quote's\"; exit 0 # comment",
            &[
                "printf first; exit 0".into(),
                "printf second; exit 7".into(),
                "touch must-not-run".into(),
            ],
        )
        .await
        .unwrap();
    assert_eq!(result.exit_code, 7, "{}", result.stderr);
    assert_eq!(result.stdout, "quote'sfirstsecond");
    assert!(!workdir.join("must-not-run").exists());
    assert!(result.command.contains(" && "));

    let requested = format!("true {}", "''".repeat(32_765));
    let result = executor
        .execute_with_checks(
            WorktreeId::new(),
            &spec,
            &requested,
            &["printf required-ran".into()],
        )
        .await
        .unwrap();
    assert_eq!(result.exit_code, 0, "{}", result.stderr);
    assert_eq!(result.stdout, "required-ran");
}

#[tokio::test]
async fn explicit_checks_admission_holds_prepared_capacity_and_cleans_unstarted_run() {
    let tmp = tempfile::tempdir().unwrap();
    let workdir = tmp.path().join("worktree");
    std::fs::create_dir(&workdir).unwrap();
    let spec = SandboxSpec {
        workdir: workdir.clone(),
        scratch_base: tmp.path().join("scratch"),
        rules: vec![PathRule::new(&workdir, PathAccess::ReadWrite)],
    };
    let executor = LocalSandboxExecutor::new(1);
    let held = executor.clone();
    let scratch = spec.scratch_base.clone();
    let gate = Box::pin(async move {
        assert_eq!(held.semaphore.available_permits(), 0);
        assert_eq!(std::fs::read_dir(scratch).unwrap().count(), 1);
        Err(crate::SandboxError::AdmissionDenied)
    });
    let error = executor
        .execute_required_checks_with_admission(
            WorktreeId::new(),
            &spec,
            &["printf should-not-start > not-started".into()],
            gate,
        )
        .await
        .unwrap_err();
    assert!(matches!(error, crate::SandboxError::AdmissionDenied));
    assert!(!workdir.join("not-started").exists());
    assert_eq!(std::fs::read_dir(&spec.scratch_base).unwrap().count(), 0);
    assert_eq!(executor.semaphore.available_permits(), 1);
}
