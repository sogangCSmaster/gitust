use super::SshUploadPackError;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::mpsc,
};

const CHUNK: usize = 16 * 1024;

pub(crate) async fn input<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    mut source: R,
    mut destination: W,
    cap: usize,
) -> Result<(), SshUploadPackError> {
    let mut bytes = [0; CHUNK];
    let mut count = 0usize;
    loop {
        let read = source
            .read(&mut bytes)
            .await
            .map_err(|_| SshUploadPackError::Io)?;
        if read == 0 {
            return destination
                .shutdown()
                .await
                .map_err(|_| SshUploadPackError::Io);
        }
        count = count.checked_add(read).ok_or(SshUploadPackError::Limit)?;
        if count > cap {
            return Err(SshUploadPackError::Limit);
        }
        if let Err(error) = destination.write_all(&bytes[..read]).await {
            // fail-open-ok: upload-pack may finish before the client's input half; its independently checked native exit status remains authoritative.
            if error.kind() == std::io::ErrorKind::BrokenPipe {
                return Ok(());
            }
            return Err(SshUploadPackError::Io);
        }
    }
}

pub(crate) async fn output<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    mut source: R,
    mut destination: W,
    cap: usize,
    failures: mpsc::Sender<SshUploadPackError>,
) {
    let mut bytes = [0; CHUNK];
    let mut count = 0usize;
    let mut failed = false;
    loop {
        let read = match source.read(&mut bytes).await {
            Ok(0) => break,
            Ok(read) => read,
            Err(_) => {
                notify(&failures, SshUploadPackError::Io).await;
                return;
            }
        };
        count = count.saturating_add(read);
        if !failed && count > cap {
            notify(&failures, SshUploadPackError::Limit).await;
            failed = true;
        }
        if !failed && destination.write_all(&bytes[..read]).await.is_err() {
            notify(&failures, SshUploadPackError::Io).await;
            failed = true;
        }
        // After refusal, keep draining the native pipe until the supervisor has killed/reaped Git.
    }
    if !failed && destination.shutdown().await.is_err() {
        notify(&failures, SshUploadPackError::Io).await;
    }
}

pub(crate) async fn stderr<R: AsyncRead + Unpin>(
    mut source: R,
    failures: mpsc::Sender<SshUploadPackError>,
) {
    let mut bytes = [0; CHUNK];
    let mut count = 0usize;
    let mut exceeded = false;
    loop {
        match source.read(&mut bytes).await {
            Ok(0) => return,
            Ok(read) => {
                count = count.saturating_add(read);
                if count > 64 * 1024 && !exceeded {
                    exceeded = true;
                    notify(&failures, SshUploadPackError::Limit).await;
                }
            }
            Err(_) => {
                notify(&failures, SshUploadPackError::Io).await;
                return;
            }
        }
    }
}

async fn notify(sender: &mpsc::Sender<SshUploadPackError>, failure: SshUploadPackError) {
    // fail-open-ok: a closed receiver means the supervisor already owns failure cleanup.
    let _ = sender.send(failure).await;
}
