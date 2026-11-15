//! Chat as the UI sees it (CHT-01 to CHT-04): messages, conversations, and
//! the limits a message must fit before it is queued.

use crate::profile::FieldError;
use mc_storage::MessageRow;
use serde::Serialize;
use ts_rs::TS;

/// CHT-01: at most this many UTF-8 bytes in one message (and one draft).
pub const MAX_TEXT_BYTES: usize = mch_protocol::MAX_CHAT_BYTES;
/// CHT-03: at most this many messages waiting for one peer.
pub const QUEUE_MESSAGES: u32 = 100;
/// CHT-03: at most this many bytes waiting for one peer (256 KiB).
pub const QUEUE_BYTES: u64 = 256 * 1024;
/// Most messages one History page returns.
pub const HISTORY_PAGE: u32 = 200;

/// One message. `state` is a CHT-02 wire name for outgoing messages
/// (queued, submitting, awaiting_receipt, delivered, failed, uncertain) and
/// `received` for incoming ones.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ChatMessage {
    /// The stable message id, 32 hex digits.
    pub id: String,
    pub peer: String,
    /// `out` or `in`.
    pub direction: String,
    pub body: String,
    pub created_utc: String,
    pub received_utc: Option<String>,
    pub state: String,
    /// Local order: pages and the window sort by it.
    #[ts(type = "number")]
    pub seq: i64,
}

impl From<&MessageRow> for ChatMessage {
    fn from(r: &MessageRow) -> Self {
        ChatMessage {
            id: hex::encode(r.wire_id),
            peer: r.peer.clone(),
            direction: r.direction.clone(),
            body: r.body.clone(),
            created_utc: r.created_utc.clone(),
            received_utc: r.received_utc.clone(),
            state: r.state.clone(),
            seq: r.seq,
        }
    }
}

/// One peer's conversation, with the operator's unsent draft if any.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Conversation {
    pub peer: String,
    pub last_utc: String,
    #[ts(type = "number")]
    pub messages: i64,
    #[ts(type = "number")]
    pub uncertain: i64,
    pub draft: Option<String>,
}

pub fn field(field: &str, message: impl Into<String>) -> Vec<FieldError> {
    vec![FieldError { field: field.into(), message: message.into() }]
}

/// CHT-01's size limit, counted in UTF-8 bytes.
pub fn check_size(text: &str) -> Result<(), Vec<FieldError>> {
    if text.len() > MAX_TEXT_BYTES {
        return Err(field(
            "text",
            format!("a message can be at most {MAX_TEXT_BYTES} bytes; this one is {} bytes", text.len()),
        ));
    }
    Ok(())
}

/// The settings key a peer's draft is kept under.
pub fn draft_key(peer: &str) -> String {
    format!("draft:{peer}")
}
