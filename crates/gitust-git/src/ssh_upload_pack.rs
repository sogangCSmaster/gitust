//! Read-only SSH upload-pack; an owned supervisor outlives cancelled caller futures.
use crate::{GitEngine, WorktreeServiceLock, MAX_SMART_HTTP_BODY_BYTES, SMART_HTTP_TIMEOUT};
use std::{process::Stdio, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    process::Child,
    sync::{mpsc, watch},
    task::JoinSet,
    time::{sleep, sleep_until, timeout, Instant},
};

#[path = "ssh_upload_pack_io.rs"]
pub(crate) mod io;

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SshUploadPackError {
    #[error("repository access denied")]
    Denied,
    #[error("repository data is unavailable")]
    Unavailable,
    #[error("Git transport failed")]
    Io,
    #[error("Git transport exceeded its limit")]
    Limit,
    #[error("Git transport timed out")]
    TimedOut,
    #[error("Git transport was cancelled")]
    Cancelled,
    #[error("Git upload-pack failed")]
    Failed,
    #[error("Git transport cleanup could not be confirmed")]
    CleanupIncomplete,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SshUploadPackOutcome {
    pub exit_code: u32,
}

#[derive(Clone, Copy)]
pub(crate) struct Completion {
    pub(crate) result: Result<SshUploadPackOutcome, SshUploadPackError>,
    pub(crate) cleanup: Result<(), SshUploadPackError>,
}

/// Clones can cancel a channel and await cleanup independently of its response task.
#[derive(Clone)]
pub struct SshUploadPackCancel {
    cancel: watch::Sender<bool>,
    completed: watch::Receiver<Option<Completion>>,
}

impl SshUploadPackCancel {
    pub fn cancel(&self) {
        self.cancel.send_replace(true);
    }

    pub async fn wait(&self) -> Result<(), SshUploadPackError> {
        completion(self.completed.clone()).await?.cleanup
    }
}

/// Dropping the run or its wait future requests cancellation; it never aborts the supervisor.
pub struct SshUploadPack {
    control: SshUploadPackCancel,
}

impl SshUploadPack {
    pub(crate) fn from_channels(
        cancel: watch::Sender<bool>,
        completed: watch::Receiver<Option<Completion>>,
    ) -> Self {
        Self {
            control: SshUploadPackCancel { cancel, completed },
        }
    }

    pub fn cancel_handle(&self) -> SshUploadPackCancel {
        self.control.clone()
    }

    pub async fn wait(self) -> Result<SshUploadPackOutcome, SshUploadPackError> {
        completion(self.control.completed.clone()).await?.result
    }
}

impl Drop for SshUploadPack {
    fn drop(&mut self) {
        self.control.cancel();
    }
}

async fn completion(
    mut receiver: watch::Receiver<Option<Completion>>,
) -> Result<Completion, SshUploadPackError> {
    loop {
        if let Some(value) = *receiver.borrow_and_update() {
            return Ok(value);
        }
        receiver
            .changed()
            .await
            .map_err(|_| SshUploadPackError::CleanupIncomplete)?;
    }
}

impl GitEngine {
    /// `gate` rechecks the original credential after local preparation, immediately before spawn.
    /// `keep_alive` owns the caller's registry, service and admission holders through cleanup.
    pub fn start_ssh_upload_pack<S, H, G>(
        &self,
        stream: S,
        keep_alive: H,
        gate: G,
    ) -> Result<SshUploadPack, SshUploadPackError>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
        H: Send + 'static,
        G: FnOnce() -> Result<(), ()> + Send + 'static,
    {
        let runtime =
            tokio::runtime::Handle::try_current().map_err(|_| SshUploadPackError::Unavailable)?;
        let (cancel, receiver) = watch::channel(false);
        let (completed, result) = watch::channel(None);
        runtime.spawn(supervise(
            self.clone(),
            stream,
            keep_alive,
            gate,
            receiver,
            completed,
        ));
        Ok(SshUploadPack::from_channels(cancel, result))
    }
}

pub(crate) struct Prepared {
    pub(crate) child: Child,
    pub(crate) process_group: u32,
    pub(crate) group_cleanup: Option<Result<(), SshUploadPackError>>,
    pub(crate) _service: WorktreeServiceLock,
}

impl Prepared {
    pub(crate) fn exited_without_reaping(&mut self) -> Result<bool, SshUploadPackError> {
        #[cfg(unix)]
        {
            let mut info = std::mem::MaybeUninit::<libc::siginfo_t>::zeroed();
            // SAFETY: info is writable; WNOWAIT retains this owned child's PID until group cleanup.
            let result = unsafe {
                libc::waitid(
                    libc::P_PID,
                    self.process_group as libc::id_t,
                    info.as_mut_ptr(),
                    libc::WEXITED | libc::WNOWAIT | libc::WNOHANG,
                )
            };
            if result != 0 {
                if std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
                    return Ok(false);
                }
                self.group_cleanup = Some(Err(SshUploadPackError::CleanupIncomplete));
                return Err(SshUploadPackError::CleanupIncomplete);
            }
            // SAFETY: zero-initialized siginfo remains valid on no event; waitid fills it on exit.
            Ok(unsafe { info.assume_init().si_pid() } != 0)
        }
        #[cfg(not(unix))]
        Err(SshUploadPackError::Unavailable)
    }

    pub(crate) fn stop_group(&mut self) -> Result<(), SshUploadPackError> {
        if let Some(result) = self.group_cleanup {
            return result;
        }
        let result = self.signal_group();
        self.group_cleanup = Some(result);
        result
    }

    fn signal_group(&self) -> Result<(), SshUploadPackError> {
        #[cfg(unix)]
        {
            let group = i32::try_from(self.process_group)
                .map_err(|_| SshUploadPackError::CleanupIncomplete)?;
            // SAFETY: this child was created in its own process group and the group is signalled only once.
            if unsafe { libc::killpg(group, libc::SIGKILL) } != 0
                && std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
            {
                return Err(SshUploadPackError::CleanupIncomplete);
            }
            Ok(())
        }
        #[cfg(not(unix))]
        Err(SshUploadPackError::CleanupIncomplete)
    }
}

impl Drop for Prepared {
    fn drop(&mut self) {
        // fail-open-ok: last-resort runtime-shutdown cleanup cannot publish a successful completion.
        let _ = self.stop_group();
    }
}

fn prepare<G>(
    engine: GitEngine,
    gate: G,
    cancel: watch::Receiver<bool>,
    deadline: Instant,
) -> Result<Prepared, SshUploadPackError>
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
        "transfer.fsckObjects=true",
        "-c",
        "transfer.hideRefs=refs/remotes/",
        "upload-pack",
        "--strict",
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
    gate().map_err(|_| SshUploadPackError::Denied)?;
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
    Ok(Prepared {
        child,
        process_group,
        group_cleanup: None,
        _service: service,
    })
}

#[derive(Clone, Copy)]
pub(crate) enum Pump {
    Input,
    Output,
    Stderr,
}

async fn supervise<S, H, G>(
    engine: GitEngine,
    stream: S,
    keep_alive: H,
    gate: G,
    mut cancel: watch::Receiver<bool>,
    completed: watch::Sender<Option<Completion>>,
) where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    H: Send + 'static,
    G: FnOnce() -> Result<(), ()> + Send + 'static,
{
    let deadline = Instant::now() + SMART_HTTP_TIMEOUT;
    let preparation_cancel = cancel.clone();
    let prepared =
        tokio::task::spawn_blocking(move || prepare(engine, gate, preparation_cancel, deadline))
            .await;
    let mut prepared = match prepared {
        Ok(Ok(prepared)) => prepared,
        value => {
            let error = match value {
                Ok(Err(error)) => error,
                _ => SshUploadPackError::Unavailable,
            };
            drop(stream);
            drop(keep_alive);
            completed.send_replace(Some(Completion {
                result: Err(error),
                cleanup: Ok(()),
            }));
            return;
        }
    };
    let mut pumps = JoinSet::new();
    let result = drive(&mut prepared, stream, &mut cancel, deadline, &mut pumps).await;
    let group = if result.is_ok() {
        // Native upload-pack exited and every pipe reached EOF; do not signal a finished group.
        // The unreaped leader still pins its identity until the wait below.
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
            // Continue owning the child and holders after reporting an uncertain cleanup.
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
        // A failed group signal cannot establish descendant cleanup, even after leader reap.
        let quarantine = (prepared, keep_alive);
        std::future::pending::<()>().await;
        drop(quarantine);
        return;
    }
    drop(prepared);
    drop(keep_alive);
    if cleanup_failed {
        return;
    }
    let result = result.and_then(|()| {
        let exit_code = reaped
            .map_err(|_| SshUploadPackError::CleanupIncomplete)?
            .code()
            .ok_or(SshUploadPackError::Failed)?;
        let exit_code = u32::try_from(exit_code).map_err(|_| SshUploadPackError::Failed)?;
        Ok(SshUploadPackOutcome { exit_code })
    });
    completed.send_replace(Some(Completion {
        result,
        cleanup: Ok(()),
    }));
}

pub(crate) fn publish_cleanup_failure(completed: &watch::Sender<Option<Completion>>) {
    completed.send_replace(Some(Completion {
        result: Err(SshUploadPackError::CleanupIncomplete),
        cleanup: Err(SshUploadPackError::CleanupIncomplete),
    }));
}

async fn drive<S>(
    prepared: &mut Prepared,
    stream: S,
    cancel: &mut watch::Receiver<bool>,
    deadline: Instant,
    pumps: &mut JoinSet<Pump>,
) -> Result<(), SshUploadPackError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let input = prepared.child.stdin.take().ok_or(SshUploadPackError::Io)?;
    let output = prepared.child.stdout.take().ok_or(SshUploadPackError::Io)?;
    let stderr = prepared.child.stderr.take().ok_or(SshUploadPackError::Io)?;
    let (reader, writer) = tokio::io::split(stream);
    let (failures, mut failure) = mpsc::channel(3);
    let input_failure = failures.clone();
    let input_task = pumps.spawn(async move {
        if let Err(error) = io::input(reader, input, MAX_SMART_HTTP_BODY_BYTES).await {
            // fail-open-ok: a closed receiver means the supervisor already owns cleanup.
            let _ = input_failure.send(error).await;
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn input_limit_accepts_exact_bytes_and_never_writes_the_excess() {
        for count in [8, 9] {
            let (writer, mut reader) = tokio::io::duplex(32);
            let result = io::input(&vec![b'x'; count][..], writer, 8).await;
            assert_eq!(
                result,
                if count == 8 {
                    Ok(())
                } else {
                    Err(SshUploadPackError::Limit)
                }
            );
            let mut bytes = Vec::new();
            reader
                .read_to_end(&mut bytes)
                .await
                .expect("bounded output");
            assert_eq!(bytes.len(), if count == 8 { 8 } else { 0 });
            let (writer, mut reader) = tokio::io::duplex(32);
            let (failures, mut failure) = mpsc::channel(1);
            io::output(&vec![b'x'; count][..], writer, 8, failures).await;
            let mut bytes = Vec::new();
            reader
                .read_to_end(&mut bytes)
                .await
                .expect("bounded pack output");
            assert_eq!(bytes.len(), if count == 8 { 8 } else { 0 });
            if count == 9 {
                assert_eq!(failure.try_recv(), Ok(SshUploadPackError::Limit));
            } else {
                assert!(failure.try_recv().is_err());
            }
        }
        let (mut input, output) = tokio::io::duplex(32);
        input.shutdown().await.expect("EOF");
        assert_eq!(io::input(output, tokio::io::sink(), 8).await, Ok(()));
    }
}
