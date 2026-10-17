use crate::admin_cmd::with_admin_session;
use anyhow::{bail, Result};
use clap::{ArgAction, Subcommand};
use gitust_core::{EnterpriseRole, TeamId};
use gitust_db::AuthStore;
use gitust_git::{GitEngine, RegistrationReceipt};
use std::path::PathBuf;

#[derive(Debug, Subcommand)]
pub enum RepositoryCommands {
    /// Offline explicit ACL registration; stop the service first. No Git refs or upstreams change.
    Register {
        #[arg(long)]
        repo: PathBuf,
        #[arg(long)]
        auth_db: PathBuf,
        #[arg(long)]
        admin_username: String,
        #[arg(long, env = "GITUST_ADMIN_PASSWORD", hide_env_values = true)]
        admin_password: String,
        #[arg(long)]
        name: String,
        /// Opaque nonnil group UUID, matching memberships assigned by administrator invitations
        #[arg(long)]
        owner_team_id: TeamId,
        /// Required: false permits all active authenticated users, never anonymous access
        #[arg(long, action = ArgAction::Set, required = true)]
        private: bool,
    },
}

pub fn handle(action: RepositoryCommands) -> Result<()> {
    let RepositoryCommands::Register {
        repo,
        auth_db,
        admin_username,
        admin_password,
        name,
        owner_team_id,
        private,
    } = action;
    if owner_team_id.0.is_nil() {
        bail!("owner team must be a nonnil group UUID");
    }
    let engine = GitEngine::new(repo);
    // The strong service holder must outlive DB writes, receipt publication and session revocation.
    let ownership = engine.lock_worktree_service()?;
    let store = AuthStore::open_persistent(auth_db)?;
    let result = with_admin_session(&store, &admin_username, &admin_password, |caller| {
        if caller.role != EnterpriseRole::SiteAdmin {
            bail!("repository registration requires an active SiteAdmin");
        }
        let id = engine.load_or_create_repository_id()?;
        let receipt = match engine.repository_registration_state(id)? {
            RegistrationReceipt::Absent => None,
            RegistrationReceipt::Pending(receipt) | RegistrationReceipt::Complete(receipt) => {
                Some(receipt)
            }
        };
        let registered =
            store.register_local_repository(caller, id, &name, owner_team_id, private, receipt)?;
        engine.complete_repository_registration(registered.binding)?;
        Ok(registered.metadata)
    });
    drop(ownership);
    let registered = result?;
    println!(
        "Registered local repository '{}' [{}]; restart the configured service.",
        registered.name, registered.id.0
    );
    Ok(())
}
