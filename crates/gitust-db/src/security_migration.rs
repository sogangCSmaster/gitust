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
}
