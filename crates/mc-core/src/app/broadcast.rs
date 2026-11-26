//! The broadcast gate (DSC-02, docs/03 §6): a CQ or a beacon goes out only
//! while the modem is idle, a beacon never above the listen mode's wrapped
//! capacity, and what the operator is told is the truth: *transmitted* on the
//! sender's own evidence, never *delivered*.
//!
//! The actor refuses from its own state first ([`broadcast_gate`]); the modem
//! task checks again, so a command it has not yet seen cannot slip through.
//! Nothing refused is ever queued for later.

use super::events::ModemState;
use crate::station::Observation;
use mercury_adapter::kiss::wrapped_capacity;
use serde::Serialize;
use std::time::Duration;
use tokio::time::Instant;
use ts_rs::TS;

/// How long after submission the sender's evidence (PENDING…CANCELPENDING for
/// a CQ, PTT ON then OFF for a beacon) may take before the broadcast is
/// reported as not confirmed.
pub const TRANSMIT_WINDOW: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum BroadcastKind {
    /// Mercury's own `CQFRAME`.
    Cq,
    /// An MCB/1 beacon on the broadcast port.
    Beacon,
    /// An MCB/1 QSY suggestion (kind 3) on the broadcast port (M3c B5).
    Qsy,
    /// The operator's general broadcast, `CALL: message`, on the broadcast
    /// port.
    Message,
}

impl BroadcastKind {
    pub fn label(self) -> &'static str {
        match self {
            BroadcastKind::Cq => "CQ",
            BroadcastKind::Beacon => "beacon",
            BroadcastKind::Qsy => "QSY suggestion",
            BroadcastKind::Message => "message",
        }
    }
}

/// What became of one broadcast. There is no delivered state: nobody
/// acknowledges a CQ or a beacon (DSC-02).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(tag = "type", rename_all = "camelCase")]
#[ts(export)]
pub enum BroadcastOutcome {
    /// The modem took it.
    Submitted,
    /// The sender's evidence followed within 15 s while idle.
    Transmitted,
    /// No such evidence within 15 s, or the modem left idle first.
    /// `reason`, when there is more to say than that.
    NotConfirmed { reason: Option<NotConfirmedReason> },
    /// Not sent, and not queued: `reason` says why, for the operator.
    Refused { reason: String },
}

impl BroadcastOutcome {
    /// Not confirmed, with nothing more to say.
    pub const NOT_CONFIRMED: BroadcastOutcome = BroadcastOutcome::NotConfirmed { reason: None };
}

/// Why a broadcast was not confirmed, beyond "no evidence".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum NotConfirmedReason {
    /// The broadcast port took a beacon but the modem never keyed (no PTT
    /// at all in the window, while idle). Mercury serves one broadcast
    /// client: a second one connects into the OS backlog and its writes go
    /// nowhere (lab Q5), so another program may hold the port. This cannot
    /// be seen at connect time, only from the outcome.
    BroadcastPortSilent,
}

impl NotConfirmedReason {
    /// The message key the UI translates.
    pub fn key(self) -> &'static str {
        match self {
            NotConfirmedReason::BroadcastPortSilent => "broadcast.notConfirmed.broadcastPortSilent",
        }
    }

    /// The reason in plain English, for the activity log and the beacon status.
    pub fn text(self) -> &'static str {
        match self {
            NotConfirmedReason::BroadcastPortSilent => {
                "the modem did not transmit; another program may be using Mercury's broadcast port"
            }
        }
    }
}

/// Why the gate refused a broadcast.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateRefusal {
    NotIdle(ModemState),
    /// A call is being placed (from the moment it was asked for).
    Calling,
    /// A beacon needs a known listen mode; a CQ does not.
    UnknownListenMode,
    OverCapacity {
        len: usize,
        capacity: usize,
    },
    NoBroadcastPort,
    Detached,
}

impl GateRefusal {
    /// The refusal in plain words for the operator.
    pub fn message(&self) -> String {
        let why = match self {
            GateRefusal::Calling => "a call is being placed".to_string(),
            GateRefusal::NotIdle(ModemState::Negotiating | ModemState::Connected | ModemState::Draining) => {
                "the modem is busy with a session".to_string()
            }
            GateRefusal::NotIdle(ModemState::Unavailable) => "no modem is available".to_string(),
            GateRefusal::NotIdle(ModemState::Attaching) => "the modem is still attaching".to_string(),
            GateRefusal::NotIdle(ModemState::Recovering) => "the modem is recovering".to_string(),
            GateRefusal::NotIdle(ModemState::Faulted) => "the modem is faulted".to_string(),
            GateRefusal::NotIdle(other) => format!("the modem is {}", format!("{other:?}").to_lowercase()),
            GateRefusal::UnknownListenMode => "listen mode unknown".to_string(),
            GateRefusal::OverCapacity { len, capacity } => {
                format!("{len} bytes is over the {capacity}-byte limit")
            }
            GateRefusal::NoBroadcastPort => "the broadcast port is not open".to_string(),
            GateRefusal::Detached => "the modem is not attached".to_string(),
        };
        format!("not sent: {why}")
    }
}

impl std::fmt::Display for GateRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message())
    }
}

/// May a broadcast go now? `len` is a beacon's payload length (`None` for a
/// CQ, which needs neither a listen mode nor the broadcast port).
pub fn broadcast_gate(
    state: ModemState,
    call_requested: bool,
    len: Option<usize>,
    listen_mode: Option<u8>,
) -> Result<(), GateRefusal> {
    if call_requested || state == ModemState::Calling {
        return Err(GateRefusal::Calling);
    }
    if !matches!(state, ModemState::Ready | ModemState::Listening) {
        return Err(GateRefusal::NotIdle(state));
    }
    let Some(len) = len else { return Ok(()) };
    let capacity = listen_mode.and_then(wrapped_capacity).ok_or(GateRefusal::UnknownListenMode)?;
    if len > capacity {
        return Err(GateRefusal::OverCapacity { len, capacity });
    }
    Ok(())
}

/// The modem's listen mode, as far as the actor knows it. It comes from
/// the attach (`MODE 1` accepted or not); a live source (telemetry) is
/// preferred once one reports.
#[derive(Debug, Clone, Copy, Default)]
pub struct ListenMode {
    attach: Option<u8>,
    live: Option<u8>,
}

impl ListenMode {
    pub fn current(&self) -> Option<u8> {
        self.live.or(self.attach)
    }

    /// What the modem task found at (re)attach.
    pub fn set_attach(&mut self, mode: Option<u8>) {
        self.attach = mode;
    }

    /// What telemetry reports now (ruling P2): `None` while it is lost or
    /// reports none (-1), and the attach value applies again.
    pub fn set_live(&mut self, mode: Option<u8>) {
        self.live = mode;
    }
}

/// The one broadcast awaiting its outcome.
#[derive(Debug, Clone, Copy)]
struct InFlight {
    kind: BroadcastKind,
    deadline: Instant,
    /// The modem said it took it.
    submitted: bool,
    /// The first half of the evidence (PENDING, or PTT ON) was seen.
    started: bool,
    /// The modem left idle before the evidence was complete.
    left_idle: bool,
}

/// Tracks one broadcast at a time from the actor's command to its outcome.
/// Each method returns the outcomes to publish, in order.
#[derive(Debug, Default)]
pub struct Tracker {
    in_flight: Option<InFlight>,
    /// A broadcast written to the broadcast connection ended not confirmed
    /// with the modem never seen to key it.
    unkeyed: bool,
}

type Outcomes = Vec<(BroadcastKind, BroadcastOutcome)>;

impl Tracker {
    /// A broadcast is still awaiting its outcome.
    pub fn busy(&self) -> bool {
        self.in_flight.is_some()
    }

    pub fn deadline(&self) -> Option<Instant> {
        self.in_flight.map(|f| f.deadline)
    }

    /// The actor handed `kind` to the modem at `now`.
    pub fn start(&mut self, kind: BroadcastKind, now: Instant) {
        self.in_flight = Some(InFlight {
            kind,
            deadline: now + TRANSMIT_WINDOW,
            submitted: false,
            started: false,
            left_idle: false,
        });
    }

    /// True once after a beacon, QSY suggestion or message ended not
    /// confirmed without the modem ever keying: it may still be waiting
    /// where it can go out later.
    pub fn take_unkeyed(&mut self) -> bool {
        std::mem::take(&mut self.unkeyed)
    }

    fn finish(&mut self, outcome: BroadcastOutcome) -> Outcomes {
        let Some(f) = self.in_flight.take() else { return Vec::new() };
        if f.kind != BroadcastKind::Cq && !f.started && matches!(outcome, BroadcastOutcome::NotConfirmed { .. }) {
            self.unkeyed = true;
        }
        let mut out = Vec::new();
        // Evidence of a transmission is evidence of a submission too.
        if !f.submitted && outcome == BroadcastOutcome::Transmitted {
            out.push((f.kind, BroadcastOutcome::Submitted));
        }
        out.push((f.kind, outcome));
        out
    }

    /// The modem took `kind`.
    pub fn submitted(&mut self, kind: BroadcastKind) -> Outcomes {
        let Some(f) = self.in_flight.as_mut().filter(|f| f.kind == kind && !f.submitted) else {
            return Vec::new();
        };
        f.submitted = true;
        let left = f.left_idle;
        let mut out = vec![(kind, BroadcastOutcome::Submitted)];
        if left {
            out.extend(self.finish(BroadcastOutcome::NOT_CONFIRMED));
        }
        out
    }

    /// The modem refused `kind`; `reason` is for the operator.
    pub fn refused(&mut self, kind: BroadcastKind, reason: String) -> Outcomes {
        if !self.in_flight.is_some_and(|f| f.kind == kind && !f.submitted) {
            return Vec::new();
        }
        self.finish(BroadcastOutcome::Refused { reason })
    }

    /// The modem was sent `kind` but never answered: it may or may not have
    /// gone out, so it is reported as not confirmed, not as refused.
    pub fn unconfirmed(&mut self, kind: BroadcastKind) -> Outcomes {
        if !self.in_flight.is_some_and(|f| f.kind == kind && !f.submitted) {
            return Vec::new();
        }
        self.finish(BroadcastOutcome::NOT_CONFIRMED)
    }

    /// An observation from the modem's control stream.
    pub fn observe(&mut self, o: &Observation) -> Outcomes {
        let Some(f) = self.in_flight.as_mut().filter(|f| !f.left_idle) else { return Vec::new() };
        let (begin, end) = match f.kind {
            BroadcastKind::Cq => (matches!(o, Observation::Pending), matches!(o, Observation::CancelPending)),
            BroadcastKind::Beacon | BroadcastKind::Qsy | BroadcastKind::Message => {
                (matches!(o, Observation::Ptt(true)), matches!(o, Observation::Ptt(false)))
            }
        };
        if begin {
            f.started = true;
        } else if end && f.started {
            return self.finish(BroadcastOutcome::Transmitted);
        }
        Vec::new()
    }

    /// The modem state changed to `state`. Leaving idle ends the window; if
    /// the modem has not answered yet, its answer (or the deadline) decides.
    pub fn state(&mut self, state: ModemState) -> Outcomes {
        if matches!(state, ModemState::Ready | ModemState::Listening) {
            return Vec::new();
        }
        let Some(f) = self.in_flight.as_mut() else { return Vec::new() };
        if f.submitted {
            return self.finish(BroadcastOutcome::NOT_CONFIRMED);
        }
        f.left_idle = true;
        Vec::new()
    }

    /// The window closed at `now` without the evidence. A beacon or message
    /// the broadcast port took (so it was connected) with no PTT at all while
    /// idle is named: the modem never transmitted it.
    pub fn expired(&mut self, now: Instant) -> Outcomes {
        let Some(f) = self.in_flight.filter(|f| now >= f.deadline) else { return Vec::new() };
        let wrapped = matches!(f.kind, BroadcastKind::Beacon | BroadcastKind::Message);
        let silent = wrapped && f.submitted && !f.started && !f.left_idle;
        let reason = silent.then_some(NotConfirmedReason::BroadcastPortSilent);
        self.finish(BroadcastOutcome::NotConfirmed { reason })
    }

    /// The modem task is being replaced: its answer will never come.
    pub fn abandon(&mut self) -> Outcomes {
        self.finish(BroadcastOutcome::NOT_CONFIRMED)
    }
}

/// The activity-log line for an outcome.
pub fn activity_text(kind: BroadcastKind, outcome: &BroadcastOutcome) -> String {
    let what = kind.label();
    match outcome {
        BroadcastOutcome::Submitted => format!("{what} submitted to the modem"),
        BroadcastOutcome::Transmitted => format!("{what} transmitted (no receipt: nobody acknowledges a broadcast)"),
        BroadcastOutcome::NotConfirmed { reason: None } => {
            format!("{what} not confirmed: no transmission seen while idle within {} s", TRANSMIT_WINDOW.as_secs())
        }
        BroadcastOutcome::NotConfirmed { reason: Some(r) } => format!("{what} not confirmed: {}", r.text()),
        BroadcastOutcome::Refused { reason } => format!("{what} {reason}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(out: &Outcomes) -> Vec<BroadcastOutcome> {
        out.iter().map(|(_, o)| o.clone()).collect()
    }

    #[test]
    fn cq_evidence_is_pending_then_cancelpending() {
        let mut t = Tracker::default();
        t.start(BroadcastKind::Cq, Instant::now());
        assert!(t.busy());
        assert_eq!(kinds(&t.submitted(BroadcastKind::Cq)), [BroadcastOutcome::Submitted]);
        // PTT alone is not CQ evidence; CANCELPENDING without PENDING neither.
        assert!(t.observe(&Observation::Ptt(true)).is_empty());
        assert!(t.observe(&Observation::Ptt(false)).is_empty());
        assert!(t.observe(&Observation::CancelPending).is_empty());
        assert!(t.observe(&Observation::Pending).is_empty());
        assert_eq!(kinds(&t.observe(&Observation::CancelPending)), [BroadcastOutcome::Transmitted]);
        assert!(!t.busy());
    }

    #[test]
    fn beacon_evidence_is_ptt_on_then_off() {
        let mut t = Tracker::default();
        t.start(BroadcastKind::Beacon, Instant::now());
        t.submitted(BroadcastKind::Beacon);
        assert!(t.observe(&Observation::Pending).is_empty());
        assert!(t.observe(&Observation::CancelPending).is_empty());
        assert!(t.observe(&Observation::Ptt(true)).is_empty());
        assert_eq!(kinds(&t.observe(&Observation::Ptt(false))), [BroadcastOutcome::Transmitted]);
    }

    #[test]
    fn message_evidence_is_ptt_on_then_off() {
        let mut t = Tracker::default();
        t.start(BroadcastKind::Message, Instant::now());
        t.submitted(BroadcastKind::Message);
        assert!(t.observe(&Observation::Pending).is_empty());
        assert!(t.observe(&Observation::CancelPending).is_empty());
        assert!(t.observe(&Observation::Ptt(true)).is_empty());
        assert_eq!(kinds(&t.observe(&Observation::Ptt(false))), [BroadcastOutcome::Transmitted]);
    }

    #[test]
    fn a_broadcast_never_keyed_is_marked_however_it_ends() {
        let t0 = Instant::now();
        // A beacon, a QSY suggestion and a message all use the broadcast connection.
        for kind in [BroadcastKind::Beacon, BroadcastKind::Qsy, BroadcastKind::Message] {
            let started = || {
                let mut t = Tracker::default();
                t.start(kind, t0);
                t
            };
            let sent = || {
                let mut t = started();
                t.submitted(kind);
                t
            };
            // The window closes (the modem left idle first, or it did not).
            let mut t = sent();
            t.expired(t0 + TRANSMIT_WINDOW);
            assert!(t.take_unkeyed(), "{kind:?}");
            assert!(!t.take_unkeyed(), "{kind:?}: taken once");
            let mut t = sent();
            t.state(ModemState::Negotiating);
            assert!(t.take_unkeyed(), "{kind:?}");
            // The modem answers after leaving idle, or never answers at all.
            let mut t = started();
            t.state(ModemState::Calling);
            t.submitted(kind);
            assert!(t.take_unkeyed(), "{kind:?}");
            let mut t = started();
            t.unconfirmed(kind);
            assert!(t.take_unkeyed(), "{kind:?}");
            let mut t = started();
            t.expired(t0 + TRANSMIT_WINDOW);
            assert!(t.take_unkeyed(), "{kind:?}");

            // Keyed (even if not seen through), transmitted or refused: not marked.
            let mut t = sent();
            t.observe(&Observation::Ptt(true));
            t.state(ModemState::Negotiating);
            assert!(!t.take_unkeyed(), "{kind:?}");
            let mut t = sent();
            t.observe(&Observation::Ptt(true));
            t.observe(&Observation::Ptt(false));
            assert!(!t.take_unkeyed(), "{kind:?}");
            let mut t = started();
            t.refused(kind, "not sent: x".into());
            assert!(!t.take_unkeyed(), "{kind:?}");
        }
        // A CQ is not written to the broadcast connection at all.
        let mut t = Tracker::default();
        t.start(BroadcastKind::Cq, t0);
        t.submitted(BroadcastKind::Cq);
        t.state(ModemState::Negotiating);
        assert!(!t.take_unkeyed());
    }

    #[test]
    fn evidence_before_the_modem_answers_still_reports_submitted_first() {
        let mut t = Tracker::default();
        t.start(BroadcastKind::Beacon, Instant::now());
        t.observe(&Observation::Ptt(true));
        let out = kinds(&t.observe(&Observation::Ptt(false)));
        assert_eq!(out, [BroadcastOutcome::Submitted, BroadcastOutcome::Transmitted]);
        assert!(t.submitted(BroadcastKind::Beacon).is_empty(), "a late answer adds nothing");
    }

    #[test]
    fn no_evidence_within_the_window_is_not_confirmed() {
        let mut t = Tracker::default();
        let t0 = Instant::now();
        t.start(BroadcastKind::Cq, t0);
        t.submitted(BroadcastKind::Cq);
        t.observe(&Observation::Pending);
        assert!(t.expired(t0 + Duration::from_secs(14)).is_empty());
        assert_eq!(t.deadline(), Some(t0 + TRANSMIT_WINDOW));
        assert_eq!(kinds(&t.expired(t0 + TRANSMIT_WINDOW)), [BroadcastOutcome::NOT_CONFIRMED]);
        assert!(!t.busy());
        assert!(t.observe(&Observation::CancelPending).is_empty());
    }

    #[test]
    fn leaving_idle_first_is_not_confirmed() {
        let mut t = Tracker::default();
        t.start(BroadcastKind::Cq, Instant::now());
        t.submitted(BroadcastKind::Cq);
        t.observe(&Observation::Pending);
        assert!(t.state(ModemState::Listening).is_empty(), "still idle");
        assert_eq!(kinds(&t.state(ModemState::Negotiating)), [BroadcastOutcome::NOT_CONFIRMED]);
    }

    #[test]
    fn leaving_idle_before_the_modem_answers_waits_for_its_answer() {
        let mut t = Tracker::default();
        t.start(BroadcastKind::Cq, Instant::now());
        assert!(t.state(ModemState::Negotiating).is_empty());
        // Evidence no longer counts once the modem has left idle.
        t.observe(&Observation::Pending);
        assert!(t.observe(&Observation::CancelPending).is_empty());
        let out = kinds(&t.refused(BroadcastKind::Cq, "not sent: x".into()));
        assert_eq!(out, [BroadcastOutcome::Refused { reason: "not sent: x".into() }]);

        t.start(BroadcastKind::Cq, Instant::now());
        t.state(ModemState::Recovering);
        let out = kinds(&t.submitted(BroadcastKind::Cq));
        assert_eq!(out, [BroadcastOutcome::Submitted, BroadcastOutcome::NOT_CONFIRMED]);
    }

    #[test]
    fn an_unanswered_command_is_not_confirmed_never_refused() {
        let mut t = Tracker::default();
        assert!(t.unconfirmed(BroadcastKind::Cq).is_empty());
        t.start(BroadcastKind::Cq, Instant::now());
        assert!(t.unconfirmed(BroadcastKind::Beacon).is_empty(), "another kind's answer");
        assert_eq!(kinds(&t.unconfirmed(BroadcastKind::Cq)), [BroadcastOutcome::NOT_CONFIRMED]);
        assert!(!t.busy());
    }

    #[test]
    fn abandon_ends_what_is_in_flight() {
        let mut t = Tracker::default();
        assert!(t.abandon().is_empty());
        t.start(BroadcastKind::Beacon, Instant::now());
        assert_eq!(kinds(&t.abandon()), [BroadcastOutcome::NOT_CONFIRMED]);
        assert!(!t.busy());
    }

    #[test]
    fn listen_mode_prefers_a_live_source() {
        let mut m = ListenMode::default();
        assert_eq!(m.current(), None);
        m.set_attach(Some(1));
        assert_eq!(m.current(), Some(1));
        m.set_live(Some(11));
        assert_eq!(m.current(), Some(11));
        // Telemetry lost, or reporting -1: the attach value applies again.
        m.set_live(None);
        assert_eq!(m.current(), Some(1));
    }

    #[test]
    fn no_activity_line_says_delivered() {
        for kind in [BroadcastKind::Cq, BroadcastKind::Beacon] {
            for o in [
                BroadcastOutcome::Submitted,
                BroadcastOutcome::Transmitted,
                BroadcastOutcome::NOT_CONFIRMED,
                BroadcastOutcome::NotConfirmed { reason: Some(NotConfirmedReason::BroadcastPortSilent) },
                BroadcastOutcome::Refused { reason: "not sent: x".into() },
            ] {
                assert!(!activity_text(kind, &o).to_lowercase().contains("deliver"));
            }
        }
    }

    /// The UI places "CQ transmitted" / "Beacon transmitted" rows in the
    /// transcript from these activity lines (Transcript.vue, store.ts
    /// `transmitted`); only a transmitted broadcast may match.
    #[test]
    fn transmitted_lines_keep_the_wording_the_transcript_reads() {
        let reads = |line: &str| {
            let l = line.to_lowercase();
            l.starts_with("cq transmitted") || l.starts_with("beacon transmitted")
        };
        assert!(activity_text(BroadcastKind::Cq, &BroadcastOutcome::Transmitted).starts_with("CQ transmitted"));
        assert!(activity_text(BroadcastKind::Beacon, &BroadcastOutcome::Transmitted).starts_with("beacon transmitted"));
        for kind in [BroadcastKind::Cq, BroadcastKind::Beacon] {
            for o in [
                BroadcastOutcome::Submitted,
                BroadcastOutcome::NOT_CONFIRMED,
                SILENT,
                BroadcastOutcome::Refused { reason: "not sent: x".into() },
            ] {
                assert!(!reads(&activity_text(kind, &o)), "{:?}", o);
            }
        }
    }

    const SILENT: BroadcastOutcome =
        BroadcastOutcome::NotConfirmed { reason: Some(NotConfirmedReason::BroadcastPortSilent) };

    #[test]
    fn a_beacon_taken_but_never_keyed_names_the_broadcast_port() {
        let mut t = Tracker::default();
        let t0 = Instant::now();
        t.start(BroadcastKind::Beacon, t0);
        t.submitted(BroadcastKind::Beacon);
        assert_eq!(kinds(&t.expired(t0 + TRANSMIT_WINDOW)), [SILENT]);
        let line = activity_text(BroadcastKind::Beacon, &SILENT);
        assert!(line.contains("another program may be using Mercury's broadcast port"), "{line}");
        assert_eq!(NotConfirmedReason::BroadcastPortSilent.key(), "broadcast.notConfirmed.broadcastPortSilent");
    }

    #[test]
    fn only_a_silent_idle_beacon_the_port_took_names_the_port() {
        let t0 = Instant::now();
        let end = t0 + TRANSMIT_WINDOW;
        // Keyed but never unkeyed: it did transmit something.
        let mut t = Tracker::default();
        t.start(BroadcastKind::Beacon, t0);
        t.submitted(BroadcastKind::Beacon);
        t.observe(&Observation::Ptt(true));
        assert_eq!(kinds(&t.expired(end)), [BroadcastOutcome::NOT_CONFIRMED]);
        // Never taken: the modem did not answer, the port is not in question.
        t.start(BroadcastKind::Beacon, t0);
        assert_eq!(kinds(&t.expired(end)), [BroadcastOutcome::NOT_CONFIRMED]);
        // A CQ is not sent on the broadcast port.
        t.start(BroadcastKind::Cq, t0);
        t.submitted(BroadcastKind::Cq);
        assert_eq!(kinds(&t.expired(end)), [BroadcastOutcome::NOT_CONFIRMED]);
    }
}
