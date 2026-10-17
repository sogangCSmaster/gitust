//! Empty preparation is distinct from an unborn default ref: every ref must be absent.
use crate::{run_git, GitEngine, GitEngineError, RepositoryHead, WorktreeServiceLock};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedEmpty {
    pub branch: String,
}
fn invalid() -> GitEngineError {
    GitEngineError::RepositoryIdentity(
        "empty repository preparation or snapshot verification failed; preserve private provisioning state".into(),
    )
}
impl GitEngine {
    /// The caller must record uncertain preparation before the first mutating Git subprocess.
    /// Keep the returned service holder through registration, publication and session revocation.
    pub fn prepare_empty_repository(
        &self,
        branch: &str,
    ) -> Result<(PreparedEmpty, WorktreeServiceLock), GitEngineError> {
        Self::validate_empty_branch(branch)?;
        let ownership = self
            .lock_inbound_mirror_repository()
            .map_err(|_| invalid())?;
        self.require_no_refs()?;
        if !matches!(
            self.inspect_repository_head().map_err(|_| invalid())?,
            RepositoryHead::Unborn { .. }
        ) {
            return Err(invalid());
        }
        run_git(
            &self.repo_path,
            ["symbolic-ref", "HEAD", &format!("refs/heads/{branch}")],
        )
        .map_err(|_| invalid())?;
        let snapshot = self.inspect_prepared_empty(branch)?;
        Ok((snapshot, ownership))
    }
    /// A completed create retry must still be empty. Never reset a populated repository.
    pub fn verify_prepared_empty(
        &self,
        expected: &PreparedEmpty,
    ) -> Result<WorktreeServiceLock, GitEngineError> {
        let ownership = self.lock_worktree_service().map_err(|_| invalid())?;
        if &self.inspect_prepared_empty(&expected.branch)? != expected {
            return Err(invalid());
        }
        Ok(ownership)
    }
    fn validate_empty_branch(branch: &str) -> Result<(), GitEngineError> {
        if branch.len() > 32 * 1024
            || Self::validate_branch_name(branch).map_err(|_| invalid())? != branch
            || branch == "agent"
            || branch.starts_with("agent/")
        {
            return Err(invalid());
        }
        Ok(())
    }
    fn require_no_refs(&self) -> Result<(), GitEngineError> {
        let repo = self.acquire_repo().map_err(|_| invalid())?;
        if !repo.is_bare() {
            return Err(invalid());
        }
        let refs = repo.references().map_err(|_| invalid())?;
        // Iterate unpeeled refs: Git for-each-ref omits dangling symbolic refs.
        if let Some(reference) = refs.all().map_err(|_| invalid())?.next() {
            reference.map_err(|_| invalid())?;
            return Err(invalid());
        }
        Ok(())
    }
    fn inspect_prepared_empty(&self, branch: &str) -> Result<PreparedEmpty, GitEngineError> {
        Self::validate_empty_branch(branch)?;
        self.require_no_refs()?;
        if !matches!(self.inspect_repository_head().map_err(|_| invalid())?,
            RepositoryHead::Unborn { branch_name } if branch_name == branch)
        {
            return Err(invalid());
        }
        let target = run_git(&self.repo_path, ["symbolic-ref", "--no-recurse", "HEAD"])
            .map_err(|_| invalid())?;
        if target != format!("refs/heads/{branch}\n").as_bytes() {
            return Err(invalid());
        }
        Self::verify_git_fsck(&self.repo_path).map_err(|_| invalid())?;
        Ok(PreparedEmpty {
            branch: branch.into(),
        })
    }
}
