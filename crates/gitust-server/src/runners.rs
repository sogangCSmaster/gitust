//! Multi-node runner fleet registration, heartbeat, lease scheduling (`RunnerFleetRegistry`),
//! and shared build cache inspection/eviction endpoints.

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use axum::{
    extract::{Path as AxumPath, Request, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use chrono::Utc;
use gitust_core::{
    AuthError, EnterpriseRole, IsolationTier, RepoId, RunnerNodeId, RunnerNodeRecord, RunnerStatus,
};
use parking_lot::RwLock;
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use crate::{
    auth::{extract_principal, extract_principal_async},
    repository_reads::{id as parse_repo_id, response},
    state::{check_repo_bola, internal_err, map_auth_err, AppState},
    workflow_cache_artifacts::{evict_shared_cache, list_shared_caches},
    workflows::{bad_req, unavailable, ApiError},
};

const MAX_RUNNER_NODES: usize = 64;
const MAX_RUNNER_BODY_BYTES: usize = 16 * 1024;
const BUILTIN_LOCAL_UUID: u128 = 1;

fn not_found(msg: impl Into<String>) -> ApiError {
    (StatusCode::NOT_FOUND, msg.into())
}

fn conflict(msg: impl Into<String>) -> ApiError {
    (StatusCode::CONFLICT, msg.into())
}

fn is_valid_runner_ident(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RegisterRunnerInput {
    pub(crate) name: String,
    pub(crate) labels: Vec<String>,
    #[serde(default)]
    pub(crate) supported_tiers: Vec<IsolationTier>,
    #[serde(default)]
    pub(crate) max_concurrency: Option<u32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct HeartbeatRunnerInput {
    #[serde(default)]
    pub(crate) status: Option<RunnerStatus>,
}

pub struct RunnerFleetRegistry {
    nodes: RwLock<BTreeMap<RunnerNodeId, RunnerNodeRecord>>,
}

impl Default for RunnerFleetRegistry {
    fn default() -> Self {
        Self::new_with_builtin_local()
    }
}

pub(crate) struct RunnerLeaseGuard {
    fleet: Arc<RunnerFleetRegistry>,
    pub(crate) runner_id: RunnerNodeId,
    pub(crate) runner_name: String,
}

impl Drop for RunnerLeaseGuard {
    fn drop(&mut self) {
        let mut guard = self.fleet.nodes.write();
        if let Some(node) = guard.get_mut(&self.runner_id) {
            node.active_leases = node.active_leases.saturating_sub(1);
        }
    }
}

impl RunnerFleetRegistry {
    #[must_use]
    pub fn new_with_builtin_local() -> Self {
        let now = Utc::now().to_rfc3339();
        let id = RunnerNodeId(Uuid::from_u128(BUILTIN_LOCAL_UUID));
        let local = RunnerNodeRecord {
            id,
            name: "built-in-local".to_owned(),
            labels: vec!["local".to_owned()],
            supported_tiers: vec![IsolationTier::HostOs],
            max_concurrency: 4,
            active_leases: 0,
            status: RunnerStatus::Online,
            registered_at: now.clone(),
            last_heartbeat_at: now,
        };
        let mut map = BTreeMap::new();
        map.insert(id, local);
        Self {
            nodes: RwLock::new(map),
        }
    }

    pub fn list_runners(&self) -> Vec<RunnerNodeRecord> {
        self.nodes.read().values().cloned().collect()
    }

    pub(crate) fn register_runner(
        &self,
        input: RegisterRunnerInput,
    ) -> Result<RunnerNodeRecord, ApiError> {
        let name = input.name.trim().to_owned();
        if !is_valid_runner_ident(&name) {
            return Err(bad_req("runner name must be 1..=64 [A-Za-z0-9_-] chars"));
        }
        if input.labels.is_empty() || input.labels.len() > 16 {
            return Err(bad_req("runner must declare 1..=16 labels"));
        }
        let mut labels_set = BTreeSet::new();
        for raw_label in input.labels {
            let clean = raw_label.trim().to_owned();
            if !is_valid_runner_ident(&clean) {
                return Err(bad_req(format!("invalid runner label '{clean}'")));
            }
            labels_set.insert(clean);
        }
        let tiers: Vec<IsolationTier> = if input.supported_tiers.is_empty() {
            vec![IsolationTier::HostOs]
        } else {
            let set: BTreeSet<IsolationTier> = input.supported_tiers.into_iter().collect();
            set.into_iter().collect()
        };
        // fail-open-ok: optional runner max_concurrency defaults to 4 when omitted.
        let max_concurrency = input.max_concurrency.unwrap_or(4);
        if !(1..=32).contains(&max_concurrency) {
            return Err(bad_req("max_concurrency must be 1..=32"));
        }

        let mut guard = self.nodes.write();
        if guard.len() >= MAX_RUNNER_NODES {
            return Err(conflict("runner fleet limit reached"));
        }
        if guard.values().any(|n| n.name == name) {
            return Err(conflict(format!("runner '{name}' is already registered")));
        }
        let now = Utc::now().to_rfc3339();
        let id = RunnerNodeId::new();
        let record = RunnerNodeRecord {
            id,
            name,
            labels: labels_set.into_iter().collect(),
            supported_tiers: tiers,
            max_concurrency,
            active_leases: 0,
            status: RunnerStatus::Online,
            registered_at: now.clone(),
            last_heartbeat_at: now,
        };
        guard.insert(id, record.clone());
        Ok(record)
    }

    pub(crate) fn heartbeat(
        &self,
        id: RunnerNodeId,
        status: Option<RunnerStatus>,
    ) -> Result<RunnerNodeRecord, ApiError> {
        let mut guard = self.nodes.write();
        let node = guard
            .get_mut(&id)
            .ok_or_else(|| not_found("runner node not found"))?;
        if let Some(st) = status {
            node.status = st;
        }
        node.last_heartbeat_at = Utc::now().to_rfc3339();
        Ok(node.clone())
    }

    pub(crate) fn deregister(&self, id: RunnerNodeId) -> Result<(), ApiError> {
        if id.0.as_u128() == BUILTIN_LOCAL_UUID {
            return Err(conflict("built-in-local runner cannot be deregistered"));
        }
        let mut guard = self.nodes.write();
        let node = guard
            .get(&id)
            .ok_or_else(|| not_found("runner node not found"))?;
        if node.active_leases > 0 {
            return Err(conflict("runner has active workflow leases"));
        }
        guard.remove(&id);
        Ok(())
    }

    pub(crate) fn acquire_lease(
        self: &Arc<Self>,
        required_label: &str,
        required_tier: IsolationTier,
    ) -> Result<RunnerLeaseGuard, ApiError> {
        let mut guard = self.nodes.write();
        let mut matching_online = false;
        let mut selected_id: Option<(RunnerNodeId, u32)> = None;
        for (id, node) in guard.iter() {
            if node.status != RunnerStatus::Online {
                continue;
            }
            if !node.labels.iter().any(|l| l == required_label)
                || !node.supported_tiers.contains(&required_tier)
            {
                continue;
            }
            matching_online = true;
            if node.active_leases < node.max_concurrency {
                match selected_id {
                    None => selected_id = Some((*id, node.active_leases)),
                    Some((_, best_leases)) if node.active_leases < best_leases => {
                        selected_id = Some((*id, node.active_leases));
                    }
                    Some(_) => {}
                }
            }
        }
        if !matching_online {
            return Err(conflict(format!(
                "no online runner available for label '{required_label}' and isolation tier '{required_tier}'"
            )));
        }
        let Some((chosen_id, _)) = selected_id else {
            return Err(conflict(
                "all matching online runners are at max concurrency",
            ));
        };
        let node = guard
            .get_mut(&chosen_id)
            .ok_or_else(|| conflict("runner unavailable"))?;
        node.active_leases = node.active_leases.saturating_add(1);
        Ok(RunnerLeaseGuard {
            fleet: Arc::clone(self),
            runner_id: chosen_id,
            runner_name: node.name.clone(),
        })
    }
}

fn parse_runner_id(raw: &str) -> Result<RunnerNodeId, ApiError> {
    Uuid::parse_str(raw.trim())
        .map(RunnerNodeId)
        .map_err(|_| bad_req("invalid runner UUID"))
}

pub(crate) async fn list_runners_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    let res = tokio::task::spawn_blocking(move || -> Result<_, ApiError> {
        let caller = extract_principal(&headers, &state.auth_store)?;
        if caller.role != EnterpriseRole::SiteAdmin {
            return Err(map_auth_err(AuthError::AdminPrivilegeRequired));
        }
        let runners = state.runner_fleet.list_runners();
        Ok(json!({ "runners": runners }))
    })
    .await
    .map_err(internal_err);
    response(res.and_then(|r| r))
}

pub(crate) async fn register_runner_handler(
    State(state): State<AppState>,
    request: Request,
) -> Response {
    let res: Result<(StatusCode, Json<RunnerNodeRecord>), ApiError> = async move {
        let (parts, body) = request.into_parts();
        let origin = extract_principal_async(&parts.headers, &state.auth_store).await?;
        if origin.role != EnterpriseRole::SiteAdmin {
            return Err(map_auth_err(AuthError::AdminPrivilegeRequired));
        }
        let bytes = axum::body::to_bytes(body, MAX_RUNNER_BODY_BYTES)
            .await
            .map_err(|_| {
                (
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "request body too large".into(),
                )
            })?;
        let input: RegisterRunnerInput =
            serde_json::from_slice(&bytes).map_err(|e| bad_req(format!("invalid JSON: {e}")))?;
        tokio::task::spawn_blocking(move || {
            let caller = extract_principal(&parts.headers, &state.auth_store)?;
            if caller.user_id != origin.user_id || caller.role != EnterpriseRole::SiteAdmin {
                return Err(map_auth_err(AuthError::AdminPrivilegeRequired));
            }
            let record = state.runner_fleet.register_runner(input)?;
            Ok((StatusCode::CREATED, Json(record)))
        })
        .await
        .map_err(internal_err)?
    }
    .await;
    match res {
        Ok(pair) => pair.into_response(),
        Err(err) => response::<()>(Err(err)),
    }
}

pub(crate) async fn heartbeat_runner_handler(
    State(state): State<AppState>,
    AxumPath(raw_runner_id): AxumPath<String>,
    request: Request,
) -> Response {
    let res = async move {
        let (parts, body) = request.into_parts();
        let origin = extract_principal_async(&parts.headers, &state.auth_store).await?;
        if origin.role != EnterpriseRole::SiteAdmin {
            return Err(map_auth_err(AuthError::AdminPrivilegeRequired));
        }
        let runner_id = parse_runner_id(&raw_runner_id)?;
        let bytes = axum::body::to_bytes(body, MAX_RUNNER_BODY_BYTES)
            .await
            .map_err(|_| {
                (
                    StatusCode::PAYLOAD_TOO_LARGE,
                    "request body too large".into(),
                )
            })?;
        let input: HeartbeatRunnerInput = if bytes.is_empty() {
            HeartbeatRunnerInput { status: None }
        } else {
            serde_json::from_slice(&bytes).map_err(|e| bad_req(format!("invalid JSON: {e}")))?
        };
        tokio::task::spawn_blocking(move || {
            let caller = extract_principal(&parts.headers, &state.auth_store)?;
            if caller.user_id != origin.user_id || caller.role != EnterpriseRole::SiteAdmin {
                return Err(map_auth_err(AuthError::AdminPrivilegeRequired));
            }
            state.runner_fleet.heartbeat(runner_id, input.status)
        })
        .await
        .map_err(internal_err)?
    }
    .await;
    response(res)
}

pub(crate) async fn delete_runner_handler(
    State(state): State<AppState>,
    AxumPath(raw_runner_id): AxumPath<String>,
    headers: HeaderMap,
) -> Response {
    let res: Result<StatusCode, ApiError> = async move {
        let origin = extract_principal_async(&headers, &state.auth_store).await?;
        if origin.role != EnterpriseRole::SiteAdmin {
            return Err(map_auth_err(AuthError::AdminPrivilegeRequired));
        }
        let runner_id = parse_runner_id(&raw_runner_id)?;
        tokio::task::spawn_blocking(move || {
            let caller = extract_principal(&headers, &state.auth_store)?;
            if caller.user_id != origin.user_id || caller.role != EnterpriseRole::SiteAdmin {
                return Err(map_auth_err(AuthError::AdminPrivilegeRequired));
            }
            state.runner_fleet.deregister(runner_id)?;
            Ok(StatusCode::NO_CONTENT)
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

pub(crate) async fn list_repo_caches_handler(
    State(state): State<AppState>,
    AxumPath(raw_id): AxumPath<String>,
    headers: HeaderMap,
) -> Response {
    let res = tokio::task::spawn_blocking(move || -> Result<_, ApiError> {
        let _inventory = state.repository_registry.clone();
        let origin = extract_principal(&headers, &state.auth_store)?;
        let repo_id: RepoId = parse_repo_id(&raw_id)?;
        let repo = state
            .read_repository(repo_id)
            .ok_or_else(|| not_found("repository not found"))?;
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
        let caches = list_shared_caches(&common)?;
        let current = extract_principal(&headers, &state.auth_store)?;
        if current.user_id != origin.user_id {
            return Err(map_auth_err(AuthError::InvalidToken));
        }
        check_repo_bola(&state.auth_store, exp_id, repo.access, &current, repo_id)?;
        Ok(json!({ "caches": caches }))
    })
    .await
    .map_err(internal_err);
    response(res.and_then(|r| r))
}

pub(crate) async fn evict_repo_cache_handler(
    State(state): State<AppState>,
    AxumPath((raw_id, key)): AxumPath<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let res: Result<StatusCode, ApiError> = async move {
        let origin = extract_principal_async(&headers, &state.auth_store).await?;
        if origin.role != EnterpriseRole::SiteAdmin {
            return Err(map_auth_err(AuthError::AdminPrivilegeRequired));
        }
        tokio::task::spawn_blocking(move || {
            let _inventory = state.repository_registry.clone();
            let caller = extract_principal(&headers, &state.auth_store)?;
            if caller.user_id != origin.user_id || caller.role != EnterpriseRole::SiteAdmin {
                return Err(map_auth_err(AuthError::AdminPrivilegeRequired));
            }
            let repo_id: RepoId = parse_repo_id(&raw_id)?;
            let repo = state
                .read_repository(repo_id)
                .ok_or_else(|| not_found("repository not found"))?;
            if repo.archived {
                return Err(conflict("repository is archived"));
            }
            if !repo.is_default && !repo.git_write {
                return Err((StatusCode::FORBIDDEN, "repository is read-only".into()));
            }
            let exp_id = if repo.is_default {
                state.repo_id
            } else {
                repo_id
            };
            check_repo_bola(&state.auth_store, exp_id, repo.access, &caller, repo_id)?;
            let _op = repo
                .local_checks_lock
                .clone()
                .try_lock_owned()
                .map_err(|_| conflict("another workflow or local check run is in progress"))?;
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
            let existed = evict_shared_cache(&common, &key)?;
            if !existed {
                return Err(not_found("shared cache key not found"));
            }
            Ok(StatusCode::NO_CONTENT)
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
