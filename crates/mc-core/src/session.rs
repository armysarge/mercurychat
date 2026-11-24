//! One MCH/1 application session over an established ARQ link.
//!
//! The session is a single task: it reads the data stream, drains operator
//! commands and modem signals, and writes frames in priority order
//! (protocol/receipts, then chat, then file chunks). File chunks are paced by
//! airtime (M4 C4): about 12 s of file data in the modem queue at the rate
//! `BUFFER` is seen to drain, so chat is not stuck behind a large backlog.

use crate::files::pace::Pacer;
use crate::files::{display_name, sanitize_name, unique_target, unpack_zstd, Prepared, UnpackError};
use mc_storage::{Direction, OutState, Store, Stored, TransferRow, TransferState};
use mch_protocol::payload::*;
use mch_protocol::{encode, Decoder, Frame, FrameType, ObjectId, ProtocolError};
use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot};

pub const APP_NAME: &str = "MercuryChat";
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Debug)]
pub struct SessionConfig {
    pub local_call: String,
    pub peer_call: String,
    pub generation: u64,
    /// The receive folder: published files land here (FIL-04).
    pub files_dir: PathBuf,
    /// Where incoming wire bytes wait (the station's `tmp/`).
    pub tmp_dir: PathBuf,
    /// The largest incoming file, as it lands on disk.
    pub max_file: u64,
    pub chunk: u32,
    pub auto_accept_files: bool,
    /// An unanswered offer is declined `timeout` after this (03: 2 min).
    pub offer_timeout: Duration,
    pub free_space: crate::station::FreeSpace,
    pub hello_timeout: Duration,
    /// The bandwidth's nominal throughput in bytes per second, until the
    /// drain rate is measured (M4 C4).
    pub nominal_bps: f64,
    /// The MCH/1 minor this side announces (`MINOR`; lower only to act as an older peer).
    pub proto_minor: u8,
    /// The capabilities this side announces (`default_capabilities()`).
    pub capabilities: Vec<String>,
}

/// What this version announces in HELLO: chat, files (`file/2`, M4 C1), QSY
/// and info (M3c B1). The old `file` is not announced: no build before M4
/// could accept a file.
pub fn default_capabilities() -> Vec<String> {
    ["chat", mch_protocol::CAP_FILE, mch_protocol::CAP_QSY, mch_protocol::CAP_INFO]
        .iter()
        .map(|c| c.to_string())
        .collect()
}

/// A minor-1 message to or from the peer; each goes on the wire only when
/// its capability was negotiated (M3c B1). The id is the proposal or request
/// id (`ObjectId::ZERO` for an unsolicited info response).
#[derive(Debug, Clone, PartialEq)]
pub enum PeerMsg {
    QsyPropose(ObjectId, QsyPropose),
    QsyAnswer(ObjectId, QsyAnswer),
    InfoRequest(ObjectId, InfoRequest),
    /// Boxed: a station share is large next to the other messages.
    InfoResponse(ObjectId, Box<InfoResponse>),
}

impl PeerMsg {
    pub fn ftype(&self) -> FrameType {
        match self {
            PeerMsg::QsyPropose(..) => FrameType::QsyPropose,
            PeerMsg::QsyAnswer(..) => FrameType::QsyAnswer,
            PeerMsg::InfoRequest(..) => FrameType::InfoRequest,
            PeerMsg::InfoResponse(..) => FrameType::InfoResponse,
        }
    }

    /// What it is, for the operator.
    pub fn what(&self) -> &'static str {
        match self {
            PeerMsg::QsyPropose(..) => "QSY proposal",
            PeerMsg::QsyAnswer(..) => "QSY answer",
            PeerMsg::InfoRequest(..) => "information request",
            PeerMsg::InfoResponse(..) => "information",
        }
    }

    pub fn to_frame(&self) -> Result<Frame, ProtocolError> {
        match self {
            PeerMsg::QsyPropose(id, p) => p.to_frame(*id),
            PeerMsg::QsyAnswer(id, a) => a.to_frame(*id),
            PeerMsg::InfoRequest(id, r) => r.to_frame(*id),
            PeerMsg::InfoResponse(id, r) => r.to_frame(*id),
        }
    }

    /// The message a minor-1 frame carries; `None` for any other type.
    pub fn from_frame(f: &Frame) -> Option<Result<PeerMsg, ProtocolError>> {
        Some(match f.ftype {
            FrameType::QsyPropose => QsyPropose::from_frame(f).map(|p| PeerMsg::QsyPropose(f.id, p)),
            FrameType::QsyAnswer => QsyAnswer::from_frame(f).map(|a| PeerMsg::QsyAnswer(f.id, a)),
            FrameType::InfoRequest => InfoRequest::from_frame(f).map(|r| PeerMsg::InfoRequest(f.id, r)),
            FrameType::InfoResponse => InfoResponse::from_frame(f).map(|r| PeerMsg::InfoResponse(f.id, Box::new(r))),
            _ => return None,
        })
    }
}

#[derive(Debug)]
pub enum SessionCommand {
    /// Send (or, with an existing `id`, deliberately retry) a chat message.
    SendChat {
        id: ObjectId,
        text: String,
    },
    /// Offer a prepared file (M4 C2); `id` is the transfer id on the wire.
    SendFile {
        id: ObjectId,
        prepared: Prepared,
    },
    /// Give an incoming offer one more window (once).
    ExtendFile {
        id: ObjectId,
    },
    AcceptFile {
        id: ObjectId,
    },
    RejectFile {
        id: ObjectId,
    },
    CancelFile {
        id: ObjectId,
    },
    /// A minor-1 message; refused (`PeerSendRefused`) when not negotiated.
    Peer(PeerMsg),
}

/// Inputs from the control plane.
#[derive(Debug, Clone, PartialEq)]
pub enum ModemSignal {
    Buffer(u64),
    Disconnected,
    ControlLost,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SessionEvent {
    /// `minor` is the peer's; `caps` are ours ∩ the peer's.
    Negotiated {
        peer_app: String,
        peer_version: String,
        minor: u8,
        chunk: u32,
        caps: Vec<String>,
        /// The peer's HELLO `max_file`: the largest file it takes.
        max_file: u64,
    },
    ChatSubmitted {
        id: ObjectId,
        at: Instant,
    },
    ChatReceived {
        id: ObjectId,
        text: String,
        duplicate: bool,
    },
    ChatDelivered {
        id: ObjectId,
        at: Instant,
    },
    FileOffered {
        id: ObjectId,
        name: String,
        size: u64,
        wire_size: u64,
        encoding: Option<Encoding>,
    },
    /// Cancelled by the operator here (`by_peer` false) or by the peer.
    FileCancelled {
        id: ObjectId,
        by_peer: bool,
    },
    FileSending {
        id: ObjectId,
        sha256: String,
    },
    FileProgress {
        id: ObjectId,
        bytes: u64,
        total: u64,
    },
    FileDelivered {
        id: ObjectId,
    },
    FileReceived {
        id: ObjectId,
        path: PathBuf,
        sha256: String,
    },
    FileRejected {
        id: ObjectId,
        reason: String,
    },
    FileFailed {
        id: ObjectId,
        reason: String,
    },
    PeerError {
        code: String,
        text: String,
    },
    /// A minor-1 message from the peer.
    Peer(PeerMsg),
    /// A minor-1 send that never went on the wire.
    PeerSendRefused {
        what: &'static str,
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum SessionEnd {
    Disconnected,
    ControlLost,
    DataLost,
    Protocol(String),
    HelloTimeout,
    CommandsClosed,
    /// Host-side sockets were dropped deliberately (lab fault injection).
    LocalDrop,
}

struct OutFile {
    id: ObjectId,
    prepared: Prepared,
    /// Wire bytes.
    size: u64,
    chunk: usize,
    sent: u64,
    file: Option<tokio::fs::File>,
    state: OutFileState,
}

#[derive(PartialEq, Debug)]
enum OutFileState {
    Offered,
    Sending,
    AwaitingReceipt,
}

struct InFile {
    id: ObjectId,
    name: String,
    /// The file as it lands on disk.
    size: u64,
    wire_size: u64,
    encoding: Option<Encoding>,
    sha256: String,
    /// Wire bytes so far.
    received: u64,
    /// The wire bytes (`tmp_dir`), then the decoded file (`files_dir/.partial`).
    wire: PathBuf,
    out: PathBuf,
    file: Option<tokio::fs::File>,
    accepted: bool,
    deadline: tokio::time::Instant,
    extended: bool,
    /// FILE_END is in; decoding and the hash check run on a blocking task.
    verifying: bool,
    /// The peer cancelled while verifying: discard what the check returns.
    cancelled: bool,
}

/// How long after a write a BUFFER report is trusted to include it.
const REPORT_SETTLE: Duration = Duration::from_millis(250);
/// Safety margin on top of the file and its wire bytes (M4 C3).
const DISK_MARGIN: u64 = 1 << 20;
/// The largest file MercuryChat handles (owner, 24 Nov: no 1 MiB product cap;
/// each receiver sets its own limit). A memory guard, not an airtime rule:
/// files are prepared and compressed in memory. 256 MiB is about a month on
/// air at 100 B/s.
pub const MAX_FILE_CEILING: u64 = 256 << 20;

type Verdict = Result<(), (String, String)>;

pub struct Session<'a> {
    cfg: SessionConfig,
    store: &'a mut Store,
    events: mpsc::UnboundedSender<SessionEvent>,
    decoder: Decoder,
    nonce: ObjectId,
    peer_hello_seen: bool,
    ack_seen: bool,
    peer_max_chunk: u32,
    peer_info: Option<(String, String, u8)>,
    /// Ours ∩ the peer's, once its HELLO is in.
    caps: Vec<String>,
    q_proto: VecDeque<Frame>,
    q_chat: VecDeque<(ObjectId, Frame)>,
    pending_user: VecDeque<SessionCommand>,
    out_file: Option<OutFile>,
    queued_files: VecDeque<(ObjectId, Prepared)>,
    in_file: Option<InFile>,
    /// Outgoing files cancelled after FILE_END went out, with their wire
    /// size: the peer may have saved one before the cancel reached it, and
    /// its receipt then wins.
    cancelled_after_end: VecDeque<(ObjectId, u64)>,
    peer_max_file: u64,
    verify: Option<(ObjectId, oneshot::Receiver<Verdict>)>,
    pacer: Pacer,
    modem_reported: u64,
    written_since_report: u64,
    /// When we last wrote to the data stream: a BUFFER report soon after may
    /// not include it yet (the modem has not read it).
    last_write: Option<std::time::Instant>,
}

fn proto_err(e: impl std::fmt::Display) -> SessionEnd {
    SessionEnd::Protocol(e.to_string())
}

impl<'a> Session<'a> {
    pub fn new(cfg: SessionConfig, store: &'a mut Store, events: mpsc::UnboundedSender<SessionEvent>) -> Self {
        let pacer = Pacer::new(cfg.nominal_bps, cfg.chunk as usize);
        Session {
            cfg,
            store,
            events,
            decoder: Decoder::new(),
            nonce: ObjectId::random(),
            peer_hello_seen: false,
            ack_seen: false,
            peer_max_chunk: 0,
            peer_info: None,
            caps: Vec::new(),
            q_proto: VecDeque::new(),
            q_chat: VecDeque::new(),
            pending_user: VecDeque::new(),
            out_file: None,
            queued_files: VecDeque::new(),
            in_file: None,
            cancelled_after_end: VecDeque::new(),
            peer_max_file: 0,
            verify: None,
            pacer,
            modem_reported: 0,
            written_since_report: 0,
            last_write: None,
        }
    }

    /// Whether a frame of this type may cross: minor-0 types always, minor-1
    /// types only with their capability in the negotiated set.
    fn negotiated(&self, t: FrameType) -> bool {
        t.capability().is_none_or(|c| self.caps.iter().any(|n| n == c))
    }

    fn ready(&self) -> bool {
        self.peer_hello_seen && self.ack_seen
    }

    /// Emit `Negotiated` once both our HELLO was acknowledged and the peer's
    /// HELLO was accepted; user traffic flows only from this point.
    fn maybe_ready(&mut self) {
        if self.ready() {
            if let Some((app, ver, minor)) = self.peer_info.take() {
                let chunk = self.agreed_chunk();
                let caps = self.caps.clone();
                let max_file = self.peer_max_file;
                self.emit(SessionEvent::Negotiated { peer_app: app, peer_version: ver, minor, chunk, caps, max_file });
            }
        }
    }

    fn emit(&self, e: SessionEvent) {
        let _ = self.events.send(e);
    }

    fn agreed_chunk(&self) -> u32 {
        self.cfg.chunk.min(self.peer_max_chunk.max(1)).min(mch_protocol::MAX_CHUNK_DATA as u32)
    }

    /// Run until the link ends. The data stream stays owned by the caller.
    pub async fn run<S>(
        mut self,
        data: &mut S,
        cmds: &mut mpsc::Receiver<SessionCommand>,
        modem: &mut mpsc::Receiver<ModemSignal>,
    ) -> SessionEnd
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let end = self.run_inner(data, cmds, modem).await;
        // Truthful states on any ending: nothing in flight is "delivered".
        let _ = self.store.mark_in_flight_uncertain();
        if let Some(of) = self.out_file.take() {
            let _ = self.store.transfer_set_state(Direction::Out, &of.id.0, TransferState::Interrupted, of.sent, None);
            let _ = tokio::fs::remove_file(&of.prepared.path).await;
        }
        for (_, p) in self.queued_files.drain(..) {
            let _ = tokio::fs::remove_file(&p.path).await;
        }
        if let Some(inf) = self.in_file.take() {
            drop(inf.file);
            let _ = tokio::fs::remove_file(&inf.wire).await;
            let _ = tokio::fs::remove_file(&inf.out).await;
            let _ =
                self.store.transfer_set_state(Direction::In, &inf.id.0, TransferState::Interrupted, inf.received, None);
        }
        let _ = self.store.log_event("session", &format!("gen {} ended: {end:?}", self.cfg.generation));
        end
    }

    async fn run_inner<S>(
        &mut self,
        data: &mut S,
        cmds: &mut mpsc::Receiver<SessionCommand>,
        modem: &mut mpsc::Receiver<ModemSignal>,
    ) -> SessionEnd
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let hello = Hello {
            app: APP_NAME.into(),
            app_version: APP_VERSION.into(),
            proto_minor: self.cfg.proto_minor,
            nonce: self.nonce.to_hex(),
            callsign: self.cfg.local_call.clone(),
            capabilities: self.cfg.capabilities.clone(),
            max_frame: mch_protocol::MAX_PAYLOAD as u32,
            max_file: self.cfg.max_file,
            max_chunk: self.cfg.chunk,
        };
        match hello.to_frame() {
            Ok(f) => self.q_proto.push_back(f),
            Err(e) => return proto_err(e),
        }
        let hello_deadline = tokio::time::Instant::now() + self.cfg.hello_timeout;
        let mut buf = vec![0u8; 8192];
        let mut cmds_open = true;

        loop {
            // 1. Flush everything that may be written now, highest priority first.
            if let Err(e) = self.flush(data).await {
                return e;
            }

            // 2. Wait for input.
            let ready = self.ready();
            let offer_deadline = self.in_file.as_ref().filter(|i| !i.accepted).map(|i| i.deadline);
            let offer_wait = async move {
                match offer_deadline {
                    Some(d) => tokio::time::sleep_until(d).await,
                    None => std::future::pending::<()>().await,
                }
            };
            let verify = &mut self.verify;
            let verified = async move {
                match verify.as_mut() {
                    Some((_, rx)) => rx.await.unwrap_or(Err(("verify".into(), "the check stopped".into()))),
                    None => std::future::pending().await,
                }
            };
            let hello_wait = async move {
                if ready {
                    std::future::pending::<()>().await
                } else {
                    tokio::time::sleep_until(hello_deadline).await
                }
            };
            tokio::select! {
                biased;
                sig = modem.recv() => match sig {
                    Some(ModemSignal::Buffer(n)) => {
                        // A rise is our bytes reaching the modem's queue: they
                        // are no longer in flight. A report may lag our writes
                        // (sent before the modem read them), so only one well
                        // after the last write clears what is in flight.
                        if n > self.modem_reported {
                            self.written_since_report = self.written_since_report.saturating_sub(n - self.modem_reported);
                        }
                        self.modem_reported = n;
                        if self.last_write.is_none_or(|t| t.elapsed() >= REPORT_SETTLE) {
                            self.written_since_report = 0;
                        }
                        self.pacer.observe_buffer(n, std::time::Instant::now());
                    }
                    Some(ModemSignal::Disconnected) => return SessionEnd::Disconnected,
                    Some(ModemSignal::ControlLost) | None => return SessionEnd::ControlLost,
                },
                r = data.read(&mut buf) => match r {
                    Ok(0) | Err(_) => return SessionEnd::DataLost,
                    Ok(n) => {
                        self.decoder.push(&buf[..n]);
                        loop {
                            match self.decoder.next_frame() {
                                Ok(Some(f)) => if let Err(e) = self.on_frame(f).await { return e; },
                                Ok(None) => break,
                                Err(e) => {
                                    self.send_error("protocol", &e.to_string());
                                    let _ = self.flush(data).await;
                                    return proto_err(e);
                                }
                            }
                        }
                    }
                },
                c = cmds.recv(), if cmds_open => match c {
                    Some(c) => if self.ready() { self.on_command(c).await } else { self.pending_user.push_back(c) },
                    None => { cmds_open = false; }
                },
                _ = hello_wait => return SessionEnd::HelloTimeout,
                _ = offer_wait => self.offer_expired().await,
                v = verified => {
                    if let Some((id, _)) = self.verify.take() {
                        self.on_verified(id, v).await;
                    }
                },
                // Re-check pacing periodically when a file is waiting for room.
                _ = tokio::time::sleep(Duration::from_millis(250)), if self.out_file.as_ref().is_some_and(|f| f.state == OutFileState::Sending) => {}
            }
        }
    }

    fn send_error(&mut self, code: &str, text: &str) {
        if let Ok(f) = Reason::new(code, text).to_frame(FrameType::Error, ObjectId::ZERO) {
            self.q_proto.push_back(f);
        }
    }

    async fn write_frame<S: AsyncWrite + Unpin>(&mut self, data: &mut S, f: &Frame) -> Result<(), SessionEnd> {
        let bytes = encode(f).map_err(proto_err)?;
        data.write_all(&bytes).await.map_err(|_| SessionEnd::DataLost)?;
        self.written_since_report += bytes.len() as u64;
        self.last_write = Some(std::time::Instant::now());
        Ok(())
    }

    async fn flush<S: AsyncWrite + Unpin>(&mut self, data: &mut S) -> Result<(), SessionEnd> {
        while let Some(f) = self.q_proto.pop_front() {
            self.write_frame(data, &f).await?;
        }
        if !self.ready() {
            return Ok(());
        }
        while let Some(c) = self.pending_user.pop_front() {
            self.on_command(c).await;
            while let Some(f) = self.q_proto.pop_front() {
                self.write_frame(data, &f).await?;
            }
        }
        while let Some((id, f)) = self.q_chat.pop_front() {
            let peer = self.cfg.peer_call.clone();
            let _ = self.store.set_out_state(&peer, &id.0, OutState::Submitting, Some(self.cfg.generation));
            self.write_frame(data, &f).await?;
            let _ = self.store.set_out_state(&peer, &id.0, OutState::AwaitingReceipt, None);
            self.emit(SessionEvent::ChatSubmitted { id, at: Instant::now() });
        }
        // File chunks only while the queue holds under ~12 s of airtime (M4
        // C4), and only when no chat/protocol frame is waiting.
        loop {
            let est = self.modem_reported + self.written_since_report;
            let Some(of) = self.out_file.as_mut() else { break };
            if of.state != OutFileState::Sending || !self.pacer.may_send(est) {
                break;
            }
            let id = of.id;
            if of.sent >= of.size {
                of.state = OutFileState::AwaitingReceipt;
                let sent = of.sent;
                let f = Frame::empty(FrameType::FileEnd, id);
                self.write_frame(data, &f).await?;
                let _ = self.store.transfer_set_state(Direction::Out, &id.0, TransferState::Sending, sent, None);
                break;
            }
            let want = (of.size - of.sent).min(self.pacer.chunk_size().min(of.chunk) as u64) as usize;
            let mut chunk = vec![0u8; want];
            let file = of.file.as_mut().expect("open while sending");
            if file.read_exact(&mut chunk).await.is_err() {
                let id = of.id;
                self.fail_out_file(id, "local file read failed");
                break;
            }
            let offset = of.sent;
            of.sent += want as u64;
            let (sent, size) = (of.sent, of.size);
            let f = chunk_frame(id, offset, &chunk).map_err(proto_err)?;
            self.write_frame(data, &f).await?;
            self.emit(SessionEvent::FileProgress { id, bytes: sent, total: size });
        }
        Ok(())
    }

    fn fail_out_file(&mut self, id: ObjectId, reason: &str) {
        if let Ok(f) = Reason::new("sender_error", reason).to_frame(FrameType::FileCancel, id) {
            self.q_proto.push_back(f);
        }
        if let Some(of) = self.out_file.take() {
            let _ = self.store.transfer_set_state(Direction::Out, &id.0, TransferState::Failed, of.sent, Some(reason));
            let _ = std::fs::remove_file(&of.prepared.path);
        }
        self.emit(SessionEvent::FileFailed { id, reason: reason.into() });
    }

    /// The outgoing transfer ended (receipt, reject, cancel): its prepared
    /// file goes, and the next one is offered.
    async fn end_out_file(&mut self, state: TransferState, reason: Option<&str>) {
        if let Some(of) = self.out_file.take() {
            let _ = self.store.transfer_set_state(Direction::Out, &of.id.0, state, of.sent, reason);
            let _ = tokio::fs::remove_file(&of.prepared.path).await;
        }
        self.start_next_file().await;
    }

    async fn on_command(&mut self, c: SessionCommand) {
        match c {
            SessionCommand::SendChat { id, text } => {
                let peer = self.cfg.peer_call.clone();
                if self.store.out_state(&peer, &id.0).ok().flatten().is_none()
                    && self.store.insert_outgoing(&peer, &id.0, &text).is_err()
                {
                    return;
                }
                match chat_frame(id, &text) {
                    Ok(f) => self.q_chat.push_back((id, f)),
                    Err(_) => {
                        let _ = self.store.set_out_state(&peer, &id.0, OutState::Failed, None);
                    }
                }
            }
            SessionCommand::SendFile { id, prepared } => {
                let refusal = if !self.negotiated(FrameType::FileOffer) {
                    Some("not supported by this station")
                } else if prepared.size > MAX_FILE_CEILING
                    || (self.peer_max_file > 0 && prepared.size > self.peer_max_file)
                {
                    Some("too large")
                } else {
                    None
                };
                if let Some(reason) = refusal {
                    let _ = tokio::fs::remove_file(&prepared.path).await;
                    self.emit(SessionEvent::FileRejected { id, reason: reason.into() });
                    return;
                }
                self.queued_files.push_back((id, prepared));
                self.start_next_file().await;
            }
            SessionCommand::AcceptFile { id } => self.accept_in_file(id).await,
            SessionCommand::RejectFile { id } => self.reject_in_file(id, "declined").await,
            SessionCommand::ExtendFile { id } => {
                let window = self.cfg.offer_timeout;
                if let Some(inf) = self.in_file.as_mut().filter(|i| i.id == id && !i.accepted && !i.extended) {
                    inf.extended = true;
                    inf.deadline += window;
                }
            }
            SessionCommand::CancelFile { id } => {
                let cancel = || Reason::new("user", "cancelled").to_frame(FrameType::FileCancel, id).ok();
                if let Some(of) = self.out_file.as_ref().filter(|f| f.id == id) {
                    if of.state == OutFileState::AwaitingReceipt {
                        if self.cancelled_after_end.len() >= 8 {
                            self.cancelled_after_end.pop_front();
                        }
                        self.cancelled_after_end.push_back((id, of.sent));
                    }
                    if let Some(f) = cancel() {
                        self.q_proto.push_back(f);
                    }
                    self.end_out_file(TransferState::Cancelled, Some("user")).await;
                    self.emit(SessionEvent::FileCancelled { id, by_peer: false });
                } else if let Some(i) = self.queued_files.iter().position(|(q, _)| *q == id) {
                    if let Some((_, p)) = self.queued_files.remove(i) {
                        let _ = tokio::fs::remove_file(&p.path).await;
                    }
                    self.emit(SessionEvent::FileCancelled { id, by_peer: false });
                } else if self.in_file.as_ref().is_some_and(|i| i.id == id) {
                    if let Some(f) = cancel() {
                        self.q_proto.push_back(f);
                    }
                    self.cancel_in_file(id, false).await;
                }
            }
            SessionCommand::Peer(m) => {
                let what = m.what();
                if !self.negotiated(m.ftype()) {
                    let reason = "not supported by this station".to_string();
                    self.emit(SessionEvent::PeerSendRefused { what, reason });
                    return;
                }
                match m.to_frame() {
                    Ok(f) => self.q_proto.push_back(f),
                    Err(e) => self.emit(SessionEvent::PeerSendRefused { what, reason: e.to_string() }),
                }
            }
        }
    }

    /// Offer the next queued file, unless one is going either way: one
    /// transfer at a time per session (M4 C2).
    async fn start_next_file(&mut self) {
        if self.out_file.is_some() || self.in_file.is_some() {
            return;
        }
        let Some((id, p)) = self.queued_files.pop_front() else { return };
        let offer = FileOffer {
            name: p.name.clone(),
            size: p.size,
            sha256: p.sha256.clone(),
            mime: p.mime.clone(),
            encoding: p.encoding,
            wire_size: p.wire_size,
        };
        let frame = match offer.to_frame(id) {
            Ok(f) => f,
            Err(e) => {
                let _ = tokio::fs::remove_file(&p.path).await;
                self.emit(SessionEvent::FileFailed { id, reason: format!("the offer is invalid: {e}") });
                return;
            }
        };
        let row = TransferRow {
            id: id.0,
            direction: Direction::Out,
            peer: self.cfg.peer_call.clone(),
            session_id: None,
            original_name: p.name.clone(),
            saved_name: None,
            size: p.size,
            wire_size: p.wire_size,
            sha256: p.sha256.clone(),
            encoding: p.encoding.map(|_| "zstd".to_string()),
            state: TransferState::Offered,
            bytes_done: 0,
            reason: None,
            created_utc: String::new(),
            updated_utc: String::new(),
            saved_path: None,
        };
        let _ = self.store.transfer_upsert(&row);
        self.q_proto.push_back(frame);
        let sha256 = p.sha256.clone();
        let size = p.wire_size;
        self.out_file =
            Some(OutFile { id, prepared: p, size, chunk: 0, sent: 0, file: None, state: OutFileState::Offered });
        self.emit(SessionEvent::FileSending { id, sha256 });
    }

    async fn on_frame(&mut self, f: Frame) -> Result<(), SessionEnd> {
        let peer = self.cfg.peer_call.clone();
        match f.ftype {
            FrameType::Hello => {
                if self.peer_hello_seen {
                    return Err(SessionEnd::Protocol("duplicate HELLO".into()));
                }
                let h = Hello::from_frame(&f).map_err(proto_err)?;
                if !h.callsign.eq_ignore_ascii_case(&self.cfg.peer_call) {
                    self.send_error("identity", "HELLO callsign does not match CONNECTED endpoint");
                    return Err(SessionEnd::Protocol(format!(
                        "peer HELLO callsign {} != CONNECTED peer {}",
                        h.callsign, self.cfg.peer_call
                    )));
                }
                self.peer_hello_seen = true;
                self.peer_max_chunk = h.max_chunk;
                self.peer_max_file = h.max_file;
                let caps: Vec<String> =
                    self.cfg.capabilities.iter().filter(|c| h.capabilities.contains(c)).cloned().collect();
                self.caps = caps.clone();
                let minor = self.cfg.proto_minor.min(h.proto_minor);
                let ack = HelloAck { nonce: h.nonce.clone(), major: 1, minor, capabilities: caps };
                self.q_proto.push_back(ack.to_frame().map_err(proto_err)?);
                let chunk = self.agreed_chunk();
                self.decoder.set_chunk_limit(chunk as usize);
                self.peer_info = Some((h.app, h.app_version, h.proto_minor));
                self.maybe_ready();
            }
            FrameType::HelloAck => {
                let a = HelloAck::from_frame(&f).map_err(proto_err)?;
                if a.nonce != self.nonce.to_hex() || a.major != 1 {
                    return Err(SessionEnd::Protocol("HELLO_ACK nonce/version mismatch".into()));
                }
                self.ack_seen = true;
                self.maybe_ready();
            }
            _ if !self.ready() => {
                return Err(SessionEnd::Protocol(format!("{:?} before negotiation completed", f.ftype)));
            }
            FrameType::Chat => {
                let text = chat_text(&f).map_err(proto_err)?;
                // Durable store (with dedup key) strictly before the receipt.
                let stored = self
                    .store
                    .store_incoming_chat(&peer, &f.id.0, &text, self.cfg.generation)
                    .map_err(|e| SessionEnd::Protocol(format!("storage failed, no receipt: {e}")))?;
                self.q_proto.push_back(Frame::empty(FrameType::ChatReceipt, f.id));
                self.emit(SessionEvent::ChatReceived { id: f.id, text, duplicate: stored == Stored::Duplicate });
            }
            FrameType::ChatReceipt => {
                if self.store.out_state(&peer, &f.id.0).ok().flatten().is_some() {
                    let _ = self.store.set_out_state(&peer, &f.id.0, OutState::Delivered, None);
                    self.emit(SessionEvent::ChatDelivered { id: f.id, at: Instant::now() });
                } else {
                    let _ = self.store.log_event("protocol", &format!("receipt for unknown message {}", f.id));
                }
            }
            // Files need file/2 (M4 C1); without it the frame ends the session.
            FrameType::FileOffer
            | FrameType::FileAccept
            | FrameType::FileReject
            | FrameType::FileChunk
            | FrameType::FileEnd
            | FrameType::FileReceipt
            | FrameType::FileCancel
                if !self.negotiated(f.ftype) =>
            {
                return Err(SessionEnd::Protocol(format!("{:?} frame without its negotiated capability", f.ftype)));
            }
            FrameType::FileOffer => self.on_offer(&f).await?,
            FrameType::FileAccept => {
                let a = FileAccept::from_frame(&f).map_err(proto_err)?;
                let Some(of) = self.out_file.as_mut().filter(|o| o.id == f.id && o.state == OutFileState::Offered)
                else {
                    return Err(SessionEnd::Protocol("FILE_ACCEPT for no pending offer".into()));
                };
                of.chunk = (a.chunk as usize).min(mch_protocol::MAX_CHUNK_DATA).min(self.cfg.chunk as usize);
                match tokio::fs::File::open(&of.prepared.path).await {
                    Ok(file) => {
                        of.file = Some(file);
                        of.state = OutFileState::Sending;
                        let _ = self.store.transfer_set_state(Direction::Out, &f.id.0, TransferState::Sending, 0, None);
                    }
                    Err(_) => self.fail_out_file(f.id, "local file vanished"),
                }
            }
            FrameType::FileReject => {
                let r = Reason::from_frame(&f).map_err(proto_err)?;
                if self.out_file.as_ref().is_some_and(|o| o.id == f.id) {
                    self.end_out_file(TransferState::Rejected, Some(&r.code)).await;
                    self.emit(SessionEvent::FileRejected { id: f.id, reason: r.code.clone() });
                }
            }
            FrameType::FileReceipt => {
                if self.out_file.as_ref().is_some_and(|o| o.id == f.id && o.state == OutFileState::AwaitingReceipt) {
                    self.end_out_file(TransferState::Complete, None).await;
                    self.emit(SessionEvent::FileDelivered { id: f.id });
                } else if let Some(i) = self.cancelled_after_end.iter().position(|(c, _)| *c == f.id) {
                    // Our cancel came too late: the peer has the file.
                    let (_, sent) = self.cancelled_after_end.remove(i).expect("found");
                    let _ = self.store.transfer_set_state(Direction::Out, &f.id.0, TransferState::Complete, sent, None);
                    self.emit(SessionEvent::FileDelivered { id: f.id });
                } else {
                    let _ = self.store.log_event("protocol", &format!("unexpected FILE_RECEIPT {}", f.id));
                }
            }
            FrameType::FileChunk => self.on_chunk(&f).await?,
            FrameType::FileEnd => self.on_file_end(f.id).await?,
            FrameType::FileCancel => {
                let r = Reason::from_frame(&f).map_err(proto_err)?;
                if self.in_file.as_ref().is_some_and(|i| i.id == f.id) {
                    self.cancel_in_file(f.id, true).await;
                } else if self.out_file.as_ref().is_some_and(|o| o.id == f.id) {
                    if r.code == "user" {
                        self.end_out_file(TransferState::Cancelled, Some("user")).await;
                        self.emit(SessionEvent::FileCancelled { id: f.id, by_peer: true });
                    } else {
                        let reason = format!("receiver: {}: {}", r.code, r.text);
                        self.end_out_file(TransferState::Failed, Some(&r.code)).await;
                        self.emit(SessionEvent::FileFailed { id: f.id, reason });
                    }
                }
            }
            FrameType::Error => {
                let r = Reason::from_frame(&f).map_err(proto_err)?;
                self.emit(SessionEvent::PeerError { code: r.code, text: r.text });
            }
            // A minor-1 type without its negotiated capability ends the session (B1).
            FrameType::QsyPropose | FrameType::QsyAnswer | FrameType::InfoRequest | FrameType::InfoResponse => {
                if !self.negotiated(f.ftype) {
                    return Err(SessionEnd::Protocol(format!("{:?} frame without its negotiated capability", f.ftype)));
                }
                if let Some(m) = PeerMsg::from_frame(&f) {
                    self.emit(SessionEvent::Peer(m.map_err(proto_err)?));
                }
            }
        }
        Ok(())
    }

    async fn on_offer(&mut self, f: &Frame) -> Result<(), SessionEnd> {
        let mut o = FileOffer::from_frame(f).map_err(proto_err)?;
        // A transfer id we already hold (from any station) is refused, and its
        // record is left alone: ids travel in the clear and could be reused.
        if self.store.transfer(Direction::In, &f.id.0).ok().flatten().is_some() {
            self.send_reject(f.id, "duplicate");
            let _ = self.store.log_event("file", &format!("offer reuses transfer id {}", f.id));
            return Ok(());
        }
        // What the operator sees can't be disguised by format characters.
        o.name = display_name(&o.name);
        let row = TransferRow {
            id: f.id.0,
            direction: Direction::In,
            peer: self.cfg.peer_call.clone(),
            session_id: None,
            original_name: o.name.clone(),
            saved_name: None,
            size: o.size,
            wire_size: o.wire_size,
            sha256: o.sha256.clone(),
            encoding: o.encoding.map(|_| "zstd".to_string()),
            state: TransferState::Offered,
            bytes_done: 0,
            reason: None,
            created_utc: String::new(),
            updated_utc: String::new(),
            saved_path: None,
        };
        let _ = self.store.transfer_upsert(&row);
        // Refused before the operator sees it (M4 C3).
        let free = self.cfg.free_space.of(&self.cfg.files_dir);
        // One transfer at a time. Offers that crossed on the air: the larger
        // transfer id goes, on both sides, and the other is refused as busy.
        let ours_goes = self.out_file.as_ref().is_some_and(|o| o.state != OutFileState::Offered || o.id.0 > f.id.0);
        let refusal = if self.in_file.is_some() || ours_goes {
            Some("busy")
        } else if o.size == 0 || o.size > self.cfg.max_file.min(MAX_FILE_CEILING) {
            Some("too large")
        } else if !self.cfg.files_dir.is_dir() {
            Some("no folder")
        } else if !can_write(&self.cfg.files_dir.join(".partial"), f.id) {
            Some("write failed")
        } else if free.map_or(true, |n| n < o.size.saturating_add(o.wire_size).saturating_add(DISK_MARGIN)) {
            Some("no space")
        } else {
            None
        };
        if let Some(code) = refusal {
            self.send_reject(f.id, code);
            let _ = self.store.transfer_set_state(Direction::In, &f.id.0, TransferState::Rejected, 0, Some(code));
            // The operator here sees what was declined for them, and why.
            self.emit(SessionEvent::FileRejected { id: f.id, reason: code.into() });
            return Ok(());
        }
        self.in_file = Some(InFile {
            id: f.id,
            name: o.name.clone(),
            size: o.size,
            wire_size: o.wire_size,
            encoding: o.encoding,
            sha256: o.sha256,
            received: 0,
            wire: self.cfg.tmp_dir.join(format!("{}.wire", f.id)),
            out: self.cfg.files_dir.join(".partial").join(format!("{}.part", f.id)),
            file: None,
            accepted: false,
            deadline: tokio::time::Instant::now() + self.cfg.offer_timeout,
            extended: false,
            verifying: false,
            cancelled: false,
        });
        self.emit(SessionEvent::FileOffered {
            id: f.id,
            name: o.name,
            size: o.size,
            wire_size: o.wire_size,
            encoding: o.encoding,
        });
        if self.cfg.auto_accept_files {
            self.accept_in_file(f.id).await;
        }
        Ok(())
    }

    fn send_reject(&mut self, id: ObjectId, code: &str) {
        if let Ok(fr) = Reason::new(code, code).to_frame(FrameType::FileReject, id) {
            self.q_proto.push_back(fr);
        }
    }

    async fn accept_in_file(&mut self, id: ObjectId) {
        let chunk = self.agreed_chunk();
        let tmp = self.cfg.tmp_dir.clone();
        let Some(inf) = self.in_file.as_mut().filter(|i| i.id == id && !i.accepted) else { return };
        let opened = async {
            tokio::fs::create_dir_all(&tmp).await?;
            tokio::fs::OpenOptions::new().write(true).create_new(true).open(&inf.wire).await
        }
        .await;
        match opened {
            Ok(file) => {
                inf.file = Some(file);
                inf.accepted = true;
                if let Ok(fr) = (FileAccept { chunk }).to_frame(id) {
                    self.q_proto.push_back(fr);
                }
                let _ = self.store.transfer_set_state(Direction::In, &id.0, TransferState::Receiving, 0, None);
            }
            Err(e) => {
                tracing::warn!("cannot create the file's temp file: {e}");
                self.reject_in_file(id, "write failed").await;
            }
        }
    }

    /// Our operator declined, the window ran out, or the file can't be stored.
    async fn reject_in_file(&mut self, id: ObjectId, code: &str) {
        if let Some(inf) = self.in_file.take_if(|i| i.id == id && !i.verifying) {
            drop(inf.file);
            let _ = tokio::fs::remove_file(&inf.wire).await;
            self.send_reject(id, code);
            let _ = self.store.transfer_set_state(Direction::In, &id.0, TransferState::Rejected, 0, Some(code));
            self.emit(SessionEvent::FileRejected { id, reason: code.into() });
        }
        self.start_next_file().await;
    }

    async fn offer_expired(&mut self) {
        let now = tokio::time::Instant::now();
        if let Some(id) = self.in_file.as_ref().filter(|i| !i.accepted && now >= i.deadline).map(|i| i.id) {
            self.reject_in_file(id, "timeout").await;
        }
    }

    /// Cancelled here or by the peer. While the check runs, its result is
    /// discarded when it returns: nothing is published, no receipt (M4).
    async fn cancel_in_file(&mut self, id: ObjectId, by_peer: bool) {
        let Some(inf) = self.in_file.as_mut().filter(|i| i.id == id) else { return };
        let received = inf.received;
        if inf.verifying {
            inf.cancelled = true;
        } else if let Some(inf) = self.in_file.take() {
            drop(inf.file);
            let _ = tokio::fs::remove_file(&inf.wire).await;
            let _ = tokio::fs::remove_file(&inf.out).await;
        }
        let code = if by_peer { "peer" } else { "user" };
        let _ = self.store.transfer_set_state(Direction::In, &id.0, TransferState::Cancelled, received, Some(code));
        self.emit(SessionEvent::FileCancelled { id, by_peer });
        self.start_next_file().await;
    }

    async fn abort_in_file(&mut self, id: ObjectId, code: &str, why: &str) {
        if let Some(inf) = self.in_file.take() {
            drop(inf.file);
            let _ = tokio::fs::remove_file(&inf.wire).await;
            let _ = tokio::fs::remove_file(&inf.out).await;
            let _ =
                self.store.transfer_set_state(Direction::In, &id.0, TransferState::Failed, inf.received, Some(code));
        }
        if let Ok(fr) = Reason::new(code, why).to_frame(FrameType::FileCancel, id) {
            self.q_proto.push_back(fr);
        }
        self.emit(SessionEvent::FileFailed { id, reason: format!("{code}: {why}") });
        self.start_next_file().await;
    }

    async fn on_chunk(&mut self, f: &Frame) -> Result<(), SessionEnd> {
        let (off, bytes) = parse_chunk(f).map_err(proto_err)?;
        let Some(inf) = self.in_file.as_mut().filter(|i| i.id == f.id && i.accepted && !i.verifying) else {
            // Chunks already queued in the modem after a cancel are discarded.
            let _ = self.store.log_event("file", &format!("discarded chunk for inactive transfer {}", f.id));
            return Ok(());
        };
        if off != inf.received || inf.received + bytes.len() as u64 > inf.wire_size {
            let why =
                format!("offset {off} len {} at received {} wire size {}", bytes.len(), inf.received, inf.wire_size);
            self.abort_in_file(f.id, "bad_offset", &why).await;
            return Ok(());
        }
        let file = inf.file.as_mut().unwrap();
        if file.write_all(bytes).await.is_err() {
            self.abort_in_file(f.id, "disk", "write failed").await;
            return Ok(());
        }
        inf.received += bytes.len() as u64;
        let (received, total) = (inf.received, inf.wire_size);
        self.emit(SessionEvent::FileProgress { id: f.id, bytes: received, total });
        Ok(())
    }

    /// All wire bytes are in: decode and check them off the session task.
    async fn on_file_end(&mut self, id: ObjectId) -> Result<(), SessionEnd> {
        let Some(inf) = self.in_file.as_mut().filter(|i| i.id == id && i.accepted && !i.verifying) else {
            return Ok(());
        };
        let flushed = match inf.file.take() {
            Some(file) => file.sync_all().await,
            None => Ok(()),
        };
        let (received, wire_size) = (inf.received, inf.wire_size);
        if flushed.is_err() {
            self.abort_in_file(id, "disk", "fsync failed").await;
            return Ok(());
        }
        if received != wire_size {
            self.abort_in_file(id, "short", &format!("received {received} of {wire_size}")).await;
            return Ok(());
        }
        inf.verifying = true;
        let (wire, out, encoding, size, sha) =
            (inf.wire.clone(), inf.out.clone(), inf.encoding, inf.size, inf.sha256.clone());
        let _ = self.store.transfer_set_state(Direction::In, &id.0, TransferState::Verifying, received, None);
        let (tx, rx) = oneshot::channel();
        tokio::task::spawn_blocking(move || {
            let _ = tx.send(verify_file(&wire, &out, encoding, size, &sha));
        });
        self.verify = Some((id, rx));
        Ok(())
    }

    /// The check is back: publish, store completion, and only then
    /// FILE_RECEIPT. Then a waiting outgoing file may go.
    async fn on_verified(&mut self, id: ObjectId, verdict: Verdict) {
        self.publish_verified(id, verdict).await;
        self.start_next_file().await;
    }

    async fn publish_verified(&mut self, id: ObjectId, verdict: Verdict) {
        let Some(inf) = self.in_file.take_if(|i| i.id == id) else { return };
        let _ = tokio::fs::remove_file(&inf.wire).await;
        if inf.cancelled {
            let _ = tokio::fs::remove_file(&inf.out).await;
            return;
        }
        if let Err((code, why)) = verdict {
            let _ = tokio::fs::remove_file(&inf.out).await;
            let _ =
                self.store.transfer_set_state(Direction::In, &id.0, TransferState::Failed, inf.received, Some(&code));
            if let Ok(fr) = Reason::new(&code, &why).to_frame(FrameType::FileCancel, id) {
                self.q_proto.push_back(fr);
            }
            self.emit(SessionEvent::FileFailed { id, reason: format!("{code}: {why}") });
            return;
        }
        let target = unique_target(&self.cfg.files_dir, &sanitize_name(&inf.name));
        if let Err(e) = tokio::fs::rename(&inf.out, &target).await {
            let _ = tokio::fs::remove_file(&inf.out).await;
            let _ = self.store.transfer_set_state(
                Direction::In,
                &id.0,
                TransferState::Failed,
                inf.received,
                Some("publish"),
            );
            self.emit(SessionEvent::FileFailed { id, reason: format!("publish failed: {e}") });
            return;
        }
        // The rename itself on disk too, before completion is recorded.
        #[cfg(unix)]
        if let Ok(d) = std::fs::File::open(&self.cfg.files_dir) {
            let _ = d.sync_all();
        }
        let saved = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let path_s = target.to_string_lossy().into_owned();
        // Completion is stored before FILE_RECEIPT is sent (FIL-03).
        if let Err(e) = self.store.transfer_complete(Direction::In, &id.0, Some((&saved, &path_s))) {
            let _ = tokio::fs::remove_file(&target).await;
            self.emit(SessionEvent::FileFailed { id, reason: format!("storage failed, no receipt: {e}") });
            return;
        }
        self.q_proto.push_back(Frame::empty(FrameType::FileReceipt, id));
        self.emit(SessionEvent::FileReceived { id, path: target, sha256: inf.sha256 });
    }
}

/// Whether a file can be created in `dir` (made if missing): checked at the
/// offer, so an unwritable folder is found before any airtime is spent.
fn can_write(dir: &Path, id: ObjectId) -> bool {
    let probe = dir.join(format!("{id}.probe"));
    let ok = std::fs::create_dir_all(dir).is_ok()
        && std::fs::OpenOptions::new().write(true).create_new(true).open(&probe).is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

/// Decode (zstd) or copy the wire bytes to `out`, then check the length and
/// SHA-256 the offer declared. Blocking.
fn verify_file(wire: &Path, out: &Path, encoding: Option<Encoding>, size: u64, sha: &str) -> Verdict {
    use std::io::Read;
    let disk = |e: std::io::Error| ("disk".to_string(), e.to_string());
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir).map_err(disk)?;
    }
    match encoding {
        Some(Encoding::Zstd) => unpack_zstd(wire, out, size).map_err(|e| match e {
            UnpackError::TooLong => ("too_long".to_string(), e.to_string()),
            UnpackError::Corrupt(_) => ("corrupt".to_string(), e.to_string()),
            UnpackError::Io(_) => ("disk".to_string(), e.to_string()),
        })?,
        None => {
            std::fs::copy(wire, out).map_err(disk)?;
            // On disk before the receipt, as the zstd path is (a power cut
            // must not leave a "complete" file empty).
            std::fs::OpenOptions::new().write(true).open(out).and_then(|f| f.sync_all()).map_err(disk)?;
        }
    }
    let mut f = std::fs::File::open(out).map_err(disk)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut len = 0u64;
    loop {
        let n = f.read(&mut buf).map_err(disk)?;
        if n == 0 {
            break;
        }
        len += n as u64;
        h.update(&buf[..n]);
    }
    if len != size {
        return Err(("short".into(), format!("decoded {len} of {size}")));
    }
    let got = hex::encode(h.finalize());
    if got != sha {
        return Err(("hash_mismatch".into(), format!("declared {sha} got {got}")));
    }
    Ok(())
}

impl From<ProtocolError> for SessionEnd {
    fn from(e: ProtocolError) -> Self {
        SessionEnd::Protocol(e.to_string())
    }
}
