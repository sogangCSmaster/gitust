//! A bounded inventory of independently owned Git repositories; secondary contexts never run jobs.
use crate::state::AppState;
use gitust_core::{RepoAccess, RepoId};
use gitust_git::{GitEngine, GitEngineError, RegistrationReceipt, WorktreeServiceLock};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
};

pub(crate) const MAX_REPOSITORIES: usize = 32;

#[derive(Clone)]
pub(crate) struct ReadRepository {
    pub(crate) engine: GitEngine,
    pub(crate) access: RepoAccess,
    pub(crate) is_default: bool,
}

pub(crate) struct RepositoryRegistry {
    entries: HashMap<RepoId, ReadRepository>,
    // Strong holders survive every cloned router and in-flight read, including cancellation.
    _ownership: Vec<WorktreeServiceLock>,
}

fn invalid() -> GitEngineError {
    GitEngineError::RepositoryIdentity(
        "configured repository inventory is invalid or unavailable".into(),
    )
}

impl RepositoryRegistry {
    pub(crate) fn load(state: &AppState, paths: Vec<PathBuf>) -> Result<Arc<Self>, GitEngineError> {
        if paths.len() >= MAX_REPOSITORIES {
            return Err(invalid());
        }
        let mut entries = HashMap::new();
        entries.insert(
            state.repo_id,
            ReadRepository {
                engine: state.git_engine.clone(),
                access: state.repo_access,
                is_default: true,
            },
        );
        let mut ownership = vec![state.git_engine.lock_worktree_service()?];
        let mut common_directories =
            HashSet::from([
                std::fs::canonicalize(state.git_engine.repository_common_directory()?)
                    .map_err(|_| invalid())?,
            ]);
        for path in paths {
            let engine = GitEngine::new(path);
            let common = std::fs::canonicalize(engine.repository_common_directory()?)
                .map_err(|_| invalid())?;
            if !common_directories.insert(common) {
                return Err(invalid());
            }
            let guard = engine.lock_worktree_service()?;
            let id = engine.load_existing_repository_id()?;
            if entries.contains_key(&id) {
                return Err(invalid());
            }
            let RegistrationReceipt::Complete(receipt) =
                engine.repository_registration_state(id)?
            else {
                return Err(invalid());
            };
            let access = RepoAccess {
                repo_id: id,
                registration: Some(receipt),
            };
            // An explicit unborn default ref is valid; malformed or broken HEADs still fail startup.
            engine.inspect_repository_head()?;
            entries.insert(
                id,
                ReadRepository {
                    engine,
                    access,
                    is_default: false,
                },
            );
            ownership.push(guard);
        }
        let expected: Vec<_> = entries
            .values()
            .map(|row| (row.access.repo_id, row.access.registration))
            .collect();
        state
            .auth_store
            .configured_repository_accesses(&expected)
            .map_err(|_| invalid())?;
        Ok(Arc::new(Self {
            entries,
            _ownership: ownership,
        }))
    }
}

impl AppState {
    pub(crate) fn read_repositories(&self) -> Vec<ReadRepository> {
        match &self.repository_registry {
            Some(registry) => {
                let mut rows: Vec<_> = registry.entries.values().cloned().collect();
                rows.sort_by_key(|row| row.access.repo_id.0);
                rows
            }
            None => vec![ReadRepository {
                engine: self.git_engine.clone(),
                access: self.repo_access,
                is_default: true,
            }],
        }
    }

    pub(crate) fn read_repository(&self, id: RepoId) -> Option<ReadRepository> {
        match &self.repository_registry {
            Some(registry) => registry.entries.get(&id).cloned(),
            None if id == self.repo_id => Some(ReadRepository {
                engine: self.git_engine.clone(),
                access: self.repo_access,
                is_default: true,
            }),
            None => None,
        }
    }
}
