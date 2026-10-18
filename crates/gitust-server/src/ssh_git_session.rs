use crate::{ssh_git_stream, ssh_git_target, AppState};
use gitust_db::AuthenticatedSshKey;
use gitust_git::{SshUploadPack, SshUploadPackCancel};
use parking_lot::Mutex;
use russh::{
    keys::{Algorithm, PublicKey},
    server::{self, Auth, Handler, Session},
    Channel, ChannelId, ChannelStream,
};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{
    sync::{watch, OwnedSemaphorePermit},
    task::JoinHandle,
};

struct Job {
    cancel: SshUploadPackCancel,
    response: JoinHandle<()>,
}
pub(crate) struct Control {
    pub(crate) closing: Arc<AtomicBool>,
    pub(crate) authenticated: watch::Sender<bool>,
    job: Mutex<Option<Job>>,
}
impl Control {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            closing: Arc::new(AtomicBool::new(false)),
            authenticated: watch::channel(false).0,
            job: Mutex::new(None),
        })
    }
    pub(crate) fn cancel(&self) {
        self.closing.store(true, Ordering::Release);
        if let Some(job) = &*self.job.lock() {
            job.cancel.cancel();
        }
    }
    pub(crate) async fn finish(&self) -> std::io::Result<()> {
        self.cancel();
        let job = self.job.lock().take();
        if let Some(job) = job {
            let cleanup = job.cancel.wait().await;
            job.response
                .await
                .map_err(|_| std::io::Error::other("SSH response task failed"))?;
            cleanup.map_err(|_| std::io::Error::other("SSH Git cleanup could not be confirmed"))?;
        }
        Ok(())
    }
    fn start(
        &self,
        run: SshUploadPack,
        channel: ChannelId,
        handle: server::Handle,
        lease: ssh_git_stream::Lease,
    ) {
        let cancel = run.cancel_handle();
        let response = tokio::spawn(async move {
            let code = match run.wait().await {
                Ok(outcome) => outcome.exit_code,
                Err(_) => 1,
            };
            // The client may have disconnected. Cleanup is already complete or quarantined;
            // delivery failure cannot turn a failed run into a successful one.
            let sent = tokio::time::timeout(Duration::from_secs(2), async {
                handle.exit_status_request(channel, code).await?;
                handle.eof(channel).await?;
                handle.close(channel).await
            })
            .await;
            if !matches!(sent, Ok(Ok(()))) {
                tracing::debug!("SSH result could not be delivered to the client");
            }
            drop(lease);
        });
        *self.job.lock() = Some(Job { cancel, response });
    }
}

pub(crate) struct GitSession {
    state: AppState,
    control: Arc<Control>,
    admission: Arc<OwnedSemaphorePermit>,
    binding: Option<AuthenticatedSshKey>,
    opened: bool,
    stream: Option<(ChannelId, ChannelStream<server::Msg>)>,
}
impl GitSession {
    pub(crate) fn new(
        state: AppState,
        control: Arc<Control>,
        admission: Arc<OwnedSemaphorePermit>,
    ) -> Self {
        Self {
            state,
            control,
            admission,
            binding: None,
            opened: false,
            stream: None,
        }
    }
    fn allowed_key(&self, user: &str, key: &PublicKey) -> bool {
        user == "git"
            && key.algorithm() == Algorithm::Ed25519
            && !self.control.closing.load(Ordering::Acquire)
    }
    fn refused(channel: ChannelId, session: &mut Session) -> Result<(), russh::Error> {
        session.channel_failure(channel)?;
        session.close(channel)
    }
}
impl Handler for GitSession {
    type Error = russh::Error;

    async fn auth_publickey_offered(
        &mut self,
        user: &str,
        key: &PublicKey,
    ) -> Result<Auth, Self::Error> {
        // An offer is not proof of possession and never creates an authenticated binding.
        Ok(if self.allowed_key(user, key) {
            Auth::Accept
        } else {
            Auth::reject()
        })
    }
    async fn auth_publickey(&mut self, user: &str, key: &PublicKey) -> Result<Auth, Self::Error> {
        if !self.allowed_key(user, key) {
            return Ok(Auth::reject());
        }
        // fail-open-ok: an encoding error explicitly rejects authentication.
        let Ok(encoded) = key.to_openssh() else {
            return Ok(Auth::reject());
        };
        let store = self.state.auth_store.clone();
        let binding =
            tokio::task::spawn_blocking(move || store.authenticate_ssh_key_binding(&encoded)).await;
        if self.control.closing.load(Ordering::Acquire) {
            return Ok(Auth::reject());
        }
        match binding {
            Ok(Ok(binding)) => {
                self.binding = Some(binding);
                self.control.authenticated.send_replace(true);
                Ok(Auth::Accept)
            }
            _ => Ok(Auth::reject()),
        }
    }
    // Certificates, passwords, keyboard interaction and forwarded channels retain the
    // library's rejecting defaults. Only a plain Ed25519 public key can reach the callback above.
    async fn channel_open_session(
        &mut self,
        channel: Channel<server::Msg>,
        reply: server::ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        if self.opened || self.binding.is_none() || self.control.closing.load(Ordering::Acquire) {
            reply
                .reject(russh::ChannelOpenFailure::AdministrativelyProhibited)
                .await;
            return Ok(());
        }
        self.opened = true;
        self.stream = Some((channel.id(), channel.into_stream()));
        reply.accept().await;
        Ok(())
    }
    async fn exec_request(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let Some((id, stream)) = self.stream.take() else {
            return Self::refused(channel, session);
        };
        let Some(binding) = self.binding.clone() else {
            return Self::refused(channel, session);
        };
        // fail-open-ok: unsupported commands are refused and the channel is closed.
        let Ok(target_id) = ssh_git_target::command_target(data) else {
            return Self::refused(channel, session);
        };
        if channel != id {
            return Self::refused(channel, session);
        }
        let target = ssh_git_target::prepare(
            self.state.clone(),
            binding,
            target_id,
            self.control.closing.clone(),
            self.admission.clone(),
        )
        .await;
        // fail-open-ok: unavailable or unauthorized targets are refused without spawning Git.
        let Ok(target) = target else {
            return Self::refused(channel, session);
        };
        if self.control.closing.load(Ordering::Acquire) {
            return Self::refused(channel, session);
        }
        let engine = target.engine();
        let authorization = target.authorization.clone();
        let (stream, lease) = ssh_git_stream::Stream::retain(stream);
        // fail-open-ok: supervisor setup failure explicitly refuses the channel.
        let Ok(run) = engine.start_ssh_upload_pack(stream, target, move || authorization.check())
        else {
            return Self::refused(channel, session);
        };
        self.control.start(run, channel, session.handle(), lease);
        session.channel_success(channel)
    }
    async fn channel_close(
        &mut self,
        _channel: ChannelId,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.control.cancel();
        self.stream.take();
        Ok(())
    }
    // Client EOF is only the end of Git stdin. ChannelStream delivers it to the input pump;
    // upload-pack must still be allowed to finish its stdout and native exit status.
    async fn env_request(
        &mut self,
        channel: ChannelId,
        _name: &str,
        _value: &str,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.channel_failure(channel)
    }
    async fn shell_request(
        &mut self,
        channel: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        Self::refused(channel, session)
    }
    async fn subsystem_request(
        &mut self,
        channel: ChannelId,
        _name: &str,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        Self::refused(channel, session)
    }
    #[allow(clippy::too_many_arguments)]
    async fn pty_request(
        &mut self,
        channel: ChannelId,
        _term: &str,
        _cols: u32,
        _rows: u32,
        _width: u32,
        _height: u32,
        _modes: &[(russh::Pty, u32)],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.channel_failure(channel)
    }
}
