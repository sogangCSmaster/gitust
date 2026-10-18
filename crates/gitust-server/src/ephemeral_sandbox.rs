//! Temporary sandbox lifecycle shared by CLI operators and explicit repository diagnostics.
use crate::ephemeral_checkout::CheckoutGuard;
use gitust_core::{AgentKind, CommitSha, RepoId, WorktreeId};
use gitust_db::AuthStore;
use gitust_git::{GitEngine, GitEngineError, WorktreeServiceLock};
use gitust_sandbox::{
    LocalSandboxExecutor, SandboxAdmission, SandboxError, SandboxExecutionResult, SandboxSpec,
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use tokio::sync::OwnedMutexGuard;

pub enum RunCommands {
    Command(String),
    Checks {
        expected: CommitSha,
        commands: Vec<String>,
    },
}
pub struct RunContext {
    pub repo_id: RepoId,
    pub executor: LocalSandboxExecutor,
    pub auth_store: Option<AuthStore>,
    pub admission: Option<SandboxAdmission>,
    pub operation: Option<Arc<OwnedMutexGuard<()>>>,
}
pub struct RunOutcome {
    pub result: Option<SandboxExecutionResult>,
    pub cleanup_confirmed: bool,
    pub fence_confirmed: bool,
    pub interrupted: bool,
    pub materialize_micros: u64,
}
#[derive(Debug)]
pub enum RunError {
    Git(GitEngineError),
    Sandbox(SandboxError),
    Task,
    Cleanup,
    AdmissionDenied,
}
impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Git(e) => e.fmt(f),
            Self::Sandbox(e) => e.fmt(f),
            Self::Task => f.write_str("sandbox preparation task did not complete"),
            Self::Cleanup => f.write_str("temporary checkout cleanup is unconfirmed"),
            Self::AdmissionDenied => f.write_str("sandbox admission was denied before execution"),
        }
    }
}
impl std::error::Error for RunError {}
impl From<GitEngineError> for RunError {
    fn from(e: GitEngineError) -> Self {
        Self::Git(e)
    }
}
impl From<SandboxError> for RunError {
    fn from(e: SandboxError) -> Self {
        Self::Sandbox(e)
    }
}
impl From<tokio::task::JoinError> for RunError {
    fn from(_: tokio::task::JoinError) -> Self {
        Self::Task
    }
}

pub async fn run_prepared_sandbox(
    engine: GitEngine,
    owner: WorktreeServiceLock,
    base_branch: String,
    commands: RunCommands,
    keep: bool,
    context: RunContext,
    cancel: impl std::future::Future<Output = ()>,
) -> Result<RunOutcome, RunError> {
    let cancelled = Arc::new(AtomicBool::new(false));
    let slot = Arc::new(Mutex::new(None));
    let guard = CheckoutGuard {
        engine: engine.clone(),
        owner: owner.clone(),
        materialized: slot.clone(),
        keep,
        cancelled: cancelled.clone(),
        operation: context.operation.clone(),
    };
    let worktree_id = WorktreeId::new();
    let id = worktree_id.0.to_string();
    let branch = format!("agent/cli/sandbox-{}", &id[..8]);
    let spawn_engine = engine.clone();
    let spawn_owner = owner.clone();
    let operation = context.operation.clone();
    let label = match commands {
        RunCommands::Command(_) => "CLI sandbox command",
        RunCommands::Checks { .. } => "Repository Checks",
    };
    let repo_id = context.repo_id;
    let m = tokio::task::spawn_blocking(move || {
        let _owner = spawn_owner;
        let _operation = operation;
        let result = spawn_engine.materialize_agent_worktree(
            worktree_id,
            repo_id,
            AgentKind::InternalBot,
            &branch,
            &base_branch,
            label,
        );
        // fail-open-ok: poison recovery retains the cleanup slot; materialization errors propagate.
        let mut slot = slot.lock().unwrap_or_else(|e| e.into_inner());
        if cancelled.load(Ordering::SeqCst) {
            drop(slot);
            // fail-open-ok: only rolls back successful materialization after cancellation; result errors still propagate.
            if let Ok(m) = &result {
                if !keep {
                    // fail-open-ok: cleanup-only rollback after cancellation; result is still returned.
                    let _ = spawn_engine.remove_agent_worktree(
                        &m.worktree_dir,
                        &m.worktree.branch,
                        &m.worktree.base_commit,
                    );
                }
            }
        // fail-open-ok: only registers a successful checkout for cleanup; result errors still propagate.
        } else if let Ok(m) = &result {
            *slot = Some(m.clone());
        }
        result
    })
    .await??;
    let checked = match &commands {
        RunCommands::Checks { expected, .. } => Some(expected.clone()),
        _ => None,
    };
    let check_engine = engine.clone();
    let target = m.clone();
    let check_owner = owner.clone();
    let operation = context.operation.clone();
    let store = context.auth_store;
    let spec = tokio::task::spawn_blocking(move || {
        let _owner = check_owner;
        let _operation = operation;
        if let Some(expected) = checked {
            if target.worktree.base_commit != expected
                || check_engine
                    .clean_worktree_commit(target.worktree.id, &target.worktree.branch)?
                    != expected
            {
                return Err(GitEngineError::VerificationInvalid(
                    "Checks checkout does not match expected commit".into(),
                ));
            }
        }
        match store {
            Some(store) => crate::state::sandbox_spec_for(&target, &store),
            None => Ok(SandboxSpec::for_linked_worktree(
                &target.worktree_dir,
                &target.worktree_admin_dir,
                &target.common_git_dir,
                target.main_workdir.as_deref(),
                &target.worktrees_base,
                &target.sandbox_scratch_base,
            )),
        }
    })
    .await??;
    let fence_engine = engine.clone();
    let target = m.worktree.clone();
    let execution_guard = tokio::task::spawn_blocking(move || {
        let fence = fence_engine.begin_execution_fence(&target)?;
        Ok::<_, GitEngineError>((fence, guard))
    })
    .await??;
    let execute = async {
        match &commands {
            RunCommands::Command(command) => {
                context
                    .executor
                    .execute(m.worktree.id, &spec, command)
                    .await
            }
            RunCommands::Checks { commands, .. } => match context.admission {
                Some(admission) => {
                    context
                        .executor
                        .execute_required_checks_with_admission(
                            m.worktree.id,
                            &spec,
                            commands,
                            admission,
                        )
                        .await
                }
                None => {
                    context
                        .executor
                        .execute_required_checks(m.worktree.id, &spec, commands)
                        .await
                }
            },
        }
    };
    let outcome = tokio::select! {
        biased;
        // fail-open-ok: cancellation returns an interrupted/unconfirmed outcome and retains the fence.
        _ = cancel => None,
        result = execute => Some(result),
    };
    let Some(outcome) = outcome else {
        let (fence, guard) = execution_guard;
        drop(fence);
        return Ok(RunOutcome {
            result: None,
            cleanup_confirmed: guard.finish().await.is_ok_and(|v| v),
            fence_confirmed: false,
            interrupted: true,
            materialize_micros: m.worktree.materialize_micros,
        });
    };
    let (fence, guard) = execution_guard;
    match outcome {
        Ok(result) => {
            let fence_confirmed = tokio::task::spawn_blocking(move || fence.complete())
                .await
                .is_ok_and(|r| r.is_ok());
            let cleanup_confirmed = guard.finish().await.is_ok_and(|v| v);
            Ok(RunOutcome {
                result: Some(result),
                cleanup_confirmed,
                fence_confirmed,
                interrupted: false,
                materialize_micros: m.worktree.materialize_micros,
            })
        }
        Err(SandboxError::AdmissionDenied) => {
            // No command started and scratch cleanup was confirmed by the executor.
            let cleared = tokio::task::spawn_blocking(move || fence.complete())
                .await
                .is_ok_and(|r| r.is_ok());
            let cleaned = guard.finish().await.is_ok_and(|v| v);
            Err(if cleared && cleaned {
                RunError::AdmissionDenied
            } else {
                RunError::Cleanup
            })
        }
        Err(error) => {
            drop(fence);
            let _confirmed = guard.finish().await.is_ok_and(|v| v);
            Err(error.into())
        }
    }
}
