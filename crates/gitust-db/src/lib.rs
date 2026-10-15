//! `gitust-db`: High-speed, thread-safe Auth, Invite-Link, Session, PAT, SSH Key,
//! and Team Repository Store with Argon2id hashing, SHA-256 token digests,
//! constant-time verification, sliding-window login rate limiting, and lock-free `0600` persistence.
//!
//! Strictly enforces `AGENTS.md` Phase 2~3 authentication & authorization rules:
//! - Zero public registration or "first web user = admin" routes.
//! - Initial SiteAdmin can ONLY be bootstrapped via `gitust admin create` CLI.
//! - Accounts are created exclusively via single-use, 72h-expiring, revocable Admin Invite Links.
//! - Only SHA-256 hashes of invite/reset/session/PAT tokens are stored in memory or on disk.
//! - Account deactivation immediately invalidates all sessions, PATs, SSH keys, and pending tokens.
//! - Strict BOLA/IDOR ownership checks on all user resources and team repositories.

mod credentials;
mod crypto;
mod invites;
mod models;
#[cfg(test)]
mod tests;

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::Utc;
use gitust_core::{
    AccountStatus, AuthError, AuthenticatedPrincipal, EnterpriseRole, EnterpriseSecurityPolicy,
    RepoId, RepositoryRecord, TeamId, UserId, UserPublicProfile,
};
use parking_lot::{Mutex, RwLock};
use uuid::Uuid;

pub use crypto::hash_token_sha256;
pub use models::{IssuedInviteLink, IssuedPersonalAccessToken, LoginSessionResponse};

use crypto::{hash_password_argon2, validate_repository_name, validate_username};
use models::{AuthState, PersistedSnapshot, StoredUser};

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
/// Disk persistence is synchronized via a dedicated `persist_last_revision` mutex OUTSIDE the in-memory `RwLock`,
/// keeping in-memory reads non-blocking even during `fsync`.
#[derive(Debug, Clone)]
pub struct AuthStore {
    pub(crate) state: Arc<RwLock<AuthState>>,
    pub(crate) persist_last_revision: Arc<Mutex<u64>>,
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
            persist_last_revision: Arc::new(Mutex::new(0)),
            persistence_path: None,
        }
    }

    /// Opens or creates an on-disk persistent `AuthStore` at `path`.
    pub fn open_persistent(path: impl AsRef<Path>) -> Result<Self, AuthError> {
        let path_buf = path.as_ref().to_path_buf();
        let state = if path_buf.exists() {
            let bytes = fs::read(&path_buf)
                .map_err(|e| AuthError::StorageError(format!("failed to read auth db: {e}")))?;
            let snap: PersistedSnapshot = serde_json::from_slice(&bytes)
                .map_err(|e| AuthError::StorageError(format!("corrupt auth db json: {e}")))?;
            AuthState::from_snapshot(snap)
        } else {
            AuthState::default()
        };
        let initial_rev = state.revision;

        Ok(Self {
            state: Arc::new(RwLock::new(state)),
            persist_last_revision: Arc::new(Mutex::new(initial_rev)),
            persistence_path: Some(path_buf),
        })
    }

    /// Persists a point-in-time snapshot to disk OUTSIDE the in-memory `RwLock`.
    /// Uses revision ordering so a slower older snapshot never overwrites a newer snapshot.
    pub(crate) fn persist_snapshot_unlocked(
        &self,
        snapshot: Option<PersistedSnapshot>,
    ) -> Result<(), AuthError> {
        let (Some(ref target_path), Some(snap)) = (&self.persistence_path, snapshot) else {
            return Ok(());
        };

        let mut last_rev = self.persist_last_revision.lock();
        if snap.revision <= *last_rev {
            return Ok(());
        }

        if let Some(parent) = target_path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).map_err(|e| {
                    AuthError::StorageError(format!("failed to create db parent dir: {e}"))
                })?;
            }
        }

        let tmp_path = target_path.with_extension(format!("tmp.{}", Uuid::new_v4()));
        let payload = serde_json::to_vec(&snap)
            .map_err(|e| AuthError::StorageError(format!("failed to serialize auth db: {e}")))?;

        {
            let mut file = fs::File::create(&tmp_path).map_err(|e| {
                AuthError::StorageError(format!("failed to create temp auth db: {e}"))
            })?;

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let perms = fs::Permissions::from_mode(0o600);
                file.set_permissions(perms).map_err(|e| {
                    AuthError::StorageError(format!("failed to set 0600 permissions: {e}"))
                })?;
            }

            file.write_all(&payload).map_err(|e| {
                AuthError::StorageError(format!("failed to write temp auth db: {e}"))
            })?;
            file.sync_all().map_err(|e| {
                AuthError::StorageError(format!("failed to fsync temp auth db: {e}"))
            })?;
        }

        fs::rename(&tmp_path, target_path).map_err(|e| {
            let _ = fs::remove_file(&tmp_path);
            AuthError::StorageError(format!("failed to atomically replace auth db: {e}"))
        })?;

        *last_rev = snap.revision;
        Ok(())
    }

    pub(crate) fn maybe_snapshot(&self, state: &mut AuthState) -> Option<PersistedSnapshot> {
        state.revision = state.revision.saturating_add(1);
        if self.persistence_path.is_some() {
            Some(state.snapshot_current())
        } else {
            if state.revision.is_multiple_of(64) || state.sessions_by_hash.len() >= 1024 {
                state.prune_expired(Utc::now());
            }
            None
        }
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
            let guard = self.state.read();
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
            let mut guard = self.state.write();
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
                created_at: Utc::now(),
            };
            let profile = user.to_public_profile();

            guard
                .username_index
                .insert(clean_username.to_ascii_lowercase(), user.id);
            guard.email_index.insert(clean_email, user.id);
            guard.users.insert(user.id, user);
            let snap = self.maybe_snapshot(&mut guard);
            (profile, snap)
        };

        self.persist_snapshot_unlocked(snapshot)?;
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
        let (profile, snapshot) = {
            let mut guard = self.state.write();
            Self::verify_active_admin_locked(&guard, caller)?;

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

            let snap = self.maybe_snapshot(&mut guard);
            (profile, snap)
        };

        self.persist_snapshot_unlocked(snapshot)?;
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
        let clean_name = validate_repository_name(name)?;

        let (repo, snapshot) = {
            let mut guard = self.state.write();
            let active_caller = Self::verify_active_user_locked(&guard, caller)?;

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
            guard.repositories.insert(repo.id, repo.clone());
            let snap = self.maybe_snapshot(&mut guard);
            (repo, snap)
        };

        self.persist_snapshot_unlocked(snapshot)?;
        Ok(repo)
    }

    #[must_use]
    pub fn repository_exists(&self, repo_id: RepoId) -> bool {
        self.state.read().repositories.contains_key(&repo_id)
    }

    /// Returns a snapshot of the authoritative [`EnterpriseSecurityPolicy`] stored in [`AuthStore`].
    #[must_use]
    pub fn security_policy(&self) -> EnterpriseSecurityPolicy {
        self.state.read().policy.clone()
    }

    /// Verifies whether `caller` is permitted to access `repo_id`.
    /// Blocks access if `caller` is deactivated or belongs to a different team on a private repository.
    pub fn authorize_repository_access(
        &self,
        caller: &AuthenticatedPrincipal,
        repo_id: RepoId,
    ) -> Result<RepositoryRecord, AuthError> {
        let guard = self.state.read();
        let active_caller = Self::verify_active_user_locked(&guard, caller)?;

        let Some(repo) = guard.repositories.get(&repo_id) else {
            return Err(AuthError::InvalidInput("repository not found".to_owned()));
        };

        if repo.is_private
            && active_caller.role != EnterpriseRole::SiteAdmin
            && !active_caller.team_ids.contains(&repo.owner_team_id)
        {
            return Err(AuthError::ForbiddenRepositoryAccess);
        }

        Ok(repo.clone())
    }

    /// Verifies that `caller` is an active user in the store and returns their refreshed principal.
    pub fn verify_active_principal(
        &self,
        caller: &AuthenticatedPrincipal,
    ) -> Result<AuthenticatedPrincipal, AuthError> {
        let guard = self.state.read();
        if guard.users.is_empty() {
            return Ok(caller.clone());
        }
        Self::verify_active_user_locked(&guard, caller)
    }

    pub fn list_users(
        &self,
        caller: &AuthenticatedPrincipal,
    ) -> Result<Vec<UserPublicProfile>, AuthError> {
        let guard = self.state.read();
        Self::verify_active_admin_locked(&guard, caller)?;
        Ok(guard
            .users
            .values()
            .map(StoredUser::to_public_profile)
            .collect())
    }
}
