//! A repository's local identity survives restarts and complete offline relocation.
use gitust_core::RepoId;
use serde::{Deserialize, Serialize};

use crate::{GitEngine, GitEngineError};

const DIRECTORY: &str = "gitust-repository";
const IDENTITY: &str = "identity.json";
const STAGING: &str = "identity.pending";
const MAX_BYTES: u64 = 4096;

fn invalid(reason: &str) -> GitEngineError {
    GitEngineError::RepositoryIdentity(format!(
        "{reason}; restore trusted repository identity metadata instead of creating a new ID"
    ))
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Identity {
    version: u32,
    repo_id: RepoId,
}

#[cfg(unix)]
pub(crate) mod storage {
    use std::{
        ffi::{CStr, CString},
        fs::{self, File},
        io::{Read, Write},
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::fs::{MetadataExt, OpenOptionsExt},
        },
        path::Path,
    };

    use super::*;

    pub(crate) fn name(value: &str) -> Result<CString, GitEngineError> {
        CString::new(value).map_err(|_| invalid("invalid identity metadata name"))
    }

    pub(crate) fn private_directory(file: &File) -> Result<(), GitEngineError> {
        let meta = file
            .metadata()
            .map_err(|_| invalid("identity directory is unreadable"))?;
        // SAFETY: geteuid has no memory arguments or ownership effects.
        let owner = unsafe { libc::geteuid() };
        if !meta.is_dir() || meta.mode() & 0o7777 != 0o700 || meta.uid() != owner {
            return Err(invalid(
                "identity directory must be owned privately with mode 0700",
            ));
        }
        Ok(())
    }

    pub(crate) fn open_at(
        parent: &File,
        name: &CStr,
        flags: libc::c_int,
    ) -> Result<Option<File>, GitEngineError> {
        // SAFETY: parent owns a live directory fd, name is NUL-terminated and mode is supplied
        // for O_CREAT. O_NOFOLLOW applies to the only path component accepted here.
        let fd = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                name.as_ptr(),
                flags | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
                0o600,
            )
        };
        if fd < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::NotFound && flags & libc::O_CREAT == 0 {
                return Ok(None);
            }
            return Err(invalid("identity metadata could not be opened safely"));
        }
        // SAFETY: openat returned a fresh owned descriptor.
        Ok(Some(unsafe { File::from_raw_fd(fd) }))
    }

    pub(crate) fn sync(file: &File) -> Result<(), GitEngineError> {
        file.sync_all()
            .map_err(|_| invalid("identity metadata could not be synchronized"))
    }

    pub(crate) fn validate_file(file: &File) -> Result<fs::Metadata, GitEngineError> {
        let meta = file
            .metadata()
            .map_err(|_| invalid("identity file is unreadable"))?;
        // SAFETY: geteuid has no memory arguments or ownership effects.
        let owner = unsafe { libc::geteuid() };
        if !meta.is_file()
            || meta.mode() & 0o7777 != 0o600
            || meta.uid() != owner
            || meta.nlink() != 1
            || meta.len() == 0
            || meta.len() > MAX_BYTES
        {
            return Err(invalid(
                "identity file must be a private bounded regular file with one link",
            ));
        }
        Ok(meta)
    }

    pub(crate) fn read(directory: &File) -> Result<RepoId, GitEngineError> {
        let mut file = open_at(directory, &name(IDENTITY)?, libc::O_RDONLY)?
            .ok_or_else(|| invalid("existing identity directory has no identity file"))?;
        let before = validate_file(&file)?;
        let mut bytes = Vec::with_capacity(before.len() as usize);
        (&mut file)
            .take(MAX_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| invalid("identity file could not be read"))?;
        let after = validate_file(&file)?;
        if bytes.len() as u64 != before.len() || before.len() != after.len() {
            return Err(invalid("identity file changed while being read"));
        }
        let record: Identity = serde_json::from_slice(&bytes)
            .map_err(|_| invalid("identity file has invalid metadata"))?;
        if record.version != 1 || record.repo_id.0.is_nil() {
            return Err(invalid("identity version or repository ID is invalid"));
        }
        Ok(record.repo_id)
    }

    pub(crate) fn publish(
        directory: &File,
        pending: &CStr,
        identity: &CStr,
    ) -> Result<(), GitEngineError> {
        // SAFETY: both names are fixed, NUL-terminated leaf names within the pinned directory.
        // No-replace publication never overwrites an identity created by another writer.
        #[cfg(target_os = "macos")]
        let result = unsafe {
            libc::renameatx_np(
                directory.as_raw_fd(),
                pending.as_ptr(),
                directory.as_raw_fd(),
                identity.as_ptr(),
                libc::RENAME_EXCL,
            )
        };
        #[cfg(target_os = "linux")]
        let result = unsafe {
            libc::renameat2(
                directory.as_raw_fd(),
                pending.as_ptr(),
                directory.as_raw_fd(),
                identity.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        return Err(invalid(
            "atomic repository identity publication is unsupported on this OS",
        ));
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        if result != 0 {
            return Err(invalid(
                "identity could not be published without replacing existing metadata",
            ));
        }
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        Ok(())
    }

    pub(super) fn load_existing(common: &Path) -> Result<RepoId, GitEngineError> {
        let parent = File::options()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(common)
            .map_err(|_| invalid("common Git directory could not be opened"))?;
        let directory = open_at(
            &parent,
            &name(DIRECTORY)?,
            libc::O_RDONLY | libc::O_DIRECTORY,
        )?
        .ok_or_else(|| invalid("repository identity must already exist"))?;
        private_directory(&directory)?;
        read(&directory)
    }

    pub(super) fn load_or_create(common: &Path) -> Result<RepoId, GitEngineError> {
        load_or_create_for_import(common, None)
    }

    pub(super) fn load_or_create_for_import(
        common: &Path,
        expected: Option<RepoId>,
    ) -> Result<RepoId, GitEngineError> {
        let parent = File::options()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(common)
            .map_err(|_| invalid("common Git directory could not be opened"))?;
        let directory_name = name(DIRECTORY)?;
        if let Some(directory) =
            open_at(&parent, &directory_name, libc::O_RDONLY | libc::O_DIRECTORY)?
        {
            private_directory(&directory)?;
            let id = read(&directory)?;
            if expected.is_some_and(|expected| expected != id) {
                return Err(invalid(
                    "repository identity does not match the import intent",
                ));
            }
            sync(&directory)?;
            sync(&parent)?;
            return Ok(id);
        }
        // SAFETY: parent pins the common Git directory; the fixed leaf has no path traversal.
        if unsafe { libc::mkdirat(parent.as_raw_fd(), directory_name.as_ptr(), 0o700) } != 0 {
            return Err(invalid(
                "identity directory could not be created exclusively",
            ));
        }
        // Persist the incomplete directory before generating an ID. A crash thereafter is an
        // explicit incomplete migration, never a reason to mint a replacement UUID.
        sync(&parent)?;
        let directory = open_at(&parent, &directory_name, libc::O_RDONLY | libc::O_DIRECTORY)?
            .ok_or_else(|| invalid("new identity directory disappeared"))?;
        private_directory(&directory)?;
        let id = match expected {
            Some(id) => id,
            None => RepoId::new(),
        };
        let bytes = serde_json::to_vec(&Identity {
            version: 1,
            repo_id: id,
        })
        .map_err(|_| invalid("identity metadata could not be encoded"))?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err(invalid("identity metadata exceeds its size limit"));
        }
        let pending = name(STAGING)?;
        let mut file = open_at(
            &directory,
            &pending,
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
        )?
        .ok_or_else(|| invalid("identity staging file is unavailable"))?;
        file.write_all(&bytes)
            .map_err(|_| invalid("identity staging file could not be written"))?;
        sync(&file)?;
        validate_file(&file)?;
        sync(&directory)?;
        publish(&directory, &pending, &name(IDENTITY)?)?;
        sync(&directory)?;
        sync(&parent)?;
        let stored = read(&directory)?;
        if stored != id {
            return Err(invalid(
                "published identity does not match the initial repository ID",
            ));
        }
        Ok(stored)
    }

    #[cfg(test)]
    #[test]
    fn atomic_publication_never_replaces_an_existing_identity() {
        let temp = tempfile::tempdir().unwrap();
        let directory = File::open(temp.path()).unwrap();
        fs::write(temp.path().join(IDENTITY), "original").unwrap();
        fs::write(temp.path().join(STAGING), "candidate").unwrap();
        assert!(publish(
            &directory,
            &name(STAGING).unwrap(),
            &name(IDENTITY).unwrap()
        )
        .is_err());
        assert_eq!(fs::read(temp.path().join(IDENTITY)).unwrap(), b"original");
        assert_eq!(fs::read(temp.path().join(STAGING)).unwrap(), b"candidate");
    }
}

impl GitEngine {
    /// Initializes only an import stage with its preallocated intent identity.
    pub fn initialize_import_repository_id(
        &self,
        expected: RepoId,
    ) -> Result<RepoId, GitEngineError> {
        if expected.0.is_nil() {
            return Err(invalid("import repository ID must be nonnil"));
        }
        let service = self.lock_worktree_service()?;
        let _initialization = self
            .repository_identity_init
            .lock()
            .map_err(|_| invalid("repository identity initialization lock is poisoned"))?;
        service.ensure_available()?;
        #[cfg(unix)]
        {
            let id = storage::load_or_create_for_import(
                &self.repository_common_directory()?,
                Some(expected),
            )?;
            service.ensure_available()?;
            Ok(id)
        }
        #[cfg(not(unix))]
        {
            Err(invalid("repository import requires Unix file semantics"))
        }
    }

    /// Reads existing identity without initializing an unregistered secondary repository.
    pub fn load_existing_repository_id(&self) -> Result<RepoId, GitEngineError> {
        let service = self.lock_worktree_service()?;
        service.ensure_available()?;
        #[cfg(unix)]
        {
            storage::load_existing(&self.repository_common_directory()?)
        }
        #[cfg(not(unix))]
        {
            Err(invalid(
                "repository identity storage requires Unix file semantics",
            ))
        }
    }

    /// Returns the configured repository's private persisted ID, creating only a legacy first ID.
    /// Existing incomplete or unsafe identity metadata always requires explicit operator repair.
    pub fn load_or_create_repository_id(&self) -> Result<RepoId, GitEngineError> {
        let service = self.lock_worktree_service()?;
        let _initialization = self
            .repository_identity_init
            .lock()
            .map_err(|_| invalid("repository identity initialization lock is poisoned"))?;
        service.ensure_available()?;
        let repo = self.acquire_repo()?;
        let common = Self::common_git_dir(&repo);
        #[cfg(unix)]
        let id = storage::load_or_create(&common)?;
        #[cfg(not(unix))]
        return Err(invalid(
            "repository identity storage requires Unix file semantics",
        ));
        #[cfg(unix)]
        {
            service.ensure_available()?;
            Ok(id)
        }
    }
}
