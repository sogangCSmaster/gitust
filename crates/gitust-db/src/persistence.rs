use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use chrono::Utc;
use gitust_core::AuthError;
use parking_lot::{RwLockReadGuard, RwLockWriteGuard};
use uuid::Uuid;

use crate::crypto::hash_token_sha256;
use crate::models::{AuthState, PersistedSnapshot};
use crate::AuthStore;

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
    pub(crate) fn of_path(path: &Path) -> Option<Self> {
        let meta = fs::metadata(path).ok()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Some(Self {
                dev: meta.dev(),
                ino: meta.ino(),
                modified: meta.modified().ok(),
                len: meta.len(),
            })
        }
        #[cfg(not(unix))]
        {
            Some(Self {
                modified: meta.modified().ok(),
                len: meta.len(),
            })
        }
    }
}

pub(crate) struct PendingDiskWrite {
    pub(crate) snapshot: PersistedSnapshot,
    pub(crate) _lock_file: Option<File>,
}

pub(crate) struct AuthWriteGuard<'a> {
    pub(crate) guard: RwLockWriteGuard<'a, AuthState>,
    pub(crate) lock_file: Option<File>,
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

fn lock_file_path(db_path: &Path) -> PathBuf {
    let mut os = db_path.as_os_str().to_os_string();
    os.push(".lock");
    PathBuf::from(os)
}

fn acquire_advisory_lock(db_path: &Path) -> Result<File, AuthError> {
    if let Some(parent) = db_path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|e| {
                AuthError::StorageError(format!("failed to create db parent dir: {e}"))
            })?;
        }
    }
    let lpath = lock_file_path(db_path);
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&lpath)
        .map_err(|e| AuthError::StorageError(format!("failed to open auth lockfile: {e}")))?;
    file.lock()
        .map_err(|e| AuthError::StorageError(format!("failed to lock auth lockfile: {e}")))?;
    Ok(file)
}

fn read_snapshot_from_disk(db_path: &Path) -> Result<Option<PersistedSnapshot>, AuthError> {
    if !db_path.exists() {
        return Ok(None);
    }
    let bytes = fs::read(db_path)
        .map_err(|e| AuthError::StorageError(format!("failed to read auth db: {e}")))?;
    let snap: PersistedSnapshot = serde_json::from_slice(&bytes)
        .map_err(|e| AuthError::StorageError(format!("corrupt auth db json: {e}")))?;
    Ok(Some(snap))
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
    /// Synchronizes in-memory state if an external process (`gitust admin ...`) modified
    /// `auth-db.json` on disk. Fast path is a single `stat(2)` check (~1µs).
    pub(crate) fn sync_from_disk_if_changed(&self) {
        let Some(ref db_path) = self.persistence_path else {
            return;
        };
        let current_stamp = DiskStamp::of_path(db_path);
        if current_stamp == *self.last_disk_stamp.lock() {
            return;
        }
        let Ok(_lock_file) = acquire_advisory_lock(db_path) else {
            return;
        };
        let locked_stamp = DiskStamp::of_path(db_path);
        if locked_stamp == *self.last_disk_stamp.lock() {
            return;
        }
        if let Ok(Some(snap)) = read_snapshot_from_disk(db_path) {
            let mut guard = self.state.write();
            apply_disk_snapshot_preserving_rate_limits(&mut guard, snap);
            *self.last_disk_stamp.lock() = locked_stamp;
        }
    }

    pub(crate) fn read_state(&self) -> RwLockReadGuard<'_, AuthState> {
        self.sync_from_disk_if_changed();
        self.state.read()
    }

    pub(crate) fn write_state(&self) -> AuthWriteGuard<'_> {
        let lock_file = if let Some(ref db_path) = self.persistence_path {
            let lf = acquire_advisory_lock(db_path).ok();
            let disk_stamp = DiskStamp::of_path(db_path);
            if disk_stamp != *self.last_disk_stamp.lock() {
                if let Ok(Some(snap)) = read_snapshot_from_disk(db_path) {
                    let mut guard = self.state.write();
                    apply_disk_snapshot_preserving_rate_limits(&mut guard, snap);
                    *self.last_disk_stamp.lock() = disk_stamp;
                    return AuthWriteGuard {
                        guard,
                        lock_file: lf,
                    };
                }
            }
            lf
        } else {
            None
        };
        AuthWriteGuard {
            guard: self.state.write(),
            lock_file,
        }
    }

    pub(crate) fn maybe_snapshot(
        &self,
        guard: &mut AuthWriteGuard<'_>,
    ) -> Option<PendingDiskWrite> {
        guard.revision = guard.revision.saturating_add(1);
        if self.persistence_path.is_some() {
            Some(PendingDiskWrite {
                snapshot: guard.snapshot_current(),
                _lock_file: guard.lock_file.take(),
            })
        } else {
            if guard.revision.is_multiple_of(64) || guard.sessions_by_hash.len() >= 1024 {
                guard.prune_expired(Utc::now());
            }
            None
        }
    }

    /// Persists a point-in-time snapshot to disk OUTSIDE the in-memory `RwLock` while holding
    /// the cross-process advisory file lock (`<auth-db>.lock`).
    pub(crate) fn persist_snapshot_unlocked(
        &self,
        pending: Option<PendingDiskWrite>,
    ) -> Result<(), AuthError> {
        let (Some(ref target_path), Some(pending_write)) = (&self.persistence_path, pending) else {
            return Ok(());
        };
        let snap = &pending_write.snapshot;

        if let Some(parent) = target_path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).map_err(|e| {
                    AuthError::StorageError(format!("failed to create db parent dir: {e}"))
                })?;
            }
        }

        let tmp_path = target_path.with_extension(format!("tmp.{}", Uuid::new_v4()));
        let payload = serde_json::to_vec(snap)
            .map_err(|e| AuthError::StorageError(format!("failed to serialize auth db: {e}")))?;

        {
            let mut file = File::create(&tmp_path).map_err(|e| {
                AuthError::StorageError(format!("failed to create temp auth db: {e}"))
            })?;

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let perms = fs::Permissions::from_mode(0o600);
                file.set_permissions(perms).map_err(|e| {
                    AuthError::StorageError(format!("failed to set 0600 permissions: {e}"))
                })?;
            }

            file.write_all(&payload).map_err(|e| {
                AuthError::StorageError(format!("failed to write temp auth db: {e}"))
            })?;
            file.sync_all().map_err(|e| {
                AuthError::StorageError(format!("failed to fsync temp auth db: {e}"))
            })?;
        }

        fs::rename(&tmp_path, target_path).map_err(|e| {
            let _ = fs::remove_file(&tmp_path);
            AuthError::StorageError(format!("failed to atomically replace auth db: {e}"))
        })?;

        *self.last_disk_stamp.lock() = DiskStamp::of_path(target_path);
        drop(pending_write);
        Ok(())
    }
}
