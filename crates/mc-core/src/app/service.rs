//! The application service actor (docs/02 §3). It alone owns the store and
//! the event log. UI commands arrive on a bounded queue; a full queue is
//! refused at once rather than making the UI wait. Every state change is
//! persisted first, then published as an event.

use super::contract::{AppCommand, AppError, AppSnapshot, CommandOk};
use super::events::{AppEvent, EventEnvelope, EventLog, ModemState, Since};
use crate::profile::{from_row, to_row, ModemTarget, StationProfile};
use mc_storage::{StorageError, Store};
use std::path::PathBuf;
use tokio::sync::{broadcast, mpsc, oneshot};

const ACTIVE_PROFILE: &str = "activeProfileId";

#[derive(Debug, Clone)]
pub struct AppOptions {
    pub command_queue: usize,
    pub event_ring: usize,
    /// Where the simulator keeps its two stations' data; `None` disables it.
    pub sim_root: Option<PathBuf>,
}

impl Default for AppOptions {
    fn default() -> Self {
        AppOptions { command_queue: 64, event_ring: 1024, sim_root: None }
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
    let mut app = App {
        store,
        log: EventLog::new(opts.event_ring),
        events: events.clone(),
        modem: ModemState::Unavailable,
        sim_root: opts.sim_root,
        pending: None,
        #[cfg(feature = "sim")]
        sim: None,
    };
    tokio::spawn(async move {
        while let Some(req) = rx.recv().await {
            match req {
                Request::Command(cmd, reply) => {
                    let _ = reply.send(app.handle(cmd));
                    // A modem change is answered at once, then carried out.
                    if let Some(target) = app.pending.take() {
                        app.switch_modem(target).await;
                    }
                }
                Request::Since(seq, reply) => {
                    let _ = reply.send(Ok(app.log.since(seq)));
                }
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
    sim_root: Option<PathBuf>,
    /// The modem the last selection asked for, attached after replying.
    pending: Option<Option<StationProfile>>,
    #[cfg(feature = "sim")]
    sim: Option<crate::sim::SimModem>,
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
                self.store.save_profile(&to_row(&p)).map_err(storage)?;
                self.emit(AppEvent::ProfilesChanged);
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

    fn set_modem(&mut self, state: ModemState, detail: Option<String>) {
        self.modem = state;
        self.emit(AppEvent::ModemState { state, detail });
    }

    /// Detach whatever modem is attached, then attach the selected one.
    async fn switch_modem(&mut self, target: Option<StationProfile>) {
        #[cfg(feature = "sim")]
        {
            self.sim = None; // drops both stations, closing their sockets and locks
        }
        match target.map(|p| (p.modem.clone(), p)) {
            None => self.set_modem(ModemState::Unavailable, None),
            Some((ModemTarget::Existing { .. }, _)) => {
                // Attaching a real Mercury is M2 work; M1 only stores the endpoints.
                self.set_modem(ModemState::Unavailable, Some("existing modems are attached from M2 on".into()))
            }
            Some((ModemTarget::Simulator, p)) => self.start_sim(&p).await,
        }
    }

    #[cfg(feature = "sim")]
    async fn start_sim(&mut self, p: &StationProfile) {
        let Some(root) = self.sim_root.clone() else {
            return self.set_modem(ModemState::Faulted, Some("simulator unavailable: no data directory".into()));
        };
        self.set_modem(ModemState::Attaching, None);
        match crate::sim::SimModem::start(&root, &p.callsign).await {
            Ok(sim) => {
                self.sim = Some(sim);
                self.set_modem(ModemState::Ready, Some("simulator".into()));
            }
            Err(e) => self.set_modem(ModemState::Faulted, Some(format!("simulator: {e}"))),
        }
    }

    #[cfg(not(feature = "sim"))]
    async fn start_sim(&mut self, _p: &StationProfile) {
        let _ = &self.sim_root;
        self.set_modem(ModemState::Faulted, Some("the simulator is not part of this build".into()));
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
        Ok(AppSnapshot {
            seq: self.log.last_seq(),
            generation: self.log.generation(),
            modem: self.modem,
            profiles,
            active_profile_id: active,
        })
    }
}
