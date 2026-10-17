//! A bare repository's symbolic HEAD is independent of existing worktree creation targets.
use crate::{GitEngine, GitEngineError, RepositoryHead, MAX_BRANCH_NAME_BYTES};
use gitust_core::CommitSha;
use serde::Serialize;
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    time::Duration,
};

#[derive(Debug, Clone, Serialize)]
pub struct RepositoryDefaultBranch {
    pub branch: String,
    pub ref_name: String,
    pub head_commit: CommitSha,
}
fn unavailable() -> GitEngineError {
    GitEngineError::HeadResolution("default branch data is unavailable".into())
}
fn canonical(full: &str) -> bool {
    full.strip_prefix("refs/heads/").is_some_and(|name| {
        !name.is_empty()
            && name.len() <= MAX_BRANCH_NAME_BYTES
            && !name.eq_ignore_ascii_case("agent")
            && !name
                .get(..6)
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case("agent/"))
    }) && <&gix::refs::FullNameRef>::try_from(full).is_ok()
}
fn symbolic_head(repo: &gix::Repository) -> Result<Option<String>, GitEngineError> {
    let head = repo.find_reference("HEAD").map_err(|_| unavailable())?;
    match head.target() {
        gix::refs::TargetRef::Symbolic(name) => {
            let full = std::str::from_utf8(name.as_bstr()).map_err(|_| unavailable())?;
            Ok(canonical(full).then(|| full.to_owned()))
        }
        gix::refs::TargetRef::Object(_) => Ok(None),
    }
}
impl GitEngine {
    /// Structural hint only. The server still requires current credentials, registration and ACL.
    pub fn inspect_changeable_default_branch(&self) -> Result<Option<String>, GitEngineError> {
        let repo = self.acquire_repo()?;
        if !repo.is_bare() {
            return Ok(None);
        }
        symbolic_head(&repo)
    }
    /// Omitted worktree bases require the actual committed symbolic default, never a main fallback.
    pub fn committed_default_branch(&self) -> Result<String, GitEngineError> {
        let repo = self.acquire_repo()?;
        let full = symbolic_head(&repo)?.ok_or(GitEngineError::DefaultBranchUnsupported)?;
        if !matches!(self.inspect_repository_head()?, RepositoryHead::Commit(_)) {
            return Err(GitEngineError::DefaultBranchUnsupported);
        }
        Ok(full[11..].into())
    }
    /// The caller holds its repository mutation guard across this operation. Target-tip validation
    /// is serialized with server writes; external Git writers must also be stopped/coordinated.
    /// Only HEAD itself uses a symbolic-target CAS. No branch tip, checkout or worktree metadata moves.
    pub fn change_default_branch_with_gate(
        &self,
        target_ref: &str,
        expected_current_ref: &str,
        expected_target_commit: &CommitSha,
        mut authorize: impl FnMut() -> Result<(), GitEngineError>,
    ) -> Result<RepositoryDefaultBranch, GitEngineError> {
        if !canonical(target_ref)
            || !canonical(expected_current_ref)
            || Self::validate_outbound_branch(&target_ref[11..])
                .map_or(true, |clean| clean != target_ref[11..])
        {
            return Err(GitEngineError::DefaultBranchInvalid);
        }
        let expected = gix::hash::ObjectId::from_hex(expected_target_commit.0.as_bytes())
            .map_err(|_| GitEngineError::DefaultBranchInvalid)?;
        if expected.is_null() {
            return Err(GitEngineError::DefaultBranchInvalid);
        }
        let _ownership = self.lock_worktree_service()?;
        let repo = self.acquire_repo()?;
        if !repo.is_bare() {
            return Err(GitEngineError::DefaultBranchUnsupported);
        }
        crate::config_guard::reject_replacement_refs(&repo)?;
        let current = self
            .inspect_changeable_default_branch()?
            .ok_or(GitEngineError::DefaultBranchUnsupported)?;
        self.inspect_repository_head()?;
        if current != expected_current_ref {
            return Err(GitEngineError::DefaultBranchConflict);
        }
        let reference = repo
            .try_find_reference(target_ref)
            .map_err(|_| unavailable())?
            .ok_or(GitEngineError::DefaultBranchConflict)?;
        let actual = reference
            .try_id()
            .ok_or(GitEngineError::DefaultBranchUnsupported)?
            .detach();
        if actual != expected {
            return Err(GitEngineError::DefaultBranchConflict);
        }
        use gix::prelude::HeaderExt;
        let header = repo.objects.header(actual).map_err(|_| unavailable())?;
        if header.kind() != gix::objs::Kind::Commit {
            return Err(GitEngineError::DefaultBranchUnsupported);
        }
        authorize()?;
        // Re-read the selected direct target after the authorization callback. The shared caller
        // guard coordinates Gitust mutations; a no-op gix edit would not retain a target ref lock.
        let reference = repo
            .try_find_reference(target_ref)
            .map_err(|_| unavailable())?
            .ok_or(GitEngineError::DefaultBranchConflict)?;
        if reference.try_id().map(|id| id.detach()) != Some(expected) {
            return Err(GitEngineError::DefaultBranchConflict);
        }
        let head_path = Self::common_git_dir(&repo).join("HEAD");
        let mut lock = gix::lock::File::acquire_to_update_resource(
            &head_path,
            gix::lock::acquire::Fail::AfterDurationWithBackoff(Duration::from_millis(100)),
            None,
        )
        .map_err(|error| match error {
            gix::lock::acquire::Error::PermanentlyLocked { .. } => {
                GitEngineError::DefaultBranchConflict
            }
            gix::lock::acquire::Error::Io(_) => unavailable(),
        })?;
        // gix-ref 0.50 reads the old value before acquiring HEAD.lock. Read it again under
        // the held lock instead; never dereference HEAD or rewrite the old branch's tip.
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let head = options.open(&head_path).map_err(|_| unavailable())?;
        let metadata = head.metadata().map_err(|_| unavailable())?;
        if !metadata.is_file() || metadata.len() > (MAX_BRANCH_NAME_BYTES + 32) as u64 {
            return Err(unavailable());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if metadata.nlink() != 1 {
                return Err(unavailable());
            }
        }
        let mut bytes = Vec::new();
        head.take((MAX_BRANCH_NAME_BYTES + 33) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| unavailable())?;
        let head = gix::refs::file::loose::Reference::try_from_path(
            "HEAD".try_into().map_err(|_| unavailable())?,
            &bytes,
        )
        .map_err(|_| unavailable())?;
        if head.target
            != gix::refs::Target::Symbolic(
                expected_current_ref.try_into().map_err(|_| unavailable())?,
            )
        {
            return Err(GitEngineError::DefaultBranchConflict);
        }
        authorize()?;
        if repo
            .try_find_reference(target_ref)
            .map_err(|_| unavailable())?
            .and_then(|reference| reference.try_id().map(|id| id.detach()))
            != Some(expected)
        {
            return Err(GitEngineError::DefaultBranchConflict);
        }
        lock.with_mut(|file| file.set_permissions(metadata.permissions()))
            .map_err(|_| unavailable())?;
        writeln!(lock, "ref: {target_ref}").map_err(|_| unavailable())?;
        lock.with_mut(|file| file.sync_all())
            .map_err(|_| unavailable())?;
        lock.commit().map_err(|_| unavailable())?;
        // A directory-sync failure after rename has an uncertain outcome: return an error,
        // preserve the actual HEAD, and let the caller refresh rather than undo a valid change.
        File::open(Self::common_git_dir(&repo))
            .and_then(|file| file.sync_all())
            .map_err(|_| unavailable())?;
        Ok(RepositoryDefaultBranch {
            branch: target_ref[11..].into(),
            ref_name: target_ref.into(),
            head_commit: CommitSha(expected.to_string()),
        })
    }
}
