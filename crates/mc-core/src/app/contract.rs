//! The typed contract between the UI and the core. The UI sends `AppCommand`
//! and gets `CommandOk` or `AppError`; it never touches sockets, files or the
//! database directly (docs/02 §2, §7).

use super::events::ModemState;
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
    pub profiles: Vec<StationProfile>,
    pub active_profile_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, TS)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
#[ts(export)]
pub enum AppCommand {
    Snapshot,
    ValidateProfile { profile: StationProfile },
    SaveProfile { profile: StationProfile },
    DeleteProfile { id: String },
    SelectProfile { id: Option<String> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(tag = "type", rename_all = "camelCase")]
#[ts(export)]
pub enum CommandOk {
    Snapshot(AppSnapshot),
    Profile(StationProfile),
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
}
