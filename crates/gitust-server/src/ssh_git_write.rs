//! Authenticated SSH `git-receive-pack` preparation, multi-stage RBAC/key-binding gates,
//! worktree lock coordination, and automatic Checks scheduling.
use crate::{
    repository_registry::{ReadRepository, RepositoryRegistry},
    ssh_git_stream,
    state::{check_repo_bola, set_worktree_status, AppState},
};
use gitust_core::{AuthenticatedPrincipal, EnterpriseRole, RepoId, UserId, WorktreeStatus};
use gitust_db::AuthenticatedSshKey;
use gitust_git::{ReceivePackRefUpdate, SshUploadPack, WorktreeServiceLock, SMART_HTTP_TIMEOUT};
use parking_lot::Mutex;
use russh::{server::Msg, ChannelStream};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tokio::sync::{OwnedMutexGuard, OwnedSemaphorePermit};

#[derive(Clone)]
struct WriteAuth {
    state: AppState,
    repository: ReadRepository,
    binding: AuthenticatedSshKey,
    user: UserId,
    role: EnterpriseRole,
    closing: Arc<AtomicBool>,
}

impl WriteAuth {
    fn verify(&self) -> Result<AuthenticatedPrincipal, ()> {
        if self.closing.load(Ordering::Acquire) {
            return Err(());
        }
        if self
            .repository
            .engine
            .load_existing_repository_id()
            .map_err(|_| ())?
            != self.repository.access.repo_id
        {
            return Err(());
        }
        let caller = self
            .state
            .auth_store
            .authorize_ssh_key_binding(&self.binding)
            .map_err(|_| ())?;
        if caller.user_id != self.user || caller.role != self.role {
            return Err(());
        }
        let allowed = self
            .state
            .auth_store
            .authorized_repository_catalog(&caller, &[self.repository.access])
            .map_err(|_| ())?;
        if allowed.len() != 1 {
            return Err(());
        }
        if !self.repository.is_default {
            if !allowed[0].can_push_ordinary {
                return Err(());
            }
            self.repository
                .engine
                .validate_ordinary_git_service()
                .map_err(|_| ())?;
        }
        Ok(caller)
    }
}

#[derive(Default)]
struct PreparedIntents {
    worktree_intents: Vec<crate::automatic_checks::Intent>,
    local_intent: Option<crate::local_checks_automatic::Intent>,
    gate_succeeded: bool,
}

impl PreparedIntents {
    fn reject_all(&mut self, state: &AppState) {
        let intents = std::mem::take(&mut self.worktree_intents);
        crate::git_http_automatic::reject_all(state, intents);
        if let Some(intent) = self.local_intent.take() {
            if crate::local_checks_automatic::reject(state, intent).is_err() {
                tracing::warn!("automatic local Checks preparation recovery is required");
            }
        }
    }

    fn finish(&mut self, state: &AppState) {
        if !self.gate_succeeded {
            self.reject_all(state);
            return;
        }
        let intents = std::mem::take(&mut self.worktree_intents);
        if let Err((status, _)) = crate::git_http_automatic::reconcile(state, intents) {
            tracing::warn!(%status, "SSH push completed but automatic Checks scheduling is unavailable");
        }
        if let Some(intent) = self.local_intent.take() {
            if crate::local_checks_automatic::confirm(state, intent).is_err() {
                tracing::warn!(
                    "SSH push completed but automatic local Checks scheduling is unavailable"
                );
            }
        }
    }
}

struct WriteTarget {
    _inventory: Option<Arc<RepositoryRegistry>>,
    _ownership: WorktreeServiceLock,
    _mutation: OwnedMutexGuard<()>,
    _exec_locks: Arc<Mutex<Vec<OwnedMutexGuard<()>>>>,
    _admission: Arc<OwnedSemaphorePermit>,
}

fn check_commands(
    auth: &WriteAuth,
    exec_locks: &Arc<Mutex<Vec<OwnedMutexGuard<()>>>>,
    prepared: &Arc<Mutex<PreparedIntents>>,
    updates: Vec<ReceivePackRefUpdate>,
) -> Result<(), ()> {
    let caller = auth.verify()?;
    if !auth.repository.is_default {
        if updates.iter().any(|u| u.is_agent_branch) {
            return Err(());
        }
        #[cfg(test)]
        crate::repository_reads::after_read(&auth.state);
        auth.verify()?;
        return Ok(());
    }

    if caller.role == EnterpriseRole::Developer {
        for u in &updates {
            if !u.is_agent_branch || u.is_deletion {
                return Err(());
            }
        }
    }

    let mut matched_worktrees = Vec::new();
    {
        let worktrees = auth.state.worktrees.read();
        for u in &updates {
            if let Some(short) = u.refname.strip_prefix("refs/heads/") {
                if let Some(wt) = worktrees.iter().find(|w| w.branch == short) {
                    matched_worktrees.push((wt.id, wt.branch.clone(), wt.status));
                }
            }
        }
    }

    let mut held_exec = Vec::with_capacity(matched_worktrees.len());
    for (wt_id, _wt_branch, wt_status) in &matched_worktrees {
        if matches!(
            wt_status,
            WorktreeStatus::Compiling | WorktreeStatus::Merged | WorktreeStatus::Discarded
        ) {
            return Err(());
        }
        let rt = auth.state.get_worktree_runtime(*wt_id).ok_or(())?;
        if caller.role == EnterpriseRole::Developer && rt.created_by != Some(caller.user_id) {
            return Err(());
        }
        let guard = rt.exec_lock.clone().try_lock_owned().map_err(|_| ())?;
        if !auth.state.worktree_runtime.read().contains_key(wt_id) {
            return Err(());
        }
        let current_status = auth
            .state
            .worktrees
            .read()
            .iter()
            .find(|w| w.id == *wt_id)
            .map(|w| w.status)
            .ok_or(())?;
        if matches!(
            current_status,
            WorktreeStatus::Compiling | WorktreeStatus::Merged | WorktreeStatus::Discarded
        ) {
            return Err(());
        }
        held_exec.push(guard);
    }
    *exec_locks.lock() = held_exec;

    let step = (|| -> Result<(), ()> {
        let intents =
            crate::git_http_automatic::prepare(&auth.state, &caller, &matched_worktrees, &updates)
                .map_err(|_| ())?;
        prepared.lock().worktree_intents = intents;
        match crate::local_checks_automatic::prepare(&auth.state, &caller, &updates)
            .map_err(|_| ())?
        {
            crate::local_checks_automatic::Eligibility::Prepared(intent) => {
                prepared.lock().local_intent = Some(intent);
            }
            crate::local_checks_automatic::Eligibility::ConsentUnproven(reason) => {
                tracing::warn!(
                    ?reason,
                    "automatic local Checks consent could not be established; no execution scheduled"
                );
            }
            crate::local_checks_automatic::Eligibility::Ineligible => {}
        }
        #[cfg(test)]
        {
            crate::repository_reads::after_read(&auth.state);
            if let Some((entered, release)) =
                auth.state.automatic_checks.pre_receive_hook.lock().take()
            {
                entered.send(()).map_err(|_| ())?;
                release.recv().map_err(|_| ())?;
            }
        }
        let fresh = auth.verify()?;
        check_repo_bola(
            &auth.state.auth_store,
            auth.state.repo_id,
            auth.state.repo_access,
            &fresh,
            auth.state.repo_id,
        )
        .map_err(|_| ())
    })();

    if step.is_err() {
        prepared.lock().reject_all(&auth.state);
        return Err(());
    }
    for (wt_id, _, _) in &matched_worktrees {
        set_worktree_status(&auth.state, *wt_id, |w| {
            w.status = WorktreeStatus::Pending;
        });
    }
    prepared.lock().gate_succeeded = true;
    Ok(())
}

pub(crate) async fn start(
    state: AppState,
    binding: AuthenticatedSshKey,
    id: RepoId,
    closing: Arc<AtomicBool>,
    admission: Arc<OwnedSemaphorePermit>,
    stream: ChannelStream<Msg>,
) -> Result<(SshUploadPack, ssh_git_stream::Lease), ()> {
    let pre_state = state.clone();
    let pre_closing = closing.clone();
    let (auth, ownership, inventory) = tokio::task::spawn_blocking(move || {
        let inventory = pre_state.repository_registry.clone();
        let repository = pre_state
            .read_repository(id)
            .filter(|r| r.git_write)
            .ok_or(())?;
        let caller = pre_state
            .auth_store
            .authorize_ssh_key_binding(&binding)
            .map_err(|_| ())?;
        let ownership = repository.engine.lock_worktree_service().map_err(|_| ())?;
        let auth = WriteAuth {
            state: pre_state,
            repository,
            binding,
            user: caller.user_id,
            role: caller.role,
            closing: pre_closing,
        };
        auth.verify()?;
        Ok::<_, ()>((auth, ownership, inventory))
    })
    .await
    .map_err(|_| ())??;

    #[cfg(test)]
    if let Some(hook) = state.git_write_before_lock_hook.lock().clone() {
        hook();
    }

    let mutation = tokio::time::timeout(
        SMART_HTTP_TIMEOUT,
        auth.repository.mutation.clone().lock_owned(),
    )
    .await
    .map_err(|_| ())?;

    if closing.load(Ordering::Acquire) {
        return Err(());
    }

    let engine = auth.repository.engine.clone();
    let exec_locks = Arc::new(Mutex::new(Vec::new()));
    let target = WriteTarget {
        _inventory: inventory,
        _ownership: ownership,
        _mutation: mutation,
        _exec_locks: exec_locks.clone(),
        _admission: admission,
    };
    let prepared = Arc::new(Mutex::new(PreparedIntents::default()));
    let spawn_auth = auth.clone();
    let cmd_auth = auth.clone();
    let cmd_prepared = prepared.clone();
    let finish_state = state;
    let (stream, lease) = ssh_git_stream::Stream::retain(stream);
    let run = engine
        .start_ssh_receive_pack(
            stream,
            target,
            move || spawn_auth.verify().map(|_| ()),
            move |updates| check_commands(&cmd_auth, &exec_locks, &cmd_prepared, updates),
            move |_outcome| {
                prepared.lock().finish(&finish_state);
            },
        )
        .map_err(|_| ())?;
    Ok((run, lease))
}
