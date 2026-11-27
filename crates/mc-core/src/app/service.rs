//! The application service actor (docs/02 §3). It alone owns the store and
//! the event log. UI commands arrive on a bounded queue; a full queue is
//! refused at once rather than making the UI wait. Every state change is
//! persisted first, then published as an event.

use super::beacon::{
    bytes_left, BeaconContentView, BeaconPrefs, BeaconState, Tick, MAX_INTERVAL_MIN, MIN_INTERVAL_MIN, SILENT_HOLD,
    STATUS_MAX,
};
use super::broadcast::{
    activity_text, broadcast_gate, BroadcastKind, BroadcastOutcome, GateRefusal, ListenMode, NotConfirmedReason,
    Tracker,
};
use super::broadcast_text;
use super::chat::{self, ChatMessage, Conversation};
use super::contract::{AppCommand, AppError, AppSnapshot, CommandOk};
use super::discovery::{self, Discovery};
use super::events::{AppEvent, EventEnvelope, EventLog, ModemDetail, ModemState, SessionInfo, Since};
use super::logbook::{self, Qso, Start, FINALISE_SETTING};

/// "Listen before transmit": Mercury's busy detector (owner, 24 Nov), on unless set to "false".
pub const BUSY_DETECT_SETTING: &str = "channel.busy_detect";
use super::modem::{self, ModemCmd, ModemHandle, ModemOptions, ModemReport, ReportKind};
use super::notify::{NotifyPrefs, NOTIFY_SETTING};
use super::telemetry::{Telemetry, Tuning};
use super::wizard::{WizardState, WIZARD_SETTING};
use crate::macros::{self, MacroDef};
use crate::profile::{from_row, to_row, ModemTarget, StationProfile};
use crate::station::Observation;
use mc_storage::{MacroRow, OutState, QsoEdit as RowEdit, QsoRow, StorageError, Store};
use mch_protocol::ObjectId;
use mercury_adapter::broadcast::BroadcastAbort;
use mercury_adapter::telemetry::Spectrum;
use mercury_adapter::Callsign;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, oneshot, watch};

mod audio;
mod files;
mod freq;
mod info;
mod qsy;
mod rig;
use freq::FreqSide;
use rig::RigSide;

const ACTIVE_PROFILE: &str = "activeProfileId";
/// The listen mode set at attach (MODE 1).
const DATAC3: u8 = 1;
/// The setting holding the interface language (`system` when absent).
pub const UI_LANGUAGE_SETTING: &str = "ui.language";
/// The values `ui.language` may hold. The UI maps `system` to the OS language.
pub const UI_LANGUAGES: [&str; 5] = ["system", "en", "es", "pt-BR", "fr"];
/// Longest history search text, in characters.
const SEARCH_MAX: usize = 128;

/// What a stored simulator profile reads in a build without the simulator.
pub const SIMULATOR_UNAVAILABLE: &str = "The simulator is not available in this build.";

/// Set once Mercury has accepted a sound card for a built-in profile (M5 §5.5).
fn audio_chosen_key(profile: &str) -> String {
    format!("builtin.audioChosen:{profile}")
}

fn macros_seeded_key(profile: &str) -> String {
    format!("macros.seeded:{profile}")
}

#[derive(Debug, Clone)]
pub struct AppOptions {
    pub command_queue: usize,
    pub event_ring: usize,
    /// The data root, which also enables the simulator; `None` disables it.
    pub sim_root: Option<PathBuf>,
    /// `{data root}/stations`; defaults to `{sim_root}/stations`.
    pub stations_root: Option<PathBuf>,
    /// The shared endpoint lock folder (MOD-02); `None` takes no endpoint locks.
    pub locks_dir: Option<PathBuf>,
    /// HELLO wait over a real Mercury; `None` is `station::RADIO_HELLO_TIMEOUT`.
    pub radio_hello_timeout: Option<std::time::Duration>,
    /// The simulator's SIM1 calls CQ and beacons, and the simulator opens its
    /// broadcast port (the desktop app turns this on; default off).
    pub sim_peer_broadcasts: bool,
    /// The simulator runs a fake rig behind it (M3b; the desktop app turns
    /// this on; default off).
    pub sim_rig: bool,
    /// How long Tune runs before MercuryChat stops it; `None` is 30 s.
    pub tune_limit: Option<std::time::Duration>,
    /// The rigctld program MercuryChat starts (tests): `None` looks on the
    /// PATH and in the profile's Hamlib folder; `Some(None)`: none is found.
    pub rigctld_program: Option<Option<PathBuf>>,
    /// Where MercuryChat's own rigctld listens; `None` is 127.0.0.1:4532.
    pub managed_listen: Option<std::net::SocketAddr>,
    /// The schedule's UTC clock and how often it is looked at (tests);
    /// `None` is the real clock and 15 s.
    pub schedule_clock: Option<fn() -> chrono::NaiveTime>,
    pub schedule_tick: Option<std::time::Duration>,
    /// The QSY windows (tests); `None` is B3's 60 s, 120 s and 20 s.
    pub qsy_windows: Option<super::qsy::QsyWindows>,
    /// The built-in modem's programs (M5): the release build's bundle, or a
    /// lab bundle in development; `None`: no built-in modem in this build.
    pub bundle: Option<crate::bundle::Bundle>,
    /// The app's log folder (`{data root}/logs`), for the diagnostics file.
    pub logs_dir: Option<PathBuf>,
    /// How often old heard records are removed while the app runs (tests);
    /// `None` is once a day.
    pub purge_every: Option<std::time::Duration>,
}

impl Default for AppOptions {
    fn default() -> Self {
        AppOptions {
            command_queue: 64,
            event_ring: 1024,
            sim_root: None,
            stations_root: None,
            locks_dir: None,
            radio_hello_timeout: None,
            sim_peer_broadcasts: false,
            sim_rig: false,
            tune_limit: None,
            rigctld_program: None,
            managed_listen: None,
            schedule_clock: None,
            schedule_tick: None,
            qsy_windows: None,
            bundle: None,
            logs_dir: None,
            purge_every: None,
        }
    }
}

/// How often a running app removes heard records past retention.
const PURGE_EVERY: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);

type Reply<T> = oneshot::Sender<Result<T, AppError>>;

enum Request {
    Command(Box<AppCommand>, Reply<CommandOk>),
    Since(u64, Reply<Since>),
    #[cfg(any(test, feature = "test-hooks"))]
    Beacon(Vec<u8>, Reply<CommandOk>),
}

#[derive(Clone)]
pub struct AppHandle {
    tx: mpsc::Sender<Request>,
    events: broadcast::Sender<EventEnvelope>,
    spectrum: watch::Receiver<Option<Arc<Spectrum>>>,
}

fn stopped() -> AppError {
    AppError::Internal { message: "the application service has stopped".into() }
}

impl AppHandle {
    pub async fn call(&self, cmd: AppCommand) -> Result<CommandOk, AppError> {
        let (reply, rx) = oneshot::channel();
        self.send(Request::Command(Box::new(cmd), reply))?;
        rx.await.map_err(|_| stopped())?
    }

    /// Events after `seq`, or `SnapshotNeeded` (docs/02 §3).
    pub async fn events_since(&self, seq: u64) -> Result<Since, AppError> {
        let (reply, rx) = oneshot::channel();
        self.send(Request::Since(seq, reply))?;
        rx.await.map_err(|_| stopped())?
    }

    /// Send one wrapped payload as a beacon, through the same gate and
    /// outcome tracking as the periodic beacons. Not a UI command: the UI
    /// never sends arbitrary payloads. Compiled only for tests (the `test-hooks`
    /// feature, which no shipped crate may enable; scripts/check.sh enforces it).
    #[cfg(any(test, feature = "test-hooks"))]
    #[doc(hidden)]
    pub async fn send_beacon_payload(&self, payload: Vec<u8>) -> Result<CommandOk, AppError> {
        let (reply, rx) = oneshot::channel();
        self.send(Request::Beacon(payload, reply))?;
        rx.await.map_err(|_| stopped())?
    }

    pub fn subscribe(&self) -> broadcast::Receiver<EventEnvelope> {
        self.events.subscribe()
    }

    /// The latest valid spectrum from Mercury's telemetry (OBS-02), or
    /// `None` while there is none: no telemetry, or it was lost. Not on the
    /// event stream: a viewer reads it at its own pace.
    pub fn spectrum(&self) -> watch::Receiver<Option<Arc<Spectrum>>> {
        self.spectrum.clone()
    }

    fn send(&self, r: Request) -> Result<(), AppError> {
        self.tx.try_send(r).map_err(|e| match e {
            mpsc::error::TrySendError::Full(_) => AppError::Busy { message: "too many pending requests".into() },
            mpsc::error::TrySendError::Closed(_) => stopped(),
        })
    }
}

/// Start the actor on the current tokio runtime.
pub fn spawn_app(store: Store, opts: AppOptions) -> AppHandle {
    let (tx, mut rx) = mpsc::channel(opts.command_queue.max(1));
    let (events, _) = broadcast::channel(opts.event_ring.max(1));
    let (reports, mut report_rx) = mpsc::channel(256);
    let stations_root = opts.stations_root.clone().or_else(|| opts.sim_root.as_ref().map(|r| r.join("stations")));
    let mut app = App {
        store,
        log: EventLog::new(opts.event_ring),
        events: events.clone(),
        modem: ModemState::Unavailable,
        modem_detail: None,
        builtin_detail: None,
        builtin_audio: None,
        bundle: opts.bundle,
        session: None,
        chat: None,
        sim_root: opts.sim_root,
        logs_dir: opts.logs_dir,
        purge_every: opts.purge_every.unwrap_or(PURGE_EVERY),
        purge_next: tokio::time::Instant::now() + opts.purge_every.unwrap_or(PURGE_EVERY),
        modem_opts: ModemOptions {
            stations_root,
            locks_dir: opts.locks_dir,
            radio_hello_timeout: opts.radio_hello_timeout,
            slot: Default::default(),
            spectrum: Default::default(),
            sim_peer_broadcasts: opts.sim_peer_broadcasts,
            sim_rig: opts.sim_rig,
            files: Default::default(),
            busy_detect: true,
            builtin: None,
        },
        reports,
        task: None,
        pending: None,
        discovery: Discovery::default(),
        listen_mode: ListenMode::default(),
        broadcasts: Tracker::default(),
        pending_message: None,
        broadcast_abort: None,
        broadcast_silent: false,
        reopen_port: false,
        beacon: BeaconState::new(),
        telemetry: Telemetry::default(),
        qso: None,
        mercury_listening: false,
        receive_only_shown: (false, false),
        rig: RigSide::with_options(opts.tune_limit, opts.rigctld_program, opts.managed_listen),
        audio: crate::app::audio::AudioSide::default(),
        freq: FreqSide::new(opts.schedule_clock, opts.schedule_tick),
        info: info::InfoSide::default(),
        qsy: qsy::QsySide::new(opts.qsy_windows.unwrap_or_default()),
        files: files::FilesSide::default(),
    };
    let spectrum = app.modem_opts.spectrum.0.subscribe();
    tokio::spawn(async move {
        // A station the wizard is still setting up stays in Receive only,
        // even if the app stopped between saving it and the wizard's progress.
        app.wizard_hold();
        // The selection survives a restart, so its modem is attached again (attach
        // only: a stored listen preference never arms transmission, RF-02).
        if let Some(p) = app.active_profile() {
            app.switch_modem(Some(p));
        }
        // The current rigctld client's messages (none without one).
        let mut rig_rx: Option<mpsc::Receiver<mercury_adapter::rigctl::RigMsg>> = None;
        loop {
            if app.rig.inbox_new {
                app.rig.inbox_new = false;
                rig_rx = app.rig.inbox.take();
            }
            let rig_wake = app.rig_wake();
            let due = app.broadcasts.deadline();
            let beacon_wake = app.beacon.sched.wake(tokio::time::Instant::now());
            let telemetry_wake = app.telemetry.wake();
            let qsy_wake = app.qsy_wake();
            let snr_wake = app.discovery.snr_wake();
            let purge_wake = app.purge_next;
            tokio::select! {
                req = rx.recv() => {
                    let Some(req) = req else { break };
                    match req {
                        Request::Command(cmd, reply) => {
                            // The link test waits on the network: it answers from its own task.
                            let cmd = match *cmd {
                                AppCommand::TestRig { config } => {
                                    app.test_rig(config, reply);
                                    continue;
                                }
                                AppCommand::BuiltInInfo => {
                                    app.builtin_info(reply);
                                    continue;
                                }
                                AppCommand::DiagnosticsPreview => {
                                    app.diagnostics(None, reply);
                                    continue;
                                }
                                AppCommand::SaveDiagnostics { path } => {
                                    app.diagnostics(Some(path), reply);
                                    continue;
                                }
                                AppCommand::SerialPorts => {
                                    tokio::spawn(async move {
                                        let r = tokio::task::spawn_blocking(mercury_adapter::serial::ports)
                                            .await
                                            .map(|ports| CommandOk::SerialPorts { ports })
                                            .map_err(|e| AppError::Internal { message: e.to_string() });
                                        let _ = reply.send(r);
                                    });
                                    continue;
                                }
                                AppCommand::CheckEndpoints { modem } => {
                                    tokio::spawn(async move {
                                        let r = match super::endpoints::check(&modem).await {
                                            Some(results) => Ok(CommandOk::Endpoints { results }),
                                            None => Err(AppError::Validation {
                                                errors: chat::field("modemKind", "Only My own Mercury is checked this way"),
                                            }),
                                        };
                                        let _ = reply.send(r);
                                    });
                                    continue;
                                }
                                AppCommand::HamlibModels { hamlib_dir } => {
                                    app.hamlib_models(hamlib_dir, reply);
                                    continue;
                                }
                                AppCommand::Shutdown => {
                                    app.shutdown(reply);
                                    continue;
                                }
                                // Shrinking and compressing run off the actor.
                                AppCommand::PrepareFile { path, photo } => {
                                    app.prepare_file(path, photo, reply);
                                    continue;
                                }
                                other => other,
                            };
                            let _ = reply.send(app.handle(cmd));
                            // A modem change is answered at once, then started.
                            if let Some(target) = app.pending.take() {
                                app.switch_modem(target);
                            }
                        }
                        Request::Since(seq, reply) => {
                            let _ = reply.send(Ok(app.log.since(seq)));
                        }
                        #[cfg(any(test, feature = "test-hooks"))]
                        Request::Beacon(payload, reply) => {
                            let sent = app.send_broadcast(BroadcastKind::Beacon, payload);
                            let _ = reply.send(sent.map(|_| CommandOk::Done));
                        }
                    }
                }
                Some(report) = report_rx.recv() => app.on_report(report),
                msg = async {
                    match rig_rx.as_mut() {
                        Some(r) => r.recv().await,
                        None => std::future::pending().await,
                    }
                } => match msg {
                    Some(msg) => app.on_rig(msg),
                    None => rig_rx = None, // the client ended
                },
                // A throttled rig event or a held call is due: handled just below.
                _ = tokio::time::sleep_until(rig_wake.unwrap_or_else(tokio::time::Instant::now)), if rig_wake.is_some() => {}
                _ = tokio::time::sleep_until(due.unwrap_or_else(tokio::time::Instant::now)), if due.is_some() => {
                    let out = app.broadcasts.expired(tokio::time::Instant::now());
                    app.publish_broadcasts(out);
                }
                _ = tokio::time::sleep_until(beacon_wake.unwrap_or_else(tokio::time::Instant::now)), if beacon_wake.is_some() => {
                    app.beacon_due();
                }
                // A throttled telemetry change is due: published just below.
                _ = tokio::time::sleep_until(telemetry_wake.unwrap_or_else(tokio::time::Instant::now)), if telemetry_wake.is_some() => {}
                // A broadcast's S/N window has closed.
                _ = tokio::time::sleep_until(snr_wake.unwrap_or_else(tokio::time::Instant::now)), if snr_wake.is_some() => {
                    let events = app.discovery.settle_snr(app.chat.as_mut(), tokio::time::Instant::now());
                    app.emit_all(events);
                }
                _ = tokio::time::sleep_until(purge_wake) => app.purge_due(),
                // A QSY window ends or a re-call is due: handled just below.
                _ = tokio::time::sleep_until(qsy_wake.unwrap_or_else(tokio::time::Instant::now)), if qsy_wake.is_some() => {}
            }
            app.sync_beacon();
            app.sync_telemetry();
            app.sync_rig();
            app.sync_audio();
            app.sync_qsy();
        }
    });
    AppHandle { tx, events, spectrum }
}

struct App {
    store: Store,
    log: EventLog,
    events: broadcast::Sender<EventEnvelope>,
    modem: ModemState,
    modem_detail: Option<String>,
    /// The built-in modem's own state (M5), as last reported.
    builtin_detail: Option<ModemDetail>,
    /// The running built-in Mercury's "sound card chosen" flag, shared with its task.
    builtin_audio: Option<Arc<std::sync::atomic::AtomicBool>>,
    bundle: Option<crate::bundle::Bundle>,
    logs_dir: Option<PathBuf>,
    /// When old heard records are next removed, and how often.
    purge_next: tokio::time::Instant,
    purge_every: std::time::Duration,
    /// The negotiated session, as the modem task last reported it.
    session: Option<SessionInfo>,
    /// The selected station's own store (its outbox, history and drafts),
    /// through the actor's own connection; the modem task has another.
    chat: Option<Store>,
    sim_root: Option<PathBuf>,
    modem_opts: ModemOptions,
    /// Where modem tasks report; the actor keeps one sender so it never closes.
    reports: mpsc::Sender<ModemReport>,
    /// The running modem task, if any. Dropping it stops the task.
    task: Option<ModemHandle>,
    /// The modem the last selection asked for, started after replying.
    pending: Option<Option<StationProfile>>,
    /// The activity log and broadcast counters; heard rows go to `chat`.
    discovery: Discovery,
    /// The modem's listen mode, for the beacon capacity gate.
    listen_mode: ListenMode,
    /// The one CQ or beacon awaiting its outcome.
    broadcasts: Tracker,
    /// The operator's broadcast message, queued and not yet transmitted.
    pending_message: Option<String>,
    broadcast_abort: Option<BroadcastAbort>,
    /// The closed port cannot take another beacon until an explicit retry or recovery.
    broadcast_silent: bool,
    /// The port was reset after a message and is not yet open again.
    reopen_port: bool,
    /// Periodic beacons: the schedule, the profile's settings, the status shown.
    beacon: BeaconState,
    /// Link quality and Mercury's telemetry (OBS-01), and its event throttle.
    telemetry: Telemetry,
    /// The record of the latest session this run, as stored.
    qso: Option<QsoRow>,
    /// Whether Mercury was last known to answer calls: from the idle states,
    /// and from a LISTEN ON sent mid-link. Receive only chosen while it is,
    /// during a call or a session, waits for the link to end (lab F2).
    mercury_listening: bool,
    /// Receive only as last published: (on, pending).
    receive_only_shown: (bool, bool),
    /// Rig control (M3b): the rigctld client, the policy and the held call.
    rig: RigSide,
    audio: crate::app::audio::AudioSide,
    freq: FreqSide,
    info: info::InfoSide,
    qsy: qsy::QsySide,
    files: files::FilesSide,
}

fn parse_peer(peer: &str) -> Result<String, AppError> {
    Callsign::parse(peer)
        .map(|c| c.as_str().to_string())
        .map_err(|e| AppError::Validation { errors: chat::field("peer", e.to_string()) })
}

/// Write an export where the operator's save dialog pointed: a whole path,
/// replaced if it exists.
fn write_export(path: &str, text: &str) -> Result<(), AppError> {
    let p = std::path::Path::new(path);
    if path.trim().is_empty() || !p.is_absolute() {
        return Err(AppError::Validation { errors: chat::field("path", "Choose where to save the file") });
    }
    std::fs::write(p, text).map_err(|e| conflict(format!("could not write {path}: {e}")))
}

fn conflict(message: String) -> AppError {
    AppError::Conflict { message }
}

fn storage(e: StorageError) -> AppError {
    AppError::Storage { message: e.to_string() }
}

fn qso_error(e: StorageError, id: &str) -> AppError {
    match e {
        StorageError::QsoNotFound(_) => AppError::NotFound { id: id.to_string() },
        StorageError::QsoFinal(_) => conflict("the record is final and cannot be edited".into()),
        e => storage(e),
    }
}

impl App {
    fn emit(&mut self, event: AppEvent) {
        let env = self.log.push(event);
        let _ = self.events.send(env); // no subscriber is fine
    }

    fn emit_all(&mut self, events: Vec<AppEvent>) {
        for e in events {
            self.emit(e);
        }
    }

    fn handle(&mut self, cmd: AppCommand) -> Result<CommandOk, AppError> {
        match cmd {
            AppCommand::Snapshot => self.snapshot().map(CommandOk::Snapshot),
            AppCommand::ValidateProfile { profile } => {
                profile.validate().map(CommandOk::Profile).map_err(|errors| AppError::Validation { errors })
            }
            AppCommand::SaveProfile { profile } => {
                let p = profile.validate().map_err(|errors| AppError::Validation { errors })?;
                // Editing the profile in use: a new callsign or modem must reach
                // the modem now, not at the next selection.
                let old = self.active_profile().filter(|old| old.id == p.id);
                let reattach = old.as_ref().is_some_and(|old| old.callsign != p.callsign || old.modem != p.modem);
                let receive_only_changed = old.as_ref().is_some_and(|old| old.receive_only != p.receive_only);
                self.store.save_profile(&to_row(&p)).map_err(storage)?;
                self.emit(AppEvent::ProfilesChanged);
                if reattach {
                    self.log.bump_generation();
                    self.pending = Some(Some(p.clone()));
                } else if receive_only_changed {
                    // Receive only applies to the running modem now, too.
                    self.receive_only_changed(p.receive_only);
                }
                Ok(CommandOk::Profile(p))
            }
            AppCommand::DeleteProfile { id } => {
                if !self.store.delete_profile(&id).map_err(storage)? {
                    return Err(AppError::NotFound { id });
                }
                self.emit(AppEvent::ProfilesChanged);
                BeaconPrefs::forget(&mut self.store, &id);
                if let Err(e) = self.store.delete_setting(&macros_seeded_key(&id)) {
                    tracing::warn!("could not remove a macro marker: {e}");
                }
                if self.store.setting(ACTIVE_PROFILE).map_err(storage)?.as_deref() == Some(id.as_str()) {
                    self.store.delete_setting(ACTIVE_PROFILE).map_err(storage)?;
                    self.log.bump_generation();
                    self.emit(AppEvent::ActiveProfileChanged { id: None });
                    self.pending = Some(None);
                }
                Ok(CommandOk::Done)
            }
            AppCommand::SetReceiveOnly { on } => {
                // Always taken and stored at once (a safety switch), in any
                // state with a station. The modem task acts on it when it is
                // idle: after a call, a session or a recovery. LISTEN OFF is
                // never sent on a live link: Mercury takes it by dropping the
                // link with no DISCONNECT and strands the peer (lab F2), so
                // until the link ends the snapshot shows it as pending.
                if self.modem == ModemState::Unavailable {
                    let state = format!("{:?}", self.modem).to_lowercase();
                    return Err(conflict(format!("the modem is {state}: pick a station first")));
                }
                let Some(mut p) = self.active_profile() else { return Err(conflict("no station is selected".into())) };
                let w = self.wizard();
                if !on && w.holds_receive_only() && w.draft_profile_id.as_deref() == Some(p.id.as_str()) {
                    return Err(AppError::Refused {
                        reason: "wizard.receiveOnly",
                        message: "Choose how this station operates in the setup first".into(),
                    });
                }
                p.receive_only = on;
                self.store.save_profile(&to_row(&p)).map_err(storage)?;
                self.emit(AppEvent::ProfilesChanged);
                self.receive_only_changed(on);
                Ok(CommandOk::Done)
            }
            AppCommand::Call { peer } => {
                let peer = Callsign::parse(&peer)
                    .map_err(|e| AppError::Validation { errors: chat::field("peer", e.to_string()) })?;
                if self.session.is_some() || self.in_call() {
                    return Err(conflict("a call or session is already in progress".into()));
                }
                if !matches!(self.modem, ModemState::Ready | ModemState::Listening) {
                    return Err(conflict(format!(
                        "the modem is {}: calls can be made once it is ready",
                        self.state_word()
                    )));
                }
                self.refuse_if_tuning()?;
                if !self.hold_call(&peer) {
                    self.command_modem(ModemCmd::Call(peer.clone()))?;
                }
                // Answered at once: a second call is refused from here on.
                self.set_modem(ModemState::Calling, Some(peer.as_str().to_string()));
                Ok(CommandOk::Done)
            }
            AppCommand::CancelCall => {
                if self.cancel_held_call() {
                    return Ok(CommandOk::Done);
                }
                if self.modem != ModemState::Calling {
                    return Err(conflict("no call is being placed".into()));
                }
                self.command_modem(ModemCmd::CancelCall).map(|_| CommandOk::Done)
            }
            AppCommand::Disconnect => {
                if !matches!(self.modem, ModemState::Negotiating | ModemState::Connected) {
                    return Err(conflict("there is no session to disconnect".into()));
                }
                self.command_modem(ModemCmd::Disconnect).map(|_| CommandOk::Done)
            }
            AppCommand::Abort => {
                if self.stop_tune_for_abort() {
                    return Ok(CommandOk::Done);
                }
                if self.cancel_held_call() {
                    return Ok(CommandOk::Done);
                }
                if !self.in_call() {
                    return Err(conflict("there is no call or session to abort".into()));
                }
                self.command_modem(ModemCmd::Abort).map(|_| CommandOk::Done)
            }
            AppCommand::SendChat { peer, text } => {
                let peer = parse_peer(&peer)?;
                if text.trim().is_empty() {
                    return Err(AppError::Validation { errors: chat::field("text", "the message is empty") });
                }
                chat::check_size(&text).map_err(|errors| AppError::Validation { errors })?;
                let store = self.chat_store()?;
                let (n, bytes) = store.peer_queue_usage(&peer).map_err(storage)?;
                if n >= chat::QUEUE_MESSAGES || bytes + text.len() as u64 > chat::QUEUE_BYTES {
                    return Err(AppError::Validation {
                        errors: chat::field(
                            "text",
                            format!(
                                "the queue for {peer} is full: at most {} messages and 256 KiB can wait to be sent",
                                chat::QUEUE_MESSAGES
                            ),
                        ),
                    });
                }
                let id = ObjectId::random();
                store.insert_outgoing(&peer, &id.0, &text).map_err(storage)?;
                let msg = self.changed(&peer, &id.0)?;
                self.submit(&peer, id);
                Ok(CommandOk::Message(msg))
            }
            AppCommand::RetryChat { peer, id } => {
                let peer = parse_peer(&peer)?;
                let Some(oid) = ObjectId::from_hex(&id) else {
                    return Err(AppError::Validation { errors: chat::field("id", "not a message id") });
                };
                let store = self.chat_store()?;
                let state = store.out_state(&peer, &oid.0).map_err(storage)?;
                match state {
                    None => return Err(AppError::NotFound { id }),
                    Some(OutState::Uncertain | OutState::Failed) => {}
                    Some(s) => {
                        return Err(conflict(format!(
                            "the message is {}: only an uncertain or failed message can be sent again",
                            s.as_str().replace('_', " ")
                        )))
                    }
                }
                // The same id: if the peer already has it, it keeps one copy and
                // confirms again (CHT-04).
                store.set_out_state(&peer, &oid.0, OutState::Queued, None).map_err(storage)?;
                let msg = self.changed(&peer, &oid.0)?;
                self.submit(&peer, oid);
                Ok(CommandOk::Message(msg))
            }
            AppCommand::SaveDraft { peer, text } => {
                let peer = parse_peer(&peer)?;
                chat::check_size(&text).map_err(|errors| AppError::Validation { errors })?;
                let store = self.chat_store()?;
                if text.is_empty() {
                    store.delete_setting(&chat::draft_key(&peer)).map_err(storage)?;
                } else {
                    store.set_setting(&chat::draft_key(&peer), &text).map_err(storage)?;
                }
                Ok(CommandOk::Done)
            }
            AppCommand::Conversations => {
                let store = self.chat_store()?;
                let mut drafts = store.settings_with_prefix("draft:").map_err(storage)?;
                let mut list: Vec<Conversation> = store
                    .conversations()
                    .map_err(storage)?
                    .into_iter()
                    .map(|c| {
                        let draft = drafts.iter().position(|d| d.0 == c.peer).map(|i| drafts.remove(i).1);
                        Conversation {
                            peer: c.peer,
                            last_utc: c.last_utc,
                            messages: c.messages,
                            uncertain: c.uncertain,
                            draft,
                            last_body: Some(c.last_body),
                            last_out: c.last_out,
                        }
                    })
                    .collect();
                // A draft to a peer with no messages yet is a conversation too.
                list.extend(drafts.into_iter().map(|(peer, text, updated)| Conversation {
                    peer,
                    last_utc: updated,
                    messages: 0,
                    uncertain: 0,
                    draft: Some(text),
                    last_body: None,
                    last_out: false,
                }));
                list.sort_by(|a, b| b.last_utc.cmp(&a.last_utc));
                Ok(CommandOk::Conversations { conversations: list })
            }
            AppCommand::History { peer, before_seq, limit } => {
                let peer = parse_peer(&peer)?;
                let rows = self
                    .chat_store()?
                    .history(&peer, before_seq, limit.clamp(1, chat::HISTORY_PAGE))
                    .map_err(storage)?;
                Ok(CommandOk::History { messages: rows.iter().map(ChatMessage::from).collect() })
            }
            AppCommand::Heard => {
                let store = self.chat.as_ref().ok_or_else(|| conflict("no station is selected".into()))?;
                let stations = discovery::heard_list(store, self.discovery.offsets()).map_err(storage)?;
                Ok(CommandOk::Heard { stations })
            }
            AppCommand::HeardBroadcasts => {
                let store = self.chat.as_ref().ok_or_else(|| conflict("no station is selected".into()))?;
                let broadcasts = discovery::latest_broadcasts(store).map_err(storage)?;
                Ok(CommandOk::HeardBroadcasts { broadcasts })
            }
            AppCommand::SetFavourite { callsign, on } => {
                let call = Callsign::parse(&callsign)
                    .map_err(|e| AppError::Validation { errors: chat::field("callsign", e.to_string()) })?;
                let store = self.chat.as_mut().ok_or_else(|| conflict("no station is selected".into()))?;
                store.set_favourite(call.as_str(), on).map_err(storage)?;
                let station =
                    discovery::heard_station(store, call.as_str(), self.discovery.offsets()).map_err(storage)?;
                self.emit(AppEvent::Heard { station });
                Ok(CommandOk::Done)
            }
            AppCommand::Activity => Ok(CommandOk::Activity { entries: self.discovery.entries() }),
            AppCommand::SendCq => self.send_broadcast(BroadcastKind::Cq, Vec::new()).map(|_| CommandOk::Done),
            AppCommand::SetBeacon { on } => self.set_beacon(on).map(|_| CommandOk::Done),
            AppCommand::SetBeaconInterval { minutes } => self.set_beacon_interval(minutes).map(|_| CommandOk::Done),
            AppCommand::SetBeaconContent { include_grid } => {
                let id = self.beacon_profile()?;
                let prefs = BeaconPrefs { include_grid, ..self.beacon.prefs.clone() };
                self.beacon_fits(&prefs)?;
                BeaconPrefs::save_include_grid(&mut self.store, &id, include_grid).map_err(storage)?;
                self.beacon.prefs.include_grid = include_grid;
                Ok(CommandOk::Done)
            }
            AppCommand::SetBeaconFields { name, qth, status } => {
                let id = self.beacon_profile()?;
                let status = status.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
                if status.as_ref().is_some_and(|s| s.len() > STATUS_MAX || s.chars().any(char::is_control)) {
                    let why = format!("At most {STATUS_MAX} bytes, on one line");
                    return Err(AppError::Validation { errors: chat::field("status", why) });
                }
                let prefs = BeaconPrefs { name, qth, status, ..self.beacon.prefs.clone() };
                self.beacon_fits(&prefs)?;
                prefs.save_fields(&mut self.store, &id).map_err(storage)?;
                self.beacon.prefs = prefs;
                self.beacon_content()
            }
            AppCommand::BeaconContent => self.beacon_content(),
            AppCommand::PreviewBeaconFields { name, qth, status } => {
                let Some(p) = self.active_profile() else { return Err(conflict("no station is selected".into())) };
                let status = status.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
                let prefs = BeaconPrefs { name, qth, status, ..self.beacon.prefs.clone() };
                let left = bytes_left(&prefs, &p, self.listen_mode.current().unwrap_or(DATAC3));
                Ok(CommandOk::BeaconContent(BeaconContentView {
                    include_grid: prefs.include_grid,
                    name,
                    qth,
                    status: prefs.status,
                    bytes_left: left,
                }))
            }
            AppCommand::SendBeaconNow => self.send_beacon().map(|_| CommandOk::Done),
            AppCommand::SendBroadcastMessage { text } => self.send_broadcast_message(&text).map(|_| CommandOk::Done),
            AppCommand::Qsos { limit } => {
                let rows = self.chat_store()?.qsos(limit.clamp(1, 500)).map_err(storage)?;
                Ok(CommandOk::Qsos { qsos: rows.iter().map(Qso::from).collect() })
            }
            AppCommand::UpdateQso { id, edit } => {
                let edit = edit.validate().map_err(|errors| AppError::Validation { errors })?;
                let row = self.chat_store()?.update_qso(&id, &edit).map_err(|e| qso_error(e, &id))?;
                Ok(CommandOk::Qso(self.qso_changed(row)))
            }
            AppCommand::FinaliseQso { id } => {
                let row = self.chat_store()?.finalise_qso(&id).map_err(|e| qso_error(e, &id))?;
                Ok(CommandOk::Qso(self.qso_changed(row)))
            }
            AppCommand::SetFinaliseOnDisconnect { on } => {
                self.store.set_setting(FINALISE_SETTING, if on { "true" } else { "false" }).map_err(storage)?;
                Ok(CommandOk::Done)
            }
            AppCommand::SetBusyDetect { on } => {
                self.store.set_setting(BUSY_DETECT_SETTING, if on { "true" } else { "false" }).map_err(storage)?;
                self.modem_opts.busy_detect = on;
                let _ = self.command_modem(ModemCmd::BusyDetect(on));
                Ok(CommandOk::Done)
            }
            AppCommand::SetAdifMode { mode } => {
                let mode = super::export::adif_mode(&mode)
                    .map_err(|e| AppError::Validation { errors: chat::field("mode", e) })?;
                self.store.set_setting(super::export::ADIF_MODE_SETTING, &mode).map_err(storage)?;
                Ok(CommandOk::Done)
            }
            AppCommand::SetUiLanguage { language } => {
                if !UI_LANGUAGES.contains(&language.as_str()) {
                    return Err(AppError::Validation {
                        errors: chat::field("language", format!("not one of {}", UI_LANGUAGES.join(", "))),
                    });
                }
                self.store.set_setting(UI_LANGUAGE_SETTING, &language).map_err(storage)?;
                Ok(CommandOk::Done)
            }
            AppCommand::SetNotifyPrefs { prefs } => {
                prefs.validate().map_err(|errors| AppError::Validation { errors })?;
                let json = serde_json::to_string(&*prefs).map_err(|e| AppError::Internal { message: e.to_string() })?;
                self.store.set_setting(NOTIFY_SETTING, &json).map_err(storage)?;
                Ok(CommandOk::Done)
            }
            AppCommand::Macros => self.macros_list().map(|macros| CommandOk::Macros { macros }),
            AppCommand::SaveMacro { macro_ } => {
                let def = macros::validate(&macro_).map_err(|errors| AppError::Validation { errors })?;
                let profile = self.macro_profile()?;
                let existing = self.store.macros(&profile).map_err(storage)?;
                let (id, sort) = if def.id.is_empty() {
                    if existing.len() >= macros::MAX_MACROS {
                        let n = macros::MAX_MACROS;
                        return Err(AppError::Validation {
                            errors: chat::field("name", format!("At most {n} macros; delete one first")),
                        });
                    }
                    (uuid::Uuid::new_v4().to_string(), existing.iter().map(|m| m.sort).max().map_or(0, |s| s + 1))
                } else {
                    // Only this profile's own macro may be edited.
                    let Some(old) = existing.iter().find(|m| m.id == def.id) else {
                        return Err(AppError::NotFound { id: def.id });
                    };
                    (old.id.clone(), old.sort)
                };
                self.mark_macros_seeded(&profile)?;
                let row = MacroRow {
                    id: id.clone(),
                    profile_id: profile,
                    name: def.name.clone(),
                    body: def.body.clone(),
                    sort,
                    quick: def.quick,
                };
                self.store.save_macro(&row).map_err(storage)?;
                Ok(CommandOk::Macro(MacroDef { id, name: def.name, body: def.body, quick: def.quick }))
            }
            AppCommand::DeleteMacro { id } => {
                let profile = self.macro_profile()?;
                if !self.store.macros(&profile).map_err(storage)?.iter().any(|m| m.id == id) {
                    return Err(AppError::NotFound { id });
                }
                self.store.delete_macro(&id).map_err(storage)?;
                Ok(CommandOk::Done)
            }
            AppCommand::ExpandMacro { id, peer, allow_email } => {
                let p = self.active_profile().ok_or_else(|| conflict("no station is selected".into()))?;
                let call = peer.as_deref().map(parse_peer).transpose()?;
                let rows = self.store.macros(&p.id).map_err(storage)?;
                let Some(row) = rows.into_iter().find(|m| m.id == id) else { return Err(AppError::NotFound { id }) };
                let ctx = macros::MacroContext {
                    call,
                    mycall: p.callsign.clone(),
                    name: p.name.clone(),
                    qth: p.qth.clone(),
                    loc: p.grid.clone(),
                    rig: p.rig.clone(),
                    pwr: p.power.clone(),
                    ant: p.antenna.clone(),
                    about: p.about.clone(),
                    email: p.email.clone(),
                    utc: chrono::Utc::now().format("%H:%M").to_string(),
                    allow_email,
                };
                let out = macros::expand(&row.body, &ctx).map_err(|errors| AppError::Validation { errors })?;
                Ok(CommandOk::Expanded { text: out.text, missing: out.missing })
            }
            AppCommand::SearchHistory { query, limit } => {
                let q = query.trim();
                if q.is_empty() || q.chars().count() > SEARCH_MAX {
                    return Err(AppError::Validation {
                        errors: chat::field("query", format!("Enter 1 to {SEARCH_MAX} characters to search for")),
                    });
                }
                let rows = self.chat_store()?.search_messages(q, limit.clamp(1, 100)).map_err(storage)?;
                Ok(CommandOk::History { messages: rows.iter().map(ChatMessage::from).collect() })
            }
            AppCommand::SelectProfile { id } => {
                let target = match &id {
                    Some(i) => {
                        let row = self.store.profiles().map_err(storage)?.into_iter().find(|r| &r.id == i);
                        let Some(row) = row else { return Err(AppError::NotFound { id: i.clone() }) };
                        let p = from_row(&row).map_err(|e| AppError::Internal { message: e.message })?;
                        self.store.set_setting(ACTIVE_PROFILE, i).map_err(storage)?;
                        Some(p)
                    }
                    None => {
                        self.store.delete_setting(ACTIVE_PROFILE).map_err(storage)?;
                        None
                    }
                };
                // A new modem selection starts a new generation: events from
                // the previous modem are stale from here on (docs/02 §3).
                self.log.bump_generation();
                self.emit(AppEvent::ActiveProfileChanged { id });
                self.pending = Some(target);
                Ok(CommandOk::Done)
            }
            AppCommand::SetRigConfig { config } => self.set_rig_config(config),
            AppCommand::RigSet { change } => self.rig_set(change),
            AppCommand::DismissRigBanner { banner } => self.dismiss_rig_banner(banner),
            AppCommand::SetBandLevels { band, levels } => self.set_band_levels(band, levels),
            AppCommand::Tune { on } => self.tune(on),
            AppCommand::SetTuneLevel { dbfs } => self.set_tune_level(dbfs),
            AppCommand::BandLevels => self.band_levels(),
            AppCommand::RigSettings => self.rig_settings(),
            // Answered from the loop (see `test_rig`), never here.
            AppCommand::TestRig { .. } => Err(AppError::Internal { message: "the rig test runs on its own".into() }),
            AppCommand::BuiltInInfo
            | AppCommand::CheckEndpoints { .. }
            | AppCommand::SerialPorts
            | AppCommand::DiagnosticsPreview
            | AppCommand::SaveDiagnostics { .. } => Err(AppError::Internal { message: "checked off the actor".into() }),
            AppCommand::CheckBuiltInPorts { ports } => Ok(match crate::app::builtin::ports::check_free(&ports) {
                Ok(()) => CommandOk::PortCheck { taken: None, suggestion: None },
                Err(t) => CommandOk::PortCheck { taken: Some(t.port), suggestion: Some(t.suggestion) },
            }),
            AppCommand::GetWizard => Ok(CommandOk::Wizard(self.wizard())),
            AppCommand::ExportLogbook { format, finalised_only, path } => {
                self.export_logbook(format, finalised_only, path)
            }
            AppCommand::ExportConversation { peer, path } => self.export_conversation(peer, path),
            AppCommand::SetWizard { state } => {
                let state = state.normalised();
                let json = serde_json::to_string(&state).map_err(|e| AppError::Internal { message: e.to_string() })?;
                self.store.set_setting(WIZARD_SETTING, &json).map_err(storage)?;
                self.wizard_hold();
                Ok(CommandOk::Wizard(state))
            }
            AppCommand::HamlibModels { .. } => {
                Err(AppError::Internal { message: "the model list runs on its own".into() })
            }
            // Answered from the loop (see `shutdown`), never here.
            AppCommand::Shutdown => Err(AppError::Internal { message: "shutdown runs on its own".into() }),
            AppCommand::CatLog => self.cat_log(),
            AppCommand::CatRadios => Ok(CommandOk::CatRadios { radios: crate::app::catradios::names(&self.store) }),
            AppCommand::ImportCatFile { text } => {
                let r = crate::app::catradios::import(&mut self.store, &text).map_err(|message| {
                    AppError::Validation { errors: vec![crate::profile::FieldError { field: "file".into(), message }] }
                })?;
                Ok(CommandOk::CatImported { added: r.added, skipped: r.skipped })
            }
            AppCommand::AudioSettings => self.audio_settings(),
            AppCommand::SetAudio { system, input, output, channel } => self.set_audio(system, input, output, channel),
            AppCommand::Frequencies => self.frequencies(),
            AppCommand::SaveFrequency { entry } => self.save_frequency(entry),
            AppCommand::DeleteFrequency { id } => self.delete_frequency(&id),
            AppCommand::RestoreSuggested => self.restore_suggested(),
            AppCommand::SetRegion { region } => self.set_region(region),
            AppCommand::TuneToEntry { id } => self.tune_to_entry(&id),
            AppCommand::SetSchedule { schedule } => self.set_schedule(schedule),
            AppCommand::RequestInfo { what } => self.request_info(what),
            AppCommand::SendEmail => self.send_email(),
            AppCommand::ShareSettings => self.share_settings(),
            AppCommand::SetShareSettings { config } => self.set_share_settings(config),
            AppCommand::ProposeQsy { entry_ids, note } => self.propose_qsy(entry_ids, note),
            AppCommand::PrepareFile { .. } => Err(AppError::Internal { message: "prepared off the actor".into() }),
            AppCommand::DiscardPrepared { prep_id } => self.discard_prepared(&prep_id),
            AppCommand::SendFile { prep_id } => self.send_file(&prep_id),
            AppCommand::AcceptFile { id } => self.accept_file(&id),
            AppCommand::DeclineFile { id } => self.decline_file(&id),
            AppCommand::ExtendFile { id } => self.extend_file(&id),
            AppCommand::CancelFile { id } => self.cancel_file(&id),
            AppCommand::Transfers => self.transfers(),
            AppCommand::ShowInFolder { id } => self.show_in_folder(&id),
            AppCommand::FileSettings => self.file_settings(),
            AppCommand::SetFileSettings { settings } => self.set_file_settings(settings),
            AppCommand::AnswerQsy { accept } => self.answer_qsy(accept),
            AppCommand::QsyTuned => self.qsy_tuned(),
            AppCommand::SuggestQsy { to, entry_ids } => self.suggest_qsy(to, entry_ids),
            AppCommand::TuneSuggested { index } => self.tune_suggested(index),
        }
    }

    /// A call is being placed, or a session is up or ending.
    fn in_call(&self) -> bool {
        matches!(
            self.modem,
            ModemState::Calling | ModemState::Negotiating | ModemState::Connected | ModemState::Draining
        )
    }

    /// Receive only was just stored as `on`: tell the modem task, and show it.
    /// M5 §7: the diagnostics file's entries, gathered here and read off the
    /// actor; listed (`path` none) or written as a zip to `path`.
    fn diagnostics(&self, path: Option<String>, reply: Reply<CommandOk>) {
        use super::diagnostics::{self, DiagEntryView, Sources, Versions};
        if let Some(p) = &path {
            if p.trim().is_empty() || !std::path::Path::new(p).is_absolute() {
                let _ = reply
                    .send(Err(AppError::Validation { errors: chat::field("path", "Choose where to save the file") }));
                return;
            }
        }
        let profile = self.active_profile();
        let mercury_log = match (&profile, &self.modem_opts.stations_root) {
            (Some(p), Some(stations)) if matches!(p.modem, ModemTarget::BuiltIn { .. }) => {
                Some(stations.join(&p.id).join("mercury").join("mercury.log"))
            }
            _ => None,
        };
        let rig_log = self.rig_log_lines();
        let logs_dir = self.logs_dir.clone();
        let bundle = self.bundle.clone();
        tokio::spawn(async move {
            let done = tokio::task::spawn_blocking(move || {
                let built = bundle.map(|b| b.versions()).unwrap_or_default();
                let versions = Versions {
                    app: env!("CARGO_PKG_VERSION").into(),
                    mercury: built.mercury,
                    hamlib: built.hamlib,
                    os: format!("{} {}", std::env::consts::OS, std::env::consts::ARCH),
                };
                let entries = diagnostics::collect(&Sources {
                    logs_dir: logs_dir.as_deref(),
                    mercury_log: mercury_log.as_deref(),
                    rig_log: &rig_log,
                    profile: profile.as_ref(),
                    versions: &versions,
                });
                match path {
                    None => Ok(CommandOk::Diagnostics { entries: entries.iter().map(DiagEntryView::from).collect() }),
                    Some(path) => diagnostics::write_zip(&entries, std::path::Path::new(&path))
                        .map(|()| CommandOk::Exported { count: entries.len() as u32, path: path.clone() })
                        .map_err(|e| conflict(format!("could not write {path}: {e}"))),
                }
            })
            .await
            .unwrap_or_else(|e| Err(AppError::Internal { message: e.to_string() }));
            let _ = reply.send(done);
        });
    }

    /// M5 §7: the logbook, oldest first, as ADIF or CSV.
    fn export_logbook(
        &mut self,
        format: super::export::ExportFormat,
        finalised_only: bool,
        path: String,
    ) -> Result<CommandOk, AppError> {
        use super::export::{self, ExportFormat};
        let rows = self.chat_store()?.qsos(u32::MAX).map_err(storage)?;
        let mut qsos: Vec<Qso> = rows.iter().map(Qso::from).collect();
        qsos.reverse();
        let qsos = export::select(qsos, finalised_only);
        let text = match format {
            ExportFormat::Adif => export::adif(&qsos, env!("CARGO_PKG_VERSION"), &self.adif_mode()),
            ExportFormat::Csv => export::csv(&qsos),
        };
        write_export(&path, &text)?;
        Ok(CommandOk::Exported { count: qsos.len() as u32, path })
    }

    /// M5 §7: one conversation as text; its files listed, not their contents.
    fn export_conversation(&mut self, peer: String, path: String) -> Result<CommandOk, AppError> {
        use super::export::{self, ChatRow};
        let peer = Callsign::parse(&peer)
            .map_err(|e| AppError::Validation { errors: chat::field("peer", e.to_string()) })?
            .as_str()
            .to_string();
        let store = self.chat_store()?;
        let messages = store.messages(&peer).map_err(storage)?;
        let files = store.transfers(u32::MAX).map_err(storage)?;
        let mut rows: Vec<ChatRow> = messages
            .iter()
            .map(|m| {
                let outgoing = m.direction == "out";
                let utc = if outgoing {
                    m.created_utc.clone()
                } else {
                    m.received_utc.clone().unwrap_or_else(|| m.created_utc.clone())
                };
                ChatRow::Message { utc, outgoing, body: m.body.clone(), state: m.state.clone() }
            })
            .collect();
        rows.extend(files.iter().filter(|f| f.peer == peer).map(|f| ChatRow::File {
            utc: f.created_utc.clone(),
            outgoing: f.direction == mc_storage::Direction::Out,
            name: f.saved_name.clone().unwrap_or_else(|| f.original_name.clone()),
            size: f.size,
            state: f.state.as_str().to_string(),
        }));
        write_export(&path, &export::conversation_text(&peer, &rows))?;
        Ok(CommandOk::Exported { count: messages.len() as u32, path })
    }

    /// The wizard's progress (see `WizardState::from_setting`).
    fn wizard(&self) -> WizardState {
        let stored = self.store.setting(WIZARD_SETTING).ok().flatten();
        let has_profiles = self.store.profiles().map(|p| !p.is_empty()).unwrap_or(false);
        WizardState::from_setting(stored.as_deref(), has_profiles)
    }

    /// Put the wizard's draft station in Receive only while the wizard holds
    /// it (Review Focus 4): it never answers a call before the operator says.
    fn wizard_hold(&mut self) {
        let w = self.wizard();
        let holds = w.holds_receive_only();
        let Some(id) = w.draft_profile_id.filter(|_| holds) else { return };
        let Some(mut p) = self
            .store
            .profiles()
            .ok()
            .and_then(|rows| rows.into_iter().find(|r| r.id == id).and_then(|r| from_row(&r).ok()))
        else {
            return;
        };
        if p.receive_only {
            return;
        }
        p.receive_only = true;
        if let Err(e) = self.store.save_profile(&to_row(&p)) {
            tracing::warn!("could not hold the wizard's station in Receive only: {e}");
            return;
        }
        self.emit(AppEvent::ProfilesChanged);
        if self.active_profile().is_some_and(|a| a.id == id) {
            self.receive_only_changed(true);
        }
    }

    fn receive_only_changed(&mut self, on: bool) {
        if let Some(t) = &self.task {
            t.set_receive_only(on);
        }
        // Turning it off on a live link sends LISTEN ON at once (harmless).
        if !on && matches!(self.modem, ModemState::Negotiating | ModemState::Connected | ModemState::Draining) {
            self.mercury_listening = true;
        }
        self.sync_receive_only();
    }

    /// Receive only chosen, but waiting for the call or session to end
    /// before the modem stops answering calls (lab F2).
    fn receive_only_pending(&self, on: bool) -> bool {
        on && self.mercury_listening && self.in_call()
    }

    /// Read the active profile's choice: after it changed, or a new profile.
    fn sync_receive_only(&mut self) {
        let on = self.active_profile().is_some_and(|p| p.receive_only);
        self.show_receive_only(on);
    }

    /// Publish Receive only (`on`, pending) if it changed.
    fn show_receive_only(&mut self, on: bool) {
        let now = (on, self.receive_only_pending(on));
        if now != self.receive_only_shown {
            self.receive_only_shown = now;
            self.emit(AppEvent::ReceiveOnly { on: now.0, pending: now.1 });
        }
    }

    /// Send a CQ (payload unused) or a beacon, if the gate allows it now
    /// (DSC-02). A refusal is answered, published and logged, never queued.
    fn send_broadcast(&mut self, kind: BroadcastKind, payload: Vec<u8>) -> Result<(), AppError> {
        let len = (kind != BroadcastKind::Cq).then_some(payload.len());
        if self.rig.tune.active {
            return Err(self.refuse_broadcast(kind, "not sent: Tune is on".into()));
        }
        let calling = self.modem == ModemState::Calling;
        if let Err(g) = broadcast_gate(self.modem, calling, len, self.listen_mode.current()) {
            return Err(self.refuse_broadcast(kind, g.message()));
        }
        if self.broadcasts.busy() {
            return Err(self.refuse_broadcast(kind, "not sent: a broadcast is still being sent".into()));
        }
        if matches!(kind, BroadcastKind::Beacon | BroadcastKind::Message) && self.broadcast_silent {
            let why = NotConfirmedReason::BroadcastPortSilent.text();
            let refusal = self.refuse_broadcast(kind, format!("not sent: {why}"));
            if kind == BroadcastKind::Beacon {
                self.record_silent_beacon();
            }
            return Err(refusal);
        }
        // A reopen that failed is asked for again, ahead of what is sent now.
        if kind != BroadcastKind::Cq {
            self.reopen_port_if_idle();
        }
        let cmd = match kind {
            BroadcastKind::Cq => ModemCmd::Cq,
            BroadcastKind::Beacon | BroadcastKind::Qsy | BroadcastKind::Message => {
                ModemCmd::Broadcast { kind, payload }
            }
        };
        if !self.task.as_ref().is_some_and(|t| t.send(cmd)) {
            return Err(self.refuse_broadcast(kind, GateRefusal::Detached.message()));
        }
        self.broadcasts.start(kind, tokio::time::Instant::now());
        Ok(())
    }

    /// The beacon these settings make, checked against the listen mode (B7).
    fn beacon_fits(&self, prefs: &BeaconPrefs) -> Result<(), AppError> {
        let Some(p) = self.active_profile() else { return Err(conflict("no station is selected".into())) };
        let left = bytes_left(prefs, &p, self.listen_mode.current().unwrap_or(DATAC3));
        if left < 0 {
            let why = format!("That beacon does not fit the listen mode: {} bytes over", -left);
            return Err(AppError::Validation { errors: chat::field("beacon", why) });
        }
        Ok(())
    }

    fn beacon_content(&self) -> Result<CommandOk, AppError> {
        let Some(p) = self.active_profile() else { return Err(conflict("no station is selected".into())) };
        let prefs = &self.beacon.prefs;
        Ok(CommandOk::BeaconContent(BeaconContentView {
            include_grid: prefs.include_grid,
            name: prefs.name,
            qth: prefs.qth,
            status: prefs.status.clone(),
            bytes_left: bytes_left(prefs, &p, self.listen_mode.current().unwrap_or(DATAC3)),
        }))
    }

    fn beacon_profile(&self) -> Result<String, AppError> {
        self.beacon.profile.clone().ok_or_else(|| conflict("no station is selected".into()))
    }

    /// Arm or disarm beacons for this run, and remember the choice. Arming
    /// needs an idle modem, as listening does (RF-02).
    fn set_beacon(&mut self, on: bool) -> Result<(), AppError> {
        let id = self.beacon_profile()?;
        if on && !matches!(self.modem, ModemState::Ready | ModemState::Listening) {
            let state = self.state_word();
            return Err(conflict(format!("the modem is {state}: beacons can be turned on once it is ready")));
        }
        BeaconPrefs::save_preferred(&mut self.store, &id, on).map_err(storage)?;
        self.beacon.prefs.preferred = on;
        match (on, self.beacon.sched.active()) {
            (true, false) => {
                // Queue the reopen before a possible Beacon now. A healthy
                // connection is kept, including if it has a beacon in flight.
                if self.task.as_ref().is_some_and(|t| t.send(ModemCmd::ReopenBroadcast)) {
                    self.broadcast_silent = false;
                }
                self.beacon.sched.arm(tokio::time::Instant::now());
            }
            (false, _) => self.beacon.sched.disarm(),
            (true, true) => {}
        }
        Ok(())
    }

    fn set_beacon_interval(&mut self, minutes: u32) -> Result<(), AppError> {
        if !(MIN_INTERVAL_MIN..=MAX_INTERVAL_MIN).contains(&minutes) {
            let why = format!("use {MIN_INTERVAL_MIN} to {MAX_INTERVAL_MIN} minutes");
            return Err(AppError::Validation { errors: chat::field("minutes", why) });
        }
        let id = self.beacon_profile()?;
        BeaconPrefs::save_interval(&mut self.store, &id, minutes).map_err(storage)?;
        self.beacon.prefs.interval_min = minutes;
        self.beacon.sched.set_interval(minutes, tokio::time::Instant::now());
        Ok(())
    }

    /// One beacon now, from the active profile, through the broadcast gate.
    fn send_beacon(&mut self) -> Result<(), AppError> {
        // Keying now would hold the QSY's retune back (M4 B7).
        self.qsy_owns_rig()?;
        let Some(p) = self.active_profile() else { return Err(conflict("no station is selected".into())) };
        // Without a known listen mode the gate refuses it anyway; DATAC3 is the one set at attach.
        let mode = self.listen_mode.current().unwrap_or(DATAC3);
        let payload = match self.beacon.payload(&p, mode) {
            Ok(payload) => payload,
            Err(why) => return Err(self.refuse_broadcast(BroadcastKind::Beacon, why)),
        };
        self.send_broadcast(BroadcastKind::Beacon, payload)
    }

    /// The operator's general broadcast to everyone: `CALL: message`.
    fn send_broadcast_message(&mut self, text: &str) -> Result<(), AppError> {
        self.qsy_owns_rig()?;
        let Some(p) = self.active_profile() else { return Err(conflict("no station is selected".into())) };
        let mode = self.listen_mode.current().unwrap_or(DATAC3);
        let payload = match broadcast_text::compose(&p.callsign, text, mode) {
            Ok(payload) => payload,
            Err(why) => return Err(self.refuse_broadcast(BroadcastKind::Message, why)),
        };
        self.send_broadcast(BroadcastKind::Message, payload)?;
        // It is only "sent" once the modem has keyed it: the activity log (and the
        // Broadcasts page) hear of it when it is transmitted.
        self.pending_message = Some(text.trim().to_string());
        Ok(())
    }

    /// The scheduler's deadline passed. Whatever happens, the next beacon is
    /// already an interval away: a refused one is never retried.
    fn beacon_due(&mut self) {
        if self.beacon.sched.tick(tokio::time::Instant::now()) == Tick::Send {
            let _ = self.send_beacon(); // a refusal is already published and logged
        }
    }

    /// Publish the beacon status if it changed.
    fn sync_beacon(&mut self) {
        if let Some(status) = self.beacon.changed(tokio::time::Instant::now()) {
            self.emit(AppEvent::Beacon { status });
        }
    }

    /// Publish the telemetry view if it changed, at most twice a second; a
    /// change held back goes out at the next allowed moment.
    fn sync_telemetry(&mut self) {
        if let Some(telemetry) = self.telemetry.take_event(tokio::time::Instant::now()) {
            self.emit(AppEvent::Telemetry { telemetry });
        }
    }

    fn refuse_broadcast(&mut self, kind: BroadcastKind, reason: String) -> AppError {
        self.publish_broadcasts(vec![(kind, BroadcastOutcome::Refused { reason: reason.clone() })]);
        conflict(reason)
    }

    /// Publish broadcast outcomes, each with its activity line.
    fn publish_broadcasts(&mut self, outcomes: Vec<(BroadcastKind, BroadcastOutcome)>) {
        for (kind, outcome) in outcomes {
            // Only a beacon the modem accepted counts as sent (last_utc, next seq).
            if kind == BroadcastKind::Beacon && outcome == BroadcastOutcome::Submitted {
                self.beacon.submitted();
            }
            let silent = kind == BroadcastKind::Beacon && self.beacon_outcome(&outcome);
            let line = match (kind, &outcome) {
                // The operator's message went out: one row for it, not a generic beacon line too.
                (BroadcastKind::Message, BroadcastOutcome::Transmitted) if self.pending_message.is_some() => {
                    let text = self.pending_message.take().unwrap_or_default();
                    self.discovery.note("message", format!("Message sent: {text}"))
                }
                _ => {
                    // Not confirmed: the message did not (provably) go out, and is not listed as sent.
                    if kind == BroadcastKind::Message && matches!(outcome, BroadcastOutcome::NotConfirmed { .. }) {
                        self.pending_message = None;
                    }
                    self.discovery.note("broadcast", activity_text(kind, &outcome))
                }
            };
            self.emit(AppEvent::Broadcast { kind, outcome });
            self.emit(line);
            if silent {
                self.record_silent_beacon();
            }
            // A silent beacon's port is already reset, and stays closed (above).
            if self.broadcasts.take_unkeyed() && !silent {
                self.reset_unkeyed_port(kind);
            }
        }
    }

    /// Stop a silent beacon from lingering on its TCP connection. This
    /// cannot wait for the modem to return to idle (lab F5).
    fn beacon_outcome(&mut self, outcome: &BroadcastOutcome) -> bool {
        match outcome {
            BroadcastOutcome::Transmitted => self.beacon.sched.on_transmitted(tokio::time::Instant::now()),
            BroadcastOutcome::NotConfirmed { reason: Some(NotConfirmedReason::BroadcastPortSilent) } => {
                if let Some(port) = self.broadcast_abort.take() {
                    port.abort();
                    let line = self.discovery.note(
                        "broadcast",
                        "broadcast port reset after an unconfirmed beacon; turn beacons off and on to reconnect",
                    );
                    self.emit(line);
                }
                self.broadcast_silent = true;
                return true;
            }
            _ => {}
        }
        false
    }

    /// A beacon, QSY suggestion or message the modem never keyed must not
    /// linger on its connection to go out later (lab F5), however its window
    /// ended. The connection is reset and made again; the beacon schedule is
    /// not touched.
    fn reset_unkeyed_port(&mut self, kind: BroadcastKind) {
        let Some(port) = self.broadcast_abort.take() else { return };
        port.abort();
        let line =
            self.discovery.note("broadcast", format!("broadcast port reset after an unconfirmed {}", kind.label()));
        self.emit(line);
        self.reopen_port = true;
        self.reopen_port_if_idle();
    }

    /// Without the port nothing is heard either. The modem task opens it only
    /// while idle, so this is asked again until a report says it is open.
    fn reopen_port_if_idle(&mut self) {
        if self.reopen_port && matches!(self.modem, ModemState::Ready | ModemState::Listening) {
            if let Some(task) = &self.task {
                task.send(ModemCmd::ReopenBroadcast);
            }
        }
    }

    fn record_silent_beacon(&mut self) {
        let was_held = self.beacon.sched.held();
        self.beacon.sched.on_silent();
        if !was_held && self.beacon.sched.held() {
            let why = NotConfirmedReason::BroadcastPortSilent.text();
            let line = self.discovery.note(
                "broadcast",
                format!("beacons held: {why} ({SILENT_HOLD} attempts in a row); turn beacons off and on to try again"),
            );
            self.emit(line);
        }
    }

    fn state_word(&self) -> String {
        format!("{:?}", self.modem).to_lowercase()
    }

    fn command_modem(&self, cmd: ModemCmd) -> Result<(), AppError> {
        if self.task.as_ref().is_some_and(|t| t.send(cmd)) {
            Ok(())
        } else {
            Err(conflict("the modem is not accepting commands".into()))
        }
    }

    /// The profile macros belong to: the active one.
    fn macro_profile(&self) -> Result<String, AppError> {
        self.active_profile().map(|p| p.id).ok_or_else(|| conflict("no station is selected".into()))
    }

    fn mark_macros_seeded(&mut self, profile: &str) -> Result<(), AppError> {
        self.store.set_setting(&macros_seeded_key(profile), "true").map_err(storage)
    }

    /// The active profile's macros; the defaults go in once per profile (a
    /// marker is kept, so deleting them all does not bring them back).
    fn macros_list(&mut self) -> Result<Vec<MacroDef>, AppError> {
        let profile = self.macro_profile()?;
        let mut rows = self.store.macros(&profile).map_err(storage)?;
        if self.store.setting(&macros_seeded_key(&profile)).map_err(storage)?.is_none() {
            if rows.is_empty() {
                for (i, (name, body)) in macros::DEFAULTS.iter().enumerate() {
                    let row = MacroRow {
                        id: uuid::Uuid::new_v4().to_string(),
                        profile_id: profile.clone(),
                        name: (*name).into(),
                        body: (*body).into(),
                        sort: i as i64,
                        quick: false,
                    };
                    self.store.save_macro(&row).map_err(storage)?;
                }
                rows = self.store.macros(&profile).map_err(storage)?;
            }
            self.mark_macros_seeded(&profile)?;
        }
        Ok(rows.into_iter().map(|m| MacroDef { id: m.id, name: m.name, body: m.body, quick: m.quick }).collect())
    }

    fn chat_store(&mut self) -> Result<&mut Store, AppError> {
        self.chat.as_mut().ok_or_else(|| conflict("no station is selected".into()))
    }

    /// Publish message `id`'s current row and return it.
    fn changed(&mut self, peer: &str, id: &[u8; 16]) -> Result<ChatMessage, AppError> {
        let row = self.chat_store()?.message(peer, id).map_err(storage)?;
        let msg = row
            .as_ref()
            .map(ChatMessage::from)
            .ok_or_else(|| AppError::Internal { message: "a message vanished as it was stored".into() })?;
        self.emit(AppEvent::MessageChanged { message: msg.clone() });
        Ok(msg)
    }

    /// Hand a queued message to the modem when a link with its peer may be up.
    /// The task only takes it once the session is negotiated; before that, the
    /// queue is read when HELLO completes.
    fn submit(&self, peer: &str, id: ObjectId) {
        if matches!(self.modem, ModemState::Calling | ModemState::Negotiating | ModemState::Connected) {
            if let Some(t) = &self.task {
                t.send(ModemCmd::Submit { peer: peer.to_string(), id });
            }
        }
    }

    /// Keep the open station's heard records to the retention window.
    fn purge_due(&mut self) {
        self.purge_next = tokio::time::Instant::now() + self.purge_every;
        if let Some(store) = self.chat.as_mut() {
            let purged = self.discovery.purge(store);
            self.emit_all(purged);
        }
    }

    /// Open the selected station's store before its modem task starts, so its
    /// first open (and migration) happens once, here.
    fn open_chat(&mut self, p: &StationProfile) {
        self.chat = None;
        let Some(root) = &self.modem_opts.stations_root else { return };
        let dir = root.join(&p.id);
        let opened = std::fs::create_dir_all(&dir)
            .map_err(|e| e.to_string())
            .and_then(|_| Store::open(&dir.join("station.db")).map_err(|e| e.to_string()));
        match opened {
            Ok(mut s) => {
                let purged = self.discovery.purge(&mut s);
                self.emit_all(purged);
                self.chat = Some(s);
                self.files_reset(&dir);
            }
            Err(e) => tracing::warn!("station store unavailable: {e}"),
        }
    }

    fn set_session(&mut self, session: Option<SessionInfo>) {
        if session.is_none() {
            if let Some(ended) = self.session.clone() {
                self.end_qso(&ended.session_id);
            }
        }
        if self.session != session {
            self.session = session.clone();
            self.emit(AppEvent::Session { session });
        }
    }

    /// The stored active profile, if it still exists and reads cleanly.
    fn active_profile(&self) -> Option<StationProfile> {
        let id = self.store.setting(ACTIVE_PROFILE).ok()??;
        let row = self.store.profiles().ok()?.into_iter().find(|r| r.id == id)?;
        from_row(&row).ok()
    }

    fn set_modem(&mut self, state: ModemState, detail: Option<String>) {
        self.modem = state;
        self.modem_detail = detail.clone();
        // A re-call that is still dialing has found nobody yet: its failure must
        // leave the window open to call again, so only a connection counts for it.
        if state == ModemState::Connected || (state == ModemState::Negotiating && !self.qsy_dialing()) {
            self.qsy_peer_seen();
        }
        // What Mercury does about calls is known only while idle; a call or a
        // session leaves it as it was.
        match state {
            ModemState::Listening => self.mercury_listening = true,
            ModemState::Ready
            | ModemState::Unavailable
            | ModemState::Attaching
            | ModemState::Recovering
            | ModemState::Faulted => self.mercury_listening = false,
            ModemState::Calling | ModemState::Negotiating | ModemState::Connected | ModemState::Draining => {}
        }
        let line = self.discovery.state(state, detail.as_deref());
        self.emit(AppEvent::ModemState { state, detail });
        self.emit(line);
        self.beacon.sched.on_state(state, tokio::time::Instant::now());
        let out = self.broadcasts.state(state);
        self.publish_broadcasts(out);
        self.reopen_port_if_idle();
        if state == ModemState::Faulted {
            // Also re-syncs the gate's live listen mode (ruling P2).
            self.telemetry.modem_faulted(&mut self.listen_mode);
        }
        // The stored choice is cached (it changes only through the paths that
        // re-read it): a state change costs no store read.
        self.show_receive_only(self.receive_only_shown.0);
    }

    /// Stop whatever modem task runs, then start the selected one. Never
    /// waits for the modem: it reports back through `on_report`.
    fn switch_modem(&mut self, target: Option<StationProfile>) {
        // The old task is told to stop; the new one waits for it to let go
        // (ModemOptions::slot).
        self.task = None;
        let abandoned = self.broadcasts.abandon();
        self.publish_broadcasts(abandoned);
        self.broadcast_abort = None;
        self.broadcast_silent = false;
        self.reopen_port = false;
        self.listen_mode = ListenMode::default();
        let telemetry = matches!(&target, Some(p) if matches!(&p.modem, ModemTarget::Existing { telemetry_url: Some(_), .. } | ModemTarget::BuiltIn { .. }));
        self.telemetry.reset(telemetry);
        // Beacons are armed for one run: a new modem or profile starts disarmed (RF-02).
        self.beacon.select(target.as_ref().map(|p| p.id.clone()), &self.store);
        self.set_session(None);
        self.chat = None;
        self.qso = None;
        self.discovery.modem_changed();
        // A new active profile (or none): its stored Receive only choice.
        self.sync_receive_only();
        self.rig_modem_changed();
        // A built-in profile's rigctld is the bundled one on its own port.
        self.rig.builtin = match target.as_ref().map(|p| &p.modem) {
            Some(ModemTarget::BuiltIn { ports }) => Some(rig::BuiltInRig {
                bundle: self.bundle.clone(),
                listen: std::net::SocketAddr::from(([127, 0, 0, 1], ports.rigctld)),
            }),
            _ => None,
        };
        self.builtin_audio = None;
        self.set_builtin(None);
        self.start_rig(target.as_ref().map(|p| p.id.as_str()));
        self.freq_profile_changed(target.as_ref().map(|p| p.id.as_str()));
        let Some(p) = target else { return self.set_modem(ModemState::Unavailable, None) };
        self.files_for_modem(&p.id);
        self.open_chat(&p);
        if p.modem == ModemTarget::Simulator {
            if !cfg!(feature = "sim") {
                // A release build (M5 §5.6): said plainly, not a fault.
                return self.set_modem(ModemState::Unavailable, Some(SIMULATOR_UNAVAILABLE.into()));
            }
            if self.sim_root.is_none() {
                return self.set_modem(ModemState::Faulted, Some("simulator unavailable: no data directory".into()));
            }
        }
        let mut opts = self.modem_opts.clone();
        opts.busy_detect = self.busy_detect();
        if let ModemTarget::BuiltIn { ports } = &p.modem {
            let (Some(bundle), Some(stations)) = (self.bundle.clone(), self.modem_opts.stations_root.clone()) else {
                let why = "the built-in modem is not part of this build".to_string();
                self.set_builtin(Some(ModemDetail::MercuryStopped { reason: why.clone() }));
                return self.set_modem(ModemState::Faulted, Some(why));
            };
            let chosen = self.store.setting(&audio_chosen_key(&p.id)).ok().flatten().is_some();
            let chosen = Arc::new(std::sync::atomic::AtomicBool::new(chosen));
            let rig = crate::app::RigConfig::load(&self.store, &p.id);
            let rigctld =
                matches!(rig.freq_control, crate::app::FreqControl::ManagedRigctld | crate::app::FreqControl::CatFile)
                    .then(|| std::net::SocketAddr::from(([127, 0, 0, 1], ports.rigctld)));
            opts.builtin = Some(crate::app::builtin::BuiltInRun {
                bundle,
                dir: stations.join(&p.id).join("mercury"),
                ptt: crate::app::builtin_mercury_ptt(&rig, ports.rigctld),
                audio_chosen: chosen.clone(),
                rigctld,
            });
            self.builtin_audio = Some(chosen);
        }
        let generation = self.log.generation();
        self.task = Some(modem::spawn(p, generation, opts, self.reports.clone()));
    }

    /// This build's built-in modem: versions now, every file hashed off the actor.
    fn builtin_info(&self, reply: Reply<CommandOk>) {
        let Some(bundle) = self.bundle.clone() else {
            let view =
                super::BuiltInView { available: false, mercury: String::new(), hamlib: String::new(), integrity: None };
            let _ = reply.send(Ok(CommandOk::BuiltIn(view)));
            return;
        };
        tokio::spawn(async move {
            let v = bundle.versions();
            let integrity = match tokio::task::spawn_blocking(move || bundle.verify_all()).await {
                Ok(Ok(())) => None,
                Ok(Err(e)) => Some(e.to_string()),
                Err(e) => Some(format!("the check did not finish: {e}")),
            };
            let view = super::BuiltInView { available: true, mercury: v.mercury, hamlib: v.hamlib, integrity };
            let _ = reply.send(Ok(CommandOk::BuiltIn(view)));
        });
    }

    /// The built-in modem's own state, published when it changes.
    fn set_builtin(&mut self, detail: Option<ModemDetail>) {
        if self.builtin_detail != detail {
            self.builtin_detail = detail.clone();
            self.emit(AppEvent::BuiltIn { detail });
        }
    }

    /// Mercury accepted a sound card: a built-in profile now starts with it
    /// (M5 §5.5), remembered per profile.
    pub(super) fn builtin_audio_chosen(&mut self) {
        let Some(flag) = &self.builtin_audio else { return };
        flag.store(true, std::sync::atomic::Ordering::SeqCst);
        if let Ok(Some(id)) = self.store.setting(ACTIVE_PROFILE) {
            if let Err(e) = self.store.set_setting(&audio_chosen_key(&id), "1") {
                tracing::warn!("could not remember the sound card choice: {e}");
            }
        }
        if self.builtin_detail == Some(ModemDetail::ChooseSoundCard) {
            self.set_builtin(None);
        }
    }

    /// A report from a modem task. One from a task the actor has replaced
    /// (another generation) is stale and dropped (docs/02 section 3).
    fn on_report(&mut self, report: ModemReport) {
        if self.task.as_ref().map(|t| t.generation) != Some(report.generation) {
            tracing::debug!(generation = report.generation, "dropping a stale modem report");
            return;
        }
        match report.kind {
            ReportKind::State { state, detail } => self.set_modem(state, detail),
            ReportKind::Session(session) => {
                let now = tokio::time::Instant::now();
                // A new session id is a new session: one draft each.
                let new_session =
                    session.as_ref().filter(|s| self.session.as_ref().map(|c| &c.session_id) != Some(&s.session_id));
                if let Some(s) = new_session {
                    let tuning = self.tuning(now);
                    // Before the session itself is recorded as the latest heard row (which has no grid).
                    let grid = self.heard_grid(&s.peer);
                    let heard = self.discovery.session(self.chat.as_mut(), &s.peer, &tuning);
                    self.emit_all(heard);
                    self.telemetry.session_started(now);
                    self.info_session_started();
                    let qsy_from = self.qsy_session_started(&s.peer);
                    self.begin_qso(s, grid, &tuning, qsy_from);
                }
                if session.is_none() {
                    self.telemetry.session_ended();
                    self.qsy_session_ended();
                    self.files_session_ended();
                }
                self.set_session(session)
            }
            ReportKind::CallFailed { peer, reason } => {
                self.qsy_call_failed();
                self.emit(AppEvent::CallFailed { peer, reason })
            }
            ReportKind::Message(message) => {
                self.telemetry.on_message(&message);
                self.emit(AppEvent::MessageChanged { message })
            }
            ReportKind::Observation(o) => {
                let now = tokio::time::Instant::now();
                match &o {
                    Observation::Busy(on) => self.beacon.on_busy(*on, now),
                    Observation::Snr(snr) => self.telemetry.on_control_snr(*snr),
                    Observation::Buffer(n) => self.telemetry.on_buffer(*n),
                    Observation::Ptt(on) => self.on_mercury_ptt(*on),
                    _ => {}
                }
                let out = self.broadcasts.observe(&o);
                self.publish_broadcasts(out);
                let tuning = self.tuning(now);
                let events = self.discovery.observation(self.chat.as_mut(), o, &tuning);
                self.emit_all(events);
            }
            ReportKind::Frame(f) => {
                self.qsy_frame(&f.payload);
                let tuning = self.tuning(tokio::time::Instant::now());
                let events = self.discovery.frame(self.chat.as_mut(), f, &tuning);
                self.emit_all(events);
            }
            ReportKind::TelemetryCmds(tx) => self.rig_telemetry(tx),
            ReportKind::BuiltIn(detail) => self.set_builtin(detail),
            ReportKind::Tune(result) => self.tune_report(result),
            ReportKind::Peer(m) => self.on_peer(m),
            ReportKind::File(ev) => self.on_file(ev),
            ReportKind::SimRig(addr) => self.rig_sim(addr),
            ReportKind::Telemetry(msg) => {
                self.rig_telemetry_msg(&msg);
                self.audio_telemetry_msg(&msg);
                let line = self.telemetry.on_msg(msg, tokio::time::Instant::now());
                // Ruling P2: the gate prefers telemetry's live listen mode.
                self.listen_mode.set_live(self.telemetry.live_listen_mode());
                self.observe_modes();
                if let Some(text) = line {
                    let line = self.discovery.note("telemetry", text);
                    self.emit(line);
                }
            }
            ReportKind::BroadcastOpened(abort) => {
                self.broadcast_abort = Some(abort);
                self.reopen_port = false;
                self.broadcast_silent = false;
            }
            ReportKind::Broadcast(why) => {
                let line = self.discovery.note("broadcast", why);
                self.emit(line);
            }
            ReportKind::ListenMode(mode) => {
                self.listen_mode.set_attach(mode);
                if mode.is_none() {
                    let why = "the modem did not accept listen mode DATAC3: beacons are not sent; CQ still works";
                    let line = self.discovery.note("broadcast", why);
                    self.emit(line);
                }
            }
            ReportKind::BroadcastSubmitted(kind) => {
                let out = self.broadcasts.submitted(kind);
                self.publish_broadcasts(out);
            }
            ReportKind::BroadcastRefused { kind, reason } => {
                let out = self.broadcasts.refused(kind, reason);
                self.publish_broadcasts(out);
            }
            ReportKind::BroadcastUnconfirmed { kind, note } => {
                let out = self.broadcasts.unconfirmed(kind);
                if !out.is_empty() {
                    self.publish_broadcasts(out);
                    let line = self.discovery.note("broadcast", note);
                    self.emit(line);
                }
            }
        }
    }

    /// The locator the heard list holds for `peer`, if any.
    fn heard_grid(&self, peer: &str) -> Option<String> {
        let store = self.chat.as_ref()?;
        discovery::heard_station(store, peer, self.discovery.offsets()).ok()?.grid
    }

    /// Draft the QSO record of a session that just started.
    /// `qsy_from`: the first session's QSO, when this is a QSY's re-call.
    fn begin_qso(&mut self, s: &SessionInfo, grid: Option<String>, tuning: &Tuning, qsy_from: Option<String>) {
        let Some(store) = self.chat.as_mut() else { return };
        let mut row = logbook::draft_row(&Start {
            session_id: &s.session_id,
            peer: &s.peer,
            start_utc: &s.since_utc,
            bandwidth: &s.bandwidth,
            grid,
            tuning,
        });
        let view = self.telemetry.view(tokio::time::Instant::now());
        row.tx_modes = view.tx_mode.as_deref().and_then(|m| logbook::with_mode(None, m));
        row.rx_modes = view.rx_mode.as_deref().and_then(|m| logbook::with_mode(None, m));
        row.notes = qsy_from.as_deref().map(logbook::qsy_note);
        if let Err(e) = store.create_qso(&row) {
            tracing::warn!("could not draft the QSO record: {e}");
            return;
        }
        self.qso = Some(row.clone());
        self.emit(AppEvent::Qso { qso: Qso::from(&row) });
    }

    /// Stamp the end of a session's record, and finalise it if asked to.
    fn end_qso(&mut self, session_id: &str) {
        let finalise = self.finalise_on_disconnect();
        let Some(store) = self.chat.as_mut() else { return };
        let ended = store.end_qso_for_session(session_id, &mc_storage::now_utc()).and_then(|row| match row {
            Some(r) if finalise => store.finalise_qso(&r.id).map(Some),
            other => Ok(other),
        });
        match ended {
            Ok(Some(row)) => {
                self.qso_changed(row);
            }
            Ok(None) => {}
            Err(e) => tracing::warn!("could not end the QSO record: {e}"),
        }
    }

    /// Add Mercury's newly observed ARQ modes to the session's draft, each once.
    fn observe_modes(&mut self) {
        let Some(session) = self.session.as_ref() else { return };
        let Some(row) =
            self.qso.as_ref().filter(|r| r.state == "draft" && r.session_id.as_deref() == Some(&session.session_id))
        else {
            return;
        };
        let view = self.telemetry.view(tokio::time::Instant::now());
        let tx_modes = view.tx_mode.as_deref().and_then(|m| logbook::with_mode(row.tx_modes.as_deref(), m));
        let rx_modes = view.rx_mode.as_deref().and_then(|m| logbook::with_mode(row.rx_modes.as_deref(), m));
        if tx_modes.is_none() && rx_modes.is_none() {
            return;
        }
        let id = row.id.clone();
        let Some(store) = self.chat.as_mut() else { return };
        match store.update_qso(&id, &RowEdit { tx_modes, rx_modes, ..RowEdit::default() }) {
            Ok(row) => {
                self.qso_changed(row);
            }
            Err(e) => tracing::warn!("could not record the observed modes: {e}"),
        }
    }

    /// Publish a changed record, and keep it as the current one if it is.
    fn qso_changed(&mut self, row: QsoRow) -> Qso {
        let qso = Qso::from(&row);
        if self.qso.as_ref().is_some_and(|c| c.id == row.id) {
            self.qso = Some(row);
        }
        self.emit(AppEvent::Qso { qso: qso.clone() });
        qso
    }

    /// "Listen before transmit": on unless the operator turned it off.
    fn busy_detect(&self) -> bool {
        self.store.setting(BUSY_DETECT_SETTING).ok().flatten().is_none_or(|v| v != "false")
    }

    /// The stored ADIF mode, or DYNAMIC (a stored value that no longer checks counts as none).
    fn adif_mode(&self) -> String {
        self.store
            .setting(super::export::ADIF_MODE_SETTING)
            .ok()
            .flatten()
            .and_then(|m| super::export::adif_mode(&m).ok())
            .unwrap_or_else(|| super::export::DEFAULT_ADIF_MODE.to_string())
    }

    fn finalise_on_disconnect(&self) -> bool {
        matches!(self.store.setting(FINALISE_SETTING), Ok(Some(v)) if v == "true")
    }

    fn ui_language(&self) -> String {
        // A stored value outside the list (an older or edited database) reads as the default.
        match self.store.setting(UI_LANGUAGE_SETTING) {
            Ok(Some(v)) if UI_LANGUAGES.contains(&v.as_str()) => v,
            _ => UI_LANGUAGES[0].to_string(),
        }
    }

    fn snapshot(&mut self) -> Result<AppSnapshot, AppError> {
        let mut profiles = Vec::new();
        for row in self.store.profiles().map_err(storage)? {
            match from_row(&row) {
                Ok(p) => profiles.push(p),
                Err(e) => tracing::warn!(id = %row.id, "skipping unreadable profile: {}", e.message),
            }
        }
        // A setting naming a profile that no longer exists reads as none.
        let active =
            self.store.setting(ACTIVE_PROFILE).map_err(storage)?.filter(|id| profiles.iter().any(|p| &p.id == id));
        let receive_only =
            active.as_ref().and_then(|id| profiles.iter().find(|p| &p.id == id)).is_some_and(|p| p.receive_only);
        Ok(AppSnapshot {
            seq: self.log.last_seq(),
            generation: self.log.generation(),
            modem: self.modem,
            modem_detail: self.modem_detail.clone(),
            builtin_detail: self.builtin_detail.clone(),
            profiles,
            receive_only,
            receive_only_pending: self.receive_only_pending(receive_only),
            listening: self.modem == ModemState::Listening,
            beacon: self.beacon.status(tokio::time::Instant::now()),
            session: self.session.clone(),
            active_profile_id: active,
            discovery: self.discovery.stats(),
            telemetry: Box::new(self.telemetry.view(tokio::time::Instant::now())),
            current_qso: self.qso.as_ref().map(|r| Box::new(Qso::from(r))),
            finalise_on_disconnect: self.finalise_on_disconnect(),
            adif_mode: self.adif_mode(),
            busy_detect: self.busy_detect(),
            ui_language: self.ui_language(),
            notify_prefs: NotifyPrefs::from_setting(self.store.setting(NOTIFY_SETTING).ok().flatten().as_deref()),
            rig: Box::new(self.rig.policy.view(tokio::time::Instant::now())),
            tune: self.rig.tune.view(),
            schedule_next: self.schedule_next(),
            qsy: self.qsy_view(tokio::time::Instant::now()),
        })
    }
}
