//! Enterprise OIDC/LDAP Single Sign-On (SSO) provider store and invite-bound assertion verification.

use argon2::password_hash::rand_core::OsRng;
use chrono::{Duration, Utc};
use gitust_core::{
    AccountStatus, AuthError, AuthenticatedPrincipal, InviteTokenPurpose, SessionId, SsoProtocol,
    SsoProviderRecord, UserId,
};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use uuid::Uuid;

pub(crate) use crate::audit_trail::append_audit_event_locked;
pub use crate::audit_trail::{compute_audit_entry_hash, GENESIS_AUDIT_HASH};
use crate::crypto::{
    dummy_password_hash, generate_csprng_token, hash_token_sha256, scoped_id_rate_limit_key,
    validate_username,
};
use crate::models::{LoginSessionResponse, SessionPurpose, StoredSession, StoredUser};
use crate::mutation_authority::MutationAuthority;
use crate::totp::{hmac_sha256, verify_user_totp_locked};
use crate::{AuthStore, DEFAULT_SESSION_TTL_HOURS};

const MAX_SSO_PROVIDERS: usize = 32;

const fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct StoredSsoProvider {
    pub(crate) record: SsoProviderRecord,
    pub(crate) key_hex: String,
    pub(crate) nonce_hex: String,
    pub(crate) ciphertext_hex: String,
    pub(crate) tag_sha256: [u8; 32],
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisterSsoProviderInput {
    pub name: String,
    pub protocol: SsoProtocol,
    pub issuer_or_url: String,
    pub client_id: String,
    pub signing_secret: String,
    pub allowed_domains: Vec<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub require_invite_binding: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SsoAssertionLoginInput {
    pub provider_id: Uuid,
    pub email: String,
    pub username: String,
    pub exp_epoch_secs: u64,
    pub signature_hex: String,
    #[serde(default)]
    pub invite_token: Option<String>,
    #[serde(default)]
    pub totp_code: Option<String>,
}

/// Computes the expected HMAC-SHA256 hex signature over an SSO assertion payload.
#[must_use]
pub fn compute_sso_assertion_signature(
    signing_secret: &str,
    provider_id: Uuid,
    email: &str,
    username: &str,
    exp_epoch_secs: u64,
) -> String {
    let norm_email = email.trim().to_ascii_lowercase();
    let norm_user = username.trim().to_ascii_lowercase();
    let msg = format!("gitust-sso-v1|{provider_id}|{norm_email}|{norm_user}|{exp_epoch_secs}");
    hex::encode(hmac_sha256(signing_secret.as_bytes(), msg.as_bytes()))
}

fn sso_keystream_xor(key: &[u8; 32], nonce: &[u8; 16], data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    for (block_idx, chunk) in data.chunks(32).enumerate() {
        let mut hasher = Sha256::new();
        hasher.update(b"gitust-sso-secret-stream-v1:");
        hasher.update(key);
        hasher.update(nonce);
        hasher.update((block_idx as u64).to_le_bytes());
        let stream: [u8; 32] = hasher.finalize().into();
        for (b, s) in chunk.iter().zip(stream.iter()) {
            out.push(b ^ s);
        }
    }
    out
}

fn compute_sso_tag(
    key: &[u8; 32],
    nonce: &[u8; 16],
    provider_id: Uuid,
    ciphertext: &[u8],
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"gitust-sso-secret-mac-v1:");
    hasher.update(key);
    hasher.update(nonce);
    hasher.update(provider_id.as_bytes());
    hasher.update((ciphertext.len() as u64).to_le_bytes());
    hasher.update(ciphertext);
    hasher.finalize().into()
}

fn encrypt_sso_provider(record: SsoProviderRecord, signing_secret: &str) -> StoredSsoProvider {
    let mut key = [0u8; 32];
    let mut nonce = [0u8; 16];
    OsRng.fill_bytes(&mut key);
    OsRng.fill_bytes(&mut nonce);
    let ciphertext = sso_keystream_xor(&key, &nonce, signing_secret.as_bytes());
    let tag_sha256 = compute_sso_tag(&key, &nonce, record.id, &ciphertext);
    StoredSsoProvider {
        record,
        key_hex: hex::encode(key),
        nonce_hex: hex::encode(nonce),
        ciphertext_hex: hex::encode(ciphertext),
        tag_sha256,
    }
}

fn decrypt_sso_secret(stored: &StoredSsoProvider) -> Result<String, AuthError> {
    let key: [u8; 32] = hex::decode(&stored.key_hex)
        .map_err(|_| AuthError::StorageError("corrupted SSO key".into()))?
        .try_into()
        .map_err(|_| AuthError::StorageError("corrupted SSO key length".into()))?;
    let nonce: [u8; 16] = hex::decode(&stored.nonce_hex)
        .map_err(|_| AuthError::StorageError("corrupted SSO nonce".into()))?
        .try_into()
        .map_err(|_| AuthError::StorageError("corrupted SSO nonce length".into()))?;
    let ciphertext = hex::decode(&stored.ciphertext_hex)
        .map_err(|_| AuthError::StorageError("corrupted SSO ciphertext".into()))?;
    let expected_tag = compute_sso_tag(&key, &nonce, stored.record.id, &ciphertext);
    if !bool::from(expected_tag.ct_eq(&stored.tag_sha256)) {
        return Err(AuthError::StorageError(
            "SSO secret integrity verification failed".into(),
        ));
    }
    String::from_utf8(sso_keystream_xor(&key, &nonce, &ciphertext))
        .map_err(|_| AuthError::StorageError("corrupted SSO secret UTF-8".into()))
}

impl AuthStore {
    pub fn list_sso_providers_for_admin(
        &self,
        caller: &AuthenticatedPrincipal,
    ) -> Result<Vec<SsoProviderRecord>, AuthError> {
        let guard = self.read_state()?;
        Self::verify_active_admin_locked(&guard, caller)?;
        Ok(guard
            .sso_providers
            .iter()
            .map(|p| p.record.clone())
            .collect())
    }

    pub fn register_sso_provider_with_credential(
        &self,
        credential: &str,
        original_user: UserId,
        input: RegisterSsoProviderInput,
    ) -> Result<SsoProviderRecord, AuthError> {
        let name = input.name.trim().to_owned();
        let issuer = input.issuer_or_url.trim().to_owned();
        let client_id = input.client_id.trim().to_owned();
        let secret = input.signing_secret.trim();
        if name.is_empty()
            || name.len() > 64
            || name.chars().any(|c| c.is_control() || c == '|')
            || issuer.is_empty()
            || issuer.len() > 256
            || issuer.chars().any(|c| c.is_control() || c.is_whitespace())
            || client_id.is_empty()
            || client_id.len() > 128
            || client_id
                .chars()
                .any(|c| c.is_control() || c.is_whitespace())
            || secret.len() < 16
            || secret.len() > 256
            || input.allowed_domains.is_empty()
            || input.allowed_domains.len() > 16
        {
            return Err(AuthError::InvalidInput(
                "invalid SSO provider parameters or signing secret length (< 16 bytes)".into(),
            ));
        }
        let mut allowed_domains = Vec::with_capacity(input.allowed_domains.len());
        for dom in input.allowed_domains {
            let clean_dom = dom.trim().trim_start_matches('@').to_ascii_lowercase();
            if clean_dom.is_empty()
                || clean_dom.len() > 128
                || clean_dom
                    .chars()
                    .any(|c| c.is_control() || c.is_whitespace() || c == '@')
            {
                return Err(AuthError::InvalidInput("invalid allowed domain".into()));
            }
            allowed_domains.push(clean_dom);
        }
        let record = SsoProviderRecord {
            id: Uuid::new_v4(),
            name,
            protocol: input.protocol,
            issuer_or_url: issuer,
            client_id,
            allowed_domains,
            enabled: input.enabled,
            require_invite_binding: input.require_invite_binding,
            created_at: Utc::now().to_rfc3339(),
        };
        let stored = encrypt_sso_provider(record.clone(), secret);
        let snapshot = {
            let mut guard = self.write_state()?;
            let admin_id =
                MutationAuthority::Credential(credential, original_user).verify(&guard)?;
            if guard.sso_providers.len() >= MAX_SSO_PROVIDERS {
                return Err(AuthError::InvalidInput(
                    "SSO provider limit exceeded".into(),
                ));
            }
            if guard
                .sso_providers
                .iter()
                .any(|p| p.record.name.eq_ignore_ascii_case(&record.name))
            {
                return Err(AuthError::InvalidInput(
                    "SSO provider name already exists".into(),
                ));
            }
            let admin_username = guard
                .users
                .get(&admin_id)
                .map(|u| u.username.clone())
                // fail-open-ok: verified active admin exists in guard.users.
                .unwrap_or_default();
            guard.sso_providers.push(stored);
            append_audit_event_locked(
                &mut guard,
                Some(admin_id),
                &admin_username,
                "sso.provider.register",
                &format!("provider:{}:{}", record.protocol.as_str(), record.name),
            );
            self.maybe_snapshot(guard)
        };
        self.persist_snapshot(snapshot)?;
        Ok(record)
    }

    pub fn delete_sso_provider_with_credential(
        &self,
        credential: &str,
        original_user: UserId,
        provider_id: Uuid,
    ) -> Result<(), AuthError> {
        let snapshot = {
            let mut guard = self.write_state()?;
            let admin_id =
                MutationAuthority::Credential(credential, original_user).verify(&guard)?;
            let before = guard.sso_providers.len();
            guard.sso_providers.retain(|p| p.record.id != provider_id);
            if guard.sso_providers.len() == before {
                return Err(AuthError::InvalidInput("SSO provider not found".into()));
            }
            let admin_username = guard
                .users
                .get(&admin_id)
                .map(|u| u.username.clone())
                // fail-open-ok: verified active admin exists in guard.users.
                .unwrap_or_default();
            append_audit_event_locked(
                &mut guard,
                Some(admin_id),
                &admin_username,
                "sso.provider.delete",
                &format!("provider:{provider_id}"),
            );
            self.maybe_snapshot(guard)
        };
        self.persist_snapshot(snapshot)
    }

    pub fn login_with_sso_assertion(
        &self,
        input: SsoAssertionLoginInput,
        client_scope: Option<&str>,
    ) -> Result<LoginSessionResponse, AuthError> {
        let clean_email = input.email.trim().to_ascii_lowercase();
        let clean_username = validate_username(&input.username)?;
        let scope = client_scope
            .map(str::trim)
            .filter(|s| !s.is_empty() && s.len() <= 128)
            // fail-open-ok: absent or invalid caller scope shares a fixed restrictive rate-limit bucket.
            .unwrap_or("default");
        let rl_key = scoped_id_rate_limit_key(scope, &clean_email);
        let (response, snapshot) = {
            let mut guard = self.write_state()?;
            let now = Utc::now();
            if Self::is_rate_limited_locked(&mut guard, &rl_key, now) {
                return Err(AuthError::RateLimitExceeded);
            }
            // fail-open-ok: UTC epoch timestamp is non-negative.
            let now_secs = u64::try_from(now.timestamp()).unwrap_or(0);
            if input.exp_epoch_secs <= now_secs
                || input.exp_epoch_secs > now_secs.saturating_add(600)
            {
                Self::record_failed_login_locked(&mut guard, rl_key, now);
                return Err(AuthError::InvalidCredentials);
            }
            let Some(stored_prov) = guard
                .sso_providers
                .iter()
                .find(|p| p.record.id == input.provider_id && p.record.enabled)
                .cloned()
            else {
                Self::record_failed_login_locked(&mut guard, rl_key, now);
                return Err(AuthError::InvalidCredentials);
            };
            let signing_secret = decrypt_sso_secret(&stored_prov)?;
            let expected_sig = compute_sso_assertion_signature(
                &signing_secret,
                input.provider_id,
                &clean_email,
                &clean_username,
                input.exp_epoch_secs,
            );
            if !bool::from(
                expected_sig
                    .as_bytes()
                    .ct_eq(input.signature_hex.trim().as_bytes()),
            ) {
                Self::record_failed_login_locked(&mut guard, rl_key, now);
                return Err(AuthError::InvalidCredentials);
            }
            guard
                .policy
                .validate_registration(&clean_email, true)
                .map_err(|e| AuthError::InvalidInput(e.to_string()))?;
            let email_domain = clean_email
                .rsplit_once('@')
                .map(|(_, d)| d)
                // fail-open-ok: validate_registration above already guaranteed `@` is present.
                .unwrap_or_default();
            if !stored_prov
                .record
                .allowed_domains
                .iter()
                .any(|d| d.eq_ignore_ascii_case(email_domain))
            {
                return Err(AuthError::InvalidInput(
                    "email domain not allowed by SSO provider".into(),
                ));
            }
            let (user_id, profile, audit_action) = if let Some(&uid) =
                guard.email_index.get(&clean_email)
            {
                let user = guard.users.get(&uid).ok_or(AuthError::InvalidCredentials)?;
                if user.status != AccountStatus::Active {
                    return Err(AuthError::AccountDeactivated);
                }
                if !user.username.eq_ignore_ascii_case(&clean_username) {
                    Self::record_failed_login_locked(&mut guard, rl_key, now);
                    return Err(AuthError::InvalidCredentials);
                }
                let prof = user.to_public_profile();
                if let Err(err) =
                    verify_user_totp_locked(&mut guard, uid, input.totp_code.as_deref(), now_secs)
                {
                    if matches!(err, AuthError::InvalidTotpCode) {
                        Self::record_failed_login_locked(&mut guard, rl_key, now);
                    }
                    return Err(err);
                }
                (uid, prof, "sso.login")
            } else {
                let Some(raw_invite) = input
                    .invite_token
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                else {
                    return Err(AuthError::PublicSignupDisabled);
                };
                let candidate_hash = hash_token_sha256(raw_invite);
                let Some(&invite_id) = guard.invite_hash_index.get(&candidate_hash) else {
                    Self::record_failed_login_locked(&mut guard, rl_key, now);
                    return Err(AuthError::InvalidToken);
                };
                let (assigned_role, team_ids) = {
                    let inv = guard
                        .invites_by_id
                        .get(&invite_id)
                        .ok_or(AuthError::InvalidToken)?;
                    if inv.token_sha256.ct_eq(&candidate_hash).unwrap_u8() != 1
                        || now >= inv.expires_at
                        || !inv.target_email.eq_ignore_ascii_case(&clean_email)
                    {
                        Self::record_failed_login_locked(&mut guard, rl_key, now);
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
                    (inv.assigned_role, inv.team_ids.clone())
                };
                if guard
                    .username_index
                    .contains_key(&clean_username.to_ascii_lowercase())
                {
                    return Err(AuthError::DuplicateUser);
                }
                if let Some(inv_mut) = guard.invites_by_id.get_mut(&invite_id) {
                    inv_mut.used_at = Some(now);
                }
                for inv in guard.invites_by_id.values_mut() {
                    if inv.id != invite_id
                        && inv.target_email == clean_email
                        && inv.used_at.is_none()
                        && inv.revoked_at.is_none()
                    {
                        inv.revoked_at = Some(now);
                    }
                }
                let user = StoredUser {
                    id: UserId::new(),
                    username: clean_username.clone(),
                    email: clean_email.clone(),
                    password_hash: dummy_password_hash().to_owned(),
                    role: assigned_role,
                    status: AccountStatus::Active,
                    team_ids,
                    membership_version: 0,
                    created_at: now,
                };
                let prof = user.to_public_profile();
                guard
                    .username_index
                    .insert(clean_username.to_ascii_lowercase(), user.id);
                guard.email_index.insert(clean_email.clone(), user.id);
                guard.users.insert(user.id, user);
                (prof.id, prof, "sso.invite_provision")
            };
            guard.failed_logins.remove(&rl_key);
            let raw_session_token = generate_csprng_token("gitust_ses");
            let token_sha256 = hash_token_sha256(&raw_session_token);
            let expires_at = now + Duration::hours(DEFAULT_SESSION_TTL_HOURS);
            let session_id = SessionId::new();
            guard.sessions_by_hash.insert(
                token_sha256,
                StoredSession {
                    id: session_id,
                    user_id,
                    token_sha256,
                    created_at: now,
                    expires_at,
                    revoked: false,
                    purpose: Some(SessionPurpose::Login),
                },
            );
            append_audit_event_locked(
                &mut guard,
                Some(user_id),
                &profile.username,
                audit_action,
                &format!("provider:{}:user:{}", input.provider_id, profile.username),
            );
            (
                LoginSessionResponse {
                    session_id,
                    raw_session_token,
                    user: profile,
                    expires_at,
                },
                self.maybe_snapshot(guard),
            )
        };
        self.persist_snapshot(snapshot)?;
        Ok(response)
    }
}
