//! Establish repository ownership before initialization or shared staging mutations.
use std::fs;

use crate::{io_err, run_git, GitEngine, GitEngineError, WorktreeServiceLock};

const SERVICE_LOCK: &str = "gitust-worktree-service.lock";

impl GitEngine {
    pub(crate) fn lock_inbound_mirror_repository(
        &self,
    ) -> Result<WorktreeServiceLock, GitEngineError> {
        let original_error = match self.acquire_repo() {
            Ok(_) => return self.lock_worktree_service(),
            Err(error) => error,
        };
        fs::create_dir_all(&self.repo_path).map_err(|e| io_err(&self.repo_path, &e))?;
        // A failed or concurrent first initialization may already have created the lock inode.
        // Never reinitialize a nonempty directory that contains any other material.
        if !only_service_lock_present(&self.repo_path)? {
            return Err(original_error);
        }
        let guard = self.lock_worktree_service_path(&self.repo_path.join(SERVICE_LOCK))?;
        match self.acquire_repo() {
            Ok(repo) if repo.is_bare() => return Ok(guard),
            Ok(_) => return Err(original_error),
            // fail-open-ok: initialization remains restricted to a lock-only directory under exclusive service ownership below.
            Err(_) => {}
        }
        // Recheck while holding the same inode used by the eventual bare repository's service.
        if !only_service_lock_present(&self.repo_path)? {
            return Err(original_error);
        }
        run_git(&self.repo_path, ["init", "--bare", "-q", "-b", "main"])?;
        self.acquire_repo()?;
        Ok(guard)
    }
}

fn only_service_lock_present(path: &std::path::Path) -> Result<bool, GitEngineError> {
    for entry in fs::read_dir(path).map_err(|e| io_err(path, &e))? {
        if entry.map_err(|e| io_err(path, &e))?.file_name() != SERVICE_LOCK {
            return Ok(false);
        }
    }
    Ok(true)
}
