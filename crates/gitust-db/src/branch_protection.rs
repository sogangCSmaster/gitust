use std::collections::HashMap;

use gitust_core::{
    AccountStatus, AuthError, AuthenticatedPrincipal, BranchProtectionRule, CommitSha,
    EnterpriseRole, PullRequestId, PullRequestProtectionStatus, PullRequestRecord,
    PullRequestStatus, RepoAccess, RepoId, ReviewVerdict, UserId,
};

use crate::models::AuthState;
use crate::pull_requests::{
    authorize_repo_locked, now_epoch_secs, validate_commit_sha, PullRequestLookup,
};
use crate::AuthStore;

const MAX_BRANCH_PROTECTION_RULES_PER_REPO: usize = 64;

fn validate_branch_pattern(raw: &str) -> Result<String, AuthError> {
    let trimmed = raw.trim();
    // fail-open-ok: normalizes an optional refs/heads/ prefix before pattern validation.
    let clean = trimmed.strip_prefix("refs/heads/").unwrap_or(trimmed);
    if clean.is_empty()
        || clean.len() > 256
        || clean.contains('\0')
        || clean.contains("..")
        || clean.contains("//")
        || clean.starts_with('/')
        || clean.ends_with('/')
        || clean.starts_with('-')
        || clean.eq_ignore_ascii_case("agent")
        || clean
            .get(..6)
            .is_some_and(|p| p.eq_ignore_ascii_case("agent/"))
        || clean.chars().any(|c| {
            c.is_control() || c.is_whitespace() || matches!(c, '~' | '^' | ':' | '?' | '[' | '\\')
        })
    {
        return Err(AuthError::InvalidInput(
            "invalid branch_pattern in branch protection rule".into(),
        ));
    }
    if let Some(idx) = clean.find('*') {
        if idx + 1 != clean.len() {
            return Err(AuthError::InvalidInput(
                "wildcard '*' in branch_pattern is only supported at the end of the pattern".into(),
            ));
        }
    }
    Ok(clean.to_owned())
}

fn find_effective_rule(
    state: &AuthState,
    repo_id: RepoId,
    branch_name: &str,
) -> Option<BranchProtectionRule> {
    let rules = state.branch_protection_rules.get(&repo_id)?;
    let clean = branch_name
        .trim()
        .strip_prefix("refs/heads/")
        // fail-open-ok: normalizes an optional refs/heads/ prefix before matching rules.
        .unwrap_or(branch_name.trim());
    if let Some(exact) = rules.iter().find(|r| r.branch_pattern == clean) {
        return Some(exact.clone());
    }
    rules
        .iter()
        .filter(|r| r.matches_branch(clean))
        .max_by_key(|r| r.branch_pattern.len())
        .cloned()
}

impl AuthStore {
    pub fn sync_pull_request_heads_batch(
        &self,
        caller: &AuthenticatedPrincipal,
        expected: RepoAccess,
        updates: &[(PullRequestId, CommitSha, CommitSha)],
    ) -> Result<Vec<PullRequestRecord>, AuthError> {
        for (_, base_sha, head_sha) in updates {
            validate_commit_sha(base_sha)?;
            validate_commit_sha(head_sha)?;
        }
        {
            let guard = self.read_state()?;
            authorize_repo_locked(&guard, caller, expected, false)?;
            let any_changed = updates.iter().any(|(pr_id, live_base, live_head)| {
                guard.pull_requests.iter().any(|pr| {
                    pr.repo_id == expected.repo_id
                        && pr.id == *pr_id
                        && matches!(
                            pr.status,
                            PullRequestStatus::Open | PullRequestStatus::Draft
                        )
                        && (pr.base_sha != *live_base || pr.head_sha != *live_head)
                })
            });
            if !any_changed {
                let mut pulls: Vec<PullRequestRecord> = guard
                    .pull_requests
                    .iter()
                    .filter(|pr| pr.repo_id == expected.repo_id)
                    .cloned()
                    .collect();
                pulls.sort_by(|a, b| b.number.cmp(&a.number));
                return Ok(pulls);
            }
        }
        let (pulls, snapshot) = {
            let mut guard = self.write_state()?;
            authorize_repo_locked(&guard, caller, expected, false)?;
            let now = now_epoch_secs();
            for (pr_id, live_base, live_head) in updates {
                if let Some(pr) = guard
                    .pull_requests
                    .iter_mut()
                    .find(|pr| pr.repo_id == expected.repo_id && pr.id == *pr_id)
                {
                    if matches!(
                        pr.status,
                        PullRequestStatus::Open | PullRequestStatus::Draft
                    ) && (pr.base_sha != *live_base || pr.head_sha != *live_head)
                    {
                        pr.base_sha = live_base.clone();
                        pr.head_sha = live_head.clone();
                        pr.updated_at_epoch_secs = now;
                    }
                }
            }
            let mut pulls: Vec<PullRequestRecord> = guard
                .pull_requests
                .iter()
                .filter(|pr| pr.repo_id == expected.repo_id)
                .cloned()
                .collect();
            pulls.sort_by(|a, b| b.number.cmp(&a.number));
            (pulls, self.maybe_snapshot(guard))
        };
        self.persist_snapshot(snapshot)?;
        Ok(pulls)
    }

    pub fn update_pull_request_status(
        &self,
        caller: &AuthenticatedPrincipal,
        expected: RepoAccess,
        lookup: PullRequestLookup,
        status: PullRequestStatus,
    ) -> Result<PullRequestRecord, AuthError> {
        if status == PullRequestStatus::Merged {
            return Err(AuthError::InvalidInput(
                "pull request cannot be marked merged without executing merge".into(),
            ));
        }
        let (updated, snapshot) = {
            let mut guard = self.write_state()?;
            let active = authorize_repo_locked(&guard, caller, expected, false)?;
            let existing = guard
                .pull_requests
                .iter()
                .find(|pr| pr.repo_id == expected.repo_id && lookup.matches(pr))
                .ok_or_else(|| AuthError::InvalidInput("pull request not found".into()))?;
            if existing.status == PullRequestStatus::Merged {
                return Err(AuthError::InvalidInput(
                    "merged pull request status cannot be changed".into(),
                ));
            }
            if active.role == EnterpriseRole::Developer && existing.author_user_id != active.user_id
            {
                return Err(AuthError::ForbiddenResourceOwnership);
            }
            let target_id = existing.id;
            let target_base = existing.base_branch.clone();
            let target_head = existing.head_branch.clone();
            if matches!(status, PullRequestStatus::Open | PullRequestStatus::Draft) {
                let duplicate_open = guard.pull_requests.iter().any(|other| {
                    other.repo_id == expected.repo_id
                        && other.id != target_id
                        && other.base_branch == target_base
                        && other.head_branch == target_head
                        && matches!(
                            other.status,
                            PullRequestStatus::Open | PullRequestStatus::Draft
                        )
                });
                if duplicate_open {
                    return Err(AuthError::InvalidInput(
                        "an open pull request already exists for this head and base branch".into(),
                    ));
                }
            }
            let pr = guard
                .pull_requests
                .iter_mut()
                .find(|pr| pr.repo_id == expected.repo_id && pr.id == target_id)
                .ok_or_else(|| AuthError::InvalidInput("pull request not found".into()))?;
            pr.status = status;
            pr.updated_at_epoch_secs = now_epoch_secs();
            let cloned = pr.clone();
            (cloned, self.maybe_snapshot(guard))
        };
        self.persist_snapshot(snapshot)?;
        Ok(updated)
    }

    pub fn record_pull_request_merged(
        &self,
        caller: &AuthenticatedPrincipal,
        expected: RepoAccess,
        lookup: PullRequestLookup,
        base_sha: CommitSha,
        head_sha: CommitSha,
        merged_sha: CommitSha,
    ) -> Result<PullRequestRecord, AuthError> {
        if expected.repo_id.0.is_nil() {
            return Err(AuthError::ForbiddenRepositoryAccess);
        }
        validate_commit_sha(&base_sha)?;
        validate_commit_sha(&head_sha)?;
        validate_commit_sha(&merged_sha)?;

        let (updated, snapshot) = {
            let mut guard = self.write_state()?;
            let merged_by_username = guard
                .users
                .get(&caller.user_id)
                .map(|u| u.username.clone())
                // fail-open-ok: caller authority was verified under pre_ref_update_gate before the Git ref updated.
                .unwrap_or_else(|| caller.username.clone());
            let pr = guard
                .pull_requests
                .iter_mut()
                .find(|pr| pr.repo_id == expected.repo_id && lookup.matches(pr))
                .ok_or_else(|| AuthError::InvalidInput("pull request not found".into()))?;
            pr.base_sha = base_sha;
            pr.head_sha = head_sha;
            pr.status = PullRequestStatus::Merged;
            pr.merged_sha = Some(merged_sha);
            pr.merged_by_username = Some(merged_by_username);
            pr.updated_at_epoch_secs = now_epoch_secs();
            let cloned = pr.clone();
            (cloned, self.maybe_snapshot(guard))
        };
        self.persist_snapshot(snapshot)?;
        Ok(updated)
    }

    pub fn list_branch_protection_rules(
        &self,
        caller: &AuthenticatedPrincipal,
        expected: RepoAccess,
    ) -> Result<Vec<BranchProtectionRule>, AuthError> {
        let guard = self.read_state()?;
        authorize_repo_locked(&guard, caller, expected, false)?;
        Ok(guard
            .branch_protection_rules
            .get(&expected.repo_id)
            .cloned()
            // fail-open-ok: repositories without configured branch protection rules have an empty list.
            .unwrap_or_default())
    }

    pub fn set_branch_protection_rules(
        &self,
        caller: &AuthenticatedPrincipal,
        expected: RepoAccess,
        rules: Vec<BranchProtectionRule>,
    ) -> Result<Vec<BranchProtectionRule>, AuthError> {
        if rules.len() > MAX_BRANCH_PROTECTION_RULES_PER_REPO {
            return Err(AuthError::InvalidInput(
                "branch protection rules limit exceeded".into(),
            ));
        }
        let mut normalized = Vec::with_capacity(rules.len());
        for mut rule in rules {
            rule.branch_pattern = validate_branch_pattern(&rule.branch_pattern)?;
            if rule.required_approvals > 10 {
                return Err(AuthError::InvalidInput(
                    "required_approvals must be between 0 and 10".into(),
                ));
            }
            if normalized
                .iter()
                .any(|r: &BranchProtectionRule| r.branch_pattern == rule.branch_pattern)
            {
                return Err(AuthError::InvalidInput(
                    "duplicate branch_pattern in branch protection rules".into(),
                ));
            }
            normalized.push(rule);
        }

        let (saved, snapshot) = {
            let mut guard = self.write_state()?;
            authorize_repo_locked(&guard, caller, expected, true)?;
            guard
                .branch_protection_rules
                .insert(expected.repo_id, normalized.clone());
            (normalized, self.maybe_snapshot(guard))
        };
        self.persist_snapshot(snapshot)?;
        Ok(saved)
    }

    pub fn upsert_branch_protection_rule(
        &self,
        caller: &AuthenticatedPrincipal,
        expected: RepoAccess,
        mut rule: BranchProtectionRule,
    ) -> Result<Vec<BranchProtectionRule>, AuthError> {
        rule.branch_pattern = validate_branch_pattern(&rule.branch_pattern)?;
        if rule.required_approvals > 10 {
            return Err(AuthError::InvalidInput(
                "required_approvals must be between 0 and 10".into(),
            ));
        }

        let (saved, snapshot) = {
            let mut guard = self.write_state()?;
            authorize_repo_locked(&guard, caller, expected, true)?;
            let list = guard
                .branch_protection_rules
                .entry(expected.repo_id)
                .or_default();
            if let Some(existing) = list
                .iter_mut()
                .find(|r| r.branch_pattern == rule.branch_pattern)
            {
                *existing = rule;
            } else {
                if list.len() >= MAX_BRANCH_PROTECTION_RULES_PER_REPO {
                    return Err(AuthError::InvalidInput(
                        "branch protection rules limit exceeded".into(),
                    ));
                }
                list.push(rule);
            }
            let out = list.clone();
            (out, self.maybe_snapshot(guard))
        };
        self.persist_snapshot(snapshot)?;
        Ok(saved)
    }

    pub fn effective_branch_protection(
        &self,
        repo_id: RepoId,
        branch_name: &str,
    ) -> Result<Option<BranchProtectionRule>, AuthError> {
        let guard = self.read_state()?;
        Ok(find_effective_rule(&guard, repo_id, branch_name))
    }

    pub fn evaluate_pull_request_protection(
        &self,
        repo_id: RepoId,
        pr: &PullRequestRecord,
        has_checks_policy: bool,
        checks_passed: bool,
    ) -> Result<PullRequestProtectionStatus, AuthError> {
        let guard = self.read_state()?;
        let rule = find_effective_rule(&guard, repo_id, &pr.base_branch);
        let repo_row = guard.repositories.get(&repo_id);

        let mut latest_verdict_by_user: HashMap<
            UserId,
            (EnterpriseRole, ReviewVerdict, CommitSha),
        > = HashMap::new();
        for review in &pr.reviews {
            let role = if guard.users.is_empty() {
                EnterpriseRole::SiteAdmin
            } else {
                let Some(user) = guard.users.get(&review.author_user_id) else {
                    continue;
                };
                if user.status != AccountStatus::Active {
                    continue;
                }
                if let Some(row) = repo_row {
                    if row.metadata.is_private
                        && user.role != EnterpriseRole::SiteAdmin
                        && !user.team_ids.contains(&row.metadata.owner_team_id)
                    {
                        continue;
                    }
                }
                user.role
            };
            latest_verdict_by_user.insert(
                review.author_user_id,
                (role, review.verdict, review.reviewed_head_sha.clone()),
            );
        }

        let mut approved_count: u8 = 0;
        let mut changes_requested = false;
        for (role, verdict, reviewed_sha) in latest_verdict_by_user.values() {
            match verdict {
                ReviewVerdict::RequestChanges => {
                    changes_requested = true;
                }
                ReviewVerdict::Approve
                    if *role != EnterpriseRole::Developer && *reviewed_sha == pr.head_sha =>
                {
                    approved_count = approved_count.saturating_add(1);
                }
                _ => {}
            }
        }

        let protected = rule.is_some();
        let require_pull_request = rule.as_ref().is_some_and(|r| r.require_pull_request);
        let required_approvals = rule.as_ref().map_or(0, |r| r.required_approvals);
        let require_passing_checks =
            rule.as_ref().is_some_and(|r| r.require_passing_checks) || has_checks_policy;
        let effective_checks_passed = if require_passing_checks {
            checks_passed
        } else {
            true
        };

        Ok(PullRequestProtectionStatus {
            protected,
            require_pull_request,
            required_approvals,
            approved_count,
            changes_requested,
            require_passing_checks,
            checks_passed: effective_checks_passed,
        })
    }
}
