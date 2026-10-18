//! A single bounded primary scheduling document, never a completed-result store.
use super::local_checks_automatic_model::{Document, DIRECTORY, MAX_BYTES};
use gitust_core::RepoId;
use parking_lot::Mutex;
use std::{
    ffi::CString,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::{MetadataExt, OpenOptionsExt},
    },
    path::Path,
};
#[derive(Clone, Copy, Debug)]
pub(crate) struct Error;
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("local automatic Checks storage is unavailable")
    }
}
impl std::error::Error for Error {}
const TARGET: &str = "state.json";
const STAGING: &str = "state.pending";
pub(crate) struct Store {
    common: File,
    repo_id: RepoId,
    mutation: Mutex<()>,
}
impl Store {
    pub fn open(common: &Path, repo_id: RepoId) -> Result<Self, Error> {
        if repo_id.0.is_nil() {
            return Err(Error);
        }
        let common = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
            .open(common)
            .map_err(|_| Error)?;
        if !common.metadata().map_err(|_| Error)?.is_dir() {
            return Err(Error);
        }
        Ok(Self {
            common,
            repo_id,
            mutation: Mutex::new(()),
        })
    }
    pub fn read(&self) -> Result<Document, Error> {
        let _guard = self.mutation.lock();
        self.read_inner()
    }
    fn read_inner(&self) -> Result<Document, Error> {
        let Some(dir) = self.directory(false)? else {
            return Ok(Document::new(self.repo_id));
        };
        let file = open_at(&dir, TARGET, libc::O_RDONLY)?.ok_or(Error)?;
        let before = regular(&file, false)?.len();
        let mut bytes = Vec::new();
        (&file)
            .take(MAX_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| Error)?;
        if bytes.len() as u64 != before || regular(&file, false)?.len() != before {
            return Err(Error);
        }
        let doc: Document = serde_json::from_slice(&bytes).map_err(|_| Error)?;
        if !doc.valid(self.repo_id) {
            return Err(Error);
        }
        Ok(doc)
    }
    pub fn update<T>(&self, f: impl FnOnce(&mut Document) -> Result<T, Error>) -> Result<T, Error> {
        let _guard = self.mutation.lock();
        let mut doc = self.read_inner()?;
        let result = f(&mut doc)?;
        if !doc.valid(self.repo_id) {
            return Err(Error);
        }
        let bytes = serde_json::to_vec(&doc).map_err(|_| Error)?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err(Error);
        }
        let dir = self.directory(true)?.ok_or(Error)?;
        if let Some(file) = open_at(&dir, STAGING, libc::O_RDONLY)? {
            regular(&file, true)?;
            unlink_at(&dir, STAGING)?;
        }
        let mut file =
            open_at(&dir, STAGING, libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL)?.ok_or(Error)?;
        file.write_all(&bytes).map_err(|_| Error)?;
        sync(&file)?;
        regular(&file, false)?;
        let from = name(STAGING)?;
        let to = name(TARGET)?;
        // SAFETY: fixed names in the same pinned private directory; publication follows file sync.
        if unsafe { libc::renameat(dir.as_raw_fd(), from.as_ptr(), dir.as_raw_fd(), to.as_ptr()) }
            != 0
        {
            return Err(Error);
        }
        sync(&dir)?;
        sync(&self.common)?;
        Ok(result)
    }
    fn directory(&self, create: bool) -> Result<Option<File>, Error> {
        if let Some(dir) = open_at(&self.common, DIRECTORY, libc::O_RDONLY | libc::O_DIRECTORY)? {
            private_directory(&dir)?;
            return Ok(Some(dir));
        }
        if !create {
            return Ok(None);
        }
        let leaf = name(DIRECTORY)?;
        // SAFETY: fixed leaf under the pinned common Git directory.
        if unsafe { libc::mkdirat(self.common.as_raw_fd(), leaf.as_ptr(), 0o700) } != 0 {
            return Err(Error);
        }
        sync(&self.common)?;
        let dir =
            open_at(&self.common, DIRECTORY, libc::O_RDONLY | libc::O_DIRECTORY)?.ok_or(Error)?;
        private_directory(&dir)?;
        Ok(Some(dir))
    }
}
fn name(value: &str) -> Result<CString, Error> {
    CString::new(value).map_err(|_| Error)
}
fn open_at(parent: &File, leaf: &str, flags: libc::c_int) -> Result<Option<File>, Error> {
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
        return Err(Error);
    }
    // SAFETY: openat returned a fresh owned descriptor.
    Ok(Some(unsafe { File::from_raw_fd(fd) }))
}
fn private_directory(file: &File) -> Result<(), Error> {
    let meta = file.metadata().map_err(|_| Error)?;
    // SAFETY: geteuid has no memory arguments.
    let uid = unsafe { libc::geteuid() };
    if !meta.is_dir() || meta.mode() & 0o7777 != 0o700 || meta.uid() != uid {
        return Err(Error);
    }
    Ok(())
}
fn regular(file: &File, allow_empty: bool) -> Result<fs::Metadata, Error> {
    let meta = file.metadata().map_err(|_| Error)?;
    // SAFETY: geteuid has no memory arguments.
    let uid = unsafe { libc::geteuid() };
    if !meta.is_file()
        || meta.mode() & 0o7777 != 0o600
        || meta.uid() != uid
        || meta.nlink() != 1
        || (!allow_empty && meta.len() == 0)
        || meta.len() > MAX_BYTES
    {
        return Err(Error);
    }
    Ok(meta)
}
fn unlink_at(parent: &File, leaf: &str) -> Result<(), Error> {
    let leaf = name(leaf)?;
    // SAFETY: the caller checked the fixed staging file while retaining service ownership.
    if unsafe { libc::unlinkat(parent.as_raw_fd(), leaf.as_ptr(), 0) } != 0 {
        return Err(Error);
    }
    Ok(())
}
fn sync(file: &File) -> Result<(), Error> {
    file.sync_all().map_err(|_| Error)
}
