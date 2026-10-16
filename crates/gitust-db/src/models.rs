use std::collections::HashMap;

use chrono::{DateTime, Duration, Utc};
use gitust_core::{
    AccountStatus, AuthenticatedPrincipal, EnterpriseRole, EnterpriseSecurityPolicy, InviteId,
    InvitePublicView, InviteTokenPurpose, PatId, PersonalAccessTokenView, RepoId, RepositoryRecord,
    SessionId, SshKeyId, SshPublicKeyView, TeamId, UserId, UserPublicProfile,
};
use serde::{Deserialize, Serialize};

use crate::LOGIN_RATE_LIMIT_WINDOW_SECS;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct StoredUser {
    pub(crate) id: UserId,
    pub(crate) username: String,
    pub(crate) email: String,
    pub(crate) password_hash: String,
    pub(crate) role: EnterpriseRole,
    pub(crate) status: AccountStatus,
    pub(crate) team_ids: Vec<TeamId>,
    pub(crate) created_at: DateTime<Utc>,
}

impl StoredUser {
    pub(crate) fn to_public_profile(&self) -> UserPublicProfile {
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

    pub(crate) fn to_principal(&self) -> AuthenticatedPrincipal {
        AuthenticatedPrincipal {
            user_id: self.id,
            username: self.username.clone(),
            role: self.role,
            team_ids: self.team_ids.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct StoredInviteToken {
    pub(crate) id: InviteId,
    pub(crate) token_sha256: [u8; 32],
    pub(crate) purpose: InviteTokenPurpose,
    pub(crate) target_email: String,
    pub(crate) assigned_role: EnterpriseRole,
    pub(crate) team_ids: Vec<TeamId>,
    pub(crate) created_by: UserId,
    pub(crate) created_at: DateTime<Utc>,
    pub(crate) expires_at: DateTime<Utc>,
    pub(crate) used_at: Option<DateTime<Utc>>,
    pub(crate) revoked_at: Option<DateTime<Utc>>,
}

impl StoredInviteToken {
    pub(crate) fn to_public_view(&self) -> InvitePublicView {
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
pub(crate) struct StoredSession {
    pub(crate) id: SessionId,
    pub(crate) user_id: UserId,
    pub(crate) token_sha256: [u8; 32],
    pub(crate) created_at: DateTime<Utc>,
    pub(crate) expires_at: DateTime<Utc>,
    pub(crate) revoked: bool,
    /// Missing on legacy snapshots: never infer general authority from an unscoped token.
    #[serde(default)]
    pub(crate) purpose: Option<SessionPurpose>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum SessionPurpose {
    Login,
    Mcp,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct StoredPat {
    pub(crate) id: PatId,
    pub(crate) owner_id: UserId,
    pub(crate) name: String,
    pub(crate) token_prefix: String,
    pub(crate) token_sha256: [u8; 32],
    pub(crate) created_at: DateTime<Utc>,
    pub(crate) expires_at: Option<DateTime<Utc>>,
    pub(crate) revoked: bool,
}

impl StoredPat {
    pub(crate) fn to_public_view(&self) -> PersonalAccessTokenView {
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
pub(crate) struct StoredSshKey {
    pub(crate) id: SshKeyId,
    pub(crate) owner_id: UserId,
    pub(crate) title: String,
    pub(crate) public_key_openssh: String,
    pub(crate) fingerprint_sha256: String,
    pub(crate) created_at: DateTime<Utc>,
    pub(crate) revoked: bool,
}

impl StoredSshKey {
    pub(crate) fn to_public_view(&self) -> SshPublicKeyView {
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
pub(crate) struct PersistedSnapshot {
    #[serde(default)]
    pub(crate) revision: u64,
    pub(crate) policy: EnterpriseSecurityPolicy,
    pub(crate) users: Vec<StoredUser>,
    pub(crate) invites: Vec<StoredInviteToken>,
    pub(crate) sessions: Vec<StoredSession>,
    pub(crate) pats: Vec<StoredPat>,
    pub(crate) ssh_keys: Vec<StoredSshKey>,
    pub(crate) repositories: Vec<RepositoryRecord>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct AuthState {
    pub(crate) revision: u64,
    pub(crate) policy: EnterpriseSecurityPolicy,
    pub(crate) users: HashMap<UserId, StoredUser>,
    pub(crate) username_index: HashMap<String, UserId>,
    pub(crate) email_index: HashMap<String, UserId>,
    pub(crate) invites_by_id: HashMap<InviteId, StoredInviteToken>,
    pub(crate) invite_hash_index: HashMap<[u8; 32], InviteId>,
    pub(crate) sessions_by_hash: HashMap<[u8; 32], StoredSession>,
    pub(crate) pats_by_id: HashMap<PatId, StoredPat>,
    pub(crate) pat_hash_index: HashMap<[u8; 32], PatId>,
    pub(crate) ssh_keys_by_id: HashMap<SshKeyId, StoredSshKey>,
    pub(crate) ssh_fingerprint_index: HashMap<String, SshKeyId>,
    pub(crate) repositories: HashMap<RepoId, RepositoryRecord>,
    pub(crate) failed_logins: HashMap<String, Vec<DateTime<Utc>>>,
}

impl AuthState {
    pub(crate) fn from_snapshot(snap: PersistedSnapshot) -> Self {
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
            if !key.revoked {
                state
                    .ssh_fingerprint_index
                    .insert(key.fingerprint_sha256.clone(), key.id);
            }
            state.ssh_keys_by_id.insert(key.id, key);
        }

        for repo in snap.repositories {
            state.repositories.insert(repo.id, repo);
        }

        state
    }

    pub(crate) fn prune_expired(&mut self, now: DateTime<Utc>) {
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

    pub(crate) fn snapshot_current(&mut self) -> PersistedSnapshot {
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
