//! Bounded in-memory OAuth 2.0 Dynamic Client Registration and PKCE S256 code store for MCP.
use chrono::{DateTime, Utc};
use gitust_core::UserId;
use std::collections::{HashMap, VecDeque};

/// Maximum number of dynamically registered OAuth 2.0 MCP clients kept in memory.
pub const MAX_OAUTH_CLIENTS: usize = 256;
/// Maximum number of pending single-use OAuth 2.0 authorization codes kept in memory.
pub const MAX_OAUTH_CODES: usize = 512;
/// Lifetime of an OAuth 2.0 authorization code in seconds (5 minutes).
pub const OAUTH_CODE_TTL_SECS: i64 = 300;

/// Dynamically registered OAuth 2.0 public client metadata (RFC 7591) for MCP clients.
#[derive(Debug, Clone)]
pub struct RegisteredOAuthClient {
    pub client_id: String,
    pub client_name: String,
    pub redirect_uris: Vec<String>,
    pub created_at: DateTime<Utc>,
}

/// Single-use OAuth 2.0 Authorization Code bound to a user, client, redirect URI, and PKCE S256 challenge.
#[derive(Debug, Clone)]
pub struct PendingOAuthCode {
    pub code_sha256: [u8; 32],
    pub authorizing_credential: [u8; 32],
    pub user_id: UserId,
    pub client_id: String,
    pub redirect_uri: String,
    pub code_challenge_s256: String,
    pub expires_at: DateTime<Utc>,
}

/// Bounded in-memory store for OAuth 2.0 Dynamic Client Registration and single-use PKCE codes.
/// Access tokens themselves are issued directly via `AuthStore::issue_oauth_session` so there is
/// zero split-brain session state and account deactivation revokes OAuth tokens immediately.
#[derive(Default)]
pub struct OAuthMcpStore {
    clients: HashMap<String, RegisteredOAuthClient>,
    codes_by_hash: HashMap<[u8; 32], PendingOAuthCode>,
    code_order: VecDeque<[u8; 32]>,
}

impl OAuthMcpStore {
    pub fn insert_client(&mut self, client: RegisteredOAuthClient) -> bool {
        if self.clients.len() >= MAX_OAUTH_CLIENTS && !self.clients.contains_key(&client.client_id)
        {
            return false;
        }
        self.clients.insert(client.client_id.clone(), client);
        true
    }

    pub fn get_client(&self, client_id: &str) -> Option<&RegisteredOAuthClient> {
        self.clients.get(client_id)
    }

    pub fn insert_code(&mut self, record: PendingOAuthCode) {
        let now = Utc::now();
        self.codes_by_hash.retain(|_, c| c.expires_at > now);
        self.code_order
            .retain(|hash| self.codes_by_hash.contains_key(hash));
        while self.codes_by_hash.len() >= MAX_OAUTH_CODES {
            if let Some(oldest_hash) = self.code_order.pop_front() {
                self.codes_by_hash.remove(&oldest_hash);
            } else {
                break;
            }
        }
        let hash = record.code_sha256;
        self.code_order.push_back(hash);
        self.codes_by_hash.insert(hash, record);
    }

    /// Atomically removes and returns a single-use authorization code by its SHA-256 hash.
    pub fn take_code(&mut self, code_sha256: &[u8; 32]) -> Option<PendingOAuthCode> {
        let removed = self.codes_by_hash.remove(code_sha256)?;
        self.code_order.retain(|h| h != code_sha256);
        Some(removed)
    }
}
