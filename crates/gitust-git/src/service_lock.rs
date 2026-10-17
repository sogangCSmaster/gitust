//! Shared process ownership and fail-closed quarantine after an unconfirmed execution.
use std::{
    fs,
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
};

use crate::{execution_fence, io_err, GitEngine, GitEngineError};

#[derive(Debug)]
pub(crate) struct WorktreeServiceLockInner {
    file: fs::File,
    quarantined: AtomicBool,
    active_fences: AtomicUsize,
}

impl Drop for WorktreeServiceLockInner {
    fn drop(&mut self) {
        // fail-open-ok: explicit unlock prevents unrelated forked children prolonging service ownership; durable markers independently fence interrupted sandbox runs.
        let _ = self.file.unlock();
    }
}

/// Exclusive repository OS lock. Clones share quarantine and execution reservations.
#[derive(Debug, Clone)]
pub struct WorktreeServiceLock(pub(crate) Arc<WorktreeServiceLockInner>);

impl WorktreeServiceLock {
    pub(crate) fn quarantine(&self) {
        self.0.quarantined.store(true, Ordering::SeqCst);
    }

    pub(crate) fn reserve_fence(&self) -> Result<(), GitEngineError> {
        if self.0.quarantined.load(Ordering::SeqCst) {
            return Err(execution_fence::quarantined());
        }
        self.0
            .active_fences
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |count| {
                (count < execution_fence::MAX_MARKERS).then_some(count + 1)
            })
            .map(|_| ())
            .map_err(|_| execution_fence::invalid("active execution marker limit exceeded"))
    }

    pub(crate) fn finish_fence(&self) {
        self.0.active_fences.fetch_sub(1, Ordering::SeqCst);
    }

    pub(crate) fn ensure_available(&self) -> Result<(), GitEngineError> {
        if self.0.quarantined.load(Ordering::SeqCst) {
            Err(execution_fence::quarantined())
        } else {
            Ok(())
        }
    }
}

impl GitEngine {
    /// Acquires service ownership, refusing an interrupted execution or a second process.
    pub fn lock_worktree_service(&self) -> Result<WorktreeServiceLock, GitEngineError> {
        let repo = self.acquire_repo()?;
        let path = Self::common_git_dir(&repo).join("gitust-worktree-service.lock");
        self.lock_worktree_service_path(&path)
    }

    pub(crate) fn lock_worktree_service_path(
        &self,
        path: &Path,
    ) -> Result<WorktreeServiceLock, GitEngineError> {
        self.acquire_service_lock(path, false)
    }

    pub(crate) fn acquire_service_lock(
        &self,
        path: &Path,
        operator_recovery: bool,
    ) -> Result<WorktreeServiceLock, GitEngineError> {
        // fail-open-ok: a poisoned Weak-handle cache still requires a live unquarantined handle or a newly acquired exclusive OS lock.
        let mut slot = self.service_lock.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(existing) = slot.upgrade() {
            if operator_recovery {
                return Err(GitEngineError::WorktreeServiceBusy(path.to_path_buf()));
            }
            let guard = WorktreeServiceLock(existing);
            guard.ensure_available()?;
            return Ok(guard);
        }
        let mut options = fs::File::options();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let file = options.open(path).map_err(|e| io_err(path, &e))?;
        let metadata = file.metadata().map_err(|e| io_err(path, &e))?;
        if !metadata.is_file() {
            return Err(execution_fence::invalid(
                "service lock is not a regular file",
            ));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o777 != 0o600 {
                file.set_permissions(fs::Permissions::from_mode(0o600))
                    .map_err(|e| io_err(path, &e))?;
            }
        }
        match file.try_lock() {
            Ok(()) => {
                let inner = Arc::new(WorktreeServiceLockInner {
                    file,
                    quarantined: AtomicBool::new(false),
                    active_fences: AtomicUsize::new(0),
                });
                if !operator_recovery {
                    let common = path.parent().ok_or_else(execution_fence::quarantined)?;
                    execution_fence::ensure_clear(common)?;
                }
                *slot = Arc::downgrade(&inner);
                Ok(WorktreeServiceLock(inner))
            }
            Err(fs::TryLockError::WouldBlock) => {
                Err(GitEngineError::WorktreeServiceBusy(path.to_path_buf()))
            }
            Err(fs::TryLockError::Error(e)) => Err(io_err(path, &e)),
        }
    }
}
