//! One MCH/1 application session over an established ARQ link.
//!
//! The session is a single task: it reads the data stream, drains operator
//! commands and modem signals, and writes frames in priority order
//! (protocol/receipts, then chat, then file chunks). File chunks are paced by
//! the modem's reported BUFFER so chat is not stuck behind a large backlog.

use crate::files::{sanitize_name, unique_target};
use mc_storage::{OutState, Store, Stored};
use mch_protocol::payload::*;
use mch_protocol::{encode, Decoder, Frame, FrameType, ObjectId, ProtocolError};
use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;

pub const APP_NAME: &str = "MercuryChat";
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Debug)]
pub struct SessionConfig {
    pub local_call: String,
    pub peer_call: String,
    pub generation: u64,
    pub files_dir: PathBuf,
    pub max_file: u64,
    pub chunk: u32,
    pub auto_accept_files: bool,
    pub hello_timeout: Duration,
    /// Pace file chunks so at most this many bytes sit in the modem queue.
    pub modem_window: u64,
    /// The MCH/1 minor this side announces (`MINOR`; lower only to act as an older peer).
    pub proto_minor: u8,
    /// The capabilities this side announces (`default_capabilities()`).
    pub capabilities: Vec<String>,
}

/// What this version announces in HELLO: chat, files, QSY and info (M3c B1).
pub fn default_capabilities() -> Vec<String> {
    ["chat", "file", mch_protocol::CAP_QSY, mch_protocol::CAP_INFO].iter().map(|c| c.to_string()).collect()
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
    SendFile {
        path: PathBuf,
        opts: SendFileOpts,
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

#[derive(Debug, Clone, Default)]
pub struct SendFileOpts {
    /// Lab fault injection: declare a wrong SHA-256 in the offer.
    pub corrupt_declared_hash: bool,
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
    path: PathBuf,
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
    size: u64,
    sha256: String,
    received: u64,
    hasher: Sha256,
    temp: PathBuf,
    file: Option<tokio::fs::File>,
    accepted: bool,
}

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
    queued_files: VecDeque<(PathBuf, SendFileOpts)>,
    in_file: Option<InFile>,
    modem_reported: u64,
    written_since_report: u64,
}

fn proto_err(e: impl std::fmt::Display) -> SessionEnd {
    SessionEnd::Protocol(e.to_string())
}

async fn sha256_file(path: &Path) -> std::io::Result<(u64, String)> {
    let mut f = tokio::fs::File::open(path).await?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut n_total = 0u64;
    loop {
        let n = f.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        n_total += n as u64;
        h.update(&buf[..n]);
    }
    Ok((n_total, hex::encode(h.finalize())))
}

impl<'a> Session<'a> {
    pub fn new(cfg: SessionConfig, store: &'a mut Store, events: mpsc::UnboundedSender<SessionEvent>) -> Self {
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
            modem_reported: 0,
            written_since_report: 0,
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
                self.emit(SessionEvent::Negotiated { peer_app: app, peer_version: ver, minor, chunk, caps });
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
            let _ = self.store.update_transfer(
                &self.cfg.peer_call,
                "out",
                &of.id.0,
                "interrupted",
                Some(of.sent),
                None,
                None,
            );
        }
        if let Some(inf) = self.in_file.take() {
            drop(inf.file);
            let _ = tokio::fs::remove_file(&inf.temp).await;
            let _ = self.store.update_transfer(
                &self.cfg.peer_call,
                "in",
                &inf.id.0,
                "interrupted",
                Some(inf.received),
                None,
                None,
            );
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
                    Some(ModemSignal::Buffer(n)) => { self.modem_reported = n; self.written_since_report = 0; }
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
        // File chunks only while the modem queue estimate is below the window,
        // and only when no chat/protocol frame is waiting.
        loop {
            let est = self.modem_reported + self.written_since_report;
            let Some(of) = self.out_file.as_mut() else { break };
            if of.state != OutFileState::Sending || est >= self.cfg.modem_window {
                break;
            }
            let id = of.id;
            if of.sent >= of.size {
                of.state = OutFileState::AwaitingReceipt;
                let f = Frame::empty(FrameType::FileEnd, id);
                self.write_frame(data, &f).await?;
                let peer = self.cfg.peer_call.clone();
                let _ = self.store.update_transfer(
                    &peer,
                    "out",
                    &id.0,
                    "awaiting_receipt",
                    Some(self.out_file.as_ref().unwrap().sent),
                    None,
                    None,
                );
                break;
            }
            let want = (of.size - of.sent).min(of.chunk as u64) as usize;
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
        let peer = self.cfg.peer_call.clone();
        let _ = self.store.update_transfer(&peer, "out", &id.0, "failed", None, None, Some(reason));
        self.out_file = None;
        self.emit(SessionEvent::FileFailed { id, reason: reason.into() });
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
            SessionCommand::SendFile { path, opts } => {
                self.queued_files.push_back((path, opts));
                self.start_next_file().await;
            }
            SessionCommand::AcceptFile { id } => self.accept_in_file(id).await,
            SessionCommand::RejectFile { id } => self.reject_in_file(id, "rejected by operator").await,
            SessionCommand::CancelFile { id } => {
                if self.out_file.as_ref().is_some_and(|f| f.id == id) {
                    if let Ok(f) = Reason::new("user", "cancelled").to_frame(FrameType::FileCancel, id) {
                        self.q_proto.push_back(f);
                    }
                    let peer = self.cfg.peer_call.clone();
                    let _ = self.store.update_transfer(&peer, "out", &id.0, "cancelled", None, None, None);
                    self.out_file = None;
                    self.start_next_file().await;
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

    async fn start_next_file(&mut self) {
        if self.out_file.is_some() {
            return;
        }
        let Some((path, opts)) = self.queued_files.pop_front() else { return };
        let id = ObjectId::random();
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into());
        let (size, mut sha) = match sha256_file(&path).await {
            Ok(v) => v,
            Err(e) => {
                self.emit(SessionEvent::FileFailed { id, reason: format!("cannot read {}: {e}", path.display()) });
                return;
            }
        };
        if size > self.cfg.max_file || size == 0 {
            self.emit(SessionEvent::FileFailed {
                id,
                reason: format!("size {size} outside 1..={}", self.cfg.max_file),
            });
            return;
        }
        let real_sha = sha.clone();
        if opts.corrupt_declared_hash {
            sha = hex::encode(Sha256::digest(b"not the file"));
        }
        let offer = FileOffer { name: name.clone(), size, sha256: sha, mime: None };
        let Ok(frame) = offer.to_frame(id) else { return };
        let peer = self.cfg.peer_call.clone();
        let _ = self.store.upsert_transfer(&peer, "out", &id.0, &name, size, &real_sha, "offered");
        self.q_proto.push_back(frame);
        self.out_file = Some(OutFile { id, path, size, chunk: 0, sent: 0, file: None, state: OutFileState::Offered });
        self.emit(SessionEvent::FileSending { id, sha256: real_sha });
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
            FrameType::FileOffer => self.on_offer(&f).await?,
            FrameType::FileAccept => {
                let a = FileAccept::from_frame(&f).map_err(proto_err)?;
                let Some(of) = self.out_file.as_mut().filter(|o| o.id == f.id && o.state == OutFileState::Offered)
                else {
                    return Err(SessionEnd::Protocol("FILE_ACCEPT for no pending offer".into()));
                };
                of.chunk = (a.chunk as usize).min(mch_protocol::MAX_CHUNK_DATA).min(self.cfg.chunk as usize);
                match tokio::fs::File::open(&of.path).await {
                    Ok(file) => {
                        of.file = Some(file);
                        of.state = OutFileState::Sending;
                        let _ = self.store.update_transfer(&peer, "out", &f.id.0, "sending", None, None, None);
                    }
                    Err(_) => self.fail_out_file(f.id, "local file vanished"),
                }
            }
            FrameType::FileReject => {
                let r = Reason::from_frame(&f).map_err(proto_err)?;
                if self.out_file.as_ref().is_some_and(|o| o.id == f.id) {
                    self.out_file = None;
                    let _ = self.store.update_transfer(&peer, "out", &f.id.0, "rejected", None, None, Some(&r.code));
                    self.emit(SessionEvent::FileRejected { id: f.id, reason: format!("{}: {}", r.code, r.text) });
                    self.start_next_file().await;
                }
            }
            FrameType::FileReceipt => {
                if self.out_file.as_ref().is_some_and(|o| o.id == f.id && o.state == OutFileState::AwaitingReceipt) {
                    self.out_file = None;
                    let _ = self.store.update_transfer(&peer, "out", &f.id.0, "delivered", None, None, None);
                    self.emit(SessionEvent::FileDelivered { id: f.id });
                    self.start_next_file().await;
                } else {
                    let _ = self.store.log_event("protocol", &format!("unexpected FILE_RECEIPT {}", f.id));
                }
            }
            FrameType::FileChunk => self.on_chunk(&f).await?,
            FrameType::FileEnd => self.on_file_end(f.id).await?,
            FrameType::FileCancel => {
                let r = Reason::from_frame(&f).map_err(proto_err)?;
                if self.in_file.as_ref().is_some_and(|i| i.id == f.id) {
                    let inf = self.in_file.take().unwrap();
                    drop(inf.file);
                    let _ = tokio::fs::remove_file(&inf.temp).await;
                    let _ = self.store.update_transfer(
                        &peer,
                        "in",
                        &f.id.0,
                        "cancelled",
                        Some(inf.received),
                        None,
                        Some(&r.code),
                    );
                    self.emit(SessionEvent::FileFailed { id: f.id, reason: format!("sender cancelled: {}", r.code) });
                } else if self.out_file.as_ref().is_some_and(|o| o.id == f.id) {
                    self.out_file = None;
                    let _ = self.store.update_transfer(&peer, "out", &f.id.0, "failed", None, None, Some(&r.code));
                    self.emit(SessionEvent::FileFailed {
                        id: f.id,
                        reason: format!("receiver cancelled: {} {}", r.code, r.text),
                    });
                    self.start_next_file().await;
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
        let peer = self.cfg.peer_call.clone();
        let o = FileOffer::from_frame(f).map_err(proto_err)?;
        if self.in_file.is_some() {
            let r = Reason::new("busy", "one transfer at a time")
                .to_frame(FrameType::FileReject, f.id)
                .map_err(proto_err)?;
            self.q_proto.push_back(r);
            return Ok(());
        }
        let _ = self.store.upsert_transfer(&peer, "in", &f.id.0, &o.name, o.size, &o.sha256, "offered");
        if o.size == 0 || o.size > self.cfg.max_file {
            let r = Reason::new("too_large", &format!("limit {}", self.cfg.max_file))
                .to_frame(FrameType::FileReject, f.id)
                .map_err(proto_err)?;
            self.q_proto.push_back(r);
            let _ = self.store.update_transfer(&peer, "in", &f.id.0, "rejected", None, None, Some("too_large"));
            return Ok(());
        }
        let partial = self.cfg.files_dir.join(".partial");
        let temp = partial.join(format!("{}.part", f.id));
        self.in_file = Some(InFile {
            id: f.id,
            name: o.name.clone(),
            size: o.size,
            sha256: o.sha256,
            received: 0,
            hasher: Sha256::new(),
            temp,
            file: None,
            accepted: false,
        });
        self.emit(SessionEvent::FileOffered { id: f.id, name: o.name, size: o.size });
        if self.cfg.auto_accept_files {
            self.accept_in_file(f.id).await;
        }
        Ok(())
    }

    async fn accept_in_file(&mut self, id: ObjectId) {
        let chunk = self.agreed_chunk();
        let peer = self.cfg.peer_call.clone();
        let Some(inf) = self.in_file.as_mut().filter(|i| i.id == id && !i.accepted) else { return };
        let dir = inf.temp.parent().unwrap().to_path_buf();
        let opened = async {
            tokio::fs::create_dir_all(&dir).await?;
            tokio::fs::OpenOptions::new().write(true).create_new(true).open(&inf.temp).await
        }
        .await;
        match opened {
            Ok(file) => {
                inf.file = Some(file);
                inf.accepted = true;
                if let Ok(fr) = (FileAccept { chunk }).to_frame(id) {
                    self.q_proto.push_back(fr);
                }
                let _ = self.store.update_transfer(&peer, "in", &id.0, "receiving", Some(0), None, None);
            }
            Err(e) => {
                let reason = format!("cannot create temp file: {e}");
                self.reject_in_file(id, &reason).await;
            }
        }
    }

    async fn reject_in_file(&mut self, id: ObjectId, why: &str) {
        if self.in_file.as_ref().is_some_and(|i| i.id == id) {
            let inf = self.in_file.take().unwrap();
            drop(inf.file);
            let _ = tokio::fs::remove_file(&inf.temp).await;
            if let Ok(fr) = Reason::new("rejected", why).to_frame(FrameType::FileReject, id) {
                self.q_proto.push_back(fr);
            }
            let peer = self.cfg.peer_call.clone();
            let _ = self.store.update_transfer(&peer, "in", &id.0, "rejected", None, None, Some(why));
        }
    }

    async fn abort_in_file(&mut self, id: ObjectId, code: &str, why: &str) {
        if let Some(inf) = self.in_file.take() {
            drop(inf.file);
            let _ = tokio::fs::remove_file(&inf.temp).await;
            let peer = self.cfg.peer_call.clone();
            let _ = self.store.update_transfer(&peer, "in", &id.0, "failed", Some(inf.received), None, Some(code));
        }
        if let Ok(fr) = Reason::new(code, why).to_frame(FrameType::FileCancel, id) {
            self.q_proto.push_back(fr);
        }
        self.emit(SessionEvent::FileFailed { id, reason: format!("{code}: {why}") });
    }

    async fn on_chunk(&mut self, f: &Frame) -> Result<(), SessionEnd> {
        let (off, bytes) = parse_chunk(f).map_err(proto_err)?;
        let Some(inf) = self.in_file.as_mut().filter(|i| i.id == f.id && i.accepted) else {
            // Chunks already queued in the modem after a cancel are discarded.
            let _ = self.store.log_event("file", &format!("discarded chunk for inactive transfer {}", f.id));
            return Ok(());
        };
        if off != inf.received || inf.received + bytes.len() as u64 > inf.size {
            let why = format!("offset {off} len {} at received {} size {}", bytes.len(), inf.received, inf.size);
            self.abort_in_file(f.id, "bad_offset", &why).await;
            return Ok(());
        }
        let file = inf.file.as_mut().unwrap();
        if file.write_all(bytes).await.is_err() {
            self.abort_in_file(f.id, "disk", "write failed").await;
            return Ok(());
        }
        inf.hasher.update(bytes);
        inf.received += bytes.len() as u64;
        let (received, size) = (inf.received, inf.size);
        self.emit(SessionEvent::FileProgress { id: f.id, bytes: received, total: size });
        Ok(())
    }

    async fn on_file_end(&mut self, id: ObjectId) -> Result<(), SessionEnd> {
        let peer = self.cfg.peer_call.clone();
        let Some(mut inf) = self.in_file.take().filter(|i| i.id == id && i.accepted) else {
            return Ok(());
        };
        let file = inf.file.take().unwrap();
        let flushed = file.sync_all().await;
        drop(file);
        let got = hex::encode(std::mem::take(&mut inf.hasher).finalize());
        let fail = if flushed.is_err() {
            Some(("disk", "fsync failed".to_string()))
        } else if inf.received != inf.size {
            Some(("short", format!("received {} of {}", inf.received, inf.size)))
        } else if got != inf.sha256 {
            Some(("hash_mismatch", format!("declared {} got {got}", inf.sha256)))
        } else {
            None
        };
        if let Some((code, why)) = fail {
            let _ = tokio::fs::remove_file(&inf.temp).await;
            let _ = self.store.update_transfer(&peer, "in", &id.0, "failed", Some(inf.received), None, Some(code));
            if let Ok(fr) = Reason::new(code, &why).to_frame(FrameType::FileCancel, id) {
                self.q_proto.push_back(fr);
            }
            self.emit(SessionEvent::FileFailed { id, reason: format!("{code}: {why}") });
            return Ok(());
        }
        let target = unique_target(&self.cfg.files_dir, &sanitize_name(&inf.name));
        if let Err(e) = tokio::fs::rename(&inf.temp, &target).await {
            let _ = tokio::fs::remove_file(&inf.temp).await;
            let _ = self.store.update_transfer(&peer, "in", &id.0, "failed", Some(inf.received), None, Some("publish"));
            self.emit(SessionEvent::FileFailed { id, reason: format!("publish failed: {e}") });
            return Ok(());
        }
        let path_s = target.to_string_lossy().into_owned();
        // Completion is stored before FILE_RECEIPT is emitted.
        self.store
            .update_transfer(&peer, "in", &id.0, "complete", Some(inf.received), Some(&path_s), None)
            .map_err(|e| SessionEnd::Protocol(format!("storage failed, no receipt: {e}")))?;
        self.q_proto.push_back(Frame::empty(FrameType::FileReceipt, id));
        self.emit(SessionEvent::FileReceived { id, path: target, sha256: got });
        Ok(())
    }
}

impl From<ProtocolError> for SessionEnd {
    fn from(e: ProtocolError) -> Self {
        SessionEnd::Protocol(e.to_string())
    }
}
