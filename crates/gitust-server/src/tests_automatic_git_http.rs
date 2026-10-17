use super::tests_automatic_checks::{git, policy, settle};
use super::tests_verification::fixture;
use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn automatic_checks_follow_actual_smart_http_push_and_git_roundtrip() {
    let (tmp, state, app, token, wt, dir) = fixture().await;
    let repo = tmp.path().join("repo");
    policy(&repo, true, "printf pushed-check; grep pushed README.md");
    let _service = crate::automatic_checks::start(&state).unwrap();
    let api = app.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let client = tmp.path().join("client");
    let auth = format!("http.extraHeader=Authorization: Bearer {token}");
    let url = format!("http://{address}/git/default.git");
    git(
        tmp.path(),
        &["-c", &auth, "clone", &url, client.to_str().unwrap()],
    );
    git(&client, &["checkout", &wt.branch]);
    std::fs::write(client.join("README.md"), "pushed revision\n").unwrap();
    git(&client, &["commit", "-am", "Push checked content"]);
    let sha = git(&client, &["rev-parse", "HEAD"]);
    git(&client, &["-c", &auth, "push", "origin", &wt.branch]);
    let records = serde_json::to_value(settle(&state, &wt).await).unwrap();
    assert_eq!(records.as_array().unwrap().len(), 1);
    assert_eq!(records[0]["automatic"], true);
    assert_eq!(records[0]["checked_commit"], sha);
    assert_eq!(records[0]["result"]["exit_code"], 0);
    assert!(records[0]["result"]["stdout"]
        .as_str()
        .unwrap()
        .contains("pushed-check"));
    assert_eq!(state.worktrees.read()[0].status, WorktreeStatus::Pending);
    assert!(state.worktrees.read()[0].verified_commit.is_none());
    assert_eq!(git(&dir, &["rev-parse", "HEAD"]), sha);
    assert_eq!(git(&dir, &["status", "--porcelain"]), "");
    // An up-to-date push has no accepted ref mutation and must not create another run.
    git(&client, &["-c", &auth, "push", "origin", &wt.branch]);
    assert_eq!(settle(&state, &wt).await.len(), 1);
    // A stale client can submit old -> current even though receive-pack rejects old.
    // The HTTP status remains 200; unchanged before/after refs must not trigger a run.
    let old = git(&client, &["rev-parse", "HEAD^"]);
    let command = format!("{old} {sha} refs/heads/{}\0report-status\n", wt.branch);
    let mut body = format!("{:04x}{command}0000", command.len() + 4).into_bytes();
    body.extend_from_slice(b"PACK\0\0\0\x02\0\0\0\0");
    // SHA-1 checksum of the empty version-2 PACK above, produced by git pack-objects.
    body.extend_from_slice(&[
        0x02, 0x9d, 0x08, 0x82, 0x3b, 0xd8, 0xa8, 0xea, 0xb5, 0x10, 0xad, 0x6a, 0xc7, 0x5c, 0x82,
        0x3c, 0xfd, 0x3e, 0xd3, 0x1e,
    ]);
    let response = api
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/git/default.git/git-receive-pack")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    assert!(String::from_utf8_lossy(&bytes).contains("incorrect old value provided"));
    assert_eq!(
        settle(&state, &wt).await.len(),
        1,
        "a rejected update must not trigger Checks"
    );
    git(&client, &["-c", &auth, "fetch", "origin"]);
    assert_eq!(
        git(&client, &["rev-parse", &format!("origin/{}", wt.branch)]),
        sha
    );
    for path in [&repo, &client, &dir] {
        git(path, &["fsck", "--strict"]);
    }
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
}
