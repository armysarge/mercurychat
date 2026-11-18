//! The typed contract between the UI and the core. The UI sends `AppCommand`
//! and gets `CommandOk` or `AppError`; it never touches sockets, files or the
//! database directly (docs/02 §2, §7).

use super::beacon::BeaconStatus;
use super::chat::{ChatMessage, Conversation};
use super::discovery::{ActivityEntry, DiscoveryStats, HeardStation};
use super::events::{ModemState, SessionInfo};
use super::logbook::{Qso, QsoEdit};
use super::notify::NotifyPrefs;
use super::rig::{RigBanner, RigChange, RigView, TuneView};
use super::rigconfig::{BandLevels, RigConfig};
use super::telemetry::TelemetryView;
use crate::macros::MacroDef;
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
    /// Receive only was chosen during a call or a session while the modem
    /// was answering calls: it takes effect when the link ends, because
    /// Mercury drops a live link unannounced on LISTEN OFF (lab F2).
    pub receive_only_pending: bool,
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
    /// The interface language the operator chose: `system`, `en`, `es`,
    /// `pt-BR` or `fr` (default `system`). The UI maps `system` itself.
    pub ui_language: String,
    /// Desktop notification preferences (app settings, not per profile).
    pub notify_prefs: NotifyPrefs,
    /// The rig as MercuryChat reads it through rigctld (M3b).
    pub rig: Box<RigView>,
    /// Mercury's tuning carrier (M3b).
    pub tune: TuneView,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, TS)]
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
    /// once when idle; during a call or a session, once it ends, never on
    /// the live link). Off: it answers calls again, at once. Stored on the
    /// active profile at once (D18).
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
    /// Choose the interface language: one of `system`, `en`, `es`, `pt-BR`,
    /// `fr`. Anything else is refused. Kept in the setting `ui.language`.
    SetUiLanguage {
        language: String,
    },
    /// Keep the desktop notification preferences (quiet hours `HH:MM`, both or neither).
    SetNotifyPrefs {
        prefs: Box<NotifyPrefs>,
    },
    /// The active profile's macros (the first call seeds three defaults, once).
    Macros,
    /// Add (empty id) or edit a macro of the active profile.
    SaveMacro {
        macro_: Box<MacroDef>,
    },
    DeleteMacro {
        id: String,
    },
    /// Expand a macro into text for the draft. Nothing is sent. `allow_email`
    /// is the operator's explicit "insert email" action.
    ExpandMacro {
        id: String,
        peer: Option<String>,
        allow_email: bool,
    },
    /// Messages (either direction) whose text or callsign contains `query`,
    /// newest first. `limit` is clamped to 1 to 100.
    SearchHistory {
        query: String,
        limit: u32,
    },
    /// The active profile's rig settings (M3b): stored, and the rig link
    /// restarted on them.
    SetRigConfig {
        config: RigConfig,
    },
    /// Change the rig through rigctld: frequency, mode, VFO, split or RF
    /// power. Refused while busy, with the reason (RIG-03).
    RigSet {
        change: RigChange,
    },
    /// Take a rig banner down, once it may go.
    DismissRigBanner {
        banner: RigBanner,
    },
    /// Store a band's TX and Tune levels for the active profile (RIG-04);
    /// applied at once if the rig is on that band and the station idle.
    SetBandLevels {
        band: String,
        levels: BandLevels,
    },
    /// Mercury's tuning carrier on or off (RIG-05). On: at the band's tune
    /// level, stopped by MercuryChat after 30 s. Off is always allowed.
    Tune {
        on: bool,
    },
    /// Change the tuning level while Tune is on; stored for the band.
    SetTuneLevel {
        dbfs: f32,
    },
}

// Every float in a command is validated finite before use.
impl Eq for AppCommand {}

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
    Macros { macros: Vec<MacroDef> },
    Macro(MacroDef),
    Expanded { text: String, missing: Vec<String> },
    Done,
}

/// Why a command was refused. On the wire each variant carries a stable
/// `key` (see `key`) beside its English `message`: the UI shows its own
/// translation of the key, and the message is the fallback for a key it does
/// not know (see `AppErrorWire`).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
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
    /// A change refused by a safety rule; `reason` is its i18n key.
    #[error("{message}")]
    Refused { reason: &'static str, message: String },
}

impl AppError {
    /// The stable message key the UI translates (`errors.<variant>`). An
    /// exhaustive match: a new variant must name its key here.
    pub fn key(&self) -> &'static str {
        match self {
            AppError::Validation { .. } => "errors.validation",
            AppError::Busy { .. } => "errors.busy",
            AppError::NotFound { .. } => "errors.notFound",
            AppError::Storage { .. } => "errors.storage",
            AppError::Internal { .. } => "errors.internal",
            AppError::Conflict { .. } => "errors.conflict",
            AppError::Refused { .. } => "errors.refused",
        }
    }
}

/// What `AppError` serialises to, and the source of the generated TypeScript
/// `AppError`: the old shape plus `key`.
#[derive(Serialize, TS)]
#[serde(tag = "code", rename_all = "camelCase", rename_all_fields = "camelCase")]
#[ts(export, rename = "AppError")]
pub(super) enum AppErrorWire {
    Validation { key: &'static str, errors: Vec<FieldError> },
    Busy { key: &'static str, message: String },
    NotFound { key: &'static str, id: String },
    Storage { key: &'static str, message: String },
    Internal { key: &'static str, message: String },
    Conflict { key: &'static str, message: String },
    Refused { key: &'static str, reason: &'static str, message: String },
}

impl From<&AppError> for AppErrorWire {
    fn from(e: &AppError) -> Self {
        let key = e.key();
        match e {
            AppError::Validation { errors } => AppErrorWire::Validation { key, errors: errors.clone() },
            AppError::Busy { message } => AppErrorWire::Busy { key, message: message.clone() },
            AppError::NotFound { id } => AppErrorWire::NotFound { key, id: id.clone() },
            AppError::Storage { message } => AppErrorWire::Storage { key, message: message.clone() },
            AppError::Internal { message } => AppErrorWire::Internal { key, message: message.clone() },
            AppError::Conflict { message } => AppErrorWire::Conflict { key, message: message.clone() },
            AppError::Refused { reason, message } => AppErrorWire::Refused { key, reason, message: message.clone() },
        }
    }
}

impl Serialize for AppError {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        AppErrorWire::from(self).serialize(serializer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// The key each variant must serialise. A match with no wildcard: adding a
    /// variant to `AppError` stops this test compiling until it is covered here.
    fn expected_key(e: &AppError) -> &'static str {
        match e {
            AppError::Validation { .. } => "errors.validation",
            AppError::Busy { .. } => "errors.busy",
            AppError::NotFound { .. } => "errors.notFound",
            AppError::Storage { .. } => "errors.storage",
            AppError::Internal { .. } => "errors.internal",
            AppError::Conflict { .. } => "errors.conflict",
            AppError::Refused { .. } => "errors.refused",
        }
    }

    fn samples() -> Vec<AppError> {
        vec![
            AppError::Validation { errors: vec![] },
            AppError::Busy { message: "b".into() },
            AppError::NotFound { id: "x".into() },
            AppError::Storage { message: "s".into() },
            AppError::Internal { message: "i".into() },
            AppError::Conflict { message: "c".into() },
            AppError::Refused { reason: "rig.refused.connected", message: "r".into() },
        ]
    }

    #[test]
    fn every_app_error_has_a_key() {
        let mut seen = HashSet::new();
        for e in samples() {
            let key = expected_key(&e);
            assert_eq!(e.key(), key);
            assert!(key.starts_with("errors."), "{key}");
            assert!(seen.insert(key), "{key} is used twice");
            let json = serde_json::to_value(&e).unwrap();
            assert_eq!(json["key"], key, "serialised beside the code: {json}");
            assert!(json["code"].is_string(), "{json}");
            if !matches!(e, AppError::Validation { .. } | AppError::NotFound { .. }) {
                assert!(json["message"].is_string(), "the English message stays as the fallback: {json}");
            }
        }
        assert_eq!(seen.len(), samples().len());
    }
}
