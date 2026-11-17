//! Client for Mercury's KISS broadcast port (docs/03 §6).
//!
//! A socket task decodes incoming bytes with [`kiss::Decoder`] into a bounded
//! channel. A full channel drops the frame and counts it: a slow consumer
//! must never stall the socket or grow memory.

use crate::client::AttachError;
use crate::kiss::{self, KissFrame, CMD_DATA_STD};
use std::io;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot};
use tokio::task::AbortHandle;

const CHANNEL_DEPTH: usize = 64;

type Write = (Vec<u8>, oneshot::Sender<io::Result<()>>);

/// Closes exactly one broadcast connection, even if the modem task is busy.
#[derive(Debug, Clone)]
pub struct BroadcastAbort {
    task: AbortHandle,
    stop: Arc<AtomicBool>,
}

impl BroadcastAbort {
    pub fn abort(&self) {
        self.stop.store(true, Ordering::Release);
        self.task.abort();
    }
}

pub struct BroadcastPort {
    writer: mpsc::Sender<Write>,
    abort: BroadcastAbort,
    dropped: Arc<AtomicU64>,
}

impl Drop for BroadcastPort {
    fn drop(&mut self) {
        self.abort.abort();
    }
}

impl BroadcastPort {
    /// Success does not prove Mercury accepted this client. It serves one
    /// broadcast client at a time, so this socket may still be in its backlog.
    pub async fn connect(
        host: &str,
        port: u16,
        t: Duration,
    ) -> Result<(BroadcastPort, mpsc::Receiver<KissFrame>), AttachError> {
        let mut sock = match tokio::time::timeout(t, TcpStream::connect((host, port))).await {
            Err(_) => return Err(AttachError::ConnectTimeout { plane: "broadcast", port }),
            Ok(Err(source)) => return Err(AttachError::Connect { plane: "broadcast", port, source }),
            Ok(Ok(s)) => s,
        };
        // On Windows an abortive close prevents a late reader from receiving
        // frames awaiting it in TCP. Keep the whole socket in one task so drop
        // cannot send a FIN via OwnedWriteHalf or leave the read half alive.
        sock.set_zero_linger().map_err(|source| AttachError::Connect { plane: "broadcast", port, source })?;
        let (tx, rx) = mpsc::channel(CHANNEL_DEPTH);
        let (writer, mut writes) = mpsc::channel::<Write>(1);
        let dropped = Arc::new(AtomicU64::new(0));
        let dropped_frames = dropped.clone();
        let task = tokio::spawn(async move {
            let mut dec = kiss::Decoder::new();
            let mut buf = [0u8; 2048];
            loop {
                tokio::select! {
                    write = writes.recv() => {
                        let Some((frame, reply)) = write else { break };
                        let result = sock.write_all(&frame).await;
                        let failed = result.is_err();
                        let _ = reply.send(result);
                        if failed { break }
                    }
                    read = sock.read(&mut buf) => {
                        let Ok(n @ 1..) = read else { break };
                        for frame in dec.push(&buf[..n]) {
                            if let Err(mpsc::error::TrySendError::Full(_)) = tx.try_send(frame) {
                                dropped_frames.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    }
                }
            }
        });
        let abort = BroadcastAbort { task: task.abort_handle(), stop: Arc::default() };
        Ok((BroadcastPort { writer, abort, dropped }, rx))
    }

    pub fn abort_handle(&self) -> BroadcastAbort {
        self.abort.clone()
    }

    /// Submit a wrapped broadcast message. The caller applies the idle and
    /// capacity gate; this writes whatever it is given.
    pub async fn send(&self, payload: &[u8]) -> io::Result<()> {
        fn closed() -> io::Error {
            io::Error::new(io::ErrorKind::NotConnected, "broadcast port closed")
        }

        if self.is_closed() {
            return Err(closed());
        }
        let (reply, outcome) = oneshot::channel();
        self.writer.send((kiss::encode(CMD_DATA_STD, payload), reply)).await.map_err(|_| closed())?;
        outcome.await.map_err(|_| closed())?
    }

    pub fn is_closed(&self) -> bool {
        self.abort.stop.load(Ordering::Acquire) || self.writer.is_closed()
    }

    /// Frames discarded because the receive channel was full.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}
