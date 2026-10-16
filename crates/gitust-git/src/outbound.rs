//! Exact-commit outbound transport; authorization belongs to the CLI/HTTP caller.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use gitust_core::CommitSha;
use gix::prelude::HeaderExt;

use crate::{io_err, GitEngine, GitEngineError};

impl GitEngine {
    /// Pushes only the immutable commit approved by the caller to one non-agent branch.
    /// Never passes a moving local branch, tags, force, or mirror flags to Git.
    /// Callers MUST establish human approval and repository authorization before calling.
    pub fn push_approved_commit(
        &self,
        remote: &str,
        branch: &str,
        approved: &CommitSha,
    ) -> Result<(), GitEngineError> {
        let branch = Self::validate_outbound_branch(branch)?;
        let remote = Self::validate_mirror_remote_url(remote)?;
        let _service_lock = self.lock_worktree_service()?;
        self.validate_approved_commit(&branch, approved)?;
        let repo = self.acquire_repo()?;

        // A fresh bare repository prevents source config (url.*.pushInsteadOf, remote.*.mirror,
        // push.followTags, includes, helpers and hooks) from changing the destination or ref set.
        // Alternates share objects for this invocation, without copying refs or writing to source.
        let staging = tempfile::tempdir().map_err(|e| io_err(&self.repo_path, &e))?;
        let mut init = outbound_command(staging.path());
        init.args(["init", "--bare", "--template=", "-q"]);
        run_bounded(init, self.fetch_timeout)?;
        let objects = std::fs::canonicalize(Self::common_git_dir(&repo).join("objects"))
            .map_err(|e| io_err(&self.repo_path, &e))?;
        let objects = objects
            .to_str()
            .filter(|p| !p.contains(['\n', '\r']))
            .ok_or_else(|| GitEngineError::InvalidRepoPath("invalid object directory".into()))?;
        let alternates = staging.path().join("objects/info/alternates");
        std::fs::write(&alternates, format!("{objects}\n")).map_err(|e| io_err(&alternates, &e))?;
        let mut push = outbound_command(staging.path());
        push.args([
            "push",
            "--porcelain",
            "--no-follow-tags",
            "--recurse-submodules=no",
            "--",
            &remote,
            &format!("{}:refs/heads/{branch}", approved.0),
        ]);
        run_bounded(push, self.fetch_timeout)
    }

    pub(crate) fn validate_approved_commit(
        &self,
        branch: &str,
        approved: &CommitSha,
    ) -> Result<(), GitEngineError> {
        if approved.0.len() != 40 || !approved.0.bytes().all(|c| c.is_ascii_hexdigit()) {
            return Err(GitEngineError::InvalidRevision(
                "expected full approved commit SHA".into(),
            ));
        }
        let repo = self.acquire_repo()?;
        let id = gix::ObjectId::from_hex(approved.0.as_bytes())
            .map_err(|e| GitEngineError::InvalidRevision(e.to_string()))?;
        let header = repo
            .objects
            .header(id)
            .map_err(|e| GitEngineError::HeadResolution(e.to_string()))?;
        if header.kind() != gix::objs::Kind::Commit {
            return Err(GitEngineError::InvalidRevision(
                "approval must identify a commit".into(),
            ));
        }
        let base = repo
            .find_reference(format!("refs/heads/{branch}").as_str())
            .map_err(|_| GitEngineError::BaseBranchNotFound(branch.to_owned()))?
            .peel_to_id_in_place()
            .map_err(|e| GitEngineError::HeadResolution(e.to_string()))?
            .detach();
        match crate::ancestry::classify_branch_relation(&repo, id, base) {
            crate::ancestry::BranchRelation::FastForward => {}
            crate::ancestry::BranchRelation::Indeterminate(reason) => {
                return Err(GitEngineError::HeadResolution(reason))
            }
            _ => {
                return Err(GitEngineError::MergeConflict(
                    "approved commit is not contained in the local base branch".into(),
                ))
            }
        }

        Ok(())
    }

    pub fn validate_outbound_branch(branch: &str) -> Result<String, GitEngineError> {
        let clean = Self::validate_branch_name(branch)?;
        let lower = clean.to_ascii_lowercase();
        if lower == "agent" || lower.starts_with("agent/") {
            return Err(GitEngineError::InvalidBranchName(
                "agent branches cannot leave this server".into(),
            ));
        }
        Ok(clean)
    }
}

fn outbound_command(dir: &Path) -> Command {
    let mut cmd = Command::new("git");
    // Strip every Git override, including GIT_CONFIG_COUNT/PARAMETERS and transport helpers.
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("GIT_") {
            cmd.env_remove(key);
        }
    }
    cmd.arg("-C").arg(dir).args([
        "-c", "core.hooksPath=/dev/null", "-c", "core.fsmonitor=false",
        "-c", "protocol.allow=never", "-c", "protocol.https.allow=always",
        "-c", "protocol.ssh.allow=always", "-c", "protocol.file.allow=always",
        "-c", "http.followRedirects=false", "-c", "http.lowSpeedLimit=1000",
        "-c", "http.lowSpeedTime=30", "-c", "credential.helper=",
        "-c", "core.sshCommand=ssh -o BatchMode=yes -o ConnectTimeout=15 -o ServerAliveInterval=15 -o ServerAliveCountMax=2",
    ]).env("GIT_CONFIG_NOSYSTEM", "1").env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0").env("GIT_ASKPASS", "false")
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    cmd
}

fn transport_error(message: impl Into<String>) -> GitEngineError {
    GitEngineError::GitCommand {
        args: "outbound push".into(),
        message: message.into(),
    }
}

fn run_bounded(mut cmd: Command, timeout: Duration) -> Result<(), GitEngineError> {
    let mut child = cmd.spawn().map_err(|e| transport_error(e.to_string()))?;
    // Keep draining after the diagnostic cap so a verbose remote cannot trigger SIGPIPE.
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| transport_error("missing stderr pipe"))?;
    let reader = std::thread::spawn(move || -> std::io::Result<()> {
        let mut stderr = stderr;
        std::io::copy(&mut stderr.by_ref().take(8192), &mut std::io::sink())?;
        std::io::copy(&mut stderr, &mut std::io::sink())?;
        Ok(())
    });
    let start = Instant::now();
    let result = loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break Ok(()),
            Ok(Some(_)) => break Err(transport_error("remote rejected push or transport failed; local approval remains unchanged")),
            Ok(None) if start.elapsed() < timeout => std::thread::sleep(Duration::from_millis(10)),
            Ok(None) => break Err(transport_error("outbound push timed out; remote outcome is unknown, retry the same approved commit")),
            Err(e) => break Err(transport_error(e.to_string())),
        }
    };
    // Kill the group even after the leader exits: transport descendants can retain pipe FDs.
    #[cfg(unix)]
    {
        // fail-open-ok: cleanup only; the operation result is already determined and errors remain errors.
        let _ = unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
    }
    if result.is_err() {
        // fail-open-ok: cleanup after failure; never converts the operation to success.
        let _ = child.kill();
    }
    child.wait().map_err(|e| transport_error(e.to_string()))?;
    reader
        .join()
        .map_err(|_| transport_error("stderr reader panicked"))?
        .map_err(|e| transport_error(e.to_string()))?;
    result
}
