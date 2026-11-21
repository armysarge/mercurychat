//! Headless Mercury adapter: control-line parsing, validated commands and a
//! paired control/data attachment. No UI dependency (docs/05 §2 WP2).

pub mod broadcast;
pub mod client;
pub mod command;
pub mod control;
#[cfg(feature = "fake")]
pub mod fake;
#[cfg(feature = "fake")]
pub mod fake_flrig;
#[cfg(feature = "fake")]
pub mod fake_rig;
#[cfg(feature = "fake")]
pub mod fake_ws;
pub mod flrig;
pub mod kiss;
pub mod monitor;
pub mod rigctl;
pub mod telemetry;

pub use client::{Ack, AdapterEvent, AttachError, Attachment, CommandError, Endpoint, MercuryClient, Timeouts};
pub use command::{Bandwidth, Callsign, Command, TuneArg};
pub use control::ControlEvent;
