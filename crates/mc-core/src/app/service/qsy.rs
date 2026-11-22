//! The actor's side of QSY (M3c B3): proposing and answering on the
//! operator's command, the rig gate, the retune (with one retry), the
//! caller's re-calls, and the fall back. The decisions are `app::qsy`'s.

use super::{conflict, App};
use crate::app::broadcast::BroadcastKind;
use crate::app::contract::{AppCommand, AppError, CommandOk};
use crate::app::events::{AppEvent, ModemState, SessionDirection};
use crate::app::modem::ModemCmd;
use crate::app::qsy::{
    self, Gate, Qsy, QsyAction, QsyInput, QsyView, QsyWindows, Role, Tuning, ALREADY_PENDING, NOTHING_TO_ANSWER,
    NO_SUCH_CANDIDATE,
};
use crate::app::rig::RigChange;
use crate::session::PeerMsg;
use mch_protocol::beacon::{self as mcb, Mode, QsySuggestion, Received, MAX_QSY_CANDIDATES};
use mch_protocol::payload::{QsyAnswer, QsyCandidate, MAX_CANDIDATES, QSY_MODES};
use mch_protocol::{ObjectId, CAP_QSY};
use std::time::Duration;
use tokio::time::Instant;

/// The pause between the caller's re-calls on the new frequency.
const RECALL_GAP: Duration = Duration::from_secs(2);

/// A retune waiting to go out, or under way.
#[derive(Debug)]
struct Retune {
    tuning: Tuning,
    /// Attempts made (a failed one is retried once).
    tries: u8,
    /// The rig requests of the attempt under way.
    in_flight: Vec<u64>,
    failed: bool,
}

#[derive(Debug, Default)]
pub(in crate::app) struct QsySide {
    machine: Option<Qsy>,
    windows: QsyWindows,
    /// The session's peer (the station to re-call or listen for).
    peer: Option<String>,
    retune: Option<Retune>,
    /// The caller keeps calling until the window ends; the next call is due then.
    next_call: Option<Instant>,
    calling: bool,
    /// The first session's QSO id, for the re-call's record: (peer, id).
    first_qso: Option<(String, String)>,
    /// The manual path's "Tuned" was pressed.
    tuned: bool,
    shown: Option<QsyView>,
    /// The last suggestion addressed to us (MCB/1 kind 3): sender, candidates.
    suggestion: Option<(String, Vec<Tuning>)>,
    /// The sequence counter of the suggestions we send.
    suggest_seq: u32,
}

impl QsySide {
    pub(in crate::app) fn new(windows: QsyWindows) -> QsySide {
        QsySide { windows, suggest_seq: rand::random(), ..QsySide::default() }
    }
}

fn refused(reason: &'static str, message: &str) -> AppError {
    AppError::Refused { reason, message: message.into() }
}

/// The refusal for one of the machine's `Refuse` lines.
fn refusal(message: String) -> AppError {
    let reason = match message.as_str() {
        ALREADY_PENDING => "qsy.refused.pending",
        NOTHING_TO_ANSWER => "qsy.refused.nothingToAnswer",
        NO_SUCH_CANDIDATE => "qsy.refused.noCandidate",
        _ => "qsy.refused.rig",
    };
    AppError::Refused { reason, message }
}

/// The English line for a notice key.
fn notice_text(key: &str, peer: &str, original: Option<&Tuning>) -> String {
    match key {
        qsy::NOTE_DECLINED => format!("{peer} declined the QSY"),
        qsy::NOTE_DECLINED_RIG => format!("{peer} declined the QSY: their rig can't change now"),
        qsy::NOTE_OWN_RIG => "QSY declined: this station's rig can't change now".into(),
        qsy::NOTE_NO_ANSWER => format!("No answer from {peer} to the QSY"),
        qsy::NOTE_TIMED_OUT => format!("The QSY offer from {peer} ran out of time"),
        qsy::NOTE_PASSBAND => format!("{peer}'s QSY asked for a passband over 20 kHz: declined"),
        qsy::NOTE_RETURN_BY_HAND => {
            format!("No contact with {peer} on the new frequency: return to your original frequency by hand")
        }
        qsy::NOTE_ABANDONED => format!("The QSY with {peer} was abandoned: the link did not drop. The session goes on"),
        qsy::NOTE_DECLINED_PENDING => format!("{peer} is busy with another QSY"),
        qsy::NOTE_FELL_BACK => match original {
            Some(t) => format!("No contact with {peer} on the new frequency: back to {}", describe(t)),
            None => format!("No contact with {peer} on the new frequency"),
        },
        other => other.to_string(),
    }
}

/// "7.105.000 USB".
pub(in crate::app) fn describe(t: &Tuning) -> String {
    let hz = t.freq_hz;
    let f = format!("{}.{:03}.{:03}", hz / 1_000_000, hz / 1000 % 1000, hz % 1000);
    if t.mode.is_empty() {
        f
    } else {
        format!("{f} {}", t.mode)
    }
}

/// A frequency-list entry as a candidate; its name cut to 32 bytes.
fn candidate(e: &crate::app::frequencies::FrequencyEntry) -> Result<QsyCandidate, AppError> {
    if !QSY_MODES.contains(&e.mode.as_str()) {
        return Err(AppError::Validation {
            errors: crate::app::chat::field("entryIds", format!("{}: the mode {} can't be proposed", e.name, e.mode)),
        });
    }
    let mut name = e.name.clone();
    while name.len() > 32 {
        name.pop();
    }
    let passband_hz = e.passband_hz.and_then(|p| u32::try_from(p).ok()).unwrap_or(0);
    Ok(QsyCandidate { freq_hz: e.freq_hz, mode: e.mode.clone(), passband_hz, name })
}

impl App {
    /// A QSY is under way: the schedule and other retunes stand aside.
    /// While a QSY is agreed, moving or falling back, the rig is its own:
    /// other tuning is refused (M4 B4).
    pub(super) fn qsy_owns_rig(&self) -> Result<(), AppError> {
        if self.qsy.machine.as_ref().is_some_and(Qsy::moving) {
            return Err(refused("qsy.refused.tuneDuringQsy", "not while a QSY is under way"));
        }
        Ok(())
    }

    pub(super) fn qsy_pending(&self) -> bool {
        self.qsy.machine.as_ref().is_some_and(Qsy::pending)
    }

    /// The machine for this session (a new one unless a QSY is under way).
    fn qsy_machine(&mut self) -> Option<&mut Qsy> {
        if !self.qsy_pending() {
            let s = self.session.as_ref()?;
            let role = match s.direction {
                SessionDirection::Outgoing => Role::Caller,
                SessionDirection::Incoming => Role::Answerer,
            };
            self.qsy.peer = Some(s.peer.clone());
            self.qsy.machine = Some(Qsy::with_windows(role, self.qsy.windows));
        }
        self.qsy.machine.as_mut()
    }

    /// The session's peer, if it negotiated `qsy/1`; otherwise the refusal.
    fn qsy_session(&self) -> Result<(), AppError> {
        let s = self.session.as_ref().ok_or_else(|| refused("qsy.refused.noSession", "there is no session"))?;
        if !s.caps.iter().any(|c| c == CAP_QSY) {
            return Err(refused("qsy.refused.unsupported", "not supported by this station"));
        }
        Ok(())
    }

    /// The gate for the worst of several candidates.
    fn qsy_gate_all(&self, candidates: &[QsyCandidate]) -> Gate {
        let mut gate = Gate::Pass;
        for c in candidates {
            match self.rig_gate_for_qsy(&Tuning::from(c)) {
                g @ Gate::Fail(_) => return g,
                g => gate = g,
            }
        }
        gate
    }

    pub(super) fn propose_qsy(&mut self, entry_ids: Vec<String>, note: Option<String>) -> Result<CommandOk, AppError> {
        self.qsy_session()?;
        self.qsy_no_transfer()?;
        if entry_ids.is_empty() || entry_ids.len() > MAX_CANDIDATES {
            return Err(AppError::Validation {
                errors: crate::app::chat::field("entryIds", format!("Choose 1 to {MAX_CANDIDATES} frequencies")),
            });
        }
        let entries = self.entries()?;
        let mut candidates = Vec::with_capacity(entry_ids.len());
        for id in &entry_ids {
            let e =
                entries.iter().find(|e| &e.id == id).ok_or_else(|| conflict("that entry no longer exists".into()))?;
            candidates.push(candidate(e)?);
        }
        let note = note.map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
        if note.as_ref().is_some_and(|n| n.len() > 64) {
            return Err(AppError::Validation { errors: crate::app::chat::field("note", "At most 64 bytes") });
        }
        let gate = self.qsy_gate_all(&candidates);
        let input = QsyInput::ProposeAsked { id: ObjectId::random(), candidates, note, gate };
        self.qsy_input(input)
    }

    /// A QSY ends the session, so never while a file is under way (M4,
    /// Review Focus 1): propose and accept are refused; declining is not.
    fn qsy_no_transfer(&self) -> Result<(), AppError> {
        if self.files_active() {
            return Err(refused("qsy.refused.transfer", crate::app::qsy::REASON_TRANSFER));
        }
        Ok(())
    }

    pub(super) fn answer_qsy(&mut self, accept: Option<u8>) -> Result<CommandOk, AppError> {
        if accept.is_some() {
            self.qsy_no_transfer()?;
        }
        let gate = match (accept, self.qsy.machine.as_ref()) {
            (Some(i), Some(m)) => match m.candidates().get(usize::from(i)) {
                Some(c) => self.rig_gate_for_qsy(&Tuning::from(c)),
                None => Gate::Pass, // the machine refuses the index
            },
            _ => Gate::Pass,
        };
        if self.qsy.machine.is_none() {
            return Err(refusal(NOTHING_TO_ANSWER.into()));
        }
        self.qsy_input(QsyInput::AnswerAsked { accept, gate })
    }

    pub(super) fn qsy_tuned(&mut self) -> Result<CommandOk, AppError> {
        let manual = self.qsy.machine.as_ref().is_some_and(|m| m.view(Instant::now()).manual.is_some());
        if !manual {
            return Err(conflict("there is nothing to tune by hand".into()));
        }
        self.qsy.tuned = true;
        self.publish_qsy();
        Ok(CommandOk::Done)
    }

    /// A QSY message from the session's peer.
    pub(super) fn on_peer_qsy(&mut self, m: PeerMsg) {
        let input = match m {
            PeerMsg::QsyPropose(id, propose) => QsyInput::ProposalReceived { id, propose },
            PeerMsg::QsyAnswer(id, answer) => {
                let gate = match (&answer, self.qsy.machine.as_ref()) {
                    (QsyAnswer::Accept { index }, Some(m)) => m
                        .candidates()
                        .get(usize::from(*index))
                        .map_or(Gate::Pass, |c| self.rig_gate_for_qsy(&Tuning::from(c))),
                    _ => Gate::Pass,
                };
                QsyInput::AnswerReceived { id, answer, gate }
            }
            _ => return,
        };
        if let Err(e) = self.qsy_input(input) {
            tracing::debug!("a peer QSY message refused: {e}");
        }
    }

    /// Feed the machine and carry out what it decides.
    fn qsy_input(&mut self, input: QsyInput) -> Result<CommandOk, AppError> {
        let now = Instant::now();
        let Some(m) = self.qsy_machine() else {
            return Err(refused("qsy.refused.noSession", "there is no session"));
        };
        let actions = m.on(input, now);
        self.qsy_actions(actions)
    }

    fn qsy_actions(&mut self, actions: Vec<QsyAction>) -> Result<CommandOk, AppError> {
        let mut result = Ok(CommandOk::Done);
        for a in actions {
            match a {
                QsyAction::Send(m) => {
                    if let Err(e) = self.command_modem(ModemCmd::Peer(m)) {
                        tracing::warn!("a QSY message was not sent: {e}");
                    }
                }
                QsyAction::Refuse(message) => result = Err(refusal(message)),
                QsyAction::Disconnect => {
                    // The modem drains the queue first.
                    if let Err(e) = self.command_modem(ModemCmd::Disconnect) {
                        tracing::warn!("could not disconnect for the QSY: {e}");
                    }
                }
                QsyAction::Retune(t) | QsyAction::Fallback(t) => {
                    self.qsy.calling = false;
                    self.qsy.next_call = None;
                    self.cancel_qsy_call();
                    let line = self.discovery.note("rig", format!("QSY: tuning to {}", describe(&t)));
                    self.emit(line);
                    self.qsy.retune = Some(Retune { tuning: t, tries: 0, in_flight: vec![], failed: false });
                }
                QsyAction::ShowManual(t) => {
                    self.qsy.tuned = false;
                    let line = self.discovery.note("rig", format!("Tune to {} now", describe(&self.qsy_dial(&t))));
                    self.emit(line);
                }
                QsyAction::Call => {
                    self.qsy.calling = true;
                    self.qsy.next_call = Some(Instant::now());
                }
                QsyAction::Listen => {}
                QsyAction::Notify(key) => {
                    let peer = self.qsy.peer.clone().unwrap_or_default();
                    let mut original = self.qsy.machine.as_ref().and_then(Qsy::original).cloned();
                    if !self.rig.policy.link_up() {
                        // Tuned by hand: say where the dial goes.
                        original = original.map(|t| self.qsy_dial(&t));
                    }
                    let text = notice_text(&key, &peer, original.as_ref());
                    let line = self.discovery.note("qsy", text.clone());
                    self.emit(line);
                    self.emit(AppEvent::QsyNotice { key, text, peer, original: original.as_ref().map(describe) });
                }
                QsyAction::Done => {
                    self.qsy.calling = false;
                    self.qsy.next_call = None;
                }
            }
        }
        let owns_rig = self.qsy.machine.as_ref().is_some_and(Qsy::moving);
        self.beacon.sched.on_qsy(owns_rig, Instant::now());
        self.publish_qsy();
        result
    }

    /// The window is over (or the peer was found): stop a re-call still out.
    fn cancel_qsy_call(&mut self) {
        if self.modem == ModemState::Calling && self.session.is_none() {
            let _ = self.handle(AppCommand::CancelCall);
        }
    }

    fn publish_qsy(&mut self) {
        let view = self.qsy_view(Instant::now());
        let key = |v: &Option<QsyView>| v.as_ref().map(|v| (v.stage.clone(), v.chosen, v.manual.clone()));
        if key(&view) != key(&self.qsy.shown) {
            self.qsy.shown = view.clone();
            self.emit(AppEvent::Qsy { qsy: view });
        }
    }

    /// The card for the snapshot: `None` when no QSY is under way.
    pub(super) fn qsy_view(&self, now: Instant) -> Option<QsyView> {
        let m = self.qsy.machine.as_ref().filter(|m| m.pending())?;
        let mut v = m.view(now);
        v.manual = if self.qsy.tuned { None } else { v.manual.map(|t| self.qsy_dial(&t)) };
        Some(v)
    }

    /// The manual path's instruction: the radio's dial, which is the
    /// displayed frequency less this station's own offset (B3, B4).
    fn qsy_dial(&self, t: &Tuning) -> Tuning {
        let dial = i64::try_from(t.freq_hz).unwrap_or(i64::MAX) - i64::from(self.rig_offset_hz());
        Tuning { freq_hz: u64::try_from(dial).unwrap_or(0), ..t.clone() }
    }

    /// The session ended: the machine hears DISCONNECTED, with the tuning to come back to.
    pub(super) fn qsy_session_ended(&mut self) {
        if !self.qsy_pending() {
            return;
        }
        let now = Instant::now();
        let original = self.qsy_original(now);
        if let (Some(peer), Some(q)) = (self.qsy.peer.clone(), self.qso.as_ref()) {
            self.qsy.first_qso = Some((peer, q.id.clone()));
        }
        let _ = self.qsy_input(QsyInput::Disconnected { now, original });
    }

    /// A session began: on the new frequency, the peer has been found.
    /// Returns the QSO the new record links to, if it is the QSY's re-call.
    pub(super) fn qsy_session_started(&mut self, peer: &str) -> Option<String> {
        self.qsy_peer_seen();
        let (p, id) = self.qsy.first_qso.take()?;
        (p == peer).then_some(id)
    }

    /// PENDING or CONNECTED on the new frequency. It also cancels a fall
    /// back whose retune has not gone out yet (B3: a connection in progress
    /// beats a timer); one already sent to the rig completes.
    pub(super) fn qsy_peer_seen(&mut self) {
        let stage = self.qsy.machine.as_ref().map(|m| m.view(Instant::now()).stage);
        let unsent = self.qsy.retune.as_ref().is_none_or(|r| r.in_flight.is_empty());
        let cancel = match stage.as_deref() {
            Some("moving") => true,
            Some("fallingBack") => unsent,
            _ => false,
        };
        if cancel {
            self.qsy.retune = None;
            let _ = self.qsy_input(QsyInput::PeerSeen);
        }
    }

    /// A call failed: while the window is open the caller calls again.
    pub(super) fn qsy_call_failed(&mut self) {
        if self.qsy.calling {
            self.qsy.next_call = Some(Instant::now() + RECALL_GAP);
        }
    }

    /// The displayed tuning now: the rig's, else the telemetry frequency;
    /// `None` when neither knows it (M4 B2).
    fn qsy_original(&self, now: Instant) -> Option<Tuning> {
        let v = self.rig.policy.view(now);
        if let (true, Some(f)) = (self.rig.policy.link_up(), v.frequency.as_ref()) {
            return Some(Tuning { freq_hz: f.value, mode: v.mode.unwrap_or_default(), passband_hz: v.passband_hz });
        }
        let freq = self.tuning(now).freq.map(|f| f.value).filter(|hz| *hz > 0)?;
        Some(Tuning { freq_hz: freq, mode: String::new(), passband_hz: None })
    }

    pub(super) fn qsy_wake(&self) -> Option<Instant> {
        let deadline = self.qsy.machine.as_ref().and_then(Qsy::deadline);
        [deadline, self.qsy.next_call].into_iter().flatten().min()
    }

    /// After every turn of the loop: the windows, the retune and the re-call.
    pub(super) fn sync_qsy(&mut self) {
        let now = Instant::now();
        if self.qsy.machine.as_ref().and_then(Qsy::deadline).is_some_and(|d| now >= d) {
            let _ = self.qsy_input(QsyInput::Tick(now));
        }
        self.send_retune();
        let due = self.qsy.next_call.is_some_and(|t| now >= t);
        let idle = matches!(self.modem, ModemState::Ready | ModemState::Listening) && self.session.is_none();
        if self.qsy.calling && due && idle && self.qsy.retune.is_none() {
            self.qsy.next_call = None;
            let Some(peer) = self.qsy.peer.clone() else { return };
            if let Err(e) = self.handle(AppCommand::Call { peer }) {
                tracing::debug!("the QSY re-call was not made: {e}");
                self.qsy.next_call = Some(now + RECALL_GAP);
            }
        }
    }

    /// Send the retune once the station is idle and Mercury's PTT is off.
    fn send_retune(&mut self) {
        let Some(r) = self.qsy.retune.as_ref() else { return };
        if !r.in_flight.is_empty() || !self.retune_clear() {
            return;
        }
        let t = r.tuning.clone();
        let mut changes = vec![RigChange::Frequency { hz: t.freq_hz }];
        if !t.mode.is_empty() {
            changes.push(RigChange::Mode { mode: t.mode.clone(), passband_hz: t.passband_hz.unwrap_or(0) });
        }
        match self.rig_set_all_ids(&changes) {
            Ok(ids) => {
                if let Some(r) = self.qsy.retune.as_mut() {
                    r.in_flight = ids;
                    r.tries += 1;
                }
            }
            Err(e) => {
                tracing::info!("the QSY retune was refused: {e}");
                if let Some(r) = self.qsy.retune.as_mut() {
                    r.tries += 1;
                }
                self.retune_attempt_failed();
            }
        }
    }

    /// An attempt failed: retry once, then the manual path.
    fn retune_attempt_failed(&mut self) {
        let Some(r) = self.qsy.retune.as_mut() else { return };
        r.in_flight.clear();
        r.failed = false;
        if r.tries < 2 {
            return; // sent again on the next turn
        }
        self.qsy.retune = None;
        let _ = self.qsy_input(QsyInput::RigSet { ok: false });
    }

    /// One of the retune's rig requests was answered.
    pub(super) fn qsy_rig_done(&mut self, id: u64, ok: bool) {
        let Some(r) = self.qsy.retune.as_mut() else { return };
        let Some(i) = r.in_flight.iter().position(|x| *x == id) else { return };
        r.in_flight.remove(i);
        r.failed |= !ok;
        if !r.in_flight.is_empty() {
            return;
        }
        if r.failed {
            self.retune_attempt_failed();
        } else {
            self.qsy.retune = None;
            let _ = self.qsy_input(QsyInput::RigSet { ok: true });
        }
    }

    /// "Suggest QSY" on a heard station (B5): one kind 3, only on this command.
    pub(super) fn suggest_qsy(&mut self, to: String, entry_ids: Vec<String>) -> Result<CommandOk, AppError> {
        let field = |message: String| AppError::Validation { errors: crate::app::chat::field("entryIds", message) };
        let me = self.active_profile().ok_or_else(|| conflict("no station is selected".into()))?;
        if entry_ids.is_empty() || entry_ids.len() > MAX_QSY_CANDIDATES {
            return Err(field(format!("Choose 1 to {MAX_QSY_CANDIDATES} frequencies")));
        }
        let entries = self.entries()?;
        let mut candidates = Vec::with_capacity(entry_ids.len());
        for id in &entry_ids {
            let e =
                entries.iter().find(|e| &e.id == id).ok_or_else(|| conflict("that entry no longer exists".into()))?;
            let mode = Mode::from_name(&e.mode)
                .ok_or_else(|| field(format!("{}: the mode {} can't be suggested", e.name, e.mode)))?;
            let hz = u32::try_from(e.freq_hz).map_err(|_| field(format!("{}: frequency out of range", e.name)))?;
            candidates.push((hz, mode));
        }
        let s = QsySuggestion::new(self.qsy.suggest_seq, &me.callsign, &to, candidates)
            .map_err(|e| AppError::Validation { errors: crate::app::chat::field("to", format!("not sent: {e}")) })?;
        let payload = match self.listen_mode.current() {
            Some(mode) => {
                s.encode_for(mode).map_err(|e| self.refuse_broadcast(BroadcastKind::Qsy, format!("not sent: {e}")))?
            }
            None => s.encode(), // the gate refuses it without a listen mode
        };
        self.send_broadcast(BroadcastKind::Qsy, payload)?;
        self.qsy.suggest_seq = self.qsy.suggest_seq.wrapping_add(1);
        Ok(CommandOk::Done)
    }

    /// A broadcast frame: a kind 3 addressed to us is shown as a card.
    pub(super) fn qsy_frame(&mut self, payload: &[u8]) {
        let Received::Qsy(q) = mcb::classify(payload) else { return };
        let Some(me) = self.active_profile() else { return };
        if !q.to.eq_ignore_ascii_case(me.callsign.trim()) {
            return;
        }
        let candidates: Vec<Tuning> = q
            .candidates
            .iter()
            .map(|(hz, mode)| Tuning { freq_hz: u64::from(*hz), mode: mode.name().to_string(), passband_hz: None })
            .collect();
        let first = candidates.first().map(describe).unwrap_or_default();
        let line = self.discovery.note("qsy", format!("{} suggests {first}", q.from));
        self.emit(line);
        self.qsy.suggestion = Some((q.from.clone(), candidates.clone()));
        self.emit(AppEvent::QsySuggested { from: q.from, candidates });
    }

    /// "Tune and listen": the M3b rig set, refused while busy (RIG-03).
    pub(super) fn tune_suggested(&mut self, index: u8) -> Result<CommandOk, AppError> {
        self.qsy_owns_rig()?;
        let t = self
            .qsy
            .suggestion
            .as_ref()
            .and_then(|(_, c)| c.get(usize::from(index)))
            .cloned()
            .ok_or_else(|| conflict("there is no such suggestion".into()))?;
        let mut changes = vec![RigChange::Frequency { hz: t.freq_hz }];
        if !t.mode.is_empty() {
            changes.push(RigChange::Mode { mode: t.mode.clone(), passband_hz: 0 });
        }
        self.rig_set_all(&changes)
    }
}
