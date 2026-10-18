use super::{HistoryError, LocalChecksRecord, DIRECTORY, MAX_BYTES, MAX_RECORDS};
use gitust_core::{RepoId, SandboxRunId};
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
    path::Path,
};
const TARGET: &str = "history.json";
const STAGING: &str = "history.pending";
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
    repo_id: RepoId,
}
impl LocalChecksHistoryStore {
    /// Pins the actual common Git directory without creating a history directory on reads.
    pub fn open(common: &Path, actual_repo_id: RepoId) -> Result<Self, HistoryError> {
        if actual_repo_id.0.is_nil() {
            return Err(HistoryError::InvalidStorage);
        }
        let common = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
            .open(common)
            .map_err(|_| HistoryError::StorageUnavailable)?;
        if !common
            .metadata()
            .map_err(|_| HistoryError::StorageUnavailable)?
            .is_dir()
        {
            return Err(HistoryError::InvalidStorage);
        }
        Ok(Self {
            common,
            repo_id: actual_repo_id,
        })
    }
    pub fn list(&self) -> Result<Vec<LocalChecksRecord>, HistoryError> {
        let _guard = HISTORY_IO_LOCK.lock();
        self.list_inner()
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
        sync(&directory)?;
        sync(&self.common)
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
