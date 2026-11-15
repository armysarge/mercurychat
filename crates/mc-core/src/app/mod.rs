//! The application layer the UI talks to: typed commands in, one event stream
//! out (docs/02 §2–§3).

mod contract;
mod events;

pub use contract::{AppCommand, AppError, AppSnapshot, CommandOk};
pub use events::{is_current, AppEvent, EventEnvelope, EventLog, ModemState, Since};

use ts_rs::TS;

/// Write every UI contract type to `TS_RS_EXPORT_DIR` (`apps/desktop/src/bindings`).
pub fn export_bindings() -> Result<(), ts_rs::ExportError> {
    let cfg = ts_rs::Config::from_env();
    AppCommand::export_all(&cfg)?;
    CommandOk::export_all(&cfg)?;
    AppError::export_all(&cfg)?;
    EventEnvelope::export_all(&cfg)?;
    Since::export_all(&cfg)
}
