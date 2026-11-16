//! M2 Task 3: the modem runs in its own task. The service answers while a
//! modem attaches, ignores reports from a modem it has moved on from, and
//! holds the endpoint lock (MOD-02) for as long as a real Mercury is attached.

use mc_core::app::{spawn_app, AppCommand, AppEvent, AppHandle, AppOptions, CommandOk, EventEnvelope, ModemState};
use mc_core::lock::lock_endpoint;
use mc_core::profile::{ModemTarget, StationProfile};
use mc_storage::Store;
use mercury_adapter::fake::FakeNet;
use std::path::Path;
use std::time::{Duration, Instant};
use tokio::net::TcpListener;
use tokio::sync::broadcast::Receiver;

fn opts(root: &Path) -> AppOptions {
    AppOptions {
        sim_root: Some(root.to_path_buf()),
        stations_root: Some(root.join("stations")),
        locks_dir: Some(root.join("locks")),
        ..AppOptions::default()
    }
}

fn existing(call: &str, control: u16, data: u16) -> StationProfile {
    let mut p = StationProfile::new_existing(call);
    p.modem = ModemTarget::Existing {
        host: "127.0.0.1".into(),
        control_port: control,
        data_port: data,
        broadcast_port: None,
        telemetry_url: None,
    };
    p
}

async fn save(h: &AppHandle, p: StationProfile) -> String {
    match h.call(AppCommand::SaveProfile { profile: p }).await.unwrap() {
        CommandOk::Profile(p) => p.id,
        other => panic!("{other:?}"),
    }
}

async fn next(rx: &mut Receiver<EventEnvelope>, within: Duration) -> Option<EventEnvelope> {
    tokio::time::timeout(within, rx.recv()).await.ok().map(|r| r.unwrap())
}

/// The next modem state event within `within`.
async fn next_state(rx: &mut Receiver<EventEnvelope>, within: Duration) -> Option<(ModemState, Option<String>)> {
    let end = tokio::time::Instant::now() + within;
    loop {
        let left = end.saturating_duration_since(tokio::time::Instant::now());
        match next(rx, left).await?.event {
            AppEvent::ModemState { state, detail } => return Some((state, detail)),
            _ => continue,
        }
    }
}

/// A "modem" that accepts TCP and never says a word: attach hangs.
async fn silent_modem() -> (u16, u16) {
    let control = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let data = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let ports = (control.local_addr().unwrap().port(), data.local_addr().unwrap().port());
    for l in [control, data] {
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((s, _)) = l.accept().await {
                held.push(s);
            }
        });
    }
    ports
}

#[tokio::test(flavor = "multi_thread")]
async fn snapshot_answers_while_attaching() {
    let dir = tempfile::tempdir().unwrap();
    let h = spawn_app(Store::open_in_memory().unwrap(), opts(dir.path()));
    let (c, d) = silent_modem().await;
    let id = save(&h, existing("ZS6ABC", c, d)).await;
    let mut rx = h.subscribe();
    h.call(AppCommand::SelectProfile { id: Some(id) }).await.unwrap();
    assert_eq!(next_state(&mut rx, Duration::from_secs(2)).await.map(|s| s.0), Some(ModemState::Attaching));
    let t = Instant::now();
    let snap = match h.call(AppCommand::Snapshot).await.unwrap() {
        CommandOk::Snapshot(s) => s,
        other => panic!("{other:?}"),
    };
    assert!(t.elapsed() < Duration::from_millis(200), "snapshot took {:?} during an attach", t.elapsed());
    assert_eq!(snap.modem, ModemState::Attaching);
}

#[tokio::test(flavor = "multi_thread")]
async fn stale_generation_reports_are_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let h = spawn_app(Store::open_in_memory().unwrap(), opts(dir.path()));
    let (c, d) = silent_modem().await;
    let a = save(&h, existing("ZS6AAA", c, d)).await;
    let b = save(&h, StationProfile::new_simulator("ZS6BBB")).await;
    let mut rx = h.subscribe();
    h.call(AppCommand::SelectProfile { id: Some(a) }).await.unwrap();
    assert_eq!(next_state(&mut rx, Duration::from_secs(2)).await.map(|s| s.0), Some(ModemState::Attaching));
    h.call(AppCommand::SelectProfile { id: Some(b) }).await.unwrap();
    loop {
        match next_state(&mut rx, Duration::from_secs(5)).await {
            Some((ModemState::Listening, _)) => break,
            Some(_) => continue,
            None => panic!("simulator never became ready"),
        }
    }
    // A's attach would fail after its 5 s command timeout; nothing of it may surface.
    let late = next_state(&mut rx, Duration::from_secs(7)).await;
    assert_eq!(late, None, "a report from the previous modem leaked through");
}

#[tokio::test(flavor = "multi_thread")]
async fn occupied_endpoint_faults_once() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let shared_locks = dir.path().join("locks");
    let mut o1 = opts(&dir.path().join("one"));
    o1.locks_dir = Some(shared_locks.clone());
    let mut o2 = opts(&dir.path().join("two"));
    o2.locks_dir = Some(shared_locks);

    let first = spawn_app(Store::open_in_memory().unwrap(), o1);
    let id1 = save(&first, existing("ZS6AAA", net.a_base, net.a_base + 1)).await;
    let mut rx1 = first.subscribe();
    first.call(AppCommand::SelectProfile { id: Some(id1) }).await.unwrap();
    loop {
        match next_state(&mut rx1, Duration::from_secs(10)).await {
            Some((ModemState::Listening, _)) => break,
            Some((ModemState::Faulted, d)) => panic!("first attach faulted: {d:?}"),
            Some(_) => continue,
            None => panic!("first never ready"),
        }
    }

    let second = spawn_app(Store::open_in_memory().unwrap(), o2);
    let id2 = save(&second, existing("ZS6BBB", net.a_base, net.a_base + 1)).await;
    let mut rx2 = second.subscribe();
    second.call(AppCommand::SelectProfile { id: Some(id2) }).await.unwrap();
    match next_state(&mut rx2, Duration::from_secs(5)).await {
        Some((ModemState::Faulted, Some(d))) => {
            assert!(d.contains("another MercuryChat instance is using the modem"), "{d}")
        }
        other => panic!("expected Faulted with the lock text first, got {other:?}"),
    }
    assert_eq!(next_state(&mut rx2, Duration::from_secs(2)).await, None, "no takeover loop");
}

#[tokio::test(flavor = "multi_thread")]
async fn deselect_releases_the_endpoint_lock() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let h = spawn_app(Store::open_in_memory().unwrap(), opts(dir.path()));
    let id = save(&h, existing("ZS6AAA", net.a_base, net.a_base + 1)).await;
    let mut rx = h.subscribe();
    h.call(AppCommand::SelectProfile { id: Some(id) }).await.unwrap();
    while !matches!(next_state(&mut rx, Duration::from_secs(10)).await, Some((ModemState::Listening, _))) {}
    assert!(lock_endpoint(&dir.path().join("locks"), "127.0.0.1", net.a_base).is_err(), "held while attached");
    h.call(AppCommand::SelectProfile { id: None }).await.unwrap();
    let end = Instant::now() + Duration::from_secs(2);
    loop {
        if lock_endpoint(&dir.path().join("locks"), "127.0.0.1", net.a_base).is_ok() {
            break;
        }
        assert!(Instant::now() < end, "the endpoint lock was not released after deselecting");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[cfg(feature = "sim")]
#[tokio::test(flavor = "multi_thread")]
async fn sim_peer_echoes() {
    use mc_core::sim::{SimModem, PEER_CALLSIGN};
    use mc_core::{SessionCommand, SessionEvent};
    use mch_protocol::ObjectId;
    use mercury_adapter::Callsign;
    use tokio::sync::mpsc;

    let dir = tempfile::tempdir().unwrap();
    let mut sim = SimModem::start(&dir.path().join("station"), &dir.path().join("peer"), "ZS6ABC").await.unwrap();
    let info = sim.station.call(&Callsign::parse(PEER_CALLSIGN).unwrap(), Duration::from_secs(10)).await.unwrap();
    let (cmd_tx, mut cmd_rx) = mpsc::channel(8);
    let (_req_tx, mut req_rx) = mpsc::channel(1);
    let (ev_tx, mut ev_rx) = mpsc::unbounded_channel();
    cmd_tx.send(SessionCommand::SendChat { id: ObjectId::random(), text: "hi 👋".into() }).await.unwrap();
    let watch = async {
        while let Some(ev) = ev_rx.recv().await {
            if let SessionEvent::ChatReceived { text, .. } = ev {
                return text;
            }
        }
        String::new()
    };
    let got = tokio::time::timeout(Duration::from_secs(15), async {
        tokio::select! {
            t = watch => t,
            end = sim.station.run_session(&info, &mut cmd_rx, &mut req_rx, ev_tx) => panic!("session ended first: {end:?}"),
        }
    })
    .await
    .expect("echo within 15 s");
    assert_eq!(got, "echo: hi 👋");
}
