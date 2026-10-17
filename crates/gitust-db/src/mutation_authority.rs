//! Resolve request authority only from the state protected by the mutation transaction.
//! Principal wrappers remain available for trusted operator/internal callers.
use crate::{crypto::hash_token_sha256, models::AuthState, AuthStore};
use gitust_core::{AuthError, AuthenticatedPrincipal, EnterpriseRole, UserId};

pub(crate) enum MutationAuthority<'a> {
    Operator(&'a AuthenticatedPrincipal),
    Credential(&'a str, UserId),
}

impl MutationAuthority<'_> {
    fn current(&self, state: &AuthState) -> Result<AuthenticatedPrincipal, AuthError> {
        match *self {
            Self::Operator(caller) => AuthStore::verify_active_user_locked(state, caller),
            Self::Credential(token, _) => {
                let hash = hash_token_sha256(token);
                if token.starts_with("gitust_pat_") {
                    AuthStore::authenticate_pat_hash_locked(state, &hash)
                } else {
                    AuthStore::authenticate_session_hash_locked(state, &hash, false)
                }
            }
        }
    }

    pub(crate) fn active(&self, state: &AuthState) -> Result<AuthenticatedPrincipal, AuthError> {
        let current = self.current(state)?;
        if matches!(self, Self::Credential(_, user) if *user != current.user_id) {
            return Err(AuthError::Unauthenticated);
        }
        Ok(current)
    }

    pub(crate) fn verify(&self, state: &AuthState) -> Result<UserId, AuthError> {
        let current = self.current(state)?;
        if current.role != EnterpriseRole::SiteAdmin
            || matches!(self, Self::Credential(_, user) if *user != current.user_id)
        {
            return Err(AuthError::AdminPrivilegeRequired);
        }
        Ok(current.user_id)
    }
}
