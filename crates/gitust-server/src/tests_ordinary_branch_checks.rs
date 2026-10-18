use super::tests_git_http_secondary::git as run_git;
use super::tests_repository_visibility::fixture;
use super::*;
use crate::{
    local_checks_automatic as automatic,
    local_checks_automatic_model::{History, Phase},
};
use gitust_core::CommitSha;
use serde_json::{json, Value};

fn git(repo: &std::path::Path, args: &[&str]) -> String {
    super::tests_automatic_checks::git(repo, args)
}

async fn api_json(
    app: &Router,
    method: &str,
    uri: String,
    token: &str,
    payload: Option<Value>,
) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(payload.map_or_else(Body::empty, |v| Body::from(v.to_string())))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let value = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()));
    (status, value)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn automatic_checks_run_on_non_default_and_newly_created_ordinary_branches_with_branch_filter(
) {
    let (_temp, state, token, _) = fixture();
    let state = state.with_trusted_sandbox_exec(true);
    let admin = state.auth_store.authenticate_session(&token).unwrap();
    let service = automatic::start(&state).await.unwrap();
    let repo = state.git_engine.repo_path();

    // Commit a workflow policy on `main` that allows `main` and `release/v1`.
    std::fs::write(
        repo.join(".gitust-checks.json"),
        json!({
            "version": 1,
            "auto_run": true,
            "branches": ["main", "release/v1"],
            "steps": [
                {"name": "verify-branch", "run": "test -f release.txt && printf branch-ok"}
            ]
        })
        .to_string(),
    )
    .unwrap();
    git(repo, &["add", ".gitust-checks.json"]);
    git(repo, &["commit", "-qm", "enable workflow policy on main"]);

    // Stage a commit for a brand-new ordinary branch `release/v1` without creating `refs/heads/release/v1` yet.
    git(repo, &["checkout", "--detach", "HEAD"]);
    std::fs::write(repo.join("release.txt"), "v1.0\n").unwrap();
    git(repo, &["add", "release.txt"]);
    git(repo, &["commit", "-qm", "release v1 initial"]);
    let release_first = git(repo, &["rev-parse", "HEAD"]);
    git(repo, &["checkout", "main"]);

    // Creating `refs/heads/feature/excluded` (not in `branches` filter) must be Ineligible.
    let excluded_update = gitust_git::ReceivePackRefUpdate {
        old_oid: "0".repeat(40),
        new_oid: release_first.clone(),
        refname: "refs/heads/feature/excluded".into(),
        is_agent_branch: false,
        is_deletion: false,
    };
    assert!(matches!(
        automatic::prepare(&state, &admin, &[excluded_update]).unwrap(),
        automatic::Eligibility::Ineligible
    ));

    // Creating `refs/heads/release/v1` (in `branches` filter) uses `main`'s prior committed consent.
    let create_release = gitust_git::ReceivePackRefUpdate {
        old_oid: "0".repeat(40),
        new_oid: release_first.clone(),
        refname: "refs/heads/release/v1".into(),
        is_agent_branch: false,
        is_deletion: false,
    };
    let automatic::Eligibility::Prepared(intent1) =
        automatic::prepare(&state, &admin, &[create_release]).unwrap()
    else {
        panic!("expected new ordinary branch creation to be eligible via default-branch consent");
    };
    let run1 = intent1.run_id;
    git(repo, &["branch", "release/v1", &release_first]);
    automatic::confirm(&state, intent1).unwrap();

    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            let doc = state
                .local_checks_automatic
                .store(&state)
                .unwrap()
                .read()
                .unwrap();
            if let Some(job) = doc.recent.into_iter().find(|j| j.run_id == run1) {
                assert_eq!(job.phase, Phase::Completed);
                assert_eq!(job.history, History::Saved);
                assert_eq!(job.source_ref, "refs/heads/release/v1");
                assert_eq!(job.expected_commit, CommitSha(release_first.clone()));
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();

    // Updating existing `refs/heads/release/v1` uses `refs/heads/release/v1`'s prior committed consent.
    git(repo, &["checkout", "--detach", "release/v1"]);
    std::fs::write(repo.join("release.txt"), "v1.1\n").unwrap();
    git(repo, &["add", "release.txt"]);
    git(repo, &["commit", "-qm", "release v1 second"]);
    let release_second = git(repo, &["rev-parse", "HEAD"]);
    git(repo, &["checkout", "main"]);

    let update_release = gitust_git::ReceivePackRefUpdate {
        old_oid: release_first,
        new_oid: release_second.clone(),
        refname: "refs/heads/release/v1".into(),
        is_agent_branch: false,
        is_deletion: false,
    };
    let automatic::Eligibility::Prepared(intent2) =
        automatic::prepare(&state, &admin, &[update_release]).unwrap()
    else {
        panic!("expected existing ordinary branch update to be eligible");
    };
    let run2 = intent2.run_id;
    git(repo, &["branch", "-f", "release/v1", &release_second]);
    automatic::confirm(&state, intent2).unwrap();

    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            let doc = state
                .local_checks_automatic
                .store(&state)
                .unwrap()
                .read()
                .unwrap();
            if let Some(job) = doc.recent.into_iter().find(|j| j.run_id == run2) {
                assert_eq!(job.phase, Phase::Completed);
                assert_eq!(job.history, History::Saved);
                assert_eq!(job.source_ref, "refs/heads/release/v1");
                assert_eq!(job.expected_commit, CommitSha(release_second.clone()));
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();

    service.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn secondary_git_write_repository_supports_explicit_and_automatic_ordinary_branch_checks() {
    let (temp, state, token, _bare, secondary_id, readonly_id) =
        super::tests_git_http_write::fixture();
    let state = state.with_trusted_sandbox_exec(true);
    let service = automatic::start(&state).await.unwrap();
    let app = build_router(state.clone());

    let (status, catalog) = api_json(&app, "GET", "/api/v1/repos".into(), &token, None).await;
    assert_eq!(status, StatusCode::OK);
    for row in catalog["repositories"].as_array().unwrap() {
        let id = row["id"].as_str().unwrap();
        let writable = id == state.repo_id.0.to_string() || id == secondary_id.0.to_string();
        assert_eq!(row["can_run_local_checks"], writable, "row: {row}");
        assert_eq!(
            row["can_view_local_checks_automatic"], writable,
            "row: {row}"
        );
    }
    assert_eq!(
        api_json(
            &app,
            "GET",
            format!("/api/v1/repos/{}/local-check-runs", readonly_id.0),
            &token,
            None,
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    struct Stop(tokio::task::JoinHandle<()>);
    impl Drop for Stop {
        fn drop(&mut self) {
            self.0.abort();
        }
    }
    let server_app = app.clone();
    let _server = Stop(tokio::spawn(async move {
        axum::serve(listener, server_app).await.unwrap()
    }));

    let remote = format!("http://{address}/git/{}.git", secondary_id.0);
    let client = temp.path().join("secondary-writer");
    assert!(run_git(
        temp.path(),
        &["clone", &remote, client.to_str().unwrap()],
        Some(&token),
    )
    .status
    .success());

    // Push 1: commit workflow policy on `main` (activates prior consent for subsequent pushes).
    std::fs::write(
        client.join(".gitust-checks.json"),
        json!({
            "version": 1,
            "auto_run": true,
            "branches": ["main", "release/v1"],
            "steps": [
                {"name": "secondary-step", "run": "printf secondary-workflow-ok"}
            ]
        })
        .to_string(),
    )
    .unwrap();
    git(&client, &["add", ".gitust-checks.json"]);
    git(&client, &["commit", "-qm", "enable secondary workflow"]);
    assert!(run_git(&client, &["push", "origin", "main"], Some(&token))
        .status
        .success());

    // Push 2: create new ordinary branch `release/v1` and push to secondary `--git-write-repo`.
    git(&client, &["checkout", "-b", "release/v1"]);
    std::fs::write(client.join("secondary-release.txt"), "ok\n").unwrap();
    git(&client, &["add", "secondary-release.txt"]);
    git(&client, &["commit", "-qm", "secondary release branch"]);
    let expected_sha = git(&client, &["rev-parse", "HEAD"]);
    assert!(
        run_git(&client, &["push", "origin", "release/v1"], Some(&token),)
            .status
            .success()
    );

    // Wait for automatic Local Checks on the secondary repository to complete and save history.
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            let (status, auto_body) = api_json(
                &app,
                "GET",
                format!(
                    "/api/v1/repos/{}/local-check-automatic-runs",
                    secondary_id.0
                ),
                &token,
                None,
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            if let Some(run) = auto_body["runs"].as_array().and_then(|runs| runs.first()) {
                if run["phase"] == "completed" {
                    assert_eq!(run["source_ref"], "refs/heads/release/v1");
                    assert_eq!(run["expected_commit"], expected_sha);
                    assert_eq!(run["history"], "saved");
                    break;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();

    // Explicit Local Checks on the non-default ordinary branch `refs/heads/release/v1` of the secondary repo.
    let (status, exec_body) = api_json(
        &app,
        "POST",
        format!("/api/v1/repos/{}/local-check-runs", secondary_id.0),
        &token,
        Some(json!({
            "expected_source_ref": "refs/heads/release/v1",
            "expected_commit": expected_sha,
            "timeout_secs": 10
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{exec_body}");
    assert_eq!(exec_body["status"], "passed");
    assert_eq!(exec_body["record"]["source_ref"], "refs/heads/release/v1");
    assert_eq!(
        exec_body["record"]["result"]["stdout"],
        "secondary-workflow-ok"
    );

    let (status, history_body) = api_json(
        &app,
        "GET",
        format!("/api/v1/repos/{}/local-check-runs", secondary_id.0),
        &token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(history_body["records"].as_array().unwrap().len(), 2);

    service.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn active_ephemeral_checks_worktree_on_secondary_git_write_repo_preserves_hosting_and_hides_ref(
) {
    let (_temp, state, token, _bare, secondary_id, _readonly_id) =
        super::tests_git_http_write::fixture();
    let app = build_router(state.clone());
    let repo = state.writable_repository(secondary_id).unwrap();
    let owner = repo.engine.lock_worktree_service().unwrap();

    let wt_id = WorktreeId::new();
    let branch = format!("agent/cli/sandbox-{}", &wt_id.0.to_string()[..8]);
    owner.register_ephemeral_worktree(wt_id);
    let materialized = repo
        .engine
        .materialize_agent_worktree(
            wt_id,
            secondary_id,
            AgentKind::InternalBot,
            &branch,
            "main",
            "Repository Checks",
        )
        .unwrap();

    // While registered as an active ephemeral worktree, validate_ordinary_git_service succeeds
    // and info/refs advertisement succeeds while hiding `refs/heads/agent/cli/*`.
    repo.engine.validate_ordinary_git_service().unwrap();
    for uri in [
        format!(
            "/git/{}.git/info/refs?service=git-upload-pack",
            secondary_id.0
        ),
        format!(
            "/git/{}.git/info/refs?service=git-receive-pack",
            secondary_id.0
        ),
    ] {
        let resp =
            super::tests_git_http_secondary::request(&app, "GET", uri, &token, Body::empty()).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let text = String::from_utf8_lossy(&bytes);
        assert!(
            !text.contains("refs/heads/agent/cli/"),
            "ephemeral sandbox ref leaked in advertisement: {text}"
        );
    }

    // Once unregistered while the directory still exists on disk, validate_ordinary_git_service fails closed.
    owner.unregister_ephemeral_worktree(wt_id);
    assert!(repo.engine.validate_ordinary_git_service().is_err());

    // After cleanup, validate_ordinary_git_service succeeds again.
    repo.engine
        .remove_agent_worktree(
            &materialized.worktree_dir,
            &materialized.worktree.branch,
            &materialized.worktree.base_commit,
        )
        .unwrap();
    repo.engine.validate_ordinary_git_service().unwrap();
}
