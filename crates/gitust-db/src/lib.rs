//! `gitust-db`: High-speed, thread-safe Auth, Invite-Link, Session, PAT, SSH Key,
//! and Team Repository Store with Argon2id hashing, SHA-256 token digests,
//! constant-time verification, sliding-window login rate limiting, and lock-protected `0600` persistence.
//!
//! Strictly enforces `AGENTS.md` Phase 2~3 authentication & authorization rules:
//! - Zero public registration or "first web user = admin" routes.
//! - Initial SiteAdmin can ONLY be bootstrapped via `gitust admin create` CLI.
//! - Accounts are created exclusively via single-use, 72h-expiring, revocable Admin Invite Links.
//! - Only SHA-256 hashes of invite/reset/session/PAT tokens are stored in memory or on disk.
//! - Account deactivation immediately invalidates all sessions, PATs, SSH keys, and pending tokens.
//! - Strict BOLA/IDOR ownership checks on all user resources and team repositories.

mod automatic_checks;
mod credential_mutations;
mod credentials;
mod crypto;
mod disk_lock;
mod invites;
mod mutation_authority;
pub use invites::AccountInviteRequest;
mod invite_authority;
mod repository_group_invites;
#[cfg(test)]
mod tests_repository_group_invites;
pub use repository_group_invites::{
    RepositoryGroupInvite, RepositoryGroupInviteError, RepositoryGroupInviteOutcome,
    RepositoryGroupInviteRequest,
};
mod models;
mod repository_group_members;
mod stored_user;
pub use repository_group_members::{GroupMember, GroupMemberError};
mod oauth_session;
mod persistence;
mod repository_registration;
mod repository_visibility;
pub use repository_registration::{AuthorizedRepository, RegisteredRepository};
pub use repository_visibility::RepositoryVisibilityError;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_automatic_actor;
#[cfg(test)]
mod tests_credential_mutations;
#[cfg(test)]
mod tests_persistence;
#[cfg(test)]
mod tests_repository_group_members;
#[cfg(test)]
mod tests_repository_registration;
#[cfg(test)]
mod tests_repository_rename;
#[cfg(test)]
mod tests_repository_visibility;
#[cfg(test)]
mod tests_review_regressions;
#[cfg(test)]
mod tests_session_scope;
#[cfg(test)]
mod tests_snapshot_limits;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::Utc;
use gitust_core::{
    AccountStatus, AuthError, AuthenticatedPrincipal, EnterpriseRole, EnterpriseSecurityPolicy,
    PersonalAccessTokenView, RepoId, RepositoryRecord, SshPublicKeyView, TeamId, UserId,
    UserPublicProfile,
};
use parking_lot::{Mutex, RwLock};
use subtle::ConstantTimeEq;

pub use crypto::{hash_token_sha256, validate_repository_name};
pub use models::{IssuedInviteLink, IssuedPersonalAccessToken, LoginSessionResponse};

use crypto::{hash_password_argon2, validate_username};
use models::{AuthState, StoredUser};
use persistence::DiskStamp;

/// Maximum failed login attempts allowed per identifier or account within the rate-limit window.
pub const MAX_LOGIN_ATTEMPTS: usize = 5;
/// Sliding window duration in seconds for login rate limiting (15 minutes).
pub const LOGIN_RATE_LIMIT_WINDOW_SECS: i64 = 15 * 60;
/// Maximum tracked rate-limit keys to prevent memory exhaustion DoS from random usernames.
pub(crate) const MAX_RATE_LIMIT_TRACKED_KEYS: usize = 10_000;
/// Default session TTL in hours (7 days).
pub(crate) const DEFAULT_SESSION_TTL_HOURS: i64 = 24 * 7;
/// Minimum required password length.
pub(crate) const MIN_PASSWORD_LEN: usize = 10;

/// High-concurrency, O(1) indexed Authentication & Authorization Store.
/// Disk persistence is synchronized via cross-process advisory file locking (`<auth-db>.lock`)
/// and `DiskStamp` metadata checks; a pending durable mutation keeps its in-memory write lock.
#[derive(Debug, Clone)]
pub struct AuthStore {
    pub(crate) state: Arc<RwLock<AuthState>>,
    pub(crate) last_disk_stamp: Arc<Mutex<Option<DiskStamp>>>,
    pub(crate) persistence_path: Option<PathBuf>,
}

impl Default for AuthStore {
    fn default() -> Self {
        Self::new_in_memory()
    }
}

impl AuthStore {
    #[must_use]
    pub fn new_in_memory() -> Self {
        Self {
            state: Arc::new(RwLock::new(AuthState::default())),
            last_disk_stamp: Arc::new(Mutex::new(None)),
            persistence_path: None,
        }
    }

    /// Opens or creates an on-disk persistent `AuthStore` at `path`.
    pub fn open_persistent(path: impl AsRef<Path>) -> Result<Self, AuthError> {
        let path_buf = path.as_ref().to_path_buf();
        let (snapshot, initial_stamp) = persistence::load_initial_snapshot_locked(&path_buf)?;
        let state = snapshot.map_or_else(AuthState::default, AuthState::from_snapshot);

        Ok(Self {
            state: Arc::new(RwLock::new(state)),
            last_disk_stamp: Arc::new(Mutex::new(initial_stamp)),
            persistence_path: Some(path_buf),
        })
    }

    /// Path of the persistent snapshot, so sandbox callers can hide the DB and lockfile.
    #[must_use]
    pub fn persistence_path(&self) -> Option<&Path> {
        self.persistence_path.as_deref()
    }

    /// Exclusively invoked by the `gitust admin create` CLI command.
    /// There is NO web route that exposes this method.
    /// Fails with `AuthError::AdminAlreadyBootstrapped` if any user already exists.
    pub fn bootstrap_initial_admin_cli(
        &self,
        username: &str,
        email: &str,
        password: &str,
    ) -> Result<UserPublicProfile, AuthError> {
        let clean_username = validate_username(username)?;
        let clean_email = email.trim().to_ascii_lowercase();

        {
            let guard = self.read_state()?;
            if !guard.users.is_empty() {
                return Err(AuthError::AdminAlreadyBootstrapped);
            }
            guard
                .policy
                .validate_registration(&clean_email, true)
                .map_err(|e| AuthError::InvalidInput(e.to_string()))?;
        }

        // Perform CPU-intensive Argon2id hashing outside the lock
        let password_hash = hash_password_argon2(password)?;

        let (profile, snapshot) = {
            let mut guard = self.write_state()?;
            if !guard.users.is_empty() {
                return Err(AuthError::AdminAlreadyBootstrapped);
            }
            guard
                .policy
                .validate_registration(&clean_email, true)
                .map_err(|e| AuthError::InvalidInput(e.to_string()))?;

            let user = StoredUser {
                id: UserId::new(),
                username: clean_username.clone(),
                email: clean_email.clone(),
                password_hash,
                role: EnterpriseRole::SiteAdmin,
                status: AccountStatus::Active,
                team_ids: Vec::new(),
                membership_version: 0,
                created_at: Utc::now(),
            };
            let profile = user.to_public_profile();

            guard
                .username_index
                .insert(clean_username.to_ascii_lowercase(), user.id);
            guard.email_index.insert(clean_email, user.id);
            guard.users.insert(user.id, user);
            let snap = self.maybe_snapshot(guard);
            (profile, snap)
        };

        self.persist_snapshot(snapshot)?;
        Ok(profile)
    }

    pub(crate) fn verify_active_admin_locked(
        state: &AuthState,
        caller: &AuthenticatedPrincipal,
    ) -> Result<UserId, AuthError> {
        let Some(admin_user) = state.users.get(&caller.user_id) else {
            return Err(AuthError::Unauthenticated);
        };
        if admin_user.status != AccountStatus::Active {
            return Err(AuthError::AccountDeactivated);
        }
        if admin_user.role != EnterpriseRole::SiteAdmin {
            return Err(AuthError::AdminPrivilegeRequired);
        }
        Ok(admin_user.id)
    }

    pub(crate) fn verify_active_user_locked(
        state: &AuthState,
        caller: &AuthenticatedPrincipal,
    ) -> Result<AuthenticatedPrincipal, AuthError> {
        let Some(user) = state.users.get(&caller.user_id) else {
            return Err(AuthError::Unauthenticated);
        };
        if user.status != AccountStatus::Active {
            return Err(AuthError::AccountDeactivated);
        }
        Ok(user.to_principal())
    }

    /// Deactivates (`Suspended`) a user account and atomically revokes all of their
    /// active sessions, PATs, SSH public keys, and pending password-reset/invite links.
    pub fn deactivate_user(
        &self,
        caller: &AuthenticatedPrincipal,
        target_user_id: UserId,
    ) -> Result<UserPublicProfile, AuthError> {
        self.deactivate_user_with_authority(
            crate::mutation_authority::MutationAuthority::Operator(caller),
            target_user_id,
        )
    }
    pub fn deactivate_user_with_credential(
        &self,
        token: &str,
        user: gitust_core::UserId,
        target_user_id: UserId,
    ) -> Result<UserPublicProfile, AuthError> {
        self.deactivate_user_with_authority(
            crate::mutation_authority::MutationAuthority::Credential(token, user),
            target_user_id,
        )
    }
    fn deactivate_user_with_authority(
        &self,
        authority: crate::mutation_authority::MutationAuthority<'_>,
        target_user_id: UserId,
    ) -> Result<UserPublicProfile, AuthError> {
        let (profile, snapshot) = {
            let mut guard = self.write_state()?;
            authority.verify(&guard)?;

            let active_admin_count = guard
                .users
                .values()
                .filter(|u| {
                    u.status == AccountStatus::Active && u.role == EnterpriseRole::SiteAdmin
                })
                .count();

            let Some(target) = guard.users.get(&target_user_id) else {
                return Err(AuthError::InvalidInput("target user not found".to_owned()));
            };

            if target.status == AccountStatus::Active
                && target.role == EnterpriseRole::SiteAdmin
                && active_admin_count <= 1
            {
                return Err(AuthError::CannotDeactivateLastAdmin);
            }

            let profile = {
                let Some(target_mut) = guard.users.get_mut(&target_user_id) else {
                    return Err(AuthError::InvalidInput("target user not found".to_owned()));
                };
                target_mut.status = AccountStatus::Suspended;
                target_mut.to_public_profile()
            };

            let now = Utc::now();
            for ses in guard.sessions_by_hash.values_mut() {
                if ses.user_id == target_user_id {
                    ses.revoked = true;
                }
            }
            for pat in guard.pats_by_id.values_mut() {
                if pat.owner_id == target_user_id {
                    pat.revoked = true;
                }
            }
            for ssh_key in guard.ssh_keys_by_id.values_mut() {
                if ssh_key.owner_id == target_user_id {
                    ssh_key.revoked = true;
                }
            }
            for inv in guard.invites_by_id.values_mut() {
                if inv.target_email == profile.email
                    && inv.used_at.is_none()
                    && inv.revoked_at.is_none()
                {
                    inv.revoked_at = Some(now);
                }
            }

            let snap = self.maybe_snapshot(guard);
            (profile, snap)
        };

        self.persist_snapshot(snapshot)?;
        Ok(profile)
    }

    /// Registers a team-owned repository and enforces BOLA/team access checks.
    pub fn create_team_repository(
        &self,
        caller: &AuthenticatedPrincipal,
        name: &str,
        owner_team_id: TeamId,
        is_private: bool,
    ) -> Result<RepositoryRecord, AuthError> {
        self.create_team_repository_with_authority(
            crate::mutation_authority::MutationAuthority::Operator(caller),
            name,
            owner_team_id,
            is_private,
        )
    }
    pub fn create_team_repository_with_credential(
        &self,
        token: &str,
        user: gitust_core::UserId,
        name: &str,
        owner_team_id: TeamId,
        is_private: bool,
    ) -> Result<RepositoryRecord, AuthError> {
        self.create_team_repository_with_authority(
            crate::mutation_authority::MutationAuthority::Credential(token, user),
            name,
            owner_team_id,
            is_private,
        )
    }
    fn create_team_repository_with_authority(
        &self,
        authority: crate::mutation_authority::MutationAuthority<'_>,
        name: &str,
        owner_team_id: TeamId,
        is_private: bool,
    ) -> Result<RepositoryRecord, AuthError> {
        let clean_name = validate_repository_name(name)?;

        let (repo, snapshot) = {
            let mut guard = self.write_state()?;
            let active_caller = authority.active(&guard)?;

            if active_caller.role != EnterpriseRole::SiteAdmin
                && !active_caller.team_ids.contains(&owner_team_id)
            {
                return Err(AuthError::ForbiddenRepositoryAccess);
            }

            let repo = RepositoryRecord {
                id: RepoId::new(),
                name: clean_name,
                owner_team_id,
                is_private,
                created_at: Utc::now(),
            };
            guard.repositories.insert(repo.id, repo.clone().into());
            let snap = self.maybe_snapshot(guard);
            (repo, snap)
        };

        self.persist_snapshot(snapshot)?;
        Ok(repo)
    }

    pub fn repository_exists(&self, repo_id: RepoId) -> Result<bool, AuthError> {
        Ok(self.read_state()?.repositories.contains_key(&repo_id))
    }

    /// Returns a snapshot of the authoritative [`EnterpriseSecurityPolicy`] stored in [`AuthStore`].
    pub fn security_policy(&self) -> Result<EnterpriseSecurityPolicy, AuthError> {
        Ok(self.read_state()?.policy.clone())
    }

    /// Verifies whether `caller` is permitted to access `repo_id`.
    /// Blocks access if `caller` is deactivated or belongs to a different team on a private repository.
    pub fn authorize_repository_access(
        &self,
        caller: &AuthenticatedPrincipal,
        repo_id: RepoId,
    ) -> Result<RepositoryRecord, AuthError> {
        let guard = self.read_state()?;
        let active_caller = Self::verify_active_user_locked(&guard, caller)?;

        let Some(repo) = guard.repositories.get(&repo_id) else {
            return Err(AuthError::InvalidInput("repository not found".to_owned()));
        };

        if repo.metadata.is_private
            && active_caller.role != EnterpriseRole::SiteAdmin
            && !active_caller
                .team_ids
                .contains(&repo.metadata.owner_team_id)
        {
            return Err(AuthError::ForbiddenRepositoryAccess);
        }

        Ok(repo.metadata.clone())
    }

    /// Verifies that `caller` is an active user in the store and returns their refreshed principal.
    pub fn verify_active_principal(
        &self,
        caller: &AuthenticatedPrincipal,
    ) -> Result<AuthenticatedPrincipal, AuthError> {
        let guard = self.read_state()?;
        if guard.users.is_empty() {
            return Ok(caller.clone());
        }
        Self::verify_active_user_locked(&guard, caller)
    }

    pub fn list_users(
        &self,
        caller: &AuthenticatedPrincipal,
    ) -> Result<Vec<UserPublicProfile>, AuthError> {
        let guard = self.read_state()?;
        Self::verify_active_admin_locked(&guard, caller)?;
        let mut users: Vec<UserPublicProfile> = guard
            .users
            .values()
            .map(StoredUser::to_public_profile)
            .collect();
        users.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        Ok(users)
    }

    /// Revokes the active session corresponding to `raw_session_token` for `caller`.
    pub fn revoke_session(
        &self,
        caller: &AuthenticatedPrincipal,
        raw_session_token: &str,
    ) -> Result<(), AuthError> {
        let candidate_hash = hash_token_sha256(raw_session_token);
        let snapshot = {
            let mut guard = self.write_state()?;
            let active_caller = Self::verify_active_user_locked(&guard, caller)?;
            let Some(session) = guard.sessions_by_hash.get_mut(&candidate_hash) else {
                return Err(AuthError::InvalidToken);
            };
            if session.token_sha256.ct_eq(&candidate_hash).unwrap_u8() != 1 {
                return Err(AuthError::InvalidToken);
            }
            if session.user_id != active_caller.user_id
                && active_caller.role != EnterpriseRole::SiteAdmin
            {
                return Err(AuthError::ForbiddenResourceOwnership);
            }
            session.revoked = true;
            self.maybe_snapshot(guard)
        };
        self.persist_snapshot(snapshot)
    }

    /// Lists Personal Access Tokens owned by `caller` (never exposes token digests or raw tokens).
    pub fn list_personal_access_tokens(
        &self,
        caller: &AuthenticatedPrincipal,
    ) -> Result<Vec<PersonalAccessTokenView>, AuthError> {
        let guard = self.read_state()?;
        let active_caller = Self::verify_active_user_locked(&guard, caller)?;
        let mut pats: Vec<PersonalAccessTokenView> = guard
            .pats_by_id
            .values()
            .filter(|pat| pat.owner_id == active_caller.user_id)
            .map(models::StoredPat::to_public_view)
            .collect();
        pats.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        Ok(pats)
    }

    /// Lists SSH public keys owned by `caller`.
    pub fn list_ssh_public_keys(
        &self,
        caller: &AuthenticatedPrincipal,
    ) -> Result<Vec<SshPublicKeyView>, AuthError> {
        let guard = self.read_state()?;
        let active_caller = Self::verify_active_user_locked(&guard, caller)?;
        let mut keys: Vec<SshPublicKeyView> = guard
            .ssh_keys_by_id
            .values()
            .filter(|key| key.owner_id == active_caller.user_id)
            .map(models::StoredSshKey::to_public_view)
            .collect();
        keys.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        Ok(keys)
    }
}
