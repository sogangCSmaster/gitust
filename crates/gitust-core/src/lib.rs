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

    #[must_use]
    pub fn parse_str(s: &str) -> Option<Self> {
        // fail-open-ok: invalid UUID becomes None; managed resource callers reject it instead of gaining authority.
        Uuid::parse_str(s).ok().map(Self)
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct UserId(pub Uuid);

impl UserId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for UserId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TeamId(pub Uuid);

impl TeamId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for TeamId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct InviteId(pub Uuid);

impl InviteId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for InviteId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(pub Uuid);

impl SessionId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for SessionId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PatId(pub Uuid);

impl PatId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for PatId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SshKeyId(pub Uuid);

impl SshKeyId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for SshKeyId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CommitSha(pub String);

/// Default expiration window for single-use invite links (72 hours per `AGENTS.md`).
pub const DEFAULT_INVITE_EXPIRY_HOURS: i64 = 72;
/// Maximum byte length of an account email address.
pub const MAX_EMAIL_BYTES: usize = 254;

// ============================================================================
// Core Domain & Authorization Errors
// ============================================================================

#[derive(Debug, Error, PartialEq, Eq)]
pub enum DomainError {
    #[error("repository not found: {0}")]
    RepositoryNotFound(String),

    #[error("agent worktree not found: {0}")]
    WorktreeNotFound(String),

    #[error("malformed email address: '{0}'")]
    MalformedEmail(String),

    #[error("email address must not exceed 254 bytes")]
    EmailTooLong,

    #[error(
        "registration rejected by enterprise policy: email '{0}' is outside allowed domain '@{1}'"
    )]
    DomainLockViolation(String, String),

    #[error("public registration is disabled (Enterprise Invite-Only mode active)")]
    InviteOnlyActive,

    #[error("invalid operation for current state: {0}")]
    InvalidStateTransition(String),
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum AuthError {
    #[error("initial site admin has already been bootstrapped; use an admin invite link instead")]
    AdminAlreadyBootstrapped,

    #[error("public sign-up is strictly disabled; accounts can only be created via a Site Admin invite link")]
    PublicSignupDisabled,

    #[error("invalid or unknown invite/reset token")]
    InvalidToken,

    #[error("invite/reset token has already been used (one-time use enforced)")]
    TokenAlreadyUsed,

    #[error("invite/reset token has expired")]
    TokenExpired,

    #[error("invite/reset token was revoked by an administrator")]
    TokenRevoked,

    #[error("token purpose mismatch")]
    TokenPurposeMismatch,

    #[error("invalid credentials")]
    InvalidCredentials,

    #[error("account is deactivated; all sessions, SSH keys, and tokens are invalidated")]
    AccountDeactivated,

    #[error("too many failed login attempts; rate limit exceeded, try again later")]
    RateLimitExceeded,

    #[error("authentication required")]
    Unauthenticated,

    #[error("forbidden: site administrator privileges required")]
    AdminPrivilegeRequired,

    #[error("forbidden (BOLA/IDOR): cannot access or mutate another user's resource")]
    ForbiddenResourceOwnership,

    #[error("forbidden: user does not belong to the team owning this repository")]
    ForbiddenRepositoryAccess,

    #[error("cannot deactivate the last active site administrator")]
    CannotDeactivateLastAdmin,

    #[error("username or email already exists")]
    DuplicateUser,

    #[error("invalid input: {0}")]
    InvalidInput(String),

    #[error("internal storage error: {0}")]
    StorageError(String),
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
    /// Worktree exists but no sandbox verification has run yet.
    Pending,
    Compiling,
    Verified,
    Failed,
    Merged,
    Discarded,
}

/// Public agent worktree model.
///
/// Deliberately contains no server filesystem paths: on-disk locations are runtime state owned
/// by the server and are never serialized to API clients.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentWorktree {
    pub id: WorktreeId,
    pub repo_id: RepoId,
    pub agent: AgentKind,
    pub branch: String,
    pub base_branch: String,
    /// Commit the agent branch was created from.
    pub base_commit: CommitSha,
    pub prompt_summary: String,
    /// Measured wall-clock time to create the branch and check out the worktree.
    pub materialize_micros: u64,
    pub status: WorktreeStatus,
    /// Exact clean commit verified by the latest successful sandbox command.
    #[serde(default)]
    pub verified_commit: Option<CommitSha>,
    /// Committed merge-target Checks policy used by this verification; None means no policy.
    #[serde(default)]
    pub verified_checks_oid: Option<String>,
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
    /// to the upstream GitHub mirror. Only a `Verified` worktree may be merged.
    pub fn approve_and_merge(&mut self, synced_to_github: bool) -> Result<(), DomainError> {
        if self.status != WorktreeStatus::Verified {
            return Err(DomainError::InvalidStateTransition(format!(
                "only a verified worktree can be merged (current status: {:?})",
                self.status
            )));
        }
        self.status = WorktreeStatus::Merged;
        self.github_synced = synced_to_github;
        Ok(())
    }
}

// ============================================================================
// Enterprise Self-Hosted IAM, Invite-Only Auth & Security Policy
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EnterpriseRole {
    SiteAdmin,
    Maintainer,
    Developer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AccountStatus {
    Active,
    Suspended,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InviteTokenPurpose {
    AccountInvite,
    PasswordReset,
}

/// Safe public representation of a user account (never contains password hashes).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserPublicProfile {
    pub id: UserId,
    pub username: String,
    pub email: String,
    pub role: EnterpriseRole,
    pub status: AccountStatus,
    pub team_ids: Vec<TeamId>,
    pub created_at: DateTime<Utc>,
}

/// Safe public representation of an invite or password-reset link metadata (never contains raw token or hash).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvitePublicView {
    pub id: InviteId,
    pub purpose: InviteTokenPurpose,
    pub target_email: String,
    pub assigned_role: EnterpriseRole,
    pub created_by: UserId,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub used_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
}

/// Safe public metadata for a Personal Access Token (PAT) used for HTTPS Git access.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersonalAccessTokenView {
    pub id: PatId,
    pub owner_id: UserId,
    pub name: String,
    pub token_prefix: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub revoked: bool,
}

/// Safe public metadata for an SSH public key used for `russh` Git access.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SshPublicKeyView {
    pub id: SshKeyId,
    pub owner_id: UserId,
    pub title: String,
    pub fingerprint_sha256: String,
    pub created_at: DateTime<Utc>,
    pub revoked: bool,
}

/// Team-scoped repository metadata for RBAC / BOLA enforcement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepositoryRecord {
    pub id: RepoId,
    pub name: String,
    pub owner_team_id: TeamId,
    pub is_private: bool,
    pub created_at: DateTime<Utc>,
}

/// Verified principal returned after authenticating a Session, PAT, or SSH Key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthenticatedPrincipal {
    pub user_id: UserId,
    pub username: String,
    pub role: EnterpriseRole,
    pub team_ids: Vec<TeamId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnterpriseSecurityPolicy {
    /// Strict On-Premise Privacy Mode: 0 bytes of source code leave the internal network.
    pub strict_on_premise_privacy: bool,
    /// Disable public sign-up (only Site Admin invites allowed in Phase 2~3).
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
        if trimmed.len() > MAX_EMAIL_BYTES {
            return Err(DomainError::EmailTooLong);
        }
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

    #[test]
    fn registration_email_limit_counts_utf8_bytes() {
        let policy = EnterpriseSecurityPolicy::default();
        let at_limit = format!("{}a@company.internal", "é".repeat(118));
        assert_eq!(at_limit.len(), MAX_EMAIL_BYTES);
        assert!(policy.validate_registration(&at_limit, true).is_ok());
        let over_limit = format!("{}é@company.internal", "é".repeat(118));
        assert_eq!(over_limit.len(), MAX_EMAIL_BYTES + 1);
        assert_eq!(
            policy.validate_registration(&over_limit, true),
            Err(DomainError::EmailTooLong)
        );
    }
}
