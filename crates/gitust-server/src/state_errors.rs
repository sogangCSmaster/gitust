//! HTTP status code mappings for `AuthError`, `GitEngineError`, `SandboxError`, and task join errors.

use axum::http::StatusCode;
use gitust_core::AuthError;
use gitust_git::GitEngineError;
use gitust_sandbox::SandboxError;

pub(crate) fn map_auth_err(err: AuthError) -> (StatusCode, String) {
    let code = match err {
        AuthError::Unauthenticated | AuthError::InvalidCredentials => StatusCode::UNAUTHORIZED,
        AuthError::AccountDeactivated
        | AuthError::AdminPrivilegeRequired
        | AuthError::ForbiddenResourceOwnership
        | AuthError::ForbiddenRepositoryAccess
        | AuthError::PublicSignupDisabled
        | AuthError::AdminAlreadyBootstrapped => StatusCode::FORBIDDEN,
        AuthError::RateLimitExceeded => StatusCode::TOO_MANY_REQUESTS,
        AuthError::DuplicateUser | AuthError::CannotDeactivateLastAdmin => StatusCode::CONFLICT,
        AuthError::InvalidToken
        | AuthError::TokenAlreadyUsed
        | AuthError::TokenExpired
        | AuthError::TokenRevoked
        | AuthError::TokenPurposeMismatch
        | AuthError::InvalidInput(_) => StatusCode::BAD_REQUEST,
        AuthError::StorageError(_) => StatusCode::SERVICE_UNAVAILABLE,
    };
    (code, err.to_string())
}

pub(crate) fn map_git_err(err: GitEngineError) -> (StatusCode, String) {
    let code = match &err {
        GitEngineError::DefaultBranchInvalid
        | GitEngineError::InvalidBranchName(_)
        | GitEngineError::InvalidRevision(_)
        | GitEngineError::InvalidRepoPath(_)
        | GitEngineError::InvalidRemoteUrl(_)
        | GitEngineError::BranchNotAgentScoped(_) => StatusCode::BAD_REQUEST,
        GitEngineError::BaseBranchNotFound(_)
        | GitEngineError::RevisionNotFound(_)
        | GitEngineError::PathNotFound(_) => StatusCode::NOT_FOUND,
        GitEngineError::DefaultBranchConflict
        | GitEngineError::DefaultBranchUnsupported
        | GitEngineError::BranchAlreadyExists(_)
        | GitEngineError::MergeConflict(_)
        | GitEngineError::VerificationInvalid(_)
        | GitEngineError::ExecutionFence(_)
        | GitEngineError::RepositoryIdentity(_)
        | GitEngineError::WorktreeServiceBusy(_) => StatusCode::CONFLICT,
        GitEngineError::CredentialsInRepoConfig => StatusCode::PRECONDITION_FAILED,
        _ => {
            tracing::error!(error = %err, "git engine failure");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal git engine error".to_owned(),
            );
        }
    };
    (code, err.to_string())
}

pub(crate) fn map_sandbox_err(err: SandboxError) -> (StatusCode, String) {
    match &err {
        SandboxError::TimedOut { .. } => (StatusCode::REQUEST_TIMEOUT, err.to_string()),
        SandboxError::IsolationUnavailable(_) => (StatusCode::SERVICE_UNAVAILABLE, err.to_string()),
        _ => {
            tracing::error!(error = %err, "sandbox failure");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal sandbox error".to_owned(),
            )
        }
    }
}

pub(crate) fn internal_err(e: impl std::fmt::Display) -> (StatusCode, String) {
    tracing::error!(error = %e, "internal task failure");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        "internal error".to_owned(),
    )
}
