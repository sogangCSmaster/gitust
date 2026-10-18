use std::fmt;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{CommitSha, RepoId, UserId};

/// Strongly-typed UUID identifier for a pull request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PullRequestId(pub Uuid);

impl PullRequestId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    #[must_use]
    pub fn parse_str(s: &str) -> Option<Self> {
        // fail-open-ok: returns None on malformed UUID so callers can reject with 400/404.
        Uuid::parse_str(s.trim()).ok().map(Self)
    }
}

impl Default for PullRequestId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for PullRequestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Strongly-typed UUID identifier for a pull request review.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PullRequestReviewId(pub Uuid);

impl PullRequestReviewId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for PullRequestReviewId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for PullRequestReviewId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Strongly-typed UUID identifier for a pull request comment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PullRequestCommentId(pub Uuid);

impl PullRequestCommentId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for PullRequestCommentId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for PullRequestCommentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Lifecycle state of a repository pull request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PullRequestStatus {
    Open,
    Draft,
    Merged,
    Closed,
}

/// Review decision submitted on a pull request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewVerdict {
    Approve,
    RequestChanges,
    Comment,
}

/// A review recorded against a specific head commit of a pull request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullRequestReview {
    pub id: PullRequestReviewId,
    pub author_user_id: UserId,
    pub author_username: String,
    pub verdict: ReviewVerdict,
    pub body: String,
    pub reviewed_head_sha: CommitSha,
    pub created_at_epoch_secs: u64,
}

/// A general or inline diff comment on a pull request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullRequestComment {
    pub id: PullRequestCommentId,
    pub author_user_id: UserId,
    pub author_username: String,
    pub body: String,
    pub file_path: Option<String>,
    pub line_number: Option<u32>,
    pub commit_sha: Option<CommitSha>,
    pub created_at_epoch_secs: u64,
}

/// Persisted pull request record scoped to a repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullRequestRecord {
    pub id: PullRequestId,
    pub number: u64,
    pub repo_id: RepoId,
    pub title: String,
    pub description: String,
    pub author_user_id: UserId,
    pub author_username: String,
    pub base_branch: String,
    pub head_branch: String,
    pub base_sha: CommitSha,
    pub head_sha: CommitSha,
    pub status: PullRequestStatus,
    pub merged_sha: Option<CommitSha>,
    pub merged_by_username: Option<String>,
    pub reviews: Vec<PullRequestReview>,
    pub comments: Vec<PullRequestComment>,
    pub created_at_epoch_secs: u64,
    pub updated_at_epoch_secs: u64,
}

/// Branch protection rule for an ordinary branch pattern (e.g. `"main"`, `"release/*"`, `"*"`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BranchProtectionRule {
    pub branch_pattern: String,
    #[serde(default)]
    pub require_pull_request: bool,
    #[serde(default)]
    pub required_approvals: u8,
    #[serde(default)]
    pub require_passing_checks: bool,
    #[serde(default)]
    pub restrict_direct_pushes: bool,
}

impl BranchProtectionRule {
    /// Checks whether `branch` (normalized short ordinary branch name) matches `branch_pattern`.
    #[must_use]
    pub fn matches_branch(&self, branch: &str) -> bool {
        let pattern = self.branch_pattern.trim();
        // fail-open-ok: normalizes an optional refs/heads/ prefix before exact or wildcard branch matching.
        let pattern = pattern.strip_prefix("refs/heads/").unwrap_or(pattern);
        if pattern == "*" || pattern == branch {
            return true;
        }
        if let Some(prefix) = pattern.strip_suffix('*') {
            return !prefix.is_empty() && branch.starts_with(prefix);
        }
        false
    }
}

/// Live Git mergeability preview between `base_branch` and `head_branch`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullRequestMergePreview {
    pub can_merge: bool,
    pub fast_forward: bool,
    pub conflicting_files: Vec<String>,
    pub base_sha: CommitSha,
    pub head_sha: CommitSha,
}

/// Evaluated branch-protection and review/checks gate status for a pull request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullRequestProtectionStatus {
    pub protected: bool,
    pub require_pull_request: bool,
    pub required_approvals: u8,
    pub approved_count: u8,
    pub changes_requested: bool,
    pub require_passing_checks: bool,
    pub checks_passed: bool,
}
