//! Explicit primary diagnostics never alter worktree verification or automatic dispatch.
use crate::{
    auth::{extract_principal, extract_principal_async},
    ephemeral_sandbox::{run_prepared_sandbox, RunCommands, RunContext, RunError},
    local_checks_history::LocalChecksRecord,
    repository_local_checks_result::{save, Failure, Report},
    state::{check_repo_bola, AppState},
};
use axum::{
    extract::{Path, Request, State},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use gitust_core::{CommitSha, EnterpriseRole, UserId};
use gitust_sandbox::SandboxError;
use serde::Deserialize;
use std::sync::{Arc, Mutex};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    expected_source_ref: String,
    expected_commit: String,
    timeout_secs: Option<u64>,
}
fn forbidden() -> Failure {
    Failure::new(StatusCode::FORBIDDEN, "local_checks_forbidden")
}
fn auth_error((status, _): (StatusCode, String), final_check: bool) -> Failure {
    if status.is_server_error() {
        Failure::unavailable()
    } else if status == StatusCode::UNAUTHORIZED && !final_check {
        Failure::new(status, "local_checks_unauthorized")
    } else {
        forbidden()
    }
}
fn authorize(state: &AppState, headers: &HeaderMap, origin: UserId) -> Result<(), Failure> {
    let current = extract_principal(headers, &state.auth_store).map_err(|e| auth_error(e, true))?;
    if current.user_id != origin || current.role != EnterpriseRole::SiteAdmin {
        return Err(forbidden());
    }
    check_repo_bola(
        &state.auth_store,
        state.repo_id,
        state.repo_access,
        &current,
        state.repo_id,
    )
    .map_err(|e| auth_error(e, true))?;
    if state
        .git_engine
        .load_existing_repository_id()
        .map_err(|_| Failure::unavailable())?
        != state.repo_id
    {
        return Err(Failure::unavailable());
    }
    if !state.trusted_sandbox_exec {
        return Err(Failure::new(StatusCode::FORBIDDEN, "local_checks_disabled"));
    }
    Ok(())
}
fn selected(state: &AppState, source: &str, expected: &CommitSha) -> Result<String, Failure> {
    let changed = || Failure::new(StatusCode::CONFLICT, "local_checks_target_changed");
    let base = state
        .git_engine
        .committed_default_branch()
        .map_err(|_| changed())?;
    if format!("refs/heads/{base}") != source
        || state
            .git_engine
            .resolve_checks_ref(source)
            .map_err(|_| changed())?
            != *expected
    {
        return Err(changed());
    }
    Ok(base)
}
pub(crate) async fn run(
    State(state): State<AppState>,
    Path(raw): Path<String>,
    request: Request,
) -> Response {
    match value(state, raw, request).await {
        Ok(response) => response,
        Err(error) => error.response(),
    }
}
async fn value(state: AppState, raw: String, request: Request) -> Result<Response, Failure> {
    let repo_id = crate::repository_reads::id(&raw)
        .map_err(|_| Failure::new(StatusCode::BAD_REQUEST, "invalid_local_checks_request"))?;
    let (parts, body) = request.into_parts();
    let origin = extract_principal_async(&parts.headers, &state.auth_store)
        .await
        .map_err(|e| auth_error(e, false))?;
    if origin.role != EnterpriseRole::SiteAdmin {
        return Err(forbidden());
    }
    if repo_id != state.repo_id {
        return Err(Failure::new(
            StatusCode::NOT_FOUND,
            "local_checks_not_hosted",
        ));
    }
    let bytes = axum::body::to_bytes(body, 256 * 1024).await.map_err(|_| {
        Failure::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "local_checks_request_too_large",
        )
    })?;
    let input: Input = serde_json::from_slice(&bytes)
        .map_err(|_| Failure::new(StatusCode::BAD_REQUEST, "invalid_local_checks_request"))?;
    if !matches!(input.expected_commit.len(), 40 | 64)
        || !input.expected_commit.bytes().all(|b| b.is_ascii_hexdigit())
        || !input.expected_commit.bytes().any(|b| b != b'0')
        || input.timeout_secs.is_some_and(|t| !(1..=600).contains(&t))
    {
        return Err(Failure::new(
            StatusCode::BAD_REQUEST,
            "invalid_local_checks_request",
        ));
    }
    let expected = CommitSha(input.expected_commit.to_ascii_lowercase());
    let operation = state
        .local_checks_lock
        .clone()
        .try_lock_owned()
        .map(Arc::new)
        .map_err(|_| Failure::new(StatusCode::CONFLICT, "local_checks_busy"))?;
    let snapshot = state.clone();
    let headers = parts.headers.clone();
    let origin_id = origin.user_id;
    let source = input.expected_source_ref.clone();
    let commit = expected.clone();
    let held = operation.clone();
    let (owner, base, checks) = tokio::task::spawn_blocking(move || {
        let _operation = held;
        let _inventory = snapshot.repository_registry.clone();
        authorize(&snapshot, &headers, origin_id)?;
        let owner = snapshot
            .git_engine
            .lock_worktree_service()
            .map_err(|_| Failure::unavailable())?;
        snapshot
            .git_engine
            .ensure_config_has_no_credentials()
            .map_err(|_| Failure::unavailable())?;
        let base = selected(&snapshot, &source, &commit)?;
        let checks = snapshot
            .git_engine
            .required_checks_at_commit(&commit)
            .map_err(|_| Failure::new(StatusCode::BAD_REQUEST, "local_checks_policy_invalid"))?
            .ok_or_else(|| Failure::new(StatusCode::BAD_REQUEST, "local_checks_policy_missing"))?;
        Ok::<_, Failure>((owner, base, checks))
    })
    .await
    .map_err(|_| Failure::unavailable())??;
    let denied = Arc::new(Mutex::new(None));
    let gate_denied = denied.clone();
    let snapshot = state.clone();
    let headers = parts.headers.clone();
    let held = operation.clone();
    let gate_owner = owner.clone();
    let source = input.expected_source_ref.clone();
    let commit = expected.clone();
    let admission = Box::pin(async move {
        let check = tokio::task::spawn_blocking(move || {
            let _operation = held;
            let _owner = gate_owner;
            authorize(&snapshot, &headers, origin_id)?;
            selected(&snapshot, &source, &commit).map(|_| ())
        })
        .await;
        let error = match check {
            Ok(Ok(())) => return Ok(()),
            Ok(Err(error)) => error,
            Err(_) => Failure::unavailable(),
        };
        // fail-open-ok: poison recovery stores only a denial; commands remain forbidden.
        *gate_denied.lock().unwrap_or_else(|e| e.into_inner()) = Some(error);
        Err(SandboxError::AdmissionDenied)
    });
    let executor = match input.timeout_secs {
        Some(t) => state.sandbox_executor.clone().with_timeout_secs(t),
        None => state.sandbox_executor.clone(),
    };
    let outcome = run_prepared_sandbox(
        state.git_engine.clone(),
        owner.clone(),
        base,
        RunCommands::Checks {
            expected: expected.clone(),
            commands: checks.commands,
        },
        false,
        RunContext {
            repo_id,
            executor,
            auth_store: Some(state.auth_store.clone()),
            admission: Some(admission),
            operation: Some(operation.clone()),
        },
        std::future::pending(),
    )
    .await;
    let mut report = Report::unconfirmed(repo_id);
    let mut status = StatusCode::SERVICE_UNAVAILABLE;
    match outcome {
        Ok(outcome)
            if outcome.cleanup_confirmed && outcome.fence_confirmed && !outcome.interrupted =>
        {
            if let Some(result) = outcome.result {
                let row = LocalChecksRecord::new(
                    input.expected_source_ref,
                    expected,
                    checks.blob_oid,
                    result,
                )
                .map_err(|_| Failure::unavailable())?;
                report.status = if row.result.exit_code == 0 {
                    "passed"
                } else {
                    "failed"
                };
                report.cleanup = "confirmed";
                report.reason = None;
                report.history = save(
                    state.git_engine.clone(),
                    owner.clone(),
                    operation.clone(),
                    repo_id,
                    row.clone(),
                )
                .await;
                if report.history.status == "saved" {
                    status = StatusCode::OK;
                } else {
                    report.reason = Some("history_unconfirmed");
                }
                report.record = Some(row);
            }
        }
        Err(RunError::AdmissionDenied) => {
            // fail-open-ok: poison recovery cannot turn denied admission into an execution.
            let failure = denied
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take()
                .unwrap_or_else(Failure::unavailable);
            return Err(failure);
        }
        _ => {}
    }
    // This includes output-bearing storage-failed responses; user activity alone is insufficient.
    let snapshot = state;
    let headers = parts.headers;
    tokio::task::spawn_blocking(move || {
        let _owner = owner;
        let _operation = operation;
        let response = report.response(status);
        authorize(&snapshot, &headers, origin_id)?;
        Ok(response)
    })
    .await
    .map_err(|_| Failure::unavailable())?
}
