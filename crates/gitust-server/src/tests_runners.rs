//! Runner inventory, unsupported execution rejection, and local workflow cache tests.

use super::tests_pull_requests::{api_json, git, invite_and_login_role};
use super::tests_repository_visibility::fixture;
use super::*;
use gitust_core::EnterpriseRole;
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
async fn registered_remote_runners_never_execute_on_the_server() {
    let (_temp, mut state, admin_token, _) = fixture();
    state = state.with_trusted_sandbox_exec(true);
    let primary_id_str = state.repo_id.0.to_string();
    let primary_repo = state.git_engine.repo_path().to_path_buf();
    let app = build_router(state.clone());
    let dev_token = invite_and_login_role(
        &state,
        &admin_token,
        "dev@company.internal",
        "dev",
        EnterpriseRole::Developer,
        vec![],
    );

    // 1. GET /api/v1/admin/runners lists the built-in local runner for SiteAdmin, rejects Developer (403)
    let (status, _) = api_json(
        &app,
        "GET",
        "/api/v1/admin/runners".to_owned(),
        &dev_token,
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "non-admin must not list runner fleet"
    );

    let (status, list_res) = api_json(
        &app,
        "GET",
        "/api/v1/admin/runners".to_owned(),
        &admin_token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "list runners: {list_res}");
    let runners = list_res["runners"].as_array().expect("runners array");
    assert_eq!(runners.len(), 1);
    assert_eq!(runners[0]["name"], "built-in-local");
    assert_eq!(runners[0]["status"], "online");

    // 2. Non-admin cannot register a runner
    let (status, _) = api_json(
        &app,
        "POST",
        "/api/v1/admin/runners".to_owned(),
        &dev_token,
        Some(json!({
            "name": "unauth-runner",
            "labels": ["linux-x64"],
            "supported_tiers": ["container_vm"],
            "max_concurrency": 2
        })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // 3. Workflow requiring `linux-x64` + `container_vm` fails closed (409) before runner registration
    let vm_wf_sha = commit_json(
        &primary_repo,
        ".github/workflows/isolated.json",
        json!({
            "name": "Isolated VM Pipeline",
            "on": ["push"],
            "jobs": [{
                "id": "isolated_build",
                "name": "Isolated Build",
                "needs": [],
                "runs_on": "linux-x64",
                "isolation_tier": "container_vm",
                "steps": [{
                    "name": "Verify container_vm isolation tier env",
                    "run": "echo \"TIER=$GITUST_ISOLATION_TIER VM=$GITUST_CONTAINER_VM_SANDBOX\""
                }]
            }]
        }),
        "Add container_vm workflow",
    );
    let (status, no_runner_res) = trigger_run(
        &app,
        &primary_id_str,
        &admin_token,
        &vm_wf_sha,
        Some("Isolated VM Pipeline"),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_IMPLEMENTED,
        "unsupported VM backend must fail closed: {no_runner_res}"
    );

    // 4. Register `vm-runner-01` supporting `linux-x64` and `container_vm`
    let (status, reg_res) = api_json(
        &app,
        "POST",
        "/api/v1/admin/runners".to_owned(),
        &admin_token,
        Some(json!({
            "name": "vm-runner-01",
            "labels": ["linux-x64", "gpu"],
            "supported_tiers": ["host_os", "container_vm"],
            "max_concurrency": 2
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "register runner: {reg_res}");
    let runner_id = reg_res["id"].as_str().expect("runner id").to_owned();
    assert_eq!(reg_res["name"], "vm-runner-01");
    assert_eq!(reg_res["status"], "online");

    // Registering inventory cannot enable an unimplemented execution backend.
    let (status, _) = trigger_run(
        &app,
        &primary_id_str,
        &admin_token,
        &vm_wf_sha,
        Some("Isolated VM Pipeline"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED);

    let remote_sha = commit_json(
        &primary_repo,
        ".gitust-workflows.json",
        json!({
            "name": "Remote host pipeline", "jobs": [{
                "id": "remote", "runs_on": "linux-x64",
                "steps": [{"run": "echo remote-work"}]
            }]
        }),
        "Add remote host workflow",
    );
    let (status, _) = trigger_run(
        &app,
        &primary_id_str,
        &admin_token,
        &remote_sha,
        Some("Remote host pipeline"),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);

    // 6. Transition `vm-runner-01` to `draining` via heartbeat; subsequent scheduling fails closed
    let (status, hb_res) = api_json(
        &app,
        "POST",
        format!("/api/v1/admin/runners/{runner_id}/heartbeat"),
        &admin_token,
        Some(json!({ "status": "draining" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "heartbeat: {hb_res}");
    assert_eq!(hb_res["status"], "draining");

    let (status, _) = trigger_run(
        &app,
        &primary_id_str,
        &admin_token,
        &remote_sha,
        Some("Remote host pipeline"),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);

    // 7. Deregister `vm-runner-01` (204 No Content)
    let (status, _) = api_json(
        &app,
        "DELETE",
        format!("/api/v1/admin/runners/{runner_id}"),
        &admin_token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_job_caches_support_reuse_listing_and_eviction() {
    let (_temp, mut state, admin_token, _) = fixture();
    state = state.with_trusted_sandbox_exec(true);
    let primary_id_str = state.repo_id.0.to_string();
    let primary_repo = state.git_engine.repo_path().to_path_buf();
    let app = build_router(state.clone());

    let shared_cache_sha = commit_json(
        &primary_repo,
        ".github/workflows/shared-cache.json",
        json!({
            "name": "Local Job Cache",
            "on": ["push"],
            "jobs": [
                {
                    "id": "warm_cache",
                    "name": "Warm Cache",
                    "needs": [],
                    "runs_on": "local",
                    "isolation_tier": "host_os",
                    "cache": { "key": "shared-target-v1", "paths": [".shared-cache"] },
                    "steps": [{
                        "name": "Populate shared cache",
                        "run": "mkdir -p .shared-cache && echo \"compiled-locally\" > .shared-cache/artifact.bin"
                    }]
                },
                {
                    "id": "consume_cache",
                    "name": "Consume Cache",
                    "needs": ["warm_cache"],
                    "runs_on": "local",
                    "isolation_tier": "host_os",
                    "cache": { "key": "shared-target-v1", "paths": [".shared-cache"] },
                    "steps": [{
                        "name": "Read shared cache",
                        "run": "cat .shared-cache/artifact.bin"
                    }]
                }
            ]
        }),
        "Add local-cache shared cache workflow",
    );

    let (status, run_res) = trigger_run(
        &app,
        &primary_id_str,
        &admin_token,
        &shared_cache_sha,
        Some("Local Job Cache"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "local-cache run: {run_res}");
    assert_eq!(run_res["status"], "passed");
    assert_eq!(run_res["jobs"][0]["runner_id"], "built-in-local");
    assert_eq!(run_res["jobs"][0]["cache_hit"], false);
    assert_eq!(run_res["jobs"][1]["runner_id"], "built-in-local");
    assert_eq!(run_res["jobs"][1]["cache_hit"], true);
    assert!(run_res["jobs"][1]["redacted_output"]
        .as_str()
        .unwrap_or_default()
        .contains("compiled-locally"));

    // List shared caches via GET /api/v1/repos/{id}/caches
    let (status, caches_res) = api_json(
        &app,
        "GET",
        format!("/api/v1/repos/{primary_id_str}/caches"),
        &admin_token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "list caches: {caches_res}");
    let caches = caches_res["caches"].as_array().expect("caches array");
    assert_eq!(caches.len(), 1);
    assert_eq!(caches[0]["key"], "shared-target-v1");
    assert_eq!(caches[0]["files_count"], 1);
    assert_eq!(caches[0]["source_job_id"], "consume_cache");
    assert!(caches[0]["size_bytes"].as_u64().unwrap_or(0) > 0);

    // Evicting while a workflow/local check holds `local_checks_lock` fails closed with 409 Conflict
    let held_guard = state
        .local_checks_lock
        .clone()
        .try_lock_owned()
        .expect("acquire local_checks_lock");
    let (status, conflict_res) = api_json(
        &app,
        "DELETE",
        format!("/api/v1/repos/{primary_id_str}/caches/shared-target-v1"),
        &admin_token,
        None,
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "evict while workflow lock held must return 409: {conflict_res}"
    );
    drop(held_guard);

    // Evict shared cache via DELETE /api/v1/repos/{id}/caches/shared-target-v1
    let (status, _) = api_json(
        &app,
        "DELETE",
        format!("/api/v1/repos/{primary_id_str}/caches/shared-target-v1"),
        &admin_token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, caches_after) = api_json(
        &app,
        "GET",
        format!("/api/v1/repos/{primary_id_str}/caches"),
        &admin_token,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(caches_after["caches"].as_array().unwrap().len(), 0);
}
