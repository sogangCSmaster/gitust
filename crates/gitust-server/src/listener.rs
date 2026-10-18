//! Hardened HTTP accept loop.
//!
//! Gate B P1 (2026-10-16): `axum::serve` had no header-read timeout, no body deadline and no
//! connection cap, so idle or trickling unauthenticated sockets kept connections open forever
//! and ~2.8k idle sockets exhausted file descriptors (`EMFILE`), making the server unreachable.
//!
//! This module serves the router through hyper directly with:
//! - [`MAX_CONNECTIONS`]: a semaphore permit is taken BEFORE `accept()`, so excess clients wait in
//!   the kernel backlog instead of consuming process file descriptors;
//! - [`HEADER_READ_TIMEOUT`]: hyper closes connections that do not send complete request headers
//!   in time (slow-loris headers, idle connections);
//! - [`BODY_READ_DEADLINE`]: a total wall-clock deadline for reading each request body
//!   ([`DeadlineBody`]), so a body trickled one byte at a time cannot pin a handler.

use std::collections::HashMap;
use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use axum::body::Body;
use axum::extract::{ConnectInfo, Request};
use axum::Router;
use bytes::Bytes;
use http_body::{Body as _, Frame, SizeHint};
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper_util::rt::{TokioIo, TokioTimer};
use hyper_util::service::TowerToHyperService;
use tokio::net::TcpListener;
use tokio::sync::{watch, Semaphore};
use tokio::task::JoinSet;
use tokio::time::Sleep;

/// Maximum concurrently served TCP connections.
pub const MAX_CONNECTIONS: usize = 512;
/// Maximum concurrent connections from one client IP, so a single source cannot occupy every
/// slot in [`MAX_CONNECTIONS`].
pub const MAX_CONNECTIONS_PER_IP: usize = 64;
/// Maximum time for a client to deliver complete request headers.
pub const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(10);
/// Maximum total time to receive a request body.
pub const BODY_READ_DEADLINE: Duration = Duration::from_secs(30);
const ACCEPT_ERROR_BACKOFF: Duration = Duration::from_millis(100);
const SHUTDOWN_DEADLINE: Duration = Duration::from_secs(30);

/// Request body wrapper that fails once a total wall-clock deadline has passed.
pub(crate) struct DeadlineBody {
    inner: Body,
    deadline: Pin<Box<Sleep>>,
}

impl DeadlineBody {
    pub(crate) fn new(inner: Body, limit: Duration) -> Self {
        Self {
            inner,
            deadline: Box::pin(tokio::time::sleep(limit)),
        }
    }
}

impl http_body::Body for DeadlineBody {
    type Data = Bytes;
    type Error = axum::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let this = self.get_mut();
        if this.deadline.as_mut().poll(cx).is_ready() {
            return Poll::Ready(Some(Err(axum::Error::new(
                "request body was not received before the deadline",
            ))));
        }
        Pin::new(&mut this.inner).poll_frame(cx)
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

/// Per-IP connection slot; released on drop.
struct IpSlot {
    ip: IpAddr,
    table: Arc<parking_lot::Mutex<HashMap<IpAddr, usize>>>,
}

impl IpSlot {
    fn try_acquire(
        table: &Arc<parking_lot::Mutex<HashMap<IpAddr, usize>>>,
        ip: IpAddr,
        max_per_ip: usize,
    ) -> Option<Self> {
        let mut guard = table.lock();
        let count = guard.entry(ip).or_insert(0);
        if *count >= max_per_ip {
            return None;
        }
        *count += 1;
        Some(Self {
            ip,
            table: Arc::clone(table),
        })
    }
}

impl Drop for IpSlot {
    fn drop(&mut self) {
        let mut guard = self.table.lock();
        if let Some(count) = guard.get_mut(&self.ip) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                guard.remove(&self.ip);
            }
        }
    }
}

/// Router middleware: applies [`BODY_READ_DEADLINE`] to every request body.
pub(crate) async fn apply_body_deadline(req: Request) -> Request {
    if req.body().is_end_stream() {
        return req;
    }
    req.map(|body| Body::new(DeadlineBody::new(body, BODY_READ_DEADLINE)))
}

/// Connection-level limits for [`serve_hardened_with`].
#[derive(Debug, Clone, Copy)]
pub struct ServeLimits {
    pub max_connections: usize,
    pub max_connections_per_ip: usize,
    pub header_read_timeout: Duration,
}

impl Default for ServeLimits {
    fn default() -> Self {
        Self {
            max_connections: MAX_CONNECTIONS,
            max_connections_per_ip: MAX_CONNECTIONS_PER_IP,
            header_read_timeout: HEADER_READ_TIMEOUT,
        }
    }
}

/// Serves `app` on `listener` with the default [`ServeLimits`].
pub async fn serve_hardened(listener: TcpListener, app: Router) -> std::io::Result<()> {
    serve_hardened_with(listener, app, ServeLimits::default()).await
}

/// Serves `app` on `listener` with connection caps and a header-read timeout.
pub async fn serve_hardened_with(
    listener: TcpListener,
    app: Router,
    limits: ServeLimits,
) -> std::io::Result<()> {
    serve_hardened_until(listener, app, limits, std::future::pending()).await
}

struct StopOnDrop(watch::Sender<bool>);
impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.send_replace(true);
    }
}

/// Closing network connections does not claim that detached blocking operations have completed.
pub(crate) async fn serve_hardened_until(
    listener: TcpListener,
    app: Router,
    limits: ServeLimits,
    shutdown: impl Future<Output = ()>,
) -> std::io::Result<()> {
    let (stop, receiver) = watch::channel(false);
    let _guard = StopOnDrop(stop.clone());
    // The supervisor owns connections through cleanup even when its caller is cancelled.
    let mut running = tokio::spawn(run(listener, app, limits, stop.clone(), receiver));
    tokio::pin!(shutdown);
    let result = tokio::select! {
        result = &mut running => return result.map_err(|_| std::io::Error::other("HTTP listener task failed"))?,
        // fail-open-ok: explicit shutdown stops admission and awaits the supervisor; no request error is discarded.
        _ = &mut shutdown => { stop.send_replace(true); running.await },
    };
    result.map_err(|_| std::io::Error::other("HTTP listener task failed"))?
}

async fn stopped(receiver: &mut watch::Receiver<bool>) {
    while !*receiver.borrow_and_update() {
        if receiver.changed().await.is_err() {
            return;
        }
    }
}

async fn run(
    listener: TcpListener,
    app: Router,
    limits: ServeLimits,
    stop: watch::Sender<bool>,
    mut receiver: watch::Receiver<bool>,
) -> std::io::Result<()> {
    let permits = Arc::new(Semaphore::new(limits.max_connections.max(1)));
    let per_ip = Arc::new(parking_lot::Mutex::new(HashMap::new()));
    let mut builder = http1::Builder::new();
    builder
        .timer(TokioTimer::new())
        .header_read_timeout(limits.header_read_timeout)
        .keep_alive(true);

    let mut connections = JoinSet::new();
    let mut result = loop {
        // Take the permit before accepting so excess clients never get a process fd.
        let accepted = tokio::select! {
            biased;
            // fail-open-ok: explicit stop ends admission; all owned connections are drained and joined below.
            _ = stopped(&mut receiver) => break Ok(()),
            completed = connections.join_next(), if !connections.is_empty() => {
                if completed.is_some_and(|value| value.is_err()) {
                    break Err(std::io::Error::other("HTTP connection task failed"));
                }
                continue;
            }
            accepted = async {
                let permit = Arc::clone(&permits).acquire_owned().await
                    .map_err(std::io::Error::other)?;
                let (stream, addr) = listener.accept().await?;
                Ok::<_, std::io::Error>((stream, addr, permit))
            } => accepted,
        };
        let (stream, addr, permit) = match accepted {
            Ok(value) => value,
            Err(error) => {
                tracing::warn!("accept error: {error}");
                tokio::select! {
                    // fail-open-ok: stop ends admission and proceeds to owned connection cleanup.
                    _ = stopped(&mut receiver) => break Ok(()),
                    // fail-open-ok: backoff only retries a failed accept; no request or authorization is skipped.
                    _ = tokio::time::sleep(ACCEPT_ERROR_BACKOFF) => continue,
                }
            }
        };
        if let Err(error) = stream.set_nodelay(true) {
            break Err(error);
        }
        let Some(ip_slot) = IpSlot::try_acquire(&per_ip, addr.ip(), limits.max_connections_per_ip)
        else {
            tracing::debug!("per-IP connection limit reached for {}", addr.ip());
            drop(stream);
            continue;
        };
        let svc =
            tower::util::MapRequest::new(app.clone(), move |mut req: hyper::Request<Incoming>| {
                req.extensions_mut().insert(ConnectInfo::<SocketAddr>(addr));
                req
            });
        let conn_builder = builder.clone();
        let mut closing = receiver.clone();
        connections.spawn(async move {
            let _permit = permit;
            let _ip_slot = ip_slot;
            let conn =
                conn_builder.serve_connection(TokioIo::new(stream), TowerToHyperService::new(svc));
            tokio::pin!(conn);
            let result = tokio::select! {
                result = &mut conn => result,
                // fail-open-ok: stop disables keep-alive; the connection result is still awaited below.
                _ = stopped(&mut closing) => {
                    conn.as_mut().graceful_shutdown();
                    conn.await
                }
            };
            if let Err(e) = result {
                tracing::debug!("connection from {addr} closed: {e}");
            }
        });
    };
    drop(listener);
    stop.send_replace(true);
    let drain = async {
        while let Some(connection) = connections.join_next().await {
            if connection.is_err() {
                result = Err(std::io::Error::other("HTTP connection cleanup failed"));
            }
        }
    };
    if tokio::time::timeout(SHUTDOWN_DEADLINE, drain)
        .await
        .is_err()
    {
        tracing::warn!("HTTP graceful shutdown deadline reached; closing remaining connections");
        connections.abort_all();
        while let Some(connection) = connections.join_next().await {
            if connection.is_err_and(|error| !error.is_cancelled()) {
                result = Err(std::io::Error::other("HTTP connection cleanup failed"));
            }
        }
    }
    result
}
