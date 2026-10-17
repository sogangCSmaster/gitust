//! Private bounded repository-local intents; all operations run on blocking workers.
use super::{automatic_run_model::Document, state::MAX_TRACKED_WORKTREES};
use axum::http::StatusCode;
use gitust_core::{RepoId, WorktreeId};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    ffi::CString,
    fs::{self, File},
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::{MetadataExt, OpenOptionsExt},
    },
    path::Path,
    sync::Arc,
};

pub(crate) const DIRECTORY: &str = "gitust-automatic-runs";
const MAX_BYTES: u64 = 64 * 1024;
pub(crate) type Failure = (StatusCode, String);
fn invalid() -> Failure {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        "automatic Checks storage is unavailable; restore trusted scheduling metadata".into(),
    )
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Header {
    version: u32,
    repo_id: RepoId,
}
pub(crate) struct Store {
    directory: File,
    #[cfg(test)]
    path: std::path::PathBuf,
    repo_id: RepoId,
    slots: Mutex<HashMap<WorktreeId, Arc<Mutex<Slot>>>>,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Slot {
    Absent,
    Present,
    Retired,
}
fn name(value: &str) -> Result<CString, Failure> {
    CString::new(value).map_err(|_| invalid())
}
fn open_at(dir: &File, leaf: &str, flags: i32) -> Result<Option<File>, Failure> {
    let leaf = name(leaf)?;
    // SAFETY: the pinned directory and NUL-terminated leaf are valid; O_CREAT receives a mode.
    let fd = unsafe {
        libc::openat(
            dir.as_raw_fd(),
            leaf.as_ptr(),
            flags | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
            0o600,
        )
    };
    if fd < 0 {
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::NotFound && flags & libc::O_CREAT == 0 {
            return Ok(None);
        }
        return Err(invalid());
    }
    // SAFETY: openat returned a fresh owned descriptor.
    Ok(Some(unsafe { File::from_raw_fd(fd) }))
}
fn validate(file: &File, directory: bool) -> Result<(), Failure> {
    let m = file.metadata().map_err(|_| invalid())?;
    // SAFETY: geteuid has no memory or ownership arguments.
    let uid = unsafe { libc::geteuid() };
    if m.uid() != uid
        || (directory && (!m.is_dir() || m.mode() & 0o7777 != 0o700))
        || (!directory
            && (!m.is_file()
                || m.mode() & 0o7777 != 0o600
                || m.nlink() != 1
                || m.len() > MAX_BYTES))
    {
        return Err(invalid());
    }
    Ok(())
}
fn read_bytes(dir: &File, leaf: &str) -> Result<Option<Vec<u8>>, Failure> {
    let Some(mut file) = open_at(dir, leaf, libc::O_RDONLY)? else {
        return Ok(None);
    };
    validate(&file, false)?;
    let before = file.metadata().map_err(|_| invalid())?.len();
    let mut bytes = Vec::new();
    (&mut file)
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| invalid())?;
    validate(&file, false)?;
    if bytes.len() as u64 != before || before != file.metadata().map_err(|_| invalid())?.len() {
        return Err(invalid());
    }
    Ok(Some(bytes))
}
fn unlink(dir: &File, leaf: &str) -> Result<(), Failure> {
    let leaf = name(leaf)?;
    // SAFETY: unlinkat only addresses this pinned directory's validated leaf.
    if unsafe { libc::unlinkat(dir.as_raw_fd(), leaf.as_ptr(), 0) } != 0 {
        return Err(invalid());
    }
    Ok(())
}
fn publish(dir: &File, leaf: &str, bytes: &[u8]) -> Result<(), Failure> {
    if bytes.len() as u64 > MAX_BYTES {
        return Err(invalid());
    }
    let pending = format!("{leaf}.pending");
    if let Some(file) = open_at(dir, &pending, libc::O_RDONLY)? {
        validate(&file, false)?;
        unlink(dir, &pending)?;
    }
    let mut file = open_at(dir, &pending, libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL)?
        .ok_or_else(invalid)?;
    file.write_all(bytes).map_err(|_| invalid())?;
    file.sync_all().map_err(|_| invalid())?;
    let from = name(&pending)?;
    let to = name(leaf)?;
    // SAFETY: fixed validated leaf names remain in one pinned private directory.
    if unsafe { libc::renameat(dir.as_raw_fd(), from.as_ptr(), dir.as_raw_fd(), to.as_ptr()) } != 0
    {
        return Err(invalid());
    }
    dir.sync_all().map_err(|_| invalid())
}
impl Store {
    pub fn open(common: &Path, repo_id: RepoId) -> Result<Self, Failure> {
        let parent = File::options()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(common)
            .map_err(|_| invalid())?;
        let mut created = false;
        let directory = match open_at(&parent, DIRECTORY, libc::O_RDONLY | libc::O_DIRECTORY)? {
            Some(file) => file,
            None => {
                let leaf = name(DIRECTORY)?;
                // SAFETY: parent pins common Git metadata and the fixed directory leaf is valid.
                if unsafe { libc::mkdirat(parent.as_raw_fd(), leaf.as_ptr(), 0o700) } != 0 {
                    return Err(invalid());
                }
                parent.sync_all().map_err(|_| invalid())?;
                created = true;
                open_at(&parent, DIRECTORY, libc::O_RDONLY | libc::O_DIRECTORY)?
                    .ok_or_else(invalid)?
            }
        };
        validate(&directory, true)?;
        if created {
            publish(
                &directory,
                "store.json",
                &serde_json::to_vec(&Header {
                    version: 1,
                    repo_id,
                })
                .map_err(|_| invalid())?,
            )?;
        }
        let bytes = read_bytes(&directory, "store.json")?.ok_or_else(invalid)?;
        let header: Header = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
        if header.version != 1 || header.repo_id != repo_id || repo_id.0.is_nil() {
            return Err(invalid());
        }
        let path = common.join(DIRECTORY);
        let mut slots = HashMap::new();
        let mut count = 0usize;
        for entry in fs::read_dir(&path).map_err(|_| invalid())? {
            count += 1;
            if count > MAX_TRACKED_WORKTREES * 2 + 2 {
                return Err(invalid());
            }
            let leaf = entry
                .map_err(|_| invalid())?
                .file_name()
                .into_string()
                .map_err(|_| invalid())?;
            if leaf == "store.json" {
                continue;
            }
            let stem = match leaf.strip_suffix(".pending") {
                Some(stem) => stem,
                None => &leaf,
            };
            let stem = stem.strip_suffix(".json").ok_or_else(invalid)?;
            let id = WorktreeId(uuid::Uuid::parse_str(stem).map_err(|_| invalid())?);
            if id.0.is_nil() {
                return Err(invalid());
            }
            let bytes = read_bytes(&directory, &leaf)?.ok_or_else(invalid)?;
            if leaf.ends_with(".pending") {
                continue;
            }
            let doc: Document = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
            if !doc.valid(repo_id, id)
                || slots
                    .insert(id, Arc::new(Mutex::new(Slot::Present)))
                    .is_some()
            {
                return Err(invalid());
            }
        }
        if slots.len() > MAX_TRACKED_WORKTREES {
            return Err(invalid());
        }
        Ok(Self {
            directory,
            #[cfg(test)]
            path,
            repo_id,
            slots: Mutex::new(slots),
        })
    }
    fn slot(&self, id: WorktreeId) -> Result<Arc<Mutex<Slot>>, Failure> {
        let mut slots = self.slots.lock();
        if let Some(slot) = slots.get(&id) {
            return Ok(slot.clone());
        }
        if slots.len() >= MAX_TRACKED_WORKTREES {
            return Err(invalid());
        }
        let slot = Arc::new(Mutex::new(Slot::Absent));
        slots.insert(id, slot.clone());
        Ok(slot)
    }
    pub fn update<T>(
        &self,
        id: WorktreeId,
        branch: &str,
        base: &str,
        change: impl FnOnce(&mut Document) -> Result<T, Failure>,
    ) -> Result<T, Failure> {
        validate(&self.directory, true)?;
        let slot = self.slot(id)?;
        let mut written = slot.lock();
        if *written == Slot::Retired {
            return Err(invalid());
        }
        let leaf = format!("{}.json", id.0);
        let mut doc = match read_bytes(&self.directory, &leaf)? {
            Some(bytes) => serde_json::from_slice::<Document>(&bytes).map_err(|_| invalid())?,
            None if *written == Slot::Absent => Document::new(self.repo_id, id, branch, base),
            None => return Err(invalid()),
        };
        if !doc.valid(self.repo_id, id) || doc.branch != branch || doc.base_branch != base {
            return Err(invalid());
        }
        let result = change(&mut doc)?;
        if !doc.valid(self.repo_id, id) {
            return Err(invalid());
        }
        publish(
            &self.directory,
            &leaf,
            &serde_json::to_vec(&doc).map_err(|_| invalid())?,
        )?;
        *written = Slot::Present;
        Ok(result)
    }
    pub fn document(
        &self,
        id: WorktreeId,
        branch: &str,
        base: &str,
    ) -> Result<Option<Document>, Failure> {
        validate(&self.directory, true)?;
        let slot = match self.slots.lock().get(&id).cloned() {
            Some(slot) => slot,
            None => return Ok(None),
        };
        let written = slot.lock();
        if *written == Slot::Retired {
            return Err(invalid());
        }
        let bytes = match read_bytes(&self.directory, &format!("{}.json", id.0))? {
            Some(bytes) => bytes,
            None if *written == Slot::Absent => return Ok(None),
            None => return Err(invalid()),
        };
        let doc: Document = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
        if !doc.valid(self.repo_id, id) || doc.branch != branch || doc.base_branch != base {
            return Err(invalid());
        }
        Ok(Some(doc))
    }
    pub fn documents(&self) -> Result<Vec<Document>, Failure> {
        validate(&self.directory, true)?;
        let ids: Vec<_> = self
            .slots
            .lock()
            .iter()
            .map(|(id, slot)| (*id, slot.clone()))
            .collect();
        let mut docs = Vec::new();
        for (id, slot) in ids {
            let written = slot.lock();
            let bytes = read_bytes(&self.directory, &format!("{}.json", id.0))?;
            match bytes {
                Some(bytes) => {
                    let doc: Document = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
                    if !doc.valid(self.repo_id, id) {
                        return Err(invalid());
                    }
                    docs.push(doc);
                }
                None if *written == Slot::Absent => {}
                None => return Err(invalid()),
            }
        }
        Ok(docs)
    }
    pub fn remove(&self, id: WorktreeId) -> Result<(), Failure> {
        validate(&self.directory, true)?;
        let slot = self.slot(id)?;
        let mut written = slot.lock();
        if *written == Slot::Retired {
            return Err(invalid());
        }
        for leaf in [format!("{}.json", id.0), format!("{}.json.pending", id.0)] {
            if let Some(file) = open_at(&self.directory, &leaf, libc::O_RDONLY)? {
                validate(&file, false)?;
                unlink(&self.directory, &leaf)?;
            } else if *written == Slot::Present && leaf.ends_with(".json") {
                return Err(invalid());
            }
        }
        self.directory.sync_all().map_err(|_| invalid())?;
        *written = Slot::Retired;
        self.slots.lock().remove(&id);
        Ok(())
    }
    #[cfg(test)]
    pub fn path(&self) -> &Path {
        &self.path
    }
}
