//! Schema migrations (docs/02 §6). Each migration runs in one transaction that
//! also sets `user_version`. Before upgrading an existing database, a copy is
//! taken with SQLite's online backup; if that copy cannot be made, nothing is
//! migrated. A failed migration rolls back, leaving the database at the last
//! version that committed. Downgrades are never automatic: a newer database is
//! refused.

use crate::{Result, StorageError};
use rusqlite::Connection;
use std::path::{Path, PathBuf};

#[doc(hidden)]
#[derive(Clone, Copy, Debug)]
pub struct Migration {
    pub to: i64,
    pub sql: &'static str,
}

pub const MIGRATIONS: &[Migration] = &[Migration { to: 1, sql: MIGRATION_1 }, Migration { to: 2, sql: MIGRATION_2 }];

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

/// Station profiles and versioned settings (M1). `modem_kind` has no CHECK:
/// Rust validates it, and later kinds (managed, M5) need no table rebuild.
const MIGRATION_2: &str = r#"
CREATE TABLE station_profiles (
  id               TEXT PRIMARY KEY,
  callsign         TEXT NOT NULL,
  name             TEXT,
  grid             TEXT,
  modem_kind       TEXT NOT NULL,
  host             TEXT,
  control_port     INTEGER,
  data_port        INTEGER,
  broadcast_port   INTEGER,
  telemetry_url    TEXT,
  lab              INTEGER NOT NULL DEFAULT 0,
  listen_preferred INTEGER NOT NULL DEFAULT 0,
  created_utc      TEXT NOT NULL,
  updated_utc      TEXT NOT NULL
);
CREATE TABLE settings (
  key         TEXT PRIMARY KEY,
  value       TEXT NOT NULL,
  updated_utc TEXT NOT NULL
);
"#;

fn user_version(conn: &Connection) -> rusqlite::Result<i64> {
    conn.pragma_query_value(None, "user_version", |r| r.get(0))
}

pub(crate) fn run(conn: &mut Connection, db_path: Option<&Path>, migrations: &[Migration]) -> Result<()> {
    let latest = migrations.last().map_or(0, |m| m.to);
    let mut at = user_version(conn)?;
    if at > latest {
        return Err(StorageError::TooNew(at));
    }
    if at == latest {
        return Ok(());
    }
    let backup = match db_path {
        Some(p) if at >= 1 => Some(backup_before(conn, p, at)?),
        _ => None,
    };
    let start = at;
    for m in migrations.iter().filter(|m| m.to > start) {
        let from = at;
        let failed = |source| StorageError::Migration { from, to: m.to, backup: backup.clone(), source };
        let tx = conn.transaction().map_err(failed)?;
        tx.execute_batch(m.sql).map_err(failed)?;
        tx.pragma_update(None, "user_version", m.to).map_err(failed)?;
        tx.commit().map_err(failed)?;
        at = m.to;
    }
    Ok(())
}

fn backup_before(conn: &Connection, db_path: &Path, from: i64) -> Result<PathBuf> {
    let dir = db_path.parent().unwrap_or(Path::new(".")).join("backups");
    let stem = db_path.file_stem().map_or_else(|| "station".into(), |s| s.to_string_lossy().into_owned());
    let dest = dir.join(format!("{stem}-v{from}-{}.db", chrono::Utc::now().format("%Y%m%dT%H%M%SZ")));
    let err = |reason: String| StorageError::Backup { path: dest.clone(), reason };
    std::fs::create_dir_all(&dir).map_err(|e| err(e.to_string()))?;
    conn.backup(rusqlite::MAIN_DB, &dest, None).map_err(|e| err(e.to_string()))?;
    Ok(dest)
}
