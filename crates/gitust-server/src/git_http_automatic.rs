//! Prepare every eligible ref intent before receive-pack, then inspect actual partial outcomes.
use crate::{
    automatic_checks::{self, Intent},
    automatic_run_model::Origin,
    automatic_run_store::Failure,
    state::{map_git_err, set_worktree_status, AppState},
};
use gitust_core::{AuthenticatedPrincipal, CommitSha, WorktreeId, WorktreeStatus};
use gitust_git::ReceivePackRefUpdate;

pub(crate) fn prepare(
    state: &AppState,
    caller: &AuthenticatedPrincipal,
    matched: &[(WorktreeId, String, WorktreeStatus)],
    updates: &[ReceivePackRefUpdate],
) -> Result<Vec<Intent>, Failure> {
    let mut prepared = Vec::new();
    let result = (|| {
        for (id, branch, _) in matched {
            let Some(update) = updates
                .iter()
                .find(|u| u.refname.strip_prefix("refs/heads/") == Some(branch.as_str()))
            else {
                continue;
            };
            if update.is_deletion || update.old_oid == update.new_oid {
                continue;
            }
            let previous = state
                .git_engine
                .resolve_branch_oid(branch)
                .map_err(map_git_err)?;
            if previous.as_deref() == Some(update.new_oid.as_str()) {
                continue;
            }
            match automatic_checks::prepare(
                state,
                caller,
                *id,
                CommitSha(update.new_oid.clone()),
                previous.map(CommitSha),
                Origin::GitHttp,
            ) {
                Ok(Some(intent)) => prepared.push(intent),
                Ok(None) => {}
                Err(error) => return Err(error),
            }
        }
        Ok::<_, Failure>(())
    })();
    if let Err(original) = result {
        reject_all(state, prepared);
        return Err(original);
    }
    Ok(prepared)
}
pub(crate) fn reject_all(state: &AppState, intents: Vec<Intent>) {
    for intent in intents {
        if let Err((status, _)) = automatic_checks::reject(intent) {
            state.automatic_checks.mark_unavailable();
            tracing::warn!(%status, "receive-pack was not started; automatic intent recovery is required");
        }
    }
}
pub(crate) fn reconcile(state: &AppState, intents: Vec<Intent>) -> Result<(), Failure> {
    let mut failed = None;
    for intent in intents {
        let id = intent.id;
        if let Err(error) = automatic_checks::confirm(state, intent) {
            set_worktree_status(state, id, |wt| wt.status = WorktreeStatus::Failed);
            if failed.is_none() {
                failed = Some(error);
            }
        }
    }
    match failed {
        Some(error) => Err(error),
        None => Ok(()),
    }
}
