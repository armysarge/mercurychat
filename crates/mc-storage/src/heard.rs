//! Heard-station observations and favourite contacts (M3a).

use crate::{now_utc, HeardRow, HeardSummaryRow, Result, Store};
use rusqlite::{params, OptionalExtension};

const HEARD_COLUMNS: &str = "id, callsign, observed_utc, kind, bandwidth, grid, caps, snr_db, snr_source,
     freq_hz, freq_source, freq_age_ms";

fn heard_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<HeardRow> {
    Ok(HeardRow {
        id: r.get(0)?,
        callsign: r.get(1)?,
        observed_utc: r.get(2)?,
        kind: r.get(3)?,
        bandwidth: r.get(4)?,
        grid: r.get(5)?,
        caps: r.get(6)?,
        snr_db: r.get(7)?,
        snr_source: r.get(8)?,
        freq_hz: r.get(9)?,
        freq_source: r.get(10)?,
        freq_age_ms: r.get(11)?,
    })
}

impl Store {
    /// Append one observation; returns its row id.
    pub fn record_heard(&mut self, o: &HeardRow) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO heard_observations (callsign, observed_utc, kind, bandwidth, grid, caps, snr_db,
               snr_source, freq_hz, freq_source, freq_age_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                o.callsign,
                o.observed_utc,
                o.kind,
                o.bandwidth,
                o.grid,
                o.caps,
                o.snr_db,
                o.snr_source,
                o.freq_hz,
                o.freq_source,
                o.freq_age_ms
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Attach a (late) SNR reading to an existing observation.
    pub fn set_heard_snr(&mut self, id: i64, snr_db: f32, source: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE heard_observations SET snr_db = ?1, snr_source = ?2 WHERE id = ?3",
            params![snr_db, source, id],
        )?;
        Ok(())
    }

    /// Give an observation the bandwidth `bw` if it has none.
    pub fn fill_heard_bandwidth(&mut self, id: i64, bw: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE heard_observations SET bandwidth = ?1 WHERE id = ?2 AND bandwidth IS NULL",
            params![bw, id],
        )?;
        Ok(())
    }

    /// The newest observation of `kind` for `callsign` within the last `within_s` seconds.
    pub fn latest_heard(&self, callsign: &str, kind: &str, within_s: i64) -> Result<Option<HeardRow>> {
        let since = (chrono::Utc::now() - chrono::Duration::seconds(within_s.max(0)))
            .format("%Y-%m-%dT%H:%M:%S%.3fZ")
            .to_string();
        Ok(self
            .conn
            .query_row(
                &format!(
                    "SELECT {HEARD_COLUMNS} FROM heard_observations
                     WHERE callsign = ?1 AND kind = ?2 AND observed_utc >= ?3
                     ORDER BY observed_utc DESC, id DESC LIMIT 1"
                ),
                params![callsign, kind, since],
                heard_row,
            )
            .optional()?)
    }

    /// One row per heard callsign: the latest observation, how many there are,
    /// and whether the callsign is a favourite. Most recently heard first.
    pub fn heard_summary(&self) -> Result<Vec<HeardSummaryRow>> {
        let mut st = self.conn.prepare(
            "SELECT h.callsign, h.observed_utc, h.kind, h.bandwidth, h.grid, h.caps, h.snr_db, h.snr_source,
                    h.freq_hz, h.freq_source, h.freq_age_ms,
                    (SELECT count(*) FROM heard_observations c WHERE c.callsign = h.callsign),
                    coalesce((SELECT favourite FROM contacts k WHERE k.callsign = h.callsign), 0)
             FROM heard_observations h
             WHERE h.id = (SELECT h2.id FROM heard_observations h2 WHERE h2.callsign = h.callsign
                           ORDER BY h2.observed_utc DESC, h2.id DESC LIMIT 1)
             ORDER BY h.observed_utc DESC, h.id DESC",
        )?;
        let rows = st.query_map([], |r| {
            Ok(HeardSummaryRow {
                callsign: r.get(0)?,
                last_utc: r.get(1)?,
                kind: r.get(2)?,
                bandwidth: r.get(3)?,
                grid: r.get(4)?,
                caps: r.get(5)?,
                snr_db: r.get(6)?,
                snr_source: r.get(7)?,
                freq_hz: r.get(8)?,
                freq_source: r.get(9)?,
                freq_age_ms: r.get(10)?,
                count: r.get(11)?,
                favourite: r.get::<_, i64>(12)? != 0,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Drop observations older than `cutoff_utc`. Contacts are never touched.
    pub fn purge_heard_before(&mut self, cutoff_utc: &str) -> Result<usize> {
        Ok(self.conn.execute("DELETE FROM heard_observations WHERE observed_utc < ?1", params![cutoff_utc])?)
    }

    /// Every callsign marked favourite, heard or not, in callsign order.
    pub fn favourites(&self) -> Result<Vec<String>> {
        let mut st = self.conn.prepare("SELECT callsign FROM contacts WHERE favourite != 0 ORDER BY callsign")?;
        let rows = st.query_map([], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn set_favourite(&mut self, callsign: &str, on: bool) -> Result<()> {
        self.conn.execute(
            "INSERT INTO contacts (callsign, favourite, updated_utc) VALUES (?1, ?2, ?3)
             ON CONFLICT (callsign) DO UPDATE SET favourite = excluded.favourite, updated_utc = excluded.updated_utc",
            params![callsign, on, now_utc()],
        )?;
        Ok(())
    }
}
