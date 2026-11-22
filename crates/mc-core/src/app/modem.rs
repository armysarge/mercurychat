//! The modem task (M2): one per selected profile. It owns the `Station`, does
//! the attach and everything after it, and reports back to the service actor
//! by message, tagged with the generation it was started for. The actor never
//! awaits it, so a slow or hung attach cannot stall commands or snapshots, and
//! a report from a modem the actor has moved on from is simply dropped.
//!
//! A real Mercury (an Existing profile) is attached only while this task holds
//! its endpoint lock (MOD-02). An occupied endpoint is reported once, with the
//! lock's own text, and the task ends: no takeover loop.

use super::broadcast::{broadcast_gate, BroadcastKind, GateRefusal};
use super::chat::ChatMessage;
use super::events::{ModemState, SessionDirection, SessionInfo};
use crate::profile::{ModemTarget, StationProfile};
use crate::session::{PeerMsg, SessionCommand, SessionEnd, SessionEvent};
use crate::station::{ConnectInfo, Observation, Station, StationConfig, StationError, StationRequest};
use mc_storage::{SessionRow, Store};
use mch_protocol::ObjectId;
use mercury_adapter::broadcast::{BroadcastAbort, BroadcastPort};
use mercury_adapter::kiss::KissFrame;
use mercury_adapter::telemetry::{Spectrum, TelemetryClient, TelemetryCmd, TelemetryLink, TelemetryMsg};
use mercury_adapter::{Callsign, Command, Endpoint, TuneArg};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::AbortHandle;

/// Where a modem task keeps its data and takes its locks.
#[derive(Debug, Clone, Default)]
pub struct ModemOptions {
    /// `{data root}/stations`: each profile's station data lives in `{id}/`.
    pub stations_root: Option<PathBuf>,
    /// The shared endpoint lock folder; `None` takes no endpoint lock (tests).
    pub locks_dir: Option<PathBuf>,
    /// HELLO wait over a real Mercury; `None` is `RADIO_HELLO_TIMEOUT`.
    pub radio_hello_timeout: Option<Duration>,
    /// Held by a modem task for its whole life (shared by every task the actor
    /// starts): a new task waits for every stopped predecessor to let go of
    /// the modem, its sockets and its locks before it takes them.
    pub slot: Arc<tokio::sync::Mutex<()>>,
    /// Where the modem's telemetry client publishes the latest spectrum.
    pub spectrum: SpectrumFeed,
    /// The simulator opens its broadcast port and its SIM1 calls CQ and
    /// beacons after attach and every minute (the desktop app; tests that
    /// pin event sequences leave it off).
    pub sim_peer_broadcasts: bool,
    /// The simulator runs a fake rigctld its PTT keys (M3b; the desktop app
    /// turns this on, default off).
    pub sim_rig: bool,
    /// The active profile's file settings, for the station (M4).
    pub files: FileOptions,
}

/// What the station takes from the profile's file settings (M4).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileOptions {
    /// `None` keeps the station's default.
    pub max_file: Option<u64>,
    pub receive_dir: Option<PathBuf>,
}

impl FileOptions {
    fn apply(&self, cfg: &mut StationConfig) {
        if let Some(n) = self.max_file {
            cfg.max_file = n;
        }
        cfg.receive_dir = self.receive_dir.clone();
    }
}

/// The latest valid spectrum from Mercury's telemetry, shared by every modem
/// task the actor starts; `None` whenever there is none to show.
#[derive(Debug, Clone)]
pub struct SpectrumFeed(pub watch::Sender<Option<Arc<Spectrum>>>);

impl Default for SpectrumFeed {
    fn default() -> Self {
        SpectrumFeed(watch::channel(None).0)
    }
}

/// What the actor asks of the modem.
#[derive(Debug)]
pub enum ModemCmd {
    /// The Receive only choice changed (it is in the shared flag): bring
    /// Mercury in line now if idle, mid-link by sending it, else at the
    /// earliest idle moment.
    ListenChanged,
    Call(Callsign),
    CancelCall,
    Disconnect,
    Abort,
    /// A queued message for `peer`, to go out if the session is with them.
    Submit {
        peer: String,
        id: ObjectId,
    },
    /// Call CQ once. Carried out only by an idle modem; refused elsewhere.
    Cq,
    /// Send one wrapped payload on the broadcast port. Idle only, as `Cq`.
    Broadcast {
        kind: BroadcastKind,
        payload: Vec<u8>,
    },
    /// The operator re-armed beacons. Retry a closed broadcast connection.
    ReopenBroadcast,
    /// Mercury's tuning carrier (M3b). Carried out while idle; `Off` also
    /// during a link (harmless there, and never left unsent).
    Tune(TuneArg),
    /// A minor-1 message for the running session (M3c B1); dropped, with a
    /// log line, when there is none.
    Peer(PeerMsg),
    /// A file command for the running session (M4); dropped, with a log
    /// line, when there is none.
    File(SessionCommand),
    /// New file settings: the station takes them now, a session from its next.
    FileSettings(FileOptions),
    Shutdown,
}

/// What became of a TUNE command (M3b).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TuneResult {
    /// The answer to the `TUNE ?` probe: false is an older Mercury.
    Supported(bool),
    Started,
    Stopped,
    /// Not carried out; the reason in plain words.
    Refused(String),
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
    /// Something the modem reported about the channel (CQ, MONITOR, PTT, ...).
    Observation(Observation),
    /// A frame from the broadcast port.
    Frame(KissFrame),
    /// The broadcast port was unavailable, closed or reopened, in plain words.
    Broadcast(String),
    /// The actor may have to close this connection while the modem is busy.
    BroadcastOpened(BroadcastAbort),
    /// The listen mode found at attach or re-attach (`None`: unknown).
    ListenMode(Option<u8>),
    /// The modem took a CQ or beacon the actor asked for.
    BroadcastSubmitted(BroadcastKind),
    /// The modem did not send it; `reason` is for the operator.
    BroadcastRefused {
        kind: BroadcastKind,
        reason: String,
    },
    /// The command was written but the modem did not answer: it may have
    /// gone out, so it is neither refused nor confirmed (03 section 6).
    BroadcastUnconfirmed {
        kind: BroadcastKind,
        note: String,
    },
    /// Mercury's telemetry WebSocket: its link, or a status.
    Telemetry(TelemetryMsg),
    /// What became of a TUNE command, or the probe after attach.
    Tune(TuneResult),
    /// A minor-1 message from the session's peer (M3c B1).
    Peer(PeerMsg),
    /// A file transfer event from the session (M4).
    File(SessionEvent),
    /// The simulator's fake rigctld listens here (M3b).
    SimRig(std::net::SocketAddr),
    /// Where to send commands for this modem's telemetry client (M3b: the
    /// per-band TX level). Sent once, when the client starts.
    TelemetryCmds(mpsc::Sender<TelemetryCmd>),
}

/// What a failed CQFRAME means, truthfully (03 section 6): a modem that never
/// took the command, or refused it, did not send it; one that gave no answer
/// may have.
fn cq_failure(e: &StationError) -> ReportKind {
    let kind = BroadcastKind::Cq;
    match e {
        StationError::ControlLost | StationError::Detached => {
            ReportKind::BroadcastRefused { kind, reason: GateRefusal::Detached.message() }
        }
        StationError::Timeout(_) => ReportKind::BroadcastUnconfirmed {
            kind,
            note: "CQ not confirmed: no answer from the modem; it may have been sent".into(),
        },
        other => ReportKind::BroadcastRefused { kind, reason: format!("not sent: the modem refused CQ ({other})") },
    }
}

/// The actor's end of one modem task. Dropping it stops the task.
pub struct ModemHandle {
    pub generation: u64,
    cmds: mpsc::Sender<ModemCmd>,
    /// The Receive only choice, shared with the task: it is read whenever the
    /// task decides about LISTEN ON (attach, recovery, after a call or link),
    /// so a change is never lost however busy the modem is.
    receive_only: Arc<AtomicBool>,
}

impl ModemHandle {
    /// Queue a command; false if the task has ended or is backed up.
    pub fn send(&self, cmd: ModemCmd) -> bool {
        self.cmds.try_send(cmd).is_ok()
    }

    /// Set Receive only. The shared flag is the truth; the nudge only makes an
    /// idle modem act on it at once (a linked one only for LISTEN ON: LISTEN
    /// OFF waits for the link to end).
    pub fn set_receive_only(&self, on: bool) {
        self.receive_only.store(on, Ordering::SeqCst);
        let _ = self.cmds.try_send(ModemCmd::ListenChanged);
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
    let receive_only = Arc::new(AtomicBool::new(profile.receive_only));
    tokio::spawn(run(profile, opts, rx, Reporter { generation, tx: reports }, receive_only.clone()));
    ModemHandle { generation, cmds: tx, receive_only }
}

/// How long a new task waits for its stopped predecessors to let go of the
/// modem. A stopping task ends a call it was placing first (about 7 s).
const HANDOVER: Duration = Duration::from_secs(30);

/// How long a Disconnect waits for receipts of messages already handed to the
/// session before it ends the link anyway (they then go uncertain). Abort
/// still ends it at once.
const RECEIPT_DRAIN: Duration = Duration::from_secs(120);

/// After a hand-over, how long an attach that finds Mercury still occupied
/// (by the predecessor's just-closed sockets) is retried.
const HANDOVER_ATTACH: Duration = Duration::from_secs(5);

/// What cut a long modem wait short.
enum Cut {
    Stop,
    Abort,
}

/// Resolves when a long wait (recovery, ending a link) must give way: on
/// Shutdown or a dropped handle, and on Abort when `abortable`. A broadcast
/// is refused (`why`); anything else is ignored: the actor only allows it
/// once the modem is idle again.
async fn cut_short(cmds: &mut mpsc::Receiver<ModemCmd>, abortable: bool, r: &Reporter, why: GateRefusal) -> Cut {
    loop {
        match cmds.recv().await {
            Some(ModemCmd::Shutdown) | None => return Cut::Stop,
            Some(ModemCmd::Abort) if abortable => return Cut::Abort,
            Some(other) => {
                if !refuse_broadcast(r, &other, why).await {
                    tracing::debug!(?other, "ignored while the modem is busy")
                }
            }
        }
    }
}

/// A CQ or beacon that reached the modem anywhere but serve's idle branch:
/// refused with `why`, never queued (03 §6). False for any other command.
async fn refuse_broadcast(r: &Reporter, cmd: &ModemCmd, why: GateRefusal) -> bool {
    let kind = match cmd {
        ModemCmd::Cq => BroadcastKind::Cq,
        ModemCmd::Broadcast { kind, .. } => *kind,
        _ => return false,
    };
    r.send(ReportKind::BroadcastRefused { kind, reason: why.message() }).await;
    true
}

#[derive(Clone)]
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

/// The observation forwarder of one modem task: the station's observations
/// go to the actor as reports, so the serve and link loops never handle them.
/// Aborted when the task ends.
#[derive(Default)]
struct Forwarders(Vec<tokio::task::JoinHandle<()>>);

impl Drop for Forwarders {
    fn drop(&mut self) {
        for t in &self.0 {
            t.abort();
        }
    }
}

impl Forwarders {
    fn observe(&mut self, st: &mut Station, r: &Reporter) {
        // The station never waits on this channel: a full one drops and counts.
        let (tx, mut rx) = mpsc::channel(256);
        st.set_observer(tx);
        let r = r.clone();
        self.0.push(tokio::spawn(async move {
            while let Some(o) = rx.recv().await {
                r.send(ReportKind::Observation(o)).await;
            }
        }));
    }
}

/// The broadcast plane of one modem task, re-openable: opened after attach
/// and again after every recovery, because a restarted Mercury closes it. Its
/// frames go to the actor as reports from a forwarder of its own. Optional:
/// when it is unavailable or closes, the actor is told, never faulted.
struct Broadcast {
    /// Host and port, or `None` when the profile has no broadcast port.
    target: Option<(String, u16)>,
    port: Option<BroadcastPort>,
    frames: Option<tokio::task::JoinHandle<()>>,
    r: Reporter,
}

impl Drop for Broadcast {
    fn drop(&mut self) {
        self.close();
    }
}

impl Broadcast {
    fn new(target: Option<(String, u16)>, r: &Reporter) -> Broadcast {
        Broadcast { target, port: None, frames: None, r: r.clone() }
    }

    /// The open port, for an idle modem to send a beacon through.
    fn port(&self) -> Option<&BroadcastPort> {
        self.port.as_ref().filter(|p| !p.is_closed())
    }

    fn close(&mut self) {
        if let Some(t) = self.frames.take() {
            t.abort();
        }
        self.port = None;
    }

    /// (Re)open the port, dropping any previous connection first. `again`
    /// says so in the activity log.
    async fn open(&mut self, again: bool) {
        let Some((host, port)) = self.target.clone() else { return };
        self.close();
        let (bp, mut rx) = match BroadcastPort::connect(&host, port, Duration::from_secs(3)).await {
            Ok(opened) => opened,
            Err(e) => {
                let why = format!("broadcast port {port} unavailable ({e}): beacons are neither heard nor sent");
                self.r.send(ReportKind::Broadcast(why)).await;
                return;
            }
        };
        let r = self.r.clone();
        self.frames = Some(tokio::spawn(async move {
            while let Some(f) = rx.recv().await {
                r.send(ReportKind::Frame(f)).await;
            }
            let why = format!("broadcast port {port} closed: beacons are not heard until it is reopened");
            r.send(ReportKind::Broadcast(why)).await;
        }));
        let abort = bp.abort_handle();
        self.port = Some(bp);
        self.r.send(ReportKind::BroadcastOpened(abort)).await;
        if again {
            self.r.send(ReportKind::Broadcast(format!("broadcast port {port} reopened"))).await;
        }
    }
}

/// Mercury's telemetry for one modem task (OBS-02): optional, read-only and
/// reconnecting by itself; its messages go to the actor as reports. Started
/// after attach; it never affects the TNC connection or a session, and it
/// dies with the task (the spectrum is cleared and, if the actor still
/// follows this task, the link reported lost).
struct Telemetry {
    client: AbortHandle,
    forward: tokio::task::JoinHandle<()>,
    spectrum: watch::Sender<Option<Arc<Spectrum>>>,
    r: Reporter,
}

impl Telemetry {
    fn start(url: String, spectrum: &SpectrumFeed, r: &Reporter) -> Telemetry {
        let (tx, mut rx) = mpsc::channel(64);
        let (cmd_tx, cmd_rx) = mpsc::channel(8);
        let client = TelemetryClient::spawn_commanded(url, tx, spectrum.0.clone(), cmd_rx);
        let _ = r.tx.try_send(ModemReport { generation: r.generation, kind: ReportKind::TelemetryCmds(cmd_tx) });
        let fwd = r.clone();
        let forward = tokio::spawn(async move {
            while let Some(m) = rx.recv().await {
                fwd.send(ReportKind::Telemetry(m)).await;
            }
        });
        Telemetry { client, forward, spectrum: spectrum.0.clone(), r: r.clone() }
    }
}

impl Drop for Telemetry {
    fn drop(&mut self) {
        self.client.abort();
        self.forward.abort();
        self.spectrum.send_replace(None);
        let reason = "the modem connection ended".to_string();
        let kind = ReportKind::Telemetry(TelemetryMsg::Link(TelemetryLink::Lost { reason }));
        let _ = self.r.tx.try_send(ModemReport { generation: self.r.generation, kind });
    }
}

/// Resolves when the actor stops this task (Shutdown, or the handle dropped).
/// A broadcast that arrives before the modem is up is refused; anything else
/// is ignored.
async fn stopped(cmds: &mut mpsc::Receiver<ModemCmd>, r: &Reporter) {
    while let Some(cmd) = cmds.recv().await {
        if matches!(cmd, ModemCmd::Shutdown) {
            return;
        }
        refuse_broadcast(r, &cmd, GateRefusal::NotIdle(ModemState::Attaching)).await;
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

async fn run(
    profile: StationProfile,
    opts: ModemOptions,
    mut cmds: mpsc::Receiver<ModemCmd>,
    r: Reporter,
    receive_only: Arc<AtomicBool>,
) {
    // A task this one replaces may still hold the modem (recovering, ending a
    // link): wait for it first, or its locks look like another instance. The
    // slot is declared first, so it is released last, after the locks.
    let mut handed_over = false;
    let _slot = match opts.slot.clone().try_lock_owned() {
        Ok(slot) => slot,
        Err(_) => {
            handed_over = true;
            r.state(ModemState::Attaching, Some("closing the previous connection".into())).await;
            tokio::select! {
                slot = tokio::time::timeout(HANDOVER, opts.slot.clone().lock_owned()) => match slot {
                    Ok(slot) => slot,
                    Err(_) => {
                        let s = HANDOVER.as_secs();
                        return r.fault(format!("the previous modem connection did not close within {s} s")).await;
                    }
                },
                _ = stopped(&mut cmds, &r) => return,
            }
        }
    };
    let Some(stations) = opts.stations_root.clone() else {
        return r.fault("no data folder for the station".into()).await;
    };
    let station_dir = stations.join(&profile.id);
    let call = match Callsign::parse(&profile.callsign) {
        Ok(c) => c,
        Err(e) => return r.fault(format!("callsign: {e}")).await,
    };
    let mut forwarders = Forwarders::default();
    match profile.modem.clone() {
        ModemTarget::Existing { host, control_port, data_port, broadcast_port, telemetry_url } => {
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
            opts.files.apply(&mut cfg);
            let mut station = match open_station(cfg).await {
                Ok(s) => s,
                Err(e) => return r.fault(format!("station data: {e}")).await,
            };
            forwarders.observe(&mut station, &r);
            // Mercury may still see the predecessor's sockets for a moment
            // after they close: after a hand-over, "occupied" is retried briefly.
            let patience = if handed_over { HANDOVER_ATTACH } else { Duration::ZERO };
            let attach = async {
                let deadline = tokio::time::Instant::now() + patience;
                loop {
                    match station.attach().await {
                        Err(StationError::Attach(mercury_adapter::AttachError::Occupied { .. }))
                            if tokio::time::Instant::now() < deadline =>
                        {
                            tokio::time::sleep(Duration::from_millis(100)).await
                        }
                        other => return other,
                    }
                }
            };
            tokio::select! {
                res = attach => if let Err(e) = res {
                    return r.fault(format!("could not attach to Mercury at {host}:{control_port}: {e}")).await;
                },
                _ = stopped(&mut cmds, &r) => return,
            }
            let mut broadcast = Broadcast::new(broadcast_port.map(|p| (host.clone(), p)), &r);
            tokio::select! {
                _ = broadcast.open(false) => {}
                _ = stopped(&mut cmds, &r) => return,
            }
            let _telemetry = telemetry_url.map(|url| Telemetry::start(url, &opts.spectrum, &r));
            let mut station = station;
            probe_tune(&mut station, &r).await;
            r.send(ReportKind::ListenMode(station.listen_mode())).await;
            r.state(ModemState::Ready, None).await;
            serve(&mut station, cmds, &r, receive_only, &mut broadcast).await;
        }
        ModemTarget::Simulator => {
            let on_air = opts.sim_peer_broadcasts;
            simulator(&station_dir, call, receive_only, cmds, r, &mut forwarders, on_air, opts.sim_rig, &opts.files)
                .await
        }
    }
}

#[cfg(feature = "sim")]
async fn simulator(
    station_dir: &Path,
    call: Callsign,
    receive_only: Arc<AtomicBool>,
    mut cmds: mpsc::Receiver<ModemCmd>,
    r: Reporter,
    forwarders: &mut Forwarders,
    on_air: bool,
    with_rig: bool,
    files: &FileOptions,
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
        _ = stopped(&mut cmds, &r) => return,
    };
    // With SIM1 on the air, our side opens the fake's broadcast port before
    // Ready, as it would Mercury's, so beacons are heard and sent (ruling P4).
    let target = on_air.then(|| ("127.0.0.1".to_string(), sim.broadcast_port()));
    let mut broadcast = Broadcast::new(target, &r);
    tokio::select! {
        _ = broadcast.open(false) => {}
        _ = stopped(&mut cmds, &r) => return,
    }
    let mut sim = sim;
    files.apply(&mut sim.station.cfg);
    // The rig a Mercury keys through: it follows our side's PTT.
    let rig = match with_rig {
        true => Some(std::sync::Arc::new(mercury_adapter::fake_rig::FakeRigctld::start().await)),
        false => None,
    };
    if let Some(rig) = &rig {
        let weak = std::sync::Arc::downgrade(rig);
        sim.net.on_ptt(0, move |on| {
            if let Some(rig) = weak.upgrade() {
                rig.set_ptt(on);
            }
        });
        r.send(ReportKind::SimRig(rig.addr)).await;
    }
    probe_tune(&mut sim.station, &r).await;
    r.send(ReportKind::ListenMode(sim.station.listen_mode())).await;
    r.state(ModemState::Ready, Some("simulator".into())).await;
    forwarders.observe(&mut sim.station, &r);
    if on_air {
        sim.start_peer_broadcasts();
    }
    serve(&mut sim.station, cmds, &r, receive_only, &mut broadcast).await;
}

#[cfg(not(feature = "sim"))]
async fn simulator(
    _dir: &Path,
    _call: Callsign,
    _receive_only: Arc<AtomicBool>,
    _cmds: mpsc::Receiver<ModemCmd>,
    r: Reporter,
    _forwarders: &mut Forwarders,
    _on_air: bool,
    _with_rig: bool,
    _files: &FileOptions,
) {
    r.fault("the simulator is not part of this build".into()).await;
}

/// Turn listening on after attach or recovery (D18). One attempt, never
/// retried: a refusal leaves the modem Ready with the reason shown, and the
/// next successful recovery tries again.
/// Does this Mercury have TUNE (M3b)? An older one answers WRONG. Asked
/// while attaching, so the answer is in before Ready.
async fn probe_tune(st: &mut Station, r: &Reporter) {
    let supported = match st.tune(TuneArg::Query).await {
        Ok(()) => true,
        Err(StationError::Wrong(_)) => false,
        Err(e) => {
            tracing::warn!("TUNE probe: {e}");
            false
        }
    };
    r.send(ReportKind::Tune(TuneResult::Supported(supported))).await;
}

async fn arm(st: &mut Station, r: &Reporter, listening: &mut bool) {
    *listening = false;
    match st.set_listen(true).await {
        Ok(()) => {
            *listening = true;
            r.state(ModemState::Listening, None).await;
        }
        Err(e) => {
            *listening = false;
            r.state(ModemState::Ready, Some(format!("not answering calls: {e}"))).await;
        }
    }
}

/// Bring Mercury's LISTEN in line with the Receive only flag whenever the
/// modem is idle again (after a call, a link, or a change made while busy).
/// This is where a Receive only chosen during a call or a session takes
/// effect: LISTEN OFF is never sent on a live link (lab F2). Mercury reports
/// DISCONNECTED only after its last frame is keyed, so LISTEN OFF now strands
/// no one (lab Q1). `dirty`: a LISTEN ON was sent mid-link and its answer was
/// not seen, so LISTEN is sent again. A LISTEN ON that fails leaves Ready
/// with the reason (never retried); a LISTEN OFF that fails is a fault:
/// Mercury may still answer.
async fn reconcile(
    st: &mut Station,
    receive_only: &AtomicBool,
    on: &mut bool,
    dirty: &mut bool,
) -> Result<Option<String>, String> {
    let want = !receive_only.load(Ordering::SeqCst);
    if want == *on && !*dirty {
        return Ok(None);
    }
    *dirty = false;
    match st.set_listen(want).await {
        Ok(()) => {
            *on = want;
            Ok(None)
        }
        Err(e) if want => {
            *on = false;
            Ok(Some(format!("not answering calls: {e}")))
        }
        Err(e) => Err(format!("LISTEN OFF: {e}")),
    }
}

/// How long a call may ring before it is given up (docs/04 section 3).
const CALL_TIMEOUT: Duration = Duration::from_secs(120);

/// Why a broadcast is refused while a link is being taken down, or while
/// the modem recovers.
const ENDING_LINK: GateRefusal = GateRefusal::NotIdle(ModemState::Draining);
const RECOVERING: GateRefusal = GateRefusal::NotIdle(ModemState::Recovering);

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
/// A broadcast meanwhile is refused: calling counts from CONNECT (03 §6).
async fn call_interrupted(cmds: &mut mpsc::Receiver<ModemCmd>, r: &Reporter) -> Calling {
    loop {
        match cmds.recv().await {
            Some(ModemCmd::CancelCall | ModemCmd::Abort) => return Calling::Cancelled,
            Some(ModemCmd::Shutdown) | None => return Calling::Stopped,
            Some(other) => {
                if !refuse_broadcast(r, &other, GateRefusal::Calling).await {
                    tracing::debug!(?other, "ignored while calling")
                }
            }
        }
    }
}

/// The attached modem: holds the station (and so its locks) and carries out
/// the actor's commands until stopped. Idle, it also watches for incoming
/// links and for the loss of the control connection.
async fn serve(
    st: &mut Station,
    mut cmds: mpsc::Receiver<ModemCmd>,
    r: &Reporter,
    receive_only: Arc<AtomicBool>,
    broadcast: &mut Broadcast,
) {
    let mut listening = false;
    // A LISTEN ON sent mid-link whose answer was not seen.
    let mut dirty = false;
    // Answering calls is the default (D18); Receive only turns it off.
    if !receive_only.load(Ordering::SeqCst) {
        arm(st, r, &mut listening).await;
    }
    let idle = |listening: bool| if listening { ModemState::Listening } else { ModemState::Ready };
    loop {
        let step = tokio::select! {
            c = cmds.recv() => Idle::Cmd(c),
            res = st.wait_incoming(Duration::from_secs(3600)) => Idle::Incoming(res),
        };
        let after = match step {
            Idle::Cmd(None | Some(ModemCmd::Shutdown)) => return,
            Idle::Cmd(Some(ModemCmd::ListenChanged)) => {
                match reconcile(st, &receive_only, &mut listening, &mut dirty).await {
                    Ok(detail) => r.state(idle(listening), detail).await,
                    Err(e) => return r.fault(e).await,
                }
                continue;
            }
            Idle::Cmd(Some(ModemCmd::Call(peer))) => {
                // The actor has already reported Calling.
                let res = tokio::select! {
                    res = st.call(&peer, CALL_TIMEOUT) => Calling::Answered(res),
                    c = call_interrupted(&mut cmds, r) => c,
                };
                match res {
                    Calling::Stopped => {
                        let _ = st.cancel_call().await;
                        return;
                    }
                    Calling::Cancelled => {
                        let res = tokio::select! {
                            res = st.cancel_call() => Some(res),
                            _ = cut_short(&mut cmds, false, r, GateRefusal::Calling) => None,
                        };
                        match res {
                            Some(Ok(())) => After::Idle(None),
                            Some(Err(_)) => After::Recover,
                            None => After::Stop,
                        }
                    }
                    Calling::Answered(Ok(info)) => {
                        link(st, &mut cmds, r, info, SessionDirection::Outgoing, &receive_only, &mut dirty).await
                    }
                    Calling::Answered(Err(StationError::ControlLost | StationError::Detached)) => After::Recover,
                    Calling::Answered(Err(StationError::BadPeer(p))) => {
                        tokio::select! {
                            a = refuse_link(st, r, &p) => a,
                            _ = cut_short(&mut cmds, false, r, ENDING_LINK) => After::Stop,
                        }
                    }
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
            Idle::Cmd(Some(ModemCmd::Cq)) => match st.cq().await {
                Ok(()) => {
                    r.send(ReportKind::BroadcastSubmitted(BroadcastKind::Cq)).await;
                    continue;
                }
                Err(e) => {
                    let lost = matches!(e, StationError::ControlLost | StationError::Detached);
                    if lost {
                        tracing::warn!("modem control lost sending CQ: {e}");
                    }
                    r.send(cq_failure(&e)).await;
                    if lost {
                        After::Recover
                    } else {
                        continue;
                    }
                }
            },
            Idle::Cmd(Some(ModemCmd::Tune(arg))) => {
                let result = match (arg, st.tune(arg).await) {
                    (TuneArg::Query, res) => TuneResult::Supported(res.is_ok()),
                    (TuneArg::Level(_), Ok(())) => TuneResult::Started,
                    (TuneArg::Off, Ok(())) => TuneResult::Stopped,
                    (_, Err(StationError::Wrong(w))) => TuneResult::Refused(format!("Mercury answered WRONG to {w}")),
                    (_, Err(e)) => TuneResult::Refused(e.to_string()),
                };
                r.send(ReportKind::Tune(result)).await;
                continue;
            }
            Idle::Cmd(Some(ModemCmd::ReopenBroadcast)) => {
                if broadcast.port().is_none() {
                    broadcast.open(true).await;
                }
                continue;
            }
            Idle::Cmd(Some(ModemCmd::Broadcast { kind, payload })) => {
                // The actor has gated it; checked again against what this
                // modem itself knows.
                let gate = broadcast_gate(idle(listening), false, Some(payload.len()), st.listen_mode());
                let sent = match (gate, broadcast.port()) {
                    (Err(g), _) => Err(g.message()),
                    (Ok(()), None) => Err(GateRefusal::NoBroadcastPort.message()),
                    (Ok(()), Some(port)) => {
                        port.send(&payload).await.map_err(|e| format!("not sent: the broadcast port failed ({e})"))
                    }
                };
                match sent {
                    Ok(()) => r.send(ReportKind::BroadcastSubmitted(kind)).await,
                    Err(reason) => r.send(ReportKind::BroadcastRefused { kind, reason }).await,
                }
                continue;
            }
            Idle::Cmd(Some(ModemCmd::FileSettings(f))) => {
                f.apply(&mut st.cfg);
                continue;
            }
            Idle::Cmd(Some(other)) => {
                tracing::debug!(?other, "no call or session for this command");
                continue;
            }
            Idle::Incoming(Ok(info)) => {
                link(st, &mut cmds, r, info, SessionDirection::Incoming, &receive_only, &mut dirty).await
            }
            Idle::Incoming(Err(StationError::Timeout(_))) => continue,
            Idle::Incoming(Err(StationError::BadPeer(p))) => {
                tokio::select! {
                    a = refuse_link(st, r, &p) => a,
                    _ = cut_short(&mut cmds, false, r, ENDING_LINK) => After::Stop,
                }
            }
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
            After::Idle(detail) => match reconcile(st, &receive_only, &mut listening, &mut dirty).await {
                Ok(extra) => r.state(idle(listening), extra.or(detail)).await,
                Err(e) => return r.fault(e).await,
            },
            After::Recover => {
                // A recovered modem is not listening until told to be (below).
                listening = false;
                dirty = false;
                r.session(None).await;
                r.state(ModemState::Recovering, None).await;
                // Recovery can take a minute; a stop must not wait for it.
                let res = tokio::select! {
                    res = st.recover(5) => res,
                    _ = cut_short(&mut cmds, false, r, RECOVERING) => return,
                };
                if let Err(e) = res {
                    return r.fault(format!("could not recover the modem: {e}")).await;
                }
                // A Mercury that restarted closed the broadcast port too.
                tokio::select! {
                    _ = broadcast.open(true) => {}
                    _ = cut_short(&mut cmds, false, r, RECOVERING) => return,
                }
                r.send(ReportKind::ListenMode(st.listen_mode())).await;
                r.state(ModemState::Ready, None).await;
                // Receive only may have been chosen while recovering.
                if !receive_only.load(Ordering::SeqCst) {
                    arm(st, r, &mut listening).await;
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

    /// Whether a message handed to this session has no receipt yet.
    fn unconfirmed(&self) -> bool {
        self.submitted.iter().any(|id| {
            self.row(id).is_some_and(|m| matches!(m.state.as_str(), "queued" | "submitting" | "awaiting_receipt"))
        })
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
            SessionEvent::Negotiated { peer_app, caps, max_file, .. } if !self.negotiated => {
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
                    session_id: self.session_id.clone(),
                    bandwidth: self.info.bandwidth.clone(),
                    files: caps.iter().any(|c| c == mch_protocol::CAP_FILE),
                    peer_max_file: max_file,
                    caps,
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
            SessionEvent::Peer(m) => r.send(ReportKind::Peer(m)).await,
            ev @ (SessionEvent::FileOffered { .. }
            | SessionEvent::FileSending { .. }
            | SessionEvent::FileProgress { .. }
            | SessionEvent::FileReceived { .. }
            | SessionEvent::FileDelivered { .. }
            | SessionEvent::FileRejected { .. }
            | SessionEvent::FileFailed { .. }
            | SessionEvent::FileCancelled { .. }) => r.send(ReportKind::File(ev)).await,
            SessionEvent::PeerSendRefused { what, reason } => {
                tracing::warn!("{what} not sent: {reason}");
            }
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
    receive_only: &AtomicBool,
    dirty: &mut bool,
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
    // File settings that came during the session: the station takes them after.
    let mut file_settings: Option<FileOptions> = None;

    // Disconnect was asked for (no new submissions from then on).
    let mut ending = false;
    // Disconnect waits for receipts until then (RF-03: a disconnect drains).
    let mut drain_until: Option<tokio::time::Instant> = None;
    // The session's control pump sends it; its reply is not awaited here,
    // because the pump only runs while the session is polled.
    let send_wire = |wire: Command| {
        let (reply, _) = oneshot::channel();
        let _ = req_tx.try_send(StationRequest::Command(wire, reply));
    };

    let end = {
        let run = st.run_session(&info, &mut chat_rx, &mut req_rx, ev_tx);
        tokio::pin!(run);
        loop {
            tokio::select! {
                end = &mut run => break end,
                Some(ev) = ev_rx.recv() => {
                    link.on_event(r, ev).await;
                    if drain_until.is_some() && !link.unconfirmed() {
                        drain_until = None;
                        send_wire(Command::Disconnect);
                    }
                }
                _ = tokio::time::sleep_until(drain_until.unwrap_or_else(tokio::time::Instant::now)), if drain_until.is_some() => {
                    tracing::info!("receipts still outstanding after {} s; disconnecting", RECEIPT_DRAIN.as_secs());
                    drain_until = None;
                    send_wire(Command::Disconnect);
                }
                c = cmds.recv() => match c {
                    Some(ModemCmd::Disconnect) => {
                        if ending || aborted {
                            continue; // one DISCONNECT is enough
                        }
                        ending = true;
                        let detail = if link.unconfirmed() {
                            drain_until = Some(tokio::time::Instant::now() + RECEIPT_DRAIN);
                            "waiting for receipts, then disconnecting"
                        } else {
                            send_wire(Command::Disconnect);
                            "sending what the modem holds, then disconnecting"
                        };
                        r.state(ModemState::Draining, Some(detail.into())).await;
                    }
                    Some(ModemCmd::Abort) => {
                        aborted = true;
                        drain_until = None;
                        send_wire(Command::Abort);
                        r.state(ModemState::Draining, Some("aborting".into())).await;
                    }
                    Some(ModemCmd::ListenChanged) => {
                        // Receive only never touches a live link. Mercury takes
                        // LISTEN OFF mid-link by releasing the radio with no
                        // DISCONNECT, which strands the peer until its own
                        // timeout (lab F2), so LISTEN OFF waits for the link to
                        // end: `reconcile` sends it once idle. LISTEN ON is
                        // harmless and goes now; its answer is not seen here,
                        // so it is repeated once idle (dirty).
                        if !receive_only.load(Ordering::SeqCst) {
                            send_wire(Command::Listen(true));
                            *dirty = true;
                        }
                    }
                    Some(ModemCmd::Tune(TuneArg::Off)) => send_wire(Command::Tune(TuneArg::Off)),
                    Some(ModemCmd::Peer(m)) => {
                        if ending || aborted || link.chats.try_send(SessionCommand::Peer(m)).is_err() {
                            tracing::warn!("a peer message was dropped: the session is ending or not taking it");
                        }
                    }
                    Some(ModemCmd::Submit { peer, id }) => {
                        if peer == link.info.peer && !ending && !aborted {
                            link.submit(r, id.0).await;
                        }
                    }
                    Some(ModemCmd::File(c)) => {
                        if ending || aborted || link.chats.try_send(c).is_err() {
                            tracing::warn!("a file command was dropped: the session is ending or not taking it");
                        }
                    }
                    Some(ModemCmd::FileSettings(f)) => file_settings = Some(f),
                    Some(ModemCmd::Shutdown) | None => {
                        // Leaving: the modem ends the link when our sockets close.
                        stop = true;
                        break SessionEnd::CommandsClosed;
                    }
                    Some(other) => {
                        let state = if ending || aborted {
                            ModemState::Draining
                        } else if link.negotiated {
                            ModemState::Connected
                        } else {
                            ModemState::Negotiating
                        };
                        if !refuse_broadcast(r, &other, GateRefusal::NotIdle(state)).await {
                            tracing::debug!(?other, "ignored during a session")
                        }
                    }
                },
            }
        }
    };
    while let Ok(ev) = ev_rx.try_recv() {
        link.on_event(r, ev).await;
    }
    if let Some(f) = file_settings {
        f.apply(&mut st.cfg);
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
        // Ending it can take a minute; Shutdown and Abort still apply meanwhile.
        SessionEnd::HelloTimeout | SessionEnd::Protocol(_) | SessionEnd::CommandsClosed => {
            let cut = {
                let ended = st.end_link_after(&end, Duration::from_secs(60));
                tokio::select! {
                    res = ended => Ok(res),
                    c = cut_short(cmds, true, r, ENDING_LINK) => Err(c),
                }
            };
            match cut {
                Ok(Ok(())) => After::Idle(None),
                Ok(Err(_)) => After::Recover,
                Err(Cut::Stop) => After::Stop,
                Err(Cut::Abort) => match st.abort().await {
                    Ok(()) => After::Idle(Some("aborted: delivery of anything not yet confirmed is uncertain".into())),
                    Err(_) => After::Recover,
                },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cq_that_timed_out_may_have_gone_out_so_it_is_not_refused() {
        match cq_failure(&StationError::Timeout("OK")) {
            ReportKind::BroadcastUnconfirmed { kind, note } => {
                assert_eq!(kind, BroadcastKind::Cq);
                assert!(note.contains("may have been sent"), "{note}");
            }
            other => panic!("a CQ with no answer is not Refused: {other:?}"),
        }
    }

    #[test]
    fn a_cq_the_modem_never_took_or_refused_is_refused() {
        for e in [StationError::Detached, StationError::ControlLost, StationError::Wrong("CQFRAME".into())] {
            match cq_failure(&e) {
                ReportKind::BroadcastRefused { kind, reason } => {
                    assert_eq!(kind, BroadcastKind::Cq);
                    assert!(reason.starts_with("not sent: "), "{reason}");
                }
                other => panic!("{e:?}: {other:?}"),
            }
        }
    }
}
