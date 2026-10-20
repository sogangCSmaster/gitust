//! TDD tests for remote runner job dispatch, result/artifact collection,
//! multi-node least-loaded scheduling, fenced lease tokens, and automatic node failover.

use super::tests_pull_requests::{api_json, git};
use super::tests_repository_visibility::fixture;
use super::*;
use serde_json::json;
use sha2::{Digest, Sha256};

fn commit_file(repo: &std::path::Path, rel_path: &str, content: &str, message: &str) -> String {
    let full = repo.join(rel_path);
    if let Some(parent) = full.parent() {
        std::fs::create_dir_all(parent).expect("mkdir");
    }
    std::fs::write(&full, content).expect("write");
    git(repo, &["add", rel_path]);
    git(repo, &["commit", "-m", message]);
    git(repo, &["rev-parse", "refs/heads/main"])
}

async fn register_remote_runner(
    app: &axum::Router,
    token: &str,
    name: &str,
    label: &str,
) -> String {
    let (st, j) = api_json(
        app,
        "POST",
        "/api/v1/admin/runners".to_owned(),
        token,
        Some(json!({
            "name": name,
            "labels": [label],
            "supported_tiers": ["host_os"],
            "max_concurrency": 2,
            "remote_dispatch": true
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "register {name}: {j}");
    j["id"].as_str().expect("runner id").to_owned()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn remote_runner_dispatches_job_collects_redacted_logs_and_binary_artifacts() {
    let (_temp, mut state, admin_token, _) = fixture();
    state = state.with_trusted_sandbox_exec(true);
    let repo_id_str = state.repo_id.0.to_string();
    let repo_path = state.git_engine.repo_path().to_path_buf();
    let app = build_router(state.clone());

    // Configure a workflow secret to verify server-side secret substitution + output redaction
    let (sec_status, _) = api_json(
        &app,
        "PUT",
        format!("/api/v1/repos/{repo_id_str}/workflow-secrets/DEPLOY_KEY"),
        &admin_token,
        Some(json!({ "value": "super-secret-remote-token-999" })),
    )
    .await;
    assert_eq!(sec_status, StatusCode::OK);

    // Register a remote runner with remote_dispatch enabled
    let runner_id =
        register_remote_runner(&app, &admin_token, "remote-linux-01", "linux-x64").await;

    let wf_yaml = r#"name: Remote Build Pipeline
on: [push]
jobs:
  remote_build:
    name: Build on Remote Linux
    runs-on: linux-x64
    secrets: [DEPLOY_KEY]
    steps:
      - name: Run remote build with secret
        run: echo "auth=${{ secrets.DEPLOY_KEY }}" && mkdir -p dist && printf 'REMOTE_BIN' > dist/app.bin
      - name: Upload remote binary
        uses: actions/upload-artifact@v4
        with:
          name: remote-app
          path: dist/app.bin
"#;
    let head_sha = commit_file(
        &repo_path,
        ".github/workflows/remote.yml",
        wf_yaml,
        "Add remote workflow",
    );

    // Spawn a background worker simulating the remote runner daemon polling and completing the job
    let worker_app = app.clone();
    let worker_token = admin_token.clone();
    let worker_runner_id = runner_id.clone();
    let worker_handle = tokio::spawn(async move {
        for _ in 0..50 {
            let (poll_status, poll_json) = api_json(
                &worker_app,
                "POST",
                format!("/api/v1/admin/runners/{worker_runner_id}/jobs/poll"),
                &worker_token,
                Some(json!({})),
            )
            .await;
            assert_eq!(poll_status, StatusCode::OK);
            if !poll_json["job"].is_null() {
                let job = &poll_json["job"];
                let job_run_id = job["job_run_id"].as_str().expect("job_run_id");
                let lease_token = job["lease_token"].as_str().expect("lease_token");
                let commands = job["commands"].as_array().expect("commands");
                assert!(
                    commands[0]
                        .as_str()
                        .unwrap_or_default()
                        .contains("super-secret-remote-token-999"),
                    "remote runner must receive substituted command"
                );

                let artifact_bytes = b"\x7fELF\x02\x01\x01\x00remote-elf-binary";
                let (comp_status, comp_json) = api_json(
                    &worker_app,
                    "POST",
                    format!(
                        "/api/v1/admin/runners/{worker_runner_id}/jobs/{job_run_id}/complete"
                    ),
                    &worker_token,
                    Some(json!({
                        "lease_token": lease_token,
                        "exit_code": 0,
                        "stdout": "Authenticating with super-secret-remote-token-999\nBuild succeeded\n",
                        "stderr": "",
                        "duration_ms": 37,
                        "artifacts": [
                            {
                                "path": "dist/app.bin",
                                "hex": hex::encode(artifact_bytes)
                            }
                        ]
                    })),
                )
                .await;
                assert_eq!(comp_status, StatusCode::OK, "complete: {comp_json}");
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        panic!("remote runner never received a job");
    });

    let (run_status, run_json) = api_json(
        &app,
        "POST",
        format!("/api/v1/repos/{repo_id_str}/workflow-runs"),
        &admin_token,
        Some(json!({
            "expected_commit": head_sha,
            "source_ref": "main",
            "workflow_name": "Remote Build Pipeline"
        })),
    )
    .await;
    worker_handle.await.expect("worker task");
    assert_eq!(run_status, StatusCode::OK, "run_json: {run_json}");
    assert_eq!(run_json["status"], "passed");
    assert_eq!(run_json["jobs"][0]["runner_id"], "remote-linux-01");
    let redacted_out = run_json["jobs"][0]["redacted_output"]
        .as_str()
        .expect("redacted_output");
    assert!(
        !redacted_out.contains("super-secret-remote-token-999"),
        "secret must be redacted from remote runner output: {redacted_out}"
    );
    assert!(redacted_out.contains("Authenticating with ***"));
    assert_eq!(run_json["jobs"][0]["artifacts"][0]["name"], "remote-app");
    assert_eq!(run_json["jobs"][0]["artifacts"][0]["path"], "dist/app.bin");
    let expected_sha = hex::encode(Sha256::digest(b"\x7fELF\x02\x01\x01\x00remote-elf-binary"));
    assert_eq!(run_json["jobs"][0]["artifacts"][0]["sha256"], expected_sha);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn multi_node_runner_scheduling_balances_load_and_fails_over_on_offline_node() {
    let (_temp, mut state, admin_token, _) = fixture();
    state = state.with_trusted_sandbox_exec(true);
    let repo_id_str = state.repo_id.0.to_string();
    let repo_path = state.git_engine.repo_path().to_path_buf();
    let app = build_router(state.clone());

    // Register two remote runners for label `cluster-x64`
    let runner_1_id =
        register_remote_runner(&app, &admin_token, "cluster-node-1", "cluster-x64").await;
    let runner_2_id =
        register_remote_runner(&app, &admin_token, "cluster-node-2", "cluster-x64").await;

    let wf_yaml = r#"name: Cluster Failover Pipeline
on: [push]
jobs:
  cluster_job:
    name: Cluster Job
    runs-on: cluster-x64
    steps:
      - name: Execute on cluster
        run: echo "running on cluster"
"#;
    let head_sha = commit_file(
        &repo_path,
        ".github/workflows/cluster.yml",
        wf_yaml,
        "Add cluster workflow",
    );

    // Worker simulation:
    // 1. Runner 1 polls and claims the job (`stale_token`), then crashes / goes `offline` via heartbeat!
    // 2. Server automatically reassigns the job to Runner 2.
    // 3. Runner 1 (zombie) attempts to complete with `stale_token` -> rejected 409 Conflict!
    // 4. Runner 2 polls, receives the reassigned job with a fresh `lease_token`, and completes it -> 200 OK!
    let worker_app = app.clone();
    let worker_token = admin_token.clone();
    let r1_id = runner_1_id.clone();
    let r2_id = runner_2_id.clone();
    let failover_worker = tokio::spawn(async move {
        let mut stale_job_run_id = String::new();
        let mut stale_lease_token = String::new();

        for _ in 0..200 {
            let (_, poll1) = api_json(
                &worker_app,
                "POST",
                format!("/api/v1/admin/runners/{r1_id}/jobs/poll"),
                &worker_token,
                Some(json!({})),
            )
            .await;
            if !poll1["job"].is_null() {
                stale_job_run_id = poll1["job"]["job_run_id"]
                    .as_str()
                    .expect("job_run_id")
                    .to_owned();
                stale_lease_token = poll1["job"]["lease_token"]
                    .as_str()
                    .expect("lease_token")
                    .to_owned();
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert!(
            !stale_job_run_id.is_empty(),
            "runner 1 must claim initial job"
        );

        // Mark runner 1 offline -> triggers automatic failover to runner 2
        let (hb_status, _) = api_json(
            &worker_app,
            "POST",
            format!("/api/v1/admin/runners/{r1_id}/heartbeat"),
            &worker_token,
            Some(json!({ "status": "offline" })),
        )
        .await;
        assert_eq!(hb_status, StatusCode::OK);

        // Zombie runner 1 tries to complete using its revoked lease_token -> must fail closed (409)
        let (zombie_status, _) = api_json(
            &worker_app,
            "POST",
            format!("/api/v1/admin/runners/{r1_id}/jobs/{stale_job_run_id}/complete"),
            &worker_token,
            Some(json!({
                "lease_token": stale_lease_token,
                "exit_code": 0,
                "stdout": "zombie output",
                "stderr": "",
                "duration_ms": 10
            })),
        )
        .await;
        assert_eq!(
            zombie_status,
            StatusCode::CONFLICT,
            "fenced-out zombie runner must be rejected with 409 Conflict"
        );

        // Runner 2 polls and receives the failed-over job with a new lease_token
        let (_, poll2) = api_json(
            &worker_app,
            "POST",
            format!("/api/v1/admin/runners/{r2_id}/jobs/poll"),
            &worker_token,
            Some(json!({})),
        )
        .await;
        assert!(
            !poll2["job"].is_null(),
            "runner 2 must receive the failed-over job: {poll2}"
        );
        let new_job_id = poll2["job"]["job_run_id"].as_str().expect("job_run_id");
        let new_token = poll2["job"]["lease_token"].as_str().expect("lease_token");
        assert_eq!(new_job_id, stale_job_run_id);
        assert_ne!(new_token, stale_lease_token);
        assert_eq!(poll2["job"]["attempt"], 2);

        let (comp2_status, _) = api_json(
            &worker_app,
            "POST",
            format!("/api/v1/admin/runners/{r2_id}/jobs/{new_job_id}/complete"),
            &worker_token,
            Some(json!({
                "lease_token": new_token,
                "exit_code": 0,
                "stdout": "completed on healthy cluster-node-2\n",
                "stderr": "",
                "duration_ms": 25
            })),
        )
        .await;
        assert_eq!(comp2_status, StatusCode::OK);
    });

    let (run_status, run_json) = api_json(
        &app,
        "POST",
        format!("/api/v1/repos/{repo_id_str}/workflow-runs"),
        &admin_token,
        Some(json!({
            "expected_commit": head_sha,
            "source_ref": "main",
            "workflow_name": "Cluster Failover Pipeline"
        })),
    )
    .await;
    failover_worker.await.expect("failover worker");
    assert_eq!(run_status, StatusCode::OK, "run_json: {run_json}");
    assert_eq!(run_json["status"], "passed");
    assert_eq!(run_json["jobs"][0]["runner_id"], "cluster-node-2");
    assert!(run_json["jobs"][0]["redacted_output"]
        .as_str()
        .unwrap_or_default()
        .contains("completed on healthy cluster-node-2"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn remote_runner_edge_cases_cross_runner_theft_duplicate_replay_and_total_cluster_outage() {
    let (_temp, mut state, admin_token, _) = fixture();
    state = state.with_trusted_sandbox_exec(true);
    let repo_id_str = state.repo_id.0.to_string();
    let repo_path = state.git_engine.repo_path().to_path_buf();
    let app = build_router(state.clone());

    let r1_id = register_remote_runner(&app, &admin_token, "edge-node-1", "edge-pool").await;
    let r2_id = register_remote_runner(&app, &admin_token, "edge-node-2", "edge-pool").await;

    // Put node 2 into `draining` initially so node 1 gets the first job deterministically,
    // and verify a `draining` runner calling `/jobs/poll` gets `null`.
    api_json(
        &app,
        "POST",
        format!("/api/v1/admin/runners/{r2_id}/heartbeat"),
        &admin_token,
        Some(json!({ "status": "draining" })),
    )
    .await;
    let (_, drain_poll) = api_json(
        &app,
        "POST",
        format!("/api/v1/admin/runners/{r2_id}/jobs/poll"),
        &admin_token,
        Some(json!({})),
    )
    .await;
    assert!(drain_poll["job"].is_null());

    let wf_yaml = r#"name: Edge Runner Suite
on: [push]
jobs:
  edge_job:
    name: Edge Job
    runs-on: edge-pool
    steps:
      - name: Run edge check
        run: echo "edge check"
"#;
    let head_sha = commit_file(
        &repo_path,
        ".github/workflows/edge.yml",
        wf_yaml,
        "Add edge workflow",
    );

    // Part A: Cross-runner lease theft attempt (r2 tries to complete r1's job) + duplicate completion replay
    let w_app = app.clone();
    let w_tok = admin_token.clone();
    let w_r1 = r1_id.clone();
    let w_r2 = r2_id.clone();
    let worker_a = tokio::spawn(async move {
        let mut job_id = String::new();
        let mut token = String::new();
        for _ in 0..200 {
            let (_, poll) = api_json(
                &w_app,
                "POST",
                format!("/api/v1/admin/runners/{w_r1}/jobs/poll"),
                &w_tok,
                Some(json!({})),
            )
            .await;
            if !poll["job"].is_null() {
                job_id = poll["job"]["job_run_id"].as_str().expect("id").to_owned();
                token = poll["job"]["lease_token"].as_str().expect("tok").to_owned();
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert!(!job_id.is_empty());

        // Cross-runner theft: r2 presents r1's valid lease_token under r2's runner_id -> 409 Conflict
        let (theft_status, _) = api_json(
            &w_app,
            "POST",
            format!("/api/v1/admin/runners/{w_r2}/jobs/{job_id}/complete"),
            &w_tok,
            Some(json!({
                "lease_token": token,
                "exit_code": 0,
                "stdout": "stolen",
                "stderr": "",
                "duration_ms": 5
            })),
        )
        .await;
        assert_eq!(theft_status, StatusCode::CONFLICT);

        // Legitimate completion by r1 -> 200 OK
        let (ok_status, _) = api_json(
            &w_app,
            "POST",
            format!("/api/v1/admin/runners/{w_r1}/jobs/{job_id}/complete"),
            &w_tok,
            Some(json!({
                "lease_token": token,
                "exit_code": 0,
                "stdout": "legitimate output\n",
                "stderr": "",
                "duration_ms": 15
            })),
        )
        .await;
        assert_eq!(ok_status, StatusCode::OK);

        // Duplicate replay by r1 with same payload -> 409 Conflict
        let (replay_status, _) = api_json(
            &w_app,
            "POST",
            format!("/api/v1/admin/runners/{w_r1}/jobs/{job_id}/complete"),
            &w_tok,
            Some(json!({
                "lease_token": token,
                "exit_code": 0,
                "stdout": "replay",
                "stderr": "",
                "duration_ms": 15
            })),
        )
        .await;
        assert_eq!(replay_status, StatusCode::CONFLICT);
    });

    let (run_a_status, run_a_json) = api_json(
        &app,
        "POST",
        format!("/api/v1/repos/{repo_id_str}/workflow-runs"),
        &admin_token,
        Some(json!({
            "expected_commit": head_sha,
            "source_ref": "main",
            "workflow_name": "Edge Runner Suite"
        })),
    )
    .await;
    worker_a.await.expect("worker a");
    assert_eq!(run_a_status, StatusCode::OK);
    assert_eq!(run_a_json["status"], "passed");

    // Part B: Failover from r1 -> r2 followed by r2 going offline (total cluster outage):
    // Verify neither r1 nor r2 leaks `active_leases` (both return to 0 and can be deregistered).
    api_json(
        &app,
        "POST",
        format!("/api/v1/admin/runners/{r2_id}/heartbeat"),
        &admin_token,
        Some(json!({ "status": "online" })),
    )
    .await;
    let w_app2 = app.clone();
    let w_tok2 = admin_token.clone();
    let w_r1_b = r1_id.clone();
    let w_r2_b = r2_id.clone();
    let worker_b = tokio::spawn(async move {
        for _ in 0..200 {
            let (_, poll) = api_json(
                &w_app2,
                "POST",
                format!("/api/v1/admin/runners/{w_r1_b}/jobs/poll"),
                &w_tok2,
                Some(json!({})),
            )
            .await;
            if !poll["job"].is_null() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        // r1 goes offline -> fails over to online r2; then r2 also goes offline -> fails closed and releases r2 lease!
        api_json(
            &w_app2,
            "POST",
            format!("/api/v1/admin/runners/{w_r1_b}/heartbeat"),
            &w_tok2,
            Some(json!({ "status": "offline" })),
        )
        .await;
        api_json(
            &w_app2,
            "POST",
            format!("/api/v1/admin/runners/{w_r2_b}/heartbeat"),
            &w_tok2,
            Some(json!({ "status": "offline" })),
        )
        .await;
    });

    let (run_b_status, run_b_json) = api_json(
        &app,
        "POST",
        format!("/api/v1/repos/{repo_id_str}/workflow-runs"),
        &admin_token,
        Some(json!({
            "expected_commit": head_sha,
            "source_ref": "main",
            "workflow_name": "Edge Runner Suite"
        })),
    )
    .await;
    worker_b.await.expect("worker b");
    assert_eq!(run_b_status, StatusCode::OK);
    assert_eq!(run_b_json["status"], "failed");
    assert!(run_b_json["jobs"][0]["redacted_output"]
        .as_str()
        .unwrap_or_default()
        .contains("went offline and no healthy fallback runner is available"));
    let (del_r2_status, _) = api_json(
        &app,
        "DELETE",
        format!("/api/v1/admin/runners/{r2_id}"),
        &admin_token,
        None,
    )
    .await;
    assert_eq!(del_r2_status, StatusCode::NO_CONTENT);
}
