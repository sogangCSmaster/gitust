//! Pure-Rust / `git merge-tree --write-tree` mergeability preview and gated branch merge.

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};

use gitust_core::{CommitSha, PullRequestMergePreview};
use gix::prelude::HeaderExt;
use gix::refs::{transaction::PreviousValue, Target};

use crate::process::{capture, git_command, run_git, terminate_group};
use crate::{ensure_reflog_identity, GitEngine, GitEngineError};

impl GitEngine {
    fn resolve_branch_commit(
        repo: &gix::Repository,
        raw_branch: &str,
        require_ordinary: bool,
    ) -> Result<(String, gix::ObjectId, CommitSha), GitEngineError> {
        let clean = if require_ordinary {
            Self::validate_outbound_branch(raw_branch)?
        } else {
            Self::validate_branch_name(raw_branch)?
        };
        crate::config_guard::reject_replacement_refs(repo)?;
        let full_ref = format!("refs/heads/{clean}");
        let reference = repo
            .try_find_reference(full_ref.as_str())
            .map_err(|e| GitEngineError::HeadResolution(e.to_string()))?
            .ok_or_else(|| GitEngineError::BaseBranchNotFound(clean.clone()))?;
        let target = reference
            .try_id()
            .ok_or_else(|| {
                GitEngineError::RefTransaction(format!(
                    "branch '{clean}' must be a direct reference, not a symbolic reference"
                ))
            })?
            .detach();
        let header = repo
            .objects
            .header(target)
            .map_err(|e| GitEngineError::HeadResolution(e.to_string()))?;
        if header.kind() != gix::objs::Kind::Commit {
            return Err(GitEngineError::HeadResolution(format!(
                "branch '{clean}' does not point to a commit object"
            )));
        }
        let sha = CommitSha(target.to_string());
        Ok((clean, target, sha))
    }

    /// Resolves `(base_sha, head_sha)` for `base_branch` and `head_branch` in-process via `gix`
    /// without spawning any `git` subprocesses.
    pub fn resolve_branch_pair_shas(
        &self,
        base_branch: &str,
        head_branch: &str,
    ) -> Result<(CommitSha, CommitSha), GitEngineError> {
        let _service_lock = self.lock_worktree_service()?;
        self.ensure_config_has_no_credentials()?;
        let repo = self.acquire_repo()?;
        let (_, _, base_sha) = Self::resolve_branch_commit(&repo, base_branch, true)?;
        let (_, _, head_sha) = Self::resolve_branch_commit(&repo, head_branch, false)?;
        Ok((base_sha, head_sha))
    }

    /// Computes a live mergeability preview between `base_branch` and `head_branch`
    /// without modifying any checkout or index.
    pub fn preview_branch_merge(
        &self,
        base_branch: &str,
        head_branch: &str,
    ) -> Result<PullRequestMergePreview, GitEngineError> {
        let _service_lock = self.lock_worktree_service()?;
        self.ensure_config_has_no_credentials()?;
        let repo = self.acquire_repo()?;
        let (clean_base, base_oid, base_sha) =
            Self::resolve_branch_commit(&repo, base_branch, true)?;
        let (clean_head, head_oid, head_sha) =
            Self::resolve_branch_commit(&repo, head_branch, false)?;

        if clean_base == clean_head
            || base_oid == head_oid
            || is_ancestor(&self.repo_path, &head_sha.0, &base_sha.0)?
        {
            return Ok(PullRequestMergePreview {
                can_merge: false,
                fast_forward: false,
                conflicting_files: Vec::new(),
                base_sha,
                head_sha,
            });
        }

        let fast_forward = is_ancestor(&self.repo_path, &base_sha.0, &head_sha.0)?;
        let (_, conflicting_files) = run_merge_tree(&self.repo_path, &base_sha.0, &head_sha.0)?;

        Ok(PullRequestMergePreview {
            can_merge: conflicting_files.is_empty(),
            fast_forward,
            conflicting_files,
            base_sha,
            head_sha,
        })
    }

    /// Merges `head_branch` into ordinary `base_branch` using `git merge-tree --write-tree`
    /// and `git commit-tree`, invoking `pre_ref_update_gate` under the actual Git ref lock
    /// immediately before committing `refs/heads/<base_branch>`.
    #[allow(clippy::too_many_arguments)]
    pub fn merge_branches_with_gate<F>(
        &self,
        base_branch: &str,
        head_branch: &str,
        expected_base_sha: &CommitSha,
        expected_head_sha: &CommitSha,
        commit_message: &str,
        author_name: &str,
        author_email: &str,
        pre_ref_update_gate: F,
    ) -> Result<CommitSha, GitEngineError>
    where
        F: FnOnce() -> Result<(), GitEngineError>,
    {
        let _service_lock = self.lock_worktree_service()?;
        self.ensure_config_has_no_credentials()?;
        let mut repo = self.acquire_repo()?;
        ensure_reflog_identity(&mut repo)?;

        let (clean_base, base_oid, live_base_sha) =
            Self::resolve_branch_commit(&repo, base_branch, true)?;
        let (clean_head, head_oid, live_head_sha) =
            Self::resolve_branch_commit(&repo, head_branch, false)?;

        if clean_base == clean_head {
            return Err(GitEngineError::MergeConflict(
                "base and head branches must be distinct".into(),
            ));
        }
        if live_base_sha != *expected_base_sha || live_head_sha != *expected_head_sha {
            return Err(GitEngineError::DefaultBranchConflict);
        }
        if base_oid == head_oid || is_ancestor(&self.repo_path, &live_head_sha.0, &live_base_sha.0)?
        {
            return Err(GitEngineError::MergeConflict(format!(
                "head branch '{clean_head}' has no commits ahead of '{clean_base}' to merge"
            )));
        }

        let (tree_oid, conflicting_files) =
            run_merge_tree(&self.repo_path, &live_base_sha.0, &live_head_sha.0)?;
        if !conflicting_files.is_empty() {
            return Err(GitEngineError::MergeConflict(format!(
                "merge conflict between '{clean_base}' and '{clean_head}': {}",
                conflicting_files.join(", ")
            )));
        }

        let merge_oid = create_merge_commit(
            &self.repo_path,
            &tree_oid,
            &live_base_sha.0,
            &live_head_sha.0,
            commit_message,
            author_name,
            author_email,
        )?;
        let merge_hex = merge_oid.to_string();

        let (main_checked_out, linked_checked_out) =
            crate::ancestry::list_checked_out_branches(&self.repo_path, repo.work_dir().is_some())?;
        if linked_checked_out.contains(clean_base.as_str()) {
            return Err(GitEngineError::MergeConflict(format!(
                "base branch '{clean_base}' is currently checked out in a linked worktree; cannot merge"
            )));
        }
        let head_on_base = main_checked_out.as_deref() == Some(clean_base.as_str());

        if head_on_base {
            crate::verification::inspect_index_flags(&self.repo_path)?;
            let status_out = run_git(
                &self.repo_path,
                [
                    OsStr::new("status"),
                    OsStr::new("--porcelain"),
                    OsStr::new("--untracked-files=no"),
                ],
            )?;
            if !status_out.iter().all(u8::is_ascii_whitespace) {
                return Err(GitEngineError::MergeConflict(format!(
                    "main working tree has uncommitted changes on '{clean_base}'; cannot merge"
                )));
            }
            run_git(
                &self.repo_path,
                [
                    OsStr::new("read-tree"),
                    OsStr::new("--dry-run"),
                    OsStr::new("-u"),
                    OsStr::new("-m"),
                    OsStr::new(live_base_sha.0.as_str()),
                    OsStr::new(merge_hex.as_str()),
                ],
            )
            .map_err(|e| {
                GitEngineError::MergeConflict(format!(
                    "working tree on '{clean_base}' would be overwritten by merge: {e}"
                ))
            })?;
        }

        let full_base = format!("refs/heads/{clean_base}");
        crate::ref_transaction::update_with_gate(
            &repo,
            full_base.as_str(),
            merge_oid,
            PreviousValue::MustExistAndMatch(Target::Object(base_oid)),
            format!("gitust: merge {clean_head} into {clean_base}"),
            || {
                let (_, current_head_oid, _) =
                    Self::resolve_branch_commit(&repo, &clean_head, false)?;
                if current_head_oid != head_oid {
                    return Err(GitEngineError::DefaultBranchConflict);
                }
                pre_ref_update_gate()
            },
        )
        .map_err(|error| match error {
            GitEngineError::RefTransaction(detail) => GitEngineError::MergeConflict(detail),
            other => other,
        })?;

        if head_on_base {
            run_git(
                &self.repo_path,
                [
                    OsStr::new("read-tree"),
                    OsStr::new("-u"),
                    OsStr::new("-m"),
                    OsStr::new(live_base_sha.0.as_str()),
                    OsStr::new(merge_hex.as_str()),
                ],
            )?;
        }

        Ok(CommitSha(merge_hex))
    }
}

fn is_ancestor(
    repo_dir: &Path,
    ancestor_sha: &str,
    descendant_sha: &str,
) -> Result<bool, GitEngineError> {
    let (code, _, stderr) = run_git_status(
        repo_dir,
        &["merge-base", "--is-ancestor", ancestor_sha, descendant_sha],
        &[],
        None,
    )?;
    match code {
        0 => Ok(true),
        1 => Ok(false),
        _ => Err(GitEngineError::GitCommand {
            args: "merge-base --is-ancestor".into(),
            message: String::from_utf8_lossy(&stderr).trim().to_owned(),
        }),
    }
}

fn run_merge_tree(
    repo_dir: &Path,
    base_sha: &str,
    head_sha: &str,
) -> Result<(String, Vec<String>), GitEngineError> {
    let (code, stdout, stderr) = run_git_status(
        repo_dir,
        &[
            "-c",
            "core.quotepath=false",
            "merge-tree",
            "--write-tree",
            "--name-only",
            "--no-messages",
            base_sha,
            head_sha,
        ],
        &[],
        None,
    )?;
    if code != 0 && code != 1 {
        return Err(GitEngineError::GitCommand {
            args: "merge-tree --write-tree".into(),
            message: String::from_utf8_lossy(&stderr).trim().to_owned(),
        });
    }
    let text = String::from_utf8(stdout).map_err(|e| GitEngineError::GitCommand {
        args: "merge-tree --write-tree".into(),
        message: e.to_string(),
    })?;
    let mut lines = text.lines();
    let tree_oid = lines
        .next()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| GitEngineError::GitCommand {
            args: "merge-tree --write-tree".into(),
            message: "missing tree OID from git merge-tree".into(),
        })?
        .to_owned();
    gix::ObjectId::from_hex(tree_oid.as_bytes()).map_err(|e| GitEngineError::GitCommand {
        args: "merge-tree --write-tree".into(),
        message: format!("invalid tree OID '{tree_oid}': {e}"),
    })?;

    if code == 0 {
        return Ok((tree_oid, Vec::new()));
    }

    let mut conflicts = BTreeSet::new();
    for line in lines {
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            conflicts.insert(trimmed.to_owned());
        }
    }
    if conflicts.is_empty() {
        conflicts.insert("(merge conflict)".to_owned());
    }
    Ok((tree_oid, conflicts.into_iter().collect()))
}

fn create_merge_commit(
    repo_dir: &Path,
    tree_oid: &str,
    base_sha: &str,
    head_sha: &str,
    message: &str,
    author_name: &str,
    author_email: &str,
) -> Result<gix::ObjectId, GitEngineError> {
    let clean_name = if author_name.trim().is_empty() {
        "gitust"
    } else {
        author_name.trim()
    };
    let clean_email = if author_email.trim().is_empty() {
        "gitust@localhost"
    } else {
        author_email.trim()
    };
    let clean_msg = if message.trim().is_empty() {
        "Merge pull request"
    } else {
        message.trim()
    };
    let (code, stdout, stderr) = run_git_status(
        repo_dir,
        &[
            "commit-tree",
            tree_oid,
            "-p",
            base_sha,
            "-p",
            head_sha,
            "-F",
            "-",
        ],
        &[
            ("GIT_AUTHOR_NAME", clean_name),
            ("GIT_AUTHOR_EMAIL", clean_email),
            ("GIT_COMMITTER_NAME", clean_name),
            ("GIT_COMMITTER_EMAIL", clean_email),
        ],
        Some(format!("{clean_msg}\n").into_bytes()),
    )?;
    if code != 0 {
        return Err(GitEngineError::GitCommand {
            args: "commit-tree".into(),
            message: String::from_utf8_lossy(&stderr).trim().to_owned(),
        });
    }
    let hex = String::from_utf8(stdout)
        .map_err(|e| GitEngineError::GitCommand {
            args: "commit-tree".into(),
            message: e.to_string(),
        })?
        .trim()
        .to_owned();
    gix::ObjectId::from_hex(hex.as_bytes()).map_err(|e| GitEngineError::GitCommand {
        args: "commit-tree".into(),
        message: format!("invalid commit OID '{hex}': {e}"),
    })
}

fn run_git_status(
    dir: &Path,
    args: &[&str],
    env: &[(&str, &str)],
    input: Option<Vec<u8>>,
) -> Result<(i32, Vec<u8>, Vec<u8>), GitEngineError> {
    use std::io::Write;

    let mut command = git_command(dir);
    command.args(args);
    for &(k, v) in env {
        command.env(k, v);
    }
    if input.is_some() {
        command.stdin(Stdio::piped());
    }
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let error = |message: String| GitEngineError::GitCommand {
        args: args.join(" "),
        message,
    };
    let mut child = command.spawn().map_err(|e| error(e.to_string()))?;
    let writer = input.map(|bytes| {
        let stdin = child.stdin.take();
        std::thread::spawn(move || match stdin {
            Some(mut stdin) => stdin.write_all(&bytes),
            None => Err(std::io::Error::other("missing stdin")),
        })
    });
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| error("missing stdout".into()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| error("missing stderr".into()))?;
    let cap = 4 * 1024 * 1024;
    let out = std::thread::spawn(move || capture(stdout, cap));
    let err = std::thread::spawn(move || capture(stderr, 8192));
    let start = Instant::now();
    let timeout = Duration::from_secs(60);
    let result = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if start.elapsed() < timeout => std::thread::sleep(Duration::from_millis(5)),
            Ok(None) => break Err(error("Git command timed out".into())),
            Err(e) => break Err(error(e.to_string())),
        }
    };
    terminate_group(&mut child);
    if result.is_err() {
        // fail-open-ok: cleanup after failure, which remains an error below.
        let _ = child.kill();
    }
    child.wait().map_err(|e| error(e.to_string()))?;
    if let Some(writer) = writer {
        writer
            .join()
            .map_err(|_| error("stdin writer panicked".into()))?
            .map_err(|e| error(e.to_string()))?;
    }
    let (stdout_bytes, truncated) = out
        .join()
        .map_err(|_| error("stdout reader panicked".into()))?
        .map_err(|e| error(e.to_string()))?;
    if truncated {
        return Err(error("Git output exceeded limit".into()));
    }
    let (stderr_bytes, _) = err
        .join()
        .map_err(|_| error("stderr reader panicked".into()))?
        .map_err(|e| error(e.to_string()))?;
    let status = result?;
    let code = status
        .code()
        .ok_or_else(|| error("process terminated by signal".into()))?;
    Ok((code, stdout_bytes, stderr_bytes))
}

#[cfg(test)]
#[path = "tests_pull_request_merge.rs"]
mod tests;
