use std::time::Instant;

use axum::{body::Body, http::Request, http::StatusCode};
use gitust_core::{AgentKind, RepoId, WorktreeId};
use gitust_git::GitEngine;
use gitust_sandbox::{LocalSandboxExecutor, SandboxSpec};
use tower::ServiceExt;

use super::{init_temp_repo, test_app_at};

#[tokio::test]
async fn performance_budget_sandbox_startup_metadata_and_git_api_p99() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo_dir = tmp.path().join("repo");
    init_temp_repo(&repo_dir);

    let engine = GitEngine::new(&repo_dir);
    let _service_lock = engine.lock_worktree_service().expect("service lock");
    let repo_id = RepoId::new();

    // Warm up CoW template on first worktree, then measure warm CoW spawn.
    let _warm = engine
        .materialize_agent_worktree(
            WorktreeId::new(),
            repo_id,
            AgentKind::ClaudeCode,
            "agent/claude/perf-warm",
            "main",
            "warm template",
        )
        .expect("materialize warm");
    let m2 = engine
        .materialize_agent_worktree(
            WorktreeId::new(),
            repo_id,
            AgentKind::CursorAgent,
            "agent/cursor/perf-cow",
            "main",
            "measured cow",
        )
        .expect("materialize cow");

    // 1. Prepared sandbox execution startup (< 100-200ms target).
    let spec = SandboxSpec::for_linked_worktree(
        &m2.worktree_dir,
        &m2.worktree_admin_dir,
        &m2.common_git_dir,
        m2.main_workdir.as_deref(),
        &m2.worktrees_base,
        &m2.sandbox_scratch_base,
    );
    let executor = LocalSandboxExecutor::new(4);
    let mut sandbox_samples_us = Vec::with_capacity(10);
    for _ in 0..10 {
        let t0 = Instant::now();
        let res = executor
            .execute(m2.worktree.id, &spec, "true")
            .await
            .expect("sandbox exec true");
        assert_eq!(res.exit_code, 0);
        sandbox_samples_us.push(t0.elapsed().as_micros() as u64);
    }
    sandbox_samples_us.sort_unstable();
    let sandbox_median_ms = sandbox_samples_us[5] as f64 / 1000.0;
    let sandbox_p99_ms = sandbox_samples_us[9] as f64 / 1000.0;
    println!(
        "sandbox_startup_ms min={:.2} median={:.2} p99={:.2}",
        sandbox_samples_us[0] as f64 / 1000.0,
        sandbox_median_ms,
        sandbox_p99_ms
    );
    assert!(
        sandbox_median_ms < 100.0 && sandbox_p99_ms < 200.0,
        "prepared sandbox startup exceeded 100-200ms target: median={sandbox_median_ms:.2}ms p99={sandbox_p99_ms:.2}ms"
    );

    drop(_service_lock);

    // 2. Metadata API p99 (< 15ms target) & Git Tree/Commit API p99 (< 30ms target).
    let (app, token) = test_app_at(repo_dir.clone(), false);
    let auth_header = format!("Bearer {token}");

    // Discover primary repo_id from /api/v1/repos
    let status_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/repos")
                .header("Authorization", &auth_header)
                .body(Body::empty())
                .expect("req"),
        )
        .await
        .expect("repos");
    assert_eq!(status_resp.status(), StatusCode::OK);
    let status_bytes = http_body_util::BodyExt::collect(status_resp.into_body())
        .await
        .expect("body")
        .to_bytes();
    let status_json: serde_json::Value = serde_json::from_slice(&status_bytes).expect("json");
    let primary_repo_id = status_json["repositories"][0]["id"]
        .as_str()
        .expect("primary repo id")
        .to_owned();

    let mut metadata_us = Vec::with_capacity(100);
    for _ in 0..100 {
        let t0 = Instant::now();
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/repos")
                    .header("Authorization", &auth_header)
                    .body(Body::empty())
                    .expect("req"),
            )
            .await
            .expect("repos");
        assert_eq!(resp.status(), StatusCode::OK);
        metadata_us.push(t0.elapsed().as_micros() as u64);
    }
    metadata_us.sort_unstable();
    let metadata_p99_ms = metadata_us[98] as f64 / 1000.0;
    println!(
        "metadata_api_ms min={:.3} median={:.3} p99={:.3}",
        metadata_us[0] as f64 / 1000.0,
        metadata_us[50] as f64 / 1000.0,
        metadata_p99_ms
    );
    assert!(
        metadata_p99_ms < 15.0,
        "general metadata API p99 exceeded 15ms target: {metadata_p99_ms:.3}ms"
    );

    let mut git_tree_commit_us = Vec::with_capacity(100);
    for i in 0..100 {
        let uri = if i % 2 == 0 {
            format!("/api/v1/repos/{primary_repo_id}/tree")
        } else {
            format!("/api/v1/repos/{primary_repo_id}/commits")
        };
        let t0 = Instant::now();
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(&uri)
                    .header("Authorization", &auth_header)
                    .body(Body::empty())
                    .expect("req"),
            )
            .await
            .expect("tree/commits");
        assert_eq!(resp.status(), StatusCode::OK);
        git_tree_commit_us.push(t0.elapsed().as_micros() as u64);
    }
    git_tree_commit_us.sort_unstable();
    let git_api_p99_ms = git_tree_commit_us[98] as f64 / 1000.0;
    println!(
        "git_tree_commit_api_ms min={:.3} median={:.3} p99={:.3}",
        git_tree_commit_us[0] as f64 / 1000.0,
        git_tree_commit_us[50] as f64 / 1000.0,
        git_api_p99_ms
    );
    assert!(
        git_api_p99_ms < 30.0,
        "Git Tree/Commit API p99 exceeded 30ms target: {git_api_p99_ms:.3}ms"
    );
}
