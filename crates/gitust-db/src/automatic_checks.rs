//! Account authority for an already accepted repository-policy job, never a login credential.
use gitust_core::{AccountStatus, AuthError, AuthenticatedPrincipal, EnterpriseRole, UserId};

use crate::AuthStore;

impl AuthStore {
    /// Resolves current account authority for an accepted automatic Checks job.
    /// This does not authenticate a request or restore its originating session/PAT.
    /// Empty stores have no account authority, including the local-operator exception.
    pub fn authorize_automatic_checks_actor(
        &self,
        user_id: UserId,
    ) -> Result<AuthenticatedPrincipal, AuthError> {
        let state = self.read_state()?;
        let user = state
            .users
            .get(&user_id)
            .ok_or(AuthError::Unauthenticated)?;
        if user.status != AccountStatus::Active {
            return Err(AuthError::AccountDeactivated);
        }
        if user.role != EnterpriseRole::SiteAdmin {
            return Err(AuthError::AdminPrivilegeRequired);
        }
        Ok(user.to_principal())
    }
}
