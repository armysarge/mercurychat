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
use std::path::{Path, PathBuf};

pub type Result<T> = std::result::Result<T, StorageError>;

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("database schema version {0} is newer than this build supports")]
    TooNew(i64),
    #[error("could not back up the database before upgrading it ({}): {reason}; nothing was changed", path.display())]
    Backup { path: PathBuf, reason: String },
    #[error("upgrading the database to version {to} failed: {source}; the database was left at version {from}; a copy taken before the upgrade is at {}", backup.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "(none: the database was new)".into()))]
    Migration { from: i64, to: i64, backup: Option<PathBuf>, source: rusqlite::Error },
    #[error("QSO record {0} is final and cannot be edited")]
    QsoFinal(String),
    #[error("QSO record {0} does not exist")]
    QsoNotFound(String),
}

mod freqlist;
mod heard;
mod logbook;
mod macros;
mod migrate;
pub use migrate::{Migration, MIGRATIONS};
mod transfers;
pub use transfers::{Direction, TransferRow, TransferState};

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
    /// Local order (the row id): pages and the UI sort by it.
    pub seq: i64,
    pub wire_id: [u8; 16],
    pub peer: String,
    pub direction: String,
    pub body: String,
    pub state: String,
    pub attempts: i64,
    pub created_utc: String,
    pub received_utc: Option<String>,
    pub session_gen: Option<i64>,
}

/// One peer's conversation summary (M2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationRow {
    pub peer: String,
    pub last_utc: String,
    pub messages: i64,
    pub uncertain: i64,
}

/// One MCH/1 session as recorded on the station (docs/02 section 6, sessions).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRow {
    pub id: String,
    pub peer: String,
    pub generation: i64,
    pub direction: String,
    pub started_utc: String,
    pub ended_utc: Option<String>,
    pub peer_app: Option<String>,
    pub outcome: Option<String>,
}

/// One heard-station observation (`heard_observations`). `id` is ignored on insert.
#[derive(Debug, Clone, PartialEq)]
pub struct HeardRow {
    pub id: i64,
    pub callsign: String,
    pub observed_utc: String,
    /// `cq`, `beacon` or `session`.
    pub kind: String,
    pub bandwidth: Option<String>,
    pub grid: Option<String>,
    pub caps: Option<i64>,
    pub snr_db: Option<f32>,
    pub snr_source: Option<String>,
    pub freq_hz: Option<i64>,
    pub freq_source: Option<String>,
    pub freq_age_ms: Option<i64>,
}

/// One callsign's latest observation plus its count and favourite flag.
#[derive(Debug, Clone, PartialEq)]
pub struct HeardSummaryRow {
    pub callsign: String,
    pub last_utc: String,
    pub kind: String,
    pub bandwidth: Option<String>,
    pub grid: Option<String>,
    pub caps: Option<i64>,
    pub snr_db: Option<f32>,
    pub snr_source: Option<String>,
    pub freq_hz: Option<i64>,
    pub freq_source: Option<String>,
    pub freq_age_ms: Option<i64>,
    pub count: i64,
    pub favourite: bool,
}

/// One `qso_records` row. `state` is `draft` or `final`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QsoRow {
    pub id: String,
    pub session_id: Option<String>,
    pub peer: String,
    pub start_utc: String,
    pub end_utc: Option<String>,
    pub grid: Option<String>,
    pub band: Option<String>,
    pub freq_hz: Option<i64>,
    pub freq_source: Option<String>,
    pub freq_age_ms: Option<i64>,
    pub bandwidth: Option<String>,
    pub tx_modes: Option<String>,
    pub rx_modes: Option<String>,
    pub rst_sent: Option<String>,
    pub rst_rcvd: Option<String>,
    pub name: Option<String>,
    pub qth: Option<String>,
    pub my_power: Option<String>,
    pub notes: Option<String>,
    pub state: String,
    /// `peer report` when a peer's answer filled the field (M3c B6).
    pub rst_rcvd_source: Option<String>,
    pub name_source: Option<String>,
    pub qth_source: Option<String>,
}

/// A partial QSO edit: only `Some` fields are written.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QsoEdit {
    pub grid: Option<String>,
    pub band: Option<String>,
    pub freq_hz: Option<i64>,
    pub freq_source: Option<String>,
    pub freq_age_ms: Option<i64>,
    pub bandwidth: Option<String>,
    pub tx_modes: Option<String>,
    pub rx_modes: Option<String>,
    pub rst_sent: Option<String>,
    pub rst_rcvd: Option<String>,
    pub name: Option<String>,
    pub qth: Option<String>,
    pub my_power: Option<String>,
    pub notes: Option<String>,
}

/// One entry of the frequency list (M3b, D15). `preset_key` marks a shipped
/// suggestion; once stored, the operator may edit or delete it like any other.
#[derive(Debug, Clone, PartialEq)]
pub struct FrequencyRow {
    pub id: String,
    pub name: String,
    pub freq_hz: u64,
    pub mode: String,
    pub passband_hz: Option<i32>,
    /// `beacon`, `cq`, `chat`.
    pub purpose: Vec<String>,
    pub notes: String,
    pub suggested: bool,
    pub preset_key: Option<String>,
    pub region: Option<u8>,
    pub sort: i64,
}

/// One text macro of a station profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacroRow {
    pub id: String,
    pub profile_id: String,
    pub name: String,
    pub body: String,
    pub sort: i64,
    /// Listed under "Quick messages" (M3c B8).
    pub quick: bool,
}

const MESSAGE_COLUMNS: &str =
    "rowid_pk, wire_id, peer, direction, body, state, attempts, created_utc, received_utc, session_gen";

fn message_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<MessageRow> {
    let id: Vec<u8> = r.get(1)?;
    Ok(MessageRow {
        seq: r.get(0)?,
        wire_id: id.try_into().unwrap_or([0; 16]),
        peer: r.get(2)?,
        direction: r.get(3)?,
        body: r.get(4)?,
        state: r.get(5)?,
        attempts: r.get(6)?,
        created_utc: r.get(7)?,
        received_utc: r.get(8)?,
        session_gen: r.get(9)?,
    })
}

pub fn now_utc() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

pub struct Store {
    conn: Connection,
}

impl Store {
    /// Open (creating if needed) and run pending migrations. The caller holds
    /// the station's data-root lock (`mc_core::lock`), so no other process
    /// can use the database while it is upgraded (docs/02 §6).
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_with_migrations(path, MIGRATIONS)
    }

    #[doc(hidden)]
    pub fn open_with_migrations(path: &Path, migrations: &[Migration]) -> Result<Self> {
        Self::init(Connection::open(path)?, Some(path), migrations)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?, None, MIGRATIONS)
    }

    /// Copy the whole database with SQLite's online backup (never a raw file
    /// copy of a live WAL database).
    pub fn backup_to(&self, dest: &Path) -> Result<()> {
        Ok(self.conn.backup(rusqlite::MAIN_DB, dest, None)?)
    }

    fn init(mut conn: Connection, path: Option<&Path>, migrations: &[Migration]) -> Result<Self> {
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        // Receipt means durable acceptance: FULL at the commit boundary.
        conn.pragma_update(None, "synchronous", "FULL")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        migrate::run(&mut conn, path, migrations)?;
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
        let mut st =
            self.conn.prepare(&format!("SELECT {MESSAGE_COLUMNS} FROM messages WHERE peer = ?1 ORDER BY rowid_pk"))?;
        let rows = st.query_map(params![peer], message_row)?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// One message, either direction (outgoing first if both share an id).
    pub fn message(&self, peer: &str, id: &[u8; 16]) -> Result<Option<MessageRow>> {
        Ok(self
            .conn
            .query_row(
                &format!(
                    "SELECT {MESSAGE_COLUMNS} FROM messages WHERE peer = ?1 AND wire_id = ?2
                     ORDER BY direction DESC LIMIT 1"
                ),
                params![peer, &id[..]],
                message_row,
            )
            .optional()?)
    }

    /// Up to `limit` messages before `before_seq` (or the latest), oldest first.
    pub fn history(&self, peer: &str, before_seq: Option<i64>, limit: u32) -> Result<Vec<MessageRow>> {
        let mut st = self.conn.prepare(&format!(
            "SELECT {MESSAGE_COLUMNS} FROM messages WHERE peer = ?1 AND rowid_pk < ?2
             ORDER BY rowid_pk DESC LIMIT ?3"
        ))?;
        let rows = st.query_map(params![peer, before_seq.unwrap_or(i64::MAX), limit], message_row)?;
        let mut v: Vec<MessageRow> = rows.collect::<std::result::Result<_, _>>()?;
        v.reverse();
        Ok(v)
    }

    /// Outgoing messages never handed to a session, in the order written.
    pub fn queued_for(&self, peer: &str) -> Result<Vec<MessageRow>> {
        let mut st = self.conn.prepare(&format!(
            "SELECT {MESSAGE_COLUMNS} FROM messages WHERE peer = ?1 AND direction = 'out' AND state = 'queued'
             ORDER BY rowid_pk"
        ))?;
        let rows = st.query_map(params![peer], message_row)?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    /// Count and body bytes of a peer's unsettled outgoing messages (CHT-03 quota).
    pub fn peer_queue_usage(&self, peer: &str) -> Result<(u32, u64)> {
        Ok(self.conn.query_row(
            "SELECT count(*), coalesce(sum(length(CAST(body AS BLOB))), 0) FROM messages
             WHERE peer = ?1 AND direction = 'out' AND state IN ('queued','submitting','awaiting_receipt')",
            params![peer],
            |r| Ok((r.get::<_, i64>(0)? as u32, r.get::<_, i64>(1)? as u64)),
        )?)
    }

    /// Every peer with history, the most recent conversation first.
    pub fn conversations(&self) -> Result<Vec<ConversationRow>> {
        let mut st = self.conn.prepare(
            "SELECT peer, max(coalesce(received_utc, created_utc)), count(*),
                    sum(CASE WHEN state = 'uncertain' THEN 1 ELSE 0 END), max(rowid_pk) AS last
             FROM messages GROUP BY peer ORDER BY last DESC",
        )?;
        let rows = st.query_map([], |r| {
            Ok(ConversationRow { peer: r.get(0)?, last_utc: r.get(1)?, messages: r.get(2)?, uncertain: r.get(3)? })
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn begin_session(&mut self, s: &SessionRow) -> Result<()> {
        self.conn.execute(
            "INSERT INTO sessions (id, peer, generation, direction, started_utc, ended_utc, peer_app, outcome)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![s.id, s.peer, s.generation, s.direction, s.started_utc, s.ended_utc, s.peer_app, s.outcome],
        )?;
        Ok(())
    }

    pub fn set_session_peer_app(&mut self, id: &str, peer_app: &str) -> Result<()> {
        self.conn.execute("UPDATE sessions SET peer_app = ?1 WHERE id = ?2", params![peer_app, id])?;
        Ok(())
    }

    pub fn end_session(&mut self, id: &str, outcome: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE sessions SET ended_utc = ?1, outcome = ?2 WHERE id = ?3",
            params![now_utc(), outcome, id],
        )?;
        Ok(())
    }

    pub fn session(&self, id: &str) -> Result<Option<SessionRow>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id, peer, generation, direction, started_utc, ended_utc, peer_app, outcome
                 FROM sessions WHERE id = ?1",
                params![id],
                |r| {
                    Ok(SessionRow {
                        id: r.get(0)?,
                        peer: r.get(1)?,
                        generation: r.get(2)?,
                        direction: r.get(3)?,
                        started_utc: r.get(4)?,
                        ended_utc: r.get(5)?,
                        peer_app: r.get(6)?,
                        outcome: r.get(7)?,
                    })
                },
            )
            .optional()?)
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

/// One `station_profiles` row (MIGRATION_2), flat; `mc_core::profile` maps it
/// to and from the validated model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileRow {
    pub id: String,
    pub callsign: String,
    pub name: Option<String>,
    pub grid: Option<String>,
    pub modem_kind: String,
    pub host: Option<String>,
    pub control_port: Option<u16>,
    pub data_port: Option<u16>,
    pub broadcast_port: Option<u16>,
    pub telemetry_url: Option<String>,
    pub lab: bool,
    pub receive_only: bool,
    pub qth: Option<String>,
    pub rig: Option<String>,
    pub power: Option<String>,
    pub antenna: Option<String>,
    pub email: Option<String>,
    pub about: Option<String>,
}

impl Store {
    /// Insert or update a profile; `created_utc` is kept on update.
    pub fn save_profile(&mut self, r: &ProfileRow) -> Result<()> {
        let now = now_utc();
        self.conn.execute(
            "INSERT INTO station_profiles (id, callsign, name, grid, modem_kind, host, control_port, data_port,
               broadcast_port, telemetry_url, lab, receive_only, created_utc, updated_utc,
               qth, rig, power, antenna, email, about)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?13, ?14, ?15, ?16, ?17, ?18, ?19)
             ON CONFLICT (id) DO UPDATE SET callsign = excluded.callsign, name = excluded.name,
               grid = excluded.grid, modem_kind = excluded.modem_kind, host = excluded.host,
               control_port = excluded.control_port, data_port = excluded.data_port,
               broadcast_port = excluded.broadcast_port, telemetry_url = excluded.telemetry_url,
               lab = excluded.lab, receive_only = excluded.receive_only, updated_utc = excluded.updated_utc,
               qth = excluded.qth, rig = excluded.rig, power = excluded.power, antenna = excluded.antenna,
               email = excluded.email, about = excluded.about",
            params![
                r.id,
                r.callsign,
                r.name,
                r.grid,
                r.modem_kind,
                r.host,
                r.control_port,
                r.data_port,
                r.broadcast_port,
                r.telemetry_url,
                r.lab,
                r.receive_only,
                now,
                r.qth,
                r.rig,
                r.power,
                r.antenna,
                r.email,
                r.about
            ],
        )?;
        Ok(())
    }

    /// All profiles, oldest first.
    pub fn profiles(&self) -> Result<Vec<ProfileRow>> {
        let mut st = self.conn.prepare(
            "SELECT id, callsign, name, grid, modem_kind, host, control_port, data_port, broadcast_port,
                    telemetry_url, lab, receive_only, qth, rig, power, antenna, email, about
             FROM station_profiles ORDER BY created_utc, rowid",
        )?;
        let rows = st.query_map([], |r| {
            Ok(ProfileRow {
                id: r.get(0)?,
                callsign: r.get(1)?,
                name: r.get(2)?,
                grid: r.get(3)?,
                modem_kind: r.get(4)?,
                host: r.get(5)?,
                control_port: r.get(6)?,
                data_port: r.get(7)?,
                broadcast_port: r.get(8)?,
                telemetry_url: r.get(9)?,
                lab: r.get(10)?,
                receive_only: r.get(11)?,
                qth: r.get(12)?,
                rig: r.get(13)?,
                power: r.get(14)?,
                antenna: r.get(15)?,
                email: r.get(16)?,
                about: r.get(17)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// True if a profile was removed.
    pub fn delete_profile(&mut self, id: &str) -> Result<bool> {
        // The profile's macros go with it, in one transaction.
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM macros WHERE profile_id = ?1", [id])?;
        let removed = tx.execute("DELETE FROM station_profiles WHERE id = ?1", [id])? > 0;
        tx.commit()?;
        Ok(removed)
    }

    pub fn set_setting(&mut self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO settings (key, value, updated_utc) VALUES (?1, ?2, ?3)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value, updated_utc = excluded.updated_utc",
            params![key, value, now_utc()],
        )?;
        Ok(())
    }

    pub fn setting(&self, key: &str) -> Result<Option<String>> {
        Ok(self.conn.query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| r.get(0)).optional()?)
    }

    /// Every setting whose key starts with `prefix`: (key without the prefix,
    /// value, updated_utc), in key order.
    pub fn settings_with_prefix(&self, prefix: &str) -> Result<Vec<(String, String, String)>> {
        let mut st = self.conn.prepare(
            "SELECT substr(key, length(?1) + 1), value, updated_utc FROM settings
             WHERE substr(key, 1, length(?1)) = ?1 ORDER BY key",
        )?;
        let rows = st.query_map([prefix], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn delete_setting(&mut self, key: &str) -> Result<()> {
        self.conn.execute("DELETE FROM settings WHERE key = ?1", [key])?;
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
        assert_eq!(s.schema_version().unwrap(), MIGRATIONS.last().unwrap().to);
        assert_eq!(s.messages("LABA").unwrap()[0].body, "persist");
    }
}
