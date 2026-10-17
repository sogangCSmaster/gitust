use std::fs::{self, File};
use std::io::{Read, Write};
use std::ops::{Deref, DerefMut};
use std::path::Path;
use std::time::SystemTime;

use chrono::Utc;
use gitust_core::AuthError;
use parking_lot::{RwLockReadGuard, RwLockWriteGuard};
use uuid::Uuid;

use crate::crypto::hash_token_sha256;
pub(crate) use crate::disk_lock::acquire_advisory_lock;
use crate::disk_lock::ensure_private_parent_dir;
use crate::models::{AuthState, PersistedSnapshot};
use crate::AuthStore;

/// Upper bound for the on-disk auth DB snapshot.
pub(crate) const MAX_AUTH_DB_BYTES: u64 = 64 * 1024 * 1024;

/// Point-in-time file metadata stamp used to detect external process updates (`gitust admin ...`)
/// to `.gitust/auth-db.json` in ~1µs via `stat(2)` without reading or parsing the file on the fast path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DiskStamp {
    #[cfg(unix)]
    dev: u64,
    #[cfg(unix)]
    ino: u64,
    modified: Option<SystemTime>,
    len: u64,
}

impl DiskStamp {
    pub(crate) fn of_path(path: &Path) -> Result<Option<Self>, AuthError> {
        match fs::symlink_metadata(path) {
            Ok(meta) if !meta.is_file() => Err(AuthError::StorageError(
                "auth db is not a regular file".into(),
            )),
            Ok(meta) => Ok(Self::of_metadata(&meta)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(AuthError::StorageError(format!(
                "failed to stat auth db: {e}"
            ))),
        }
    }

    pub(crate) fn of_metadata(meta: &fs::Metadata) -> Option<Self> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Some(Self {
                dev: meta.dev(),
                ino: meta.ino(),
                // fail-open-ok: mtime unsupported -> dev/ino/len still detect atomic renames.
                modified: meta.modified().ok(),
                len: meta.len(),
            })
        }
        #[cfg(not(unix))]
        {
            Some(Self {
                // fail-open-ok: mtime unsupported -> dev/ino/len still detect atomic renames.
                modified: meta.modified().ok(),
                len: meta.len(),
            })
        }
    }
}

pub(crate) struct PendingDiskWrite<'a> {
    snapshot: PersistedSnapshot,
    guard: AuthWriteGuard<'a>,
}

pub(crate) struct AuthWriteGuard<'a> {
    pub(crate) guard: RwLockWriteGuard<'a, AuthState>,
    pub(crate) _lock_file: Option<File>,
    pub(crate) rollback_state: Option<AuthState>,
}

impl Deref for AuthWriteGuard<'_> {
    type Target = AuthState;

    fn deref(&self) -> &Self::Target {
        &self.guard
    }
}

impl DerefMut for AuthWriteGuard<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.guard
    }
}

/// Reads and parses the snapshot, deriving the `DiskStamp` from `fstat(2)` of the very file
/// handle the bytes were read from. The returned stamp therefore always describes the returned
/// snapshot: a concurrent atomic rename (new inode) can never attach a fresh stamp to stale
/// content, which would otherwise suppress later syncs and resurrect revoked sessions.
/// Callers must hold the cross-process advisory lock.
fn read_snapshot_with_stamp(
    db_path: &Path,
) -> Result<(Option<PersistedSnapshot>, Option<DiskStamp>), AuthError> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let mut file = match options.open(db_path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((None, None)),
        Err(e) => {
            return Err(AuthError::StorageError(format!(
                "failed to open auth db: {e}"
            )))
        }
    };
    let meta = file
        .metadata()
        .map_err(|e| AuthError::StorageError(format!("failed to stat auth db: {e}")))?;
    if !meta.is_file() {
        return Err(AuthError::StorageError(
            "auth db is not a regular file".into(),
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o777 != 0o600 {
            file.set_permissions(fs::Permissions::from_mode(0o600))
                .map_err(|e| AuthError::StorageError(format!("failed to restrict auth db: {e}")))?;
        }
    }
    let stamp = DiskStamp::of_metadata(&meta);
    // Bound allocation + read time while holding the lock (sparse / oversized DB files).
    if meta.len() > MAX_AUTH_DB_BYTES {
        return Err(AuthError::StorageError(format!(
            "auth db is {} bytes, exceeding the {MAX_AUTH_DB_BYTES}-byte limit",
            meta.len()
        )));
    }
    let capacity = usize::try_from(meta.len())
        .map_err(|_| AuthError::StorageError("auth db size exceeds addressable memory".into()))?;
    let mut bytes = Vec::with_capacity(capacity);
    (&mut file)
        .take(MAX_AUTH_DB_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| AuthError::StorageError(format!("failed to read auth db: {e}")))?;
    if bytes.len() as u64 > MAX_AUTH_DB_BYTES {
        return Err(AuthError::StorageError(
            "auth db grew beyond the size limit while reading".to_owned(),
        ));
    }
    let snap: PersistedSnapshot = serde_json::from_slice(&bytes)
        .map_err(|e| AuthError::StorageError(format!("corrupt auth db json: {e}")))?;
    snap.validate_repository_records()?;
    Ok((Some(snap), stamp))
}

/// Initial load for `AuthStore::open_persistent`: holds the cross-process advisory lock for the
/// whole read → parse → stamp critical section.
pub(crate) fn load_initial_snapshot_locked(
    db_path: &Path,
) -> Result<(Option<PersistedSnapshot>, Option<DiskStamp>), AuthError> {
    let _lock_file = acquire_advisory_lock(db_path)?;
    read_snapshot_with_stamp(db_path)
}

fn apply_disk_snapshot_preserving_rate_limits(state: &mut AuthState, snap: PersistedSnapshot) {
    let mut preserved_failed = std::mem::take(&mut state.failed_logins);
    let prev_rev = state.revision;
    let mut next_state = AuthState::from_snapshot(snap);

    // Clear rate-limit lockouts for any user whose password hash changed externally
    for (uid, new_user) in &next_state.users {
        let changed = state
            .users
            .get(uid)
            .is_some_and(|old_user| old_user.password_hash != new_user.password_hash);
        if changed {
            let lower_u = new_user.username.to_ascii_lowercase();
            let lower_e = new_user.email.to_ascii_lowercase();
            let u_suf = format!(":id:{}", hex::encode(hash_token_sha256(&lower_u)));
            let e_suf = format!(":id:{}", hex::encode(hash_token_sha256(&lower_e)));
            let uid_suf = format!(":uid:{}", uid.0);
            preserved_failed.retain(|k, _| {
                !k.ends_with(&u_suf) && !k.ends_with(&e_suf) && !k.ends_with(&uid_suf)
            });
        }
    }

    next_state.failed_logins = preserved_failed;
    next_state.revision = next_state.revision.max(prev_rev);
    *state = next_state;
}

impl AuthStore {
    /// Reloads the on-disk snapshot if its stamp differs from the last observed one.
    /// Caller MUST hold the cross-process advisory lock.
    ///
    /// Fail-closed: read/parse errors propagate, and a DB that was observed before but is now
    /// missing or un-stat-able is a `StorageError` (never treated as "no change"), so revocations
    /// written by other processes can't be silently ignored or overwritten with stale state.
    fn reload_if_changed_locked(
        &self,
        db_path: &Path,
    ) -> Result<Option<RwLockWriteGuard<'_, AuthState>>, AuthError> {
        let last = *self.last_disk_stamp.lock();
        if DiskStamp::of_path(db_path)? == last {
            return Ok(None);
        }
        match read_snapshot_with_stamp(db_path)? {
            (Some(snap), read_stamp) => {
                let mut guard = self.state.write();
                apply_disk_snapshot_preserving_rate_limits(&mut guard, snap);
                *self.last_disk_stamp.lock() = read_stamp;
                Ok(Some(guard))
            }
            (None, _) if last.is_some() => Err(AuthError::StorageError(
                "auth db disappeared after being observed; refusing to use stale auth state"
                    .to_owned(),
            )),
            (None, _) => Ok(None),
        }
    }

    /// Synchronizes in-memory state if an external process (`gitust admin ...`) modified
    /// `auth-db.json` on disk. Fast path is a single unlocked `stat(2)` comparison.
    pub(crate) fn sync_from_disk_if_changed(&self) -> Result<(), AuthError> {
        let Some(ref db_path) = self.persistence_path else {
            return Ok(());
        };
        if DiskStamp::of_path(db_path)? == *self.last_disk_stamp.lock() {
            return Ok(());
        }
        let _lock_file = acquire_advisory_lock(db_path)?;
        self.reload_if_changed_locked(db_path)?;
        Ok(())
    }

    pub(crate) fn read_state(&self) -> Result<RwLockReadGuard<'_, AuthState>, AuthError> {
        self.sync_from_disk_if_changed()?;
        Ok(self.state.read())
    }

    pub(crate) fn write_state(&self) -> Result<AuthWriteGuard<'_>, AuthError> {
        let Some(ref db_path) = self.persistence_path else {
            return Ok(AuthWriteGuard {
                guard: self.state.write(),
                _lock_file: None,
                rollback_state: None,
            });
        };
        let lock_file = acquire_advisory_lock(db_path)?;
        let guard = match self.reload_if_changed_locked(db_path)? {
            Some(reloaded) => reloaded,
            None => self.state.write(),
        };
        let rollback_state = Some(guard.clone());
        Ok(AuthWriteGuard {
            guard,
            _lock_file: Some(lock_file),
            rollback_state,
        })
    }

    pub(crate) fn maybe_snapshot<'a>(
        &self,
        mut guard: AuthWriteGuard<'a>,
    ) -> Option<PendingDiskWrite<'a>> {
        guard.revision = guard.revision.saturating_add(1);
        if self.persistence_path.is_some() {
            Some(PendingDiskWrite {
                snapshot: guard.snapshot_current(),
                guard,
            })
        } else {
            if guard.revision.is_multiple_of(64) || guard.sessions_by_hash.len() >= 1024 {
                guard.prune_expired(Utc::now());
            }
            None
        }
    }

    /// Keep both memory and advisory locks until the disk write and stamp publication finish.
    /// Readers must never authorize against a mutation that can still fail persistence.
    pub(crate) fn persist_snapshot(
        &self,
        pending: Option<PendingDiskWrite<'_>>,
    ) -> Result<(), AuthError> {
        let (Some(ref target_path), Some(mut pending_write)) = (&self.persistence_path, pending)
        else {
            return Ok(());
        };
        match write_snapshot_atomically(target_path, &pending_write.snapshot) {
            Ok(new_stamp) => {
                *self.last_disk_stamp.lock() = new_stamp;
                Ok(())
            }
            Err(err) => {
                if let Some(mut prev) = pending_write.guard.rollback_state.take() {
                    prev.failed_logins = std::mem::take(&mut pending_write.guard.failed_logins);
                    prev.revision = pending_write.guard.revision.max(prev.revision);
                    *pending_write.guard = prev;
                }
                Err(err)
            }
        }
    }
}

fn write_snapshot_atomically(
    target_path: &Path,
    snap: &PersistedSnapshot,
) -> Result<Option<DiskStamp>, AuthError> {
    ensure_private_parent_dir(target_path)?;
    let tmp_path = target_path.with_extension(format!("tmp.{}", Uuid::new_v4()));
    let payload = serde_json::to_vec(snap)
        .map_err(|e| AuthError::StorageError(format!("failed to serialize auth db: {e}")))?;
    if payload.len() as u64 > MAX_AUTH_DB_BYTES {
        return Err(AuthError::StorageError(format!(
            "auth db snapshot exceeds the {MAX_AUTH_DB_BYTES}-byte limit"
        )));
    }
    let written = (|| {
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let mut file = opts
            .open(&tmp_path)
            .map_err(|e| AuthError::StorageError(format!("failed to create temp auth db: {e}")))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o600))
                .map_err(|e| {
                    AuthError::StorageError(format!("failed to set 0600 permissions: {e}"))
                })?;
        }
        file.write_all(&payload)
            .map_err(|e| AuthError::StorageError(format!("failed to write temp auth db: {e}")))?;
        file.sync_all()
            .map_err(|e| AuthError::StorageError(format!("failed to fsync temp auth db: {e}")))?;
        fs::rename(&tmp_path, target_path).map_err(|e| {
            AuthError::StorageError(format!("failed to atomically replace auth db: {e}"))
        })?;
        sync_parent_directory(target_path)?;
        let meta = file.metadata().map_err(|e| {
            AuthError::StorageError(format!("failed to stat persisted auth db: {e}"))
        })?;
        Ok(DiskStamp::of_metadata(&meta))
    })();
    if written.is_err() {
        // fail-open-ok: best-effort temp cleanup; the original write error is returned.
        let _ = fs::remove_file(&tmp_path);
    }
    written
}

#[cfg(test)]
thread_local! {
    pub(crate) static FAIL_DIRECTORY_SYNC: std::cell::RefCell<Option<std::path::PathBuf>> = const { std::cell::RefCell::new(None) };
}

fn sync_parent_directory(target: &Path) -> Result<(), AuthError> {
    #[cfg(test)]
    if FAIL_DIRECTORY_SYNC.with(|path| path.borrow().as_deref() == Some(target)) {
        return Err(AuthError::StorageError(
            "injected auth directory synchronization failure".into(),
        ));
    }
    let parent = target
        .parent()
        .ok_or_else(|| AuthError::StorageError("auth database has no parent directory".into()))?;
    let parent = if parent.as_os_str().is_empty() {
        Path::new(".")
    } else {
        parent
    };
    fs::File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| {
            AuthError::StorageError("auth database directory synchronization failed".into())
        })
}
