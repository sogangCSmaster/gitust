//! Local operator diagnostics, separate from managed-worktree history and approval authority.
//! Callers retain the actual repository's service lease across every store operation.
use chrono::{DateTime, Utc};
use gitust_core::CommitSha;
use gitust_git::{GitEngine, MAX_BRANCH_NAME_BYTES};
use gitust_sandbox::SandboxExecutionResult;
use serde::{Deserialize, Serialize};

#[cfg(unix)]
#[path = "local_checks_history_store.rs"]
mod storage;
#[cfg(unix)]
pub use storage::LocalChecksHistoryStore;

pub const DIRECTORY: &str = "gitust-local-check-runs";
pub const MAX_RECORDS: usize = 5;
pub const MAX_BYTES: u64 = 3 * 1024 * 1024;

#[derive(Debug)]
pub enum HistoryError {
    InvalidRecord,
    StorageUnavailable,
    InvalidStorage,
    ConflictingRun,
}
impl std::fmt::Display for HistoryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidRecord => "invalid completed local Checks record",
            Self::StorageUnavailable => "local Checks history storage is unavailable",
            Self::InvalidStorage => "invalid local Checks history storage",
            Self::ConflictingRun => "local Checks run ID has a different saved outcome",
        })
    }
}
impl std::error::Error for HistoryError {}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalChecksStatus {
    Passed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalChecksRecord {
    pub completed_at: DateTime<Utc>,
    pub source_ref: String,
    pub checked_commit: CommitSha,
    pub checks_blob_oid: String,
    pub status: LocalChecksStatus,
    pub result: SandboxExecutionResult,
    pub history_truncated: bool,
}
impl LocalChecksRecord {
    /// The caller confirms execution and checkout cleanup before creating a completed record.
    /// Pass a clone to preserve the full original CLI result; only this stored copy is truncated.
    pub fn new(
        source_ref: String,
        checked_commit: CommitSha,
        checks_blob_oid: String,
        result: SandboxExecutionResult,
    ) -> Result<Self, HistoryError> {
        let mut record = Self {
            completed_at: Utc::now(),
            source_ref,
            checked_commit,
            checks_blob_oid,
            status: if result.exit_code == 0 {
                LocalChecksStatus::Passed
            } else {
                LocalChecksStatus::Failed
            },
            result,
            history_truncated: false,
        };
        record.validate(false)?;
        record.history_truncated |= truncate(&mut record.result.command, 4096);
        record.history_truncated |= truncate(&mut record.result.stdout, 16 * 1024);
        record.history_truncated |= truncate(&mut record.result.stderr, 16 * 1024);
        for step in &mut record.result.steps {
            if let Some(output) = &mut step.output {
                let truncated =
                    truncate(&mut output.stdout, 1024) | truncate(&mut output.stderr, 1024);
                output.truncated |= truncated;
                record.history_truncated |= truncated;
            }
        }
        record.validate(true)?;
        Ok(record)
    }
    pub(crate) fn valid(&self) -> bool {
        self.validate(true).is_ok()
    }
    fn validate(&self, bounded: bool) -> Result<(), HistoryError> {
        let Some(branch) = self.source_ref.strip_prefix("refs/heads/") else {
            return Err(HistoryError::InvalidRecord);
        };
        if branch.len() > MAX_BRANCH_NAME_BYTES {
            return Err(HistoryError::InvalidRecord);
        }
        let ordinary = !branch.eq_ignore_ascii_case("agent")
            && !branch
                .get(..6)
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case("agent/"));
        let oid = |s: &str| {
            matches!(s.len(), 40 | 64)
                && s.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                && s.bytes().any(|b| b != b'0')
        };
        let result = &self.result;
        let normalized =
            GitEngine::validate_branch_name(branch).map_err(|_| HistoryError::InvalidRecord)?;
        if !ordinary
            || normalized != branch
            || !oid(&self.checked_commit.0)
            || !oid(&self.checks_blob_oid)
            || self.checked_commit.0.len() != self.checks_blob_oid.len()
            || result.run_id.0.is_nil()
            || result.worktree_id.0.is_nil()
            || !result.network_isolated
            || !(1..=8).contains(&result.steps.len())
            || result.command.trim().is_empty()
            || result.command.contains('\0')
            || (bounded
                && (result.command.len() > 4096
                    || result.stdout.len() > 16 * 1024
                    || result.stderr.len() > 16 * 1024))
        {
            return Err(HistoryError::InvalidRecord);
        }
        let mut failed = None;
        for step in &result.steps {
            if !step.required
                || step.command.trim().is_empty()
                || step.command.len() > 4096
                || step.command.contains('\0')
            {
                return Err(HistoryError::InvalidRecord);
            }
            if failed.is_some() {
                if step.exit_code.is_some() || step.duration_ms.is_some() || step.output.is_some() {
                    return Err(HistoryError::InvalidRecord);
                }
            } else {
                let (Some(code), Some(_), Some(output)) =
                    (step.exit_code, step.duration_ms, step.output.as_ref())
                else {
                    return Err(HistoryError::InvalidRecord);
                };
                if bounded && (output.stdout.len() > 1024 || output.stderr.len() > 1024) {
                    return Err(HistoryError::InvalidRecord);
                }
                if code != 0 {
                    failed = Some(code);
                }
            }
        }
        // fail-open-ok: an all-successful sequence has exit zero; every actual nonzero step is retained above.
        let expected = failed.unwrap_or(0);
        if result.exit_code != expected
            || (self.status == LocalChecksStatus::Passed) != (expected == 0)
        {
            return Err(HistoryError::InvalidRecord);
        }
        Ok(())
    }
}
fn truncate(text: &mut String, cap: usize) -> bool {
    if text.len() <= cap {
        return false;
    }
    let mut boundary = cap;
    while !text.is_char_boundary(boundary) {
        boundary -= 1;
    }
    text.truncate(boundary);
    true
}

#[cfg(not(unix))]
pub struct LocalChecksHistoryStore;
#[cfg(not(unix))]
impl LocalChecksHistoryStore {
    pub fn open(_: &std::path::Path, _: gitust_core::RepoId) -> Result<Self, HistoryError> {
        Err(HistoryError::StorageUnavailable)
    }
    pub fn list(&self) -> Result<Vec<LocalChecksRecord>, HistoryError> {
        Err(HistoryError::StorageUnavailable)
    }
    pub fn get(
        &self,
        _: gitust_core::SandboxRunId,
    ) -> Result<Option<LocalChecksRecord>, HistoryError> {
        Err(HistoryError::StorageUnavailable)
    }
    pub fn append(&self, _: LocalChecksRecord) -> Result<(), HistoryError> {
        Err(HistoryError::StorageUnavailable)
    }
}
