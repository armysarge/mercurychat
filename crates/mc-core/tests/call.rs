//! M2 Task 5: calls, sessions, disconnect, abort and recovery through the
//! service. The simulator's SIM1 answers and runs MCH/1; the FakeNet tests
//! drive a peer station of their own on side B.
#![cfg(feature = "sim")]

use mc_core::app::{
    spawn_app, AppCommand, AppError, AppEvent, AppHandle, AppOptions, AppSnapshot, CommandOk, EventEnvelope,
    ModemState, SessionDirection, SessionInfo,
};
use mc_core::profile::{ModemTarget, StationProfile};
use mc_core::station::{Station, StationConfig};
use mc_storage::{OutState, Store};
use mercury_adapter::fake::FakeNet;
use mercury_adapter::{Callsign, Endpoint};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast::Receiver;
use tokio::sync::mpsc;

fn opts(root: &Path) -> AppOptions {
    AppOptions {
        sim_root: Some(root.to_path_buf()),
        stations_root: Some(root.join("stations")),
        ..AppOptions::default()
    }
}

fn existing(base: u16) -> StationProfile {
    let mut p = StationProfile::new_existing("ZS6ABC");
    p.modem = ModemTarget::Existing {
        host: "127.0.0.1".into(),
        control_port: base,
        data_port: base + 1,
        broadcast_port: None,
        telemetry_url: None,
    };
    p
}

async fn snapshot(h: &AppHandle) -> AppSnapshot {
    match h.call(AppCommand::Snapshot).await.unwrap() {
        CommandOk::Snapshot(s) => s,
        other => panic!("{other:?}"),
    }
}

async fn next(rx: &mut Receiver<EventEnvelope>, secs: u64) -> AppEvent {
    tokio::time::timeout(Duration::from_secs(secs), rx.recv()).await.expect("an event in time").unwrap().event
}

/// Wait for a modem state, failing on Faulted.
async fn until(rx: &mut Receiver<EventEnvelope>, want: ModemState, secs: u64) -> Option<String> {
    loop {
        if let AppEvent::ModemState { state, detail } = next(rx, secs).await {
            if state == want {
                return detail;
            }
            assert_ne!(state, ModemState::Faulted, "faulted: {detail:?}");
        }
    }
}

/// Every modem state up to and including `last`, in order.
async fn states_until(rx: &mut Receiver<EventEnvelope>, last: ModemState, secs: u64) -> Vec<ModemState> {
    let mut seen = Vec::new();
    loop {
        if let AppEvent::ModemState { state, detail } = next(rx, secs).await {
            assert_ne!(state, ModemState::Faulted, "faulted: {detail:?}");
            seen.push(state);
            if state == last {
                return seen;
            }
        }
    }
}

/// Start an app on `profile` and wait until its modem is Ready.
async fn ready(root: &Path, profile: StationProfile) -> (AppHandle, Receiver<EventEnvelope>, String) {
    let h = spawn_app(Store::open_in_memory().unwrap(), opts(root));
    let id = match h.call(AppCommand::SaveProfile { profile }).await.unwrap() {
        CommandOk::Profile(p) => p.id,
        other => panic!("{other:?}"),
    };
    let mut rx = h.subscribe();
    h.call(AppCommand::SelectProfile { id: Some(id.clone()) }).await.unwrap();
    until(&mut rx, ModemState::Ready, 15).await;
    (h, rx, id)
}

fn station_db(root: &Path, id: &str) -> PathBuf {
    root.join("stations").join(id).join("station.db")
}

/// An outgoing message the modem has taken but not yet confirmed.
fn in_flight(root: &Path, id: &str, peer: &str) -> [u8; 16] {
    let mut store = Store::open(&station_db(root, id)).unwrap();
    let wire = mch_protocol::ObjectId::random().0;
    store.insert_outgoing(peer, &wire, "in flight").unwrap();
    store.set_out_state(peer, &wire, OutState::AwaitingReceipt, Some(1)).unwrap();
    wire
}

fn out_state(root: &Path, id: &str, peer: &str, wire: &[u8; 16]) -> Option<OutState> {
    Store::open(&station_db(root, id)).unwrap().out_state(peer, wire).unwrap()
}

async fn call(h: &AppHandle, peer: &str) -> Result<CommandOk, AppError> {
    h.call(AppCommand::Call { peer: peer.into() }).await
}

/// SIM1 on side B of `net`, run by the test.
async fn peer_station(net: &FakeNet, dir: &Path) -> Station {
    let call = Callsign::parse("SIM1").unwrap();
    let mut st = Station::open(StationConfig::defaults(call, Endpoint::base("127.0.0.1", net.b_base), dir)).unwrap();
    st.attach().await.unwrap();
    st
}

#[tokio::test(flavor = "multi_thread")]
async fn call_sim1_reaches_connected() {
    let dir = tempfile::tempdir().unwrap();
    let (h, mut rx, _) = ready(dir.path(), StationProfile::new_simulator("ZS6ABC")).await;
    assert_eq!(call(&h, "sim1").await.unwrap(), CommandOk::Done);
    let mut states = Vec::new();
    let mut session: Option<SessionInfo> = None;
    while !(states.last() == Some(&ModemState::Connected) && session.is_some()) {
        match next(&mut rx, 15).await {
            AppEvent::ModemState { state, detail } => {
                assert_ne!(state, ModemState::Faulted, "faulted: {detail:?}");
                states.push(state);
            }
            AppEvent::Session { session: s } => session = s,
            AppEvent::CallFailed { peer, reason } => panic!("call to {peer} failed: {reason}"),
            _ => {}
        }
    }
    assert_eq!(states, [ModemState::Calling, ModemState::Negotiating, ModemState::Connected]);
    let s = session.unwrap();
    assert_eq!(s.peer, "SIM1");
    assert_eq!(s.direction, SessionDirection::Outgoing);
    assert_eq!(s.peer_app.as_deref(), Some("MercuryChat"));
    let snap = snapshot(&h).await;
    assert_eq!(snap.modem, ModemState::Connected);
    assert_eq!(snap.session.as_ref().map(|s| s.peer.as_str()), Some("SIM1"));
}

#[tokio::test(flavor = "multi_thread")]
async fn second_call_is_conflict() {
    let dir = tempfile::tempdir().unwrap();
    let (h, mut rx, _) = ready(dir.path(), StationProfile::new_simulator("ZS6ABC")).await;
    call(&h, "SIM1").await.unwrap();
    match call(&h, "SIM1").await {
        Err(AppError::Conflict { message }) => assert_eq!(message, "a call or session is already in progress"),
        other => panic!("expected Conflict while calling, got {other:?}"),
    }
    until(&mut rx, ModemState::Connected, 15).await;
    match call(&h, "SIM1").await {
        Err(AppError::Conflict { message }) => assert_eq!(message, "a call or session is already in progress"),
        other => panic!("expected Conflict while connected, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn cancel_call_returns_to_ready() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    net.set_unanswered_calls(true); // nobody answers, and Mercury keeps trying
    let (h, mut rx, _) = ready(dir.path(), existing(net.a_base)).await;
    call(&h, "SIM1").await.unwrap();
    until(&mut rx, ModemState::Calling, 5).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(snapshot(&h).await.modem, ModemState::Calling, "still calling while nobody answers");
    assert_eq!(h.call(AppCommand::CancelCall).await.unwrap(), CommandOk::Done);
    assert_eq!(states_until(&mut rx, ModemState::Ready, 10).await, [ModemState::Ready]);
    assert!(net.log().iter().any(|l| l == "A> ABORT"), "the call attempt was stopped at the modem");
    assert_eq!(snapshot(&h).await.session, None);
}

#[tokio::test(flavor = "multi_thread")]
async fn disconnect_drains_then_ready() {
    let dir = tempfile::tempdir().unwrap();
    let (h, mut rx, _) = ready(dir.path(), StationProfile::new_simulator("ZS6ABC")).await;
    call(&h, "SIM1").await.unwrap();
    until(&mut rx, ModemState::Connected, 15).await;
    assert_eq!(h.call(AppCommand::Disconnect).await.unwrap(), CommandOk::Done);
    let mut ended = false;
    let mut states = Vec::new();
    while states.last() != Some(&ModemState::Ready) || !ended {
        match next(&mut rx, 15).await {
            AppEvent::ModemState { state, .. } => states.push(state),
            AppEvent::Session { session: None } => ended = true,
            _ => {}
        }
    }
    assert_eq!(states, [ModemState::Draining, ModemState::Ready]);
    assert_eq!(snapshot(&h).await.session, None);
}

#[tokio::test(flavor = "multi_thread")]
async fn abort_marks_in_flight_uncertain() {
    let dir = tempfile::tempdir().unwrap();
    let (h, mut rx, id) = ready(dir.path(), StationProfile::new_simulator("ZS6ABC")).await;
    call(&h, "SIM1").await.unwrap();
    until(&mut rx, ModemState::Connected, 15).await;
    let wire = in_flight(dir.path(), &id, "SIM1");
    assert_eq!(h.call(AppCommand::Abort).await.unwrap(), CommandOk::Done);
    let detail = until(&mut rx, ModemState::Ready, 30).await.unwrap_or_default();
    assert!(detail.contains("uncertain"), "{detail}");
    assert!(!detail.to_lowercase().contains("ptt"), "no label may claim the transmitter state: {detail}");
    assert_eq!(out_state(dir.path(), &id, "SIM1", &wire), Some(OutState::Uncertain));
}

#[tokio::test(flavor = "multi_thread")]
async fn control_loss_recovers_unarmed() {
    let dir = tempfile::tempdir().unwrap();
    let net: Arc<FakeNet> = FakeNet::start().await;
    let mut peer = peer_station(&net, &dir.path().join("peer")).await;
    let (h, mut rx, id) = ready(dir.path(), existing(net.a_base)).await;
    h.call(AppCommand::SetListen { on: true }).await.unwrap();
    until(&mut rx, ModemState::Listening, 5).await;

    // SIM1 calls us; we answer because listening is armed.
    let peer_task = tokio::spawn(async move {
        let info = peer.call(&Callsign::parse("ZS6ABC").unwrap(), Duration::from_secs(10)).await.unwrap();
        let (_cmd_tx, mut cmd_rx) = mpsc::channel(8);
        let (_req_tx, mut req_rx) = mpsc::channel(1);
        let (ev_tx, _ev_rx) = mpsc::unbounded_channel();
        peer.run_session(&info, &mut cmd_rx, &mut req_rx, ev_tx).await
    });
    let states = states_until(&mut rx, ModemState::Connected, 15).await;
    assert_eq!(states, [ModemState::Negotiating, ModemState::Connected]);
    let s = snapshot(&h).await.session.expect("a session");
    assert_eq!((s.peer.as_str(), s.direction), ("SIM1", SessionDirection::Incoming));

    let wire = in_flight(dir.path(), &id, "SIM1");
    let listen_ons = net.log().iter().filter(|l| *l == "A> LISTEN ON").count();
    net.kill_client(0); // Mercury closes our sockets mid-session
    until(&mut rx, ModemState::Recovering, 10).await;
    let detail = until(&mut rx, ModemState::Ready, 60).await;
    assert_eq!(detail.as_deref(), Some("listening is off until you turn it on"));
    assert_eq!(snapshot(&h).await.modem, ModemState::Ready, "recovery never re-arms listening");
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(net.log().iter().filter(|l| *l == "A> LISTEN ON").count(), listen_ons, "LISTEN ON after recovery");
    assert_eq!(out_state(dir.path(), &id, "SIM1", &wire), Some(OutState::Uncertain));
    assert_eq!(snapshot(&h).await.session, None);
    peer_task.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn invalid_peer_is_validation() {
    let dir = tempfile::tempdir().unwrap();
    let (h, _rx, _) = ready(dir.path(), StationProfile::new_simulator("ZS6ABC")).await;
    for bad in ["", "A B", "SIM1\rLISTEN ON", "ABCDEFGHIJKLMNOP"] {
        match call(&h, bad).await {
            Err(AppError::Validation { errors }) => assert_eq!(errors[0].field, "peer", "{bad:?}"),
            other => panic!("{bad:?}: expected Validation, got {other:?}"),
        }
    }
    assert_eq!(snapshot(&h).await.modem, ModemState::Ready);
}

#[tokio::test(flavor = "multi_thread")]
async fn non_mercurychat_peer_is_explained() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    // SIM1 accepts the link but runs no MCH/1 session: no HELLO ever comes.
    let mut peer = peer_station(&net, &dir.path().join("peer")).await;
    peer.set_listen(true).await.unwrap();
    let peer_task = tokio::spawn(async move {
        let _info = peer.wait_incoming(Duration::from_secs(30)).await;
        tokio::time::sleep(Duration::from_secs(60)).await;
    });
    let (h, mut rx, id) = ready(dir.path(), existing(net.a_base)).await;
    call(&h, "SIM1").await.unwrap();
    let (peer, reason) = loop {
        match next(&mut rx, 20).await {
            AppEvent::CallFailed { peer, reason } => break (peer, reason),
            AppEvent::Session { session: Some(s) } => panic!("a session was announced: {s:?}"),
            AppEvent::ModemState { state: ModemState::Connected, .. } => panic!("Connected without HELLO"),
            _ => {}
        }
    };
    assert_eq!(peer, "SIM1");
    assert_eq!(reason, "the station answered but is not running MercuryChat (no HELLO)");
    until(&mut rx, ModemState::Ready, 15).await;
    assert_eq!(snapshot(&h).await.session, None);
    assert!(Store::open(&station_db(dir.path(), &id)).unwrap().conversations().unwrap().is_empty());
    assert!(!net.session_up(), "the link was taken down");
    peer_task.abort();
}
