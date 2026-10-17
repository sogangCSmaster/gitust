use chrono::{DateTime, Utc};
use gitust_core::{
    AccountStatus, AuthenticatedPrincipal, EnterpriseRole, TeamId, UserId, UserPublicProfile,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Clone, Deserialize)]
#[serde(try_from = "UserWire")]
pub(crate) struct StoredUser {
    pub(crate) id: UserId,
    pub(crate) username: String,
    pub(crate) email: String,
    pub(crate) password_hash: String,
    pub(crate) role: EnterpriseRole,
    pub(crate) status: AccountStatus,
    pub(crate) team_ids: Vec<TeamId>,
    pub(crate) membership_version: u64,
    pub(crate) created_at: DateTime<Utc>,
}

// Removing the legacy required team_ids field makes old writers reject a versioned user
// instead of silently resetting its counter and accepting an obsolete membership request.
#[derive(Deserialize)]
#[serde(untagged)]
enum UserWire {
    Legacy(LegacyUser),
    Versioned(VersionedUser),
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyUser {
    id: UserId,
    username: String,
    email: String,
    password_hash: String,
    role: EnterpriseRole,
    status: AccountStatus,
    team_ids: Vec<TeamId>,
    created_at: DateTime<Utc>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct VersionedUser {
    id: UserId,
    username: String,
    email: String,
    password_hash: String,
    role: EnterpriseRole,
    status: AccountStatus,
    membership: Membership,
    created_at: DateTime<Utc>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Membership {
    team_ids: Vec<TeamId>,
    version: u64,
}

impl TryFrom<UserWire> for StoredUser {
    type Error = &'static str;
    fn try_from(wire: UserWire) -> Result<Self, Self::Error> {
        let user = match wire {
            UserWire::Legacy(u) => Self {
                id: u.id,
                username: u.username,
                email: u.email,
                password_hash: u.password_hash,
                role: u.role,
                status: u.status,
                team_ids: u.team_ids,
                membership_version: 0,
                created_at: u.created_at,
            },
            UserWire::Versioned(u) => {
                if u.membership.version == 0 {
                    return Err("versioned membership requires a positive counter");
                }
                Self {
                    id: u.id,
                    username: u.username,
                    email: u.email,
                    password_hash: u.password_hash,
                    role: u.role,
                    status: u.status,
                    team_ids: u.membership.team_ids,
                    membership_version: u.membership.version,
                    created_at: u.created_at,
                }
            }
        };
        let mut ids = HashSet::new();
        if user
            .team_ids
            .iter()
            .any(|id| id.0.is_nil() || !ids.insert(*id))
        {
            return Err("user groups must be distinct nonnil UUIDs");
        }
        // Existing over-cap accounts may be read and reduced; only a new grant enforces32.
        Ok(user)
    }
}
#[derive(Serialize)]
struct MembershipRef<'a> {
    team_ids: &'a [TeamId],
    version: u64,
}
impl Serialize for StoredUser {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut row = serializer.serialize_struct("StoredUser", 8)?;
        row.serialize_field("id", &self.id)?;
        row.serialize_field("username", &self.username)?;
        row.serialize_field("email", &self.email)?;
        row.serialize_field("password_hash", &self.password_hash)?;
        row.serialize_field("role", &self.role)?;
        row.serialize_field("status", &self.status)?;
        if self.membership_version == 0 {
            row.serialize_field("team_ids", &self.team_ids)?;
        } else {
            row.serialize_field(
                "membership",
                &MembershipRef {
                    team_ids: &self.team_ids,
                    version: self.membership_version,
                },
            )?;
        }
        row.serialize_field("created_at", &self.created_at)?;
        row.end()
    }
}
impl StoredUser {
    pub(crate) fn to_public_profile(&self) -> UserPublicProfile {
        UserPublicProfile {
            id: self.id,
            username: self.username.clone(),
            email: self.email.clone(),
            role: self.role,
            status: self.status,
            team_ids: self.team_ids.clone(),
            created_at: self.created_at,
        }
    }
    pub(crate) fn to_principal(&self) -> AuthenticatedPrincipal {
        AuthenticatedPrincipal {
            user_id: self.id,
            username: self.username.clone(),
            role: self.role,
            team_ids: self.team_ids.clone(),
        }
    }
}
