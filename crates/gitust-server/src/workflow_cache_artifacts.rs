//! Symlink-safe workflow cache restore/save, shared build cache listing/eviction,
//! and artifact collection with mandatory secret redaction.

use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
};

use chrono::Utc;
use gitust_core::SharedCacheEntry;
use gitust_git::{validate_workflow_relative_path, WorkflowArtifactSpec, WorkflowCacheSpec};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::workflows::{
    atomic_write_regular_file, bad_req, ensure_safe_dir, read_bounded_regular_file, redact_secrets,
    unavailable, workflows_storage_root, ApiError, WorkflowArtifactEntry, CACHES_DIR_NAME,
};

const MAX_CACHE_TOTAL_BYTES: u64 = 4 * 1024 * 1024;
const MAX_ARTIFACT_TOTAL_BYTES: u64 = 1024 * 1024;
const MAX_CACHE_FILES: usize = 128;
const MAX_ARTIFACT_PREVIEW_CHARS: usize = 4096;
const MAX_SHARED_CACHE_ENTRIES: usize = 32;
const MAX_CACHE_DIR_SCAN_ENTRIES: usize = 128;

#[derive(Serialize, Deserialize)]
pub(crate) struct StoredCacheBundle {
    pub(crate) version: u32,
    pub(crate) key: String,
    pub(crate) files: BTreeMap<String, String>,
    #[serde(default)]
    pub(crate) created_at_epoch_secs: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) source_job_id: Option<String>,
}

pub(crate) fn cache_key_digest_hex(cache_key: &str) -> String {
    let digest: [u8; 32] = Sha256::digest(cache_key.as_bytes()).into();
    hex::encode(digest)
}

pub(crate) fn cache_bundle_path(
    common_git_dir: &Path,
    cache_key: &str,
) -> Result<PathBuf, ApiError> {
    let caches_dir = workflows_storage_root(common_git_dir)?.join(CACHES_DIR_NAME);
    ensure_safe_dir(&caches_dir)?;
    Ok(caches_dir.join(format!("{}.json", cache_key_digest_hex(cache_key))))
}

pub(crate) fn verify_worktree_relative_no_symlinks(
    wt_dir: &Path,
    rel_path: &str,
) -> Result<PathBuf, String> {
    let clean = validate_workflow_relative_path(rel_path).map_err(|e| e.to_string())?;
    let root_meta = fs::symlink_metadata(wt_dir).map_err(|e| e.to_string())?;
    if root_meta.file_type().is_symlink() || !root_meta.file_type().is_dir() {
        return Err("worktree root is not a regular directory".into());
    }
    let mut cur = wt_dir.to_path_buf();
    for seg in clean.split('/') {
        cur.push(seg);
        match fs::symlink_metadata(&cur) {
            Ok(m) if m.file_type().is_symlink() => {
                return Err(format!("symlink escape rejected at '{clean}'"))
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(cur)
}

pub(crate) fn collect_paths(
    wt_dir: &Path,
    paths: &[String],
    max_bytes: u64,
    skip_missing: bool,
) -> Result<Vec<(String, Vec<u8>)>, String> {
    let mut out = Vec::new();
    let mut total_bytes = 0u64;
    let mut queue: Vec<(String, usize)> = Vec::new();
    for rel in paths.iter().rev() {
        let full = verify_worktree_relative_no_symlinks(wt_dir, rel)?;
        if skip_missing && !full.exists() && fs::symlink_metadata(&full).is_err() {
            continue;
        }
        queue.push((rel.clone(), 0));
    }
    while let Some((rel, depth)) = queue.pop() {
        if depth > 8 || out.len() > MAX_CACHE_FILES {
            return Err("workflow directory recursion or file count limit exceeded".into());
        }
        let full = verify_worktree_relative_no_symlinks(wt_dir, &rel)?;
        let meta = fs::symlink_metadata(&full)
            .map_err(|_| format!("workflow path '{rel}' was not produced"))?;
        if meta.file_type().is_symlink() {
            return Err(format!("symlink escape rejected at '{rel}'"));
        }
        if meta.file_type().is_file() {
            if meta.len() > max_bytes || total_bytes.saturating_add(meta.len()) > max_bytes {
                return Err(format!("workflow file '{rel}' exceeds size limit"));
            }
            let mut buf = Vec::new();
            fs::File::open(&full)
                .map_err(|e| e.to_string())?
                .take(max_bytes + 1)
                .read_to_end(&mut buf)
                .map_err(|e| e.to_string())?;
            total_bytes = total_bytes.saturating_add(buf.len() as u64);
            if total_bytes > max_bytes {
                return Err("workflow total file size limit exceeded".into());
            }
            out.push((rel, buf));
            continue;
        }
        if !meta.file_type().is_dir() {
            return Err(format!("unsupported file type at '{rel}'"));
        }
        let mut entries = Vec::new();
        for entry_res in fs::read_dir(&full).map_err(|e| e.to_string())? {
            let name = entry_res
                .map_err(|e| e.to_string())?
                .file_name()
                .into_string()
                .map_err(|_| "non-UTF-8 filename in workflow path".to_owned())?;
            entries.push(name);
        }
        entries.sort_by(|a, b| b.cmp(a));
        for name in entries {
            queue.push((format!("{rel}/{name}"), depth + 1));
        }
    }
    Ok(out)
}

pub(crate) fn restore_job_cache(
    common_git_dir: &Path,
    wt_dir: &Path,
    spec: &WorkflowCacheSpec,
) -> Result<bool, String> {
    let bundle_path = cache_bundle_path(common_git_dir, &spec.key).map_err(|(_, m)| m)?;
    let max_raw = MAX_CACHE_TOTAL_BYTES * 3;
    let Some(bytes) = read_bounded_regular_file(&bundle_path, max_raw).map_err(|(_, m)| m)? else {
        return Ok(false);
    };
    let bundle: StoredCacheBundle =
        serde_json::from_slice(&bytes).map_err(|_| "corrupted workflow cache bundle".to_owned())?;
    if bundle.version != 1 || bundle.key != spec.key {
        return Err("workflow cache key mismatch".into());
    }
    let mut restored_bytes = 0u64;
    for (rel_path, bytes_hex) in &bundle.files {
        let target = verify_worktree_relative_no_symlinks(wt_dir, rel_path)?;
        let decoded =
            hex::decode(bytes_hex).map_err(|_| "corrupted workflow cache file hex".to_owned())?;
        restored_bytes = restored_bytes.saturating_add(decoded.len() as u64);
        if restored_bytes > MAX_CACHE_TOTAL_BYTES {
            return Err("workflow cache exceeds 4 MiB limit".into());
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        verify_worktree_relative_no_symlinks(wt_dir, rel_path)?;
        fs::write(&target, &decoded).map_err(|e| e.to_string())?;
    }
    Ok(true)
}

pub(crate) fn save_job_cache(
    common_git_dir: &Path,
    wt_dir: &Path,
    spec: &WorkflowCacheSpec,
    source_job_id: Option<&str>,
) -> Result<(), String> {
    let collected = collect_paths(wt_dir, &spec.paths, MAX_CACHE_TOTAL_BYTES, true)?;
    if collected.is_empty() {
        return Ok(());
    }
    let files = BTreeMap::from_iter(collected.into_iter().map(|(k, v)| (k, hex::encode(v))));
    // fail-open-ok: UTC epoch timestamp is non-negative.
    let created_at_epoch_secs = u64::try_from(Utc::now().timestamp()).unwrap_or(0);
    let bundle = StoredCacheBundle {
        version: 1,
        key: spec.key.clone(),
        files,
        created_at_epoch_secs,
        source_job_id: source_job_id.map(str::to_owned),
    };
    let payload = serde_json::to_vec(&bundle).map_err(|e| e.to_string())?;
    let bundle_path = cache_bundle_path(common_git_dir, &spec.key).map_err(|(_, m)| m)?;
    let dir = bundle_path
        .parent()
        .ok_or_else(|| "invalid cache dir".to_owned())?;
    atomic_write_regular_file(dir, &bundle_path, &payload).map_err(|(_, m)| m)?;
    let all = scan_shared_caches(common_git_dir).map_err(|(_, m)| m)?;
    for stale in all.into_iter().skip(MAX_SHARED_CACHE_ENTRIES) {
        let stale_path = dir.join(format!("{}.json", stale.digest_sha256));
        // fail-open-ok: best-effort pruning of oldest cache bundles beyond cap.
        let _ = fs::remove_file(&stale_path);
    }
    Ok(())
}

pub(crate) fn collect_job_artifacts(
    wt_dir: &Path,
    spec: &WorkflowArtifactSpec,
    secret_values: &[String],
) -> Result<Vec<WorkflowArtifactEntry>, String> {
    let mut out = Vec::new();
    for (path, bytes) in collect_paths(wt_dir, &spec.paths, MAX_ARTIFACT_TOTAL_BYTES, false)? {
        let raw: String = String::from_utf8_lossy(&bytes)
            .chars()
            .take(MAX_ARTIFACT_PREVIEW_CHARS)
            .collect();
        out.push(WorkflowArtifactEntry {
            name: redact_secrets(&spec.name, secret_values),
            path: redact_secrets(&path, secret_values),
            size_bytes: bytes.len() as u64,
            content_preview: Some(redact_secrets(&raw, secret_values)),
        });
    }
    Ok(out)
}

fn scan_shared_caches(common_git_dir: &Path) -> Result<Vec<SharedCacheEntry>, ApiError> {
    let caches_dir = workflows_storage_root(common_git_dir)?.join(CACHES_DIR_NAME);
    if !caches_dir.exists() && fs::symlink_metadata(&caches_dir).is_err() {
        return Ok(Vec::new());
    }
    ensure_safe_dir(&caches_dir)?;
    let mut entries = Vec::new();
    let mut scanned = 0usize;
    for item_res in fs::read_dir(&caches_dir).map_err(unavailable)? {
        scanned = scanned.saturating_add(1);
        if scanned > MAX_CACHE_DIR_SCAN_ENTRIES {
            return Err(unavailable("shared cache directory entry limit exceeded"));
        }
        let item = item_res.map_err(unavailable)?;
        let fname = item
            .file_name()
            .into_string()
            .map_err(|_| unavailable("non-UTF-8 cache filename"))?;
        if fname.starts_with(".tmp-") || !fname.ends_with(".json") {
            continue;
        }
        let stem = fname.trim_end_matches(".json");
        let full_path = item.path();
        let Some(bytes) = read_bounded_regular_file(&full_path, MAX_CACHE_TOTAL_BYTES * 3)? else {
            continue;
        };
        let bundle: StoredCacheBundle = serde_json::from_slice(&bytes)
            .map_err(|_| unavailable("corrupted shared cache bundle"))?;
        let expected_digest = cache_key_digest_hex(&bundle.key);
        if bundle.version != 1 || expected_digest != stem {
            return Err(unavailable("shared cache bundle digest mismatch"));
        }
        let mut size_bytes = 0u64;
        for hex_val in bundle.files.values() {
            size_bytes = size_bytes.saturating_add((hex_val.len() / 2) as u64);
        }
        entries.push(SharedCacheEntry {
            key: bundle.key,
            digest_sha256: expected_digest,
            size_bytes,
            files_count: bundle.files.len(),
            created_at_epoch_secs: bundle.created_at_epoch_secs,
            source_job_id: bundle.source_job_id,
        });
    }
    entries.sort_by(|a, b| {
        b.created_at_epoch_secs
            .cmp(&a.created_at_epoch_secs)
            .then_with(|| a.key.cmp(&b.key))
    });
    Ok(entries)
}

pub(crate) fn list_shared_caches(common_git_dir: &Path) -> Result<Vec<SharedCacheEntry>, ApiError> {
    let mut entries = scan_shared_caches(common_git_dir)?;
    entries.truncate(MAX_SHARED_CACHE_ENTRIES);
    Ok(entries)
}

pub(crate) fn evict_shared_cache(common_git_dir: &Path, cache_key: &str) -> Result<bool, ApiError> {
    let trimmed = cache_key.trim();
    if trimmed.is_empty() || trimmed.len() > 128 || trimmed.chars().any(char::is_control) {
        return Err(bad_req("invalid cache key"));
    }
    let bundle_path = cache_bundle_path(common_git_dir, trimmed)?;
    match fs::symlink_metadata(&bundle_path) {
        Ok(m) => {
            if m.file_type().is_symlink() || !m.file_type().is_file() {
                return Err(unavailable("unsafe shared cache entry"));
            }
            fs::remove_file(&bundle_path).map_err(unavailable)?;
            Ok(true)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(unavailable(e)),
    }
}
