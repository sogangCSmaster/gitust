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

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::{Algorithm, Argon2, Params, Version};
use chrono::{DateTime, Duration, Utc};
use gitust_core::{
    AccountStatus, AuthError, AuthenticatedPrincipal, EnterpriseRole, EnterpriseSecurityPolicy,
    InviteId, InvitePublicView, InviteTokenPurpose, PatId, PersonalAccessTokenView, RepoId,
    RepositoryRecord, SessionId, SshKeyId, SshPublicKeyView, TeamId, UserId, UserPublicProfile,
    DEFAULT_INVITE_EXPIRY_HOURS,
};
use parking_lot::{Mutex, RwLock};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use uuid::Uuid;

/// Maximum failed login attempts allowed per identifier or account within the rate-limit window.
pub const MAX_LOGIN_ATTEMPTS: usize = 5;
/// Sliding window duration in seconds for login rate limiting (15 minutes).
pub const LOGIN_RATE_LIMIT_WINDOW_SECS: i64 = 15 * 60;
/// Maximum tracked rate-limit keys to prevent memory exhaustion DoS from random usernames.
const MAX_RATE_LIMIT_TRACKED_KEYS: usize = 10_000;
/// Default session TTL in hours (7 days).
const DEFAULT_SESSION_TTL_HOURS: i64 = 24 * 7;
/// Minimum required password length.
const MIN_PASSWORD_LEN: usize = 10;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredUser {
    id: UserId,
    username: String,
    email: String,
    password_hash: String,
    role: EnterpriseRole,
    status: AccountStatus,
    team_ids: Vec<TeamId>,
    created_at: DateTime<Utc>,
}

impl StoredUser {
    fn to_public_profile(&self) -> UserPublicProfile {
        UserPublicProfile {
            id: self.id,
            username: self.username.clone(),
            email: self.email.clone(),
            role: self.role,
            status: self.status,
            team_ids: self.team_ids.clone(),
            created_at: self.created_at,
        }
    }

    fn to_principal(&self) -> AuthenticatedPrincipal {
        AuthenticatedPrincipal {
            user_id: self.id,
            username: self.username.clone(),
            role: self.role,
            team_ids: self.team_ids.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredInviteToken {
    id: InviteId,
    token_sha256: [u8; 32],
    purpose: InviteTokenPurpose,
    target_email: String,
    assigned_role: EnterpriseRole,
    team_ids: Vec<TeamId>,
    created_by: UserId,
    created_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
    used_at: Option<DateTime<Utc>>,
    revoked_at: Option<DateTime<Utc>>,
}

impl StoredInviteToken {
    fn to_public_view(&self) -> InvitePublicView {
        InvitePublicView {
            id: self.id,
            purpose: self.purpose,
            target_email: self.target_email.clone(),
            assigned_role: self.assigned_role,
            created_by: self.created_by,
            created_at: self.created_at,
            expires_at: self.expires_at,
            used_at: self.used_at,
            revoked_at: self.revoked_at,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredSession {
    id: SessionId,
    user_id: UserId,
    token_sha256: [u8; 32],
    created_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
    revoked: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredPat {
    id: PatId,
    owner_id: UserId,
    name: String,
    token_prefix: String,
    token_sha256: [u8; 32],
    created_at: DateTime<Utc>,
    expires_at: Option<DateTime<Utc>>,
    revoked: bool,
}

impl StoredPat {
    fn to_public_view(&self) -> PersonalAccessTokenView {
        PersonalAccessTokenView {
            id: self.id,
            owner_id: self.owner_id,
            name: self.name.clone(),
            token_prefix: self.token_prefix.clone(),
            created_at: self.created_at,
            expires_at: self.expires_at,
            revoked: self.revoked,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredSshKey {
    id: SshKeyId,
    owner_id: UserId,
    title: String,
    public_key_openssh: String,
    fingerprint_sha256: String,
    created_at: DateTime<Utc>,
    revoked: bool,
}

impl StoredSshKey {
    fn to_public_view(&self) -> SshPublicKeyView {
        SshPublicKeyView {
            id: self.id,
            owner_id: self.owner_id,
            title: self.title.clone(),
            fingerprint_sha256: self.fingerprint_sha256.clone(),
            created_at: self.created_at,
            revoked: self.revoked,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct PersistedSnapshot {
    #[serde(default)]
    revision: u64,
    policy: EnterpriseSecurityPolicy,
    users: Vec<StoredUser>,
    invites: Vec<StoredInviteToken>,
    sessions: Vec<StoredSession>,
    pats: Vec<StoredPat>,
    ssh_keys: Vec<StoredSshKey>,
    repositories: Vec<RepositoryRecord>,
}

#[derive(Debug, Default)]
struct AuthState {
    revision: u64,
    policy: EnterpriseSecurityPolicy,
    users: HashMap<UserId, StoredUser>,
    username_index: HashMap<String, UserId>,
    email_index: HashMap<String, UserId>,
    invites_by_id: HashMap<InviteId, StoredInviteToken>,
    invite_hash_index: HashMap<[u8; 32], InviteId>,
    sessions_by_hash: HashMap<[u8; 32], StoredSession>,
    pats_by_id: HashMap<PatId, StoredPat>,
    pat_hash_index: HashMap<[u8; 32], PatId>,
    ssh_keys_by_id: HashMap<SshKeyId, StoredSshKey>,
    ssh_fingerprint_index: HashMap<String, SshKeyId>,
    repositories: HashMap<RepoId, RepositoryRecord>,
    failed_logins: HashMap<String, Vec<DateTime<Utc>>>,
}

impl AuthState {
    fn from_snapshot(snap: PersistedSnapshot) -> Self {
        let mut state = Self {
            revision: snap.revision,
            policy: snap.policy,
            ..Self::default()
        };

        for user in snap.users {
            state
                .username_index
                .insert(user.username.to_ascii_lowercase(), user.id);
            state
                .email_index
                .insert(user.email.to_ascii_lowercase(), user.id);
            state.users.insert(user.id, user);
        }

        for inv in snap.invites {
            state.invite_hash_index.insert(inv.token_sha256, inv.id);
            state.invites_by_id.insert(inv.id, inv);
        }

        for ses in snap.sessions {
            state.sessions_by_hash.insert(ses.token_sha256, ses);
        }

        for pat in snap.pats {
            state.pat_hash_index.insert(pat.token_sha256, pat.id);
            state.pats_by_id.insert(pat.id, pat);
        }

        for key in snap.ssh_keys {
            state
                .ssh_fingerprint_index
                .insert(key.fingerprint_sha256.clone(), key.id);
            state.ssh_keys_by_id.insert(key.id, key);
        }

        for repo in snap.repositories {
            state.repositories.insert(repo.id, repo);
        }

        state
    }

    fn prune_expired(&mut self, now: DateTime<Utc>) {
        self.sessions_by_hash.retain(|_, ses| {
            let user_active = self
                .users
                .get(&ses.user_id)
                .is_none_or(|u| u.status == AccountStatus::Active);
            now < ses.expires_at && !(ses.revoked && user_active)
        });
        let cutoff = now - Duration::seconds(LOGIN_RATE_LIMIT_WINDOW_SECS);
        self.failed_logins.retain(|_, list| {
            list.retain(|&ts| ts > cutoff);
            !list.is_empty()
        });
    }

    fn snapshot_current(&mut self) -> PersistedSnapshot {
        self.prune_expired(Utc::now());
        PersistedSnapshot {
            revision: self.revision,
            policy: self.policy.clone(),
            users: self.users.values().cloned().collect(),
            invites: self.invites_by_id.values().cloned().collect(),
            sessions: self.sessions_by_hash.values().cloned().collect(),
            pats: self.pats_by_id.values().cloned().collect(),
            ssh_keys: self.ssh_keys_by_id.values().cloned().collect(),
            repositories: self.repositories.values().cloned().collect(),
        }
    }
}

/// Returned once when an administrator creates an invite or password-reset link.
/// The `raw_token` is never stored in the database; only its SHA-256 digest is persisted.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IssuedInviteLink {
    pub invite: InvitePublicView,
    pub raw_token: String,
    pub invite_url_path: String,
}

/// Returned once when a user creates a Personal Access Token (PAT).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IssuedPersonalAccessToken {
    pub metadata: PersonalAccessTokenView,
    pub raw_token: String,
}

/// Returned upon successful login.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginSessionResponse {
    pub session_id: SessionId,
    pub raw_session_token: String,
    pub user: UserPublicProfile,
    pub expires_at: DateTime<Utc>,
}

/// High-concurrency, O(1) indexed Authentication & Authorization Store.
/// Disk persistence is synchronized via a dedicated `persist_mutex` OUTSIDE the in-memory `RwLock`,
/// keeping in-memory reads non-blocking even during `fsync`.
#[derive(Debug, Clone)]
pub struct AuthStore {
    state: Arc<RwLock<AuthState>>,
    persist_last_revision: Arc<Mutex<u64>>,
    persistence_path: Option<PathBuf>,
}

impl Default for AuthStore {
    fn default() -> Self {
        Self::new_in_memory()
    }
}

fn argon2_hasher() -> Result<Argon2<'static>, AuthError> {
    #[cfg(debug_assertions)]
    let params = Params::new(4096, 1, 1, None)
        .map_err(|e| AuthError::StorageError(format!("argon2 params error: {e}")))?;
    #[cfg(not(debug_assertions))]
    let params = Params::new(19456, 2, 1, None)
        .map_err(|e| AuthError::StorageError(format!("argon2 params error: {e}")))?;

    Ok(Argon2::new(Algorithm::Argon2id, Version::V0x13, params))
}

fn dummy_password_hash() -> &'static str {
    static DUMMY_HASH: OnceLock<String> = OnceLock::new();
    DUMMY_HASH.get_or_init(|| {
        let salt = SaltString::generate(&mut OsRng);
        argon2_hasher()
            .ok()
            .and_then(|a| a.hash_password(b"dummy-timing-shield-password", &salt).ok())
            .map(|h| h.to_string())
            .unwrap_or_default()
    })
}

#[must_use]
pub fn hash_token_sha256(raw_token: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(raw_token.trim().as_bytes());
    hasher.finalize().into()
}

fn generate_csprng_token(prefix: &str) -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    format!("{prefix}_{}", hex::encode(bytes))
}

fn hash_password_argon2(password: &str) -> Result<String, AuthError> {
    validate_password_strength(password)?;
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = argon2_hasher()?;
    let hash = argon2
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| AuthError::StorageError(format!("argon2 hash failed: {e}")))?;
    Ok(hash.to_string())
}

fn verify_password_argon2(password: &str, phc_hash: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(phc_hash) else {
        return false;
    };
    let Ok(argon2) = argon2_hasher() else {
        return false;
    };
    argon2.verify_password(password.as_bytes(), &parsed).is_ok()
}

fn validate_username(username: &str) -> Result<String, AuthError> {
    let trimmed = username.trim();
    if trimmed.len() < 2 || trimmed.len() > 39 {
        return Err(AuthError::InvalidInput(
            "username must be between 2 and 39 characters".to_owned(),
        ));
    }
    if !trimmed
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(AuthError::InvalidInput(
            "username may only contain ASCII alphanumeric characters, '-', or '_'".to_owned(),
        ));
    }
    Ok(trimmed.to_owned())
}

fn validate_repository_name(name: &str) -> Result<String, AuthError> {
    let trimmed = name.trim();
    if trimmed.is_empty()
        || trimmed.len() > 100
        || trimmed.starts_with('.')
        || trimmed.ends_with('.')
        || trimmed.contains("..")
        || !trimmed
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return Err(AuthError::InvalidInput(
            "repository name must be 1..=100 ASCII alphanumeric, '-', '_', or '.' characters"
                .to_owned(),
        ));
    }
    Ok(trimmed.to_owned())
}

fn validate_password_strength(password: &str) -> Result<(), AuthError> {
    if password.chars().count() < MIN_PASSWORD_LEN {
        return Err(AuthError::InvalidInput(format!(
            "password must be at least {MIN_PASSWORD_LEN} characters"
        )));
    }
    if password.len() > 256 {
        return Err(AuthError::InvalidInput(
            "password must not exceed 256 bytes".to_owned(),
        ));
    }
    Ok(())
}

fn decode_base64_standard(input: &str) -> Option<Vec<u8>> {
    let bytes = input.as_bytes();
    if bytes.is_empty() || !bytes.len().is_multiple_of(4) {
        return None;
    }
    let val = |b: u8| -> Option<u8> {
        match b {
            b'A'..=b'Z' => Some(b - b'A'),
            b'a'..=b'z' => Some(b - b'a' + 26),
            b'0'..=b'9' => Some(b - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    };
    let mut out = Vec::with_capacity((bytes.len() / 4) * 3);
    for (idx, chunk) in bytes.chunks_exact(4).enumerate() {
        let is_last = idx + 1 == bytes.len() / 4;
        let a = val(chunk[0])?;
        let b = val(chunk[1])?;
        out.push((a << 2) | (b >> 4));
        if chunk[2] == b'=' {
            if !is_last || chunk[3] != b'=' {
                return None;
            }
            break;
        }
        let c = val(chunk[2])?;
        out.push((b << 4) | (c >> 2));
        if chunk[3] == b'=' {
            if !is_last {
                return None;
            }
            break;
        }
        let d = val(chunk[3])?;
        out.push((c << 6) | d);
    }
    Some(out)
}

fn encode_base64_unpadded(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0];
        let b1 = if chunk.len() > 1 { chunk[1] } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] } else { 0 };
        out.push(char::from(TABLE[usize::from(b0 >> 2)]));
        out.push(char::from(
            TABLE[usize::from(((b0 & 0x03) << 4) | (b1 >> 4))],
        ));
        if chunk.len() > 1 {
            out.push(char::from(
                TABLE[usize::from(((b1 & 0x0f) << 2) | (b2 >> 6))],
            ));
        }
        if chunk.len() > 2 {
            out.push(char::from(TABLE[usize::from(b2 & 0x3f)]));
        }
    }
    out
}

/// Validates and canonicalizes an OpenSSH public key into `(<algorithm> <base64-blob>, SHA256:<base64>)`,
/// decoding the Base64 wire blob, verifying the embedded key-type header matches `<algorithm>`,
/// and computing the standard OpenSSH SHA-256 fingerprint over the decoded binary key blob.
fn canonicalize_openssh_pubkey(raw: &str) -> Result<(String, String), AuthError> {
    let mut parts = raw.split_whitespace();
    let (Some(algo), Some(blob)) = (parts.next(), parts.next()) else {
        return Err(AuthError::InvalidInput(
            "SSH public key must be in '<algorithm> <base64-blob>' OpenSSH format".to_owned(),
        ));
    };
    if !matches!(
        algo,
        "ssh-ed25519"
            | "ssh-rsa"
            | "ecdsa-sha2-nistp256"
            | "ecdsa-sha2-nistp384"
            | "ecdsa-sha2-nistp521"
    ) {
        return Err(AuthError::InvalidInput(format!(
            "unsupported SSH key algorithm: {algo}"
        )));
    }
    let Some(decoded) = decode_base64_standard(blob) else {
        return Err(AuthError::InvalidInput(
            "malformed base64 key blob in SSH public key".to_owned(),
        ));
    };
    if decoded.len() < 4 + algo.len() + 4 {
        return Err(AuthError::InvalidInput(
            "SSH public key blob is truncated".to_owned(),
        ));
    }
    let header_len = usize::try_from(u32::from_be_bytes([
        decoded[0], decoded[1], decoded[2], decoded[3],
    ]))
    .unwrap_or(usize::MAX);
    if header_len != algo.len() || &decoded[4..4 + algo.len()] != algo.as_bytes() {
        return Err(AuthError::InvalidInput(
            "SSH public key blob algorithm header does not match declared key type".to_owned(),
        ));
    }
    let digest: [u8; 32] = Sha256::digest(&decoded).into();
    let fingerprint = format!("SHA256:{}", encode_base64_unpadded(&digest));
    Ok((format!("{algo} {blob}"), fingerprint))
}

fn scoped_id_rate_limit_key(scope: &str, norm_id: &str) -> String {
    format!(
        "scope:{scope}:id:{}",
        hex::encode(hash_token_sha256(norm_id))
    )
}

fn scoped_account_rate_limit_key(scope: &str, user_id: UserId) -> String {
    format!("scope:{scope}:uid:{}", user_id.0)
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
    fn persist_snapshot_unlocked(
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

    fn maybe_snapshot(&self, state: &mut AuthState) -> Option<PersistedSnapshot> {
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

    // ========================================================================
    // 1. CLI-Only Initial Admin Bootstrap (`gitust admin create`)
    // ========================================================================

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

    // ========================================================================
    // 2. Invite Links & Password Reset Links (SHA-256 Only, 1-Time Use, 72h TTL)
    // ========================================================================

    fn verify_active_admin_locked(
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

    fn verify_active_user_locked(
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

    /// Issues a single-use cryptographic invite link (default 72h expiration).
    /// Only the SHA-256 digest of the token is stored in the database.
    pub fn create_invite_link(
        &self,
        caller: &AuthenticatedPrincipal,
        target_email: &str,
        assigned_role: EnterpriseRole,
        team_ids: Vec<TeamId>,
        ttl_hours: Option<i64>,
    ) -> Result<IssuedInviteLink, AuthError> {
        let hours = ttl_hours.unwrap_or(DEFAULT_INVITE_EXPIRY_HOURS);
        if !(1..=720).contains(&hours) {
            return Err(AuthError::InvalidInput(
                "invite TTL must be between 1 and 720 hours".to_owned(),
            ));
        }

        let raw_token = generate_csprng_token("gitust_inv");
        let token_sha256 = hash_token_sha256(&raw_token);
        let now = Utc::now();

        let (public_view, snapshot) = {
            let mut guard = self.state.write();
            let admin_id = Self::verify_active_admin_locked(&guard, caller)?;

            let clean_email = target_email.trim().to_ascii_lowercase();
            guard
                .policy
                .validate_registration(&clean_email, true)
                .map_err(|e| AuthError::InvalidInput(e.to_string()))?;

            if guard.email_index.contains_key(&clean_email) {
                return Err(AuthError::DuplicateUser);
            }

            let record = StoredInviteToken {
                id: InviteId::new(),
                token_sha256,
                purpose: InviteTokenPurpose::AccountInvite,
                target_email: clean_email,
                assigned_role,
                team_ids,
                created_by: admin_id,
                created_at: now,
                expires_at: now + Duration::hours(hours),
                used_at: None,
                revoked_at: None,
            };

            let public_view = record.to_public_view();
            guard.invite_hash_index.insert(token_sha256, record.id);
            guard.invites_by_id.insert(record.id, record);
            let snap = self.maybe_snapshot(&mut guard);
            (public_view, snap)
        };

        self.persist_snapshot_unlocked(snapshot)?;
        Ok(IssuedInviteLink {
            invite: public_view,
            invite_url_path: format!("/auth/accept-invite?token={raw_token}"),
            raw_token,
        })
    }

    /// Issues a single-use cryptographic password-reset link for an existing active user.
    pub fn create_password_reset_link(
        &self,
        caller: &AuthenticatedPrincipal,
        target_user_id: UserId,
        ttl_hours: Option<i64>,
    ) -> Result<IssuedInviteLink, AuthError> {
        let hours = ttl_hours.unwrap_or(DEFAULT_INVITE_EXPIRY_HOURS);
        if !(1..=72).contains(&hours) {
            return Err(AuthError::InvalidInput(
                "password reset TTL must be between 1 and 72 hours".to_owned(),
            ));
        }

        let raw_token = generate_csprng_token("gitust_rst");
        let token_sha256 = hash_token_sha256(&raw_token);
        let now = Utc::now();

        let (public_view, snapshot) = {
            let mut guard = self.state.write();
            let admin_id = Self::verify_active_admin_locked(&guard, caller)?;

            let Some(target_user) = guard.users.get(&target_user_id) else {
                return Err(AuthError::InvalidInput("target user not found".to_owned()));
            };
            if target_user.status != AccountStatus::Active {
                return Err(AuthError::AccountDeactivated);
            }

            let record = StoredInviteToken {
                id: InviteId::new(),
                token_sha256,
                purpose: InviteTokenPurpose::PasswordReset,
                target_email: target_user.email.clone(),
                assigned_role: target_user.role,
                team_ids: target_user.team_ids.clone(),
                created_by: admin_id,
                created_at: now,
                expires_at: now + Duration::hours(hours),
                used_at: None,
                revoked_at: None,
            };

            let public_view = record.to_public_view();
            guard.invite_hash_index.insert(token_sha256, record.id);
            guard.invites_by_id.insert(record.id, record);
            let snap = self.maybe_snapshot(&mut guard);
            (public_view, snap)
        };

        self.persist_snapshot_unlocked(snapshot)?;
        Ok(IssuedInviteLink {
            invite: public_view,
            invite_url_path: format!("/auth/reset-password?token={raw_token}"),
            raw_token,
        })
    }

    /// Administrator revocation of an unused invite or password-reset token.
    pub fn revoke_invite_link(
        &self,
        caller: &AuthenticatedPrincipal,
        invite_id: InviteId,
    ) -> Result<InvitePublicView, AuthError> {
        let (view, snapshot) = {
            let mut guard = self.state.write();
            Self::verify_active_admin_locked(&guard, caller)?;

            let Some(inv) = guard.invites_by_id.get_mut(&invite_id) else {
                return Err(AuthError::InvalidToken);
            };
            inv.revoked_at = Some(Utc::now());
            let view = inv.to_public_view();
            let snap = self.maybe_snapshot(&mut guard);
            (view, snap)
        };

        self.persist_snapshot_unlocked(snapshot)?;
        Ok(view)
    }

    /// Redeems a single-use invite token to create a new user account.
    pub fn redeem_invite_link(
        &self,
        raw_token: &str,
        username: &str,
        password: &str,
    ) -> Result<UserPublicProfile, AuthError> {
        let clean_username = validate_username(username)?;
        let candidate_hash = hash_token_sha256(raw_token);
        let now = Utc::now();

        {
            let guard = self.state.read();
            let Some(&invite_id) = guard.invite_hash_index.get(&candidate_hash) else {
                return Err(AuthError::InvalidToken);
            };
            let Some(inv) = guard.invites_by_id.get(&invite_id) else {
                return Err(AuthError::InvalidToken);
            };
            if inv.token_sha256.ct_eq(&candidate_hash).unwrap_u8() != 1 {
                return Err(AuthError::InvalidToken);
            }
            if inv.purpose != InviteTokenPurpose::AccountInvite {
                return Err(AuthError::TokenPurposeMismatch);
            }
            if inv.revoked_at.is_some() {
                return Err(AuthError::TokenRevoked);
            }
            if inv.used_at.is_some() {
                return Err(AuthError::TokenAlreadyUsed);
            }
            if now >= inv.expires_at {
                return Err(AuthError::TokenExpired);
            }
            if guard
                .username_index
                .contains_key(&clean_username.to_ascii_lowercase())
                || guard.email_index.contains_key(&inv.target_email)
            {
                return Err(AuthError::DuplicateUser);
            }
        }

        // Compute Argon2id password hash outside the lock
        let password_hash = hash_password_argon2(password)?;

        let (profile, snapshot) = {
            let mut guard = self.state.write();
            let check_now = Utc::now();
            let Some(&invite_id) = guard.invite_hash_index.get(&candidate_hash) else {
                return Err(AuthError::InvalidToken);
            };
            let (target_email, assigned_role, team_ids) = {
                let Some(inv) = guard.invites_by_id.get(&invite_id) else {
                    return Err(AuthError::InvalidToken);
                };
                if inv.token_sha256.ct_eq(&candidate_hash).unwrap_u8() != 1 {
                    return Err(AuthError::InvalidToken);
                }
                if inv.purpose != InviteTokenPurpose::AccountInvite {
                    return Err(AuthError::TokenPurposeMismatch);
                }
                if inv.revoked_at.is_some() {
                    return Err(AuthError::TokenRevoked);
                }
                if inv.used_at.is_some() {
                    return Err(AuthError::TokenAlreadyUsed);
                }
                if check_now >= inv.expires_at {
                    return Err(AuthError::TokenExpired);
                }
                (
                    inv.target_email.clone(),
                    inv.assigned_role,
                    inv.team_ids.clone(),
                )
            };

            if guard
                .username_index
                .contains_key(&clean_username.to_ascii_lowercase())
                || guard.email_index.contains_key(&target_email)
            {
                return Err(AuthError::DuplicateUser);
            }

            if let Some(inv_mut) = guard.invites_by_id.get_mut(&invite_id) {
                inv_mut.used_at = Some(check_now);
            }

            // Revoke any other pending AccountInvite tokens for the same target_email
            for inv in guard.invites_by_id.values_mut() {
                if inv.id != invite_id
                    && inv.target_email == target_email
                    && inv.used_at.is_none()
                    && inv.revoked_at.is_none()
                {
                    inv.revoked_at = Some(check_now);
                }
            }

            let user = StoredUser {
                id: UserId::new(),
                username: clean_username.clone(),
                email: target_email.clone(),
                password_hash,
                role: assigned_role,
                status: AccountStatus::Active,
                team_ids,
                created_at: check_now,
            };
            let profile = user.to_public_profile();

            guard
                .username_index
                .insert(clean_username.to_ascii_lowercase(), user.id);
            guard.email_index.insert(target_email, user.id);
            guard.users.insert(user.id, user);
            let snap = self.maybe_snapshot(&mut guard);
            (profile, snap)
        };

        self.persist_snapshot_unlocked(snapshot)?;
        Ok(profile)
    }

    /// Redeems a single-use password-reset link and rotates the user's Argon2id password hash,
    /// revoking all previous sessions, PATs, other pending reset tokens, and clearing failed-login lockouts.
    pub fn redeem_password_reset_link(
        &self,
        raw_token: &str,
        new_password: &str,
    ) -> Result<UserPublicProfile, AuthError> {
        let candidate_hash = hash_token_sha256(raw_token);
        let now = Utc::now();

        {
            let guard = self.state.read();
            let Some(&invite_id) = guard.invite_hash_index.get(&candidate_hash) else {
                return Err(AuthError::InvalidToken);
            };
            let Some(inv) = guard.invites_by_id.get(&invite_id) else {
                return Err(AuthError::InvalidToken);
            };
            if inv.token_sha256.ct_eq(&candidate_hash).unwrap_u8() != 1 {
                return Err(AuthError::InvalidToken);
            }
            if inv.purpose != InviteTokenPurpose::PasswordReset {
                return Err(AuthError::TokenPurposeMismatch);
            }
            if inv.revoked_at.is_some() {
                return Err(AuthError::TokenRevoked);
            }
            if inv.used_at.is_some() {
                return Err(AuthError::TokenAlreadyUsed);
            }
            if now >= inv.expires_at {
                return Err(AuthError::TokenExpired);
            }
        }

        let new_password_hash = hash_password_argon2(new_password)?;

        let (profile, snapshot) = {
            let mut guard = self.state.write();
            let check_now = Utc::now();
            let Some(&invite_id) = guard.invite_hash_index.get(&candidate_hash) else {
                return Err(AuthError::InvalidToken);
            };
            let target_email = {
                let Some(inv) = guard.invites_by_id.get(&invite_id) else {
                    return Err(AuthError::InvalidToken);
                };
                if inv.token_sha256.ct_eq(&candidate_hash).unwrap_u8() != 1 {
                    return Err(AuthError::InvalidToken);
                }
                if inv.purpose != InviteTokenPurpose::PasswordReset {
                    return Err(AuthError::TokenPurposeMismatch);
                }
                if inv.revoked_at.is_some() {
                    return Err(AuthError::TokenRevoked);
                }
                if inv.used_at.is_some() {
                    return Err(AuthError::TokenAlreadyUsed);
                }
                if check_now >= inv.expires_at {
                    return Err(AuthError::TokenExpired);
                }
                inv.target_email.clone()
            };

            let Some(&user_id) = guard.email_index.get(&target_email) else {
                return Err(AuthError::InvalidToken);
            };

            let (profile, lower_username) = {
                let Some(user) = guard.users.get_mut(&user_id) else {
                    return Err(AuthError::InvalidToken);
                };
                if user.status != AccountStatus::Active {
                    return Err(AuthError::AccountDeactivated);
                }
                user.password_hash = new_password_hash;
                (user.to_public_profile(), user.username.to_ascii_lowercase())
            };

            if let Some(inv_mut) = guard.invites_by_id.get_mut(&invite_id) {
                inv_mut.used_at = Some(check_now);
            }

            // Revoke any other unused password reset tokens for this email
            for inv in guard.invites_by_id.values_mut() {
                if inv.id != invite_id
                    && inv.target_email == target_email
                    && inv.purpose == InviteTokenPurpose::PasswordReset
                    && inv.used_at.is_none()
                    && inv.revoked_at.is_none()
                {
                    inv.revoked_at = Some(check_now);
                }
            }

            // Revoke all existing sessions & PATs on password reset
            for ses in guard.sessions_by_hash.values_mut() {
                if ses.user_id == user_id {
                    ses.revoked = true;
                }
            }
            for pat in guard.pats_by_id.values_mut() {
                if pat.owner_id == user_id {
                    pat.revoked = true;
                }
            }

            // Clear any brute-force lockouts for this user's username, email, and canonical uid
            let id_user_suffix = format!(":id:{}", hex::encode(hash_token_sha256(&lower_username)));
            let id_email_suffix = format!(":id:{}", hex::encode(hash_token_sha256(&target_email)));
            let uid_suffix = format!(":uid:{}", user_id.0);
            guard.failed_logins.retain(|k, _| {
                !k.ends_with(&id_user_suffix)
                    && !k.ends_with(&id_email_suffix)
                    && !k.ends_with(&uid_suffix)
            });

            let snap = self.maybe_snapshot(&mut guard);
            (profile, snap)
        };

        self.persist_snapshot_unlocked(snapshot)?;
        Ok(profile)
    }

    // ========================================================================
    // 3. Login with Argon2id + Scoped Dual-Key Sliding-Window Rate Limiting
    // ========================================================================

    fn is_rate_limited_locked(state: &mut AuthState, key: &str, now: DateTime<Utc>) -> bool {
        let cutoff = now - Duration::seconds(LOGIN_RATE_LIMIT_WINDOW_SECS);
        if let Some(attempts) = state.failed_logins.get_mut(key) {
            attempts.retain(|&ts| ts > cutoff);
            return attempts.len() >= MAX_LOGIN_ATTEMPTS;
        }
        false
    }

    fn record_failed_login_locked(state: &mut AuthState, key: String, now: DateTime<Utc>) {
        let cutoff = now - Duration::seconds(LOGIN_RATE_LIMIT_WINDOW_SECS);
        if state.failed_logins.len() >= MAX_RATE_LIMIT_TRACKED_KEYS
            && !state.failed_logins.contains_key(&key)
        {
            state.failed_logins.retain(|_, list| {
                list.retain(|&ts| ts > cutoff);
                !list.is_empty()
            });
            if state.failed_logins.len() >= MAX_RATE_LIMIT_TRACKED_KEYS {
                // Fail-closed under map saturation: evict a non-account (`:id:`) key with the
                // fewest attempts (or any key as fallback) rather than dropping the new attempt.
                if let Some(evict_key) = state
                    .failed_logins
                    .iter()
                    .filter(|(k, _)| k.contains(":id:"))
                    .min_by_key(|(_, v)| v.len())
                    .or_else(|| state.failed_logins.iter().min_by_key(|(_, v)| v.len()))
                    .map(|(k, _)| k.clone())
                {
                    state.failed_logins.remove(&evict_key);
                }
            }
        }
        let entry = state.failed_logins.entry(key).or_default();
        entry.retain(|&ts| ts > cutoff);
        entry.push(now);
    }

    /// Authenticates a user by username or email + password with sliding-window rate limiting.
    pub fn login(
        &self,
        identifier: &str,
        password: &str,
    ) -> Result<LoginSessionResponse, AuthError> {
        self.login_with_client_scope(identifier, password, None)
    }

    /// Authenticates a user by username or email + password, scoping brute-force rate-limit
    /// buckets by `client_scope` (e.g. client IP) so an attacker from one source cannot lock out
    /// a legitimate user or SiteAdmin logging in from another source, while still blocking
    /// username/email alternation from the same caller scope.
    pub fn login_with_client_scope(
        &self,
        identifier: &str,
        password: &str,
        client_scope: Option<&str>,
    ) -> Result<LoginSessionResponse, AuthError> {
        let trimmed_id = identifier.trim();
        if trimmed_id.is_empty() || trimmed_id.len() > 254 || password.len() > 256 {
            return Err(AuthError::InvalidCredentials);
        }
        let norm_id = trimmed_id.to_ascii_lowercase();
        let scope = client_scope
            .map(str::trim)
            .filter(|s| !s.is_empty() && s.len() <= 128)
            .unwrap_or("default");
        let id_key = scoped_id_rate_limit_key(scope, &norm_id);

        let now = Utc::now();
        let (maybe_user_id, phc_to_verify) = {
            let mut guard = self.state.write();
            let uid = guard
                .username_index
                .get(&norm_id)
                .copied()
                .or_else(|| guard.email_index.get(&norm_id).copied());

            let acct_key = uid.map(|u| scoped_account_rate_limit_key(scope, u));
            if Self::is_rate_limited_locked(&mut guard, &id_key, now)
                || acct_key
                    .as_deref()
                    .is_some_and(|k| Self::is_rate_limited_locked(&mut guard, k, now))
            {
                return Err(AuthError::RateLimitExceeded);
            }

            if let Some(id) = uid {
                if let Some(user) = guard.users.get(&id) {
                    (Some(id), user.password_hash.clone())
                } else {
                    (None, dummy_password_hash().to_owned())
                }
            } else {
                (None, dummy_password_hash().to_owned())
            }
        };

        // Run Argon2id verification outside the lock
        let password_valid = verify_password_argon2(password, &phc_to_verify);

        let (response, snapshot) = {
            let mut guard = self.state.write();
            let check_now = Utc::now();
            let acct_key = maybe_user_id.map(|u| scoped_account_rate_limit_key(scope, u));

            if Self::is_rate_limited_locked(&mut guard, &id_key, check_now)
                || acct_key
                    .as_deref()
                    .is_some_and(|k| Self::is_rate_limited_locked(&mut guard, k, check_now))
            {
                return Err(AuthError::RateLimitExceeded);
            }

            let Some(user_id) = maybe_user_id else {
                Self::record_failed_login_locked(&mut guard, id_key, check_now);
                return Err(AuthError::InvalidCredentials);
            };

            if !password_valid {
                Self::record_failed_login_locked(&mut guard, id_key, check_now);
                if let Some(ak) = acct_key {
                    Self::record_failed_login_locked(&mut guard, ak, check_now);
                }
                return Err(AuthError::InvalidCredentials);
            }

            let Some(user) = guard.users.get(&user_id) else {
                return Err(AuthError::InvalidCredentials);
            };
            if user.password_hash != phc_to_verify {
                return Err(AuthError::InvalidCredentials);
            }
            if user.status != AccountStatus::Active {
                return Err(AuthError::AccountDeactivated);
            }

            let profile = user.to_public_profile();
            let lower_username = user.username.to_ascii_lowercase();
            let lower_email = user.email.to_ascii_lowercase();

            guard.failed_logins.remove(&id_key);
            guard
                .failed_logins
                .remove(&scoped_id_rate_limit_key(scope, &lower_username));
            guard
                .failed_logins
                .remove(&scoped_id_rate_limit_key(scope, &lower_email));
            if let Some(ak) = acct_key {
                guard.failed_logins.remove(&ak);
            }

            let raw_session_token = generate_csprng_token("gitust_ses");
            let token_sha256 = hash_token_sha256(&raw_session_token);
            let expires_at = check_now + Duration::hours(DEFAULT_SESSION_TTL_HOURS);
            let session_id = SessionId::new();

            guard.sessions_by_hash.insert(
                token_sha256,
                StoredSession {
                    id: session_id,
                    user_id,
                    token_sha256,
                    created_at: check_now,
                    expires_at,
                    revoked: false,
                },
            );
            let snap = self.maybe_snapshot(&mut guard);
            (
                LoginSessionResponse {
                    session_id,
                    raw_session_token,
                    user: profile,
                    expires_at,
                },
                snap,
            )
        };

        self.persist_snapshot_unlocked(snapshot)?;
        Ok(response)
    }

    // ========================================================================
    // 4. O(1) Session, PAT (HTTPS Git), and SSH Key Authentication
    // ========================================================================

    /// Validates a session bearer token in O(1) time.
    /// Immediately rejects deactivated accounts (`AccountDeactivated`) or revoked/expired sessions.
    pub fn authenticate_session(
        &self,
        raw_session_token: &str,
    ) -> Result<AuthenticatedPrincipal, AuthError> {
        let candidate_hash = hash_token_sha256(raw_session_token);
        let guard = self.state.read();

        let Some(session) = guard.sessions_by_hash.get(&candidate_hash) else {
            return Err(AuthError::Unauthenticated);
        };
        if session.token_sha256.ct_eq(&candidate_hash).unwrap_u8() != 1 {
            return Err(AuthError::Unauthenticated);
        }

        let Some(user) = guard.users.get(&session.user_id) else {
            return Err(AuthError::Unauthenticated);
        };
        if user.status != AccountStatus::Active {
            return Err(AuthError::AccountDeactivated);
        }
        if session.revoked || Utc::now() >= session.expires_at {
            return Err(AuthError::Unauthenticated);
        }

        Ok(user.to_principal())
    }

    /// Issues a Personal Access Token (PAT) for HTTPS Git operations (`clone`/`fetch`/`push`).
    pub fn create_personal_access_token(
        &self,
        caller: &AuthenticatedPrincipal,
        name: &str,
        ttl_days: Option<i64>,
    ) -> Result<IssuedPersonalAccessToken, AuthError> {
        let clean_name = name.trim();
        if clean_name.is_empty() || clean_name.len() > 64 {
            return Err(AuthError::InvalidInput(
                "PAT name must be between 1 and 64 characters".to_owned(),
            ));
        }

        let now = Utc::now();
        let expires_at = match ttl_days {
            Some(d) if (1..=365).contains(&d) => Some(now + Duration::days(d)),
            Some(_) => {
                return Err(AuthError::InvalidInput(
                    "PAT ttl_days must be between 1 and 365 days".to_owned(),
                ))
            }
            None => None,
        };

        let raw_token = generate_csprng_token("gitust_pat");
        let token_sha256 = hash_token_sha256(&raw_token);
        let token_prefix = raw_token.chars().take(15).collect::<String>();

        let (metadata, snapshot) = {
            let mut guard = self.state.write();
            let active_caller = Self::verify_active_user_locked(&guard, caller)?;

            let pat = StoredPat {
                id: PatId::new(),
                owner_id: active_caller.user_id,
                name: clean_name.to_owned(),
                token_prefix,
                token_sha256,
                created_at: now,
                expires_at,
                revoked: false,
            };
            let metadata = pat.to_public_view();

            guard.pat_hash_index.insert(token_sha256, pat.id);
            guard.pats_by_id.insert(pat.id, pat);
            let snap = self.maybe_snapshot(&mut guard);
            (metadata, snap)
        };

        self.persist_snapshot_unlocked(snapshot)?;
        Ok(IssuedPersonalAccessToken {
            metadata,
            raw_token,
        })
    }

    /// Authenticates a Personal Access Token (PAT) for HTTPS Git access.
    /// Immediately rejects deactivated accounts (`AccountDeactivated`) or revoked/expired PATs.
    pub fn authenticate_pat(&self, raw_pat: &str) -> Result<AuthenticatedPrincipal, AuthError> {
        let candidate_hash = hash_token_sha256(raw_pat);
        let guard = self.state.read();

        let Some(&pat_id) = guard.pat_hash_index.get(&candidate_hash) else {
            return Err(AuthError::Unauthenticated);
        };
        let Some(pat) = guard.pats_by_id.get(&pat_id) else {
            return Err(AuthError::Unauthenticated);
        };
        if pat.token_sha256.ct_eq(&candidate_hash).unwrap_u8() != 1 {
            return Err(AuthError::Unauthenticated);
        }

        let Some(user) = guard.users.get(&pat.owner_id) else {
            return Err(AuthError::Unauthenticated);
        };
        if user.status != AccountStatus::Active {
            return Err(AuthError::AccountDeactivated);
        }
        if pat.revoked {
            return Err(AuthError::Unauthenticated);
        }
        if let Some(exp) = pat.expires_at {
            if Utc::now() >= exp {
                return Err(AuthError::Unauthenticated);
            }
        }

        Ok(user.to_principal())
    }

    /// Revokes a Personal Access Token (PAT) with strict BOLA/IDOR ownership verification.
    pub fn revoke_personal_access_token(
        &self,
        caller: &AuthenticatedPrincipal,
        pat_id: PatId,
    ) -> Result<PersonalAccessTokenView, AuthError> {
        let (view, snapshot) = {
            let mut guard = self.state.write();
            let active_caller = Self::verify_active_user_locked(&guard, caller)?;

            let Some(pat) = guard.pats_by_id.get_mut(&pat_id) else {
                return Err(AuthError::InvalidToken);
            };
            if pat.owner_id != active_caller.user_id
                && active_caller.role != EnterpriseRole::SiteAdmin
            {
                return Err(AuthError::ForbiddenResourceOwnership);
            }

            pat.revoked = true;
            let view = pat.to_public_view();
            let snap = self.maybe_snapshot(&mut guard);
            (view, snap)
        };

        self.persist_snapshot_unlocked(snapshot)?;
        Ok(view)
    }

    /// Registers an SSH public key for `russh` Git access after canonicalizing `<algorithm> <base64-blob>`.
    pub fn register_ssh_public_key(
        &self,
        caller: &AuthenticatedPrincipal,
        title: &str,
        public_key_openssh: &str,
    ) -> Result<SshPublicKeyView, AuthError> {
        let clean_title = title.trim();
        if clean_title.is_empty() || clean_title.len() > 64 {
            return Err(AuthError::InvalidInput(
                "SSH key title must be between 1 and 64 characters".to_owned(),
            ));
        }

        let (canonical_key, fingerprint_sha256) = canonicalize_openssh_pubkey(public_key_openssh)?;

        let (view, snapshot) = {
            let mut guard = self.state.write();
            let active_caller = Self::verify_active_user_locked(&guard, caller)?;

            if guard
                .ssh_fingerprint_index
                .contains_key(&fingerprint_sha256)
            {
                return Err(AuthError::InvalidInput(
                    "SSH public key already registered".to_owned(),
                ));
            }

            let record = StoredSshKey {
                id: SshKeyId::new(),
                owner_id: active_caller.user_id,
                title: clean_title.to_owned(),
                public_key_openssh: canonical_key,
                fingerprint_sha256: fingerprint_sha256.clone(),
                created_at: Utc::now(),
                revoked: false,
            };
            let view = record.to_public_view();

            guard
                .ssh_fingerprint_index
                .insert(fingerprint_sha256, record.id);
            guard.ssh_keys_by_id.insert(record.id, record);
            let snap = self.maybe_snapshot(&mut guard);
            (view, snap)
        };

        self.persist_snapshot_unlocked(snapshot)?;
        Ok(view)
    }

    /// Authenticates an SSH public key for `russh` Git operations.
    /// Canonicalizes `<algorithm> <base64-blob>` so trailing comment variations still match.
    pub fn authenticate_ssh_public_key(
        &self,
        public_key_openssh: &str,
    ) -> Result<AuthenticatedPrincipal, AuthError> {
        let (_canonical_key, fingerprint_sha256) = canonicalize_openssh_pubkey(public_key_openssh)
            .map_err(|_| AuthError::Unauthenticated)?;

        let guard = self.state.read();
        let Some(&key_id) = guard.ssh_fingerprint_index.get(&fingerprint_sha256) else {
            return Err(AuthError::Unauthenticated);
        };
        let Some(key_rec) = guard.ssh_keys_by_id.get(&key_id) else {
            return Err(AuthError::Unauthenticated);
        };

        let Some(user) = guard.users.get(&key_rec.owner_id) else {
            return Err(AuthError::Unauthenticated);
        };
        if user.status != AccountStatus::Active {
            return Err(AuthError::AccountDeactivated);
        }
        if key_rec.revoked {
            return Err(AuthError::Unauthenticated);
        }

        Ok(user.to_principal())
    }

    /// Revokes an SSH public key with strict BOLA/IDOR ownership verification.
    pub fn revoke_ssh_public_key(
        &self,
        caller: &AuthenticatedPrincipal,
        ssh_key_id: SshKeyId,
    ) -> Result<SshPublicKeyView, AuthError> {
        let (view, snapshot) = {
            let mut guard = self.state.write();
            let active_caller = Self::verify_active_user_locked(&guard, caller)?;

            let Some(key_rec) = guard.ssh_keys_by_id.get_mut(&ssh_key_id) else {
                return Err(AuthError::InvalidToken);
            };
            if key_rec.owner_id != active_caller.user_id
                && active_caller.role != EnterpriseRole::SiteAdmin
            {
                return Err(AuthError::ForbiddenResourceOwnership);
            }

            key_rec.revoked = true;
            let view = key_rec.to_public_view();
            let snap = self.maybe_snapshot(&mut guard);
            (view, snap)
        };

        self.persist_snapshot_unlocked(snapshot)?;
        Ok(view)
    }

    // ========================================================================
    // 5. Instant Account Deactivation & Team Repository BOLA Enforcement
    // ========================================================================

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

    pub fn list_invites(
        &self,
        caller: &AuthenticatedPrincipal,
    ) -> Result<Vec<InvitePublicView>, AuthError> {
        let guard = self.state.read();
        Self::verify_active_admin_locked(&guard, caller)?;
        Ok(guard
            .invites_by_id
            .values()
            .map(StoredInviteToken::to_public_view)
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_store_with_admin() -> (AuthStore, AuthenticatedPrincipal) {
        let store = AuthStore::new_in_memory();
        let admin_profile = store
            .bootstrap_initial_admin_cli(
                "rootadmin",
                "rootadmin@company.internal",
                "SuperSecretAdminPass!2026",
            )
            .expect("initial CLI admin bootstrap must succeed");

        let login_res = store
            .login("rootadmin", "SuperSecretAdminPass!2026")
            .expect("admin login must succeed");
        let principal = store
            .authenticate_session(&login_res.raw_session_token)
            .expect("session auth must succeed");

        assert_eq!(admin_profile.id, principal.user_id);
        (store, principal)
    }

    #[test]
    fn gate_cli_only_initial_admin_and_no_second_bootstrap() {
        let (store, _) = setup_store_with_admin();

        let second = store.bootstrap_initial_admin_cli(
            "attacker",
            "attacker@company.internal",
            "AnotherSecretPass!2026",
        );
        assert_eq!(second, Err(AuthError::AdminAlreadyBootstrapped));
    }

    #[test]
    fn gate_invite_link_single_use_expiration_and_revocation() {
        let (store, admin) = setup_store_with_admin();
        let team_a = TeamId::new();

        // 1. Issue valid invite link and redeem once -> succeeds
        let issued = store
            .create_invite_link(
                &admin,
                "alice@company.internal",
                EnterpriseRole::Developer,
                vec![team_a],
                None,
            )
            .expect("admin should issue invite");

        let alice = store
            .redeem_invite_link(&issued.raw_token, "alice", "AliceStrongPass!2026")
            .expect("first redemption must succeed");
        assert_eq!(alice.username, "alice");

        // 2. Reusing the exact same invite link a second time must be rejected (One-Time Use)
        let reuse_err =
            store.redeem_invite_link(&issued.raw_token, "alice2", "AliceStrongPass!2026");
        assert_eq!(reuse_err, Err(AuthError::TokenAlreadyUsed));

        // 3. Revoked invite link must be rejected
        let issued_bob = store
            .create_invite_link(
                &admin,
                "bob@company.internal",
                EnterpriseRole::Developer,
                vec![team_a],
                None,
            )
            .expect("admin should issue bob invite");
        store
            .revoke_invite_link(&admin, issued_bob.invite.id)
            .expect("admin should revoke bob invite");

        let revoked_err =
            store.redeem_invite_link(&issued_bob.raw_token, "bob", "BobStrongPass!2026");
        assert_eq!(revoked_err, Err(AuthError::TokenRevoked));

        // 4. Expired invite link must be rejected
        let issued_charlie = store
            .create_invite_link(
                &admin,
                "charlie@company.internal",
                EnterpriseRole::Developer,
                vec![team_a],
                Some(1),
            )
            .expect("admin should issue charlie invite");
        {
            let mut guard = store.state.write();
            if let Some(inv) = guard.invites_by_id.get_mut(&issued_charlie.invite.id) {
                inv.expires_at = Utc::now() - Duration::minutes(1);
            }
        }
        let expired_err = store.redeem_invite_link(
            &issued_charlie.raw_token,
            "charlie",
            "CharlieStrongPass!2026",
        );
        assert_eq!(expired_err, Err(AuthError::TokenExpired));
    }

    #[test]
    fn gate_deactivated_account_immediately_invalidates_session_pat_ssh_and_reset_links() {
        let (store, admin) = setup_store_with_admin();
        let team_a = TeamId::new();

        let invite = store
            .create_invite_link(
                &admin,
                "dev1@company.internal",
                EnterpriseRole::Developer,
                vec![team_a],
                None,
            )
            .expect("invite must succeed");
        let dev_profile = store
            .redeem_invite_link(&invite.raw_token, "dev1", "Dev1StrongPass!2026")
            .expect("redeem must succeed");

        let session = store
            .login("dev1", "Dev1StrongPass!2026")
            .expect("login must succeed");
        let dev_principal = store
            .authenticate_session(&session.raw_session_token)
            .expect("session valid");

        let pat = store
            .create_personal_access_token(&dev_principal, "laptop-git-https", Some(30))
            .expect("pat creation must succeed");
        let ssh_key_str =
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIGitustTestKeyDev100 dev1@company.internal";
        store
            .register_ssh_public_key(&dev_principal, "MacBook Pro", ssh_key_str)
            .expect("ssh key registration must succeed");

        let reset_link = store
            .create_password_reset_link(&admin, dev_profile.id, None)
            .expect("issue password reset");

        // Verify all 3 credentials work before deactivation
        assert!(store
            .authenticate_session(&session.raw_session_token)
            .is_ok());
        assert!(store.authenticate_pat(&pat.raw_token).is_ok());
        assert!(store.authenticate_ssh_public_key(ssh_key_str).is_ok());

        // SiteAdmin deactivates dev1
        store
            .deactivate_user(&admin, dev_profile.id)
            .expect("deactivation must succeed");

        // All credentials + pending reset link + re-login must immediately fail with AccountDeactivated / TokenRevoked!
        assert_eq!(
            store.authenticate_session(&session.raw_session_token),
            Err(AuthError::AccountDeactivated)
        );
        assert_eq!(
            store.authenticate_pat(&pat.raw_token),
            Err(AuthError::AccountDeactivated)
        );
        assert_eq!(
            store.authenticate_ssh_public_key(ssh_key_str),
            Err(AuthError::AccountDeactivated)
        );
        assert_eq!(
            store.redeem_password_reset_link(&reset_link.raw_token, "NewDevPass!2026"),
            Err(AuthError::TokenRevoked)
        );
        assert_eq!(
            store.login("dev1", "Dev1StrongPass!2026").unwrap_err(),
            AuthError::AccountDeactivated
        );
    }

    #[test]
    fn gate_idor_bola_cross_user_token_and_cross_team_repo_blocked() {
        let (store, admin) = setup_store_with_admin();
        let team_alpha = TeamId::new();
        let team_beta = TeamId::new();

        let inv_a = store
            .create_invite_link(
                &admin,
                "alpha@company.internal",
                EnterpriseRole::Developer,
                vec![team_alpha],
                None,
            )
            .expect("invite alpha");
        store
            .redeem_invite_link(&inv_a.raw_token, "alpha", "AlphaStrongPass!2026")
            .expect("redeem alpha");
        let ses_a = store
            .login("alpha", "AlphaStrongPass!2026")
            .expect("login alpha");
        let principal_a = store
            .authenticate_session(&ses_a.raw_session_token)
            .expect("auth alpha");

        let inv_b = store
            .create_invite_link(
                &admin,
                "beta@company.internal",
                EnterpriseRole::Developer,
                vec![team_beta],
                None,
            )
            .expect("invite beta");
        store
            .redeem_invite_link(&inv_b.raw_token, "beta", "BetaStrongPass!2026")
            .expect("redeem beta");
        let ses_b = store
            .login("beta", "BetaStrongPass!2026")
            .expect("login beta");
        let principal_b = store
            .authenticate_session(&ses_b.raw_session_token)
            .expect("auth beta");

        // 1. BFLA: Non-admin User Beta tries to issue an invite link -> must be blocked
        let bfla_invite = store.create_invite_link(
            &principal_b,
            "friend@company.internal",
            EnterpriseRole::Developer,
            vec![],
            None,
        );
        assert_eq!(bfla_invite.unwrap_err(), AuthError::AdminPrivilegeRequired);

        // 2. IDOR / BOLA: User Beta tries to revoke User Alpha's PAT & SSH key
        let alpha_pat = store
            .create_personal_access_token(&principal_a, "alpha-token", None)
            .expect("alpha pat");
        let alpha_ssh = store
            .register_ssh_public_key(
                &principal_a,
                "alpha-key",
                "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAlphaKey000 comment@host",
            )
            .expect("alpha ssh");

        assert_eq!(
            store.revoke_personal_access_token(&principal_b, alpha_pat.metadata.id),
            Err(AuthError::ForbiddenResourceOwnership)
        );
        assert_eq!(
            store.revoke_ssh_public_key(&principal_b, alpha_ssh.id),
            Err(AuthError::ForbiddenResourceOwnership)
        );

        // 3. Canonical SSH key check: re-registering same key with a different comment must be rejected
        assert!(store
            .register_ssh_public_key(
                &principal_b,
                "duplicate-key-different-comment",
                "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAlphaKey000 attacker@otherhost",
            )
            .is_err());

        // 4. Cross-Team Repository BOLA: User Beta tries to access Team Alpha's private repository
        let alpha_repo = store
            .create_team_repository(&principal_a, "alpha-core-engine", team_alpha, true)
            .expect("create team alpha repo");

        assert!(store
            .authorize_repository_access(&principal_a, alpha_repo.id)
            .is_ok());
        assert_eq!(
            store.authorize_repository_access(&principal_b, alpha_repo.id),
            Err(AuthError::ForbiddenRepositoryAccess)
        );
    }

    #[test]
    fn gate_login_rate_limiting_blocks_dual_identifier_alternation_and_password_reset_clears() {
        let (store, admin) = setup_store_with_admin();
        let root_id = admin.user_id;

        // Alternate between username ("rootadmin") and email ("rootadmin@company.internal")
        for i in 0..MAX_LOGIN_ATTEMPTS {
            let id_str = if i % 2 == 0 {
                "rootadmin"
            } else {
                "rootadmin@company.internal"
            };
            let res = store.login(id_str, "WrongPassword123!");
            assert_eq!(res.unwrap_err(), AuthError::InvalidCredentials);
        }

        // Next attempt using EITHER username or email must be blocked by canonical uid rate limiter!
        assert_eq!(
            store
                .login("rootadmin", "SuperSecretAdminPass!2026")
                .unwrap_err(),
            AuthError::RateLimitExceeded
        );
        assert_eq!(
            store
                .login("rootadmin@company.internal", "SuperSecretAdminPass!2026")
                .unwrap_err(),
            AuthError::RateLimitExceeded
        );

        // An attacker locking out their own client scope must NOT lock out the legitimate admin
        // from a different client scope (Account Lockout DoS defense).
        assert!(store
            .login_with_client_scope("rootadmin", "SuperSecretAdminPass!2026", Some("10.0.0.10"),)
            .is_ok());

        // Redeeming a password reset link clears the lockout and rotates the password
        let reset = store
            .create_password_reset_link(&admin, root_id, None)
            .expect("issue reset");
        store
            .redeem_password_reset_link(&reset.raw_token, "RotatedAdminPass!2026")
            .expect("redeem reset");

        assert!(store.login("rootadmin", "RotatedAdminPass!2026").is_ok());

        // Flooding MAX_RATE_LIMIT_TRACKED_KEYS fake identifier keys must NOT fail open or allow
        // brute-forcing a real account.
        {
            let now = Utc::now();
            let mut st = store.state.write();
            for i in 0..MAX_RATE_LIMIT_TRACKED_KEYS {
                AuthStore::record_failed_login_locked(
                    &mut st,
                    format!("scope:flood:id:fake_{i}"),
                    now,
                );
            }
        }
        for _ in 0..MAX_LOGIN_ATTEMPTS {
            let _ = store.login_with_client_scope("rootadmin", "WrongPass!999", Some("flood"));
        }
        assert_eq!(
            store
                .login_with_client_scope("rootadmin", "RotatedAdminPass!2026", Some("flood"))
                .unwrap_err(),
            AuthError::RateLimitExceeded
        );

        // Oversized (1.9 MiB) login identifiers are rejected before allocating or inserting into
        // `failed_logins`, and all stored `:id:` keys use fixed-length SHA-256 hex digests.
        let huge_id = "x".repeat(1_900_000);
        assert_eq!(
            store
                .login_with_client_scope(&huge_id, "WrongPass!999", Some("huge"))
                .unwrap_err(),
            AuthError::InvalidCredentials
        );
        assert!(store
            .state
            .read()
            .failed_logins
            .keys()
            .all(|k| k.len() <= 200));
    }

    #[test]
    fn ssh_pubkey_rejects_mismatched_wire_header_and_malformed_base64() {
        // Declared `ssh-rsa`, but wire blob header encodes `ssh-ed25519`.
        assert!(canonicalize_openssh_pubkey(
            "ssh-rsa AAAAC3NzaC1lZDI1NTE5AAAAIAlphaKey000 user@host"
        )
        .is_err());
        // Non-4-byte-aligned base64 blob.
        assert!(canonicalize_openssh_pubkey(
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAlphaKey user@host"
        )
        .is_err());
    }

    #[test]
    fn gate_persistent_db_never_writes_raw_tokens_to_disk() {
        let temp_dir = tempfile::tempdir().expect("create tempdir");
        let db_path = temp_dir.path().join("gitust-auth.json");

        let store = AuthStore::open_persistent(&db_path).expect("open persistent store");
        store
            .bootstrap_initial_admin_cli(
                "admin1",
                "admin1@company.internal",
                "Admin1StrongPass!2026",
            )
            .expect("bootstrap admin");
        let login = store
            .login("admin1", "Admin1StrongPass!2026")
            .expect("login admin");
        let admin_principal = store
            .authenticate_session(&login.raw_session_token)
            .expect("auth session");

        let issued_invite = store
            .create_invite_link(
                &admin_principal,
                "newdev@company.internal",
                EnterpriseRole::Developer,
                vec![],
                None,
            )
            .expect("create invite");

        let on_disk_json = fs::read_to_string(&db_path).expect("read db file");
        assert!(!on_disk_json.contains(&issued_invite.raw_token));
        assert!(!on_disk_json.contains(&login.raw_session_token));
        assert!(!on_disk_json.contains("Admin1StrongPass!2026"));
    }
}
