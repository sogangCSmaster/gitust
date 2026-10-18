//! Sandboxed DAG + Matrix Workflow Execution with Cache Restore/Save, Artifact Collection,
//! and Mandatory `***` Secret Log Redaction.

use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{atomic::AtomicBool, Arc, Mutex},
    time::Instant,
};

use axum::{
    extract::{Path as AxumPath, Request, State},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use chrono::Utc;
use gitust_core::{AgentKind, AuthError, CommitSha, EnterpriseRole, RepoId, UserId, WorktreeId};
use gitust_git::{ExpandedWorkflowJob, GitEngine, WorktreeServiceLock};
use serde::Deserialize;
use tokio::sync::OwnedMutexGuard;
use uuid::Uuid;

use crate::{
    auth::{extract_principal, extract_principal_async},
    ephemeral_checkout::CheckoutGuard,
    repository_reads::{id as parse_repo_id, response},
    repository_registry::ReadRepository,
    state::{check_repo_bola, internal_err, map_auth_err, sandbox_spec_for, AppState},
    workflow_cache_artifacts::{collect_job_artifacts, restore_job_cache, save_job_cache},
    workflows::*,
};

const MAX_TRIGGER_BODY_BYTES: usize = 16 * 1024;

fn not_found(msg: impl Into<String>) -> ApiError {
    (StatusCode::NOT_FOUND, msg.into())
}

fn conflict(msg: impl Into<String>) -> ApiError {
    (StatusCode::CONFLICT, msg.into())
}

fn expected_repo_id(state: &AppState, repo: &ReadRepository, id: RepoId) -> RepoId {
    if repo.is_default {
        state.repo_id
    } else {
        id
    }
}

fn verify_live_admin_and_writable_repo(
    state: &AppState,
    headers: &HeaderMap,
    origin_uid: UserId,
    repo_id: RepoId,
) -> Result<(gitust_core::AuthenticatedPrincipal, ReadRepository), ApiError> {
    let caller = extract_principal(headers, &state.auth_store)?;
    if caller.user_id != origin_uid || caller.role != EnterpriseRole::SiteAdmin {
        return Err(map_auth_err(AuthError::AdminPrivilegeRequired));
    }
    let repo = state
        .read_repository(repo_id)
        .ok_or_else(|| not_found("repository not found"))?;
    if repo.archived {
        return Err(conflict("repository is archived"));
    }
    if !repo.is_default && !repo.git_write {
        return Err((StatusCode::FORBIDDEN, "repository is read-only".into()));
    }
    let exp_id = expected_repo_id(state, &repo, repo_id);
    check_repo_bola(&state.auth_store, exp_id, repo.access, &caller, repo_id)?;
    Ok((caller, repo))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TriggerWorkflowRunInput {
    expected_commit: String,
    source_ref: String,
    #[serde(default)]
    workflow_name: Option<String>,
}

struct PreparedWorkflow {
    repo: ReadRepository,
    owner: WorktreeServiceLock,
    short_branch: String,
    expected_commit: CommitSha,
    workflow_name: String,
    expanded_jobs: Vec<ExpandedWorkflowJob>,
    resolved_secrets: BTreeMap<String, String>,
    secret_values: Vec<String>,
    common_git_dir: PathBuf,
}

async fn execute_single_expanded_job(
    state: &AppState,
    headers: &HeaderMap,
    origin_uid: UserId,
    operation: &Arc<OwnedMutexGuard<()>>,
    prep: &PreparedWorkflow,
    exp_job: ExpandedWorkflowJob,
) -> Result<WorkflowJobRun, ApiError> {
    let mut commands = Vec::with_capacity(exp_job.steps.len());
    for step in &exp_job.steps {
        commands.push(substitute_secrets_in_command(
            &step.run,
            &prep.resolved_secrets,
        )?);
    }

    let lease = state
        .runner_fleet
        .acquire_lease(&exp_job.runs_on, exp_job.isolation_tier)?;

    let slot = Arc::new(Mutex::new(None));
    let worktree_id = WorktreeId::new();
    prep.owner.register_ephemeral_worktree(worktree_id);
    let guard = CheckoutGuard {
        worktree_id,
        engine: prep.repo.engine.clone(),
        owner: prep.owner.clone(),
        materialized: slot.clone(),
        keep: false,
        cancelled: Arc::new(AtomicBool::new(false)),
        operation: Some(operation.clone()),
        finishing: false,
    };

    let wt_branch = format!("agent/cli/wf-{}", &worktree_id.0.to_string()[..8]);
    let eng = prep.repo.engine.clone();
    let (slot_cl, rid) = (slot.clone(), prep.repo.access.repo_id);
    let br = prep.short_branch.clone();
    let (store, common) = (state.auth_store.clone(), prep.common_git_dir.clone());
    let (cache, expected) = (exp_job.cache.clone(), prep.expected_commit.clone());
    let (st_cl, hdr_cl) = (state.clone(), headers.clone());
    let iso_tier = exp_job.isolation_tier;

    let (m, spec, fence, cache_hit) = tokio::task::spawn_blocking(move || {
        verify_live_admin_and_writable_repo(&st_cl, &hdr_cl, origin_uid, rid)?;
        let bot = AgentKind::InternalBot;
        let m = eng
            .materialize_agent_worktree(worktree_id, rid, bot, &wt_branch, &br, "WF")
            .map_err(unavailable)?;
        // fail-open-ok: poison recovery retains the cleanup slot.
        *slot_cl.lock().unwrap_or_else(|e| e.into_inner()) = Some(m.clone());
        let cleaned = eng
            .clean_worktree_commit(m.worktree.id, &m.worktree.branch)
            .map_err(unavailable)?;
        if m.worktree.base_commit != expected || cleaned != expected {
            return Err(unavailable("workflow source commit changed"));
        }
        let hit = match cache {
            Some(ref c) => restore_job_cache(&common, &m.worktree_dir, c).map_err(unavailable)?,
            None => false,
        };
        let spec = sandbox_spec_for(&m, &store)
            .map_err(unavailable)?
            .with_isolation_tier(iso_tier);
        let fence = eng
            .begin_execution_fence(&m.worktree)
            .map_err(unavailable)?;
        Ok::<_, ApiError>((m, spec, fence, hit))
    })
    .await
    .map_err(internal_err)??;

    let job_start = Instant::now();
    let exec_res = state
        .sandbox_executor
        .execute_required_checks(m.worktree.id, &spec, &commands)
        .await;

    let (mut status_str, mut combined_output, duration_ms) = match exec_res {
        Ok(res) => {
            tokio::task::spawn_blocking(move || fence.complete())
                .await
                .map_err(internal_err)?
                .map_err(unavailable)?;
            let needs_nl =
                !res.stdout.is_empty() && !res.stdout.ends_with('\n') && !res.stderr.is_empty();
            let sep = if needs_nl { "\n" } else { "" };
            let out = format!("{}{sep}{}", res.stdout, res.stderr);
            let st = if res.exit_code == 0 {
                "passed"
            } else {
                "failed"
            }
            .to_owned();
            (
                st,
                redact_secrets(&out, &prep.secret_values),
                res.duration_ms,
            )
        }
        Err(err) => {
            drop(fence);
            // fail-open-ok: display-only elapsed time saturates rather than overflowing.
            let dur = u64::try_from(job_start.elapsed().as_millis()).unwrap_or(u64::MAX);
            (
                "failed".to_owned(),
                redact_secrets(&err.to_string(), &prep.secret_values),
                dur,
            )
        }
    };

    let mut artifacts = Vec::new();
    if status_str == "passed" {
        let (p_common, p_wt) = (prep.common_git_dir.clone(), m.worktree_dir.clone());
        let (p_cache, p_art) = (exp_job.cache.clone(), exp_job.artifacts.clone());
        let p_sec = prep.secret_values.clone();
        let src_job_id = exp_job.job_id.clone();
        let post_res = tokio::task::spawn_blocking(move || {
            if let Some(ref c) = p_cache {
                save_job_cache(&p_common, &p_wt, c, Some(&src_job_id))?;
            }
            match p_art {
                Some(ref a) => collect_job_artifacts(&p_wt, a, &p_sec),
                None => Ok(Vec::new()),
            }
        })
        .await
        .map_err(internal_err)?;

        match post_res {
            Ok(collected) => artifacts = collected,
            Err(err_msg) => {
                status_str = "failed".to_owned();
                if !combined_output.is_empty() && !combined_output.ends_with('\n') {
                    combined_output.push('\n');
                }
                combined_output.push_str(&redact_secrets(&err_msg, &prep.secret_values));
            }
        }
    }

    if !guard.finish().await.map_err(unavailable)? {
        return Err(unavailable("workflow checkout cleanup unconfirmed"));
    }

    let runner_name = lease.runner_name.clone();
    drop(lease);
    let redact = combined_output;
    Ok(WorkflowJobRun::from_expanded(
        exp_job,
        Some(runner_name),
        status_str,
        cache_hit,
        artifacts,
        redact,
        duration_ms,
    ))
}

fn prepare_workflow_run(
    state: &AppState,
    headers: &HeaderMap,
    origin_uid: UserId,
    raw_id: &str,
    input: TriggerWorkflowRunInput,
) -> Result<PreparedWorkflow, ApiError> {
    let _inventory = state.repository_registry.clone();
    let repo_id = parse_repo_id(raw_id)?;
    let (caller, repo) = verify_live_admin_and_writable_repo(state, headers, origin_uid, repo_id)?;
    let eng = &repo.engine;
    let owner = eng.lock_worktree_service().map_err(unavailable)?;
    if eng.load_existing_repository_id().map_err(unavailable)? != repo_id {
        return Err(unavailable("repository identity mismatch"));
    }

    let clean_sha = input.expected_commit.trim().to_ascii_lowercase();
    if clean_sha.len() != 40 || !clean_sha.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(bad_req("expected_commit must be a 40-character hex SHA"));
    }
    let expected_commit = CommitSha(clean_sha);

    let trimmed_ref = input.source_ref.trim();
    // fail-open-ok: normalizes optional refs/heads/ prefix before strict outbound branch validation.
    let short_branch = trimmed_ref
        .strip_prefix("refs/heads/")
        .unwrap_or(trimmed_ref);
    let clean_branch =
        GitEngine::validate_outbound_branch(short_branch).map_err(map_workflow_git_err)?;
    let full_ref = format!("refs/heads/{clean_branch}");
    let live_commit = eng
        .resolve_checks_ref(&full_ref)
        .map_err(map_workflow_git_err)?;
    if live_commit != expected_commit {
        return Err(conflict(
            "workflow branch head does not match expected_commit",
        ));
    }

    let workflows = eng
        .workflows_at_commit(&expected_commit)
        .map_err(map_workflow_git_err)?;
    let raw_wf = input.workflow_name.as_deref().map(str::trim);
    let selected = match raw_wf.filter(|s| !s.is_empty()) {
        Some(n) => workflows
            .into_iter()
            .find(|w| w.name == n)
            .ok_or_else(|| not_found(format!("workflow '{n}' not found")))?,
        None => workflows
            .into_iter()
            .next()
            .ok_or_else(|| not_found("no workflow definitions found at expected_commit"))?,
    };
    let expanded_jobs = selected.expand_jobs().map_err(map_workflow_git_err)?;
    let resolved_secrets = state
        .auth_store
        .resolve_workflow_secrets(&caller, repo.access)
        .map_err(map_auth_err)?;
    for job in &expanded_jobs {
        for req_sec in &job.secrets {
            if !resolved_secrets.contains_key(req_sec) {
                return Err(bad_req(format!("missing workflow secret '{req_sec}'")));
            }
        }
    }
    let mut secret_values: Vec<String> = resolved_secrets.values().cloned().collect();
    secret_values.sort_by_key(|b| std::cmp::Reverse(b.len()));
    let common_git_dir = eng.repository_common_directory().map_err(unavailable)?;

    Ok(PreparedWorkflow {
        repo,
        owner,
        short_branch: clean_branch,
        expected_commit,
        workflow_name: selected.name,
        expanded_jobs,
        resolved_secrets,
        secret_values,
        common_git_dir,
    })
}

async fn trigger_workflow_inner(
    state: AppState,
    raw_id: String,
    request: Request,
) -> Result<WorkflowRunRecord, ApiError> {
    let (parts, body) = request.into_parts();
    let origin = extract_principal_async(&parts.headers, &state.auth_store).await?;
    if origin.role != EnterpriseRole::SiteAdmin {
        return Err(map_auth_err(AuthError::AdminPrivilegeRequired));
    }
    if !state.trusted_sandbox_exec {
        return Err((StatusCode::FORBIDDEN, "sandbox exec disabled".into()));
    }
    let too_big = |_| {
        (
            StatusCode::PAYLOAD_TOO_LARGE,
            "request body too large".into(),
        )
    };
    let bytes = axum::body::to_bytes(body, MAX_TRIGGER_BODY_BYTES)
        .await
        .map_err(too_big)?;
    let input: TriggerWorkflowRunInput =
        serde_json::from_slice(&bytes).map_err(|e| bad_req(format!("invalid JSON: {e}")))?;

    let (prep_state, prep_headers) = (state.clone(), parts.headers.clone());
    let origin_uid = origin.user_id;
    let mut prep = tokio::task::spawn_blocking(move || {
        prepare_workflow_run(&prep_state, &prep_headers, origin_uid, &raw_id, input)
    })
    .await
    .map_err(internal_err)??;

    let operation = Arc::new(
        prep.repo
            .local_checks_lock
            .clone()
            .try_lock_owned()
            .map_err(|_| conflict("another workflow or local check run is in progress"))?,
    );

    let expanded_jobs = std::mem::take(&mut prep.expanded_jobs);
    let mut job_outcomes: BTreeMap<String, bool> = BTreeMap::new();
    let mut job_runs: Vec<WorkflowJobRun> = Vec::with_capacity(expanded_jobs.len());

    for exp_job in expanded_jobs {
        let deps_passed = exp_job
            .needs
            .iter()
            .all(|dep| job_outcomes.get(dep).is_some_and(|passed| *passed));
        if !deps_passed {
            job_outcomes.insert(exp_job.job_id.clone(), false);
            let empty = String::new();
            let skipped = WorkflowJobRun::from_expanded(
                exp_job,
                None,
                "skipped".into(),
                false,
                Vec::new(),
                empty,
                0,
            );
            job_runs.push(skipped);
            continue;
        }

        let job_id_key = exp_job.job_id.clone();
        let job_run = execute_single_expanded_job(
            &state,
            &parts.headers,
            origin_uid,
            &operation,
            &prep,
            exp_job,
        )
        .await?;
        let passed = job_run.status == "passed";
        let prev = job_outcomes.get(&job_id_key).is_none_or(|passed| *passed);
        job_outcomes.insert(job_id_key, prev && passed);
        job_runs.push(job_run);
    }

    let overall_passed = job_runs.iter().all(|j| j.status == "passed");
    let run_record = WorkflowRunRecord {
        run_id: Uuid::new_v4().hyphenated().to_string(),
        repo_id: prep.repo.access.repo_id.0.to_string(),
        workflow_name: prep.workflow_name,
        commit_sha: prep.expected_commit.0,
        source_ref: format!("refs/heads/{}", prep.short_branch),
        status: if overall_passed { "passed" } else { "failed" }.to_owned(),
        jobs: job_runs,
        created_at: Utc::now().to_rfc3339(),
    };

    let final_headers = parts.headers;
    tokio::task::spawn_blocking(move || -> Result<WorkflowRunRecord, ApiError> {
        let _owner = prep.owner;
        let _op = operation;
        let repo_id = prep.repo.access.repo_id;
        verify_live_admin_and_writable_repo(&state, &final_headers, origin_uid, repo_id)?;
        save_workflow_run(&prep.common_git_dir, repo_id, &run_record)?;
        Ok(run_record)
    })
    .await
    .map_err(internal_err)?
}

pub(crate) async fn trigger_workflow_run(
    State(state): State<AppState>,
    AxumPath(raw_id): AxumPath<String>,
    request: Request,
) -> Response {
    response(trigger_workflow_inner(state, raw_id, request).await)
}
