//! Explicit agent commits without granting the sandbox write access to shared Git metadata.
use std::path::PathBuf;

use gitust_core::{CommitSha, WorktreeId};
use gix::refs::transaction::PreviousValue;
use gix::refs::Target;
use serde::Serialize;

use crate::worktree_commit_index::IndexTransaction;
use crate::worktree_commit_normalize::Normalization;
use crate::worktree_commit_paths::{self as sources, invalid, MAX_TOTAL_BYTES};
use crate::{ensure_reflog_identity, GitEngine, GitEngineError, WorktreeServiceLock};

#[derive(Debug, Clone, Serialize)]
pub struct WorktreeCommit {
    pub worktree_id: WorktreeId,
    pub branch: String,
    pub commit: CommitSha,
    pub parent: CommitSha,
}

/// Owns the index/service locks. Dropping an uncommitted preparation leaves refs/index unchanged.
/// Object writes during preparation are unreachable until the final compare-and-swap succeeds.
pub struct PreparedWorktreeCommit {
    result: WorktreeCommit,
    directory: PathBuf,
    admin: PathBuf,
    common: PathBuf,
    normalization: Normalization,
    index: IndexTransaction,
    _service: WorktreeServiceLock,
}

impl PreparedWorktreeCommit {
    /// Immutable identity of the object prepared before its ref compare-and-swap.
    pub fn result(&self) -> &WorktreeCommit {
        &self.result
    }
}

impl GitEngine {
    /// Call from a blocking worker while retaining the caller's per-worktree operation guard.
    pub fn prepare_worktree_commit(
        &self,
        id: WorktreeId,
        branch: &str,
        expected: &CommitSha,
        paths: &[String],
        message: &str,
        author_email: &str,
    ) -> Result<PreparedWorktreeCommit, GitEngineError> {
        sources::validate(paths, message, author_email)?;
        let branch = Self::validate_agent_branch_name(branch)?;
        let service = self.lock_worktree_service()?;
        let (directory, admin, common) = self.commit_checkout(id, &branch, expected)?;
        let mut index = IndexTransaction::new(&directory, &admin, &expected.0)?;
        let repo = self.acquire_repo()?;
        let parent = gix::ObjectId::from_hex(expected.0.as_bytes())
            .map_err(|_| invalid("invalid expected commit SHA"))?;
        let commit = repo
            .find_commit(parent)
            .map_err(|_| invalid("expected parent is not a commit"))?;
        let parent_tree = commit
            .tree()
            .map_err(|_| invalid("cannot read expected parent tree"))?;
        sources::validate_aliases(&directory, paths, &parent_tree)?;
        let mut selected = Vec::new();
        let mut total = 0;
        for path in paths {
            let tracked = parent_tree
                .lookup_entry_by_path(path)
                .map_err(|e| invalid(e.to_string()))?;
            if tracked
                .as_ref()
                .is_some_and(|entry| entry.mode().is_tree() || entry.mode().is_commit())
            {
                return Err(invalid(
                    "directory and submodule commit paths are not supported",
                ));
            }
            let source = sources::read(&directory, path)?;
            if let Some(source) = &source {
                total += source.bytes.len();
                if total > MAX_TOTAL_BYTES {
                    return Err(invalid("selected sources exceed the 32 MiB commit limit"));
                }
            } else if tracked.is_none() {
                return Err(invalid(
                    "a missing commit path must name a tracked file deletion",
                ));
            }
            selected.push((path, source));
        }
        let regular: Vec<_> = selected
            .iter()
            .filter_map(|(path, source)| {
                source
                    .as_ref()
                    .filter(|source| source.mode != 0o120000)
                    .map(|_| path.as_str())
            })
            .collect();
        let deleted: Vec<_> = selected
            .iter()
            .filter(|(_, source)| source.is_none())
            .map(|(path, _)| path.as_str())
            .collect();
        let mut normalization = Normalization::new(
            &directory,
            &common,
            &admin,
            &regular,
            &deleted,
            &index.staging_index(),
            &parent_tree,
        )?;
        for (path, source) in &selected {
            normalization.validate_selected_attribute(
                &directory,
                path,
                source.as_ref().map(|source| source.bytes.as_slice()),
            )?;
        }
        normalization.prepare_blobs(&repo, &selected)?;
        let mut entries = Vec::new();
        let mut normalized_bytes = 0;
        let mut planned = Vec::with_capacity(selected.len());
        for (path, source) in selected {
            let (mode, oid) = match source {
                Some(source) if source.mode == 0o120000 => (
                    source.mode,
                    repo.write_blob(source.bytes)
                        .map_err(|e| invalid(e.to_string()))?
                        .detach(),
                ),
                Some(source) => {
                    normalization.write_entry(&repo, path, source.mode, source.bytes)?
                }
                None => (0, gix::ObjectId::null(repo.object_hash())),
            };
            if mode != 0 {
                normalized_bytes += repo
                    .find_header(oid)
                    .map_err(|e| invalid(e.to_string()))?
                    .size();
                if normalized_bytes > MAX_TOTAL_BYTES as u64 {
                    return Err(invalid("normalized files exceed the 32 MiB commit limit"));
                }
            }
            entries.extend_from_slice(format!("{mode:o} {oid}\t").as_bytes());
            entries.extend_from_slice(path.as_bytes());
            entries.push(0);
            planned.push((path, mode, oid));
        }
        let tree = index.prepare_tree(&directory, entries)?;
        let prepared_tree = repo.find_tree(tree).map_err(|e| invalid(e.to_string()))?;
        for (path, mode, oid) in planned {
            let actual = prepared_tree
                .lookup_entry_by_path(path)
                .map_err(|e| invalid(e.to_string()))?;
            let matches = match actual {
                None => mode == 0,
                Some(entry) => {
                    mode != 0 && u32::from(entry.mode().0) == mode && entry.object_id() == oid
                }
            };
            if !matches {
                return Err(invalid("Git did not stage an explicit requested path exactly; reserved or unsupported file names cannot be committed"));
            }
        }
        if tree == parent_tree.id {
            return Err(invalid("selected files have no changes to commit"));
        }
        let author = gix::actor::Signature {
            name: "Gitust agent".into(),
            email: author_email.into(),
            time: gix::date::Time::new(chrono::Utc::now().timestamp(), 0),
        };
        // Builtin Git conversions use private metadata; no hooks, custom filters, or GPG programs run.
        let commit = repo
            .write_object(gix::objs::Commit {
                tree,
                parents: [parent].into(),
                author: author.clone(),
                committer: author,
                encoding: None,
                message: message.into(),
                extra_headers: Vec::new(),
            })
            .map_err(|e| invalid(e.to_string()))?
            .detach();
        Ok(PreparedWorktreeCommit {
            result: WorktreeCommit {
                worktree_id: id,
                branch,
                commit: CommitSha(commit.to_string()),
                parent: expected.clone(),
            },
            directory,
            admin,
            common,
            normalization,
            index,
            _service: service,
        })
    }

    pub fn finish_worktree_commit(
        &self,
        prepared: PreparedWorktreeCommit,
    ) -> Result<WorktreeCommit, GitEngineError> {
        self.finish_worktree_commit_with_gate(prepared, || Ok(()))
    }

    /// `publish_gate` runs after the last potentially slow inspection and immediately before CAS.
    /// HTTP/MCP callers must recheck the original credential here, not just its cached role.
    pub fn finish_worktree_commit_with_gate(
        &self,
        prepared: PreparedWorktreeCommit,
        publish_gate: impl FnOnce() -> Result<(), GitEngineError>,
    ) -> Result<WorktreeCommit, GitEngineError> {
        let _service = self.lock_worktree_service()?;
        let result = &prepared.result;
        let (directory, admin, common) =
            self.commit_checkout(result.worktree_id, &result.branch, &result.parent)?;
        if directory != prepared.directory || admin != prepared.admin || common != prepared.common {
            return Err(invalid(
                "managed checkout identity changed during commit preparation",
            ));
        }
        prepared.index.validate()?;
        prepared.normalization.validate()?;
        let mut repo = self.acquire_repo()?;
        ensure_reflog_identity(&mut repo)?;
        let commit = gix::ObjectId::from_hex(result.commit.0.as_bytes())
            .map_err(|_| invalid("invalid prepared commit SHA"))?;
        let parent = gix::ObjectId::from_hex(result.parent.0.as_bytes())
            .map_err(|_| invalid("invalid expected parent SHA"))?;
        crate::ref_transaction::update_with_gate(
            &repo,
            format!("refs/heads/{}", result.branch).as_str(),
            commit,
            PreviousValue::MustExistAndMatch(Target::Object(parent)),
            "gitust: explicit agent commit",
            publish_gate,
        )
        .map_err(|e| GitEngineError::RefTransaction(e.to_string()))?;
        prepared.index.publish().map_err(|error| invalid(format!(
            "commit {} was created, but worktree index recovery is required: {error}; inspect this exact SHA and restore the index before rerunning verification; no approval was granted",
            result.commit.0
        )))?;
        Ok(result.clone())
    }

    fn commit_checkout(
        &self,
        id: WorktreeId,
        branch: &str,
        expected: &CommitSha,
    ) -> Result<(PathBuf, PathBuf, PathBuf), GitEngineError> {
        self.ensure_config_has_no_credentials()?;
        if self.load_worktree_approval(id)?.is_some() {
            return Err(invalid(
                "approved/merged worktrees cannot receive agent commits",
            ));
        }
        let leaf = id.0.to_string();
        let managed = self
            .list_agent_worktrees()?
            .into_iter()
            .find(|entry| {
                entry.worktree_dir.file_name().and_then(|s| s.to_str()) == Some(leaf.as_str())
            })
            .ok_or_else(|| invalid("managed checkout is missing"))?;
        if managed.detached || managed.branch != branch || managed.head != *expected {
            return Err(invalid("checkout is detached, belongs to another branch, or changed from the expected parent"));
        }
        let repo = self.acquire_repo()?;
        let common = Self::common_git_dir(&repo);
        let admin =
            crate::worktree_metadata::read_worktree_admin_dir(&managed.worktree_dir, &common)?;
        crate::config_guard::reject_linked_worktree_config(&admin)?;
        crate::worktree_origin::read_or_migrate(&admin, &common, id, Some(branch))?
            .ok_or_else(|| invalid("worktree creation target is unknown; recreate the worktree"))?;
        let checkout = GitEngine::new(&managed.worktree_dir).acquire_repo()?;
        let mut head = checkout
            .head()
            .map_err(|_| invalid("cannot read checkout HEAD"))?;
        if head
            .referent_name()
            .map(|name| name.shorten().to_string())
            .as_deref()
            != Some(branch)
            || head
                .peel_to_commit_in_place()
                .map_err(|_| invalid("cannot resolve checkout HEAD"))?
                .id()
                .to_string()
                != expected.0
            || self.resolve_branch_oid(branch)?.as_deref() != Some(expected.0.as_str())
        {
            return Err(invalid(
                "checkout HEAD does not match the expected agent parent",
            ));
        }
        Ok((managed.worktree_dir, admin, common))
    }
}
