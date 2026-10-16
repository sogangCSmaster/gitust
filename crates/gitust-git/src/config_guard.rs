//! Shared repository `.git/config` and `config.worktree` credential inspection guard.
//!
//! Agent linked worktrees have read access to `<common_git_dir>/config` and `config.worktree`.
//! This module fails closed if either config file contains embedded HTTP userinfo, `http.extraHeader`,
//! `credential.helper = store` / `!cmd`, or `[include]` / `[includeIf]` directives.

use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::{io_err, GitEngine, GitEngineError};

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
}

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
            "-c", "http.followRedirects=false",
            "-c", "credential.helper=",
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
        .stdin(Stdio::null());
    crate::process::sanitize_git_environment(&mut cmd);
    crate::process::run_capped(cmd, timeout, 0)?;
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
                // fail-open-ok: without an authority delimiter the entire remaining URL is inspected for userinfo.
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
    // fail-open-ok: returning `true` ("contains credentials") on parse failure is strictly fail-closed.
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
            if key_lower == "extraheader"
                || (sec_name == "http"
                    && matches!(key_lower.as_str(), "cookiefile" | "sslkey" | "sslcert"))
                || (sec_name == "core" && matches!(key_lower.as_str(), "sshcommand" | "askpass"))
                || (sec_name == "filter"
                    && matches!(key_lower.as_str(), "clean" | "smudge" | "process"))
                || (sec_name == "diff" && matches!(key_lower.as_str(), "command" | "textconv"))
                || (sec_name == "url"
                    && matches!(key_lower.as_str(), "insteadof" | "pushinsteadof"))
            {
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
