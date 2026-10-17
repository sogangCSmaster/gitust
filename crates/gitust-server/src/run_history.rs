//! Bounded completed-run diagnostics. History never restores verification or approval authority.
use crate::state::{
    check_repo_bola, find_authorized_worktree, internal_err, map_auth_err, AppState,
};
use axum::{
    extract::{Path as RoutePath, State},
    http::{header, StatusCode},
    response::IntoResponse,
    Json,
};
use chrono::{DateTime, Utc};
use gitust_core::{AuthenticatedPrincipal, CommitSha, EnterpriseRole, UserId, WorktreeId};
use gitust_sandbox::SandboxExecutionResult;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

const DIRECTORY: &str = "gitust-check-runs";
const MAX_RECORDS: usize = 5;
const MAX_BYTES: u64 = 3 * 1024 * 1024;
type Failure = (StatusCode, String);

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RunRecord {
    completed_at: DateTime<Utc>,
    user_id: UserId,
    verification_requested: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) automatic: bool,
    checked_commit: Option<CommitSha>,
    checks_blob_oid: Option<String>,
    pub(crate) result: SandboxExecutionResult,
    history_truncated: bool,
}

impl RunRecord {
    fn valid(&self, id: WorktreeId) -> bool {
        let result = &self.result;
        let oid = |value: &str| {
            matches!(value.len(), 40 | 64) && value.bytes().all(|b| b.is_ascii_hexdigit())
        };
        result.worktree_id == id
            && result.command.len() <= 4096
            && result.stdout.len() <= 16 * 1024
            && result.stderr.len() <= 16 * 1024
            && (1..=9).contains(&result.steps.len())
            && result.steps.iter().all(|step| {
                step.command.len() <= 4096
                    && step.output.as_ref().is_none_or(|output| {
                        step.exit_code.is_some()
                            && output.stdout.len() <= 1024
                            && output.stderr.len() <= 1024
                    })
            })
            && !(self.automatic && self.verification_requested)
            && (self.verification_requested || self.automatic) == self.checked_commit.is_some()
            && (!self.automatic || self.checks_blob_oid.is_some())
            && self
                .checked_commit
                .as_ref()
                .is_none_or(|commit| oid(&commit.0))
            && self
                .checks_blob_oid
                .as_ref()
                .is_none_or(|value| (self.verification_requested || self.automatic) && oid(value))
    }

    pub(crate) fn new(
        user_id: UserId,
        verify: bool,
        checked: Option<CommitSha>,
        checks_blob_oid: Option<String>,
        mut result: SandboxExecutionResult,
    ) -> Self {
        let mut truncated = truncate(&mut result.command, 4096);
        truncated |= truncate(&mut result.stdout, 16 * 1024);
        truncated |= truncate(&mut result.stderr, 16 * 1024);
        for step in &mut result.steps {
            truncated |= truncate(&mut step.command, 4096);
            // Nine steps plus aggregate output and JSON escaping must still fit the 3MiB file cap.
            if let Some(output) = &mut step.output {
                truncated |= truncate(&mut output.stdout, 1024);
                truncated |= truncate(&mut output.stderr, 1024);
            }
        }
        Self {
            completed_at: Utc::now(),
            user_id,
            verification_requested: verify,
            automatic: false,
            checked_commit: checked,
            checks_blob_oid,
            result,
            history_truncated: truncated,
        }
    }
}

fn truncate(text: &mut String, cap: usize) -> bool {
    if text.len() <= cap {
        return false;
    }
    let mut boundary = cap;
    while !text.is_char_boundary(boundary) {
        boundary -= 1;
    }
    text.truncate(boundary);
    true
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct History {
    version: u32,
    worktree_id: WorktreeId,
    branch: String,
    records: Vec<RunRecord>,
}

fn storage_error(error: impl std::fmt::Display) -> Failure {
    tracing::error!(error = %error, "run history storage failed");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        "run history storage failed".into(),
    )
}

fn directory(common: &Path, create: bool) -> Result<Option<PathBuf>, Failure> {
    let dir = common.join(DIRECTORY);
    match fs::symlink_metadata(&dir) {
        Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => {}
        Ok(_) => return Err(storage_error("history path is not a real directory")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && !create => return Ok(None),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(&dir) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(storage_error(e)),
            }
            let meta = fs::symlink_metadata(&dir).map_err(storage_error)?;
            if !meta.is_dir() || meta.file_type().is_symlink() {
                return Err(storage_error("invalid history directory"));
            }
            fs::File::open(common)
                .and_then(|file| file.sync_all())
                .map_err(storage_error)?;
        }
        Err(e) => return Err(storage_error(e)),
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if fs::symlink_metadata(&dir)
            .map_err(storage_error)?
            .permissions()
            .mode()
            & 0o077
            != 0
        {
            return Err(storage_error(
                "history directory permissions must be private",
            ));
        }
    }
    Ok(Some(dir))
}

pub(crate) fn read(common: &Path, id: WorktreeId, branch: &str) -> Result<Vec<RunRecord>, Failure> {
    let Some(dir) = directory(common, false)? else {
        return Ok(Vec::new());
    };
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = match options.open(dir.join(format!("{}.json", id.0))) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(storage_error(e)),
    };
    let meta = file.metadata().map_err(storage_error)?;
    if !meta.is_file() || meta.len() > MAX_BYTES {
        return Err(storage_error("history is not a bounded regular file"));
    }
    let mut payload = Vec::new();
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut payload)
        .map_err(storage_error)?;
    if payload.len() as u64 > MAX_BYTES {
        return Err(storage_error("history exceeds limit"));
    }
    let history: History =
        serde_json::from_slice(&payload).map_err(|_| storage_error("invalid history JSON"))?;
    if history.version != 1
        || history.worktree_id != id
        || history.branch != branch
        || history.records.len() > MAX_RECORDS
        || history.records.iter().any(|record| !record.valid(id))
    {
        return Err(storage_error("history identity, version or count mismatch"));
    }
    Ok(history.records)
}

/// The caller retains the worktree operation guard throughout read/modify/write.
pub(crate) fn append(
    common: &Path,
    id: WorktreeId,
    branch: &str,
    record: RunRecord,
) -> Result<(), Failure> {
    if !record.valid(id) {
        return Err(storage_error("invalid run record"));
    }
    let dir = directory(common, true)?.ok_or_else(|| storage_error("missing history directory"))?;
    let mut records = read(common, id, branch)?;
    records.insert(0, record);
    records.truncate(MAX_RECORDS);
    let payload = serde_json::to_vec(&History {
        version: 1,
        worktree_id: id,
        branch: branch.into(),
        records,
    })
    .map_err(storage_error)?;
    if payload.len() as u64 > MAX_BYTES {
        return Err(storage_error("history exceeds limit"));
    }
    let temporary = dir.join(format!("{}.tmp", id.0));
    // One staging file per worktree bounds crash leftovers. The operation guard excludes writers.
    match fs::symlink_metadata(&temporary) {
        Ok(meta) if meta.is_file() && !meta.file_type().is_symlink() && meta.len() <= MAX_BYTES => {
            fs::remove_file(&temporary).map_err(storage_error)?
        }
        Ok(_) => return Err(storage_error("invalid interrupted history write")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(storage_error(e)),
    }
    let target = dir.join(format!("{}.json", id.0));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let mut file = options.open(&temporary).map_err(storage_error)?;
        file.write_all(&payload).map_err(storage_error)?;
        file.sync_all().map_err(storage_error)?;
        fs::rename(&temporary, target).map_err(storage_error)?;
        fs::File::open(&dir)
            .and_then(|file| file.sync_all())
            .map_err(storage_error)
    })();
    if result.is_err() {
        // fail-open-ok: only remove this writer's temporary file; return the original persistence failure.
        let _ = fs::remove_file(temporary);
    }
    result
}

pub(crate) fn remove(common: &Path, id: WorktreeId) -> Result<(), Failure> {
    let Some(dir) = directory(common, false)? else {
        return Ok(());
    };
    for extension in ["json", "tmp"] {
        match fs::remove_file(dir.join(format!("{}.{extension}", id.0))) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(storage_error(e)),
        }
    }
    fs::File::open(dir)
        .and_then(|file| file.sync_all())
        .map_err(storage_error)
}

pub(crate) async fn list_handler(
    State(state): State<AppState>,
    caller: AuthenticatedPrincipal,
    RoutePath(id): RoutePath<uuid::Uuid>,
) -> Result<impl IntoResponse, Failure> {
    if caller.role != EnterpriseRole::SiteAdmin {
        return Err(map_auth_err(gitust_core::AuthError::AdminPrivilegeRequired));
    }
    let wt = find_authorized_worktree(&state, &caller, id).await?;
    let runtime = state
        .get_worktree_runtime(wt.id)
        .ok_or_else(|| (StatusCode::NOT_FOUND, "worktree not found".into()))?;
    let repo_id = state.repo_id;
    tokio::task::spawn_blocking(move || {
        let records = read(&runtime.common_git_dir, wt.id, &wt.branch)?;
        check_repo_bola(&state.auth_store, repo_id, &caller, wt.repo_id)?;
        let current = state
            .auth_store
            .verify_active_principal(&caller)
            .map_err(map_auth_err)?;
        if current.role != EnterpriseRole::SiteAdmin {
            return Err(map_auth_err(gitust_core::AuthError::AdminPrivilegeRequired));
        }
        Ok(([(header::CACHE_CONTROL, "no-store")], Json(records)))
    })
    .await
    .map_err(internal_err)?
}
