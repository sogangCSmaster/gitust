//! Keep the real index locked and unchanged until the branch CAS succeeds.
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use crate::worktree_commit_paths::invalid;
use crate::{git_command, io_err, GitEngineError};

const MAX_INDEX_BYTES: u64 = 32 * 1024 * 1024;

pub(crate) struct IndexTransaction {
    lock: File,
    lock_path: PathBuf,
    index_path: PathBuf,
    original: Vec<u8>,
    scratch: tempfile::TempDir,
}

impl IndexTransaction {
    pub fn new(dir: &Path, admin: &Path, parent: &str) -> Result<Self, GitEngineError> {
        let index_path = admin.join("index");
        let lock_path = admin.join("index.lock");
        let mut options = File::options();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let lock = options
            .open(&lock_path)
            .map_err(|e| io_err(&lock_path, &e))?;
        let scratch = match tempfile::Builder::new()
            .prefix("gitust-commit-")
            .tempdir_in(admin)
        {
            Ok(scratch) => scratch,
            Err(error) => {
                // fail-open-ok: cleanup only after staging allocation failed; original error is returned.
                let _ = fs::remove_file(&lock_path);
                return Err(io_err(admin, &error));
            }
        };
        let mut transaction = Self {
            lock,
            lock_path,
            index_path,
            original: Vec::new(),
            scratch,
        };
        crate::verification::inspect_index_flags(dir)?;
        let mut command = git_command(dir);
        command.args([
            "diff-index",
            "--cached",
            "--no-ext-diff",
            "--no-textconv",
            "--name-only",
            "--no-renames",
            "-z",
            parent,
            "--",
        ]);
        let (changed, truncated) =
            crate::process::run_capped(command, Duration::from_secs(10), 1024)?;
        if truncated || !changed.is_empty() {
            return Err(invalid(
                "existing staged/unmerged changes must be committed or unstaged explicitly first",
            ));
        }
        transaction.original = read_index(&transaction.index_path)?;
        let mut command = transaction.command(dir);
        command.args(["read-tree", parent]);
        transaction.run(command, None, 0)?;
        Ok(transaction)
    }

    pub fn staging_index(&self) -> PathBuf {
        self.scratch.path().join("index")
    }

    fn command(&self, dir: &Path) -> Command {
        let mut command = git_command(dir);
        command
            .args([
                "-c",
                "core.splitIndex=false",
                "-c",
                "index.sparse=false",
                "-c",
                "index.version=2",
            ])
            .env("GIT_INDEX_FILE", self.scratch.path().join("index"));
        command
    }

    fn run(
        &self,
        command: Command,
        input: Option<Vec<u8>>,
        cap: usize,
    ) -> Result<Vec<u8>, GitEngineError> {
        let (bytes, truncated) =
            crate::process::run_capped_with_input(command, Duration::from_secs(10), cap, input)?;
        if truncated {
            return Err(invalid("Git commit plumbing output exceeded limit"));
        }
        Ok(bytes)
    }

    pub fn prepare_tree(
        &mut self,
        dir: &Path,
        entries: Vec<u8>,
    ) -> Result<gix::ObjectId, GitEngineError> {
        let mut command = self.command(dir);
        command.args(["update-index", "-z", "--index-info"]);
        self.run(command, Some(entries), 0)?;
        let mut command = self.command(dir);
        command.arg("write-tree");
        let output = self.run(command, None, 128)?;
        let text = std::str::from_utf8(&output).map_err(|_| invalid("invalid tree SHA"))?;
        let tree = gix::ObjectId::from_hex(text.trim().as_bytes())
            .map_err(|_| invalid("invalid tree SHA"))?;
        let updated = read_index(&self.scratch.path().join("index"))?;
        self.lock
            .write_all(&updated)
            .map_err(|e| io_err(&self.lock_path, &e))?;
        self.lock
            .sync_all()
            .map_err(|e| io_err(&self.lock_path, &e))?;
        Ok(tree)
    }

    pub fn validate(&self) -> Result<(), GitEngineError> {
        if read_index(&self.index_path)? != self.original || !self.owns_lock() {
            return Err(invalid("worktree index changed while preparing commit"));
        }
        Ok(())
    }

    fn owns_lock(&self) -> bool {
        match (fs::symlink_metadata(&self.lock_path), self.lock.metadata()) {
            #[cfg(unix)]
            (Ok(path), Ok(file)) => {
                use std::os::unix::fs::MetadataExt;
                path.is_file()
                    && !path.file_type().is_symlink()
                    && path.dev() == file.dev()
                    && path.ino() == file.ino()
            }
            #[cfg(not(unix))]
            (Ok(path), Ok(_)) => path.is_file() && !path.file_type().is_symlink(),
            _ => false,
        }
    }

    pub fn publish(&self) -> Result<(), GitEngineError> {
        if !self.owns_lock() {
            return Err(invalid("worktree index lock changed after branch commit"));
        }
        fs::rename(&self.lock_path, &self.index_path).map_err(|e| io_err(&self.index_path, &e))?;
        let admin = self
            .index_path
            .parent()
            .ok_or_else(|| invalid("index admin directory missing"))?;
        File::open(admin)
            .and_then(|dir| dir.sync_all())
            .map_err(|e| io_err(admin, &e))
    }
}

impl Drop for IndexTransaction {
    fn drop(&mut self) {
        if self.owns_lock() {
            // fail-open-ok: cleanup of only our own lock on cancellation/error; no ref update or success is inferred.
            let _ = fs::remove_file(&self.lock_path);
        }
    }
}

fn read_index(path: &Path) -> Result<Vec<u8>, GitEngineError> {
    let mut options = File::options();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path).map_err(|e| io_err(path, &e))?;
    let meta = file.metadata().map_err(|e| io_err(path, &e))?;
    if !meta.is_file() || meta.len() > MAX_INDEX_BYTES {
        return Err(invalid("worktree index must be a bounded regular file"));
    }
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    file.take(MAX_INDEX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| io_err(path, &e))?;
    if bytes.len() as u64 > MAX_INDEX_BYTES {
        return Err(invalid("worktree index exceeds commit limit"));
    }
    Ok(bytes)
}
