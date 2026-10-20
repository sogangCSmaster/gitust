//! Remote runner job dispatch queue, fenced lease tokens, least-loaded multi-node assignment,
//! automatic failover when a node transitions offline, and result/artifact collection.

use std::collections::{BTreeMap, BTreeSet};

use axum::{
    extract::{Path as AxumPath, Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use gitust_core::{
    AuthError, EnterpriseRole, IsolationTier, RunnerNodeId, RunnerNodeRecord, RunnerStatus,
};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio::sync::oneshot;
use uuid::Uuid;

use crate::{
    auth::{extract_principal, extract_principal_async},
    repository_reads::response,
    state::{internal_err, map_auth_err, AppState},
    workflow_artifacts_api::save_artifact_blob,
    workflow_cache_artifacts::{MAX_ARTIFACT_PREVIEW_CHARS, MAX_ARTIFACT_TOTAL_BYTES},
    workflows::{bad_req, redact_secrets, ApiError, WorkflowArtifactEntry},
};

const MAX_COMPLETE_BODY_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
enum RemoteJobPhase {
    Queued,
    Running,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct RemoteJobView {
    pub(crate) job_run_id: String,
    pub(crate) lease_token: String,
    pub(crate) attempt: u32,
    pub(crate) job_id: String,
    pub(crate) display_name: String,
    pub(crate) runs_on: String,
    pub(crate) isolation_tier: IsolationTier,
    pub(crate) commands: Vec<String>,
    pub(crate) artifact_name: Option<String>,
    pub(crate) artifact_paths: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RemoteArtifactUpload {
    pub(crate) path: String,
    pub(crate) hex: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CompleteRemoteJobInput {
    pub(crate) lease_token: String,
    pub(crate) exit_code: i32,
    #[serde(default)]
    pub(crate) stdout: String,
    #[serde(default)]
    pub(crate) stderr: String,
    #[serde(default)]
    pub(crate) duration_ms: u64,
    #[serde(default)]
    pub(crate) artifacts: Vec<RemoteArtifactUpload>,
}

pub(crate) struct CompletedRemoteJob {
    pub(crate) final_runner_id: RunnerNodeId,
    pub(crate) final_runner_name: String,
    pub(crate) exit_code: i32,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
    pub(crate) duration_ms: u64,
    pub(crate) artifacts: Vec<RemoteArtifactUpload>,
}

pub(crate) type RemoteJobOutcome = Result<CompletedRemoteJob, (RunnerNodeId, String, String)>;

struct InFlightRemoteJob {
    job_run_id: Uuid,
    lease_token: String,
    attempt: u32,
    assigned_runner_id: RunnerNodeId,
    assigned_runner_name: String,
    phase: RemoteJobPhase,
    job_id: String,
    display_name: String,
    runs_on: String,
    isolation_tier: IsolationTier,
    commands: Vec<String>,
    artifact_name: Option<String>,
    artifact_paths: Vec<String>,
    completion_tx: Option<oneshot::Sender<RemoteJobOutcome>>,
}

#[derive(Default)]
struct RemoteDispatchInner {
    enabled_workers: BTreeSet<RunnerNodeId>,
    jobs: BTreeMap<Uuid, InFlightRemoteJob>,
}

#[derive(Default)]
pub(crate) struct RemoteDispatchStore {
    inner: Mutex<RemoteDispatchInner>,
}

impl RemoteDispatchStore {
    pub(crate) fn enable_remote_worker(&self, id: RunnerNodeId) {
        self.inner.lock().enabled_workers.insert(id);
    }

    pub(crate) fn disable_remote_worker(&self, id: RunnerNodeId) {
        self.inner.lock().enabled_workers.remove(&id);
    }

    pub(crate) fn is_remote_worker_enabled(&self, id: RunnerNodeId) -> bool {
        self.inner.lock().enabled_workers.contains(&id)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn enqueue_job(
        &self,
        assigned_runner_id: RunnerNodeId,
        assigned_runner_name: String,
        job_id: String,
        display_name: String,
        runs_on: String,
        isolation_tier: IsolationTier,
        commands: Vec<String>,
        artifact_name: Option<String>,
        artifact_paths: Vec<String>,
    ) -> (Uuid, oneshot::Receiver<RemoteJobOutcome>) {
        let job_run_id = Uuid::new_v4();
        let lease_token = format!("lease-{}-1", Uuid::new_v4());
        let (tx, rx) = oneshot::channel();
        let record = InFlightRemoteJob {
            job_run_id,
            lease_token,
            attempt: 1,
            assigned_runner_id,
            assigned_runner_name,
            phase: RemoteJobPhase::Queued,
            job_id,
            display_name,
            runs_on,
            isolation_tier,
            commands,
            artifact_name,
            artifact_paths,
            completion_tx: Some(tx),
        };
        self.inner.lock().jobs.insert(job_run_id, record);
        (job_run_id, rx)
    }

    pub(crate) fn cancel_job(&self, job_run_id: Uuid) -> Option<(RunnerNodeId, String)> {
        self.inner
            .lock()
            .jobs
            .remove(&job_run_id)
            .map(|j| (j.assigned_runner_id, j.assigned_runner_name))
    }

    pub(crate) fn poll_next_for_runner(&self, runner_id: RunnerNodeId) -> Option<RemoteJobView> {
        let mut guard = self.inner.lock();
        guard.enabled_workers.insert(runner_id);
        for job in guard.jobs.values_mut() {
            if job.assigned_runner_id == runner_id && job.phase == RemoteJobPhase::Queued {
                job.phase = RemoteJobPhase::Running;
                return Some(RemoteJobView {
                    job_run_id: job.job_run_id.hyphenated().to_string(),
                    lease_token: job.lease_token.clone(),
                    attempt: job.attempt,
                    job_id: job.job_id.clone(),
                    display_name: job.display_name.clone(),
                    runs_on: job.runs_on.clone(),
                    isolation_tier: job.isolation_tier,
                    commands: job.commands.clone(),
                    artifact_name: job.artifact_name.clone(),
                    artifact_paths: job.artifact_paths.clone(),
                });
            }
        }
        None
    }

    pub(crate) fn complete_job(
        &self,
        runner_id: RunnerNodeId,
        job_run_id: Uuid,
        input: CompleteRemoteJobInput,
    ) -> Result<RunnerNodeId, ApiError> {
        let mut guard = self.inner.lock();
        let Some(job) = guard.jobs.get(&job_run_id) else {
            return Err((
                StatusCode::CONFLICT,
                "remote job not found or already completed".into(),
            ));
        };
        if job.assigned_runner_id != runner_id || job.lease_token != input.lease_token.trim() {
            return Err((
                StatusCode::CONFLICT,
                "stale or invalid runner lease_token".into(),
            ));
        }
        let Some(mut removed) = guard.jobs.remove(&job_run_id) else {
            return Err((StatusCode::CONFLICT, "remote job already removed".into()));
        };
        if let Some(tx) = removed.completion_tx.take() {
            // fail-open-ok: oneshot receiver is only dropped if the waiting workflow task already timed out and cancelled the run.
            let _ = tx.send(Ok(CompletedRemoteJob {
                final_runner_id: removed.assigned_runner_id,
                final_runner_name: removed.assigned_runner_name,
                exit_code: input.exit_code,
                stdout: input.stdout,
                stderr: input.stderr,
                duration_ms: input.duration_ms,
                artifacts: input.artifacts,
            }));
        }
        Ok(runner_id)
    }

    pub(crate) fn failover_jobs_from_runner(
        &self,
        failed_runner_id: RunnerNodeId,
        nodes: &mut BTreeMap<RunnerNodeId, RunnerNodeRecord>,
        builtin_local_uuid: u128,
    ) {
        let mut guard = self.inner.lock();
        let RemoteDispatchInner {
            ref enabled_workers,
            ref mut jobs,
        } = *guard;
        let mut to_fail = Vec::new();
        for (job_id, job) in jobs.iter_mut() {
            if job.assigned_runner_id != failed_runner_id {
                continue;
            }
            // Select least-loaded healthy online remote runner matching label + tier
            let mut candidate: Option<(RunnerNodeId, String, u32)> = None;
            for (cand_id, cand_node) in nodes.iter() {
                if *cand_id == failed_runner_id || cand_id.0.as_u128() == builtin_local_uuid {
                    continue;
                }
                if !enabled_workers.contains(cand_id)
                    || cand_node.status != RunnerStatus::Online
                    || cand_node.active_leases >= cand_node.max_concurrency
                {
                    continue;
                }
                if !cand_node.labels.iter().any(|l| l == &job.runs_on)
                    || !cand_node.supported_tiers.contains(&job.isolation_tier)
                {
                    continue;
                }
                match candidate {
                    None => {
                        candidate =
                            Some((*cand_id, cand_node.name.clone(), cand_node.active_leases))
                    }
                    Some((best_id, ref best_name, best_leases))
                        if (cand_node.active_leases, cand_node.name.as_str(), *cand_id)
                            < (best_leases, best_name.as_str(), best_id) =>
                    {
                        candidate =
                            Some((*cand_id, cand_node.name.clone(), cand_node.active_leases))
                    }
                    Some(_) => {}
                }
            }
            if let Some((new_id, new_name, _)) = candidate {
                if let Some(old_node) = nodes.get_mut(&failed_runner_id) {
                    old_node.active_leases = old_node.active_leases.saturating_sub(1);
                }
                if let Some(new_node) = nodes.get_mut(&new_id) {
                    new_node.active_leases = new_node.active_leases.saturating_add(1);
                }
                job.assigned_runner_id = new_id;
                job.assigned_runner_name = new_name;
                job.attempt = job.attempt.saturating_add(1);
                job.lease_token = format!("lease-{}-{}", Uuid::new_v4(), job.attempt);
                job.phase = RemoteJobPhase::Queued;
            } else {
                to_fail.push(*job_id);
            }
        }
        for job_id in to_fail {
            if let Some(mut removed) = guard.jobs.remove(&job_id) {
                if let Some(tx) = removed.completion_tx.take() {
                    let msg = format!(
                        "remote runner '{}' went offline and no healthy fallback runner is available",
                        removed.assigned_runner_name
                    );
                    // fail-open-ok: oneshot receiver is only dropped if the waiting workflow task already timed out and failed the run.
                    let _ = tx.send(Err((
                        removed.assigned_runner_id,
                        removed.assigned_runner_name,
                        msg,
                    )));
                }
            }
        }
    }
}

pub(crate) fn collect_remote_job_artifacts(
    common_git_dir: &std::path::Path,
    run_id: &str,
    artifact_name: &str,
    uploads: Vec<RemoteArtifactUpload>,
    secret_values: &[String],
) -> Result<Vec<WorkflowArtifactEntry>, String> {
    let mut out = Vec::with_capacity(uploads.len());
    let mut total_bytes = 0u64;
    for up in uploads {
        let bytes =
            hex::decode(up.hex.trim()).map_err(|_| "invalid hex in remote artifact".to_owned())?;
        total_bytes = total_bytes.saturating_add(bytes.len() as u64);
        if total_bytes > MAX_ARTIFACT_TOTAL_BYTES {
            return Err("remote workflow artifacts exceed 1 MiB limit".into());
        }
        let clean_name = redact_secrets(artifact_name, secret_values);
        let clean_path = redact_secrets(&up.path, secret_values);
        let sha256_hex =
            save_artifact_blob(common_git_dir, run_id, &clean_name, &clean_path, &bytes)
                .map_err(|(_, m)| m)?;
        let digest: [u8; 32] = Sha256::digest(&bytes).into();
        debug_assert_eq!(sha256_hex, hex::encode(digest));
        let preview: String = String::from_utf8_lossy(&bytes)
            .chars()
            .take(MAX_ARTIFACT_PREVIEW_CHARS)
            .collect();
        out.push(WorkflowArtifactEntry {
            name: clean_name,
            path: clean_path,
            size_bytes: bytes.len() as u64,
            sha256: Some(sha256_hex),
            content_preview: Some(redact_secrets(&preview, secret_values)),
        });
    }
    Ok(out)
}

pub(crate) async fn execute_remote_expanded_job(
    state: &AppState,
    common_git_dir: &std::path::Path,
    secret_values: &[String],
    run_id: &str,
    mut lease: crate::runners::RunnerLeaseGuard,
    exp_job: gitust_git::ExpandedWorkflowJob,
    commands: Vec<String>,
) -> Result<crate::workflows::WorkflowJobRun, ApiError> {
    let rx = state
        .runner_fleet
        .enqueue_remote_job(&mut lease, &exp_job, commands);
    let wait_res = tokio::time::timeout(std::time::Duration::from_secs(15), rx).await;
    let completed_res = match wait_res {
        Ok(Ok(Ok(completed))) => {
            lease.in_flight_job_id = None;
            lease.runner_id = completed.final_runner_id;
            lease.runner_name = completed.final_runner_name.clone();
            Ok(completed)
        }
        Ok(Ok(Err((final_id, final_name, err_msg)))) => {
            lease.in_flight_job_id = None;
            lease.runner_id = final_id;
            lease.runner_name = final_name;
            Err(err_msg)
        }
        Ok(Err(_)) => {
            state.runner_fleet.cancel_lease_remote_job(&mut lease);
            Err("remote runner job channel closed unexpectedly".to_owned())
        }
        Err(_) => {
            state.runner_fleet.cancel_lease_remote_job(&mut lease);
            Err("remote runner job timed out waiting for completion".to_owned())
        }
    };

    match completed_res {
        Ok(completed) => {
            let needs_nl = !completed.stdout.is_empty()
                && !completed.stdout.ends_with('\n')
                && !completed.stderr.is_empty();
            let sep = if needs_nl { "\n" } else { "" };
            let out = format!("{}{sep}{}", completed.stdout, completed.stderr);
            let mut status_str = if completed.exit_code == 0 {
                "passed"
            } else {
                "failed"
            }
            .to_owned();
            let mut combined_output = redact_secrets(&out, secret_values);
            let mut artifacts = Vec::new();
            if status_str == "passed" && !completed.artifacts.is_empty() {
                let art_name = match exp_job.artifacts.as_ref() {
                    Some(a) => a.name.clone(),
                    None => "remote-artifact".to_owned(),
                };
                let p_common = common_git_dir.to_path_buf();
                let p_run_id = run_id.to_owned();
                let p_sec = secret_values.to_vec();
                let uploads = completed.artifacts;
                let art_res = tokio::task::spawn_blocking(move || {
                    collect_remote_job_artifacts(&p_common, &p_run_id, &art_name, uploads, &p_sec)
                })
                .await
                .map_err(internal_err)?;
                match art_res {
                    Ok(collected) => artifacts = collected,
                    Err(err_msg) => {
                        status_str = "failed".to_owned();
                        if !combined_output.is_empty() && !combined_output.ends_with('\n') {
                            combined_output.push('\n');
                        }
                        combined_output.push_str(&redact_secrets(&err_msg, secret_values));
                    }
                }
            }
            let runner_name = lease.runner_name.clone();
            drop(lease);
            Ok(crate::workflows::WorkflowJobRun::from_expanded(
                exp_job,
                Some(runner_name),
                status_str,
                false,
                artifacts,
                combined_output,
                completed.duration_ms,
            ))
        }
        Err(err_msg) => {
            let runner_name = lease.runner_name.clone();
            drop(lease);
            Ok(crate::workflows::WorkflowJobRun::from_expanded(
                exp_job,
                Some(runner_name),
                "failed".into(),
                false,
                Vec::new(),
                redact_secrets(&err_msg, secret_values),
                0,
            ))
        }
    }
}

pub(crate) async fn poll_runner_job_handler(
    State(state): State<AppState>,
    AxumPath(raw_runner_id): AxumPath<String>,
    request: Request,
) -> Response {
    let res = async move {
        let (parts, _body) = request.into_parts();
        let origin = extract_principal_async(&parts.headers, &state.auth_store).await?;
        if origin.role != EnterpriseRole::SiteAdmin {
            return Err(map_auth_err(AuthError::AdminPrivilegeRequired));
        }
        let runner_id = Uuid::parse_str(raw_runner_id.trim())
            .map(RunnerNodeId)
            .map_err(|_| bad_req("invalid runner UUID"))?;
        tokio::task::spawn_blocking(move || {
            let caller = extract_principal(&parts.headers, &state.auth_store)?;
            if caller.user_id != origin.user_id || caller.role != EnterpriseRole::SiteAdmin {
                return Err(map_auth_err(AuthError::AdminPrivilegeRequired));
            }
            let job = state.runner_fleet.poll_remote_job(runner_id)?;
            Ok(Json(json!({ "job": job })).into_response())
        })
        .await
        .map_err(internal_err)?
    }
    .await;
    match res {
        Ok(resp) => resp,
        Err(err) => response::<()>(Err(err)),
    }
}

pub(crate) async fn complete_runner_job_handler(
    State(state): State<AppState>,
    AxumPath((raw_runner_id, raw_job_id)): AxumPath<(String, String)>,
    request: Request,
) -> Response {
    let res = async move {
        let (parts, body) = request.into_parts();
        let origin = extract_principal_async(&parts.headers, &state.auth_store).await?;
        if origin.role != EnterpriseRole::SiteAdmin {
            return Err(map_auth_err(AuthError::AdminPrivilegeRequired));
        }
        let runner_id = Uuid::parse_str(raw_runner_id.trim())
            .map(RunnerNodeId)
            .map_err(|_| bad_req("invalid runner UUID"))?;
        let job_run_id =
            Uuid::parse_str(raw_job_id.trim()).map_err(|_| bad_req("invalid job_run_id UUID"))?;
        let bytes = axum::body::to_bytes(body, MAX_COMPLETE_BODY_BYTES)
            .await
            .map_err(|_| {
                (
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "completion payload exceeds size limit".into(),
                )
            })?;
        let input: CompleteRemoteJobInput =
            serde_json::from_slice(&bytes).map_err(|e| bad_req(format!("invalid JSON: {e}")))?;
        tokio::task::spawn_blocking(move || {
            let caller = extract_principal(&parts.headers, &state.auth_store)?;
            if caller.user_id != origin.user_id || caller.role != EnterpriseRole::SiteAdmin {
                return Err(map_auth_err(AuthError::AdminPrivilegeRequired));
            }
            state
                .runner_fleet
                .complete_remote_job(runner_id, job_run_id, input)?;
            Ok(Json(json!({ "status": "completed" })).into_response())
        })
        .await
        .map_err(internal_err)?
    }
    .await;
    match res {
        Ok(resp) => resp,
        Err(err) => response::<()>(Err(err)),
    }
}
