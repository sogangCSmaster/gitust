//! Cross-process advisory lock for the on-disk auth DB (`<auth-db>.lock`).
//!
//! Gate B P1 (2026-10-16): the lock used to be a blocking `flock` on a world-readable (0644)
//! lockfile, so any local user could open it read-only, hold `LOCK_EX`, and freeze every async
//! worker that authenticated a request. The lock is now:
//! - created `0600` (and existing lockfiles are tightened to `0600`), with `O_NOFOLLOW`, inside a
//!   parent directory created `0700`;
//! - acquired with a bounded `try_lock` loop that fails closed with `StorageError` after
//!   [`LOCK_WAIT_DEADLINE`] instead of blocking indefinitely.

use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use gitust_core::AuthError;

/// Maximum time to wait for another process holding the auth DB lock.
pub(crate) const LOCK_WAIT_DEADLINE: Duration = Duration::from_secs(2);
const LOCK_BACKOFF_MIN: Duration = Duration::from_millis(2);
const LOCK_BACKOFF_MAX: Duration = Duration::from_millis(50);

fn lock_file_path(db_path: &Path) -> PathBuf {
    let mut os = db_path.as_os_str().to_os_string();
    os.push(".lock");
    PathBuf::from(os)
}

fn storage_err(what: &str, e: impl std::fmt::Display) -> AuthError {
    AuthError::StorageError(format!("{what}: {e}"))
}

/// Creates the DB parent directory (new directories are created `0700`).
pub(crate) fn ensure_private_parent_dir(db_path: &Path) -> Result<(), AuthError> {
    let Some(parent) = db_path.parent() else {
        return Ok(());
    };
    if parent.as_os_str().is_empty() || parent.is_dir() {
        return Ok(());
    }
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(parent)
        .map_err(|e| storage_err("failed to create db parent dir", e))
}

fn open_private_lockfile(lpath: &Path) -> Result<File, AuthError> {
    let mut opts = OpenOptions::new();
    opts.create(true).read(true).write(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let file = opts
        .open(lpath)
        .map_err(|e| storage_err("failed to open auth lockfile", e))?;
    let meta = file
        .metadata()
        .map_err(|e| storage_err("failed to stat auth lockfile", e))?;
    if !meta.is_file() {
        return Err(AuthError::StorageError(
            "auth lockfile is not a regular file".to_owned(),
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // Tighten lockfiles created by older versions (0644) so other local users cannot open
        // them at all. Fails closed if we are not allowed to (e.g. foreign owner).
        if meta.permissions().mode() & 0o077 != 0 {
            file.set_permissions(fs::Permissions::from_mode(0o600))
                .map_err(|e| storage_err("failed to restrict auth lockfile to 0600", e))?;
        }
    }
    Ok(file)
}

/// Acquires the exclusive cross-process lock, waiting at most `deadline`.
pub(crate) fn acquire_advisory_lock_within(
    db_path: &Path,
    deadline: Duration,
) -> Result<File, AuthError> {
    ensure_private_parent_dir(db_path)?;
    let file = open_private_lockfile(&lock_file_path(db_path))?;
    let start = Instant::now();
    let mut backoff = LOCK_BACKOFF_MIN;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(fs::TryLockError::WouldBlock) => {
                let elapsed = start.elapsed();
                if elapsed >= deadline {
                    return Err(AuthError::StorageError(format!(
                        "auth db is locked by another process (waited {}ms)",
                        elapsed.as_millis()
                    )));
                }
                std::thread::sleep(backoff.min(deadline.saturating_sub(elapsed)));
                backoff = (backoff * 2).min(LOCK_BACKOFF_MAX);
            }
            Err(fs::TryLockError::Error(e)) => {
                return Err(storage_err("failed to lock auth lockfile", e));
            }
        }
    }
}

/// Acquires the exclusive cross-process lock with the default [`LOCK_WAIT_DEADLINE`].
pub(crate) fn acquire_advisory_lock(db_path: &Path) -> Result<File, AuthError> {
    acquire_advisory_lock_within(db_path, LOCK_WAIT_DEADLINE)
}
