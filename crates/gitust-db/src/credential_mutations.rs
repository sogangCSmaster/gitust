use crate::{
    crypto::{canonicalize_openssh_pubkey, generate_csprng_token, hash_token_sha256},
    models::{IssuedPersonalAccessToken, StoredPat, StoredSshKey},
    mutation_authority::MutationAuthority,
    AuthStore,
};
use chrono::{Duration, Utc};
use gitust_core::{
    AuthError, AuthenticatedPrincipal, EnterpriseRole, PatId, PersonalAccessTokenView, SshKeyId,
    SshPublicKeyView, UserId,
};

impl AuthStore {
    /// Issues a Personal Access Token (PAT) for HTTPS Git operations (`clone`/`fetch`/`push`).
    pub fn create_personal_access_token(
        &self,
        caller: &AuthenticatedPrincipal,
        name: &str,
        ttl_days: Option<i64>,
    ) -> Result<IssuedPersonalAccessToken, AuthError> {
        self.create_personal_access_token_with_authority(
            MutationAuthority::Operator(caller),
            name,
            ttl_days,
        )
    }
    pub fn create_personal_access_token_with_credential(
        &self,
        token: &str,
        user: UserId,
        name: &str,
        ttl_days: Option<i64>,
    ) -> Result<IssuedPersonalAccessToken, AuthError> {
        self.create_personal_access_token_with_authority(
            MutationAuthority::Credential(token, user),
            name,
            ttl_days,
        )
    }
    fn create_personal_access_token_with_authority(
        &self,
        authority: MutationAuthority<'_>,
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
            let active_caller = authority.active(&guard)?;

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
            let snap = self.maybe_snapshot(guard);
            (metadata, snap)
        };

        self.persist_snapshot(snapshot)?;
        Ok(IssuedPersonalAccessToken {
            metadata,
            raw_token,
        })
    }
    /// Revokes a Personal Access Token (PAT) with strict BOLA/IDOR ownership verification.
    pub fn revoke_personal_access_token(
        &self,
        caller: &AuthenticatedPrincipal,
        pat_id: PatId,
    ) -> Result<PersonalAccessTokenView, AuthError> {
        self.revoke_personal_access_token_with_authority(
            MutationAuthority::Operator(caller),
            pat_id,
        )
    }
    pub fn revoke_personal_access_token_with_credential(
        &self,
        token: &str,
        user: UserId,
        pat_id: PatId,
    ) -> Result<PersonalAccessTokenView, AuthError> {
        self.revoke_personal_access_token_with_authority(
            MutationAuthority::Credential(token, user),
            pat_id,
        )
    }
    fn revoke_personal_access_token_with_authority(
        &self,
        authority: MutationAuthority<'_>,
        pat_id: PatId,
    ) -> Result<PersonalAccessTokenView, AuthError> {
        let (view, snapshot) = {
            let mut guard = self.write_state()?;
            let active_caller = authority.active(&guard)?;

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
            let snap = self.maybe_snapshot(guard);
            (view, snap)
        };

        self.persist_snapshot(snapshot)?;
        Ok(view)
    }
    /// Registers an SSH public key for `russh` Git access after canonicalizing `<algorithm> <base64-blob>`.
    pub fn register_ssh_public_key(
        &self,
        caller: &AuthenticatedPrincipal,
        title: &str,
        public_key_openssh: &str,
    ) -> Result<SshPublicKeyView, AuthError> {
        self.register_ssh_public_key_with_authority(
            MutationAuthority::Operator(caller),
            title,
            public_key_openssh,
        )
    }
    pub fn register_ssh_public_key_with_credential(
        &self,
        token: &str,
        user: UserId,
        title: &str,
        public_key_openssh: &str,
    ) -> Result<SshPublicKeyView, AuthError> {
        self.register_ssh_public_key_with_authority(
            MutationAuthority::Credential(token, user),
            title,
            public_key_openssh,
        )
    }
    fn register_ssh_public_key_with_authority(
        &self,
        authority: MutationAuthority<'_>,
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
            let active_caller = authority.active(&guard)?;

            if guard
                .ssh_fingerprint_index
                .get(&fingerprint_sha256)
                .and_then(|id| guard.ssh_keys_by_id.get(id))
                .is_some_and(|key| !key.revoked)
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
            let snap = self.maybe_snapshot(guard);
            (view, snap)
        };

        self.persist_snapshot(snapshot)?;
        Ok(view)
    }
    /// Revokes an SSH public key with strict BOLA/IDOR ownership verification.
    pub fn revoke_ssh_public_key(
        &self,
        caller: &AuthenticatedPrincipal,
        ssh_key_id: SshKeyId,
    ) -> Result<SshPublicKeyView, AuthError> {
        self.revoke_ssh_public_key_with_authority(MutationAuthority::Operator(caller), ssh_key_id)
    }
    pub fn revoke_ssh_public_key_with_credential(
        &self,
        token: &str,
        user: UserId,
        ssh_key_id: SshKeyId,
    ) -> Result<SshPublicKeyView, AuthError> {
        self.revoke_ssh_public_key_with_authority(
            MutationAuthority::Credential(token, user),
            ssh_key_id,
        )
    }
    fn revoke_ssh_public_key_with_authority(
        &self,
        authority: MutationAuthority<'_>,
        ssh_key_id: SshKeyId,
    ) -> Result<SshPublicKeyView, AuthError> {
        let (view, snapshot) = {
            let mut guard = self.write_state()?;
            let active_caller = authority.active(&guard)?;

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
            let fingerprint = key_rec.fingerprint_sha256.clone();
            if guard.ssh_fingerprint_index.get(&fingerprint) == Some(&ssh_key_id) {
                guard.ssh_fingerprint_index.remove(&fingerprint);
            }
            let snap = self.maybe_snapshot(guard);
            (view, snap)
        };

        self.persist_snapshot(snapshot)?;
        Ok(view)
    }
}
