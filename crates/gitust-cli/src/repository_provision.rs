//! Private import/create transaction; admission, crash identity and publication share one path.
use crate::{
    admin_cmd::with_admin_session_token,
    repository_import_storage::{error, Directory, ImportClaim},
};
use anyhow::{bail, Result};
use gitust_core::{AuthenticatedPrincipal, EnterpriseRole, RepoId, TeamId};
use gitust_db::{hash_token_sha256, validate_repository_name, AuthStore};
use gitust_git::{
    GitEngine, PreparedEmpty, PreparedImport, RegistrationReceipt, WorktreeServiceLock,
};
use serde::{Deserialize, Serialize};
use std::{
    cell::RefCell,
    fs,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

#[derive(Clone)]
pub(crate) enum Kind {
    Import(String),
    Create,
}
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum CreateOperation {
    Create,
}
// A present null is not a legacy field omission: reject it, just as the import-v1 reader did.
fn present_operation<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<Option<CreateOperation>, D::Error> {
    CreateOperation::deserialize(d).map(Some)
}
impl Kind {
    fn version(&self) -> u32 {
        match self {
            Self::Import(_) => 1,
            Self::Create => 2,
        }
    }
    fn operation(&self) -> Option<CreateOperation> {
        match self {
            Self::Import(_) => None,
            Self::Create => Some(CreateOperation::Create),
        }
    }
    fn accepts(&self, version: u32, operation: Option<CreateOperation>) -> bool {
        version == self.version() && operation == self.operation()
    }
}
pub(crate) struct ProvisionArgs<'a> {
    pub repo: &'a Path,
    pub auth_db: &'a Path,
    pub admin_username: &'a str,
    pub admin_password: &'a str,
    pub name: &'a str,
    pub owner_team_id: TeamId,
    pub private: bool,
    pub default_branch: &'a str,
    pub acknowledge_stopped_processes: bool,
    pub kind: Kind,
}
#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum Snapshot {
    Import(PreparedImport),
    Empty(PreparedEmpty),
}
impl Snapshot {
    fn branch(&self) -> &str {
        match self {
            Self::Import(s) => &s.branch,
            Self::Empty(s) => &s.branch,
        }
    }
    fn matches(&self, kind: &Kind) -> bool {
        matches!(
            (self, kind),
            (Self::Import(_), Kind::Import(_)) | (Self::Empty(_), Kind::Create)
        )
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Intent {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_operation"
    )]
    operation: Option<CreateOperation>,
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
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_operation"
    )]
    operation: Option<CreateOperation>,
    version: u32,
    repo_id: RepoId,
    stage_dev: u64,
    stage_ino: u64,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Ready {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_operation"
    )]
    operation: Option<CreateOperation>,
    version: u32,
    repo_id: RepoId,
    snapshot: Snapshot,
    stage_dev: u64,
    stage_ino: u64,
}

pub(crate) struct ProvisionTransaction {
    pub(crate) parent: Directory,
    pub(crate) envelope: Directory,
    pub(crate) leaf: String,
    intent: Intent,
    final_exists: bool,
    kind: Kind,
    stage: RefCell<Option<Directory>>,
    ownership: RefCell<Option<WorktreeServiceLock>>,
    // Release destination ownership after every stage and repository-service handle.
    _claim: ImportClaim,
}
fn digest(value: &str) -> String {
    hash_token_sha256(value)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn paths(args: &ProvisionArgs<'_>) -> Result<(PathBuf, String, PathBuf)> {
    // Check the DB's exact policy before login or any destination claim. Keep the raw name
    // in the request digest so existing import-v1 journals remain byte-compatible.
    validate_repository_name(args.name).map_err(|_| anyhow::anyhow!("invalid repository name"))?;
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
    if !parent.is_dir() || args.owner_team_id.0.is_nil() {
        return Err(error());
    }
    let branch = GitEngine::validate_branch_name(args.default_branch).map_err(|_| error())?;
    if branch != args.default_branch
        || branch.len() > 32 * 1024
        || branch == "agent"
        || branch.starts_with("agent/")
    {
        bail!("provisioning requires an explicit non-agent default branch");
    }
    let source = match &args.kind {
        Kind::Import(source) => source.as_str(),
        Kind::Create => "",
    };
    // canonical_source resolves local/file sources to an absolute path; network sources remain URLs.
    if Path::new(source).is_absolute() && parent.starts_with(source) {
        bail!("import destination must be outside the source repository");
    }
    let db_meta = fs::symlink_metadata(args.auth_db)
        .map_err(|_| anyhow::anyhow!("provisioning requires an existing private auth database"))?;
    if !db_meta.is_file() {
        bail!("provisioning requires an existing regular auth database");
    }
    if matches!(args.kind, Kind::Create)
        && (db_meta.uid() != unsafe { libc::geteuid() }
            || db_meta.mode() & 0o7777 != 0o600
            || db_meta.nlink() != 1)
    {
        bail!("create requires an existing private, singly linked auth database");
    }
    let db = fs::canonicalize(args.auth_db).map_err(|_| error())?;
    let envelope = parent.join(format!(".gitust-import-{}", digest(&leaf)));
    if db.starts_with(parent.join(&leaf)) || db.starts_with(envelope) {
        bail!("auth database must be outside the provisioning destination and journal");
    }
    Ok((parent, leaf, db))
}
impl ProvisionTransaction {
    pub(crate) fn open(args: &ProvisionArgs<'_>) -> Result<Self> {
        let (parent_path, leaf, db) = paths(args)?;
        let parent = Directory::parent(&parent_path)?;
        let prefix = format!(".gitust-import-{}", digest(&leaf));
        let claim = parent.claim(&format!("{prefix}.lock"))?;
        parent.same_path()?;
        let final_exists = match fs::symlink_metadata(parent_path.join(&leaf)) {
            Ok(meta) => {
                if !meta.is_dir() {
                    bail!("destination already exists and is not an owned provision");
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
            bail!("provisioning never adopts an existing destination");
        }
        let (envelope, created) = parent.child(&prefix, !final_exists)?;
        let meta = parent.file.metadata().map_err(|_| error())?;
        // The import-v1 tuple is intentionally unchanged for existing private journals.
        let source = match &args.kind {
            Kind::Import(source) => source.as_str(),
            Kind::Create => "create-v2",
        };
        let request = serde_json::to_string(&(
            source,
            db,
            args.name,
            args.owner_team_id,
            args.private,
            args.default_branch,
        ))
        .map_err(|_| error())?;
        let request = hash_token_sha256(&request);
        let intent = if created {
            let intent = Intent {
                version: args.kind.version(),
                operation: args.kind.operation(),
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
        if !args.kind.accepts(intent.version, intent.operation)
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
            kind: args.kind.clone(),
            stage: RefCell::new(None),
            ownership: RefCell::new(None),
            _claim: claim,
        })
    }
    pub(crate) fn id(&self) -> RepoId {
        self.intent.repo_id
    }
    fn preparation(&self) -> Result<Option<Preparation>> {
        let pending = self.envelope.read::<Preparation>("preparation.json")?;
        if pending
            .as_ref()
            .is_some_and(|p| !self.kind.accepts(p.version, p.operation) || p.repo_id != self.id())
        {
            return Err(error());
        }
        Ok(pending)
    }
    pub(crate) fn mark_preparing(&self) -> Result<()> {
        if self.preparation()?.is_none() {
            let (directory, created) = self.envelope.child("repository", true)?;
            if !created {
                bail!("unconfirmed provisioning stage exists; preserve it for operator recovery");
            }
            let meta = directory.file.metadata().map_err(|_| error())?;
            self.envelope.write_new(
                "preparation.json",
                &Preparation {
                    version: self.kind.version(),
                    operation: self.kind.operation(),
                    repo_id: self.id(),
                    stage_dev: meta.dev(),
                    stage_ino: meta.ino(),
                },
            )?;
        }
        self.envelope.sync()?;
        self.parent.sync()
    }
    pub(crate) fn prepare(&self, args: &ProvisionArgs<'_>) -> Result<GitEngine> {
        self.parent.same_path()?;
        self.envelope.same_path()?;
        let pending = self.preparation()?.is_some();
        if pending && !args.acknowledge_stopped_processes {
            bail!("prior preparation may still be running; stop all prior provisioning processes and retry with the matching stopped-process acknowledgement");
        }
        let ready = self.envelope.read::<Ready>("prepared.json")?;
        if ready.as_ref().is_some_and(|r| {
            !self.kind.accepts(r.version, r.operation)
                || r.repo_id != self.id()
                || r.snapshot.branch() != args.default_branch
                || !r.snapshot.matches(&self.kind)
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
            match ready.snapshot {
                Snapshot::Import(snapshot) => engine.verify_prepared_import(&snapshot),
                Snapshot::Empty(snapshot) => engine.verify_prepared_empty(&snapshot),
            }
            .map_err(|_| error())?
        } else {
            let (snapshot, ownership) = match &self.kind {
                Kind::Import(source) => engine
                    .prepare_repository_import(source, args.default_branch)
                    .map(|(s, g)| (Snapshot::Import(s), g)),
                Kind::Create => engine
                    .prepare_empty_repository(args.default_branch)
                    .map(|(s, g)| (Snapshot::Empty(s), g)),
            }
            .map_err(|_| error())?;
            engine
                .initialize_import_repository_id(self.id())
                .map_err(|_| error())?;
            directory.same_path()?;
            self.parent.same_path()?;
            self.envelope.same_path()?;
            self.envelope.write_new(
                "prepared.json",
                &Ready {
                    version: self.kind.version(),
                    operation: self.kind.operation(),
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
pub(crate) fn complete_registration_and_publish(
    args: &ProvisionArgs<'_>,
    store: &AuthStore,
    origin: &AuthenticatedPrincipal,
    token: &str,
    tx: &ProvisionTransaction,
    engine: &GitEngine,
) -> Result<()> {
    let current = recheck_admin(store, origin, token)?;
    let receipt = match engine
        .repository_registration_state(tx.id())
        .map_err(|_| error())?
    {
        RegistrationReceipt::Absent => None,
        RegistrationReceipt::Pending(r) | RegistrationReceipt::Complete(r) => Some(r),
    };
    let current = recheck_admin(store, &current, token)?;
    let registered = store
        .register_local_repository(
            &current,
            tx.id(),
            args.name,
            args.owner_team_id,
            args.private,
            receipt,
        )
        .map_err(|_| {
            anyhow::anyhow!(
                "repository registration failed; preserve the same provisioning intent and retry"
            )
        })?;
    engine
        .complete_repository_registration(registered.binding)
        .map_err(|_| error())?;
    recheck_admin(store, origin, token)?;
    tx.publish(engine)?;
    Ok(())
}
pub(crate) fn run(args: &ProvisionArgs<'_>) -> Result<RepoId> {
    let (_, _, db) = paths(args)?;
    let holder = RefCell::new(None);
    let store = AuthStore::open_persistent(db)
        .map_err(|_| anyhow::anyhow!("provisioning auth database is unavailable"))?;
    let result = with_admin_session_token(
        &store,
        args.admin_username,
        args.admin_password,
        |origin, token| {
            recheck_admin(&store, origin, token)?;
            *holder.borrow_mut() = Some(ProvisionTransaction::open(args)?);
            let held = holder.borrow();
            let tx = held.as_ref().ok_or_else(error)?;
            let engine = tx.prepare(args)?;
            complete_registration_and_publish(args, &store, origin, token, tx, &engine)?;
            Ok(tx.id())
        },
    );
    // Metadata/credential errors are deliberately not formatted: upstream and auth errors can carry private input.
    result.map_err(|_|anyhow::anyhow!("repository provisioning did not complete; preserve its private journal. Retry only the identical request after inspecting its state; interrupted preparation requires stopped-process acknowledgement, while incomplete metadata requires trusted operator repair"))
}
