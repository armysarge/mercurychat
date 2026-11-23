//! The actor's side of files (M4): preparing, sending, answering offers,
//! the transfers list and its events, the file settings, and clearing what
//! a crash or an unsent prepare left behind.

use super::{conflict, storage, App, Reply};
use crate::app::contract::{AppError, CommandOk};
use crate::app::events::AppEvent;
use crate::app::files::{airtime_s, FileSettings, PreparedView, TransferView, OFFER_WINDOW_S};
use crate::app::modem::{FileOptions, ModemCmd};
use crate::files::pace::nominal_rate;
use crate::files::{prepare, PhotoSize, PrepareError, Prepared};
use crate::session::{SessionCommand, SessionEvent, MAX_FILE_CEILING};
use mc_storage::{Direction, TransferRow, TransferState};
use mch_protocol::ObjectId;
use mercury_adapter::Bandwidth;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::time::Instant;

/// Progress events per transfer, at most this often (the last one always).
const PROGRESS_EVERY: Duration = Duration::from_millis(500);
/// A rate is "measured" once progress has run this long.
const MEASURE_AFTER: Duration = Duration::from_secs(2);

#[derive(Debug, Default)]
pub(in crate::app) struct FilesSide {
    /// Prepared, not yet sent: shared with the prepare tasks.
    prepared: Arc<Mutex<HashMap<String, Prepared>>>,
    progress: HashMap<[u8; 16], Progress>,
    /// Incoming offers awaiting an answer: their deadline, and whether the
    /// operator has extended it.
    offers: HashMap<[u8; 16], (Instant, bool)>,
    /// The last measured transfer rate this session, bytes a second.
    measured_bps: Option<f64>,
    /// Transfers of this session not yet seen to end: at its end, each is
    /// published once more (the engine marks them interrupted silently).
    live: std::collections::HashSet<[u8; 16]>,
}

#[derive(Debug)]
struct Progress {
    first: (Instant, u64),
    bytes: u64,
    emitted: Option<Instant>,
}

fn refused(reason: &'static str, message: impl Into<String>) -> AppError {
    AppError::Refused { reason, message: message.into() }
}

fn parse_id(id: &str) -> Result<[u8; 16], AppError> {
    hex::decode(id)
        .ok()
        .and_then(|v| <[u8; 16]>::try_from(v).ok())
        .ok_or_else(|| AppError::NotFound { id: id.to_string() })
}

pub(super) fn options(s: &FileSettings) -> FileOptions {
    FileOptions { max_file: Some(s.max_incoming), receive_dir: s.receive_dir.as_ref().map(PathBuf::from) }
}

/// Everything in `dir` goes (the folder stays).
fn empty_dir(dir: &Path) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let _ = if p.is_dir() { std::fs::remove_dir_all(&p) } else { std::fs::remove_file(&p) };
    }
}

fn bandwidth(token: &str) -> Bandwidth {
    match token {
        "500" | "BW500" => Bandwidth::Bw500,
        "2750" | "BW2750" => Bandwidth::Bw2750,
        _ => Bandwidth::Bw2300,
    }
}

impl App {
    fn station_dir(&self) -> Option<PathBuf> {
        let root = self.modem_opts.stations_root.as_ref()?;
        Some(root.join(self.active_profile()?.id))
    }

    fn file_settings_now(&self) -> Option<(String, FileSettings)> {
        let p = self.active_profile()?;
        let s = FileSettings::load(&self.store, &p.id);
        Some((p.id, s))
    }

    /// The modem gets the profile's file settings before it starts.
    pub(super) fn files_for_modem(&mut self, profile_id: &str) {
        self.modem_opts.files = options(&FileSettings::load(&self.store, profile_id));
    }

    /// A station was selected (or the app started): nothing from before is
    /// in flight, so its temp files go (M4 C5, Review Focus 4).
    pub(super) fn files_reset(&mut self, station_dir: &Path) {
        self.files.prepared.lock().unwrap().clear();
        self.files.progress.clear();
        self.files.offers.clear();
        self.files.measured_bps = None;
        self.files.live.clear();
        empty_dir(&station_dir.join("tmp"));
        empty_dir(&station_dir.join("files").join(".partial"));
        if let Some(d) = self.modem_opts.files.receive_dir.as_ref() {
            empty_dir(&d.join(".partial"));
        }
    }

    /// The rate an estimate uses, and whether it was measured.
    fn file_rate(&self) -> (f64, bool) {
        match self.files.measured_bps {
            Some(r) => (r, true),
            None => {
                let bw = self.session.as_ref().map(|s| bandwidth(&s.bandwidth)).unwrap_or(Bandwidth::Bw2300);
                (nominal_rate(bw), false)
            }
        }
    }

    /// Whether any transfer is under way (offered, waiting, moving, checking).
    pub(super) fn files_active(&self) -> bool {
        self.chat
            .as_ref()
            .and_then(|s| s.transfers(50).ok())
            .is_some_and(|rows| rows.iter().any(|r| !r.state.is_final()))
    }

    pub(super) fn prepare_file(&mut self, path: String, photo: PhotoSize, reply: Reply<CommandOk>) {
        let Some(tmp) = self.station_dir().map(|d| d.join("tmp")) else {
            let _ = reply.send(Err(conflict("no station is selected".into())));
            return;
        };
        let (rate, measured) = self.file_rate();
        let map = self.files.prepared.clone();
        tokio::spawn(async move {
            let src = PathBuf::from(&path);
            let res = tokio::task::spawn_blocking(move || prepare(&src, photo, &tmp, MAX_FILE_CEILING)).await;
            let out = match res {
                Ok(Ok(p)) => {
                    let prep_id = uuid::Uuid::new_v4().to_string();
                    let view = PreparedView {
                        prep_id: prep_id.clone(),
                        name: p.name.clone(),
                        size: p.size,
                        wire_size: p.wire_size,
                        compressed: p.encoding.is_some(),
                        shrunk: p.shrunk,
                        keeps_metadata: p.keeps_metadata,
                        airtime_s: airtime_s(p.wire_size, rate),
                        measured,
                    };
                    map.lock().unwrap().insert(prep_id, p);
                    Ok(CommandOk::Prepared(view))
                }
                Ok(Err(PrepareError::TooLarge { size, max })) => Err(refused(
                    "files.refused.tooLarge",
                    format!("the file is {size} bytes; the most that can be sent is {max} bytes"),
                )),
                Ok(Err(PrepareError::Unreadable(e))) => Err(conflict(format!("the file cannot be read: {e}"))),
                Ok(Err(PrepareError::Io(e))) => Err(AppError::Internal { message: format!("preparing failed: {e}") }),
                Err(e) => Err(AppError::Internal { message: format!("preparing stopped: {e}") }),
            };
            let _ = reply.send(out);
        });
    }

    pub(super) fn discard_prepared(&mut self, prep_id: &str) -> Result<CommandOk, AppError> {
        if let Some(p) = self.files.prepared.lock().unwrap().remove(prep_id) {
            let _ = std::fs::remove_file(&p.path);
        }
        Ok(CommandOk::Done)
    }

    pub(super) fn send_file(&mut self, prep_id: &str) -> Result<CommandOk, AppError> {
        let s = self.session.clone().ok_or_else(|| refused("files.refused.noSession", "there is no session"))?;
        if !s.files {
            return Err(refused("files.refused.unsupported", "not supported by this station"));
        }
        self.files_no_qsy()?;
        let size = {
            let map = self.files.prepared.lock().unwrap();
            map.get(prep_id).map(|p| p.size).ok_or_else(|| conflict("that file is no longer prepared".into()))?
        };
        if s.peer_max_file > 0 && size > s.peer_max_file {
            return Err(refused(
                "files.refused.overPeerLimit",
                format!("{} takes files up to {} bytes; this one is {size}", s.peer, s.peer_max_file),
            ));
        }
        let prepared = self.files.prepared.lock().unwrap().remove(prep_id).expect("checked above");
        let id = ObjectId::random();
        let row = TransferRow {
            id: id.0,
            direction: Direction::Out,
            peer: s.peer.clone(),
            session_id: Some(s.session_id.clone()),
            original_name: prepared.name.clone(),
            saved_name: None,
            size: prepared.size,
            wire_size: prepared.wire_size,
            sha256: prepared.sha256.clone(),
            encoding: prepared.encoding.map(|_| "zstd".to_string()),
            state: TransferState::Waiting,
            bytes_done: 0,
            reason: None,
            created_utc: String::new(),
            updated_utc: String::new(),
            saved_path: None,
        };
        let path = prepared.path.clone();
        let store = self.chat.as_mut().ok_or_else(|| conflict("the station's data is unavailable".into()))?;
        store.transfer_upsert(&row).map_err(storage)?;
        if let Err(e) = self.command_modem(ModemCmd::File(SessionCommand::SendFile { id, prepared })) {
            let _ = std::fs::remove_file(path);
            if let Some(store) = self.chat.as_mut() {
                let _ = store.transfer_set_state(Direction::Out, &id.0, TransferState::Failed, 0, Some("modem"));
            }
            return Err(e);
        }
        self.files.live.insert(id.0);
        self.publish_transfer(&id.0);
        Ok(CommandOk::Done)
    }

    fn row(&self, id: &[u8; 16]) -> Option<TransferRow> {
        let s = self.chat.as_ref()?;
        s.transfer(Direction::Out, id).ok().flatten().or_else(|| s.transfer(Direction::In, id).ok().flatten())
    }

    fn incoming(&self, id: &str) -> Result<[u8; 16], AppError> {
        let raw = parse_id(id)?;
        match self.chat.as_ref().and_then(|s| s.transfer(Direction::In, &raw).ok().flatten()) {
            Some(r) if r.state == TransferState::Offered => Ok(raw),
            Some(_) => Err(conflict("that offer has already been answered".into())),
            None => Err(AppError::NotFound { id: id.to_string() }),
        }
    }

    /// No file starts while a QSY is being arranged: its disconnect would cut
    /// the file short (the other order is `qsy_no_transfer`).
    fn files_no_qsy(&self) -> Result<(), AppError> {
        if self.qsy_pending() {
            return Err(refused("files.refused.qsy", "not while a QSY is being arranged"));
        }
        Ok(())
    }

    pub(super) fn accept_file(&mut self, id: &str) -> Result<CommandOk, AppError> {
        let raw = self.incoming(id)?;
        self.files_no_qsy()?;
        self.command_modem(ModemCmd::File(SessionCommand::AcceptFile { id: ObjectId(raw) }))?;
        self.files.offers.remove(&raw);
        Ok(CommandOk::Done)
    }

    pub(super) fn decline_file(&mut self, id: &str) -> Result<CommandOk, AppError> {
        let raw = self.incoming(id)?;
        self.command_modem(ModemCmd::File(SessionCommand::RejectFile { id: ObjectId(raw) }))?;
        self.files.offers.remove(&raw);
        Ok(CommandOk::Done)
    }

    pub(super) fn extend_file(&mut self, id: &str) -> Result<CommandOk, AppError> {
        let raw = self.incoming(id)?;
        match self.files.offers.get(&raw) {
            Some((_, true)) => return Err(conflict("the answer window was already extended once".into())),
            None => return Err(conflict("that offer has no answer window".into())),
            Some(_) => {}
        }
        self.command_modem(ModemCmd::File(SessionCommand::ExtendFile { id: ObjectId(raw) }))?;
        if let Some(o) = self.files.offers.get_mut(&raw) {
            o.0 += Duration::from_secs(u64::from(OFFER_WINDOW_S));
            o.1 = true;
        }
        self.publish_transfer(&raw);
        Ok(CommandOk::Done)
    }

    pub(super) fn cancel_file(&mut self, id: &str) -> Result<CommandOk, AppError> {
        let raw = parse_id(id)?;
        let row = self.row(&raw).ok_or_else(|| AppError::NotFound { id: id.to_string() })?;
        if row.state.is_final() {
            return Err(conflict("that transfer has already ended".into()));
        }
        self.command_modem(ModemCmd::File(SessionCommand::CancelFile { id: ObjectId(raw) }))?;
        Ok(CommandOk::Done)
    }

    pub(super) fn transfers(&self) -> Result<CommandOk, AppError> {
        let store = self.chat.as_ref().ok_or_else(|| conflict("no station is selected".into()))?;
        let rows = store.transfers(200).map_err(storage)?;
        let now = Instant::now();
        Ok(CommandOk::Transfers { transfers: rows.iter().map(|r| self.view(r, now)).collect() })
    }

    /// Opens the file manager on a received file, never the file (FIL-04).
    pub(super) fn show_in_folder(&self, id: &str) -> Result<CommandOk, AppError> {
        let raw = parse_id(id)?;
        let row = self
            .chat
            .as_ref()
            .and_then(|s| s.transfer(Direction::In, &raw).ok().flatten())
            .ok_or_else(|| AppError::NotFound { id: id.to_string() })?;
        let path = row
            .saved_path
            .filter(|_| row.state == TransferState::Complete)
            .map(PathBuf::from)
            .ok_or_else(|| conflict("that file was not saved".into()))?;
        if !path.exists() {
            return Err(conflict("the file is no longer in its folder".into()));
        }
        let spawned = if cfg!(windows) {
            let mut arg = std::ffi::OsString::from("/select,");
            arg.push(&path);
            std::process::Command::new("explorer").arg(arg).spawn()
        } else if cfg!(target_os = "macos") {
            std::process::Command::new("open").arg("-R").arg(&path).spawn()
        } else {
            let dir = path.parent().unwrap_or(Path::new("."));
            std::process::Command::new("xdg-open").arg(dir).spawn()
        };
        spawned.map_err(|e| conflict(format!("the file manager did not open: {e}")))?;
        Ok(CommandOk::Done)
    }

    pub(super) fn file_settings(&self) -> Result<CommandOk, AppError> {
        let (_, s) = self.file_settings_now().ok_or_else(|| conflict("no station is selected".into()))?;
        Ok(CommandOk::FileSettings(s))
    }

    /// Stored at once; the station takes them now, a session from its next.
    pub(super) fn set_file_settings(&mut self, settings: FileSettings) -> Result<CommandOk, AppError> {
        settings.check().map_err(|(f, m)| AppError::Validation { errors: crate::app::chat::field(f, m) })?;
        let id = self.active_profile().map(|p| p.id).ok_or_else(|| conflict("no station is selected".into()))?;
        settings.save(&mut self.store, &id).map_err(storage)?;
        self.modem_opts.files = options(&settings);
        let _ = self.command_modem(ModemCmd::FileSettings(self.modem_opts.files.clone()));
        Ok(CommandOk::FileSettings(settings))
    }

    fn view(&self, r: &TransferRow, now: Instant) -> TransferView {
        let mut v = TransferView::from(r);
        if r.state.is_final() {
            return v;
        }
        if let Some(p) = self.files.progress.get(&r.id) {
            v.bytes_done = v.bytes_done.max(p.bytes);
            let dt = now.saturating_duration_since(p.first.0);
            if dt >= MEASURE_AFTER && p.bytes > p.first.1 {
                let rate = (p.bytes - p.first.1) as f64 / dt.as_secs_f64();
                v.eta_s = Some(
                    r.wire_size.saturating_sub(p.bytes).div_ceil(rate.max(1.0) as u64).min(u32::MAX as u64) as u32
                );
            }
        }
        if r.state == TransferState::Offered {
            if let Some((deadline, _)) = self.files.offers.get(&r.id) {
                v.offer_expires_s = Some(deadline.saturating_duration_since(now).as_secs().min(u32::MAX as u64) as u32);
            }
        }
        v
    }

    fn publish_transfer(&mut self, id: &[u8; 16]) {
        if let Some(r) = self.row(id) {
            let v = self.view(&r, Instant::now());
            self.emit(AppEvent::Transfer { transfer: v });
        }
    }

    /// A file event from the session: its row, as the UI sees it.
    pub(super) fn on_file(&mut self, ev: SessionEvent) {
        let now = Instant::now();
        let (id, progress) = match &ev {
            SessionEvent::FileProgress { id, bytes, total } => (id.0, Some((*bytes, *total))),
            SessionEvent::FileOffered { id, .. }
            | SessionEvent::FileSending { id, .. }
            | SessionEvent::FileReceived { id, .. }
            | SessionEvent::FileDelivered { id }
            | SessionEvent::FileRejected { id, .. }
            | SessionEvent::FileFailed { id, .. }
            | SessionEvent::FileCancelled { id, .. } => (id.0, None),
            _ => return,
        };
        // An end the engine did not store (a refusal before the wire, a file
        // still waiting its turn) is stored here; one it stored stays.
        let ended: Option<(TransferState, String)> = match &ev {
            SessionEvent::FileRejected { reason, .. } => Some((TransferState::Rejected, reason.clone())),
            SessionEvent::FileFailed { reason, .. } => Some((TransferState::Failed, reason.clone())),
            SessionEvent::FileCancelled { by_peer, .. } => {
                Some((TransferState::Cancelled, if *by_peer { "peer" } else { "user" }.to_string()))
            }
            _ => None,
        };
        let session_id = self.session.as_ref().map(|s| s.session_id.clone());
        if let Some(store) = self.chat.as_mut() {
            if let Some(sid) = &session_id {
                let _ = store.transfer_set_session(Direction::In, &id, sid);
            }
            if let Some((state, reason)) = ended.clone() {
                for dir in [Direction::Out, Direction::In] {
                    if let Ok(Some(r)) = store.transfer(dir, &id) {
                        if !r.state.is_final() {
                            let _ = store.transfer_set_state(dir, &id, state, r.bytes_done, Some(&reason));
                        }
                    }
                }
            }
        }
        if ended.is_some() || matches!(ev, SessionEvent::FileReceived { .. } | SessionEvent::FileDelivered { .. }) {
            self.files.live.remove(&id);
        } else {
            self.files.live.insert(id);
        }
        if matches!(ev, SessionEvent::FileOffered { .. }) {
            self.files.offers.insert(id, (now + Duration::from_secs(u64::from(OFFER_WINDOW_S)), false));
        }
        if let Some((bytes, total)) = progress {
            let p = self.files.progress.entry(id).or_insert(Progress { first: (now, bytes), bytes, emitted: None });
            p.bytes = bytes;
            let dt = now.saturating_duration_since(p.first.0);
            if dt >= MEASURE_AFTER && bytes > p.first.1 {
                self.files.measured_bps = Some((bytes - p.first.1) as f64 / dt.as_secs_f64());
            }
            let due = p.emitted.is_none_or(|t| now.saturating_duration_since(t) >= PROGRESS_EVERY) || bytes >= total;
            if !due {
                return;
            }
            p.emitted = Some(now);
        } else if !matches!(ev, SessionEvent::FileSending { .. } | SessionEvent::FileOffered { .. }) {
            self.files.progress.remove(&id);
            self.files.offers.remove(&id);
        }
        self.publish_transfer(&id);
    }

    /// The session ended: whatever had not finished is interrupted (there is
    /// no resume, FIL-05), and the next estimate starts from nominal again.
    pub(super) fn files_session_ended(&mut self) {
        let live: Vec<[u8; 16]> = self.files.live.drain().collect();
        for id in &live {
            if let Some(r) = self.row(id).filter(|r| !r.state.is_final()) {
                if let Some(store) = self.chat.as_mut() {
                    let _ = store.transfer_set_state(
                        r.direction,
                        id,
                        TransferState::Interrupted,
                        r.bytes_done,
                        Some("link"),
                    );
                }
            }
        }
        self.files.progress.clear();
        self.files.offers.clear();
        self.files.measured_bps = None;
        for id in live {
            self.publish_transfer(&id);
        }
    }
}
