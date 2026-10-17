//! `gitust admin` CLI subcommands (`create`, `invite`, `reset-password`, `deactivate`).

use std::path::PathBuf;

use anyhow::Result;
use clap::Subcommand;
use gitust_core::{AuthenticatedPrincipal, EnterpriseRole, UserPublicProfile};
use gitust_db::AuthStore;

#[derive(Debug, Subcommand)]
pub enum AdminCommands {
    /// Create the initial Site Administrator account (CLI-only bootstrap; web bootstrap is strictly disabled)
    Create {
        /// Initial Site Administrator username
        #[arg(long)]
        username: String,

        /// Initial Site Administrator corporate email (must match allowed domain, default @company.internal)
        #[arg(long)]
        email: String,

        /// Initial Site Administrator password (min 10 characters, hashed with Argon2id; supports GITUST_ADMIN_PASSWORD env)
        #[arg(long, env = "GITUST_ADMIN_PASSWORD", hide_env_values = true)]
        password: String,

        /// Path to the persistent on-premise Auth Database file
        #[arg(long, default_value = ".gitust/auth-db.json")]
        auth_db: PathBuf,
    },

    /// Issue a single-use 72-hour invite link (only SHA-256 stored in DB; share via messenger)
    Invite {
        /// SiteAdmin username or email authenticating this CLI action
        #[arg(long)]
        admin_username: String,

        /// SiteAdmin password (supports GITUST_ADMIN_PASSWORD env)
        #[arg(long, env = "GITUST_ADMIN_PASSWORD", hide_env_values = true)]
        admin_password: String,

        /// Target invitee corporate email (must match allowed domain)
        #[arg(long)]
        email: String,

        /// Assigned role (`Developer`, `Maintainer`, or `SiteAdmin`)
        #[arg(long, default_value = "Developer")]
        role: String,

        /// Expiration in hours (1..=720, default 72)
        #[arg(long)]
        ttl_hours: Option<i64>,

        /// Opaque group UUID membership; repeat up to 32 times (there is no named-team registry)
        #[arg(long = "team-id")]
        team_ids: Vec<gitust_core::TeamId>,

        /// Path to the persistent on-premise Auth Database file
        #[arg(long, default_value = ".gitust/auth-db.json")]
        auth_db: PathBuf,
    },

    /// Issue a single-use password-reset link for an existing active user
    ResetPassword {
        /// SiteAdmin username or email authenticating this CLI action
        #[arg(long)]
        admin_username: String,

        /// SiteAdmin password (supports GITUST_ADMIN_PASSWORD env)
        #[arg(long, env = "GITUST_ADMIN_PASSWORD", hide_env_values = true)]
        admin_password: String,

        /// Target user's username or email
        #[arg(long)]
        target: String,

        /// Expiration in hours (1..=72, default 72)
        #[arg(long)]
        ttl_hours: Option<i64>,

        /// Path to the persistent on-premise Auth Database file
        #[arg(long, default_value = ".gitust/auth-db.json")]
        auth_db: PathBuf,
    },

    /// Deactivate a user account and immediately revoke all sessions, PATs, SSH keys, and pending invites
    Deactivate {
        /// SiteAdmin username or email authenticating this CLI action
        #[arg(long)]
        admin_username: String,

        /// SiteAdmin password (supports GITUST_ADMIN_PASSWORD env)
        #[arg(long, env = "GITUST_ADMIN_PASSWORD", hide_env_values = true)]
        admin_password: String,

        /// Target user's username or email
        #[arg(long)]
        target: String,

        /// Path to the persistent on-premise Auth Database file
        #[arg(long, default_value = ".gitust/auth-db.json")]
        auth_db: PathBuf,
    },
}

fn parse_enterprise_role(raw: &str) -> Result<EnterpriseRole> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "siteadmin" | "site-admin" | "admin" => Ok(EnterpriseRole::SiteAdmin),
        "maintainer" => Ok(EnterpriseRole::Maintainer),
        "developer" | "dev" => Ok(EnterpriseRole::Developer),
        other => {
            anyhow::bail!("unknown role '{other}' (expected Developer, Maintainer, or SiteAdmin)")
        }
    }
}

pub fn handle_admin_command(action: AdminCommands) -> Result<()> {
    match action {
        AdminCommands::Create {
            username,
            email,
            password,
            auth_db,
        } => {
            let store = AuthStore::open_persistent(&auth_db)?;
            let admin = store.bootstrap_initial_admin_cli(&username, &email, &password)?;
            println!(
                "✓ Bootstrapped initial SiteAdmin '{}' ({}) [id: {}] in {}",
                admin.username,
                admin.email,
                admin.id.0,
                auth_db.display()
            );
        }
        AdminCommands::Invite {
            admin_username,
            admin_password,
            email,
            role,
            ttl_hours,
            team_ids,
            auth_db,
        } => {
            let assigned_role = parse_enterprise_role(&role)?;
            let store = AuthStore::open_persistent(&auth_db)?;
            with_admin_session(&store, &admin_username, &admin_password, |caller| {
                let issued =
                    store.create_invite_link(caller, &email, assigned_role, team_ids, ttl_hours)?;
                println!(
                    "✓ Issued single-use invite link for {} ({:?}, expires {}):\n  URL path: {}\n  Token:    {}",
                    issued.invite.target_email,
                    issued.invite.assigned_role,
                    issued.invite.expires_at,
                    issued.invite_url_path,
                    issued.raw_token
                );
                Ok(())
            })?;
        }
        AdminCommands::ResetPassword {
            admin_username,
            admin_password,
            target,
            ttl_hours,
            auth_db,
        } => {
            let store = AuthStore::open_persistent(&auth_db)?;
            with_admin_session(&store, &admin_username, &admin_password, |caller| {
                let found = find_user(&store, caller, &target)?;
                let issued = store.create_password_reset_link(caller, found.id, ttl_hours)?;
                println!(
                    "✓ Issued single-use password-reset link for '{}' ({}, expires {}):\n  URL path: {}\n  Token:    {}",
                    found.username,
                    found.email,
                    issued.invite.expires_at,
                    issued.invite_url_path,
                    issued.raw_token
                );
                Ok(())
            })?;
        }
        AdminCommands::Deactivate {
            admin_username,
            admin_password,
            target,
            auth_db,
        } => {
            let store = AuthStore::open_persistent(&auth_db)?;
            with_admin_session(&store, &admin_username, &admin_password, |caller| {
                let found = find_user(&store, caller, &target)?;
                let deactivated = store.deactivate_user(caller, found.id)?;
                println!(
                    "✓ Deactivated user '{}' ({}) [id: {}] and revoked all sessions, PATs, SSH keys, and pending invites",
                    deactivated.username, deactivated.email, deactivated.id.0
                );
                Ok(())
            })?;
        }
    }
    Ok(())
}

/// Runs `op` with a short-lived admin session and ALWAYS revokes it afterwards.
/// A failed revocation is surfaced as an error (never silently ignored), so the operator learns
/// that a persisted admin session may still be live until its expiry.
pub(crate) fn with_admin_session<T>(
    store: &AuthStore,
    admin_username: &str,
    admin_password: &str,
    op: impl FnOnce(&AuthenticatedPrincipal) -> Result<T>,
) -> Result<T> {
    with_admin_session_token(store, admin_username, admin_password, |caller, _| {
        op(caller)
    })
}

/// The originating credential is borrowed in memory only, so slow operator workflows can recheck it.
pub(crate) fn with_admin_session_token<T>(
    store: &AuthStore,
    admin_username: &str,
    admin_password: &str,
    op: impl FnOnce(&AuthenticatedPrincipal, &str) -> Result<T>,
) -> Result<T> {
    let session = store.login(admin_username, admin_password)?;
    let fallback_caller = AuthenticatedPrincipal {
        user_id: session.user.id,
        username: session.user.username,
        role: session.user.role,
        team_ids: session.user.team_ids,
    };
    let result = store
        .authenticate_session(&session.raw_session_token)
        .map_err(anyhow::Error::from)
        .and_then(|caller| op(&caller, &session.raw_session_token));
    let revoked = store.revoke_session(&fallback_caller, &session.raw_session_token);
    match (result, revoked) {
        (Ok(value), Ok(())) => Ok(value),
        (Ok(_), Err(e)) => Err(anyhow::anyhow!(
            "operation completed, but revoking the temporary admin session failed: {e} \
             (it stays valid until {})",
            session.expires_at
        )),
        (Err(e), Ok(())) => Err(e),
        (Err(e), Err(revoke_err)) => Err(e.context(format!(
            "additionally, revoking the temporary admin session failed: {revoke_err}"
        ))),
    }
}

fn find_user(
    store: &AuthStore,
    caller: &AuthenticatedPrincipal,
    target: &str,
) -> Result<UserPublicProfile> {
    let target_norm = target.trim().to_ascii_lowercase();
    store
        .list_users(caller)?
        .into_iter()
        .find(|u| {
            u.username.eq_ignore_ascii_case(&target_norm)
                || u.email.eq_ignore_ascii_case(&target_norm)
        })
        .ok_or_else(|| anyhow::anyhow!("user '{target}' not found"))
}
