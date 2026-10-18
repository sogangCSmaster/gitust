//! Symlink-safe workflow cache restore/save and artifact collection with mandatory secret redaction.

use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
};

use gitust_git::{validate_workflow_relative_path, WorkflowArtifactSpec, WorkflowCacheSpec};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::workflows::{
    atomic_write_regular_file, ensure_safe_dir, read_bounded_regular_file, redact_secrets,
    workflows_storage_root, ApiError, WorkflowArtifactEntry, CACHES_DIR_NAME,
};

const MAX_CACHE_TOTAL_BYTES: u64 = 4 * 1024 * 1024;
const MAX_ARTIFACT_TOTAL_BYTES: u64 = 1024 * 1024;
const MAX_CACHE_FILES: usize = 128;
const MAX_ARTIFACT_PREVIEW_CHARS: usize = 4096;

#[derive(Serialize, Deserialize)]
pub(super) struct StoredCacheBundle {
    pub(super) version: u32,
    pub(super) key: String,
    pub(super) files: BTreeMap<String, String>,
}

pub(super) fn cache_bundle_path(
    common_git_dir: &Path,
    cache_key: &str,
) -> Result<PathBuf, ApiError> {
    let caches_dir = workflows_storage_root(common_git_dir)?.join(CACHES_DIR_NAME);
    ensure_safe_dir(&caches_dir)?;
    let digest: [u8; 32] = Sha256::digest(cache_key.as_bytes()).into();
    Ok(caches_dir.join(format!("{}.json", hex::encode(digest))))
}

pub(super) fn verify_worktree_relative_no_symlinks(
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

pub(super) fn collect_paths(
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

pub(super) fn restore_job_cache(
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

pub(super) fn save_job_cache(
    common_git_dir: &Path,
    wt_dir: &Path,
    spec: &WorkflowCacheSpec,
) -> Result<(), String> {
    let collected = collect_paths(wt_dir, &spec.paths, MAX_CACHE_TOTAL_BYTES, true)?;
    if collected.is_empty() {
        return Ok(());
    }
    let files = BTreeMap::from_iter(collected.into_iter().map(|(k, v)| (k, hex::encode(v))));
    let bundle = StoredCacheBundle {
        version: 1,
        key: spec.key.clone(),
        files,
    };
    let payload = serde_json::to_vec(&bundle).map_err(|e| e.to_string())?;
    let bundle_path = cache_bundle_path(common_git_dir, &spec.key).map_err(|(_, m)| m)?;
    let dir = bundle_path
        .parent()
        .ok_or_else(|| "invalid cache dir".to_owned())?;
    atomic_write_regular_file(dir, &bundle_path, &payload).map_err(|(_, m)| m)
}

pub(super) fn collect_job_artifacts(
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
