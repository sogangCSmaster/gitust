//! Bind verification to a clean, attached managed checkout, never just a branch label.
use std::io::Read;
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};

use gitust_core::{CommitSha, WorktreeId};

use crate::{git_command, GitEngine, GitEngineError};

impl GitEngine {
    pub fn clean_worktree_commit(
        &self,
        id: WorktreeId,
        branch: &str,
    ) -> Result<CommitSha, GitEngineError> {
        let branch = Self::validate_agent_branch_name(branch)?;
        self.ensure_config_has_no_credentials()?;
        let id = id.0.to_string();
        let entry = self
            .list_agent_worktrees()?
            .into_iter()
            .find(|w| w.worktree_dir.file_name().and_then(|s| s.to_str()) == Some(id.as_str()))
            .ok_or_else(|| invalid("managed checkout is missing"))?;
        if entry.branch != branch {
            return Err(invalid(
                "checkout is detached or belongs to a different branch",
            ));
        }
        // Index flags may hide actual tracked modifications from status. Reject those flags
        // before checking status, without collecting filenames.
        inspect_checkout(&entry.worktree_dir, true)?;
        inspect_checkout(&entry.worktree_dir, false)?;
        let checkout = GitEngine::new(&entry.worktree_dir).acquire_repo()?;
        let mut head = checkout
            .head()
            .map_err(|_| invalid("cannot read checkout HEAD"))?;
        if head
            .referent_name()
            .map(|name| name.shorten().to_string())
            .as_deref()
            != Some(branch.as_str())
        {
            return Err(invalid("checkout branch changed during inspection"));
        }
        let head_sha = head
            .peel_to_commit_in_place()
            .map_err(|_| invalid("cannot resolve checkout HEAD"))?
            .id()
            .to_string();
        let current = self.resolve_branch_oid(&branch)?;
        if head_sha != entry.head.0 || current.as_deref() != Some(entry.head.0.as_str()) {
            return Err(invalid("branch changed while inspecting checkout"));
        }
        Ok(entry.head)
    }
}

fn invalid(message: &str) -> GitEngineError {
    GitEngineError::VerificationInvalid(message.into())
}

/// Both commands are read-only and bounded in time and memory, including descendant pipes.
fn inspect_checkout(dir: &Path, flags: bool) -> Result<(), GitEngineError> {
    let mut command = git_command(dir);
    if flags {
        command.args(["ls-files", "-v", "-z"]);
    } else {
        command.args([
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=normal",
            "--ignore-submodules=none",
        ]);
    }
    command
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command
        .spawn()
        .map_err(|_| invalid("cannot inspect checkout"))?;
    let output = child.stdout.take();
    let reader = std::thread::spawn(move || -> std::io::Result<bool> {
        let mut output = output.ok_or_else(|| std::io::Error::other("missing Git output"))?;
        let mut buf = [0_u8; 4096];
        let mut at_start = true;
        loop {
            let count = output.read(&mut buf)?;
            if count == 0 {
                return Ok(at_start);
            }
            if !flags {
                return Ok(false);
            }
            for byte in &buf[..count] {
                if at_start && (*byte == b'S' || byte.is_ascii_lowercase()) {
                    return Ok(false);
                }
                at_start = *byte == 0;
            }
        }
    });
    let start = Instant::now();
    let mut timed_out = false;
    let mut process_error = false;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {}
            Err(_) => {
                process_error = true;
                break;
            }
        }
        if start.elapsed() >= Duration::from_secs(10) {
            timed_out = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    // Always stop descendants too: they can keep stdout alive after the leader exits.
    #[cfg(unix)]
    {
        // fail-open-ok: cleanup only; timeout/read/process failures are rejected below.
        let _ = unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
    }
    if timed_out || process_error || !reader.is_finished() {
        // fail-open-ok: best-effort cleanup; child status and reader errors still fail closed.
        let _ = child.kill();
    }
    let status = child.wait();
    let clean = reader.join();
    if timed_out {
        return Err(invalid(
            "checkout inspection timed out; retry after checking repository state",
        ));
    }
    if process_error
        || !matches!(status, Ok(status) if status.success())
        || !matches!(clean, Ok(Ok(true)))
    {
        return Err(invalid(if flags {
            "index cannot be verified or uses assume-unchanged/skip-worktree; clear those flags before verification"
        } else {
            "checkout has changes or cannot be inspected; commit or remove changes and retry"
        }));
    }
    Ok(())
}
