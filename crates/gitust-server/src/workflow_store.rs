use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

use axum::http::StatusCode;
use gitust_core::RepoId;
use gitust_db::{SqlValue, SqliteWalDb};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::workflows::{unavailable, ApiError, WorkflowArtifactEntry, WorkflowRunRecord};

const WORKFLOWS_STORAGE_DIR: &str = "gitust-workflows";
pub(crate) const WORKFLOWS_SQLITE_FILE_NAME: &str = "workflows.sqlite3";
const RUNS_FILE_NAME: &str = "runs.json";
const ARTIFACTS_DIR_NAME: &str = "artifacts";
const MAX_RUNS_JSON_BYTES: u64 = 4 * 1024 * 1024;
const MAX_ARTIFACTS_JSON_BYTES: u64 = 2 * 1024 * 1024;
const JSON_MIRROR_WINDOW_RUNS: usize = 25;

const WORKFLOWS_CONTENT: &str =
    "SELECT seq, run_id, repo_id, payload_json FROM workflow_runs ORDER BY seq;";
const ARTIFACTS_CONTENT: &str =
    "SELECT run_id, payload_json FROM workflow_artifacts ORDER BY run_id;";
const WORKFLOWS_SCHEMA_SQL: &str = "
CREATE TABLE IF NOT EXISTS gitust_history_checkpoints (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS store_metadata (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS workflow_runs (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id TEXT NOT NULL UNIQUE,
    repo_id TEXT NOT NULL,
    payload_json TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS workflow_artifacts (
    run_id TEXT PRIMARY KEY,
    payload_json TEXT NOT NULL
);
";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredRunsEnvelope {
    version: u32,
    #[serde(default)]
    sqlite_required: bool,
    repo_id: RepoId,
    runs: Vec<WorkflowRunRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredArtifactsEnvelope {
    version: u32,
    run_id: String,
    artifacts: Vec<WorkflowArtifactEntry>,
}

pub(crate) fn ensure_safe_dir(path: &Path) -> Result<(), ApiError> {
    if let Err(e) = fs::symlink_metadata(path) {
        if e.kind() != std::io::ErrorKind::NotFound {
            return Err(unavailable(e));
        }
        fs::create_dir_all(path).map_err(unavailable)?;
    }
    let meta = fs::symlink_metadata(path).map_err(unavailable)?;
    if meta.file_type().is_symlink() || !meta.file_type().is_dir() {
        return Err(unavailable("workflow storage dir is not a safe directory"));
    }
    Ok(())
}

pub(crate) fn workflows_storage_root(common_git_dir: &Path) -> Result<PathBuf, ApiError> {
    let root = common_git_dir.join(WORKFLOWS_STORAGE_DIR);
    ensure_safe_dir(&root)?;
    Ok(root)
}

pub(crate) fn read_bounded_regular_file(
    path: &Path,
    max_bytes: u64,
) -> Result<Option<Vec<u8>>, ApiError> {
    let meta = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(unavailable(e)),
    };
    if meta.file_type().is_symlink() || !meta.file_type().is_file() || meta.len() > max_bytes {
        return Err(unavailable(
            "workflow storage file invalid or exceeds size limit",
        ));
    }
    let mut buf = Vec::new();
    fs::File::open(path)
        .map_err(unavailable)?
        .take(max_bytes + 1)
        .read_to_end(&mut buf)
        .map_err(unavailable)?;
    if buf.len() as u64 > max_bytes {
        return Err(unavailable("workflow storage file exceeds size limit"));
    }
    Ok(Some(buf))
}

pub(crate) fn atomic_write_regular_file(
    dir: &Path,
    final_path: &Path,
    payload: &[u8],
) -> Result<(), ApiError> {
    ensure_safe_dir(dir)?;
    match fs::symlink_metadata(final_path) {
        Ok(meta) if meta.file_type().is_symlink() || !meta.file_type().is_file() => {
            return Err(unavailable(
                "refusing to overwrite non-regular workflow file",
            ));
        }
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(unavailable(e)),
    }
    let tmp_path = dir.join(format!(".tmp-wf-{}", Uuid::new_v4()));
    let write_res = (|| -> std::io::Result<()> {
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp_path)?;
        f.write_all(payload)?;
        f.sync_all()?;
        fs::rename(&tmp_path, final_path)
    })();
    if let Err(err) = write_res {
        // fail-open-ok: best-effort temp file cleanup on failed atomic write; error is returned below.
        let _ = fs::remove_file(&tmp_path);
        return Err(unavailable(err));
    }
    Ok(())
}

fn open_workflows_sqlite(root: &Path) -> Result<SqliteWalDb, ApiError> {
    let sqlite_path = root.join(WORKFLOWS_SQLITE_FILE_NAME);
    let db = SqliteWalDb::open(&sqlite_path).map_err(unavailable)?;
    db.execute_batch(WORKFLOWS_SCHEMA_SQL)
        .map_err(unavailable)?;
    Ok(db)
}

fn migrate_workflow_runs(root: &Path, db: &SqliteWalDb, repo_id: RepoId) -> Result<(), ApiError> {
    let expected_repo = repo_id.0.to_string();
    db.with_transaction(|conn| {
        let rows = conn.query_rows("SELECT value FROM store_metadata WHERE key = 'json_migration_v1';", &[])?;
        if let Some(row) = rows.first() {
            if row.first().and_then(SqlValue::as_str) != Some(expected_repo.as_str()) {
                return Err("workflow migration identity mismatch".into());
            }
            if !conn.verify_history_checkpoint("workflow_runs", WORKFLOWS_CONTENT)?
                || !conn.verify_history_checkpoint("workflow_artifacts", ARTIFACTS_CONTENT)? {
                return Err("workflow migration checkpoint is missing".into());
            }
            return Ok(());
        }
        let bytes = read_bounded_regular_file(&root.join(RUNS_FILE_NAME), MAX_RUNS_JSON_BYTES)
            .map_err(|(_, message)| message)?;
        if let Some(bytes) = bytes {
            let env: StoredRunsEnvelope = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
            if env.version != 1 || env.repo_id != repo_id || env.sqlite_required {
                return Err("workflow migration source is incomplete or has wrong identity".into());
            }
            let existing = conn.query_rows("SELECT run_id, repo_id, payload_json FROM workflow_runs;", &[])?;
            let mut payloads = std::collections::HashMap::new();
            for row in existing {
                let id = row.first().and_then(SqlValue::as_str).ok_or("missing workflow run ID")?;
                if row.get(1).and_then(SqlValue::as_str) != Some(expected_repo.as_str()) {
                    return Err("workflow migration repository mismatch".into());
                }
                let payload = row.get(2).and_then(SqlValue::as_str).ok_or("missing workflow run payload")?;
                payloads.insert(id.to_owned(), payload.to_owned());
            }
            for prior in env.runs.iter().rev() {
                let payload = serde_json::to_string(prior).map_err(|e| e.to_string())?;
                if let Some(existing) = payloads.get(&prior.run_id) {
                    if existing != &payload {
                        return Err("workflow JSON and SQLite migration overlap differs".into());
                    }
                    continue;
                }
                conn.execute(
                    "INSERT OR IGNORE INTO workflow_runs (run_id, repo_id, payload_json) VALUES (?1, ?2, ?3);",
                    &[SqlValue::Text(prior.run_id.clone()), SqlValue::Text(expected_repo.clone()), SqlValue::Text(payload)],
                )?;
            }
        }
        conn.execute("INSERT INTO store_metadata (key, value) VALUES ('json_migration_v1', ?1);", &[SqlValue::Text(expected_repo.clone())])?;
        conn.save_history_checkpoint("workflow_runs", WORKFLOWS_CONTENT)?;
        conn.save_history_checkpoint("workflow_artifacts", ARTIFACTS_CONTENT)
    }).map_err(unavailable)
}

pub(crate) fn load_workflow_runs(
    common_git_dir: &Path,
    repo_id: RepoId,
) -> Result<Vec<WorkflowRunRecord>, ApiError> {
    let root = workflows_storage_root(common_git_dir)?;
    let sqlite_path = root.join(WORKFLOWS_SQLITE_FILE_NAME);
    match fs::symlink_metadata(&sqlite_path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            return Err(unavailable("refusing to open symlinked workflows.sqlite3"));
        }
        Ok(_) => {
            let db = open_workflows_sqlite(&root)?;
            migrate_workflow_runs(&root, &db, repo_id)?;
            let rows = db
                .query_rows(
                    "SELECT repo_id, payload_json FROM workflow_runs ORDER BY seq DESC;",
                    &[],
                )
                .map_err(unavailable)?;
            let expected_repo = repo_id.0.to_string();
            let mut runs = Vec::with_capacity(rows.len());
            for row in rows {
                let row_repo = row
                    .first()
                    .and_then(SqlValue::as_str)
                    .ok_or_else(|| unavailable("missing workflow repo_id"))?;
                if row_repo != expected_repo {
                    return Err(unavailable("workflow runs storage identity mismatch"));
                }
                let payload = row
                    .get(1)
                    .and_then(SqlValue::as_str)
                    .ok_or_else(|| unavailable("missing workflow payload_json"))?;
                let run: WorkflowRunRecord = serde_json::from_str(payload).map_err(unavailable)?;
                runs.push(run);
            }
            return Ok(runs);
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(unavailable(e)),
    }

    let path = root.join(RUNS_FILE_NAME);
    let Some(bytes) = read_bounded_regular_file(&path, MAX_RUNS_JSON_BYTES)? else {
        return Ok(Vec::new());
    };
    let env: StoredRunsEnvelope = serde_json::from_slice(&bytes).map_err(unavailable)?;
    if env.version != 1 || env.repo_id != repo_id || env.sqlite_required {
        return Err(unavailable(
            "workflow runs storage identity mismatch or missing SQLite history",
        ));
    }
    Ok(env.runs)
}

pub(crate) fn save_workflow_run(
    common_git_dir: &Path,
    repo_id: RepoId,
    run: &WorkflowRunRecord,
) -> Result<(), ApiError> {
    let root = workflows_storage_root(common_git_dir)?;
    let existing_runs = load_workflow_runs(common_git_dir, repo_id)?;

    let artifacts: Vec<WorkflowArtifactEntry> = run
        .jobs
        .iter()
        .flat_map(|j| j.artifacts.iter().cloned())
        .collect();
    let art_env = StoredArtifactsEnvelope {
        version: 1,
        run_id: run.run_id.clone(),
        artifacts,
    };
    let art_payload = serde_json::to_vec_pretty(&art_env).map_err(unavailable)?;
    if art_payload.len() as u64 > MAX_ARTIFACTS_JSON_BYTES {
        return Err(unavailable(
            "workflow artifacts metadata exceeds size limit",
        ));
    }
    let art_json_str = String::from_utf8(art_payload.clone()).map_err(unavailable)?;
    let run_json_str = serde_json::to_string(run).map_err(unavailable)?;

    let db = open_workflows_sqlite(&root)?;
    migrate_workflow_runs(&root, &db, repo_id)?;
    let repo_id_str = repo_id.0.to_string();
    db.with_transaction(|conn| {
        if !conn.verify_history_checkpoint("workflow_runs", WORKFLOWS_CONTENT)?
            || !conn.verify_history_checkpoint("workflow_artifacts", ARTIFACTS_CONTENT)?
        {
            return Err("workflow history checkpoint is missing".into());
        }
        conn.execute(
            "INSERT OR REPLACE INTO workflow_runs (seq, run_id, repo_id, payload_json)
             VALUES ((SELECT seq FROM workflow_runs WHERE run_id = ?1), ?1, ?2, ?3);",
            &[
                SqlValue::Text(run.run_id.clone()),
                SqlValue::Text(repo_id_str),
                SqlValue::Text(run_json_str),
            ],
        )?;
        conn.execute(
            "INSERT OR REPLACE INTO workflow_artifacts (run_id, payload_json)
             VALUES (?1, ?2);",
            &[
                SqlValue::Text(run.run_id.clone()),
                SqlValue::Text(art_json_str),
            ],
        )?;
        conn.save_history_checkpoint("workflow_runs", WORKFLOWS_CONTENT)?;
        conn.save_history_checkpoint("workflow_artifacts", ARTIFACTS_CONTENT)
    })
    .map_err(unavailable)?;

    let mut mirror_runs = existing_runs;
    mirror_runs.retain(|r| r.run_id != run.run_id);
    mirror_runs.insert(0, run.clone());
    mirror_runs.truncate(JSON_MIRROR_WINDOW_RUNS);
    let runs_env = StoredRunsEnvelope {
        version: 1,
        sqlite_required: true,
        repo_id,
        runs: mirror_runs,
    };
    let runs_payload = serde_json::to_vec_pretty(&runs_env).map_err(unavailable)?;
    if runs_payload.len() as u64 <= MAX_RUNS_JSON_BYTES {
        atomic_write_regular_file(&root, &root.join(RUNS_FILE_NAME), &runs_payload)?;
    }

    let artifacts_dir = root.join(ARTIFACTS_DIR_NAME);
    ensure_safe_dir(&artifacts_dir)?;
    let art_path = artifacts_dir.join(format!("{}.json", run.run_id));
    atomic_write_regular_file(&artifacts_dir, &art_path, &art_payload)
}

pub(crate) fn load_workflow_run_artifacts(
    common_git_dir: &Path,
    clean_run_id: &str,
) -> Result<Vec<WorkflowArtifactEntry>, ApiError> {
    let root = workflows_storage_root(common_git_dir)?;
    let sqlite_path = root.join(WORKFLOWS_SQLITE_FILE_NAME);
    match fs::symlink_metadata(&sqlite_path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            return Err(unavailable("refusing to open symlinked workflows.sqlite3"));
        }
        Ok(_) => {
            let db = open_workflows_sqlite(&root)?;
            db.with_transaction(|conn| {
                let marker = conn.query_rows(
                    "SELECT value FROM store_metadata WHERE key = 'json_migration_v1';",
                    &[],
                )?;
                if !marker.is_empty()
                    && !conn.verify_history_checkpoint("workflow_artifacts", ARTIFACTS_CONTENT)?
                {
                    return Err("workflow artifact checkpoint is missing".into());
                }
                Ok(())
            })
            .map_err(unavailable)?;
            let rows = db
                .query_rows(
                    "SELECT payload_json FROM workflow_artifacts WHERE run_id = ?1;",
                    &[SqlValue::Text(clean_run_id.to_owned())],
                )
                .map_err(unavailable)?;
            if let Some(row) = rows.first() {
                let json = row
                    .first()
                    .and_then(SqlValue::as_str)
                    .ok_or_else(|| unavailable("missing workflow artifacts payload_json"))?;
                let env: StoredArtifactsEnvelope =
                    serde_json::from_str(json).map_err(unavailable)?;
                if env.version != 1 || env.run_id != clean_run_id {
                    return Err(unavailable("workflow artifacts identity mismatch"));
                }
                return Ok(env.artifacts);
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(unavailable(e)),
    }

    let art_path = root
        .join(ARTIFACTS_DIR_NAME)
        .join(format!("{clean_run_id}.json"));
    let Some(bytes) = read_bounded_regular_file(&art_path, MAX_ARTIFACTS_JSON_BYTES)? else {
        return Err((
            StatusCode::NOT_FOUND,
            "workflow run artifacts not found".into(),
        ));
    };
    let env: StoredArtifactsEnvelope = serde_json::from_slice(&bytes).map_err(unavailable)?;
    if env.version != 1 || env.run_id != clean_run_id {
        return Err(unavailable("workflow artifacts identity mismatch"));
    }
    Ok(env.artifacts)
}
