//! Descriptor-relative, private import metadata and atomic no-overwrite directory publication.
use anyhow::{bail, Result};
use serde::{de::DeserializeOwned, Serialize};
use std::{
    ffi::CString,
    fs::{self, File},
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::{MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
};

// A 32KiB UTF-8 branch can double when JSON quotes are escaped, plus fixed record fields.
pub(crate) const LIMIT: u64 = 128 * 1024;
pub(crate) fn error() -> anyhow::Error {
    anyhow::anyhow!("import metadata is unavailable or unsafe; preserve it for operator recovery")
}
fn name(leaf: &str) -> Result<CString> {
    if leaf.is_empty() || leaf == "." || leaf == ".." || leaf.contains('/') {
        return Err(error());
    }
    CString::new(leaf).map_err(|_| error())
}
pub(crate) struct Directory {
    pub file: File,
    pub path: PathBuf,
}

pub(crate) struct ImportClaim(File);

impl Drop for ImportClaim {
    fn drop(&mut self) {
        // fail-open-ok: releasing this owner's claim cannot authorize another mutation here; explicit unlock prevents inherited descriptors prolonging ownership, and failure only retains the lock until all copies close.
        let _ = self.0.unlock();
    }
}

impl Directory {
    pub fn parent(path: &Path) -> Result<Self> {
        let file = File::options()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)
            .map_err(|_| error())?;
        Ok(Self {
            file,
            path: path.into(),
        })
    }
    pub fn same_path(&self) -> Result<()> {
        let before = self.file.metadata().map_err(|_| error())?;
        let after = fs::symlink_metadata(&self.path).map_err(|_| error())?;
        if !after.is_dir() || before.dev() != after.dev() || before.ino() != after.ino() {
            return Err(error());
        }
        Ok(())
    }
    pub fn sync(&self) -> Result<()> {
        self.file.sync_all().map_err(|_| error())
    }
    pub fn open(&self, leaf: &str, flags: i32) -> Result<Option<File>> {
        let leaf = name(leaf)?;
        // SAFETY: a pinned directory and one validated NUL-terminated leaf; fd is newly owned.
        let fd = unsafe {
            libc::openat(
                self.file.as_raw_fd(),
                leaf.as_ptr(),
                flags | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
                0o600,
            )
        };
        if fd < 0 {
            let e = std::io::Error::last_os_error();
            if e.kind() == std::io::ErrorKind::NotFound && flags & libc::O_CREAT == 0 {
                return Ok(None);
            }
            return Err(error());
        }
        Ok(Some(unsafe { File::from_raw_fd(fd) }))
    }
    pub fn child(&self, leaf: &str, create: bool) -> Result<(Self, bool)> {
        let leaf_name = name(leaf)?;
        let mut created = false;
        if create {
            // SAFETY: validated single leaf and pinned directory.
            if unsafe { libc::mkdirat(self.file.as_raw_fd(), leaf_name.as_ptr(), 0o700) } == 0 {
                created = true;
                self.sync()?;
            } else if std::io::Error::last_os_error().kind() != std::io::ErrorKind::AlreadyExists {
                return Err(error());
            }
        }
        let file = self
            .open(leaf, libc::O_RDONLY | libc::O_DIRECTORY)?
            .ok_or_else(error)?;
        let meta = file.metadata().map_err(|_| error())?;
        if meta.mode() & 0o7777 != 0o700 || meta.uid() != unsafe { libc::geteuid() } {
            return Err(error());
        }
        Ok((
            Self {
                file,
                path: self.path.join(leaf),
            },
            created,
        ))
    }
    pub fn claim(&self, leaf: &str) -> Result<ImportClaim> {
        let file = self
            .open(leaf, libc::O_RDWR | libc::O_CREAT)?
            .ok_or_else(error)?;
        validate(&file, false)?;
        match file.try_lock() {
            Ok(()) => {}
            Err(fs::TryLockError::WouldBlock) => {
                bail!("another import owns this destination");
            }
            Err(fs::TryLockError::Error(_)) => return Err(error()),
        }
        let claim = ImportClaim(file);
        claim.0.sync_all().map_err(|_| error())?;
        self.sync()?;
        Ok(claim)
    }
    pub fn read<T: DeserializeOwned>(&self, leaf: &str) -> Result<Option<T>> {
        let Some(mut file) = self.open(leaf, libc::O_RDONLY)? else {
            return Ok(None);
        };
        let before = validate(&file, true)?;
        let mut bytes = Vec::new();
        (&mut file)
            .take(LIMIT + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| error())?;
        let after = validate(&file, true)?;
        if bytes.len() as u64 != before.len() || before.len() != after.len() {
            return Err(error());
        }
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|_| error())
    }
    pub fn write_new<T: Serialize>(&self, leaf: &str, value: &T) -> Result<()> {
        let bytes = serde_json::to_vec(value).map_err(|_| error())?;
        if bytes.is_empty() || bytes.len() as u64 > LIMIT {
            return Err(error());
        }
        let pending = format!("{leaf}.pending");
        let mut file = self
            .open(&pending, libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL)?
            .ok_or_else(error)?;
        file.write_all(&bytes).map_err(|_| error())?;
        file.sync_all().map_err(|_| error())?;
        validate(&file, true)?;
        self.sync()?;
        self.rename_exclusive(&pending, self, leaf)?;
        self.sync()
    }
    pub fn remove_known(&self, leaf: &str) -> Result<()> {
        let leaf = name(leaf)?;
        // SAFETY: only callers that decoded and validated this fixed record may remove it.
        if unsafe { libc::unlinkat(self.file.as_raw_fd(), leaf.as_ptr(), 0) } != 0 {
            return Err(error());
        }
        self.sync()
    }
    pub fn rename_exclusive(&self, from: &str, to: &Self, leaf: &str) -> Result<()> {
        self.same_path()?;
        to.same_path()?;
        let from = name(from)?;
        let leaf = name(leaf)?;
        // SAFETY: pinned directory descriptors, validated leaves, kernel no-replace semantics.
        #[cfg(target_os = "macos")]
        let rc = unsafe {
            libc::renameatx_np(
                self.file.as_raw_fd(),
                from.as_ptr(),
                to.file.as_raw_fd(),
                leaf.as_ptr(),
                libc::RENAME_EXCL,
            )
        };
        #[cfg(target_os = "linux")]
        let rc = unsafe {
            libc::renameat2(
                self.file.as_raw_fd(),
                from.as_ptr(),
                to.file.as_raw_fd(),
                leaf.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        bail!("atomic directory import publication is unsupported on this OS");
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        if rc != 0 {
            bail!("import destination already exists or publication failed; preserve the import intent");
        }
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        Ok(())
    }
}
fn validate(file: &File, nonempty: bool) -> Result<fs::Metadata> {
    let meta = file.metadata().map_err(|_| error())?;
    if !meta.is_file()
        || meta.uid() != unsafe { libc::geteuid() }
        || meta.mode() & 0o7777 != 0o600
        || meta.nlink() != 1
        || meta.len() > LIMIT
        || (nonempty && meta.len() == 0)
    {
        return Err(error());
    }
    Ok(meta)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn releasing_import_claim_does_not_wait_for_an_inherited_descriptor() {
        let root = tempfile::tempdir().unwrap();
        let directory = Directory::parent(root.path()).unwrap();
        let held = directory.claim("import.lock").unwrap();
        let inherited = held.0.try_clone().unwrap();
        assert!(directory.claim("import.lock").is_err());
        drop(held);
        // A forked child also retains the same open-file description until exec or exit.
        let next = directory
            .claim("import.lock")
            .expect("owner release must be immediate");
        drop(inherited);
        assert!(directory.claim("import.lock").is_err());
        drop(next);
        assert!(directory.claim("import.lock").is_ok());
    }
}
