//! The QSY state machine (M3c B3), the pure part: what each station does
//! from a proposal to the re-call on the new frequency, or the fall back.
//!
//! The service computes the rig gate and carries out the actions; this
//! file only decides. Both stations run the same machine with their own
//! `Role` (the session's direction, not who proposed).
//!
//! The handshake is propose → accept → the proposer's confirming accept
//! (the same answer echoed once its own gate passes again). A station
//! commits to the move only when both checks have passed, and only the
//! session's caller disconnects.

use crate::session::PeerMsg;
use mch_protocol::payload::{QsyAnswer, QsyCandidate, QsyPropose};
use mch_protocol::ObjectId;
use serde::Serialize;
use std::time::Duration;
use tokio::time::Instant;
use ts_rs::TS;

/// The answerer's time to accept or decline.
pub const ANSWER_WINDOW: Duration = Duration::from_secs(60);
/// How long the caller calls on the new frequency.
pub const RECALL_WINDOW: Duration = Duration::from_secs(120);
/// The answerer listens this much longer than the caller calls.
pub const LISTEN_GRACE: Duration = Duration::from_secs(20);

/// Refusals the operator sees.
pub const ALREADY_PENDING: &str = "a QSY is already pending";
pub const NOTHING_TO_ANSWER: &str = "there is no QSY to answer";
pub const NO_SUCH_CANDIDATE: &str = "no such candidate";

/// Decline reason codes on the wire (at most 32 bytes).
pub const REASON_RIG: &str = "rig";
pub const REASON_PENDING: &str = "pending";
pub const REASON_TIMEOUT: &str = "timeout";
pub const REASON_LATE: &str = "late";
pub const REASON_INDEX: &str = "index";

/// What the operator is told (i18n keys; the service words them).
pub const NOTE_DECLINED: &str = "qsy.notify.declined";
pub const NOTE_DECLINED_RIG: &str = "qsy.notify.declinedRig";
pub const NOTE_NO_ANSWER: &str = "qsy.notify.noAnswer";
pub const NOTE_TIMED_OUT: &str = "qsy.notify.timedOut";
pub const NOTE_FELL_BACK: &str = "qsy.notify.fellBack";
pub const NOTE_OWN_RIG: &str = "qsy.notify.ownRig";

/// The windows (B3): constants in the app; tests shorten them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QsyWindows {
    pub answer: Duration,
    pub recall: Duration,
    pub grace: Duration,
}

impl Default for QsyWindows {
    fn default() -> Self {
        QsyWindows { answer: ANSWER_WINDOW, recall: RECALL_WINDOW, grace: LISTEN_GRACE }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Caller,
    Answerer,
}

/// Displayed values; the service sets the rig to these minus its offset (B4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Tuning {
    #[ts(type = "number")]
    pub freq_hz: u64,
    pub mode: String,
    pub passband_hz: Option<i32>,
}

impl From<&QsyCandidate> for Tuning {
    /// A passband of 0 on the wire is the rig's default.
    fn from(c: &QsyCandidate) -> Tuning {
        Tuning {
            freq_hz: c.freq_hz,
            mode: c.mode.clone(),
            passband_hz: (c.passband_hz != 0).then(|| i32::try_from(c.passband_hz).unwrap_or(i32::MAX)),
        }
    }
}

/// The station's rig gate at that moment (B3). `NoRig` passes, on the manual path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate {
    Pass,
    NoRig,
    Fail(&'static str),
}

#[derive(Debug)]
pub enum QsyInput {
    ProposeAsked {
        id: ObjectId,
        candidates: Vec<QsyCandidate>,
        note: Option<String>,
        gate: Gate,
    },
    ProposalReceived {
        id: ObjectId,
        propose: QsyPropose,
    },
    /// The operator's choice: an index, or `None` to decline.
    AnswerAsked {
        accept: Option<u8>,
        gate: Gate,
    },
    AnswerReceived {
        id: ObjectId,
        answer: QsyAnswer,
        gate: Gate,
    },
    Disconnected {
        now: Instant,
        original: Tuning,
    },
    /// The result of the retune (after its one retry).
    RigSet {
        ok: bool,
    },
    /// PENDING or CONNECTED on the new frequency.
    PeerSeen,
    Tick(Instant),
}

#[derive(Debug, Clone, PartialEq)]
pub enum QsyAction {
    Send(PeerMsg),
    Refuse(String),
    Disconnect,
    Retune(Tuning),
    ShowManual(Tuning),
    Call,
    Listen,
    Fallback(Tuning),
    Notify(String),
    Done,
}

/// The QSY card as the UI shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct QsyView {
    /// idle, proposed, offered, accepted, agreed, moving, fallingBack.
    pub stage: String,
    pub candidates: Vec<QsyCandidateView>,
    pub chosen: Option<u8>,
    pub seconds_left: Option<u32>,
    pub manual: Option<Tuning>,
}

/// A candidate as the UI shows it (mch-protocol has no ts-rs).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct QsyCandidateView {
    #[ts(type = "number")]
    pub freq_hz: u64,
    pub mode: String,
    pub passband_hz: u32,
    pub name: String,
}

impl From<&QsyCandidate> for QsyCandidateView {
    fn from(c: &QsyCandidate) -> Self {
        QsyCandidateView { freq_hz: c.freq_hz, mode: c.mode.clone(), passband_hz: c.passband_hz, name: c.name.clone() }
    }
}

#[derive(Debug, Clone)]
enum Stage {
    Idle,
    /// We proposed; awaiting the answer.
    Proposed {
        id: ObjectId,
        candidates: Vec<QsyCandidate>,
        deadline: Instant,
    },
    /// The peer proposed; awaiting our operator.
    Offered {
        id: ObjectId,
        candidates: Vec<QsyCandidate>,
        deadline: Instant,
    },
    /// We accepted; awaiting the proposer's confirmation.
    Accepted {
        id: ObjectId,
        candidates: Vec<QsyCandidate>,
        chosen: u8,
        deadline: Instant,
        no_rig: bool,
    },
    /// Both checks passed; awaiting DISCONNECTED.
    Agreed {
        id: ObjectId,
        candidates: Vec<QsyCandidate>,
        chosen: u8,
        no_rig: bool,
    },
    /// Apart, on the new frequency, until the window ends or the peer is seen.
    Moving {
        candidates: Vec<QsyCandidate>,
        chosen: u8,
        target: Tuning,
        original: Tuning,
        deadline: Instant,
        no_rig: bool,
        retuning: bool,
        manual: Option<Tuning>,
    },
    /// The window ended; the retune to the original is under way.
    FallingBack {
        original: Tuning,
    },
}

#[derive(Debug)]
pub struct Qsy {
    role: Role,
    stage: Stage,
    windows: QsyWindows,
    /// The tuning to come back to, from the last DISCONNECTED that moved us.
    original: Option<Tuning>,
}

fn answer(id: ObjectId, a: QsyAnswer) -> QsyAction {
    QsyAction::Send(PeerMsg::QsyAnswer(id, a))
}

fn decline(id: ObjectId, reason: Option<&str>) -> QsyAction {
    answer(id, QsyAnswer::Decline { reason: reason.map(String::from) })
}

fn note(key: &str) -> QsyAction {
    QsyAction::Notify(key.into())
}

impl Qsy {
    pub fn new(role: Role) -> Qsy {
        Qsy::with_windows(role, QsyWindows::default())
    }

    pub fn with_windows(role: Role, windows: QsyWindows) -> Qsy {
        Qsy { role, stage: Stage::Idle, windows, original: None }
    }

    /// When the next `Tick` is due, if a window is running.
    pub fn deadline(&self) -> Option<Instant> {
        match &self.stage {
            Stage::Proposed { deadline, .. }
            | Stage::Offered { deadline, .. }
            | Stage::Accepted { deadline, .. }
            | Stage::Moving { deadline, .. } => Some(*deadline),
            _ => None,
        }
    }

    /// The tuning the station falls back to (once it has moved).
    pub fn original(&self) -> Option<&Tuning> {
        self.original.as_ref()
    }

    /// The candidates on the card (empty when idle).
    pub fn candidates(&self) -> &[QsyCandidate] {
        match &self.stage {
            Stage::Proposed { candidates, .. }
            | Stage::Offered { candidates, .. }
            | Stage::Accepted { candidates, .. }
            | Stage::Agreed { candidates, .. }
            | Stage::Moving { candidates, .. } => candidates,
            _ => &[],
        }
    }

    pub fn role(&self) -> Role {
        self.role
    }

    /// A QSY is under way (proposed, offered, agreed or moving).
    pub fn pending(&self) -> bool {
        !matches!(self.stage, Stage::Idle)
    }

    fn idle(&mut self) {
        self.stage = Stage::Idle;
    }

    /// The station's own move: the caller calls, the answerer listens.
    fn go(&self) -> QsyAction {
        match self.role {
            Role::Caller => QsyAction::Call,
            Role::Answerer => QsyAction::Listen,
        }
    }

    fn window(&self) -> Duration {
        match self.role {
            Role::Caller => self.windows.recall,
            Role::Answerer => self.windows.recall + self.windows.grace,
        }
    }

    /// Both checks passed: commit. Only the caller disconnects.
    fn agree(&mut self, id: ObjectId, candidates: Vec<QsyCandidate>, chosen: u8, no_rig: bool) -> Vec<QsyAction> {
        self.stage = Stage::Agreed { id, candidates, chosen, no_rig };
        match self.role {
            Role::Caller => vec![QsyAction::Disconnect],
            Role::Answerer => vec![],
        }
    }

    pub fn on(&mut self, input: QsyInput, now: Instant) -> Vec<QsyAction> {
        match input {
            QsyInput::ProposeAsked { id, candidates, note, gate } => {
                if self.pending() {
                    return vec![QsyAction::Refuse(ALREADY_PENDING.into())];
                }
                if let Gate::Fail(reason) = gate {
                    return vec![QsyAction::Refuse(reason.into())];
                }
                self.stage =
                    Stage::Proposed { id, candidates: candidates.clone(), deadline: now + self.windows.answer };
                vec![QsyAction::Send(PeerMsg::QsyPropose(id, QsyPropose { candidates, note }))]
            }
            QsyInput::ProposalReceived { id, propose } => {
                if self.pending() {
                    return vec![decline(id, Some(REASON_PENDING))];
                }
                self.stage = Stage::Offered { id, candidates: propose.candidates, deadline: now + self.windows.answer };
                vec![]
            }
            QsyInput::AnswerAsked { accept, gate } => self.on_answer_asked(accept, gate, now),
            QsyInput::AnswerReceived { id, answer, gate } => self.on_answer_received(id, answer, gate),
            QsyInput::Disconnected { now, original } => self.on_disconnected(now, original),
            QsyInput::RigSet { ok } => self.on_rig_set(ok),
            QsyInput::PeerSeen => match self.stage {
                Stage::Moving { .. } => {
                    self.idle();
                    vec![QsyAction::Done]
                }
                _ => vec![],
            },
            QsyInput::Tick(t) => self.on_tick(t),
        }
    }

    fn on_answer_asked(&mut self, accept: Option<u8>, gate: Gate, now: Instant) -> Vec<QsyAction> {
        let Stage::Offered { id, candidates, .. } = &self.stage else {
            return vec![QsyAction::Refuse(NOTHING_TO_ANSWER.into())];
        };
        let (id, candidates) = (*id, candidates.clone());
        let Some(index) = accept else {
            self.idle();
            return vec![decline(id, None), QsyAction::Done];
        };
        if usize::from(index) >= candidates.len() {
            return vec![QsyAction::Refuse(NO_SUCH_CANDIDATE.into())];
        }
        if let Gate::Fail(reason) = gate {
            self.idle();
            return vec![decline(id, Some(REASON_RIG)), QsyAction::Refuse(reason.into())];
        }
        self.stage = Stage::Accepted {
            id,
            candidates,
            chosen: index,
            deadline: now + self.windows.answer,
            no_rig: gate == Gate::NoRig,
        };
        vec![answer(id, QsyAnswer::Accept { index })]
    }

    fn on_answer_received(&mut self, id: ObjectId, a: QsyAnswer, gate: Gate) -> Vec<QsyAction> {
        match (&self.stage, a) {
            // Our proposal accepted: check our own gate again, then confirm.
            (Stage::Proposed { id: p, candidates, .. }, QsyAnswer::Accept { index }) if *p == id => {
                let candidates = candidates.clone();
                if usize::from(index) >= candidates.len() {
                    self.idle();
                    return vec![decline(id, Some(REASON_INDEX)), note(NOTE_DECLINED)];
                }
                if let Gate::Fail(reason) = gate {
                    self.idle();
                    tracing::info!("our own rig refused the accepted QSY: {reason}");
                    return vec![decline(id, Some(REASON_RIG)), note(NOTE_OWN_RIG)];
                }
                let mut actions = vec![answer(id, QsyAnswer::Accept { index })];
                actions.extend(self.agree(id, candidates, index, gate == Gate::NoRig));
                actions
            }
            // The proposer's confirmation of our accept.
            (Stage::Accepted { id: p, candidates, chosen, no_rig, .. }, QsyAnswer::Accept { index })
                if *p == id && index == *chosen =>
            {
                let (candidates, chosen, no_rig) = (candidates.clone(), *chosen, *no_rig);
                self.agree(id, candidates, chosen, no_rig)
            }
            (Stage::Agreed { id: p, .. }, QsyAnswer::Accept { .. }) if *p == id => vec![],
            // An accept for nothing pending (late, or after we withdrew): decline, stay.
            (_, QsyAnswer::Accept { .. }) => vec![decline(id, Some(REASON_LATE))],
            // A decline of what is pending, before anyone has moved.
            (
                Stage::Proposed { id: p, .. }
                | Stage::Offered { id: p, .. }
                | Stage::Accepted { id: p, .. }
                | Stage::Agreed { id: p, .. },
                QsyAnswer::Decline { reason },
            ) if *p == id => {
                self.idle();
                let key = if reason.as_deref() == Some(REASON_RIG) { NOTE_DECLINED_RIG } else { NOTE_DECLINED };
                vec![note(key), QsyAction::Done]
            }
            (_, QsyAnswer::Decline { .. }) => vec![],
        }
    }

    fn on_disconnected(&mut self, now: Instant, original: Tuning) -> Vec<QsyAction> {
        let (candidates, chosen, no_rig) = match &self.stage {
            // Accepted covers the link dropping before the confirmation
            // arrived: we agreed, so we go (the window brings us back).
            Stage::Agreed { candidates, chosen, no_rig, .. } | Stage::Accepted { candidates, chosen, no_rig, .. } => {
                (candidates.clone(), *chosen, *no_rig)
            }
            Stage::Proposed { .. } | Stage::Offered { .. } => {
                self.idle();
                return vec![QsyAction::Done];
            }
            _ => return vec![],
        };
        self.original = Some(original.clone());
        let target = Tuning::from(&candidates[usize::from(chosen)]);
        let deadline = now + self.window();
        let actions = if no_rig {
            vec![QsyAction::ShowManual(target.clone()), self.go()]
        } else {
            vec![QsyAction::Retune(target.clone())]
        };
        self.stage = Stage::Moving {
            candidates,
            chosen,
            manual: no_rig.then(|| target.clone()),
            target,
            original,
            deadline,
            no_rig,
            retuning: !no_rig,
        };
        actions
    }

    fn on_rig_set(&mut self, ok: bool) -> Vec<QsyAction> {
        let go = self.go();
        match &mut self.stage {
            Stage::Moving { retuning: retuning @ true, target, manual, .. } => {
                *retuning = false;
                if ok {
                    vec![go]
                } else {
                    // The manual path; the window keeps running.
                    *manual = Some(target.clone());
                    vec![QsyAction::ShowManual(target.clone()), go]
                }
            }
            Stage::FallingBack { original } => {
                let original = original.clone();
                self.idle();
                if ok {
                    vec![QsyAction::Done]
                } else {
                    vec![QsyAction::ShowManual(original), QsyAction::Done]
                }
            }
            _ => vec![],
        }
    }

    fn on_tick(&mut self, now: Instant) -> Vec<QsyAction> {
        match &self.stage {
            Stage::Proposed { id, deadline, .. } if now >= *deadline => {
                // Withdrawn: the answerer is told, and a late accept is declined.
                let id = *id;
                self.idle();
                vec![decline(id, Some(REASON_TIMEOUT)), note(NOTE_NO_ANSWER), QsyAction::Done]
            }
            Stage::Offered { id, deadline, .. } if now >= *deadline => {
                let id = *id;
                self.idle();
                vec![decline(id, Some(REASON_TIMEOUT)), note(NOTE_TIMED_OUT), QsyAction::Done]
            }
            Stage::Accepted { deadline, .. } if now >= *deadline => {
                self.idle();
                vec![note(NOTE_NO_ANSWER), QsyAction::Done]
            }
            Stage::Moving { deadline, original, no_rig, .. } if now >= *deadline => {
                let (original, no_rig) = (original.clone(), *no_rig);
                if no_rig {
                    self.idle();
                    vec![QsyAction::ShowManual(original), note(NOTE_FELL_BACK), QsyAction::Done]
                } else {
                    self.stage = Stage::FallingBack { original: original.clone() };
                    vec![QsyAction::Fallback(original), note(NOTE_FELL_BACK)]
                }
            }
            _ => vec![],
        }
    }

    pub fn view(&self, now: Instant) -> QsyView {
        let left = |d: &Instant| Some(d.saturating_duration_since(now).as_secs_f64().ceil() as u32);
        let cands = |c: &[QsyCandidate]| c.iter().map(QsyCandidateView::from).collect();
        let (stage, candidates, chosen, seconds_left, manual) = match &self.stage {
            Stage::Idle => ("idle", vec![], None, None, None),
            Stage::Proposed { candidates, deadline, .. } => ("proposed", cands(candidates), None, left(deadline), None),
            Stage::Offered { candidates, deadline, .. } => ("offered", cands(candidates), None, left(deadline), None),
            Stage::Accepted { candidates, chosen, deadline, .. } => {
                ("accepted", cands(candidates), Some(*chosen), left(deadline), None)
            }
            Stage::Agreed { candidates, chosen, .. } => ("agreed", cands(candidates), Some(*chosen), None, None),
            Stage::Moving { candidates, chosen, deadline, manual, .. } => {
                ("moving", cands(candidates), Some(*chosen), left(deadline), manual.clone())
            }
            Stage::FallingBack { .. } => ("fallingBack", vec![], None, None, None),
        };
        QsyView { stage: stage.into(), candidates, chosen, seconds_left, manual }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: Duration = Duration::from_secs(1);

    fn cands() -> Vec<QsyCandidate> {
        vec![
            QsyCandidate { freq_hz: 7_105_000, mode: "USB".into(), passband_hz: 2400, name: "40m".into() },
            QsyCandidate { freq_hz: 14_105_000, mode: "USB".into(), passband_hz: 0, name: "20m".into() },
        ]
    }

    fn original() -> Tuning {
        Tuning { freq_hz: 7_074_000, mode: "PKTUSB".into(), passband_hz: None }
    }

    fn target(i: usize) -> Tuning {
        Tuning::from(&cands()[i])
    }

    fn sent(actions: &[QsyAction]) -> Vec<&PeerMsg> {
        actions.iter().filter_map(|a| if let QsyAction::Send(m) = a { Some(m) } else { None }).collect()
    }

    fn disconnects(actions: &[QsyAction]) -> bool {
        actions.contains(&QsyAction::Disconnect)
    }

    fn declined_with(actions: &[QsyAction], reason: &str) -> bool {
        sent(actions)
            .iter()
            .any(|m| matches!(m, PeerMsg::QsyAnswer(_, QsyAnswer::Decline { reason: Some(r) }) if r == reason))
    }

    /// Deliver `actions`' messages to `to`, with `gate` as `to`'s gate.
    fn relay(actions: &[QsyAction], to: &mut Qsy, gate: Gate, now: Instant) -> Vec<QsyAction> {
        let mut out = vec![];
        for m in sent(actions) {
            let input = match m.clone() {
                PeerMsg::QsyPropose(id, propose) => QsyInput::ProposalReceived { id, propose },
                PeerMsg::QsyAnswer(id, answer) => QsyInput::AnswerReceived { id, answer, gate },
                other => panic!("not a QSY message: {other:?}"),
            };
            out.extend(to.on(input, now));
        }
        out
    }

    fn propose(q: &mut Qsy, gate: Gate, now: Instant) -> Vec<QsyAction> {
        q.on(QsyInput::ProposeAsked { id: ObjectId::random(), candidates: cands(), note: None, gate }, now)
    }

    /// Drive both machines to agreement on candidate 0; returns the caller's
    /// and the answerer's actions at their last step.
    fn agree(proposer_is_caller: bool, a: &mut Qsy, b: &mut Qsy, ga: Gate, gb: Gate, t: Instant) -> Vec<QsyAction> {
        let (p, o, gp, go) = if proposer_is_caller { (a, b, ga, gb) } else { (b, a, gb, ga) };
        let prop = propose(p, gp, t);
        relay(&prop, o, go, t);
        let acc = o.on(QsyInput::AnswerAsked { accept: Some(0), gate: go }, t);
        let conf = relay(&acc, p, gp, t);
        let mut last = relay(&conf, o, go, t);
        last.extend(conf);
        last
    }

    fn pair() -> (Qsy, Qsy) {
        (Qsy::new(Role::Caller), Qsy::new(Role::Answerer))
    }

    #[test]
    fn answerer_gate_fail_declines_rig() {
        let (mut a, mut b) = pair();
        let t = Instant::now();
        relay(&propose(&mut a, Gate::Pass, t), &mut b, Gate::Pass, t);
        let acts = b.on(QsyInput::AnswerAsked { accept: Some(0), gate: Gate::Fail("the rig link is down") }, t);
        assert!(declined_with(&acts, REASON_RIG), "{acts:?}");
        assert!(!disconnects(&acts) && !b.pending());
        let back = relay(&acts, &mut a, Gate::Pass, t);
        assert!(!disconnects(&back) && !a.pending(), "{back:?}");
    }

    #[test]
    fn proposer_gate_fail_sends_nothing() {
        let (mut a, _) = pair();
        let acts = propose(&mut a, Gate::Fail("the station is keyed"), Instant::now());
        assert_eq!(acts, vec![QsyAction::Refuse("the station is keyed".into())]);
        assert!(!a.pending());
    }

    #[test]
    fn proposer_regate_fail_declines_and_stays() {
        let (mut a, mut b) = pair();
        let t = Instant::now();
        relay(&propose(&mut a, Gate::Pass, t), &mut b, Gate::Pass, t);
        let acc = b.on(QsyInput::AnswerAsked { accept: Some(0), gate: Gate::Pass }, t);
        let back = relay(&acc, &mut a, Gate::Fail("the station is busy"), t);
        assert!(declined_with(&back, REASON_RIG), "{back:?}");
        assert!(!disconnects(&back) && !a.pending());
        let fin = relay(&back, &mut b, Gate::Pass, t);
        assert!(!disconnects(&fin) && !b.pending(), "{fin:?}");
        assert!(fin.contains(&QsyAction::Notify(NOTE_DECLINED_RIG.into())));
    }

    #[test]
    fn answer_window_expiry_declines_both() {
        let (mut a, mut b) = pair();
        let t = Instant::now();
        relay(&propose(&mut a, Gate::Pass, t), &mut b, Gate::Pass, t);
        assert!(a.on(QsyInput::Tick(t + 59 * S), t + 59 * S).is_empty());
        assert!(b.on(QsyInput::Tick(t + 59 * S), t + 59 * S).is_empty());
        let ea = a.on(QsyInput::Tick(t + 60 * S), t + 60 * S);
        let eb = b.on(QsyInput::Tick(t + 60 * S), t + 60 * S);
        assert!(declined_with(&ea, REASON_TIMEOUT) && ea.contains(&QsyAction::Notify(NOTE_NO_ANSWER.into())));
        assert!(declined_with(&eb, REASON_TIMEOUT) && eb.contains(&QsyAction::Notify(NOTE_TIMED_OUT.into())));
        assert!(!a.pending() && !b.pending());
        assert!(relay(&ea, &mut b, Gate::Pass, t + 61 * S).is_empty());
        assert!(relay(&eb, &mut a, Gate::Pass, t + 61 * S).is_empty());
    }

    #[test]
    fn late_accept_is_declined() {
        let (mut a, mut b) = pair();
        let t = Instant::now();
        relay(&propose(&mut a, Gate::Pass, t), &mut b, Gate::Pass, t);
        a.on(QsyInput::Tick(t + 60 * S), t + 60 * S);
        // b's operator accepts just before its own window closed; it reaches a late.
        let acc = b.on(QsyInput::AnswerAsked { accept: Some(0), gate: Gate::Pass }, t + 59 * S);
        let back = relay(&acc, &mut a, Gate::Pass, t + 61 * S);
        assert!(declined_with(&back, REASON_LATE), "{back:?}");
        assert!(!disconnects(&back) && !a.pending());
        let fin = relay(&back, &mut b, Gate::Pass, t + 62 * S);
        assert!(!disconnects(&fin) && !b.pending());
    }

    #[test]
    fn accept_then_caller_disconnects_answerer_waits() {
        // The answerer (callee) proposes; the caller accepts.
        let (mut a, mut b) = pair();
        let t = Instant::now();
        let prop = propose(&mut b, Gate::Pass, t);
        relay(&prop, &mut a, Gate::Pass, t);
        let acc = a.on(QsyInput::AnswerAsked { accept: Some(0), gate: Gate::Pass }, t);
        assert!(!disconnects(&acc), "the caller waits for the proposer's own check");
        let conf = relay(&acc, &mut b, Gate::Pass, t);
        assert!(!disconnects(&conf), "only the caller disconnects");
        assert!(b.pending());
        let go = relay(&conf, &mut a, Gate::Pass, t);
        assert_eq!(go, vec![QsyAction::Disconnect]);
        assert_eq!(b.view(t).stage, "agreed");
        let moved = b.on(QsyInput::Disconnected { now: t + S, original: original() }, t + S);
        assert_eq!(moved, vec![QsyAction::Retune(target(0))]);
    }

    #[test]
    fn link_loss_after_accept_still_qsys() {
        let (mut a, mut b) = pair();
        let t = Instant::now();
        relay(&propose(&mut a, Gate::Pass, t), &mut b, Gate::Pass, t);
        let acc = b.on(QsyInput::AnswerAsked { accept: Some(0), gate: Gate::Pass }, t);
        let conf = relay(&acc, &mut a, Gate::Pass, t);
        assert!(disconnects(&conf));
        // The confirmation never reaches b: the link drops first.
        let da = a.on(QsyInput::Disconnected { now: t + S, original: original() }, t + S);
        let db = b.on(QsyInput::Disconnected { now: t + 2 * S, original: original() }, t + 2 * S);
        assert_eq!(da, vec![QsyAction::Retune(target(0))]);
        assert_eq!(db, vec![QsyAction::Retune(target(0))]);
        assert_eq!(b.on(QsyInput::RigSet { ok: true }, t + 3 * S), vec![QsyAction::Listen]);
        let end = t + 2 * S + RECALL_WINDOW + LISTEN_GRACE;
        assert!(b.on(QsyInput::Tick(end), end).contains(&QsyAction::Fallback(original())), "not stranded");
    }

    #[test]
    fn one_qsy_at_a_time() {
        let (mut a, mut b) = pair();
        let t = Instant::now();
        let first = propose(&mut a, Gate::Pass, t);
        let PeerMsg::QsyPropose(first_id, _) = sent(&first)[0].clone() else { panic!() };
        assert_eq!(propose(&mut a, Gate::Pass, t), vec![QsyAction::Refuse(ALREADY_PENDING.into())]);
        // b proposes at the same moment: a declines it, and its own stands.
        let theirs = propose(&mut b, Gate::Pass, t);
        let back = relay(&theirs, &mut a, Gate::Pass, t);
        assert!(declined_with(&back, REASON_PENDING), "{back:?}");
        assert_eq!(a.view(t).stage, "proposed");
        assert_eq!(a.view(t).candidates.len(), 2);
        assert!(matches!(&a.stage, Stage::Proposed { id, .. } if *id == first_id));
        // An offered station refuses its own operator's proposal too.
        let (mut c, mut d) = pair();
        relay(&propose(&mut c, Gate::Pass, t), &mut d, Gate::Pass, t);
        assert_eq!(propose(&mut d, Gate::Pass, t), vec![QsyAction::Refuse(ALREADY_PENDING.into())]);
    }

    #[test]
    fn unsettable_candidate_fails_the_gate() {
        let cant = Gate::Fail("With the frequency offset, that is below 0 Hz on the radio's dial");
        let (mut a, mut b) = pair();
        let t = Instant::now();
        let acts = propose(&mut a, cant, t);
        assert!(sent(&acts).is_empty() && matches!(acts[..], [QsyAction::Refuse(_)]));
        relay(&propose(&mut a, Gate::Pass, t), &mut b, Gate::Pass, t);
        let ans = b.on(QsyInput::AnswerAsked { accept: Some(0), gate: cant }, t);
        assert!(declined_with(&ans, REASON_RIG) && !disconnects(&ans));
        let back = relay(&ans, &mut a, Gate::Pass, t);
        assert!(!disconnects(&back));
    }

    fn moving(caller: bool, ga: Gate, gb: Gate) -> (Qsy, Instant) {
        let (mut a, mut b) = pair();
        let t = Instant::now();
        agree(true, &mut a, &mut b, ga, gb, t);
        let mut q = if caller { a } else { b };
        assert_eq!(q.view(t).stage, "agreed");
        let gate = if caller { ga } else { gb };
        let acts = q.on(QsyInput::Disconnected { now: t, original: original() }, t);
        if gate == Gate::NoRig {
            assert_eq!(acts[0], QsyAction::ShowManual(target(0)));
        } else {
            assert_eq!(acts, vec![QsyAction::Retune(target(0))]);
        }
        (q, t)
    }

    #[test]
    fn caller_calls_until_120s_then_falls_back() {
        let (mut q, t) = moving(true, Gate::Pass, Gate::Pass);
        assert_eq!(q.on(QsyInput::RigSet { ok: true }, t + S), vec![QsyAction::Call]);
        assert!(q.on(QsyInput::Tick(t + 119 * S), t + 119 * S).is_empty());
        let fb = q.on(QsyInput::Tick(t + 120 * S), t + 120 * S);
        assert_eq!(fb, vec![QsyAction::Fallback(original()), QsyAction::Notify(NOTE_FELL_BACK.into())]);
        assert_eq!(q.on(QsyInput::RigSet { ok: true }, t + 121 * S), vec![QsyAction::Done]);
        assert!(!q.pending());
    }

    #[test]
    fn answerer_listens_140s_then_falls_back() {
        let (mut q, t) = moving(false, Gate::Pass, Gate::Pass);
        assert_eq!(q.on(QsyInput::RigSet { ok: true }, t + S), vec![QsyAction::Listen]);
        assert!(q.on(QsyInput::Tick(t + 139 * S), t + 139 * S).is_empty());
        assert!(q.on(QsyInput::Tick(t + 140 * S), t + 140 * S).contains(&QsyAction::Fallback(original())));
    }

    #[test]
    fn peer_seen_cancels_fallback() {
        let (mut q, t) = moving(true, Gate::Pass, Gate::Pass);
        q.on(QsyInput::RigSet { ok: true }, t + S);
        assert_eq!(q.on(QsyInput::PeerSeen, t + 30 * S), vec![QsyAction::Done]);
        assert!(!q.pending());
        assert!(q.on(QsyInput::Tick(t + 200 * S), t + 200 * S).is_empty());
    }

    #[test]
    fn rig_fail_shows_manual_and_keeps_timers() {
        let (mut q, t) = moving(true, Gate::Pass, Gate::Pass);
        let acts = q.on(QsyInput::RigSet { ok: false }, t + 2 * S);
        assert_eq!(acts, vec![QsyAction::ShowManual(target(0)), QsyAction::Call]);
        let v = q.view(t + 2 * S);
        assert_eq!((v.manual, v.seconds_left), (Some(target(0)), Some(118)));
        assert!(q.on(QsyInput::Tick(t + 120 * S), t + 120 * S).contains(&QsyAction::Fallback(original())));
    }

    #[test]
    fn no_rig_accepts_with_manual_path() {
        let (mut a, mut b) = pair();
        let t = Instant::now();
        relay(&propose(&mut a, Gate::Pass, t), &mut b, Gate::Pass, t);
        let acc = b.on(QsyInput::AnswerAsked { accept: Some(0), gate: Gate::NoRig }, t);
        assert!(matches!(sent(&acc)[..], [PeerMsg::QsyAnswer(_, QsyAnswer::Accept { index: 0 })]));
        relay(&relay(&acc, &mut a, Gate::Pass, t), &mut b, Gate::NoRig, t);
        let acts = b.on(QsyInput::Disconnected { now: t, original: original() }, t);
        assert_eq!(acts, vec![QsyAction::ShowManual(target(0)), QsyAction::Listen]);
        assert_eq!(b.view(t).manual, Some(target(0)));
        let end = t + RECALL_WINDOW + LISTEN_GRACE;
        let fb = b.on(QsyInput::Tick(end), end);
        assert!(fb.contains(&QsyAction::ShowManual(original())), "Tune back to the original: {fb:?}");
        assert!(!fb.iter().any(|a| matches!(a, QsyAction::Fallback(_) | QsyAction::Retune(_))));
        assert!(!b.pending());
    }

    #[test]
    fn never_back_to_session_freq_inside_window() {
        let (mut q, t) = moving(false, Gate::Pass, Gate::Pass);
        let mut all = q.on(QsyInput::RigSet { ok: false }, t + S);
        for s in (10..140).step_by(10) {
            all.extend(q.on(QsyInput::Tick(t + s * S), t + s * S));
        }
        assert!(
            !all.iter().any(|a| matches!(a, QsyAction::Fallback(_) | QsyAction::Retune(_) | QsyAction::Done)),
            "{all:?}"
        );
        assert_eq!(q.view(t + 139 * S).stage, "moving");
    }

    #[test]
    fn passband_zero_is_rig_default() {
        assert_eq!(target(1).passband_hz, None);
        assert_eq!(target(0).passband_hz, Some(2400));
    }
}
