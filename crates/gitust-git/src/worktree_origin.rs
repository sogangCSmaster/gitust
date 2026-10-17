//! Immutable creation target, independent of reflog retention. Missing legacy evidence grants no target.
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;

use gitust_core::{CommitSha, WorktreeId};
use serde::{Deserialize, Serialize};

use crate::{io_err, GitEngine, GitEngineError, MAX_PROMPT_SUMMARY_BYTES};

const FILE: &str = "gitust-origin.json";
// A 4 KiB display label can expand sixfold through JSON control-character escaping.
const CAP: u64 = 32 * 1024;
const REFLOG_CAP: u64 = 8192;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WorktreeOrigin {
    version: u32,
    worktree_id: WorktreeId,
    pub(crate) branch: String,
    pub(crate) base_branch: String,
    pub(crate) base_commit: CommitSha,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) prompt_summary: Option<String>,
}

fn invalid() -> GitEngineError {
    GitEngineError::VerificationInvalid("worktree creation metadata is invalid or unavailable; restore trusted metadata or recreate the worktree".into())
}

fn open(path: &Path) -> Result<Option<fs::File>, GitEngineError> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = match options.open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(io_err(path, &e)),
    };
    let meta = file.metadata().map_err(|e| io_err(path, &e))?;
    if !meta.is_file() {
        return Err(invalid());
    }
    Ok(Some(file))
}

pub(crate) fn persist(
    admin: &Path,
    id: WorktreeId,
    branch: &str,
    base: &str,
    commit: &CommitSha,
    prompt_summary: Option<&str>,
) -> Result<(), GitEngineError> {
    if prompt_summary.is_some_and(|summary| summary.len() > MAX_PROMPT_SUMMARY_BYTES) {
        return Err(invalid());
    }
    let record = WorktreeOrigin {
        version: 1,
        worktree_id: id,
        branch: branch.into(),
        base_branch: base.into(),
        base_commit: commit.clone(),
        prompt_summary: prompt_summary.map(str::to_owned),
    };
    let bytes = serde_json::to_vec(&record).map_err(|_| invalid())?;
    if bytes.len() as u64 > CAP {
        return Err(invalid());
    }
    let path = admin.join(FILE);
    let temporary = admin.join("gitust-origin.tmp");
    match fs::symlink_metadata(&temporary) {
        Ok(meta) if meta.is_file() && !meta.file_type().is_symlink() && meta.len() <= CAP => {
            fs::remove_file(&temporary).map_err(|e| io_err(&temporary, &e))?;
        }
        Ok(_) => return Err(invalid()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(io_err(&temporary, &e)),
    }
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let result = (|| {
        let mut file = options
            .open(&temporary)
            .map_err(|e| io_err(&temporary, &e))?;
        file.write_all(&bytes).map_err(|e| io_err(&temporary, &e))?;
        file.sync_all().map_err(|e| io_err(&temporary, &e))?;
        // Atomic publication without replacing an existing immutable creation record.
        fs::hard_link(&temporary, &path).map_err(|e| io_err(&path, &e))?;
        fs::remove_file(&temporary).map_err(|e| io_err(&temporary, &e))?;
        fs::File::open(admin)
            .and_then(|dir| dir.sync_all())
            .map_err(|e| io_err(admin, &e))
    })();
    if result.is_err() {
        // fail-open-ok: remove only this record's staging file and propagate the original write failure.
        let _ = fs::remove_file(&temporary);
    }
    result
}

pub(crate) fn read_or_migrate(
    admin: &Path,
    common: &Path,
    id: WorktreeId,
    branch: Option<&str>,
) -> Result<Option<WorktreeOrigin>, GitEngineError> {
    let path = admin.join(FILE);
    if let Some(file) = open(&path)? {
        let mut bytes = Vec::new();
        file.take(CAP + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| io_err(&path, &e))?;
        if bytes.len() as u64 > CAP {
            return Err(invalid());
        }
        let record: WorktreeOrigin = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
        let sha = &record.base_commit.0;
        if record.version != 1
            || record.worktree_id != id
            || GitEngine::validate_agent_branch_name(&record.branch)? != record.branch
            || GitEngine::validate_branch_name(&record.base_branch)? != record.base_branch
            || branch.is_some_and(|branch| branch != record.branch)
            || !matches!(sha.len(), 40 | 64)
            || !sha.bytes().all(|b| b.is_ascii_hexdigit())
            || record
                .prompt_summary
                .as_ref()
                .is_some_and(|summary| summary.len() > MAX_PROMPT_SUMMARY_BYTES)
        {
            return Err(invalid());
        }
        return Ok(Some(record));
    }
    let Some(branch) = branch else {
        return Ok(None);
    };
    let branch = GitEngine::validate_agent_branch_name(branch)?;
    let log_path = common.join("logs/refs/heads").join(&branch);
    let Some(file) = open(&log_path)? else {
        return Ok(None);
    };
    // Only the creation entry is evidence. Later reflog entries must never invent a target.
    let mut first = String::new();
    BufReader::new(file.take(REFLOG_CAP + 1))
        .read_line(&mut first)
        .map_err(|e| io_err(&log_path, &e))?;
    if first.len() as u64 > REFLOG_CAP {
        return Ok(None);
    }
    let Some((meta, message)) = first.trim_end_matches(['\r', '\n']).split_once('\t') else {
        return Ok(None);
    };
    let mut fields = meta.split_whitespace();
    let (Some(old), Some(sha)) = (fields.next(), fields.next()) else {
        return Ok(None);
    };
    if !matches!(sha.len(), 40 | 64)
        || old.len() != sha.len()
        || !old.bytes().all(|b| b == b'0')
        || !sha.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Ok(None);
    }
    let Some(base) = message.strip_prefix("gitust: create agent branch from ") else {
        return Ok(None);
    };
    let base = match GitEngine::validate_branch_name(base) {
        Ok(base) if base == message["gitust: create agent branch from ".len()..] => base,
        Ok(_) | Err(_) => return Ok(None), // fail-open-ok: absent legacy evidence grants no verification/merge target.
    };
    let commit = CommitSha(sha.to_owned());
    persist(admin, id, &branch, &base, &commit, None)?;
    Ok(Some(WorktreeOrigin {
        version: 1,
        worktree_id: id,
        branch,
        base_branch: base,
        base_commit: commit,
        prompt_summary: None,
    }))
}
