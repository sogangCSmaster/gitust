use super::*;
use std::path::Path;

pub(super) fn git(path: &Path, args: &[&str], token: Option<&str>) -> std::process::Output {
    let mut command = fixture_git();
    command
        .arg("-C")
        .arg(path)
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "credential.helper=",
            "-c",
            "protocol.version=0",
        ])
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "false")
        .env("GIT_AUTHOR_NAME", "Fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@company.internal")
        .env("GIT_COMMITTER_NAME", "Fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@company.internal")
        .env("NO_PROXY", "127.0.0.1")
        .env("no_proxy", "127.0.0.1");
    if let Some(token) = token {
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
pub(super) fn oracle(path: &Path, args: &[&str]) -> Vec<u8> {
    let out = git(path, args, None);
    assert!(out.status.success(), "fixture Git oracle failed");
    out.stdout
}
pub(super) async fn request(
    app: &Router,
    method: &str,
    uri: String,
    token: &str,
    body: Body,
) -> axum::response::Response {
    let mut builder = Request::builder().method(method).uri(uri);
    if !token.is_empty() {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    app.clone()
        .oneshot(builder.body(body).unwrap())
        .await
        .unwrap()
}
pub(super) fn fixture() -> (tempfile::TempDir, AppState, String, PathBuf, RepoId) {
    let (temp, state, session, id) = super::tests_repository_visibility::fixture();
    let secondary = state
        .read_repository(id)
        .unwrap()
        .engine
        .repo_path()
        .to_owned();
    std::fs::write(
        secondary.join("README.md"),
        "secondary independent contents\n",
    )
    .unwrap();
    oracle(&secondary, &["commit", "-am", "secondary distinct commit"]);
    oracle(
        &secondary,
        &["tag", "-a", "clone-proof", "-m", "annotated tag identity"],
    );
    (temp, state, session, secondary, id)
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn configured_secondary_native_clone_fetch_uses_its_own_objects_and_never_accepts_push() {
    let (temp, state, session, secondary, id) = fixture();
    let route_id = id.0;
    let admin = state.auth_store.authenticate_session(&session).unwrap();
    let pat = state
        .auth_store
        .create_personal_access_token(&admin, "native clone", None)
        .unwrap();
    let primary = state.git_engine.repo_path().to_owned();
    let refs = [
        "for-each-ref",
        "--format=%(refname)%00%(objectname)",
        "refs/heads",
        "refs/tags",
    ];
    let primary_before = oracle(&primary, &refs);
    let app = build_router(state.clone());
    let advertised = request(
        &app,
        "GET",
        format!("/git/{route_id}.git/info/refs?service=git-upload-pack"),
        &pat.raw_token,
        Body::empty(),
    )
    .await;
    assert_eq!(advertised.status(), StatusCode::OK);
    assert_eq!(
        advertised.headers()[header::CONTENT_TYPE],
        "application/x-git-upload-pack-advertisement"
    );
    let (_, catalog) = call(&app, "GET", "/api/v1/repos".into(), &session, None).await;
    let catalog: serde_json::Value = serde_json::from_str(&catalog).unwrap();
    assert!(catalog["repositories"]
        .as_array()
        .unwrap()
        .iter()
        .all(|row| row["can_clone"] == true));
    for alias in [
        "default".to_string(),
        "gitust".to_string(),
        state.repo_id.0.to_string(),
    ] {
        assert_eq!(
            request(
                &app,
                "GET",
                format!("/git/{alias}.git/info/refs?service=git-upload-pack"),
                &session,
                Body::empty()
            )
            .await
            .status(),
            StatusCode::OK
        );
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    struct Stop(tokio::task::JoinHandle<()>);
    impl Drop for Stop {
        fn drop(&mut self) {
            self.0.abort();
        }
    }
    let _server = Stop(server);
    let remote = format!("http://{address}/git/{route_id}.git");
    let client = temp.path().join("secondary-client.git");
    let clone = git(
        temp.path(),
        &["clone", "--bare", &remote, client.to_str().unwrap()],
        Some(&pat.raw_token),
    );
    assert!(clone.status.success(), "authorized secondary clone failed");
    assert_eq!(
        oracle(&client, &["rev-parse", "HEAD"]),
        oracle(&secondary, &["rev-parse", "HEAD"])
    );
    assert_ne!(
        oracle(&client, &["rev-parse", "HEAD"]),
        oracle(&primary, &["rev-parse", "HEAD"])
    );
    assert_eq!(oracle(&client, &refs), oracle(&secondary, &refs));
    assert_eq!(
        oracle(&client, &["cat-file", "-t", "refs/tags/clone-proof"]),
        b"tag\n"
    );
    std::fs::write(secondary.join("README.md"), "secondary moved tip\n").unwrap();
    oracle(&secondary, &["commit", "-am", "secondary fetch change"]);
    oracle(
        &secondary,
        &["tag", "-a", "fetched-tag", "-m", "second annotated object"],
    );
    assert!(git(
        &client,
        &[
            "fetch",
            "origin",
            "+refs/heads/*:refs/heads/*",
            "+refs/tags/*:refs/tags/*"
        ],
        Some(&pat.raw_token)
    )
    .status
    .success());
    assert_eq!(oracle(&client, &refs), oracle(&secondary, &refs));
    assert_eq!(
        oracle(&client, &["cat-file", "-t", "refs/tags/fetched-tag"]),
        b"tag\n"
    );
    let before_push = oracle(&secondary, &refs);
    assert!(!git(
        &client,
        &["push", "origin", "HEAD:refs/heads/denied"],
        Some(&pat.raw_token)
    )
    .status
    .success());
    let router = build_router(state.clone());
    for (method, path) in [
        ("GET", "info/refs?service=git-receive-pack"),
        ("POST", "git-receive-pack"),
    ] {
        assert_eq!(
            request(
                &router,
                method,
                format!("/git/{route_id}.git/{path}"),
                &session,
                Body::from("0000")
            )
            .await
            .status(),
            StatusCode::NOT_FOUND
        );
    }
    assert_eq!(oracle(&secondary, &refs), before_push);
    assert_eq!(oracle(&primary, &refs), primary_before);
    for repo in [&primary, &secondary, &client] {
        oracle(repo, &["fsck", "--full", "--strict"]);
    }
}
