//! GitHub Actions-compatible Workflows API (`GET /workflows`, `GET /workflow-runs/{run_id}/artifacts`,
//! and `GET/PUT/DELETE /workflow-secrets`) with encrypted secret management and symlink-safe persistence.

use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

use axum::{
    extract::{Path as AxumPath, Request, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use gitust_core::{AuthError, AuthenticatedPrincipal, EnterpriseRole, RepoId, UserId};
use gitust_git::{ExpandedWorkflowJob, GitEngineError};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::{
    auth::{extract_principal, extract_principal_async},
    repository_reads::{id as parse_repo_id, response},
    repository_registry::ReadRepository,
    state::{check_repo_bola, internal_err, map_auth_err, AppState},
};

pub(crate) type ApiError = (StatusCode, String);

const WORKFLOWS_STORAGE_DIR: &str = "gitust-workflows";
const RUNS_FILE_NAME: &str = "runs.json";
const ARTIFACTS_DIR_NAME: &str = "artifacts";
pub(crate) const CACHES_DIR_NAME: &str = "caches";
const MAX_RUNS_JSON_BYTES: u64 = 4 * 1024 * 1024;
const MAX_ARTIFACTS_JSON_BYTES: u64 = 2 * 1024 * 1024;
const MAX_SAVED_WORKFLOW_RUNS: usize = 25;
const MAX_SECRET_BODY_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowArtifactEntry {
    pub name: String,
    pub path: String,
    pub size_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_preview: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowJobRun {
    pub job_id: String,
    pub display_name: String,
    pub needs: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runner_id: Option<String>,
    #[serde(default)]
    pub isolation_tier: gitust_core::IsolationTier,
    pub matrix_values: BTreeMap<String, String>,
    pub status: String,
    pub cache_hit: bool,
    pub artifacts: Vec<WorkflowArtifactEntry>,
    pub redacted_output: String,
    pub duration_ms: u64,
}

impl WorkflowJobRun {
    pub(crate) fn from_expanded(
        exp: ExpandedWorkflowJob,
        runner_id: Option<String>,
        status: String,
        cache_hit: bool,
        artifacts: Vec<WorkflowArtifactEntry>,
        redacted_output: String,
        duration_ms: u64,
    ) -> Self {
        Self {
            job_id: exp.job_id,
            display_name: exp.display_name,
            needs: exp.needs,
            runner_id,
            isolation_tier: exp.isolation_tier,
            matrix_values: exp.matrix_values,
            status,
            cache_hit,
            artifacts,
            redacted_output,
            duration_ms,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowRunRecord {
    pub run_id: String,
    pub repo_id: String,
    pub workflow_name: String,
    pub commit_sha: String,
    pub source_ref: String,
    pub status: String,
    pub jobs: Vec<WorkflowJobRun>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredRunsEnvelope {
    version: u32,
    repo_id: RepoId,
    runs: Vec<WorkflowRunRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredArtifactsEnvelope {
    version: u32,
    run_id: String,
    artifacts: Vec<WorkflowArtifactEntry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PutWorkflowSecretRequest {
    value: String,
}

pub(crate) fn unavailable(_: impl std::fmt::Display) -> ApiError {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        "repository workflow data is unavailable".into(),
    )
}

pub(crate) fn bad_req(msg: impl Into<String>) -> ApiError {
    (StatusCode::BAD_REQUEST, msg.into())
}

fn no_store_json<T: Serialize>(res: Result<T, ApiError>) -> Response {
    match res {
        Ok(payload) => ([(header::CACHE_CONTROL, "no-store")], Json(payload)).into_response(),
        Err(err) => response::<()>(Err(err)),
    }
}

pub(crate) fn map_workflow_git_err(err: GitEngineError) -> ApiError {
    match err {
        GitEngineError::VerificationInvalid(msg)
        | GitEngineError::InvalidRevision(msg)
        | GitEngineError::InvalidRepoPath(msg)
        | GitEngineError::InvalidBranchName(msg) => (StatusCode::BAD_REQUEST, msg),
        GitEngineError::RevisionNotFound(msg) | GitEngineError::PathNotFound(msg) => {
            (StatusCode::NOT_FOUND, msg)
        }
        other => unavailable(other),
    }
}

/// Replaces every occurrence of every configured secret value with `"***"`.
pub(crate) fn redact_secrets(input: &str, secret_values: &[String]) -> String {
    let mut out = input.to_owned();
    for secret in secret_values {
        if !secret.is_empty() {
            out = out.replace(secret.as_str(), "***");
        }
    }
    out
}

pub(crate) fn substitute_secrets_in_command(
    command: &str,
    secrets: &BTreeMap<String, String>,
) -> Result<String, ApiError> {
    let mut out = String::with_capacity(command.len());
    let mut rest = command;
    while let Some(start) = rest.find("${{") {
        out.push_str(&rest[..start]);
        let after_start = &rest[start + 3..];
        let Some(end) = after_start.find("}}") else {
            return Err(bad_req("unclosed template expression in workflow step"));
        };
        let expr = after_start[..end].trim();
        let Some(sec_name) = expr.strip_prefix("secrets.").map(str::trim) else {
            return Err(bad_req(format!(
                "unresolved workflow expression '${{{{ {expr} }}}}'"
            )));
        };
        let val = secrets
            .get(sec_name)
            .ok_or_else(|| bad_req(format!("workflow secret '{sec_name}' is not configured")))?;
        if val.contains('\0') || val.contains('\n') || val.contains('\r') {
            return Err(bad_req(
                "workflow secret value must not contain NUL or newlines",
            ));
        }
        out.push_str(val);
        rest = &after_start[end + 2..];
    }
    out.push_str(rest);
    Ok(out)
}

pub(crate) fn ensure_safe_dir(path: &Path) -> Result<(), ApiError> {
    if let Err(e) = fs::symlink_metadata(path) {
        if e.kind() != std::io::ErrorKind::NotFound {
            return Err(unavailable(e));
        }
        fs::create_dir_all(path).map_err(unavailable)?;
    }
    let meta = fs::symlink_metadata(path).map_err(unavailable)?;
    if meta.file_type().is_symlink() || !meta.file_type().is_dir() {
        return Err(unavailable("workflow storage dir is not a safe directory"));
    }
    Ok(())
}

pub(crate) fn workflows_storage_root(common_git_dir: &Path) -> Result<PathBuf, ApiError> {
    let root = common_git_dir.join(WORKFLOWS_STORAGE_DIR);
    ensure_safe_dir(&root)?;
    Ok(root)
}

pub(crate) fn read_bounded_regular_file(
    path: &Path,
    max_bytes: u64,
) -> Result<Option<Vec<u8>>, ApiError> {
    let meta = match fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(unavailable(e)),
    };
    if meta.file_type().is_symlink() || !meta.file_type().is_file() || meta.len() > max_bytes {
        return Err(unavailable(
            "workflow storage file invalid or exceeds size limit",
        ));
    }
    let mut buf = Vec::new();
    fs::File::open(path)
        .map_err(unavailable)?
        .take(max_bytes + 1)
        .read_to_end(&mut buf)
        .map_err(unavailable)?;
    if buf.len() as u64 > max_bytes {
        return Err(unavailable("workflow storage file exceeds size limit"));
    }
    Ok(Some(buf))
}

pub(crate) fn atomic_write_regular_file(
    dir: &Path,
    final_path: &Path,
    payload: &[u8],
) -> Result<(), ApiError> {
    ensure_safe_dir(dir)?;
    match fs::symlink_metadata(final_path) {
        Ok(meta) if meta.file_type().is_symlink() || !meta.file_type().is_file() => {
            return Err(unavailable(
                "refusing to overwrite non-regular workflow file",
            ));
        }
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(unavailable(e)),
    }
    let tmp_path = dir.join(format!(".tmp-wf-{}", Uuid::new_v4()));
    let write_res = (|| -> std::io::Result<()> {
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp_path)?;
        f.write_all(payload)?;
        f.sync_all()?;
        fs::rename(&tmp_path, final_path)
    })();
    if let Err(err) = write_res {
        // fail-open-ok: best-effort temp file cleanup on failed atomic write; error is returned below.
        let _ = fs::remove_file(&tmp_path);
        return Err(unavailable(err));
    }
    Ok(())
}

pub(crate) fn load_workflow_runs(
    common_git_dir: &Path,
    repo_id: RepoId,
) -> Result<Vec<WorkflowRunRecord>, ApiError> {
    let path = workflows_storage_root(common_git_dir)?.join(RUNS_FILE_NAME);
    let Some(bytes) = read_bounded_regular_file(&path, MAX_RUNS_JSON_BYTES)? else {
        return Ok(Vec::new());
    };
    let env: StoredRunsEnvelope = serde_json::from_slice(&bytes).map_err(unavailable)?;
    if env.version != 1 || env.repo_id != repo_id {
        return Err(unavailable("workflow runs storage identity mismatch"));
    }
    Ok(env.runs)
}

pub(crate) fn save_workflow_run(
    common_git_dir: &Path,
    repo_id: RepoId,
    run: &WorkflowRunRecord,
) -> Result<(), ApiError> {
    let root = workflows_storage_root(common_git_dir)?;
    let mut runs = load_workflow_runs(common_git_dir, repo_id)?;
    runs.insert(0, run.clone());
    runs.truncate(MAX_SAVED_WORKFLOW_RUNS);
    let runs_env = StoredRunsEnvelope {
        version: 1,
        repo_id,
        runs,
    };
    let runs_payload = serde_json::to_vec_pretty(&runs_env).map_err(unavailable)?;
    if runs_payload.len() as u64 > MAX_RUNS_JSON_BYTES {
        return Err(unavailable("workflow runs history exceeds size limit"));
    }
    atomic_write_regular_file(&root, &root.join(RUNS_FILE_NAME), &runs_payload)?;

    let artifacts_dir = root.join(ARTIFACTS_DIR_NAME);
    ensure_safe_dir(&artifacts_dir)?;
    let artifacts = run
        .jobs
        .iter()
        .flat_map(|j| j.artifacts.iter().cloned())
        .collect();
    let art_env = StoredArtifactsEnvelope {
        version: 1,
        run_id: run.run_id.clone(),
        artifacts,
    };
    let art_payload = serde_json::to_vec_pretty(&art_env).map_err(unavailable)?;
    if art_payload.len() as u64 > MAX_ARTIFACTS_JSON_BYTES {
        return Err(unavailable(
            "workflow artifacts metadata exceeds size limit",
        ));
    }
    let art_path = artifacts_dir.join(format!("{}.json", run.run_id));
    atomic_write_regular_file(&artifacts_dir, &art_path, &art_payload)
}

fn with_admin_repo_read<T, F>(
    state: &AppState,
    raw_id: &str,
    headers: &HeaderMap,
    op: F,
) -> Result<T, ApiError>
where
    F: FnOnce(&ReadRepository, RepoId, &Path) -> Result<T, ApiError>,
{
    let _inventory = state.repository_registry.clone();
    let origin = extract_principal(headers, &state.auth_store)?;
    if origin.role != EnterpriseRole::SiteAdmin {
        return Err(map_auth_err(AuthError::AdminPrivilegeRequired));
    }
    let repo_id = parse_repo_id(raw_id)?;
    let repo = state
        .read_repository(repo_id)
        .ok_or_else(|| (StatusCode::NOT_FOUND, "repository not found".into()))?;
    let exp_id = if repo.is_default {
        state.repo_id
    } else {
        repo_id
    };
    check_repo_bola(&state.auth_store, exp_id, repo.access, &origin, repo_id)?;
    let _ownership = repo.engine.lock_worktree_service().map_err(unavailable)?;
    if repo
        .engine
        .load_existing_repository_id()
        .map_err(unavailable)?
        != repo_id
    {
        return Err(unavailable("repository identity mismatch"));
    }
    let common = repo
        .engine
        .repository_common_directory()
        .map_err(unavailable)?;
    let result = op(&repo, repo_id, &common)?;
    let current = extract_principal(headers, &state.auth_store)?;
    if current.user_id != origin.user_id || current.role != EnterpriseRole::SiteAdmin {
        return Err(map_auth_err(AuthError::AdminPrivilegeRequired));
    }
    check_repo_bola(&state.auth_store, exp_id, repo.access, &current, repo_id)?;
    Ok(result)
}

fn with_admin_repo_write<T, F>(
    state: &AppState,
    raw_id: &str,
    headers: &HeaderMap,
    initial_uid: UserId,
    op: F,
) -> Result<T, ApiError>
where
    F: FnOnce(&AuthenticatedPrincipal, &ReadRepository) -> Result<T, ApiError>,
{
    let _inventory = state.repository_registry.clone();
    let caller = extract_principal(headers, &state.auth_store)?;
    if caller.user_id != initial_uid || caller.role != EnterpriseRole::SiteAdmin {
        return Err(map_auth_err(AuthError::AdminPrivilegeRequired));
    }
    let repo_id = parse_repo_id(raw_id)?;
    let repo = state
        .read_repository(repo_id)
        .ok_or_else(|| (StatusCode::NOT_FOUND, "repository not found".into()))?;
    if repo.archived {
        return Err((StatusCode::CONFLICT, "repository is archived".into()));
    }
    op(&caller, &repo)
}

pub(crate) async fn list_workflows_and_runs(
    State(state): State<AppState>,
    AxumPath(raw_id): AxumPath<String>,
    headers: HeaderMap,
) -> Response {
    let res = tokio::task::spawn_blocking(move || {
        with_admin_repo_read(&state, &raw_id, &headers, |repo, repo_id, common| {
            let default_branch = match repo.engine.committed_default_branch() {
                Ok(branch) => branch,
                Err(GitEngineError::RevisionNotFound(_)) => "main".to_owned(),
                Err(e) => return Err(unavailable(e)),
            };
            let workflows = match repo.engine.workflows_for_branch(&default_branch) {
                Ok((_sha, wfs)) => wfs,
                Err(GitEngineError::RevisionNotFound(_)) => Vec::new(),
                Err(e) => return Err(map_workflow_git_err(e)),
            };
            let runs = load_workflow_runs(common, repo_id)?;
            Ok(json!({ "workflows": workflows, "runs": runs }))
        })
    })
    .await
    .map_err(internal_err)
    .and_then(|v| v);
    no_store_json(res)
}

pub(crate) async fn list_run_artifacts(
    State(state): State<AppState>,
    AxumPath((raw_id, raw_run_id)): AxumPath<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let res = tokio::task::spawn_blocking(move || {
        let parsed =
            Uuid::parse_str(raw_run_id.trim()).map_err(|_| bad_req("invalid workflow run_id"))?;
        if parsed.is_nil() {
            return Err(bad_req("invalid workflow run_id"));
        }
        let clean_run_id = parsed.hyphenated().to_string();
        with_admin_repo_read(&state, &raw_id, &headers, |_repo, _repo_id, common| {
            let art_path = workflows_storage_root(common)?
                .join(ARTIFACTS_DIR_NAME)
                .join(format!("{clean_run_id}.json"));
            let Some(bytes) = read_bounded_regular_file(&art_path, MAX_ARTIFACTS_JSON_BYTES)?
            else {
                return Err((
                    StatusCode::NOT_FOUND,
                    "workflow run artifacts not found".into(),
                ));
            };
            let env: StoredArtifactsEnvelope =
                serde_json::from_slice(&bytes).map_err(unavailable)?;
            if env.version != 1 || env.run_id != clean_run_id {
                return Err(unavailable("workflow artifacts identity mismatch"));
            }
            Ok(json!({ "artifacts": env.artifacts }))
        })
    })
    .await
    .map_err(internal_err)
    .and_then(|v| v);
    no_store_json(res)
}

pub(crate) async fn list_secrets(
    State(state): State<AppState>,
    AxumPath(raw_id): AxumPath<String>,
    headers: HeaderMap,
) -> Response {
    let res = tokio::task::spawn_blocking(move || {
        let _inventory = state.repository_registry.clone();
        let caller = extract_principal(&headers, &state.auth_store)?;
        if caller.role != EnterpriseRole::SiteAdmin {
            return Err(map_auth_err(AuthError::AdminPrivilegeRequired));
        }
        let repo_id = parse_repo_id(&raw_id)?;
        let repo = state
            .read_repository(repo_id)
            .ok_or_else(|| (StatusCode::NOT_FOUND, "repository not found".into()))?;
        let secrets = state
            .auth_store
            .list_workflow_secrets(&caller, repo.access)
            .map_err(map_auth_err)?;
        Ok(json!({ "secrets": secrets }))
    })
    .await
    .map_err(internal_err)
    .and_then(|v| v);
    no_store_json(res)
}

pub(crate) async fn put_secret(
    State(state): State<AppState>,
    AxumPath((raw_id, name)): AxumPath<(String, String)>,
    request: Request,
) -> Response {
    let res = async move {
        let (parts, body) = request.into_parts();
        let initial = extract_principal_async(&parts.headers, &state.auth_store).await?;
        if initial.role != EnterpriseRole::SiteAdmin {
            return Err(map_auth_err(AuthError::AdminPrivilegeRequired));
        }
        let too_big = |_| {
            (
                StatusCode::PAYLOAD_TOO_LARGE,
                "request body too large".into(),
            )
        };
        let bytes = axum::body::to_bytes(body, MAX_SECRET_BODY_BYTES)
            .await
            .map_err(too_big)?;
        let req: PutWorkflowSecretRequest =
            serde_json::from_slice(&bytes).map_err(|e| bad_req(format!("invalid JSON: {e}")))?;
        tokio::task::spawn_blocking(move || {
            with_admin_repo_write(
                &state,
                &raw_id,
                &parts.headers,
                initial.user_id,
                |caller, repo| {
                    state
                        .auth_store
                        .set_workflow_secret(caller, repo.access, &name, &req.value)
                        .map_err(map_auth_err)
                },
            )
        })
        .await
        .map_err(internal_err)?
    }
    .await;
    response(res)
}

pub(crate) async fn delete_secret(
    State(state): State<AppState>,
    AxumPath((raw_id, name)): AxumPath<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let res: Result<StatusCode, ApiError> = async move {
        let initial = extract_principal_async(&headers, &state.auth_store).await?;
        if initial.role != EnterpriseRole::SiteAdmin {
            return Err(map_auth_err(AuthError::AdminPrivilegeRequired));
        }
        tokio::task::spawn_blocking(move || {
            with_admin_repo_write(
                &state,
                &raw_id,
                &headers,
                initial.user_id,
                |caller, repo| {
                    state
                        .auth_store
                        .delete_workflow_secret(caller, repo.access, &name)
                        .map_err(map_auth_err)?;
                    Ok(StatusCode::NO_CONTENT)
                },
            )
        })
        .await
        .map_err(internal_err)?
    }
    .await;
    match res {
        Ok(code) => code.into_response(),
        Err(err) => response::<()>(Err(err)),
    }
}
