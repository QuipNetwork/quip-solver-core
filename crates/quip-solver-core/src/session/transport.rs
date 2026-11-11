//! Confirm that the request stream reached the socket before the runtime stops.
//!
//! Hyper's HTTP/2 connection task writes the final request frames after the
//! session loop returns. When `run_code` dropped the runtime at that point, the
//! task died with frames still queued, and the coordinator lost the final
//! results and refunds while the process exited Clean. [`TransportClose`]
//! closes that gap with two signals that need no Hyper internals:
//!
//! 1. The request body yielded its end. Hyper polled it to `None`, so the final
//!    DATA frame and `END_STREAM` were handed to the HTTP/2 layer.
//! 2. Every socket was released. Hyper's connection task owns the socket and
//!    drops it only when the task ends, which is after the graceful close and
//!    `poll_shutdown`. The queued frames were written by then.
//!
//! The socket wrapper also records the first write, flush, or shutdown error.
//!
//! Accepted residual risk: Hyper discards stream resets and HTTP/2 protocol
//! errors from the peer, so this module does not detect them. A peer that
//! resets the stream made a coordinator decision. That is not a lost write.

use super::{ExitCode, MinerMsg, SessionExit};
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::UnixStream;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::{timeout_at, Instant};
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::Stream;

#[derive(Default)]
struct SocketState {
    /// Sockets that are open. Hyper releases a socket when its connection task ends.
    open: usize,
    /// The first write, flush, or shutdown error on any socket.
    first_error: Option<String>,
}

/// Wraps each socket the connector opens, so its errors and release are seen.
#[derive(Clone)]
pub(super) struct SocketObserver(watch::Sender<SocketState>);

impl SocketObserver {
    pub(super) fn wrap(&self, socket: UnixStream) -> ObservedSocket {
        self.0.send_modify(|state| state.open += 1);
        ObservedSocket {
            socket,
            observer: self.clone(),
            #[cfg(test)]
            fail_shutdown: false,
        }
    }

    fn record<T>(&self, operation: &str, result: &Poll<io::Result<T>>) {
        if let Poll::Ready(Err(error)) = result {
            let _ = self.0.send_if_modified(|state| {
                if state.first_error.is_some() {
                    return false;
                }
                state.first_error = Some(format!("socket {operation} failed: {error}"));
                true
            });
        }
    }
}

/// A `UnixStream` that reports write errors and its own release.
pub(super) struct ObservedSocket {
    socket: UnixStream,
    observer: SocketObserver,
    /// Test hook: make `poll_shutdown` fail as a broken socket would.
    #[cfg(test)]
    fail_shutdown: bool,
}

impl AsyncRead for ObservedSocket {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.socket).poll_read(cx, buf)
    }
}

impl AsyncWrite for ObservedSocket {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let result = Pin::new(&mut self.socket).poll_write(cx, buf);
        self.observer.record("write", &result);
        result
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        let result = Pin::new(&mut self.socket).poll_write_vectored(cx, bufs);
        self.observer.record("vectored write", &result);
        result
    }

    fn is_write_vectored(&self) -> bool {
        self.socket.is_write_vectored()
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let result = Pin::new(&mut self.socket).poll_flush(cx);
        self.observer.record("flush", &result);
        result
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        #[cfg(test)]
        let result = if self.fail_shutdown {
            Poll::Ready(Err(io::Error::other("injected shutdown failure")))
        } else {
            Pin::new(&mut self.socket).poll_shutdown(cx)
        };
        #[cfg(not(test))]
        let result = Pin::new(&mut self.socket).poll_shutdown(cx);
        self.observer.record("shutdown", &result);
        result
    }
}

impl Drop for ObservedSocket {
    fn drop(&mut self) {
        self.observer.0.send_modify(|state| state.open -= 1);
    }
}

/// The outbound request body. It signals when Hyper polls it to its end.
pub(super) struct RequestStream {
    inner: ReceiverStream<MinerMsg>,
    ended: Option<oneshot::Sender<()>>,
}

impl Stream for RequestStream {
    type Item = MinerMsg;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let result = Pin::new(&mut self.inner).poll_next(cx);
        if let Poll::Ready(None) = result {
            if let Some(ended) = self.ended.take() {
                let _ = ended.send(());
            }
        }
        result
    }
}

/// Waits, after the session loop, for the request stream to reach the socket.
pub(super) struct TransportClose {
    body_ended: oneshot::Receiver<()>,
    sockets: watch::Receiver<SocketState>,
}

impl TransportClose {
    /// Build the close tracker, the request body over `rx`, and the socket observer.
    pub(super) fn new(rx: mpsc::Receiver<MinerMsg>) -> (Self, RequestStream, SocketObserver) {
        let (ended, body_ended) = oneshot::channel();
        let (observer, sockets) = watch::channel(SocketState::default());
        (
            Self {
                body_ended,
                sockets,
            },
            RequestStream {
                inner: ReceiverStream::new(rx),
                ended: Some(ended),
            },
            SocketObserver(observer),
        )
    }

    /// Wait until `deadline` for the body end and the socket release.
    ///
    /// Call this after every handle on the channel is dropped, so the
    /// connection can close. A clean `result` becomes exit 70 when either
    /// signal is late or a socket error was recorded. An existing error is
    /// kept, because it already ends the process with its own cause.
    pub(super) async fn finish(
        mut self,
        result: Result<(), Box<dyn std::error::Error>>,
        deadline: Instant,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let step_failure = match timeout_at(deadline, self.body_ended).await {
            Err(_) => Some("the request body did not end within the shutdown deadline"),
            Ok(Err(_)) => Some("the request body was dropped before its end"),
            Ok(Ok(())) => {
                match timeout_at(deadline, self.sockets.wait_for(|s| s.open == 0)).await {
                    // The sender lives in each open socket, so a closed channel
                    // also means that every socket was released.
                    Ok(_) => None,
                    Err(_) => Some("the connection was not released within the shutdown deadline"),
                }
            }
        };
        let socket_error = self.sockets.borrow().first_error.clone();
        let Some(detail) = socket_error.or_else(|| step_failure.map(str::to_owned)) else {
            return result;
        };
        let detail = format!("session transport did not close cleanly: {detail}");
        if let Err(error) = &result {
            tracing::warn!(detail, %error, "session already failed; keeping its exit code");
            return result;
        }
        tracing::warn!(
            detail,
            "final messages may not have reached the coordinator"
        );
        Err(SessionExit::new(ExitCode::InternalFatal, detail).into())
    }
}

#[cfg(test)]
mod tests {
    use super::super::map_err_to_exit;
    use super::*;
    use std::time::Duration;
    use tokio::io::AsyncWriteExt as _;
    use tokio_stream::StreamExt as _;

    fn exit_of(result: Result<(), Box<dyn std::error::Error>>) -> (ExitCode, String) {
        let error = result.expect_err("transport close must fail");
        let text = error.to_string();
        (map_err_to_exit(error, "test"), text)
    }

    #[tokio::test]
    async fn clean_close_keeps_the_clean_result() {
        let (tx, rx) = mpsc::channel(1);
        let (close, mut body, observer) = TransportClose::new(rx);
        let (socket, _peer) = UnixStream::pair().unwrap();
        let mut socket = observer.wrap(socket);
        drop(tx);
        assert!(body.next().await.is_none());
        socket.shutdown().await.unwrap();
        drop(socket);
        drop(observer);
        close
            .finish(Ok(()), Instant::now() + Duration::from_secs(1))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn socket_shutdown_error_turns_a_clean_session_into_internal_fatal() {
        let (tx, rx) = mpsc::channel(1);
        let (close, mut body, observer) = TransportClose::new(rx);
        let (socket, _peer) = UnixStream::pair().unwrap();
        let mut socket = observer.wrap(socket);
        socket.fail_shutdown = true;
        drop(tx);
        assert!(body.next().await.is_none());
        assert!(socket.shutdown().await.is_err());
        drop(socket);
        let result = close
            .finish(Ok(()), Instant::now() + Duration::from_secs(1))
            .await;
        let (code, text) = exit_of(result);
        assert_eq!(code, ExitCode::InternalFatal);
        assert!(text.contains("socket shutdown failed"), "{text}");
    }

    #[tokio::test]
    async fn socket_write_error_turns_a_clean_session_into_internal_fatal() {
        for vectored in [false, true] {
            let (tx, rx) = mpsc::channel(1);
            let (close, mut body, observer) = TransportClose::new(rx);
            let (socket, peer) = UnixStream::pair().unwrap();
            let mut socket = observer.wrap(socket);
            drop(peer);
            drop(tx);
            assert!(body.next().await.is_none());
            let write = if vectored {
                socket.write_vectored(&[io::IoSlice::new(b"final")]).await
            } else {
                socket.write(b"final").await
            };
            assert!(write.is_err());
            drop(socket);
            let result = close
                .finish(Ok(()), Instant::now() + Duration::from_secs(1))
                .await;
            let (code, text) = exit_of(result);
            assert_eq!(code, ExitCode::InternalFatal);
            assert!(text.contains("write failed"), "{text}");
        }
    }

    #[tokio::test]
    async fn unreleased_connection_is_internal_fatal_at_the_deadline() {
        let (tx, rx) = mpsc::channel(1);
        let (close, mut body, observer) = TransportClose::new(rx);
        // The peer never reads and the socket stays open, as with a connection
        // task stalled by flow control.
        let (socket, _peer) = UnixStream::pair().unwrap();
        let socket = observer.wrap(socket);
        drop(tx);
        assert!(body.next().await.is_none());
        let start = Instant::now();
        let grace = Duration::from_millis(50);
        let result = tokio::time::timeout(
            grace + Duration::from_millis(250),
            close.finish(Ok(()), start + grace),
        )
        .await
        .expect("transport close overran its deadline");
        assert!(start.elapsed() >= grace);
        let (code, text) = exit_of(result);
        assert_eq!(code, ExitCode::InternalFatal);
        assert!(text.contains("connection was not released"), "{text}");
        drop(socket);
    }

    #[tokio::test]
    async fn unpolled_body_is_internal_fatal_at_the_deadline() {
        let (tx, rx) = mpsc::channel(1);
        let (close, _body, observer) = TransportClose::new(rx);
        drop(tx);
        drop(observer);
        let result = close
            .finish(Ok(()), Instant::now() + Duration::from_millis(20))
            .await;
        let (code, text) = exit_of(result);
        assert_eq!(code, ExitCode::InternalFatal);
        assert!(text.contains("request body did not end"), "{text}");
    }

    #[tokio::test]
    async fn close_failure_keeps_an_existing_session_error() {
        let (_tx, rx) = mpsc::channel(1);
        let (close, body, _observer) = TransportClose::new(rx);
        drop(body);
        let result = close
            .finish(
                Err(SessionExit::new(ExitCode::ConfigInvalid, "bad welcome").into()),
                Instant::now() + Duration::from_secs(1),
            )
            .await;
        let (code, text) = exit_of(result);
        assert_eq!(code, ExitCode::ConfigInvalid);
        assert_eq!(text, "bad welcome");
    }
}
