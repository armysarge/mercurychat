//! M3a Task 5: manual CQ and the idle-only broadcast gate (DSC-02, 03 §6),
//! through the service against the FakeNet modem pair. Side A is the app's
//! station; side B is the peer.
#![cfg(feature = "sim")]

use mc_core::app::{
    broadcast_gate, spawn_app, AppCommand, AppError, AppEvent, AppHandle, AppOptions, BroadcastKind, BroadcastOutcome,
    CommandOk, EventEnvelope, GateRefusal, HeardSource, ModemState,
};
use mc_core::profile::{ModemTarget, StationProfile};
use mc_storage::Store;
use mercury_adapter::fake::FakeNet;
use std::path::Path;
use std::time::Duration;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::broadcast::Receiver;
use tokio::time::Instant;

const ME: &str = "ZS6ABC";
const PEER: &str = "ZS6PEER";

const ALL_STATES: [ModemState; 10] = [
    ModemState::Unavailable,
    ModemState::Attaching,
    ModemState::Ready,
    ModemState::Listening,
    ModemState::Calling,
    ModemState::Negotiating,
    ModemState::Connected,
    ModemState::Draining,
    ModemState::Recovering,
    ModemState::Faulted,
];

// ---- the gate itself -------------------------------------------------------

#[test]
fn gate_allows_cq_only_when_idle() {
    for state in ALL_STATES {
        let got = broadcast_gate(state, false, None, Some(1));
        match state {
            ModemState::Ready | ModemState::Listening => assert_eq!(got, Ok(()), "{state:?}"),
            ModemState::Calling => assert_eq!(got, Err(GateRefusal::Calling), "{state:?}"),
            other => assert_eq!(got, Err(GateRefusal::NotIdle(other)), "{state:?}"),
        }
    }
}

#[test]
fn gate_allows_beacon_only_when_idle() {
    for state in ALL_STATES {
        let got = broadcast_gate(state, false, Some(40), Some(1));
        match state {
            ModemState::Ready | ModemState::Listening => assert_eq!(got, Ok(()), "{state:?}"),
            ModemState::Calling => assert_eq!(got, Err(GateRefusal::Calling), "{state:?}"),
            other => assert_eq!(got, Err(GateRefusal::NotIdle(other)), "{state:?}"),
        }
    }
}

#[test]
fn gate_counts_a_requested_call_as_calling() {
    for state in [ModemState::Ready, ModemState::Listening] {
        assert_eq!(broadcast_gate(state, true, None, Some(1)), Err(GateRefusal::Calling));
        assert_eq!(broadcast_gate(state, true, Some(10), Some(1)), Err(GateRefusal::Calling));
    }
    assert_eq!(GateRefusal::Calling.message(), "not sent: a call is being placed");
}

#[test]
fn gate_checks_beacon_capacity_and_listen_mode() {
    let idle = ModemState::Ready;
    assert_eq!(broadcast_gate(idle, false, Some(123), Some(1)), Ok(()));
    let over = broadcast_gate(idle, false, Some(124), Some(1));
    assert_eq!(over, Err(GateRefusal::OverCapacity { len: 124, capacity: 123 }));
    assert_eq!(over.unwrap_err().message(), "not sent: 124 bytes is over the 123-byte limit");
    assert_eq!(broadcast_gate(idle, false, Some(10), None), Err(GateRefusal::UnknownListenMode));
    // A mode outside the pinned table is as unknown as none.
    assert_eq!(broadcast_gate(idle, false, Some(10), Some(8)), Err(GateRefusal::UnknownListenMode));
    assert_eq!(GateRefusal::UnknownListenMode.message(), "not sent: listen mode unknown");
    // CQ needs no listen mode.
    assert_eq!(broadcast_gate(idle, false, None, None), Ok(()));
}

#[test]
fn every_refusal_has_operator_text() {
    let mut all: Vec<GateRefusal> = ALL_STATES.iter().map(|s| GateRefusal::NotIdle(*s)).collect();
    all.extend([
        GateRefusal::Calling,
        GateRefusal::UnknownListenMode,
        GateRefusal::OverCapacity { len: 200, capacity: 123 },
        GateRefusal::NoBroadcastPort,
        GateRefusal::Detached,
    ]);
    for g in all {
        let m = g.message();
        assert!(m.starts_with("not sent: "), "{g:?}: {m}");
        assert!(m.len() > "not sent: ".len() + 5, "{g:?}: {m}");
    }
    for s in [ModemState::Negotiating, ModemState::Connected, ModemState::Draining] {
        assert_eq!(GateRefusal::NotIdle(s).message(), "not sent: the modem is busy with a session");
    }
}

// ---- through the service ---------------------------------------------------

fn opts(root: &Path) -> AppOptions {
    AppOptions {
        sim_root: Some(root.to_path_buf()),
        stations_root: Some(root.join("stations")),
        ..AppOptions::default()
    }
}

/// A station on side `side` (0: A, 1: B) of `net`, with its broadcast port.
fn existing(net: &FakeNet, call: &str, side: usize) -> StationProfile {
    let mut p = StationProfile::new_existing(call);
    let (base, bcast) = if side == 0 { (net.a_base, net.a_bcast) } else { (net.b_base, net.b_bcast) };
    p.modem = ModemTarget::Existing {
        host: "127.0.0.1".into(),
        control_port: base,
        data_port: base + 1,
        broadcast_port: Some(bcast),
        telemetry_url: None,
    };
    p
}

/// The next event that `want` accepts, within `secs`.
async fn wait_for<T>(rx: &mut Receiver<EventEnvelope>, secs: u64, mut want: impl FnMut(&AppEvent) -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        let ev = match tokio::time::timeout_at(deadline, rx.recv()).await {
            Err(_) => panic!("no matching event within {secs} s"),
            Ok(Err(RecvError::Lagged(_))) => continue,
            Ok(Err(RecvError::Closed)) => panic!("the event stream closed"),
            Ok(Ok(env)) => env.event,
        };
        if let AppEvent::ModemState { state: ModemState::Faulted, detail } = &ev {
            panic!("faulted: {detail:?}");
        }
        if let Some(t) = want(&ev) {
            return t;
        }
    }
}

async fn until_state(rx: &mut Receiver<EventEnvelope>, want: ModemState) {
    wait_for(rx, 15, |e| matches!(e, AppEvent::ModemState { state, .. } if *state == want).then_some(())).await
}

/// The outcomes of one broadcast of `kind`, up to its final one (anything
/// but Submitted), and every event seen meanwhile.
async fn outcomes(rx: &mut Receiver<EventEnvelope>, kind: BroadcastKind) -> (Vec<BroadcastOutcome>, Vec<AppEvent>) {
    let mut got = Vec::new();
    let mut seen = Vec::new();
    loop {
        let o = wait_for(rx, 20, |e| {
            seen.push(e.clone());
            match e {
                AppEvent::Broadcast { kind: k, outcome } if *k == kind => Some(outcome.clone()),
                _ => None,
            }
        })
        .await;
        let last = o != BroadcastOutcome::Submitted;
        got.push(o);
        if last {
            return (got, seen);
        }
    }
}

async fn ready(root: &Path, profile: StationProfile) -> (AppHandle, Receiver<EventEnvelope>) {
    let h = spawn_app(Store::open_in_memory().unwrap(), opts(root));
    let id = match h.call(AppCommand::SaveProfile { profile }).await.unwrap() {
        CommandOk::Profile(p) => p.id,
        other => panic!("{other:?}"),
    };
    let mut rx = h.subscribe();
    h.call(AppCommand::SelectProfile { id: Some(id) }).await.unwrap();
    until_state(&mut rx, ModemState::Listening).await;
    (h, rx)
}

fn conflict(res: Result<CommandOk, AppError>) -> String {
    match res {
        Err(AppError::Conflict { message }) => message,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

async fn broadcast_lines(h: &AppHandle) -> Vec<String> {
    match h.call(AppCommand::Activity).await.unwrap() {
        CommandOk::Activity { entries } => {
            entries.into_iter().filter(|e| e.kind == "broadcast").map(|e| e.text).collect()
        }
        other => panic!("{other:?}"),
    }
}

fn cq_lines(net: &FakeNet) -> Vec<String> {
    net.log().into_iter().filter(|l| l.contains("CQFRAME")).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn cq_is_transmitted_and_heard_by_peer() {
    let net = FakeNet::start().await;
    let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (ha, mut rxa) = ready(da.path(), existing(&net, ME, 0)).await;
    let (_hb, mut rxb) = ready(db.path(), existing(&net, PEER, 1)).await;
    assert_eq!(ha.call(AppCommand::SendCq).await.unwrap(), CommandOk::Done);
    let (got, _) = outcomes(&mut rxa, BroadcastKind::Cq).await;
    assert_eq!(got, [BroadcastOutcome::Submitted, BroadcastOutcome::Transmitted]);
    let sent = cq_lines(&net);
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert!(sent[0].starts_with(&format!("A> CQFRAME {ME} ")), "{sent:?}");
    let st = wait_for(&mut rxb, 10, |e| match e {
        AppEvent::Heard { station } if station.callsign == ME => Some(station.clone()),
        _ => None,
    })
    .await;
    assert_eq!(st.source, Some(HeardSource::Cq));
    let lines = broadcast_lines(&ha).await;
    assert!(lines.iter().any(|l| l.contains("CQ") && l.contains("transmitted")), "{lines:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn cq_refused_while_calling_and_not_sent_later() {
    let net = FakeNet::start().await;
    net.set_unanswered_calls(true); // the call stays Calling until cancelled
    let dir = tempfile::tempdir().unwrap();
    let (h, mut rx) = ready(dir.path(), existing(&net, ME, 0)).await;
    assert_eq!(h.call(AppCommand::Call { peer: PEER.into() }).await.unwrap(), CommandOk::Done);
    let why = conflict(h.call(AppCommand::SendCq).await);
    assert_eq!(why, "not sent: a call is being placed");
    let (got, _) = outcomes(&mut rx, BroadcastKind::Cq).await;
    assert_eq!(got, [BroadcastOutcome::Refused { reason: why.clone() }]);
    // The call ends; the refused CQ must not go out afterwards.
    assert_eq!(h.call(AppCommand::CancelCall).await.unwrap(), CommandOk::Done);
    until_state(&mut rx, ModemState::Listening).await;
    // The modem takes commands in order: once a LISTEN OFF sent now is through,
    // anything queued before it would have been sent.
    let offs = net.log().iter().filter(|l| *l == "A> LISTEN OFF").count();
    h.call(AppCommand::SetReceiveOnly { on: true }).await.unwrap();
    until_state(&mut rx, ModemState::Ready).await;
    assert!(net.log().iter().filter(|l| *l == "A> LISTEN OFF").count() > offs, "{:?}", net.log());
    assert!(net.log().iter().any(|l| l == "A> LISTEN ON"), "{:?}", net.log());
    assert_eq!(cq_lines(&net), Vec::<String>::new());
    let lines = broadcast_lines(&h).await;
    assert!(lines.iter().any(|l| l.contains(&why)), "{lines:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn beacon_payload_over_123_bytes_is_refused() {
    let net = FakeNet::start().await;
    let dir = tempfile::tempdir().unwrap();
    let (h, mut rx) = ready(dir.path(), existing(&net, ME, 0)).await;
    let why = conflict(h.send_beacon_payload(vec![0x42; 124]).await);
    assert_eq!(why, "not sent: 124 bytes is over the 123-byte limit");
    let (got, _) = outcomes(&mut rx, BroadcastKind::Beacon).await;
    assert_eq!(got, [BroadcastOutcome::Refused { reason: why.clone() }]);
    // Exactly the capacity goes out whole.
    assert_eq!(h.send_beacon_payload(vec![0x42; 123]).await.unwrap(), CommandOk::Done);
    let (got, _) = outcomes(&mut rx, BroadcastKind::Beacon).await;
    assert_eq!(got, [BroadcastOutcome::Submitted, BroadcastOutcome::Transmitted]);
    assert_eq!(net.broadcast_log(), [(0, vec![0x42; 123])]);
    let lines = broadcast_lines(&h).await;
    assert!(lines.iter().any(|l| l.contains(&why)), "{lines:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn unknown_listen_mode_refuses_beacon_but_allows_cq() {
    let net = FakeNet::start().await;
    net.set_mode_wrong(0, true); // MODE 1 answered WRONG at attach
    let dir = tempfile::tempdir().unwrap();
    let (h, mut rx) = ready(dir.path(), existing(&net, ME, 0)).await;
    let why = conflict(h.send_beacon_payload(vec![0x42; 20]).await);
    assert_eq!(why, "not sent: listen mode unknown");
    let (got, _) = outcomes(&mut rx, BroadcastKind::Beacon).await;
    assert_eq!(got, [BroadcastOutcome::Refused { reason: why }]);
    assert_eq!(h.call(AppCommand::SendCq).await.unwrap(), CommandOk::Done);
    let (got, _) = outcomes(&mut rx, BroadcastKind::Cq).await;
    assert_eq!(got, [BroadcastOutcome::Submitted, BroadcastOutcome::Transmitted]);
    assert!(net.broadcast_log().is_empty(), "{:?}", net.broadcast_log());
}

#[tokio::test(flavor = "multi_thread")]
async fn cq_label_is_transmitted_never_delivered() {
    let net = FakeNet::start().await;
    let dir = tempfile::tempdir().unwrap();
    let (h, mut rx) = ready(dir.path(), existing(&net, ME, 0)).await;
    h.call(AppCommand::SendCq).await.unwrap();
    let (got, seen) = outcomes(&mut rx, BroadcastKind::Cq).await;
    assert_eq!(got.last(), Some(&BroadcastOutcome::Transmitted));
    for ev in &seen {
        let json = serde_json::to_string(ev).unwrap().to_lowercase();
        assert!(!json.contains("deliver"), "a CQ is never delivered: {json}");
    }
    let lines = broadcast_lines(&h).await;
    assert!(!lines.is_empty());
    assert!(lines.iter().all(|l| !l.to_lowercase().contains("deliver")), "{lines:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn second_cq_while_one_is_being_sent_is_refused_not_queued() {
    let net = FakeNet::start().await;
    let dir = tempfile::tempdir().unwrap();
    let (h, mut rx) = ready(dir.path(), existing(&net, ME, 0)).await;
    h.call(AppCommand::SendCq).await.unwrap();
    let why = conflict(h.call(AppCommand::SendCq).await);
    assert_eq!(why, "not sent: a broadcast is still being sent");
    // The refusal and the first CQ's outcomes, in whatever interleaving.
    let mut got = Vec::new();
    while got.last() != Some(&BroadcastOutcome::Transmitted) {
        got.extend(outcomes(&mut rx, BroadcastKind::Cq).await.0);
    }
    let refused = BroadcastOutcome::Refused { reason: why };
    assert_eq!(got.iter().filter(|o| **o == refused).count(), 1, "{got:?}");
    got.retain(|o| *o != refused);
    assert_eq!(got, [BroadcastOutcome::Submitted, BroadcastOutcome::Transmitted]);
    // A later one goes once the first is done.
    h.call(AppCommand::SendCq).await.unwrap();
    let (got, _) = outcomes(&mut rx, BroadcastKind::Cq).await;
    assert_eq!(got.last(), Some(&BroadcastOutcome::Transmitted));
    assert_eq!(cq_lines(&net).len(), 2, "{:?}", cq_lines(&net));
}
