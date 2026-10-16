//! Server ownership survives restarts; legacy CLI worktrees remain administrator-managed.
use gitust_core::{UserId, WorktreeId};
use gitust_git::{GitEngineError, MaterializedWorktree};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Owner {
    version: u32,
    worktree_id: WorktreeId,
    branch: String,
    user_id: UserId,
}

fn error(e: impl std::fmt::Display) -> GitEngineError {
    GitEngineError::ApprovalState(format!("worktree owner record: {e}"))
}

pub(crate) fn persist(m: &MaterializedWorktree, user_id: UserId) -> Result<(), GitEngineError> {
    let dir = m.common_git_dir.join("gitust-owners");
    fs::create_dir_all(&dir).map_err(error)?;
    if fs::symlink_metadata(&dir)
        .map_err(error)?
        .file_type()
        .is_symlink()
    {
        return Err(error("owner directory must not be a symlink"));
    }
    let target = dir.join(format!("{}.json", m.worktree.id.0));
    let tmp = dir.join(format!("{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let mut file = options.open(&tmp).map_err(error)?;
        let bytes = serde_json::to_vec(&Owner {
            version: 1,
            worktree_id: m.worktree.id,
            branch: m.worktree.branch.clone(),
            user_id,
        })
        .map_err(error)?;
        file.write_all(&bytes).map_err(error)?;
        file.sync_all().map_err(error)?;
        fs::rename(&tmp, &target).map_err(error)?;
        fs::File::open(&dir)
            .and_then(|d| d.sync_all())
            .map_err(error)
    })();
    if result.is_err() {
        // fail-open-ok: only remove our temporary file; the original persistence error is returned.
        let _ = fs::remove_file(&tmp);
    }
    result
}

pub(crate) fn read(m: &MaterializedWorktree) -> Result<Option<UserId>, GitEngineError> {
    let dir = m.common_git_dir.join("gitust-owners");
    match fs::symlink_metadata(&dir) {
        Ok(meta) if !meta.is_dir() || meta.file_type().is_symlink() => {
            return Err(error("invalid owner directory"))
        }
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(error(e)),
    }
    let path = dir.join(format!("{}.json", m.worktree.id.0));
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = match options.open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(error(e)),
    };
    let meta = file.metadata().map_err(error)?;
    if !meta.is_file() || meta.len() > 4096 {
        return Err(error("invalid owner record size or type"));
    }
    let mut bytes = Vec::new();
    file.take(4097).read_to_end(&mut bytes).map_err(error)?;
    let record: Owner = serde_json::from_slice(&bytes).map_err(error)?;
    if record.version != 1
        || record.worktree_id != m.worktree.id
        || record.branch != m.worktree.branch
    {
        return Err(error("owner identity does not match managed worktree"));
    }
    Ok(Some(record.user_id))
}
