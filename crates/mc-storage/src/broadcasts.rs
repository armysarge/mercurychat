//! General broadcasts heard (the latest-broadcasts list).

use crate::{BroadcastRow, Result, Store};
use rusqlite::{params, OptionalExtension};

const COLUMNS: &str =
    "id, observed_utc, callsign, message, snr_db, snr_source, mode, freq_hz, freq_source, freq_age_ms";

fn broadcast_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<BroadcastRow> {
    Ok(BroadcastRow {
        id: r.get(0)?,
        observed_utc: r.get(1)?,
        callsign: r.get(2)?,
        message: r.get(3)?,
        snr_db: r.get(4)?,
        snr_source: r.get(5)?,
        mode: r.get(6)?,
        freq_hz: r.get(7)?,
        freq_source: r.get(8)?,
        freq_age_ms: r.get(9)?,
    })
}

impl Store {
    /// Append one broadcast; returns its row id.
    pub fn record_broadcast(&mut self, b: &BroadcastRow) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO broadcasts_heard (observed_utc, callsign, message, snr_db, snr_source, mode,
               freq_hz, freq_source, freq_age_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                b.observed_utc,
                b.callsign,
                b.message,
                b.snr_db,
                b.snr_source,
                b.mode,
                b.freq_hz,
                b.freq_source,
                b.freq_age_ms
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Attach the S/N and decode mode, which are known a moment later.
    pub fn set_broadcast_snr(&mut self, id: i64, snr_db: f32, source: &str, mode: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE broadcasts_heard SET snr_db = ?1, snr_source = ?2, mode = ?3 WHERE id = ?4",
            params![snr_db, source, mode, id],
        )?;
        Ok(())
    }

    pub fn broadcast(&self, id: i64) -> Result<Option<BroadcastRow>> {
        Ok(self
            .conn
            .query_row(&format!("SELECT {COLUMNS} FROM broadcasts_heard WHERE id = ?1"), [id], broadcast_row)
            .optional()?)
    }

    /// The newest `limit` broadcasts, newest first.
    pub fn latest_broadcasts(&self, limit: u32) -> Result<Vec<BroadcastRow>> {
        let mut st = self
            .conn
            .prepare(&format!("SELECT {COLUMNS} FROM broadcasts_heard ORDER BY observed_utc DESC, id DESC LIMIT ?1"))?;
        let rows = st.query_map([limit], broadcast_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Drop broadcasts older than `cutoff_utc`.
    pub fn purge_broadcasts_before(&mut self, cutoff_utc: &str) -> Result<usize> {
        Ok(self.conn.execute("DELETE FROM broadcasts_heard WHERE observed_utc < ?1", params![cutoff_utc])?)
    }
}
