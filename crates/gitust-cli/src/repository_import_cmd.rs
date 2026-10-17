//! Offline import preserves the import-v1 journal and request contract.
use crate::repository_import_storage::error;
#[cfg(test)]
use crate::repository_provision::ProvisionTransaction;
use crate::repository_provision::{self, Kind, ProvisionArgs};
use anyhow::{bail, Result};
use clap::{ArgAction, Args};
use gitust_core::{RepoId, TeamId};
use gitust_git::GitEngine;
use std::{fs, path::PathBuf};
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

impl ImportArgs {
    fn provision(&self) -> Result<ProvisionArgs<'_>> {
        Ok(ProvisionArgs {
            repo: &self.repo,
            auth_db: &self.auth_db,
            admin_username: &self.admin_username,
            admin_password: &self.admin_password,
            name: &self.name,
            owner_team_id: self.owner_team_id,
            private: self.private,
            default_branch: &self.default_branch,
            acknowledge_stopped_processes: self.acknowledge_stopped_import_processes,
            kind: Kind::Import(canonical_source(&self.source)?),
        })
    }
}
#[cfg(test)]
pub(crate) struct ImportTransaction(ProvisionTransaction);
#[cfg(test)]
impl std::ops::Deref for ImportTransaction {
    type Target = ProvisionTransaction;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
#[cfg(test)]
impl ImportTransaction {
    pub(crate) fn open(args: &ImportArgs) -> Result<Self> {
        ProvisionTransaction::open(&args.provision()?).map(Self)
    }
    pub(crate) fn prepare(&self, args: &ImportArgs) -> Result<GitEngine> {
        self.0.prepare(&args.provision()?)
    }
}
#[cfg(test)]
pub(crate) use repository_provision::recheck_admin;
pub(crate) fn run(args: &ImportArgs) -> Result<RepoId> {
    repository_provision::run(&args.provision()?)
}
#[cfg(test)]
use {gitust_db::AuthStore, std::path::Path};
#[cfg(test)]
#[path = "tests_import_journal.rs"]
mod journal_tests;
