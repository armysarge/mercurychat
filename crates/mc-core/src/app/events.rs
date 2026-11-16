//! The application event stream (docs/02 §3). Every event has a sequence
//! number, rising by one from 1, and the modem-session generation it belongs
//! to. A bounded ring keeps the recent ones; a client asking for events older
//! than the ring holds is told to take a snapshot, never handed a gap.

use super::beacon::BeaconStatus;
use super::broadcast::{BroadcastKind, BroadcastOutcome};
use super::chat::ChatMessage;
use super::discovery::{ActivityEntry, HeardStation};
use super::logbook::Qso;
use super::telemetry::TelemetryView;
use serde::Serialize;
use std::collections::VecDeque;
use ts_rs::TS;

/// MOD-01, in order. A connected TCP socket alone is never `Connected`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(tag = "type", rename_all = "camelCase")]
#[ts(export)]
pub enum ModemState {
    Unavailable,
    Attaching,
    Ready,
    Listening,
    Calling,
    Negotiating,
    Connected,
    Draining,
    Recovering,
    Faulted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum SessionDirection {
    Outgoing,
    Incoming,
}

/// The negotiated MCH/1 session in progress.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SessionInfo {
    pub peer: String,
    pub direction: SessionDirection,
    pub since_utc: String,
    pub peer_app: Option<String>,
    /// One id per session; a new id is a new session (and a new QSO draft).
    pub session_id: String,
    /// The negotiated bandwidth token the modem reported.
    pub bandwidth: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
#[ts(export)]
pub enum AppEvent {
    ModemState {
        state: ModemState,
        detail: Option<String>,
    },
    /// A session began (Some) or ended (None).
    Session {
        session: Option<SessionInfo>,
    },
    /// A call did not lead to a session; `reason` says why, in plain words.
    CallFailed {
        peer: String,
        reason: String,
    },
    /// A message was queued, changed state, or arrived.
    MessageChanged {
        message: ChatMessage,
    },
    ProfilesChanged,
    ActiveProfileChanged {
        id: Option<String>,
    },
    Fault {
        code: String,
        message: String,
    },
    /// A station was heard, or its favourite mark changed (DSC-01).
    Heard {
        station: HeardStation,
    },
    /// A line was added to the activity log.
    Activity {
        entry: ActivityEntry,
    },
    /// What became of a CQ or beacon: submitted, transmitted, not confirmed
    /// or refused. Never delivered (DSC-02).
    Broadcast {
        kind: BroadcastKind,
        outcome: BroadcastOutcome,
    },
    /// The beacon schedule, its suppression or the channel state changed.
    Beacon {
        status: BeaconStatus,
    },
    /// The telemetry view changed (OBS-01); at most 2 a second, and the
    /// snapshot always has the latest.
    Telemetry {
        telemetry: TelemetryView,
    },
    /// A QSO record was drafted, edited, ended or finalised.
    Qso {
        qso: Qso,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct EventEnvelope {
    #[ts(type = "number")]
    pub seq: u64,
    #[ts(type = "number")]
    pub generation: u64,
    pub event: AppEvent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(tag = "type", rename_all = "camelCase")]
#[ts(export)]
pub enum Since {
    Events { events: Vec<EventEnvelope> },
    SnapshotNeeded,
}

pub struct EventLog {
    ring: VecDeque<EventEnvelope>,
    capacity: usize,
    last_seq: u64,
    generation: u64,
}

impl EventLog {
    pub fn new(capacity: usize) -> EventLog {
        EventLog { ring: VecDeque::with_capacity(capacity), capacity: capacity.max(1), last_seq: 0, generation: 0 }
    }

    pub fn push(&mut self, event: AppEvent) -> EventEnvelope {
        self.last_seq += 1;
        let env = EventEnvelope { seq: self.last_seq, generation: self.generation, event };
        if self.ring.len() == self.capacity {
            self.ring.pop_front();
        }
        self.ring.push_back(env.clone());
        env
    }

    /// Start a new modem-session generation; events from older ones are stale.
    pub fn bump_generation(&mut self) -> u64 {
        self.generation += 1;
        self.generation
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn last_seq(&self) -> u64 {
        self.last_seq
    }

    /// Events after `seq`, or `SnapshotNeeded` if some of them have left the ring.
    pub fn since(&self, seq: u64) -> Since {
        let oldest = self.ring.front().map_or(self.last_seq + 1, |e| e.seq);
        if seq + 1 < oldest {
            return Since::SnapshotNeeded;
        }
        Since::Events { events: self.ring.iter().filter(|e| e.seq > seq).cloned().collect() }
    }
}

/// Whether an event belongs to the current generation (docs/02 §3: ignore
/// stale events from previous socket generations).
pub fn is_current(env: &EventEnvelope, generation: u64) -> bool {
    env.generation == generation
}
