//! The actor's side of rig control (M3b design §7.1): it owns the rigctld
//! client task, checks every change against the gate at the moment it is
//! forwarded, holds a call while a frequency change is in flight, feeds the
//! policy what the client and Mercury report, and carries out its actions.

use super::{conflict, storage, App};
use crate::app::contract::{AppError, CommandOk};
use crate::app::discovery::Reading;
use crate::app::events::{AppEvent, ModemState};
use crate::app::modem::ModemCmd;
use crate::app::rig::{
    rig_gate, Activity, OwnerCheck, RigAction, RigBanner, RigChange, RigPolicy, TxLevelResult, RIG_SOURCE,
};
use crate::app::rigconfig::{rig_key, BandLevels, RigArrangement, RigConfig};
use crate::app::telemetry::Tuning;
use crate::band::{band_for, BANDS};
use mc_storage::now_utc;
use mercury_adapter::rigctl::{Level, PollRate, RigClient, RigClientCmd, RigLink, RigMsg, RigReply, RigRequest};
use mercury_adapter::telemetry::{RadioList, TelemetryCmd, TelemetryMsg};
use mercury_adapter::Callsign;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::AbortHandle;
use tokio::time::Instant;

/// A rig reading this fresh is the heard list's and the logbook's frequency.
pub const RIG_FRESH: Duration = Duration::from_secs(2);
/// The longest a call waits for a frequency change already sent to the rig.
pub const CALL_HOLD: Duration = Duration::from_secs(1);
/// Rig events at most this often; the snapshot always has the latest.
pub const RIG_EVENT_GAP: Duration = Duration::from_millis(500);
/// The last frequency is stored at most this often.
pub const LAST_SAVE_GAP: Duration = Duration::from_secs(5);
/// How long Mercury has to confirm a new TX level in its status (RIG-04).
pub const TX_LEVEL_CONFIRM: Duration = Duration::from_secs(5);
/// Two levels this close are the same.
pub const TX_LEVEL_TOLERANCE: f32 = 0.01;
/// Highest frequency a rig change may ask for (6 m tops out at 54 MHz).
pub const MAX_FREQ_HZ: u64 = 60_000_000;

struct AbortOnDrop(AbortHandle);
impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// A call held until the frequency change before it reaches the rig.
pub(super) struct HeldCall {
    peer: Callsign,
    before: ModemState,
    until: Instant,
}

/// The last tuning, restored on startup when the operator opts in (RIG-03).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LastTuning {
    freq_hz: u64,
    mode: Option<String>,
    passband_hz: Option<i32>,
}

pub(super) struct RigSide {
    pub(super) policy: RigPolicy,
    profile_id: Option<String>,
    tx: Option<mpsc::Sender<RigClientCmd>>,
    task: Option<AbortOnDrop>,
    /// A new client's message stream, for the actor loop to take.
    pub(super) inbox: Option<mpsc::Receiver<RigMsg>>,
    pub(super) inbox_new: bool,
    rate: Option<PollRate>,
    seq: u64,
    in_flight: Option<(u64, RigChange)>,
    held_call: Option<HeldCall>,
    dirty: bool,
    emitted_at: Option<Instant>,
    restore_pending: bool,
    last_saved: Option<LastTuning>,
    last_saved_at: Option<Instant>,
    /// The rig settings the link runs on.
    cfg: RigConfig,
    /// Commands for the modem's telemetry client (set_tx_gain), if it has one.
    tele_tx: Option<mpsc::Sender<TelemetryCmd>>,
    /// Mercury's last `radio_list`, for the two-owner check.
    radio_list: Option<RadioList>,
    /// The band the rig was last on, and a TX level waiting to be applied.
    band: Option<String>,
    tx_pending: Option<(String, f32)>,
    /// A TX level sent, awaiting Mercury's status: (band, dB, deadline).
    tx_wait: Option<(String, f32, Instant)>,
}

impl Default for RigSide {
    fn default() -> Self {
        RigSide {
            policy: RigPolicy::new(false),
            profile_id: None,
            tx: None,
            task: None,
            inbox: None,
            inbox_new: false,
            rate: None,
            seq: 0,
            in_flight: None,
            held_call: None,
            dirty: false,
            emitted_at: None,
            restore_pending: false,
            last_saved: None,
            last_saved_at: None,
            cfg: RigConfig::default(),
            tele_tx: None,
            radio_list: None,
            band: None,
            tx_pending: None,
            tx_wait: None,
        }
    }
}

/// Does Mercury key through the rigctld MercuryChat reads (RIG-01; design §2.11)?
fn owner_check(cfg: &RigConfig, rl: Option<&RadioList>) -> OwnerCheck {
    let Some(rl) = rl else { return OwnerCheck::Unknown };
    if cfg.arrangement != RigArrangement::Rigctld || rl.ptt_method != "hamlib" {
        return OwnerCheck::Ok;
    }
    let ours = format!("{}:{}", cfg.host, cfg.port);
    let norm = |a: &str| a.trim().replace("localhost", "127.0.0.1");
    match rl.selected_model {
        Some(2) if norm(&rl.device_path) == norm(&ours) => OwnerCheck::Ok,
        Some(2) => OwnerCheck::Warn {
            reason: format!(
                "Mercury keys through the rigctld at {}, MercuryChat reads the one at {ours}: the PTT check only works if both reach the same rigctld",
                rl.device_path
            ),
        },
        Some(model) => OwnerCheck::Refused {
            reason: format!(
                "Mercury keys the radio through its own CAT connection (Hamlib model {model} on {}), so the CAT port would have two owners: set the PTT in Mercury to Hamlib model 2 (rigctld) at {ours}",
                rl.device_path
            ),
        },
        None => OwnerCheck::Warn { reason: "Mercury keys through Hamlib, but which rig it uses could not be read".into() },
    }
}

fn refused(r: crate::app::rig::RigRefusal) -> AppError {
    AppError::Refused { reason: r.key(), message: r.message().to_string() }
}

fn invalid(field: &str, message: &str) -> AppError {
    AppError::Validation { errors: crate::app::chat::field(field, message) }
}

fn request_for(change: &RigChange) -> Option<RigRequest> {
    Some(match change {
        RigChange::Frequency { hz } => RigRequest::SetFreq(*hz),
        RigChange::Mode { mode, passband_hz } => RigRequest::SetMode { mode: mode.clone(), passband_hz: *passband_hz },
        RigChange::Vfo { vfo } => RigRequest::SetVfo(vfo.clone()),
        RigChange::Split { on, tx_vfo } => RigRequest::SetSplit { on: *on, tx_vfo: tx_vfo.clone() },
        RigChange::RfPower { level } => RigRequest::SetLevel(Level::RfPower, *level),
        RigChange::TxLevel | RigChange::Tune => return None,
    })
}

fn token_ok(s: &str) -> bool {
    !s.is_empty() && s.len() <= 16 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Reject a change that is malformed whatever the rig's state.
fn validate(change: &RigChange) -> Result<(), AppError> {
    match change {
        RigChange::Frequency { hz } if *hz == 0 || *hz > MAX_FREQ_HZ => {
            Err(invalid("frequency", "Enter a frequency between 1 Hz and 60 MHz"))
        }
        RigChange::Mode { mode, passband_hz } if !token_ok(mode) || !(0..=20_000).contains(passband_hz) => {
            Err(invalid("mode", "Choose a mode and a passband of 0 to 20000 Hz"))
        }
        RigChange::Vfo { vfo } if !token_ok(vfo) => Err(invalid("vfo", "Choose a VFO")),
        RigChange::Split { tx_vfo, .. } if !token_ok(tx_vfo) => Err(invalid("vfo", "Choose a VFO")),
        RigChange::RfPower { level } if !level.is_finite() || !(0.0..=1.0).contains(level) => {
            Err(invalid("power", "RF power is 0 to 100 %"))
        }
        RigChange::TxLevel | RigChange::Tune => Err(invalid("change", "use the TX level or Tune controls")),
        _ => Ok(()),
    }
}

impl App {
    /// What the station is doing, for the rig gate.
    pub(super) fn activity(&self) -> Activity {
        match self.modem {
            ModemState::Calling | ModemState::Negotiating => Activity::Calling,
            ModemState::Connected => Activity::Connected,
            ModemState::Draining | ModemState::Recovering => Activity::Draining,
            _ if self.rig.held_call.is_some() => Activity::Calling,
            _ if self.broadcasts.deadline().is_some() => Activity::Broadcasting,
            _ => Activity::Idle,
        }
    }

    /// The modem is attached (Tune and TX level go through it).
    pub(super) fn attached(&self) -> bool {
        matches!(
            self.modem,
            ModemState::Ready
                | ModemState::Listening
                | ModemState::Calling
                | ModemState::Negotiating
                | ModemState::Connected
                | ModemState::Draining
        )
    }

    /// Start (or stop) the rig link for a profile's settings. `None`: no profile.
    pub(super) fn start_rig(&mut self, profile_id: Option<&str>) {
        let cfg = profile_id.map(|id| RigConfig::load(&self.store, id)).unwrap_or_default();
        let r = &mut self.rig;
        r.task = None;
        r.tx = None;
        r.rate = None;
        r.in_flight = None;
        r.profile_id = profile_id.map(String::from);
        let configured = profile_id.is_some() && cfg.arrangement == RigArrangement::Rigctld;
        // No rig before and none now: nothing to tell the UI.
        let was_configured = r.policy.configured();
        r.policy = RigPolicy::new(configured);
        r.policy.set_activity(Activity::Idle);
        let check = owner_check(&cfg, r.radio_list.as_ref());
        let refused = matches!(check, OwnerCheck::Refused { .. });
        r.policy.set_owner_check(check);
        r.cfg = cfg.clone();
        r.restore_pending = configured && cfg.restore_on_startup;
        r.inbox_new = true;
        r.inbox = None;
        r.dirty = configured || was_configured;
        if configured && refused {
            // Two owners of the CAT port: no rig link until Mercury is changed.
            let reason = "not connected: Mercury owns the CAT port".to_string();
            r.policy.on_msg(&RigMsg::Link(RigLink::Lost { reason }), Instant::now());
        } else if configured {
            let (cmd_tx, cmd_rx) = mpsc::channel(32);
            let (out_tx, out_rx) = mpsc::channel(256);
            let task = RigClient::spawn(format!("{}:{}", cfg.host, cfg.port), cmd_rx, out_tx);
            r.task = Some(AbortOnDrop(task));
            r.tx = Some(cmd_tx);
            r.inbox = Some(out_rx);
        }
    }

    pub(super) fn set_rig_config(&mut self, config: RigConfig) -> Result<CommandOk, AppError> {
        let Some(id) = self.active_profile().map(|p| p.id) else {
            return Err(conflict("no station is selected".into()));
        };
        if config.arrangement == RigArrangement::Rigctld {
            let host = config.host.trim();
            if host.is_empty() || host.len() > 253 || host.chars().any(|c| c.is_whitespace() || c.is_control()) {
                return Err(invalid("host", "Enter the rigctld host, e.g. 127.0.0.1"));
            }
            if config.port == 0 {
                return Err(invalid("port", "Enter the rigctld port, e.g. 4532"));
            }
        }
        let config = RigConfig { host: config.host.trim().to_string(), ..config };
        config.save(&mut self.store, &id).map_err(|message| AppError::Storage { message })?;
        self.start_rig(Some(&id));
        Ok(CommandOk::Done)
    }

    fn rig_request(&mut self, req: RigRequest) -> Option<u64> {
        let tx = self.rig.tx.as_ref()?;
        self.rig.seq += 1;
        let id = self.rig.seq;
        tx.try_send(RigClientCmd::Request { id, req }).ok()?;
        Some(id)
    }

    pub(super) fn rig_set(&mut self, change: RigChange) -> Result<CommandOk, AppError> {
        validate(&change)?;
        let activity = self.activity();
        rig_gate(activity, &self.rig.policy, &change, self.attached()).map_err(refused)?;
        let req = request_for(&change).ok_or_else(|| invalid("change", "not a rig change"))?;
        let id = self.rig_request(req).ok_or_else(|| conflict("the rig link is not accepting commands".into()))?;
        self.rig.in_flight = Some((id, change));
        Ok(CommandOk::Done)
    }

    pub(super) fn dismiss_rig_banner(&mut self, banner: RigBanner) -> Result<CommandOk, AppError> {
        if !self.rig.policy.dismiss(banner) {
            return Err(conflict(match banner {
                RigBanner::LostWhileKeyed => "not yet: the rig is not known to be unkeyed".into(),
                _ => "not yet: the rig still disagrees with the modem".into(),
            }));
        }
        self.rig.dirty = true;
        Ok(CommandOk::Done)
    }

    /// A call arrives: hold it while a frequency change is on its way to the
    /// rig, so the change never lands after `CONNECT` (Review Focus 1).
    /// Returns true if it was held (the caller then only shows Calling).
    pub(super) fn hold_call(&mut self, peer: &Callsign) -> bool {
        if self.rig.in_flight.is_none() {
            return false;
        }
        self.rig.held_call =
            Some(HeldCall { peer: peer.clone(), before: self.modem, until: Instant::now() + CALL_HOLD });
        true
    }

    /// Cancel a held call (CancelCall or Abort before it went out).
    pub(super) fn cancel_held_call(&mut self) -> bool {
        let Some(h) = self.rig.held_call.take() else { return false };
        self.set_modem(h.before, None);
        true
    }

    fn release_held_call(&mut self, late: bool) {
        let Some(h) = self.rig.held_call.take() else { return };
        if late {
            let line = self.discovery.note("rig", "the frequency change had not finished after 1 s; calling anyway");
            self.emit(line);
        }
        if self.modem != ModemState::Calling {
            return; // the state moved on (a fault, a new selection)
        }
        if self.command_modem(ModemCmd::Call(h.peer.clone())).is_err() {
            self.set_modem(h.before, None);
            self.emit(AppEvent::CallFailed {
                peer: h.peer.as_str().to_string(),
                reason: "the modem is not accepting commands".into(),
            });
        }
    }

    /// Something the rigctld client said.
    pub(super) fn on_rig(&mut self, msg: RigMsg) {
        let now = Instant::now();
        if let RigMsg::Done { id, result } = &msg {
            self.rig_done(*id, result.clone(), now);
        }
        let came_up = matches!(msg, RigMsg::Link(RigLink::Up));
        let actions = self.rig.policy.on_msg(&msg, now);
        self.rig_actions(actions);
        if came_up && self.rig.restore_pending {
            self.rig.restore_pending = false;
            self.restore_last();
        }
        if matches!(msg, RigMsg::Freq(_) | RigMsg::Mode { .. }) {
            self.save_last(now);
        }
        self.rig.dirty = true;
    }

    /// Mercury's `PTT ON` / `PTT OFF`.
    pub(super) fn on_mercury_ptt(&mut self, on: bool) {
        let actions = self.rig.policy.on_mercury_ptt(on, Instant::now());
        self.rig_actions(actions);
        self.rig.dirty |= self.rig.policy.configured();
    }

    fn rig_done(&mut self, id: u64, result: Result<RigReply, String>, now: Instant) {
        let Some((want, change)) = self.rig.in_flight.take() else { return };
        if want != id {
            self.rig.in_flight = Some((want, change));
            return;
        }
        match result {
            Ok(_) => {
                // The operator's own choice is the "last frequency", stored at once.
                let v = self.rig.policy.view(now);
                let chosen = match &change {
                    RigChange::Frequency { hz } => {
                        Some(LastTuning { freq_hz: *hz, mode: v.mode, passband_hz: v.passband_hz })
                    }
                    RigChange::Mode { mode, passband_hz } => v.frequency.map(|f| LastTuning {
                        freq_hz: f.value,
                        mode: Some(mode.clone()),
                        passband_hz: Some(*passband_hz),
                    }),
                    RigChange::RfPower { level } => {
                        self.rig.policy.set_rf_power(*level, now);
                        None
                    }
                    _ => None,
                };
                if let Some(last) = chosen {
                    self.store_last(last, now);
                }
            }
            Err(e) => {
                let line = self.discovery.note("rig", format!("the rig did not take the change ({e})"));
                self.emit(line);
            }
        }
        self.release_held_call(false);
    }

    fn rig_actions(&mut self, actions: Vec<RigAction>) {
        for a in actions {
            let text = match a {
                RigAction::SendPttOff => {
                    let sent = self.rig_request(RigRequest::PttOff).is_some();
                    if sent {
                        "the rig link is back: sent PTT off once".to_string()
                    } else {
                        "the rig link is back, but PTT off could not be sent: unkey the radio by hand".to_string()
                    }
                }
                RigAction::Banner(RigBanner::PttNotFollowing) => {
                    "the rig's PTT does not follow the modem: check the radio and its CAT link".into()
                }
                RigAction::Banner(RigBanner::LostWhileKeyed) => {
                    "rig link lost while transmitting: unkey the radio by hand".into()
                }
                RigAction::Banner(RigBanner::RestartModem) => {
                    "Mercury reports PTT on but the rig is not keyed: restart the modem to key again".into()
                }
            };
            let line = self.discovery.note("rig", text);
            self.emit(line);
        }
    }

    fn restore_last(&mut self) {
        let Some(id) = self.rig.profile_id.clone() else { return };
        let Some(last) = self
            .store
            .setting(&rig_key(&id, "last"))
            .ok()
            .flatten()
            .and_then(|j| serde_json::from_str::<LastTuning>(&j).ok())
        else {
            return;
        };
        let change = RigChange::Frequency { hz: last.freq_hz };
        if rig_gate(self.activity(), &self.rig.policy, &change, self.attached()).is_err() {
            let line = self.discovery.note("rig", "the last frequency was not restored: the station is busy");
            self.emit(line);
            return;
        }
        self.rig_request(RigRequest::SetFreq(last.freq_hz));
        if let (Some(mode), Some(pb)) = (last.mode, last.passband_hz) {
            self.rig_request(RigRequest::SetMode { mode, passband_hz: pb });
        }
        let line = self.discovery.note("rig", format!("restored the last frequency, {} Hz", last.freq_hz));
        self.emit(line);
    }

    fn save_last(&mut self, now: Instant) {
        let v = self.rig.policy.view(now);
        let Some(f) = v.frequency else { return };
        let last = LastTuning { freq_hz: f.value, mode: v.mode, passband_hz: v.passband_hz };
        if self.rig.last_saved.as_ref() == Some(&last)
            || self.rig.last_saved_at.is_some_and(|t| now.saturating_duration_since(t) < LAST_SAVE_GAP)
        {
            return;
        }
        self.store_last(last, now);
    }

    fn store_last(&mut self, last: LastTuning, now: Instant) {
        let Some(id) = self.rig.profile_id.clone() else { return };
        if let Ok(json) = serde_json::to_string(&last) {
            if let Err(e) = self.store.set_setting(&rig_key(&id, "last"), &json).map_err(storage) {
                tracing::warn!("could not store the last frequency: {e}");
            }
        }
        self.rig.last_saved = Some(last);
        self.rig.last_saved_at = Some(now);
    }

    /// The frequency (and offset) to record with a heard station or a QSO:
    /// the rig's, when fresh and its link is up; else Mercury's telemetry.
    pub(super) fn tuning(&self, now: Instant) -> Tuning {
        let mut t = self.telemetry.tuning(now);
        if self.rig.policy.link_up() {
            if let Some((hz, at)) = self.rig.policy.frequency() {
                let age = now.saturating_duration_since(at);
                if age <= RIG_FRESH {
                    t.freq = Some(Reading {
                        value: hz,
                        source: RIG_SOURCE.into(),
                        at_utc: now_utc(),
                        age_ms: Some(age.as_millis() as u64),
                    });
                }
            }
        }
        t
    }

    /// After every turn of the loop: the activity and poll rate, a held
    /// call's deadline, and the throttled rig event.
    pub(super) fn sync_rig(&mut self) {
        let now = Instant::now();
        if self.rig.held_call.as_ref().is_some_and(|h| now >= h.until) {
            self.rig.in_flight = None;
            self.release_held_call(true);
        }
        self.sync_tx_level(now);
        let activity = self.activity();
        self.rig.policy.set_activity(activity);
        let rate = self.rig.policy.poll_rate(activity);
        if self.rig.rate != Some(rate) {
            if let Some(tx) = &self.rig.tx {
                if tx.try_send(RigClientCmd::Rate(rate)).is_ok() {
                    self.rig.rate = Some(rate);
                }
            }
        }
        if self.rig.dirty && self.rig.emitted_at.is_none_or(|t| now.saturating_duration_since(t) >= RIG_EVENT_GAP) {
            self.rig.dirty = false;
            self.rig.emitted_at = Some(now);
            let rig = self.rig.policy.view(now);
            self.emit(AppEvent::Rig { rig });
        }
    }

    /// When the loop must next wake for the rig: a throttled event, a held
    /// call or a TX level awaiting confirmation.
    pub(super) fn rig_wake(&self) -> Option<Instant> {
        let event = self.rig.dirty.then(|| self.rig.emitted_at.map(|t| t + RIG_EVENT_GAP)).flatten();
        let held = self.rig.held_call.as_ref().map(|h| h.until);
        let tx = self.rig.tx_wait.as_ref().map(|w| w.2);
        [event, held, tx].into_iter().flatten().min()
    }

    /// A new modem (or none): its telemetry client, and what it said, are gone.
    pub(super) fn rig_modem_changed(&mut self) {
        self.rig.tele_tx = None;
        self.rig.radio_list = None;
        self.rig.tx_wait = None;
        self.rig.tx_pending = None;
        self.rig.band = None;
        self.rig.policy.set_tx_level(None);
    }

    /// The modem's telemetry client takes commands here.
    pub(super) fn rig_telemetry(&mut self, tx: mpsc::Sender<TelemetryCmd>) {
        self.rig.tele_tx = Some(tx);
    }

    /// What the rig side reads from Mercury's telemetry.
    pub(super) fn rig_telemetry_msg(&mut self, msg: &TelemetryMsg) {
        match msg {
            TelemetryMsg::RadioList(rl) => {
                let before = self.rig.policy.owner_check().clone();
                self.rig.radio_list = Some(rl.clone());
                let after = owner_check(&self.rig.cfg, Some(rl));
                if after != before {
                    let was_refused = matches!(before, OwnerCheck::Refused { .. });
                    let refused = matches!(after, OwnerCheck::Refused { .. });
                    if let OwnerCheck::Refused { reason } | OwnerCheck::Warn { reason } = &after {
                        let line = self.discovery.note("rig", reason.clone());
                        self.emit(line);
                    }
                    if refused != was_refused {
                        // Stop the link, or start it again now that Mercury has changed.
                        let id = self.rig.profile_id.clone();
                        self.start_rig(id.as_deref());
                    } else {
                        self.rig.policy.set_owner_check(after);
                    }
                    self.rig.dirty = true;
                }
            }
            TelemetryMsg::Status(st) => {
                if let (Some((band, db, _)), Some(now)) = (&self.rig.tx_wait, st.tx_gain_db) {
                    if (now - db).abs() <= TX_LEVEL_TOLERANCE {
                        let result = TxLevelResult::Applied { band: band.clone(), db: *db };
                        self.rig.tx_wait = None;
                        self.tx_level_done(result);
                    }
                }
            }
            TelemetryMsg::CommandFailed { reason } => {
                if let Some((band, _, _)) = self.rig.tx_wait.take() {
                    self.tx_level_done(TxLevelResult::NotApplied { band, reason: reason.clone() });
                }
            }
            TelemetryMsg::Link(_) => {}
        }
    }

    fn tx_level_done(&mut self, result: TxLevelResult) {
        let text = match &result {
            TxLevelResult::Applied { band, db } => format!("TX level for {band} applied: {db:+.1} dB"),
            TxLevelResult::NotApplied { band, reason } => format!("TX level for {band} not applied: {reason}"),
            TxLevelResult::Pending { .. } => return,
        };
        let line = self.discovery.note("rig", text);
        self.emit(line);
        self.rig.policy.set_tx_level(Some(result));
        self.rig.dirty = true;
    }

    pub(super) fn set_band_levels(&mut self, band: String, levels: BandLevels) -> Result<CommandOk, AppError> {
        let Some(id) = self.active_profile().map(|p| p.id) else {
            return Err(conflict("no station is selected".into()));
        };
        if !BANDS.iter().any(|(_, _, b)| *b == band) {
            return Err(invalid("band", "Choose an amateur band, e.g. 40m"));
        }
        levels.validate().map_err(|m| invalid("levels", &m))?;
        BandLevels::save(&mut self.store, &id, &band, levels).map_err(|message| AppError::Storage { message })?;
        if self.rig.band.as_deref() == Some(band.as_str()) {
            self.rig.tx_pending = Some((band, levels.tx_gain_db));
        }
        Ok(CommandOk::Done)
    }

    /// Follow the band, and apply a stored TX level once the station is idle.
    fn sync_tx_level(&mut self, now: Instant) {
        if let Some((band, _, deadline)) = &self.rig.tx_wait {
            if now >= *deadline {
                let band = band.clone();
                self.rig.tx_wait = None;
                let reason = format!("Mercury did not confirm the new level within {} s", TX_LEVEL_CONFIRM.as_secs());
                self.tx_level_done(TxLevelResult::NotApplied { band, reason });
            }
        }
        let band = self.tuning(now).freq.and_then(|f| band_for(f.value)).map(String::from);
        if band.is_some() && band != self.rig.band {
            self.rig.band = band.clone();
            let stored = match (self.rig.profile_id.as_deref(), band.as_deref()) {
                (Some(id), Some(b)) => BandLevels::load_all(&self.store, id).get(b).copied(),
                _ => None,
            };
            // A band without stored levels leaves Mercury's own level alone.
            self.rig.tx_pending = stored.map(|l| (band.clone().unwrap_or_default(), l.tx_gain_db));
        }
        let Some((band, db)) = self.rig.tx_pending.clone() else { return };
        if self.rig.tx_wait.is_some()
            || rig_gate(self.activity(), &self.rig.policy, &RigChange::TxLevel, self.attached()).is_err()
        {
            return; // waits until the station is idle
        }
        self.rig.tx_pending = None;
        let sent = self.rig.tele_tx.as_ref().is_some_and(|t| t.try_send(TelemetryCmd::SetTxGain(db)).is_ok());
        if sent {
            self.rig.tx_wait = Some((band.clone(), db, now + TX_LEVEL_CONFIRM));
            self.rig.policy.set_tx_level(Some(TxLevelResult::Pending { band, db }));
            self.rig.dirty = true;
        } else {
            let reason = "no telemetry connection to Mercury".to_string();
            self.tx_level_done(TxLevelResult::NotApplied { band, reason });
        }
    }
}
