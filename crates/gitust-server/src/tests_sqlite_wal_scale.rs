use std::collections::BTreeMap;

use gitust_core::{CommitSha, IsolationTier, RepoId, SandboxRunId, WorktreeId};
use gitust_db::SqliteWalDb;
use gitust_sandbox::{
    SandboxBackend, SandboxCommandOutput, SandboxCommandResult, SandboxExecutionResult,
};
use uuid::Uuid;

use crate::{
    local_checks_history::{LocalChecksHistoryStore, LocalChecksRecord},
    workflow_store::{
        load_workflow_run_artifacts, load_workflow_runs, save_workflow_run, workflows_storage_root,
        WORKFLOWS_SQLITE_FILE_NAME,
    },
    workflows::{WorkflowArtifactEntry, WorkflowJobRun, WorkflowRunRecord},
};

#[test]
fn workflow_store_sqlite_wal_retains_over_30_runs_and_artifacts() {
    let temp = tempfile::tempdir().expect("tempdir");
    let common_git_dir = temp.path();
    let repo_id = RepoId::new();

    let mut run_ids = Vec::new();
    for idx in 1..=32u32 {
        let run_id = Uuid::new_v4().hyphenated().to_string();
        run_ids.push(run_id.clone());
        let record = WorkflowRunRecord {
            run_id: run_id.clone(),
            repo_id: repo_id.0.to_string(),
            workflow_name: format!("CI #{idx}"),
            commit_sha: format!("{idx:040x}"),
            source_ref: "refs/heads/main".to_owned(),
            status: "passed".to_owned(),
            jobs: vec![WorkflowJobRun {
                job_id: "build".to_owned(),
                display_name: "Build".to_owned(),
                needs: Vec::new(),
                runner_id: None,
                isolation_tier: IsolationTier::default(),
                matrix_values: BTreeMap::new(),
                status: "passed".to_owned(),
                cache_hit: false,
                artifacts: vec![WorkflowArtifactEntry {
                    name: format!("artifact-{idx}"),
                    path: format!("dist/out-{idx}.bin"),
                    size_bytes: 128,
                    content_preview: Some(format!("preview-{idx}")),
                }],
                redacted_output: "ok".to_owned(),
                duration_ms: 10,
            }],
            created_at: "2026-10-19T00:00:00Z".to_owned(),
        };
        save_workflow_run(common_git_dir, repo_id, &record)
            .expect("save workflow run into SQLite WAL");
    }

    let wal_path = workflows_storage_root(common_git_dir)
        .expect("root")
        .join(WORKFLOWS_SQLITE_FILE_NAME);
    let wal_db = SqliteWalDb::open(&wal_path).expect("open workflows.sqlite3");
    assert_eq!(wal_db.journal_mode().expect("journal_mode"), "wal");

    let loaded = load_workflow_runs(common_git_dir, repo_id).expect("load all 32 runs");
    assert_eq!(loaded.len(), 32);
    assert_eq!(loaded[0].run_id, run_ids[31]);
    assert_eq!(loaded[31].run_id, run_ids[0]);

    let oldest_artifacts =
        load_workflow_run_artifacts(common_git_dir, &run_ids[0]).expect("load run #1 artifacts");
    assert_eq!(oldest_artifacts.len(), 1);
    assert_eq!(oldest_artifacts[0].name, "artifact-1");
}

#[test]
fn local_checks_sqlite_wal_retains_all_runs_beyond_5_record_window() {
    let temp = tempfile::tempdir().expect("tempdir");
    let repo_id = RepoId::new();
    let store = LocalChecksHistoryStore::open(temp.path(), repo_id).expect("open store");

    let mut run_ids = Vec::new();
    for idx in 1..=12u32 {
        let run_id = SandboxRunId::new();
        run_ids.push(run_id);
        let exec = SandboxExecutionResult {
            run_id,
            worktree_id: WorktreeId::new(),
            command: "cargo test".into(),
            exit_code: 0,
            duration_ms: 5,
            stdout: format!("run {idx} ok"),
            stderr: String::new(),
            output_truncated: false,
            steps: vec![SandboxCommandResult {
                command: "cargo test".into(),
                required: true,
                exit_code: Some(0),
                duration_ms: Some(5),
                output: Some(SandboxCommandOutput {
                    stdout: format!("run {idx} ok"),
                    stderr: String::new(),
                    truncated: false,
                }),
            }],
            backend: SandboxBackend::MacosSeatbelt,
            network_isolated: true,
        };
        let record = LocalChecksRecord::new(
            format!("refs/heads/feat-{idx}"),
            CommitSha(format!("{idx:040x}")),
            "b".repeat(40),
            exec,
        )
        .expect("valid record");
        store.append(record).expect("append record");
    }

    let wal_path = temp.path().join("gitust-local-check-runs/history.sqlite3");
    let wal_db = SqliteWalDb::open(&wal_path).expect("open history.sqlite3");
    assert_eq!(wal_db.journal_mode().expect("journal_mode"), "wal");

    let bounded_recent = store.list().expect("bounded 5-record JSON window");
    assert_eq!(bounded_recent.len(), 5);
    assert_eq!(bounded_recent[0].result.run_id, run_ids[11]);

    let all_wal = store.list_all_wal().expect("all 12 records from WAL");
    assert_eq!(all_wal.len(), 12);
    assert!(
        all_wal.iter().any(|r| r.result.run_id == run_ids[0]),
        "earliest run #1 must be retained in SQLite WAL for PR branch protection"
    );
}
