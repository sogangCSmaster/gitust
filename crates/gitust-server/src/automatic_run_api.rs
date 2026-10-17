//! SiteAdmin diagnostics; lifecycle records confer no execution or approval capability.
use crate::{
    automatic_run_store::Failure,
    state::{check_repo_bola, find_authorized_worktree, internal_err, map_auth_err, AppState},
};
use axum::{
    extract::{Path, State},
    http::header,
    response::IntoResponse,
    Json,
};
use gitust_core::{AuthenticatedPrincipal, EnterpriseRole};
pub(crate) async fn list(
    State(state): State<AppState>,
    caller: AuthenticatedPrincipal,
    Path(id): Path<uuid::Uuid>,
) -> Result<impl IntoResponse, Failure> {
    if caller.role != EnterpriseRole::SiteAdmin {
        return Err(map_auth_err(gitust_core::AuthError::AdminPrivilegeRequired));
    }
    let wt = find_authorized_worktree(&state, &caller, id).await?;
    let store = state.automatic_checks.store()?;
    tokio::task::spawn_blocking(move || {
        let records = match store.document(wt.id, &wt.branch, &wt.base_branch)? {
            Some(doc) => doc.records(),
            None => Vec::new(),
        };
        let current = state
            .auth_store
            .authorize_automatic_checks_actor(caller.user_id)
            .map_err(map_auth_err)?;
        if current.role != EnterpriseRole::SiteAdmin {
            return Err(map_auth_err(gitust_core::AuthError::AdminPrivilegeRequired));
        }
        check_repo_bola(
            &state.auth_store,
            state.repo_id,
            state.repo_access,
            &current,
            wt.repo_id,
        )?;
        Ok(([(header::CACHE_CONTROL, "no-store")], Json(records)))
    })
    .await
    .map_err(internal_err)?
}
