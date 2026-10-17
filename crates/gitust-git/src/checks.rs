//! Required verification commands come from the committed merge target, never agent input.
use gix::prelude::HeaderExt;
use serde::Deserialize;

use crate::{GitEngine, GitEngineError};
use gitust_core::CommitSha;

const CHECKS_PATH: &str = ".gitust-checks.json";
const MAX_CHECKS_BYTES: u64 = 16_384;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequiredChecks {
    pub blob_oid: String,
    pub commands: Vec<String>,
    pub auto_run: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChecksConfig {
    version: u32,
    commands: Vec<String>,
    #[serde(default)]
    auto_run: bool,
}

impl GitEngine {
    pub fn required_checks(&self, base: &str) -> Result<Option<RequiredChecks>, GitEngineError> {
        let base = Self::validate_outbound_branch(base)?;
        let repo = self.acquire_repo()?;
        // Fully qualify the branch: a tag or SHA-looking branch must not change policy lookup.
        let mut reference = repo
            .find_reference(&format!("refs/heads/{base}"))
            .map_err(|e| invalid(format!("cannot read checks base branch: {e}")))?;
        let commit = reference
            .peel_to_commit()
            .map_err(|e| invalid(format!("cannot resolve checks base commit: {e}")))?;
        from_commit(&repo, commit)
    }

    /// Explicit local execution binds policy to one full commit, independent of later ref changes.
    pub fn required_checks_at_commit(
        &self,
        expected: &CommitSha,
    ) -> Result<Option<RequiredChecks>, GitEngineError> {
        let oid = gix::hash::ObjectId::from_hex(expected.0.as_bytes())
            .map_err(|_| invalid("checks require a full commit ID"))?;
        if oid.is_null() {
            return Err(invalid("checks require a nonzero commit ID"));
        }
        let repo = self.acquire_repo()?;
        crate::config_guard::reject_replacement_refs(&repo)?;
        let commit = repo
            .find_object(oid)
            .map_err(|_| invalid("checks commit is unavailable"))?
            .try_into_commit()
            .map_err(|_| invalid("checks target must be a commit"))?;
        from_commit(&repo, commit)
    }
}

fn from_commit(
    repo: &gix::Repository,
    commit: gix::Commit<'_>,
) -> Result<Option<RequiredChecks>, GitEngineError> {
    let tree = commit.tree().map_err(|e| invalid(e.to_string()))?;
    let Some(entry) = tree
        .lookup_entry_by_path(CHECKS_PATH)
        .map_err(|e| invalid(e.to_string()))?
    else {
        return Ok(None);
    };
    if !entry.mode().is_blob() {
        return Err(invalid("checks configuration must be a regular file"));
    }
    let header = repo
        .objects
        .header(entry.oid())
        .map_err(|e| invalid(e.to_string()))?;
    if header.kind() != gix::objs::Kind::Blob || header.size() > MAX_CHECKS_BYTES {
        return Err(invalid(
            "checks configuration must be a blob of at most 16384 bytes",
        ));
    }
    let object = entry.object().map_err(|e| invalid(e.to_string()))?;
    let config: ChecksConfig = serde_json::from_slice(&object.data)
        .map_err(|e| invalid(format!("invalid checks configuration: {e}")))?;
    if config.version != 1
        || config.commands.is_empty()
        || config.commands.len() > 8
        || config
            .commands
            .iter()
            .any(|c| c.trim().is_empty() || c.len() > 4096 || c.contains('\0'))
    {
        return Err(invalid("checks require version 1 and 1..=8 nonempty commands of at most 4096 bytes without NUL"));
    }
    Ok(Some(RequiredChecks {
        blob_oid: entry.oid().to_string(),
        commands: config.commands,
        auto_run: config.auto_run,
    }))
}

fn invalid(message: impl Into<String>) -> GitEngineError {
    GitEngineError::VerificationInvalid(message.into())
}
