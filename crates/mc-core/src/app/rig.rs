//! Rig control policy (M3b design §7): the change gate, the PTT cross-check,
//! what a lost rig link means, and the rig as the operator sees it. Pure: no
//! sockets, and the clock is passed in. The service actor feeds it what the
//! rigctld client and Mercury report and carries out the actions it returns.
//!
//! Safety rules (owner decisions, 18 November 2026):
//! - MercuryChat never keys the rig. The only PTT write is one `T 0` after a
//!   rig link lost while keyed comes back, and never while Mercury says it is
//!   transmitting (§7.9).
//! - The cross-check faults only on a good reading that contradicts Mercury
//!   after a 0.5 s window. A timeout or an `RPRT` error is "unknown", never
//!   "off" (§7.8).
//! - PTT unknown refuses Tune; it does not refuse an idle frequency, mode or
//!   power change (setting the frequency never keys). A reading that stays
//!   unknown shows a standing "PTT check off" warning instead (§7.10).

use super::discovery::Reading;
use mc_storage::now_utc;
use mercury_adapter::rigctl::{Level, PollRate, PttReading, RigCaps, RigLink, RigMsg};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tokio::time::Instant;
use ts_rs::TS;

/// How long the rig has to follow a Mercury PTT edge.
pub const PTT_WINDOW: Duration = Duration::from_millis(500);
/// PTT unknown for this long while the link is up: "PTT check off".
pub const PTT_CHECK_OFF_AFTER: Duration = Duration::from_secs(3);
/// Good readings that agree with Mercury, in a row, to clear a fault.
pub const AGREE_TO_CLEAR: u32 = 2;
pub const RIG_SOURCE: &str = "rigctld";

/// What the station is doing, as far as rig changes care.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Activity {
    Idle,
    Calling,
    Connected,
    Draining,
    /// A CQ or beacon is in flight.
    Broadcasting,
    Tuning,
}

/// A change the operator (or the schedule) asks of the rig.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
#[ts(export)]
pub enum RigChange {
    Frequency {
        #[ts(type = "number")]
        hz: u64,
    },
    Mode {
        mode: String,
        passband_hz: i32,
    },
    Vfo {
        vfo: String,
    },
    Split {
        on: bool,
        tx_vfo: String,
    },
    /// RF power, 0.0–1.0 of the rig's maximum.
    RfPower {
        level: f32,
    },
    /// Apply a band's TX level through Mercury.
    TxLevel,
    /// Start a tuning carrier through Mercury.
    Tune,
}

// Built only from finite values (the actor rejects NaN), so equality is total.
impl Eq for RigChange {}

impl RigChange {
    /// Changes made through rigctld (the others go through Mercury).
    fn needs_rig(&self) -> bool {
        !matches!(self, RigChange::TxLevel | RigChange::Tune)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RigRefusal {
    Calling,
    Connected,
    Draining,
    Tuning,
    Broadcasting,
    PttOn,
    PttUnknown,
    LinkDown,
    Unsupported,
    NotAttached,
}

impl RigRefusal {
    /// The i18n key of the reason.
    pub fn key(&self) -> &'static str {
        match self {
            RigRefusal::Calling => "rig.refused.calling",
            RigRefusal::Connected => "rig.refused.connected",
            RigRefusal::Draining => "rig.refused.draining",
            RigRefusal::Tuning => "rig.refused.tuning",
            RigRefusal::Broadcasting => "rig.refused.broadcasting",
            RigRefusal::PttOn => "rig.refused.pttOn",
            RigRefusal::PttUnknown => "rig.refused.pttUnknown",
            RigRefusal::LinkDown => "rig.refused.linkDown",
            RigRefusal::Unsupported => "rig.refused.unsupported",
            RigRefusal::NotAttached => "rig.refused.notAttached",
        }
    }

    /// The reason in English (the UI shows its own language from `key`).
    pub fn message(&self) -> &'static str {
        match self {
            RigRefusal::Calling => "not while calling",
            RigRefusal::Connected => "not while connected",
            RigRefusal::Draining => "not while the modem is still sending",
            RigRefusal::Tuning => "not while tuning",
            RigRefusal::Broadcasting => "not while a CQ or beacon is going out",
            RigRefusal::PttOn => "not while transmitting",
            RigRefusal::PttUnknown => "the rig's PTT cannot be read, so the transmission could not be checked",
            RigRefusal::LinkDown => "the rig link is down",
            RigRefusal::Unsupported => "not supported by this rig",
            RigRefusal::NotAttached => "the modem is not attached",
        }
    }
}

/// Banners the rig can raise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum RigBanner {
    /// The rig's PTT disagrees with Mercury's.
    PttNotFollowing,
    /// The rig link was lost while transmitting: unkey by hand.
    LostWhileKeyed,
    /// Mercury must be restarted to key the rig again.
    RestartModem,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RigAction {
    /// Send rigctld `T 0` once.
    SendPttOff,
    Banner(RigBanner),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
#[ts(export)]
pub enum RigLinkView {
    /// No rigctld configured.
    Unconfigured,
    Connecting,
    Up,
    Lost {
        since_utc: String,
        reason: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum RigPttView {
    On,
    Off,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RigCapsView {
    pub model: String,
    pub can_set_freq: bool,
    pub can_set_mode: bool,
    pub can_set_vfo: bool,
    pub can_split: bool,
    pub can_get_ptt: bool,
    pub can_set_power: bool,
    /// `swr`, `alc`, `power`: the meters this rig reports.
    pub meters: Vec<String>,
    pub modes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RigMeters {
    pub swr: Option<Reading<f32>>,
    pub alc: Option<Reading<f32>>,
    pub power: Option<Reading<f32>>,
}

/// Whether Mercury keys through the same rigctld (RIG-01; design §2.11).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, TS)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
#[ts(export)]
pub enum OwnerCheck {
    Ok,
    Refused {
        reason: String,
    },
    Warn {
        reason: String,
    },
    #[default]
    Unknown,
}

/// What happened to the last band change's TX level (RIG-04).
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
#[ts(export)]
pub enum TxLevelResult {
    Pending { band: String, db: f32 },
    Applied { band: String, db: f32 },
    NotApplied { band: String, reason: String },
}

impl Eq for TxLevelResult {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RigView {
    pub link: RigLinkView,
    pub frequency: Option<Reading<u64>>,
    pub mode: Option<String>,
    pub passband_hz: Option<i32>,
    pub vfo: Option<String>,
    pub split: Option<bool>,
    pub rf_power: Option<Reading<f32>>,
    pub ptt: RigPttView,
    /// The rig's PTT has read unknown for a while with the link up: the
    /// cross-check cannot run (e.g. rigctld started without `-P`).
    pub ptt_check_off: bool,
    /// The cross-check's current fault, if the condition holds now.
    pub fault: Option<RigBanner>,
    /// Banners shown until dismissed (or, for the critical one, until safe).
    pub banners: Vec<RigBanner>,
    pub meters: RigMeters,
    pub caps: Option<RigCapsView>,
    pub owner_check: OwnerCheck,
    pub tx_level_applied: Option<TxLevelResult>,
}

#[derive(Debug, Clone)]
struct Stamped<T> {
    value: T,
    at: Instant,
    at_utc: String,
}

impl<T: Clone> Stamped<T> {
    fn new(value: T, at: Instant) -> Self {
        Stamped { value, at, at_utc: now_utc() }
    }
    fn reading(&self, now: Instant) -> Reading<T> {
        Reading {
            value: self.value.clone(),
            source: RIG_SOURCE.into(),
            at_utc: self.at_utc.clone(),
            age_ms: Some(now.saturating_duration_since(self.at).as_millis() as u64),
        }
    }
}

/// The cross-check against Mercury's latest PTT edge.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Check {
    /// No Mercury edge yet.
    None,
    /// Waiting for the rig to follow `want` since `since`.
    Pending { want: bool, since: Instant },
    /// The rig followed.
    Agreed { want: bool },
}

#[derive(Debug, Default)]
struct Recovery {
    /// The link was lost while keyed (or unknown during activity).
    lost_while_keyed: bool,
    ptt_off_sent: bool,
    /// Mercury said `PTT ON` when the link came back: wait for its OFF.
    awaiting_mercury_off: bool,
    /// Mercury's OFF arrived: check the rig after this instant.
    check_after: Option<Instant>,
    /// The link has come back at least once after being lost.
    recovered: bool,
}

pub struct RigPolicy {
    configured: bool,
    link: RigLinkView,
    caps: Option<RigCaps>,
    freq: Option<Stamped<u64>>,
    mode: Option<(String, i32)>,
    vfo: Option<String>,
    split: Option<bool>,
    rf_power: Option<Stamped<f32>>,
    meters: [Option<Stamped<f32>>; 3],
    rig_ptt: RigPttView,
    unknown_since: Option<Instant>,
    mercury_ptt: Option<bool>,
    activity: Activity,
    check: Check,
    fault: Option<RigBanner>,
    agree: u32,
    banners: Vec<RigBanner>,
    recovery: Recovery,
    owner_check: OwnerCheck,
    tx_level: Option<TxLevelResult>,
}

impl RigPolicy {
    /// `configured`: a rigctld is set up for this profile.
    pub fn new(configured: bool) -> RigPolicy {
        RigPolicy {
            configured,
            link: if configured { RigLinkView::Connecting } else { RigLinkView::Unconfigured },
            caps: None,
            freq: None,
            mode: None,
            vfo: None,
            split: None,
            rf_power: None,
            meters: [None, None, None],
            rig_ptt: RigPttView::Unknown,
            unknown_since: None,
            mercury_ptt: None,
            activity: Activity::Idle,
            check: Check::None,
            fault: None,
            agree: 0,
            banners: vec![],
            recovery: Recovery::default(),
            owner_check: OwnerCheck::Unknown,
            tx_level: None,
        }
    }

    pub fn configured(&self) -> bool {
        self.configured
    }
    pub fn link_up(&self) -> bool {
        self.link == RigLinkView::Up
    }
    pub fn caps(&self) -> Option<&RigCaps> {
        self.caps.as_ref()
    }
    pub fn rig_ptt(&self) -> RigPttView {
        self.rig_ptt
    }
    pub fn mercury_ptt(&self) -> Option<bool> {
        self.mercury_ptt
    }
    /// The rig's frequency and when it was read.
    pub fn frequency(&self) -> Option<(u64, Instant)> {
        self.freq.as_ref().map(|f| (f.value, f.at))
    }
    pub fn set_activity(&mut self, a: Activity) {
        self.activity = a;
    }
    pub fn activity(&self) -> Activity {
        self.activity
    }
    pub fn set_owner_check(&mut self, c: OwnerCheck) {
        self.owner_check = c;
    }
    pub fn owner_check(&self) -> &OwnerCheck {
        &self.owner_check
    }
    pub fn set_tx_level(&mut self, r: Option<TxLevelResult>) {
        self.tx_level = r;
    }
    pub fn tx_level(&self) -> Option<&TxLevelResult> {
        self.tx_level.as_ref()
    }
    /// A successful RF power set (rigctld does not report it unasked).
    pub fn set_rf_power(&mut self, level: f32, now: Instant) {
        self.rf_power = Some(Stamped::new(level, now));
    }

    /// How fast the client should read PTT.
    pub fn poll_rate(&self, activity: Activity) -> PollRate {
        if activity != Activity::Idle || self.mercury_ptt == Some(true) {
            PollRate::Active
        } else {
            PollRate::Idle
        }
    }

    /// Mercury's `PTT ON` / `PTT OFF` control event, as it arrives.
    pub fn on_mercury_ptt(&mut self, on: bool, now: Instant) -> Vec<RigAction> {
        self.mercury_ptt = Some(on);
        self.check = Check::Pending { want: on, since: now };
        let r = &mut self.recovery;
        if !on && r.awaiting_mercury_off {
            r.awaiting_mercury_off = false;
            r.check_after = Some(now + PTT_WINDOW);
        }
        vec![]
    }

    /// Something the rigctld client reported.
    pub fn on_msg(&mut self, msg: &RigMsg, now: Instant) -> Vec<RigAction> {
        match msg {
            RigMsg::Link(RigLink::Connecting) => {
                self.link = RigLinkView::Connecting;
                vec![]
            }
            RigMsg::Link(RigLink::Up) => self.link_up_again(),
            RigMsg::Link(RigLink::Lost { reason }) => self.link_lost(reason, now),
            RigMsg::Caps(c) => {
                self.caps = Some(c.clone());
                vec![]
            }
            RigMsg::Freq(hz) => {
                self.freq = Some(Stamped::new(*hz, now));
                vec![]
            }
            RigMsg::Mode { mode, passband_hz } => {
                self.mode = Some((mode.clone(), *passband_hz));
                vec![]
            }
            RigMsg::Vfo(v) => {
                self.vfo = Some(v.clone());
                vec![]
            }
            RigMsg::Split { on, .. } => {
                self.split = Some(*on);
                vec![]
            }
            RigMsg::Meter(level, v) => {
                let i = match level {
                    Level::Swr => 0,
                    Level::Alc => 1,
                    Level::RfPowerMeter => 2,
                    Level::RfPower => {
                        self.rf_power = Some(Stamped::new(*v, now));
                        return vec![];
                    }
                };
                self.meters[i] = Some(Stamped::new(*v, now));
                vec![]
            }
            RigMsg::Ptt(PttReading::Unknown { .. }) => {
                self.rig_ptt = RigPttView::Unknown;
                self.unknown_since.get_or_insert(now);
                vec![]
            }
            RigMsg::Ptt(PttReading::On) => self.ptt_reading(true, now),
            RigMsg::Ptt(PttReading::Off) => self.ptt_reading(false, now),
            RigMsg::Done { .. } => vec![],
        }
    }

    /// Timers: nothing fires on time alone today (an unanswered window is
    /// unknown, not a fault), but the actor ticks so a future rule can.
    pub fn tick(&mut self, _now: Instant) -> Vec<RigAction> {
        vec![]
    }

    fn ptt_reading(&mut self, keyed: bool, now: Instant) -> Vec<RigAction> {
        self.rig_ptt = if keyed { RigPttView::On } else { RigPttView::Off };
        self.unknown_since = None;
        let mut actions = vec![];

        // The cross-check.
        let contradicts = match self.check {
            Check::None => false,
            Check::Pending { want, since } => {
                if keyed == want {
                    self.check = Check::Agreed { want };
                    false
                } else {
                    now.saturating_duration_since(since) >= PTT_WINDOW
                }
            }
            Check::Agreed { want } => keyed != want,
        };
        if contradicts {
            self.agree = 0;
            if self.fault.is_none() {
                self.fault = Some(RigBanner::PttNotFollowing);
                self.raise(RigBanner::PttNotFollowing, &mut actions);
                // After a rigctld restart, a Mercury that says it keys while
                // the rig stays unkeyed has lost its rig session (L19).
                if self.recovery.recovered && self.mercury_ptt == Some(true) && !keyed {
                    self.raise(RigBanner::RestartModem, &mut actions);
                }
            }
        } else if self.fault.is_some() && matches!(self.check, Check::Agreed { .. }) {
            self.agree += 1;
            if self.agree >= AGREE_TO_CLEAR {
                self.fault = None;
                self.agree = 0;
            }
        }

        // The one PTT-off after a recovery that waited for Mercury's OFF.
        if let Some(after) = self.recovery.check_after {
            if now >= after {
                self.recovery.check_after = None;
                if keyed && !self.recovery.ptt_off_sent {
                    self.recovery.ptt_off_sent = true;
                    actions.push(RigAction::SendPttOff);
                }
            }
        }
        if !keyed && self.recovery.check_after.is_none() && !self.recovery.awaiting_mercury_off {
            self.recovery.lost_while_keyed = false;
        }
        actions
    }

    fn link_lost(&mut self, reason: &str, now: Instant) -> Vec<RigAction> {
        let was_keyed = self.rig_ptt == RigPttView::On
            || self.mercury_ptt == Some(true)
            || (self.rig_ptt == RigPttView::Unknown && self.activity != Activity::Idle);
        self.link = RigLinkView::Lost { since_utc: now_utc(), reason: reason.to_string() };
        self.rig_ptt = RigPttView::Unknown;
        self.unknown_since = None;
        self.meters = [None, None, None];
        self.recovery =
            Recovery { lost_while_keyed: was_keyed, recovered: self.recovery.recovered, ..Recovery::default() };
        let _ = now;
        let mut actions = vec![];
        if was_keyed {
            self.raise(RigBanner::LostWhileKeyed, &mut actions);
        }
        actions
    }

    fn link_up_again(&mut self) -> Vec<RigAction> {
        let was_lost = matches!(self.link, RigLinkView::Lost { .. });
        self.link = RigLinkView::Up;
        if !was_lost {
            return vec![];
        }
        self.recovery.recovered = true;
        let r = &mut self.recovery;
        if !r.lost_while_keyed || r.ptt_off_sent {
            return vec![];
        }
        if self.mercury_ptt == Some(true) {
            // Mercury may be keying again by itself (patch 0003): never cut it.
            r.awaiting_mercury_off = true;
            vec![]
        } else {
            r.ptt_off_sent = true;
            vec![RigAction::SendPttOff]
        }
    }

    fn raise(&mut self, b: RigBanner, actions: &mut Vec<RigAction>) {
        if !self.banners.contains(&b) {
            self.banners.push(b);
            actions.push(RigAction::Banner(b));
        }
    }

    /// Dismiss a banner, if it may go now: the PTT fault once the condition
    /// has cleared, the lost-while-keyed one once the rig reads unkeyed on a
    /// live link. Returns whether it went.
    pub fn dismiss(&mut self, b: RigBanner) -> bool {
        let may = match b {
            RigBanner::PttNotFollowing => self.fault.is_none(),
            RigBanner::LostWhileKeyed => self.link_up() && self.rig_ptt == RigPttView::Off,
            RigBanner::RestartModem => true,
        };
        if may {
            self.banners.retain(|x| *x != b);
        }
        may
    }

    pub fn view(&self, now: Instant) -> RigView {
        let caps = self.caps.as_ref().map(|c| RigCapsView {
            model: c.model.clone(),
            can_set_freq: c.can_set_freq,
            can_set_mode: c.can_set_mode,
            can_set_vfo: c.can_set_vfo,
            can_split: c.can_split,
            can_get_ptt: c.can_get_ptt,
            can_set_power: c.set_levels.contains(&Level::RfPower),
            meters: [(Level::Swr, "swr"), (Level::Alc, "alc"), (Level::RfPowerMeter, "power")]
                .into_iter()
                .filter(|(l, _)| c.get_levels.contains(l))
                .map(|(_, n)| n.to_string())
                .collect(),
            modes: c.modes.clone(),
        });
        let meter = |i: usize| self.meters[i].as_ref().map(|m| m.reading(now));
        RigView {
            link: self.link.clone(),
            frequency: self.freq.as_ref().map(|f| f.reading(now)),
            mode: self.mode.as_ref().map(|m| m.0.clone()),
            passband_hz: self.mode.as_ref().map(|m| m.1),
            vfo: self.vfo.clone(),
            split: self.split,
            rf_power: self.rf_power.as_ref().map(|p| p.reading(now)),
            ptt: self.rig_ptt,
            ptt_check_off: self.link_up()
                && self.unknown_since.is_some_and(|s| now.saturating_duration_since(s) >= PTT_CHECK_OFF_AFTER),
            fault: self.fault,
            banners: self.banners.clone(),
            meters: RigMeters { swr: meter(0), alc: meter(1), power: meter(2) },
            caps,
            owner_check: self.owner_check.clone(),
            tx_level_applied: self.tx_level.clone(),
        }
    }
}

/// May `change` be made now? Checked by the actor at the moment it forwards
/// the change, so a change and a call start cannot both win.
/// Mercury's tuning carrier as the UI shows it (M3b, RIG-05).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TuneView {
    /// Does this Mercury have TUNE: `null` until it answers after attach.
    pub supported: Option<bool>,
    pub active: bool,
    /// When MercuryChat stops it, while it is on.
    pub ends_utc: Option<String>,
    /// The level it runs at, dBFS at the sound card.
    pub level_dbfs: f32,
}

// The level is validated finite before use, so equality is total.
impl Eq for TuneView {}

impl Default for TuneView {
    fn default() -> Self {
        TuneView { supported: None, active: false, ends_utc: None, level_dbfs: -20.0 }
    }
}

pub fn rig_gate(activity: Activity, rig: &RigPolicy, change: &RigChange, attached: bool) -> Result<(), RigRefusal> {
    match activity {
        Activity::Idle => {}
        Activity::Calling => return Err(RigRefusal::Calling),
        Activity::Connected => return Err(RigRefusal::Connected),
        Activity::Draining => return Err(RigRefusal::Draining),
        Activity::Broadcasting => return Err(RigRefusal::Broadcasting),
        Activity::Tuning => return Err(RigRefusal::Tuning),
    }
    if rig.rig_ptt == RigPttView::On || rig.mercury_ptt == Some(true) {
        return Err(RigRefusal::PttOn);
    }
    if change.needs_rig() && !rig.link_up() {
        return Err(RigRefusal::LinkDown);
    }
    if *change == RigChange::Tune {
        // Without a rigctld there is no PTT reading to check: Tune relies on
        // Mercury's own stop then. With one, an unreadable PTT refuses it.
        if rig.configured && rig.rig_ptt == RigPttView::Unknown {
            return Err(RigRefusal::PttUnknown);
        }
        if !attached {
            return Err(RigRefusal::NotAttached);
        }
    }
    if *change == RigChange::TxLevel && !attached {
        return Err(RigRefusal::NotAttached);
    }
    let caps = rig.caps.as_ref();
    let supported = match change {
        RigChange::Frequency { .. } => caps.is_some_and(|c| c.can_set_freq),
        RigChange::Mode { .. } => caps.is_some_and(|c| c.can_set_mode),
        RigChange::Vfo { .. } => caps.is_some_and(|c| c.can_set_vfo),
        RigChange::Split { .. } => caps.is_some_and(|c| c.can_split),
        RigChange::RfPower { .. } => caps.is_some_and(|c| c.set_levels.contains(&Level::RfPower)),
        RigChange::TxLevel | RigChange::Tune => true,
    };
    if !supported {
        return Err(RigRefusal::Unsupported);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mercury_adapter::rigctl::RigCaps;

    const MS: Duration = Duration::from_millis(1);

    fn full_caps() -> RigCaps {
        RigCaps {
            model: "Dummy".into(),
            can_set_freq: true,
            can_set_mode: true,
            can_set_vfo: true,
            can_split: true,
            can_get_ptt: true,
            get_levels: Level::ALL.to_vec(),
            set_levels: vec![Level::RfPower],
            modes: vec!["PKTUSB".into()],
        }
    }

    /// A policy with the link up and the rig reading `ptt`.
    fn up(ptt: Option<bool>, t0: Instant) -> RigPolicy {
        let mut p = RigPolicy::new(true);
        p.on_msg(&RigMsg::Caps(full_caps()), t0);
        p.on_msg(&RigMsg::Link(RigLink::Up), t0);
        match ptt {
            Some(true) => p.on_msg(&RigMsg::Ptt(PttReading::On), t0),
            Some(false) => p.on_msg(&RigMsg::Ptt(PttReading::Off), t0),
            None => p.on_msg(&RigMsg::Ptt(PttReading::Unknown { reason: "RPRT -11".into() }), t0),
        };
        p
    }

    fn reading(p: &mut RigPolicy, on: bool, at: Instant) -> Vec<RigAction> {
        p.on_msg(&RigMsg::Ptt(if on { PttReading::On } else { PttReading::Off }), at)
    }

    fn unknown(p: &mut RigPolicy, at: Instant) -> Vec<RigAction> {
        p.on_msg(&RigMsg::Ptt(PttReading::Unknown { reason: "no reply within 200 ms".into() }), at)
    }

    fn lost(p: &mut RigPolicy, at: Instant) -> Vec<RigAction> {
        p.on_msg(&RigMsg::Link(RigLink::Lost { reason: "closed".into() }), at)
    }

    fn back(p: &mut RigPolicy, at: Instant) -> Vec<RigAction> {
        p.on_msg(&RigMsg::Link(RigLink::Up), at)
    }

    fn changes() -> Vec<RigChange> {
        vec![
            RigChange::Frequency { hz: 7_105_000 },
            RigChange::Mode { mode: "PKTUSB".into(), passband_hz: 3000 },
            RigChange::Vfo { vfo: "VFOB".into() },
            RigChange::Split { on: true, tx_vfo: "VFOB".into() },
            RigChange::RfPower { level: 0.5 },
            RigChange::TxLevel,
            RigChange::Tune,
        ]
    }

    /// The rules restated independently of the gate (Global Constraints).
    fn expected(a: Activity, ptt: Option<bool>, link_up: bool, change: &RigChange) -> Result<(), RigRefusal> {
        let busy = match a {
            Activity::Idle => None,
            Activity::Calling => Some(RigRefusal::Calling),
            Activity::Connected => Some(RigRefusal::Connected),
            Activity::Draining => Some(RigRefusal::Draining),
            Activity::Broadcasting => Some(RigRefusal::Broadcasting),
            Activity::Tuning => Some(RigRefusal::Tuning),
        };
        if let Some(r) = busy {
            return Err(r);
        }
        if ptt == Some(true) && link_up {
            return Err(RigRefusal::PttOn);
        }
        let through_mercury = matches!(change, RigChange::TxLevel | RigChange::Tune);
        if !through_mercury && !link_up {
            return Err(RigRefusal::LinkDown);
        }
        if *change == RigChange::Tune && (ptt.is_none() || !link_up) {
            return Err(RigRefusal::PttUnknown);
        }
        Ok(())
    }

    #[test]
    fn gate_table() {
        let t0 = Instant::now();
        let activities = [
            Activity::Idle,
            Activity::Calling,
            Activity::Connected,
            Activity::Draining,
            Activity::Broadcasting,
            Activity::Tuning,
        ];
        for a in activities {
            for ptt in [Some(true), Some(false), None] {
                for link_up in [true, false] {
                    for change in changes() {
                        let mut p = up(ptt, t0);
                        if !link_up {
                            lost(&mut p, t0);
                        }
                        assert_eq!(
                            rig_gate(a, &p, &change, true),
                            expected(a, ptt, link_up, &change),
                            "{a:?} ptt={ptt:?} link_up={link_up} {change:?}"
                        );
                    }
                }
            }
        }
        // Spot checks from the plan.
        let idle_unknown = up(None, t0);
        assert_eq!(rig_gate(Activity::Idle, &idle_unknown, &RigChange::Frequency { hz: 1 }, true), Ok(()));
        assert_eq!(rig_gate(Activity::Idle, &idle_unknown, &RigChange::Tune, true), Err(RigRefusal::PttUnknown));
        let idle_off = up(Some(false), t0);
        assert_eq!(rig_gate(Activity::Idle, &idle_off, &RigChange::Tune, false), Err(RigRefusal::NotAttached));
        // Mercury saying PTT ON refuses too, whatever the rig reads.
        let mut merc_on = up(Some(false), t0);
        merc_on.on_mercury_ptt(true, t0);
        assert_eq!(rig_gate(Activity::Idle, &merc_on, &RigChange::Frequency { hz: 1 }, true), Err(RigRefusal::PttOn));
    }

    #[test]
    fn unsupported_changes_are_refused() {
        let t0 = Instant::now();
        let mut p = RigPolicy::new(true);
        p.on_msg(
            &RigMsg::Caps(RigCaps { can_split: false, set_levels: vec![], can_set_vfo: false, ..full_caps() }),
            t0,
        );
        p.on_msg(&RigMsg::Link(RigLink::Up), t0);
        reading(&mut p, false, t0);
        for c in [
            RigChange::Split { on: true, tx_vfo: "VFOB".into() },
            RigChange::RfPower { level: 0.5 },
            RigChange::Vfo { vfo: "VFOB".into() },
        ] {
            assert_eq!(rig_gate(Activity::Idle, &p, &c, true), Err(RigRefusal::Unsupported), "{c:?}");
        }
        assert_eq!(rig_gate(Activity::Idle, &p, &RigChange::Frequency { hz: 7_000_000 }, true), Ok(()));
    }

    #[test]
    fn without_a_rigctld_only_mercury_changes_pass() {
        let p = RigPolicy::new(false);
        assert_eq!(p.view(Instant::now()).link, RigLinkView::Unconfigured);
        assert_eq!(rig_gate(Activity::Idle, &p, &RigChange::Frequency { hz: 1 }, true), Err(RigRefusal::LinkDown));
        assert_eq!(rig_gate(Activity::Idle, &p, &RigChange::Tune, true), Ok(()), "Tune relies on Mercury's stop");
        assert_eq!(rig_gate(Activity::Idle, &p, &RigChange::TxLevel, true), Ok(()));
        assert_eq!(rig_gate(Activity::Idle, &p, &RigChange::TxLevel, false), Err(RigRefusal::NotAttached));
    }

    #[test]
    fn ptt_on_seen_within_window_is_fine() {
        let t0 = Instant::now();
        let mut p = up(Some(false), t0);
        p.on_mercury_ptt(true, t0);
        assert!(reading(&mut p, false, t0 + 100 * MS).is_empty(), "not yet: inside the window");
        assert!(reading(&mut p, true, t0 + 300 * MS).is_empty());
        assert!(reading(&mut p, true, t0 + 900 * MS).is_empty());
        assert_eq!(p.view(t0).fault, None);
    }

    #[test]
    fn ptt_on_not_followed_faults_after_window() {
        let t0 = Instant::now();
        let mut p = up(Some(false), t0);
        p.on_mercury_ptt(true, t0);
        assert!(reading(&mut p, false, t0 + 400 * MS).is_empty());
        let a = reading(&mut p, false, t0 + 600 * MS);
        assert_eq!(a, vec![RigAction::Banner(RigBanner::PttNotFollowing)]);
        assert_eq!(p.view(t0).fault, Some(RigBanner::PttNotFollowing));
        assert!(reading(&mut p, false, t0 + 850 * MS).is_empty(), "raised once");
        assert!(!p.view(t0).banners.contains(&RigBanner::RestartModem), "no recovery happened");
    }

    #[test]
    fn rig_still_keyed_after_off_faults() {
        let t0 = Instant::now();
        let mut p = up(Some(true), t0);
        p.on_mercury_ptt(true, t0);
        reading(&mut p, true, t0 + 100 * MS);
        p.on_mercury_ptt(false, t0 + 1000 * MS);
        assert!(reading(&mut p, true, t0 + 1300 * MS).is_empty());
        assert_eq!(reading(&mut p, true, t0 + 1600 * MS), vec![RigAction::Banner(RigBanner::PttNotFollowing)]);
    }

    #[test]
    fn a_rig_that_keys_by_itself_after_agreeing_faults() {
        let t0 = Instant::now();
        let mut p = up(Some(false), t0);
        p.on_mercury_ptt(false, t0);
        reading(&mut p, false, t0 + 100 * MS);
        assert_eq!(reading(&mut p, true, t0 + 5000 * MS), vec![RigAction::Banner(RigBanner::PttNotFollowing)]);
    }

    #[test]
    fn timeouts_inside_the_window_never_fault() {
        let t0 = Instant::now();
        let mut p = up(Some(false), t0);
        p.on_mercury_ptt(true, t0);
        for i in 1..=8 {
            assert!(unknown(&mut p, t0 + i * 250 * MS).is_empty());
            assert!(p.tick(t0 + i * 250 * MS).is_empty());
        }
        let v = p.view(t0 + 2000 * MS);
        assert_eq!(v.ptt, RigPttView::Unknown);
        assert_eq!(v.fault, None);
    }

    #[test]
    fn fault_clears_after_two_agreeing_readings_but_banner_stays_until_dismissed() {
        let t0 = Instant::now();
        let mut p = up(Some(false), t0);
        p.on_mercury_ptt(true, t0);
        reading(&mut p, false, t0 + 600 * MS);
        assert!(!p.dismiss(RigBanner::PttNotFollowing), "not while the fault holds");
        reading(&mut p, true, t0 + 850 * MS);
        assert_eq!(p.view(t0).fault, Some(RigBanner::PttNotFollowing), "one agreeing reading is not enough");
        reading(&mut p, true, t0 + 1100 * MS);
        let v = p.view(t0);
        assert_eq!(v.fault, None);
        assert_eq!(v.banners, vec![RigBanner::PttNotFollowing], "the banner stays until dismissed");
        assert!(p.dismiss(RigBanner::PttNotFollowing));
        assert!(p.view(t0).banners.is_empty());
    }

    #[test]
    fn permanent_unknown_is_a_warning_not_a_fault() {
        let t0 = Instant::now();
        let mut p = up(None, t0);
        for i in 1..=40 {
            assert!(unknown(&mut p, t0 + i * 250 * MS).is_empty());
        }
        let v = p.view(t0 + 10_000 * MS);
        assert!(v.ptt_check_off);
        assert_eq!(v.fault, None);
        assert!(v.banners.is_empty());
        assert!(!up(None, t0).view(t0 + 1000 * MS).ptt_check_off, "not after a moment");
        reading(&mut p, false, t0 + 10_250 * MS);
        assert!(!p.view(t0 + 10_250 * MS).ptt_check_off);
    }

    #[test]
    fn loss_while_keyed_raises_critical_and_recovery_sends_one_ptt_off() {
        let t0 = Instant::now();
        let mut p = up(Some(true), t0);
        p.on_mercury_ptt(true, t0);
        reading(&mut p, true, t0 + 100 * MS);
        p.on_mercury_ptt(false, t0 + 200 * MS);
        assert_eq!(lost(&mut p, t0 + 300 * MS), vec![RigAction::Banner(RigBanner::LostWhileKeyed)]);
        assert!(!p.dismiss(RigBanner::LostWhileKeyed), "not while the rig may be keyed");
        assert_eq!(back(&mut p, t0 + 2000 * MS), vec![RigAction::SendPttOff]);
        assert!(reading(&mut p, false, t0 + 2100 * MS).is_empty());
        assert!(p.dismiss(RigBanner::LostWhileKeyed), "safe once the rig reads unkeyed");
        // A second cycle while idle: nothing.
        assert!(lost(&mut p, t0 + 3000 * MS).is_empty());
        assert!(back(&mut p, t0 + 4000 * MS).is_empty());
    }

    #[test]
    fn recovery_waits_for_mercury_ptt_off() {
        let t0 = Instant::now();
        let mut p = up(Some(true), t0);
        p.on_mercury_ptt(true, t0);
        reading(&mut p, true, t0 + 100 * MS);
        lost(&mut p, t0 + 200 * MS);
        assert!(back(&mut p, t0 + 1000 * MS).is_empty(), "never while Mercury transmits");
        assert!(!reading(&mut p, true, t0 + 1100 * MS).contains(&RigAction::SendPttOff));
        p.on_mercury_ptt(false, t0 + 2000 * MS);
        assert!(!reading(&mut p, true, t0 + 2200 * MS).contains(&RigAction::SendPttOff), "inside the window");
        let a = reading(&mut p, true, t0 + 2600 * MS);
        assert_eq!(a.iter().filter(|x| **x == RigAction::SendPttOff).count(), 1, "{a:?}");
        assert!(!reading(&mut p, true, t0 + 2900 * MS).contains(&RigAction::SendPttOff), "only once");

        // The same, but the rig unkeys with Mercury: no PTT-off at all.
        let mut q = up(Some(true), t0);
        q.on_mercury_ptt(true, t0);
        lost(&mut q, t0 + 200 * MS);
        back(&mut q, t0 + 1000 * MS);
        q.on_mercury_ptt(false, t0 + 2000 * MS);
        assert!(reading(&mut q, false, t0 + 2600 * MS).is_empty());
        assert!(reading(&mut q, false, t0 + 2900 * MS).is_empty());
    }

    #[test]
    fn loss_while_idle_degrades_without_banner() {
        let t0 = Instant::now();
        let mut p = up(Some(false), t0);
        p.on_mercury_ptt(false, t0);
        assert!(lost(&mut p, t0 + 100 * MS).is_empty());
        let v = p.view(t0);
        assert!(matches!(v.link, RigLinkView::Lost { .. }));
        assert!(v.banners.is_empty());
        assert!(back(&mut p, t0 + 1000 * MS).is_empty());
    }

    #[test]
    fn unknown_during_activity_counts_as_keyed_at_loss() {
        let t0 = Instant::now();
        let mut p = up(None, t0);
        p.set_activity(Activity::Connected);
        assert_eq!(lost(&mut p, t0), vec![RigAction::Banner(RigBanner::LostWhileKeyed)]);
    }

    #[test]
    fn mercury_on_rig_off_after_restart_suggests_modem_restart() {
        let t0 = Instant::now();
        let mut p = up(Some(false), t0);
        p.on_mercury_ptt(false, t0);
        lost(&mut p, t0 + 100 * MS);
        back(&mut p, t0 + 500 * MS);
        p.on_mercury_ptt(true, t0 + 1000 * MS);
        let a = reading(&mut p, false, t0 + 1600 * MS);
        assert_eq!(a, vec![RigAction::Banner(RigBanner::PttNotFollowing), RigAction::Banner(RigBanner::RestartModem)]);
    }

    #[test]
    fn poll_rate_follows_activity() {
        let t0 = Instant::now();
        let mut p = up(Some(false), t0);
        assert_eq!(p.poll_rate(Activity::Idle), PollRate::Idle);
        for a in [Activity::Calling, Activity::Connected, Activity::Draining, Activity::Broadcasting, Activity::Tuning]
        {
            assert_eq!(p.poll_rate(a), PollRate::Active, "{a:?}");
        }
        p.on_mercury_ptt(true, t0);
        assert_eq!(p.poll_rate(Activity::Idle), PollRate::Active, "Mercury keying");
        p.on_mercury_ptt(false, t0);
        assert_eq!(p.poll_rate(Activity::Idle), PollRate::Idle);
    }

    #[test]
    fn readings_reach_the_view_with_source_and_age() {
        let t0 = Instant::now();
        let mut p = up(Some(false), t0);
        p.on_msg(&RigMsg::Freq(7_105_000), t0);
        p.on_msg(&RigMsg::Mode { mode: "PKTUSB".into(), passband_hz: 3000 }, t0);
        p.on_msg(&RigMsg::Meter(Level::Swr, 1.3), t0);
        let v = p.view(t0 + 1500 * MS);
        let f = v.frequency.unwrap();
        assert_eq!((f.value, f.source.as_str(), f.age_ms), (7_105_000, RIG_SOURCE, Some(1500)));
        assert_eq!(v.mode.as_deref(), Some("PKTUSB"));
        assert_eq!(v.passband_hz, Some(3000));
        assert_eq!(v.meters.swr.unwrap().value, 1.3);
        let caps = v.caps.unwrap();
        assert!(caps.can_set_power);
        assert_eq!(caps.meters, ["swr", "alc", "power"]);
    }

    #[test]
    fn refusals_have_keys_and_messages() {
        let all = [
            RigRefusal::Calling,
            RigRefusal::Connected,
            RigRefusal::Draining,
            RigRefusal::Tuning,
            RigRefusal::Broadcasting,
            RigRefusal::PttOn,
            RigRefusal::PttUnknown,
            RigRefusal::LinkDown,
            RigRefusal::Unsupported,
            RigRefusal::NotAttached,
        ];
        let keys: std::collections::HashSet<_> = all.iter().map(|r| r.key()).collect();
        assert_eq!(keys.len(), all.len());
        assert!(all.iter().all(|r| r.key().starts_with("rig.refused.") && !r.message().is_empty()));
    }
}
