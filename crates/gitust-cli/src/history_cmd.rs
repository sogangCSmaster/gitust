//! Explicit repository-local diagnostic persistence, separate from verification history.
use gitust_core::{CommitSha, RepoId, SandboxRunId};
use gitust_git::{GitEngine, WorktreeServiceLock};
use gitust_sandbox::SandboxExecutionResult;
use gitust_server::local_checks_history::{
    HistoryError, LocalChecksHistoryStore, LocalChecksRecord,
};
use serde::Serialize;
use std::path::Path;

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum SaveStatus {
    Saved,
    Failed,
    NotAttempted,
}

#[derive(Serialize)]
pub(crate) struct HistoryWrite {
    status: SaveStatus,
    reason: Option<&'static str>,
    repo_id: Option<RepoId>,
    run_id: Option<SandboxRunId>,
}

impl HistoryWrite {
    pub(crate) fn not_attempted() -> Self {
        Self {
            status: SaveStatus::NotAttempted,
            reason: Some("execution_not_confirmed"),
            repo_id: None,
            run_id: None,
        }
    }

    pub(crate) fn failed(&self) -> bool {
        matches!(self.status, SaveStatus::Failed)
    }
}

fn reason(error: HistoryError) -> &'static str {
    match error {
        HistoryError::InvalidRecord => "invalid_record",
        HistoryError::StorageUnavailable => "storage_unavailable",
        HistoryError::InvalidStorage => "invalid_storage",
        HistoryError::ConflictingRun => "conflicting_run",
    }
}

pub(crate) async fn save_completed(
    engine: GitEngine,
    owner: WorktreeServiceLock,
    source_ref: String,
    checked_commit: CommitSha,
    checks_blob_oid: String,
    result: SandboxExecutionResult,
) -> HistoryWrite {
    let run_id = result.run_id;
    let stored = tokio::task::spawn_blocking(move || {
        let _owner = owner;
        let mut history = HistoryWrite {
            status: SaveStatus::Failed,
            reason: Some("repository_unavailable"),
            repo_id: None,
            run_id: Some(run_id),
        };
        let repo_id = match engine.load_or_create_repository_id() {
            Ok(id) => id,
            // fail-open-ok: failed identity admission reports history failed and converts execution exit0 to exit1.
            Err(_) => return history,
        };
        history.repo_id = Some(repo_id);
        let common = match engine.repository_common_directory() {
            Ok(common) => common,
            // fail-open-ok: inaccessible repository metadata cannot publish a saved history result.
            Err(_) => return history,
        };
        let append = (|| {
            let record =
                LocalChecksRecord::new(source_ref, checked_commit, checks_blob_oid, result)?;
            LocalChecksHistoryStore::open(&common, repo_id)?.append(record)
        })();
        match append {
            Ok(()) => {
                history.status = SaveStatus::Saved;
                history.reason = None;
            }
            Err(error) => history.reason = Some(reason(error)),
        }
        history
    })
    .await;
    match stored {
        Ok(history) => history,
        Err(_) => HistoryWrite {
            status: SaveStatus::Failed,
            reason: Some("storage_unavailable"),
            repo_id: None,
            run_id: Some(run_id),
        },
    }
}

#[derive(Serialize)]
pub(crate) struct HistoryReport {
    version: u32,
    status: &'static str,
    reason: Option<&'static str>,
    repo_id: Option<RepoId>,
    records: Vec<LocalChecksRecord>,
}

impl HistoryReport {
    fn rejected(reason: &'static str) -> Self {
        Self {
            version: 1,
            status: "rejected",
            reason: Some(reason),
            repo_id: None,
            records: Vec::new(),
        }
    }

    pub(crate) fn exit_code(&self) -> i32 {
        if self.status == "ok" {
            0
        } else {
            2
        }
    }
}

pub(crate) async fn read_history(repo: &Path, run_id: Option<&str>) -> HistoryReport {
    let run_id = match run_id {
        Some(raw) => match serde_json::from_value::<SandboxRunId>(raw.into()) {
            Ok(id) if !id.0.is_nil() => Some(id),
            _ => return HistoryReport::rejected("invalid_run_id"),
        },
        None => None,
    };
    let repo = repo.to_owned();
    let read = tokio::task::spawn_blocking(move || {
        let engine = GitEngine::new(repo);
        let _owner = match engine.lock_worktree_service() {
            Ok(owner) => owner,
            // fail-open-ok: unavailable ownership rejects the read with exit2 before any storage access.
            Err(_) => return HistoryReport::rejected("repository_unavailable"),
        };
        let repo_id = match engine.load_existing_repository_id() {
            Ok(id) => id,
            // fail-open-ok: missing or invalid existing identity rejects with exit2 and never creates a replacement ID.
            Err(_) => return HistoryReport::rejected("repository_unavailable"),
        };
        let mut report = HistoryReport {
            version: 1,
            status: "rejected",
            reason: Some("repository_unavailable"),
            repo_id: Some(repo_id),
            records: Vec::new(),
        };
        let common = match engine.repository_common_directory() {
            Ok(common) => common,
            // fail-open-ok: report remains rejected/exit2; metadata failure is never returned as an empty successful history.
            Err(_) => return report,
        };
        let records = (|| {
            let store = LocalChecksHistoryStore::open(&common, repo_id)?;
            match run_id {
                Some(id) => store.get(id).map(|record| record.into_iter().collect()),
                None => store.list(),
            }
        })();
        match records {
            Ok(records) => {
                report.status = if run_id.is_some() && records.is_empty() {
                    "not_found"
                } else {
                    "ok"
                };
                report.reason = None;
                report.records = records;
            }
            Err(error) => report.reason = Some(reason(error)),
        }
        report
    })
    .await;
    match read {
        Ok(report) => report,
        Err(_) => HistoryReport::rejected("storage_unavailable"),
    }
}
