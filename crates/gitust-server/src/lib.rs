//! `gitust-server`: High-performance Axum HTTP/REST, Invite-Only Auth, OAuth 2.0 + PKCE (`S256`)
//! Streamable HTTP & Stdio MCP Gateway, and Real-Time Sync Server for Gitust.

#[cfg(test)]
use std::path::PathBuf;

use axum::routing::{delete, get, post};
use axum::{Json, Router};
#[cfg(test)]
use gitust_db::AuthStore;
use serde::Serialize;
use tower_http::cors::CorsLayer;

pub mod auth;
mod automatic_checks;
mod automatic_run_api;
mod automatic_run_model;
mod automatic_run_page;
mod automatic_run_recovery;
mod automatic_run_store;
mod branch_protection;
pub mod enterprise_security;
mod ephemeral_checkout;
pub mod ephemeral_sandbox;
pub mod git_http;
mod git_http_auth;
mod git_http_automatic;
mod git_http_read;
mod git_http_write;
pub mod listener;
mod local_checks_automatic;
mod local_checks_automatic_model;
mod local_checks_automatic_store;
mod local_checks_automatic_worker;
pub mod local_checks_history;
pub mod mcp;
pub mod oauth;
mod outbound;
mod pull_requests;
mod pull_requests_merge;
pub(crate) mod quota;
pub mod repo;
mod repository_admin;
mod repository_default_branch;
mod repository_group_invites;
mod repository_group_members;
mod repository_local_checks;
mod repository_local_checks_automatic;
mod repository_local_checks_exec;
mod repository_local_checks_result;
mod repository_reads;
mod repository_registry;
mod repository_rename;
pub mod repository_search;
mod repository_summary;
mod repository_visibility;
mod run_history;
pub mod runners;
mod sandbox_policy;
mod source_of_truth_sync;
mod ssh_git;
mod ssh_git_host_key;
mod ssh_git_session;
mod ssh_git_stream;
mod ssh_git_target;
mod ssh_git_write;
mod startup;
pub use startup::*;
pub mod state;
mod state_errors;
mod state_oauth;
mod transports;
pub mod webui;
pub(crate) mod workflow_cache_artifacts;
mod workflow_exec;
mod workflows;
mod worktree_owners;
pub mod worktrees;

pub use auth::{
    CreateInviteRequest, CreatePatRequest, CreateTeamRepoRequest, LoginRequest,
    RedeemInviteRequest, RedeemPasswordResetRequest, RegisterSshKeyRequest,
};
pub use mcp::{
    handle_mcp_json_line, handle_mcp_json_line_with_options, serve_mcp_stdio, serve_mcp_stream,
};
pub use oauth::{
    base64url_encode_sha256, pkce_s256_challenge, OAuthAuthorizationServerMetadata,
    OAuthAuthorizeRequest, OAuthAuthorizeResponse, OAuthClientRegistrationRequest,
    OAuthClientRegistrationResponse, OAuthProtectedResourceMetadata, OAuthTokenRequest,
    OAuthTokenResponse,
};
pub use repo::{RepoBlobQuery, RepoCommitsQuery, RepoDiffQuery, RepoTreeQuery};
pub use state::{
    AppState, OAuthMcpStore, PendingOAuthCode, RegisteredOAuthClient, SpawnQuotaTracker,
    WorktreeRuntime, MAX_MCP_LINE_BYTES, MAX_OAUTH_CLIENTS, MAX_OAUTH_CODES,
    MAX_PROMPT_SUMMARY_BYTES, MAX_SANDBOX_COMMAND_BYTES, MAX_TRACKED_WORKTREES,
    MAX_WORKTREES_PER_USER, OAUTH_CODE_TTL_SECS,
};
pub use worktrees::{SandboxExecRequest, SpawnWorktreeRequest};

#[cfg(test)]
#[allow(unused_imports)]
use axum::http::{header, HeaderMap, StatusCode};
#[cfg(test)]
#[allow(unused_imports)]
use gitust_core::{
    AgentKind, AgentWorktree, AuthError, AuthenticatedPrincipal, EnterpriseRole,
    EnterpriseSecurityPolicy, InviteId, InvitePublicView, PatId, PersonalAccessTokenView, RepoId,
    RepositoryRecord, SshKeyId, SshPublicKeyView, TeamId, UserId, UserPublicProfile, WorktreeId,
    WorktreeStatus,
};
#[cfg(test)]
#[allow(unused_imports)]
use gitust_db::{IssuedInviteLink, IssuedPersonalAccessToken, LoginSessionResponse};
#[cfg(test)]
#[allow(unused_imports)]
use gitust_git::{GitEngine, GitEngineError, RepoHeadSnapshot};
#[cfg(test)]
#[allow(unused_imports)]
use gitust_sandbox::{
    builtin_mcp_tools, LocalSandboxExecutor, McpToolDescriptor, SandboxError,
    SandboxExecutionResult, SandboxSpec,
};
#[cfg(test)]
#[allow(unused_imports)]
use gitust_sync::{BiDirectionalSyncConfig, SyncStatusReport};
#[cfg(test)]
#[allow(unused_imports)]
use std::collections::{HashMap, VecDeque};
#[cfg(test)]
#[allow(unused_imports)]
use std::sync::Arc;
#[cfg(test)]
#[allow(unused_imports)]
use uuid::Uuid;

#[derive(Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
    pub engine: &'static str,
    pub privacy_mode: &'static str,
}

async fn health_handler() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        engine: "gitust-pure-rust (gix + local-os-sandbox)",
        privacy_mode: "STRICT_ON_PREMISE_ZERO_CLOUD_LEAKAGE",
    })
}

pub fn build_router(state: AppState) -> Router {
    Router::new()
        // Embedded Single-Binary Web UI & Invite/Reset SPA Routes
        .route("/", get(webui::web_index_handler))
        .route("/auth/accept-invite", get(webui::web_index_handler))
        .route("/auth/reset-password", get(webui::web_index_handler))
        .route("/assets/{*path}", get(webui::web_asset_handler))
        .route("/api/v1/health", get(health_handler))
        .route("/api/v1/git/head", get(worktrees::git_head_handler))
        .route("/api/v1/repo", get(repository_summary::handler))
        // Step 2-4: Blazing-Fast (<30ms) In-Process gix Code, Commit, Tree & Diff Browsing
        .route("/api/v1/repo/commits", get(repo::repo_commits_handler))
        .route("/api/v1/repo/tree", get(repo::repo_tree_handler))
        .route("/api/v1/repo/blob", get(repo::repo_blob_handler))
        .route("/api/v1/repo/diff", get(repo::repo_diff_handler))
        // OAuth 2.0 Discovery, Dynamic Client Registration & PKCE (S256) for HTTP MCP
        .route(
            "/.well-known/oauth-protected-resource",
            get(oauth::oauth_protected_resource_metadata_handler),
        )
        .route(
            "/.well-known/oauth-authorization-server",
            get(oauth::oauth_authorization_server_metadata_handler),
        )
        .route(
            "/oauth/register",
            post(oauth::oauth_register_client_handler),
        )
        .route("/oauth/authorize", post(oauth::oauth_authorize_handler))
        .route("/oauth/token", post(oauth::oauth_token_handler))
        // Streamable HTTP MCP JSON-RPC 2.0 Endpoints (OAuth 2.0 Bearer / PAT protected)
        .route("/mcp", post(mcp::mcp_http_post_handler))
        .route("/api/v1/mcp", post(mcp::mcp_http_post_handler))
        // Auth & Invite-Only Onboarding Routes (No public signup / no web admin bootstrap!)
        .route(
            "/api/v1/auth/register",
            post(auth::public_register_disabled_handler),
        )
        .route("/api/v1/auth/login", post(auth::login_handler))
        .route("/api/v1/auth/logout", post(auth::logout_handler))
        .route("/api/v1/auth/me", get(auth::me_handler))
        .route(
            "/api/v1/auth/invites/redeem",
            post(auth::redeem_invite_handler),
        )
        .route(
            "/api/v1/auth/password-reset/redeem",
            post(auth::redeem_password_reset_handler),
        )
        .route(
            "/api/v1/auth/pats",
            get(auth::list_pats_handler).post(auth::create_pat_handler),
        )
        .route("/api/v1/auth/pats/{id}", delete(auth::revoke_pat_handler))
        .route(
            "/api/v1/auth/ssh-keys",
            get(auth::list_ssh_keys_handler).post(auth::register_ssh_key_handler),
        )
        .route(
            "/api/v1/auth/ssh-keys/{id}",
            delete(auth::revoke_ssh_key_handler),
        )
        .route(
            "/api/v1/auth/totp/status",
            get(enterprise_security::totp_status_handler),
        )
        .route(
            "/api/v1/auth/totp/enroll",
            post(enterprise_security::totp_enroll_handler),
        )
        .route(
            "/api/v1/auth/totp/verify",
            post(enterprise_security::totp_verify_handler),
        )
        .route(
            "/api/v1/auth/totp",
            delete(enterprise_security::totp_disable_handler),
        )
        .route(
            "/api/v1/auth/sso/login",
            post(enterprise_security::sso_login_handler),
        )
        // SiteAdmin Routes (BFLA protected)
        .route(
            "/api/v1/admin/sso/providers",
            get(enterprise_security::list_sso_providers_handler)
                .post(enterprise_security::register_sso_provider_handler),
        )
        .route(
            "/api/v1/admin/sso/providers/{id}",
            delete(enterprise_security::delete_sso_provider_handler),
        )
        .route(
            "/api/v1/admin/audit-log",
            get(enterprise_security::audit_log_handler),
        )
        .route(
            "/api/v1/admin/invites",
            get(auth::list_invites_handler).post(auth::create_invite_handler),
        )
        .route(
            "/api/v1/admin/invites/{id}",
            delete(auth::revoke_invite_handler),
        )
        .route("/api/v1/admin/users", get(auth::list_users_handler))
        .route(
            "/api/v1/admin/users/{id}/deactivate",
            post(auth::deactivate_user_handler),
        )
        .route(
            "/api/v1/admin/users/{id}/password-reset",
            post(auth::create_password_reset_handler),
        )
        .route(
            "/api/v1/admin/repos",
            post(repository_admin::create_hosted_repo),
        )
        // Team Repository BOLA Routes
        .route(
            "/api/v1/repos",
            get(repository_reads::catalog).post(auth::create_team_repo_handler),
        )
        .route(
            "/api/v1/repos/{id}/settings",
            get(repository_admin::get_settings).patch(repository_admin::update_settings),
        )
        .route(
            "/api/v1/repos/{id}/branches",
            get(repository_reads::branches),
        )
        .route(
            "/api/v1/repos/{id}/default-branch",
            axum::routing::patch(repository_default_branch::change),
        )
        .route(
            "/api/v1/repos/{id}/group-members",
            get(repository_group_members::lookup),
        )
        .route(
            "/api/v1/repos/{id}/group-members/{user_id}",
            axum::routing::patch(repository_group_members::change),
        )
        .route(
            "/api/v1/repos/{id}/group-invites",
            axum::routing::post(repository_group_invites::create),
        )
        .route(
            "/api/v1/repos/{id}/visibility",
            axum::routing::patch(repository_visibility::change),
        )
        .route(
            "/api/v1/repos/{id}/local-check-automatic-runs",
            get(repository_local_checks_automatic::list),
        )
        .route(
            "/api/v1/repos/{id}/local-check-runs",
            get(repository_local_checks::list).post(repository_local_checks_exec::run),
        )
        .route(
            "/api/v1/repos/{id}/pulls",
            get(pull_requests::list).post(pull_requests::create),
        )
        .route(
            "/api/v1/repos/{id}/pulls/{pr_id}",
            get(pull_requests::get_one),
        )
        .route(
            "/api/v1/repos/{id}/pulls/{pr_id}/reviews",
            post(pull_requests::add_review),
        )
        .route(
            "/api/v1/repos/{id}/pulls/{pr_id}/comments",
            post(pull_requests::add_comment),
        )
        .route(
            "/api/v1/repos/{id}/pulls/{pr_id}/status",
            post(pull_requests_merge::update_status),
        )
        .route(
            "/api/v1/repos/{id}/pulls/{pr_id}/merge",
            post(pull_requests::merge),
        )
        .route(
            "/api/v1/repos/{id}/branch-protection",
            get(branch_protection::list).put(branch_protection::update),
        )
        .route(
            "/api/v1/repos/{id}/sync/topology",
            get(source_of_truth_sync::get_topology).put(source_of_truth_sync::update_topology),
        )
        .route(
            "/api/v1/repos/{id}/sync/backup-push",
            post(source_of_truth_sync::backup_push),
        )
        .route(
            "/api/v1/repos/{id}/sync/rollback",
            post(source_of_truth_sync::rollback),
        )
        .route(
            "/api/v1/repos/{id}/workflows",
            get(workflows::list_workflows_and_runs),
        )
        .route(
            "/api/v1/repos/{id}/workflow-runs",
            post(workflow_exec::trigger_workflow_run),
        )
        .route(
            "/api/v1/repos/{id}/workflow-runs/{run_id}/artifacts",
            get(workflows::list_run_artifacts),
        )
        .route(
            "/api/v1/repos/{id}/workflow-secrets",
            get(workflows::list_secrets),
        )
        .route(
            "/api/v1/repos/{id}/workflow-secrets/{name}",
            axum::routing::put(workflows::put_secret).delete(workflows::delete_secret),
        )
        .route(
            "/api/v1/repos/{id}/caches",
            get(runners::list_repo_caches_handler),
        )
        .route(
            "/api/v1/repos/{id}/caches/{key}",
            delete(runners::evict_repo_cache_handler),
        )
        .route(
            "/api/v1/admin/runners",
            get(runners::list_runners_handler).post(runners::register_runner_handler),
        )
        .route(
            "/api/v1/admin/runners/{runner_id}/heartbeat",
            post(runners::heartbeat_runner_handler),
        )
        .route(
            "/api/v1/admin/runners/{runner_id}",
            delete(runners::delete_runner_handler),
        )
        .route("/api/v1/repos/{id}/commits", get(repository_reads::commits))
        .route("/api/v1/repos/{id}/tree", get(repository_reads::tree))
        .route("/api/v1/repos/{id}/blob", get(repository_reads::blob))
        .route("/api/v1/repos/{id}/diff", get(repository_reads::diff))
        .route(
            "/api/v1/repos/{id}/search",
            get(repository_search::search_repository_handler),
        )
        .route(
            "/api/v1/repos/{id}",
            get(auth::get_team_repo_handler).patch(repository_rename::rename),
        )
        .route("/api/v1/repo/automatic-runs", get(automatic_run_page::list))
        // Agent Swarm, Sync & MCP Routes (Authenticated & Team/Role BOLA protected)
        .route(
            "/api/v1/swarm/worktrees",
            get(worktrees::list_worktrees_handler).post(worktrees::spawn_worktree_handler),
        )
        .route(
            "/api/v1/swarm/worktrees/{id}",
            delete(worktrees::discard_worktree_handler),
        )
        .route(
            "/api/v1/swarm/worktrees/{id}/sandbox",
            post(worktrees::exec_worktree_sandbox_handler),
        )
        .route(
            "/api/v1/swarm/worktrees/{id}/runs",
            get(run_history::list_handler),
        )
        .route(
            "/api/v1/swarm/worktrees/{id}/automatic-runs",
            get(automatic_run_api::list),
        )
        .route(
            "/api/v1/swarm/worktrees/{id}/approve",
            post(worktrees::approve_worktree_handler),
        )
        .route("/api/v1/sync/github", post(repo::sync_github_handler))
        .route(
            "/api/v1/swarm/worktrees/{id}/publish",
            post(outbound::publish_worktree_handler),
        )
        .route("/api/v1/mcp/tools", get(mcp::mcp_tools_handler))
        .route(
            "/api/v1/enterprise/policy",
            get(auth::enterprise_policy_handler),
        )
        // Phase 3 Step 3-1: Pure-Rust Smart HTTP Git Hosting (PAT / Bearer / Basic + Team BOLA + Agent Branch Policy)
        .route(
            "/git/{repo}/info/refs",
            get(git_http::git_info_refs_handler),
        )
        .route(
            "/git/{repo}/git-upload-pack",
            post(git_http::git_upload_pack_handler),
        )
        .route(
            "/git/{repo}/git-receive-pack",
            post(git_http::git_receive_pack_handler),
        )
        .layer(axum::middleware::map_request(listener::apply_body_deadline))
        .layer(CorsLayer::permissive())
        .with_state(state)
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_listener;
#[cfg(all(test, unix))]
mod tests_ssh_git_push;
#[cfg(test)]
mod tests_ssh_git_target;
