//! docs/02 §6: migrations take a backup first, run one transaction each, and a
//! failure leaves the original database recoverable at its old version.

use mc_storage::{Migration, StorageError, Store, MIGRATIONS};
use rusqlite::Connection;
use std::path::{Path, PathBuf};

const ID: [u8; 16] = [7; 16];

fn version(path: &Path) -> i64 {
    Connection::open(path).unwrap().pragma_query_value(None, "user_version", |r| r.get(0)).unwrap()
}

fn has_table(path: &Path, name: &str) -> bool {
    let n: i64 = Connection::open(path)
        .unwrap()
        .query_row("SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1", [name], |r| r.get(0))
        .unwrap();
    n == 1
}

/// A version-1 database holding one incoming chat.
fn v1_db(dir: &Path) -> PathBuf {
    let path = dir.join("station.db");
    let mut s = Store::open_with_migrations(&path, &MIGRATIONS[..1]).unwrap();
    s.store_incoming_chat("ZS6ABC", &ID, "hello", 1).unwrap();
    drop(s);
    assert_eq!(version(&path), 1);
    path
}

fn chat_survives(s: &Store) {
    let rows = s.messages("ZS6ABC").unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].body, "hello");
}

fn backups(dir: &Path) -> Vec<PathBuf> {
    match std::fs::read_dir(dir.join("backups")) {
        Ok(rd) => rd.map(|e| e.unwrap().path()).collect(),
        Err(_) => Vec::new(),
    }
}

#[test]
fn fresh_database_is_at_latest() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("station.db");
    let s = Store::open(&path).unwrap();
    assert_eq!(s.schema_version().unwrap(), 2);
    drop(s);
    assert!(has_table(&path, "station_profiles"));
    assert!(has_table(&path, "settings"));
    assert!(backups(dir.path()).is_empty(), "a new database needs no backup");
}

#[test]
fn v1_database_upgrades_with_backup() {
    let dir = tempfile::tempdir().unwrap();
    let path = v1_db(dir.path());
    let s = Store::open(&path).unwrap();
    assert_eq!(s.schema_version().unwrap(), 2);
    chat_survives(&s);
    drop(s);

    let b = backups(dir.path());
    assert_eq!(b.len(), 1, "{b:?}");
    let name = b[0].file_name().unwrap().to_string_lossy().into_owned();
    assert!(name.starts_with("station-v1-") && name.ends_with("Z.db"), "{name}");
    assert_eq!(version(&b[0]), 1);
    let old = Store::open_with_migrations(&b[0], &MIGRATIONS[..1]).unwrap();
    chat_survives(&old);
}

#[test]
fn failed_migration_leaves_original_recoverable() {
    let dir = tempfile::tempdir().unwrap();
    let path = v1_db(dir.path());
    let broken = [MIGRATIONS[0], Migration { to: 2, sql: "CREATE TABLE t(a); SELECT no_such_fn();" }];
    match Store::open_with_migrations(&path, &broken) {
        Err(e @ StorageError::Migration { from: 1, to: 2, backup: Some(_), .. }) => {
            let msg = e.to_string();
            assert!(
                msg.contains("the database was left at version 1; a copy taken before the upgrade is at "),
                "{msg}"
            );
        }
        Err(e) => panic!("expected Migration error, got {e}"),
        Ok(_) => panic!("broken migration succeeded"),
    }
    assert_eq!(version(&path), 1);
    assert!(!has_table(&path, "t"), "the failed migration's changes were rolled back");
    let s = Store::open_with_migrations(&path, &MIGRATIONS[..1]).unwrap();
    chat_survives(&s);
}

#[test]
fn backup_failure_blocks_migration() {
    let dir = tempfile::tempdir().unwrap();
    let path = v1_db(dir.path());
    std::fs::write(dir.path().join("backups"), "not a directory").unwrap();
    match Store::open(&path) {
        Err(StorageError::Backup { .. }) => {}
        Err(e) => panic!("expected Backup error, got {e}"),
        Ok(_) => panic!("migrated without a backup"),
    }
    assert_eq!(version(&path), 1);
    assert!(!has_table(&path, "station_profiles"));
}

#[test]
fn newer_database_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("station.db");
    Connection::open(&path).unwrap().pragma_update(None, "user_version", 99).unwrap();
    assert!(matches!(Store::open(&path), Err(StorageError::TooNew(99))));
}
