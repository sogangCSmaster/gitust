use super::ephemeral_sandbox::RunError;
use gitust_git::{GitEngine, MaterializedWorktree, WorktreeServiceLock};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use tokio::sync::OwnedMutexGuard;

pub(super) struct CheckoutGuard {
    pub engine: GitEngine,
    pub owner: WorktreeServiceLock,
    pub materialized: Arc<Mutex<Option<MaterializedWorktree>>>,
    pub keep: bool,
    pub cancelled: Arc<AtomicBool>,
    pub operation: Option<Arc<OwnedMutexGuard<()>>>,
}
impl CheckoutGuard {
    pub async fn finish(self) -> Result<bool, RunError> {
        self.cancelled.store(true, Ordering::SeqCst);
        let taken = self
            .materialized
            .lock()
            // fail-open-ok: recover a cleanup-only slot; no execution or authority is granted.
            .unwrap_or_else(|e| e.into_inner())
            .take();
        let Some(m) = taken else {
            return Ok(true);
        };
        if self.keep {
            eprintln!(
                "[gitust sandbox] kept worktree {} on branch {}",
                m.worktree_dir.display(),
                m.worktree.branch
            );
            return Ok(true);
        }
        let engine = self.engine.clone();
        let owner = self.owner.clone();
        let operation = self.operation.clone();
        tokio::task::spawn_blocking(move || {
            let _owner = owner;
            let _operation = operation;
            engine.remove_agent_worktree(
                &m.worktree_dir,
                &m.worktree.branch,
                &m.worktree.base_commit,
            )?;
            Ok(!m.worktree_dir.try_exists().map_err(|_| RunError::Cleanup)?
                && engine.resolve_branch_oid(&m.worktree.branch)?.is_none())
        })
        .await?
    }
}
impl Drop for CheckoutGuard {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::SeqCst);
        let taken = self
            .materialized
            .lock()
            // fail-open-ok: recovering a poisoned cleanup slot cannot authorize commands.
            .unwrap_or_else(|e| e.into_inner())
            .take();
        let Some(m) = taken else {
            return;
        };
        if self.keep {
            eprintln!(
                "[gitust sandbox] kept worktree {} on branch {}",
                m.worktree_dir.display(),
                m.worktree.branch
            );
            return;
        }
        let engine = self.engine.clone();
        let owner = self.owner.clone();
        let operation = self.operation.clone();
        let cleanup = move || {
            let _owner = owner;
            let _operation = operation;
            if let Err(_error) = engine.remove_agent_worktree(
                &m.worktree_dir,
                &m.worktree.branch,
                &m.worktree.base_commit,
            ) {
                tracing::warn!("temporary checkout cleanup could not be confirmed");
            }
        };
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => {
                runtime.spawn_blocking(cleanup);
            }
            Err(_) => cleanup(),
        }
    }
}
