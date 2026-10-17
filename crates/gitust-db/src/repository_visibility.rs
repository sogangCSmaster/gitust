use crate::{crypto::hash_token_sha256, AuthStore};
use gitust_core::{AuthError, EnterpriseRole, RepoAccess, RepositoryRecord, UserId};

#[derive(Debug)]
pub enum RepositoryVisibilityError {
    Auth(AuthError),
    Conflict,
    NotPublished(AuthError),
    Unconfirmed(AuthError),
}
impl From<AuthError> for RepositoryVisibilityError {
    fn from(error: AuthError) -> Self {
        Self::Auth(error)
    }
}
impl std::fmt::Display for RepositoryVisibilityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Auth(error) => std::fmt::Display::fmt(error, f),
            Self::Conflict => f.write_str("repository visibility changed; refresh before retrying"),
            Self::NotPublished(_) => f.write_str("repository visibility update was not published"),
            Self::Unconfirmed(_) => f.write_str("repository visibility update could not be confirmed; re-read current visibility before retrying"),
        }
    }
}
impl std::error::Error for RepositoryVisibilityError {}

impl AuthStore {
    /// Original login/PAT, active administrator, binding and privacy CAS share the write lock.
    pub fn change_registered_repository_visibility(
        &self,
        original_credential: &str,
        original_user: UserId,
        expected: RepoAccess,
        expected_is_private: bool,
        is_private: bool,
    ) -> Result<RepositoryRecord, RepositoryVisibilityError> {
        let binding = expected
            .registration
            .ok_or(AuthError::ForbiddenRepositoryAccess)?;
        if !binding.valid_for(expected.repo_id) {
            return Err(AuthError::StorageError(
                "repository registration evidence is incomplete or conflicting".into(),
            )
            .into());
        }
        let credential_hash = hash_token_sha256(original_credential);
        let is_pat = original_credential.starts_with("gitust_pat_");
        let (record, snapshot) = {
            let mut guard = self.write_state()?;
            let current = if is_pat {
                Self::authenticate_pat_hash_locked(&guard, &credential_hash)?
            } else {
                Self::authenticate_session_hash_locked(&guard, &credential_hash, false)?
            };
            if current.user_id != original_user || current.role != EnterpriseRole::SiteAdmin {
                return Err(AuthError::AdminPrivilegeRequired.into());
            }
            let row = guard
                .repositories
                .get_mut(&expected.repo_id)
                .ok_or_else(|| {
                    AuthError::StorageError(
                        "repository registration evidence is incomplete or conflicting".into(),
                    )
                })?;
            if row.binding != Some(binding) {
                return Err(AuthError::StorageError(
                    "repository registration evidence is incomplete or conflicting".into(),
                )
                .into());
            }
            if row.metadata.is_private != expected_is_private {
                return Err(RepositoryVisibilityError::Conflict);
            }
            row.metadata.is_private = is_private;
            let record = row.metadata.clone();
            // A matching no-op must still confirm durability after an earlier uncertain write.
            (record, self.maybe_snapshot(guard))
        };
        self.persist_snapshot_classified(snapshot)
            .map_err(|failure| {
                if failure.publication_attempted {
                    RepositoryVisibilityError::Unconfirmed(failure.error)
                } else {
                    RepositoryVisibilityError::NotPublished(failure.error)
                }
            })?;
        Ok(record)
    }
}
