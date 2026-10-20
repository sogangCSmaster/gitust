//! Upgrade legacy secret records and remove shared-password account authority before serving.
use crate::{models::AuthState, secret_storage::SecretKey, AuthStore};
use gitust_core::AuthError;
use std::collections::HashSet;

pub(crate) const DISABLED_SSO_PASSWORD: &str = "!sso-password-disabled";

fn visit_secrets(state: &mut AuthState, key: &SecretKey) -> Result<(), AuthError> {
    for (repo, secrets) in &mut state.workflow_secrets {
        for item in secrets {
            let mut binding = repo.0.as_bytes().to_vec();
            binding.extend_from_slice(&(item.name.len() as u64).to_le_bytes());
            binding.extend_from_slice(item.name.as_bytes());
            key.migrate(
                &format!("workflow:{}:{}", repo.0, item.name),
                "wf",
                &binding,
                &mut item.secret,
            )?;
        }
    }
    for (user, item) in &mut state.totp_enrollments {
        key.migrate(
            &format!("totp:{}", user.0),
            "totp",
            user.0.as_bytes(),
            &mut item.secret,
        )?;
    }
    for item in &mut state.sso_providers {
        key.migrate(
            &format!("sso:{}", item.record.id),
            "sso",
            item.record.id.as_bytes(),
            &mut item.secret,
        )?;
    }
    Ok(())
}
pub(crate) fn validate_snapshot_key(
    snap: &crate::models::PersistedSnapshot,
    key: &SecretKey,
) -> Result<(), AuthError> {
    if snap.security_version != 1 {
        return Ok(());
    }
    validate_secrets(
        &snap.workflow_secrets,
        &snap.totp_enrollments,
        &snap.sso_providers,
        key,
    )
}
fn validate_secrets(
    workflows: &std::collections::HashMap<
        gitust_core::RepoId,
        Vec<crate::workflow_secrets::StoredWorkflowSecret>,
    >,
    totp: &std::collections::HashMap<gitust_core::UserId, crate::totp::StoredTotpEnrollment>,
    sso: &[crate::sso_audit::StoredSsoProvider],
    key: &SecretKey,
) -> Result<(), AuthError> {
    for (repo, list) in workflows {
        for item in list {
            let _plain = zeroize::Zeroizing::new(
                key.open(&format!("workflow:{}:{}", repo.0, item.name), &item.secret)?,
            );
        }
    }
    for (user, item) in totp {
        let _plain = zeroize::Zeroizing::new(key.open(&format!("totp:{}", user.0), &item.secret)?);
    }
    for item in sso {
        let _plain =
            zeroize::Zeroizing::new(key.open(&format!("sso:{}", item.record.id), &item.secret)?);
    }
    Ok(())
}

impl AuthStore {
    pub(crate) fn upgrade_security_state(&self) -> Result<(), AuthError> {
        let mut guard = self.write_state()?;
        if guard.security_version > 1 {
            return Err(AuthError::StorageError(
                "unsupported security storage version".into(),
            ));
        }
        if guard.security_version == 1 {
            validate_secrets(
                &guard.workflow_secrets,
                &guard.totp_enrollments,
                &guard.sso_providers,
                &self.secret_key,
            )?;
            return Ok(());
        }
        visit_secrets(&mut guard, &self.secret_key)?;
        let mut affected = HashSet::new();
        for user in guard.users.values_mut() {
            if crate::crypto::is_legacy_sso_password_hash(&user.password_hash) {
                user.password_hash = DISABLED_SSO_PASSWORD.to_owned();
                affected.insert(user.id);
            }
        }
        for session in guard.sessions_by_hash.values_mut() {
            if affected.contains(&session.user_id) {
                session.revoked = true;
            }
        }
        for pat in guard.pats_by_id.values_mut() {
            if affected.contains(&pat.owner_id) {
                pat.revoked = true;
            }
        }
        for ssh in guard.ssh_keys_by_id.values_mut() {
            if affected.contains(&ssh.owner_id) {
                ssh.revoked = true;
            }
        }
        guard.security_version = 1;
        let snapshot = self.maybe_snapshot(guard);
        self.persist_snapshot(snapshot)
    }

    pub fn rotate_master_secret_key_with_credential(
        &self,
        credential: &str,
        original_user: gitust_core::UserId,
        new_key_bytes: [u8; 32],
        revoke_existing_credentials: bool,
    ) -> Result<usize, AuthError> {
        let new_key = SecretKey::supplied(new_key_bytes);
        let caller_token_hash = crate::crypto::hash_token_sha256(credential);
        let (rotated_count, prev_key, snapshot) = {
            let mut guard = self.write_state()?;
            let admin_id =
                crate::mutation_authority::MutationAuthority::Credential(credential, original_user)
                    .verify(&guard)?;
            let mut count = 0usize;
            for (repo, list) in &mut guard.workflow_secrets {
                for item in list {
                    let ctx = format!("workflow:{}:{}", repo.0, item.name);
                    let plain = zeroize::Zeroizing::new(self.secret_key.open(&ctx, &item.secret)?);
                    item.secret = new_key.seal(&ctx, &plain)?;
                    count = count.saturating_add(1);
                }
            }
            for (user, item) in &mut guard.totp_enrollments {
                let ctx = format!("totp:{}", user.0);
                let plain = zeroize::Zeroizing::new(self.secret_key.open(&ctx, &item.secret)?);
                item.secret = new_key.seal(&ctx, &plain)?;
                count = count.saturating_add(1);
            }
            for item in &mut guard.sso_providers {
                let ctx = format!("sso:{}", item.record.id);
                let plain = zeroize::Zeroizing::new(self.secret_key.open(&ctx, &item.secret)?);
                item.secret = new_key.seal(&ctx, &plain)?;
                count = count.saturating_add(1);
            }
            if revoke_existing_credentials {
                for (hash, session) in &mut guard.sessions_by_hash {
                    if *hash != caller_token_hash {
                        session.revoked = true;
                    }
                }
                for pat in guard.pats_by_id.values_mut() {
                    if pat.token_sha256 != caller_token_hash {
                        pat.revoked = true;
                    }
                }
            }
            let admin_username = guard
                .users
                .get(&admin_id)
                .map(|u| u.username.clone())
                // fail-open-ok: verified active admin exists in guard.users.
                .unwrap_or_default();
            crate::sso_audit::append_audit_event_locked(
                &mut guard,
                Some(admin_id),
                &admin_username,
                "security.secrets.rotate",
                &format!("rotated:{count}:revoke:{revoke_existing_credentials}"),
            );
            let prev_key = self.secret_key.replace_supplied(new_key_bytes);
            (count, prev_key, self.maybe_snapshot(guard))
        };
        if let Err(err) = self.persist_snapshot(snapshot) {
            self.secret_key.restore_supplied(prev_key);
            return Err(err);
        }
        Ok(rotated_count)
    }
}
