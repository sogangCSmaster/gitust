use super::*;
use crate::state::{set_worktree_status, SpawnLifecycle, SpawnReservation};
use axum::body::Body;
use axum::http::Request;
use http_body_util::BodyExt;
use tower::ServiceExt;

#[path = "tests_invite_credentials.rs"]
mod tests_invite_credentials;
#[path = "tests_repository_group_invite_auth.rs"]
mod tests_repository_group_invite_auth;
#[path = "tests_repository_group_invites.rs"]
mod tests_repository_group_invites;

#[path = "tests_approval_target.rs"]
mod tests_approval_target;
#[path = "tests_auth.rs"]
mod tests_auth;
#[path = "tests_automatic_http_revocation.rs"]
mod tests_automatic_http_revocation;
#[path = "tests_automatic_store.rs"]
mod tests_automatic_store;
#[path = "tests_check_steps.rs"]
mod tests_check_steps;
#[path = "tests_external_review.rs"]
mod tests_external_review;
#[path = "tests_git_http.rs"]
mod tests_git_http;
#[path = "tests_mcp.rs"]
mod tests_mcp;
#[path = "tests_mcp_commit.rs"]
mod tests_mcp_commit;
#[path = "tests_mcp_commit_protected.rs"]
mod tests_mcp_commit_protected;
#[path = "tests_mcp_commit_stdio.rs"]
mod tests_mcp_commit_stdio;
#[path = "tests_mcp_discard.rs"]
mod tests_mcp_discard;
#[path = "tests_oauth.rs"]
mod tests_oauth;
#[path = "tests_outbound.rs"]
mod tests_outbound;
#[path = "tests_repository_default.rs"]
mod tests_repository_default;
#[path = "tests_repository_summary.rs"]
mod tests_repository_summary;
#[path = "tests_repository_unborn.rs"]
mod tests_repository_unborn;
#[path = "tests_required_checks.rs"]
mod tests_required_checks;
#[path = "tests_run_history.rs"]
mod tests_run_history;
#[path = "tests_verification.rs"]
mod tests_verification;
#[path = "tests_worktree_labels.rs"]
mod tests_worktree_labels;

fn test_app_at(repo: PathBuf, trusted_sandbox_exec: bool) -> (Router, String) {
    let auth_store = AuthStore::new_in_memory();
    auth_store
        .bootstrap_initial_admin_cli(
            "siteadmin",
            "siteadmin@company.internal",
            "AdminPassword!2026",
        )
        .expect("bootstrap initial admin");
    let login = auth_store
        .login("siteadmin", "AdminPassword!2026")
        .expect("login initial admin");

    let state = AppState::with_auth_store(
        repo,
        "https://github.com/sogangCSmaster/gitust.git".to_owned(),
        auth_store,
    )
    .expect("recover server state")
    .with_trusted_sandbox_exec(trusted_sandbox_exec);
    (build_router(state), login.raw_session_token)
}

fn test_app() -> (Router, String) {
    // Deliberately not a repository: these tests must never touch the developer's checkout.
    test_app_at(PathBuf::from("/nonexistent/gitust-test-repo"), false)
}

/// App with one SiteAdmin and two Developers; returns (router, admin, dev_a, dev_b) tokens.
fn test_app_with_devs(repo: PathBuf) -> (Router, String, String, String) {
    let store = AuthStore::new_in_memory();
    store
        .bootstrap_initial_admin_cli(
            "siteadmin",
            "siteadmin@company.internal",
            "AdminPassword!2026",
        )
        .expect("bootstrap");
    let admin = store
        .login("siteadmin", "AdminPassword!2026")
        .expect("login");
    let admin_principal = store
        .authenticate_session(&admin.raw_session_token)
        .expect("principal");
    let mut dev_tokens = Vec::new();
    for name in ["deva", "devb"] {
        let invite = store
            .create_invite_link(
                &admin_principal,
                &format!("{name}@company.internal"),
                EnterpriseRole::Developer,
                vec![],
                None,
            )
            .expect("invite");
        store
            .redeem_invite_link(&invite.raw_token, name, "DevPassword!2026")
            .expect("redeem");
        dev_tokens.push(
            store
                .login(name, "DevPassword!2026")
                .expect("login")
                .raw_session_token,
        );
    }
    let state = AppState::with_auth_store(
        repo,
        "https://github.com/sogangCSmaster/gitust.git".to_owned(),
        store,
    )
    .expect("recover server state");
    let dev_b = dev_tokens.pop().expect("dev b");
    let dev_a = dev_tokens.pop().expect("dev a");
    (build_router(state), admin.raw_session_token, dev_a, dev_b)
}

pub(crate) fn fixture_git() -> std::process::Command {
    let mut command = std::process::Command::new("git");
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("GIT_") {
            command.env_remove(key);
        }
    }
    command.env("GIT_CONFIG_NOSYSTEM", "1");
    command
}

fn init_temp_repo(dir: &std::path::Path) {
    let git = |args: &[&str]| {
        let out = fixture_git()
            .arg("-C")
            .arg(dir)
            .args(["-c", "commit.gpgsign=false"])
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_AUTHOR_NAME", "T")
            .env("GIT_AUTHOR_EMAIL", "t@company.internal")
            .env("GIT_COMMITTER_NAME", "T")
            .env("GIT_COMMITTER_EMAIL", "t@company.internal")
            .env("GIT_AUTHOR_DATE", "1700000000 +0000")
            .env("GIT_COMMITTER_DATE", "1700000000 +0000")
            .output()
            .expect("git");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    std::fs::create_dir_all(dir).expect("mkdir");
    git(&["init", "-q", "-b", "main"]);
    std::fs::write(dir.join("README.md"), "hello\n").expect("write");
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "init"]);
}

pub(super) async fn call(
    app: &Router,
    method: &str,
    uri: String,
    token: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, String) {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"));
    let body = match body {
        Some(v) => {
            req = req.header(header::CONTENT_TYPE, "application/json");
            Body::from(v.to_string())
        }
        None => Body::empty(),
    };
    let res = app
        .clone()
        .oneshot(req.body(body).expect("req"))
        .await
        .expect("res");
    let status = res.status();
    let bytes = res.into_body().collect().await.expect("body").to_bytes();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

#[path = "tests_worktree_lifecycle.rs"]
mod tests_worktree_lifecycle;
use tests_worktree_lifecycle::*;
#[path = "tests_automatic_checks.rs"]
mod tests_automatic_checks;
#[path = "tests_automatic_git_http.rs"]
mod tests_automatic_git_http;
#[path = "tests_webui_assets.rs"]
mod webui_assets;

#[path = "tests_automatic_intents.rs"]
mod tests_automatic_intents;
#[path = "tests_automatic_restart.rs"]
mod tests_automatic_restart;

#[path = "tests_repository_automatic_runs.rs"]
mod tests_repository_automatic_runs;
#[path = "tests_repository_registration.rs"]
mod tests_repository_registration;

#[path = "tests_repository_catalog.rs"]
mod tests_repository_catalog;

#[path = "tests_repository_local_checks.rs"]
mod tests_repository_local_checks;

#[path = "tests_repository_rename.rs"]
mod tests_repository_rename;

#[path = "tests_repository_read_credentials.rs"]
mod tests_repository_read_credentials;

#[path = "tests_sync_credentials.rs"]
mod tests_sync_credentials;

#[path = "tests_repository_branches.rs"]
mod tests_repository_branches;

#[path = "tests_repository_visibility.rs"]
mod tests_repository_visibility;
#[path = "tests_repository_visibility_auth.rs"]
mod tests_repository_visibility_auth;

#[path = "tests_git_http_secondary.rs"]
mod tests_git_http_secondary;
#[path = "tests_git_http_secondary_auth.rs"]
mod tests_git_http_secondary_auth;

#[path = "tests_credential_mutations.rs"]
mod tests_credential_mutations;

#[path = "tests_repository_group_members.rs"]
mod tests_repository_group_members;

#[path = "tests_repository_group_members_auth.rs"]
mod tests_repository_group_members_auth;

#[path = "tests_git_http_write.rs"]
mod tests_git_http_write;
#[path = "tests_git_http_write_auth.rs"]
mod tests_git_http_write_auth;

#[path = "tests_repository_local_checks_exec.rs"]
mod tests_repository_local_checks_exec;

#[path = "tests_local_checks_automatic.rs"]
mod tests_local_checks_automatic;
#[path = "tests_ordinary_branch_checks.rs"]
mod tests_ordinary_branch_checks;
#[path = "tests_pull_requests.rs"]
mod tests_pull_requests;
#[path = "tests_pull_requests_regressions.rs"]
mod tests_pull_requests_regressions;
#[path = "tests_repository_admin.rs"]
mod tests_repository_admin;
