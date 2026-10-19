use gitust_db::AuthStore;
use gitust_git::GitEngineError;
use gitust_sandbox::SandboxSpec;
use std::path::PathBuf;

/// Hide the configured auth DB and its lock even when it is outside the default .gitust directory.
pub(crate) fn sandbox_spec_for(
    m: &gitust_git::MaterializedWorktree,
    store: &AuthStore,
) -> Result<SandboxSpec, GitEngineError> {
    use gitust_sandbox::{PathAccess, PathRule};
    let mut spec = SandboxSpec::for_linked_worktree(
        &m.worktree_dir,
        &m.worktree_admin_dir,
        &m.common_git_dir,
        m.main_workdir.as_deref(),
        &m.worktrees_base,
        &m.sandbox_scratch_base,
    );
    if let Some(path) = store.persistence_path() {
        spec.rules.push(PathRule::new(path, PathAccess::Hidden));
        for suffix in [".lock", ".sqlite3", ".sqlite3-wal", ".sqlite3-shm"] {
            let mut sidecar = path.as_os_str().to_os_string();
            sidecar.push(suffix);
            spec.rules
                .push(PathRule::new(PathBuf::from(sidecar), PathAccess::Hidden));
        }
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            // Hide a dedicated auth directory too (including atomic-write temporary files).
            // Do not hide a configured shared repo/worktree ancestor in its entirety.
            let parent = std::fs::canonicalize(parent).map_err(|e| {
                GitEngineError::ApprovalState(format!("cannot resolve auth directory: {e}"))
            })?;
            if !m.worktree_dir.starts_with(&parent) && !m.common_git_dir.starts_with(&parent) {
                spec.rules.push(PathRule::new(parent, PathAccess::Hidden));
            }
        }
    }
    Ok(spec)
}
