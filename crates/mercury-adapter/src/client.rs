//! Paired control/data attachment to one Mercury instance.
//!
//! * Control and data are attached as a pair; a partial attach closes both.
//! * Mercury closes a second client at once (one-client rule); that is detected
//!   as `Occupied`, never retried in a takeover loop.
//! * Commands are serialised: one outstanding OK/WRONG at a time with a local
//!   ack timeout. A timed-out command poisons the client until re-attach,
//!   because its effect is unknown (docs/02 §3).

use crate::command::Command;
use crate::control::{parse_event, ControlEvent, LineParser};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::OwnedWriteHalf;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot, Mutex};

static GENERATION: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoint {
    pub host: String,
    pub control_port: u16,
    pub data_port: u16,
}

impl Endpoint {
    /// Mercury convention: control on base, data on base + 1.
    pub fn base(host: &str, base_port: u16) -> Self {
        Endpoint { host: host.into(), control_port: base_port, data_port: base_port + 1 }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Timeouts {
    pub connect: Duration,
    /// How long to watch a fresh socket for Mercury's immediate refusal close.
    pub refusal_probe: Duration,
    pub command_ack: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Timeouts {
            connect: Duration::from_secs(3),
            refusal_probe: Duration::from_millis(400),
            command_ack: Duration::from_secs(5),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AttachError {
    #[error("{plane} port {port}: connection failed: {source}")]
    Connect { plane: &'static str, port: u16, source: std::io::Error },
    #[error("{plane} port {port}: timed out connecting")]
    ConnectTimeout { plane: &'static str, port: u16 },
    #[error("{plane} port {port}: another client holds this Mercury (connection closed at once)")]
    Occupied { plane: &'static str, port: u16 },
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CommandError {
    #[error("no OK/WRONG within the ack timeout; command effect unknown — re-attach required")]
    Timeout,
    #[error("an earlier command timed out; re-attach before issuing commands")]
    NeedsReconcile,
    #[error("control connection closed")]
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ack {
    Ok,
    Wrong,
}

/// Events delivered by the control reader.
#[derive(Debug, Clone, PartialEq)]
pub enum AdapterEvent {
    Control(ControlEvent),
    /// Raw line, for transcripts/fixtures.
    RawLine(String),
    ControlClosed,
    ControlFault(String),
}

pub struct Attachment {
    pub client: MercuryClient,
    pub events: mpsc::Receiver<AdapterEvent>,
    pub data: TcpStream,
}

pub struct MercuryClient {
    generation: u64,
    writer: Mutex<OwnedWriteHalf>,
    pending: Arc<StdMutex<Option<oneshot::Sender<Ack>>>>,
    needs_reconcile: AtomicBool,
    closed: Arc<AtomicBool>,
    ack_timeout: Duration,
}

async fn connect_one(host: &str, port: u16, plane: &'static str, t: &Timeouts) -> Result<TcpStream, AttachError> {
    let s = tokio::time::timeout(t.connect, TcpStream::connect((host, port)))
        .await
        .map_err(|_| AttachError::ConnectTimeout { plane, port })?
        .map_err(|source| AttachError::Connect { plane, port, source })?;
    s.set_nodelay(true).ok();
    Ok(s)
}

/// True when the peer closed the fresh connection (Mercury's refusal).
async fn refused(s: &TcpStream, probe: Duration) -> bool {
    let mut b = [0u8; 1];
    matches!(tokio::time::timeout(probe, s.peek(&mut b)).await, Ok(Ok(0)) | Ok(Err(_)))
}

impl MercuryClient {
    pub async fn attach(ep: &Endpoint, t: Timeouts) -> Result<Attachment, AttachError> {
        let ctrl = connect_one(&ep.host, ep.control_port, "control", &t).await?;
        let data = match connect_one(&ep.host, ep.data_port, "data", &t).await {
            Ok(d) => d,
            Err(e) => {
                drop(ctrl);
                return Err(e);
            }
        };
        let (rc, rd) = tokio::join!(refused(&ctrl, t.refusal_probe), refused(&data, t.refusal_probe));
        if rc {
            return Err(AttachError::Occupied { plane: "control", port: ep.control_port });
        }
        if rd {
            return Err(AttachError::Occupied { plane: "data", port: ep.data_port });
        }

        let (mut rh, wh) = ctrl.into_split();
        let (tx, rx) = mpsc::channel(1024);
        let pending: Arc<StdMutex<Option<oneshot::Sender<Ack>>>> = Arc::default();
        let closed = Arc::new(AtomicBool::new(false));
        {
            let pending = pending.clone();
            let closed = closed.clone();
            tokio::spawn(async move {
                let mut parser = LineParser::new();
                let mut buf = vec![0u8; 2048];
                loop {
                    let n = match rh.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => n,
                    };
                    let lines = match parser.push(&buf[..n]) {
                        Ok(l) => l,
                        Err(e) => {
                            let _ = tx.send(AdapterEvent::ControlFault(e.to_string())).await;
                            break;
                        }
                    };
                    for line in lines {
                        let _ = tx.send(AdapterEvent::RawLine(line.clone())).await;
                        let ev = parse_event(&line);
                        let ack = match ev {
                            ControlEvent::Ok => Some(Ack::Ok),
                            ControlEvent::Wrong => Some(Ack::Wrong),
                            _ => None,
                        };
                        if let Some(a) = ack {
                            if let Some(waiter) = pending.lock().unwrap().take() {
                                let _ = waiter.send(a);
                                continue;
                            }
                        }
                        if tx.send(AdapterEvent::Control(ev)).await.is_err() {
                            return;
                        }
                    }
                }
                closed.store(true, Ordering::SeqCst);
                pending.lock().unwrap().take();
                let _ = tx.send(AdapterEvent::ControlClosed).await;
            });
        }

        Ok(Attachment {
            client: MercuryClient {
                generation: GENERATION.fetch_add(1, Ordering::SeqCst),
                writer: Mutex::new(wh),
                pending,
                needs_reconcile: AtomicBool::new(false),
                closed,
                ack_timeout: t.command_ack,
            },
            events: rx,
            data,
        })
    }

    /// Attachment generation; events/state from older generations are stale.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    /// Send one command and wait for its OK/WRONG. Acknowledgement is not
    /// completion of any RF operation.
    pub async fn command(&self, cmd: &Command) -> Result<Ack, CommandError> {
        if self.needs_reconcile.load(Ordering::SeqCst) {
            return Err(CommandError::NeedsReconcile);
        }
        if self.is_closed() {
            return Err(CommandError::Closed);
        }
        let mut w = self.writer.lock().await;
        let (tx, rx) = oneshot::channel();
        *self.pending.lock().unwrap() = Some(tx);
        if w.write_all(cmd.wire().as_bytes()).await.is_err() {
            self.pending.lock().unwrap().take();
            return Err(CommandError::Closed);
        }
        match tokio::time::timeout(self.ack_timeout, rx).await {
            Ok(Ok(a)) => Ok(a),
            Ok(Err(_)) => Err(CommandError::Closed),
            Err(_) => {
                self.pending.lock().unwrap().take();
                self.needs_reconcile.store(true, Ordering::SeqCst);
                Err(CommandError::Timeout)
            }
        }
    }

    /// Send a query whose answer is an asynchronous line (BUFFER, SN, VERSION).
    pub async fn query(&self, word: &str) -> Result<(), CommandError> {
        if !word.bytes().all(|b| b.is_ascii_uppercase()) {
            return Err(CommandError::Closed);
        }
        let mut w = self.writer.lock().await;
        w.write_all(format!("{word}\r").as_bytes()).await.map_err(|_| CommandError::Closed)
    }
}
