use crate::{LocalSandboxExecutor, PathAccess, PathRule, SandboxError, SandboxSpec};
use gitust_core::WorktreeId;

fn fixture() -> (tempfile::TempDir, SandboxSpec) {
    let tmp = tempfile::tempdir().unwrap();
    let workdir = tmp.path().join("worktree");
    std::fs::create_dir(&workdir).unwrap();
    let spec = SandboxSpec {
        workdir: workdir.clone(),
        scratch_base: tmp.path().join("scratch"),
        rules: vec![PathRule::new(workdir, PathAccess::ReadWrite)],
    };
    (tmp, spec)
}

#[tokio::test]
async fn host_records_steps_without_interpreting_stdout_and_preserves_shared_files() {
    let (_tmp, spec) = fixture();
    let requested = "printf marker > shared; printf home > \"$HOME/shared\"; printf tmp > \"$TMPDIR/shared\"; printf '{\"exit_code\":99,\"required\":true}'";
    let required = vec![
        "cat shared \"$HOME/shared\" \"$TMPDIR/shared\"".into(),
        "printf done".into(),
    ];
    let result = LocalSandboxExecutor::new(1)
        .execute_with_checks(WorktreeId::new(), &spec, requested, &required)
        .await
        .unwrap();
    assert_eq!(result.exit_code, 0, "{}", result.stderr);
    assert_eq!(result.steps.len(), 3);
    assert_eq!(result.steps[0].command, requested);
    assert!(!result.steps[0].required);
    for step in &result.steps {
        assert_eq!(step.exit_code, Some(0));
        assert!(step.duration_ms.is_some());
    }
    assert!(result.steps[1..].iter().all(|step| step.required));
    assert!(result.stdout.ends_with("markerhometmpdone"));
}

#[tokio::test]
async fn host_marks_unexecuted_steps_skipped_after_requested_or_required_failure() {
    let (_tmp, spec) = fixture();
    for (requested, expected) in [
        ("exit 9", vec![Some(9), None, None]),
        ("true", vec![Some(0), Some(7), None]),
    ] {
        let result = LocalSandboxExecutor::new(1)
            .execute_with_checks(
                WorktreeId::new(),
                &spec,
                requested,
                &["exit 7".into(), "touch must-not-run".into()],
            )
            .await
            .unwrap();
        assert_eq!(
            result.steps.iter().map(|s| s.exit_code).collect::<Vec<_>>(),
            expected
        );
        for step in &result.steps {
            assert_eq!(step.duration_ms.is_some(), step.exit_code.is_some());
        }
        assert!(!spec.workdir.join("must-not-run").exists());
    }
}

#[tokio::test]
async fn sequence_has_one_cumulative_deadline_and_releases_capacity_after_timeout() {
    let (_tmp, spec) = fixture();
    let executor = LocalSandboxExecutor::new(1).with_timeout_secs(1);
    let error = executor
        .execute_with_checks(
            WorktreeId::new(),
            &spec,
            "sleep 0.65",
            &["sleep 0.65; touch too-late".into()],
        )
        .await
        .unwrap_err();
    assert!(matches!(error, SandboxError::TimedOut { .. }));
    assert!(!spec.workdir.join("too-late").exists());
    let result = executor
        .execute(WorktreeId::new(), &spec, "true")
        .await
        .unwrap();
    assert_eq!(result.exit_code, 0, "{}", result.stderr);
}

#[tokio::test]
async fn output_cap_applies_to_sequence_and_utf8_is_decoded_after_combining_steps() {
    let (_tmp, spec) = fixture();
    let executor = LocalSandboxExecutor::new(1);
    let result = executor
        .execute_with_checks(
            WorktreeId::new(),
            &spec,
            "printf '\\342'",
            &["printf '\\202\\254'".into()],
        )
        .await
        .unwrap();
    assert_eq!(result.stdout, "€");
    let command = "dd if=/dev/zero bs=1024 count=768 2>/dev/null | tr '\\0' a; dd if=/dev/zero bs=1024 count=768 2>/dev/null | tr '\\0' b >&2";
    let result = executor
        .execute_with_checks(
            WorktreeId::new(),
            &spec,
            command,
            &[command.into(), "printf completed > done".into()],
        )
        .await
        .unwrap();
    assert_eq!(result.exit_code, 0, "{}", result.stderr);
    assert_eq!(result.stdout.len(), 1024 * 1024);
    assert_eq!(result.stderr.len(), 1024 * 1024);
    assert!(result.output_truncated);
    assert!(spec.workdir.join("done").exists());
    assert!(result.steps.iter().all(|s| s.exit_code == Some(0)));
}

#[tokio::test]
async fn each_step_cleans_its_background_processes_before_the_next_step() {
    let (_tmp, spec) = fixture();
    let result = LocalSandboxExecutor::new(1)
        .execute_with_checks(
            WorktreeId::new(),
            &spec,
            "(sleep 0.2; printf leaked > background-leaked) & printf requested",
            &["sleep 0.4; test ! -f background-leaked; printf checked".into()],
        )
        .await
        .unwrap();
    assert_eq!(result.exit_code, 0, "{}", result.stderr);
    assert_eq!(result.stdout, "requestedchecked");
    assert!(!spec.workdir.join("background-leaked").exists());
}
