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

/// A session with SIM1 calling us, up and Connected. The peer task is the
/// test's to abort.
async fn connected(
    net: &FakeNet,
    dir: &Path,
    h: &AppHandle,
    rx: &mut Receiver<EventEnvelope>,
) -> tokio::task::JoinHandle<mc_core::session::SessionEnd> {
    let mut peer = peer_station(net, &dir.join("peer")).await;
    let peer_task = tokio::spawn(async move {
        let info = peer.call(&Callsign::parse("ZS6ABC").unwrap(), Duration::from_secs(10)).await.unwrap();
        let (_cmd_tx, mut cmd_rx) = mpsc::channel(8);
        let (_req_tx, mut req_rx) = mpsc::channel(1);
        let (ev_tx, _ev_rx) = mpsc::unbounded_channel();
        peer.run_session(&info, &mut cmd_rx, &mut req_rx, ev_tx).await
    });
    until(rx, ModemState::Connected).await;
    // The session is reported just after the Connected state.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while snapshot(h).await.session.is_none() {
        assert!(tokio::time::Instant::now() < deadline, "no session after Connected");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    peer_task
}

/// The next ReceiveOnly event, within 10 s.
async fn receive_only_event(rx: &mut Receiver<EventEnvelope>) -> (bool, bool) {
    loop {
        let ev = tokio::time::timeout(Duration::from_secs(10), rx.recv()).await.expect("a ReceiveOnly event").unwrap();
        if let AppEvent::ReceiveOnly { on, pending } = ev.event {
            return (on, pending);
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn receive_only_mid_session_waits_for_the_session_to_end() {
    // Lab F2: Mercury takes LISTEN OFF on a live link by dropping it without a
    // DISCONNECT, stranding the peer. So the live link is never touched.
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let h = spawn_app(Store::open_in_memory().unwrap(), opts(dir.path()));
    let mut rx = select(&h, existing(net.a_base)).await;
    until(&mut rx, ModemState::Listening).await;
    let peer_task = connected(&net, dir.path(), &h, &mut rx).await;
    let offs = listen_offs(&net);

    assert_eq!(h.call(AppCommand::SetReceiveOnly { on: true }).await.unwrap(), CommandOk::Done);
    assert_eq!(receive_only_event(&mut rx).await, (true, true), "stored at once, pending");
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(listen_offs(&net), offs, "LISTEN OFF reached the modem mid-session");
    assert!(net.listening(0));
    let s = snapshot(&h).await;
    assert!(s.session.is_some(), "the session continues");
    assert_eq!(s.modem, ModemState::Connected);
    assert!(s.receive_only, "the preference is stored at once");
    assert!(s.receive_only_pending, "the snapshot shows it waiting for the session");
    assert!(net.session_up());

    // The session ends: exactly one LISTEN OFF, and nothing pending.
    h.call(AppCommand::Disconnect).await.unwrap();
    until(&mut rx, ModemState::Ready).await;
    assert_eq!(receive_only_event(&mut rx).await, (true, false));
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(listen_offs(&net), offs + 1, "one LISTEN OFF after the session");
    assert!(!net.listening(0));
    let s = snapshot(&h).await;
    assert_eq!((s.modem, s.listening, s.session), (ModemState::Ready, false, None));
    assert!(s.receive_only && !s.receive_only_pending);
    peer_task.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn receive_only_while_draining_waits_for_the_link_to_end() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let h = spawn_app(Store::open_in_memory().unwrap(), opts(dir.path()));
    let mut rx = select(&h, existing(net.a_base)).await;
    until(&mut rx, ModemState::Listening).await;
    let peer_task = connected(&net, dir.path(), &h, &mut rx).await;
    let offs = listen_offs(&net);

    // Mercury is slow to end the link: the modem stays Draining.
    net.set_hold_disconnect(true);
    h.call(AppCommand::Disconnect).await.unwrap();
    until(&mut rx, ModemState::Draining).await;
    h.call(AppCommand::SetReceiveOnly { on: true }).await.unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(listen_offs(&net), offs, "LISTEN OFF reached the modem while draining");
    assert!(snapshot(&h).await.receive_only_pending);

    net.set_hold_disconnect(false);
    net.drop_link();
    until(&mut rx, ModemState::Ready).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(listen_offs(&net), offs + 1, "one LISTEN OFF after the link");
    assert!(!net.listening(0));
    assert!(!snapshot(&h).await.receive_only_pending);
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

/// Have `peer` call us and then say nothing (no HELLO), held by the test.
fn silent_call(mut peer: Station) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let _ = peer.call(&Callsign::parse("ZS6ABC").unwrap(), Duration::from_secs(10)).await;
        tokio::time::sleep(Duration::from_secs(60)).await;
    })
}

async fn saved_profile(h: &AppHandle) -> StationProfile {
    snapshot(h).await.profiles.into_iter().next().unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn receive_only_set_mid_session_survives_an_abrupt_link_end() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let peer = peer_station(&net, &dir.path().join("peer")).await;
    let o = AppOptions { radio_hello_timeout: Some(Duration::from_secs(1)), ..opts(dir.path()) };
    let h = spawn_app(Store::open_in_memory().unwrap(), o);
    let mut rx = select(&h, existing(net.a_base)).await;
    until(&mut rx, ModemState::Listening).await;
    // The peer calls (it never sends HELLO): the link ends by HelloTimeout.
    let _peer = silent_call(peer);
    until(&mut rx, ModemState::Negotiating).await;
    // The HELLO wait ends and the modem is asked to DISCONNECT; Mercury is slow
    // to do it, so the modem waits for the link to end (the cut-short wait).
    net.set_hold_disconnect(true);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while count(&net, "A> DISCONNECT") == 0 {
        assert!(tokio::time::Instant::now() < deadline, "no DISCONNECT after the HELLO timeout");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(h.call(AppCommand::SetReceiveOnly { on: true }).await.unwrap(), CommandOk::Done);
    tokio::time::sleep(Duration::from_millis(300)).await;
    net.set_hold_disconnect(false);
    net.drop_link(); // the link ends now
    until(&mut rx, ModemState::Ready).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let s = snapshot(&h).await;
    assert_eq!((s.modem, s.listening, s.receive_only), (ModemState::Ready, false, true));
    assert!(!net.listening(0), "Mercury must not be answering calls");

    let ons = listen_ons(&net);
    net.kill_client(0);
    until(&mut rx, ModemState::Recovering).await;
    until(&mut rx, ModemState::Ready).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(listen_ons(&net), ons, "recovery sent LISTEN ON for a Receive only station");
    assert!(!net.listening(0));
}

#[tokio::test(flavor = "multi_thread")]
async fn receive_only_off_mid_session_listens_at_once_and_nothing_is_left_pending() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let h = spawn_app(Store::open_in_memory().unwrap(), opts(dir.path()));
    let mut rx = select(&h, existing(net.a_base)).await;
    until(&mut rx, ModemState::Listening).await;
    let peer_task = connected(&net, dir.path(), &h, &mut rx).await;
    let (offs, ons) = (listen_offs(&net), listen_ons(&net));

    // On (waits for the session), then off again: LISTEN ON is harmless
    // mid-link and goes at once; the deferred LISTEN OFF is forgotten.
    h.call(AppCommand::SetReceiveOnly { on: true }).await.unwrap();
    assert!(snapshot(&h).await.receive_only_pending);
    h.call(AppCommand::SetReceiveOnly { on: false }).await.unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while listen_ons(&net) == ons {
        assert!(tokio::time::Instant::now() < deadline, "LISTEN ON never reached the modem mid-session");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let s = snapshot(&h).await;
    assert_eq!((s.modem, s.receive_only, s.receive_only_pending), (ModemState::Connected, false, false));
    assert!(net.session_up());

    h.call(AppCommand::Disconnect).await.unwrap();
    until(&mut rx, ModemState::Listening).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(listen_offs(&net), offs, "no LISTEN OFF for a choice taken back");
    assert!(net.listening(0));
    peer_task.abort();
}

#[tokio::test(flavor = "multi_thread")]
async fn saving_the_active_profile_as_receive_only_applies_it() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let h = spawn_app(Store::open_in_memory().unwrap(), opts(dir.path()));
    let mut rx = select(&h, existing(net.a_base)).await;
    until(&mut rx, ModemState::Listening).await;

    let p = receive_only(saved_profile(&h).await);
    h.call(AppCommand::SaveProfile { profile: p }).await.unwrap();
    until(&mut rx, ModemState::Ready).await;
    let s = snapshot(&h).await;
    assert!(s.receive_only && !s.listening);
    assert!(!net.listening(0), "Mercury still answers after the profile was saved as Receive only");

    let ons = listen_ons(&net);
    net.kill_client(0);
    until(&mut rx, ModemState::Recovering).await;
    until(&mut rx, ModemState::Ready).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(listen_ons(&net), ons, "recovery re-armed a Receive only profile");
}

#[tokio::test(flavor = "multi_thread")]
async fn receive_only_during_recovery_prevents_listen_on_after_it() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let h = spawn_app(Store::open_in_memory().unwrap(), opts(dir.path()));
    let mut rx = select(&h, existing(net.a_base)).await;
    until(&mut rx, ModemState::Listening).await;
    let ons = listen_ons(&net);

    net.kill_client(0);
    until(&mut rx, ModemState::Recovering).await;
    assert_eq!(h.call(AppCommand::SetReceiveOnly { on: true }).await.unwrap(), CommandOk::Done);
    until(&mut rx, ModemState::Ready).await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    let s = snapshot(&h).await;
    assert_eq!((s.modem, s.listening, s.receive_only), (ModemState::Ready, false, true));
    assert_eq!(listen_ons(&net), ons, "LISTEN ON after a recovery the operator declined");
    assert!(!net.listening(0));
}

#[tokio::test(flavor = "multi_thread")]
async fn receive_only_while_calling_sends_listen_off_when_the_call_ends() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    net.set_unanswered_calls(true);
    let h = spawn_app(Store::open_in_memory().unwrap(), opts(dir.path()));
    let mut rx = select(&h, existing(net.a_base)).await;
    until(&mut rx, ModemState::Listening).await;
    h.call(AppCommand::Call { peer: "ZS6PEER".into() }).await.unwrap();
    until(&mut rx, ModemState::Calling).await;

    assert_eq!(h.call(AppCommand::SetReceiveOnly { on: true }).await.unwrap(), CommandOk::Done);
    let offs = listen_offs(&net);
    h.call(AppCommand::CancelCall).await.unwrap();
    until(&mut rx, ModemState::Ready).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let s = snapshot(&h).await;
    assert_eq!((s.modem, s.listening, s.receive_only), (ModemState::Ready, false, true));
    assert!(listen_offs(&net) > offs, "LISTEN OFF never went out after the call");
    assert!(!net.listening(0));
}

/// Owner, 24 Nov: Mercury's busy detector ("Listen before transmit") is on by
/// default, sent on attach, so the channel shows Clear or Busy and Mercury
/// waits for a clear channel; the operator can turn it off, at once and for
/// later attaches.
#[tokio::test(flavor = "multi_thread")]
async fn busy_detect_is_on_by_default_and_can_be_turned_off() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let h = spawn_app(Store::open_in_memory().unwrap(), opts(dir.path()));
    let mut rx = select(&h, existing(net.a_base)).await;
    until(&mut rx, ModemState::Listening).await;
    assert_eq!(count(&net, "A> BUSYDETECT ON"), 1, "{:?}", net.log());
    assert!(snapshot(&h).await.busy_detect);

    assert_eq!(h.call(AppCommand::SetBusyDetect { on: false }).await.unwrap(), CommandOk::Done);
    let end = std::time::Instant::now() + Duration::from_secs(5);
    while count(&net, "A> BUSYDETECT OFF") == 0 {
        assert!(std::time::Instant::now() < end, "BUSYDETECT OFF sent: {:?}", net.log());
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(!snapshot(&h).await.busy_detect);
}
