//! General binary/file workflow artifact upload/download (`GET/PUT .../artifacts/{name}`)
//! and multi-runner shared cache upload/download (`GET/PUT .../caches/{key}`).

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use axum::{
    extract::{Path as AxumPath, Query, Request, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use chrono::Utc;
use gitust_core::{AuthError, EnterpriseRole};
use gitust_git::{validate_workflow_relative_path, WorkflowArtifactDownloadSpec};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    auth::extract_principal_async,
    repository_reads::response,
    state::{check_repo_bola, internal_err, map_auth_err, AppState},
    workflow_cache_artifacts::{
        cache_bundle_path, cache_key_digest_hex, scan_shared_caches,
        verify_worktree_relative_no_symlinks, StoredCacheBundle, MAX_ARTIFACT_PREVIEW_CHARS,
        MAX_ARTIFACT_TOTAL_BYTES, MAX_CACHE_TOTAL_BYTES, MAX_SHARED_CACHE_ENTRIES,
    },
    workflows::{
        atomic_write_regular_file, bad_req, ensure_safe_dir, load_workflow_run_artifacts,
        load_workflow_runs, no_store_json, read_bounded_regular_file, save_workflow_run,
        unavailable, with_admin_repo_read, with_admin_repo_write, workflows_storage_root, ApiError,
        WorkflowArtifactEntry,
    },
};

const ARTIFACT_BLOBS_DIR_NAME: &str = "artifact-blobs";

#[derive(Debug, Default, Serialize, Deserialize)]
struct ArtifactBlobIndex {
    entries: BTreeMap<String, String>,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct ArtifactPathQuery {
    #[serde(default)]
    path: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SharedCacheFileInput {
    hex: String,
    sha256: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PutSharedCacheRequest {
    files: BTreeMap<String, SharedCacheFileInput>,
    #[serde(default)]
    source_job_id: Option<String>,
}

fn hex_sha256(bytes: &[u8]) -> String {
    let digest: [u8; 32] = Sha256::digest(bytes).into();
    hex::encode(digest)
}

fn validate_clean_run_id(raw_run_id: &str) -> Result<String, ApiError> {
    let parsed =
        Uuid::parse_str(raw_run_id.trim()).map_err(|_| bad_req("invalid workflow run_id"))?;
    if parsed.is_nil() {
        return Err(bad_req("invalid workflow run_id"));
    }
    Ok(parsed.hyphenated().to_string())
}

fn validate_artifact_or_cache_label<'a>(raw: &'a str, kind: &str) -> Result<&'a str, ApiError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.len() > 128 || trimmed.chars().any(char::is_control) {
        return Err(bad_req(format!("invalid {kind}")));
    }
    Ok(trimmed)
}

fn artifact_name_dir(
    common_git_dir: &Path,
    clean_run_id: &str,
    artifact_name: &str,
) -> Result<PathBuf, ApiError> {
    let root = workflows_storage_root(common_git_dir)?;
    let blobs_root = root.join(ARTIFACT_BLOBS_DIR_NAME);
    ensure_safe_dir(&blobs_root)?;
    let run_dir = blobs_root.join(clean_run_id);
    ensure_safe_dir(&run_dir)?;
    let name_dir = run_dir.join(hex_sha256(artifact_name.as_bytes()));
    ensure_safe_dir(&name_dir)?;
    Ok(name_dir)
}

pub(crate) fn save_artifact_blob(
    common_git_dir: &Path,
    clean_run_id: &str,
    artifact_name: &str,
    rel_path: &str,
    bytes: &[u8],
) -> Result<String, ApiError> {
    if bytes.len() as u64 > MAX_ARTIFACT_TOTAL_BYTES {
        return Err(bad_req("artifact exceeds 1 MiB limit"));
    }
    let clean_path = validate_workflow_relative_path(rel_path)
        .map_err(|_| bad_req("invalid artifact relative path"))?;
    let name_dir = artifact_name_dir(common_git_dir, clean_run_id, artifact_name)?;
    let path_hash = hex_sha256(clean_path.as_bytes());
    let blob_path = name_dir.join(format!("{path_hash}.bin"));
    atomic_write_regular_file(&name_dir, &blob_path, bytes)?;

    let idx_path = name_dir.join("index.json");
    let mut index = match read_bounded_regular_file(&idx_path, 64 * 1024)? {
        Some(raw) => serde_json::from_slice::<ArtifactBlobIndex>(&raw)
            .map_err(|_| unavailable("corrupted artifact blob index"))?,
        None => ArtifactBlobIndex::default(),
    };
    let content_sha = hex_sha256(bytes);
    index.entries.insert(clean_path, content_sha.clone());
    let idx_bytes = serde_json::to_vec(&index).map_err(unavailable)?;
    atomic_write_regular_file(&name_dir, &idx_path, &idx_bytes)?;
    Ok(content_sha)
}

fn load_artifact_blob(
    common_git_dir: &Path,
    clean_run_id: &str,
    artifact_name: &str,
    rel_path: &str,
    expected_sha256: Option<&str>,
) -> Result<(Vec<u8>, String), ApiError> {
    let clean_path = validate_workflow_relative_path(rel_path)
        .map_err(|_| bad_req("invalid artifact relative path"))?;
    let name_dir = artifact_name_dir(common_git_dir, clean_run_id, artifact_name)?;
    let path_hash = hex_sha256(clean_path.as_bytes());
    let blob_path = name_dir.join(format!("{path_hash}.bin"));
    let bytes =
        read_bounded_regular_file(&blob_path, MAX_ARTIFACT_TOTAL_BYTES)?.ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                "workflow artifact blob not found".into(),
            )
        })?;
    let actual_sha = hex_sha256(&bytes);
    if let Some(exp) = expected_sha256 {
        if exp != actual_sha {
            return Err(unavailable("workflow artifact SHA-256 integrity mismatch"));
        }
    }
    Ok((bytes, actual_sha))
}

pub(crate) fn restore_job_downloaded_artifacts(
    common_git_dir: &Path,
    clean_run_id: &str,
    wt_dir: &Path,
    downloads: &[WorkflowArtifactDownloadSpec],
) -> Result<(), String> {
    for dl in downloads {
        let name_dir =
            artifact_name_dir(common_git_dir, clean_run_id, &dl.name).map_err(|(_, m)| m)?;
        let idx_path = name_dir.join("index.json");
        let raw_idx = read_bounded_regular_file(&idx_path, 64 * 1024)
            .map_err(|(_, m)| m)?
            .ok_or_else(|| format!("downloaded artifact '{}' was not produced", dl.name))?;
        let index: ArtifactBlobIndex = serde_json::from_slice(&raw_idx)
            .map_err(|_| "corrupted artifact blob index".to_owned())?;
        if index.entries.is_empty() {
            return Err(format!("downloaded artifact '{}' is empty", dl.name));
        }
        for (rel_path, exp_sha) in &index.entries {
            let (bytes, _) = load_artifact_blob(
                common_git_dir,
                clean_run_id,
                &dl.name,
                rel_path,
                Some(exp_sha.as_str()),
            )
            .map_err(|(_, m)| m)?;
            let target_rel = if dl.path == "." {
                rel_path.clone()
            } else {
                format!("{}/{rel_path}", dl.path)
            };
            let target = verify_worktree_relative_no_symlinks(wt_dir, &target_rel)?;
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            verify_worktree_relative_no_symlinks(wt_dir, &target_rel)?;
            fs::write(&target, &bytes).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

pub(crate) async fn download_run_artifact(
    State(state): State<AppState>,
    AxumPath((raw_id, raw_run_id, raw_name)): AxumPath<(String, String, String)>,
    Query(query): Query<ArtifactPathQuery>,
    headers: HeaderMap,
) -> Response {
    let res = tokio::task::spawn_blocking(move || {
        let clean_run_id = validate_clean_run_id(&raw_run_id)?;
        let clean_name = validate_artifact_or_cache_label(&raw_name, "artifact name")?.to_owned();
        let req_path = match query
            .path
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(p) => Some(
                validate_workflow_relative_path(p)
                    .map_err(|_| bad_req("invalid artifact path query"))?,
            ),
            None => None,
        };
        with_admin_repo_read(&state, &raw_id, &headers, |_repo, _repo_id, common| {
            let artifacts = load_workflow_run_artifacts(common, &clean_run_id)?;
            let matched = artifacts
                .iter()
                .find(|a| a.name == clean_name && req_path.as_deref().is_none_or(|rp| a.path == rp))
                .ok_or_else(|| (StatusCode::NOT_FOUND, "workflow artifact not found".into()))?;
            load_artifact_blob(
                common,
                &clean_run_id,
                &matched.name,
                &matched.path,
                matched.sha256.as_deref(),
            )
        })
    })
    .await
    .map_err(internal_err)
    .and_then(|v| v);

    match res {
        Ok((bytes, sha256_hex)) => {
            let mut resp_headers = HeaderMap::new();
            resp_headers.insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static("application/octet-stream"),
            );
            resp_headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            // fail-open-ok: sha256_hex is hex::encode output (64 lowercase ASCII hex digits), always valid HTTP header value.
            if let Ok(hv) = HeaderValue::from_str(&sha256_hex) {
                resp_headers.insert("x-artifact-sha256", hv);
            }
            (StatusCode::OK, resp_headers, bytes).into_response()
        }
        Err(err) => response::<()>(Err(err)),
    }
}

pub(crate) async fn upload_run_artifact(
    State(state): State<AppState>,
    AxumPath((raw_id, raw_run_id, raw_name)): AxumPath<(String, String, String)>,
    Query(query): Query<ArtifactPathQuery>,
    request: Request,
) -> Response {
    let res = async move {
        let (parts, body) = request.into_parts();
        let initial = extract_principal_async(&parts.headers, &state.auth_store).await?;
        if initial.role != EnterpriseRole::SiteAdmin {
            return Err(map_auth_err(AuthError::AdminPrivilegeRequired));
        }
        // fail-open-ok: MAX_ARTIFACT_TOTAL_BYTES is a compile-time constant (1 MiB) that fits in usize.
        let max_len = usize::try_from(MAX_ARTIFACT_TOTAL_BYTES).unwrap_or(1024 * 1024);
        let bytes = axum::body::to_bytes(body, max_len).await.map_err(|_| {
            (
                StatusCode::PAYLOAD_TOO_LARGE,
                "artifact exceeds 1 MiB limit".into(),
            )
        })?;
        let clean_run_id = validate_clean_run_id(&raw_run_id)?;
        let clean_name = validate_artifact_or_cache_label(&raw_name, "artifact name")?.to_owned();
        let raw_rel_path = match query
            .path
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(p) => p,
            None => &clean_name,
        };
        let clean_path = validate_workflow_relative_path(raw_rel_path)
            .map_err(|_| bad_req("invalid artifact path"))?;

        tokio::task::spawn_blocking(move || {
            let state_cl = state.clone();
            with_admin_repo_write(
                &state_cl,
                &raw_id,
                &parts.headers,
                initial.user_id,
                |caller, repo| {
                    let repo_id = repo.access.repo_id;
                    let exp_id = if repo.is_default {
                        state.repo_id
                    } else {
                        repo_id
                    };
                    check_repo_bola(&state.auth_store, exp_id, repo.access, caller, repo_id)?;
                    let _owner = repo.engine.lock_worktree_service().map_err(unavailable)?;
                    let common = repo
                        .engine
                        .repository_common_directory()
                        .map_err(unavailable)?;
                    let runs = load_workflow_runs(&common, repo_id)?;
                    let mut target_run = runs
                        .into_iter()
                        .find(|r| r.run_id == clean_run_id)
                        .ok_or_else(|| (StatusCode::NOT_FOUND, "workflow run not found".into()))?;
                    let sha256_hex = save_artifact_blob(
                        &common,
                        &clean_run_id,
                        &clean_name,
                        &clean_path,
                        &bytes,
                    )?;
                    let secret_values: Vec<String> = state
                        .auth_store
                        .resolve_workflow_secrets(caller, repo.access)
                        .map_err(map_auth_err)?
                        .into_values()
                        .collect();
                    let raw_preview: String = String::from_utf8_lossy(&bytes)
                        .chars()
                        .take(MAX_ARTIFACT_PREVIEW_CHARS)
                        .collect();
                    let preview = crate::workflows::redact_secrets(&raw_preview, &secret_values);
                    let entry = WorkflowArtifactEntry {
                        name: clean_name.clone(),
                        path: clean_path.clone(),
                        size_bytes: bytes.len() as u64,
                        sha256: Some(sha256_hex.clone()),
                        content_preview: Some(preview),
                    };
                    if let Some(job) = target_run.jobs.first_mut() {
                        job.artifacts
                            .retain(|a| !(a.name == clean_name && a.path == clean_path));
                        job.artifacts.push(entry);
                    }
                    save_workflow_run(&common, repo_id, &target_run)?;
                    Ok((
                        StatusCode::CREATED,
                        Json(json!({
                            "name": clean_name,
                            "path": clean_path,
                            "size_bytes": bytes.len(),
                            "sha256": sha256_hex,
                        })),
                    ))
                },
            )
        })
        .await
        .map_err(internal_err)?
    }
    .await;

    match res {
        Ok(reply) => reply.into_response(),
        Err(err) => response::<()>(Err(err)),
    }
}

fn compute_cache_bundle_sha256(files: &BTreeMap<String, String>) -> String {
    let mut hasher = Sha256::new();
    for (k, v) in files {
        hasher.update(k.as_bytes());
        hasher.update(b"\0");
        hasher.update(v.as_bytes());
        hasher.update(b"\n");
    }
    let digest: [u8; 32] = hasher.finalize().into();
    hex::encode(digest)
}

pub(crate) async fn put_shared_cache_bundle(
    State(state): State<AppState>,
    AxumPath((raw_id, raw_key)): AxumPath<(String, String)>,
    request: Request,
) -> Response {
    let res = async move {
        let (parts, body) = request.into_parts();
        let initial = extract_principal_async(&parts.headers, &state.auth_store).await?;
        if initial.role != EnterpriseRole::SiteAdmin {
            return Err(map_auth_err(AuthError::AdminPrivilegeRequired));
        }
        let clean_key = validate_artifact_or_cache_label(&raw_key, "cache key")?.to_owned();
        // fail-open-ok: MAX_CACHE_TOTAL_BYTES * 3 is a compile-time constant (12 MiB) that fits in usize.
        let max_body = usize::try_from(MAX_CACHE_TOTAL_BYTES * 3).unwrap_or(12 * 1024 * 1024);
        let bytes = axum::body::to_bytes(body, max_body).await.map_err(|_| {
            (
                StatusCode::PAYLOAD_TOO_LARGE,
                "cache bundle exceeds size limit".into(),
            )
        })?;
        let req: PutSharedCacheRequest =
            serde_json::from_slice(&bytes).map_err(|e| bad_req(format!("invalid JSON: {e}")))?;
        if req.files.is_empty() || req.files.len() > 128 {
            return Err(bad_req("cache bundle must contain 1..=128 files"));
        }
        let mut clean_files = BTreeMap::new();
        let mut total_bytes = 0u64;
        for (rel_path, file_in) in req.files {
            let clean_path = validate_workflow_relative_path(&rel_path)
                .map_err(|_| bad_req("invalid cache file path"))?;
            let decoded =
                hex::decode(&file_in.hex).map_err(|_| bad_req("invalid hex in cache file"))?;
            let actual_sha = hex_sha256(&decoded);
            if actual_sha != file_in.sha256.trim().to_ascii_lowercase() {
                return Err(bad_req("cache file SHA-256 mismatch"));
            }
            total_bytes = total_bytes.saturating_add(decoded.len() as u64);
            if total_bytes > MAX_CACHE_TOTAL_BYTES {
                return Err(bad_req("cache bundle exceeds 4 MiB limit"));
            }
            clean_files.insert(clean_path, hex::encode(decoded));
        }
        let bundle_sha256 = compute_cache_bundle_sha256(&clean_files);
        let files_count = clean_files.len();
        let source_job_id = req.source_job_id;

        tokio::task::spawn_blocking(move || {
            let state_cl = state.clone();
            with_admin_repo_write(
                &state_cl,
                &raw_id,
                &parts.headers,
                initial.user_id,
                |caller, repo| {
                    let repo_id = repo.access.repo_id;
                    let exp_id = if repo.is_default {
                        state.repo_id
                    } else {
                        repo_id
                    };
                    check_repo_bola(&state.auth_store, exp_id, repo.access, caller, repo_id)?;
                    let _owner = repo.engine.lock_worktree_service().map_err(unavailable)?;
                    let common = repo
                        .engine
                        .repository_common_directory()
                        .map_err(unavailable)?;
                    // fail-open-ok: UTC epoch timestamp is non-negative.
                    let created_at_epoch_secs = u64::try_from(Utc::now().timestamp()).unwrap_or(0);
                    let bundle = StoredCacheBundle {
                        version: 1,
                        key: clean_key.clone(),
                        files: clean_files,
                        created_at_epoch_secs,
                        source_job_id,
                    };
                    let payload = serde_json::to_vec(&bundle).map_err(unavailable)?;
                    let bundle_path = cache_bundle_path(&common, &clean_key)?;
                    let dir = bundle_path
                        .parent()
                        .ok_or_else(|| unavailable("invalid cache dir"))?;
                    atomic_write_regular_file(dir, &bundle_path, &payload)?;
                    let all = scan_shared_caches(&common)?;
                    for stale in all.into_iter().skip(MAX_SHARED_CACHE_ENTRIES) {
                        let stale_path = dir.join(format!("{}.json", stale.digest_sha256));
                        // fail-open-ok: best-effort pruning of oldest cache bundles beyond cap.
                        let _ = fs::remove_file(&stale_path);
                    }
                    Ok((
                        StatusCode::CREATED,
                        Json(json!({
                            "key": clean_key,
                            "digest_sha256": cache_key_digest_hex(&clean_key),
                            "bundle_sha256": bundle_sha256,
                            "size_bytes": total_bytes,
                            "files_count": files_count,
                        })),
                    ))
                },
            )
        })
        .await
        .map_err(internal_err)?
    }
    .await;

    match res {
        Ok(reply) => reply.into_response(),
        Err(err) => response::<()>(Err(err)),
    }
}

pub(crate) async fn get_shared_cache_bundle(
    State(state): State<AppState>,
    AxumPath((raw_id, raw_key)): AxumPath<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let res = tokio::task::spawn_blocking(move || {
        let clean_key = validate_artifact_or_cache_label(&raw_key, "cache key")?.to_owned();
        with_admin_repo_read(&state, &raw_id, &headers, |_repo, _repo_id, common| {
            let bundle_path = cache_bundle_path(common, &clean_key)?;
            let raw = read_bounded_regular_file(&bundle_path, MAX_CACHE_TOTAL_BYTES * 3)?
                .ok_or_else(|| (StatusCode::NOT_FOUND, "shared cache key not found".into()))?;
            let bundle: StoredCacheBundle = serde_json::from_slice(&raw)
                .map_err(|_| unavailable("corrupted shared cache bundle"))?;
            if bundle.version != 1 || bundle.key != clean_key {
                return Err(unavailable("shared cache key mismatch"));
            }
            let mut out_files = BTreeMap::new();
            for (rel_path, hex_val) in &bundle.files {
                let decoded = hex::decode(hex_val)
                    .map_err(|_| unavailable("corrupted shared cache file hex"))?;
                out_files.insert(
                    rel_path.clone(),
                    json!({
                        "hex": hex_val,
                        "sha256": hex_sha256(&decoded),
                    }),
                );
            }
            let bundle_sha256 = compute_cache_bundle_sha256(&bundle.files);
            Ok(json!({
                "key": bundle.key,
                "digest_sha256": cache_key_digest_hex(&clean_key),
                "bundle_sha256": bundle_sha256,
                "files": out_files,
                "source_job_id": bundle.source_job_id,
                "created_at_epoch_secs": bundle.created_at_epoch_secs,
            }))
        })
    })
    .await
    .map_err(internal_err)
    .and_then(|v| v);
    no_store_json(res)
}
