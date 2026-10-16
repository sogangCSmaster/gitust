use std::sync::OnceLock;

use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::{Algorithm, Argon2, Params, Version};
use gitust_core::{AuthError, UserId};
use rand::RngCore;
use sha2::{Digest, Sha256};

use crate::MIN_PASSWORD_LEN;

pub(crate) fn argon2_hasher() -> Result<Argon2<'static>, AuthError> {
    #[cfg(debug_assertions)]
    let params = Params::new(4096, 1, 1, None)
        .map_err(|e| AuthError::StorageError(format!("argon2 params error: {e}")))?;
    #[cfg(not(debug_assertions))]
    let params = Params::new(19456, 2, 1, None)
        .map_err(|e| AuthError::StorageError(format!("argon2 params error: {e}")))?;

    Ok(Argon2::new(Algorithm::Argon2id, Version::V0x13, params))
}

pub(crate) fn dummy_password_hash() -> &'static str {
    static DUMMY_HASH: OnceLock<String> = OnceLock::new();
    DUMMY_HASH.get_or_init(|| {
        let salt = SaltString::generate(&mut OsRng);
        // fail-open-ok: only the timing shield degrades; an empty dummy hash never verifies.
        argon2_hasher()
            .ok()
            .and_then(|a| a.hash_password(b"dummy-timing-shield-password", &salt).ok())
            .map(|h| h.to_string())
            .unwrap_or_default()
    })
}

#[must_use]
pub fn hash_token_sha256(raw_token: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(raw_token.trim().as_bytes());
    hasher.finalize().into()
}

pub(crate) fn generate_csprng_token(prefix: &str) -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    format!("{prefix}_{}", hex::encode(bytes))
}

pub(crate) fn hash_password_argon2(password: &str) -> Result<String, AuthError> {
    validate_password_strength(password)?;
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = argon2_hasher()?;
    let hash = argon2
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| AuthError::StorageError(format!("argon2 hash failed: {e}")))?;
    Ok(hash.to_string())
}

pub(crate) fn verify_password_argon2(password: &str, phc_hash: &str) -> bool {
    // fail-open-ok: errors map to `false` (reject), i.e. fail closed.
    let Ok(parsed) = PasswordHash::new(phc_hash) else {
        return false;
    };
    // fail-open-ok: errors map to `false` (reject), i.e. fail closed.
    let Ok(argon2) = argon2_hasher() else {
        return false;
    };
    argon2.verify_password(password.as_bytes(), &parsed).is_ok()
}

pub(crate) fn validate_username(username: &str) -> Result<String, AuthError> {
    let trimmed = username.trim();
    if trimmed.len() < 2 || trimmed.len() > 39 {
        return Err(AuthError::InvalidInput(
            "username must be between 2 and 39 characters".to_owned(),
        ));
    }
    if !trimmed
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(AuthError::InvalidInput(
            "username may only contain ASCII alphanumeric characters, '-', or '_'".to_owned(),
        ));
    }
    Ok(trimmed.to_owned())
}

pub(crate) fn validate_repository_name(name: &str) -> Result<String, AuthError> {
    let trimmed = name.trim();
    if trimmed.is_empty()
        || trimmed.len() > 100
        || trimmed.starts_with('.')
        || trimmed.ends_with('.')
        || trimmed.contains("..")
        || !trimmed
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return Err(AuthError::InvalidInput(
            "repository name must be 1..=100 ASCII alphanumeric, '-', '_', or '.' characters"
                .to_owned(),
        ));
    }
    Ok(trimmed.to_owned())
}

pub(crate) fn validate_password_strength(password: &str) -> Result<(), AuthError> {
    if password.chars().count() < MIN_PASSWORD_LEN {
        return Err(AuthError::InvalidInput(format!(
            "password must be at least {MIN_PASSWORD_LEN} characters"
        )));
    }
    if password.len() > 256 {
        return Err(AuthError::InvalidInput(
            "password must not exceed 256 bytes".to_owned(),
        ));
    }
    Ok(())
}

fn decode_base64_standard(input: &str) -> Option<Vec<u8>> {
    let bytes = input.as_bytes();
    if bytes.is_empty() || !bytes.len().is_multiple_of(4) {
        return None;
    }
    let val = |b: u8| -> Option<u8> {
        match b {
            b'A'..=b'Z' => Some(b - b'A'),
            b'a'..=b'z' => Some(b - b'a' + 26),
            b'0'..=b'9' => Some(b - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    };
    let mut out = Vec::with_capacity((bytes.len() / 4) * 3);
    for (idx, chunk) in bytes.chunks_exact(4).enumerate() {
        let is_last = idx + 1 == bytes.len() / 4;
        let a = val(chunk[0])?;
        let b = val(chunk[1])?;
        out.push((a << 2) | (b >> 4));
        if chunk[2] == b'=' {
            if !is_last || chunk[3] != b'=' || b & 0x0f != 0 {
                return None;
            }
            break;
        }
        let c = val(chunk[2])?;
        out.push((b << 4) | (c >> 2));
        if chunk[3] == b'=' {
            if !is_last || c & 0x03 != 0 {
                return None;
            }
            break;
        }
        let d = val(chunk[3])?;
        out.push((c << 6) | d);
    }
    Some(out)
}

fn encode_base64_unpadded(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0];
        let b1 = if chunk.len() > 1 { chunk[1] } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] } else { 0 };
        out.push(char::from(TABLE[usize::from(b0 >> 2)]));
        out.push(char::from(
            TABLE[usize::from(((b0 & 0x03) << 4) | (b1 >> 4))],
        ));
        if chunk.len() > 1 {
            out.push(char::from(
                TABLE[usize::from(((b1 & 0x0f) << 2) | (b2 >> 6))],
            ));
        }
        if chunk.len() > 2 {
            out.push(char::from(TABLE[usize::from(b2 & 0x3f)]));
        }
    }
    out
}

/// Validates and canonicalizes an OpenSSH public key into `(<algorithm> <base64-blob>, SHA256:<base64>)`,
/// decoding the Base64 wire blob, verifying the embedded key-type header matches `<algorithm>`,
/// and computing the standard OpenSSH SHA-256 fingerprint over the decoded binary key blob.
pub(crate) fn canonicalize_openssh_pubkey(raw: &str) -> Result<(String, String), AuthError> {
    if raw.len() > 16 * 1024 {
        return Err(AuthError::InvalidInput(
            "SSH public key exceeds 16384 bytes".into(),
        ));
    }
    let mut parts = raw.split_whitespace();
    let (Some(algo), Some(blob)) = (parts.next(), parts.next()) else {
        return Err(AuthError::InvalidInput(
            "SSH public key must be in '<algorithm> <base64-blob>' OpenSSH format".to_owned(),
        ));
    };
    if !matches!(
        algo,
        "ssh-ed25519"
            | "ssh-rsa"
            | "ecdsa-sha2-nistp256"
            | "ecdsa-sha2-nistp384"
            | "ecdsa-sha2-nistp521"
    ) {
        return Err(AuthError::InvalidInput(format!(
            "unsupported SSH key algorithm: {algo}"
        )));
    }
    let Some(decoded) = decode_base64_standard(blob) else {
        return Err(AuthError::InvalidInput(
            "malformed base64 key blob in SSH public key".to_owned(),
        ));
    };
    if decoded.len() < 4 + algo.len() + 4 {
        return Err(AuthError::InvalidInput(
            "SSH public key blob is truncated".to_owned(),
        ));
    }
    let header_len = usize::try_from(u32::from_be_bytes([
        decoded[0], decoded[1], decoded[2], decoded[3],
    ]))
    // fail-open-ok: unrepresentable header length becomes an impossible value and is rejected below.
    .unwrap_or(usize::MAX);
    if header_len != algo.len() || &decoded[4..4 + algo.len()] != algo.as_bytes() {
        return Err(AuthError::InvalidInput(
            "SSH public key blob algorithm header does not match declared key type".to_owned(),
        ));
    }
    let digest: [u8; 32] = Sha256::digest(&decoded).into();
    let fingerprint = format!("SHA256:{}", encode_base64_unpadded(&digest));
    Ok((format!("{algo} {blob}"), fingerprint))
}

pub(crate) fn scoped_id_rate_limit_key(scope: &str, norm_id: &str) -> String {
    format!(
        "scope:{scope}:id:{}",
        hex::encode(hash_token_sha256(norm_id))
    )
}

pub(crate) fn scoped_account_rate_limit_key(scope: &str, user_id: UserId) -> String {
    format!("scope:{scope}:uid:{}", user_id.0)
}
