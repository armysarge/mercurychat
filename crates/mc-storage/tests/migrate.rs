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
    assert_eq!(s.schema_version().unwrap(), MIGRATIONS.last().unwrap().to);
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
    assert_eq!(s.schema_version().unwrap(), MIGRATIONS.last().unwrap().to);
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

fn profile_row(id: &str) -> mc_storage::ProfileRow {
    mc_storage::ProfileRow {
        id: id.into(),
        callsign: "ZS6ABC".into(),
        name: None,
        grid: None,
        modem_kind: "simulator".into(),
        host: None,
        control_port: None,
        data_port: None,
        broadcast_port: None,
        telemetry_url: None,
        lab: false,
        receive_only: false,
        qth: None,
        rig: None,
        power: None,
        antenna: None,
        email: None,
        about: None,
    }
}

#[test]
fn v3_profile_survives_migration_4_with_station_info_absent() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("app.db");
    {
        let conn = Connection::open(&path).unwrap();
        for m in &MIGRATIONS[..3] {
            conn.execute_batch(m.sql).unwrap();
        }
        conn.pragma_update(None, "user_version", 3).unwrap();
        conn.execute(
            "INSERT INTO station_profiles (id, callsign, modem_kind, lab, created_utc, updated_utc)
             VALUES ('p3', 'ZS6ABC', 'simulator', 0, '2025-01-01T00:00:00Z', '2025-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
    }
    let s = Store::open(&path).unwrap();
    let rows = s.profiles().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].callsign, "ZS6ABC");
    assert_eq!(
        (&rows[0].qth, &rows[0].rig, &rows[0].power, &rows[0].antenna, &rows[0].email, &rows[0].about),
        (&None, &None, &None, &None, &None, &None)
    );
}

#[test]
fn deleting_a_profile_deletes_its_macros() {
    let mut s = Store::open_in_memory().unwrap();
    s.save_profile(&profile_row("p1")).unwrap();
    s.save_profile(&profile_row("p2")).unwrap();
    for (id, profile) in [("m1", "p1"), ("m2", "p1"), ("m3", "p2")] {
        s.save_macro(&mc_storage::MacroRow {
            id: id.into(),
            profile_id: profile.into(),
            name: id.into(),
            body: "b".into(),
            sort: 0,
            quick: false,
        })
        .unwrap();
    }
    assert!(s.delete_profile("p1").unwrap());
    assert!(s.macros("p1").unwrap().is_empty(), "the deleted profile's macros went with it");
    assert_eq!(s.macros("p2").unwrap().len(), 1, "another profile's macros stay");
}

#[test]
fn migration_7_adds_source_columns() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("station.db");
    {
        let s = Store::open_with_migrations(&path, &MIGRATIONS[..6]).unwrap();
        drop(s);
        Connection::open(&path)
            .unwrap()
            .execute(
                "INSERT INTO qso_records (id, peer, start_utc, rst_rcvd, name, state)
                 VALUES ('q6', 'ZS6ABC', '2026-11-01T00:00:00Z', '59', 'Pat', 'draft')",
                [],
            )
            .unwrap();
    }
    assert_eq!(version(&path), 6);
    let s = Store::open(&path).unwrap();
    assert!(version(&path) >= 7);
    let cols: Vec<String> = {
        let conn = Connection::open(&path).unwrap();
        let mut st = conn.prepare("SELECT name FROM pragma_table_info('qso_records')").unwrap();
        st.query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
    };
    for c in ["rst_rcvd_source", "name_source", "qth_source"] {
        assert!(cols.iter().any(|n| n == c), "no {c} in {cols:?}");
    }
    // A record from before keeps its values, with no source.
    let q = &s.qsos(10).unwrap()[0];
    assert_eq!((q.rst_rcvd.as_deref(), q.name.as_deref()), (Some("59"), Some("Pat")));
    assert_eq!((&q.rst_rcvd_source, &q.name_source, &q.qth_source), (&None, &None, &None));
}

#[test]
fn migration_8_defaults_quick_off() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("station.db");
    {
        let s = Store::open_with_migrations(&path, &MIGRATIONS[..7]).unwrap();
        drop(s);
        Connection::open(&path)
            .unwrap()
            .execute("INSERT INTO macros (id, profile_id, name, body, sort) VALUES ('m1', 'p1', 'TU', 'TU 73', 0)", [])
            .unwrap();
    }
    assert_eq!(version(&path), 7);
    let mut s = Store::open(&path).unwrap();
    assert!(version(&path) >= 8);
    let m = s.macros("p1").unwrap().remove(0);
    assert_eq!((m.name.as_str(), m.quick), ("TU", false), "a macro from before is not quick");
    s.save_macro(&mc_storage::MacroRow { quick: true, ..m }).unwrap();
    assert!(s.macros("p1").unwrap()[0].quick);
}

/// M4 C5: migration 9 extends M0's transfers table; an old row keeps its
/// values and reads with wire size = size and a creation time.
#[test]
fn migration_9_adds_transfers() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("station.db");
    {
        let _s = Store::open_with_migrations(&path, &MIGRATIONS[..8]).unwrap();
    }
    {
        // An M0-era row, as M0's engine wrote it.
        let c = rusqlite::Connection::open(&path).unwrap();
        c.execute(
            "INSERT INTO transfers (wire_id, peer, direction, offered_name, size, sha256, state, updated_utc)
             VALUES (?1, 'ZS6PEER', 'out', 'old.txt', 42, 'ab', 'delivered', '2026-01-01T00:00:00Z')",
            rusqlite::params![&ID[..]],
        )
        .unwrap();
    }
    assert_eq!(version(&path), 8);
    let s = Store::open(&path).unwrap();
    assert!(version(&path) >= 9);
    assert!(!backups(dir.path()).is_empty(), "a backup before upgrading");
    let r = s.transfer(mc_storage::Direction::Out, &ID).unwrap().expect("the old row");
    assert_eq!((r.original_name.as_str(), r.size, r.wire_size), ("old.txt", 42, 42));
    assert_eq!(r.state, mc_storage::TransferState::Complete, "M0's delivered is complete");
    assert_eq!(r.created_utc, r.updated_utc);
}
