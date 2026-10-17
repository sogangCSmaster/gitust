use chrono::{Duration, Utc};
use gitust_core::{
    AccountStatus, AuthError, AuthenticatedPrincipal, EnterpriseRole, InviteId, InvitePublicView,
    InviteTokenPurpose, TeamId, UserId, UserPublicProfile, DEFAULT_INVITE_EXPIRY_HOURS,
};
use subtle::ConstantTimeEq;

use crate::crypto::{
    generate_csprng_token, hash_password_argon2, hash_token_sha256, validate_username,
};
use crate::invite_authority::InviteAuthority;
use crate::models::{IssuedInviteLink, StoredInviteToken, StoredUser};
use crate::AuthStore;

pub struct AccountInviteRequest<'a> {
    pub target_email: &'a str,
    pub assigned_role: EnterpriseRole,
    pub team_ids: Vec<TeamId>,
    pub ttl_hours: Option<i64>,
}

impl AuthStore {
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
        self.create_invite_authorized(
            InviteAuthority::Operator(caller),
            AccountInviteRequest {
                target_email,
                assigned_role,
                team_ids,
                ttl_hours,
            },
        )
    }

    pub fn create_invite_link_with_credential(
        &self,
        credential: &str,
        original_user: UserId,
        request: AccountInviteRequest<'_>,
    ) -> Result<IssuedInviteLink, AuthError> {
        self.create_invite_authorized(
            InviteAuthority::Credential(credential, original_user),
            request,
        )
    }

    fn create_invite_authorized(
        &self,
        authority: InviteAuthority<'_>,
        request: AccountInviteRequest<'_>,
    ) -> Result<IssuedInviteLink, AuthError> {
        let AccountInviteRequest {
            target_email,
            assigned_role,
            team_ids,
            ttl_hours,
        } = request;
        let (public_view, raw_token, snapshot) = {
            let mut guard = self.write_state()?;
            let admin_id = authority.verify(&guard)?;
            // fail-open-ok: omitted TTL retains the existing bounded 72-hour policy.
            let hours = ttl_hours.unwrap_or(DEFAULT_INVITE_EXPIRY_HOURS);
            let (record, raw_token) = prepare_account_invite(
                &guard,
                AccountInviteInput {
                    id: InviteId::new(),
                    creator: admin_id,
                    email: target_email,
                    assigned_role,
                    team_ids,
                    hours,
                    repository_origin: None,
                },
            )?;
            let public_view = record.to_public_view();
            guard
                .invite_hash_index
                .insert(record.token_sha256, record.id);
            guard.invites_by_id.insert(record.id, record);
            (public_view, raw_token, self.maybe_snapshot(guard))
        };

        self.persist_snapshot(snapshot)?;
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
        self.create_reset_authorized(InviteAuthority::Operator(caller), target_user_id, ttl_hours)
    }

    pub fn create_password_reset_link_with_credential(
        &self,
        credential: &str,
        original_user: UserId,
        target_user_id: UserId,
        ttl_hours: Option<i64>,
    ) -> Result<IssuedInviteLink, AuthError> {
        self.create_reset_authorized(
            InviteAuthority::Credential(credential, original_user),
            target_user_id,
            ttl_hours,
        )
    }

    fn create_reset_authorized(
        &self,
        authority: InviteAuthority<'_>,
        target_user_id: UserId,
        ttl_hours: Option<i64>,
    ) -> Result<IssuedInviteLink, AuthError> {
        // fail-open-ok: omitted TTL uses the documented bounded default; supplied values are validated below.
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
            let mut guard = self.write_state()?;
            let admin_id = authority.verify(&guard)?;

            let Some(target_user) = guard.users.get(&target_user_id) else {
                return Err(AuthError::InvalidInput("target user not found".to_owned()));
            };
            if target_user.status != AccountStatus::Active {
                return Err(AuthError::AccountDeactivated);
            }

            let record = StoredInviteToken {
                repository_origin: None,
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
            let snap = self.maybe_snapshot(guard);
            (public_view, snap)
        };

        self.persist_snapshot(snapshot)?;
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
        self.revoke_invite_authorized(InviteAuthority::Operator(caller), invite_id)
    }

    pub fn revoke_invite_link_with_credential(
        &self,
        credential: &str,
        original_user: UserId,
        invite_id: InviteId,
    ) -> Result<InvitePublicView, AuthError> {
        self.revoke_invite_authorized(
            InviteAuthority::Credential(credential, original_user),
            invite_id,
        )
    }

    fn revoke_invite_authorized(
        &self,
        authority: InviteAuthority<'_>,
        invite_id: InviteId,
    ) -> Result<InvitePublicView, AuthError> {
        let (view, snapshot) = {
            let mut guard = self.write_state()?;
            authority.verify(&guard)?;

            let Some(inv) = guard.invites_by_id.get_mut(&invite_id) else {
                return Err(AuthError::InvalidToken);
            };
            inv.revoked_at = Some(Utc::now());
            let view = inv.to_public_view();
            let snap = self.maybe_snapshot(guard);
            (view, snap)
        };

        self.persist_snapshot(snapshot)?;
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
            let guard = self.read_state()?;
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
            let mut guard = self.write_state()?;
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
                membership_version: 0,
                created_at: check_now,
            };
            let profile = user.to_public_profile();

            guard
                .username_index
                .insert(clean_username.to_ascii_lowercase(), user.id);
            guard.email_index.insert(target_email, user.id);
            guard.users.insert(user.id, user);
            let snap = self.maybe_snapshot(guard);
            (profile, snap)
        };

        self.persist_snapshot(snapshot)?;
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
            let guard = self.read_state()?;
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
            let mut guard = self.write_state()?;
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

            let snap = self.maybe_snapshot(guard);
            (profile, snap)
        };

        self.persist_snapshot(snapshot)?;
        Ok(profile)
    }

    pub fn list_invites(
        &self,
        caller: &AuthenticatedPrincipal,
    ) -> Result<Vec<InvitePublicView>, AuthError> {
        let guard = self.read_state()?;
        Self::verify_active_admin_locked(&guard, caller)?;
        let mut invites: Vec<InvitePublicView> = guard
            .invites_by_id
            .values()
            .map(StoredInviteToken::to_public_view)
            .collect();
        invites.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        Ok(invites)
    }
}

pub(crate) struct AccountInviteInput<'a> {
    pub id: InviteId,
    pub creator: UserId,
    pub email: &'a str,
    pub assigned_role: EnterpriseRole,
    pub team_ids: Vec<TeamId>,
    pub hours: i64,
    pub repository_origin: Option<gitust_core::RepositoryRegistration>,
}

pub(crate) fn prepare_account_invite(
    state: &crate::models::AuthState,
    input: AccountInviteInput<'_>,
) -> Result<(StoredInviteToken, String), AuthError> {
    let mut unique = std::collections::HashSet::new();
    if input.team_ids.len() > 32
        || input
            .team_ids
            .iter()
            .any(|id| id.0.is_nil() || !unique.insert(*id))
    {
        return Err(AuthError::InvalidInput(
            "invite teams must contain at most 32 distinct nonnil group UUIDs".into(),
        ));
    }
    if !(1..=720).contains(&input.hours) {
        return Err(AuthError::InvalidInput(
            "invite TTL must be between 1 and 720 hours".into(),
        ));
    }
    let email = input.email.trim().to_ascii_lowercase();
    state
        .policy
        .validate_registration(&email, true)
        .map_err(|e| AuthError::InvalidInput(e.to_string()))?;
    if state.email_index.contains_key(&email) {
        return Err(AuthError::DuplicateUser);
    }
    let raw = generate_csprng_token("gitust_inv");
    let now = Utc::now();
    Ok((
        StoredInviteToken {
            id: input.id,
            token_sha256: hash_token_sha256(&raw),
            purpose: InviteTokenPurpose::AccountInvite,
            target_email: email,
            assigned_role: input.assigned_role,
            team_ids: input.team_ids,
            created_by: input.creator,
            created_at: now,
            expires_at: now + Duration::hours(input.hours),
            used_at: None,
            revoked_at: None,
            repository_origin: input.repository_origin,
        },
        raw,
    ))
}
