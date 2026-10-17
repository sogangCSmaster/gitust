//! Configured repositories can have a valid default branch without its first commit.
use super::*;
use crate::repository_registry::RepositoryRegistry;
use std::path::Path;
use std::process::{Command, Output};

fn git_output(path: &Path, args: &[&str], credential: Option<&str>) -> Output {
    let mut command = Command::new("git");
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("GIT_") {
            command.env_remove(key);
        }
    }
    for key in [
        "http_proxy",
        "https_proxy",
        "all_proxy",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
    ] {
        command.env_remove(key);
    }
    command
        .arg("-C")
        .arg(path)
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "http.lowSpeedLimit=1",
            "-c",
            "http.lowSpeedTime=5",
            "-c",
            "http.proxy=",
            "-c",
            "credential.helper=",
        ])
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_AUTHOR_NAME", "Fixture")
        .env("GIT_COMMITTER_NAME", "Fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@company.internal")
        .env("GIT_COMMITTER_EMAIL", "fixture@company.internal");
    if let Some(token) = credential {
        command
            .env("GIT_CONFIG_COUNT", "1")
            .env("GIT_CONFIG_KEY_0", "http.extraHeader")
            .env(
                "GIT_CONFIG_VALUE_0",
                format!("Authorization: Bearer {token}"),
            );
    }
    command.output().unwrap()
}
fn git(path: &Path, args: &[&str]) -> String {
    let output = git_output(path, args, None);
    assert!(output.status.success(), "synthetic Git operation failed");
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}
fn register_empty(path: &Path, store: &AuthStore, token: &str, bare: bool) -> RepoId {
    std::fs::create_dir(path).unwrap();
    let args = if bare {
        vec!["init", "-q", "--bare", "-b", "main"]
    } else {
        vec!["init", "-q", "-b", "main"]
    };
    git(path, &args);
    let engine = GitEngine::new(path);
    let id = engine.load_or_create_repository_id().unwrap();
    let admin = store.authenticate_session(token).unwrap();
    let row = store
        .register_local_repository(&admin, id, "unborn", TeamId::new(), true, None)
        .unwrap();
    engine
        .complete_repository_registration(row.binding)
        .unwrap();
    id
}
fn fixture(bare: bool) -> (tempfile::TempDir, AppState, String) {
    let temp = tempfile::tempdir().unwrap();
    let store = AuthStore::open_persistent(temp.path().join("auth/auth.json")).unwrap();
    store
        .bootstrap_initial_admin_cli("admin", "admin@company.internal", "Password!2026")
        .unwrap();
    let token = store
        .login("admin", "Password!2026")
        .unwrap()
        .raw_session_token;
    let path = temp.path().join("primary");
    register_empty(&path, &store, &token, bare);
    let mut state =
        AppState::with_auth_store(path, "https://example.invalid/repo.git".into(), store).unwrap();
    state.repository_registry = Some(RepositoryRegistry::load(&state, vec![]).unwrap());
    (temp, state, token)
}
async fn json(app: &Router, route: &str, token: &str) -> (StatusCode, serde_json::Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(route)
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let value = serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
    (status, value)
}

#[tokio::test]
async fn unborn_primary_has_nullable_identity_and_no_fabricated_revision() {
    for bare in [false, true] {
        let (_temp, state, token) = fixture(bare);
        let id = state.repo_id;
        let app = build_router(state.clone());
        for route in ["/api/v1/repo", "/api/v1/repos"] {
            let (status, body) = json(&app, route, &token).await;
            assert_eq!(status, StatusCode::OK);
            let row = if route.ends_with("repos") {
                &body["repositories"][0]
            } else {
                &body
            };
            assert_eq!(row["id"], id.0.to_string());
            assert_eq!(row["branch"], "main");
            assert!(row["head_commit"].is_null());
        }
        for suffix in ["commits", "tree", "blob?path=README.md", "diff"] {
            assert_eq!(
                json(&app, &format!("/api/v1/repos/{}/{suffix}", id.0), &token)
                    .await
                    .0,
                StatusCode::NOT_FOUND
            );
        }
        assert!(state.git_engine.inspect_head_identity().is_err());
        assert!(state.git_engine.inspect_head_zero_copy().is_err());
        assert!(state.worktrees.read().is_empty());
    }
}

#[tokio::test]
async fn registered_unborn_secondary_is_hosted_with_its_existing_identity_and_read_only_scope() {
    let (temp, mut state, token) = fixture(true);
    let secondary = temp.path().join("secondary");
    let id = register_empty(&secondary, &state.auth_store, &token, true);
    state.repository_registry = Some(RepositoryRegistry::load(&state, vec![secondary]).unwrap());
    let admin = state.auth_store.authenticate_session(&token).unwrap();
    let team = state
        .auth_store
        .authorize_repository_access(&admin, id)
        .unwrap()
        .owner_team_id;
    let member = account(
        &state,
        &token,
        "browser",
        EnterpriseRole::Developer,
        vec![team],
    );
    let outsider = account(
        &state,
        &token,
        "outsider",
        EnterpriseRole::Developer,
        vec![],
    );
    let app = build_router(state.clone());
    let (status, body) = json(&app, "/api/v1/repos", &token).await;
    assert_eq!(status, StatusCode::OK);
    let row = body["repositories"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == id.0.to_string())
        .unwrap();
    assert_eq!(row["capability"], "browse");
    assert_eq!(row["is_default"], false);
    assert_eq!(row["can_rename"], true);
    assert_eq!(row["branch"], "main");
    assert!(row["head_commit"].is_null());
    let (_, body) = json(&app, "/api/v1/repos", &member).await;
    assert_eq!(body["repositories"].as_array().unwrap().len(), 1);
    assert_eq!(body["repositories"][0]["id"], id.0.to_string());
    assert_eq!(body["repositories"][0]["can_rename"], false);
    assert_eq!(
        json(&app, "/api/v1/repo", &member).await.0,
        StatusCode::FORBIDDEN
    );
    let (_, body) = json(&app, "/api/v1/repos", &outsider).await;
    assert_eq!(body["repositories"], serde_json::json!([]));
    let route = format!("/api/v1/repos/{}", id.0);
    assert_eq!(
        call(
            &app,
            "PATCH",
            route.clone(),
            &member,
            Some(serde_json::json!({"name":"forbidden"}))
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &app,
            "PATCH",
            route,
            &token,
            Some(serde_json::json!({"name":"unborn-renamed"}))
        )
        .await
        .0,
        StatusCode::OK
    );
}

fn account(
    state: &AppState,
    admin: &str,
    name: &str,
    role: EnterpriseRole,
    teams: Vec<TeamId>,
) -> String {
    let principal = state.auth_store.authenticate_session(admin).unwrap();
    let invite = state
        .auth_store
        .create_invite_link(
            &principal,
            &format!("{name}@company.internal"),
            role,
            teams,
            None,
        )
        .unwrap();
    state
        .auth_store
        .redeem_invite_link(&invite.raw_token, name, "Password!2026")
        .unwrap();
    state
        .auth_store
        .login(name, "Password!2026")
        .unwrap()
        .raw_session_token
}

#[tokio::test]
async fn unborn_metadata_rechecks_current_session_and_pat_after_git_inspection() {
    for use_pat in [false, true] {
        for route in ["/api/v1/repo", "/api/v1/repos"] {
            let (_temp, state, session) = fixture(true);
            let admin = state.auth_store.authenticate_session(&session).unwrap();
            let pat = state
                .auth_store
                .create_personal_access_token(&admin, "read", None)
                .unwrap();
            let token = if use_pat { pat.raw_token } else { session };
            let store = state.auth_store.clone();
            let revoke = token.clone();
            *state.repository_read_hook.lock() = Some(std::sync::Arc::new(move || {
                if use_pat {
                    store
                        .revoke_personal_access_token(&admin, pat.metadata.id)
                        .unwrap();
                } else {
                    store.revoke_session(&admin, &revoke).unwrap();
                }
            }));
            assert_eq!(
                json(&build_router(state), route, &token).await.0,
                StatusCode::UNAUTHORIZED
            );
        }
    }
}

async fn client_git(path: &Path, args: &[&str], token: Option<&str>) -> Output {
    let path = path.to_owned();
    let args: Vec<_> = args.iter().map(|arg| (*arg).to_owned()).collect();
    let token = token.map(str::to_owned);
    tokio::task::spawn_blocking(move || {
        let args: Vec<_> = args.iter().map(String::as_str).collect();
        git_output(&path, &args, token.as_deref())
    })
    .await
    .unwrap()
}
struct ServerTask(tokio::task::JoinHandle<()>);
impl Drop for ServerTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn first_ordinary_push_preserves_authority_identity_and_git_integrity() {
    let (temp, state, session) = fixture(true);
    let repo = state.git_engine.repo_path().to_owned();
    let id = state.repo_id;
    let admin = state.auth_store.authenticate_session(&session).unwrap();
    let registration = state
        .auth_store
        .authorize_repository_access(&admin, id)
        .unwrap();
    let team = registration.owner_team_id;
    let developer = account(
        &state,
        &session,
        "developer",
        EnterpriseRole::Developer,
        vec![team],
    );
    let outsider = account(
        &state,
        &session,
        "outsider",
        EnterpriseRole::Developer,
        vec![],
    );
    let maintainer = account(
        &state,
        &session,
        "maintainer",
        EnterpriseRole::Maintainer,
        vec![team],
    );
    let principal = state.auth_store.authenticate_session(&maintainer).unwrap();
    let pat = state
        .auth_store
        .create_personal_access_token(&principal, "first-push", None)
        .unwrap();
    let revoked = state
        .auth_store
        .login("admin", "Password!2026")
        .unwrap()
        .raw_session_token;
    state.auth_store.revoke_session(&admin, &revoked).unwrap();
    let revoked_pat = state
        .auth_store
        .create_personal_access_token(&admin, "revoked", None)
        .unwrap();
    state
        .auth_store
        .revoke_personal_access_token(&admin, revoked_pat.metadata.id)
        .unwrap();
    let identity = std::fs::read(repo.join("gitust-repository/identity.json")).unwrap();
    let receipt = std::fs::read(repo.join("gitust-repository/registration.json")).unwrap();
    let app = build_router(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let router = app.clone();
    let mut server = ServerTask(tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    }));
    let url = format!("http://{address}/git/{}.git", id.0);
    let client = temp.path().join("client");
    let clone = client_git(
        temp.path(),
        &[
            "-c",
            "init.defaultBranch=main",
            "clone",
            &url,
            client.to_str().unwrap(),
        ],
        Some(&session),
    )
    .await;
    assert!(clone.status.success(), "empty clone failed");
    git(&client, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    std::fs::write(client.join("README.md"), "first real commit\n").unwrap();
    git(&client, &["add", "README.md"]);
    git(&client, &["commit", "-qm", "first commit"]);
    let expected = git(&client, &["rev-parse", "HEAD"]);
    for credential in [
        None,
        Some(developer.as_str()),
        Some(outsider.as_str()),
        Some(revoked.as_str()),
        Some(revoked_pat.raw_token.as_str()),
    ] {
        let denied = client_git(&client, &["push", &url, "HEAD:refs/heads/main"], credential).await;
        assert!(
            !denied.status.success(),
            "unauthorized ordinary first push succeeded"
        );
        assert!(git(&repo, &["for-each-ref", "--format=%(refname)"]).is_empty());
    }
    let topic = client_git(
        &client,
        &["push", &url, "HEAD:refs/heads/topic"],
        Some(&session),
    )
    .await;
    assert!(
        topic.status.success(),
        "authorized initial topic push failed"
    );
    assert_eq!(git(&repo, &["rev-parse", "topic"]), expected);
    let (_, body) = json(&app, "/api/v1/repo", &session).await;
    assert!(
        body["head_commit"].is_null(),
        "another branch must not replace the default"
    );
    assert_eq!(body["branch"], "main");
    assert_eq!(
        json(
            &app,
            &format!("/api/v1/repos/{}/commits?rev=topic", id.0),
            &session
        )
        .await
        .0,
        StatusCode::OK
    );
    let pushed = client_git(
        &client,
        &["push", &url, "HEAD:refs/heads/main"],
        Some(&pat.raw_token),
    )
    .await;
    assert!(
        pushed.status.success(),
        "authorized initial default push failed"
    );
    assert!(
        client_git(&client, &["fetch", &url, "main"], Some(&pat.raw_token))
            .await
            .status
            .success()
    );
    assert_eq!(git(&repo, &["rev-parse", "HEAD"]), expected);
    assert_eq!(git(&client, &["rev-parse", "FETCH_HEAD"]), expected);
    let (_, body) = json(&app, "/api/v1/repo", &session).await;
    assert_eq!(body["head_commit"], expected);
    assert_eq!(body["id"], id.0.to_string());
    assert_eq!(
        std::fs::read(repo.join("gitust-repository/identity.json")).unwrap(),
        identity
    );
    assert_eq!(
        std::fs::read(repo.join("gitust-repository/registration.json")).unwrap(),
        receipt
    );
    assert_eq!(
        state
            .auth_store
            .authorize_repository_access(&admin, id)
            .unwrap(),
        registration
    );
    assert!(state.worktrees.read().is_empty());
    assert!(state.automatic_checks.store().is_err());
    git(&repo, &["fsck", "--full", "--strict"]);
    git(&client, &["fsck", "--full", "--strict"]);
    server.0.abort();
    let _ = (&mut server.0).await;
    drop(server);
    drop(app);
    let store = state.auth_store.clone();
    drop(state);
    let restarted =
        AppState::with_auth_store(repo, "https://example.invalid/repo.git".into(), store).unwrap();
    assert_eq!(restarted.repo_id, id);
    assert_eq!(
        restarted
            .git_engine
            .inspect_head_identity()
            .unwrap()
            .head_sha
            .0,
        expected
    );
}
