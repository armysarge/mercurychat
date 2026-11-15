//! The application service actor (docs/02 §3). It alone owns the store and
//! the event log. UI commands arrive on a bounded queue; a full queue is
//! refused at once rather than making the UI wait. Every state change is
//! persisted first, then published as an event.

use super::contract::{AppCommand, AppError, AppSnapshot, CommandOk};
use super::events::{AppEvent, EventEnvelope, EventLog, ModemState, SessionInfo, Since};
use super::modem::{self, ModemCmd, ModemHandle, ModemOptions, ModemReport, ReportKind};
use crate::profile::{from_row, to_row, FieldError, ModemTarget, StationProfile};
use mc_storage::{StorageError, Store};
use mercury_adapter::Callsign;
use std::path::PathBuf;
use tokio::sync::{broadcast, mpsc, oneshot};

const ACTIVE_PROFILE: &str = "activeProfileId";

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
}

impl Default for AppOptions {
    fn default() -> Self {
        AppOptions { command_queue: 64, event_ring: 1024, sim_root: None, stations_root: None, locks_dir: None }
    }
}

type Reply<T> = oneshot::Sender<Result<T, AppError>>;

enum Request {
    Command(AppCommand, Reply<CommandOk>),
    Since(u64, Reply<Since>),
}

#[derive(Clone)]
pub struct AppHandle {
    tx: mpsc::Sender<Request>,
    events: broadcast::Sender<EventEnvelope>,
}

fn stopped() -> AppError {
    AppError::Internal { message: "the application service has stopped".into() }
}

impl AppHandle {
    pub async fn call(&self, cmd: AppCommand) -> Result<CommandOk, AppError> {
        let (reply, rx) = oneshot::channel();
        self.send(Request::Command(cmd, reply))?;
        rx.await.map_err(|_| stopped())?
    }

    /// Events after `seq`, or `SnapshotNeeded` (docs/02 §3).
    pub async fn events_since(&self, seq: u64) -> Result<Since, AppError> {
        let (reply, rx) = oneshot::channel();
        self.send(Request::Since(seq, reply))?;
        rx.await.map_err(|_| stopped())?
    }

    pub fn subscribe(&self) -> broadcast::Receiver<EventEnvelope> {
        self.events.subscribe()
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
        sim_root: opts.sim_root,
        modem_opts: ModemOptions { stations_root, locks_dir: opts.locks_dir },
        reports,
        task: None,
        pending: None,
    };
    tokio::spawn(async move {
        // The selection survives a restart, so its modem is attached again (attach
        // only: a stored listen preference never arms transmission, RF-02).
        if let Some(p) = app.active_profile() {
            app.switch_modem(Some(p));
        }
        loop {
            tokio::select! {
                req = rx.recv() => {
                    let Some(req) = req else { break };
                    match req {
                        Request::Command(cmd, reply) => {
                            let _ = reply.send(app.handle(cmd));
                            // A modem change is answered at once, then started.
                            if let Some(target) = app.pending.take() {
                                app.switch_modem(target);
                            }
                        }
                        Request::Since(seq, reply) => {
                            let _ = reply.send(Ok(app.log.since(seq)));
                        }
                    }
                }
                Some(report) = report_rx.recv() => app.on_report(report),
            }
        }
    });
    AppHandle { tx, events }
}

struct App {
    store: Store,
    log: EventLog,
    events: broadcast::Sender<EventEnvelope>,
    modem: ModemState,
    modem_detail: Option<String>,
    /// The negotiated session, as the modem task last reported it.
    session: Option<SessionInfo>,
    sim_root: Option<PathBuf>,
    modem_opts: ModemOptions,
    /// Where modem tasks report; the actor keeps one sender so it never closes.
    reports: mpsc::Sender<ModemReport>,
    /// The running modem task, if any. Dropping it stops the task.
    task: Option<ModemHandle>,
    /// The modem the last selection asked for, started after replying.
    pending: Option<Option<StationProfile>>,
}

fn conflict(message: String) -> AppError {
    AppError::Conflict { message }
}

fn storage(e: StorageError) -> AppError {
    AppError::Storage { message: e.to_string() }
}

impl App {
    fn emit(&mut self, event: AppEvent) {
        let env = self.log.push(event);
        let _ = self.events.send(env); // no subscriber is fine
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
                let reattach = self
                    .active_profile()
                    .is_some_and(|old| old.id == p.id && (old.callsign != p.callsign || old.modem != p.modem));
                self.store.save_profile(&to_row(&p)).map_err(storage)?;
                self.emit(AppEvent::ProfilesChanged);
                if reattach {
                    self.log.bump_generation();
                    self.pending = Some(Some(p.clone()));
                }
                Ok(CommandOk::Profile(p))
            }
            AppCommand::DeleteProfile { id } => {
                if !self.store.delete_profile(&id).map_err(storage)? {
                    return Err(AppError::NotFound { id });
                }
                self.emit(AppEvent::ProfilesChanged);
                if self.store.setting(ACTIVE_PROFILE).map_err(storage)?.as_deref() == Some(id.as_str()) {
                    self.store.delete_setting(ACTIVE_PROFILE).map_err(storage)?;
                    self.log.bump_generation();
                    self.emit(AppEvent::ActiveProfileChanged { id: None });
                    self.pending = Some(None);
                }
                Ok(CommandOk::Done)
            }
            AppCommand::SetListen { on } => {
                if !matches!(self.modem, ModemState::Ready | ModemState::Listening) {
                    let state = format!("{:?}", self.modem).to_lowercase();
                    return Err(conflict(format!("the modem is {state}: listening can be changed once it is ready")));
                }
                let Some(mut p) = self.active_profile() else { return Err(conflict("no station is selected".into())) };
                p.listen_preferred = on;
                self.store.save_profile(&to_row(&p)).map_err(storage)?;
                self.emit(AppEvent::ProfilesChanged);
                if !self.task.as_ref().is_some_and(|t| t.send(ModemCmd::SetListen(on))) {
                    return Err(conflict("the modem is not accepting commands".into()));
                }
                Ok(CommandOk::Done)
            }
            AppCommand::Call { peer } => {
                let peer = Callsign::parse(&peer).map_err(|e| AppError::Validation {
                    errors: vec![FieldError { field: "peer".into(), message: e.to_string() }],
                })?;
                if self.session.is_some() || self.in_call() {
                    return Err(conflict("a call or session is already in progress".into()));
                }
                if !matches!(self.modem, ModemState::Ready | ModemState::Listening) {
                    return Err(conflict(format!(
                        "the modem is {}: calls can be made once it is ready",
                        self.state_word()
                    )));
                }
                self.command_modem(ModemCmd::Call(peer.clone()))?;
                // Answered at once: a second call is refused from here on.
                self.set_modem(ModemState::Calling, Some(peer.as_str().to_string()));
                Ok(CommandOk::Done)
            }
            AppCommand::CancelCall => {
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
                if !self.in_call() {
                    return Err(conflict("there is no call or session to abort".into()));
                }
                self.command_modem(ModemCmd::Abort).map(|_| CommandOk::Done)
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
        }
    }

    /// A call is being placed, or a session is up or ending.
    fn in_call(&self) -> bool {
        matches!(
            self.modem,
            ModemState::Calling | ModemState::Negotiating | ModemState::Connected | ModemState::Draining
        )
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

    fn set_session(&mut self, session: Option<SessionInfo>) {
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
        self.emit(AppEvent::ModemState { state, detail });
    }

    /// Stop whatever modem task runs, then start the selected one. Never
    /// waits for the modem: it reports back through `on_report`.
    fn switch_modem(&mut self, target: Option<StationProfile>) {
        self.task = None; // its Drop tells the old task to stop
        self.set_session(None);
        let Some(p) = target else { return self.set_modem(ModemState::Unavailable, None) };
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
            ReportKind::Session(session) => self.set_session(session),
            ReportKind::CallFailed { peer, reason } => self.emit(AppEvent::CallFailed { peer, reason }),
        }
    }

    fn snapshot(&self) -> Result<AppSnapshot, AppError> {
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
        let listen_preferred =
            active.as_ref().and_then(|id| profiles.iter().find(|p| &p.id == id)).is_some_and(|p| p.listen_preferred);
        Ok(AppSnapshot {
            seq: self.log.last_seq(),
            generation: self.log.generation(),
            modem: self.modem,
            modem_detail: self.modem_detail.clone(),
            profiles,
            listen_preferred,
            session: self.session.clone(),
            active_profile_id: active,
        })
    }
}
