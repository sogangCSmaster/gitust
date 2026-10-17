use crate::{crypto::validate_repository_name, models::PersistedSnapshot, AuthStore};
use gitust_core::{
    AuthError, AuthenticatedPrincipal, EnterpriseRole, RepoAccess, RepoId, RepositoryRecord,
    RepositoryRegistration, TeamId,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

fn invalid() -> AuthError {
    AuthError::StorageError("repository registration evidence is incomplete or conflicting".into())
}

#[derive(Debug, Clone)]
pub(crate) struct StoredRepository {
    pub(crate) metadata: RepositoryRecord,
    pub(crate) binding: Option<RepositoryRegistration>,
}
impl Serialize for StoredRepository {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        match self.binding {
            None => self.metadata.serialize(serializer),
            Some(binding) => {
                let mut row = serializer.serialize_struct("RegisteredRepository", 2)?;
                row.serialize_field("metadata", &self.metadata)?;
                row.serialize_field("binding", &binding)?;
                row.end()
            }
        }
    }
}
impl<'de> Deserialize<'de> for StoredRepository {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Row;
        impl<'de> serde::de::Visitor<'de> for Row {
            type Value = StoredRepository;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a legacy metadata record or an explicit registered repository")
            }
            fn visit_map<M: serde::de::MapAccess<'de>>(
                self,
                mut map: M,
            ) -> Result<Self::Value, M::Error> {
                use serde::de::Error;
                let (mut metadata, mut binding) = (None, None);
                let (mut id, mut name, mut owner_team_id, mut is_private, mut created_at) =
                    (None, None, None, None, None);
                let mut seen = HashSet::new();
                while let Some(key) = map.next_key::<String>()? {
                    if !seen.insert(key.clone()) {
                        return Err(M::Error::custom("duplicate repository record field"));
                    }
                    match key.as_str() {
                        "metadata" => metadata = Some(map.next_value::<RepositoryRecord>()?),
                        "binding" => binding = Some(map.next_value::<RepositoryRegistration>()?),
                        "id" => id = Some(map.next_value()?),
                        "name" => name = Some(map.next_value()?),
                        "owner_team_id" => owner_team_id = Some(map.next_value()?),
                        "is_private" => is_private = Some(map.next_value()?),
                        "created_at" => created_at = Some(map.next_value()?),
                        _ => return Err(M::Error::custom("unknown repository record field")),
                    }
                }
                if metadata.is_some() || binding.is_some() {
                    if id.is_some()
                        || name.is_some()
                        || owner_team_id.is_some()
                        || is_private.is_some()
                        || created_at.is_some()
                    {
                        return Err(M::Error::custom("mixed repository record formats"));
                    }
                    return Ok(StoredRepository {
                        metadata: metadata.ok_or_else(|| M::Error::missing_field("metadata"))?,
                        binding: Some(binding.ok_or_else(|| M::Error::missing_field("binding"))?),
                    });
                }
                Ok(StoredRepository {
                    metadata: RepositoryRecord {
                        id: id.ok_or_else(|| M::Error::missing_field("id"))?,
                        name: name.ok_or_else(|| M::Error::missing_field("name"))?,
                        owner_team_id: owner_team_id
                            .ok_or_else(|| M::Error::missing_field("owner_team_id"))?,
                        is_private: is_private
                            .ok_or_else(|| M::Error::missing_field("is_private"))?,
                        created_at: created_at
                            .ok_or_else(|| M::Error::missing_field("created_at"))?,
                    },
                    binding: None,
                })
            }
        }
        deserializer.deserialize_map(Row)
    }
}
impl From<RepositoryRecord> for StoredRepository {
    fn from(metadata: RepositoryRecord) -> Self {
        Self {
            metadata,
            binding: None,
        }
    }
}
impl PersistedSnapshot {
    pub(crate) fn validate_repository_records(&self) -> Result<(), AuthError> {
        let mut ids = HashSet::new();
        for row in &self.repositories {
            if row.metadata.id.0.is_nil()
                || !ids.insert(row.metadata.id)
                || row.binding.is_some_and(|binding| {
                    !binding.valid_for(row.metadata.id) || row.metadata.owner_team_id.0.is_nil()
                })
            {
                return Err(invalid());
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizedRepository {
    pub id: RepoId,
    pub metadata: Option<RepositoryRecord>,
    pub can_rename: bool,
    pub can_change_visibility: bool,
}

pub struct RegisteredRepository {
    pub metadata: RepositoryRecord,
    pub binding: RepositoryRegistration,
}

impl AuthStore {
    /// Only the pair of explicit registration anchors authorizes a configured service.
    pub fn configured_repository_access(
        &self,
        repo_id: RepoId,
        receipt: Option<RepositoryRegistration>,
    ) -> Result<RepoAccess, AuthError> {
        self.configured_repository_accesses(&[(repo_id, receipt)])?
            .pop()
            .ok_or_else(invalid)
    }

    /// Startup checks all explicitly configured anchors under one authoritative snapshot.
    pub fn configured_repository_accesses(
        &self,
        expected: &[(RepoId, Option<RepositoryRegistration>)],
    ) -> Result<Vec<RepoAccess>, AuthError> {
        if expected.is_empty() || expected.len() > 32 {
            return Err(invalid());
        }
        let guard = self.read_state()?;
        let mut ids = HashSet::new();
        let mut result = Vec::with_capacity(expected.len());
        for &(repo_id, receipt) in expected {
            if repo_id.0.is_nil() || !ids.insert(repo_id) {
                return Err(invalid());
            }
            match (guard.repositories.get(&repo_id), receipt) {
                (None, None) => result.push(RepoAccess {
                    repo_id,
                    registration: None,
                }),
                (Some(row), Some(receipt))
                    if receipt.valid_for(repo_id) && row.binding == Some(receipt) =>
                {
                    result.push(RepoAccess {
                        repo_id,
                        registration: Some(receipt),
                    })
                }
                _ => return Err(invalid()),
            }
        }
        Ok(result)
    }

    /// Filters only configured services, checking pinned bindings and current membership once.
    pub fn authorized_repository_catalog(
        &self,
        caller: &AuthenticatedPrincipal,
        expected: &[RepoAccess],
    ) -> Result<Vec<AuthorizedRepository>, AuthError> {
        if expected.is_empty() || expected.len() > 32 {
            return Err(invalid());
        }
        let guard = self.read_state()?;
        let active = Self::verify_active_user_locked(&guard, caller)?;
        let mut visible = Vec::with_capacity(expected.len());
        let mut ids = HashSet::new();
        for access in expected {
            if access.repo_id.0.is_nil() || !ids.insert(access.repo_id) {
                return Err(invalid());
            }
            let row = guard.repositories.get(&access.repo_id);
            match (access.registration, row) {
                (None, None) => visible.push(AuthorizedRepository {
                    id: access.repo_id,
                    metadata: None,
                    can_rename: false,
                    can_change_visibility: false,
                }),
                (Some(binding), Some(row))
                    if binding.valid_for(access.repo_id) && row.binding == Some(binding) =>
                {
                    if !row.metadata.is_private
                        || active.role == EnterpriseRole::SiteAdmin
                        || active.team_ids.contains(&row.metadata.owner_team_id)
                    {
                        visible.push(AuthorizedRepository {
                            id: access.repo_id,
                            metadata: Some(row.metadata.clone()),
                            can_rename: active.role == EnterpriseRole::SiteAdmin,
                            can_change_visibility: active.role == EnterpriseRole::SiteAdmin,
                        });
                    }
                }
                _ => return Err(invalid()),
            }
        }
        Ok(visible)
    }

    /// Changes only the display name of an exactly pinned registered repository.
    pub fn rename_registered_repository(
        &self,
        caller: &AuthenticatedPrincipal,
        expected: RepoAccess,
        name: &str,
    ) -> Result<RepositoryRecord, AuthError> {
        let name = validate_repository_name(name)?;
        let binding = expected
            .registration
            .ok_or(AuthError::ForbiddenRepositoryAccess)?;
        if !binding.valid_for(expected.repo_id) {
            return Err(invalid());
        }
        let (record, snapshot) = {
            let mut guard = self.write_state()?;
            let active = Self::verify_active_user_locked(&guard, caller)?;
            if active.role != EnterpriseRole::SiteAdmin {
                return Err(AuthError::AdminPrivilegeRequired);
            }
            let row = guard
                .repositories
                .get_mut(&expected.repo_id)
                .ok_or_else(invalid)?;
            if row.binding != Some(binding) {
                return Err(invalid());
            }
            row.metadata.name = name;
            let record = row.metadata.clone();
            // Even an identical-name retry must reconfirm persistence after an uncertain fsync.
            (record, self.maybe_snapshot(guard))
        };
        self.persist_snapshot(snapshot)?;
        Ok(record)
    }

    /// Offline CLI registration: existing metadata is never adopted, and retries retain the nonce.
    pub fn register_local_repository(
        &self,
        caller: &AuthenticatedPrincipal,
        repo_id: RepoId,
        name: &str,
        owner_team_id: TeamId,
        is_private: bool,
        receipt: Option<RepositoryRegistration>,
    ) -> Result<RegisteredRepository, AuthError> {
        let name = validate_repository_name(name)?;
        if repo_id.0.is_nil()
            || owner_team_id.0.is_nil()
            || receipt.is_some_and(|r| !r.valid_for(repo_id))
        {
            return Err(invalid());
        }
        let (result, snapshot) = {
            let mut guard = self.write_state()?;
            let current = Self::verify_active_user_locked(&guard, caller)?;
            if current.role != EnterpriseRole::SiteAdmin {
                return Err(AuthError::AdminPrivilegeRequired);
            }
            let result = if let Some(row) = guard.repositories.get(&repo_id) {
                let binding = row.binding.ok_or_else(invalid)?;
                if !binding.valid_for(repo_id)
                    || receipt.is_some_and(|r| r != binding)
                    || row.metadata.name != name
                    || row.metadata.owner_team_id != owner_team_id
                    || row.metadata.is_private != is_private
                {
                    return Err(invalid());
                }
                RegisteredRepository {
                    metadata: row.metadata.clone(),
                    binding,
                }
            } else {
                if receipt.is_some() {
                    return Err(invalid());
                }
                let metadata = RepositoryRecord {
                    id: repo_id,
                    name,
                    owner_team_id,
                    is_private,
                    created_at: chrono::Utc::now(),
                };
                let binding = RepositoryRegistration::new(repo_id);
                guard.repositories.insert(
                    repo_id,
                    StoredRepository {
                        metadata: metadata.clone(),
                        binding: Some(binding),
                    },
                );
                RegisteredRepository { metadata, binding }
            };
            (result, self.maybe_snapshot(guard))
        };
        self.persist_snapshot(snapshot)?;
        Ok(result)
    }

    /// The pinned binding, refreshed account and current team ACL share one auth snapshot.
    pub fn authorize_service_repository(
        &self,
        caller: &AuthenticatedPrincipal,
        expected: RepoAccess,
        target: RepoId,
    ) -> Result<(), AuthError> {
        if expected.repo_id.0.is_nil() || target.0.is_nil() {
            return Err(AuthError::ForbiddenRepositoryAccess);
        }
        let guard = self.read_state()?;
        // Legacy stdio operator fixtures may have no accounts; registered access never permits this.
        let active = if guard.users.is_empty() && expected.registration.is_none() {
            caller.clone()
        } else {
            Self::verify_active_user_locked(&guard, caller)?
        };
        let row = guard.repositories.get(&target);
        if target == expected.repo_id {
            match (expected.registration, row) {
                (None, None) => return Ok(()),
                (Some(binding), Some(row))
                    if binding.valid_for(target) && row.binding == Some(binding) => {}
                _ => return Err(AuthError::ForbiddenRepositoryAccess),
            }
        }
        let row = row.ok_or(AuthError::ForbiddenRepositoryAccess)?;
        if row.metadata.is_private
            && active.role != EnterpriseRole::SiteAdmin
            && !active.team_ids.contains(&row.metadata.owner_team_id)
        {
            return Err(AuthError::ForbiddenRepositoryAccess);
        }
        Ok(())
    }
}
