//! Confirm host process-group shutdown before reporting a completed command.

use std::path::Path;
use std::time::Duration;

use crate::SandboxError;

const CLEANUP_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_PROCESS_TABLE_BYTES: usize = 4 * 1024 * 1024;
const MAX_OBSERVATION_FAILURES: usize = 3;

fn incomplete(message: impl Into<String>) -> SandboxError {
    SandboxError::CleanupIncomplete {
        message: message.into(),
    }
}

pub(crate) fn signal_group(pid: u32) -> Result<(), SandboxError> {
    let group = i32::try_from(pid).map_err(|_| incomplete("invalid host process group"))?;
    if group <= 1 {
        return Err(incomplete("invalid host process group"));
    }
    #[cfg(unix)]
    {
        // SAFETY: this positive host group was created from our child with process_group(0).
        if unsafe { libc::killpg(group, libc::SIGKILL) } != 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ESRCH) {
                return Err(incomplete("could not stop the sandbox process group"));
            }
        }
        Ok(())
    }
    #[cfg(not(unix))]
    Err(incomplete("host process-group cleanup is unsupported"))
}

pub(crate) async fn wait_group_stopped(pid: u32) -> Result<(), SandboxError> {
    let group = i32::try_from(pid).map_err(|_| incomplete("invalid host process group"))?;
    wait_with_observer(|| {
        tokio::task::spawn_blocking(move || observe_group(Path::new("/bin/ps"), group))
    })
    .await
}

async fn wait_with_observer<F>(mut observe: F) -> Result<(), SandboxError>
where
    F: FnMut() -> tokio::task::JoinHandle<Result<bool, SandboxError>>,
{
    tokio::time::timeout(CLEANUP_TIMEOUT, async move {
        let mut failures = 0;
        loop {
            match observe()
                .await
                .map_err(|_| incomplete("host process observer did not complete"))?
            {
                // Only a fresh valid stopped observation can confirm cleanup.
                Ok(false) => return Ok(()),
                Ok(true) => {}
                Err(error) => {
                    failures += 1;
                    if failures >= MAX_OBSERVATION_FAILURES {
                        return Err(error);
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .map_err(|_| incomplete("sandbox process-group shutdown was not confirmed"))?
}

fn observe_group(program: &Path, group: i32) -> Result<bool, SandboxError> {
    let output = std::process::Command::new(program)
        .args(["-ax", "-o", "pid=,ppid=,pgid=,stat="])
        .env_clear()
        .env("LANG", "C")
        .output()
        .map_err(|_| incomplete("host process observer could not start"))?;
    if !output.status.success() {
        return Err(incomplete("host process observer failed"));
    }
    if output.stdout.len() > MAX_PROCESS_TABLE_BYTES {
        return Err(incomplete("host process table exceeds the cleanup limit"));
    }
    let table = std::str::from_utf8(&output.stdout)
        .map_err(|_| incomplete("host process table is not valid text"))?;
    group_is_running(table, group)
}

fn group_is_running(table: &str, group: i32) -> Result<bool, SandboxError> {
    let mut observed = false;
    let mut running = false;
    for line in table.lines() {
        let mut columns = line.split_whitespace();
        let mut number = || {
            columns
                .next()
                .ok_or_else(|| incomplete("invalid host process table"))?
                .parse::<i32>()
                .map_err(|_| incomplete("invalid host process table"))
        };
        let pid = number()?;
        let parent = number()?;
        let pgid = number()?;
        let state = columns
            .next()
            .ok_or_else(|| incomplete("invalid host process table"))?;
        if pid <= 0 || parent < 0 || pgid < 0 || columns.next().is_some() {
            return Err(incomplete("invalid host process table"));
        }
        observed = true;
        running |= pgid == group && !state.starts_with('Z');
    }
    if !observed {
        return Err(incomplete("host process observer returned no rows"));
    }
    Ok(running)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleanup_observer_distinguishes_live_groups_from_zombies() {
        let table = "12 1 12 Z\n13 1 12 S+\n15 1 15 R\n";
        assert!(group_is_running(table, 12).unwrap());
        assert!(!group_is_running("12 1 12 Z\n15 1 15 S\n", 12).unwrap());
        assert!(!group_is_running(table, 99).unwrap());
    }

    #[test]
    fn cleanup_observer_fails_closed_on_errors_and_invalid_process_tables() {
        for table in ["", "12 1 12", "12 1 invalid S", "12 1 12 S unexpected"] {
            assert!(matches!(
                group_is_running(table, 12),
                Err(SandboxError::CleanupIncomplete { .. })
            ));
        }
        for program in ["/bin/false", "/nonexistent-gitust-process-observer"] {
            assert!(matches!(
                observe_group(Path::new(program), 12),
                Err(SandboxError::CleanupIncomplete { .. })
            ));
        }
        for group in [0, 1, u32::MAX] {
            assert!(signal_group(group).is_err());
        }
    }

    #[tokio::test]
    async fn cleanup_observer_retries_transient_error_before_valid_stopped_table() {
        let mut calls = 0;
        let result = wait_with_observer(|| {
            calls += 1;
            let table = if calls == 1 { "" } else { "15 1 15 S\n" };
            tokio::spawn(async move { group_is_running(table, 12) })
        })
        .await;
        assert!(result.is_ok());
        assert_eq!(calls, 2);
    }

    #[tokio::test]
    async fn cleanup_observer_rejects_after_three_failed_observations() {
        let mut calls = 0;
        let result = wait_with_observer(|| {
            calls += 1;
            let table = match calls {
                2 | 4 => "12 1 12 S\n",
                _ => "",
            };
            tokio::spawn(async move { group_is_running(table, 12) })
        })
        .await;
        assert!(matches!(
            result,
            Err(SandboxError::CleanupIncomplete { .. })
        ));
        assert_eq!(calls, 5);
    }

    #[tokio::test]
    async fn cleanup_observer_waits_for_running_group_to_stop() {
        let mut calls = 0;
        let result = wait_with_observer(|| {
            calls += 1;
            let table = if calls == 1 {
                "12 1 12 S\n15 1 15 R\n"
            } else {
                "12 1 12 Z\n15 1 15 R\n"
            };
            tokio::spawn(async move { group_is_running(table, 12) })
        })
        .await;
        assert!(result.is_ok());
        assert_eq!(calls, 2);
    }

    #[tokio::test]
    async fn cleanup_observer_running_group_keeps_existing_deadline() {
        let result =
            wait_with_observer(|| tokio::spawn(async { group_is_running("12 1 12 S\n", 12) }))
                .await;
        assert!(matches!(
            result,
            Err(SandboxError::CleanupIncomplete { message })
                if message == "sandbox process-group shutdown was not confirmed"
        ));
    }
}
