//! Committed Checks policy lookup and exact ordinary branch selection never grant approval.
use gix::prelude::HeaderExt;
use serde::{Deserialize, Serialize};

use crate::{GitEngine, GitEngineError, MAX_BRANCH_NAME_BYTES};
use gitust_core::CommitSha;

const CHECKS_PATH: &str = ".gitust-checks.json";
const MAX_CHECKS_BYTES: u64 = 16_384;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChecksWorkflowStep {
    pub name: String,
    pub run: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequiredChecks {
    pub blob_oid: String,
    pub commands: Vec<String>,
    pub steps: Vec<ChecksWorkflowStep>,
    pub auto_run: bool,
    pub branches: Option<Vec<String>>,
}

impl RequiredChecks {
    #[must_use]
    pub fn allows_auto_run_for_branch(&self, branch: &str) -> bool {
        if !self.auto_run {
            return false;
        }
        if !GitEngine::validate_outbound_branch(branch).is_ok_and(|clean| clean == branch) {
            return false;
        }
        match &self.branches {
            Some(allowed) => allowed.iter().any(|item| item == branch),
            None => true,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChecksConfig {
    version: u32,
    #[serde(default)]
    commands: Option<Vec<String>>,
    #[serde(default)]
    steps: Option<Vec<ChecksWorkflowStep>>,
    #[serde(default)]
    auto_run: bool,
    #[serde(default)]
    branches: Option<Vec<String>>,
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

    /// Resolves one canonical ordinary branch directly, without tag peeling or HEAD substitution.
    pub fn resolve_checks_ref(&self, full_ref: &str) -> Result<CommitSha, GitEngineError> {
        let invalid_ref = || {
            GitEngineError::InvalidRevision(
                "Checks source must name a canonical ordinary refs/heads/... branch".into(),
            )
        };
        let name = full_ref
            .strip_prefix("refs/heads/")
            .ok_or_else(invalid_ref)?;
        if name.is_empty()
            || name.len() > MAX_BRANCH_NAME_BYTES
            || name.eq_ignore_ascii_case("agent")
            || name
                .get(..6)
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case("agent/"))
            || <&gix::refs::FullNameRef>::try_from(full_ref).is_err()
            || Self::validate_branch_name(name).map_err(|_| invalid_ref())? != name
        {
            return Err(invalid_ref());
        }
        let repo = self.acquire_repo()?;
        crate::config_guard::reject_replacement_refs(&repo)?;
        let reference = repo
            .try_find_reference(full_ref)
            .map_err(|e| invalid(e.to_string()))?
            .ok_or_else(|| GitEngineError::RevisionNotFound(full_ref.into()))?;
        let target = reference
            .try_id()
            .ok_or_else(|| invalid("Checks source must be a direct branch target"))?
            .detach();
        let header = repo
            .objects
            .header(target)
            .map_err(|e| invalid(e.to_string()))?;
        if header.kind() != gix::objs::Kind::Commit {
            return Err(invalid(
                "Checks source must target an ordinary commit object",
            ));
        }
        Ok(CommitSha(target.to_string()))
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

fn valid_command(cmd: &str) -> bool {
    !cmd.trim().is_empty() && cmd.len() <= 4096 && !cmd.contains('\0')
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
    if config.version != 1 {
        return Err(invalid("checks require version 1"));
    }
    let (commands, steps) = match (config.commands, config.steps) {
        (Some(commands), None) => {
            if commands.is_empty()
                || commands.len() > 8
                || !commands.iter().all(|c| valid_command(c))
            {
                return Err(invalid(
                    "checks require version 1 and 1..=8 nonempty commands of at most 4096 bytes without NUL",
                ));
            }
            (commands, Vec::new())
        }
        (None, Some(steps)) => {
            if steps.is_empty()
                || steps.len() > 8
                || !steps.iter().all(|s| {
                    !s.name.trim().is_empty()
                        && s.name.len() <= 128
                        && !s.name.contains('\0')
                        && valid_command(&s.run)
                })
            {
                return Err(invalid(
                    "checks workflow steps require 1..=8 named steps with 1..=128 byte names and 1..=4096 byte commands without NUL",
                ));
            }
            let commands = steps.iter().map(|s| s.run.clone()).collect();
            (commands, steps)
        }
        _ => {
            return Err(invalid(
                "checks configuration must specify exactly one of commands or steps",
            ))
        }
    };
    if let Some(ref branches) = config.branches {
        if branches.is_empty()
            || branches.len() > 16
            || !branches
                .iter()
                .all(|b| GitEngine::validate_outbound_branch(b).is_ok_and(|clean| clean == *b))
            || (0..branches.len()).any(|i| branches[i + 1..].contains(&branches[i]))
        {
            return Err(invalid(
                "checks branches filter requires 1..=16 unique canonical ordinary branch names",
            ));
        }
    }
    Ok(Some(RequiredChecks {
        blob_oid: entry.oid().to_string(),
        commands,
        steps,
        auto_run: config.auto_run,
        branches: config.branches,
    }))
}

fn invalid(message: impl Into<String>) -> GitEngineError {
    GitEngineError::VerificationInvalid(message.into())
}
