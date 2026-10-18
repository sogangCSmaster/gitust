//! Slice 6 (Phase 4.1): Verified Snapshot Rollback for Source-of-Truth Repositories.

use std::collections::BTreeSet;
use std::time::Duration;

use chrono::Utc;
use gitust_core::{CommitSha, RepoId};
use gitust_sync::{MirrorRefPin, RollbackVerificationReport};
use uuid::Uuid;

use super::{mirror_transport_command, sync_state_err, validate_mirror_ref_pin};
use crate::ancestry::is_agent_branch_short;
use crate::process::{git_command, run_capped, run_capped_with_input};
use crate::{run_git, GitEngine, GitEngineError};

impl GitEngine {
    pub fn ensure_rollback_snapshot_objects(
        &self,
        repo_id: RepoId,
        default_remote_url: &str,
        snapshot_id: Uuid,
    ) -> Result<(), GitEngineError> {
        let topology = self.load_sync_topology(repo_id, default_remote_url)?;
        let snapshot = topology
            .snapshots
            .iter()
            .find(|s| s.snapshot_id == snapshot_id)
            .cloned()
            .ok_or_else(|| GitEngineError::RevisionNotFound(snapshot_id.to_string()))?;

        let mut missing_shas = Vec::new();
        for pin in &snapshot.pinned_refs {
            validate_mirror_ref_pin(pin)?;
            if run_git(
                &self.repo_path,
                [
                    "cat-file",
                    "-e",
                    &format!("{}^{{object}}", pin.target_sha.0),
                ],
            )
            .is_err()
            {
                missing_shas.push(pin.target_sha.0.clone());
            }
        }
        if !missing_shas.is_empty() {
            let clean_url = Self::validate_mirror_remote_url(&topology.backup_remote_url)?;
            let mut fetch = mirror_transport_command(&self.repo_path);
            fetch.args([
                "fetch",
                "--no-tags",
                "--no-write-fetch-head",
                "--recurse-submodules=no",
                "--",
                &clean_url,
            ]);
            for sha in &missing_shas {
                fetch.arg(sha);
            }
            run_capped(fetch, self.fetch_timeout, 1024 * 1024)?;
        }
        Ok(())
    }

    pub fn rollback_sync_snapshot_with_gate(
        &self,
        repo_id: RepoId,
        default_remote_url: &str,
        snapshot_id: Uuid,
        gate: impl FnOnce() -> Result<(), GitEngineError>,
    ) -> Result<RollbackVerificationReport, GitEngineError> {
        let _service = self.lock_worktree_service()?;
        let topology = self.load_sync_topology(repo_id, default_remote_url)?;
        let snapshot = topology
            .snapshots
            .iter()
            .find(|s| s.snapshot_id == snapshot_id)
            .cloned()
            .ok_or_else(|| GitEngineError::RevisionNotFound(snapshot_id.to_string()))?;

        for pin in &snapshot.pinned_refs {
            validate_mirror_ref_pin(pin)?;
            run_git(
                &self.repo_path,
                [
                    "cat-file",
                    "-e",
                    &format!("{}^{{object}}", pin.target_sha.0),
                ],
            )?;
        }

        let out = run_git(
            &self.repo_path,
            [
                "for-each-ref",
                "--format=%(refname) %(objectname)",
                "refs/heads/",
                "refs/tags/",
            ],
        )?;
        let text = String::from_utf8(out)
            .map_err(|e| sync_state_err(format!("non-UTF-8 ref listing: {e}")))?;

        let pinned_names: BTreeSet<&str> = snapshot
            .pinned_refs
            .iter()
            .map(|p| p.ref_name.as_str())
            .collect();
        let head_branch = match self.inspect_repository_head()? {
            crate::RepositoryHead::Commit(id) => format!("refs/heads/{}", id.branch_name),
            crate::RepositoryHead::Unborn { branch_name } => format!("refs/heads/{branch_name}"),
        };

        let mut preserved_agent_refs: u32 = 0;
        let mut stale_ordinary_refs: Vec<(String, String)> = Vec::new();
        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let Some((ref_name, sha)) = trimmed.split_once(' ') else {
                return Err(sync_state_err("malformed for-each-ref line"));
            };
            if let Some(short) = ref_name.strip_prefix("refs/heads/") {
                if is_agent_branch_short(short) {
                    preserved_agent_refs = preserved_agent_refs.saturating_add(1);
                    continue;
                }
            }
            let current_pin = MirrorRefPin {
                ref_name: ref_name.to_owned(),
                target_sha: CommitSha(sha.to_owned()),
            };
            validate_mirror_ref_pin(&current_pin)?;
            if !pinned_names.contains(ref_name) && ref_name != head_branch {
                stale_ordinary_refs.push((ref_name.to_owned(), sha.to_owned()));
            }
        }

        gate()?;

        let mut script = String::from("start\n");
        for pin in &snapshot.pinned_refs {
            script.push_str(&format!("update {} {}\n", pin.ref_name, pin.target_sha.0));
        }
        for (stale_ref, old_sha) in &stale_ordinary_refs {
            script.push_str(&format!("delete {stale_ref} {old_sha}\n"));
        }
        script.push_str("prepare\ncommit\n");

        let mut update_cmd = git_command(&self.repo_path);
        update_cmd.args(["update-ref", "--stdin"]);
        run_capped_with_input(
            update_cmd,
            Duration::from_secs(60),
            64 * 1024,
            Some(script.into_bytes()),
        )?;

        if self.acquire_repo()?.work_dir().is_some() {
            run_git(&self.repo_path, ["reset", "--hard", "HEAD"])?;
        }
        Self::verify_git_fsck(&self.repo_path)?;

        // fail-open-ok: saturating u32 counter for telemetry in RollbackVerificationReport.
        let restored_refs = u32::try_from(snapshot.pinned_refs.len()).unwrap_or(u32::MAX);
        Ok(RollbackVerificationReport {
            repo_id,
            snapshot_id,
            restored_refs,
            preserved_agent_refs,
            fsck_passed: true,
            rolled_back_at: Utc::now(),
        })
    }
}
