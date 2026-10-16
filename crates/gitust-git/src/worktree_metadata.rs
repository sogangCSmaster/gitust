//! Authenticate both directions of Git linked-worktree administration pointers.
use crate::{io_err, GitEngineError};
use std::fs::{self, OpenOptions};
use std::io::Read;
use std::path::{Path, PathBuf};

fn read_metadata_file(path: &Path) -> Result<String, GitEngineError> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path).map_err(|e| io_err(path, &e))?;
    if !file.metadata().map_err(|e| io_err(path, &e))?.is_file() {
        return Err(GitEngineError::VerificationInvalid(
            "worktree metadata must be a regular file".into(),
        ));
    }
    let mut text = String::new();
    file.take(16385)
        .read_to_string(&mut text)
        .map_err(|e| io_err(path, &e))?;
    if text.len() > 16384 {
        return Err(GitEngineError::VerificationInvalid(
            "worktree metadata exceeds limit".into(),
        ));
    }
    Ok(text)
}

/// Resolves `<worktree>/.git` (`gitdir: <common>/worktrees/<name>`) to the admin dir path.
pub(crate) fn read_worktree_admin_dir(
    worktree_dir: &Path,
    common_git_dir: &Path,
) -> Result<PathBuf, GitEngineError> {
    let pointer = worktree_dir.join(".git");
    let text = read_metadata_file(&pointer)?;
    let raw = text
        .trim()
        .strip_prefix("gitdir:")
        .map(str::trim)
        .ok_or_else(|| GitEngineError::Io {
            path: pointer.display().to_string(),
            message: "not a linked worktree pointer".to_owned(),
        })?;
    let path = Path::new(raw);
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        worktree_dir.join(path)
    };
    let admin = fs::canonicalize(&path).map_err(|e| io_err(&path, &e))?;
    let expected_parent = fs::canonicalize(common_git_dir.join("worktrees"))
        .map_err(|e| io_err(common_git_dir, &e))?;
    let invalid = || {
        GitEngineError::VerificationInvalid(
            "linked worktree metadata does not match this repository and checkout".into(),
        )
    };
    if !fs::symlink_metadata(&pointer)
        .map_err(|e| io_err(&pointer, &e))?
        .is_file()
        || admin.parent() != Some(expected_parent.as_path())
    {
        return Err(invalid());
    }
    let reverse = read_metadata_file(&admin.join("gitdir"))?;
    let reverse = fs::canonicalize(admin.join(reverse.trim())).map_err(|e| io_err(&admin, &e))?;
    let expected_pointer = fs::canonicalize(&pointer).map_err(|e| io_err(&pointer, &e))?;
    let common = read_metadata_file(&admin.join("commondir"))?;
    let actual_common =
        fs::canonicalize(admin.join(common.trim())).map_err(|e| io_err(&admin, &e))?;
    let expected_common =
        fs::canonicalize(common_git_dir).map_err(|e| io_err(common_git_dir, &e))?;
    if reverse != expected_pointer || actual_common != expected_common {
        return Err(invalid());
    }
    Ok(admin)
}
