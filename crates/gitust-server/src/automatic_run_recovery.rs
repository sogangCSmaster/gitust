//! Startup reconciliation never repeats an execution whose outcome is uncertain.
use crate::{
    automatic_run_model::{Origin, Phase, Reason},
    automatic_run_store::{Failure, Store},
    state::{map_git_err, AppState},
};
use gitust_core::WorktreeStatus;

pub(crate) fn reconcile(state: &AppState, store: &Store) -> Result<(), Failure> {
    for doc in store.documents()? {
        let wt = state
            .worktrees
            .read()
            .iter()
            .find(|w| w.id == doc.worktree_id)
            .cloned();
        let mut uncertain = false;
        store.update(doc.worktree_id, &doc.branch, &doc.base_branch, |current| {
            let valid = wt.as_ref().is_some_and(|w| {
                w.repo_id == state.repo_id
                    && w.branch == doc.branch
                    && w.base_branch == doc.base_branch
                    && !matches!(w.status, WorktreeStatus::Merged | WorktreeStatus::Discarded)
            });
            if let Some(job) = current.running.take() {
                let completed = crate::run_history::read(
                    &state
                        .git_engine
                        .repository_common_directory()
                        .map_err(map_git_err)?,
                    doc.worktree_id,
                    &doc.branch,
                )?
                .into_iter()
                .find(|r| r.result.run_id == job.run_id);
                if completed.as_ref().is_some_and(|r| {
                    !r.automatic
                        || r.user_id != job.user_id
                        || r.checked_commit.as_ref() != Some(&job.expected)
                        || r.checks_blob_oid.as_ref() != Some(&job.checks_oid)
                }) {
                    return Err((
                        axum::http::StatusCode::SERVICE_UNAVAILABLE,
                        "automatic history binding does not match its durable intent".into(),
                    ));
                }
                let phase = match completed {
                    Some(record) if record.result.exit_code == 0 => Phase::Completed,
                    Some(_) => Phase::Failed,
                    None => Phase::Interrupted,
                };
                uncertain = matches!(phase, Phase::Interrupted | Phase::Failed);
                current.finish(job, phase);
            }
            if let Some(job) = current.prepared.as_ref() {
                if !valid || job.origin == Origin::LocalOperator {
                    let job = current.prepared.take().ok_or_else(|| {
                        (
                            axum::http::StatusCode::SERVICE_UNAVAILABLE,
                            "automatic recovery changed".into(),
                        )
                    })?;
                    let reason = if job.origin == Origin::LocalOperator {
                        Reason::OperatorContextEnded
                    } else {
                        Reason::StateChanged
                    };
                    current.finish_reason(job, Phase::Cancelled, reason);
                } else {
                    let actual = state
                        .git_engine
                        .resolve_branch_oid(&doc.branch)
                        .map_err(map_git_err)?;
                    if actual.as_deref() == Some(job.expected.0.as_str()) {
                        let clean = state
                            .git_engine
                            .clean_worktree_commit(doc.worktree_id, &doc.branch);
                        if clean.as_ref().is_ok_and(|sha| sha == &job.expected) {
                            current.queue_prepared();
                        } else {
                            let job = current.prepared.take().ok_or_else(|| {
                                (
                                    axum::http::StatusCode::SERVICE_UNAVAILABLE,
                                    "automatic recovery changed".into(),
                                )
                            })?;
                            current.finish(job, Phase::Blocked);
                        }
                    } else {
                        let phase =
                            if actual.as_deref() == job.previous.as_ref().map(|s| s.0.as_str()) {
                                Phase::NotApplied
                            } else {
                                Phase::Superseded
                            };
                        let job = current.prepared.take().ok_or_else(|| {
                            (
                                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                                "automatic recovery changed".into(),
                            )
                        })?;
                        current.finish(job, phase);
                    }
                }
            }
            if current
                .queued
                .as_ref()
                .is_some_and(|j| !valid || j.origin == Origin::LocalOperator)
            {
                let job = current.queued.take().ok_or_else(|| {
                    (
                        axum::http::StatusCode::SERVICE_UNAVAILABLE,
                        "automatic recovery changed".into(),
                    )
                })?;
                let reason = if job.origin == Origin::LocalOperator {
                    Reason::OperatorContextEnded
                } else {
                    Reason::StateChanged
                };
                current.finish_reason(job, Phase::Cancelled, reason);
            }
            Ok(())
        })?;
        if uncertain {
            crate::state::set_worktree_status(state, doc.worktree_id, |wt| {
                wt.status = WorktreeStatus::Failed
            });
        }
    }
    Ok(())
}
