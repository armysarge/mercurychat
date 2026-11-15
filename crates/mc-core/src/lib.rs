#![allow(clippy::absurd_extreme_comparisons, clippy::too_many_arguments)]
//! MercuryChat headless core: station driver and MCH/1 session engine.
//! No UI dependency; the Tauri shell and the M0 test client both use it.

pub mod app;
pub mod files;
pub mod lock;
pub mod logs;
pub mod profile;
pub mod session;
#[cfg(feature = "sim")]
pub mod sim;
pub mod station;

pub use session::{ModemSignal, SendFileOpts, SessionCommand, SessionEnd, SessionEvent};
pub use station::{ConnectInfo, Station, StationConfig, StationError, StationRequest, TranscriptLine};
