//! Authenticated SSH `git-receive-pack` supervisor with pre-command target validation and RBAC gate.
use crate::{
    ssh_upload_pack::{
        io, publish_cleanup_failure, Completion, Prepared, Pump, SshUploadPack,
        SshUploadPackCancel, SshUploadPackError, SshUploadPackOutcome,
    },
    GitEngine, ReceivePackRefUpdate, MAX_SMART_HTTP_BODY_BYTES, SMART_HTTP_TIMEOUT,
};
use std::{path::PathBuf, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::{mpsc, watch},
    task::JoinSet,
    time::{sleep, sleep_until, timeout, Instant},
};

const MAX_COMMAND_HEADER_BYTES: usize = 256 * 1024;

pub type SshReceivePack = SshUploadPack;
pub type SshReceivePackCancel = SshUploadPackCancel;
pub type SshReceivePackError = SshUploadPackError;
pub type SshReceivePackOutcome = SshUploadPackOutcome;

impl GitEngine {
    /// Supervises `git receive-pack` over an SSH channel stream.
    ///
    /// - `spawn_gate` runs on `spawn_blocking` immediately before spawning `git receive-pack`.
    /// - `command_gate` runs on `spawn_blocking` after parsing the client's ref-update pkt-lines
    ///   (up to `0000`) and validating target refs on disk, before any command byte reaches
    ///   `git receive-pack` stdin.
    /// - `finish_hook` runs on `spawn_blocking` after child reap and before releasing `keep_alive`.
    pub fn start_ssh_receive_pack<S, H, G, C, F>(
        &self,
        stream: S,
        keep_alive: H,
        spawn_gate: G,
        command_gate: C,
        finish_hook: F,
    ) -> Result<SshReceivePack, SshReceivePackError>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
        H: Send + 'static,
        G: FnOnce() -> Result<(), ()> + Send + 'static,
        C: FnOnce(Vec<ReceivePackRefUpdate>) -> Result<(), ()> + Send + 'static,
        F: FnOnce(Result<SshReceivePackOutcome, SshReceivePackError>) + Send + 'static,
    {
        let runtime =
            tokio::runtime::Handle::try_current().map_err(|_| SshUploadPackError::Unavailable)?;
        let (cancel, receiver) = watch::channel(false);
        let (completed, result) = watch::channel(None);
        runtime.spawn(supervise(
            self.clone(),
            stream,
            keep_alive,
            spawn_gate,
            command_gate,
            finish_hook,
            receiver,
            completed,
        ));
        Ok(SshUploadPack::from_channels(cancel, result))
    }
}

fn prepare<G>(
    engine: GitEngine,
    spawn_gate: G,
    cancel: watch::Receiver<bool>,
    deadline: Instant,
) -> Result<(Prepared, PathBuf), SshUploadPackError>
where
    G: FnOnce() -> Result<(), ()>,
{
    let service = engine
        .lock_worktree_service()
        .map_err(|_| SshUploadPackError::Unavailable)?;
    engine
        .ensure_config_has_no_credentials()
        .map_err(|_| SshUploadPackError::Unavailable)?;
    let repository = engine
        .acquire_repo()
        .map_err(|_| SshUploadPackError::Unavailable)?;
    let common = GitEngine::common_git_dir(&repository);
    let mut command = crate::process::git_command(&common);
    command.args([
        "-c",
        "core.hooksPath=/dev/null",
        "-c",
        "core.fsmonitor=false",
        "-c",
        "transfer.fsckObjects=true",
        "-c",
        "transfer.hideRefs=refs/remotes/",
        "-c",
        "transfer.hideRefs=refs/heads/agent/cli/",
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
        "receive-pack",
        "--",
        ".",
    ]);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    if *cancel.borrow() {
        return Err(SshUploadPackError::Cancelled);
    }
    if Instant::now() >= deadline {
        return Err(SshUploadPackError::TimedOut);
    }
    spawn_gate().map_err(|_| SshUploadPackError::Denied)?;
    if *cancel.borrow() {
        return Err(SshUploadPackError::Cancelled);
    }
    if Instant::now() >= deadline {
        return Err(SshUploadPackError::TimedOut);
    }
    let child = tokio::process::Command::from(command)
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| SshUploadPackError::Unavailable)?;
    let process_group = child.id().ok_or(SshUploadPackError::Unavailable)?;
    Ok((
        Prepared {
            child,
            process_group,
            group_cleanup: None,
            _service: service,
        },
        common,
    ))
}

#[allow(clippy::too_many_arguments)]
async fn supervise<S, H, G, C, F>(
    engine: GitEngine,
    stream: S,
    keep_alive: H,
    spawn_gate: G,
    command_gate: C,
    finish_hook: F,
    mut cancel: watch::Receiver<bool>,
    completed: watch::Sender<Option<Completion>>,
) where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    H: Send + 'static,
    G: FnOnce() -> Result<(), ()> + Send + 'static,
    C: FnOnce(Vec<ReceivePackRefUpdate>) -> Result<(), ()> + Send + 'static,
    F: FnOnce(Result<SshReceivePackOutcome, SshReceivePackError>) + Send + 'static,
{
    let deadline = Instant::now() + SMART_HTTP_TIMEOUT;
    let preparation_cancel = cancel.clone();
    let prepared = tokio::task::spawn_blocking(move || {
        prepare(engine, spawn_gate, preparation_cancel, deadline)
    })
    .await;
    let (mut prepared, common) = match prepared {
        Ok(Ok(pair)) => pair,
        value => {
            let error = match value {
                Ok(Err(error)) => error,
                _ => SshUploadPackError::Unavailable,
            };
            drop(stream);
            // fail-open-ok: prepare already failed before spawning Git; completion reports the error below.
            let _ = tokio::task::spawn_blocking(move || {
                finish_hook(Err(error));
                drop(keep_alive);
            })
            .await;
            completed.send_replace(Some(Completion {
                result: Err(error),
                cleanup: Ok(()),
            }));
            return;
        }
    };
    let mut pumps = JoinSet::new();
    let result = drive(
        &mut prepared,
        common,
        stream,
        command_gate,
        &mut cancel,
        deadline,
        &mut pumps,
    )
    .await;
    let group = if result.is_ok() {
        prepared.group_cleanup = Some(Ok(()));
        Ok(())
    } else {
        prepared.stop_group()
    };
    let mut cleanup_failed = group.is_err();
    if cleanup_failed {
        publish_cleanup_failure(&completed);
    }
    let reaped = match timeout(Duration::from_secs(2), prepared.child.wait()).await {
        Ok(value) => value,
        Err(_) => {
            cleanup_failed = true;
            publish_cleanup_failure(&completed);
            prepared.child.wait().await
        }
    };
    if reaped.is_err() {
        publish_cleanup_failure(&completed);
        let quarantine = (prepared, keep_alive, pumps);
        std::future::pending::<()>().await;
        drop(quarantine);
        return;
    }
    pumps.abort_all();
    while pumps.join_next().await.is_some() {}
    if group.is_err() {
        let quarantine = (prepared, keep_alive);
        std::future::pending::<()>().await;
        drop(quarantine);
        return;
    }
    let final_result = result.and_then(|()| {
        let exit_code = reaped
            .map_err(|_| SshUploadPackError::CleanupIncomplete)?
            .code()
            .ok_or(SshUploadPackError::Failed)?;
        let exit_code = u32::try_from(exit_code).map_err(|_| SshUploadPackError::Failed)?;
        Ok(SshUploadPackOutcome { exit_code })
    });
    let hook_outcome = if cleanup_failed {
        Err(SshUploadPackError::CleanupIncomplete)
    } else {
        final_result
    };
    if tokio::task::spawn_blocking(move || {
        finish_hook(hook_outcome);
        drop(prepared);
        drop(keep_alive);
    })
    .await
    .is_err()
    {
        publish_cleanup_failure(&completed);
        return;
    }
    if cleanup_failed {
        return;
    }
    completed.send_replace(Some(Completion {
        result: final_result,
        cleanup: Ok(()),
    }));
}

async fn drive<S, C>(
    prepared: &mut Prepared,
    common_git_dir: PathBuf,
    stream: S,
    command_gate: C,
    cancel: &mut watch::Receiver<bool>,
    deadline: Instant,
    pumps: &mut JoinSet<Pump>,
) -> Result<(), SshUploadPackError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    C: FnOnce(Vec<ReceivePackRefUpdate>) -> Result<(), ()> + Send + 'static,
{
    let mut input = prepared.child.stdin.take().ok_or(SshUploadPackError::Io)?;
    let output = prepared.child.stdout.take().ok_or(SshUploadPackError::Io)?;
    let stderr = prepared.child.stderr.take().ok_or(SshUploadPackError::Io)?;
    let (reader, writer) = tokio::io::split(stream);
    let (failures, mut failure) = mpsc::channel(3);
    let input_failure = failures.clone();
    let input_task = pumps.spawn(async move {
        if let Err(error) = receive_input(reader, &mut input, common_git_dir, command_gate).await {
            // fail-open-ok: a closed receiver means the supervisor already owns cleanup.
            let _ = input_failure.send(error).await;
            std::future::pending::<()>().await;
        }
        drop(input);
        Pump::Input
    });
    let output_failure = failures.clone();
    pumps.spawn(async move {
        io::output(output, writer, MAX_SMART_HTTP_BODY_BYTES, output_failure).await;
        Pump::Output
    });
    pumps.spawn(async move {
        io::stderr(stderr, failures).await;
        Pump::Stderr
    });
    let mut exited = false;
    loop {
        if *cancel.borrow() {
            return Err(SshUploadPackError::Cancelled);
        }
        match failure.try_recv() {
            Ok(error) => return Err(error),
            Err(mpsc::error::TryRecvError::Empty | mpsc::error::TryRecvError::Disconnected) => {}
        }
        if exited && pumps.is_empty() {
            break;
        }
        tokio::select! {
            biased;
            changed = cancel.changed() => {
                if changed.is_err() || *cancel.borrow() { return Err(SshUploadPackError::Cancelled); }
            }
            // fail-open-ok: this infallible timer ends the transport with an explicit timeout error.
            _ = sleep_until(deadline) => return Err(SshUploadPackError::TimedOut),
            Some(error) = failure.recv() => return Err(error),
            // fail-open-ok: this infallible timer only schedules the checked, non-reaping exit observation.
            _ = sleep(Duration::from_millis(5)), if !exited => {
                if prepared.exited_without_reaping()? {
                    exited = true;
                    input_task.abort();
                }
            }
            result = pumps.join_next(), if !pumps.is_empty() => {
                if result.is_some_and(|value| value.is_err_and(|error| !error.is_cancelled())) {
                    return Err(SshUploadPackError::Io);
                }
            }
        }
    }
    Ok(())
}

async fn read_first_header<R: AsyncRead + Unpin>(
    source: &mut R,
    header: &mut [u8; 4],
) -> Result<bool, SshUploadPackError> {
    let mut filled = 0usize;
    while filled < 4 {
        let read = source
            .read(&mut header[filled..])
            .await
            .map_err(|_| SshUploadPackError::Io)?;
        if read == 0 {
            if filled == 0 {
                return Ok(false);
            }
            return Err(SshUploadPackError::Denied);
        }
        filled += read;
    }
    Ok(true)
}

async fn read_command_header<R: AsyncRead + Unpin>(
    source: &mut R,
) -> Result<Vec<u8>, SshUploadPackError> {
    let mut hdr = [0u8; 4];
    if !read_first_header(source, &mut hdr).await? {
        return Ok(Vec::new());
    }
    let mut cmd_buf = Vec::with_capacity(256);
    loop {
        if cmd_buf.len().saturating_add(4) > MAX_COMMAND_HEADER_BYTES {
            return Err(SshUploadPackError::Limit);
        }
        cmd_buf.extend_from_slice(&hdr);
        if &hdr == b"0000" {
            return Ok(cmd_buf);
        }
        let hex_hdr = std::str::from_utf8(&hdr).map_err(|_| SshUploadPackError::Denied)?;
        let pkt_len = usize::from_str_radix(hex_hdr, 16).map_err(|_| SshUploadPackError::Denied)?;
        if !(4..=65520).contains(&pkt_len) {
            return Err(SshUploadPackError::Denied);
        }
        let payload_len = pkt_len - 4;
        if cmd_buf.len().saturating_add(payload_len) > MAX_COMMAND_HEADER_BYTES {
            return Err(SshUploadPackError::Limit);
        }
        let start = cmd_buf.len();
        cmd_buf.resize(start + payload_len, 0);
        source
            .read_exact(&mut cmd_buf[start..])
            .await
            .map_err(|_| SshUploadPackError::Denied)?;
        source
            .read_exact(&mut hdr)
            .await
            .map_err(|_| SshUploadPackError::Denied)?;
    }
}

async fn receive_input<R, W, C>(
    mut source: R,
    mut destination: W,
    common_git_dir: PathBuf,
    command_gate: C,
) -> Result<(), SshUploadPackError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
    C: FnOnce(Vec<ReceivePackRefUpdate>) -> Result<(), ()> + Send + 'static,
{
    let cmd_buf = read_command_header(&mut source).await?;
    if cmd_buf.is_empty() {
        return destination
            .shutdown()
            .await
            .map_err(|_| SshUploadPackError::Io);
    }
    if cmd_buf == b"0000" {
        if let Err(error) = destination.write_all(&cmd_buf).await {
            // fail-open-ok: receive-pack may exit on a no-op flush; its native exit status is checked.
            if error.kind() == std::io::ErrorKind::BrokenPipe {
                return Ok(());
            }
            return Err(SshUploadPackError::Io);
        }
        return destination
            .shutdown()
            .await
            .map_err(|_| SshUploadPackError::Io);
    }
    let updates =
        crate::inspect_receive_pack_updates(&cmd_buf).map_err(|_| SshUploadPackError::Denied)?;
    let gated = tokio::task::spawn_blocking(move || {
        crate::git_hosting::validate_receive_targets(&common_git_dir, &updates)
            .map_err(|_| SshUploadPackError::Denied)?;
        command_gate(updates).map_err(|_| SshUploadPackError::Denied)
    })
    .await
    .map_err(|_| SshUploadPackError::Unavailable)?;
    gated?;
    if let Err(error) = destination.write_all(&cmd_buf).await {
        // fail-open-ok: receive-pack may exit early; its native exit status remains authoritative.
        if error.kind() == std::io::ErrorKind::BrokenPipe {
            return Ok(());
        }
        return Err(SshUploadPackError::Io);
    }
    io::input(
        source,
        destination,
        MAX_SMART_HTTP_BODY_BYTES.saturating_sub(cmd_buf.len()),
    )
    .await
}
