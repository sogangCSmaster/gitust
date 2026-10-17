//! Offline empty bare repository creation uses the same destination admission as import.
use crate::repository_provision::{self, Kind, ProvisionArgs};
use anyhow::Result;
use clap::{ArgAction, Args};
use gitust_core::{RepoId, TeamId};
use std::path::PathBuf;

#[derive(Debug, Args)]
pub(crate) struct CreateArgs {
    /// New destination with an existing parent; never adopts an unrelated existing directory.
    #[arg(long)]
    pub repo: PathBuf,
    /// Existing private AuthStore, outside the destination and provisioning journal.
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
    /// Explicit non-agent symbolic default branch; no commit is created.
    #[arg(long)]
    pub default_branch: String,
    /// First stop ALL earlier provisioning processes; this does not repair corrupt metadata.
    #[arg(long)]
    pub acknowledge_stopped_create_processes: bool,
}
impl CreateArgs {
    pub(crate) fn provision(&self) -> ProvisionArgs<'_> {
        ProvisionArgs {
            repo: &self.repo,
            auth_db: &self.auth_db,
            admin_username: &self.admin_username,
            admin_password: &self.admin_password,
            name: &self.name,
            owner_team_id: self.owner_team_id,
            private: self.private,
            default_branch: &self.default_branch,
            acknowledge_stopped_processes: self.acknowledge_stopped_create_processes,
            kind: Kind::Create,
        }
    }
}
pub(crate) fn run(args: &CreateArgs) -> Result<RepoId> {
    repository_provision::run(&args.provision())
}
