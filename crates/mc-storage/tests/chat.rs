//! M2 chat storage: conversation list, history pages, per-peer queue usage
//! (CHT-03) and session rows, on the station's own database.

use mc_storage::{OutState, SessionRow, Store, MIGRATIONS};
use std::path::Path;

fn id(n: u8) -> [u8; 16] {
    let mut b = [0u8; 16];
    b[15] = n;
    b
}

fn out(s: &mut Store, peer: &str, n: u8, body: &str) {
    s.insert_outgoing(peer, &id(n), body).unwrap();
}

#[test]
fn conversations_order_by_latest() {
    let mut s = Store::open_in_memory().unwrap();
    out(&mut s, "ZS6AAA", 1, "a");
    s.store_incoming_chat("ZS6BBB", &id(2), "b", 1).unwrap();
    out(&mut s, "ZS6AAA", 3, "c");
    s.set_out_state("ZS6AAA", &id(3), OutState::Uncertain, None).unwrap();
    let c = s.conversations().unwrap();
    assert_eq!(c.iter().map(|c| c.peer.as_str()).collect::<Vec<_>>(), ["ZS6AAA", "ZS6BBB"]);
    assert_eq!((c[0].messages, c[0].uncertain), (2, 1));
    assert_eq!((c[1].messages, c[1].uncertain), (1, 0));
    assert!(!c[0].last_utc.is_empty());
}

#[test]
fn history_pages_backwards() {
    let mut s = Store::open_in_memory().unwrap();
    for n in 0..25u8 {
        out(&mut s, "ZS6AAA", n, &format!("m{n}"));
    }
    out(&mut s, "ZS6OTHER", 100, "elsewhere");
    let latest = s.history("ZS6AAA", None, 10).unwrap();
    let bodies = |v: &[mc_storage::MessageRow]| v.iter().map(|m| m.body.clone()).collect::<Vec<_>>();
    assert_eq!(bodies(&latest), (15..25).map(|n| format!("m{n}")).collect::<Vec<_>>());
    let older = s.history("ZS6AAA", Some(latest[0].seq), 10).unwrap();
    assert_eq!(bodies(&older), (5..15).map(|n| format!("m{n}")).collect::<Vec<_>>());
    let oldest = s.history("ZS6AAA", Some(older[0].seq), 10).unwrap();
    assert_eq!(bodies(&oldest), (0..5).map(|n| format!("m{n}")).collect::<Vec<_>>());
    assert!(latest.windows(2).all(|w| w[0].seq < w[1].seq), "oldest first within a page");
}

#[test]
fn history_rows_carry_times_and_session() {
    let mut s = Store::open_in_memory().unwrap();
    s.store_incoming_chat("ZS6AAA", &id(1), "hi", 7).unwrap();
    let m = &s.history("ZS6AAA", None, 10).unwrap()[0];
    assert_eq!((m.direction.as_str(), m.session_gen), ("in", Some(7)));
    assert!(m.created_utc.ends_with('Z') && m.received_utc.is_some());
    assert_eq!(s.message("ZS6AAA", &id(1)).unwrap().as_ref(), Some(m));
    assert_eq!(s.message("ZS6AAA", &id(9)).unwrap(), None);
}

#[test]
fn queue_usage_counts_only_unsettled() {
    let mut s = Store::open_in_memory().unwrap();
    out(&mut s, "ZS6AAA", 1, "1234"); // queued
    out(&mut s, "ZS6AAA", 2, "12345678"); // submitting
    s.set_out_state("ZS6AAA", &id(2), OutState::Submitting, Some(1)).unwrap();
    out(&mut s, "ZS6AAA", 3, "xx"); // awaiting receipt
    s.set_out_state("ZS6AAA", &id(3), OutState::AwaitingReceipt, None).unwrap();
    out(&mut s, "ZS6AAA", 4, "delivered");
    s.set_out_state("ZS6AAA", &id(4), OutState::Delivered, None).unwrap();
    out(&mut s, "ZS6AAA", 5, "uncertain");
    s.set_out_state("ZS6AAA", &id(5), OutState::Uncertain, None).unwrap();
    s.store_incoming_chat("ZS6AAA", &id(6), "incoming", 1).unwrap();
    assert_eq!(s.peer_queue_usage("ZS6AAA").unwrap(), (3, 14));
    assert_eq!(s.peer_queue_usage("ZS6NONE").unwrap(), (0, 0));
}

#[test]
fn queued_for_keeps_insert_order() {
    let mut s = Store::open_in_memory().unwrap();
    for n in [5u8, 1, 3] {
        out(&mut s, "ZS6AAA", n, &format!("q{n}"));
    }
    s.set_out_state("ZS6AAA", &id(1), OutState::Uncertain, None).unwrap();
    let q = s.queued_for("ZS6AAA").unwrap();
    assert_eq!(q.iter().map(|m| m.body.as_str()).collect::<Vec<_>>(), ["q5", "q3"]);
}

#[test]
fn session_rows_round_trip() {
    let mut s = Store::open_in_memory().unwrap();
    let row = SessionRow {
        id: "s1".into(),
        peer: "ZS6AAA".into(),
        generation: 3,
        direction: "out".into(),
        started_utc: "2026-11-15T10:00:00Z".into(),
        ended_utc: None,
        peer_app: Some("MercuryChat 0.0.1".into()),
        outcome: None,
    };
    s.begin_session(&row).unwrap();
    s.end_session("s1", "disconnected").unwrap();
    let back = s.session("s1").unwrap().unwrap();
    assert_eq!(back.outcome.as_deref(), Some("disconnected"));
    assert!(back.ended_utc.is_some());
    assert_eq!((back.peer, back.generation, back.peer_app), (row.peer, 3, row.peer_app));
}

#[test]
fn migration_3_on_an_m1_database() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("station.db");
    {
        let mut s = Store::open_with_migrations(&path, &MIGRATIONS[..2]).unwrap();
        s.store_incoming_chat("ZS6AAA", &id(1), "kept", 1).unwrap();
        assert_eq!(s.schema_version().unwrap(), 2);
    }
    let s = Store::open(&path).unwrap();
    assert_eq!(s.schema_version().unwrap(), MIGRATIONS.last().unwrap().to);
    assert!(MIGRATIONS.last().unwrap().to >= 3);
    assert_eq!(s.history("ZS6AAA", None, 10).unwrap()[0].body, "kept");
    let backups: Vec<_> = std::fs::read_dir(dir.path().join("backups")).unwrap().collect();
    assert_eq!(backups.len(), 1, "a backup is taken before the upgrade");
    assert!(Path::new(&backups[0].as_ref().unwrap().path()).to_string_lossy().contains("-v2-"));
}
