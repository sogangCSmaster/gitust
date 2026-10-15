//! Shared repository `.git/config` and `config.worktree` credential inspection guard.
//!
//! Agent linked worktrees have read access to `<common_git_dir>/config` and `config.worktree`.
//! This module fails closed if either config file contains embedded HTTP userinfo, `http.extraHeader`,
//! `credential.helper = store` / `!cmd`, or `[include]` / `[includeIf]` directives.

use std::fs;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::{io_err, GitEngineError};

/// Executes a hardened `git fetch` into `refs/remotes/upstream/*` and `refs/remotes/upstream-tags/*`:
/// - Excludes `^refs/heads/agent/*` at protocol negotiation time so upstream `agent/*` objects and
///   refs are never downloaded.
/// - Enforces protocol allowlists (`https`, `ssh`, `file` only) and strips hooks/fsmonitor/env overrides.
/// - Enforces Git's built-in stall detection (`http.lowSpeedLimit=1000`, `http.lowSpeedTime=30`,
///   SSH `ConnectTimeout=15`, `ServerAliveInterval=15`, `ServerAliveCountMax=2`) in addition to an
///   overall process-group wall-clock ceiling (`timeout`).
pub(crate) fn run_git_fetch_with_timeout(
    repo_path: &Path,
    clean_url: &str,
    timeout: Duration,
) -> Result<(), GitEngineError> {
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(repo_path)
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "protocol.allow=never",
            "-c",
            "protocol.https.allow=always",
            "-c",
            "protocol.ssh.allow=always",
            "-c",
            "protocol.file.allow=always",
            "-c",
            "http.lowSpeedLimit=1000",
            "-c",
            "http.lowSpeedTime=30",
            "-c",
            "fetch.writeCommitGraph=true",
            "-c",
            "core.sshCommand=ssh -o BatchMode=yes -o ConnectTimeout=15 -o ServerAliveInterval=15 -o ServerAliveCountMax=2",
            "fetch",
            "--prune",
            "--no-tags",
            "--no-write-fetch-head",
            "--",
            clean_url,
            "+refs/heads/*:refs/remotes/upstream/*",
            "^refs/heads/agent/*",
            "+refs/tags/*:refs/remotes/upstream-tags/*",
        ])
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_COMMON_DIR")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_OBJECT_DIRECTORY")
        .env_remove("GIT_ALTERNATE_OBJECT_DIRECTORIES")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    let mut child = cmd.spawn().map_err(|e| GitEngineError::GitCommand {
        args: "fetch".to_owned(),
        message: e.to_string(),
    })?;

    let stderr_handle = child.stderr.take().map(|stderr| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = stderr.take(8192).read_to_end(&mut buf);
            buf
        })
    });

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if Instant::now() >= deadline {
                    #[cfg(unix)]
                    {
                        let pgid = format!("-{}", child.id());
                        let _ = Command::new("kill")
                            .args(["-KILL", "--", &pgid])
                            .stdin(Stdio::null())
                            .stdout(Stdio::null())
                            .stderr(Stdio::null())
                            .status();
                    }
                    let _ = child.kill();
                    let _ = child.wait();
                    if let Some(h) = stderr_handle {
                        let _ = h.join();
                    }
                    return Err(GitEngineError::GitCommand {
                        args: "fetch".to_owned(),
                        message: format!("git fetch timed out after {}ms", timeout.as_millis()),
                    });
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                if let Some(h) = stderr_handle {
                    let _ = h.join();
                }
                return Err(GitEngineError::GitCommand {
                    args: "fetch".to_owned(),
                    message: e.to_string(),
                });
            }
        }
    };

    let stderr_bytes = stderr_handle
        .and_then(|h| h.join().ok())
        .unwrap_or_default();

    if !status.success() {
        return Err(GitEngineError::GitCommand {
            args: "fetch".to_owned(),
            message: String::from_utf8_lossy(&stderr_bytes).trim().to_owned(),
        });
    }
    Ok(())
}

/// Fails closed if the shared repository config (`config` or `config.worktree`) contains
/// credentials or external config includes an agent could read: userinfo in `http(s)://` URLs
/// (`https://user:token@host`, `https://token@host`), any `http.extraHeader` setting,
/// `credential.helper = store` / `!cmd`, or `[include]` / `[includeIf]` directives.
pub(crate) fn reject_credentials_in_config(common_git_dir: &Path) -> Result<(), GitEngineError> {
    for name in ["config", "config.worktree"] {
        let config_path = common_git_dir.join(name);
        let text = match fs::read_to_string(&config_path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(io_err(&config_path, &e)),
        };
        if config_contains_credentials(&text) {
            return Err(GitEngineError::CredentialsInRepoConfig);
        }
    }
    Ok(())
}

pub(crate) fn contains_http_userinfo(text: &str) -> bool {
    for scheme in ["http://", "https://"] {
        let mut rest = text;
        while let Some(idx) = rest.find(scheme) {
            let after = &rest[idx + scheme.len()..];
            let authority_end = after
                .find(|c: char| c == '/' || c == '\'' || c == ']' || c.is_whitespace())
                .unwrap_or(after.len());
            if after[..authority_end].contains('@') {
                return true;
            }
            rest = &after[authority_end..];
        }
    }
    false
}

pub(crate) fn config_contains_credentials(text: &str) -> bool {
    // 1. Formal RFC-compliant `git-config` AST inspection via `gix::config::File`.
    // Fails closed if the config cannot be parsed by `gitoxide`, and inspects all unquoted
    // section names, subsection headers, keys, and escape-decoded values.
    let Ok(cfg) = gix::config::File::try_from(text) else {
        return true;
    };
    for section in cfg.sections() {
        let header = section.header();
        let sec_name = String::from_utf8_lossy(header.name().as_ref()).to_ascii_lowercase();
        if sec_name == "include" || sec_name == "includeif" {
            return true;
        }
        if let Some(sub) = header.subsection_name() {
            let sub_lower = String::from_utf8_lossy(sub.as_ref())
                .to_ascii_lowercase()
                .replace('"', "");
            if contains_http_userinfo(&sub_lower) {
                return true;
            }
        }
        for key in section.body().value_names() {
            let key_str: &str = key.as_ref();
            let key_lower = key_str.to_ascii_lowercase();
            if key_lower == "extraheader" {
                return true;
            }
            for val in section.values(key_str) {
                let decoded = String::from_utf8_lossy(val.as_ref())
                    .to_ascii_lowercase()
                    .replace('"', "");
                let trimmed_val = decoded.trim();
                if key_lower == "helper"
                    && (trimmed_val.starts_with("store") || trimmed_val.starts_with('!'))
                {
                    return true;
                }
                if contains_http_userinfo(trimmed_val) {
                    return true;
                }
            }
        }
    }

    // 2. Defense-in-depth lexical scan (folds backslash line continuations and strips mid-token quotes).
    let folded: std::borrow::Cow<'_, str> = if text.contains('\\') {
        std::borrow::Cow::Owned(text.replace("\\\r\n", "").replace("\\\n", ""))
    } else {
        std::borrow::Cow::Borrowed(text)
    };
    for raw in folded.lines() {
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with(';') {
            continue;
        }
        let line = trimmed.to_ascii_lowercase().replace('"', "");
        if line.contains("extraheader") {
            return true;
        }
        if let Some(section) = line.strip_prefix('[') {
            let header = section.trim_start();
            if let Some(rest) = header.strip_prefix("include") {
                if rest.starts_with(']')
                    || rest.starts_with(char::is_whitespace)
                    || rest.starts_with("if")
                {
                    return true;
                }
            }
        }
        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim();
            let unquoted = value
                .trim()
                .trim_matches(|c: char| c.is_whitespace() || c == '\'');
            if key == "include.path" || (key.starts_with("includeif.") && key.ends_with(".path")) {
                return true;
            }
            if (key == "helper" || key.ends_with(".helper"))
                && (unquoted.starts_with("store") || unquoted.starts_with('!'))
            {
                return true;
            }
        }
        if contains_http_userinfo(&line) {
            return true;
        }
    }
    false
}
