use crate::crypto::{generate_csprng_token, hash_token_sha256};
use crate::models::{LoginSessionResponse, SessionPurpose, StoredSession};
use crate::{AuthStore, DEFAULT_SESSION_TTL_HOURS};
use chrono::{Duration, Utc};
use gitust_core::{AccountStatus, AuthError, SessionId, UserId};

impl AuthStore {
    /// Issues a session bearer token (`gitust_ses_...`) for an active user after a verified
    /// OAuth 2.0 Authorization Code + PKCE (`S256`) exchange.
    ///
    /// Using the unified `sessions_by_hash` store avoids split-brain token tables and guarantees
    /// that account deactivation immediately invalidates OAuth-issued MCP tokens.
    pub fn issue_oauth_session(&self, user_id: UserId) -> Result<LoginSessionResponse, AuthError> {
        self.issue_oauth_session_bound(user_id, None)
    }

    /// Exchanges a code only while the credential that authorized it is still valid.
    /// Validation and session issuance share the same persistence transaction.
    pub fn issue_oauth_session_from_authorization(
        &self,
        user_id: UserId,
        credential: [u8; 32],
    ) -> Result<LoginSessionResponse, AuthError> {
        self.issue_oauth_session_bound(user_id, Some(credential))
    }

    fn issue_oauth_session_bound(
        &self,
        user_id: UserId,
        credential: Option<[u8; 32]>,
    ) -> Result<LoginSessionResponse, AuthError> {
        let (response, snapshot) = {
            let mut guard = self.write_state()?;
            let now = Utc::now();
            if let Some(hash) = credential {
                let valid_session = guard.sessions_by_hash.get(&hash).is_some_and(|s| {
                    s.user_id == user_id
                        && !s.revoked
                        && s.expires_at > now
                        && s.purpose == Some(SessionPurpose::Login)
                });
                let valid_pat = guard
                    .pat_hash_index
                    .get(&hash)
                    .and_then(|id| guard.pats_by_id.get(id))
                    .is_some_and(|p| {
                        p.owner_id == user_id
                            && !p.revoked
                            && p.expires_at.is_none_or(|expiry| expiry > now)
                    });
                if !valid_session && !valid_pat {
                    return Err(AuthError::Unauthenticated);
                }
            }
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
                    purpose: Some(SessionPurpose::Mcp),
                },
            );
            let snap = self.maybe_snapshot(guard);
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

        self.persist_snapshot(snapshot)?;
        Ok(response)
    }
}
