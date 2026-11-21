//! The frequency list (M3b, D15): migration 6, round trips and presets.

use mc_storage::{FrequencyRow, Store, MIGRATIONS};
use rusqlite::Connection;

fn version(path: &std::path::Path) -> i64 {
    Connection::open(path).unwrap().pragma_query_value(None, "user_version", |r| r.get(0)).unwrap()
}

fn row(id: &str, freq_hz: u64, sort: i64) -> FrequencyRow {
    FrequencyRow {
        id: id.into(),
        name: format!("entry {id}"),
        freq_hz,
        mode: "PKTUSB".into(),
        passband_hz: Some(3000),
        purpose: vec!["cq".into(), "chat".into()],
        notes: String::new(),
        suggested: false,
        preset_key: None,
        region: None,
        sort,
    }
}

fn preset(key: &str, freq_hz: u64) -> FrequencyRow {
    FrequencyRow { suggested: true, preset_key: Some(key.into()), region: Some(1), ..row(key, freq_hz, 0) }
}

#[test]
fn migration_6_creates_frequency_list() {
    let dir = tempfile::tempdir().unwrap();
    let fresh = dir.path().join("fresh.db");
    let s = Store::open(&fresh).unwrap();
    assert!(s.schema_version().unwrap() >= 6);
    assert!(s.frequencies().unwrap().is_empty());
    drop(s);

    let old = dir.path().join("old.db");
    drop(Store::open_with_migrations(&old, &MIGRATIONS[..5]).unwrap());
    assert_eq!(version(&old), 5);
    let s = Store::open(&old).unwrap();
    assert!(s.schema_version().unwrap() >= 6);
    assert!(s.frequencies().unwrap().is_empty());
}

#[test]
fn frequencies_round_trip_and_sort() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Store::open(&dir.path().join("a.db")).unwrap();
    let b = row("b", 7_074_000, 1);
    let a = row("a", 14_105_000, 0);
    let c =
        FrequencyRow { notes: "net on Sundays".into(), passband_hz: None, purpose: vec![], ..row("c", 3_590_000, 1) };
    for r in [&b, &a, &c] {
        s.save_frequency(r).unwrap();
    }
    assert_eq!(s.frequencies().unwrap(), vec![a.clone(), c.clone(), b.clone()], "by sort, then frequency");
    let a2 = FrequencyRow { name: "renamed".into(), ..a.clone() };
    s.save_frequency(&a2).unwrap();
    assert_eq!(s.frequencies().unwrap()[0], a2);
    assert!(s.delete_frequency("c").unwrap());
    assert!(!s.delete_frequency("c").unwrap());
    assert_eq!(s.frequencies().unwrap().len(), 2);
}

#[test]
fn insert_missing_presets_skips_edited_and_existing() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Store::open(&dir.path().join("a.db")).unwrap();
    let set = [preset("r1-40m-a", 7_045_000), preset("r1-20m-a", 14_105_000)];
    assert_eq!(s.insert_missing_presets(&set).unwrap(), 2);
    assert_eq!(s.insert_missing_presets(&set).unwrap(), 0);
    let mut edited = s.frequencies().unwrap().into_iter().find(|r| r.id == "r1-40m-a").unwrap();
    edited.name = "My 40 m".into();
    s.save_frequency(&edited).unwrap();
    assert!(s.delete_frequency("r1-20m-a").unwrap());
    assert_eq!(s.insert_missing_presets(&set).unwrap(), 1, "only the deleted one comes back");
    let all = s.frequencies().unwrap();
    assert_eq!(all.len(), 2);
    assert!(all.iter().any(|r| r.name == "My 40 m"), "the edit is kept");
}
