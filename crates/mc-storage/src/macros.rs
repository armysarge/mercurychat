//! Per-profile text macros (M3a).

use crate::{MacroRow, Result, Store};
use rusqlite::params;

impl Store {
    /// A profile's macros in display order.
    pub fn macros(&self, profile_id: &str) -> Result<Vec<MacroRow>> {
        let mut st = self
            .conn
            .prepare("SELECT id, profile_id, name, body, sort FROM macros WHERE profile_id = ?1 ORDER BY sort, id")?;
        let rows = st.query_map(params![profile_id], |r| {
            Ok(MacroRow { id: r.get(0)?, profile_id: r.get(1)?, name: r.get(2)?, body: r.get(3)?, sort: r.get(4)? })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn save_macro(&mut self, m: &MacroRow) -> Result<()> {
        self.conn.execute(
            "INSERT INTO macros (id, profile_id, name, body, sort) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (id) DO UPDATE SET profile_id = excluded.profile_id, name = excluded.name,
               body = excluded.body, sort = excluded.sort",
            params![m.id, m.profile_id, m.name, m.body, m.sort],
        )?;
        Ok(())
    }

    /// True if a macro was removed.
    pub fn delete_macro(&mut self, id: &str) -> Result<bool> {
        Ok(self.conn.execute("DELETE FROM macros WHERE id = ?1", params![id])? > 0)
    }
}
