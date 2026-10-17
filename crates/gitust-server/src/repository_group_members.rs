//! SiteAdmin management of one actual configured repository's owning-group membership.
use crate::{
    auth::{bearer_token, extract_principal_async},
    repository_reads::id,
    state::{map_auth_err, AppState},
};
use axum::{
    extract::{Path, Query, Request, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use gitust_core::{AuthError, EnterpriseRole, UserId};
use gitust_db::{GroupMember, GroupMemberError};
use serde::Deserialize;
type Failure = (StatusCode, &'static str);
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Lookup {
    username: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Change {
    expected_membership_version: String,
    is_member: bool,
}
enum Operation {
    Lookup(String),
    Change(UserId, u64, bool),
}
fn version(raw: &str) -> Result<u64, Failure> {
    let invalid = || (StatusCode::BAD_REQUEST, "invalid_membership_request");
    if raw.is_empty()
        || raw.len() > 20
        || !raw.bytes().all(|c| c.is_ascii_digit())
        || raw.len() > 1 && raw.starts_with('0')
    {
        return Err(invalid());
    }
    raw.parse().map_err(|_| invalid())
}
fn auth(error: AuthError) -> Failure {
    let (status, _) = map_auth_err(error);
    (
        status,
        match status {
            StatusCode::UNAUTHORIZED => "unauthenticated",
            StatusCode::FORBIDDEN => "forbidden",
            StatusCode::BAD_REQUEST => "invalid_membership_request",
            _ => "repository_unavailable",
        },
    )
}
fn failure(error: GroupMemberError) -> Failure {
    match error {
        GroupMemberError::Auth(e) => auth(e),
        GroupMemberError::NotFound => (StatusCode::NOT_FOUND, "group_member_not_found"),
        GroupMemberError::Conflict => (StatusCode::CONFLICT, "membership_conflict"),
        GroupMemberError::Limit => (StatusCode::CONFLICT, "membership_limit"),
        GroupMemberError::VersionExhausted => {
            (StatusCode::CONFLICT, "membership_version_exhausted")
        }
        GroupMemberError::Inactive => (StatusCode::FORBIDDEN, "group_member_inactive"),
        GroupMemberError::NotPublished(_) => {
            (StatusCode::SERVICE_UNAVAILABLE, "membership_not_published")
        }
        GroupMemberError::Unconfirmed(_) => {
            (StatusCode::SERVICE_UNAVAILABLE, "membership_unconfirmed")
        }
    }
}
fn response(value: Result<GroupMember, Failure>) -> Response {
    let mut result = match value {
        Ok(member) => Json(serde_json::json!({
            "repository_id": member.repository_id,
            "owner_team_id": member.owner_team_id,
            "user_id": member.user_id,
            "username": member.username,
            "role": member.role,
            "status": member.status,
            "is_member": member.is_member,
            "membership_version": member.membership_version.to_string(),
        }))
        .into_response(),
        Err((status, code)) => (status, Json(serde_json::json!({"code": code}))).into_response(),
    };
    result.headers_mut().insert(
        header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    result
}
pub(crate) async fn lookup(
    State(state): State<AppState>,
    Path(raw): Path<String>,
    request: Request,
) -> Response {
    response(value(state, raw, None, request).await)
}
pub(crate) async fn change(
    State(state): State<AppState>,
    Path((raw, user)): Path<(String, String)>,
    request: Request,
) -> Response {
    response(value(state, raw, Some(user), request).await)
}
async fn value(
    state: AppState,
    raw: String,
    user: Option<String>,
    request: Request,
) -> Result<GroupMember, Failure> {
    let repository = id(&raw).map_err(|_| (StatusCode::BAD_REQUEST, "invalid_repository_id"))?;
    let (parts, body) = request.into_parts();
    let origin = extract_principal_async(&parts.headers, &state.auth_store)
        .await
        .map_err(|(status, _)| {
            (
                status,
                if status.is_server_error() {
                    "repository_unavailable"
                } else if status == StatusCode::FORBIDDEN {
                    "forbidden"
                } else {
                    "unauthenticated"
                },
            )
        })?;
    if origin.role != EnterpriseRole::SiteAdmin {
        return Err((StatusCode::FORBIDDEN, "forbidden"));
    }
    let operation = if let Some(user) = user {
        let id = uuid::Uuid::parse_str(&user)
            .map_err(|_| (StatusCode::BAD_REQUEST, "invalid_membership_request"))?;
        if id.is_nil() {
            return Err((StatusCode::BAD_REQUEST, "invalid_membership_request"));
        }
        let bytes = axum::body::to_bytes(body, 4096).await.map_err(|_| {
            (
                StatusCode::PAYLOAD_TOO_LARGE,
                "membership_request_too_large",
            )
        })?;
        let request: Change = serde_json::from_slice(&bytes)
            .map_err(|_| (StatusCode::BAD_REQUEST, "invalid_membership_request"))?;
        Operation::Change(
            UserId(id),
            version(&request.expected_membership_version)?,
            request.is_member,
        )
    } else {
        let Query(request) = Query::<Lookup>::try_from_uri(&parts.uri)
            .map_err(|_| (StatusCode::BAD_REQUEST, "invalid_membership_request"))?;
        Operation::Lookup(request.username)
    };
    tokio::task::spawn_blocking(move || {
        let _inventory = state.repository_registry.clone();
        let repository = state
            .read_repository(repository)
            .ok_or((StatusCode::NOT_FOUND, "repository_not_hosted"))?;
        if repository.access.registration.is_none() {
            return Err((StatusCode::FORBIDDEN, "unsupported_repository"));
        }
        let _ownership = repository
            .engine
            .lock_worktree_service()
            .map_err(|_| (StatusCode::SERVICE_UNAVAILABLE, "repository_unavailable"))?;
        #[cfg(test)]
        crate::repository_reads::after_read(&state);
        let credential = bearer_token(&parts.headers)
            .map_err(|_| (StatusCode::UNAUTHORIZED, "unauthenticated"))?;
        match operation {
            Operation::Lookup(username) => state.auth_store.lookup_registered_group_member(
                credential,
                origin.user_id,
                repository.access,
                &username,
            ),
            Operation::Change(user, expected, member) => {
                state.auth_store.change_registered_group_member(
                    credential,
                    origin.user_id,
                    repository.access,
                    user,
                    expected,
                    member,
                )
            }
        }
        .map_err(failure)
    })
    .await
    .map_err(|_| (StatusCode::SERVICE_UNAVAILABLE, "repository_unavailable"))?
}
