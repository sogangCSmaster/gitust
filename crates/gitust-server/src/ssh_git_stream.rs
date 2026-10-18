//! Retain russh's close-on-drop stream until the native exit status has been sent.
use parking_lot::Mutex;
use russh::{server::Msg, ChannelStream};
use std::{
    io,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

pub(crate) type Lease = Arc<Mutex<ChannelStream<Msg>>>;
pub(crate) struct Stream(Lease);
impl Stream {
    pub(crate) fn retain(stream: ChannelStream<Msg>) -> (Self, Lease) {
        let lease = Arc::new(Mutex::new(stream));
        (Self(lease.clone()), lease)
    }
}
impl AsyncRead for Stream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        // The mutex covers one nonblocking poll only, never an await or native I/O.
        Pin::new(&mut *self.0.lock()).poll_read(cx, buffer)
    }
}
impl AsyncWrite for Stream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut *self.0.lock()).poll_write(cx, bytes)
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut *self.0.lock()).poll_flush(cx)
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut *self.0.lock()).poll_shutdown(cx)
    }
}
