//! Bounded host Git execution with a deterministic environment and descendant cleanup.
use std::ffi::OsStr;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use crate::GitEngineError;

pub(crate) fn sanitize_git_environment(command: &mut Command) {
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("GIT_") {
            command.env_remove(key);
        }
    }
    command
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "false");
}

pub(crate) fn git_command(dir: &Path) -> Command {
    let mut command = Command::new("git");
    sanitize_git_environment(&mut command);
    command
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "core.fsmonitor=false",
        ])
        .stdin(Stdio::null());
    command
}

pub(crate) fn terminate_group(child: &mut Child) {
    #[cfg(unix)]
    {
        // fail-open-ok: cleanup cannot change the already determined command result.
        let _ = unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
    }
}

pub(crate) fn capture<R: Read>(mut input: R, cap: usize) -> std::io::Result<(Vec<u8>, bool)> {
    let mut bytes = Vec::new();
    input
        .by_ref()
        .take((cap as u64).saturating_add(1))
        .read_to_end(&mut bytes)?;
    let truncated = bytes.len() > cap;
    bytes.truncate(cap);
    // Retain the pipe through EOF, so verbose Git output never changes its exit status via SIGPIPE.
    std::io::copy(&mut input, &mut std::io::sink())?;
    Ok((bytes, truncated))
}

pub(crate) fn run_capped(
    command: Command,
    timeout: Duration,
    cap: usize,
) -> Result<(Vec<u8>, bool), GitEngineError> {
    run_capped_with_input(command, timeout, cap, None)
}

pub(crate) fn run_capped_with_input(
    mut command: Command,
    timeout: Duration,
    cap: usize,
    input: Option<Vec<u8>>,
) -> Result<(Vec<u8>, bool), GitEngineError> {
    if input.is_some() {
        command.stdin(Stdio::piped());
    }
    let description = format!("{:?}", command.get_program());
    let error = |message: String| GitEngineError::GitCommand {
        args: description.clone(),
        message,
    };
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
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
    let out = std::thread::spawn(move || capture(stdout, cap));
    let err = std::thread::spawn(move || capture(stderr, 8192));
    let start = Instant::now();
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
    let stdout = out
        .join()
        .map_err(|_| error("stdout reader panicked".into()))?
        .map_err(|e| error(e.to_string()))?;
    let (stderr, _) = err
        .join()
        .map_err(|_| error("stderr reader panicked".into()))?
        .map_err(|e| error(e.to_string()))?;
    if !result?.success() {
        return Err(error(String::from_utf8_lossy(&stderr).trim().to_owned()));
    }
    Ok(stdout)
}

pub(crate) fn run_git<I, S>(dir: &Path, args: I) -> Result<Vec<u8>, GitEngineError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = git_command(dir);
    command.args(args);
    let (bytes, truncated) = run_capped(command, Duration::from_secs(60), 64 * 1024 * 1024)?;
    if truncated {
        return Err(GitEngineError::GitCommand {
            args: "git".into(),
            message: "Git output exceeded limit".into(),
        });
    }
    Ok(bytes)
}
