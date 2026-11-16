//! The typed contract between the UI and the core. The UI sends `AppCommand`
//! and gets `CommandOk` or `AppError`; it never touches sockets, files or the
//! database directly (docs/02 §2, §7).

use super::beacon::BeaconStatus;
use super::chat::{ChatMessage, Conversation};
use super::discovery::{ActivityEntry, DiscoveryStats, HeardStation};
use super::events::{ModemState, SessionInfo};
use super::logbook::{Qso, QsoEdit};
use super::telemetry::TelemetryView;
use crate::profile::{FieldError, StationProfile};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AppSnapshot {
    #[ts(type = "number")]
    pub seq: u64,
    #[ts(type = "number")]
    pub generation: u64,
    pub modem: ModemState,
    /// Why the modem is in its state (a fault's reason), as last reported.
    pub modem_detail: Option<String>,
    pub profiles: Vec<StationProfile>,
    pub active_profile_id: Option<String>,
    /// The active profile is Receive only (D18): it never answers calls by
    /// itself. Stored on the profile; the default is to answer calls.
    pub receive_only: bool,
    /// The modem is listening for calls right now (actual state, not the
    /// setting): false while attaching, receive-only, or after a LISTEN ON
    /// failure.
    pub listening: bool,
    /// Periodic beacons (DSC-03): the stored preference is shown, never
    /// applied by itself; `active` is for this run only.
    pub beacon: BeaconStatus,
    /// The MCH/1 session in progress, once negotiated.
    pub session: Option<SessionInfo>,
    /// Broadcast traffic that was not a station, since the app started.
    pub discovery: DiscoveryStats,
    /// Link quality and Mercury's telemetry (OBS-01); `null` is unavailable.
    pub telemetry: Box<TelemetryView>,
    /// The QSO record of the latest session this run (a draft until it is
    /// finalised); `null` before the first session.
    pub current_qso: Option<Box<Qso>>,
    /// Whether a session's record is finalised when it ends (default no).
    pub finalise_on_disconnect: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, TS)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
#[ts(export)]
pub enum AppCommand {
    Snapshot,
    ValidateProfile {
        profile: StationProfile,
    },
    SaveProfile {
        profile: StationProfile,
    },
    DeleteProfile {
        id: String,
    },
    SelectProfile {
        id: Option<String>,
    },
    /// Receive only on: the station stops answering calls (LISTEN OFF, at
    /// once, even mid-session). Off: it answers calls again. Stored on the
    /// active profile (D18).
    SetReceiveOnly {
        on: bool,
    },
    /// Call a station; the session starts once it answers with MCH/1.
    Call {
        peer: String,
    },
    /// Stop a call that has not been answered yet.
    CancelCall,
    /// End the session gracefully: the modem sends what it holds first.
    Disconnect,
    /// End at once; anything not yet confirmed becomes uncertain.
    Abort,
    /// Queue a message; it goes out when a session with `peer` is up.
    SendChat {
        peer: String,
        text: String,
    },
    /// Send an uncertain or failed message again, under the same id.
    RetryChat {
        peer: String,
        id: String,
    },
    /// Keep (or, with empty text, clear) the unsent draft for a peer.
    SaveDraft {
        peer: String,
        text: String,
    },
    Conversations,
    /// Up to `limit` (at most 200) messages before `before_seq`, oldest first.
    History {
        peer: String,
        #[ts(type = "number | null")]
        before_seq: Option<i64>,
        limit: u32,
    },
    /// The heard list: stations heard in the last 30 days, then favourites.
    Heard,
    /// Mark or unmark a callsign as a favourite; it stays listed while marked.
    SetFavourite {
        callsign: String,
        on: bool,
    },
    /// The activity log, oldest first (the last 500 lines).
    Activity,
    /// Call CQ once (Mercury's CQFRAME), only while the modem is idle
    /// (DSC-02). A refused CQ is never queued for later.
    SendCq,
    /// Arm or disarm periodic beacons for this run, and remember the choice
    /// on the active profile. Arming needs an idle modem; a stored
    /// preference never arms by itself (RF-02).
    SetBeacon {
        on: bool,
    },
    /// The beacon interval in minutes, 5 to 60 (default 10), kept per profile.
    SetBeaconInterval {
        minutes: u32,
    },
    /// Send one beacon now, through the same gate as a CQ. It neither arms
    /// nor moves the scheduled beacon.
    SendBeaconNow,
    /// Whether beacons carry the profile's locator (default yes), kept per
    /// profile. An MCB/1 beacon has only callsign, locator and capabilities.
    SetBeaconContent {
        include_grid: bool,
    },
    /// Logbook records, newest first (at most 500).
    Qsos {
        limit: u32,
    },
    /// Edit a draft record; a final one is refused.
    UpdateQso {
        id: String,
        edit: Box<QsoEdit>,
    },
    /// Lock a record against further edits.
    FinaliseQso {
        id: String,
    },
    /// Finalise a record when its session ends (default off).
    SetFinaliseOnDisconnect {
        on: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(tag = "type", rename_all = "camelCase")]
#[ts(export)]
pub enum CommandOk {
    Snapshot(AppSnapshot),
    Profile(StationProfile),
    Message(ChatMessage),
    Conversations { conversations: Vec<Conversation> },
    History { messages: Vec<ChatMessage> },
    Heard { stations: Vec<HeardStation> },
    Activity { entries: Vec<ActivityEntry> },
    Qsos { qsos: Vec<Qso> },
    Qso(Qso),
    Done,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS, thiserror::Error)]
#[serde(tag = "code", rename_all = "camelCase", rename_all_fields = "camelCase")]
#[ts(export)]
pub enum AppError {
    #[error("{} field(s) need attention", errors.len())]
    Validation { errors: Vec<FieldError> },
    #[error("{message}")]
    Busy { message: String },
    #[error("no profile with id {id}")]
    NotFound { id: String },
    #[error("storage: {message}")]
    Storage { message: String },
    #[error("{message}")]
    Internal { message: String },
    /// The request does not fit the current state (e.g. no modem attached).
    #[error("{message}")]
    Conflict { message: String },
}
