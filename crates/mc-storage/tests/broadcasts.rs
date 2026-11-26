//! General broadcasts heard (the latest-broadcasts list).

use mc_storage::{BroadcastRow, HeardRow, Store, MIGRATIONS};

fn row(n: u32, utc: &str) -> BroadcastRow {
    BroadcastRow {
        id: 0,
        observed_utc: utc.into(),
        callsign: format!("ZS6AA{n}"),
        message: format!("message {n}"),
        snr_db: None,
        snr_source: None,
        mode: None,
        freq_hz: Some(7_105_000),
        freq_source: Some("telemetry".into()),
        freq_age_ms: Some(120),
    }
}

fn minute(n: u32) -> String {
    format!("2026-11-26T14:{n:02}:00.000Z")
}

#[test]
fn newest_first_and_limited() {
    let mut s = Store::open_in_memory().unwrap();
    for n in 0..12 {
        s.record_broadcast(&row(n, &minute(n))).unwrap();
    }
    // The same instant: the later insert is the newer.
    let tie = s.record_broadcast(&row(99, &minute(11))).unwrap();
    let got = s.latest_broadcasts(10).unwrap();
    assert_eq!(got.len(), 10);
    assert_eq!(got[0].id, tie);
    assert_eq!(got[1].message, "message 11");
    assert_eq!(got[9].message, "message 3");
    assert_eq!(got[1], BroadcastRow { id: got[1].id, ..row(11, &minute(11)) });
}

#[test]
fn snr_arrives_later() {
    let mut s = Store::open_in_memory().unwrap();
    let id = s.record_broadcast(&row(1, &minute(1))).unwrap();
    s.set_broadcast_snr(id, 7.5, "modem monitor", "DATAC3").unwrap();
    let got = s.broadcast(id).unwrap().unwrap();
    assert_eq!(
        (got.snr_db, got.snr_source.as_deref(), got.mode.as_deref()),
        (Some(7.5), Some("modem monitor"), Some("DATAC3"))
    );
    assert_eq!(s.broadcast(id + 1).unwrap(), None);
}

#[test]
fn purge_drops_only_older() {
    let mut s = Store::open_in_memory().unwrap();
    s.record_broadcast(&row(1, "2026-10-01T00:00:00.000Z")).unwrap();
    s.record_broadcast(&row(2, &minute(2))).unwrap();
    assert_eq!(s.purge_broadcasts_before("2026-11-01T00:00:00.000Z").unwrap(), 1);
    let left: Vec<String> = s.latest_broadcasts(10).unwrap().into_iter().map(|r| r.message).collect();
    assert_eq!(left, ["message 2"]);
}

#[test]
fn a_version_10_database_upgrades_with_its_data() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("station.db");
    let mut old = Store::open_with_migrations(&path, &MIGRATIONS[..10]).unwrap();
    old.record_heard(&HeardRow {
        id: 0,
        callsign: "ZS6ABC".into(),
        observed_utc: minute(1),
        kind: "cq".into(),
        bandwidth: None,
        grid: None,
        caps: None,
        snr_db: Some(3.0),
        snr_source: Some("modem monitor".into()),
        freq_hz: None,
        freq_source: None,
        freq_age_ms: None,
    })
    .unwrap();
    drop(old);
    let mut s = Store::open(&path).unwrap();
    assert_eq!(s.schema_version().unwrap(), 11);
    assert_eq!(s.heard_summary().unwrap().len(), 1);
    assert_eq!(s.latest_broadcasts(10).unwrap(), []);
    s.record_broadcast(&row(1, &minute(2))).unwrap();
    assert_eq!(std::fs::read_dir(dir.path().join("backups")).unwrap().count(), 1, "backed up first");
}
