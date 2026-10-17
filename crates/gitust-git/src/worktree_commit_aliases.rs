//! Reject paths that alias a different Git spelling on this filesystem.
use crate::worktree_commit_paths::invalid;
use crate::{io_err, GitEngineError};

#[cfg(unix)]
pub(crate) fn validate_aliases(
    root: &std::path::Path,
    paths: &[String],
    original: &gix::Tree<'_>,
) -> Result<(), GitEngineError> {
    use std::collections::{HashMap, HashSet};
    use std::ffi::CString;
    use std::fs::File;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::fs::OpenOptionsExt;

    let error = || {
        invalid("commit path aliases another Git spelling; use exact tracked paths; case-only renames require a normal Git client")
    };
    let mut cache = HashMap::<(u64, u64), HashSet<Vec<u8>>>::new();
    let (mut scanned, mut bytes) = (0_usize, 0_usize);
    for path in paths {
        let mut parent = File::options()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(root)
            .map_err(|e| io_err(root, &e))?;
        let mut tree_id = Some(original.id);
        let components: Vec<_> = path.split('/').collect();
        for (position, component) in components.iter().enumerate() {
            let name = CString::new(*component).map_err(|_| error())?;
            let Some(observed) = metadata(parent.as_raw_fd(), &name)? else {
                break;
            };
            let tree = tree_id
                .map(|oid| original.repo.find_tree(oid))
                .transpose()
                .map_err(|e| invalid(e.to_string()))?;
            let mut exact = None;
            if let Some(tree) = &tree {
                for entry in tree.iter() {
                    let entry = entry.map_err(|e| invalid(e.to_string()))?;
                    let candidate: &[u8] = entry.filename().as_ref();
                    if candidate == component.as_bytes() {
                        exact = Some((entry.mode(), entry.object_id()));
                        continue;
                    }
                    // ASCII non-aliases need no syscall; non-ASCII names use the filesystem's
                    // own normalization/case rules rather than an invented Unicode fold.
                    if candidate.is_ascii()
                        && component.is_ascii()
                        && !candidate.eq_ignore_ascii_case(component.as_bytes())
                    {
                        continue;
                    }
                    let candidate = CString::new(candidate).map_err(|_| error())?;
                    if metadata(parent.as_raw_fd(), &candidate)? == Some(observed) {
                        let identity = parent.metadata().map_err(|e| io_err(root, &e))?;
                        use std::os::unix::fs::MetadataExt;
                        let key = (identity.dev(), identity.ino());
                        if let std::collections::hash_map::Entry::Vacant(entry) = cache.entry(key) {
                            entry.insert(names(parent.as_raw_fd(), &mut scanned, &mut bytes)?);
                        }
                        let names = &cache[&key];
                        // Hardlinks with two real directory entries on a case-sensitive FS
                        // are valid. A case/normalization alias has only one actual spelling.
                        if !names.contains(component.as_bytes())
                            || !names.contains(candidate.as_bytes())
                        {
                            return Err(error());
                        }
                    }
                }
            }
            if exact.is_none() {
                use std::os::unix::fs::MetadataExt;
                let identity = parent.metadata().map_err(|e| io_err(root, &e))?;
                let key = (identity.dev(), identity.ino());
                if let std::collections::hash_map::Entry::Vacant(entry) = cache.entry(key) {
                    entry.insert(names(parent.as_raw_fd(), &mut scanned, &mut bytes)?);
                }
                if !cache[&key].contains(component.as_bytes()) {
                    return Err(error());
                }
            }
            if position + 1 < components.len() {
                // SAFETY: name is NUL-terminated and parent owns a live pinned directory fd.
                let fd = unsafe {
                    libc::openat(
                        parent.as_raw_fd(),
                        name.as_ptr(),
                        libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                    )
                };
                if fd < 0 {
                    return Err(invalid(
                        "commit parent is missing, inaccessible or a symlink",
                    ));
                }
                // SAFETY: openat returned a fresh owned file descriptor.
                parent = unsafe { File::from_raw_fd(fd) };
                tree_id = match exact {
                    Some((mode, oid)) if mode.is_tree() => Some(oid),
                    Some(_) => {
                        return Err(invalid("commit path traverses a tracked file or submodule"))
                    }
                    None => None,
                };
            }
        }
    }
    Ok(())
}

#[cfg(unix)]
fn metadata(
    fd: std::os::fd::RawFd,
    name: &std::ffi::CStr,
) -> Result<Option<(libc::dev_t, libc::ino_t)>, GitEngineError> {
    let mut output = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: output is writable stat storage and name is NUL-terminated.
    if unsafe {
        libc::fstatat(
            fd,
            name.as_ptr(),
            output.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } != 0
    {
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::NotFound {
            return Ok(None);
        }
        return Err(invalid(format!(
            "cannot inspect commit path alias: {error}"
        )));
    }
    // SAFETY: fstatat initialized the structure on success.
    let output = unsafe { output.assume_init() };
    Ok(Some((output.st_dev, output.st_ino)))
}

#[cfg(unix)]
fn names(
    fd: std::os::fd::RawFd,
    count: &mut usize,
    bytes: &mut usize,
) -> Result<std::collections::HashSet<Vec<u8>>, GitEngineError> {
    // SAFETY: duplicating a live fd transfers only the duplicate to fdopendir/closedir.
    let duplicate = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0) };
    if duplicate < 0 {
        return Err(invalid("cannot open directory alias inventory"));
    }
    // SAFETY: duplicate is a fresh owned descriptor for a directory.
    let pointer = unsafe { libc::fdopendir(duplicate) };
    if pointer.is_null() {
        // SAFETY: fdopendir failed and did not take ownership of duplicate.
        unsafe {
            libc::close(duplicate);
        }
        return Err(invalid("cannot enumerate directory alias inventory"));
    }
    struct Directory(*mut libc::DIR);
    impl Drop for Directory {
        fn drop(&mut self) {
            // SAFETY: fdopendir created this directory stream and we own it exclusively.
            unsafe {
                libc::closedir(self.0);
            }
        }
    }
    let directory = Directory(pointer);
    let mut result = std::collections::HashSet::new();
    loop {
        #[cfg(target_os = "macos")]
        // SAFETY: __error returns this thread's errno slot.
        let errno = unsafe { libc::__error() };
        #[cfg(target_os = "linux")]
        // SAFETY: __errno_location returns this thread's errno slot.
        let errno = unsafe { libc::__errno_location() };
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        return Err(invalid("directory alias checks require macOS or Linux"));
        // SAFETY: errno is thread-local, and readdir owns a valid live stream.
        let entry = unsafe {
            *errno = 0;
            libc::readdir(directory.0)
        };
        if entry.is_null() {
            // SAFETY: errno remains this thread's live slot.
            if unsafe { *errno } != 0 {
                return Err(invalid("directory alias inventory failed"));
            }
            break;
        }
        // SAFETY: readdir provides a NUL-terminated d_name valid until its next call.
        let name = unsafe { std::ffi::CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
        if name == b"." || name == b".." {
            continue;
        }
        *count += 1;
        *bytes += name.len();
        if *count > 65_536 || *bytes > 8 * 1024 * 1024 {
            return Err(invalid(
                "directory alias inventory exceeds the commit limit",
            ));
        }
        result.insert(name.to_vec());
    }
    Ok(result)
}

#[cfg(not(unix))]
pub(crate) fn validate_aliases(
    _: &std::path::Path,
    _: &[String],
    _: &gix::Tree<'_>,
) -> Result<(), GitEngineError> {
    Err(invalid("managed commits require a Unix platform"))
}
