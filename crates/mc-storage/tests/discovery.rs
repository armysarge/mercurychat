//! M3a storage: heard observations, favourites, QSO drafts, macros, history search.

use mc_storage::{HeardRow, MacroRow, QsoEdit, QsoRow, StorageError, Store, MIGRATIONS};

fn heard(call: &str, utc: &str, kind: &str) -> HeardRow {
    HeardRow {
        id: 0,
        callsign: call.into(),
        observed_utc: utc.into(),
        kind: kind.into(),
        bandwidth: None,
        grid: None,
        caps: None,
        snr_db: None,
        snr_source: None,
        freq_hz: None,
        freq_source: None,
        freq_age_ms: None,
    }
}

fn qso(id: &str, session: Option<&str>) -> QsoRow {
    QsoRow {
        id: id.into(),
        session_id: session.map(Into::into),
        peer: "ZS6ABC".into(),
        start_utc: "2026-01-01T10:00:00.000Z".into(),
        end_utc: None,
        grid: None,
        band: None,
        freq_hz: None,
        freq_source: None,
        freq_age_ms: None,
        bandwidth: None,
        tx_modes: None,
        rx_modes: None,
        rst_sent: None,
        rst_rcvd: None,
        name: None,
        qth: None,
        my_power: None,
        notes: None,
        state: "draft".into(),
        rst_rcvd_source: None,
        name_source: None,
        qth_source: None,
    }
}

#[test]
fn migration_4_upgrades_an_m2_database() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("station.db");
    let mut s = Store::open_with_migrations(&path, &MIGRATIONS[..3]).unwrap();
    assert_eq!(s.schema_version().unwrap(), 3);
    s.store_incoming_chat("ZS6ABC", &[5; 16], "hello", 1).unwrap();
    drop(s);

    let mut s = Store::open(&path).unwrap();
    assert_eq!(s.schema_version().unwrap(), MIGRATIONS.last().unwrap().to);
    let rows = s.messages("ZS6ABC").unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].body, "hello");
    // The new tables work on the upgraded file.
    s.record_heard(&heard("ZS6ABC", "2026-01-01T10:00:00.000Z", "cq")).unwrap();
    s.create_qso(&qso("q1", None)).unwrap();
    assert_eq!(s.qsos(10).unwrap().len(), 1);
}

#[test]
fn heard_summary_keeps_latest_and_count() {
    let mut s = Store::open_in_memory().unwrap();
    let mut a = heard("ZS6ABC", "2026-01-01T10:00:00.000Z", "cq");
    a.grid = Some("KG33".into());
    s.record_heard(&a).unwrap();
    let mut b = heard("ZS6ABC", "2026-01-01T11:00:00.000Z", "beacon");
    b.freq_hz = Some(7_078_000);
    b.freq_age_ms = Some(1500);
    let id = s.record_heard(&b).unwrap();
    s.set_heard_snr(id, 12.5, "modem").unwrap();
    s.record_heard(&heard("ZS1XYZ", "2026-01-01T09:00:00.000Z", "cq")).unwrap();

    let sum = s.heard_summary().unwrap();
    assert_eq!(sum.len(), 2);
    assert_eq!(sum[0].callsign, "ZS6ABC");
    assert_eq!(sum[0].count, 2);
    assert_eq!(sum[0].kind, "beacon");
    assert_eq!(sum[0].last_utc, "2026-01-01T11:00:00.000Z");
    assert_eq!(sum[0].snr_db, Some(12.5));
    assert_eq!(sum[0].freq_hz, Some(7_078_000));
    assert_eq!(sum[0].freq_age_ms, Some(1500));
    assert!(!sum[0].favourite);
    assert_eq!(sum[1].callsign, "ZS1XYZ");
    assert_eq!(sum[1].count, 1);
}

#[test]
fn latest_heard_respects_kind_and_window() {
    let mut s = Store::open_in_memory().unwrap();
    s.record_heard(&heard("ZS6ABC", &mc_storage::now_utc(), "cq")).unwrap();
    s.record_heard(&heard("ZS6ABC", "2020-01-01T00:00:00.000Z", "beacon")).unwrap();
    assert!(s.latest_heard("ZS6ABC", "cq", 60).unwrap().is_some());
    assert!(s.latest_heard("ZS6ABC", "beacon", 60).unwrap().is_none());
    assert!(s.latest_heard("ZS1XYZ", "cq", 60).unwrap().is_none());
}

#[test]
fn purge_drops_only_old_observations() {
    let mut s = Store::open_in_memory().unwrap();
    s.record_heard(&heard("ZS6ABC", "2026-01-01T00:00:00.000Z", "cq")).unwrap();
    s.record_heard(&heard("ZS6ABC", "2026-01-20T00:00:00.000Z", "cq")).unwrap();
    s.record_heard(&heard("ZS1XYZ", "2026-01-25T00:00:00.000Z", "cq")).unwrap();
    // Cut-off 30 days before 2026-01-31.
    let n = s.purge_heard_before("2026-01-01T00:00:00.001Z").unwrap();
    assert_eq!(n, 1);
    let sum = s.heard_summary().unwrap();
    assert_eq!(sum.iter().map(|r| r.count).sum::<i64>(), 2);
}

#[test]
fn favourite_survives_heard_purge() {
    let mut s = Store::open_in_memory().unwrap();
    s.record_heard(&heard("ZS6ABC", "2026-01-01T00:00:00.000Z", "cq")).unwrap();
    s.set_favourite("ZS6ABC", true).unwrap();
    assert!(s.heard_summary().unwrap()[0].favourite);
    s.purge_heard_before("2026-02-01T00:00:00.000Z").unwrap();
    assert!(s.heard_summary().unwrap().is_empty());
    // Still a favourite with nothing heard: listed through `favourites`.
    assert_eq!(s.favourites().unwrap(), ["ZS6ABC"]);
    s.record_heard(&heard("ZS6ABC", "2026-03-01T00:00:00.000Z", "cq")).unwrap();
    assert!(s.heard_summary().unwrap()[0].favourite);
    s.set_favourite("ZS6ABC", false).unwrap();
    assert!(!s.heard_summary().unwrap()[0].favourite);
    assert!(s.favourites().unwrap().is_empty());
}

#[test]
fn final_qso_refuses_edit() {
    let mut s = Store::open_in_memory().unwrap();
    s.create_qso(&qso("q1", Some("s1"))).unwrap();
    let edit = QsoEdit { name: Some("Sam".into()), freq_age_ms: Some(900), ..Default::default() };
    let r = s.update_qso("q1", &edit).unwrap();
    assert_eq!(r.name.as_deref(), Some("Sam"));
    assert_eq!(r.freq_age_ms, Some(900));
    assert_eq!(r.grid, None);
    let f = s.finalise_qso("q1").unwrap();
    assert_eq!(f.state, "final");
    let err = s.update_qso("q1", &QsoEdit { name: Some("Other".into()), ..Default::default() }).unwrap_err();
    assert!(matches!(err, StorageError::QsoFinal(_)), "{err:?}");
    assert_eq!(s.qsos(10).unwrap()[0].name.as_deref(), Some("Sam"));
    assert!(matches!(s.update_qso("nope", &edit).unwrap_err(), StorageError::QsoNotFound(_)));
}

#[test]
fn end_qso_sets_end_once() {
    let mut s = Store::open_in_memory().unwrap();
    s.create_qso(&qso("q1", Some("s1"))).unwrap();
    let r = s.end_qso_for_session("s1", "2026-01-01T10:30:00.000Z").unwrap().unwrap();
    assert_eq!(r.end_utc.as_deref(), Some("2026-01-01T10:30:00.000Z"));
    assert!(s.end_qso_for_session("s1", "2026-01-01T11:00:00.000Z").unwrap().is_none());
    assert_eq!(s.qsos(10).unwrap()[0].end_utc.as_deref(), Some("2026-01-01T10:30:00.000Z"));
    assert!(s.end_qso_for_session("unknown", "2026-01-01T11:00:00.000Z").unwrap().is_none());
}

#[test]
fn search_escapes_like_wildcards() {
    let mut s = Store::open_in_memory().unwrap();
    s.insert_outgoing("ZS6ABC", &[1; 16], "signal is 100% good").unwrap();
    s.insert_outgoing("ZS6ABC", &[2; 16], "signal is 1000 good").unwrap();
    s.insert_outgoing("ZS6ABC", &[3; 16], "snake_case and snakeXcase").unwrap();
    let hits = s.search_messages("100%", 10).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].body, "signal is 100% good");
    let hits = s.search_messages("snake_case", 10).unwrap();
    assert_eq!(hits.len(), 1);
    assert!(s.search_messages("", 10).unwrap().is_empty());
}

#[test]
fn search_matches_callsign_case_insensitively() {
    let mut s = Store::open_in_memory().unwrap();
    s.insert_outgoing("ZS6ABC", &[1; 16], "first").unwrap();
    s.insert_outgoing("ZS1XYZ", &[2; 16], "second").unwrap();
    s.insert_outgoing("ZS6ABC", &[3; 16], "third").unwrap();
    let hits = s.search_messages("zs6abc", 10).unwrap();
    assert_eq!(hits.iter().map(|m| m.body.as_str()).collect::<Vec<_>>(), ["third", "first"]);
    assert_eq!(s.search_messages("zs6abc", 1).unwrap().len(), 1);
}

#[test]
fn macros_round_trip_in_order() {
    let mut s = Store::open_in_memory().unwrap();
    let m = |id: &str, sort| MacroRow {
        id: id.into(),
        profile_id: "p1".into(),
        name: id.into(),
        body: "73".into(),
        sort,
        quick: false,
    };
    s.save_macro(&m("b", 2)).unwrap();
    s.save_macro(&m("a", 1)).unwrap();
    s.save_macro(&MacroRow { profile_id: "p2".into(), ..m("c", 1) }).unwrap();
    let ids: Vec<_> = s.macros("p1").unwrap().into_iter().map(|x| x.id).collect();
    assert_eq!(ids, ["a", "b"]);
    let mut edited = m("a", 3);
    edited.body = "CQ CQ".into();
    s.save_macro(&edited).unwrap();
    assert_eq!(s.macros("p1").unwrap()[1].body, "CQ CQ");
    assert!(s.delete_macro("a").unwrap());
    assert!(!s.delete_macro("a").unwrap());
}

#[test]
fn a_new_frequency_replaces_the_whole_reading() {
    let mut s = Store::open_in_memory().unwrap();
    let mut q = qso("q1", Some("s1"));
    (q.freq_hz, q.freq_source, q.freq_age_ms, q.band) =
        (Some(14_105_000), Some("mercury telemetry".into()), Some(200), Some("20m".into()));
    s.create_qso(&q).unwrap();
    let edit = QsoEdit { freq_hz: Some(7_074_000), freq_source: Some("operator".into()), ..Default::default() };
    let r = s.update_qso("q1", &edit).unwrap();
    assert_eq!(
        (r.freq_hz, r.freq_source.as_deref(), r.freq_age_ms, r.band),
        (Some(7_074_000), Some("operator"), None, None)
    );
}
