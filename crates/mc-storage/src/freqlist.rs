//! The frequency list (M3b, D15), shared by every station profile.

use crate::{now_utc, FrequencyRow, Result, Store};
use rusqlite::{params, OptionalExtension};

const COLUMNS: &str = "id, name, freq_hz, mode, passband_hz, purpose, notes, suggested, preset_key, region, sort";

fn read(r: &rusqlite::Row<'_>) -> rusqlite::Result<FrequencyRow> {
    let purpose: String = r.get(5)?;
    let freq: i64 = r.get(2)?;
    Ok(FrequencyRow {
        id: r.get(0)?,
        name: r.get(1)?,
        freq_hz: freq as u64,
        mode: r.get(3)?,
        passband_hz: r.get(4)?,
        purpose: purpose.split(',').filter(|p| !p.is_empty()).map(String::from).collect(),
        notes: r.get(6)?,
        suggested: r.get(7)?,
        preset_key: r.get(8)?,
        region: r.get(9)?,
        sort: r.get(10)?,
    })
}

impl Store {
    /// Every entry, by `sort`, then frequency.
    pub fn frequencies(&self) -> Result<Vec<FrequencyRow>> {
        let mut st = self.conn.prepare(&format!("SELECT {COLUMNS} FROM frequency_list ORDER BY sort, freq_hz, id"))?;
        let rows = st.query_map([], read)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Insert or replace an entry (by id).
    pub fn save_frequency(&mut self, f: &FrequencyRow) -> Result<()> {
        let now = now_utc();
        self.conn.execute(
            "INSERT INTO frequency_list (id, name, freq_hz, mode, passband_hz, purpose, notes, suggested, preset_key,
               region, sort, created_utc, updated_utc)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?12)
             ON CONFLICT (id) DO UPDATE SET name = excluded.name, freq_hz = excluded.freq_hz, mode = excluded.mode,
               passband_hz = excluded.passband_hz, purpose = excluded.purpose, notes = excluded.notes,
               suggested = excluded.suggested, preset_key = excluded.preset_key, region = excluded.region,
               sort = excluded.sort, updated_utc = excluded.updated_utc",
            params![
                f.id,
                f.name,
                f.freq_hz as i64,
                f.mode,
                f.passband_hz,
                f.purpose.join(","),
                f.notes,
                f.suggested,
                f.preset_key,
                f.region,
                f.sort,
                now
            ],
        )?;
        Ok(())
    }

    /// True if an entry was removed.
    pub fn delete_frequency(&mut self, id: &str) -> Result<bool> {
        Ok(self.conn.execute("DELETE FROM frequency_list WHERE id = ?1", params![id])? > 0)
    }

    /// Add the presets whose `preset_key` is not stored (deleted ones come
    /// back; edited ones are left as the operator made them). Returns how
    /// many were added.
    pub fn insert_missing_presets(&mut self, presets: &[FrequencyRow]) -> Result<usize> {
        let mut added = 0;
        for p in presets {
            let Some(key) = &p.preset_key else { continue };
            let have: Option<i64> = self
                .conn
                .query_row("SELECT 1 FROM frequency_list WHERE preset_key = ?1", params![key], |r| r.get(0))
                .optional()?;
            if have.is_none() {
                // A deleted preset may come back with the same id: replace any row holding it.
                self.conn.execute("DELETE FROM frequency_list WHERE id = ?1", params![p.id])?;
                self.save_frequency(p)?;
                added += 1;
            }
        }
        Ok(added)
    }
}
