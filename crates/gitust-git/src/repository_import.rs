//! Import preparation operates only on an operator-owned private stage, never a final path.
use crate::{run_git, GitEngine, GitEngineError, WorktreeServiceLock};
use gitust_core::CommitSha;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedImport {
    pub branch: String,
    pub head: CommitSha,
    pub refs_digest: String,
}
fn invalid() -> GitEngineError {
    GitEngineError::RepositoryIdentity(
        "import Git preparation or snapshot verification failed; preserve the private import state"
            .into(),
    )
}
impl GitEngine {
    /// Callers must durably fence the entire preparation before calling this method.
    /// The returned strong ownership holder must survive identity/registration and publication.
    pub fn prepare_repository_import(
        &self,
        source: &str,
        branch: &str,
    ) -> Result<(PreparedImport, WorktreeServiceLock), GitEngineError> {
        let branch = Self::validate_branch_name(branch).map_err(|_| invalid())?;
        if branch == "agent" || branch.starts_with("agent/") {
            return Err(invalid());
        }
        let ownership = self
            .lock_inbound_mirror_repository()
            .map_err(|_| invalid())?;
        let outcome = self.sync_inbound_read_only(source).map_err(|_| invalid())?;
        if !outcome.skipped_branches.is_empty() || !outcome.skipped_tags.is_empty() {
            return Err(invalid());
        }
        let reference = format!("refs/heads/{branch}");
        run_git(
            &self.repo_path,
            ["rev-parse", "--verify", &format!("{reference}^{{commit}}")],
        )
        .map_err(|_| invalid())?;
        run_git(&self.repo_path, ["symbolic-ref", "HEAD", &reference]).map_err(|_| invalid())?;
        let snapshot = self.inspect_prepared_import(&branch)?;
        Ok((snapshot, ownership))
    }
    /// Checks the sealed stage again on retry; it never fetches newer upstream content.
    pub fn verify_prepared_import(
        &self,
        expected: &PreparedImport,
    ) -> Result<WorktreeServiceLock, GitEngineError> {
        let ownership = self.lock_worktree_service().map_err(|_| invalid())?;
        if &self.inspect_prepared_import(&expected.branch)? != expected {
            return Err(invalid());
        }
        Ok(ownership)
    }
    fn inspect_prepared_import(&self, branch: &str) -> Result<PreparedImport, GitEngineError> {
        let repo = self.acquire_repo().map_err(|_| invalid())?;
        if !repo.is_bare() {
            return Err(invalid());
        }
        let identity = self.inspect_head_identity().map_err(|_| invalid())?;
        if identity.branch_name != branch {
            return Err(invalid());
        }
        let refs = run_git(
            &self.repo_path,
            [
                "for-each-ref",
                "--sort=refname",
                "--format=%(refname)%00%(objectname)",
            ],
        )
        .map_err(|_| invalid())?;
        Self::verify_git_fsck(&self.repo_path).map_err(|_| invalid())?;
        Ok(PreparedImport {
            branch: branch.into(),
            head: identity.head_sha,
            refs_digest: format!("{:x}", Sha256::digest(refs)),
        })
    }
}
