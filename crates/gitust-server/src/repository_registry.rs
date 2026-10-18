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
    pub(crate) git_write: bool,
    pub(crate) mutation: Arc<tokio::sync::Mutex<()>>,
    pub(crate) local_checks_lock: Arc<tokio::sync::Mutex<()>>,
    pub(crate) local_checks_automatic: Arc<crate::local_checks_automatic::Registry>,
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
        Self::load_with_git_write(state, paths, Vec::new())
    }

    pub(crate) fn load_with_git_write(
        state: &AppState,
        read_only: Vec<PathBuf>,
        git_write: Vec<PathBuf>,
    ) -> Result<Arc<Self>, GitEngineError> {
        if read_only.len().saturating_add(git_write.len()) >= MAX_REPOSITORIES {
            return Err(invalid());
        }
        let mut entries = HashMap::new();
        entries.insert(
            state.repo_id,
            ReadRepository {
                engine: state.git_engine.clone(),
                access: state.repo_access,
                is_default: true,
                git_write: true,
                mutation: state.repo_mutation_lock.clone(),
                local_checks_lock: state.local_checks_lock.clone(),
                local_checks_automatic: state.local_checks_automatic.clone(),
            },
        );
        let mut ownership = vec![state.git_engine.lock_worktree_service()?];
        let mut common_directories =
            HashSet::from([
                std::fs::canonicalize(state.git_engine.repository_common_directory()?)
                    .map_err(|_| invalid())?,
            ]);
        let paths = read_only
            .into_iter()
            .map(|p| (p, false))
            .chain(git_write.into_iter().map(|p| (p, true)));
        for (path, git_write) in paths {
            let engine = GitEngine::new(path);
            let common = std::fs::canonicalize(engine.repository_common_directory()?)
                .map_err(|_| invalid())?;
            if !common_directories.insert(common) {
                return Err(invalid());
            }
            let guard = engine.lock_worktree_service()?;
            if git_write {
                engine.validate_ordinary_git_service()?;
            }
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
                    git_write,
                    mutation: Arc::new(tokio::sync::Mutex::new(())),
                    local_checks_lock: Arc::new(tokio::sync::Mutex::new(())),
                    local_checks_automatic: Arc::new(
                        crate::local_checks_automatic::Registry::default(),
                    ),
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
    fn primary_read_repository(&self) -> ReadRepository {
        ReadRepository {
            engine: self.git_engine.clone(),
            access: self.repo_access,
            is_default: true,
            git_write: true,
            mutation: self.repo_mutation_lock.clone(),
            local_checks_lock: self.local_checks_lock.clone(),
            local_checks_automatic: self.local_checks_automatic.clone(),
        }
    }

    pub(crate) fn read_repositories(&self) -> Vec<ReadRepository> {
        match &self.repository_registry {
            Some(registry) => {
                let mut rows: Vec<_> = registry.entries.values().cloned().collect();
                rows.sort_by_key(|row| row.access.repo_id.0);
                rows
            }
            None => vec![self.primary_read_repository()],
        }
    }

    pub(crate) fn read_repository(&self, id: RepoId) -> Option<ReadRepository> {
        match &self.repository_registry {
            Some(registry) => registry.entries.get(&id).cloned(),
            None if id == self.repo_id => Some(self.primary_read_repository()),
            None => None,
        }
    }

    pub(crate) fn writable_repository(&self, id: RepoId) -> Option<ReadRepository> {
        if id == self.repo_id {
            return Some(self.primary_read_repository());
        }
        self.read_repository(id).filter(|r| r.git_write)
    }
}
