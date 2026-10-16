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
use tokio::sync::Semaphore;
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
    let permits = Arc::new(Semaphore::new(limits.max_connections.max(1)));
    let per_ip = Arc::new(parking_lot::Mutex::new(HashMap::new()));
    let mut builder = http1::Builder::new();
    builder
        .timer(TokioTimer::new())
        .header_read_timeout(limits.header_read_timeout)
        .keep_alive(true);

    loop {
        // Take the permit before accepting so excess clients never get a process fd.
        let permit = Arc::clone(&permits)
            .acquire_owned()
            .await
            .map_err(std::io::Error::other)?;
        let (stream, addr) = match listener.accept().await {
            Ok(accepted) => accepted,
            Err(e) => {
                tracing::warn!("accept error: {e}");
                tokio::time::sleep(ACCEPT_ERROR_BACKOFF).await;
                continue;
            }
        };
        stream.set_nodelay(true)?;
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
        tokio::spawn(async move {
            let _permit = permit;
            let _ip_slot = ip_slot;
            let conn =
                conn_builder.serve_connection(TokioIo::new(stream), TowerToHyperService::new(svc));
            if let Err(e) = conn.await {
                tracing::debug!("connection from {addr} closed: {e}");
            }
        });
    }
}
