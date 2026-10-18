//! A bounded inventory of independently owned Git repositories and online runtime settings.
use crate::state::AppState;
use axum::http::StatusCode;
use gitust_core::{RepoAccess, RepoId};
use gitust_git::{GitEngine, GitEngineError, RegistrationReceipt, WorktreeServiceLock};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
};

pub(crate) const MAX_REPOSITORIES: usize = 32;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RepoRuntimeSettings {
    #[serde(default)]
    pub(crate) archived: bool,
    #[serde(default)]
    pub(crate) workbench_enabled: Option<bool>,
    #[serde(default)]
    pub(crate) description: Option<String>,
}

#[derive(Default)]
pub(crate) struct DynamicRepositoryRegistry {
    pub(crate) online_entries: HashMap<RepoId, ReadRepository>,
    pub(crate) settings: HashMap<RepoId, RepoRuntimeSettings>,
}

#[derive(Clone)]
pub(crate) struct ReadRepository {
    pub(crate) engine: GitEngine,
    pub(crate) access: RepoAccess,
    pub(crate) is_default: bool,
    pub(crate) git_write: bool,
    pub(crate) workbench_enabled: bool,
    pub(crate) archived: bool,
    pub(crate) description: Option<String>,
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

pub(crate) fn load_persisted_repo_settings(
    engine: &GitEngine,
) -> Result<RepoRuntimeSettings, GitEngineError> {
    let common = match engine.repository_common_directory() {
        Ok(common) => common,
        Err(GitEngineError::OpenRepo { .. }) if cfg!(test) => {
            return Ok(RepoRuntimeSettings::default())
        }
        Err(e) => return Err(e),
    };
    let path = common.join("gitust-repository").join("settings.json");
    if !path.is_file() {
        return Ok(RepoRuntimeSettings::default());
    }
    let bytes = std::fs::read(path).map_err(|_| invalid())?;
    serde_json::from_slice(&bytes).map_err(|_| invalid())
}

pub(crate) fn persist_repo_settings(
    engine: &GitEngine,
    settings: &RepoRuntimeSettings,
) -> Result<(), GitEngineError> {
    let common = engine.repository_common_directory()?;
    let dir = common.join("gitust-repository");
    std::fs::create_dir_all(&dir).map_err(|_| invalid())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|_| invalid())?;
    }
    let tmp = dir.join("settings.json.tmp");
    let final_path = dir.join("settings.json");
    let bytes = serde_json::to_vec(settings).map_err(|_| invalid())?;
    std::fs::write(&tmp, &bytes).map_err(|_| invalid())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))
            .map_err(|_| invalid())?;
    }
    std::fs::rename(&tmp, &final_path).map_err(|_| invalid())?;
    Ok(())
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
        let primary_settings = load_persisted_repo_settings(&state.git_engine)?;
        entries.insert(
            state.repo_id,
            ReadRepository {
                engine: state.git_engine.clone(),
                access: state.repo_access,
                is_default: true,
                git_write: true,
                workbench_enabled: true,
                archived: primary_settings.archived,
                description: primary_settings.description,
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
            let sec_settings = load_persisted_repo_settings(&engine)?;
            let workbench_enabled =
                git_write && matches!(sec_settings.workbench_enabled, Some(true));
            if git_write {
                engine.validate_git_write_service(workbench_enabled)?;
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
                    workbench_enabled,
                    archived: sec_settings.archived,
                    description: sec_settings.description,
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

pub(crate) fn recover_dynamic_registry(
    git_engine: &GitEngine,
    primary_repo_id: RepoId,
    auth_store: &gitust_db::AuthStore,
    worktrees: &mut std::collections::VecDeque<gitust_core::AgentWorktree>,
    worktree_runtime: &mut HashMap<gitust_core::WorktreeId, crate::state::WorktreeRuntime>,
) -> Result<DynamicRepositoryRegistry, GitEngineError> {
    let mut dyn_reg = DynamicRepositoryRegistry::default();
    let primary_settings = load_persisted_repo_settings(git_engine)?;
    dyn_reg.settings.insert(primary_repo_id, primary_settings);
    let primary_common = match git_engine.repository_common_directory() {
        Ok(common) => common,
        Err(GitEngineError::OpenRepo { .. }) if cfg!(test) => return Ok(dyn_reg),
        Err(e) => return Err(e),
    };
    let hosted_root = primary_common.join("gitust-hosted-repos");
    if !hosted_root.is_dir() {
        return Ok(dyn_reg);
    }
    let read_dir = std::fs::read_dir(&hosted_root).map_err(|_| invalid())?;
    for entry in read_dir {
        let entry = entry.map_err(|_| invalid())?;
        let path = entry.path();
        if !path.is_dir()
            || path
                .extension()
                .and_then(|e| e.to_str())
                .is_none_or(|ext| ext != "git")
        {
            continue;
        }
        if dyn_reg.online_entries.len().saturating_add(1) >= MAX_REPOSITORIES {
            return Err(invalid());
        }
        let sec_engine = GitEngine::new(&path);
        let sec_id = sec_engine.load_existing_repository_id()?;
        if sec_id == primary_repo_id || dyn_reg.online_entries.contains_key(&sec_id) {
            return Err(invalid());
        }
        let RegistrationReceipt::Complete(receipt) =
            sec_engine.repository_registration_state(sec_id)?
        else {
            return Err(invalid());
        };
        let access = auth_store
            .configured_repository_access(sec_id, Some(receipt))
            .map_err(|_| invalid())?;
        let _guard = sec_engine.lock_worktree_service()?;
        sec_engine.inspect_repository_head()?;
        let sec_settings = load_persisted_repo_settings(&sec_engine)?;
        let workbench_enabled = matches!(sec_settings.workbench_enabled, Some(true));
        sec_engine.validate_git_write_service(workbench_enabled)?;
        if workbench_enabled {
            let recovered = sec_engine.recover_agent_worktrees(sec_id)?;
            if worktrees.len().saturating_add(recovered.len()) > crate::state::MAX_TRACKED_WORKTREES
            {
                return Err(GitEngineError::ApprovalState(
                    "managed worktree limit exceeded; refusing partial recovery".into(),
                ));
            }
            for m in recovered {
                let runtime = crate::state::WorktreeRuntime {
                    common_git_dir: m.common_git_dir.clone(),
                    created_by: crate::worktree_owners::read(&m)?,
                    sandbox_spec: crate::state::sandbox_spec_for(&m, auth_store)?,
                    worktree_dir: m.worktree_dir,
                    exec_lock: Arc::new(tokio::sync::Mutex::new(())),
                };
                worktree_runtime.insert(m.worktree.id, runtime);
                worktrees.push_back(m.worktree);
            }
        }
        let read_repo = ReadRepository {
            engine: sec_engine,
            access,
            is_default: false,
            git_write: true,
            workbench_enabled,
            archived: sec_settings.archived,
            description: sec_settings.description.clone(),
            mutation: Arc::new(tokio::sync::Mutex::new(())),
            local_checks_lock: Arc::new(tokio::sync::Mutex::new(())),
            local_checks_automatic: Arc::new(crate::local_checks_automatic::Registry::default()),
        };
        dyn_reg.settings.insert(sec_id, sec_settings);
        dyn_reg.online_entries.insert(sec_id, read_repo);
    }
    Ok(dyn_reg)
}

impl AppState {
    fn apply_dynamic_settings(&self, mut repo: ReadRepository) -> ReadRepository {
        let key = if repo.is_default {
            repo.access = self.repo_access;
            self.repo_id
        } else {
            repo.access.repo_id
        };
        let dyn_guard = self.dynamic_registry.read();
        if let Some(settings) = dyn_guard.settings.get(&key) {
            repo.archived = settings.archived;
            if let Some(wb) = settings.workbench_enabled {
                repo.workbench_enabled = repo.is_default || (repo.git_write && wb);
            }
            repo.description.clone_from(&settings.description);
        }
        repo
    }

    fn primary_read_repository(&self) -> ReadRepository {
        let base = ReadRepository {
            engine: self.git_engine.clone(),
            access: self.repo_access,
            is_default: true,
            git_write: true,
            workbench_enabled: true,
            archived: false,
            description: None,
            mutation: self.repo_mutation_lock.clone(),
            local_checks_lock: self.local_checks_lock.clone(),
            local_checks_automatic: self.local_checks_automatic.clone(),
        };
        self.apply_dynamic_settings(base)
    }

    pub(crate) fn read_repositories(&self) -> Vec<ReadRepository> {
        let mut by_id: HashMap<RepoId, ReadRepository> = match &self.repository_registry {
            Some(registry) => registry
                .entries
                .iter()
                .map(|(k, v)| (*k, self.apply_dynamic_settings(v.clone())))
                .collect(),
            None => HashMap::from([(self.repo_id, self.primary_read_repository())]),
        };
        let dyn_guard = self.dynamic_registry.read();
        for (id, entry) in &dyn_guard.online_entries {
            if by_id.len() < MAX_REPOSITORIES && !by_id.contains_key(id) {
                let mut cloned = entry.clone();
                if let Some(settings) = dyn_guard.settings.get(id) {
                    cloned.archived = settings.archived;
                    if let Some(wb) = settings.workbench_enabled {
                        cloned.workbench_enabled = cloned.git_write && wb;
                    }
                    cloned.description.clone_from(&settings.description);
                }
                by_id.insert(*id, cloned);
            }
        }
        drop(dyn_guard);
        let mut rows: Vec<_> = by_id.into_values().collect();
        rows.sort_by_key(|row| row.access.repo_id.0);
        rows
    }

    pub(crate) fn read_repository(&self, id: RepoId) -> Option<ReadRepository> {
        let base = match &self.repository_registry {
            Some(registry) => registry.entries.get(&id).cloned(),
            None if id == self.repo_id => Some(self.primary_read_repository()),
            None => None,
        };
        if let Some(repo) = base {
            return Some(self.apply_dynamic_settings(repo));
        }
        let dyn_guard = self.dynamic_registry.read();
        let repo = dyn_guard.online_entries.get(&id).cloned()?;
        drop(dyn_guard);
        Some(self.apply_dynamic_settings(repo))
    }

    pub(crate) fn writable_repository(&self, id: RepoId) -> Option<ReadRepository> {
        self.read_repository(id)
            .filter(|r| (r.is_default || r.git_write) && !r.archived)
    }

    pub(crate) fn workbench_repository(
        &self,
        id: RepoId,
    ) -> Result<ReadRepository, (StatusCode, String)> {
        let Some(repo) = self.read_repository(id) else {
            return Err((
                StatusCode::NOT_FOUND,
                "repository is not served by this instance".to_owned(),
            ));
        };
        if repo.archived {
            return Err((
                StatusCode::FORBIDDEN,
                "repository is archived and read-only".to_owned(),
            ));
        }
        if !repo.git_write || (!repo.is_default && !repo.workbench_enabled) {
            return Err((
                StatusCode::NOT_FOUND,
                "repository is not served by this instance".to_owned(),
            ));
        }
        Ok(repo)
    }
}
