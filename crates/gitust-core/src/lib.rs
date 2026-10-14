//! `gitust-core`: Pure domain models, strongly-typed Newtypes, and Enterprise policies.
//! Zero `unwrap()` / `expect()` in production code paths.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

// ============================================================================
// Strongly-Typed Domain Identifiers (Newtype Pattern)
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RepoId(pub Uuid);

impl RepoId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for RepoId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WorktreeId(pub Uuid);

impl WorktreeId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for WorktreeId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SandboxRunId(pub Uuid);

impl SandboxRunId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for SandboxRunId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CommitSha(pub String);

// ============================================================================
// Core Domain Errors
// ============================================================================

#[derive(Debug, Error, PartialEq, Eq)]
pub enum DomainError {
    #[error("repository not found: {0}")]
    RepositoryNotFound(String),

    #[error("agent worktree not found: {0}")]
    WorktreeNotFound(String),

    #[error("malformed email address: '{0}'")]
    MalformedEmail(String),

    #[error("registration rejected by enterprise policy: email '{0}' is outside allowed domain '@{1}'")]
    DomainLockViolation(String, String),

    #[error("public registration is disabled (Enterprise Invite-Only mode active)")]
    InviteOnlyActive,

    #[error("invalid operation for current state: {0}")]
    InvalidStateTransition(String),
}

// ============================================================================
// Agent Swarm & Zero-Copy Worktree Isolation Models
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentKind {
    ClaudeCode,
    CursorAgent,
    CodexSwarm,
    InternalBot,
}

impl AgentKind {
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::ClaudeCode => "Claude Code",
            Self::CursorAgent => "Cursor Agent",
            Self::CodexSwarm => "Codex Swarm",
            Self::InternalBot => "Internal Bot",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorktreeStatus {
    Compiling,
    Verified,
    Failed,
    Merged,
    Discarded,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentWorktree {
    pub id: WorktreeId,
    pub repo_id: RepoId,
    pub agent: AgentKind,
    pub branch: String,
    pub base_branch: String,
    pub prompt_summary: String,
    pub fork_latency_micros: u64,
    pub status: WorktreeStatus,
    pub last_sandbox_duration_ms: u64,
    pub tests_passed: u32,
    pub tests_total: u32,
    pub files_changed: u32,
    pub additions: u32,
    pub deletions: u32,
    pub github_synced: bool,
    pub created_at: DateTime<Utc>,
}

impl AgentWorktree {
    /// Transitions this agent worktree into the `Merged` state and records whether it was synced
    /// to the upstream GitHub mirror.
    pub fn approve_and_merge(&mut self, synced_to_github: bool) -> Result<(), DomainError> {
        if self.status == WorktreeStatus::Discarded {
            return Err(DomainError::InvalidStateTransition(
                "cannot merge a discarded worktree".to_owned(),
            ));
        }
        self.status = WorktreeStatus::Merged;
        self.github_synced = synced_to_github;
        Ok(())
    }
}

// ============================================================================
// Enterprise Self-Hosted IAM, Auth & Security Policy
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EnterpriseRole {
    SiteAdmin,
    Maintainer,
    Developer,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnterpriseSecurityPolicy {
    /// Strict On-Premise Privacy Mode: 0 bytes of source code leave the internal network.
    pub strict_on_premise_privacy: bool,
    /// Disable public sign-up (only Site Admin invites or corporate OIDC/LDAP allowed).
    pub invite_only: bool,
    /// Mandatory hardware 2FA / Passkey & SSH commit signature verification.
    pub require_2fa_and_signed_commits: bool,
    /// Allowed corporate email domain (e.g. "company.internal").
    pub allowed_email_domain: String,
    /// Auto-push human-approved (`a` key) merges to the upstream GitHub mirror.
    pub auto_sync_approved_merges_to_github: bool,
}

impl Default for EnterpriseSecurityPolicy {
    fn default() -> Self {
        Self {
            strict_on_premise_privacy: true,
            invite_only: true,
            require_2fa_and_signed_commits: true,
            allowed_email_domain: "company.internal".to_owned(),
            auto_sync_approved_merges_to_github: true,
        }
    }
}

impl EnterpriseSecurityPolicy {
    /// Strictly validates whether a new user email is permitted to register on this private instance.
    /// Prevents empty local parts (`"@company.internal"`), multiple `@` injections, whitespace,
    /// and handles RFC 5321 case-insensitive domain matching.
    pub fn validate_registration(&self, email: &str, is_invited: bool) -> Result<(), DomainError> {
        if self.invite_only && !is_invited {
            return Err(DomainError::InviteOnlyActive);
        }

        let trimmed = email.trim();
        let Some((local, domain)) = trimmed.rsplit_once('@') else {
            return Err(DomainError::MalformedEmail(email.to_owned()));
        };

        if local.is_empty()
            || domain.is_empty()
            || local.contains('@')
            || local.chars().any(|c| c.is_whitespace() || c.is_control())
            || domain.chars().any(|c| c.is_whitespace() || c.is_control())
        {
            return Err(DomainError::MalformedEmail(email.to_owned()));
        }

        let clean_allowed = self.allowed_email_domain.trim().trim_start_matches('@');
        if clean_allowed.is_empty() || !domain.eq_ignore_ascii_case(clean_allowed) {
            return Err(DomainError::DomainLockViolation(
                email.to_owned(),
                clean_allowed.to_owned(),
            ));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enterprise_policy_enforces_invite_and_strict_domain_lock() {
        let policy = EnterpriseSecurityPolicy::default();

        // Uninvited user rejected
        assert_eq!(
            policy.validate_registration("dev@company.internal", false),
            Err(DomainError::InviteOnlyActive)
        );

        // Invited user with external domain rejected
        assert!(matches!(
            policy.validate_registration("hacker@gmail.com", true),
            Err(DomainError::DomainLockViolation(_, _))
        ));

        // Edge Case 1: Empty local part ("@company.internal") must be rejected
        assert!(matches!(
            policy.validate_registration("@company.internal", true),
            Err(DomainError::MalformedEmail(_))
        ));

        // Edge Case 2: Multiple '@' injection ("attacker@evil.com@company.internal") must be rejected
        assert!(matches!(
            policy.validate_registration("attacker@evil.com@company.internal", true),
            Err(DomainError::MalformedEmail(_))
        ));

        // Edge Case 3: Case-insensitive corporate domain ("Dev@Company.Internal") must succeed
        assert!(policy
            .validate_registration("Dev@Company.Internal", true)
            .is_ok());
    }
}
