//! Station driver: one Mercury attachment, one store, one session at a time.
//!
//! Safety defaults (docs/03 §2): after attach the modem is set to PUBLIC OFF
//! and LISTEN OFF (receive-only); listening and calling are explicit. On any
//! recovery the station re-attaches with bounded backoff, quarantines a
//! possibly surviving RF session with ABORT, and never re-calls or resends.

use crate::session::{ModemSignal, Session, SessionCommand, SessionConfig, SessionEnd, SessionEvent};
use mc_storage::Store;
use mercury_adapter::{
    Ack, AdapterEvent, Attachment, Bandwidth, Callsign, Command, ControlEvent, Endpoint, MercuryClient, Timeouts,
};
use std::path::PathBuf;
use std::time::Duration;
use tokio::sync::mpsc;

#[derive(Debug, thiserror::Error)]
pub enum StationError {
    #[error("attach: {0}")]
    Attach(#[from] mercury_adapter::AttachError),
    #[error("command: {0}")]
    Command(#[from] mercury_adapter::CommandError),
    #[error("modem refused {0}")]
    Wrong(String),
    #[error("not attached")]
    Detached,
    #[error("control connection lost")]
    ControlLost,
    #[error("timed out waiting for {0}")]
    Timeout(&'static str),
    #[error("CONNECTED {src} {dest} does not include local callsign {local}")]
    NotOurSession { src: String, dest: String, local: String },
    #[error("call ended without connecting")]
    CallFailed,
    /// Mercury reported a peer that is not a valid callsign (untrusted input).
    #[error("peer callsign rejected: {0:?}")]
    BadPeer(String),
    #[error("storage: {0}")]
    Storage(#[from] mc_storage::StorageError),
    #[error("{0}")]
    Lock(#[from] crate::lock::LockError),
}

#[derive(Clone, Debug)]
pub struct StationConfig {
    pub callsign: Callsign,
    pub endpoint: Endpoint,
    pub data_dir: PathBuf,
    pub bandwidth: Bandwidth,
    pub max_file: u64,
    pub chunk: u32,
    pub auto_accept_files: bool,
    pub hello_timeout: Duration,
    pub modem_window: u64,
    pub timeouts: Timeouts,
}

impl StationConfig {
    /// The app's station defaults (M2): BW2300, files up to 256 KiB in 1 KiB
    /// chunks, never auto-accepted, a 5 s HELLO wait.
    pub fn defaults(callsign: Callsign, endpoint: Endpoint, data_dir: &std::path::Path) -> Self {
        StationConfig {
            callsign,
            endpoint,
            data_dir: data_dir.to_path_buf(),
            bandwidth: Bandwidth::Bw2300,
            max_file: 256 * 1024,
            chunk: 1024,
            auto_accept_files: false,
            hello_timeout: Duration::from_secs(5),
            modem_window: 2 * 1024 + 256,
            timeouts: Timeouts::default(),
        }
    }
}

/// Control-plane requests accepted while a session is running.
#[derive(Debug)]
pub enum StationRequest {
    /// DISCONNECT (graceful drain) or ABORT; the session ends on DISCONNECTED.
    Command(Command, tokio::sync::oneshot::Sender<Result<Ack, mercury_adapter::CommandError>>),
    /// Lab fault: close both sockets without telling the modem.
    DropSockets,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectInfo {
    pub source: String,
    pub dest: String,
    pub bandwidth: String,
    pub peer: String,
    pub initiator: bool,
}

/// Raw control transcript entry for fixtures and run logs.
#[derive(Clone, Debug)]
pub struct TranscriptLine {
    pub t_ms: u128,
    pub generation: u64,
    pub line: String,
}

pub struct Station {
    pub cfg: StationConfig,
    pub store: Store,
    att: Option<Attachment>,
    started: std::time::Instant,
    pub transcript: Vec<TranscriptLine>,
    pub ptt_events: u64,
    pub stale_bytes_discarded: u64,
    pub stale_tx_purged: u64,
    _lock: crate::lock::LockFile,
}

impl Station {
    pub fn open(cfg: StationConfig) -> Result<Self, StationError> {
        std::fs::create_dir_all(cfg.data_dir.join("files")).ok();
        // MOD-02: the data root is ours alone before the store is opened.
        let lock = crate::lock::lock_data_root(&cfg.data_dir)?;
        let mut store = Store::open(&cfg.data_dir.join("station.db"))?;
        // A previous run may have ended mid-session (crash, kill, power):
        // anything that may have reached the modem is uncertain, never resent
        // on its own. Messages never handed to a session stay queued.
        let n = store.mark_in_flight_uncertain()?;
        if n > 0 {
            store.log_event("recovery", &format!("start-up: {n} message(s) marked uncertain"))?;
        }
        store.mark_transfers_interrupted()?;
        Ok(Station {
            cfg,
            store,
            att: None,
            started: std::time::Instant::now(),
            transcript: Vec::new(),
            ptt_events: 0,
            stale_bytes_discarded: 0,
            stale_tx_purged: 0,
            _lock: lock,
        })
    }

    pub fn files_dir(&self) -> PathBuf {
        self.cfg.data_dir.join("files")
    }

    pub fn is_attached(&self) -> bool {
        self.att.as_ref().is_some_and(|a| !a.client.is_closed())
    }

    pub fn generation(&self) -> u64 {
        self.att.as_ref().map(|a| a.client.generation()).unwrap_or(0)
    }

    fn note(&mut self, ev: &AdapterEvent) {
        if let AdapterEvent::RawLine(l) = ev {
            let generation = self.generation();
            self.transcript.push(TranscriptLine {
                t_ms: self.started.elapsed().as_millis(),
                generation,
                line: l.clone(),
            });
            if self.transcript.len() > 20_000 {
                self.transcript.drain(..5_000);
            }
            if l == "PTT ON" {
                self.ptt_events += 1;
            }
        }
    }

    async fn cmd(&mut self, c: Command) -> Result<(), StationError> {
        let att = self.att.as_ref().ok_or(StationError::Detached)?;
        let wire = c.wire();
        self.transcript.push(TranscriptLine {
            t_ms: self.started.elapsed().as_millis(),
            generation: att.client.generation(),
            line: format!("> {}", wire.trim_end()),
        });
        match att.client.command(&c).await? {
            Ack::Ok => Ok(()),
            Ack::Wrong => Err(StationError::Wrong(wire.trim_end().into())),
        }
    }

    /// Wait for a control event matching `f`, recording everything seen.
    async fn wait_for<F>(
        &mut self,
        what: &'static str,
        timeout: Duration,
        mut f: F,
    ) -> Result<ControlEvent, StationError>
    where
        F: FnMut(&ControlEvent) -> bool,
    {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let att = self.att.as_mut().ok_or(StationError::Detached)?;
            let ev = match tokio::time::timeout_at(deadline, att.events.recv()).await {
                Err(_) => return Err(StationError::Timeout(what)),
                Ok(None) => return Err(StationError::ControlLost),
                Ok(Some(ev)) => ev,
            };
            self.note(&ev);
            match ev {
                AdapterEvent::Control(c) if f(&c) => return Ok(c),
                AdapterEvent::ControlClosed | AdapterEvent::ControlFault(_) => return Err(StationError::ControlLost),
                _ => {}
            }
        }
    }

    /// Attach and put the modem in the safe idle state.
    pub async fn attach(&mut self) -> Result<(), StationError> {
        self.att = None;
        let att = MercuryClient::attach(&self.cfg.endpoint, self.cfg.timeouts).await?;
        self.att = Some(att);
        self.init_modem().await
    }

    /// Safe idle configuration on a fresh attachment.
    async fn init_modem(&mut self) -> Result<(), StationError> {
        let call = self.cfg.callsign.clone();
        self.cmd(Command::MyCall(call.clone())).await?;
        let want = call.as_str().to_string();
        self.wait_for("REGISTERED", Duration::from_secs(5), |e| matches!(e, ControlEvent::Registered(c) if *c == want))
            .await?;
        self.cmd(Command::Public(false)).await?;
        self.cmd(Command::Listen(false)).await?;
        self.cmd(Command::Bw(self.cfg.bandwidth)).await?;
        self.purge_stale_tx().await?;
        self.store.log_event("modem", &format!("attached gen {}", self.generation()))?;
        Ok(())
    }

    /// Mercury keeps bytes written to its data port while no session is up
    /// and sends them as the first payload of the next session (observed in
    /// the M0 lab). If the idle modem reports queued TX bytes, ABORT discards
    /// them before any session can start. Returns the bytes found queued.
    pub async fn purge_stale_tx(&mut self) -> Result<u64, StationError> {
        self.att.as_ref().ok_or(StationError::Detached)?.client.query("BUFFER").await?;
        let queued =
            match self.wait_for("BUFFER", Duration::from_secs(3), |e| matches!(e, ControlEvent::Buffer(_))).await {
                Ok(ControlEvent::Buffer(n)) => n,
                _ => return Ok(0),
            };
        if queued > 0 {
            self.cmd(Command::Abort).await?;
            let _ = self
                .wait_for("DISCONNECTED after purge", Duration::from_secs(3), |e| *e == ControlEvent::Disconnected)
                .await;
            self.stale_tx_purged += queued;
            self.store.log_event("modem", &format!("idle modem had {queued} queued TX byte(s); purged with ABORT"))?;
        }
        Ok(queued)
    }

    /// Re-attach with bounded exponential backoff (1, 2, 4, 8, 15 s cap,
    /// +/-20% jitter), then quarantine any surviving RF session. Leaves the
    /// station receive-only: no automatic re-call, listen or resend.
    pub async fn recover(&mut self, max_attempts: u32) -> Result<(), StationError> {
        self.att = None;
        let n = self.store.mark_in_flight_uncertain()?;
        self.store.mark_transfers_interrupted()?;
        self.store.log_event("recovery", &format!("{n} message(s) marked uncertain"))?;
        let mut delay = 1.0f64;
        let mut last = None;
        for attempt in 0..max_attempts {
            // Sockets only: ABORT must be the first command. Mercury treats a
            // vanished host as a DISCONNECT and starts draining; a LISTEN OFF
            // during that drain releases the radio WITHOUT telling the peer
            // (observed in the M0 lab: peer waited ~235 s for its timeout).
            // ABORT sends one DISCONNECT frame, so the peer ends at once.
            let attached = match MercuryClient::attach(&self.cfg.endpoint, self.cfg.timeouts).await {
                Ok(att) => {
                    self.att = Some(att);
                    Ok(())
                }
                Err(e) => Err(StationError::from(e)),
            };
            let attached = match attached {
                Ok(()) => {
                    self.cmd(Command::Abort).await?;
                    let _ = self
                        .wait_for("post-recovery DISCONNECTED", Duration::from_secs(5), |e| {
                            *e == ControlEvent::Disconnected
                        })
                        .await;
                    let keyed = self.wait_teardown_keyed(Duration::from_secs(20)).await;
                    self.store.log_event("recovery", &format!("ABORT teardown frame keyed: {keyed}"))?;
                    self.init_modem().await
                }
                Err(e) => Err(e),
            };
            match attached {
                Ok(()) => {
                    let stale = self.discard_stale_data(Duration::from_millis(1500)).await;
                    self.stale_bytes_discarded += stale;
                    self.store.log_event(
                        "recovery",
                        &format!("discarded {stale} stale data byte(s) from the previous session"),
                    )?;
                    self.store.log_event("recovery", &format!("re-attached after {} attempt(s)", attempt + 1))?;
                    return Ok(());
                }
                Err(e) => last = Some(e),
            }
            let jitter = 0.8 + rand::random::<f64>() * 0.4;
            tokio::time::sleep(Duration::from_secs_f64(delay * jitter)).await;
            delay = (delay * 2.0).min(15.0);
        }
        Err(last.unwrap_or(StationError::Detached))
    }

    pub async fn set_listen(&mut self, on: bool) -> Result<(), StationError> {
        self.cmd(Command::Listen(on)).await
    }

    fn connect_info(&self, ev: ControlEvent) -> Result<ConnectInfo, StationError> {
        let ControlEvent::Connected { source, dest, bandwidth } = ev else { unreachable!() };
        let local = self.cfg.callsign.as_str();
        let (peer, initiator) = if source.eq_ignore_ascii_case(local) {
            (dest.clone(), true)
        } else if dest.eq_ignore_ascii_case(local) {
            (source.clone(), false)
        } else {
            return Err(StationError::NotOurSession { src: source, dest, local: local.into() });
        };
        // The normalised callsign is the peer's identity: upper case, any
        // suffix (/P, -1) kept. Anything else is refused before a session,
        // message or dedup key can exist.
        let peer = match Callsign::parse(&peer) {
            Ok(c) => c.as_str().to_string(),
            Err(_) => return Err(StationError::BadPeer(peer.chars().take(64).collect())),
        };
        Ok(ConnectInfo { source, dest, bandwidth, peer, initiator })
    }

    /// Place a call and wait for CONNECTED (or DISCONNECTED = failed).
    pub async fn call(&mut self, peer: &Callsign, timeout: Duration) -> Result<ConnectInfo, StationError> {
        self.cmd(Command::Connect { mycall: self.cfg.callsign.clone(), theircall: peer.clone() }).await?;
        let ev = match self
            .wait_for("CONNECTED", timeout, |e| {
                matches!(e, ControlEvent::Connected { .. } | ControlEvent::Disconnected)
            })
            .await
        {
            Ok(ev) => ev,
            Err(StationError::Timeout(w)) => {
                // Cancel the call; do not leave it running.
                let _ = self.cmd(Command::Disconnect).await;
                return Err(StationError::Timeout(w));
            }
            Err(e) => return Err(e),
        };
        if ev == ControlEvent::Disconnected {
            return Err(StationError::CallFailed);
        }
        self.connect_info(ev)
    }

    /// Stop a call that has not been answered: ABORT, then DISCONNECTED. If
    /// the link came up meanwhile, give its teardown frame a moment to key.
    pub async fn cancel_call(&mut self) -> Result<(), StationError> {
        self.cmd(Command::Abort).await?;
        let _ = self.wait_for("DISCONNECTED", Duration::from_secs(5), |e| *e == ControlEvent::Disconnected).await;
        self.wait_teardown_keyed(Duration::from_secs(2)).await;
        Ok(())
    }

    pub async fn wait_incoming(&mut self, timeout: Duration) -> Result<ConnectInfo, StationError> {
        let ev = self.wait_for("incoming CONNECTED", timeout, |e| matches!(e, ControlEvent::Connected { .. })).await?;
        self.connect_info(ev)
    }

    /// Graceful disconnect: Mercury drains queued data first (may take minutes).
    pub async fn disconnect(&mut self, timeout: Duration) -> Result<(), StationError> {
        self.cmd(Command::Disconnect).await?;
        self.wait_for("DISCONNECTED", timeout, |e| *e == ControlEvent::Disconnected).await?;
        Ok(())
    }

    /// Abort discards queued modem data; RF may continue briefly.
    pub async fn abort(&mut self) -> Result<(), StationError> {
        self.cmd(Command::Abort).await?;
        let _ = self.wait_for("DISCONNECTED", Duration::from_secs(5), |e| *e == ControlEvent::Disconnected).await;
        self.wait_teardown_keyed(Duration::from_secs(20)).await;
        Ok(())
    }

    /// After ABORT, Mercury reports DISCONNECTED at once but keys its single
    /// DISCONNECT frame later, in its next slot. A LISTEN OFF (or other
    /// radio-releasing command) before that frame is on the air cancels it and
    /// strands the peer until its ~240 s timeout (observed in the M0 lab).
    /// Wait for that PTT ON → PTT OFF cycle, bounded by `max`. Returns whether
    /// it was seen; on timeout the caller proceeds (conservative fallback).
    pub async fn wait_teardown_keyed(&mut self, max: Duration) -> bool {
        if self.wait_for("teardown PTT ON", max, |e| *e == ControlEvent::Ptt(true)).await.is_err() {
            return false;
        }
        self.wait_for("teardown PTT OFF", Duration::from_secs(35), |e| *e == ControlEvent::Ptt(false)).await.is_ok()
    }

    /// Read and discard data-socket bytes until `quiet` passes with none.
    /// Bytes outside an application session never reach a decoder.
    pub async fn discard_stale_data(&mut self, quiet: Duration) -> u64 {
        use tokio::io::AsyncReadExt;
        let Some(att) = self.att.as_mut() else { return 0 };
        let mut buf = [0u8; 4096];
        let mut n_total = 0u64;
        while let Ok(Ok(n)) = tokio::time::timeout(quiet, att.data.read(&mut buf)).await {
            if n == 0 {
                break;
            }
            n_total += n as u64;
        }
        n_total
    }

    /// Drop the sockets without telling the modem (simulates a host crash).
    pub fn drop_sockets(&mut self) {
        self.att = None;
    }

    /// Run one MCH/1 session on the established link. Returns when the link
    /// or the application session ends.
    pub async fn run_session(
        &mut self,
        info: &ConnectInfo,
        cmds: &mut mpsc::Receiver<SessionCommand>,
        reqs: &mut mpsc::Receiver<StationRequest>,
        events: mpsc::UnboundedSender<SessionEvent>,
    ) -> SessionEnd {
        let generation = self.generation();
        let scfg = SessionConfig {
            local_call: self.cfg.callsign.as_str().into(),
            peer_call: info.peer.clone(),
            generation,
            files_dir: self.files_dir(),
            max_file: self.cfg.max_file,
            chunk: self.cfg.chunk,
            auto_accept_files: self.cfg.auto_accept_files,
            hello_timeout: self.cfg.hello_timeout,
            modem_window: self.cfg.modem_window,
        };
        let Some(att) = self.att.as_mut() else { return SessionEnd::ControlLost };
        let (sig_tx, mut sig_rx) = mpsc::channel(256);
        let started = self.started;
        let transcript = &mut self.transcript;
        let ptt = &mut self.ptt_events;
        let events_rx = &mut att.events;
        let data = &mut att.data;
        let client = &att.client;
        let session = Session::new(scfg, &mut self.store, events);

        // Forward control events into modem signals while the session runs.
        let pump = async {
            let mut poll = tokio::time::interval(Duration::from_secs(2));
            loop {
                tokio::select! {
                    ev = events_rx.recv() => {
                        let Some(ev) = ev else { let _ = sig_tx.send(ModemSignal::ControlLost).await; break };
                        match &ev {
                            AdapterEvent::RawLine(l) => {
                                transcript.push(TranscriptLine { t_ms: started.elapsed().as_millis(), generation, line: l.clone() });
                                if l == "PTT ON" { *ptt += 1; }
                            }
                            AdapterEvent::Control(ControlEvent::Buffer(n)) => { let _ = sig_tx.send(ModemSignal::Buffer(*n)).await; }
                            AdapterEvent::Control(ControlEvent::Disconnected) => { let _ = sig_tx.send(ModemSignal::Disconnected).await; break }
                            AdapterEvent::ControlClosed | AdapterEvent::ControlFault(_) => { let _ = sig_tx.send(ModemSignal::ControlLost).await; break }
                            _ => {}
                        }
                    }
                    _ = poll.tick() => { let _ = client.query("BUFFER").await; }
                    r = reqs.recv() => match r {
                        Some(StationRequest::Command(c, reply)) => {
                            transcript.push(TranscriptLine { t_ms: started.elapsed().as_millis(), generation, line: format!("> {}", c.wire().trim_end()) });
                            let _ = reply.send(client.command(&c).await);
                        }
                        Some(StationRequest::DropSockets) => return true,
                        None => std::future::pending::<()>().await,
                    }
                }
            }
            std::future::pending::<bool>().await
        };
        let end = tokio::select! {
            end = session.run(data, cmds, &mut sig_rx) => end,
            _drop = pump => SessionEnd::LocalDrop,
        };
        if end == SessionEnd::LocalDrop {
            // The session future is dropped here; record truthful states.
            let _ = self.store.mark_in_flight_uncertain();
            let _ = self.store.mark_transfers_interrupted();
            self.att = None;
        }
        end
    }
}
