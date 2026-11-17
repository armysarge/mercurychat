//! QSO records (drafts become final once) and history search (M3a).

use crate::{message_row, MessageRow, QsoEdit, QsoRow, Result, StorageError, Store, MESSAGE_COLUMNS};
use rusqlite::{params, OptionalExtension};

const QSO_COLUMNS: &str = "id, session_id, peer, start_utc, end_utc, grid, band, freq_hz, freq_source, freq_age_ms,
     bandwidth, tx_modes, rx_modes, rst_sent, rst_rcvd, name, qth, my_power, notes, state";

fn qso_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<QsoRow> {
    Ok(QsoRow {
        id: r.get(0)?,
        session_id: r.get(1)?,
        peer: r.get(2)?,
        start_utc: r.get(3)?,
        end_utc: r.get(4)?,
        grid: r.get(5)?,
        band: r.get(6)?,
        freq_hz: r.get(7)?,
        freq_source: r.get(8)?,
        freq_age_ms: r.get(9)?,
        bandwidth: r.get(10)?,
        tx_modes: r.get(11)?,
        rx_modes: r.get(12)?,
        rst_sent: r.get(13)?,
        rst_rcvd: r.get(14)?,
        name: r.get(15)?,
        qth: r.get(16)?,
        my_power: r.get(17)?,
        notes: r.get(18)?,
        state: r.get(19)?,
    })
}

/// Escape `\`, `%` and `_` so user text matches literally under `LIKE ... ESCAPE '\'`.
fn like_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        if matches!(c, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

impl Store {
    fn qso(&self, id: &str) -> Result<QsoRow> {
        self.conn
            .query_row(&format!("SELECT {QSO_COLUMNS} FROM qso_records WHERE id = ?1"), params![id], qso_row)
            .optional()?
            .ok_or_else(|| StorageError::QsoNotFound(id.to_string()))
    }

    pub fn create_qso(&mut self, q: &QsoRow) -> Result<()> {
        self.conn.execute(
            &format!(
                "INSERT INTO qso_records ({QSO_COLUMNS})
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20)"
            ),
            params![
                q.id,
                q.session_id,
                q.peer,
                q.start_utc,
                q.end_utc,
                q.grid,
                q.band,
                q.freq_hz,
                q.freq_source,
                q.freq_age_ms,
                q.bandwidth,
                q.tx_modes,
                q.rx_modes,
                q.rst_sent,
                q.rst_rcvd,
                q.name,
                q.qth,
                q.my_power,
                q.notes,
                q.state
            ],
        )?;
        Ok(())
    }

    /// Apply the `Some` fields of `edit` to a draft. A final record is refused.
    /// A new `freq_hz` replaces the whole frequency reading (band, source and
    /// age too, `None` clearing them): a stale age never outlives its value.
    pub fn update_qso(&mut self, id: &str, edit: &QsoEdit) -> Result<QsoRow> {
        let n = self.conn.execute(
            "UPDATE qso_records SET
               grid = COALESCE(?2, grid),
               band = CASE WHEN ?4 IS NOT NULL THEN ?3 ELSE COALESCE(?3, band) END,
               freq_hz = COALESCE(?4, freq_hz),
               freq_source = CASE WHEN ?4 IS NOT NULL THEN ?5 ELSE COALESCE(?5, freq_source) END,
               freq_age_ms = CASE WHEN ?4 IS NOT NULL THEN ?6 ELSE COALESCE(?6, freq_age_ms) END,
               bandwidth = COALESCE(?7, bandwidth), tx_modes = COALESCE(?8, tx_modes),
               rx_modes = COALESCE(?9, rx_modes), rst_sent = COALESCE(?10, rst_sent),
               rst_rcvd = COALESCE(?11, rst_rcvd), name = COALESCE(?12, name), qth = COALESCE(?13, qth),
               my_power = COALESCE(?14, my_power), notes = COALESCE(?15, notes)
             WHERE id = ?1 AND state = 'draft'",
            params![
                id,
                edit.grid,
                edit.band,
                edit.freq_hz,
                edit.freq_source,
                edit.freq_age_ms,
                edit.bandwidth,
                edit.tx_modes,
                edit.rx_modes,
                edit.rst_sent,
                edit.rst_rcvd,
                edit.name,
                edit.qth,
                edit.my_power,
                edit.notes
            ],
        )?;
        let row = self.qso(id)?;
        if n == 0 {
            return Err(StorageError::QsoFinal(id.to_string()));
        }
        Ok(row)
    }

    /// Lock a record against further edits (idempotent).
    pub fn finalise_qso(&mut self, id: &str) -> Result<QsoRow> {
        self.conn.execute("UPDATE qso_records SET state = 'final' WHERE id = ?1", params![id])?;
        self.qso(id)
    }

    /// Stamp the end time on the session's record, once. `None` when there is no
    /// such record or it already has an end time.
    pub fn end_qso_for_session(&mut self, session_id: &str, end_utc: &str) -> Result<Option<QsoRow>> {
        let id: Option<String> = self
            .conn
            .query_row(
                "SELECT id FROM qso_records WHERE session_id = ?1 AND end_utc IS NULL
                 ORDER BY start_utc DESC, rowid DESC LIMIT 1",
                params![session_id],
                |r| r.get(0),
            )
            .optional()?;
        let Some(id) = id else { return Ok(None) };
        self.conn
            .execute("UPDATE qso_records SET end_utc = ?1 WHERE id = ?2 AND end_utc IS NULL", params![end_utc, id])?;
        Ok(Some(self.qso(&id)?))
    }

    /// Newest first.
    pub fn qsos(&self, limit: u32) -> Result<Vec<QsoRow>> {
        let mut st = self
            .conn
            .prepare(&format!("SELECT {QSO_COLUMNS} FROM qso_records ORDER BY start_utc DESC, rowid DESC LIMIT ?1"))?;
        let rows = st.query_map(params![limit], qso_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Literal substring search over message bodies and peer callsigns
    /// (ASCII case-insensitive), newest first.
    pub fn search_messages(&self, query: &str, limit: u32) -> Result<Vec<MessageRow>> {
        if query.is_empty() {
            return Ok(Vec::new());
        }
        let pattern = format!("%{}%", like_escape(query));
        let mut st = self.conn.prepare(&format!(
            "SELECT {MESSAGE_COLUMNS} FROM messages
             WHERE body LIKE ?1 ESCAPE '\\' OR peer LIKE ?1 ESCAPE '\\'
             ORDER BY rowid_pk DESC LIMIT ?2"
        ))?;
        let rows = st.query_map(params![pattern, limit], message_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}
