//! Bounded literal source reads; never follow a directory or regular-file symlink.
use std::collections::HashSet;
use std::fs::File;
use std::io::Read;
use std::path::Path;

use crate::{io_err, GitEngineError};

#[path = "worktree_commit_aliases.rs"]
mod aliases;
pub(crate) use aliases::validate_aliases;

pub(crate) const MAX_PATHS: usize = 128;
pub(crate) const MAX_PATH_BYTES: usize = 4096;
pub(crate) const MAX_PATH_TOTAL_BYTES: usize = 64 * 1024;
pub(crate) const MAX_FILE_BYTES: usize = 8 * 1024 * 1024;
pub(crate) const MAX_TOTAL_BYTES: usize = 32 * 1024 * 1024;

pub(crate) fn invalid(message: impl Into<String>) -> GitEngineError {
    GitEngineError::VerificationInvalid(message.into())
}

pub(crate) fn validate(paths: &[String], message: &str, email: &str) -> Result<(), GitEngineError> {
    if message.trim().is_empty() || message.len() > 8192 || message.contains('\0') {
        return Err(invalid(
            "commit message must contain 1..8192 bytes without NUL",
        ));
    }
    if email.is_empty()
        || email.len() > 254
        || email
            .chars()
            .any(|c| c.is_control() || matches!(c, '<' | '>'))
    {
        return Err(invalid("invalid commit author email"));
    }
    if paths.is_empty() || paths.len() > MAX_PATHS {
        return Err(invalid("commit requires 1..128 explicit file paths"));
    }
    let mut seen = HashSet::with_capacity(paths.len());
    let mut total = 0;
    for path in paths {
        total += path.len();
        if path.is_empty()
            || path.len() > MAX_PATH_BYTES
            || total > MAX_PATH_TOTAL_BYTES
            || path.contains('\0')
            || path.contains('\\')
            || path.starts_with('/')
            || path
                .split('/')
                .any(|p| p.is_empty() || matches!(p, "." | "..") || p.eq_ignore_ascii_case(".git"))
            || !seen.insert(path)
        {
            return Err(invalid("commit paths must be unique literal relative file paths within byte limits; metadata and traversal are forbidden"));
        }
    }
    Ok(())
}

pub(crate) struct Source {
    pub mode: u32,
    pub bytes: Vec<u8>,
}

#[cfg(unix)]
pub(crate) fn read(root: &Path, path: &str) -> Result<Option<Source>, GitEngineError> {
    use std::ffi::CString;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

    let error_path = root.join(path);
    let mut parent = File::options()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(root)
        .map_err(|e| io_err(root, &e))?;
    let components: Vec<_> = path.split('/').collect();
    for component in &components[..components.len() - 1] {
        let name = CString::new(*component).map_err(|_| invalid("NUL in commit path"))?;
        // Each directory is pinned by a descriptor before the next component is resolved.
        let descriptor = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if descriptor < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::NotFound {
                return Ok(None);
            }
            return Err(io_err(&error_path, &error));
        }
        // SAFETY: openat returned a fresh owned descriptor.
        parent = unsafe { File::from_raw_fd(descriptor) };
    }
    let leaf = CString::new(components[components.len() - 1])
        .map_err(|_| invalid("NUL in commit path"))?;
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    let result = unsafe {
        libc::fstatat(
            parent.as_raw_fd(),
            leaf.as_ptr(),
            stat.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if result != 0 {
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::NotFound {
            return Ok(None);
        }
        return Err(io_err(&error_path, &error));
    }
    // SAFETY: fstatat initialized stat on success.
    let stat = unsafe { stat.assume_init() };
    if stat.st_mode & libc::S_IFMT == libc::S_IFLNK {
        // Record the link text, including an external target; never open its destination.
        let mut bytes = vec![0; MAX_PATH_BYTES + 1];
        let length = unsafe {
            libc::readlinkat(
                parent.as_raw_fd(),
                leaf.as_ptr(),
                bytes.as_mut_ptr().cast(),
                bytes.len(),
            )
        };
        if length < 0 {
            return Err(io_err(&error_path, &std::io::Error::last_os_error()));
        }
        if length as usize > MAX_PATH_BYTES {
            return Err(invalid("symlink target exceeds commit limit"));
        }
        bytes.truncate(length as usize);
        return Ok(Some(Source {
            mode: 0o120000,
            bytes,
        }));
    }
    if stat.st_mode & libc::S_IFMT != libc::S_IFREG {
        return Err(invalid("commit paths must name regular files, symbolic links, or deleted tracked files; directories/devices/FIFOs are forbidden"));
    }
    let descriptor = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            leaf.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
        )
    };
    if descriptor < 0 {
        return Err(io_err(&error_path, &std::io::Error::last_os_error()));
    }
    // SAFETY: openat returned a fresh owned descriptor.
    let file = unsafe { File::from_raw_fd(descriptor) };
    let meta = file.metadata().map_err(|e| io_err(&error_path, &e))?;
    if !meta.is_file() || meta.len() > MAX_FILE_BYTES as u64 {
        return Err(invalid(
            "commit source is not a regular file within the 8 MiB limit",
        ));
    }
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    file.take(MAX_FILE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| io_err(&error_path, &e))?;
    if bytes.len() > MAX_FILE_BYTES {
        return Err(invalid("commit source grew beyond the 8 MiB limit"));
    }
    let mode = if meta.mode() & 0o100 != 0 {
        0o100755
    } else {
        0o100644
    };
    Ok(Some(Source { mode, bytes }))
}

#[cfg(not(unix))]
pub(crate) fn read(_root: &Path, _path: &str) -> Result<Option<Source>, GitEngineError> {
    Err(invalid(
        "managed host-side commits currently require a Unix platform",
    ))
}
