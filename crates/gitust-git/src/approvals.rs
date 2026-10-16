//! Durable, exact-commit human approvals. No approval is inferred from branch ancestry alone.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use gitust_core::{CommitSha, WorktreeId};
use serde::{Deserialize, Serialize};

use crate::{io_err, GitEngine, GitEngineError};

const MAX_APPROVAL_BYTES: u64 = 8192;
const APPROVAL_DIR: &str = "gitust-approvals";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorktreeApproval {
    version: u32,
    pub worktree_id: WorktreeId,
    pub branch: String,
    pub base_branch: String,
    pub approved_commit: CommitSha,
    published_remote: Option<String>,
}

impl WorktreeApproval {
    #[must_use]
    pub fn published_to(&self, remote: &str) -> bool {
        self.published_remote.as_deref() == Some(remote)
    }
}

impl GitEngine {
    /// Requires the caller's worktree/repository mutation guards and human approval.
    /// Only a successful durable write is reported as an approved merge.
    pub fn approve_managed_worktree(
        &self,
        id: WorktreeId,
        branch: &str,
        base: &str,
    ) -> Result<CommitSha, GitEngineError> {
        self.approve_managed_worktree_inner(id, branch, base, None)
    }

    /// Server approvals must match both the verified checkout and the exact merge target.
    pub fn approve_verified_worktree(
        &self,
        id: WorktreeId,
        branch: &str,
        base: &str,
        expected: &CommitSha,
    ) -> Result<CommitSha, GitEngineError> {
        self.approve_managed_worktree_inner(id, branch, base, Some(expected))
    }

    fn approve_managed_worktree_inner(
        &self,
        id: WorktreeId,
        branch: &str,
        base: &str,
        expected: Option<&CommitSha>,
    ) -> Result<CommitSha, GitEngineError> {
        let _service = self.lock_worktree_service()?;
        let branch = Self::validate_agent_branch_name(branch)?;
        let base = Self::validate_outbound_branch(base)?;
        let managed = self.list_agent_worktrees()?.into_iter().any(|w| {
            w.branch == branch
                && w.worktree_dir.file_name().and_then(|s| s.to_str())
                    == Some(id.0.to_string().as_str())
        });
        if !managed {
            return Err(GitEngineError::WorktreeNotManaged(id.0.to_string()));
        }
        if self.load_worktree_approval(id)?.is_some() {
            return Err(approval_error("worktree already has a durable approval"));
        }
        // Validate storage before touching Git; a later I/O error still leaves the local merge
        // intact and requires explicit operator recovery. Never claim an unrecorded approval.
        self.approval_directory(true)?;
        let clean_commit = self.clean_worktree_commit(id, &branch)?;
        if let Some(expected) = expected {
            if clean_commit != *expected {
                return Err(GitEngineError::VerificationInvalid(
                    "checkout changed since verification; run verification again".into(),
                ));
            }
        }
        let approved_commit = self.merge_agent_branch_at(&branch, &base, Some(&clean_commit))?;
        let approval = WorktreeApproval {
            version: 1,
            worktree_id: id,
            branch,
            base_branch: base,
            approved_commit: approved_commit.clone(),
            published_remote: None,
        };
        self.write_worktree_approval(&approval).map_err(|err| approval_error(format!(
            "local merge succeeded at {}; approval persistence failed: {err}; explicitly review this SHA before recovery",
            approved_commit.0
        )))?;
        Ok(approved_commit)
    }

    /// Loads a bounded record without following symlinks. Missing records mean no approval;
    /// malformed, mismatched, unknown-version or unreadable records are errors.
    pub fn load_worktree_approval(
        &self,
        id: WorktreeId,
    ) -> Result<Option<WorktreeApproval>, GitEngineError> {
        let Some(dir) = self.approval_directory(false)? else {
            return Ok(None);
        };
        let path = dir.join(format!("{}.json", id.0));
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let file = match options.open(&path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(io_err(&path, &e)),
        };
        let meta = file.metadata().map_err(|e| io_err(&path, &e))?;
        if !meta.is_file() || meta.len() > MAX_APPROVAL_BYTES {
            return Err(approval_error(
                "approval record is not a bounded regular file",
            ));
        }
        let mut payload = Vec::new();
        file.take(MAX_APPROVAL_BYTES + 1)
            .read_to_end(&mut payload)
            .map_err(|e| io_err(&path, &e))?;
        if payload.len() as u64 > MAX_APPROVAL_BYTES {
            return Err(approval_error("approval record exceeds size limit"));
        }
        let record: WorktreeApproval = serde_json::from_slice(&payload)
            .map_err(|e| approval_error(format!("invalid approval record: {e}")))?;
        if record.version != 1
            || record.worktree_id != id
            || Self::validate_agent_branch_name(&record.branch)? != record.branch
            || Self::validate_outbound_branch(&record.base_branch)? != record.base_branch
        {
            return Err(approval_error(
                "unknown version or mismatched approval identity",
            ));
        }
        self.validate_approved_commit(&record.base_branch, &record.approved_commit)?;
        Ok(Some(record))
    }

    /// Publishes the stored SHA, then durably records the destination before returning success.
    /// The caller retains its per-worktree guard throughout; a disconnect cannot drop this work.
    pub fn publish_managed_worktree(
        &self,
        id: WorktreeId,
        remote: &str,
    ) -> Result<WorktreeApproval, GitEngineError> {
        let _service = self.lock_worktree_service()?;
        let mut approval = self
            .load_worktree_approval(id)?
            .ok_or_else(|| approval_error("worktree has no durable human approval"))?;
        let remote = Self::validate_mirror_remote_url(remote)?;
        self.push_approved_commit(&remote, &approval.base_branch, &approval.approved_commit)?;
        approval.published_remote = Some(remote);
        self.write_worktree_approval(&approval).map_err(|err| approval_error(format!(
            "remote accepted the approved commit but publication persistence failed: {err}; retry the same worktree"
        )))?;
        Ok(approval)
    }

    pub(crate) fn remove_worktree_approval(&self, id: WorktreeId) -> Result<(), GitEngineError> {
        let Some(dir) = self.approval_directory(false)? else {
            return Ok(());
        };
        let path = dir.join(format!("{}.json", id.0));
        match fs::remove_file(&path) {
            Ok(()) => sync_directory(&dir),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(io_err(&path, &e)),
        }
    }

    fn approval_directory(&self, create: bool) -> Result<Option<PathBuf>, GitEngineError> {
        let repo = self.acquire_repo()?;
        let dir = Self::common_git_dir(&repo).join(APPROVAL_DIR);
        match fs::symlink_metadata(&dir) {
            Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => {}
            Ok(_) => {
                return Err(approval_error(
                    "approval directory must be a real directory",
                ))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && !create => return Ok(None),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let mut builder = fs::DirBuilder::new();
                #[cfg(unix)]
                {
                    use std::os::unix::fs::DirBuilderExt;
                    builder.mode(0o700);
                }
                builder.create(&dir).map_err(|e| io_err(&dir, &e))?;
                sync_directory(Self::common_git_dir(&repo).as_path())?;
            }
            Err(e) => return Err(io_err(&dir, &e)),
        }
        Ok(Some(dir))
    }

    fn write_worktree_approval(&self, record: &WorktreeApproval) -> Result<(), GitEngineError> {
        let dir = self
            .approval_directory(true)?
            .ok_or_else(|| approval_error("approval directory missing"))?;
        let path = dir.join(format!("{}.json", record.worktree_id.0));
        let payload = serde_json::to_vec(record).map_err(|e| approval_error(e.to_string()))?;
        if payload.len() as u64 > MAX_APPROVAL_BYTES {
            return Err(approval_error("approval record exceeds size limit"));
        }
        let mut temp = tempfile::NamedTempFile::new_in(&dir).map_err(|e| io_err(&dir, &e))?;
        temp.write_all(&payload).map_err(|e| io_err(&path, &e))?;
        temp.as_file().sync_all().map_err(|e| io_err(&path, &e))?;
        temp.persist(&path).map_err(|e| io_err(&path, &e.error))?;
        sync_directory(&dir)
    }
}

fn sync_directory(path: &Path) -> Result<(), GitEngineError> {
    File::open(path)
        .and_then(|f| f.sync_all())
        .map_err(|e| io_err(path, &e))
}

fn approval_error(message: impl Into<String>) -> GitEngineError {
    GitEngineError::ApprovalState(message.into())
}
