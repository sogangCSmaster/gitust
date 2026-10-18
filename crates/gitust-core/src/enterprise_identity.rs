//! Enterprise identity models for Slice 9 (Phase 4.4):
//! OIDC/LDAP Single Sign-On (SSO) providers, RFC 6238 TOTP Two-Factor Authentication (2FA),
//! and Tamper-Evident SHA-256 Hash-Chained Audit Trail records.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SsoProtocol {
    Oidc,
    Ldap,
}

impl SsoProtocol {
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Oidc => "oidc",
            Self::Ldap => "ldap",
        }
    }
}

/// Public representation of a registered OIDC or LDAP SSO identity provider.
/// Never exposes the provider's HMAC/client signing secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SsoProviderRecord {
    pub id: Uuid,
    pub name: String,
    pub protocol: SsoProtocol,
    pub issuer_or_url: String,
    pub client_id: String,
    pub allowed_domains: Vec<String>,
    pub enabled: bool,
    pub require_invite_binding: bool,
    pub created_at: String,
}

/// Current RFC 6238 TOTP 2FA enrollment state for an authenticated user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TotpStatus {
    pub enrolled: bool,
    pub verified: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enrolled_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used_step: Option<u64>,
}

/// Returned once when a user initiates TOTP enrollment (`POST /api/v1/auth/totp/enroll`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TotpEnrollmentChallenge {
    pub secret_base32: String,
    pub otpauth_uri: String,
    pub digits: u32,
    pub period_secs: u64,
}

/// Single entry in the tamper-evident SHA-256 hash-chained enterprise audit log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEventRecord {
    pub seq: u64,
    pub timestamp: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor_user_id: Option<Uuid>,
    pub actor_username: String,
    pub action: String,
    pub target: String,
    pub prev_hash: String,
    pub entry_hash: String,
}

/// Cryptographic verification report for the hash-chained audit trail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditChainVerificationReport {
    pub verified: bool,
    pub entries_checked: usize,
    pub head_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_invalid_seq: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}
