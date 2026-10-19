//! Versioned AEAD envelopes. Persistent keys are supplied by the operator, never serialized.
use aes_gcm::{
    aead::{Aead, KeyInit, Payload},
    Aes256Gcm, Nonce,
};
use gitust_core::AuthError;
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

#[derive(Clone)]
pub(crate) struct SecretKey(Option<Arc<Zeroizing<[u8; 32]>>>);
impl std::fmt::Debug for SecretKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretKey([redacted])")
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct StoredSecret {
    #[serde(default)]
    pub(crate) secret_version: u8,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) key_hex: String,
    pub(crate) nonce_hex: String,
    pub(crate) ciphertext_hex: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) tag_sha256: Option<[u8; 32]>,
}
impl std::fmt::Debug for StoredSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoredSecret")
            .field("version", &self.secret_version)
            .finish_non_exhaustive()
    }
}
fn invalid() -> AuthError {
    AuthError::StorageError(
        "secret key or encrypted record is missing, unsupported or invalid".into(),
    )
}
impl SecretKey {
    #[cfg(test)]
    pub(crate) fn absent() -> Self {
        Self(None)
    }
    pub(crate) fn supplied(key: [u8; 32]) -> Self {
        Self(Some(Arc::new(Zeroizing::new(key))))
    }
    pub(crate) fn ephemeral() -> Self {
        let mut key = [0; 32];
        OsRng.fill_bytes(&mut key);
        Self::supplied(key)
    }
    pub(crate) fn from_environment() -> Result<Self, AuthError> {
        match std::env::var("GITUST_SECRET_KEY") {
            Ok(raw) => {
                let raw = Zeroizing::new(raw);
                if raw.len() != 64 {
                    return Err(invalid());
                }
                let mut key = Zeroizing::new([0; 32]);
                hex::decode_to_slice(raw.as_bytes(), key.as_mut()).map_err(|_| invalid())?;
                Ok(Self(Some(Arc::new(key))))
            }
            Err(std::env::VarError::NotPresent) => Ok(Self(None)),
            Err(_) => Err(invalid()),
        }
    }
    fn cipher(&self) -> Result<Aes256Gcm, AuthError> {
        let key = self.0.as_ref().ok_or_else(|| {
            AuthError::StorageError(
                "GITUST_SECRET_KEY (64 hex characters) is required for persistent secrets".into(),
            )
        })?;
        Aes256Gcm::new_from_slice(key.as_ref().as_ref()).map_err(|_| invalid())
    }
    pub(crate) fn seal(&self, context: &str, plaintext: &[u8]) -> Result<StoredSecret, AuthError> {
        let mut nonce = [0u8; 12];
        OsRng.fill_bytes(&mut nonce);
        let ciphertext = self
            .cipher()?
            .encrypt(
                &Nonce::from(nonce),
                Payload {
                    msg: plaintext,
                    aad: context.as_bytes(),
                },
            )
            .map_err(|_| invalid())?;
        Ok(StoredSecret {
            secret_version: 1,
            key_hex: String::new(),
            nonce_hex: hex::encode(nonce),
            ciphertext_hex: hex::encode(ciphertext),
            tag_sha256: None,
        })
    }
    pub(crate) fn open(&self, context: &str, stored: &StoredSecret) -> Result<Vec<u8>, AuthError> {
        if stored.secret_version != 1 || !stored.key_hex.is_empty() || stored.tag_sha256.is_some() {
            return Err(invalid());
        }
        let nonce: [u8; 12] = hex::decode(&stored.nonce_hex)
            .map_err(|_| invalid())?
            .try_into()
            .map_err(|_| invalid())?;
        let ciphertext = hex::decode(&stored.ciphertext_hex).map_err(|_| invalid())?;
        self.cipher()?
            .decrypt(
                &Nonce::from(nonce),
                Payload {
                    msg: &ciphertext,
                    aad: context.as_bytes(),
                },
            )
            .map_err(|_| invalid())
    }
    /// Legacy decoding is restricted to upgrade; ordinary reads accept only AEAD records.
    pub(crate) fn migrate(
        &self,
        context: &str,
        family: &str,
        legacy_binding: &[u8],
        stored: &mut StoredSecret,
    ) -> Result<(), AuthError> {
        if stored.secret_version != 0 {
            let _plain = Zeroizing::new(self.open(context, stored)?);
            return Ok(());
        }
        self.cipher()?;
        let key = Zeroizing::new(hex::decode(&stored.key_hex).map_err(|_| invalid())?);
        let nonce = hex::decode(&stored.nonce_hex).map_err(|_| invalid())?;
        if key.len() != 32 || nonce.len() != 16 {
            return Err(invalid());
        }
        let ciphertext = hex::decode(&stored.ciphertext_hex).map_err(|_| invalid())?;
        let mut mac = Sha256::new();
        mac.update(format!("gitust-{family}-secret-mac-v1:"));
        mac.update(&*key);
        mac.update(&nonce);
        mac.update(legacy_binding);
        mac.update((ciphertext.len() as u64).to_le_bytes());
        mac.update(&ciphertext);
        let expected: [u8; 32] = mac.finalize().into();
        if !bool::from(expected.ct_eq(&stored.tag_sha256.ok_or_else(invalid)?)) {
            return Err(invalid());
        }
        let mut plaintext = Zeroizing::new(Vec::with_capacity(ciphertext.len()));
        for (index, chunk) in ciphertext.chunks(32).enumerate() {
            let mut stream = Sha256::new();
            stream.update(format!("gitust-{family}-secret-stream-v1:"));
            stream.update(&*key);
            stream.update(&nonce);
            stream.update((index as u64).to_le_bytes());
            plaintext.extend(chunk.iter().zip(stream.finalize()).map(|(a, b)| a ^ b));
        }
        *stored = self.seal(context, &plaintext)?;
        Ok(())
    }
}
