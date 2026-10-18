use crate::local_checks_history::{
    HistoryError, LocalChecksHistoryStore, LocalChecksRecord, MAX_BYTES,
};
use axum::{
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use gitust_core::RepoId;
use gitust_git::{GitEngine, WorktreeServiceLock};
use serde::Serialize;
use std::sync::Arc;
use tokio::sync::OwnedMutexGuard;

#[derive(Clone, Copy)]
pub(super) struct Failure {
    pub status: StatusCode,
    pub code: &'static str,
}
impl Failure {
    pub fn new(status: StatusCode, code: &'static str) -> Self {
        Self { status, code }
    }
    pub fn unavailable() -> Self {
        Self::new(StatusCode::SERVICE_UNAVAILABLE, "local_checks_unavailable")
    }
    pub fn response(self) -> Response {
        no_store((self.status, Json(serde_json::json!({"code":self.code}))).into_response())
    }
}
fn no_store(mut response: Response) -> Response {
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    response
}
#[derive(Serialize)]
pub(super) struct History {
    pub status: &'static str,
    pub reason: Option<&'static str>,
}
#[derive(Serialize)]
pub(super) struct Report {
    pub version: u32,
    pub repo_id: RepoId,
    pub status: &'static str,
    pub reason: Option<&'static str>,
    pub cleanup: &'static str,
    pub history: History,
    pub record: Option<LocalChecksRecord>,
}
impl Report {
    pub fn unconfirmed(repo_id: RepoId) -> Self {
        Self {
            version: 1,
            repo_id,
            status: "unconfirmed",
            reason: Some("run_unconfirmed"),
            cleanup: "unconfirmed",
            history: History {
                status: "not_attempted",
                reason: Some("execution_not_confirmed"),
            },
            record: None,
        }
    }
    pub fn response(self, status: StatusCode) -> Response {
        match serde_json::to_vec(&self) {
            Ok(bytes) if bytes.len() as u64 <= MAX_BYTES => no_store(
                (status, [(header::CONTENT_TYPE, "application/json")], bytes).into_response(),
            ),
            _ => Failure::unavailable().response(),
        }
    }
}
fn history_reason(error: HistoryError) -> &'static str {
    match error {
        HistoryError::InvalidRecord => "invalid_record",
        HistoryError::StorageUnavailable => "storage_unavailable",
        HistoryError::InvalidStorage => "invalid_storage",
        HistoryError::ConflictingRun => "conflicting_run",
    }
}
pub(super) async fn save(
    engine: GitEngine,
    owner: WorktreeServiceLock,
    operation: Arc<OwnedMutexGuard<()>>,
    repo_id: RepoId,
    record: LocalChecksRecord,
) -> History {
    let saved = tokio::task::spawn_blocking(move || {
        let _owner = owner;
        let _operation = operation;
        let identity = || {
            if engine
                .load_existing_repository_id()
                .map_err(|_| "repository_unavailable")?
                != repo_id
            {
                return Err("repository_unavailable");
            }
            Ok(())
        };
        identity()?;
        let common = engine
            .repository_common_directory()
            .map_err(|_| "repository_unavailable")?;
        LocalChecksHistoryStore::open(&common, repo_id)
            .and_then(|store| store.append(record))
            .map_err(history_reason)?;
        identity()
    })
    .await;
    match saved {
        Ok(Ok(())) => History {
            status: "saved",
            reason: None,
        },
        Ok(Err(reason)) => History {
            status: "failed",
            reason: Some(reason),
        },
        Err(_) => History {
            status: "failed",
            reason: Some("storage_unavailable"),
        },
    }
}
