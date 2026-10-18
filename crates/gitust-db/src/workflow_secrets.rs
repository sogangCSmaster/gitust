use std::collections::BTreeMap;

use argon2::password_hash::rand_core::OsRng;
use chrono::{DateTime, Utc};
use gitust_core::{AuthError, AuthenticatedPrincipal, EnterpriseRole, RepoAccess, RepoId};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::pull_requests::authorize_repo_locked;
use crate::AuthStore;

const MAX_WORKFLOW_SECRETS_PER_REPO: usize = 64;
const MAX_SECRET_VALUE_BYTES: usize = 8192;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowSecretMetadata {
    pub name: String,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct StoredWorkflowSecret {
    pub(crate) name: String,
    pub(crate) key_hex: String,
    pub(crate) nonce_hex: String,
    pub(crate) ciphertext_hex: String,
    pub(crate) tag_sha256: [u8; 32],
    pub(crate) updated_at: DateTime<Utc>,
}

pub fn validate_workflow_secret_name(raw: &str) -> Result<String, AuthError> {
    let trimmed = raw.trim();
    let bytes = trimmed.as_bytes();
    if bytes.is_empty()
        || bytes.len() > 64
        || !bytes[0].is_ascii_uppercase()
        || !bytes[1..]
            .iter()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || *b == b'_')
    {
        return Err(AuthError::InvalidInput(
            "workflow secret name must match ^[A-Z][A-Z0-9_]{0,63}$".into(),
        ));
    }
    Ok(trimmed.to_owned())
}

fn validate_workflow_secret_value(value: &str) -> Result<(), AuthError> {
    if value.is_empty() || value.len() > MAX_SECRET_VALUE_BYTES || value.contains('\0') {
        return Err(AuthError::InvalidInput(
            "workflow secret value must be 1..=8192 bytes without NUL".into(),
        ));
    }
    Ok(())
}

fn keystream_xor(key: &[u8; 32], nonce: &[u8; 16], data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    for (block_idx, chunk) in data.chunks(32).enumerate() {
        let mut hasher = Sha256::new();
        hasher.update(b"gitust-wf-secret-stream-v1:");
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

fn compute_secret_tag(
    key: &[u8; 32],
    nonce: &[u8; 16],
    repo_id: RepoId,
    name: &str,
    ciphertext: &[u8],
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"gitust-wf-secret-mac-v1:");
    hasher.update(key);
    hasher.update(nonce);
    hasher.update(repo_id.0.as_bytes());
    hasher.update((name.len() as u64).to_le_bytes());
    hasher.update(name.as_bytes());
    hasher.update((ciphertext.len() as u64).to_le_bytes());
    hasher.update(ciphertext);
    hasher.finalize().into()
}

fn encrypt_secret(
    repo_id: RepoId,
    name: &str,
    plaintext: &str,
    updated_at: DateTime<Utc>,
) -> StoredWorkflowSecret {
    let mut key = [0u8; 32];
    let mut nonce = [0u8; 16];
    OsRng.fill_bytes(&mut key);
    OsRng.fill_bytes(&mut nonce);
    let ciphertext = keystream_xor(&key, &nonce, plaintext.as_bytes());
    let tag_sha256 = compute_secret_tag(&key, &nonce, repo_id, name, &ciphertext);
    StoredWorkflowSecret {
        name: name.to_owned(),
        key_hex: hex::encode(key),
        nonce_hex: hex::encode(nonce),
        ciphertext_hex: hex::encode(ciphertext),
        tag_sha256,
        updated_at,
    }
}

fn decrypt_secret(repo_id: RepoId, stored: &StoredWorkflowSecret) -> Result<String, AuthError> {
    let key_bytes = hex::decode(&stored.key_hex)
        .map_err(|_| AuthError::StorageError("corrupted workflow secret key".into()))?;
    let key: [u8; 32] = key_bytes
        .try_into()
        .map_err(|_| AuthError::StorageError("corrupted workflow secret key length".into()))?;
    let nonce_bytes = hex::decode(&stored.nonce_hex)
        .map_err(|_| AuthError::StorageError("corrupted workflow secret nonce".into()))?;
    let nonce: [u8; 16] = nonce_bytes
        .try_into()
        .map_err(|_| AuthError::StorageError("corrupted workflow secret nonce length".into()))?;
    let ciphertext = hex::decode(&stored.ciphertext_hex)
        .map_err(|_| AuthError::StorageError("corrupted workflow secret ciphertext".into()))?;
    let expected_tag = compute_secret_tag(&key, &nonce, repo_id, &stored.name, &ciphertext);
    if !bool::from(expected_tag.ct_eq(&stored.tag_sha256)) {
        return Err(AuthError::StorageError(
            "workflow secret integrity verification failed".into(),
        ));
    }
    let plain_bytes = keystream_xor(&key, &nonce, &ciphertext);
    let plain = String::from_utf8(plain_bytes)
        .map_err(|_| AuthError::StorageError("corrupted workflow secret UTF-8 payload".into()))?;
    validate_workflow_secret_value(&plain)?;
    Ok(plain)
}

impl AuthStore {
    pub fn set_workflow_secret(
        &self,
        caller: &AuthenticatedPrincipal,
        expected: RepoAccess,
        name: &str,
        value: &str,
    ) -> Result<WorkflowSecretMetadata, AuthError> {
        let clean_name = validate_workflow_secret_name(name)?;
        validate_workflow_secret_value(value)?;
        let now = Utc::now();
        let encrypted = encrypt_secret(expected.repo_id, &clean_name, value, now);

        let (meta, snapshot) = {
            let mut guard = self.write_state()?;
            let active = authorize_repo_locked(&guard, caller, expected, true)?;
            if active.role != EnterpriseRole::SiteAdmin {
                return Err(AuthError::AdminPrivilegeRequired);
            }
            let list = guard.workflow_secrets.entry(expected.repo_id).or_default();
            if let Some(existing) = list.iter_mut().find(|s| s.name == clean_name) {
                *existing = encrypted;
            } else {
                if list.len() >= MAX_WORKFLOW_SECRETS_PER_REPO {
                    return Err(AuthError::InvalidInput(
                        "workflow secrets limit exceeded for repository".into(),
                    ));
                }
                list.push(encrypted);
                list.sort_by(|a, b| a.name.cmp(&b.name));
            }
            (
                WorkflowSecretMetadata {
                    name: clean_name,
                    updated_at: now,
                },
                self.maybe_snapshot(guard),
            )
        };
        self.persist_snapshot(snapshot)?;
        Ok(meta)
    }

    pub fn list_workflow_secrets(
        &self,
        caller: &AuthenticatedPrincipal,
        expected: RepoAccess,
    ) -> Result<Vec<WorkflowSecretMetadata>, AuthError> {
        let guard = self.read_state()?;
        let active = authorize_repo_locked(&guard, caller, expected, true)?;
        if active.role != EnterpriseRole::SiteAdmin {
            return Err(AuthError::AdminPrivilegeRequired);
        }
        let mut out: Vec<WorkflowSecretMetadata> = guard
            .workflow_secrets
            .get(&expected.repo_id)
            .map(|list| {
                list.iter()
                    .map(|s| WorkflowSecretMetadata {
                        name: s.name.clone(),
                        updated_at: s.updated_at,
                    })
                    .collect()
            })
            // fail-open-ok: repositories without configured workflow secrets return an empty list.
            .unwrap_or_default();
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    pub fn delete_workflow_secret(
        &self,
        caller: &AuthenticatedPrincipal,
        expected: RepoAccess,
        name: &str,
    ) -> Result<(), AuthError> {
        let clean_name = validate_workflow_secret_name(name)?;
        let snapshot = {
            let mut guard = self.write_state()?;
            let active = authorize_repo_locked(&guard, caller, expected, true)?;
            if active.role != EnterpriseRole::SiteAdmin {
                return Err(AuthError::AdminPrivilegeRequired);
            }
            let Some(list) = guard.workflow_secrets.get_mut(&expected.repo_id) else {
                return Err(AuthError::InvalidInput("workflow secret not found".into()));
            };
            let before = list.len();
            list.retain(|s| s.name != clean_name);
            if list.len() == before {
                return Err(AuthError::InvalidInput("workflow secret not found".into()));
            }
            self.maybe_snapshot(guard)
        };
        self.persist_snapshot(snapshot)
    }

    pub fn resolve_workflow_secrets(
        &self,
        caller: &AuthenticatedPrincipal,
        expected: RepoAccess,
    ) -> Result<BTreeMap<String, String>, AuthError> {
        let guard = self.read_state()?;
        let active = authorize_repo_locked(&guard, caller, expected, true)?;
        if active.role != EnterpriseRole::SiteAdmin {
            return Err(AuthError::AdminPrivilegeRequired);
        }
        let mut resolved = BTreeMap::new();
        if let Some(list) = guard.workflow_secrets.get(&expected.repo_id) {
            for item in list {
                let plain = decrypt_secret(expected.repo_id, item)?;
                resolved.insert(item.name.clone(), plain);
            }
        }
        Ok(resolved)
    }
}
