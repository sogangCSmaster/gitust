use chrono::{DateTime, Duration, Utc};
use gitust_core::{
    AccountStatus, AuthError, AuthenticatedPrincipal, EnterpriseRole, PatId,
    PersonalAccessTokenView, SessionId, SshKeyId, SshPublicKeyView, UserId,
};
use subtle::ConstantTimeEq;

use crate::crypto::{
    canonicalize_openssh_pubkey, dummy_password_hash, generate_csprng_token, hash_token_sha256,
    scoped_account_rate_limit_key, scoped_id_rate_limit_key, verify_password_argon2,
};
use crate::models::{
    AuthState, IssuedPersonalAccessToken, LoginSessionResponse, StoredPat, StoredSession,
    StoredSshKey,
};
use crate::{
    AuthStore, DEFAULT_SESSION_TTL_HOURS, LOGIN_RATE_LIMIT_WINDOW_SECS, MAX_LOGIN_ATTEMPTS,
    MAX_RATE_LIMIT_TRACKED_KEYS,
};

impl AuthStore {
    pub(crate) fn is_rate_limited_locked(
        state: &mut AuthState,
        key: &str,
        now: DateTime<Utc>,
    ) -> bool {
        let cutoff = now - Duration::seconds(LOGIN_RATE_LIMIT_WINDOW_SECS);
        if let Some(attempts) = state.failed_logins.get_mut(key) {
            attempts.retain(|&ts| ts > cutoff);
            return attempts.len() >= MAX_LOGIN_ATTEMPTS;
        }
        false
    }

    pub(crate) fn record_failed_login_locked(
        state: &mut AuthState,
        key: String,
        now: DateTime<Utc>,
    ) {
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
            let mut guard = self.write_state()?;
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
            let mut guard = self.write_state()?;
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

    /// Issues a session bearer token (`gitust_ses_...`) for an active user after a verified
    /// OAuth 2.0 Authorization Code + PKCE (`S256`) exchange.
    ///
    /// Using the unified `sessions_by_hash` store avoids split-brain token tables and guarantees
    /// that account deactivation immediately invalidates OAuth-issued MCP tokens.
    pub fn issue_oauth_session(&self, user_id: UserId) -> Result<LoginSessionResponse, AuthError> {
        let now = Utc::now();
        let (response, snapshot) = {
            let mut guard = self.write_state()?;
            let user = guard
                .users
                .get(&user_id)
                .ok_or(AuthError::Unauthenticated)?;
            if user.status != AccountStatus::Active {
                return Err(AuthError::AccountDeactivated);
            }
            let profile = user.to_public_profile();
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

    /// Validates a session bearer token in O(1) time.
    /// Immediately rejects deactivated accounts (`AccountDeactivated`) or revoked/expired sessions.
    pub fn authenticate_session(
        &self,
        raw_session_token: &str,
    ) -> Result<AuthenticatedPrincipal, AuthError> {
        let candidate_hash = hash_token_sha256(raw_session_token);
        let guard = self.read_state()?;

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
            let mut guard = self.write_state()?;
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
        let guard = self.read_state()?;

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
            let mut guard = self.write_state()?;
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
            let mut guard = self.write_state()?;
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

        let guard = self.read_state()?;
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
            let mut guard = self.write_state()?;
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
}
