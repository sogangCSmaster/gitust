use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use serde_json::json;
use sha2::{Digest, Sha256};
use tower::ServiceExt;

use super::{fixture_git, init_temp_repo, test_app_at};

fn commit_file(repo_dir: &std::path::Path, rel_path: &str, content: &[u8], msg: &str) -> String {
    let full = repo_dir.join(rel_path);
    if let Some(parent) = full.parent() {
        std::fs::create_dir_all(parent).expect("mkdir");
    }
    std::fs::write(&full, content).expect("write file");
    let status = fixture_git()
        .arg("-C")
        .arg(repo_dir)
        .args(["add", rel_path])
        .status()
        .expect("git add");
    assert!(status.success());
    let status = fixture_git()
        .arg("-C")
        .arg(repo_dir)
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@company.internal")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@company.internal")
        .args(["-c", "commit.gpgsign=false", "commit", "-q", "-m", msg])
        .status()
        .expect("git commit");
    assert!(status.success());
    let out = fixture_git()
        .arg("-C")
        .arg(repo_dir)
        .args(["rev-parse", "HEAD"])
        .output()
        .expect("rev-parse");
    String::from_utf8(out.stdout)
        .expect("utf8")
        .trim()
        .to_owned()
}

#[tokio::test]
async fn yaml_workflow_and_github_actions_uses_checkout_cache_composite_and_cross_job_artifacts() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo_dir = tmp.path().join("repo");
    init_temp_repo(&repo_dir);

    let composite_yml = r#"name: Setup Env
runs:
  using: composite
  steps:
    - name: Write greeting
      run: echo "GREETING=${{ inputs.greeting }}" > greeting.txt
"#;
    commit_file(
        &repo_dir,
        ".github/actions/setup-env/action.yml",
        composite_yml.as_bytes(),
        "add local composite action",
    );

    let workflow_yml = r#"name: YAML Actions CI
on:
  push:
  pull_request:
jobs:
  build:
    name: Build Binary
    runs-on: local
    steps:
      - name: Checkout
        uses: actions/checkout@v4
      - name: Restore build cache
        uses: actions/cache@v4
        with:
          key: cargo-cache-v1
          path: .cache/cargo
      - name: Run local composite action
        uses: ./.github/actions/setup-env
        with:
          greeting: hello-from-composite
      - name: Produce binary and cached file
        run: mkdir -p dist .cache/cargo && printf '\x89PNG\r\n\x1a\nBINARY_PAYLOAD' > dist/app.bin && cp greeting.txt dist/greeting.txt && echo "cached-123" > .cache/cargo/index.txt
      - name: Upload build artifact
        uses: actions/upload-artifact@v4
        with:
          name: app-bundle
          path: |
            dist/app.bin
            dist/greeting.txt
  verify:
    name: Verify Artifact
    needs: build
    runs-on: local
    steps:
      - uses: actions/checkout@v4
      - name: Download build artifact from upstream job
        uses: actions/download-artifact@v4
        with:
          name: app-bundle
          path: downloaded
      - name: Verify downloaded files match
        run: grep -q "hello-from-composite" downloaded/dist/greeting.txt && test -s downloaded/dist/app.bin
"#;
    let head_sha = commit_file(
        &repo_dir,
        ".github/workflows/ci.yml",
        workflow_yml.as_bytes(),
        "add yaml workflow",
    );

    let (app, token) = test_app_at(repo_dir.clone(), true);
    let auth = format!("Bearer {token}");

    let repos_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/repos")
                .header("Authorization", &auth)
                .body(Body::empty())
                .expect("req"),
        )
        .await
        .expect("repos");
    let repos_json: serde_json::Value = serde_json::from_slice(
        &repos_resp
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes(),
    )
    .expect("json");
    let repo_id = repos_json["repositories"][0]["id"]
        .as_str()
        .expect("repo id")
        .to_owned();

    // 1. Verify YAML workflow listing
    let wf_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/repos/{repo_id}/workflows"))
                .header("Authorization", &auth)
                .body(Body::empty())
                .expect("req"),
        )
        .await
        .expect("workflows");
    assert_eq!(wf_resp.status(), StatusCode::OK);
    let wf_json: serde_json::Value = serde_json::from_slice(
        &wf_resp
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes(),
    )
    .expect("json");
    assert_eq!(wf_json["workflows"][0]["name"], "YAML Actions CI");
    assert_eq!(
        wf_json["workflows"][0]["jobs"]
            .as_array()
            .expect("jobs")
            .len(),
        2
    );

    // 2. Trigger workflow run and verify both jobs pass (including composite action + download-artifact)
    let trigger_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/repos/{repo_id}/workflow-runs"))
                .header("Authorization", &auth)
                .header("Content-Type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "expected_commit": head_sha,
                        "source_ref": "refs/heads/main",
                        "workflow_name": "YAML Actions CI"
                    }))
                    .expect("json"),
                ))
                .expect("req"),
        )
        .await
        .expect("trigger");
    let status = trigger_resp.status();
    let run_bytes = trigger_resp
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    assert_eq!(
        status,
        StatusCode::OK,
        "trigger failed: {}",
        String::from_utf8_lossy(&run_bytes)
    );
    let run_json: serde_json::Value = serde_json::from_slice(&run_bytes).expect("json");
    assert_eq!(run_json["status"], "passed", "run_json: {run_json}");
    let run_id = run_json["run_id"].as_str().expect("run_id").to_owned();

    // 3. Download binary artifact generated during workflow run and verify exact bytes & SHA-256
    let dl_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/api/v1/repos/{repo_id}/workflow-runs/{run_id}/artifacts/app-bundle/download?path=dist/app.bin"
                ))
                .header("Authorization", &auth)
                .body(Body::empty())
                .expect("req"),
        )
        .await
        .expect("download");
    assert_eq!(dl_resp.status(), StatusCode::OK);
    let expected_bin = b"\x89PNG\r\n\x1a\nBINARY_PAYLOAD";
    let expected_sha = hex::encode(Sha256::digest(expected_bin));
    assert_eq!(
        dl_resp
            .headers()
            .get("x-artifact-sha256")
            .and_then(|v| v.to_str().ok()),
        Some(expected_sha.as_str())
    );
    let dl_bytes = dl_resp
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    assert_eq!(dl_bytes.as_ref(), expected_bin);

    // 4. Upload a general binary artifact via HTTP PUT and download it back
    let custom_bin: Vec<u8> = (0..=255u8).collect();
    let custom_sha = hex::encode(Sha256::digest(&custom_bin));
    let up_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!(
                    "/api/v1/repos/{repo_id}/workflow-runs/{run_id}/artifacts/manual-upload?path=reports/raw.bin"
                ))
                .header("Authorization", &auth)
                .header("Content-Type", "application/octet-stream")
                .body(Body::from(custom_bin.clone()))
                .expect("req"),
        )
        .await
        .expect("upload");
    assert_eq!(up_resp.status(), StatusCode::CREATED);

    let dl2_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/api/v1/repos/{repo_id}/workflow-runs/{run_id}/artifacts/manual-upload/download?path=reports/raw.bin"
                ))
                .header("Authorization", &auth)
                .body(Body::empty())
                .expect("req"),
        )
        .await
        .expect("download manual");
    assert_eq!(dl2_resp.status(), StatusCode::OK);
    assert_eq!(
        dl2_resp
            .headers()
            .get("x-artifact-sha256")
            .and_then(|v| v.to_str().ok()),
        Some(custom_sha.as_str())
    );
    let dl2_bytes = dl2_resp
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    assert_eq!(dl2_bytes.as_ref(), custom_bin.as_slice());
}

#[tokio::test]
async fn yaml_workflow_rejects_anchors_aliases_tags_and_unsupported_remote_uses_fail_closed() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo_dir = tmp.path().join("repo");
    init_temp_repo(&repo_dir);

    let anchor_yml = r#"name: Anchor Bomb
on: push
jobs:
  build: &a
    runs-on: local
    steps:
      - run: echo ok
  copy: *a
"#;
    commit_file(
        &repo_dir,
        ".github/workflows/bad.yml",
        anchor_yml.as_bytes(),
        "add anchor yaml",
    );

    let (app, token) = test_app_at(repo_dir.clone(), true);
    let auth = format!("Bearer {token}");
    let repos_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/repos")
                .header("Authorization", &auth)
                .body(Body::empty())
                .expect("req"),
        )
        .await
        .expect("repos");
    let repos_json: serde_json::Value = serde_json::from_slice(
        &repos_resp
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes(),
    )
    .expect("json");
    let repo_id = repos_json["repositories"][0]["id"]
        .as_str()
        .expect("repo id")
        .to_owned();

    let wf_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/repos/{repo_id}/workflows"))
                .header("Authorization", &auth)
                .body(Body::empty())
                .expect("req"),
        )
        .await
        .expect("workflows");
    assert_eq!(wf_resp.status(), StatusCode::BAD_REQUEST);

    let remote_uses_yml = r#"name: Remote Uses
on: push
jobs:
  build:
    runs-on: local
    steps:
      - uses: evil-org/steal-secrets@v1
"#;
    commit_file(
        &repo_dir,
        ".github/workflows/bad.yml",
        remote_uses_yml.as_bytes(),
        "replace with unsupported remote uses",
    );
    let wf_resp2 = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/repos/{repo_id}/workflows"))
                .header("Authorization", &auth)
                .body(Body::empty())
                .expect("req"),
        )
        .await
        .expect("workflows");
    assert_eq!(wf_resp2.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn shared_cache_http_upload_and_download_across_runners_verifies_sha256() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let repo_dir = tmp.path().join("repo");
    init_temp_repo(&repo_dir);

    let (app, token) = test_app_at(repo_dir, true);
    let auth = format!("Bearer {token}");
    let repos_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/repos")
                .header("Authorization", &auth)
                .body(Body::empty())
                .expect("req"),
        )
        .await
        .expect("repos");
    let repos_json: serde_json::Value = serde_json::from_slice(
        &repos_resp
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes(),
    )
    .expect("json");
    let repo_id = repos_json["repositories"][0]["id"]
        .as_str()
        .expect("repo id")
        .to_owned();

    let file_bytes = b"compiled-rlib-content-2026";
    let file_hex = hex::encode(file_bytes);
    let file_sha256 = hex::encode(Sha256::digest(file_bytes));

    // Tampered SHA-256 must be rejected fail-closed
    let bad_put = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!(
                    "/api/v1/repos/{repo_id}/caches/shared-target-v1"
                ))
                .header("Authorization", &auth)
                .header("Content-Type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "source_job_id": "runner-node-a",
                        "files": {
                            "target/release/libapp.rlib": {
                                "hex": file_hex,
                                "sha256": "0000000000000000000000000000000000000000000000000000000000000000"
                            }
                        }
                    }))
                    .expect("json"),
                ))
                .expect("req"),
        )
        .await
        .expect("bad put");
    assert_eq!(bad_put.status(), StatusCode::BAD_REQUEST);

    // Valid upload succeeds
    let good_put = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/v1/repos/{repo_id}/caches/shared-target-v1"))
                .header("Authorization", &auth)
                .header("Content-Type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "source_job_id": "runner-node-a",
                        "files": {
                            "target/release/libapp.rlib": {
                                "hex": file_hex,
                                "sha256": file_sha256
                            }
                        }
                    }))
                    .expect("json"),
                ))
                .expect("req"),
        )
        .await
        .expect("good put");
    assert_eq!(good_put.status(), StatusCode::CREATED);

    // Another runner downloads the shared cache bundle and verifies content & sha256
    let get_cache = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/api/v1/repos/{repo_id}/caches/shared-target-v1"))
                .header("Authorization", &auth)
                .body(Body::empty())
                .expect("req"),
        )
        .await
        .expect("get cache");
    assert_eq!(get_cache.status(), StatusCode::OK);
    let cache_json: serde_json::Value = serde_json::from_slice(
        &get_cache
            .into_body()
            .collect()
            .await
            .expect("body")
            .to_bytes(),
    )
    .expect("json");
    assert_eq!(cache_json["key"], "shared-target-v1");
    assert_eq!(
        cache_json["files"]["target/release/libapp.rlib"]["hex"],
        file_hex
    );
    assert_eq!(
        cache_json["files"]["target/release/libapp.rlib"]["sha256"],
        file_sha256
    );

    // Path traversal inside shared cache file map (`../escape.bin`) is rejected fail-closed (400 Bad Request)
    let trav_put = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/api/v1/repos/{repo_id}/caches/shared-target-v1"))
                .header("Authorization", &auth)
                .header("Content-Type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "source_job_id": "runner-node-a",
                        "files": {
                            "../escape.bin": {
                                "hex": file_hex,
                                "sha256": file_sha256
                            }
                        }
                    }))
                    .expect("json"),
                ))
                .expect("req"),
        )
        .await
        .expect("trav put");
    assert_eq!(trav_put.status(), StatusCode::BAD_REQUEST);
}
