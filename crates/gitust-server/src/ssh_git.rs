//! Optional read-only SSH listener with owned session and Git cleanup lifetimes.
use crate::{
    ssh_git_host_key,
    ssh_git_session::{Control, GitSession},
    AppState,
};
use russh::{keys::Algorithm, server, MethodKind, MethodSet};
use std::{
    borrow::Cow,
    io,
    net::{Shutdown, SocketAddr},
    sync::Arc,
    time::Duration,
};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::{watch, Semaphore},
    task::{JoinHandle, JoinSet},
    time::{sleep_until, Instant},
};

const AUTH_DEADLINE: Duration = Duration::from_secs(15);
const CONNECTION_DEADLINE: Duration = Duration::from_secs(120);
const CONNECTIONS: usize = 8;

pub(crate) struct Listener {
    tcp: TcpListener,
    state: AppState,
    config: Arc<server::Config>,
}
impl Listener {
    pub(crate) async fn bind(addr: SocketAddr, state: AppState) -> io::Result<Self> {
        let engine = state.git_engine.clone();
        let key = tokio::task::spawn_blocking(move || ssh_git_host_key::load(&engine))
            .await
            .map_err(|_| io::Error::other("SSH host key loading failed"))??;
        let config = server::Config {
            keys: vec![key],
            methods: MethodSet::from(&[MethodKind::PublicKey][..]),
            preferred: russh::Preferred {
                key: Cow::Owned(vec![Algorithm::Ed25519]),
                ..Default::default()
            },
            max_auth_attempts: 3,
            auth_rejection_time: Duration::from_millis(500),
            auth_rejection_time_initial: Some(Duration::from_millis(500)),
            inactivity_timeout: Some(Duration::from_secs(30)),
            window_size: 256 * 1024,
            maximum_packet_size: 32 * 1024,
            channel_buffer_size: 4,
            event_buffer_size: 8,
            ..Default::default()
        };
        let tcp = TcpListener::bind(addr).await?;
        Ok(Self {
            tcp,
            state,
            config: Arc::new(config),
        })
    }
    pub(crate) fn local_addr(&self) -> io::Result<SocketAddr> {
        self.tcp.local_addr()
    }
    pub(crate) fn start(self) -> Running {
        let (stop, receiver) = watch::channel(false);
        let inner_stop = stop.clone();
        Running {
            stop,
            task: Some(tokio::spawn(self.run(receiver, inner_stop))),
        }
    }
    async fn run(
        self,
        mut stop: watch::Receiver<bool>,
        notify: watch::Sender<bool>,
    ) -> io::Result<()> {
        let permits = Arc::new(Semaphore::new(CONNECTIONS));
        let mut sessions = JoinSet::new();
        let result = loop {
            tokio::select! {
                // fail-open-ok: explicit shutdown stops admission and joins all connections below.
                _ = stopped(&mut stop) => break Ok(()),
                finished = sessions.join_next(), if !sessions.is_empty() => {
                    match finished {
                        Some(Ok(Ok(()))) => {},
                        Some(Ok(Err(error))) => break Err(error),
                        _ => break Err(io::Error::other("SSH connection task failed")),
                    }
                }
                accepted = async {
                    let permit = permits.clone().acquire_owned().await.map_err(io::Error::other)?;
                    let (stream, _) = self.tcp.accept().await?;
                    Ok::<_, io::Error>((stream, Arc::new(permit)))
                } => {
                    let (stream, permit) = match accepted { Ok(value) => value, Err(error) => break Err(error) };
                    sessions.spawn(connection(stream, self.state.clone(), self.config.clone(), stop.clone(), permit));
                }
            }
        };
        drop(self.tcp);
        notify.send_replace(true);
        let mut cleanup = Ok(());
        while let Some(session) = sessions.join_next().await {
            match session {
                Ok(Ok(())) => {}
                Ok(Err(error)) => cleanup = Err(error),
                Err(_) => cleanup = Err(io::Error::other("SSH connection cleanup failed")),
            }
        }
        result.and(cleanup)
    }
}

pub(crate) struct Running {
    stop: watch::Sender<bool>,
    task: Option<JoinHandle<io::Result<()>>>,
}
impl Running {
    pub(crate) async fn finished(&mut self) -> io::Result<()> {
        let result = match self.task.as_mut() {
            Some(task) => task
                .await
                .map_err(|_| io::Error::other("SSH listener task failed"))?,
            None => return Ok(()),
        };
        self.task.take();
        result
    }
    pub(crate) async fn shutdown(mut self) -> io::Result<()> {
        self.stop.send_replace(true);
        self.finished().await
    }
}
impl Drop for Running {
    fn drop(&mut self) {
        self.stop.send_replace(true);
    }
}

async fn stopped(receiver: &mut watch::Receiver<bool>) {
    while !*receiver.borrow_and_update() {
        if receiver.changed().await.is_err() {
            return;
        }
    }
}
async fn authenticated(receiver: &mut watch::Receiver<bool>) {
    while !*receiver.borrow_and_update() {
        if receiver.changed().await.is_err() {
            return;
        }
    }
}

struct ConnectionOwner {
    socket: std::net::TcpStream,
    control: Arc<Control>,
}
impl ConnectionOwner {
    fn close(&self) {
        self.control.cancel();
        if let Err(error) = self.socket.shutdown(Shutdown::Both) {
            // Already closed sockets need no further action; unexpected errors are reported.
            if error.kind() != io::ErrorKind::NotConnected {
                tracing::warn!("SSH socket shutdown could not be confirmed");
            }
        }
    }
}
impl Drop for ConnectionOwner {
    fn drop(&mut self) {
        self.close();
    }
}

async fn connection(
    stream: TcpStream,
    state: AppState,
    config: Arc<server::Config>,
    mut stop: watch::Receiver<bool>,
    admission: Arc<tokio::sync::OwnedSemaphorePermit>,
) -> io::Result<()> {
    stream.set_nodelay(true)?;
    let socket = stream.into_std()?;
    let control = Control::new();
    let owner = ConnectionOwner {
        socket: socket.try_clone()?,
        control: control.clone(),
    };
    let stream = TcpStream::from_std(socket)?;
    let started = Instant::now();
    let auth_deadline = started + AUTH_DEADLINE;
    let session = GitSession::new(state, control.clone(), admission.clone());
    let handshake = server::run_stream(config, stream, session);
    tokio::pin!(handshake);
    let running = tokio::select! {
        result = &mut handshake => result,
        // fail-open-ok: a deadline closes the socket before joining the pending handshake.
        _ = sleep_until(auth_deadline) => { owner.close(); handshake.await },
        // fail-open-ok: stop closes the socket and joins; it never grants authentication.
        _ = stopped(&mut stop) => { owner.close(); handshake.await },
    };
    // fail-open-ok: a rejected/disconnected handshake ends only this connection; Git cleanup is independently checked below.
    if let Ok(running) = running {
        tokio::pin!(running);
        let mut auth = control.authenticated.subscribe();
        let authenticated = authenticated(&mut auth);
        tokio::pin!(authenticated);
        let mut signed_in = false;
        loop {
            tokio::select! {
                // fail-open-ok: protocol errors end this connection, followed by checked native cleanup.
                _ = &mut running => break,
                // fail-open-ok: only the verified-key callback sets this signal; command authorization remains independent.
                _ = &mut authenticated, if !signed_in => signed_in = true,
                // fail-open-ok: timeout closes the socket and joins the protocol task before native cleanup.
                _ = sleep_until(auth_deadline), if !signed_in => { owner.close(); let _closed = running.await; break; },
                // fail-open-ok: the whole-connection timeout also cancels the Git job and joins below.
                _ = sleep_until(started + CONNECTION_DEADLINE) => { owner.close(); let _closed = running.await; break; },
                // fail-open-ok: shutdown rejects further commands, closes the socket and joins both tasks.
                _ = stopped(&mut stop) => { owner.close(); let _closed = running.await; break; },
            }
        }
    }
    owner.close();
    // Neither a timeout nor a dropped listener aborts the owned Git supervisor.
    control.finish().await
}
