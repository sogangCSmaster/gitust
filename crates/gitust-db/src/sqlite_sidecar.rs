use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use gitust_core::{AuditEventRecord, AuthError, PullRequestRecord};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::models::{AuthState, PersistedSnapshot, JSON_WINDOW_CAP};
use crate::sqlite_wal::{SqlValue, SqliteWalDb};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct SqliteCheckpoint {
    version: u8,
    pull_request_count: usize,
    audit_log_count: usize,
    pull_requests_sha256: String,
    audit_log_sha256: String,
}

pub(crate) fn checkpoint(
    prs: &[PullRequestRecord],
    audit: &[AuditEventRecord],
) -> Result<SqliteCheckpoint, AuthError> {
    fn digest(value: &impl Serialize) -> Result<String, AuthError> {
        let bytes = serde_json::to_vec(value)
            .map_err(|e| AuthError::StorageError(format!("SQLite checkpoint encoding: {e}")))?;
        Ok(hex::encode(Sha256::digest(bytes)))
    }
    Ok(SqliteCheckpoint {
        version: 1,
        pull_request_count: prs.len(),
        audit_log_count: audit.len(),
        pull_requests_sha256: digest(&prs)?,
        audit_log_sha256: digest(&audit)?,
    })
}

fn incomplete() -> AuthError {
    AuthError::StorageError(
        "SQLite history is missing or does not match the committed checkpoint".into(),
    )
}

const SCHEMA_SQL: &str = "
CREATE TABLE IF NOT EXISTS pull_requests (
    seq INTEGER PRIMARY KEY,
    repo_id TEXT NOT NULL,
    number INTEGER NOT NULL,
    id TEXT NOT NULL UNIQUE,
    payload_json TEXT NOT NULL,
    UNIQUE(repo_id, number)
);
CREATE TABLE IF NOT EXISTS audit_log (
    seq INTEGER PRIMARY KEY,
    timestamp TEXT NOT NULL,
    action TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    prev_hash TEXT NOT NULL,
    entry_hash TEXT NOT NULL
);
";

pub fn sqlite_wal_path(db_path: &Path) -> PathBuf {
    let mut os = db_path.as_os_str().to_os_string();
    os.push(".sqlite3");
    PathBuf::from(os)
}

fn open_sidecar(db_path: &Path) -> Result<SqliteWalDb, AuthError> {
    let wal_path = sqlite_wal_path(db_path);
    let db = SqliteWalDb::open(&wal_path).map_err(AuthError::StorageError)?;
    db.execute_batch(SCHEMA_SQL)
        .map_err(AuthError::StorageError)?;
    Ok(db)
}

pub(crate) fn sync_state_to_sqlite(db_path: &Path, state: &mut AuthState) -> Result<(), AuthError> {
    let db = open_sidecar(db_path)?;
    let dirty_ids: Vec<gitust_core::PullRequestId> = state.dirty_pr_ids.iter().copied().collect();
    db.with_transaction(|conn| {
        sync_pull_requests_tx(conn, &state.pull_requests, &dirty_ids)?;
        sync_audit_log_tx(conn, &state.audit_log)?;
        Ok(())
    })
    .map_err(AuthError::StorageError)?;
    state.dirty_pr_ids.clear();
    Ok(())
}

pub(crate) fn reconcile_sqlite_after_rollback(
    db_path: &Path,
    state: &mut AuthState,
) -> Result<(), AuthError> {
    let db = open_sidecar(db_path)?;
    db.with_transaction(|conn| {
        conn.execute_batch("DELETE FROM pull_requests; DELETE FROM audit_log;")?;
        for (idx, pr) in state.pull_requests.iter().enumerate() {
            upsert_pull_request(conn, idx, pr)?;
        }
        for entry in &state.audit_log {
            insert_audit_entry(conn, entry)?;
        }
        Ok(())
    })
    .map_err(AuthError::StorageError)
}

fn sync_pull_requests_tx(
    conn: &SqliteWalDb,
    prs: &[PullRequestRecord],
    dirty_ids: &[gitust_core::PullRequestId],
) -> Result<(), String> {
    let rows = conn.query_rows("SELECT COUNT(*) FROM pull_requests;", &[])?;
    let sql_count_i64 = rows
        .first()
        .and_then(|r| r.first())
        .and_then(SqlValue::as_i64)
        .ok_or_else(|| "failed to read pull_requests count".to_string())?;
    let sql_count =
        usize::try_from(sql_count_i64).map_err(|_| "negative pull_requests count".to_string())?;

    if sql_count > prs.len() {
        conn.execute_batch("DELETE FROM pull_requests;")?;
        for (idx, pr) in prs.iter().enumerate() {
            upsert_pull_request(conn, idx, pr)?;
        }
        return Ok(());
    }

    for (idx, pr) in prs.iter().enumerate().skip(sql_count) {
        upsert_pull_request(conn, idx, pr)?;
    }

    if !dirty_ids.is_empty() {
        for (idx, pr) in prs.iter().enumerate().take(sql_count) {
            if dirty_ids.contains(&pr.id) {
                upsert_pull_request(conn, idx, pr)?;
            }
        }
    }
    Ok(())
}

fn upsert_pull_request(
    conn: &SqliteWalDb,
    seq: usize,
    pr: &PullRequestRecord,
) -> Result<(), String> {
    let seq_i64 = i64::try_from(seq).map_err(|_| "pull_request seq overflow".to_string())?;
    let number_i64 =
        i64::try_from(pr.number).map_err(|_| "pull_request number overflow".to_string())?;
    let payload_json = serde_json::to_string(pr)
        .map_err(|e| format!("failed to serialize pull_request for SQLite: {e}"))?;
    conn.execute(
        "INSERT INTO pull_requests (seq, repo_id, number, id, payload_json)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(seq) DO UPDATE SET
             repo_id = excluded.repo_id,
             number = excluded.number,
             id = excluded.id,
             payload_json = excluded.payload_json;",
        &[
            SqlValue::Integer(seq_i64),
            SqlValue::Text(pr.repo_id.0.to_string()),
            SqlValue::Integer(number_i64),
            SqlValue::Text(pr.id.0.to_string()),
            SqlValue::Text(payload_json),
        ],
    )
}

fn sync_audit_log_tx(conn: &SqliteWalDb, audit_log: &[AuditEventRecord]) -> Result<(), String> {
    let rows = conn.query_rows(
        "SELECT COUNT(*), COALESCE(MAX(seq), 0) FROM audit_log;",
        &[],
    )?;
    let row = rows
        .first()
        .ok_or_else(|| "failed to read audit_log summary".to_string())?;
    let sql_count_i64 = row
        .first()
        .and_then(SqlValue::as_i64)
        .ok_or_else(|| "missing audit_log count".to_string())?;
    let max_seq_i64 = row
        .get(1)
        .and_then(SqlValue::as_i64)
        .ok_or_else(|| "missing audit_log max_seq".to_string())?;
    let sql_count =
        usize::try_from(sql_count_i64).map_err(|_| "negative audit_log count".to_string())?;
    let max_seq =
        u64::try_from(max_seq_i64).map_err(|_| "negative audit_log max_seq".to_string())?;

    let incremental_ok = sql_count <= audit_log.len()
        && (sql_count == 0
            || audit_log
                .get(sql_count - 1)
                .is_some_and(|entry| entry.seq == max_seq));

    let start_idx = if incremental_ok {
        sql_count
    } else {
        conn.execute_batch("DELETE FROM audit_log;")?;
        0
    };

    for entry in &audit_log[start_idx..] {
        insert_audit_entry(conn, entry)?;
    }
    Ok(())
}

fn insert_audit_entry(conn: &SqliteWalDb, entry: &AuditEventRecord) -> Result<(), String> {
    let seq_i64 = i64::try_from(entry.seq).map_err(|_| "audit seq overflow".to_string())?;
    let payload_json = serde_json::to_string(entry)
        .map_err(|e| format!("failed to serialize audit entry for SQLite: {e}"))?;
    conn.execute(
        "INSERT OR REPLACE INTO audit_log
         (seq, timestamp, action, payload_json, prev_hash, entry_hash)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6);",
        &[
            SqlValue::Integer(seq_i64),
            SqlValue::Text(entry.timestamp.clone()),
            SqlValue::Text(entry.action.clone()),
            SqlValue::Text(payload_json),
            SqlValue::Text(entry.prev_hash.clone()),
            SqlValue::Text(entry.entry_hash.clone()),
        ],
    )
}

pub(crate) fn hydrate_snapshot_from_sqlite(
    db_path: &Path,
    snap: &mut PersistedSnapshot,
) -> Result<(), AuthError> {
    let wal_path = sqlite_wal_path(db_path);
    match std::fs::symlink_metadata(&wal_path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            return Err(AuthError::StorageError(format!(
                "refusing to open symlinked SQLite sidecar: {:?}",
                wal_path
            )));
        }
        Ok(_) => {}
        Err(e) if e.kind() == ErrorKind::NotFound => {
            if snap.sqlite_checkpoint.is_some()
                || snap.pull_requests.len() == JSON_WINDOW_CAP
                || snap.audit_log.first().is_some_and(|entry| entry.seq > 1)
            {
                return Err(incomplete());
            }
            return Ok(());
        }
        Err(e) => return Err(AuthError::StorageError(e.to_string())),
    }

    let db = open_sidecar(db_path)?;

    if snap.pull_requests.len() == JSON_WINDOW_CAP {
        let pr_rows = db
            .query_rows(
                "SELECT payload_json FROM pull_requests ORDER BY seq ASC;",
                &[],
            )
            .map_err(AuthError::StorageError)?;
        if !pr_rows.is_empty() {
            let mut sql_prs = Vec::with_capacity(pr_rows.len());
            for row in pr_rows {
                let json = row.first().and_then(SqlValue::as_str).ok_or_else(|| {
                    AuthError::StorageError("missing pull_request payload_json".to_string())
                })?;
                let pr: PullRequestRecord = serde_json::from_str(json)
                    .map_err(|e| AuthError::StorageError(format!("corrupt SQLite PR json: {e}")))?;
                sql_prs.push(pr);
            }
            let first_window_id = snap.pull_requests[0].id;
            if let Some(prefix_end) = sql_prs.iter().position(|p| p.id == first_window_id) {
                let mut combined = Vec::with_capacity(sql_prs.len());
                combined.extend_from_slice(&sql_prs[..prefix_end]);
                for snap_pr in &snap.pull_requests {
                    if !sql_prs[prefix_end..].iter().any(|p| p.id == snap_pr.id) {
                        return Err(incomplete());
                    }
                    combined.push(snap_pr.clone());
                }
                snap.pull_requests = combined;
            } else {
                return Err(incomplete());
            }
        } else {
            return Err(incomplete());
        }
    }

    if let Some(first_window_seq) = snap.audit_log.first().map(|e| e.seq).filter(|&s| s > 1) {
        let audit_rows = db
            .query_rows("SELECT payload_json FROM audit_log ORDER BY seq ASC;", &[])
            .map_err(AuthError::StorageError)?;
        if !audit_rows.is_empty() {
            let mut sql_audit = Vec::with_capacity(audit_rows.len());
            for row in audit_rows {
                let json = row.first().and_then(SqlValue::as_str).ok_or_else(|| {
                    AuthError::StorageError("missing audit_log payload_json".to_string())
                })?;
                let entry: AuditEventRecord = serde_json::from_str(json).map_err(|e| {
                    AuthError::StorageError(format!("corrupt SQLite audit json: {e}"))
                })?;
                sql_audit.push(entry);
            }
            let max_sql_seq = sql_audit.last().map_or(0, |e| e.seq);
            if let Some(prefix_end) = sql_audit.iter().position(|e| e.seq == first_window_seq) {
                let mut combined = Vec::with_capacity(sql_audit.len());
                combined.extend_from_slice(&sql_audit[..prefix_end]);
                if snap
                    .audit_log
                    .last()
                    .is_some_and(|entry| entry.seq > max_sql_seq)
                {
                    return Err(incomplete());
                }
                combined.extend(snap.audit_log.iter().cloned());
                snap.audit_log = combined;
            } else {
                return Err(incomplete());
            }
        } else {
            return Err(incomplete());
        }
    }
    if let Some(expected) = &snap.sqlite_checkpoint {
        if &checkpoint(&snap.pull_requests, &snap.audit_log)? != expected {
            return Err(incomplete());
        }
    }
    Ok(())
}
