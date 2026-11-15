//! The application service actor (docs/02 §3). It alone owns the store and
//! the event log. UI commands arrive on a bounded queue; a full queue is
//! refused at once rather than making the UI wait. Every state change is
//! persisted first, then published as an event.

use super::contract::{AppCommand, AppError, AppSnapshot, CommandOk};
use super::events::{AppEvent, EventEnvelope, EventLog, ModemState, Since};
use crate::profile::{from_row, to_row};
use mc_storage::{StorageError, Store};
use tokio::sync::{broadcast, mpsc, oneshot};

const ACTIVE_PROFILE: &str = "activeProfileId";

#[derive(Debug, Clone, Copy)]
pub struct AppOptions {
    pub command_queue: usize,
    pub event_ring: usize,
}

impl Default for AppOptions {
    fn default() -> Self {
        AppOptions { command_queue: 64, event_ring: 1024 }
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
    let mut app =
        App { store, log: EventLog::new(opts.event_ring), events: events.clone(), modem: ModemState::Unavailable };
    tokio::spawn(async move {
        while let Some(req) = rx.recv().await {
            match req {
                Request::Command(cmd, reply) => {
                    let _ = reply.send(app.handle(cmd));
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
}

fn storage(e: StorageError) -> AppError {
    AppError::Storage { message: e.to_string() }
}

impl App {
    fn emit(&mut self, event: AppEvent) {
        let env = self.log.push(event);
        let _ = self.events.send(env); // no subscriber is fine
    }

    fn exists(&self, id: &str) -> Result<bool, AppError> {
        Ok(self.store.profiles().map_err(storage)?.iter().any(|r| r.id == id))
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
                    self.emit(AppEvent::ActiveProfileChanged { id: None });
                }
                Ok(CommandOk::Done)
            }
            AppCommand::SelectProfile { id } => {
                match &id {
                    Some(i) if !self.exists(i)? => return Err(AppError::NotFound { id: i.clone() }),
                    Some(i) => self.store.set_setting(ACTIVE_PROFILE, i).map_err(storage)?,
                    None => self.store.delete_setting(ACTIVE_PROFILE).map_err(storage)?,
                }
                self.emit(AppEvent::ActiveProfileChanged { id });
                Ok(CommandOk::Done)
            }
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
        Ok(AppSnapshot {
            seq: self.log.last_seq(),
            generation: self.log.generation(),
            modem: self.modem,
            profiles,
            active_profile_id: active,
        })
    }
}
