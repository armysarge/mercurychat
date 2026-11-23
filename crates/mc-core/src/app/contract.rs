//! The typed contract between the UI and the core. The UI sends `AppCommand`
//! and gets `CommandOk` or `AppError`; it never touches sockets, files or the
//! database directly (docs/02 §2, §7).

use super::audio::AudioSettingsView;
use super::beacon::{BeaconContentView, BeaconStatus};
use super::chat::{ChatMessage, Conversation};
use super::discovery::{ActivityEntry, DiscoveryStats, HeardStation};
use super::endpoints::EndpointCheck;
use super::events::{ModemDetail, ModemState, SessionInfo};
use super::export::ExportFormat;
use super::files::{FileSettings, PreparedView, TransferView};
use super::frequencies::FrequencyEntry;
use super::info::ShareConfig;
use super::logbook::{Qso, QsoEdit};
use super::notify::NotifyPrefs;
use super::qsy::QsyView;
use super::rig::{RigBanner, RigChange, RigTestStep, RigView, TuneView};
use super::rigconfig::{BandLevels, HamlibModelView, RigConfig};
use super::schedule::{Schedule, ScheduleNext};
use super::telemetry::TelemetryView;
use super::wizard::WizardState;
use crate::macros::MacroDef;
use crate::profile::{FieldError, StationProfile};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// The built-in modem of this build (M5): shown next to every mention of
/// Mercury and Hamlib.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct BuiltInView {
    /// This build has a built-in modem (a release build, or a lab bundle).
    pub available: bool,
    /// Without a leading "v"; empty when unknown.
    pub mercury: String,
    pub hamlib: String,
    /// `null`: every shipped file is as built; else what is wrong, in words.
    pub integrity: Option<String>,
}

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
    /// The built-in modem's own state (M5): starting, stopped, or waiting
    /// for a sound card. `null` for other profiles.
    pub builtin_detail: Option<ModemDetail>,
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
    /// The next scheduled QSY, while the schedule is on (M3b).
    pub schedule_next: Option<ScheduleNext>,
    /// The QSY under way, if any (M3c B3).
    pub qsy: Option<QsyView>,
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
    /// What else the beacon carries (M3c B7): the profile's name and QTH, and
    /// a status line of up to 40 bytes. Any of them makes it kind 2. Refused,
    /// unsaved, when the beacon would not fit the listen mode.
    SetBeaconFields {
        name: bool,
        qth: bool,
        status: Option<String>,
    },
    /// The beacon's content and the bytes left at the current listen mode.
    BeaconContent,
    /// The bytes left were these fields chosen; nothing is saved (Settings →
    /// Beacon's live count).
    PreviewBeaconFields {
        name: bool,
        qth: bool,
        status: Option<String>,
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
    /// The active profile's stored TX and Tune levels, by band.
    BandLevels,
    /// The active profile's rig settings, region and schedule (Settings: Rig).
    RigSettings,
    /// Try these rig settings on a separate, short-lived connection that only
    /// reads; the running link is not touched.
    TestRig {
        config: RigConfig,
    },
    /// The built-in modem of this build (M5 G4): its versions and whether
    /// every shipped file is as built. Checked off the actor.
    BuiltInInfo,
    /// Are these built-in ports free on 127.0.0.1? The first taken one and a
    /// free set when not (M5 §5.1); nothing is started.
    CheckBuiltInPorts {
        ports: crate::profile::BuiltInPorts,
    },
    /// SET-03, read-only: connect to each of My own Mercury's endpoints and
    /// close again, nothing sent; each named with what went wrong in words.
    /// Nothing is saved or selected.
    CheckEndpoints {
        modem: crate::profile::ModemTarget,
    },
    /// This computer's serial ports for the rig's CAT and PTT, from the
    /// system's records (none is opened), in natural order.
    SerialPorts,
    /// Write the active station's logbook to `path` (the operator's save
    /// dialog): ADIF 3.1.4 or CSV, all records or only finalised ones,
    /// oldest first (M5 §7).
    ExportLogbook {
        format: ExportFormat,
        finalised_only: bool,
        path: String,
    },
    /// Write one conversation to `path` as plain UTF-8 text with UTC times;
    /// files are listed by name, never their contents.
    ExportConversation {
        peer: String,
        path: String,
    },
    /// The first-run wizard's progress (M5 §6); an install with stations and
    /// no stored progress reads as done.
    GetWizard,
    /// Keep the wizard's progress. While it has not passed "How you
    /// operate", its draft station is held in Receive only.
    SetWizard {
        state: WizardState,
    },
    /// Hamlib's rig models, from `rigctld -l` (rigctld on the PATH, else in
    /// `hamlib_dir`); `found: false` when there is no rigctld (M3b.1 §5.3).
    HamlibModels {
        hamlib_dir: String,
    },
    /// The CAT log: MercuryChat's rigctld's stderr, then MercuryChat's own rig
    /// link errors, each the last 200 lines.
    CatLog,
    /// The app is exiting: stop what MercuryChat started (its own rigctld)
    /// and answer once it has gone (M3b.1 §5.3).
    Shutdown,
    /// Mercury's sound devices, from its UI link (M3b.1 §6).
    AudioSettings,
    /// Apply sound devices in Mercury (`set_audio_config`); refused while the
    /// station is busy. Mercury's verdict arrives as an activity note.
    SetAudio {
        system: String,
        input: String,
        output: String,
        channel: String,
    },
    /// The frequency list (shared by every profile), by sort then frequency.
    Frequencies,
    /// Add (empty id) or change an entry; answered with the whole list.
    SaveFrequency {
        entry: FrequencyEntry,
    },
    DeleteFrequency {
        id: String,
    },
    /// Add back the region's suggestions that are missing; custom entries
    /// and edited suggestions are left alone.
    RestoreSuggested,
    /// The active profile's IARU region (1, 2 or 3) for suggestions.
    SetRegion {
        region: u8,
    },
    /// Tune the rig to an entry: frequency, then mode, one refusal for both.
    TuneToEntry {
        id: String,
    },
    /// The active profile's QSY schedule (off by default).
    SetSchedule {
        schedule: Schedule,
    },
    /// Ask the session's peer for its SNR of us or its station info (M3c B6).
    /// Refused without a session or when the peer lacks `info/1`.
    RequestInfo {
        #[ts(type = "\"snr\" | \"station\"")]
        what: mch_protocol::payload::InfoWhat,
    },
    /// Send the profile's email to the peer, once, as an unsolicited answer.
    /// The UI asks for confirmation first. Refused with no email or no `info/1`.
    SendEmail,
    /// What the active profile shares in answers.
    ShareSettings,
    SetShareSettings {
        config: ShareConfig,
    },
    /// Propose a QSY to the session's peer (M3c B3): 1 to 5 frequency-list
    /// entries. Refused without `qsy/1`, or while a QSY is already pending.
    ProposeQsy {
        entry_ids: Vec<String>,
        note: Option<String>,
    },
    /// Answer the peer's proposal: a candidate index, or `None` to decline.
    AnswerQsy {
        accept: Option<u8>,
    },
    /// The manual path's "Tuned": an acknowledgement only.
    QsyTuned,
    /// Suggest a QSY to a heard station before any connection (M3c B5): 1 to
    /// 3 frequency-list entries in one MCB/1 kind 3, through the CQ gate.
    SuggestQsy {
        to: String,
        entry_ids: Vec<String>,
    },
    /// "Tune and listen" on the last suggestion addressed to us: the M3b rig
    /// set, refused while busy (RIG-03).
    TuneSuggested {
        index: u8,
    },
    /// Make a file ready to send (M4): shrink a photo to `photo`, compress,
    /// and say what it costs. Answered once done; nothing is sent.
    PrepareFile {
        path: String,
        photo: crate::files::PhotoSize,
    },
    /// The operator closed the send dialog: the prepared file goes.
    DiscardPrepared {
        prep_id: String,
    },
    /// Offer a prepared file to the session's peer. Refused without a
    /// session, without `file/2`, or over the peer's limit (shown).
    SendFile {
        prep_id: String,
    },
    AcceptFile {
        id: String,
    },
    DeclineFile {
        id: String,
    },
    /// One more answer window for an incoming offer (once).
    ExtendFile {
        id: String,
    },
    /// Stop a transfer either way, or one still waiting to go.
    CancelFile {
        id: String,
    },
    /// The transfers, newest first (at most 200).
    Transfers,
    /// Open the file manager at a received file; the file itself is never
    /// opened (FIL-04).
    ShowInFolder {
        id: String,
    },
    /// The active profile's file settings.
    FileSettings,
    SetFileSettings {
        settings: FileSettings,
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
    Conversations {
        conversations: Vec<Conversation>,
    },
    History {
        messages: Vec<ChatMessage>,
    },
    Heard {
        stations: Vec<HeardStation>,
    },
    Activity {
        entries: Vec<ActivityEntry>,
    },
    Frequencies {
        entries: Vec<FrequencyEntry>,
    },
    Restored {
        added: u32,
    },
    RigTest {
        steps: Vec<RigTestStep>,
    },
    RigSettings {
        config: RigConfig,
        region: u8,
        schedule: Schedule,
    },
    HamlibModels {
        models: Vec<HamlibModelView>,
        found: bool,
    },
    BuiltIn(BuiltInView),
    PortCheck {
        taken: Option<u16>,
        suggestion: Option<crate::profile::BuiltInPorts>,
    },
    Wizard(WizardState),
    /// An export written: how many records (or messages) and where.
    Exported {
        count: u32,
        path: String,
    },
    SerialPorts {
        ports: Vec<String>,
    },
    Endpoints {
        results: Vec<EndpointCheck>,
    },
    CatLog {
        lines: Vec<String>,
    },
    Audio(AudioSettingsView),
    BandLevels {
        levels: std::collections::BTreeMap<String, BandLevels>,
    },
    Qsos {
        qsos: Vec<Qso>,
    },
    Qso(Qso),
    Macros {
        macros: Vec<MacroDef>,
    },
    Macro(MacroDef),
    Expanded {
        text: String,
        missing: Vec<String>,
    },
    Share(ShareConfig),
    BeaconContent(BeaconContentView),
    Prepared(PreparedView),
    Transfers {
        transfers: Vec<TransferView>,
    },
    FileSettings(FileSettings),
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
