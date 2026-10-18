//! `gitust-sync`: Bi-Directional Real-Time GitHub Mirroring Policy & Engine.
//! Keeps noisy/experimental AI agent branches (`agent/*`) 100% private on-premise
//! while allowing human-approved branches (`main`, release branches, verified PRs)
//! to sync with an existing GitHub repository.

use chrono::{DateTime, Utc};
use gitust_core::{CommitSha, RepoId};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SyncTopologyMode {
    #[serde(rename = "inbound_mirror")]
    #[default]
    InboundMirror,
    #[serde(rename = "source_of_truth")]
    SourceOfTruth,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MirrorRefPin {
    pub ref_name: String,
    pub target_sha: CommitSha,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupMirrorSnapshot {
    pub snapshot_id: Uuid,
    pub repo_id: RepoId,
    pub backup_remote_url: String,
    pub pinned_refs: Vec<MirrorRefPin>,
    pub captured_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupMirrorPushReport {
    pub repo_id: RepoId,
    pub backup_remote_url: String,
    pub snapshot_id: Uuid,
    pub pushed_branches: u32,
    pub pushed_tags: u32,
    pub filtered_agent_refs: u32,
    pub verified_remote_match: bool,
    pub pushed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RollbackVerificationReport {
    pub repo_id: RepoId,
    pub snapshot_id: Uuid,
    pub restored_refs: u32,
    pub preserved_agent_refs: u32,
    pub fsck_passed: bool,
    pub rolled_back_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncTopologyState {
    pub repo_id: RepoId,
    pub mode: SyncTopologyMode,
    pub backup_remote_url: String,
    #[serde(default)]
    pub snapshots: Vec<BackupMirrorSnapshot>,
    #[serde(default)]
    pub last_push_report: Option<BackupMirrorPushReport>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BiDirectionalSyncConfig {
    pub repo_id: RepoId,
    pub github_remote_url: String,
    pub sync_inbound_webhooks: bool,
    pub sync_outbound_approved_only: bool,
    pub private_agent_branch_prefix: String,
    #[serde(default)]
    pub topology_mode: SyncTopologyMode,
}

impl BiDirectionalSyncConfig {
    #[must_use]
    pub fn new(repo_id: RepoId, github_remote_url: impl Into<String>) -> Self {
        Self {
            repo_id,
            github_remote_url: github_remote_url.into(),
            sync_inbound_webhooks: true,
            sync_outbound_approved_only: true,
            private_agent_branch_prefix: "agent/".to_owned(),
            topology_mode: SyncTopologyMode::InboundMirror,
        }
    }

    /// Determines whether a branch is allowed to be pushed to the external GitHub mirror.
    /// Experimental agent branches (`agent/*`) are strictly blocked from leaving the on-premise server
    /// until merged into an approved branch (`main`, `master`, `release/*`).
    #[must_use]
    pub fn should_sync_branch_outbound(&self, branch_name: &str, human_approved: bool) -> bool {
        let trimmed = branch_name.trim();
        let normalized = trimmed
            .strip_prefix("refs/heads/")
            // fail-open-ok: optional prefix normalization precedes full branch validation and private namespace checks.
            .unwrap_or(trimmed)
            .trim_matches('/');

        // Reject empty, traversal, or malformed Git ref names (fail-closed)
        if normalized.is_empty()
            || normalized == "@"
            || normalized == "HEAD"
            || normalized.starts_with('-')
            || normalized.ends_with('.')
            || normalized
                .split('/')
                .any(|seg| seg.is_empty() || seg.starts_with('.') || seg.ends_with(".lock"))
            || normalized.starts_with("refs/")
            || normalized.contains("..")
            || normalized.contains("//")
            || normalized.contains("@{")
            || normalized.ends_with(".lock")
            || normalized.chars().any(|c| {
                c.is_control()
                    || c.is_whitespace()
                    || matches!(c, '~' | '^' | ':' | '?' | '*' | '[' | '\\')
            })
        {
            return false;
        }

        let prefix = self.private_agent_branch_prefix.trim_matches('/');
        if !prefix.is_empty() {
            let norm_lower = normalized.to_ascii_lowercase();
            let prefix_lower = prefix.to_ascii_lowercase();
            if norm_lower == prefix_lower
                || norm_lower
                    .strip_prefix(&prefix_lower)
                    .is_some_and(|rest| rest.starts_with('/'))
            {
                return false;
            }
        }

        if self.topology_mode == SyncTopologyMode::SourceOfTruth {
            return true;
        }
        if self.sync_outbound_approved_only {
            return human_approved;
        }
        true
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncStatusReport {
    pub github_remote_url: String,
    pub last_synced_commit: Option<CommitSha>,
    pub sync_latency_ms: u64,
    pub private_agent_branches_filtered: u32,
    #[serde(default)]
    pub skipped_branches: Vec<(String, String)>,
    #[serde(default)]
    pub skipped_tags: Vec<(String, String)>,
    #[serde(default)]
    pub local_only_branches: Vec<String>,
    pub synced_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_agent_branches_and_ref_traversal_from_leaking_to_github() {
        let cfg = BiDirectionalSyncConfig::new(
            RepoId::new(),
            "https://github.com/sogangCSmaster/gitust.git",
        );

        // Agent branches must never leak to GitHub even if flag is true
        assert!(!cfg.should_sync_branch_outbound("agent/claude/task-101", true));
        assert!(!cfg.should_sync_branch_outbound("agent/cursor/fix-db", false));
        assert!(!cfg.should_sync_branch_outbound("refs/heads/agent/claude/task-101", true));
        assert!(!cfg.should_sync_branch_outbound("/agent/claude/task-101", true));
        assert!(!cfg.should_sync_branch_outbound("Agent/Claude/Task-101", true));
        assert!(!cfg.should_sync_branch_outbound("main/../agent/secret", true));
        assert!(!cfg.should_sync_branch_outbound("", true));
        for invalid in [
            "-main",
            "@",
            "HEAD",
            "a.lock/nested",
            ".hidden/main",
            "main.",
        ] {
            assert!(
                !cfg.should_sync_branch_outbound(invalid, true),
                "accepted {invalid}"
            );
        }

        // Approved main branch syncs to GitHub
        assert!(cfg.should_sync_branch_outbound("main", true));
        assert!(cfg.should_sync_branch_outbound("refs/heads/main", true));
        assert!(!cfg.should_sync_branch_outbound("main", false));
    }

    #[test]
    fn source_of_truth_mode_allows_ordinary_branches_and_blocks_agent_branches() {
        let mut cfg = BiDirectionalSyncConfig::new(
            RepoId::new(),
            "https://github.com/sogangCSmaster/gitust-backup.git",
        );
        assert_eq!(cfg.topology_mode, SyncTopologyMode::InboundMirror);

        cfg.topology_mode = SyncTopologyMode::SourceOfTruth;
        assert!(cfg.should_sync_branch_outbound("main", false));
        assert!(cfg.should_sync_branch_outbound("release/v1", false));
        assert!(cfg.should_sync_branch_outbound("feature/login", false));
        assert!(!cfg.should_sync_branch_outbound("agent/claude/task-101", true));
        assert!(!cfg.should_sync_branch_outbound("refs/heads/agent/cursor/fix", true));
        assert!(!cfg.should_sync_branch_outbound("main/../agent/secret", true));

        let pin = MirrorRefPin {
            ref_name: "refs/heads/main".to_owned(),
            target_sha: CommitSha("a".repeat(40)),
        };
        assert_eq!(pin.ref_name, "refs/heads/main");
    }
}
