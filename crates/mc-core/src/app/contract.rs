//! The typed contract between the UI and the core. The UI sends `AppCommand`
//! and gets `CommandOk` or `AppError`; it never touches sockets, files or the
//! database directly (docs/02 §2, §7).

use super::chat::{ChatMessage, Conversation};
use super::events::{ModemState, SessionInfo};
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
    /// The active profile's stored listen preference (RF-02): shown, never
    /// applied by itself on start-up or recovery.
    pub listen_preferred: bool,
    /// The MCH/1 session in progress, once negotiated.
    pub session: Option<SessionInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, TS)]
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
    /// Turn listening (automatic answers to incoming calls) on or off, and
    /// remember the choice on the active profile.
    SetListen {
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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(tag = "type", rename_all = "camelCase")]
#[ts(export)]
pub enum CommandOk {
    Snapshot(AppSnapshot),
    Profile(StationProfile),
    Message(ChatMessage),
    Conversations(Vec<Conversation>),
    History(Vec<ChatMessage>),
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
    /// The request does not fit the current state (e.g. no modem attached).
    #[error("{message}")]
    Conflict { message: String },
}
