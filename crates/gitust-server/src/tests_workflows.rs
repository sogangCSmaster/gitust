use super::tests_pull_requests::{api_json, git, invite_and_login_role};
use super::tests_repository_visibility::fixture;
use super::*;
use serde_json::{json, Value};

fn commit_json(repo: &std::path::Path, rel_path: &str, content: Value, message: &str) -> String {
    let full = repo.join(rel_path);
    if let Some(parent) = full.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&full, serde_json::to_string_pretty(&content).unwrap()).unwrap();
    git(repo, &["add", rel_path]);
    git(repo, &["commit", "-m", message]);
    git(repo, &["rev-parse", "refs/heads/main"])
}

async fn trigger_run(
    app: &axum::Router,
    repo_id: &str,
    token: &str,
    sha: &str,
    wf_name: Option<&str>,
) -> (StatusCode, Value) {
    let mut payload = json!({ "expected_commit": sha, "source_ref": "main" });
    if let Some(name) = wf_name {
        payload["workflow_name"] = json!(name);
    }
    api_json(
        app,
        "POST",
        format!("/api/v1/repos/{repo_id}/workflow-runs"),
        token,
        Some(payload),
    )
    .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workflow_dag_validation_rejects_cycles_and_unknown_needs_and_expands_matrices_with_upstream_skip(
) {
    let (_temp, mut state, admin_token, _) = fixture();
    state = state.with_trusted_sandbox_exec(true);
    let primary_id_str = state.repo_id.0.to_string();
    let primary_repo = state.git_engine.repo_path().to_path_buf();
    let app = build_router(state.clone());

    // 1. Cyclic `needs` (`job_a` -> `job_b` -> `job_a`) must be rejected fail-closed
    let cycle_sha = commit_json(
        &primary_repo,
        ".gitust-workflows.json",
        json!({
            "name": "Cyclic CI",
            "on": ["push"],
            "jobs": [
                { "id": "job_a", "name": "Job A", "needs": ["job_b"], "steps": [{ "name": "A", "run": "echo a" }] },
                { "id": "job_b", "name": "Job B", "needs": ["job_a"], "steps": [{ "name": "B", "run": "echo b" }] }
            ]
        }),
        "Add cyclic workflow",
    );
    let (status, body) = trigger_run(&app, &primary_id_str, &admin_token, &cycle_sha, None).await;
    assert!(
        status == StatusCode::BAD_REQUEST || status == StatusCode::CONFLICT,
        "cyclic workflow must fail closed, got {status}: {body}"
    );

    // 2. Unknown `needs` dependency must be rejected fail-closed
    let unknown_needs_sha = commit_json(
        &primary_repo,
        ".gitust-workflows.json",
        json!({
            "name": "Unknown Needs CI",
            "on": ["push"],
            "jobs": [
                { "id": "job_a", "name": "Job A", "needs": ["missing_job"], "steps": [{ "name": "A", "run": "echo a" }] }
            ]
        }),
        "Add unknown needs workflow",
    );
    let (status, body) = trigger_run(
        &app,
        &primary_id_str,
        &admin_token,
        &unknown_needs_sha,
        None,
    )
    .await;
    assert!(
        status == StatusCode::BAD_REQUEST || status == StatusCode::CONFLICT,
        "unknown needs workflow must fail closed, got {status}: {body}"
    );

    // 3. Matrix expansion + topological DAG execution (`lint` -> `build (debug, release)` -> `package`)
    let matrix_sha = commit_json(
        &primary_repo,
        ".gitust-workflows.json",
        json!({
            "name": "Matrix Pipeline",
            "on": ["push", "pull_request"],
            "jobs": [
                {
                    "id": "package",
                    "name": "Package",
                    "needs": ["build"],
                    "steps": [{ "name": "Bundle", "run": "echo packaging-after-matrix" }]
                },
                {
                    "id": "build",
                    "name": "Build",
                    "needs": ["lint"],
                    "strategy": { "matrix": { "target": ["debug", "release"] } },
                    "steps": [{ "name": "Compile", "run": "echo building-${{ matrix.target }}" }]
                },
                {
                    "id": "lint",
                    "name": "Lint",
                    "needs": [],
                    "steps": [{ "name": "Check", "run": "echo lint-ok" }]
                }
            ]
        }),
        "Add valid matrix DAG workflow",
    );

    let (status, overview) = api_json(
        &app,
        "GET",
        format!("/api/v1/repos/{primary_id_str}/workflows"),
        &admin_token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "list workflows: {overview}");
    assert_eq!(overview["workflows"][0]["name"], "Matrix Pipeline");

    let (status, run_res) =
        trigger_run(&app, &primary_id_str, &admin_token, &matrix_sha, None).await;
    assert_eq!(status, StatusCode::OK, "trigger matrix workflow: {run_res}");
    assert_eq!(run_res["status"], "passed");
    let jobs = run_res["jobs"].as_array().expect("jobs array");
    assert_eq!(jobs.len(), 4, "lint + 2 matrix build jobs + package");
    assert_eq!(jobs[0]["job_id"], "lint");
    assert_eq!(jobs[0]["status"], "passed");
    assert!(jobs[0]["redacted_output"]
        .as_str()
        .unwrap_or_default()
        .contains("lint-ok"));
    assert_eq!(jobs[1]["display_name"], "Build (target=debug)");
    assert_eq!(jobs[1]["status"], "passed");
    assert!(jobs[1]["redacted_output"]
        .as_str()
        .unwrap_or_default()
        .contains("building-debug"));
    assert_eq!(jobs[2]["display_name"], "Build (target=release)");
    assert_eq!(jobs[2]["status"], "passed");
    assert!(jobs[2]["redacted_output"]
        .as_str()
        .unwrap_or_default()
        .contains("building-release"));
    assert_eq!(jobs[3]["job_id"], "package");
    assert_eq!(jobs[3]["status"], "passed");
    assert!(jobs[3]["redacted_output"]
        .as_str()
        .unwrap_or_default()
        .contains("packaging-after-matrix"));

    // 4. Upstream failure skips downstream `needs` jobs without executing their commands
    let fail_skip_sha = commit_json(
        &primary_repo,
        ".gitust-workflows.json",
        json!({
            "name": "Fail Skip Pipeline",
            "on": ["push"],
            "jobs": [
                {
                    "id": "build",
                    "name": "Build",
                    "needs": [],
                    "steps": [{ "name": "Fail step", "run": "echo build-failing && exit 1" }]
                },
                {
                    "id": "deploy",
                    "name": "Deploy",
                    "needs": ["build"],
                    "steps": [{ "name": "Must Not Run", "run": "echo DOWNSTREAM_SHOULD_NOT_RUN" }]
                }
            ]
        }),
        "Add upstream failure workflow",
    );

    let (status, fail_run) =
        trigger_run(&app, &primary_id_str, &admin_token, &fail_skip_sha, None).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "trigger failing workflow: {fail_run}"
    );
    assert_eq!(fail_run["status"], "failed");
    let fail_jobs = fail_run["jobs"].as_array().expect("jobs array");
    assert_eq!(fail_jobs.len(), 2);
    assert_eq!(fail_jobs[0]["job_id"], "build");
    assert_eq!(fail_jobs[0]["status"], "failed");
    assert_eq!(fail_jobs[1]["job_id"], "deploy");
    assert_eq!(fail_jobs[1]["status"], "skipped");
    assert!(!fail_jobs[1]["redacted_output"]
        .as_str()
        .unwrap_or_default()
        .contains("DOWNSTREAM_SHOULD_NOT_RUN"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn encrypted_workflow_secrets_enforce_admin_rbac_encryption_at_rest_and_mandatory_log_redaction(
) {
    let (temp, mut state, admin_token, _) = fixture();
    state = state.with_trusted_sandbox_exec(true);
    let auth_file = temp.path().join("auth/auth.json");

    let primary_id = state.repo_id;
    let primary_id_str = primary_id.0.to_string();
    let primary_repo = state.git_engine.repo_path().to_path_buf();
    let admin_principal = state.auth_store.authenticate_session(&admin_token).unwrap();
    let team_id = state
        .auth_store
        .authorize_repository_access(&admin_principal, primary_id)
        .unwrap()
        .owner_team_id;

    let dev_token = invite_and_login_role(
        &state,
        &admin_token,
        "dev-wf@company.internal",
        "dev_wf",
        EnterpriseRole::Developer,
        vec![team_id],
    );
    let app = build_router(state.clone());

    // 1. Developer cannot PUT or GET workflow secrets (403 FORBIDDEN)
    let (status, _) = api_json(
        &app,
        "PUT",
        format!("/api/v1/repos/{primary_id_str}/workflow-secrets/DEPLOY_TOKEN"),
        &dev_token,
        Some(json!({ "value": "should-not-save" })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let (status, _) = api_json(
        &app,
        "GET",
        format!("/api/v1/repos/{primary_id_str}/workflow-secrets"),
        &dev_token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // 2. SiteAdmin stores DEPLOY_TOKEN encrypted at rest
    let raw_secret = "super-secret-prod-token-9999";
    let (status, put_res) = api_json(
        &app,
        "PUT",
        format!("/api/v1/repos/{primary_id_str}/workflow-secrets/DEPLOY_TOKEN"),
        &admin_token,
        Some(json!({ "value": raw_secret })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "put secret: {put_res}");
    assert_eq!(put_res["name"], "DEPLOY_TOKEN");
    assert!(put_res.get("value").is_none());

    let persisted_auth_bytes = std::fs::read_to_string(&auth_file).unwrap();
    assert!(
        !persisted_auth_bytes.contains(raw_secret),
        "raw secret must be encrypted at rest in auth.json"
    );

    let (status, list_res) = api_json(
        &app,
        "GET",
        format!("/api/v1/repos/{primary_id_str}/workflow-secrets"),
        &admin_token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "list secrets: {list_res}");
    assert_eq!(list_res["secrets"][0]["name"], "DEPLOY_TOKEN");
    assert!(!list_res.to_string().contains(raw_secret));

    // 3. Workflow step uses `${{ secrets.DEPLOY_TOKEN }}` and output is redacted with `***`
    let secret_wf_sha = commit_json(
        &primary_repo,
        ".gitust-workflows.json",
        json!({
            "name": "Secret Deploy",
            "on": ["push"],
            "jobs": [{
                "id": "deploy",
                "name": "Deploy",
                "needs": [],
                "secrets": ["DEPLOY_TOKEN"],
                "steps": [{
                    "name": "Verify & Print Secret",
                    "run": "test \"${{ secrets.DEPLOY_TOKEN }}\" = \"super-secret-prod-token-9999\" && echo \"token=${{ secrets.DEPLOY_TOKEN }}\" && echo \"err=${{ secrets.DEPLOY_TOKEN }}\" >&2"
                }]
            }]
        }),
        "Add secret workflow",
    );

    let (status, run_res) =
        trigger_run(&app, &primary_id_str, &admin_token, &secret_wf_sha, None).await;
    assert_eq!(status, StatusCode::OK, "run secret workflow: {run_res}");
    assert_eq!(run_res["status"], "passed");
    assert!(
        !run_res.to_string().contains(raw_secret),
        "API must never leak raw secret"
    );
    let redacted_out = run_res["jobs"][0]["redacted_output"]
        .as_str()
        .unwrap_or_default();
    assert!(
        redacted_out.contains("token=***"),
        "stdout must redact secret: {redacted_out}"
    );
    assert!(
        redacted_out.contains("err=***"),
        "stderr must redact secret: {redacted_out}"
    );

    let common_dir = state.git_engine.repository_common_directory().unwrap();
    let runs_disk =
        std::fs::read_to_string(common_dir.join("gitust-workflows").join("runs.json")).unwrap();
    assert!(
        !runs_disk.contains(raw_secret),
        "persisted runs.json must never contain raw secret"
    );

    // 4. Secret expressions in cache.key or artifacts.name must be rejected fail-closed
    let secret_in_cache_sha = commit_json(
        &primary_repo,
        ".github/workflows/deploy.json",
        json!({
            "name": "Secret Leak Cache Key",
            "on": ["push"],
            "jobs": [{
                "id": "leak",
                "name": "Leak",
                "needs": [],
                "cache": { "key": "cache-${{ secrets.DEPLOY_TOKEN }}", "paths": ["dist"] },
                "steps": [{ "name": "noop", "run": "echo ok" }]
            }]
        }),
        "Reject secret in cache key",
    );
    let (status, _) = trigger_run(
        &app,
        &primary_id_str,
        &admin_token,
        &secret_in_cache_sha,
        Some("Secret Leak Cache Key"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // 5. DELETE /api/v1/repos/{id}/workflow-secrets/DEPLOY_TOKEN removes the secret
    let (status, _) = api_json(
        &app,
        "DELETE",
        format!("/api/v1/repos/{primary_id_str}/workflow-secrets/DEPLOY_TOKEN"),
        &admin_token,
        None,
    )
    .await;
    assert!(status == StatusCode::NO_CONTENT || status == StatusCode::OK);

    let (status, after_del) = api_json(
        &app,
        "GET",
        format!("/api/v1/repos/{primary_id_str}/workflow-secrets"),
        &admin_token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(after_del["secrets"].as_array().unwrap().len(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workflow_caches_and_artifacts_persist_across_runs_and_reject_path_traversal_and_symlinks()
{
    let (_temp, mut state, admin_token, _) = fixture();
    state = state.with_trusted_sandbox_exec(true);
    let primary_id_str = state.repo_id.0.to_string();
    let primary_repo = state.git_engine.repo_path().to_path_buf();
    let app = build_router(state.clone());

    // 1. First and second run with cache + artifact collection
    let cache_art_sha = commit_json(
        &primary_repo,
        ".github/workflows/ci.json",
        json!({
            "name": "Cache and Artifact CI",
            "on": ["push"],
            "jobs": [{
                "id": "build",
                "name": "Build with Cache",
                "needs": [],
                "cache": { "key": "cargo-v1", "paths": [".cache-dir"] },
                "artifacts": { "name": "dist-bundle", "paths": ["dist/output.txt"] },
                "steps": [
                    {
                        "name": "Check or populate cache",
                        "run": "if [ -f .cache-dir/cached.txt ]; then echo \"FROM_CACHE=$(cat .cache-dir/cached.txt)\"; else mkdir -p .cache-dir && echo \"warm-value-42\" > .cache-dir/cached.txt && echo \"CACHE_MISS_INIT\"; fi"
                    },
                    {
                        "name": "Produce artifact",
                        "run": "mkdir -p dist && echo \"artifact-payload-v1\" > dist/output.txt"
                    }
                ]
            }]
        }),
        "Add cache and artifact workflow",
    );

    let wf_name = Some("Cache and Artifact CI");
    let (status, run1) =
        trigger_run(&app, &primary_id_str, &admin_token, &cache_art_sha, wf_name).await;
    assert_eq!(status, StatusCode::OK, "run 1: {run1}");
    assert_eq!(run1["status"], "passed");
    assert_eq!(run1["jobs"][0]["cache_hit"], false);
    assert!(run1["jobs"][0]["redacted_output"]
        .as_str()
        .unwrap_or_default()
        .contains("CACHE_MISS_INIT"));
    assert_eq!(run1["jobs"][0]["artifacts"][0]["name"], "dist-bundle");
    assert_eq!(run1["jobs"][0]["artifacts"][0]["path"], "dist/output.txt");

    let run1_id = run1["run_id"].as_str().unwrap();
    let (status, art_res) = api_json(
        &app,
        "GET",
        format!("/api/v1/repos/{primary_id_str}/workflow-runs/{run1_id}/artifacts"),
        &admin_token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "get artifacts: {art_res}");
    assert_eq!(art_res["artifacts"][0]["name"], "dist-bundle");
    assert_eq!(art_res["artifacts"][0]["path"], "dist/output.txt");
    assert!(art_res["artifacts"][0]["content_preview"]
        .as_str()
        .unwrap_or_default()
        .contains("artifact-payload-v1"));

    // Second run with the same cache key (`cargo-v1`) must hit cache (`cache_hit: true`)
    let (status, run2) =
        trigger_run(&app, &primary_id_str, &admin_token, &cache_art_sha, wf_name).await;
    assert_eq!(status, StatusCode::OK, "run 2: {run2}");
    assert_eq!(run2["status"], "passed");
    assert_eq!(run2["jobs"][0]["cache_hit"], true);
    assert!(run2["jobs"][0]["redacted_output"]
        .as_str()
        .unwrap_or_default()
        .contains("FROM_CACHE=warm-value-42"));

    // 2. Path traversal in `artifacts.paths` (`../etc/passwd`) must fail closed
    let traversal_sha = commit_json(
        &primary_repo,
        ".gitust-workflows.json",
        json!({
            "name": "Traversal Workflow",
            "on": ["push"],
            "jobs": [{
                "id": "bad",
                "name": "Bad",
                "needs": [],
                "artifacts": { "name": "escape", "paths": ["../etc/passwd"] },
                "steps": [{ "name": "noop", "run": "echo noop" }]
            }]
        }),
        "Add path traversal workflow",
    );
    let (status, _) = trigger_run(
        &app,
        &primary_id_str,
        &admin_token,
        &traversal_sha,
        Some("Traversal Workflow"),
    )
    .await;
    assert!(
        status == StatusCode::BAD_REQUEST || status == StatusCode::CONFLICT,
        "path traversal in artifacts.paths must fail closed, got {status}"
    );

    // 3. Symlink escape in `artifacts.paths` created during step execution must fail closed
    let symlink_sha = commit_json(
        &primary_repo,
        ".gitust-workflows.json",
        json!({
            "name": "Symlink Escape Workflow",
            "on": ["push"],
            "jobs": [{
                "id": "symlink_job",
                "name": "Symlink Job",
                "needs": [],
                "artifacts": { "name": "escape", "paths": ["dist/leak.txt"] },
                "steps": [{ "name": "Create symlink", "run": "mkdir -p dist && ln -s /etc/hosts dist/leak.txt" }]
            }]
        }),
        "Add symlink escape workflow",
    );
    let (status, sym_res) = trigger_run(
        &app,
        &primary_id_str,
        &admin_token,
        &symlink_sha,
        Some("Symlink Escape Workflow"),
    )
    .await;
    if status == StatusCode::OK {
        assert_eq!(sym_res["status"], "failed");
        assert!(sym_res["jobs"][0]["artifacts"]
            .as_array()
            .unwrap()
            .is_empty());
    } else {
        assert!(
            status == StatusCode::BAD_REQUEST
                || status == StatusCode::CONFLICT
                || status == StatusCode::SERVICE_UNAVAILABLE
        );
    }

    // 4. Archiving the repository blocks workflow runs with 409 CONFLICT
    state.dynamic_registry.write().settings.insert(
        state.repo_id,
        crate::repository_registry::RepoRuntimeSettings {
            archived: true,
            workbench_enabled: Some(true),
            description: None,
        },
    );
    let (status, _) = trigger_run(&app, &primary_id_str, &admin_token, &symlink_sha, wf_name).await;
    assert_eq!(status, StatusCode::CONFLICT);
}
