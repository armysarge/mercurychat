//! docs/02 §3: events carry a monotonic sequence and a session generation; a
//! client that falls behind the ring is told to take a snapshot.

use mc_core::app::{is_current, AppEvent, EventLog, ModemState, Since};

fn ev() -> AppEvent {
    AppEvent::ProfilesChanged
}

fn seqs(s: Since) -> Vec<u64> {
    match s {
        Since::Events { events } => events.iter().map(|e| e.seq).collect(),
        Since::SnapshotNeeded => panic!("unexpected SnapshotNeeded"),
    }
}

#[test]
fn seq_is_monotonic_from_one() {
    let mut log = EventLog::new(16);
    let got: Vec<u64> = (0..3).map(|_| log.push(ev()).seq).collect();
    assert_eq!(got, [1, 2, 3]);
    assert_eq!(log.last_seq(), 3);
}

#[test]
fn since_returns_only_newer() {
    let mut log = EventLog::new(16);
    for _ in 0..5 {
        log.push(ev());
    }
    assert_eq!(seqs(log.since(3)), [4, 5]);
}

#[test]
fn since_older_than_ring_needs_snapshot() {
    let mut log = EventLog::new(4);
    for _ in 0..10 {
        log.push(ev());
    }
    assert!(matches!(log.since(2), Since::SnapshotNeeded));
    assert_eq!(seqs(log.since(6)), [7, 8, 9, 10]);
    assert_eq!(seqs(log.since(10)), Vec::<u64>::new());
}

#[test]
fn since_zero_on_a_fresh_log_is_empty_not_snapshot() {
    let log = EventLog::new(4);
    assert_eq!(seqs(log.since(0)), Vec::<u64>::new());
}

#[test]
fn stale_generation_is_filtered() {
    let mut log = EventLog::new(16);
    let old = log.push(ev());
    let gen = log.bump_generation();
    assert_eq!(gen, old.generation + 1);
    assert!(!is_current(&old, gen));
    assert!(is_current(&log.push(ev()), gen));
}

#[test]
fn modem_state_wire_names() {
    assert_eq!(serde_json::to_string(&ModemState::Draining).unwrap(), r#"{"type":"draining"}"#);
    let e = AppEvent::ModemState { state: ModemState::Ready, detail: Some("simulator".into()) };
    assert_eq!(
        serde_json::to_string(&e).unwrap(),
        r#"{"type":"modemState","state":{"type":"ready"},"detail":"simulator"}"#
    );
}

#[test]
fn bindings_are_exported() {
    mc_core::app::export_bindings().unwrap();
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../apps/desktop/src/bindings");
    for f in ["ModemState.ts", "AppEvent.ts", "EventEnvelope.ts", "Since.ts", "AppSnapshot.ts", "AppCommand.ts"] {
        assert!(dir.join(f).exists(), "{f} missing");
    }
    let env = std::fs::read_to_string(dir.join("EventEnvelope.ts")).unwrap();
    assert!(env.contains("seq: number"), "u64 counters must be numbers on the wire: {env}");
}
