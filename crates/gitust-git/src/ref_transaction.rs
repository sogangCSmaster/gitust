//! Direct-ref CAS checked by Git while its real ref locks are held.
use std::{
    collections::BTreeSet,
    fs::OpenOptions,
    io::{BufRead, BufReader, Read, Write},
    process::{Child, ChildStdin, ChildStdout, Stdio},
    sync::mpsc::{self, Receiver, SyncSender},
    thread::JoinHandle,
    time::{Duration, Instant},
};

use gix::refs::{
    transaction::{Change, LogChange, PreviousValue, RefEdit, RefLog},
    Target,
};

use crate::{
    process::{capture, git_command, terminate_group},
    GitEngine, GitEngineError, MAX_BRANCH_NAME_BYTES,
};

const MAX_INPUT: usize = 64 * 1024 * 1024;
const MAX_EDITS: usize = 100_000;
const DEADLINE: Duration = Duration::from_secs(30);
const ABORT_GRACE: Duration = Duration::from_secs(2);

fn failed() -> GitEngineError {
    GitEngineError::RefTransaction(
        "Git ref transaction could not be confirmed; inspect the current refs before retrying"
            .into(),
    )
}

pub(crate) fn update_with_gate(
    repo: &gix::Repository,
    name: &str,
    target: gix::ObjectId,
    expected: PreviousValue,
    message: impl Into<gix::bstr::BString>,
    gate: impl FnOnce() -> Result<(), GitEngineError>,
) -> Result<(), GitEngineError> {
    let name = name.try_into().map_err(|_| failed())?;
    apply_with_gate(
        repo,
        vec![RefEdit {
            name,
            deref: false,
            change: Change::Update {
                expected,
                new: Target::Object(target),
                log: LogChange {
                    mode: RefLog::AndReference,
                    force_create_reflog: false,
                    message: message.into(),
                },
            },
        }],
        gate,
    )
}

pub(crate) fn apply(repo: &gix::Repository, edits: Vec<RefEdit>) -> Result<(), GitEngineError> {
    apply_with_gate(repo, edits, || Ok(()))
}

fn encode(edits: &[RefEdit]) -> Result<(Vec<u8>, String), GitEngineError> {
    if edits.len() > MAX_EDITS {
        return Err(failed());
    }
    let mut bytes = b"start\0".to_vec();
    let mut names = BTreeSet::new();
    let mut messages = BTreeSet::new();
    for edit in edits {
        let name = std::str::from_utf8(edit.name.as_bstr()).map_err(|_| failed())?;
        if edit.deref
            || !(name.starts_with("refs/heads/") || name.starts_with("refs/tags/"))
            || name.len() > MAX_BRANCH_NAME_BYTES + 11
            || !names.insert(name)
        {
            return Err(failed());
        }
        let (command, new, expected) = match &edit.change {
            Change::Update {
                log,
                expected,
                new: Target::Object(new),
            } if log.mode == RefLog::AndReference && !log.force_create_reflog && !new.is_null() => {
                let message = std::str::from_utf8(&log.message).map_err(|_| failed())?;
                if message.len() > 64 * 1024 || message.contains(['\0', '\n', '\r']) {
                    return Err(failed());
                }
                messages.insert(message);
                (
                    if matches!(expected, PreviousValue::MustNotExist) {
                        "create "
                    } else {
                        "update "
                    },
                    Some(new),
                    expected,
                )
            }
            Change::Delete {
                expected,
                log: RefLog::AndReference,
            } => ("delete ", None, expected),
            _ => return Err(failed()),
        };
        let previous = match expected {
            PreviousValue::MustNotExist if new.is_some() => None,
            PreviousValue::MustExistAndMatch(Target::Object(oid)) if !oid.is_null() => {
                Some(oid.to_string())
            }
            _ => return Err(failed()),
        };
        let new = new.map(ToString::to_string);
        if bytes.len().saturating_add(name.len()).saturating_add(256) > MAX_INPUT {
            return Err(failed());
        }
        bytes.extend_from_slice(command.as_bytes());
        bytes.extend_from_slice(name.as_bytes());
        bytes.push(0);
        if let Some(new) = new {
            bytes.extend_from_slice(new.as_bytes());
            bytes.push(0);
        }
        if let Some(previous) = previous {
            bytes.extend_from_slice(previous.as_bytes());
            bytes.push(0);
        }
    }
    bytes.extend_from_slice(b"prepare\0");
    let message = if messages.len() == 1 {
        messages.into_iter().next().ok_or_else(failed)?.to_owned()
    } else {
        "gitust sync: apply inbound mirror refs".into()
    };
    Ok((bytes, message))
}

/// Git's no-deref protects the referent but may replace a symbolic ref with an object.
/// Check the raw loose kind after prepare, under Git's locks; packed refs are direct.
fn validate_locked(
    repo: &gix::Repository,
    edits: &[RefEdit],
    started: Instant,
) -> Result<(), GitEngineError> {
    let common = GitEngine::common_git_dir(repo);
    for edit in edits {
        if started.elapsed() >= DEADLINE {
            return Err(failed());
        }
        let name = std::str::from_utf8(edit.name.as_bstr()).map_err(|_| failed())?;
        let path = common.join(name);
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let file = match options.open(&path) {
            Ok(file) => file,
            // Git already checked packed/missing values under its own lock in prepare.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            // fail-open-ok: a failed locked-ref read aborts publication; only private diagnostic detail is discarded.
            Err(_) => return Err(failed()),
        };
        let metadata = file.metadata().map_err(|_| failed())?;
        if !metadata.is_file() || metadata.len() > (MAX_BRANCH_NAME_BYTES + 128) as u64 {
            return Err(failed());
        }
        let mut bytes = Vec::new();
        file.take((MAX_BRANCH_NAME_BYTES + 129) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| failed())?;
        let reference = gix::refs::file::loose::Reference::try_from_path(edit.name.clone(), &bytes)
            .map_err(|_| failed())?;
        let expected = match &edit.change {
            Change::Update { expected, .. } | Change::Delete { expected, .. } => expected,
        };
        if !matches!((expected, &reference.target),
            (PreviousValue::MustExistAndMatch(Target::Object(old)), Target::Object(actual)) if old == actual)
        {
            return Err(failed());
        }
    }
    Ok(())
}

pub(crate) fn apply_with_gate(
    repo: &gix::Repository,
    edits: Vec<RefEdit>,
    gate: impl FnOnce() -> Result<(), GitEngineError>,
) -> Result<(), GitEngineError> {
    if edits.is_empty() {
        return gate();
    }
    if repo.namespace().is_some() {
        return Err(failed());
    }
    let (input, message) = encode(&edits)?;
    let signature = repo.committer().transpose().map_err(|_| failed())?;
    let mut command = git_command(repo.path());
    command.args([
        "-c",
        "core.filesRefLockTimeout=100",
        "-c",
        "core.packedRefsTimeout=100",
        "update-ref",
        "--stdin",
        "-z",
        "--no-deref",
        "-m",
        &message,
    ]);
    if let Some(signature) = signature {
        command
            .env(
                "GIT_COMMITTER_NAME",
                std::str::from_utf8(signature.name).map_err(|_| failed())?,
            )
            .env(
                "GIT_COMMITTER_EMAIL",
                std::str::from_utf8(signature.email).map_err(|_| failed())?,
            );
    }
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let child = command.spawn().map_err(|_| failed())?;
    let mut process = PreparedProcess::new(child, input)?;
    process.ready.recv_timeout(DEADLINE).map_err(|_| failed())?;
    validate_locked(repo, &edits, process.started)?;
    gate()?;
    process.commit()
}

fn acknowledgement(reader: &mut BufReader<ChildStdout>, expected: &[u8]) -> std::io::Result<()> {
    let mut line = Vec::new();
    reader.by_ref().take(129).read_until(b'\n', &mut line)?;
    if line != expected {
        return Err(std::io::Error::other("unexpected ref transaction response"));
    }
    Ok(())
}

fn protocol(
    mut input: ChildStdin,
    output: ChildStdout,
    bytes: Vec<u8>,
    ready: SyncSender<()>,
    decision: Receiver<bool>,
) -> std::io::Result<()> {
    let mut output = BufReader::new(output);
    input.write_all(&bytes)?;
    input.flush()?;
    acknowledgement(&mut output, b"start: ok\n")?;
    acknowledgement(&mut output, b"prepare: ok\n")?;
    ready
        .send(())
        .map_err(|_| std::io::Error::other("transaction receiver closed"))?;
    // fail-open-ok: a disconnected decision channel defaults to false, which aborts and cannot publish refs.
    let commit = decision.recv().unwrap_or_default();
    input.write_all(if commit { b"commit\0" } else { b"abort\0" })?;
    input.flush()?;
    drop(input);
    acknowledgement(
        &mut output,
        if commit {
            b"commit: ok\n"
        } else {
            b"abort: ok\n"
        },
    )?;
    std::io::copy(&mut output, &mut std::io::sink())?;
    Ok(())
}

type Capture = JoinHandle<std::io::Result<(Vec<u8>, bool)>>;
struct PreparedProcess {
    child: Child,
    ready: Receiver<()>,
    decision: Option<SyncSender<bool>>,
    worker: Option<JoinHandle<std::io::Result<()>>>,
    stderr: Option<Capture>,
    started: Instant,
    reaped: bool,
    group_cleaned: bool,
}
impl PreparedProcess {
    fn new(child: Child, bytes: Vec<u8>) -> Result<Self, GitEngineError> {
        let (ready_sender, ready) = mpsc::sync_channel(1);
        let (decision, receiver) = mpsc::sync_channel(1);
        let mut process = Self {
            child,
            ready,
            decision: Some(decision),
            worker: None,
            stderr: None,
            started: Instant::now(),
            reaped: false,
            group_cleaned: false,
        };
        let input = process.child.stdin.take().ok_or_else(failed)?;
        let output = process.child.stdout.take().ok_or_else(failed)?;
        let stderr = process.child.stderr.take().ok_or_else(failed)?;
        process.stderr = Some(std::thread::spawn(move || capture(stderr, 8192)));
        process.worker = Some(std::thread::spawn(move || {
            protocol(input, output, bytes, ready_sender, receiver)
        }));
        Ok(process)
    }
    fn commit(&mut self) -> Result<(), GitEngineError> {
        if self.started.elapsed() >= DEADLINE {
            return Err(failed());
        }
        self.decision
            .take()
            .ok_or_else(failed)?
            .try_send(true)
            .map_err(|_| failed())?;
        let status = loop {
            match self.child.try_wait() {
                Ok(Some(status)) => {
                    self.reaped = true;
                    break status;
                }
                Ok(None) if self.started.elapsed() < DEADLINE => {
                    std::thread::sleep(Duration::from_micros(200))
                }
                _ => return Err(failed()),
            }
        };
        // Reap inherited pipe owners before joining readers, even if Git itself exited.
        self.cleanup_group();
        self.worker
            .take()
            .ok_or_else(failed)?
            .join()
            .map_err(|_| failed())?
            .map_err(|_| failed())?;
        self.stderr
            .take()
            .ok_or_else(failed)?
            .join()
            .map_err(|_| failed())?
            .map_err(|_| failed())?;
        if !status.success() {
            return Err(failed());
        }
        Ok(())
    }

    fn cleanup_group(&mut self) {
        if !self.group_cleaned {
            terminate_group(&mut self.child);
            self.group_cleaned = true;
        }
    }
}
impl Drop for PreparedProcess {
    fn drop(&mut self) {
        // Closing the decision channel asks the worker to abort a prepared transaction.
        // Give Git time to remove its own locks before resorting to termination.
        self.decision.take();
        let abort_started = Instant::now();
        while !self.reaped && abort_started.elapsed() < ABORT_GRACE {
            match self.child.try_wait() {
                Ok(Some(_)) => self.reaped = true,
                Ok(None) => std::thread::sleep(Duration::from_millis(5)),
                Err(_) => break,
            }
        }
        // Git has either finished its graceful abort or exceeded its cleanup bound.
        // Descendants must not retain stdout/stderr and leave the joins below blocked.
        self.cleanup_group();
        if !self.reaped {
            // fail-open-ok: cleanup after an already failed/unconfirmed transaction cannot grant publication success.
            let _ = self.child.kill();
            // fail-open-ok: waiting is cleanup only; a failure above remains an error and is never retried unconditionally.
            let _ = self.child.wait();
        }
        if let Some(worker) = self.worker.take() {
            // fail-open-ok: the transaction already failed; collect its protocol worker after closing/terminating the child.
            let _ = worker.join();
        }
        if let Some(stderr) = self.stderr.take() {
            // fail-open-ok: discard captured diagnostic output only after draining and collecting the failed child.
            let _ = stderr.join();
        }
    }
}
