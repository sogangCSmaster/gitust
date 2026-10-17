//! Host-side source reads retain the execution sandbox's private-file boundary.
use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

use axum::http::StatusCode;
use gitust_sandbox::PathAccess;

use crate::state::WorktreeRuntime;

type Error = (StatusCode, String);

#[cfg(unix)]
fn identity(meta: &std::fs::Metadata) -> (u64, u64) {
    use std::os::unix::fs::MetadataExt;
    (meta.dev(), meta.ino())
}

#[cfg(not(unix))]
fn identity(_: &std::fs::Metadata) -> (u64, u64) {
    (0, 0)
}

fn storage_error() -> Error {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        "cannot validate private commit inputs".into(),
    )
}

fn candidate(
    paths: &mut HashSet<PathBuf>,
    bytes: &mut usize,
    path: PathBuf,
) -> Result<bool, Error> {
    if paths.contains(&path) {
        return Ok(false);
    }
    let size = path.as_os_str().len();
    if paths.len() >= 1024 || size > 524_288 - *bytes {
        return Err((
            StatusCode::BAD_REQUEST,
            "commit input tree is too complex".into(),
        ));
    }
    *bytes += size;
    paths.insert(path);
    Ok(true)
}

pub(super) fn check(runtime: &WorktreeRuntime, paths: &[String]) -> Result<(), Error> {
    let root = std::fs::canonicalize(&runtime.worktree_dir).map_err(|_| storage_error())?;
    let mut private = Vec::new();
    let mut private_ids = HashSet::new();
    for rule in &runtime.sandbox_spec.rules {
        if rule.access != PathAccess::Hidden {
            continue;
        }
        let path = if rule.path.is_absolute() {
            rule.path.clone()
        } else {
            std::env::current_dir()
                .map_err(|_| storage_error())?
                .join(&rule.path)
        };
        let meta = match std::fs::symlink_metadata(&path) {
            Ok(meta) => Some(meta),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            // fail-open-ok: unreadable protected paths reject the commit without reading inputs.
            Err(_) => return Err(storage_error()),
        };
        // The broad hidden main/base directories are overridden by this own checkout's
        // writable rule. Files outside it still matter when linked into the checkout.
        if path.starts_with(&root) {
            let parent = std::fs::canonicalize(path.parent().ok_or_else(storage_error)?)
                .map_err(|_| storage_error())?;
            private.push(parent.join(path.file_name().ok_or_else(storage_error)?));
            if let Some(meta) = meta {
                private_ids.insert(identity(&meta));
            }
        } else if let Some(meta) = meta.filter(|meta| meta.is_file()) {
            private_ids.insert(identity(&meta));
        }
    }
    let mut candidates = HashSet::new();
    let mut candidate_bytes = 0;
    for path in paths {
        if Path::new(path)
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
        {
            return Err((
                StatusCode::BAD_REQUEST,
                "commit paths must be relative literal files".into(),
            ));
        }
        candidate(&mut candidates, &mut candidate_bytes, root.join(path))?;
        // Attribute files are also read by host-side normalization, so they must pass the
        // same policy even though they were not selected as commit payloads.
        candidate(
            &mut candidates,
            &mut candidate_bytes,
            root.join(".gitattributes"),
        )?;
        let mut directory = PathBuf::new();
        let components: Vec<_> = path.split('/').collect();
        for component in &components[..components.len().saturating_sub(1)] {
            directory.push(component);
            candidate(
                &mut candidates,
                &mut candidate_bytes,
                root.join(&directory).join(".gitattributes"),
            )?;
        }
    }
    for path in [
        runtime.common_git_dir.join("config"),
        runtime.common_git_dir.join("info/attributes"),
    ] {
        candidate(&mut candidates, &mut candidate_bytes, path)?;
    }
    let mut checked = HashSet::new();
    let mut checked_bytes = 0;
    for input in candidates {
        let mut current = input.as_path();
        loop {
            if candidate(&mut checked, &mut checked_bytes, current.to_path_buf())? {
                if private
                    .iter()
                    .any(|protected| current.starts_with(protected))
                {
                    return Err((
                        StatusCode::FORBIDDEN,
                        "commit input is hidden by the sandbox policy".into(),
                    ));
                }
                match std::fs::symlink_metadata(current) {
                    Ok(meta) if private_ids.contains(&identity(&meta)) => {
                        return Err((
                            StatusCode::FORBIDDEN,
                            "commit input is hidden by the sandbox policy".into(),
                        ));
                    }
                    Ok(meta) if meta.file_type().is_symlink() && current != input => {
                        return Err((
                            StatusCode::BAD_REQUEST,
                            "commit parent must not be a symlink".into(),
                        ));
                    }
                    Ok(_) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    // fail-open-ok: every other metadata error rejects the commit.
                    Err(_) => return Err(storage_error()),
                }
            }
            if current == root || !current.starts_with(&root) {
                break;
            }
            current = current.parent().ok_or_else(storage_error)?;
        }
    }
    Ok(())
}
