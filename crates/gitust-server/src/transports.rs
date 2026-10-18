use crate::{listener, ssh_git};
use std::io;

pub(crate) async fn serve(
    http: tokio::net::TcpListener,
    app: axum::Router,
    ssh: Option<ssh_git::Listener>,
) -> io::Result<()> {
    let Some(ssh) = ssh else {
        return listener::serve_hardened(http, app).await;
    };
    #[cfg(unix)]
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let shutdown = async {
        #[cfg(unix)]
        tokio::select! {
            result = tokio::signal::ctrl_c() => result,
            result = terminate.recv() => result.ok_or_else(|| io::Error::other("shutdown signal stream ended")),
        }
        #[cfg(not(unix))]
        tokio::signal::ctrl_c().await
    };
    let mut ssh = ssh.start();
    let (stop_http, mut http_stopped) = tokio::sync::watch::channel(false);
    let http =
        listener::serve_hardened_until(http, app, listener::ServeLimits::default(), async move {
            while !*http_stopped.borrow_and_update() {
                if http_stopped.changed().await.is_err() {
                    return;
                }
            }
        });
    tokio::pin!(http);
    let mut http_finished = false;
    let result = tokio::select! {
        result = &mut http => { http_finished = true; result },
        result = ssh.finished() => result,
        result = shutdown => result,
    };
    stop_http.send_replace(true);
    let (ssh_cleanup, http_cleanup) = tokio::join!(ssh.shutdown(), async {
        if http_finished {
            Ok(())
        } else {
            http.await
        }
    });
    result.and(http_cleanup).and(ssh_cleanup)
}
