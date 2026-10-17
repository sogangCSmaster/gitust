use crate::{
    crypto::validate_username, models::AuthState, mutation_authority::MutationAuthority, AuthStore,
};
use gitust_core::{AccountStatus, AuthError, EnterpriseRole, RepoAccess, RepoId, TeamId, UserId};

#[derive(Debug)]
pub enum GroupMemberError {
    Auth(AuthError),
    NotFound,
    Conflict,
    Limit,
    VersionExhausted,
    Inactive,
    NotPublished(AuthError),
    Unconfirmed(AuthError),
}
impl From<AuthError> for GroupMemberError {
    fn from(error: AuthError) -> Self {
        Self::Auth(error)
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupMember {
    pub repository_id: RepoId,
    pub owner_team_id: TeamId,
    pub user_id: UserId,
    pub username: String,
    pub role: EnterpriseRole,
    pub status: AccountStatus,
    pub is_member: bool,
    pub membership_version: u64,
}
fn group(state: &AuthState, expected: RepoAccess) -> Result<TeamId, AuthError> {
    let binding = expected
        .registration
        .ok_or(AuthError::ForbiddenRepositoryAccess)?;
    let row = state
        .repositories
        .get(&expected.repo_id)
        .ok_or_else(invalid)?;
    if !binding.valid_for(expected.repo_id)
        || row.binding != Some(binding)
        || row.metadata.owner_team_id.0.is_nil()
    {
        return Err(invalid());
    }
    Ok(row.metadata.owner_team_id)
}
fn view(
    state: &AuthState,
    expected: RepoAccess,
    team: TeamId,
    id: UserId,
) -> Result<GroupMember, GroupMemberError> {
    let user = state.users.get(&id).ok_or(GroupMemberError::NotFound)?;
    Ok(GroupMember {
        repository_id: expected.repo_id,
        owner_team_id: team,
        user_id: id,
        username: user.username.clone(),
        role: user.role,
        status: user.status,
        is_member: user.team_ids.contains(&team),
        membership_version: user.membership_version,
    })
}
impl AuthStore {
    /// Exact account lookup and actual owning-group metadata use one current auth snapshot.
    pub fn lookup_registered_group_member(
        &self,
        credential: &str,
        origin: UserId,
        expected: RepoAccess,
        username: &str,
    ) -> Result<GroupMember, GroupMemberError> {
        if username.len() > 128 {
            return Err(AuthError::InvalidInput("invalid account username".into()).into());
        }
        let username = validate_username(username)?.to_ascii_lowercase();
        let state = self.read_state()?;
        MutationAuthority::Credential(credential, origin).verify(&state)?;
        let team = group(&state, expected)?;
        let id = state
            .username_index
            .get(&username)
            .copied()
            .ok_or(GroupMemberError::NotFound)?;
        view(&state, expected, team, id)
    }
    /// Compare before even a no-op, preventing an old grant after a newer revocation.
    pub fn change_registered_group_member(
        &self,
        credential: &str,
        origin: UserId,
        expected: RepoAccess,
        user_id: UserId,
        expected_version: u64,
        is_member: bool,
    ) -> Result<GroupMember, GroupMemberError> {
        let (result, pending) = {
            let mut state = self.write_state().map_err(GroupMemberError::NotPublished)?;
            MutationAuthority::Credential(credential, origin).verify(&state)?;
            let team = group(&state, expected)?;
            let user = state
                .users
                .get_mut(&user_id)
                .ok_or(GroupMemberError::NotFound)?;
            if user.membership_version != expected_version {
                return Err(GroupMemberError::Conflict);
            }
            if is_member && user.status != AccountStatus::Active {
                return Err(GroupMemberError::Inactive);
            }
            let existing = user.team_ids.contains(&team);
            if existing != is_member {
                if is_member && user.team_ids.len() >= 32 {
                    return Err(GroupMemberError::Limit);
                }
                let next = user
                    .membership_version
                    .checked_add(1)
                    .ok_or(GroupMemberError::VersionExhausted)?;
                if is_member {
                    user.team_ids.push(team);
                } else {
                    user.team_ids.retain(|id| *id != team);
                }
                user.membership_version = next;
            }
            let result = view(&state, expected, team, user_id)?;
            (result, self.maybe_snapshot(state))
        };
        self.persist_snapshot_classified(pending)
            .map_err(|failure| {
                if failure.publication_attempted {
                    GroupMemberError::Unconfirmed(failure.error)
                } else {
                    GroupMemberError::NotPublished(failure.error)
                }
            })?;
        Ok(result)
    }
}
fn invalid() -> AuthError {
    AuthError::StorageError("repository registration evidence is incomplete or conflicting".into())
}
