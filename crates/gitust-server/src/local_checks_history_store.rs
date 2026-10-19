use super::{
    HistoryError, LocalChecksRecord, LocalChecksStatus, DIRECTORY, MAX_BYTES, MAX_RECORDS,
};
use gitust_core::{RepoId, SandboxRunId};
use gitust_db::{SqlValue, SqliteWalDb};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    ffi::CString,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::{MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
};
const TARGET: &str = "history.json";
const STAGING: &str = "history.pending";
pub const SQLITE_TARGET: &str = "history.sqlite3";
const LOCAL_CHECKS_SCHEMA_SQL: &str = "
CREATE TABLE IF NOT EXISTS local_checks_history (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id TEXT NOT NULL UNIQUE,
    repo_id TEXT NOT NULL,
    commit_sha TEXT NOT NULL,
    passed INTEGER NOT NULL,
    payload_json TEXT NOT NULL
);
";
static HISTORY_IO_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct History {
    version: u32,
    repo_id: RepoId,
    records: Vec<LocalChecksRecord>,
}

/// Synchronous bounded storage; async callers use spawn_blocking and retain their service lease.
/// `actual_repo_id` comes from the opened repository's persisted identity, never a request UUID.
pub struct LocalChecksHistoryStore {
    common: File,
    common_path: PathBuf,
    repo_id: RepoId,
}
impl LocalChecksHistoryStore {
    /// Pins the actual common Git directory without creating a history directory on reads.
    pub fn open(common: &Path, actual_repo_id: RepoId) -> Result<Self, HistoryError> {
        if actual_repo_id.0.is_nil() {
            return Err(HistoryError::InvalidStorage);
        }
        let common_file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
            .open(common)
            .map_err(|_| HistoryError::StorageUnavailable)?;
        if !common_file
            .metadata()
            .map_err(|_| HistoryError::StorageUnavailable)?
            .is_dir()
        {
            return Err(HistoryError::InvalidStorage);
        }
        Ok(Self {
            common: common_file,
            common_path: common.to_path_buf(),
            repo_id: actual_repo_id,
        })
    }
    pub fn list(&self) -> Result<Vec<LocalChecksRecord>, HistoryError> {
        let _guard = HISTORY_IO_LOCK.lock();
        self.list_inner()
    }
    pub fn list_all_wal(&self) -> Result<Vec<LocalChecksRecord>, HistoryError> {
        let _guard = HISTORY_IO_LOCK.lock();
        let mut out = self.list_inner()?;
        let Some(directory) = self.directory(false)? else {
            return Ok(out);
        };
        let sqlite_path = self
            .verified_history_dir_path(&directory)?
            .join(SQLITE_TARGET);
        match fs::symlink_metadata(&sqlite_path) {
            Ok(meta) if meta.file_type().is_symlink() || !meta.file_type().is_file() => {
                return Err(HistoryError::InvalidStorage);
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
            Err(_err) => return Err(HistoryError::StorageUnavailable),
        }
        let db = SqliteWalDb::open(&sqlite_path).map_err(|_| HistoryError::InvalidStorage)?;
        db.execute_batch(LOCAL_CHECKS_SCHEMA_SQL)
            .map_err(|_| HistoryError::InvalidStorage)?;
        let rows = db
            .query_rows(
                "SELECT repo_id, payload_json FROM local_checks_history ORDER BY seq DESC;",
                &[],
            )
            .map_err(|_| HistoryError::InvalidStorage)?;
        let expected_repo = self.repo_id.0.to_string();
        let mut seen: HashSet<SandboxRunId> = out.iter().map(|r| r.result.run_id).collect();
        for row in rows {
            let row_repo = row
                .first()
                .and_then(SqlValue::as_str)
                .ok_or(HistoryError::InvalidStorage)?;
            if row_repo != expected_repo {
                return Err(HistoryError::InvalidStorage);
            }
            let payload = row
                .get(1)
                .and_then(SqlValue::as_str)
                .ok_or(HistoryError::InvalidStorage)?;
            let record: LocalChecksRecord =
                serde_json::from_str(payload).map_err(|_| HistoryError::InvalidStorage)?;
            if !record.valid() {
                return Err(HistoryError::InvalidStorage);
            }
            if seen.insert(record.result.run_id) {
                out.push(record);
            }
        }
        Ok(out)
    }
    fn list_inner(&self) -> Result<Vec<LocalChecksRecord>, HistoryError> {
        let Some(directory) = self.directory(false)? else {
            return Ok(Vec::new());
        };
        let file =
            open_at(&directory, TARGET, libc::O_RDONLY)?.ok_or(HistoryError::InvalidStorage)?;
        let before = regular(&file, false)?;
        let mut bytes = Vec::with_capacity(before.len() as usize);
        (&file)
            .take(MAX_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| HistoryError::StorageUnavailable)?;
        let after = regular(&file, false)?;
        if bytes.len() as u64 != before.len() || before.len() != after.len() {
            return Err(HistoryError::InvalidStorage);
        }
        let history: History =
            serde_json::from_slice(&bytes).map_err(|_| HistoryError::InvalidStorage)?;
        let mut ids = HashSet::new();
        if history.version != 1
            || history.repo_id != self.repo_id
            || history.records.len() > MAX_RECORDS
            || history
                .records
                .iter()
                .any(|record| !record.valid() || !ids.insert(record.result.run_id))
        {
            return Err(HistoryError::InvalidStorage);
        }
        Ok(history.records)
    }
    pub fn get(&self, run: SandboxRunId) -> Result<Option<LocalChecksRecord>, HistoryError> {
        if run.0.is_nil() {
            return Err(HistoryError::InvalidRecord);
        }
        Ok(self
            .list()?
            .into_iter()
            .find(|record| record.result.run_id == run))
    }
    pub fn append(&self, record: LocalChecksRecord) -> Result<(), HistoryError> {
        if !record.valid() {
            return Err(HistoryError::InvalidRecord);
        }
        let _guard = HISTORY_IO_LOCK.lock();
        let mut records = self.list_inner()?;
        if let Some(existing) = records
            .iter()
            .find(|saved| saved.result.run_id == record.result.run_id)
        {
            let mut retry = record;
            retry.completed_at = existing.completed_at;
            if encode(existing)? != encode(&retry)? {
                return Err(HistoryError::ConflictingRun);
            }
            let directory = self.directory(false)?.ok_or(HistoryError::InvalidStorage)?;
            sync(&directory)?;
            return sync(&self.common);
        }
        let record_for_wal = record.clone();
        records.insert(0, record);
        records.truncate(MAX_RECORDS);
        let payload = encode(&History {
            version: 1,
            repo_id: self.repo_id,
            records,
        })?;
        if payload.len() as u64 > MAX_BYTES {
            return Err(HistoryError::InvalidRecord);
        }
        let directory = self
            .directory(true)?
            .ok_or(HistoryError::StorageUnavailable)?;
        // The caller's service lease excludes other writers. Keep a single bounded crash leftover.
        if let Some(stale) = open_at(&directory, STAGING, libc::O_RDONLY)? {
            regular(&stale, true)?;
            unlink_at(&directory, STAGING)?;
        }
        let mut file = open_at(
            &directory,
            STAGING,
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
        )?
        .ok_or(HistoryError::StorageUnavailable)?;
        file.write_all(&payload)
            .map_err(|_| HistoryError::StorageUnavailable)?;
        sync(&file)?;
        regular(&file, false)?;
        let staging = name(STAGING)?;
        let target = name(TARGET)?;
        // SAFETY: both fixed leaf names live within the pinned private history directory.
        if unsafe {
            libc::renameat(
                directory.as_raw_fd(),
                staging.as_ptr(),
                directory.as_raw_fd(),
                target.as_ptr(),
            )
        } != 0
        {
            return Err(HistoryError::StorageUnavailable);
        }
        self.append_sqlite_wal(&directory, &record_for_wal)?;
        sync(&directory)?;
        sync(&self.common)
    }
    fn verified_history_dir_path(&self, directory: &File) -> Result<PathBuf, HistoryError> {
        let fd_meta = directory
            .metadata()
            .map_err(|_| HistoryError::StorageUnavailable)?;
        let dir_path = self.common_path.join(DIRECTORY);
        let path_meta =
            fs::symlink_metadata(&dir_path).map_err(|_| HistoryError::StorageUnavailable)?;
        if path_meta.file_type().is_symlink()
            || !path_meta.is_dir()
            || fd_meta.dev() != path_meta.dev()
            || fd_meta.ino() != path_meta.ino()
        {
            return Err(HistoryError::InvalidStorage);
        }
        Ok(dir_path)
    }
    fn append_sqlite_wal(
        &self,
        directory: &File,
        record: &LocalChecksRecord,
    ) -> Result<(), HistoryError> {
        let sqlite_path = self
            .verified_history_dir_path(directory)?
            .join(SQLITE_TARGET);
        let db = SqliteWalDb::open(&sqlite_path).map_err(|_| HistoryError::StorageUnavailable)?;
        db.execute_batch(LOCAL_CHECKS_SCHEMA_SQL)
            .map_err(|_| HistoryError::StorageUnavailable)?;
        let payload_json =
            serde_json::to_string(record).map_err(|_| HistoryError::InvalidRecord)?;
        let passed = i64::from(record.status == LocalChecksStatus::Passed);
        db.with_transaction(|conn| {
            conn.execute(
                "INSERT OR REPLACE INTO local_checks_history
                 (run_id, repo_id, commit_sha, passed, payload_json)
                 VALUES (?1, ?2, ?3, ?4, ?5);",
                &[
                    SqlValue::Text(record.result.run_id.0.to_string()),
                    SqlValue::Text(self.repo_id.0.to_string()),
                    SqlValue::Text(record.checked_commit.0.clone()),
                    SqlValue::Integer(passed),
                    SqlValue::Text(payload_json),
                ],
            )
        })
        .map_err(|_| HistoryError::StorageUnavailable)
    }
    fn directory(&self, create: bool) -> Result<Option<File>, HistoryError> {
        if let Some(directory) =
            open_at(&self.common, DIRECTORY, libc::O_RDONLY | libc::O_DIRECTORY)?
        {
            private_directory(&directory)?;
            return Ok(Some(directory));
        }
        if !create {
            return Ok(None);
        }
        let directory_name = name(DIRECTORY)?;
        // SAFETY: mkdirat receives a fixed leaf and a pinned common-directory descriptor.
        if unsafe { libc::mkdirat(self.common.as_raw_fd(), directory_name.as_ptr(), 0o700) } != 0 {
            return Err(HistoryError::StorageUnavailable);
        }
        // A missing committed snapshot in an existing directory is incomplete storage, not empty history.
        sync(&self.common)?;
        let directory = open_at(&self.common, DIRECTORY, libc::O_RDONLY | libc::O_DIRECTORY)?
            .ok_or(HistoryError::StorageUnavailable)?;
        private_directory(&directory)?;
        Ok(Some(directory))
    }
}
fn name(value: &str) -> Result<CString, HistoryError> {
    CString::new(value).map_err(|_| HistoryError::InvalidStorage)
}
fn open_at(parent: &File, leaf: &str, flags: libc::c_int) -> Result<Option<File>, HistoryError> {
    let leaf = name(leaf)?;
    // SAFETY: fixed NUL-terminated leaf, live parent fd, and a creation mode for O_CREAT.
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            leaf.as_ptr(),
            flags | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
            0o600,
        )
    };
    if fd < 0 {
        if std::io::Error::last_os_error().kind() == std::io::ErrorKind::NotFound {
            return Ok(None);
        }
        return Err(HistoryError::StorageUnavailable);
    }
    // SAFETY: openat returned a fresh owned descriptor.
    Ok(Some(unsafe { File::from_raw_fd(fd) }))
}
fn private_directory(file: &File) -> Result<(), HistoryError> {
    let meta = file
        .metadata()
        .map_err(|_| HistoryError::StorageUnavailable)?;
    // SAFETY: geteuid has no memory arguments.
    let uid = unsafe { libc::geteuid() };
    if !meta.is_dir() || meta.mode() & 0o7777 != 0o700 || meta.uid() != uid {
        return Err(HistoryError::InvalidStorage);
    }
    Ok(())
}
fn regular(file: &File, allow_empty: bool) -> Result<fs::Metadata, HistoryError> {
    let meta = file
        .metadata()
        .map_err(|_| HistoryError::StorageUnavailable)?;
    // SAFETY: geteuid has no memory arguments.
    let uid = unsafe { libc::geteuid() };
    if !meta.is_file()
        || meta.mode() & 0o7777 != 0o600
        || meta.uid() != uid
        || meta.nlink() != 1
        || (!allow_empty && meta.len() == 0)
        || meta.len() > MAX_BYTES
    {
        return Err(HistoryError::InvalidStorage);
    }
    Ok(meta)
}
fn unlink_at(parent: &File, leaf: &str) -> Result<(), HistoryError> {
    let leaf = name(leaf)?;
    // SAFETY: the caller checked the fixed staging file while retaining service ownership.
    if unsafe { libc::unlinkat(parent.as_raw_fd(), leaf.as_ptr(), 0) } != 0 {
        return Err(HistoryError::StorageUnavailable);
    }
    Ok(())
}
fn sync(file: &File) -> Result<(), HistoryError> {
    file.sync_all()
        .map_err(|_| HistoryError::StorageUnavailable)
}
fn encode(value: &impl Serialize) -> Result<Vec<u8>, HistoryError> {
    serde_json::to_vec(value).map_err(|_| HistoryError::InvalidRecord)
}
