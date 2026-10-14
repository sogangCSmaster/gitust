//! `gitust-sync`: Bi-Directional Real-Time GitHub Mirroring Engine.
//! Keeps noisy/experimental AI agent branches (`agent/*`) 100% private on-premise
//! while syncing human-approved merges (`main`, release branches, verified PRs)
//! bi-directionally with an existing GitHub repository in `<100ms`.

use chrono::{DateTime, Utc};
use gitust_core::{CommitSha, RepoId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BiDirectionalSyncConfig {
    pub repo_id: RepoId,
    pub github_remote_url: String,
    pub sync_inbound_webhooks: bool,
    pub sync_outbound_approved_only: bool,
    pub private_agent_branch_prefix: String,
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
        }
    }

    /// Determines whether a branch is allowed to be pushed to the external GitHub mirror.
    /// Experimental agent branches (`agent/*`) are strictly blocked from leaving the on-premise server
    /// until merged into an approved branch (`main`, `master`, `release/*`).
    #[must_use]
    pub fn should_sync_branch_outbound(&self, branch_name: &str, human_approved: bool) -> bool {
        if branch_name.starts_with(&self.private_agent_branch_prefix) {
            return false;
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
    pub last_synced_commit: CommitSha,
    pub sync_latency_ms: u64,
    pub private_agent_branches_filtered: u32,
    pub synced_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_agent_branches_from_leaking_to_github() {
        let cfg = BiDirectionalSyncConfig::new(
            RepoId::new(),
            "https://github.com/sogangCSmaster/gitust.git",
        );

        // Agent branches must never leak to GitHub even if flag is true
        assert!(!cfg.should_sync_branch_outbound("agent/claude/task-101", true));
        assert!(!cfg.should_sync_branch_outbound("agent/cursor/fix-db", false));

        // Approved main branch syncs to GitHub
        assert!(cfg.should_sync_branch_outbound("main", true));
        assert!(!cfg.should_sync_branch_outbound("main", false));
    }
}
