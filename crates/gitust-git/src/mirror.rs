//! Step 2-4: Inbound Read-Only Mirror Synchronization (`gitust sync`) and Gate 7.4 Ref Verification.
//!
//! Synchronizes upstream branches (`refs/heads/*`) and tags (`refs/tags/*`) into the local Gitust
//! repository using batched fast-forward-only CAS reference transactions while preserving
//! local-only branches, on-premise `refs/heads/agent/*` branches, and checked-out worktrees.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs;
use std::path::Path;
use std::time::Instant;

use gitust_core::CommitSha;
use gix::refs::transaction::{Change, LogChange, PreviousValue, RefEdit, RefLog};
use gix::refs::{FullName, Target};
use serde::{Deserialize, Serialize};

use crate::ancestry::{
    classify_branch_relation, is_agent_branch_short, list_checked_out_branches, BranchRelation,
};
use crate::config_guard::{
    contains_http_userinfo, reject_credentials_in_config, run_git_fetch_with_timeout,
};
use crate::{
    ensure_reflog_identity, io_err, run_git, GitEngine, GitEngineError, INITIAL_FETCH_TIMEOUT,
};

const UPSTREAM_REMOTE_PREFIX: &str = "refs/remotes/upstream/";
const UPSTREAM_TAGS_PREFIX: &str = "refs/remotes/upstream-tags/";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboundMirrorSyncOutcome {
    pub github_remote_url: String,
    pub last_synced_commit: Option<CommitSha>,
    pub synced_branches_count: u32,
    pub synced_tags_count: u32,
    pub private_agent_branches_filtered: u32,
    #[serde(default)]
    pub skipped_branches: Vec<(String, String)>,
    #[serde(default)]
    pub skipped_tags: Vec<(String, String)>,
    #[serde(default)]
    pub local_only_branches: Vec<String>,
    pub sync_latency_ms: u64,
}

impl GitEngine {
    /// Validates and normalizes an upstream Git remote URL or local repository path.
    ///
    /// - Rejects empty strings, strings > 2048 bytes, leading `-` (flag injection), whitespace/control
    ///   characters, `::` remote helper syntax (`ext::`), and embedded HTTP credentials.
    /// - Allows `https://`, `ssh://`, `git@`, `file://`, or an existing local directory.
    pub fn validate_mirror_remote_url(raw: &str) -> Result<String, GitEngineError> {
        let trimmed = raw.trim();
        if trimmed.is_empty()
            || trimmed.len() > 2048
            || trimmed.starts_with('-')
            || trimmed.contains("::")
            || trimmed.chars().any(|c| c.is_control() || c.is_whitespace())
        {
            return Err(GitEngineError::InvalidRemoteUrl(raw.to_owned()));
        }

        let lower = trimmed.to_ascii_lowercase();
        if let Some(after_scheme) = lower.strip_prefix("https://") {
            if after_scheme.is_empty()
                || after_scheme.starts_with('/')
                || contains_http_userinfo(&lower)
            {
                return Err(GitEngineError::InvalidRemoteUrl(raw.to_owned()));
            }
            return Ok(trimmed.to_owned());
        }
        if let Some(after_scheme) = lower.strip_prefix("ssh://") {
            if after_scheme.is_empty() || after_scheme.starts_with('/') {
                return Err(GitEngineError::InvalidRemoteUrl(raw.to_owned()));
            }
            return Ok(trimmed.to_owned());
        }
        if let Some(after_git) = trimmed.strip_prefix("git@") {
            if !after_git.contains(':') || after_git.starts_with(':') {
                return Err(GitEngineError::InvalidRemoteUrl(raw.to_owned()));
            }
            return Ok(trimmed.to_owned());
        }
        if let Some(file_path) = trimmed.strip_prefix("file://") {
            let p = Path::new(file_path);
            let canon = fs::canonicalize(p)
                .map_err(|_| GitEngineError::InvalidRemoteUrl(raw.to_owned()))?;
            if !canon.is_dir() {
                return Err(GitEngineError::InvalidRemoteUrl(raw.to_owned()));
            }
            return Ok(canon.display().to_string());
        }

        let local_path = Path::new(trimmed);
        let canon = fs::canonicalize(local_path)
            .map_err(|_| GitEngineError::InvalidRemoteUrl(raw.to_owned()))?;
        if !canon.is_dir() {
            return Err(GitEngineError::InvalidRemoteUrl(raw.to_owned()));
        }
        Ok(canon.display().to_string())
    }

    /// Phase 1 of inbound mirror sync: validates `upstream_url`, initializes a bare repository only
    /// if `self.repo_path` does not exist or is an empty directory, and fetches upstream heads/tags
    /// into staging namespaces (`refs/remotes/upstream/*` and `refs/remotes/upstream-tags/*`)
    /// while excluding `^refs/heads/agent/*` at the protocol level and enforcing stall detection.
    pub fn fetch_inbound_mirror_staging(
        &self,
        upstream_url: &str,
    ) -> Result<String, GitEngineError> {
        let clean_url = Self::validate_mirror_remote_url(upstream_url)?;

        let should_init_bare = if !self.repo_path.exists() {
            fs::create_dir_all(&self.repo_path).map_err(|e| io_err(&self.repo_path, &e))?;
            true
        } else if self.repo_path.is_dir() {
            fs::read_dir(&self.repo_path)
                .map_err(|e| io_err(&self.repo_path, &e))?
                .next()
                .is_none()
        } else {
            false
        };

        if should_init_bare {
            run_git(
                &self.repo_path,
                [
                    OsStr::new("init"),
                    OsStr::new("--bare"),
                    OsStr::new("-q"),
                    OsStr::new("-b"),
                    OsStr::new("main"),
                ],
            )?;
        }

        let repo = self.acquire_repo()?;
        let common_git_dir = Self::common_git_dir(&repo);
        reject_credentials_in_config(&common_git_dir)?;

        let has_existing_upstream = repo.references().ok().is_some_and(|p| {
            p.prefixed(UPSTREAM_REMOTE_PREFIX)
                .is_ok_and(|mut it| it.next().is_some())
        });
        let effective_timeout = if self.fetch_timeout_overridden || has_existing_upstream {
            self.fetch_timeout
        } else {
            INITIAL_FETCH_TIMEOUT
        };

        run_git_fetch_with_timeout(&self.repo_path, &clean_url, effective_timeout)?;
        Ok(clean_url)
    }

    /// Phase 2 of inbound mirror sync: acquires the cross-process worktree service lock, classifies
    /// branch ancestry in-process via `gix`, and applies all fast-forward branch and new tag updates
    /// in a single batched `gix` CAS reference transaction.
    ///
    /// Non-fast-forward branches, branches checked out in linked worktrees, dirty main worktree
    /// branches, and moved tags are recorded in `skipped_branches` / `skipped_tags` without aborting
    /// synchronization of the remaining refs.
    pub fn apply_inbound_mirror_refs(
        &self,
        upstream_url: &str,
        start: Instant,
    ) -> Result<InboundMirrorSyncOutcome, GitEngineError> {
        let _service_lock = self.lock_worktree_service()?;

        let mut repo = gix::ThreadSafeRepository::open(&self.repo_path)
            .map_err(|e| GitEngineError::OpenRepo {
                path: self.repo_path.display().to_string(),
                message: e.to_string(),
            })?
            .to_thread_local();
        repo.object_cache_size_if_unset(4 * 1024 * 1024);
        ensure_reflog_identity(&mut repo)?;

        let (main_checked_out, linked_checked_out) =
            list_checked_out_branches(&self.repo_path, repo.work_dir().is_some())?;

        let mut upstream_branches: BTreeMap<String, gix::ObjectId> = BTreeMap::new();
        let mut upstream_tags: BTreeMap<String, gix::ObjectId> = BTreeMap::new();
        let mut filtered_upstream_agent_refs: Vec<String> = Vec::new();
        let mut local_non_agent_branches: BTreeMap<String, gix::ObjectId> = BTreeMap::new();
        let mut local_agent_branches_count: usize = 0;
        let mut local_tags: BTreeMap<String, gix::ObjectId> = BTreeMap::new();

        {
            let refs_platform = repo
                .references()
                .map_err(|e| GitEngineError::RefTransaction(e.to_string()))?;
            let all_refs = refs_platform
                .all()
                .map_err(|e| GitEngineError::RefTransaction(e.to_string()))?;

            for reference_res in all_refs {
                let Ok(mut r) = reference_res else {
                    continue;
                };
                let full_name = r.name().as_bstr().to_string();
                let Ok(oid) = r.follow_to_object().map(|id| id.detach()) else {
                    continue;
                };
                if let Some(short) = full_name.strip_prefix(UPSTREAM_REMOTE_PREFIX) {
                    if short == "HEAD" {
                        continue;
                    }
                    if is_agent_branch_short(short) {
                        filtered_upstream_agent_refs.push(full_name);
                        continue;
                    }
                    upstream_branches.insert(short.to_owned(), oid);
                } else if let Some(tag) = full_name.strip_prefix(UPSTREAM_TAGS_PREFIX) {
                    upstream_tags.insert(tag.to_owned(), oid);
                } else if let Some(short) = full_name.strip_prefix("refs/heads/") {
                    if is_agent_branch_short(short) {
                        local_agent_branches_count = local_agent_branches_count.saturating_add(1);
                    } else {
                        local_non_agent_branches.insert(short.to_owned(), oid);
                    }
                } else if let Some(tag) = full_name.strip_prefix("refs/tags/") {
                    local_tags.insert(tag.to_owned(), oid);
                }
            }
        }

        let local_only_branches: Vec<String> = local_non_agent_branches
            .keys()
            .filter(|b| !upstream_branches.contains_key(*b))
            .cloned()
            .collect();

        let private_agent_branches_filtered = u32::try_from(
            local_agent_branches_count.saturating_add(filtered_upstream_agent_refs.len()),
        )
        .unwrap_or(u32::MAX);
        let mut batched_edits: Vec<(bool, String, RefEdit)> = Vec::new();
        // Cleanup any legacy or case-variant `refs/remotes/upstream/agent/*` staging refs left on disk
        // (protocol-level `^refs/heads/agent/*` already excludes lowercase `agent/*` during `git fetch`).
        for staging_ref in &filtered_upstream_agent_refs {
            if let Ok(name) = FullName::try_from(staging_ref.as_str()) {
                let _ = repo.edit_reference(RefEdit {
                    change: Change::Delete {
                        expected: PreviousValue::Any,
                        log: RefLog::AndReference,
                    },
                    name,
                    deref: false,
                });
            }
        }

        let mut synced_branches_count: u32 = 0;
        let mut skipped_branches: Vec<(String, String)> = Vec::new();
        let mut skipped_tags: Vec<(String, String)> = Vec::new();

        for (branch, upstream_oid) in &upstream_branches {
            let full_head_ref = format!("refs/heads/{branch}");
            if let Some(local_oid) = local_non_agent_branches.get(branch).copied() {
                if local_oid == *upstream_oid {
                    continue;
                }
                match classify_branch_relation(&repo, local_oid, *upstream_oid) {
                    BranchRelation::FastForward => {}
                    BranchRelation::LocalAhead => {
                        skipped_branches.push((
                            branch.clone(),
                            "local branch is ahead of upstream".to_owned(),
                        ));
                        continue;
                    }
                    BranchRelation::Diverged => {
                        skipped_branches.push((
                            branch.clone(),
                            "non-fast-forward (diverged from upstream)".to_owned(),
                        ));
                        continue;
                    }
                    BranchRelation::Indeterminate(reason) => {
                        skipped_branches.push((branch.clone(), reason));
                        continue;
                    }
                }
                if linked_checked_out.contains(branch.as_str()) {
                    skipped_branches.push((
                        branch.clone(),
                        "checked out in a linked worktree".to_owned(),
                    ));
                    continue;
                }
                if main_checked_out.as_deref() == Some(branch.as_str()) {
                    if try_fast_forward_main_worktree(
                        &self.repo_path,
                        branch,
                        *upstream_oid,
                        &mut skipped_branches,
                    ) {
                        synced_branches_count = synced_branches_count.saturating_add(1);
                    }
                } else {
                    push_cas_edit(
                        &mut batched_edits,
                        true,
                        branch,
                        &full_head_ref,
                        PreviousValue::MustExistAndMatch(Target::Object(local_oid)),
                        *upstream_oid,
                        "gitust sync: fast-forward inbound mirror branch",
                    );
                }
            } else if main_checked_out.as_deref() == Some(branch.as_str())
                || linked_checked_out.contains(branch.as_str())
            {
                skipped_branches.push((
                    branch.clone(),
                    "unborn branch checked out in a working tree".to_owned(),
                ));
            } else {
                push_cas_edit(
                    &mut batched_edits,
                    true,
                    branch,
                    &full_head_ref,
                    PreviousValue::MustNotExist,
                    *upstream_oid,
                    "gitust sync: create inbound mirror branch",
                );
            }
        }

        let mut synced_tags_count: u32 = 0;
        for (tag, upstream_tag_oid) in &upstream_tags {
            if let Some(local_tag_oid) = local_tags.get(tag).copied() {
                if local_tag_oid != *upstream_tag_oid {
                    skipped_tags.push((
                        tag.clone(),
                        "tag already exists locally with a different target OID".to_owned(),
                    ));
                }
            } else {
                let full_tag_ref = format!("refs/tags/{tag}");
                push_cas_edit(
                    &mut batched_edits,
                    false,
                    tag,
                    &full_tag_ref,
                    PreviousValue::MustNotExist,
                    *upstream_tag_oid,
                    "gitust sync: create inbound mirror tag",
                );
            }
        }

        apply_batched_ref_edits(
            &repo,
            batched_edits,
            &mut synced_branches_count,
            &mut synced_tags_count,
            &mut skipped_branches,
            &mut skipped_tags,
        );

        let preferred_branch = if upstream_branches.contains_key("main") {
            Some("main")
        } else {
            upstream_branches.keys().next().map(String::as_str)
        };
        if let Some(target_branch) = preferred_branch {
            if repo.work_dir().is_none() {
                let head_valid = repo
                    .head()
                    .ok()
                    .and_then(|mut h| h.peel_to_commit_in_place().ok())
                    .is_some();
                if !head_valid {
                    let target_ref = format!("refs/heads/{target_branch}");
                    let _ = run_git(
                        &self.repo_path,
                        [
                            OsStr::new("symbolic-ref"),
                            OsStr::new("HEAD"),
                            OsStr::new(target_ref.as_str()),
                        ],
                    );
                }
            }
        }

        let last_synced_commit = preferred_branch
            .and_then(|b| upstream_branches.get(b))
            .map(|oid| CommitSha(oid.to_string()));

        let sync_latency_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);

        Ok(InboundMirrorSyncOutcome {
            github_remote_url: upstream_url.trim().to_owned(),
            last_synced_commit,
            synced_branches_count,
            synced_tags_count,
            private_agent_branches_filtered,
            skipped_branches,
            skipped_tags,
            local_only_branches,
            sync_latency_ms,
        })
    }

    /// Synchronizes an upstream GitHub (or local upstream) repository into this repository as a
    /// read-only inbound mirror while keeping all local-only and `refs/heads/agent/*` branches intact.
    pub fn sync_inbound_read_only(
        &self,
        upstream_url: &str,
    ) -> Result<InboundMirrorSyncOutcome, GitEngineError> {
        let start = Instant::now();
        let _clean_url = self.fetch_inbound_mirror_staging(upstream_url)?;
        self.apply_inbound_mirror_refs(upstream_url, start)
    }

    /// Lists all non-agent `refs/heads/*` and `refs/tags/*` mapped to their direct target object OID hex
    /// (preserving annotated `Tag` object OIDs rather than peeling to commits).
    /// Used by Verification Gate 7.4 to verify that a mirror matches its upstream repository 1:1.
    pub fn list_non_agent_refs(&self) -> Result<BTreeMap<String, String>, GitEngineError> {
        let repo = gix::ThreadSafeRepository::open(&self.repo_path)
            .map_err(|e| GitEngineError::OpenRepo {
                path: self.repo_path.display().to_string(),
                message: e.to_string(),
            })?
            .to_thread_local();
        let refs_platform = repo
            .references()
            .map_err(|e| GitEngineError::RefTransaction(e.to_string()))?;
        let all_refs = refs_platform
            .all()
            .map_err(|e| GitEngineError::RefTransaction(e.to_string()))?;

        let mut map = BTreeMap::new();
        for reference_res in all_refs {
            let Ok(mut r) = reference_res else {
                continue;
            };
            let full_name = r.name().as_bstr().to_string();
            if let Some(short) = full_name.strip_prefix("refs/heads/") {
                if is_agent_branch_short(short) {
                    continue;
                }
            } else if !full_name.starts_with("refs/tags/") {
                continue;
            }
            if let Ok(id) = r.follow_to_object() {
                map.insert(full_name, id.detach().to_string());
            }
        }
        Ok(map)
    }
}

fn try_fast_forward_main_worktree(
    repo_path: &Path,
    branch: &str,
    upstream_oid: gix::ObjectId,
    skipped_branches: &mut Vec<(String, String)>,
) -> bool {
    let Ok(status_out) = run_git(repo_path, ["status", "--porcelain", "--untracked-files=no"])
    else {
        skipped_branches.push((
            branch.to_owned(),
            "failed to inspect main working tree status".to_owned(),
        ));
        return false;
    };
    if !status_out.iter().all(|b| b.is_ascii_whitespace()) {
        skipped_branches.push((
            branch.to_owned(),
            "checked-out main working tree has uncommitted changes".to_owned(),
        ));
        return false;
    }
    let oid_hex = upstream_oid.to_string();
    match run_git(
        repo_path,
        [
            OsStr::new("merge"),
            OsStr::new("--ff-only"),
            OsStr::new("-q"),
            OsStr::new(oid_hex.as_str()),
        ],
    ) {
        Ok(_) => true,
        Err(e) => {
            skipped_branches.push((branch.to_owned(), e.to_string()));
            false
        }
    }
}

fn push_cas_edit(
    batched_edits: &mut Vec<(bool, String, RefEdit)>,
    is_branch: bool,
    short_name: &str,
    full_ref: &str,
    expected: PreviousValue,
    target_oid: gix::ObjectId,
    message: &str,
) {
    if let Ok(name) = FullName::try_from(full_ref) {
        batched_edits.push((
            is_branch,
            short_name.to_owned(),
            RefEdit {
                change: Change::Update {
                    log: LogChange {
                        mode: RefLog::AndReference,
                        force_create_reflog: false,
                        message: message.into(),
                    },
                    expected,
                    new: Target::Object(target_oid),
                },
                name,
                deref: false,
            },
        ));
    }
}

fn apply_batched_ref_edits(
    repo: &gix::Repository,
    batched_edits: Vec<(bool, String, RefEdit)>,
    synced_branches_count: &mut u32,
    synced_tags_count: &mut u32,
    skipped_branches: &mut Vec<(String, String)>,
    skipped_tags: &mut Vec<(String, String)>,
) {
    if batched_edits.is_empty() {
        return;
    }
    let only_edits: Vec<RefEdit> = batched_edits.iter().map(|(_, _, e)| e.clone()).collect();
    if repo.edit_references(only_edits).is_ok() {
        for (is_branch, _, _) in &batched_edits {
            if *is_branch {
                *synced_branches_count = synced_branches_count.saturating_add(1);
            } else {
                *synced_tags_count = synced_tags_count.saturating_add(1);
            }
        }
        return;
    }

    for (is_branch, short_name, edit) in batched_edits {
        match repo.edit_reference(edit) {
            Ok(_) => {
                if is_branch {
                    *synced_branches_count = synced_branches_count.saturating_add(1);
                } else {
                    *synced_tags_count = synced_tags_count.saturating_add(1);
                }
            }
            Err(e) => {
                let reason = format!("CAS reference update failed: {e}");
                if is_branch {
                    skipped_branches.push((short_name, reason));
                } else {
                    skipped_tags.push((short_name, reason));
                }
            }
        }
    }
}
