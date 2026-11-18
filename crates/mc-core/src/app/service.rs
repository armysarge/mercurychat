//! The application service actor (docs/02 §3). It alone owns the store and
//! the event log. UI commands arrive on a bounded queue; a full queue is
//! refused at once rather than making the UI wait. Every state change is
//! persisted first, then published as an event.

use super::beacon::{BeaconPrefs, BeaconState, Tick, MAX_INTERVAL_MIN, MIN_INTERVAL_MIN, SILENT_HOLD};
use super::broadcast::{
    activity_text, broadcast_gate, BroadcastKind, BroadcastOutcome, GateRefusal, ListenMode, NotConfirmedReason,
    Tracker,
};
use super::chat::{self, ChatMessage, Conversation};
use super::contract::{AppCommand, AppError, AppSnapshot, CommandOk};
use super::discovery::{self, Discovery};
use super::events::{AppEvent, EventEnvelope, EventLog, ModemState, SessionInfo, Since};
use super::logbook::{self, Qso, Start, FINALISE_SETTING};
use super::modem::{self, ModemCmd, ModemHandle, ModemOptions, ModemReport, ReportKind};
use super::notify::{NotifyPrefs, NOTIFY_SETTING};
use super::telemetry::{Telemetry, Tuning};
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

mod rig;
use rig::RigSide;

const ACTIVE_PROFILE: &str = "activeProfileId";
/// The setting holding the interface language (`system` when absent).
pub const UI_LANGUAGE_SETTING: &str = "ui.language";
/// The values `ui.language` may hold. The UI maps `system` to the OS language.
pub const UI_LANGUAGES: [&str; 5] = ["system", "en", "es", "pt-BR", "fr"];
/// Longest history search text, in characters.
const SEARCH_MAX: usize = 128;

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
        }
    }
}

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
        session: None,
        chat: None,
        sim_root: opts.sim_root,
        modem_opts: ModemOptions {
            stations_root,
            locks_dir: opts.locks_dir,
            radio_hello_timeout: opts.radio_hello_timeout,
            slot: Default::default(),
            spectrum: Default::default(),
            sim_peer_broadcasts: opts.sim_peer_broadcasts,
        },
        reports,
        task: None,
        pending: None,
        discovery: Discovery::default(),
        listen_mode: ListenMode::default(),
        broadcasts: Tracker::default(),
        broadcast_abort: None,
        broadcast_silent: false,
        beacon: BeaconState::new(),
        telemetry: Telemetry::default(),
        qso: None,
        mercury_listening: false,
        receive_only_shown: (false, false),
        rig: RigSide::default(),
    };
    let spectrum = app.modem_opts.spectrum.0.subscribe();
    tokio::spawn(async move {
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
            tokio::select! {
                req = rx.recv() => {
                    let Some(req) = req else { break };
                    match req {
                        Request::Command(cmd, reply) => {
                            let _ = reply.send(app.handle(*cmd));
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
            }
            app.sync_beacon();
            app.sync_telemetry();
            app.sync_rig();
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
    broadcast_abort: Option<BroadcastAbort>,
    /// The closed port cannot take another beacon until an explicit retry or recovery.
    broadcast_silent: bool,
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
}

fn parse_peer(peer: &str) -> Result<String, AppError> {
    Callsign::parse(peer)
        .map(|c| c.as_str().to_string())
        .map_err(|e| AppError::Validation { errors: chat::field("peer", e.to_string()) })
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
                BeaconPrefs::save_include_grid(&mut self.store, &id, include_grid).map_err(storage)?;
                self.beacon.prefs.include_grid = include_grid;
                Ok(CommandOk::Done)
            }
            AppCommand::SendBeaconNow => self.send_beacon().map(|_| CommandOk::Done),
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
                };
                self.store.save_macro(&row).map_err(storage)?;
                Ok(CommandOk::Macro(MacroDef { id, name: def.name, body: def.body }))
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
        let len = (kind == BroadcastKind::Beacon).then_some(payload.len());
        let calling = self.modem == ModemState::Calling;
        if let Err(g) = broadcast_gate(self.modem, calling, len, self.listen_mode.current()) {
            return Err(self.refuse_broadcast(kind, g.message()));
        }
        if self.broadcasts.busy() {
            return Err(self.refuse_broadcast(kind, "not sent: a broadcast is still being sent".into()));
        }
        if kind == BroadcastKind::Beacon && self.broadcast_silent {
            let why = NotConfirmedReason::BroadcastPortSilent.text();
            let refusal = self.refuse_broadcast(kind, format!("not sent: {why}"));
            self.record_silent_beacon();
            return Err(refusal);
        }
        let cmd = match kind {
            BroadcastKind::Cq => ModemCmd::Cq,
            BroadcastKind::Beacon => ModemCmd::Broadcast(payload),
        };
        if !self.task.as_ref().is_some_and(|t| t.send(cmd)) {
            return Err(self.refuse_broadcast(kind, GateRefusal::Detached.message()));
        }
        self.broadcasts.start(kind, tokio::time::Instant::now());
        Ok(())
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
        let Some(p) = self.active_profile() else { return Err(conflict("no station is selected".into())) };
        let payload = match self.beacon.payload(&p.callsign, p.grid.as_deref()) {
            Ok(payload) => payload,
            Err(why) => return Err(self.refuse_broadcast(BroadcastKind::Beacon, why)),
        };
        self.send_broadcast(BroadcastKind::Beacon, payload)
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
            let line = self.discovery.note("broadcast", activity_text(kind, &outcome));
            self.emit(AppEvent::Broadcast { kind, outcome });
            self.emit(line);
            if silent {
                self.record_silent_beacon();
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
                    };
                    self.store.save_macro(&row).map_err(storage)?;
                }
                rows = self.store.macros(&profile).map_err(storage)?;
            }
            self.mark_macros_seeded(&profile)?;
        }
        Ok(rows.into_iter().map(|m| MacroDef { id: m.id, name: m.name, body: m.body }).collect())
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
        self.listen_mode = ListenMode::default();
        let telemetry =
            matches!(&target, Some(p) if matches!(&p.modem, ModemTarget::Existing { telemetry_url: Some(_), .. }));
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
        self.start_rig(target.as_ref().map(|p| p.id.as_str()));
        let Some(p) = target else { return self.set_modem(ModemState::Unavailable, None) };
        self.open_chat(&p);
        if p.modem == ModemTarget::Simulator {
            if !cfg!(feature = "sim") {
                return self.set_modem(ModemState::Faulted, Some("the simulator is not part of this build".into()));
            }
            if self.sim_root.is_none() {
                return self.set_modem(ModemState::Faulted, Some("simulator unavailable: no data directory".into()));
            }
        }
        let generation = self.log.generation();
        self.task = Some(modem::spawn(p, generation, self.modem_opts.clone(), self.reports.clone()));
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
                    self.begin_qso(s, grid, &tuning);
                }
                if session.is_none() {
                    self.telemetry.session_ended();
                }
                self.set_session(session)
            }
            ReportKind::CallFailed { peer, reason } => self.emit(AppEvent::CallFailed { peer, reason }),
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
                let tuning = self.tuning(tokio::time::Instant::now());
                let events = self.discovery.frame(self.chat.as_mut(), f, &tuning);
                self.emit_all(events);
            }
            ReportKind::TelemetryCmds(tx) => self.rig_telemetry(tx),
            ReportKind::Telemetry(msg) => {
                self.rig_telemetry_msg(&msg);
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
    fn begin_qso(&mut self, s: &SessionInfo, grid: Option<String>, tuning: &Tuning) {
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
            ui_language: self.ui_language(),
            notify_prefs: NotifyPrefs::from_setting(self.store.setting(NOTIFY_SETTING).ok().flatten().as_deref()),
            rig: Box::new(self.rig.policy.view(tokio::time::Instant::now())),
        })
    }
}
