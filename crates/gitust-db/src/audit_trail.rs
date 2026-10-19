//! Tamper-Evident SHA-256 Hash-Chained Enterprise Audit Trail (`GENESIS_AUDIT_HASH`).

use chrono::Utc;
use gitust_core::{
    AuditChainVerificationReport, AuditEventRecord, AuthError, AuthenticatedPrincipal, UserId,
};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use uuid::Uuid;

use crate::models::AuthState;
use crate::AuthStore;

pub const GENESIS_AUDIT_HASH: &str =
    "0000000000000000000000000000000000000000000000000000000000000000";

/// Computes the deterministic SHA-256 hash chaining `prev_hash` with the current audit event fields.
#[must_use]
pub fn compute_audit_entry_hash(
    seq: u64,
    timestamp: &str,
    actor_user_id: Option<Uuid>,
    actor_username: &str,
    action: &str,
    target: &str,
    prev_hash: &str,
) -> String {
    let actor_id = match actor_user_id {
        Some(u) => u.to_string(),
        None => String::new(),
    };
    let canonical =
        format!("{seq}|{timestamp}|{actor_id}|{actor_username}|{action}|{target}|{prev_hash}");
    let digest: [u8; 32] = Sha256::digest(canonical.as_bytes()).into();
    hex::encode(digest)
}

pub(crate) fn append_audit_event_locked(
    state: &mut AuthState,
    actor_user_id: Option<UserId>,
    actor_username: &str,
    action: &str,
    target: &str,
) {
    let (seq, prev_hash) = match state.audit_log.last() {
        Some(last) => (last.seq.saturating_add(1), last.entry_hash.clone()),
        None => (1, GENESIS_AUDIT_HASH.to_owned()),
    };
    let timestamp = Utc::now().to_rfc3339();
    let actor_uuid = actor_user_id.map(|u| u.0);
    let entry_hash = compute_audit_entry_hash(
        seq,
        &timestamp,
        actor_uuid,
        actor_username,
        action,
        target,
        &prev_hash,
    );
    state.audit_log.push(AuditEventRecord {
        seq,
        timestamp,
        actor_user_id: actor_uuid,
        actor_username: actor_username.to_owned(),
        action: action.to_owned(),
        target: target.to_owned(),
        prev_hash,
        entry_hash,
    });
}

fn invalid_chain_report(
    checked: usize,
    head_hash: String,
    seq: u64,
    reason: String,
) -> AuditChainVerificationReport {
    AuditChainVerificationReport {
        verified: false,
        entries_checked: checked,
        head_hash,
        first_invalid_seq: Some(seq),
        reason: Some(reason),
    }
}

pub(crate) fn verify_audit_chain_locked(
    entries: &[AuditEventRecord],
) -> AuditChainVerificationReport {
    let head_hash = match entries.last() {
        Some(e) => e.entry_hash.clone(),
        None => GENESIS_AUDIT_HASH.to_owned(),
    };
    let Some(first) = entries.first() else {
        return AuditChainVerificationReport {
            verified: true,
            entries_checked: 0,
            head_hash,
            first_invalid_seq: None,
            reason: None,
        };
    };
    let start_seq = first.seq;
    if start_seq != 1 {
        return invalid_chain_report(
            1,
            head_hash,
            start_seq,
            "missing genesis audit entry (log prefix truncated)".into(),
        );
    }
    let mut expected_prev: &str = GENESIS_AUDIT_HASH;
    for (idx, entry) in entries.iter().enumerate() {
        let expected_seq = start_seq.saturating_add(idx as u64);
        if entry.seq != expected_seq {
            return invalid_chain_report(
                idx + 1,
                head_hash,
                entry.seq,
                format!("sequence discontinuity at index {idx}"),
            );
        }
        if !bool::from(entry.prev_hash.as_bytes().ct_eq(expected_prev.as_bytes())) {
            return invalid_chain_report(
                idx + 1,
                head_hash,
                entry.seq,
                format!("prev_hash mismatch at seq {}", entry.seq),
            );
        }
        let recomputed = compute_audit_entry_hash(
            entry.seq,
            &entry.timestamp,
            entry.actor_user_id,
            &entry.actor_username,
            &entry.action,
            &entry.target,
            &entry.prev_hash,
        );
        if !bool::from(recomputed.as_bytes().ct_eq(entry.entry_hash.as_bytes())) {
            return invalid_chain_report(
                idx + 1,
                head_hash,
                entry.seq,
                format!("entry_hash tamper detected at seq {}", entry.seq),
            );
        }
        expected_prev = &entry.entry_hash;
    }
    AuditChainVerificationReport {
        verified: true,
        entries_checked: entries.len(),
        head_hash,
        first_invalid_seq: None,
        reason: None,
    }
}

impl AuthStore {
    pub fn audit_log_for_admin(
        &self,
        caller: &AuthenticatedPrincipal,
    ) -> Result<(Vec<AuditEventRecord>, AuditChainVerificationReport), AuthError> {
        let guard = self.read_state()?;
        Self::verify_active_admin_locked(&guard, caller)?;
        let report = verify_audit_chain_locked(&guard.audit_log);
        Ok((guard.audit_log.clone(), report))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audit_hash_chain_detects_payload_link_and_prefix_truncation_tampering() {
        let mut state = AuthState::default();
        append_audit_event_locked(&mut state, None, "admin", "admin.bootstrap", "user:admin");
        append_audit_event_locked(&mut state, None, "admin", "sso.login", "user:admin");
        let ok_report = verify_audit_chain_locked(&state.audit_log);
        assert!(ok_report.verified);
        assert_eq!(ok_report.entries_checked, 2);

        let truncated = vec![state.audit_log[1].clone()];
        let trunc_report = verify_audit_chain_locked(&truncated);
        assert!(!trunc_report.verified);
        assert_eq!(trunc_report.first_invalid_seq, Some(2));

        state.audit_log[1].target = "user:tampered".to_owned();
        let bad_report = verify_audit_chain_locked(&state.audit_log);
        assert!(!bad_report.verified);
        assert_eq!(bad_report.first_invalid_seq, Some(2));
    }
}
