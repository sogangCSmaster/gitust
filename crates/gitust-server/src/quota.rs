//! Worktree spawn quota tracking (`SpawnQuotaTracker`), cancellation-safe RAII reservation
//! guards (`SpawnReservation`), and sandbox run status guards (`RunStatusGuard`).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use axum::http::StatusCode;
use gitust_core::{AgentWorktree, UserId, WorktreeId, WorktreeStatus};
use gitust_git::GitEngine;
use parking_lot::Mutex;

use crate::state::{
    set_worktree_status, AppState, WorktreeRuntime, MAX_TRACKED_WORKTREES, MAX_WORKTREES_PER_USER,
};

/// Tracks in-flight worktree materializations so global and per-user limits are enforced
/// atomically without holding a lock across blocking `git worktree add` checkouts.
#[derive(Default)]
pub struct SpawnQuotaTracker {
    pub(crate) in_flight_total: usize,
    pub(crate) in_flight_by_user: HashMap<UserId, usize>,
}

impl SpawnQuotaTracker {
    pub(crate) fn release(&mut self, user_id: UserId) {
        self.in_flight_total = self.in_flight_total.saturating_sub(1);
        if let Some(count) = self.in_flight_by_user.get_mut(&user_id) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                self.in_flight_by_user.remove(&user_id);
            }
        }
    }
}

pub(crate) enum SpawnLifecycle {
    InFlight,
    Materialized {
        engine: GitEngine,
        worktree_dir: PathBuf,
        branch: String,
        base_commit: gitust_core::CommitSha,
    },
    Committed,
    Failed,
    Cancelled,
}

/// RAII reservation of one global and one per-user worktree slot. Automatically releases the
/// in-flight slot and rolls back any uncommitted checkout on drop if the checkout fails or the
/// request future is cancelled.
pub(crate) struct SpawnReservation {
    pub(crate) state: AppState,
    pub(crate) user_id: UserId,
    pub(crate) lifecycle: Arc<Mutex<SpawnLifecycle>>,
}

impl SpawnReservation {
    pub(crate) fn reserve(state: &AppState, user_id: UserId) -> Result<Self, (StatusCode, String)> {
        let mut quota = state.spawn_quota.lock();
        let tracked = state.worktrees.read().len();
        if tracked.saturating_add(quota.in_flight_total) >= MAX_TRACKED_WORKTREES {
            return Err((
                StatusCode::TOO_MANY_REQUESTS,
                format!(
                    "worktree limit ({MAX_TRACKED_WORKTREES}) reached; discard worktrees first"
                ),
            ));
        }
        let runtimes = state.worktree_runtime.read();
        let worktrees = state.worktrees.read();
        let owned = worktrees
            .iter()
            .filter(|wt| {
                !matches!(
                    wt.status,
                    WorktreeStatus::Merged | WorktreeStatus::Discarded
                )
            })
            .filter(|wt| {
                runtimes
                    .get(&wt.id)
                    .is_some_and(|r| r.created_by == Some(user_id))
            })
            .count();
        drop(worktrees);
        drop(runtimes);
        // fail-open-ok: an absent in-flight bucket means zero reservations; owned worktrees are counted separately.
        let user_in_flight = quota.in_flight_by_user.get(&user_id).copied().unwrap_or(0);
        if owned.saturating_add(user_in_flight) >= MAX_WORKTREES_PER_USER {
            return Err((
                StatusCode::TOO_MANY_REQUESTS,
                format!(
                    "per-user worktree limit ({MAX_WORKTREES_PER_USER}) reached; discard worktrees first"
                ),
            ));
        }
        quota.in_flight_total += 1;
        *quota.in_flight_by_user.entry(user_id).or_insert(0) += 1;
        Ok(Self {
            state: state.clone(),
            user_id,
            lifecycle: Arc::new(Mutex::new(SpawnLifecycle::InFlight)),
        })
    }

    pub(crate) fn commit(self, wt: AgentWorktree, runtime: WorktreeRuntime) {
        *self.lifecycle.lock() = SpawnLifecycle::Committed;
        let mut quota = self.state.spawn_quota.lock();
        self.state.worktree_runtime.write().insert(wt.id, runtime);
        self.state.worktrees.write().push_front(wt);
        quota.release(self.user_id);
    }
}

impl Drop for SpawnReservation {
    fn drop(&mut self) {
        let prev = std::mem::replace(&mut *self.lifecycle.lock(), SpawnLifecycle::Cancelled);
        match prev {
            SpawnLifecycle::Committed => {}
            SpawnLifecycle::Failed => {
                self.state.spawn_quota.lock().release(self.user_id);
            }
            SpawnLifecycle::InFlight | SpawnLifecycle::Cancelled => {
                // `spawn_blocking` is still running and will roll back the worktree + release quota.
            }
            SpawnLifecycle::Materialized {
                engine,
                worktree_dir,
                branch,
                base_commit,
            } => {
                let state = self.state.clone();
                let user_id = self.user_id;
                tokio::task::spawn_blocking(move || {
                    // fail-open-ok: best-effort worktree rollback on request cancellation Drop before releasing quota.
                    let _ = engine.remove_agent_worktree(&worktree_dir, &branch, &base_commit);
                    state.spawn_quota.lock().release(user_id);
                });
            }
        }
    }
}

/// Marks a run as failed if the handler future is dropped mid-run (e.g. client disconnect), so
/// the worktree never stays `Compiling` forever.
pub(crate) struct RunStatusGuard {
    pub(crate) state: AppState,
    pub(crate) id: WorktreeId,
    pub(crate) armed: bool,
}

impl Drop for RunStatusGuard {
    fn drop(&mut self) {
        if self.armed {
            set_worktree_status(&self.state, self.id, |w| w.status = WorktreeStatus::Failed);
        }
    }
}
