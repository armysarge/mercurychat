//! File transfers (M4 C5): one row per transfer and direction, in M0's
//! `transfers` table as migration 9 extends it.

use crate::{now_utc, Result, Store};
use rusqlite::{params, OptionalExtension, Row};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Out,
    In,
}

impl Direction {
    pub fn as_str(self) -> &'static str {
        match self {
            Direction::Out => "out",
            Direction::In => "in",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

impl TransferState {
    pub fn is_final(self) -> bool {
        use TransferState::*;
        matches!(self, Complete | Rejected | Cancelled | Failed | Interrupted)
    }

    pub fn as_str(self) -> &'static str {
        use TransferState::*;
        match self {
            Offered => "offered",
            Waiting => "waiting",
            Sending => "sending",
            Receiving => "receiving",
            Verifying => "verifying",
            Complete => "complete",
            Rejected => "rejected",
            Cancelled => "cancelled",
            Failed => "failed",
            Interrupted => "interrupted",
        }
    }

    /// An unknown word (a later version's) reads as interrupted: never complete.
    pub fn parse(s: &str) -> TransferState {
        use TransferState::*;
        [Offered, Waiting, Sending, Receiving, Verifying, Complete, Rejected, Cancelled, Failed, Interrupted]
            .into_iter()
            .find(|t| t.as_str() == s)
            .unwrap_or(Interrupted)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferRow {
    pub id: [u8; 16],
    pub direction: Direction,
    pub peer: String,
    pub session_id: Option<i64>,
    pub original_name: String,
    pub saved_name: Option<String>,
    /// The file as it lands on disk.
    pub size: u64,
    /// The bytes the chunks carry.
    pub wire_size: u64,
    pub sha256: String,
    pub encoding: Option<String>,
    pub state: TransferState,
    pub bytes_done: u64,
    pub reason: Option<String>,
    pub created_utc: String,
    pub updated_utc: String,
    pub saved_path: Option<String>,
}

const COLUMNS: &str = "wire_id, direction, peer, session_id, offered_name, saved_name, size, wire_size, sha256, \
     encoding, state, received, detail, created_utc, updated_utc, local_path";

fn from_row(r: &Row) -> rusqlite::Result<TransferRow> {
    let id: Vec<u8> = r.get(0)?;
    let dir: String = r.get(1)?;
    let state: String = r.get(10)?;
    Ok(TransferRow {
        id: id.try_into().unwrap_or([0; 16]),
        direction: if dir == "in" { Direction::In } else { Direction::Out },
        peer: r.get(2)?,
        session_id: r.get(3)?,
        original_name: r.get(4)?,
        saved_name: r.get(5)?,
        size: r.get::<_, i64>(6)? as u64,
        wire_size: r.get::<_, i64>(7)? as u64,
        sha256: r.get(8)?,
        encoding: r.get(9)?,
        state: TransferState::parse(&state),
        bytes_done: r.get::<_, i64>(11)? as u64,
        reason: r.get(12)?,
        created_utc: r.get(13)?,
        updated_utc: r.get(14)?,
        saved_path: r.get(15)?,
    })
}

impl Store {
    /// Insert a transfer, or update every field but its creation time.
    pub fn transfer_upsert(&mut self, t: &TransferRow) -> Result<()> {
        let now = now_utc();
        self.conn.execute(
            "INSERT INTO transfers (wire_id, direction, peer, session_id, offered_name, saved_name, size, wire_size,
               sha256, encoding, state, received, detail, created_utc, updated_utc, local_path)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?14, ?15)
             ON CONFLICT (peer, direction, wire_id) DO UPDATE SET session_id = excluded.session_id,
               offered_name = excluded.offered_name, saved_name = excluded.saved_name, size = excluded.size,
               wire_size = excluded.wire_size, sha256 = excluded.sha256, encoding = excluded.encoding,
               state = excluded.state, received = excluded.received, detail = excluded.detail,
               updated_utc = excluded.updated_utc, local_path = excluded.local_path",
            params![
                &t.id[..],
                t.direction.as_str(),
                t.peer,
                t.session_id,
                t.original_name,
                t.saved_name,
                t.size as i64,
                t.wire_size as i64,
                t.sha256,
                t.encoding,
                t.state.as_str(),
                t.bytes_done as i64,
                t.reason,
                now,
                t.saved_path,
            ],
        )?;
        Ok(())
    }

    pub fn transfer_set_state(
        &mut self,
        dir: Direction,
        id: &[u8; 16],
        state: TransferState,
        bytes_done: u64,
        reason: Option<&str>,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE transfers SET state = ?1, received = ?2, detail = COALESCE(?3, detail), updated_utc = ?4
             WHERE direction = ?5 AND wire_id = ?6",
            params![state.as_str(), bytes_done as i64, reason, now_utc(), dir.as_str(), &id[..]],
        )?;
        Ok(())
    }

    /// Complete, durably, before FILE_RECEIPT goes out (FIL-03): an incoming
    /// file with the name and path it was saved under.
    pub fn transfer_complete(&mut self, dir: Direction, id: &[u8; 16], saved: Option<(&str, &str)>) -> Result<()> {
        let (name, path) = saved.unzip();
        self.conn.execute(
            "UPDATE transfers SET state = 'complete', received = wire_size, saved_name = COALESCE(?1, saved_name),
               local_path = COALESCE(?2, local_path), updated_utc = ?3
             WHERE direction = ?4 AND wire_id = ?5",
            params![name, path, now_utc(), dir.as_str(), &id[..]],
        )?;
        Ok(())
    }

    pub fn transfer(&self, dir: Direction, id: &[u8; 16]) -> Result<Option<TransferRow>> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM transfers WHERE direction = ?1 AND wire_id = ?2"),
                params![dir.as_str(), &id[..]],
                from_row,
            )
            .optional()?)
    }

    /// Newest first.
    pub fn transfers(&self, limit: u32) -> Result<Vec<TransferRow>> {
        let mut st = self.conn.prepare(&format!("SELECT {COLUMNS} FROM transfers ORDER BY rowid_pk DESC LIMIT ?1"))?;
        let rows = st.query_map(params![limit], from_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// On start-up: nothing mid-flight survives a restart (no resume, FIL-05).
    pub fn transfers_interrupt_unfinished(&mut self) -> Result<u64> {
        let n = self.conn.execute(
            "UPDATE transfers SET state = 'interrupted', detail = 'restart', updated_utc = ?1
             WHERE state IN ('offered', 'waiting', 'sending', 'receiving', 'verifying', 'accepted')",
            params![now_utc()],
        )?;
        Ok(n as u64)
    }
}
