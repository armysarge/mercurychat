#![allow(clippy::too_many_arguments)]
//! SQLite persistence (docs/02 §6). One writer per station data root.
//!
//! Delivery semantics enforced here:
//! * An incoming CHAT is committed together with its dedup key, with
//!   `synchronous=FULL`, *before* the caller may emit CHAT_RECEIPT.
//! * Dedup keys live in their own table so deleting history does not
//!   re-enable duplicates inside the retention window.
//! * Outgoing states: queued → submitting → awaiting_receipt → delivered;
//!   socket loss after a write gives `uncertain`, never failed/delivered.

use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;

pub type Result<T> = std::result::Result<T, StorageError>;

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("database schema version {0} is newer than this build supports")]
    TooNew(i64),
}

const SCHEMA_VERSION: i64 = 1;

const MIGRATION_1: &str = r#"
CREATE TABLE messages (
  rowid_pk     INTEGER PRIMARY KEY,
  wire_id      BLOB NOT NULL CHECK (length(wire_id) = 16),
  peer         TEXT NOT NULL,
  direction    TEXT NOT NULL CHECK (direction IN ('in','out')),
  body         TEXT NOT NULL,
  created_utc  TEXT NOT NULL,
  received_utc TEXT,
  state        TEXT NOT NULL,
  session_gen  INTEGER,
  attempts     INTEGER NOT NULL DEFAULT 0,
  UNIQUE (peer, direction, wire_id)
);
CREATE TABLE dedup (
  peer       TEXT NOT NULL,
  kind       TEXT NOT NULL,
  wire_id    BLOB NOT NULL,
  first_utc  TEXT NOT NULL,
  PRIMARY KEY (peer, kind, wire_id)
);
CREATE TABLE transfers (
  rowid_pk     INTEGER PRIMARY KEY,
  wire_id      BLOB NOT NULL CHECK (length(wire_id) = 16),
  peer         TEXT NOT NULL,
  direction    TEXT NOT NULL CHECK (direction IN ('in','out')),
  offered_name TEXT NOT NULL,
  local_path   TEXT,
  size         INTEGER NOT NULL,
  sha256       TEXT NOT NULL,
  received     INTEGER NOT NULL DEFAULT 0,
  state        TEXT NOT NULL,
  detail       TEXT,
  updated_utc  TEXT NOT NULL,
  UNIQUE (peer, direction, wire_id)
);
CREATE TABLE events (
  id       INTEGER PRIMARY KEY,
  utc      TEXT NOT NULL,
  category TEXT NOT NULL,
  detail   TEXT NOT NULL
);
"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutState {
    Queued,
    Submitting,
    AwaitingReceipt,
    Delivered,
    Failed,
    Uncertain,
}

impl OutState {
    pub fn as_str(self) -> &'static str {
        match self {
            OutState::Queued => "queued",
            OutState::Submitting => "submitting",
            OutState::AwaitingReceipt => "awaiting_receipt",
            OutState::Delivered => "delivered",
            OutState::Failed => "failed",
            OutState::Uncertain => "uncertain",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "queued" => OutState::Queued,
            "submitting" => OutState::Submitting,
            "awaiting_receipt" => OutState::AwaitingReceipt,
            "delivered" => OutState::Delivered,
            "failed" => OutState::Failed,
            "uncertain" => OutState::Uncertain,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stored {
    New,
    Duplicate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageRow {
    pub wire_id: [u8; 16],
    pub peer: String,
    pub direction: String,
    pub body: String,
    pub state: String,
    pub attempts: i64,
}

pub fn now_utc() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        // Receipt means durable acceptance: FULL at the commit boundary.
        conn.pragma_update(None, "synchronous", "FULL")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        let v: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if v > SCHEMA_VERSION {
            return Err(StorageError::TooNew(v));
        }
        if v < 1 {
            let tx = conn.unchecked_transaction()?;
            tx.execute_batch(MIGRATION_1)?;
            tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
            tx.commit()?;
        }
        Ok(Store { conn })
    }

    pub fn schema_version(&self) -> Result<i64> {
        Ok(self.conn.pragma_query_value(None, "user_version", |r| r.get(0))?)
    }

    /// Commit an incoming chat and its dedup key in one durable transaction.
    /// `Duplicate` means the peer re-sent an ID already accepted: re-emit the
    /// receipt but do not display it again.
    pub fn store_incoming_chat(&mut self, peer: &str, id: &[u8; 16], body: &str, session_gen: u64) -> Result<Stored> {
        let tx = self.conn.transaction()?;
        let now = now_utc();
        let fresh = tx.execute(
            "INSERT OR IGNORE INTO dedup (peer, kind, wire_id, first_utc) VALUES (?1, 'chat', ?2, ?3)",
            params![peer, &id[..], now],
        )?;
        if fresh == 0 {
            tx.commit()?;
            return Ok(Stored::Duplicate);
        }
        tx.execute(
            "INSERT INTO messages (wire_id, peer, direction, body, created_utc, received_utc, state, session_gen)
             VALUES (?1, ?2, 'in', ?3, ?4, ?4, 'received', ?5)",
            params![&id[..], peer, body, now, session_gen as i64],
        )?;
        tx.commit()?;
        Ok(Stored::New)
    }

    pub fn insert_outgoing(&mut self, peer: &str, id: &[u8; 16], body: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO messages (wire_id, peer, direction, body, created_utc, state)
             VALUES (?1, ?2, 'out', ?3, ?4, 'queued')",
            params![&id[..], peer, body, now_utc()],
        )?;
        Ok(())
    }

    pub fn set_out_state(&mut self, peer: &str, id: &[u8; 16], st: OutState, session_gen: Option<u64>) -> Result<()> {
        let bump = if st == OutState::Submitting { 1 } else { 0 };
        self.conn.execute(
            "UPDATE messages SET state = ?1, attempts = attempts + ?2, session_gen = COALESCE(?3, session_gen)
             WHERE peer = ?4 AND direction = 'out' AND wire_id = ?5",
            params![st.as_str(), bump, session_gen.map(|g| g as i64), peer, &id[..]],
        )?;
        Ok(())
    }

    /// Socket/session loss: every message that may have reached the modem is
    /// now uncertain. Returns how many were marked.
    pub fn mark_in_flight_uncertain(&mut self) -> Result<usize> {
        Ok(self.conn.execute(
            "UPDATE messages SET state = 'uncertain'
             WHERE direction = 'out' AND state IN ('submitting','awaiting_receipt')",
            [],
        )?)
    }

    pub fn out_state(&self, peer: &str, id: &[u8; 16]) -> Result<Option<OutState>> {
        let s: Option<String> = self
            .conn
            .query_row(
                "SELECT state FROM messages WHERE peer = ?1 AND direction = 'out' AND wire_id = ?2",
                params![peer, &id[..]],
                |r| r.get(0),
            )
            .optional()?;
        Ok(s.and_then(|s| OutState::parse(&s)))
    }

    pub fn messages(&self, peer: &str) -> Result<Vec<MessageRow>> {
        let mut st = self.conn.prepare(
            "SELECT wire_id, peer, direction, body, state, attempts FROM messages WHERE peer = ?1 ORDER BY rowid_pk",
        )?;
        let rows = st.query_map(params![peer], |r| {
            let id: Vec<u8> = r.get(0)?;
            Ok(MessageRow {
                wire_id: id.try_into().unwrap_or([0; 16]),
                peer: r.get(1)?,
                direction: r.get(2)?,
                body: r.get(3)?,
                state: r.get(4)?,
                attempts: r.get(5)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Delete message history for a peer (dedup keys are retained).
    pub fn delete_history(&mut self, peer: &str) -> Result<usize> {
        Ok(self.conn.execute("DELETE FROM messages WHERE peer = ?1", params![peer])?)
    }

    // ---- transfers -------------------------------------------------------

    pub fn upsert_transfer(
        &mut self,
        peer: &str,
        dir: &str,
        id: &[u8; 16],
        name: &str,
        size: u64,
        sha256: &str,
        state: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO transfers (wire_id, peer, direction, offered_name, size, sha256, state, updated_utc)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT (peer, direction, wire_id) DO UPDATE SET state = excluded.state, updated_utc = excluded.updated_utc",
            params![&id[..], peer, dir, name, size as i64, sha256, state, now_utc()],
        )?;
        Ok(())
    }

    pub fn update_transfer(
        &mut self,
        peer: &str,
        dir: &str,
        id: &[u8; 16],
        state: &str,
        received: Option<u64>,
        local_path: Option<&str>,
        detail: Option<&str>,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE transfers SET state = ?1, received = COALESCE(?2, received), local_path = COALESCE(?3, local_path),
               detail = COALESCE(?4, detail), updated_utc = ?5
             WHERE peer = ?6 AND direction = ?7 AND wire_id = ?8",
            params![state, received.map(|v| v as i64), local_path, detail, now_utc(), peer, dir, &id[..]],
        )?;
        Ok(())
    }

    pub fn transfer_state(&self, peer: &str, dir: &str, id: &[u8; 16]) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT state FROM transfers WHERE peer = ?1 AND direction = ?2 AND wire_id = ?3",
                params![peer, dir, &id[..]],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// On restart: transfers that were mid-flight are interrupted, not complete.
    pub fn mark_transfers_interrupted(&mut self) -> Result<usize> {
        Ok(self.conn.execute(
            "UPDATE transfers SET state = 'interrupted', updated_utc = ?1
             WHERE state IN ('offered','accepted','sending','receiving','verifying')",
            params![now_utc()],
        )?)
    }

    pub fn log_event(&mut self, category: &str, detail: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO events (utc, category, detail) VALUES (?1, ?2, ?3)",
            params![now_utc(), category, detail],
        )?;
        // Bounded diagnostics: keep the newest 5000 rows.
        self.conn.execute("DELETE FROM events WHERE id <= (SELECT MAX(id) - 5000 FROM events)", [])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_suppressed_and_survives_history_delete() {
        let mut s = Store::open_in_memory().unwrap();
        let id = [7u8; 16];
        assert_eq!(s.store_incoming_chat("LABA", &id, "hi", 1).unwrap(), Stored::New);
        assert_eq!(s.store_incoming_chat("LABA", &id, "hi", 2).unwrap(), Stored::Duplicate);
        assert_eq!(s.messages("LABA").unwrap().len(), 1);
        s.delete_history("LABA").unwrap();
        assert_eq!(s.store_incoming_chat("LABA", &id, "hi", 3).unwrap(), Stored::Duplicate);
        // Same ID from a different peer is a different message.
        assert_eq!(s.store_incoming_chat("LABC", &id, "hi", 3).unwrap(), Stored::New);
    }

    #[test]
    fn in_flight_becomes_uncertain_not_failed() {
        let mut s = Store::open_in_memory().unwrap();
        let (a, b, c) = ([1u8; 16], [2u8; 16], [3u8; 16]);
        for id in [&a, &b, &c] {
            s.insert_outgoing("LABB", id, "x").unwrap();
        }
        s.set_out_state("LABB", &a, OutState::AwaitingReceipt, Some(1)).unwrap();
        s.set_out_state("LABB", &b, OutState::Delivered, Some(1)).unwrap();
        assert_eq!(s.mark_in_flight_uncertain().unwrap(), 1);
        assert_eq!(s.out_state("LABB", &a).unwrap(), Some(OutState::Uncertain));
        assert_eq!(s.out_state("LABB", &b).unwrap(), Some(OutState::Delivered));
        assert_eq!(s.out_state("LABB", &c).unwrap(), Some(OutState::Queued));
    }

    #[test]
    fn reopen_keeps_data_and_version() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("station.db");
        {
            let mut s = Store::open(&p).unwrap();
            s.store_incoming_chat("LABA", &[9; 16], "persist", 1).unwrap();
        }
        let s = Store::open(&p).unwrap();
        assert_eq!(s.schema_version().unwrap(), 1);
        assert_eq!(s.messages("LABA").unwrap()[0].body, "persist");
    }
}
