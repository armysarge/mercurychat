//! Client for Mercury's KISS broadcast port (docs/03 §6).
//!
//! A reader task decodes incoming bytes with [`kiss::Decoder`] into a bounded
//! channel. A full channel drops the frame and counts it: a slow consumer
//! must never stall the socket or grow memory.

use crate::client::AttachError;
use crate::kiss::{self, KissFrame, CMD_DATA_STD};
use std::io;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::OwnedWriteHalf;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, Mutex};

const CHANNEL_DEPTH: usize = 64;

pub struct BroadcastPort {
    writer: Mutex<OwnedWriteHalf>,
    closed: Arc<AtomicBool>,
    dropped: Arc<AtomicU64>,
}

impl BroadcastPort {
    pub async fn connect(
        host: &str,
        port: u16,
        t: Duration,
    ) -> Result<(BroadcastPort, mpsc::Receiver<KissFrame>), AttachError> {
        let sock = match tokio::time::timeout(t, TcpStream::connect((host, port))).await {
            Err(_) => return Err(AttachError::ConnectTimeout { plane: "broadcast", port }),
            Ok(Err(source)) => return Err(AttachError::Connect { plane: "broadcast", port, source }),
            Ok(Ok(s)) => s,
        };
        let (mut r, w) = sock.into_split();
        let (tx, rx) = mpsc::channel(CHANNEL_DEPTH);
        let closed = Arc::new(AtomicBool::new(false));
        let dropped = Arc::new(AtomicU64::new(0));
        let (c2, d2) = (closed.clone(), dropped.clone());
        tokio::spawn(async move {
            let mut dec = kiss::Decoder::new();
            let mut buf = [0u8; 2048];
            'read: loop {
                let n = match r.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                for f in dec.push(&buf[..n]) {
                    match tx.try_send(f) {
                        Ok(()) => {}
                        Err(mpsc::error::TrySendError::Full(_)) => {
                            d2.fetch_add(1, Ordering::Relaxed);
                        }
                        Err(mpsc::error::TrySendError::Closed(_)) => break 'read,
                    }
                }
            }
            c2.store(true, Ordering::Release);
        });
        Ok((BroadcastPort { writer: Mutex::new(w), closed, dropped }, rx))
    }

    /// Submit a wrapped broadcast message. The caller applies the idle and
    /// capacity gate; this writes whatever it is given.
    pub async fn send(&self, payload: &[u8]) -> io::Result<()> {
        if self.is_closed() {
            return Err(io::Error::new(io::ErrorKind::NotConnected, "broadcast port closed"));
        }
        let mut w = self.writer.lock().await;
        w.write_all(&kiss::encode(CMD_DATA_STD, payload)).await?;
        w.flush().await
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    /// Frames discarded because the receive channel was full.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}
