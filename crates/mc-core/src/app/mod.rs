//! The application layer the UI talks to: typed commands in, one event stream
//! out (docs/02 §2–§3).

mod audio;
mod beacon;
mod broadcast;
mod chat;
mod contract;
mod discovery;
mod events;
mod frequencies;
mod logbook;
mod modem;
mod notify;
mod rig;
mod rigconfig;
pub mod schedule;
mod service;
mod telemetry;

pub use crate::macros::{expand, Expanded, MacroContext, MacroDef};
pub use audio::{AudioDeviceView, AudioSettingsView};
pub use beacon::{
    BeaconPrefs, BeaconScheduler, BeaconState, BeaconStatus, ChannelState, Tick, BUSY_HOLD, DEFAULT_INTERVAL_MIN,
    MAX_INTERVAL_MIN, MIN_INTERVAL_MIN, SILENT_HOLD,
};
pub use broadcast::{
    broadcast_gate, BroadcastKind, BroadcastOutcome, GateRefusal, NotConfirmedReason, TRANSMIT_WINDOW,
};
pub use chat::{ChatMessage, Conversation, MAX_TEXT_BYTES, QUEUE_BYTES, QUEUE_MESSAGES};
pub use contract::{AppCommand, AppError, AppSnapshot, CommandOk};
pub use discovery::{ActivityEntry, DiscoveryStats, HeardSource, HeardStation, Reading};
pub use events::{is_current, AppEvent, EventEnvelope, EventLog, ModemState, SessionDirection, SessionInfo, Since};
pub use frequencies::{FrequencyEntry, DEFAULT_MODES, MAX_ENTRY_HZ, PURPOSES, SEEDED_SETTING};
pub use logbook::{Qso, QsoEdit, FINALISE_SETTING, OPERATOR_SOURCE};
pub use modem::{ModemCmd, ModemOptions, ModemReport, ReportKind, SpectrumFeed};
pub use notify::{NotifyPrefs, NOTIFY_SETTING};
pub use rig::{
    rig_gate, Activity, OwnerCheck, RigAction, RigBanner, RigCapsView, RigChange, RigLinkView, RigMeters, RigPolicy,
    RigPttView, RigRefusal, RigTestStep, RigView, TuneView, TxLevelResult, AGREE_TO_CLEAR, PTT_CHECK_OFF_AFTER,
    PTT_WINDOW, RIG_SOURCE,
};
pub use rigconfig::{
    mercury_ptt_for, ptt_echo_matches, rigctld_spec_for, BandLevels, CatLink, CatSettings, Endpoint, FreqControl,
    HamlibModelView, LineState, Parity, PttLine, PttMethod, PttSettings, RigConfig, READ_EVERY_CHOICES, TUNE_RANGE,
    TX_GAIN_RANGE,
};
pub use schedule::{Schedule, ScheduleNext, ScheduleSlot, MAX_SLOTS};
pub use service::{spawn_app, AppHandle, AppOptions};
pub use telemetry::{
    AudioView, TelemetryLinkView, TelemetryView, Tuning, CONTROL_SOURCE, EVENT_GAP, FREQ_FRESH_MS, OFFSET_FRESH_MS,
    TELEMETRY_SOURCE,
};

use ts_rs::TS;

/// Write every UI contract type to `TS_RS_EXPORT_DIR` (`apps/desktop/src/bindings`).
pub fn export_bindings() -> Result<(), ts_rs::ExportError> {
    let cfg = ts_rs::Config::from_env();
    AppCommand::export_all(&cfg)?;
    CommandOk::export_all(&cfg)?;
    contract::AppErrorWire::export_all(&cfg)?;
    EventEnvelope::export_all(&cfg)?;
    HeardStation::export_all(&cfg)?;
    ActivityEntry::export_all(&cfg)?;
    DiscoveryStats::export_all(&cfg)?;
    BroadcastKind::export_all(&cfg)?;
    BroadcastOutcome::export_all(&cfg)?;
    BeaconStatus::export_all(&cfg)?;
    ChannelState::export_all(&cfg)?;
    TelemetryView::export_all(&cfg)?;
    Qso::export_all(&cfg)?;
    QsoEdit::export_all(&cfg)?;
    MacroDef::export_all(&cfg)?;
    NotifyPrefs::export_all(&cfg)?;
    RigConfig::export_all(&cfg)?;
    BandLevels::export_all(&cfg)?;
    RigView::export_all(&cfg)?;
    RigChange::export_all(&cfg)?;
    Since::export_all(&cfg)
}
