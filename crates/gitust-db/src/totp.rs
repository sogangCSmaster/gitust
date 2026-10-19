//! Pure-Rust RFC 6238 TOTP (Time-Based One-Time Password) & RFC 2104 HMAC-SHA256 engine
//! with encrypted-at-rest secret storage and replay-resistant step verification.

use argon2::password_hash::rand_core::OsRng;
use chrono::Utc;
use gitust_core::{AuthError, AuthenticatedPrincipal, TotpEnrollmentChallenge, TotpStatus, UserId};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::crypto::scoped_account_rate_limit_key;
use crate::models::AuthState;
use crate::mutation_authority::MutationAuthority;
use crate::sso_audit::append_audit_event_locked;
use crate::AuthStore;

pub const TOTP_PERIOD_SECS: u64 = 30;
pub const TOTP_DIGITS: u32 = 6;
const BASE32_ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct StoredTotpEnrollment {
    #[serde(flatten)]
    pub(crate) secret: crate::secret_storage::StoredSecret,
    pub(crate) verified: bool,
    pub(crate) enrolled_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) last_used_step: Option<u64>,
}

/// RFC 2104 HMAC-SHA256 implementation over `sha2::Sha256` (64-byte block size).
#[must_use]
pub fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    let mut key_block = [0u8; 64];
    if key.len() > 64 {
        let hashed: [u8; 32] = Sha256::digest(key).into();
        key_block[..32].copy_from_slice(&hashed);
    } else {
        key_block[..key.len()].copy_from_slice(key);
    }
    let mut ipad = [0x36u8; 64];
    let mut opad = [0x5cu8; 64];
    for i in 0..64 {
        ipad[i] ^= key_block[i];
        opad[i] ^= key_block[i];
    }
    let mut inner = Sha256::new();
    inner.update(ipad);
    inner.update(message);
    let inner_digest: [u8; 32] = inner.finalize().into();

    let mut outer = Sha256::new();
    outer.update(opad);
    outer.update(inner_digest);
    outer.finalize().into()
}

/// RFC 4648 Base32 encoding without padding (`A-Z2-7`).
#[must_use]
pub fn base32_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity((bytes.len() * 8).div_ceil(5));
    let mut buffer = 0u32;
    let mut bits_left = 0u32;
    for &b in bytes {
        buffer = (buffer << 8) | u32::from(b);
        bits_left += 8;
        while bits_left >= 5 {
            bits_left -= 5;
            let idx = ((buffer >> bits_left) & 0x1f) as usize;
            out.push(char::from(BASE32_ALPHABET[idx]));
        }
    }
    if bits_left > 0 {
        let idx = ((buffer << (5 - bits_left)) & 0x1f) as usize;
        out.push(char::from(BASE32_ALPHABET[idx]));
    }
    out
}

/// RFC 4648 Base32 decoding (case-insensitive, ignores `=` padding and whitespace).
pub fn base32_decode(encoded: &str) -> Result<Vec<u8>, AuthError> {
    let clean = encoded.trim().trim_end_matches('=');
    if clean.is_empty() || clean.len() > 128 {
        return Err(AuthError::InvalidInput("invalid Base32 TOTP secret".into()));
    }
    let mut out = Vec::with_capacity(clean.len() * 5 / 8);
    let mut buffer = 0u32;
    let mut bits_left = 0u32;
    for ch in clean.chars() {
        if ch == ' ' || ch == '-' {
            continue;
        }
        let val = match ch.to_ascii_uppercase() {
            'A'..='Z' => (ch.to_ascii_uppercase() as u8) - b'A',
            '2'..='7' => (ch as u8) - b'2' + 26,
            _ => {
                return Err(AuthError::InvalidInput(
                    "invalid character in Base32 TOTP secret".into(),
                ))
            }
        };
        buffer = (buffer << 5) | u32::from(val);
        bits_left += 5;
        if bits_left >= 8 {
            bits_left -= 8;
            out.push(((buffer >> bits_left) & 0xff) as u8);
        }
    }
    if out.is_empty() {
        return Err(AuthError::InvalidInput("empty Base32 TOTP secret".into()));
    }
    Ok(out)
}

/// Computes a 6-digit RFC 6238 TOTP code for a given 30-second time step using HMAC-SHA256.
#[must_use]
pub fn compute_totp_code_for_step(secret: &[u8], step: u64) -> String {
    let mac = hmac_sha256(secret, &step.to_be_bytes());
    let offset = (mac[31] & 0x0f) as usize;
    let binary = u32::from_be_bytes([
        mac[offset] & 0x7f,
        mac[offset + 1],
        mac[offset + 2],
        mac[offset + 3],
    ]);
    let code = binary % 1_000_000;
    format!("{code:06}")
}

/// Computes the 6-digit RFC 6238 TOTP code for `secret_base32` at `unix_secs`.
pub fn compute_totp_code_at(secret_base32: &str, unix_secs: u64) -> Result<String, AuthError> {
    let secret = base32_decode(secret_base32)?;
    Ok(compute_totp_code_for_step(
        &secret,
        unix_secs / TOTP_PERIOD_SECS,
    ))
}

fn verify_totp_step_constant_time(
    secret: &[u8],
    raw_code: &str,
    now_secs: u64,
    last_used_step: Option<u64>,
) -> Result<u64, AuthError> {
    let clean = raw_code.trim();
    if clean.len() != 6 || !clean.bytes().all(|b| b.is_ascii_digit()) {
        return Err(AuthError::InvalidTotpCode);
    }
    let current_step = now_secs / TOTP_PERIOD_SECS;
    let candidates = [
        current_step.saturating_sub(1),
        current_step,
        current_step.saturating_add(1),
    ];
    let mut matched_step: Option<u64> = None;
    for step in candidates {
        let expected = compute_totp_code_for_step(secret, step);
        if bool::from(expected.as_bytes().ct_eq(clean.as_bytes())) {
            matched_step = Some(step);
        }
    }
    let Some(step) = matched_step else {
        return Err(AuthError::InvalidTotpCode);
    };
    if last_used_step.is_some_and(|last| step <= last) {
        return Err(AuthError::InvalidTotpCode);
    }
    Ok(step)
}

pub(crate) fn verify_user_totp_locked(
    key: &crate::secret_storage::SecretKey,
    state: &mut AuthState,
    user_id: UserId,
    totp_code: Option<&str>,
    now_secs: u64,
) -> Result<(), AuthError> {
    let Some(enrollment) = state.totp_enrollments.get(&user_id) else {
        return Ok(());
    };
    if !enrollment.verified {
        return Ok(());
    }
    let Some(code) = totp_code.map(str::trim).filter(|s| !s.is_empty()) else {
        return Err(AuthError::TotpRequired);
    };
    let secret =
        zeroize::Zeroizing::new(key.open(&format!("totp:{}", user_id.0), &enrollment.secret)?);
    let matched_step =
        verify_totp_step_constant_time(&secret, code, now_secs, enrollment.last_used_step)?;
    if let Some(enr_mut) = state.totp_enrollments.get_mut(&user_id) {
        enr_mut.last_used_step = Some(matched_step);
    }
    Ok(())
}

impl AuthStore {
    pub fn totp_status(&self, caller: &AuthenticatedPrincipal) -> Result<TotpStatus, AuthError> {
        let guard = self.read_state()?;
        let active = Self::verify_active_user_locked(&guard, caller)?;
        let Some(enr) = guard.totp_enrollments.get(&active.user_id) else {
            return Ok(TotpStatus {
                enrolled: false,
                verified: false,
                enrolled_at: None,
                last_used_step: None,
            });
        };
        Ok(TotpStatus {
            enrolled: true,
            verified: enr.verified,
            enrolled_at: Some(enr.enrolled_at.clone()),
            last_used_step: enr.last_used_step,
        })
    }

    pub fn enroll_totp_with_credential(
        &self,
        credential: &str,
        original_user: UserId,
    ) -> Result<TotpEnrollmentChallenge, AuthError> {
        let mut raw_secret = zeroize::Zeroizing::new([0u8; 20]);
        OsRng.fill_bytes(raw_secret.as_mut());
        let secret_base32 = base32_encode(raw_secret.as_ref());
        let now_str = Utc::now().to_rfc3339();
        let stored = StoredTotpEnrollment {
            secret: self
                .secret_key
                .seal(&format!("totp:{}", original_user.0), raw_secret.as_ref())?,
            verified: false,
            enrolled_at: now_str,
            last_used_step: None,
        };

        let (username, snapshot) = {
            let mut guard = self.write_state()?;
            let active = MutationAuthority::Credential(credential, original_user).active(&guard)?;
            if guard
                .totp_enrollments
                .get(&active.user_id)
                .is_some_and(|e| e.verified)
            {
                return Err(AuthError::InvalidInput(
                    "two-factor authentication is already verified; disable it before re-enrolling"
                        .into(),
                ));
            }
            guard.totp_enrollments.insert(active.user_id, stored);
            append_audit_event_locked(
                &mut guard,
                Some(active.user_id),
                &active.username,
                "totp.enroll",
                &format!("user:{}", active.username),
            );
            (active.username, self.maybe_snapshot(guard))
        };
        self.persist_snapshot(snapshot)?;
        let otpauth_uri = format!(
            "otpauth://totp/Gitust:{username}?secret={secret_base32}&issuer=Gitust&algorithm=SHA256&digits={TOTP_DIGITS}&period={TOTP_PERIOD_SECS}"
        );
        Ok(TotpEnrollmentChallenge {
            secret_base32,
            otpauth_uri,
            digits: TOTP_DIGITS,
            period_secs: TOTP_PERIOD_SECS,
        })
    }

    pub fn verify_totp_with_credential(
        &self,
        credential: &str,
        original_user: UserId,
        code: &str,
    ) -> Result<TotpStatus, AuthError> {
        let (status, snapshot) = {
            let mut guard = self.write_state()?;
            let active = MutationAuthority::Credential(credential, original_user).active(&guard)?;
            let now = Utc::now();
            let rl_key = scoped_account_rate_limit_key("totp_verify", active.user_id);
            if Self::is_rate_limited_locked(&mut guard, &rl_key, now) {
                return Err(AuthError::RateLimitExceeded);
            }
            let Some(enr) = guard.totp_enrollments.get(&active.user_id) else {
                return Err(AuthError::InvalidInput("TOTP enrollment not found".into()));
            };
            let secret = zeroize::Zeroizing::new(
                self.secret_key
                    .open(&format!("totp:{}", active.user_id.0), &enr.secret)?,
            );
            // fail-open-ok: UTC epoch timestamp is non-negative.
            let now_secs = u64::try_from(now.timestamp()).unwrap_or(0);
            let matched_step =
                match verify_totp_step_constant_time(&secret, code, now_secs, enr.last_used_step) {
                    Ok(step) => step,
                    Err(err) => {
                        Self::record_failed_login_locked(&mut guard, rl_key, now);
                        return Err(err);
                    }
                };
            guard.failed_logins.remove(&rl_key);
            let enr_mut = guard
                .totp_enrollments
                .get_mut(&active.user_id)
                .ok_or_else(|| AuthError::InvalidInput("TOTP enrollment not found".into()))?;
            enr_mut.verified = true;
            enr_mut.last_used_step = Some(matched_step);
            let st = TotpStatus {
                enrolled: true,
                verified: true,
                enrolled_at: Some(enr_mut.enrolled_at.clone()),
                last_used_step: enr_mut.last_used_step,
            };
            append_audit_event_locked(
                &mut guard,
                Some(active.user_id),
                &active.username,
                "totp.verify",
                &format!("user:{}", active.username),
            );
            (st, self.maybe_snapshot(guard))
        };
        self.persist_snapshot(snapshot)?;
        Ok(status)
    }

    pub fn disable_totp_with_credential(
        &self,
        credential: &str,
        original_user: UserId,
        code: &str,
    ) -> Result<TotpStatus, AuthError> {
        let (status, snapshot) = {
            let mut guard = self.write_state()?;
            let active = MutationAuthority::Credential(credential, original_user).active(&guard)?;
            let now = Utc::now();
            let rl_key = scoped_account_rate_limit_key("totp_disable", active.user_id);
            if Self::is_rate_limited_locked(&mut guard, &rl_key, now) {
                return Err(AuthError::RateLimitExceeded);
            }
            let Some(enr) = guard.totp_enrollments.get(&active.user_id) else {
                return Err(AuthError::InvalidInput("TOTP enrollment not found".into()));
            };
            if enr.verified {
                let secret = zeroize::Zeroizing::new(
                    self.secret_key
                        .open(&format!("totp:{}", active.user_id.0), &enr.secret)?,
                );
                // fail-open-ok: UTC epoch timestamp is non-negative.
                let now_secs = u64::try_from(now.timestamp()).unwrap_or(0);
                if let Err(err) =
                    verify_totp_step_constant_time(&secret, code, now_secs, enr.last_used_step)
                {
                    Self::record_failed_login_locked(&mut guard, rl_key, now);
                    return Err(err);
                }
            }
            guard.failed_logins.remove(&rl_key);
            guard.totp_enrollments.remove(&active.user_id);
            append_audit_event_locked(
                &mut guard,
                Some(active.user_id),
                &active.username,
                "totp.disable",
                &format!("user:{}", active.username),
            );
            (
                TotpStatus {
                    enrolled: false,
                    verified: false,
                    enrolled_at: None,
                    last_used_step: None,
                },
                self.maybe_snapshot(guard),
            )
        };
        self.persist_snapshot(snapshot)?;
        Ok(status)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base32_roundtrip_and_rfc6238_totp_generation() {
        let raw = b"12345678901234567890";
        let b32 = base32_encode(raw);
        assert_eq!(b32, "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ");
        assert_eq!(base32_decode(&b32).unwrap(), raw);

        let code_t59 = compute_totp_code_at(&b32, 59).unwrap();
        assert_eq!(code_t59.len(), 6);
        assert!(code_t59.bytes().all(|b| b.is_ascii_digit()));
        assert_eq!(
            verify_totp_step_constant_time(raw, &code_t59, 59, None).unwrap(),
            1
        );
        // Replaying the same step is rejected
        assert_eq!(
            verify_totp_step_constant_time(raw, &code_t59, 59, Some(1)),
            Err(AuthError::InvalidTotpCode)
        );
    }
}
