//! Files as the UI sees them (M4): the per-profile file settings, a prepared
//! file before it is sent, and a transfer's row.

use crate::files::PhotoSize;
use mc_storage::{Store, TransferRow};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// The smallest and largest incoming-file limit the operator may set.
pub const MIN_INCOMING: u64 = 1_024;
pub const MAX_INCOMING: u64 = crate::session::MAX_FILE_CEILING;
/// An unanswered incoming offer is declined after this (03: 2 min).
pub const OFFER_WINDOW_S: u32 = 120;
/// Framing on top of the wire bytes, for the airtime estimate.
const FRAMING: f64 = 1.10;

/// Per profile, under the setting `profile.{id}.files`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct FileSettings {
    /// The largest incoming file, as it lands on disk (1 KiB to 256 MiB).
    #[ts(type = "number")]
    pub max_incoming: u64,
    /// Where received files land; `None` is the station's `files/` folder.
    pub receive_dir: Option<String>,
    /// The photo size a send starts at.
    pub photo_default: PhotoSize,
}

impl Default for FileSettings {
    fn default() -> Self {
        FileSettings { max_incoming: 64 * 1024, receive_dir: None, photo_default: PhotoSize::Medium }
    }
}

fn key(profile_id: &str) -> String {
    format!("profile.{profile_id}.files")
}

impl FileSettings {
    pub fn load(store: &Store, profile_id: &str) -> FileSettings {
        store.setting(&key(profile_id)).ok().flatten().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    pub fn save(&self, store: &mut Store, profile_id: &str) -> mc_storage::Result<()> {
        store.set_setting(&key(profile_id), &serde_json::to_string(self).expect("settings serialise"))
    }

    /// The field in error and why, if any.
    pub fn check(&self) -> Result<(), (&'static str, String)> {
        if !(MIN_INCOMING..=MAX_INCOMING).contains(&self.max_incoming) {
            return Err(("maxIncoming", "Choose 1 KiB to 256 MiB".into()));
        }
        if self.receive_dir.as_ref().is_some_and(|d| d.trim().is_empty()) {
            return Err(("receiveDir", "Choose a folder, or the station's own".into()));
        }
        Ok(())
    }
}

/// A file made ready to send (shrunk, compressed), before Send.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PreparedView {
    pub prep_id: String,
    pub name: String,
    #[ts(type = "number")]
    pub size: u64,
    #[ts(type = "number")]
    pub wire_size: u64,
    pub compressed: bool,
    pub shrunk: bool,
    /// An original photo whose location and camera data go with it.
    pub keeps_metadata: bool,
    /// Estimated airtime in seconds, at the measured rate if there is one.
    pub airtime_s: u32,
    pub measured: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum TransferDirection {
    Out,
    In,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum TransferState {
    Offered,
    Waiting,
    Sending,
    Receiving,
    Verifying,
    Complete,
    Rejected,
    Cancelled,
    Failed,
    Interrupted,
}

impl From<mc_storage::TransferState> for TransferState {
    fn from(s: mc_storage::TransferState) -> Self {
        use mc_storage::TransferState as S;
        match s {
            S::Offered => TransferState::Offered,
            S::Waiting => TransferState::Waiting,
            S::Sending => TransferState::Sending,
            S::Receiving => TransferState::Receiving,
            S::Verifying => TransferState::Verifying,
            S::Complete => TransferState::Complete,
            S::Rejected => TransferState::Rejected,
            S::Cancelled => TransferState::Cancelled,
            S::Failed => TransferState::Failed,
            S::Interrupted => TransferState::Interrupted,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TransferView {
    /// The transfer id on the wire, in hex.
    pub id: String,
    pub direction: TransferDirection,
    pub peer: String,
    pub original_name: String,
    /// The name it was saved under, when that differs or once saved (FIL-04).
    pub saved_name: Option<String>,
    #[ts(type = "number")]
    pub size: u64,
    #[ts(type = "number")]
    pub wire_size: u64,
    pub state: TransferState,
    /// Wire bytes so far.
    #[ts(type = "number")]
    pub bytes_done: u64,
    /// Seconds left, only once the rate has been measured.
    pub eta_s: Option<u32>,
    pub reason: Option<String>,
    /// An incoming offer's seconds left to answer.
    pub offer_expires_s: Option<u32>,
    /// When the transfer was offered or queued (UTC): its place in the
    /// conversation.
    pub created_utc: String,
}

impl From<&TransferRow> for TransferView {
    fn from(r: &TransferRow) -> Self {
        TransferView {
            id: hex::encode(r.id),
            direction: match r.direction {
                mc_storage::Direction::Out => TransferDirection::Out,
                mc_storage::Direction::In => TransferDirection::In,
            },
            peer: r.peer.clone(),
            original_name: r.original_name.clone(),
            saved_name: r.saved_name.clone(),
            size: r.size,
            wire_size: r.wire_size,
            state: r.state.into(),
            bytes_done: r.bytes_done,
            eta_s: None,
            reason: r.reason.clone(),
            offer_expires_s: None,
            created_utc: r.created_utc.clone(),
        }
    }
}

/// Seconds of airtime for `wire_size` bytes at `rate` bytes a second, with
/// framing, rounded up.
pub fn airtime_s(wire_size: u64, rate: f64) -> u32 {
    if rate <= 0.0 {
        return u32::MAX;
    }
    (wire_size as f64 * FRAMING / rate).ceil().min(u32::MAX as f64) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn airtime_has_framing_and_rounds_up() {
        assert_eq!(airtime_s(1900, 19.0), 110);
        assert_eq!(airtime_s(1, 115.0), 1);
        assert_eq!(airtime_s(0, 19.0), 0);
    }

    #[test]
    fn bounds() {
        let ok = FileSettings::default();
        assert!(ok.check().is_ok());
        assert!(FileSettings { max_incoming: 1023, ..ok.clone() }.check().is_err());
        assert!(FileSettings { max_incoming: MAX_INCOMING + 1, ..ok.clone() }.check().is_err());
        assert!(FileSettings { max_incoming: MAX_INCOMING, ..ok }.check().is_ok());
    }

    /// Owner, 24 Nov: no 1 MiB ceiling; the receiver chooses. Only a memory
    /// guard remains (files are prepared in memory).
    #[test]
    fn the_receiver_may_take_large_files() {
        let ok = FileSettings::default();
        assert!(FileSettings { max_incoming: 50 << 20, ..ok.clone() }.check().is_ok());
        assert_eq!(MAX_INCOMING, crate::session::MAX_FILE_CEILING);
        assert_eq!(MAX_INCOMING, 256 << 20);
    }
}
