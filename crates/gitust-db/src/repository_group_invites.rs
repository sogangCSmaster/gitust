use crate::{invite_authority::InviteAuthority, models::IssuedInviteLink, AuthStore};
use gitust_core::{
    AuthError, EnterpriseRole, InviteId, InvitePublicView, InviteTokenPurpose, RepoAccess, RepoId,
    TeamId, UserId, DEFAULT_INVITE_EXPIRY_HOURS,
};

pub struct RepositoryGroupInviteRequest {
    pub invite_id: InviteId,
    pub target_email: String,
    pub assigned_role: EnterpriseRole,
}
pub enum RepositoryGroupInviteOutcome {
    Created(IssuedInviteLink),
    AlreadyIssued(InvitePublicView),
}
pub struct RepositoryGroupInvite {
    pub repository_id: RepoId,
    pub owner_team_id: TeamId,
    pub outcome: RepositoryGroupInviteOutcome,
}
#[derive(Debug)]
pub enum RepositoryGroupInviteError {
    Auth(AuthError),
    Conflict,
    NotPublished(AuthError),
    Unconfirmed(AuthError),
}
impl From<AuthError> for RepositoryGroupInviteError {
    fn from(error: AuthError) -> Self {
        Self::Auth(error)
    }
}

impl AuthStore {
    /// A caller-generated key confirms metadata on replay, never another copy of the raw link.
    pub fn create_registered_repository_group_invite(
        &self,
        original_credential: &str,
        original_user: UserId,
        expected: RepoAccess,
        request: RepositoryGroupInviteRequest,
    ) -> Result<RepositoryGroupInvite, RepositoryGroupInviteError> {
        let binding = expected
            .registration
            .ok_or(AuthError::ForbiddenRepositoryAccess)?;
        if !binding.valid_for(expected.repo_id) {
            return Err(invalid().into());
        }
        if request.invite_id.0.is_nil() {
            return Err(AuthError::InvalidInput("invite ID must be nonnil".into()).into());
        }
        let email = request.target_email.trim().to_ascii_lowercase();
        let (owner_team_id, outcome, snapshot) = {
            let mut guard = self
                .write_state()
                .map_err(RepositoryGroupInviteError::NotPublished)?;
            let creator =
                InviteAuthority::Credential(original_credential, original_user).verify(&guard)?;
            let row = guard
                .repositories
                .get(&expected.repo_id)
                .ok_or_else(invalid)?;
            if row.binding != Some(binding) || row.metadata.owner_team_id.0.is_nil() {
                return Err(invalid().into());
            }
            let team = row.metadata.owner_team_id;
            let outcome = if let Some(stored) = guard.invites_by_id.get(&request.invite_id) {
                if stored.purpose != InviteTokenPurpose::AccountInvite
                    || stored.repository_origin != Some(binding)
                    || stored.created_by != creator
                    || stored.target_email != email
                    || stored.assigned_role != request.assigned_role
                    || stored.team_ids != [team]
                {
                    return Err(RepositoryGroupInviteError::Conflict);
                }
                RepositoryGroupInviteOutcome::AlreadyIssued(stored.to_public_view())
            } else {
                let (record, raw_token) = crate::invites::prepare_account_invite(
                    &guard,
                    crate::invites::AccountInviteInput {
                        id: request.invite_id,
                        creator,
                        email: &email,
                        assigned_role: request.assigned_role,
                        team_ids: vec![team],
                        hours: DEFAULT_INVITE_EXPIRY_HOURS,
                        repository_origin: Some(binding),
                    },
                )?;
                let invite = record.to_public_view();
                guard
                    .invite_hash_index
                    .insert(record.token_sha256, record.id);
                guard.invites_by_id.insert(record.id, record);
                RepositoryGroupInviteOutcome::Created(IssuedInviteLink {
                    invite,
                    invite_url_path: format!("/auth/accept-invite?token={raw_token}"),
                    raw_token,
                })
            };
            // A stored replay reconfirms durability after a previous ambiguous publication.
            (team, outcome, self.maybe_snapshot(guard))
        };
        self.persist_snapshot_classified(snapshot)
            .map_err(|failure| {
                if failure.publication_attempted {
                    RepositoryGroupInviteError::Unconfirmed(failure.error)
                } else {
                    RepositoryGroupInviteError::NotPublished(failure.error)
                }
            })?;
        Ok(RepositoryGroupInvite {
            repository_id: expected.repo_id,
            owner_team_id,
            outcome,
        })
    }
}
fn invalid() -> AuthError {
    AuthError::StorageError("repository registration evidence is incomplete or conflicting".into())
}
