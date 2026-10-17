//! Durable evidence that a sandbox may still modify its checkout after its server dies.
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

use gitust_core::{AgentWorktree, SandboxRunId, WorktreeId};
use serde::{Deserialize, Serialize};

use crate::{GitEngine, GitEngineError, WorktreeServiceLock};

const DIRECTORY: &str = "gitust-active-executions";
const MAX_BYTES: u64 = 4096;
pub(crate) const MAX_MARKERS: usize = 1024;

pub(crate) fn invalid(reason: &str) -> GitEngineError {
    GitEngineError::ExecutionFence(reason.into())
}

pub(crate) fn quarantined() -> GitEngineError {
    invalid("an unconfirmed sandbox execution blocks repository changes; stop all previous sandbox processes and run the explicit sandbox recovery command")
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Marker {
    version: u32,
    fence_id: SandboxRunId,
    worktree_id: WorktreeId,
    branch: String,
}

impl Marker {
    fn valid(&self, path: &Path) -> bool {
        self.version == 1
            && !self.fence_id.0.is_nil()
            && !self.worktree_id.0.is_nil()
            && path.file_name().and_then(|name| name.to_str())
                == Some(format!("{}.json", self.fence_id.0).as_str())
            && GitEngine::validate_agent_branch_name(&self.branch)
                .is_ok_and(|branch| branch == self.branch)
    }
}

fn sync_directory(path: &Path) -> Result<(), GitEngineError> {
    fs::File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(|_| invalid("execution marker directory could not be synchronized"))
}

fn directory(common: &Path, create: bool) -> Result<Option<PathBuf>, GitEngineError> {
    let path = common.join(DIRECTORY);
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
        Ok(_) => {
            return Err(invalid(
                "execution marker directory is not a real directory",
            ))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && !create => {
            return Ok(None);
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                // Do not expose private host paths from storage errors to execution clients.
                Err(_error) => {
                    return Err(invalid("execution marker directory could not be created"))
                }
            }
        }
        Err(_error) => return Err(invalid("execution marker directory could not be inspected")),
    }
    let metadata = fs::symlink_metadata(&path)
        .map_err(|_| invalid("execution marker directory could not be inspected"))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(invalid(
            "execution marker directory is not a real directory",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o777 != 0o700 {
            return Err(invalid(
                "execution marker directory permissions must be private",
            ));
        }
    }
    if create {
        sync_directory(common)?;
    }
    Ok(Some(path))
}

fn paths(directory: &Path) -> Result<Vec<PathBuf>, GitEngineError> {
    let mut paths = Vec::new();
    let entries =
        fs::read_dir(directory).map_err(|_| invalid("execution markers could not be listed"))?;
    for entry in entries {
        if paths.len() >= MAX_MARKERS {
            return Err(invalid("execution marker count exceeds its limit"));
        }
        paths.push(
            entry
                .map_err(|_| invalid("execution marker entry could not be read"))?
                .path(),
        );
    }
    Ok(paths)
}

fn read(path: &Path) -> Result<Marker, GitEngineError> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options
        .open(path)
        .map_err(|_| invalid("execution marker could not be safely opened"))?;
    let metadata = file
        .metadata()
        .map_err(|_| invalid("execution marker could not be inspected"))?;
    if !metadata.is_file() || metadata.len() > MAX_BYTES {
        return Err(invalid("execution marker must be a bounded regular file"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if metadata.permissions().mode() & 0o777 != 0o600 || metadata.nlink() != 1 {
            return Err(invalid(
                "execution marker permissions or link count are invalid",
            ));
        }
    }
    let mut payload = Vec::new();
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut payload)
        .map_err(|_| invalid("execution marker could not be read"))?;
    if payload.len() as u64 > MAX_BYTES {
        return Err(invalid("execution marker exceeds its limit"));
    }
    let marker: Marker = serde_json::from_slice(&payload).map_err(|_| {
        invalid("execution marker metadata is corrupt; preserve it for manual inspection")
    })?;
    if !marker.valid(path) {
        return Err(invalid("execution marker identity or version is invalid"));
    }
    Ok(marker)
}

pub(crate) fn ensure_clear(common: &Path) -> Result<(), GitEngineError> {
    if let Some(directory) = directory(common, false)? {
        if !paths(&directory)?.is_empty() {
            return Err(quarantined());
        }
    }
    Ok(())
}

/// The caller may complete this only after confirming every command process has stopped.
/// An unresolved Drop deliberately preserves evidence and quarantines the current engine.
#[derive(Debug)]
pub struct ExecutionFence {
    service: WorktreeServiceLock,
    path: PathBuf,
    marker: Marker,
    completed: bool,
}

impl ExecutionFence {
    pub fn complete(mut self) -> Result<(), GitEngineError> {
        let parent = self.path.parent().ok_or_else(quarantined)?;
        let common = parent.parent().ok_or_else(quarantined)?;
        directory(common, false)?.ok_or_else(quarantined)?;
        if read(&self.path)? != self.marker {
            return Err(invalid("execution marker changed during the sandbox run"));
        }
        fs::remove_file(&self.path)
            .map_err(|_| invalid("completed execution marker could not be removed"))?;
        sync_directory(parent)?;
        self.completed = true;
        self.service.finish_fence();
        Ok(())
    }
}

impl Drop for ExecutionFence {
    fn drop(&mut self) {
        if !self.completed {
            self.service.quarantine();
        }
    }
}

impl GitEngine {
    /// Call on a blocking worker while holding the worktree operation guard, before any spawn.
    pub fn begin_execution_fence(
        &self,
        worktree: &AgentWorktree,
    ) -> Result<ExecutionFence, GitEngineError> {
        let service = self.lock_worktree_service()?;
        service.reserve_fence()?;
        let result = (|| {
            let repo = self.acquire_repo()?;
            let common = Self::common_git_dir(&repo);
            let directory = directory(&common, true)?.ok_or_else(quarantined)?;
            if paths(&directory)?.len() >= MAX_MARKERS {
                return Err(invalid("active execution marker limit exceeded"));
            }
            let marker = Marker {
                version: 1,
                fence_id: SandboxRunId::new(),
                worktree_id: worktree.id,
                branch: worktree.branch.clone(),
            };
            let path = directory.join(format!("{}.json", marker.fence_id.0));
            if !marker.valid(&path) {
                return Err(invalid("execution worktree binding is invalid"));
            }
            let payload = serde_json::to_vec(&marker)
                .map_err(|_| invalid("execution marker could not be encoded"))?;
            if payload.len() as u64 > MAX_BYTES {
                return Err(invalid("execution marker exceeds its limit"));
            }
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
            }
            let mut file = options
                .open(&path)
                .map_err(|_| invalid("execution marker could not be created"))?;
            file.write_all(&payload)
                .and_then(|_| file.sync_all())
                .map_err(|_| invalid("execution marker could not be persisted"))?;
            sync_directory(&directory)?;
            service.ensure_available()?;
            Ok((path, marker))
        })();
        match result {
            Ok((path, marker)) => Ok(ExecutionFence {
                service,
                path,
                marker,
                completed: false,
            }),
            Err(error) => {
                service.quarantine();
                Err(error)
            }
        }
    }

    /// The operator must first stop every old sandbox process. This does not inspect or kill PIDs.
    /// Refuses active service ownership and validates all evidence before removing any marker.
    pub fn recover_execution_fences_after_operator_confirmation(
        &self,
    ) -> Result<usize, GitEngineError> {
        let repo = self.acquire_repo()?;
        let common = Self::common_git_dir(&repo);
        let _service =
            self.acquire_service_lock(&common.join("gitust-worktree-service.lock"), true)?;
        let Some(directory) = directory(&common, false)? else {
            return Ok(0);
        };
        let markers = paths(&directory)?;
        for path in &markers {
            read(path)?;
        }
        for path in &markers {
            fs::remove_file(path)
                .map_err(|_| invalid("confirmed execution marker could not be removed"))?;
        }
        sync_directory(&directory)?;
        Ok(markers.len())
    }
}
