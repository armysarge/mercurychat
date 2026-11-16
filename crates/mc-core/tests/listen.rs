//! D18 (owner decision, 16 Nov 2026): a station answers calls by default. After
//! attach and after every successful recovery the modem goes to Listening
//! unless the profile is Receive only. This reverses the M2 rule (RF-02) for
//! listening only; beacons keep it. Checked against a fake Mercury whose
//! command log shows exactly what reached the modem.

use mc_core::app::{
    spawn_app, AppCommand, AppError, AppEvent, AppHandle, AppOptions, CommandOk, EventEnvelope, ModemState,
};
use mc_core::profile::{ModemTarget, StationProfile};
use mc_core::station::{Station, StationConfig};
use mc_storage::Store;
use mercury_adapter::fake::FakeNet;
use mercury_adapter::{Callsign, Endpoint};
use std::path::Path;
use std::time::Duration;
use tokio::sync::broadcast::Receiver;
use tokio::sync::mpsc;

fn opts(root: &Path) -> AppOptions {
    AppOptions { stations_root: Some(root.join("stations")), ..AppOptions::default() }
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

async fn state(rx: &mut Receiver<EventEnvelope>) -> (ModemState, Option<String>) {
    loop {
        let ev = tokio::time::timeout(Duration::from_secs(60), rx.recv()).await.expect("a modem state").unwrap();
        if let AppEvent::ModemState { state, detail } = ev.event {
            return (state, detail);
        }
    }
}

async fn until(rx: &mut Receiver<EventEnvelope>, want: ModemState) -> Option<String> {
    loop {
        let (s, d) = state(rx).await;
        if s == want {
            return d;
        }
        assert_ne!(s, ModemState::Faulted, "faulted: {d:?}");
    }
}

async fn snapshot(h: &AppHandle) -> mc_core::app::AppSnapshot {
    match h.call(AppCommand::Snapshot).await.unwrap() {
        CommandOk::Snapshot(s) => s,
        other => panic!("{other:?}"),
    }
}

fn count(net: &FakeNet, line: &str) -> usize {
    net.log().iter().filter(|l| l.as_str() == line).count()
}

fn listen_ons(net: &FakeNet) -> usize {
    count(net, "A> LISTEN ON")
}

fn listen_offs(net: &FakeNet) -> usize {
    count(net, "A> LISTEN OFF")
}

fn receive_only(mut p: StationProfile) -> StationProfile {
    p.receive_only = true;
    p
}

async fn select(h: &AppHandle, profile: StationProfile) -> Receiver<EventEnvelope> {
    let id = match h.call(AppCommand::SaveProfile { profile }).await.unwrap() {
        CommandOk::Profile(p) => p.id,
        other => panic!("{other:?}"),
    };
    let rx = h.subscribe();
    h.call(AppCommand::SelectProfile { id: Some(id) }).await.unwrap();
    rx
}

#[tokio::test(flavor = "multi_thread")]
async fn attach_listens_by_default() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let h = spawn_app(Store::open_in_memory().unwrap(), opts(dir.path()));
    let mut rx = select(&h, existing(net.a_base)).await;
    until(&mut rx, ModemState::Listening).await;
    let s = snapshot(&h).await;
    assert!(s.listening && !s.receive_only);
    assert_eq!(s.modem, ModemState::Listening);
    assert_eq!(listen_ons(&net), 1);
    assert!(net.listening(0));
}

#[tokio::test(flavor = "multi_thread")]
async fn recovery_resumes_listening() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let h = spawn_app(Store::open_in_memory().unwrap(), opts(dir.path()));
    let mut rx = select(&h, existing(net.a_base)).await;
    until(&mut rx, ModemState::Listening).await;
    assert_eq!(listen_ons(&net), 1);

    net.kill_client(0); // Mercury closes our sockets
    until(&mut rx, ModemState::Recovering).await;
    assert!(!snapshot(&h).await.listening);
    until(&mut rx, ModemState::Listening).await;
    assert!(snapshot(&h).await.listening);
    assert_eq!(listen_ons(&net), 2, "recovery sent LISTEN ON once");
    assert!(net.listening(0));
}

#[tokio::test(flavor = "multi_thread")]
async fn receive_only_profile_never_sends_listen_on() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let h = spawn_app(Store::open_in_memory().unwrap(), opts(dir.path()));
    let mut rx = select(&h, receive_only(existing(net.a_base))).await;
    let detail = until(&mut rx, ModemState::Ready).await;
    assert_eq!(detail, None);
    let s = snapshot(&h).await;
    assert!(s.receive_only && !s.listening);

    // Nor after a recovery.
    net.kill_client(0);
    until(&mut rx, ModemState::Recovering).await;
    until(&mut rx, ModemState::Ready).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(listen_ons(&net), 0, "a Receive only station sent LISTEN ON");
    assert!(!net.listening(0));
    assert_eq!(snapshot(&h).await.modem, ModemState::Ready);
}

/// SIM1 on side B of `net`, run by the test.
async fn peer_station(net: &FakeNet, dir: &Path) -> Station {
    let call = Callsign::parse("SIM1").unwrap();
    let mut st = Station::open(StationConfig::defaults(call, Endpoint::base("127.0.0.1", net.b_base), dir)).unwrap();
    st.attach().await.unwrap();
    st
}

#[tokio::test(flavor = "multi_thread")]
async fn receive_only_mid_session_sends_listen_off_and_keeps_the_session() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let mut peer = peer_station(&net, &dir.path().join("peer")).await;
    let h = spawn_app(Store::open_in_memory().unwrap(), opts(dir.path()));
    let mut rx = select(&h, existing(net.a_base)).await;
    until(&mut rx, ModemState::Listening).await;

    let peer_task = tokio::spawn(async move {
        let info = peer.call(&Callsign::parse("ZS6ABC").unwrap(), Duration::from_secs(10)).await.unwrap();
        let (_cmd_tx, mut cmd_rx) = mpsc::channel(8);
        let (_req_tx, mut req_rx) = mpsc::channel(1);
        let (ev_tx, _ev_rx) = mpsc::unbounded_channel();
        peer.run_session(&info, &mut cmd_rx, &mut req_rx, ev_tx).await
    });
    until(&mut rx, ModemState::Connected).await;
    assert!(snapshot(&h).await.session.is_some());
    let offs = listen_offs(&net);

    assert_eq!(h.call(AppCommand::SetReceiveOnly { on: true }).await.unwrap(), CommandOk::Done);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while listen_offs(&net) == offs {
        assert!(tokio::time::Instant::now() < deadline, "LISTEN OFF never reached the modem");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(!net.listening(0));
    tokio::time::sleep(Duration::from_millis(300)).await;
    let s = snapshot(&h).await;
    assert!(s.session.is_some(), "the session continues");
    assert_eq!(s.modem, ModemState::Connected);
    assert!(s.receive_only);
    assert!(net.session_up());

    // After the session the modem is Ready, not Listening.
    h.call(AppCommand::Disconnect).await.unwrap();
    until(&mut rx, ModemState::Ready).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let s = snapshot(&h).await;
    assert_eq!((s.modem, s.listening, s.session), (ModemState::Ready, false, None));
    peer_task.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn listen_on_failure_stays_ready_with_reason_and_no_retry_loop() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    net.set_listen_wrong(0, true);
    let h = spawn_app(Store::open_in_memory().unwrap(), opts(dir.path()));
    let mut rx = select(&h, existing(net.a_base)).await;
    // Ready is reported first, then Ready again with the reason.
    let reason = loop {
        if let Some(d) = until(&mut rx, ModemState::Ready).await {
            break d;
        }
    };
    assert!(reason.starts_with("not answering calls: "), "{reason}");
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(listen_ons(&net), 1, "one attempt only");
    let s = snapshot(&h).await;
    assert_eq!((s.modem, s.listening, s.receive_only), (ModemState::Ready, false, false));
    assert_eq!(s.modem_detail.as_deref(), Some(reason.as_str()));

    // The next successful recovery tries again, once.
    net.set_listen_wrong(0, false);
    net.kill_client(0);
    until(&mut rx, ModemState::Recovering).await;
    until(&mut rx, ModemState::Listening).await;
    assert_eq!(listen_ons(&net), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn beacons_still_stay_off_after_recovery() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let h = spawn_app(Store::open_in_memory().unwrap(), opts(dir.path()));
    let mut p = existing(net.a_base);
    if let ModemTarget::Existing { broadcast_port, .. } = &mut p.modem {
        *broadcast_port = Some(net.a_bcast);
    }
    let mut rx = select(&h, p).await;
    until(&mut rx, ModemState::Listening).await;
    assert_eq!(h.call(AppCommand::SetBeacon { on: true }).await.unwrap(), CommandOk::Done);
    assert!(snapshot(&h).await.beacon.active);

    net.kill_client(0);
    until(&mut rx, ModemState::Recovering).await;
    until(&mut rx, ModemState::Listening).await;
    let s = snapshot(&h).await;
    assert!(s.listening, "listening resumes");
    assert!(!s.beacon.active, "but beacons do not (RF-02)");
    assert!(s.beacon.preferred, "the preference is kept");
    assert!(net.broadcast_log().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn receive_only_switch_stops_and_resumes_answering_and_is_stored() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let h = spawn_app(Store::open_in_memory().unwrap(), opts(dir.path()));
    let mut rx = select(&h, existing(net.a_base)).await;
    until(&mut rx, ModemState::Listening).await;
    assert!(!snapshot(&h).await.receive_only);

    assert_eq!(h.call(AppCommand::SetReceiveOnly { on: true }).await.unwrap(), CommandOk::Done);
    until(&mut rx, ModemState::Ready).await;
    let s = snapshot(&h).await;
    assert!(s.receive_only && !s.listening);
    assert!(s.profiles[0].receive_only, "stored on the profile");
    assert!(!net.listening(0));

    h.call(AppCommand::SetReceiveOnly { on: false }).await.unwrap();
    until(&mut rx, ModemState::Listening).await;
    let s = snapshot(&h).await;
    assert!(!s.receive_only && s.listening);
    assert!(net.listening(0));
}

#[tokio::test(flavor = "multi_thread")]
async fn receive_only_survives_restart_and_turning_it_off_listens_again() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("mercurychat.db");
    let net = FakeNet::start().await;
    {
        let h = spawn_app(Store::open(&db).unwrap(), opts(dir.path()));
        let mut rx = select(&h, existing(net.a_base)).await;
        until(&mut rx, ModemState::Listening).await;
        h.call(AppCommand::SetReceiveOnly { on: true }).await.unwrap();
        until(&mut rx, ModemState::Ready).await;
    } // the app stops
    let before = listen_ons(&net);

    let h = spawn_app(Store::open(&db).unwrap(), opts(dir.path()));
    let mut rx = h.subscribe();
    until(&mut rx, ModemState::Ready).await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    let s = snapshot(&h).await;
    assert!(s.receive_only && !s.listening);
    assert_eq!(listen_ons(&net), before, "a Receive only station sent LISTEN ON on restart");

    h.call(AppCommand::SetReceiveOnly { on: false }).await.unwrap();
    until(&mut rx, ModemState::Listening).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn set_receive_only_while_unavailable_is_conflict() {
    let h = spawn_app(Store::open_in_memory().unwrap(), AppOptions::default());
    match h.call(AppCommand::SetReceiveOnly { on: true }).await {
        Err(AppError::Conflict { message }) => assert!(message.contains("unavailable"), "{message}"),
        other => panic!("expected Conflict, got {other:?}"),
    }
}
