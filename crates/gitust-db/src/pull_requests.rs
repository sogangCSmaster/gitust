use chrono::Utc;
use gitust_core::{
    AuthError, AuthenticatedPrincipal, CommitSha, EnterpriseRole, PullRequestComment,
    PullRequestCommentId, PullRequestId, PullRequestRecord, PullRequestReview, PullRequestReviewId,
    PullRequestStatus, RepoAccess, RepoId, ReviewVerdict,
};

use crate::models::AuthState;
use crate::AuthStore;

const MAX_PR_TITLE_BYTES: usize = 256;
const MAX_PR_TEXT_BYTES: usize = 65_536;
const MAX_COMMENT_FILE_PATH_BYTES: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PullRequestLookup {
    Id(PullRequestId),
    Number(u64),
}

impl PullRequestLookup {
    #[must_use]
    pub fn parse_str(raw: &str) -> Option<Self> {
        let trimmed = raw.trim();
        if let Some(id) = PullRequestId::parse_str(trimmed) {
            return Some(Self::Id(id));
        }
        // fail-open-ok: returns None on non-numeric / non-UUID strings so callers return 400/404.
        trimmed
            .parse::<u64>()
            .ok()
            .filter(|&n| n >= 1)
            .map(Self::Number)
    }

    #[must_use]
    pub fn matches(self, record: &PullRequestRecord) -> bool {
        match self {
            Self::Id(id) => record.id == id,
            Self::Number(num) => record.number == num,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CreatePullRequestInput {
    pub repo_id: RepoId,
    pub title: String,
    pub description: String,
    pub base_branch: String,
    pub head_branch: String,
    pub base_sha: CommitSha,
    pub head_sha: CommitSha,
    pub status: PullRequestStatus,
}

pub(crate) fn now_epoch_secs() -> u64 {
    // fail-open-ok: clamps pre-1970 clock skew to 0 for non-authoritative display timestamps.
    u64::try_from(Utc::now().timestamp()).unwrap_or(0)
}

pub(crate) fn validate_commit_sha(sha: &CommitSha) -> Result<(), AuthError> {
    let raw = sha.0.trim();
    let valid_len = raw.len() == 40 || raw.len() == 64;
    let valid_hex = raw
        .bytes()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    let non_zero = raw.bytes().any(|b| b != b'0');
    if !valid_len || !valid_hex || !non_zero {
        return Err(AuthError::InvalidInput(
            "invalid commit SHA: expected lowercase 40/64-char hex digest".into(),
        ));
    }
    Ok(())
}

fn validate_comment_file_path(raw: &str) -> Result<String, AuthError> {
    let trimmed = raw.trim();
    if trimmed.is_empty()
        || trimmed.len() > MAX_COMMENT_FILE_PATH_BYTES
        || trimmed.contains('\0')
        || trimmed.contains('\\')
        || trimmed.starts_with('/')
        || trimmed.ends_with('/')
    {
        return Err(AuthError::InvalidInput(
            "invalid file_path: must be a safe repo-relative path".into(),
        ));
    }
    for segment in trimmed.split('/') {
        if segment.is_empty()
            || segment == "."
            || segment == ".."
            || segment.chars().any(char::is_control)
        {
            return Err(AuthError::InvalidInput(
                "invalid file_path: path traversal or empty segments are forbidden".into(),
            ));
        }
    }
    Ok(trimmed.to_owned())
}

pub(crate) fn authorize_repo_locked(
    state: &AuthState,
    caller: &AuthenticatedPrincipal,
    expected: RepoAccess,
    require_ordinary_push: bool,
) -> Result<AuthenticatedPrincipal, AuthError> {
    if expected.repo_id.0.is_nil() {
        return Err(AuthError::ForbiddenRepositoryAccess);
    }
    let active = if state.users.is_empty() && expected.registration.is_none() {
        caller.clone()
    } else {
        AuthStore::verify_active_user_locked(state, caller)?
    };
    let row = state.repositories.get(&expected.repo_id);
    match (expected.registration, row) {
        (None, None) => {}
        (Some(binding), Some(row))
            if binding.valid_for(expected.repo_id) && row.binding == Some(binding) =>
        {
            if row.metadata.is_private
                && active.role != EnterpriseRole::SiteAdmin
                && !active.team_ids.contains(&row.metadata.owner_team_id)
            {
                return Err(AuthError::ForbiddenRepositoryAccess);
            }
        }
        _ => return Err(AuthError::ForbiddenRepositoryAccess),
    }
    if require_ordinary_push && active.role == EnterpriseRole::Developer {
        return Err(AuthError::AdminPrivilegeRequired);
    }
    Ok(active)
}

impl AuthStore {
    pub fn list_pull_requests(
        &self,
        caller: &AuthenticatedPrincipal,
        expected: RepoAccess,
    ) -> Result<Vec<PullRequestRecord>, AuthError> {
        let guard = self.read_state()?;
        authorize_repo_locked(&guard, caller, expected, false)?;
        let mut pulls: Vec<PullRequestRecord> = guard
            .pull_requests
            .iter()
            .filter(|pr| pr.repo_id == expected.repo_id)
            .cloned()
            .collect();
        pulls.sort_by(|a, b| b.number.cmp(&a.number));
        Ok(pulls)
    }

    pub fn get_pull_request(
        &self,
        caller: &AuthenticatedPrincipal,
        expected: RepoAccess,
        lookup: PullRequestLookup,
    ) -> Result<Option<PullRequestRecord>, AuthError> {
        let guard = self.read_state()?;
        authorize_repo_locked(&guard, caller, expected, false)?;
        Ok(guard
            .pull_requests
            .iter()
            .find(|pr| pr.repo_id == expected.repo_id && lookup.matches(pr))
            .cloned())
    }

    pub fn sync_pull_request_heads(
        &self,
        caller: &AuthenticatedPrincipal,
        expected: RepoAccess,
        lookup: PullRequestLookup,
        live_base_sha: CommitSha,
        live_head_sha: CommitSha,
    ) -> Result<Option<PullRequestRecord>, AuthError> {
        validate_commit_sha(&live_base_sha)?;
        validate_commit_sha(&live_head_sha)?;
        {
            let guard = self.read_state()?;
            authorize_repo_locked(&guard, caller, expected, false)?;
            let Some(existing) = guard
                .pull_requests
                .iter()
                .find(|pr| pr.repo_id == expected.repo_id && lookup.matches(pr))
            else {
                return Ok(None);
            };
            if !matches!(
                existing.status,
                PullRequestStatus::Open | PullRequestStatus::Draft
            ) || (existing.base_sha == live_base_sha && existing.head_sha == live_head_sha)
            {
                return Ok(Some(existing.clone()));
            }
        }
        let (updated, snapshot) = {
            let mut guard = self.write_state()?;
            authorize_repo_locked(&guard, caller, expected, false)?;
            let Some(pr) = guard
                .pull_requests
                .iter_mut()
                .find(|pr| pr.repo_id == expected.repo_id && lookup.matches(pr))
            else {
                return Ok(None);
            };
            if matches!(
                pr.status,
                PullRequestStatus::Open | PullRequestStatus::Draft
            ) && (pr.base_sha != live_base_sha || pr.head_sha != live_head_sha)
            {
                pr.base_sha = live_base_sha;
                pr.head_sha = live_head_sha;
                pr.updated_at_epoch_secs = now_epoch_secs();
            }
            let cloned = pr.clone();
            guard.dirty_pr_ids.insert(cloned.id);
            (cloned, self.maybe_snapshot(guard))
        };
        self.persist_snapshot(snapshot)?;
        Ok(Some(updated))
    }

    pub fn create_pull_request(
        &self,
        caller: &AuthenticatedPrincipal,
        expected: RepoAccess,
        input: CreatePullRequestInput,
    ) -> Result<PullRequestRecord, AuthError> {
        if input.repo_id != expected.repo_id {
            return Err(AuthError::ForbiddenRepositoryAccess);
        }
        let title = input.title.trim().to_owned();
        if title.is_empty() || title.len() > MAX_PR_TITLE_BYTES || title.contains('\0') {
            return Err(AuthError::InvalidInput(
                "pull request title must be 1..=256 bytes without NUL".into(),
            ));
        }
        let description = input.description.trim().to_owned();
        if description.len() > MAX_PR_TEXT_BYTES || description.contains('\0') {
            return Err(AuthError::InvalidInput(
                "pull request description exceeds 65536 bytes or contains NUL".into(),
            ));
        }
        if !matches!(
            input.status,
            PullRequestStatus::Open | PullRequestStatus::Draft
        ) {
            return Err(AuthError::InvalidInput(
                "new pull request status must be open or draft".into(),
            ));
        }
        validate_commit_sha(&input.base_sha)?;
        validate_commit_sha(&input.head_sha)?;
        if input.base_branch == input.head_branch {
            return Err(AuthError::InvalidInput(
                "base_branch and head_branch must be distinct".into(),
            ));
        }

        let (record, snapshot) = {
            let mut guard = self.write_state()?;
            let active = authorize_repo_locked(&guard, caller, expected, false)?;
            let duplicate_open = guard.pull_requests.iter().any(|pr| {
                pr.repo_id == expected.repo_id
                    && pr.base_branch == input.base_branch
                    && pr.head_branch == input.head_branch
                    && matches!(
                        pr.status,
                        PullRequestStatus::Open | PullRequestStatus::Draft
                    )
            });
            if duplicate_open {
                return Err(AuthError::InvalidInput(
                    "an open or draft pull request already exists for this base and head branch"
                        .into(),
                ));
            }
            let next_number = guard
                .pull_requests
                .iter()
                .filter(|pr| pr.repo_id == expected.repo_id)
                .map(|pr| pr.number)
                // fail-open-ok: first pull request in a repository starts at number 1.
                .max()
                .unwrap_or(0)
                .saturating_add(1);
            let now = now_epoch_secs();
            let record = PullRequestRecord {
                id: PullRequestId::new(),
                number: next_number,
                repo_id: expected.repo_id,
                title,
                description,
                author_user_id: active.user_id,
                author_username: active.username,
                base_branch: input.base_branch,
                head_branch: input.head_branch,
                base_sha: input.base_sha,
                head_sha: input.head_sha,
                status: input.status,
                merged_sha: None,
                merged_by_username: None,
                reviews: Vec::new(),
                comments: Vec::new(),
                created_at_epoch_secs: now,
                updated_at_epoch_secs: now,
            };
            guard.pull_requests.push(record.clone());
            (record, self.maybe_snapshot(guard))
        };
        self.persist_snapshot(snapshot)?;
        Ok(record)
    }

    pub fn add_pull_request_review(
        &self,
        caller: &AuthenticatedPrincipal,
        expected: RepoAccess,
        lookup: PullRequestLookup,
        verdict: ReviewVerdict,
        body: String,
        reviewed_head_sha: CommitSha,
    ) -> Result<PullRequestRecord, AuthError> {
        validate_commit_sha(&reviewed_head_sha)?;
        let clean_body = body.trim().to_owned();
        if clean_body.len() > MAX_PR_TEXT_BYTES || clean_body.contains('\0') {
            return Err(AuthError::InvalidInput(
                "review body exceeds 65536 bytes or contains NUL".into(),
            ));
        }
        if matches!(
            verdict,
            ReviewVerdict::RequestChanges | ReviewVerdict::Comment
        ) && clean_body.is_empty()
        {
            return Err(AuthError::InvalidInput(
                "review body is required when requesting changes or commenting".into(),
            ));
        }
        let require_maintainer = verdict == ReviewVerdict::Approve;

        let (updated, snapshot) = {
            let mut guard = self.write_state()?;
            let active = authorize_repo_locked(&guard, caller, expected, require_maintainer)?;
            let pr = guard
                .pull_requests
                .iter_mut()
                .find(|pr| pr.repo_id == expected.repo_id && lookup.matches(pr))
                .ok_or_else(|| AuthError::InvalidInput("pull request not found".into()))?;
            if !matches!(
                pr.status,
                PullRequestStatus::Open | PullRequestStatus::Draft
            ) {
                return Err(AuthError::InvalidInput(
                    "cannot review a closed or merged pull request".into(),
                ));
            }
            let now = now_epoch_secs();
            pr.head_sha = reviewed_head_sha.clone();
            pr.reviews.push(PullRequestReview {
                id: PullRequestReviewId::new(),
                author_user_id: active.user_id,
                author_username: active.username,
                verdict,
                body: clean_body,
                reviewed_head_sha,
                created_at_epoch_secs: now,
            });
            pr.updated_at_epoch_secs = now;
            let cloned = pr.clone();
            guard.dirty_pr_ids.insert(cloned.id);
            (cloned, self.maybe_snapshot(guard))
        };
        self.persist_snapshot(snapshot)?;
        Ok(updated)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn add_pull_request_comment(
        &self,
        caller: &AuthenticatedPrincipal,
        expected: RepoAccess,
        lookup: PullRequestLookup,
        body: String,
        file_path: Option<String>,
        line_number: Option<u32>,
        commit_sha: Option<CommitSha>,
    ) -> Result<PullRequestRecord, AuthError> {
        let clean_body = body.trim().to_owned();
        if clean_body.is_empty()
            || clean_body.len() > MAX_PR_TEXT_BYTES
            || clean_body.contains('\0')
        {
            return Err(AuthError::InvalidInput(
                "comment body must be 1..=65536 bytes without NUL".into(),
            ));
        }
        let clean_path = match file_path {
            Some(ref raw) => Some(validate_comment_file_path(raw)?),
            None => None,
        };
        if let Some(line) = line_number {
            if line == 0 || clean_path.is_none() {
                return Err(AuthError::InvalidInput(
                    "line_number requires a valid file_path and must be >= 1".into(),
                ));
            }
        }
        if let Some(ref sha) = commit_sha {
            validate_commit_sha(sha)?;
        }

        let (updated, snapshot) = {
            let mut guard = self.write_state()?;
            let active = authorize_repo_locked(&guard, caller, expected, false)?;
            let pr = guard
                .pull_requests
                .iter_mut()
                .find(|pr| pr.repo_id == expected.repo_id && lookup.matches(pr))
                .ok_or_else(|| AuthError::InvalidInput("pull request not found".into()))?;
            let now = now_epoch_secs();
            pr.comments.push(PullRequestComment {
                id: PullRequestCommentId::new(),
                author_user_id: active.user_id,
                author_username: active.username,
                body: clean_body,
                file_path: clean_path,
                line_number,
                commit_sha,
                created_at_epoch_secs: now,
            });
            pr.updated_at_epoch_secs = now;
            let cloned = pr.clone();
            guard.dirty_pr_ids.insert(cloned.id);
            (cloned, self.maybe_snapshot(guard))
        };
        self.persist_snapshot(snapshot)?;
        Ok(updated)
    }
}
