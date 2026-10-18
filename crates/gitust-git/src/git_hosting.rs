//! Ordinary secondary receive-pack never owns or updates a checkout.
use crate::{GitEngine, GitEngineError};
use std::{fs, path::Path};

fn unavailable() -> GitEngineError {
    GitEngineError::RepositoryIdentity(
        "ordinary Git hosting requires a bare repository without linked or managed worktrees"
            .into(),
    )
}
fn absent_or_only_ephemeral(
    path: &Path,
    ownership: &crate::WorktreeServiceLock,
) -> Result<(), GitEngineError> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            for entry in fs::read_dir(path).map_err(|_| unavailable())? {
                let entry = entry.map_err(|_| unavailable())?;
                let ft = entry.file_type().map_err(|_| unavailable())?;
                if !ft.is_dir() || ft.is_symlink() {
                    return Err(unavailable());
                }
                let Some(id) = entry
                    .file_name()
                    .to_str()
                    .and_then(gitust_core::WorktreeId::parse_str)
                else {
                    return Err(unavailable());
                };
                if !ownership.is_active_ephemeral_worktree(id) {
                    return Err(unavailable());
                }
            }
            Ok(())
        }
        _ => Err(unavailable()),
    }
}
impl GitEngine {
    /// Validate both at stopped configuration and under the selected mutation guard before RPC.
    pub fn validate_ordinary_git_service(&self) -> Result<(), GitEngineError> {
        let ownership = self.lock_worktree_service()?;
        let repo = gix::open(&self.repo_path).map_err(|_| unavailable())?;
        if repo.work_dir().is_some() {
            return Err(unavailable());
        }
        let common = Self::common_git_dir(&repo);
        absent_or_only_ephemeral(&common.join("worktrees"), &ownership)?;
        absent_or_only_ephemeral(&self.worktrees_base(&repo, &common), &ownership)
    }
}

/// Authorizing a command name must not follow a symbolic or filesystem alias into another ref.
/// Service ownership coordinates Gitust writers; unrelated host Git writers must be stopped.
pub(crate) fn validate_receive_targets(
    common: &Path,
    updates: &[crate::ReceivePackRefUpdate],
) -> Result<(), GitEngineError> {
    use std::{fs::OpenOptions, io::Read, time::Instant};
    let failure =
        || GitEngineError::InvalidRevision("unsupported receive-pack target layout".into());
    let started = Instant::now();
    let cap = crate::MAX_BRANCH_NAME_BYTES + 128;
    'update: for update in updates {
        let mut parent = common.to_owned();
        let mut components = update.refname.split('/').peekable();
        while let Some(component) = components.next() {
            if components.peek().is_none() {
                break;
            }
            if started.elapsed() >= crate::SMART_HTTP_TIMEOUT {
                return Err(failure());
            }
            parent.push(component);
            match fs::symlink_metadata(&parent) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue 'update,
                Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
                _ => return Err(failure()),
            }
        }
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let file = match options.open(common.join(&update.refname)) {
            Ok(file) => file,
            // Packed refs can contain only direct OIDs; the native receiver checks their old value.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            _ => return Err(failure()),
        };
        let metadata = file.metadata().map_err(|_| failure())?;
        if !metadata.is_file() || metadata.len() > cap as u64 {
            return Err(failure());
        }
        let mut bytes = Vec::new();
        file.take((cap + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| failure())?;
        if bytes.len() > cap {
            return Err(failure());
        }
        let reference = gix::refs::file::loose::Reference::try_from_path(
            update.refname.as_str().try_into().map_err(|_| failure())?,
            &bytes,
        )
        .map_err(|_| failure())?;
        if !matches!(reference.target, gix::refs::Target::Object(_)) {
            return Err(failure());
        }
    }
    Ok(())
}
