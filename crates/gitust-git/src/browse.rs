//! Step 2-4: Blazing-fast (`< 30ms`) in-process `gix` Commit Log, Tree Browser & Blob Viewer.

use std::time::Instant;

use gitust_core::CommitSha;
use serde::{Deserialize, Serialize};

use crate::{GitEngine, GitEngineError};

/// Maximum blob size returned inline by `read_blob` (1 MiB).
pub const MAX_BLOB_VIEW_BYTES: usize = 1_048_576;
/// Maximum recursive tree entries returned in a single `inspect_tree` call.
pub const MAX_TREE_ENTRIES: usize = 5_000;
/// Maximum directory recursion depth in `collect_tree_entries`.
const MAX_TREE_DEPTH: usize = 64;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitEntry {
    pub sha: CommitSha,
    pub short_sha: String,
    pub summary: String,
    pub message: String,
    pub author_name: String,
    pub author_email: String,
    pub authored_timestamp: i64,
    pub parent_shas: Vec<CommitSha>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoCommitList {
    pub rev: String,
    pub head_sha: CommitSha,
    pub commits: Vec<CommitEntry>,
    pub lookup_micros: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoTreeNode {
    pub path: String,
    pub filename: String,
    pub oid: String,
    pub is_dir: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoTreeSnapshot {
    pub rev: String,
    pub commit_sha: CommitSha,
    pub base_path: String,
    pub entries: Vec<RepoTreeNode>,
    pub truncated: bool,
    pub lookup_micros: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoBlobSnapshot {
    pub rev: String,
    pub commit_sha: CommitSha,
    pub path: String,
    pub oid: String,
    pub size: usize,
    pub is_binary: bool,
    pub truncated: bool,
    pub line_count: usize,
    pub content: Option<String>,
    pub lookup_micros: u64,
}

impl GitEngine {
    /// Validates a revision specifier (`HEAD`, branch name, tag, or commit SHA) against flag
    /// injection (`-`), reflog syntax (`@{`), traversal (`..`), control/whitespace chars, and
    /// excessive `~N`/`^N` ancestry walk depth (`> 1_000` total steps).
    pub fn validate_revision_spec(rev: Option<&str>) -> Result<String, GitEngineError> {
        let trimmed = rev.map(str::trim).unwrap_or("");
        if trimmed.is_empty() {
            return Ok("HEAD".to_owned());
        }
        if trimmed.len() > 256
            || trimmed.starts_with('-')
            || trimmed.contains("..")
            || trimmed.contains("@{")
            || trimmed.ends_with(".lock")
            || trimmed.chars().any(|c| {
                c.is_control() || c.is_whitespace() || matches!(c, ':' | '?' | '*' | '[' | '\\')
            })
        {
            return Err(GitEngineError::InvalidRevision(trimmed.to_owned()));
        }
        let split_idx = trimmed.find(['~', '^']).unwrap_or(trimmed.len());
        let (anchor, mut suffix) = trimmed.split_at(split_idx);
        if anchor.is_empty() {
            return Err(GitEngineError::InvalidRevision(trimmed.to_owned()));
        }
        let mut total_steps = 0usize;
        while !suffix.is_empty() {
            let op = suffix.as_bytes()[0] as char;
            if !matches!(op, '~' | '^') {
                return Err(GitEngineError::InvalidRevision(trimmed.to_owned()));
            }
            suffix = &suffix[1..];
            let digits_len = suffix.bytes().take_while(u8::is_ascii_digit).count();
            let count = if digits_len == 0 {
                1usize
            } else {
                let num = suffix[..digits_len]
                    .parse::<usize>()
                    .map_err(|_| GitEngineError::InvalidRevision(trimmed.to_owned()))?;
                suffix = &suffix[digits_len..];
                num
            };
            total_steps = total_steps.saturating_add(count);
            if count > 1_000 || total_steps > 1_000 {
                return Err(GitEngineError::InvalidRevision(trimmed.to_owned()));
            }
        }
        Ok(trimmed.to_owned())
    }

    /// Validates and normalizes a repository-relative file or directory path.
    pub fn validate_repo_relative_path(path: &str) -> Result<String, GitEngineError> {
        let trimmed = path.trim().trim_matches('/');
        if trimmed.is_empty() {
            return Ok(String::new());
        }
        if trimmed.len() > 1024 || trimmed.contains('\\') || trimmed.chars().any(char::is_control) {
            return Err(GitEngineError::InvalidRepoPath(path.to_owned()));
        }
        let mut segments = Vec::new();
        for seg in trimmed.split('/') {
            if seg.is_empty() || seg == "." || seg == ".." || seg == ".git" {
                return Err(GitEngineError::InvalidRepoPath(path.to_owned()));
            }
            segments.push(seg);
        }
        Ok(segments.join("/"))
    }

    /// Resolves a revision specifier (`HEAD`, branch, tag, 40-hex OID, plus `~N`/`^N` ancestry
    /// suffixes) to a `gix::Commit` in-process without spawning any subprocess.
    pub(crate) fn resolve_commit<'a>(
        repo: &'a gix::Repository,
        rev: Option<&str>,
    ) -> Result<(String, gix::Commit<'a>), GitEngineError> {
        let clean_rev = Self::validate_revision_spec(rev)?;
        let split_idx = clean_rev.find(['~', '^']).unwrap_or(clean_rev.len());
        let (anchor, mut suffix) = clean_rev.split_at(split_idx);
        if anchor.is_empty() {
            return Err(GitEngineError::InvalidRevision(clean_rev));
        }

        let mut commit = if anchor == "HEAD" {
            let mut head = repo
                .head()
                .map_err(|e| GitEngineError::HeadResolution(e.to_string()))?;
            head.peel_to_commit_in_place()
                .map_err(|_| GitEngineError::RevisionNotFound(clean_rev.clone()))?
        } else if let Some(mut r) = [
            format!("refs/heads/{anchor}"),
            format!("refs/tags/{anchor}"),
            anchor.to_owned(),
        ]
        .into_iter()
        .find_map(|cand| repo.try_find_reference(&cand).ok().flatten())
        {
            r.peel_to_commit()
                .map_err(|_| GitEngineError::RevisionNotFound(clean_rev.clone()))?
        } else if (7..=40).contains(&anchor.len()) && anchor.bytes().all(|b| b.is_ascii_hexdigit())
        {
            let oid = if anchor.len() == 40 {
                gix::ObjectId::from_hex(anchor.as_bytes())
                    .map_err(|_| GitEngineError::RevisionNotFound(clean_rev.clone()))?
            } else {
                let prefix = gix::hash::Prefix::from_hex(anchor)
                    .map_err(|_| GitEngineError::RevisionNotFound(clean_rev.clone()))?;
                repo.objects
                    .lookup_prefix(prefix, None)
                    .ok()
                    .flatten()
                    .and_then(Result::ok)
                    .ok_or_else(|| GitEngineError::RevisionNotFound(clean_rev.clone()))?
            };
            repo.find_object(oid)
                .ok()
                .and_then(|o| o.peel_to_commit().ok())
                .ok_or_else(|| GitEngineError::RevisionNotFound(clean_rev.clone()))?
        } else {
            return Err(GitEngineError::RevisionNotFound(clean_rev));
        };

        let mut total_steps = 0usize;
        while !suffix.is_empty() {
            let op = suffix.as_bytes()[0] as char;
            suffix = &suffix[1..];
            let digits_len = suffix.bytes().take_while(u8::is_ascii_digit).count();
            let count = if digits_len == 0 {
                1usize
            } else {
                let num = suffix[..digits_len]
                    .parse::<usize>()
                    .map_err(|_| GitEngineError::InvalidRevision(clean_rev.clone()))?;
                suffix = &suffix[digits_len..];
                num
            };
            total_steps = total_steps.saturating_add(count);
            if count > 1_000 || total_steps > 1_000 {
                return Err(GitEngineError::InvalidRevision(clean_rev));
            }
            match op {
                '~' => {
                    for _ in 0..count {
                        let parent_id = commit
                            .parent_ids()
                            .next()
                            .ok_or_else(|| GitEngineError::RevisionNotFound(clean_rev.clone()))?;
                        commit = parent_id
                            .object()
                            .ok()
                            .and_then(|o| o.peel_to_commit().ok())
                            .ok_or_else(|| GitEngineError::RevisionNotFound(clean_rev.clone()))?;
                    }
                }
                '^' => {
                    if count > 0 {
                        let parent_id = commit
                            .parent_ids()
                            .nth(count - 1)
                            .ok_or_else(|| GitEngineError::RevisionNotFound(clean_rev.clone()))?;
                        commit = parent_id
                            .object()
                            .ok()
                            .and_then(|o| o.peel_to_commit().ok())
                            .ok_or_else(|| GitEngineError::RevisionNotFound(clean_rev.clone()))?;
                    }
                }
                _ => return Err(GitEngineError::InvalidRevision(clean_rev)),
            }
        }
        Ok((clean_rev, commit))
    }

    /// Lists commit history starting from `rev` (default `HEAD`) up to `limit` (clamped to `1..=200`)
    /// in-process via `gitoxide` (`gix`).
    pub fn list_commits(
        &self,
        rev: Option<&str>,
        limit: usize,
    ) -> Result<RepoCommitList, GitEngineError> {
        let start = Instant::now();
        let capped_limit = limit.clamp(1, 200);
        let repo = self.acquire_repo()?;
        let (clean_rev, head_commit) = Self::resolve_commit(&repo, rev)?;
        let head_id = head_commit.id;
        let head_sha = CommitSha(head_id.to_string());

        let walk = repo
            .rev_walk([head_id])
            .all()
            .map_err(|e| GitEngineError::HeadResolution(e.to_string()))?;

        let mut commits = Vec::with_capacity(capped_limit.min(64));
        for info_res in walk.take(capped_limit) {
            let info = info_res.map_err(|e| GitEngineError::HeadResolution(e.to_string()))?;
            let commit = info
                .object()
                .map_err(|e| GitEngineError::HeadResolution(e.to_string()))?;
            let full_sha = commit.id.to_string();
            let short_sha = full_sha.chars().take(7).collect();
            let raw_msg = commit
                .message_raw()
                .map(|m| String::from_utf8_lossy(m.as_ref()).trim().to_owned())
                .unwrap_or_default();
            let summary = raw_msg.lines().next().unwrap_or("").to_owned();
            let authored_timestamp = commit.time().map(|t| t.seconds).unwrap_or(0);
            let (author_name, author_email) = match commit.author() {
                Ok(sig) => (
                    String::from_utf8_lossy(sig.name.as_ref()).into_owned(),
                    String::from_utf8_lossy(sig.email.as_ref()).into_owned(),
                ),
                Err(_) => ("unknown".to_owned(), String::new()),
            };
            let parent_shas = commit
                .parent_ids()
                .map(|pid| CommitSha(pid.to_string()))
                .collect();

            commits.push(CommitEntry {
                sha: CommitSha(full_sha),
                short_sha,
                summary,
                message: raw_msg,
                author_name,
                author_email,
                authored_timestamp,
                parent_shas,
            });
        }

        let lookup_micros = u64::try_from(start.elapsed().as_micros()).unwrap_or(u64::MAX);
        Ok(RepoCommitList {
            rev: clean_rev,
            head_sha,
            commits,
            lookup_micros,
        })
    }

    /// Inspects a directory tree at `rev` and optional `dir_path` in-process via `gitoxide` (`gix`).
    pub fn inspect_tree(
        &self,
        rev: Option<&str>,
        dir_path: Option<&str>,
        recursive: bool,
    ) -> Result<RepoTreeSnapshot, GitEngineError> {
        let start = Instant::now();
        let clean_path = Self::validate_repo_relative_path(dir_path.unwrap_or(""))?;
        let repo = self.acquire_repo()?;
        let (clean_rev, commit) = Self::resolve_commit(&repo, rev)?;
        let commit_sha = CommitSha(commit.id.to_string());
        let root_tree = commit
            .tree()
            .map_err(|e| GitEngineError::TreeTraversal(e.to_string()))?;

        let target_tree = if clean_path.is_empty() {
            root_tree
        } else {
            let entry = root_tree
                .lookup_entry_by_path(&clean_path)
                .map_err(|e| GitEngineError::TreeTraversal(e.to_string()))?
                .ok_or_else(|| GitEngineError::PathNotFound(clean_path.clone()))?;
            if !entry.mode().is_tree() {
                return Err(GitEngineError::InvalidRepoPath(format!(
                    "'{clean_path}' is not a directory"
                )));
            }
            entry
                .object()
                .map_err(|e| GitEngineError::TreeTraversal(e.to_string()))?
                .try_into_tree()
                .map_err(|e| GitEngineError::TreeTraversal(e.to_string()))?
        };

        let mut entries = Vec::with_capacity(64);
        let mut truncated = false;
        collect_tree_entries(
            &repo,
            &target_tree,
            &clean_path,
            recursive,
            0,
            &mut entries,
            &mut truncated,
        )?;

        let lookup_micros = u64::try_from(start.elapsed().as_micros()).unwrap_or(u64::MAX);
        Ok(RepoTreeSnapshot {
            rev: clean_rev,
            commit_sha,
            base_path: clean_path,
            entries,
            truncated,
            lookup_micros,
        })
    }

    /// Reads a file blob at `rev:file_path` in-process via `gitoxide` (`gix`), enforcing a 1 MiB
    /// inline preview cap (`MAX_BLOB_VIEW_BYTES`) and automatic binary detection.
    pub fn read_blob(
        &self,
        rev: Option<&str>,
        file_path: &str,
    ) -> Result<RepoBlobSnapshot, GitEngineError> {
        let start = Instant::now();
        let clean_path = Self::validate_repo_relative_path(file_path)?;
        if clean_path.is_empty() {
            return Err(GitEngineError::InvalidRepoPath(
                "blob path must not be empty".to_owned(),
            ));
        }

        let repo = self.acquire_repo()?;
        let (clean_rev, commit) = Self::resolve_commit(&repo, rev)?;
        let commit_sha = CommitSha(commit.id.to_string());
        let root_tree = commit
            .tree()
            .map_err(|e| GitEngineError::TreeTraversal(e.to_string()))?;

        let entry = root_tree
            .lookup_entry_by_path(&clean_path)
            .map_err(|e| GitEngineError::TreeTraversal(e.to_string()))?
            .ok_or_else(|| GitEngineError::PathNotFound(clean_path.clone()))?;
        if entry.mode().is_tree() {
            return Err(GitEngineError::InvalidRepoPath(format!(
                "'{clean_path}' is a directory, not a blob"
            )));
        }
        if !entry.mode().is_blob_or_symlink() {
            return Err(GitEngineError::InvalidRepoPath(format!(
                "'{clean_path}' is not a regular file or symlink blob"
            )));
        }

        let entry_id = entry.oid();
        let oid = entry_id.to_string();
        use gix::prelude::HeaderExt;
        let header = repo
            .objects
            .header(entry_id)
            .map_err(|e| GitEngineError::TreeTraversal(e.to_string()))?;
        if header.kind() != gix::objs::Kind::Blob {
            return Err(GitEngineError::InvalidRepoPath(format!(
                "'{clean_path}' does not resolve to a blob object"
            )));
        }
        let header_size = usize::try_from(header.size()).unwrap_or(usize::MAX);
        if header_size > MAX_BLOB_VIEW_BYTES {
            let lookup_micros = u64::try_from(start.elapsed().as_micros()).unwrap_or(u64::MAX);
            return Ok(RepoBlobSnapshot {
                rev: clean_rev,
                commit_sha,
                path: clean_path,
                oid,
                size: header_size,
                is_binary: false,
                truncated: true,
                line_count: 0,
                content: None,
                lookup_micros,
            });
        }

        let obj = entry
            .object()
            .map_err(|e| GitEngineError::TreeTraversal(e.to_string()))?;
        let data = &obj.data;
        let size = data.len();
        let is_binary = data[..size.min(8192)].contains(&0);
        let truncated = size > MAX_BLOB_VIEW_BYTES;

        let (content, line_count) = if is_binary || truncated {
            (None, 0)
        } else {
            let text = String::from_utf8_lossy(data).into_owned();
            let lines = text.lines().count();
            (Some(text), lines)
        };

        let lookup_micros = u64::try_from(start.elapsed().as_micros()).unwrap_or(u64::MAX);
        Ok(RepoBlobSnapshot {
            rev: clean_rev,
            commit_sha,
            path: clean_path,
            oid,
            size,
            is_binary,
            truncated,
            line_count,
            content,
            lookup_micros,
        })
    }
}

fn collect_tree_entries(
    repo: &gix::Repository,
    tree: &gix::Tree<'_>,
    prefix: &str,
    recursive: bool,
    depth: usize,
    out: &mut Vec<RepoTreeNode>,
    truncated: &mut bool,
) -> Result<(), GitEngineError> {
    for entry_res in tree.iter() {
        if out.len() >= MAX_TREE_ENTRIES {
            *truncated = true;
            return Ok(());
        }
        let entry = entry_res.map_err(|e| GitEngineError::TreeTraversal(e.to_string()))?;
        let filename = entry.filename().to_string();
        let path = if prefix.is_empty() {
            filename.clone()
        } else {
            format!("{prefix}/{filename}")
        };
        let is_dir = entry.mode().is_tree();
        let oid = entry.oid().to_string();
        out.push(RepoTreeNode {
            path: path.clone(),
            filename,
            oid,
            is_dir,
        });
        if recursive && is_dir {
            if depth >= MAX_TREE_DEPTH {
                *truncated = true;
                continue;
            }
            let child_tree = repo
                .find_object(entry.oid())
                .map_err(|e| GitEngineError::TreeTraversal(e.to_string()))?
                .try_into_tree()
                .map_err(|e| GitEngineError::TreeTraversal(e.to_string()))?;
            collect_tree_entries(repo, &child_tree, &path, true, depth + 1, out, truncated)?;
        }
    }
    Ok(())
}
