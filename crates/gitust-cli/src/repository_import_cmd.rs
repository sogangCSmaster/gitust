//! Offline import: one fixed identity, a private crash journal, then no-overwrite publication.
use crate::{
    admin_cmd::with_admin_session_token,
    repository_import_storage::{error, Directory},
};
use anyhow::{bail, Result};
use clap::{ArgAction, Args};
use gitust_core::{AuthenticatedPrincipal, EnterpriseRole, RepoId, TeamId};
use gitust_db::{hash_token_sha256, AuthStore};
use gitust_git::{GitEngine, PreparedImport, RegistrationReceipt, WorktreeServiceLock};
use serde::{Deserialize, Serialize};
use std::{
    cell::RefCell,
    fs,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

#[derive(Debug, Args)]
pub(crate) struct ImportArgs {
    /// Credential-free source supported by inbound mirror policy; never copied with local clone.
    pub source: String,
    #[arg(long)]
    pub repo: PathBuf,
    /// An existing private AuthStore, outside the import destination and staging envelope.
    #[arg(long)]
    pub auth_db: PathBuf,
    #[arg(long)]
    pub admin_username: String,
    #[arg(long, env = "GITUST_ADMIN_PASSWORD", hide_env_values = true)]
    pub admin_password: String,
    #[arg(long)]
    pub name: String,
    #[arg(long)]
    pub owner_team_id: TeamId,
    #[arg(long, action=ArgAction::Set, required=true)]
    pub private: bool,
    /// Explicit existing non-agent branch. Unborn repositories are not supported.
    #[arg(long)]
    pub default_branch: String,
    /// Required only after an interrupted preparation; first stop ALL prior import processes.
    #[arg(long)]
    pub acknowledge_stopped_import_processes: bool,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Intent {
    version: u32,
    repo_id: RepoId,
    request: [u8; 32],
    parent_dev: u64,
    parent_ino: u64,
    leaf: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Preparation {
    version: u32,
    repo_id: RepoId,
    stage_dev: u64,
    stage_ino: u64,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Ready {
    version: u32,
    repo_id: RepoId,
    snapshot: PreparedImport,
    stage_dev: u64,
    stage_ino: u64,
}

pub(crate) struct ImportTransaction {
    parent: Directory,
    envelope: Directory,
    leaf: String,
    intent: Intent,
    final_exists: bool,
    source: String,
    _claim: fs::File,
    stage: RefCell<Option<Directory>>,
    ownership: RefCell<Option<WorktreeServiceLock>>,
}
fn digest(value: &str) -> String {
    hash_token_sha256(value)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn canonical_source(raw: &str) -> Result<String> {
    let clean = GitEngine::validate_mirror_remote_url(raw)
        .map_err(|_| anyhow::anyhow!("import source is invalid or contains credentials"))?;
    if clean.contains("://") {
        let authority = clean
            .split("://")
            .nth(1)
            .and_then(|s| s.split('/').next())
            .ok_or_else(error)?;
        if authority
            .rsplit_once('@')
            .is_some_and(|(user, _)| user.contains(':'))
            || clean.contains('?')
            || clean.contains('#')
        {
            bail!("import source must not contain passwords, queries, or fragments");
        }
    }
    if !clean.contains("://") && !clean.starts_with("git@") {
        let path = fs::canonicalize(&clean).map_err(|_| error())?;
        return path.to_str().map(str::to_owned).ok_or_else(error);
    }
    Ok(clean)
}
fn paths(args: &ImportArgs) -> Result<(PathBuf, String, PathBuf, String)> {
    let leaf = args
        .repo
        .file_name()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty() && *s != "." && *s != "..")
        .ok_or_else(error)?
        .to_owned();
    let parent_path = match args.repo.parent().filter(|p| !p.as_os_str().is_empty()) {
        Some(parent) => parent,
        None => Path::new("."),
    };
    let parent = fs::canonicalize(parent_path).map_err(|_| error())?;
    if !parent.is_dir()
        || args.owner_team_id.0.is_nil()
        || args.name.is_empty()
        || args.name.len() > 1024
    {
        return Err(error());
    }
    let branch = GitEngine::validate_branch_name(&args.default_branch).map_err(|_| error())?;
    if branch != args.default_branch
        || branch.len() > 32 * 1024
        || branch == "agent"
        || branch.starts_with("agent/")
    {
        bail!("import requires an explicit non-agent default branch");
    }
    let source = canonical_source(&args.source)?;
    // canonical_source resolves local/file sources to an absolute path; network sources remain URLs.
    if Path::new(&source).is_absolute() && parent.starts_with(&source) {
        bail!("import destination must be outside the source repository");
    }
    let db_meta = fs::symlink_metadata(&args.auth_db)
        .map_err(|_| anyhow::anyhow!("import requires an existing private auth database"))?;
    if !db_meta.is_file() {
        bail!("import requires an existing regular auth database");
    }
    let db = fs::canonicalize(&args.auth_db).map_err(|_| error())?;
    let envelope = parent.join(format!(".gitust-import-{}", digest(&leaf)));
    if db.starts_with(parent.join(&leaf)) || db.starts_with(envelope) {
        bail!("auth database must be outside the import destination and journal");
    }
    Ok((parent, leaf, db, source))
}
impl ImportTransaction {
    pub(crate) fn open(args: &ImportArgs) -> Result<Self> {
        let (parent_path, leaf, db, source) = paths(args)?;
        let parent = Directory::parent(&parent_path)?;
        let prefix = format!(".gitust-import-{}", digest(&leaf));
        let claim = parent.claim(&format!("{prefix}.lock"))?;
        parent.same_path()?;
        let final_exists = match fs::symlink_metadata(parent_path.join(&leaf)) {
            Ok(meta) => {
                if !meta.is_dir() {
                    bail!("import destination already exists and is not an owned import");
                }
                true
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
            Err(_metadata_error) => return Err(error()),
        };
        let journal_exists = parent
            .open(&prefix, libc::O_RDONLY | libc::O_DIRECTORY)?
            .is_some();
        if final_exists && !journal_exists {
            bail!("import never adopts an existing destination");
        }
        let (envelope, created) = parent.child(&prefix, !final_exists)?;
        let meta = parent.file.metadata().map_err(|_| error())?;
        let request = serde_json::to_string(&(
            &source,
            db,
            args.name.as_str(),
            args.owner_team_id,
            args.private,
            args.default_branch.as_str(),
        ))
        .map_err(|_| error())?;
        let request = hash_token_sha256(&request);
        let intent = if created {
            let intent = Intent {
                version: 1,
                repo_id: RepoId::new(),
                request,
                parent_dev: meta.dev(),
                parent_ino: meta.ino(),
                leaf: leaf.clone(),
            };
            envelope.write_new("intent.json", &intent)?;
            intent
        } else {
            envelope.read::<Intent>("intent.json")?.ok_or_else(error)?
        };
        if intent.version != 1
            || intent.repo_id.0.is_nil()
            || intent.request != request
            || intent.parent_dev != meta.dev()
            || intent.parent_ino != meta.ino()
            || intent.leaf != leaf
        {
            return Err(error());
        }
        if final_exists
            && envelope
                .open("repository", libc::O_RDONLY | libc::O_DIRECTORY)?
                .is_some()
        {
            return Err(error());
        }
        Ok(Self {
            parent,
            envelope,
            leaf,
            intent,
            final_exists,
            source,
            _claim: claim,
            stage: RefCell::new(None),
            ownership: RefCell::new(None),
        })
    }
    pub(crate) fn id(&self) -> RepoId {
        self.intent.repo_id
    }
    fn preparation(&self) -> Result<Option<Preparation>> {
        let pending = self.envelope.read::<Preparation>("preparation.json")?;
        if pending
            .as_ref()
            .is_some_and(|p| p.version != 1 || p.repo_id != self.id())
        {
            return Err(error());
        }
        Ok(pending)
    }
    pub(crate) fn mark_preparing(&self) -> Result<()> {
        if self.preparation()?.is_none() {
            let (directory, created) = self.envelope.child("repository", true)?;
            if !created {
                bail!("unconfirmed import stage exists; preserve it for operator recovery");
            }
            let meta = directory.file.metadata().map_err(|_| error())?;
            self.envelope.write_new(
                "preparation.json",
                &Preparation {
                    version: 1,
                    repo_id: self.id(),
                    stage_dev: meta.dev(),
                    stage_ino: meta.ino(),
                },
            )?;
        }
        self.envelope.sync()?;
        self.parent.sync()
    }
    pub(crate) fn prepare(&self, args: &ImportArgs) -> Result<GitEngine> {
        self.parent.same_path()?;
        self.envelope.same_path()?;
        let pending = self.preparation()?.is_some();
        if pending && !args.acknowledge_stopped_import_processes {
            bail!("prior import preparation may still be running; stop all prior import processes and retry with --acknowledge-stopped-import-processes");
        }
        let ready = self.envelope.read::<Ready>("prepared.json")?;
        if ready.as_ref().is_some_and(|r| {
            r.version != 1 || r.repo_id != self.id() || r.snapshot.branch != args.default_branch
        }) {
            return Err(error());
        }
        if ready.is_none() && !pending {
            self.mark_preparing()?;
        }
        let directory = if self.final_exists {
            if ready.is_none() {
                return Err(error());
            }
            self.parent.child(&self.leaf, false)?.0
        } else {
            self.envelope.child("repository", false)?.0
        };
        let meta = directory.file.metadata().map_err(|_| error())?;
        let expected_inode = if let Some(ready) = &ready {
            (ready.stage_dev, ready.stage_ino)
        } else {
            let marker = self.preparation()?.ok_or_else(error)?;
            (marker.stage_dev, marker.stage_ino)
        };
        if (meta.dev(), meta.ino()) != expected_inode {
            return Err(error());
        }
        if self
            .preparation()?
            .is_some_and(|marker| (marker.stage_dev, marker.stage_ino) != expected_inode)
        {
            return Err(error());
        }
        let engine = GitEngine::new(&directory.path);
        let ownership = if let Some(ready) = ready {
            if engine.load_existing_repository_id().map_err(|_| error())? != self.id() {
                return Err(error());
            }
            engine
                .verify_prepared_import(&ready.snapshot)
                .map_err(|_| error())?
        } else {
            let (snapshot,ownership)=engine.prepare_repository_import(&self.source,&args.default_branch).map_err(|_|anyhow::anyhow!("import preparation failed; preserve the intent and stop prior processes before acknowledging a retry"))?;
            engine
                .initialize_import_repository_id(self.id())
                .map_err(|_| error())?;
            directory.same_path()?;
            self.parent.same_path()?;
            self.envelope.same_path()?;
            self.envelope.write_new(
                "prepared.json",
                &Ready {
                    version: 1,
                    repo_id: self.id(),
                    snapshot,
                    stage_dev: meta.dev(),
                    stage_ino: meta.ino(),
                },
            )?;
            ownership
        };
        *self.stage.borrow_mut() = Some(directory);
        *self.ownership.borrow_mut() = Some(ownership);
        if self.preparation()?.is_some() {
            self.envelope.remove_known("preparation.json")?;
        }
        Ok(engine)
    }
    pub(crate) fn publish(&self, engine: &GitEngine) -> Result<()> {
        self.parent.same_path()?;
        self.envelope.same_path()?;
        let stage = self.stage.borrow();
        let directory = stage.as_ref().ok_or_else(error)?;
        directory.same_path()?;
        if engine.load_existing_repository_id().map_err(|_| error())? != self.id() {
            return Err(error());
        }
        if !self.final_exists {
            self.envelope
                .rename_exclusive("repository", &self.parent, &self.leaf)?;
        }
        // A rename may already have succeeded when synchronization fails. Never roll it back.
        directory.file.sync_all().map_err(|_| error())?;
        self.envelope.sync()?;
        self.parent.sync()
    }
}
pub(crate) fn recheck_admin(
    store: &AuthStore,
    origin: &AuthenticatedPrincipal,
    token: &str,
) -> Result<AuthenticatedPrincipal> {
    let current = store.authenticate_session(token).map_err(|_| {
        anyhow::anyhow!("originating administrator session is no longer authorized")
    })?;
    if current.user_id != origin.user_id || current.role != EnterpriseRole::SiteAdmin {
        bail!("originating administrator session is no longer authorized");
    }
    Ok(current)
}
pub(crate) fn run(args: &ImportArgs) -> Result<RepoId> {
    let (_, _, db, _) = paths(args)?;
    let holder = RefCell::new(None);
    let store = AuthStore::open_persistent(db)
        .map_err(|_| anyhow::anyhow!("import auth database is unavailable"))?;
    let result = with_admin_session_token(
        &store,
        &args.admin_username,
        &args.admin_password,
        |origin, token| {
            recheck_admin(&store, origin, token)?;
            *holder.borrow_mut() = Some(ImportTransaction::open(args)?);
            let held = holder.borrow();
            let tx = held.as_ref().ok_or_else(error)?;
            let engine = tx.prepare(args)?;
            let current = recheck_admin(&store, origin, token)?;
            let receipt = match engine
                .repository_registration_state(tx.id())
                .map_err(|_| error())?
            {
                RegistrationReceipt::Absent => None,
                RegistrationReceipt::Pending(r) | RegistrationReceipt::Complete(r) => Some(r),
            };
            let current = recheck_admin(&store, &current, token)?;
            let registered = store
                .register_local_repository(
                    &current,
                    tx.id(),
                    &args.name,
                    args.owner_team_id,
                    args.private,
                    receipt,
                )
                .map_err(|_| {
                    anyhow::anyhow!(
                        "repository registration failed; preserve the same import intent and retry"
                    )
                })?;
            engine
                .complete_repository_registration(registered.binding)
                .map_err(|_| error())?;
            recheck_admin(&store, origin, token)?;
            tx.publish(&engine)?;
            Ok(tx.id())
        },
    );
    // Metadata/credential errors are deliberately not formatted: upstream and auth errors can carry private input.
    result.map_err(|_|anyhow::anyhow!("repository import did not complete; preserve its private journal. Retry only the identical request after inspecting its state; interrupted preparation requires stopped-process acknowledgement, while incomplete metadata requires trusted operator repair"))
}

#[cfg(test)]
#[path = "tests_import_journal.rs"]
mod journal_tests;
