//! The modem task (M2): one per selected profile. It owns the `Station`, does
//! the attach and everything after it, and reports back to the service actor
//! by message, tagged with the generation it was started for. The actor never
//! awaits it, so a slow or hung attach cannot stall commands or snapshots, and
//! a report from a modem the actor has moved on from is simply dropped.
//!
//! A real Mercury (an Existing profile) is attached only while this task holds
//! its endpoint lock (MOD-02). An occupied endpoint is reported once, with the
//! lock's own text, and the task ends: no takeover loop.

use super::chat::ChatMessage;
use super::events::{ModemState, SessionDirection, SessionInfo};
use crate::profile::{ModemTarget, StationProfile};
use crate::session::{SessionCommand, SessionEnd, SessionEvent};
use crate::station::{ConnectInfo, Station, StationConfig, StationError, StationRequest};
use mc_storage::{SessionRow, Store};
use mch_protocol::ObjectId;
use mercury_adapter::{Callsign, Command, Endpoint};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::sync::mpsc;

/// Where a modem task keeps its data and takes its locks.
#[derive(Debug, Clone, Default)]
pub struct ModemOptions {
    /// `{data root}/stations`: each profile's station data lives in `{id}/`.
    pub stations_root: Option<PathBuf>,
    /// The shared endpoint lock folder; `None` takes no endpoint lock (tests).
    pub locks_dir: Option<PathBuf>,
    /// HELLO wait over a real Mercury; `None` is `RADIO_HELLO_TIMEOUT`.
    pub radio_hello_timeout: Option<Duration>,
}

/// What the actor asks of the modem.
#[derive(Debug)]
pub enum ModemCmd {
    SetListen(bool),
    Call(Callsign),
    CancelCall,
    Disconnect,
    Abort,
    /// A queued message for `peer`, to go out if the session is with them.
    Submit {
        peer: String,
        id: ObjectId,
    },
    Shutdown,
}

/// What the modem tells the actor, for the generation it was started in.
#[derive(Debug)]
pub struct ModemReport {
    pub generation: u64,
    pub kind: ReportKind,
}

#[derive(Debug)]
pub enum ReportKind {
    State {
        state: ModemState,
        detail: Option<String>,
    },
    /// A session was negotiated (Some) or has ended (None).
    Session(Option<SessionInfo>),
    CallFailed {
        peer: String,
        reason: String,
    },
    /// A message changed state or arrived.
    Message(ChatMessage),
}

/// The actor's end of one modem task. Dropping it stops the task.
pub struct ModemHandle {
    pub generation: u64,
    cmds: mpsc::Sender<ModemCmd>,
}

impl ModemHandle {
    /// Queue a command; false if the task has ended or is backed up.
    pub fn send(&self, cmd: ModemCmd) -> bool {
        self.cmds.try_send(cmd).is_ok()
    }
}

impl Drop for ModemHandle {
    fn drop(&mut self) {
        let _ = self.cmds.try_send(ModemCmd::Shutdown);
    }
}

pub fn spawn(
    profile: StationProfile,
    generation: u64,
    opts: ModemOptions,
    reports: mpsc::Sender<ModemReport>,
) -> ModemHandle {
    let (tx, rx) = mpsc::channel(64);
    tokio::spawn(run(profile, opts, rx, Reporter { generation, tx: reports }));
    ModemHandle { generation, cmds: tx }
}

struct Reporter {
    generation: u64,
    tx: mpsc::Sender<ModemReport>,
}

impl Reporter {
    async fn state(&self, state: ModemState, detail: Option<String>) {
        let report = ModemReport { generation: self.generation, kind: ReportKind::State { state, detail } };
        let _ = self.tx.send(report).await;
    }

    async fn fault(&self, detail: String) {
        self.state(ModemState::Faulted, Some(detail)).await;
    }

    async fn send(&self, kind: ReportKind) {
        let _ = self.tx.send(ModemReport { generation: self.generation, kind }).await;
    }

    async fn session(&self, s: Option<SessionInfo>) {
        self.send(ReportKind::Session(s)).await;
    }

    async fn message(&self, m: ChatMessage) {
        self.send(ReportKind::Message(m)).await;
    }

    async fn call_failed(&self, peer: &str, reason: String) {
        self.send(ReportKind::CallFailed { peer: peer.to_string(), reason }).await;
    }
}

/// Resolves when the actor stops this task (Shutdown, or the handle dropped).
/// Anything else that arrives before the modem is up is ignored.
async fn stopped(cmds: &mut mpsc::Receiver<ModemCmd>) {
    while let Some(cmd) = cmds.recv().await {
        if matches!(cmd, ModemCmd::Shutdown) {
            return;
        }
    }
}

/// Open the station store. A just-stopped task for the same profile may still
/// be releasing its data-root lock, so a busy lock is retried briefly; that is
/// our own predecessor finishing, not another instance.
async fn open_station(cfg: StationConfig) -> Result<Station, StationError> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        match Station::open(cfg.clone()) {
            Err(StationError::Lock(crate::lock::LockError::DataRootBusy { .. }))
                if tokio::time::Instant::now() < deadline =>
            {
                tokio::time::sleep(Duration::from_millis(50)).await
            }
            other => return other,
        }
    }
}

async fn run(profile: StationProfile, opts: ModemOptions, mut cmds: mpsc::Receiver<ModemCmd>, r: Reporter) {
    let Some(stations) = opts.stations_root.clone() else {
        return r.fault("no data folder for the station".into()).await;
    };
    let station_dir = stations.join(&profile.id);
    // RF-02: a stored preference is shown, never applied by itself.
    let unarmed = profile.listen_preferred.then(|| UNARMED.to_string());
    let call = match Callsign::parse(&profile.callsign) {
        Ok(c) => c,
        Err(e) => return r.fault(format!("callsign: {e}")).await,
    };
    match profile.modem.clone() {
        ModemTarget::Existing { host, control_port, data_port, .. } => {
            let _lock = match &opts.locks_dir {
                Some(dir) => match crate::lock::lock_endpoint(dir, &host, control_port) {
                    Ok(l) => Some(l),
                    Err(e) => return r.fault(e.to_string()).await,
                },
                None => None,
            };
            r.state(ModemState::Attaching, None).await;
            let endpoint = Endpoint { host: host.clone(), control_port, data_port };
            let mut cfg = StationConfig::defaults(call, endpoint, &station_dir);
            cfg.hello_timeout = opts.radio_hello_timeout.unwrap_or(crate::station::RADIO_HELLO_TIMEOUT);
            let mut station = match open_station(cfg).await {
                Ok(s) => s,
                Err(e) => return r.fault(format!("station data: {e}")).await,
            };
            tokio::select! {
                res = station.attach() => match res {
                    Ok(()) => r.state(ModemState::Ready, unarmed).await,
                    Err(e) => return r.fault(format!("could not attach to Mercury at {host}:{control_port}: {e}")).await,
                },
                _ = stopped(&mut cmds) => return,
            }
            let mut station = station;
            serve(&mut station, cmds, &r, profile.listen_preferred).await;
        }
        ModemTarget::Simulator => simulator(&station_dir, call, unarmed, cmds, r).await,
    }
}

#[cfg(feature = "sim")]
async fn simulator(
    station_dir: &Path,
    call: Callsign,
    unarmed: Option<String>,
    mut cmds: mpsc::Receiver<ModemCmd>,
    r: Reporter,
) {
    r.state(ModemState::Attaching, None).await;
    let peer_dir = station_dir.join("sim-peer");
    let start = async {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        loop {
            match crate::sim::SimModem::start(station_dir, &peer_dir, call.as_str()).await {
                Err(StationError::Lock(crate::lock::LockError::DataRootBusy { .. }))
                    if tokio::time::Instant::now() < deadline =>
                {
                    tokio::time::sleep(Duration::from_millis(50)).await
                }
                other => return other,
            }
        }
    };
    let sim = tokio::select! {
        res = start => match res {
            Ok(sim) => sim,
            Err(e) => return r.fault(format!("simulator: {e}")).await,
        },
        _ = stopped(&mut cmds) => return,
    };
    let listen_preferred = unarmed.is_some();
    let detail = match unarmed {
        Some(u) => format!("simulator; {u}"),
        None => "simulator".into(),
    };
    r.state(ModemState::Ready, Some(detail)).await;
    let mut sim = sim;
    serve(&mut sim.station, cmds, &r, listen_preferred).await;
}

#[cfg(not(feature = "sim"))]
async fn simulator(
    _dir: &Path,
    _call: Callsign,
    _unarmed: Option<String>,
    _cmds: mpsc::Receiver<ModemCmd>,
    r: Reporter,
) {
    r.fault("the simulator is not part of this build".into()).await;
}

/// Shown on Ready when the profile prefers listening but it is not armed.
const UNARMED: &str = "listening is off until you turn it on";

/// How long a call may ring before it is given up (docs/04 section 3).
const CALL_TIMEOUT: Duration = Duration::from_secs(120);

/// What the operator is told when Mercury reports an invalid peer callsign.
const BAD_PEER: &str = "peer callsign rejected";

/// Take down a link we refuse, and say so; a failed ABORT means the modem
/// itself is in doubt.
async fn refuse_link(st: &mut Station, r: &Reporter, peer: &str) -> After {
    let shown: String = peer.chars().take(32).collect();
    let after = match st.abort().await {
        Ok(()) => After::Idle(None),
        Err(_) => After::Recover,
    };
    r.call_failed(&shown, BAD_PEER.into()).await;
    after
}

/// What the operator is told when the peer never sends HELLO (docs/04 section 3).
const NO_HELLO: &str = "the station answered but is not running MercuryChat (no HELLO)";

/// What an idle modem saw first.
enum Idle {
    Cmd(Option<ModemCmd>),
    Incoming(Result<ConnectInfo, StationError>),
}

/// What became of a call being placed.
enum Calling {
    Answered(Result<ConnectInfo, StationError>),
    Cancelled,
    Stopped,
}

/// Where the task goes after a link ends.
enum After {
    Idle(Option<String>),
    Recover,
    Stop,
}

/// Resolves when a call being placed should stop: CancelCall or Abort
/// (Cancelled), or Shutdown and a dropped handle (Stopped).
async fn call_interrupted(cmds: &mut mpsc::Receiver<ModemCmd>) -> Calling {
    loop {
        match cmds.recv().await {
            Some(ModemCmd::CancelCall | ModemCmd::Abort) => return Calling::Cancelled,
            Some(ModemCmd::Shutdown) | None => return Calling::Stopped,
            Some(other) => tracing::debug!(?other, "ignored while calling"),
        }
    }
}

/// The attached modem: holds the station (and so its locks) and carries out
/// the actor's commands until stopped. Idle, it also watches for incoming
/// links and for the loss of the control connection.
async fn serve(st: &mut Station, mut cmds: mpsc::Receiver<ModemCmd>, r: &Reporter, listen_preferred: bool) {
    let mut listening = false;
    let mut preferred = listen_preferred;
    let idle = |listening: bool| if listening { ModemState::Listening } else { ModemState::Ready };
    loop {
        let step = tokio::select! {
            c = cmds.recv() => Idle::Cmd(c),
            res = st.wait_incoming(Duration::from_secs(3600)) => Idle::Incoming(res),
        };
        let after = match step {
            Idle::Cmd(None | Some(ModemCmd::Shutdown)) => return,
            Idle::Cmd(Some(ModemCmd::SetListen(on))) => {
                preferred = on;
                match st.set_listen(on).await {
                    Ok(()) => {
                        listening = on;
                        r.state(idle(on), None).await;
                    }
                    Err(e) => r.fault(format!("LISTEN {}: {e}", if on { "ON" } else { "OFF" })).await,
                }
                continue;
            }
            Idle::Cmd(Some(ModemCmd::Call(peer))) => {
                // The actor has already reported Calling.
                let res = tokio::select! {
                    res = st.call(&peer, CALL_TIMEOUT) => Calling::Answered(res),
                    c = call_interrupted(&mut cmds) => c,
                };
                match res {
                    Calling::Stopped => {
                        let _ = st.cancel_call().await;
                        return;
                    }
                    Calling::Cancelled => match st.cancel_call().await {
                        Ok(()) => After::Idle(None),
                        Err(_) => After::Recover,
                    },
                    Calling::Answered(Ok(info)) => link(st, &mut cmds, r, info, SessionDirection::Outgoing).await,
                    Calling::Answered(Err(StationError::ControlLost | StationError::Detached)) => After::Recover,
                    Calling::Answered(Err(StationError::BadPeer(p))) => refuse_link(st, r, &p).await,
                    Calling::Answered(Err(e)) => {
                        let reason = match e {
                            StationError::CallFailed => format!("{} did not answer", peer.as_str()),
                            StationError::Timeout(_) => {
                                format!("{} did not answer within {} s", peer.as_str(), CALL_TIMEOUT.as_secs())
                            }
                            other => other.to_string(),
                        };
                        r.call_failed(peer.as_str(), reason).await;
                        After::Idle(None)
                    }
                }
            }
            Idle::Cmd(Some(other)) => {
                tracing::debug!(?other, "no call or session for this command");
                continue;
            }
            Idle::Incoming(Ok(info)) => link(st, &mut cmds, r, info, SessionDirection::Incoming).await,
            Idle::Incoming(Err(StationError::Timeout(_))) => continue,
            Idle::Incoming(Err(StationError::BadPeer(p))) => refuse_link(st, r, &p).await,
            Idle::Incoming(Err(StationError::NotOurSession { src, dest, .. })) => {
                tracing::warn!(%src, %dest, "ignoring a link that is not ours");
                continue;
            }
            Idle::Incoming(Err(e)) => {
                tracing::warn!("modem control lost while idle: {e}");
                After::Recover
            }
        };
        match after {
            After::Stop => return,
            After::Idle(detail) => r.state(idle(listening), detail).await,
            After::Recover => {
                // Recovery leaves the modem not listening (RF-02).
                listening = false;
                r.session(None).await;
                r.state(ModemState::Recovering, None).await;
                match st.recover(5).await {
                    Ok(()) => r.state(ModemState::Ready, preferred.then(|| UNARMED.to_string())).await,
                    Err(e) => return r.fault(format!("could not recover the modem: {e}")).await,
                }
            }
        }
    }
}

fn outcome(end: &SessionEnd, aborted: bool) -> String {
    match end {
        SessionEnd::Disconnected if aborted => "aborted".into(),
        SessionEnd::Disconnected => "disconnected".into(),
        SessionEnd::ControlLost => "control_lost".into(),
        SessionEnd::DataLost => "data_lost".into(),
        SessionEnd::Protocol(m) => format!("protocol: {m}"),
        SessionEnd::HelloTimeout => "hello_timeout".into(),
        SessionEnd::CommandsClosed => "commands_closed".into(),
        SessionEnd::LocalDrop => "local_drop".into(),
    }
}

/// The application side of one link: its session record, the messages it
/// has handed to the session, and the reports about them.
struct Link {
    info: ConnectInfo,
    direction: SessionDirection,
    generation: u64,
    session_id: String,
    /// A second connection to station.db: the session holds the station's own.
    records: Option<Store>,
    chats: mpsc::Sender<SessionCommand>,
    submitted: HashSet<[u8; 16]>,
    negotiated: bool,
}

impl Link {
    fn row(&self, id: &[u8; 16]) -> Option<ChatMessage> {
        let row = self.records.as_ref()?.message(&self.info.peer, id).ok()??;
        Some(ChatMessage::from(&row))
    }

    async fn report(&mut self, r: &Reporter, id: &[u8; 16]) {
        if let Some(m) = self.row(id) {
            r.message(m).await;
        }
    }

    /// Hand one queued message to the session, once per session. Only after
    /// negotiation: before it, the queue is read when HELLO completes.
    async fn submit(&mut self, r: &Reporter, id: [u8; 16]) {
        if !self.negotiated || self.submitted.contains(&id) {
            return;
        }
        let Some(mut m) = self.row(&id) else { return };
        if m.direction != "out" || m.state != "queued" {
            return; // never an automatic resend of anything else (CHT-03)
        }
        let cmd = SessionCommand::SendChat { id: ObjectId(id), text: m.body.clone() };
        if self.chats.try_send(cmd).is_err() {
            tracing::warn!("the session is not taking messages; {} stays queued", hex::encode(id));
            return;
        }
        self.submitted.insert(id);
        m.state = "submitting".into();
        r.message(m).await;
    }

    async fn on_event(&mut self, r: &Reporter, ev: SessionEvent) {
        match ev {
            SessionEvent::Negotiated { peer_app, .. } if !self.negotiated => {
                self.negotiated = true;
                let since = mc_storage::now_utc();
                if let Some(rec) = self.records.as_mut() {
                    let row = SessionRow {
                        id: self.session_id.clone(),
                        peer: self.info.peer.clone(),
                        generation: self.generation as i64,
                        direction: match self.direction {
                            SessionDirection::Outgoing => "out".into(),
                            SessionDirection::Incoming => "in".into(),
                        },
                        started_utc: since.clone(),
                        ended_utc: None,
                        peer_app: Some(peer_app.clone()),
                        outcome: None,
                    };
                    if let Err(e) = rec.begin_session(&row) {
                        tracing::warn!("could not record the session: {e}");
                    }
                }
                let peer = self.info.peer.clone();
                r.state(ModemState::Connected, Some(peer.clone())).await;
                r.session(Some(SessionInfo {
                    peer: peer.clone(),
                    direction: self.direction,
                    since_utc: since,
                    peer_app: Some(peer_app),
                }))
                .await;
                // The outbox for this peer goes out in order, after HELLO.
                let queued = match self.records.as_ref().map(|s| s.queued_for(&peer)) {
                    Some(Ok(rows)) => rows,
                    Some(Err(e)) => {
                        tracing::warn!("could not read the outbox: {e}");
                        Vec::new()
                    }
                    None => Vec::new(),
                };
                for row in queued {
                    self.submit(r, row.wire_id).await;
                }
            }
            SessionEvent::ChatSubmitted { id, .. } | SessionEvent::ChatDelivered { id, .. } => {
                self.report(r, &id.0).await
            }
            SessionEvent::ChatReceived { id, duplicate: false, .. } => self.report(r, &id.0).await,
            other => tracing::debug!(?other, "session event"),
        }
    }
}

/// One established link: run the MCH/1 session on it while taking the
/// actor's Disconnect, Abort and Submit, record it, and say where to go next.
async fn link(
    st: &mut Station,
    cmds: &mut mpsc::Receiver<ModemCmd>,
    r: &Reporter,
    info: ConnectInfo,
    direction: SessionDirection,
) -> After {
    r.state(ModemState::Negotiating, Some(info.peer.clone())).await;
    let records = match Store::open(&st.cfg.data_dir.join("station.db")) {
        Ok(s) => Some(s),
        Err(e) => {
            tracing::warn!("session records unavailable: {e}");
            None
        }
    };
    let (chat_tx, mut chat_rx) = mpsc::channel(256);
    let (req_tx, mut req_rx) = mpsc::channel(4);
    let (ev_tx, mut ev_rx) = mpsc::unbounded_channel();
    let mut link = Link {
        info: info.clone(),
        direction,
        generation: st.generation(),
        session_id: uuid::Uuid::new_v4().to_string(),
        records,
        chats: chat_tx,
        submitted: HashSet::new(),
        negotiated: false,
    };
    let mut aborted = false;
    let mut stop = false;

    let end = {
        let run = st.run_session(&info, &mut chat_rx, &mut req_rx, ev_tx);
        tokio::pin!(run);
        loop {
            tokio::select! {
                end = &mut run => break end,
                Some(ev) = ev_rx.recv() => link.on_event(r, ev).await,
                c = cmds.recv() => {
                    let wire = match c {
                        Some(ModemCmd::Disconnect) => Command::Disconnect,
                        Some(ModemCmd::Abort) => {
                            aborted = true;
                            Command::Abort
                        }
                        Some(ModemCmd::Submit { peer, id }) => {
                            if peer == link.info.peer {
                                link.submit(r, id.0).await;
                            }
                            continue;
                        }
                        Some(ModemCmd::Shutdown) | None => {
                            // Leaving: the modem ends the link when our sockets close.
                            stop = true;
                            break SessionEnd::CommandsClosed;
                        }
                        Some(other) => {
                            tracing::debug!(?other, "ignored during a session");
                            continue;
                        }
                    };
                    // The session's control pump sends it; its reply is not awaited
                    // here, because the pump only runs while the session is polled.
                    let (reply, _) = tokio::sync::oneshot::channel();
                    let _ = req_tx.try_send(StationRequest::Command(wire, reply));
                    let detail = if aborted { "aborting" } else { "sending what the modem holds, then disconnecting" };
                    r.state(ModemState::Draining, Some(detail.into())).await;
                }
            }
        }
    };
    while let Ok(ev) = ev_rx.try_recv() {
        link.on_event(r, ev).await;
    }
    if stop {
        return After::Stop;
    }
    // Whatever was in flight is now uncertain (or was confirmed): say which.
    for id in link.submitted.clone() {
        link.report(r, &id).await;
    }
    if aborted {
        // ABORT's single DISCONNECT frame keys after DISCONNECTED; let it out.
        st.wait_teardown_keyed(Duration::from_secs(20)).await;
    }
    if link.negotiated {
        if let Some(rec) = link.records.as_mut() {
            if let Err(e) = rec.end_session(&link.session_id, &outcome(&end, aborted)) {
                tracing::warn!("could not record the session end: {e}");
            }
        }
        r.session(None).await;
    } else if direction == SessionDirection::Outgoing && !aborted {
        let reason = match &end {
            SessionEnd::HelloTimeout => NO_HELLO.to_string(),
            SessionEnd::Protocol(m) => format!("the session could not start: {m}"),
            _ => "the link ended before the session started".to_string(),
        };
        r.call_failed(&info.peer, reason).await;
    }
    match end {
        SessionEnd::ControlLost | SessionEnd::DataLost | SessionEnd::LocalDrop => After::Recover,
        SessionEnd::Disconnected if aborted => {
            After::Idle(Some("aborted: delivery of anything not yet confirmed is uncertain".into()))
        }
        SessionEnd::Disconnected => After::Idle(None),
        // The application session ended but the link may still be up.
        SessionEnd::HelloTimeout | SessionEnd::Protocol(_) | SessionEnd::CommandsClosed => {
            match st.disconnect(Duration::from_secs(60)).await {
                Ok(()) => After::Idle(None),
                Err(StationError::Timeout(_)) => match st.abort().await {
                    Ok(()) => After::Idle(None),
                    Err(_) => After::Recover,
                },
                Err(_) => After::Recover,
            }
        }
    }
}
