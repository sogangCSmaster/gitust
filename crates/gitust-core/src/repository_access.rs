use crate::{RepoId, TeamId};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A local operator's explicit association between one Git identity and one auth record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepositoryRegistration {
    pub version: u32,
    pub repo_id: RepoId,
    pub registration_nonce: Uuid,
}

impl RepositoryRegistration {
    pub fn new(repo_id: RepoId) -> Self {
        Self {
            version: 1,
            repo_id,
            registration_nonce: Uuid::new_v4(),
        }
    }
    pub fn valid_for(self, repo_id: RepoId) -> bool {
        self.version == 1
            && self.repo_id == repo_id
            && !repo_id.0.is_nil()
            && !self.registration_nonce.is_nil()
    }
}

/// Immutable startup evidence; current ACLs must still be checked in the auth store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RepoAccess {
    pub repo_id: RepoId,
    pub registration: Option<RepositoryRegistration>,
}

impl std::str::FromStr for TeamId {
    type Err = String;
    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        let id = Uuid::parse_str(raw).map_err(|_| "team must be a group UUID".to_owned())?;
        if id.is_nil() {
            return Err("team must be a nonnil group UUID".into());
        }
        Ok(Self(id))
    }
}
