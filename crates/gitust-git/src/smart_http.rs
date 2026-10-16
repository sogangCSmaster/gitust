//! Hardened Git Smart HTTP (`git-upload-pack` & `git-receive-pack --stateless-rpc`) engine.
//!
//! Enforces:
//! - Deterministic stateless-rpc v0/v1 pkt-line framing
//! - Pre-execution pkt-line command parsing for `git-receive-pack` (`inspect_receive_pack_updates`)
//!   to enforce `refs/heads/agent/*` vs protected branch RBAC before `git receive-pack` runs
//! - Strict `transfer.fsckObjects=true` and `receive.fsckObjects=true` quarantine validation
//! - Hook execution disabled (`core.hooksPath=/dev/null`) and sanitized environment

use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::{GitEngine, GitEngineError, AGENT_BRANCH_PREFIX};

/// Maximum request or response payload size for Smart HTTP pack operations (64 MiB).
pub const MAX_SMART_HTTP_BODY_BYTES: usize = 64 * 1024 * 1024;
/// Maximum wall-clock execution time for a single Smart HTTP stateless-rpc invocation.
pub const SMART_HTTP_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmartHttpService {
    UploadPack,
    ReceivePack,
}

impl SmartHttpService {
    #[must_use]
    pub fn parse(service: &str) -> Option<Self> {
        match service.trim() {
            "git-upload-pack" => Some(Self::UploadPack),
            "git-receive-pack" => Some(Self::ReceivePack),
            _ => None,
        }
    }

    #[must_use]
    pub const fn as_git_subcommand(self) -> &'static str {
        match self {
            Self::UploadPack => "upload-pack",
            Self::ReceivePack => "receive-pack",
        }
    }

    #[must_use]
    pub const fn advertisement_preamble(self) -> &'static [u8] {
        match self {
            Self::UploadPack => b"001e# service=git-upload-pack\n0000",
            Self::ReceivePack => b"001f# service=git-receive-pack\n0000",
        }
    }

    #[must_use]
    pub const fn advertisement_content_type(self) -> &'static str {
        match self {
            Self::UploadPack => "application/x-git-upload-pack-advertisement",
            Self::ReceivePack => "application/x-git-receive-pack-advertisement",
        }
    }

    #[must_use]
    pub const fn result_content_type(self) -> &'static str {
        match self {
            Self::UploadPack => "application/x-git-upload-pack-result",
            Self::ReceivePack => "application/x-git-receive-pack-result",
        }
    }
}

/// Parsed ref-update command from the initial pkt-line header of a `git-receive-pack` request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceivePackRefUpdate {
    pub old_oid: String,
    pub new_oid: String,
    pub refname: String,
    pub is_agent_branch: bool,
    pub is_deletion: bool,
}

fn is_valid_hex_oid(s: &str) -> bool {
    s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Parses and validates all pkt-line ref-update commands at the start of a `git-receive-pack`
/// HTTP request body (up to the `0000` flush packet preceding the packfile stream).
pub fn inspect_receive_pack_updates(
    body: &[u8],
) -> Result<Vec<ReceivePackRefUpdate>, GitEngineError> {
    let mut offset = 0usize;
    let mut updates = Vec::new();
    let mut saw_flush = false;

    while offset + 4 <= body.len() {
        let hex_hdr = std::str::from_utf8(&body[offset..offset + 4]).map_err(|_| {
            GitEngineError::InvalidRevision("malformed pkt-line length header".to_owned())
        })?;
        if hex_hdr == "0000" {
            saw_flush = true;
            break;
        }
        let pkt_len = usize::from_str_radix(hex_hdr, 16).map_err(|_| {
            GitEngineError::InvalidRevision("invalid pkt-line hex length".to_owned())
        })?;
        if !(4..=65520).contains(&pkt_len) || offset + pkt_len > body.len() {
            return Err(GitEngineError::InvalidRevision(
                "pkt-line length out of bounds".to_owned(),
            ));
        }

        let payload = &body[offset + 4..offset + pkt_len];
        offset += pkt_len;

        // First command pkt-line contains `<old> <new> <ref>\0<caps>`, subsequent lines have no NUL.
        let cmd_bytes = match payload.iter().position(|&b| b == 0) {
            Some(nul_pos) if updates.is_empty() => &payload[..nul_pos],
            Some(_) => {
                return Err(GitEngineError::InvalidBranchName(
                    "unexpected NUL byte in subsequent receive-pack command".to_owned(),
                ));
            }
            None => payload.strip_suffix(b"\n").unwrap_or(payload),
        };

        let cmd_str = std::str::from_utf8(cmd_bytes).map_err(|_| {
            GitEngineError::InvalidBranchName("non-UTF8 ref update in receive-pack".to_owned())
        })?;
        let mut parts = cmd_str.splitn(3, ' ');
        let old_oid = parts.next().unwrap_or("");
        let new_oid = parts.next().unwrap_or("");
        let raw_ref = parts.next().unwrap_or("");

        if !is_valid_hex_oid(old_oid)
            || !is_valid_hex_oid(new_oid)
            || raw_ref.is_empty()
            || raw_ref.ends_with('/')
        {
            return Err(GitEngineError::InvalidRevision(
                "malformed <old-oid> <new-oid> <refname> tuple in receive-pack".to_owned(),
            ));
        }

        let (refname, is_agent_branch) = if let Some(short) = raw_ref.strip_prefix("refs/heads/") {
            if short.starts_with('/') {
                return Err(GitEngineError::InvalidBranchName(raw_ref.to_owned()));
            }
            let clean = GitEngine::validate_branch_name(short)?;
            if clean != short || clean.eq_ignore_ascii_case("agent") {
                return Err(GitEngineError::InvalidBranchName(raw_ref.to_owned()));
            }
            if clean.to_ascii_lowercase().starts_with(AGENT_BRANCH_PREFIX)
                && !clean.starts_with(AGENT_BRANCH_PREFIX)
            {
                return Err(GitEngineError::InvalidBranchName(raw_ref.to_owned()));
            }
            let is_agent = if clean.starts_with(AGENT_BRANCH_PREFIX) {
                GitEngine::validate_agent_branch_name(&clean)?;
                true
            } else {
                false
            };
            (format!("refs/heads/{clean}"), is_agent)
        } else if let Some(short_tag) = raw_ref.strip_prefix("refs/tags/") {
            if short_tag.starts_with('/') {
                return Err(GitEngineError::InvalidBranchName(raw_ref.to_owned()));
            }
            let clean_tag = GitEngine::validate_branch_name(short_tag)?;
            if clean_tag != short_tag {
                return Err(GitEngineError::InvalidBranchName(raw_ref.to_owned()));
            }
            (format!("refs/tags/{clean_tag}"), false)
        } else {
            return Err(GitEngineError::InvalidBranchName(raw_ref.to_owned()));
        };

        if refname != raw_ref {
            return Err(GitEngineError::InvalidBranchName(raw_ref.to_owned()));
        }

        let is_deletion = new_oid.bytes().all(|b| b == b'0');
        updates.push(ReceivePackRefUpdate {
            old_oid: old_oid.to_ascii_lowercase(),
            new_oid: new_oid.to_ascii_lowercase(),
            refname,
            is_agent_branch,
            is_deletion,
        });
    }

    if !saw_flush || updates.is_empty() {
        return Err(GitEngineError::InvalidRevision(
            "receive-pack request missing valid command pkt-lines or 0000 flush packet".to_owned(),
        ));
    }

    Ok(updates)
}

impl GitEngine {
    /// Generates the Smart HTTP `GET /info/refs?service=git-upload-pack|git-receive-pack`
    /// advertisement payload (`# service=<svc>\n0000` + `--advertise-refs` output).
    pub fn smart_http_advertise_refs(
        &self,
        service: SmartHttpService,
    ) -> Result<Vec<u8>, GitEngineError> {
        self.ensure_config_has_no_credentials()?;
        let repo = self.acquire_repo()?;
        let common_git_dir = Self::common_git_dir(&repo);
        let refs_out = run_git_stateless_rpc(&common_git_dir, service, true, &[])?;
        let preamble = service.advertisement_preamble();
        let mut full = Vec::with_capacity(preamble.len() + refs_out.len());
        full.extend_from_slice(preamble);
        full.extend_from_slice(&refs_out);
        Ok(full)
    }

    /// Executes `POST /git-upload-pack` (`--stateless-rpc`) for `git clone` / `git fetch`.
    pub fn smart_http_upload_pack(&self, request_body: &[u8]) -> Result<Vec<u8>, GitEngineError> {
        if request_body.is_empty() || request_body.len() > MAX_SMART_HTTP_BODY_BYTES {
            return Err(GitEngineError::InvalidRevision(
                "invalid upload-pack request body length".to_owned(),
            ));
        }
        self.ensure_config_has_no_credentials()?;
        let repo = self.acquire_repo()?;
        let common_git_dir = Self::common_git_dir(&repo);
        run_git_stateless_rpc(
            &common_git_dir,
            SmartHttpService::UploadPack,
            false,
            request_body,
        )
    }

    /// Executes `POST /git-receive-pack` (`--stateless-rpc`) for `git push` after caller has
    /// verified `inspect_receive_pack_updates(request_body)` against role and branch rules.
    pub fn smart_http_receive_pack(&self, request_body: &[u8]) -> Result<Vec<u8>, GitEngineError> {
        if request_body.is_empty() || request_body.len() > MAX_SMART_HTTP_BODY_BYTES {
            return Err(GitEngineError::InvalidRevision(
                "invalid receive-pack request body length".to_owned(),
            ));
        }
        let _service_lock = self.lock_worktree_service()?;
        self.ensure_config_has_no_credentials()?;
        let repo = self.acquire_repo()?;
        let common_git_dir = Self::common_git_dir(&repo);
        run_git_stateless_rpc(
            &common_git_dir,
            SmartHttpService::ReceivePack,
            false,
            request_body,
        )
    }

    /// Resolves `refs/heads/<branch>` to its current peeled object ID hex, or `Ok(None)` if absent.
    pub fn resolve_branch_oid(&self, branch: &str) -> Result<Option<String>, GitEngineError> {
        let full = format!("refs/heads/{}", branch.trim_start_matches("refs/heads/"));
        let repo = self.acquire_repo()?;
        let Some(mut r) = repo
            .try_find_reference(full.as_str())
            .map_err(|e| GitEngineError::RefTransaction(e.to_string()))?
        else {
            return Ok(None);
        };
        let oid = r
            .peel_to_id_in_place()
            .map_err(|e| GitEngineError::HeadResolution(e.to_string()))?
            .detach()
            .to_string();
        Ok(Some(oid))
    }
}

fn read_bounded_stream<R: Read>(mut reader: R, cap: usize) -> Result<Vec<u8>, std::io::Error> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 16384];
    loop {
        let n = reader.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        if buf.len() + n > cap {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("git stateless-rpc output exceeded {cap} bytes"),
            ));
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    Ok(buf)
}

fn terminate_stateless_rpc_child(child: &mut std::process::Child) {
    #[cfg(unix)]
    // fail-open-ok: PID fits in positive i32 on Unix; fallback child.kill() still runs below.
    if let Ok(pgid) = i32::try_from(child.id()) {
        if pgid > 0 {
            // SAFETY: child was spawned with `process_group(0)`, so `pgid` is its own process group.
            unsafe {
                // fail-open-ok: best-effort SIGKILL on child process group.
                let _ = libc::killpg(pgid, libc::SIGKILL);
            }
        }
    }
    // fail-open-ok: best-effort SIGKILL on git subprocess.
    let _ = child.kill();
    // fail-open-ok: reap killed child process to prevent zombie.
    let _ = child.wait();
}

fn run_git_stateless_rpc(
    common_git_dir: &Path,
    service: SmartHttpService,
    advertise_only: bool,
    stdin_bytes: &[u8],
) -> Result<Vec<u8>, GitEngineError> {
    let subcmd = service.as_git_subcommand();
    let mut cmd = Command::new("git");
    cmd.args([
        "-c",
        "core.hooksPath=/dev/null",
        "-c",
        "core.fsmonitor=false",
        "-c",
        "transfer.fsckObjects=true",
        "-c",
        "receive.fsckObjects=true",
        "-c",
        "receive.denyNonFastForwards=true",
        "-c",
        "receive.denyDeleteCurrent=true",
        "-c",
        "receive.denyCurrentBranch=updateInstead",
        "-c",
        "receive.advertisePushOptions=false",
        subcmd,
        "--stateless-rpc",
    ]);
    if advertise_only {
        cmd.arg("--advertise-refs");
    }
    cmd.arg("--").arg(common_git_dir);
    cmd.env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env_remove("GIT_PROTOCOL")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_COMMON_DIR")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_OBJECT_DIRECTORY")
        .env_remove("GIT_ALTERNATE_OBJECT_DIRECTORIES")
        .stdin(if advertise_only {
            Stdio::null()
        } else {
            Stdio::piped()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    let mut child = cmd.spawn().map_err(|e| GitEngineError::GitCommand {
        args: format!("{subcmd} --stateless-rpc"),
        message: e.to_string(),
    })?;

    let mut child_stdin = child.stdin.take();
    let child_stdout = child
        .stdout
        .take()
        .ok_or_else(|| GitEngineError::GitCommand {
            args: format!("{subcmd} --stateless-rpc"),
            message: "missing child stdout pipe".to_owned(),
        })?;
    let child_stderr = child
        .stderr
        .take()
        .ok_or_else(|| GitEngineError::GitCommand {
            args: format!("{subcmd} --stateless-rpc"),
            message: "missing child stderr pipe".to_owned(),
        })?;

    std::thread::scope(|s| {
        let writer = s.spawn(move || {
            if let Some(mut stdin) = child_stdin.take() {
                // fail-open-ok: child may exit early on invalid pack header; exit status and stderr are checked below.
                let _ = stdin.write_all(stdin_bytes);
                // fail-open-ok: best-effort flush before closing child stdin pipe.
                let _ = stdin.flush();
            }
        });
        let out_reader =
            s.spawn(move || read_bounded_stream(child_stdout, MAX_SMART_HTTP_BODY_BYTES));
        let err_reader = s.spawn(move || read_bounded_stream(child_stderr, 64 * 1024));

        let deadline = Instant::now() + SMART_HTTP_TIMEOUT;
        let status = loop {
            match child.try_wait() {
                Ok(Some(st)) => break Ok(st),
                Ok(None) => {
                    if Instant::now() >= deadline {
                        terminate_stateless_rpc_child(&mut child);
                        break Err(GitEngineError::GitCommand {
                            args: format!("{subcmd} --stateless-rpc"),
                            message: "smart HTTP git subprocess timed out".to_owned(),
                        });
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(e) => {
                    terminate_stateless_rpc_child(&mut child);
                    break Err(GitEngineError::GitCommand {
                        args: format!("{subcmd} --stateless-rpc"),
                        message: e.to_string(),
                    });
                }
            }
        }?;

        // fail-open-ok: writer thread join; any pipe break is surfaced by child status / stdout read below.
        let _ = writer.join();
        let stdout = out_reader
            .join()
            .map_err(|_| GitEngineError::GitCommand {
                args: format!("{subcmd} --stateless-rpc"),
                message: "stdout reader thread panicked".to_owned(),
            })?
            .map_err(|e| GitEngineError::GitCommand {
                args: format!("{subcmd} --stateless-rpc"),
                message: e.to_string(),
            })?;
        let stderr = err_reader
            .join()
            .map_err(|_| GitEngineError::GitCommand {
                args: format!("{subcmd} --stateless-rpc"),
                message: "stderr reader thread panicked".to_owned(),
            })?
            .map_err(|e| GitEngineError::GitCommand {
                args: format!("{subcmd} --stateless-rpc"),
                message: e.to_string(),
            })?;

        if !status.success() {
            return Err(GitEngineError::GitCommand {
                args: format!("{subcmd} --stateless-rpc"),
                message: String::from_utf8_lossy(&stderr).trim().to_owned(),
            });
        }
        Ok(stdout)
    })
}
