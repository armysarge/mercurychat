//! Headless Mercury adapter: control-line parsing, validated commands and a
//! paired control/data attachment. No UI dependency (docs/05 §2 WP2).

pub mod client;
pub mod command;
pub mod control;
pub mod kiss;
#[cfg(feature = "fake")]
pub mod fake;

pub use client::{Ack, AdapterEvent, AttachError, Attachment, CommandError, Endpoint, MercuryClient, Timeouts};
pub use command::{Bandwidth, Callsign, Command};
pub use control::ControlEvent;
