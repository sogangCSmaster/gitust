use gitust_core::{CommitSha, RepoId, SandboxRunId, WorktreeId};
use gitust_sandbox::{
    SandboxBackend, SandboxCommandOutput, SandboxCommandResult, SandboxExecutionResult,
};
use gitust_server::local_checks_history::{
    LocalChecksHistoryStore, LocalChecksRecord, LocalChecksStatus,
};

fn result(failed: bool) -> SandboxExecutionResult {
    SandboxExecutionResult {
        run_id: SandboxRunId::new(),
        worktree_id: WorktreeId::new(),
        command: "first && second".into(),
        exit_code: if failed { 7 } else { 0 },
        duration_ms: 3,
        stdout: "ordinary output".into(),
        stderr: String::new(),
        output_truncated: false,
        steps: ["first", "second"]
            .into_iter()
            .enumerate()
            .map(|(index, command)| {
                let skipped = failed && index == 1;
                SandboxCommandResult {
                    command: command.into(),
                    required: true,
                    exit_code: (!skipped).then_some(if failed { 7 } else { 0 }),
                    duration_ms: (!skipped).then_some(1),
                    output: (!skipped).then(|| SandboxCommandOutput {
                        stdout: "ordinary output".into(),
                        stderr: String::new(),
                        truncated: false,
                    }),
                }
            })
            .collect(),
        backend: SandboxBackend::MacosSeatbelt,
        network_isolated: true,
    }
}
fn record(failed: bool) -> LocalChecksRecord {
    LocalChecksRecord::new(
        "refs/heads/main".into(),
        CommitSha("a".repeat(40)),
        "b".repeat(40),
        result(failed),
    )
    .unwrap()
}

#[test]
fn append_reopen_get_and_retention_preserve_actual_result_and_repository_identity() {
    let temp = tempfile::tempdir().unwrap();
    let repo = RepoId::new();
    let store = LocalChecksHistoryStore::open(temp.path(), repo).unwrap();
    assert!(store.list().unwrap().is_empty());
    assert!(!temp.path().join("gitust-local-check-runs").exists());
    let mut saved = Vec::new();
    for index in 0..6 {
        let mut item = record(index == 5);
        // Equal timestamps must not turn append order into an unstable time sort.
        if let Some(first) = saved.first() {
            let first: &LocalChecksRecord = first;
            item.completed_at = first.completed_at;
        }
        store.append(item.clone()).unwrap();
        saved.push(item);
    }
    let reopened = LocalChecksHistoryStore::open(temp.path(), repo).unwrap();
    let actual = reopened.list().unwrap();
    assert_eq!(actual.len(), 5);
    assert_eq!(actual[0].status, LocalChecksStatus::Failed);
    for (actual, expected) in actual.iter().zip(saved[1..].iter().rev()) {
        assert_eq!(
            serde_json::to_value(actual).unwrap(),
            serde_json::to_value(expected).unwrap()
        );
        assert_eq!(
            serde_json::to_value(reopened.get(expected.result.run_id).unwrap().unwrap()).unwrap(),
            serde_json::to_value(expected).unwrap()
        );
    }
    assert!(reopened.get(saved[0].result.run_id).unwrap().is_none());
    reopened.append(saved[5].clone()).unwrap();
    assert_eq!(reopened.list().unwrap().len(), 5);
    let mut conflicting = saved[5].clone();
    conflicting.result.stdout = "different outcome".into();
    assert!(reopened.append(conflicting).is_err());
    assert!(LocalChecksHistoryStore::open(temp.path(), RepoId::new())
        .unwrap()
        .list()
        .is_err());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(temp.path().join("gitust-local-check-runs"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(temp.path().join("gitust-local-check-runs/history.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    assert!(!temp.path().join("gitust-check-runs").exists());
    assert!(!temp.path().join("gitust-automatic-runs").exists());
}

#[test]
fn stored_utf8_is_bounded_without_changing_the_original_execution_result() {
    let temp = tempfile::tempdir().unwrap();
    let repo = RepoId::new();
    let mut original = result(false);
    original.stdout = "가".repeat(7000);
    original.stderr = "나".repeat(7000);
    original.steps[0].output.as_mut().unwrap().stdout = "다".repeat(1000);
    original.steps[0].output.as_mut().unwrap().stderr = "라".repeat(1000);
    let before = serde_json::to_value(&original).unwrap();
    let item = LocalChecksRecord::new(
        "refs/heads/main".into(),
        CommitSha("a".repeat(40)),
        "b".repeat(40),
        original.clone(),
    )
    .unwrap();
    assert!(item.history_truncated);
    assert!(item.result.stdout.len() <= 16 * 1024);
    assert!(item.result.stderr.len() <= 16 * 1024);
    let output = item.result.steps[0].output.as_ref().unwrap();
    assert!(output.stdout.len() <= 1024 && output.stderr.len() <= 1024 && output.truncated);
    assert_eq!(serde_json::to_value(original).unwrap(), before);
    let store = LocalChecksHistoryStore::open(temp.path(), repo).unwrap();
    store.append(item.clone()).unwrap();
    assert_eq!(
        serde_json::to_value(store.get(item.result.run_id).unwrap().unwrap()).unwrap(),
        serde_json::to_value(item).unwrap()
    );
}

#[test]
fn invalid_completed_steps_and_malformed_history_fail_without_replacing_saved_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let repo = RepoId::new();
    let mut invalid = result(false);
    invalid.steps[0].required = false;
    assert!(LocalChecksRecord::new(
        "refs/heads/main".into(),
        CommitSha("a".repeat(40)),
        "b".repeat(40),
        invalid
    )
    .is_err());
    let store = LocalChecksHistoryStore::open(temp.path(), repo).unwrap();
    store.append(record(false)).unwrap();
    let path = temp.path().join("gitust-local-check-runs/history.json");
    std::fs::write(&path, b"{unfinished JSON").unwrap();
    assert!(store.list().is_err());
    assert!(store.get(SandboxRunId::new()).is_err());
    assert!(store.append(record(false)).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"{unfinished JSON");
}
