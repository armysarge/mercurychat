//! The application layer the UI talks to: typed commands in, one event stream
//! out (docs/02 §2–§3).

mod beacon;
mod broadcast;
mod chat;
mod contract;
mod discovery;
mod events;
mod logbook;
mod modem;
mod service;
mod telemetry;

pub use beacon::{
    BeaconPrefs, BeaconScheduler, BeaconState, BeaconStatus, ChannelState, Tick, BUSY_HOLD, DEFAULT_INTERVAL_MIN,
    MAX_INTERVAL_MIN, MIN_INTERVAL_MIN,
};
pub use broadcast::{broadcast_gate, BroadcastKind, BroadcastOutcome, GateRefusal, TRANSMIT_WINDOW};
pub use chat::{ChatMessage, Conversation, MAX_TEXT_BYTES, QUEUE_BYTES, QUEUE_MESSAGES};
pub use contract::{AppCommand, AppError, AppSnapshot, CommandOk};
pub use discovery::{ActivityEntry, DiscoveryStats, HeardSource, HeardStation, Reading};
pub use events::{is_current, AppEvent, EventEnvelope, EventLog, ModemState, SessionDirection, SessionInfo, Since};
pub use logbook::{Qso, QsoEdit, FINALISE_SETTING, OPERATOR_SOURCE};
pub use modem::{ModemCmd, ModemOptions, ModemReport, ReportKind, SpectrumFeed};
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
    AppError::export_all(&cfg)?;
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
    Since::export_all(&cfg)
}
