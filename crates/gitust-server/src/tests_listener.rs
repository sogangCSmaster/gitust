//! Regression tests for the hardened HTTP listener (Gate B P1, 2026-10-16: no header/body
//! timeouts and no connection cap let idle unauthenticated sockets exhaust file descriptors).

use std::time::{Duration, Instant};

use axum::body::Body;
use axum::routing::{get, post};
use axum::Router;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::listener::{serve_hardened_until, serve_hardened_with, DeadlineBody, ServeLimits};

#[tokio::test]
async fn shutdown_or_caller_cancellation_closes_idle_keepalive_and_stops_accepting() {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    for cancel_caller in [false, true] {
        let requests = Arc::new(AtomicUsize::new(0));
        let seen = requests.clone();
        let app = Router::new().route(
            "/health",
            get(move || {
                let seen = seen.clone();
                async move {
                    seen.fetch_add(1, Ordering::SeqCst);
                    "ok"
                }
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let (stop, mut stopped) = tokio::sync::watch::channel(false);
        let limits = ServeLimits {
            header_read_timeout: Duration::from_secs(30),
            ..fast_limits()
        };
        let server = tokio::spawn(serve_hardened_until(listener, app, limits, async move {
            while !*stopped.borrow_and_update() {
                if stopped.changed().await.is_err() {
                    return;
                }
            }
        }));
        let mut stream = TcpStream::connect(addr).await.expect("connect");
        stream
            .write_all(b"GET /health HTTP/1.1\r\nHost: fixture\r\n\r\n")
            .await
            .expect("request");
        let mut response = Vec::new();
        while !response.ends_with(b"\r\n\r\nok") {
            let mut buffer = [0; 256];
            let count = tokio::time::timeout(Duration::from_secs(2), stream.read(&mut buffer))
                .await
                .expect("response deadline")
                .expect("response");
            assert_ne!(count, 0);
            response.extend_from_slice(&buffer[..count]);
            assert!(response.len() < 1024);
        }
        assert!(response.starts_with(b"HTTP/1.1 200"));
        if cancel_caller {
            server.abort();
            assert!(server.await.expect_err("caller cancelled").is_cancelled());
        } else {
            stop.send_replace(true);
            tokio::time::timeout(Duration::from_secs(2), server)
                .await
                .expect("shutdown deadline")
                .expect("join")
                .expect("shutdown");
        }
        assert!(time_until_closed(&mut stream, Duration::from_secs(2))
            .await
            .is_some());
        let deadline = Instant::now() + Duration::from_secs(2);
        while TcpStream::connect(addr).await.is_ok() {
            assert!(
                Instant::now() < deadline,
                "listener socket remained connectable after shutdown"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let _ = stream
            .write_all(b"GET /health HTTP/1.1\r\nHost: fixture\r\n\r\n")
            .await;
        assert_eq!(requests.load(Ordering::SeqCst), 1);
    }
}

async fn spawn_server(limits: ServeLimits) -> std::net::SocketAddr {
    let app = Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/echo", post(|body: String| async move { body }));
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(serve_hardened_with(listener, app, limits));
    addr
}

/// Reads until EOF/error; returns how long the server took to close the connection.
async fn time_until_closed(stream: &mut TcpStream, max: Duration) -> Option<Duration> {
    let start = Instant::now();
    let mut buf = [0u8; 1024];
    loop {
        match tokio::time::timeout(max.saturating_sub(start.elapsed()), stream.read(&mut buf)).await
        {
            Ok(Ok(0) | Err(_)) => return Some(start.elapsed()),
            Ok(Ok(_)) => continue,
            Err(_) => return None,
        }
    }
}

fn fast_limits() -> ServeLimits {
    ServeLimits {
        max_connections: 64,
        max_connections_per_ip: 64,
        header_read_timeout: Duration::from_millis(300),
    }
}

#[tokio::test]
async fn slow_loris_partial_headers_are_closed() {
    let addr = spawn_server(fast_limits()).await;
    let mut s = TcpStream::connect(addr).await.expect("connect");
    s.write_all(b"GET /health HTTP/1.1\r\nHost: x\r\n")
        .await
        .expect("write");
    let closed = time_until_closed(&mut s, Duration::from_secs(3)).await;
    assert!(
        closed.is_some(),
        "partial-header connection was never closed"
    );
}

#[tokio::test]
async fn idle_connection_without_request_is_closed() {
    let addr = spawn_server(fast_limits()).await;
    let mut s = TcpStream::connect(addr).await.expect("connect");
    let closed = time_until_closed(&mut s, Duration::from_secs(3)).await;
    assert!(closed.is_some(), "idle connection was never closed");
}

#[tokio::test]
async fn idle_keep_alive_after_request_is_closed() {
    let addr = spawn_server(fast_limits()).await;
    let mut s = TcpStream::connect(addr).await.expect("connect");
    s.write_all(b"GET /health HTTP/1.1\r\nHost: x\r\n\r\n")
        .await
        .expect("write");
    let mut buf = [0u8; 256];
    let n = s.read(&mut buf).await.expect("read response");
    assert!(String::from_utf8_lossy(&buf[..n]).starts_with("HTTP/1.1 200"));
    let closed = time_until_closed(&mut s, Duration::from_secs(3)).await;
    assert!(
        closed.is_some(),
        "idle keep-alive connection was never closed"
    );
}

#[tokio::test]
async fn per_ip_connection_cap_rejects_excess_and_keeps_serving() {
    let addr = spawn_server(ServeLimits {
        max_connections: 64,
        max_connections_per_ip: 2,
        header_read_timeout: Duration::from_secs(30),
    })
    .await;
    let _a = TcpStream::connect(addr).await.expect("a");
    let _b = TcpStream::connect(addr).await.expect("b");
    tokio::time::sleep(Duration::from_millis(100)).await;
    let mut c = TcpStream::connect(addr).await.expect("c");
    let closed = time_until_closed(&mut c, Duration::from_secs(2)).await;
    assert!(
        closed.is_some(),
        "connection over the per-IP cap was not rejected"
    );

    drop(_a);
    tokio::time::sleep(Duration::from_millis(100)).await;
    let mut d = TcpStream::connect(addr).await.expect("d");
    d.write_all(b"GET /health HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
        .await
        .expect("write");
    let mut out = Vec::new();
    d.read_to_end(&mut out).await.expect("read");
    assert!(String::from_utf8_lossy(&out).starts_with("HTTP/1.1 200"));
}

/// Body that yields whatever the test sends through the channel (trickle simulation).
struct ChannelBody(tokio::sync::mpsc::Receiver<bytes::Bytes>);

impl http_body::Body for ChannelBody {
    type Data = bytes::Bytes;
    type Error = std::convert::Infallible;

    fn poll_frame(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        self.0
            .poll_recv(cx)
            .map(|chunk| chunk.map(|b| Ok(http_body::Frame::data(b))))
    }
}

#[tokio::test]
async fn trickled_body_hits_total_deadline() {
    let (tx, rx) = tokio::sync::mpsc::channel::<bytes::Bytes>(4);
    let body = Body::new(DeadlineBody::new(
        Body::new(ChannelBody(rx)),
        Duration::from_millis(200),
    ));
    let feeder = tokio::spawn(async move {
        for _ in 0..50 {
            if tx.send(bytes::Bytes::from_static(b"x")).await.is_err() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    });
    let start = Instant::now();
    let res = axum::body::to_bytes(body, 1024 * 1024).await;
    assert!(res.is_err(), "trickled body was accepted past the deadline");
    assert!(start.elapsed() < Duration::from_secs(1));
    feeder.abort();
}

/// Gate A/B R2 P1: `POST /mcp` and `GET /api/v1/auth/pats` must run `AuthStore` lock/reload checks
/// on `spawn_blocking` so holding `<auth-db>.lock` after a DB mtime bump never starves `/api/v1/health`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn held_auth_lock_under_concurrent_mcp_and_pat_requests_does_not_starve_health() {
    let temp = tempfile::tempdir().expect("tempdir");
    let db_path = temp.path().join("auth-db.json");
    let lock_path = temp.path().join("auth-db.json.lock");
    let store = gitust_db::AuthStore::open_persistent(&db_path).expect("open store");
    store
        .bootstrap_initial_admin_cli("admin1", "admin1@company.internal", "Admin1StrongPass!2026")
        .expect("bootstrap");
    let session = store
        .login("admin1", "Admin1StrongPass!2026")
        .expect("login");
    let token = session.raw_session_token;

    let app = crate::build_router(
        crate::AppState::with_auth_store(
            temp.path().to_path_buf(),
            "https://github.com/org/repo.git".to_owned(),
            store,
        )
        .expect("recover server state"),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(serve_hardened_with(listener, app, fast_limits()));

    // Bump DB size/mtime so every AuthStore read enters `acquire_advisory_lock`, then hold the lock.
    let mut raw = std::fs::read(&db_path).expect("read db");
    raw.push(b'\n');
    std::fs::write(&db_path, &raw).expect("bump db");
    let lock_file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&lock_path)
        .expect("open lock");
    lock_file.try_lock().expect("hold lock");

    let mut waiters = Vec::new();
    for i in 0..8 {
        let tok = token.clone();
        waiters.push(tokio::spawn(async move {
            let mut s = TcpStream::connect(addr).await.expect("connect");
            let req = if i % 2 == 0 {
                format!(
                    "POST /mcp HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {tok}\r\nContent-Type: application/json\r\nContent-Length: 36\r\nConnection: close\r\n\r\n{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}}"
                )
            } else {
                format!(
                    "GET /api/v1/auth/pats HTTP/1.1\r\nHost: x\r\nAuthorization: Bearer {tok}\r\nConnection: close\r\n\r\n"
                )
            };
            let _ = s.write_all(req.as_bytes()).await;
            let mut out = Vec::new();
            let _ = s.read_to_end(&mut out).await;
        }));
    }
    tokio::time::sleep(Duration::from_millis(80)).await;

    let health_start = Instant::now();
    let mut h = TcpStream::connect(addr).await.expect("connect health");
    h.write_all(b"GET /api/v1/health HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
        .await
        .expect("write health");
    let mut resp = Vec::new();
    h.read_to_end(&mut resp).await.expect("read health");
    let elapsed = health_start.elapsed();
    drop(lock_file);
    for w in waiters {
        let _ = w.await;
    }

    assert!(
        String::from_utf8_lossy(&resp).starts_with("HTTP/1.1 200"),
        "unexpected health response: {}",
        String::from_utf8_lossy(&resp)
    );
    assert!(
        elapsed < Duration::from_millis(400),
        "/api/v1/health was starved for {elapsed:?} while auth lock was held"
    );
}
