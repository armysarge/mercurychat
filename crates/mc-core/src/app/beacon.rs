//! Periodic beacons (DSC-03): opt-in, jittered, idle-only, never bursting.
//!
//! [`BeaconScheduler`] is a pure state machine: it is told the time and what
//! the modem and the channel are doing, and says when the next beacon is due
//! and whether one may go now. The actor owns the real clock and the actual
//! send (through the same gate as a CQ). Four rules shape it:
//!
//! * A beacon never transmits outside idle. A beacon that falls due while the
//!   modem is busy, or while the channel has been busy for under
//!   [`BUSY_HOLD`], is skipped, and the next one is one jittered interval
//!   after the suppression ends. There is never a catch-up.
//! * Beacons are armed for one run only. Start-up, a profile switch and a
//!   recovery all leave them disarmed (RF-02): the stored preference is only
//!   shown.
//! * `BUSY ON` is advisory (03 §6): after [`BUSY_HOLD`] with no `BUSY OFF`, a
//!   due beacon may go out anyway.
//! * An unresponsive broadcast port is reset. After [`SILENT_HOLD`]
//!   attempts that time out without PTT or are refused because of the reset,
//!   the schedule holds, with the reason shown, until a beacon is seen to go out
//!   or beacons are armed again (lab F3: another program may hold Mercury's
//!   one broadcast client slot, which cannot be seen at connect time).

use super::broadcast::{GateRefusal, NotConfirmedReason};
use super::events::ModemState;
use mc_storage::{now_utc, StorageError, Store};
use mch_protocol::beacon::{Beacon, CAP_CHAT};
use serde::Serialize;
use std::time::Duration;
use tokio::time::Instant;
use ts_rs::TS;

/// The shortest and longest configurable interval, in minutes.
pub const MIN_INTERVAL_MIN: u32 = 5;
pub const MAX_INTERVAL_MIN: u32 = 60;
pub const DEFAULT_INTERVAL_MIN: u32 = 10;
/// A channel that has shown `BUSY ON` this long no longer holds a beacon back.
pub const BUSY_HOLD: Duration = Duration::from_secs(10);
/// This many consecutive attempts at an unresponsive broadcast port hold
/// the schedule (lab F3).
pub const SILENT_HOLD: u32 = 2;

/// Jitter is `interval × (JITTER_LOW + JITTER_SPAN × u)` for `u` in [0, 1).
const JITTER_LOW: f64 = 0.85;
const JITTER_SPAN: f64 = 0.30;

/// A source of `u` in [0, 1); production uses `rand`, tests a fixed value.
pub type Jitter = Box<dyn FnMut() -> f64 + Send>;

pub fn random_jitter() -> Jitter {
    Box::new(rand::random::<f64>)
}

/// What the modem has said about the channel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
#[ts(export)]
pub enum ChannelState {
    /// No BUSY line has been seen yet.
    Unknown,
    Clear,
    Busy {
        since_utc: String,
    },
}

/// The beacon state the UI shows (DSC-03).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct BeaconStatus {
    /// Armed for this run: beacons are being scheduled.
    pub active: bool,
    /// The active profile's stored preference: shown, never applied by itself.
    pub preferred: bool,
    pub interval_min: u32,
    /// Whether the beacon carries the profile's locator.
    pub include_grid: bool,
    pub next_due_utc: Option<String>,
    /// Why no beacon can go right now, in plain words.
    pub suppressed: Option<String>,
    pub last_utc: Option<String>,
    pub channel: ChannelState,
}

impl BeaconStatus {
    fn idle() -> BeaconStatus {
        BeaconStatus {
            active: false,
            preferred: false,
            interval_min: DEFAULT_INTERVAL_MIN,
            include_grid: true,
            next_due_utc: None,
            suppressed: None,
            last_utc: None,
            channel: ChannelState::Unknown,
        }
    }
}

/// What a tick asks of the actor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tick {
    Nothing,
    /// Send one beacon now. The next is already scheduled.
    Send,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Channel {
    Unknown,
    Clear,
    Busy { since: Instant },
}

/// Why a beacon cannot go at a given moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Block {
    /// The modem is not idle; this ends only when its state changes.
    Modem,
    /// The channel is busy until `until` (the hold's end).
    Channel { until: Instant },
}

pub struct BeaconScheduler {
    interval_min: u32,
    jitter: Jitter,
    active: bool,
    state: ModemState,
    channel: Channel,
    next_due: Option<Instant>,
    /// A beacon fell due while the modem was not idle: waiting for it to be.
    waiting: bool,
    /// Beacons in a row the modem never keyed.
    silent: u32,
    /// Held after [`SILENT_HOLD`] of them: nothing is scheduled until a
    /// beacon is seen to go out or the operator arms beacons again.
    held: bool,
}

impl BeaconScheduler {
    pub fn new(jitter: Jitter) -> BeaconScheduler {
        BeaconScheduler {
            interval_min: DEFAULT_INTERVAL_MIN,
            jitter,
            active: false,
            state: ModemState::Unavailable,
            channel: Channel::Unknown,
            next_due: None,
            waiting: false,
            silent: 0,
            held: false,
        }
    }

    /// Held for a silent broadcast port.
    pub fn held(&self) -> bool {
        self.held
    }

    /// Count a silent beacon attempt or a retry refused because the port
    /// was reset. The second in a row holds the schedule.
    pub fn on_silent(&mut self) {
        self.silent = self.silent.saturating_add(1);
        if self.active && self.silent >= SILENT_HOLD {
            self.held = true;
            self.waiting = false;
            self.next_due = None;
        }
    }

    /// A beacon was seen to go out (its PTT came and went): the port works.
    /// A hold ends, and the next beacon is one interval from `now`.
    pub fn on_transmitted(&mut self, now: Instant) {
        self.silent = 0;
        if std::mem::take(&mut self.held) && self.active {
            self.next_due = Some(now + self.jittered());
        }
    }

    pub fn active(&self) -> bool {
        self.active
    }

    pub fn interval_min(&self) -> u32 {
        self.interval_min
    }

    /// When the actor next has to look at the scheduler: the due time, or the
    /// end of a busy hold (so that change is published when it happens).
    pub fn wake(&self, now: Instant) -> Option<Instant> {
        let hold_end = match self.channel {
            Channel::Busy { since } if self.active && now < since + BUSY_HOLD => Some(since + BUSY_HOLD),
            _ => None,
        };
        match (self.deadline(), hold_end) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// When the next beacon is due.
    pub fn deadline(&self) -> Option<Instant> {
        self.next_due.filter(|_| self.active)
    }

    fn jittered(&mut self) -> Duration {
        let u = (self.jitter)().clamp(0.0, 0.999_999);
        Duration::from_secs_f64(f64::from(self.interval_min) * 60.0 * (JITTER_LOW + JITTER_SPAN * u))
    }

    /// Arm for this run: the first beacon is one jittered interval away.
    pub fn arm(&mut self, now: Instant) {
        self.active = true;
        self.waiting = false;
        self.silent = 0;
        self.held = false;
        self.next_due = Some(now + self.jittered());
    }

    pub fn disarm(&mut self) {
        self.active = false;
        self.waiting = false;
        self.silent = 0;
        self.held = false;
        self.next_due = None;
    }

    /// A new modem (start-up, profile switch): nothing is armed, nothing is
    /// known of the channel.
    pub fn reset(&mut self) {
        self.disarm();
        self.state = ModemState::Unavailable;
        self.channel = Channel::Unknown;
    }

    /// A new interval; an armed schedule is redrawn from `now`.
    pub fn set_interval(&mut self, minutes: u32, now: Instant) {
        self.interval_min = minutes;
        if self.active && !self.waiting && !self.held {
            self.next_due = Some(now + self.jittered());
        }
    }

    fn block(&self, now: Instant) -> Option<Block> {
        if !matches!(self.state, ModemState::Ready | ModemState::Listening) {
            return Some(Block::Modem);
        }
        match self.channel {
            Channel::Busy { since } if now < since + BUSY_HOLD => Some(Block::Channel { until: since + BUSY_HOLD }),
            _ => None,
        }
    }

    /// Why nothing can be sent at `now`, for the operator.
    pub fn suppressed(&self, now: Instant) -> Option<String> {
        if !self.active {
            return Some("beacons are off until you turn them on".into());
        }
        if self.held {
            return Some(NotConfirmedReason::BroadcastPortSilent.text().into());
        }
        match self.block(now)? {
            Block::Modem => {
                if self.state == ModemState::Calling {
                    let why = GateRefusal::Calling.message();
                    return Some(why.trim_start_matches("not sent: ").to_string());
                }
                let why = GateRefusal::NotIdle(self.state).message();
                Some(why.trim_start_matches("not sent: ").to_string())
            }
            Block::Channel { .. } => Some("the channel is busy".into()),
        }
    }

    /// `Some(since)` while the channel is reported busy.
    pub fn busy_since(&self) -> Option<Instant> {
        match self.channel {
            Channel::Busy { since } => Some(since),
            _ => None,
        }
    }

    /// Whether any BUSY line has been seen since the modem attached.
    pub fn channel_known(&self) -> bool {
        self.channel != Channel::Unknown
    }

    /// The deadline passed: send, skip, or wait. Never more than one beacon,
    /// and never one while blocked.
    pub fn tick(&mut self, now: Instant) -> Tick {
        let Some(due) = self.next_due else { return Tick::Nothing };
        if !self.active || now < due {
            return Tick::Nothing;
        }
        match self.block(now) {
            None => {
                self.next_due = Some(now + self.jittered());
                Tick::Send
            }
            // The suppression's end is known: one interval after it.
            Some(Block::Channel { until }) => {
                self.next_due = Some(until + self.jittered());
                Tick::Nothing
            }
            // Unknown end: wait for the modem to be idle again.
            Some(Block::Modem) => {
                self.next_due = None;
                self.waiting = true;
                Tick::Nothing
            }
        }
    }

    pub fn on_state(&mut self, state: ModemState, now: Instant) {
        self.state = state;
        // Not attached: nothing stays armed across it (a recovery, a re-attach).
        if matches!(state, ModemState::Unavailable | ModemState::Attaching | ModemState::Recovering) {
            self.disarm();
            return;
        }
        if self.active && self.waiting && self.block(now) != Some(Block::Modem) {
            self.waiting = false;
            let from = match self.block(now) {
                Some(Block::Channel { until }) => until,
                _ => now,
            };
            self.next_due = Some(from + self.jittered());
        }
    }

    pub fn on_busy(&mut self, busy: bool, now: Instant) {
        self.channel = match (busy, self.channel) {
            (true, Channel::Busy { since }) => Channel::Busy { since },
            (true, _) => Channel::Busy { since: now },
            (false, _) => Channel::Clear,
        };
    }
}

// ---- what the actor keeps around the scheduler ------------------------------

/// A profile's stored beacon settings. The key scheme is
/// `profile.{id}.beacon.{preferred|interval_min|include_grid}` in the shared
/// settings table: the preference is shown and never applied by itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BeaconPrefs {
    pub preferred: bool,
    pub interval_min: u32,
    pub include_grid: bool,
}

impl Default for BeaconPrefs {
    fn default() -> Self {
        BeaconPrefs { preferred: false, interval_min: DEFAULT_INTERVAL_MIN, include_grid: true }
    }
}

fn key(id: &str, name: &str) -> String {
    format!("profile.{id}.beacon.{name}")
}

impl BeaconPrefs {
    /// What is stored for profile `id`; anything missing or unreadable is the default.
    pub fn load(store: &Store, id: &str) -> BeaconPrefs {
        let get = |name: &str| {
            store.setting(&key(id, name)).unwrap_or_else(|e| {
                tracing::warn!("could not read a beacon setting: {e}");
                None
            })
        };
        let d = BeaconPrefs::default();
        BeaconPrefs {
            preferred: get("preferred").map_or(d.preferred, |v| v == "1"),
            interval_min: get("interval_min")
                .and_then(|v| v.parse().ok())
                .filter(|m| (MIN_INTERVAL_MIN..=MAX_INTERVAL_MIN).contains(m))
                .unwrap_or(d.interval_min),
            include_grid: get("include_grid").map_or(d.include_grid, |v| v == "1"),
        }
    }

    pub fn save_preferred(store: &mut Store, id: &str, on: bool) -> Result<(), StorageError> {
        store.set_setting(&key(id, "preferred"), if on { "1" } else { "0" })
    }

    pub fn save_interval(store: &mut Store, id: &str, minutes: u32) -> Result<(), StorageError> {
        store.set_setting(&key(id, "interval_min"), &minutes.to_string())
    }

    pub fn save_include_grid(store: &mut Store, id: &str, on: bool) -> Result<(), StorageError> {
        store.set_setting(&key(id, "include_grid"), if on { "1" } else { "0" })
    }

    /// Drop a deleted profile's settings.
    pub fn forget(store: &mut Store, id: &str) {
        for name in ["preferred", "interval_min", "include_grid"] {
            if let Err(e) = store.delete_setting(&key(id, name)) {
                tracing::warn!("could not remove a beacon setting: {e}");
            }
        }
    }
}

/// The scheduler with everything the actor tracks around it: the active
/// profile's settings, the beacon sequence number, and the UTC strings of the
/// status (taken once, so an unchanged status compares equal).
pub struct BeaconState {
    pub sched: BeaconScheduler,
    pub prefs: BeaconPrefs,
    /// The profile the settings belong to.
    pub profile: Option<String>,
    seq: u32,
    last_utc: Option<String>,
    busy_utc: Option<String>,
    due: Option<(Instant, String)>,
    shown: BeaconStatus,
}

impl BeaconState {
    pub fn new() -> BeaconState {
        let mut state = BeaconState {
            sched: BeaconScheduler::new(random_jitter()),
            prefs: BeaconPrefs::default(),
            profile: None,
            seq: rand::random::<u32>(),
            last_utc: None,
            busy_utc: None,
            due: None,
            shown: BeaconStatus::idle(),
        };
        state.shown = state.status(Instant::now());
        state
    }

    /// A new modem or profile: disarmed, channel unknown, settings reloaded.
    pub fn select(&mut self, profile: Option<String>, store: &Store) {
        self.sched.reset();
        self.busy_utc = None;
        self.prefs = profile.as_deref().map_or_else(BeaconPrefs::default, |id| BeaconPrefs::load(store, id));
        self.sched.set_interval(self.prefs.interval_min, Instant::now());
        self.profile = profile;
    }

    pub fn on_busy(&mut self, busy: bool, now: Instant) {
        self.sched.on_busy(busy, now);
        match (busy, self.busy_utc.is_some()) {
            (true, false) => self.busy_utc = Some(now_utc()),
            (false, _) => self.busy_utc = None,
            _ => {}
        }
    }

    /// The MCB/1 payload for the next beacon. The sequence number moves only
    /// when [`submitted`](Self::submitted) says one was actually sent.
    pub fn payload(&self, callsign: &str, grid: Option<&str>) -> Result<Vec<u8>, String> {
        let grid = grid.filter(|_| self.prefs.include_grid);
        Beacon::new(self.seq, callsign, grid, CAP_CHAT).map(|b| b.encode()).map_err(|e| format!("not sent: {e}"))
    }

    /// A beacon was handed to the modem.
    pub fn submitted(&mut self) {
        self.seq = self.seq.wrapping_add(1);
        self.last_utc = Some(now_utc());
    }

    fn due_utc(&mut self, now: Instant) -> Option<String> {
        let at = self.sched.deadline()?;
        if let Some((cached, text)) = &self.due {
            if *cached == at {
                return Some(text.clone());
            }
        }
        let ahead = chrono::Duration::from_std(at.saturating_duration_since(now)).unwrap_or_default();
        let text = (chrono::Utc::now() + ahead).format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();
        self.due = Some((at, text.clone()));
        Some(text)
    }

    pub fn status(&mut self, now: Instant) -> BeaconStatus {
        let channel = match (self.sched.channel_known(), self.sched.busy_since(), &self.busy_utc) {
            (false, _, _) => ChannelState::Unknown,
            (true, Some(_), Some(since)) => ChannelState::Busy { since_utc: since.clone() },
            _ => ChannelState::Clear,
        };
        BeaconStatus {
            active: self.sched.active(),
            preferred: self.prefs.preferred,
            interval_min: self.prefs.interval_min,
            include_grid: self.prefs.include_grid,
            next_due_utc: self.due_utc(now),
            suppressed: self.sched.suppressed(now),
            last_utc: self.last_utc.clone(),
            channel,
        }
    }

    /// The status, if it differs from what was last published.
    pub fn changed(&mut self, now: Instant) -> Option<BeaconStatus> {
        let status = self.status(now);
        (status != self.shown).then(|| {
            self.shown = status.clone();
            status
        })
    }
}

impl Default for BeaconState {
    fn default() -> Self {
        BeaconState::new()
    }
}
