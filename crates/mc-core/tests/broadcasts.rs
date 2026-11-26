//! General broadcasts heard (`CALL: message`), through the service against the
//! FakeNet modem pair. Side A is the app's station; side B is on the air.
#![cfg(feature = "sim")]

use mc_core::app::{
    spawn_app, AppCommand, AppEvent, AppHandle, AppOptions, CommandOk, EventEnvelope, HeardBroadcast, ModemState,
};
use mc_core::profile::{ModemTarget, StationProfile};
use mc_storage::Store;
use mch_protocol::beacon::{Beacon, CAP_CHAT};
use mercury_adapter::fake::FakeNet;
use std::path::Path;
use std::time::Duration;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::broadcast::Receiver;
use tokio::time::Instant;

fn opts(root: &Path) -> AppOptions {
    AppOptions {
        sim_root: Some(root.to_path_buf()),
        stations_root: Some(root.join("stations")),
        ..AppOptions::default()
    }
}

/// A station on side A of `net`, with its broadcast port.
fn existing(net: &FakeNet) -> StationProfile {
    let mut p = StationProfile::new_existing("ZS6ABC");
    p.modem = ModemTarget::Existing {
        host: "127.0.0.1".into(),
        control_port: net.a_base,
        data_port: net.a_base + 1,
        broadcast_port: Some(net.a_bcast),
        telemetry_url: None,
    };
    p
}

async fn wait_for<T>(rx: &mut Receiver<EventEnvelope>, secs: u64, mut want: impl FnMut(&AppEvent) -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        let ev = match tokio::time::timeout_at(deadline, rx.recv()).await {
            Err(_) => panic!("no matching event within {secs} s"),
            Ok(Err(RecvError::Lagged(_))) => continue,
            Ok(Err(RecvError::Closed)) => panic!("the event stream closed"),
            Ok(Ok(env)) => env.event,
        };
        if let Some(t) = want(&ev) {
            return t;
        }
    }
}

async fn until_ready(rx: &mut Receiver<EventEnvelope>) {
    wait_for(rx, 15, |e| matches!(e, AppEvent::ModemState { state: ModemState::Ready, .. }).then_some(())).await
}

async fn save(h: &AppHandle, profile: StationProfile) -> String {
    match h.call(AppCommand::SaveProfile { profile }).await.unwrap() {
        CommandOk::Profile(p) => p.id,
        other => panic!("{other:?}"),
    }
}

async fn select(h: &AppHandle, rx: &mut Receiver<EventEnvelope>, id: &str) {
    h.call(AppCommand::SelectProfile { id: Some(id.to_string()) }).await.unwrap();
    until_ready(rx).await;
}

/// An app on side A of `net`, Ready.
async fn ready(root: &Path, net: &FakeNet) -> (AppHandle, Receiver<EventEnvelope>, String) {
    let h = spawn_app(Store::open_in_memory().unwrap(), opts(root));
    let id = save(&h, existing(net)).await;
    let mut rx = h.subscribe();
    select(&h, &mut rx, &id).await;
    (h, rx, id)
}

/// Put `payload` on the air from side B. The modem may still be settling its
/// listen mode, so a frame sent on another mode is sent again.
fn air(net: &FakeNet, payload: &[u8]) {
    assert!(net.air_broadcast(1, payload.to_vec()), "no session is up");
}

async fn broadcast_event(rx: &mut Receiver<EventEnvelope>, secs: u64) -> HeardBroadcast {
    wait_for(rx, secs, |e| match e {
        AppEvent::BroadcastHeard { broadcast } => Some(broadcast.clone()),
        _ => None,
    })
    .await
}

async fn activity_line(rx: &mut Receiver<EventEnvelope>, kind: &str, secs: u64) -> String {
    wait_for(rx, secs, |e| match e {
        AppEvent::Activity { entry } if entry.kind == kind => Some(entry.text.clone()),
        _ => None,
    })
    .await
}

/// No `BroadcastHeard` for `secs`: long enough for every window to close.
async fn no_more_broadcast_events(rx: &mut Receiver<EventEnvelope>, secs: u64) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while let Ok(got) = tokio::time::timeout_at(deadline, rx.recv()).await {
        match got {
            Ok(env) => assert!(!matches!(env.event, AppEvent::BroadcastHeard { .. }), "{:?}", env.event),
            Err(RecvError::Lagged(_)) => {}
            Err(RecvError::Closed) => panic!("the event stream closed"),
        }
    }
}

async fn listed(h: &AppHandle) -> Vec<HeardBroadcast> {
    match h.call(AppCommand::HeardBroadcasts).await.unwrap() {
        CommandOk::HeardBroadcasts { broadcasts } => broadcasts,
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn a_general_broadcast_is_listed() {
    let net = FakeNet::start().await;
    let dir = tempfile::tempdir().unwrap();
    let (h, mut rx, _) = ready(dir.path(), &net).await;

    air(&net, b"ZS6PEER: net tonight 7.105");
    let first = broadcast_event(&mut rx, 5).await;
    assert_eq!((first.callsign.as_str(), first.message.as_str()), ("ZS6PEER", "net tonight 7.105"));
    assert_eq!((first.snr.clone(), first.mode.clone()), (None, None));

    let settled = broadcast_event(&mut rx, 5).await;
    assert_eq!(settled.id, first.id);
    let snr = settled.snr.clone().expect("the S/N arrives with the second event");
    assert_eq!((snr.value, snr.source.as_str(), snr.at_utc.as_str()), (7.5, "modem monitor", first.at_utc.as_str()));
    assert_eq!(settled.mode.as_deref(), Some("DATAC3"));
    assert_eq!(listed(&h).await, vec![settled]);
}

#[tokio::test]
async fn the_latest_ten_are_listed_newest_first() {
    let net = FakeNet::start().await;
    let dir = tempfile::tempdir().unwrap();
    let (h, mut rx, _) = ready(dir.path(), &net).await;
    for n in 0..11 {
        air(&net, format!("ZS6PEER: number {n}").as_bytes());
        broadcast_event(&mut rx, 5).await;
    }
    let got: Vec<String> = listed(&h).await.into_iter().map(|b| b.message).collect();
    let want: Vec<String> = (1..11).rev().map(|n| format!("number {n}")).collect();
    assert_eq!(got, want);
}

#[tokio::test]
async fn it_does_not_join_the_heard_list() {
    let net = FakeNet::start().await;
    let dir = tempfile::tempdir().unwrap();
    let (h, mut rx, _) = ready(dir.path(), &net).await;
    air(&net, b"ZS6PEER: hello all");
    broadcast_event(&mut rx, 5).await;
    assert!(activity_line(&mut rx, "broadcast", 5).await.contains("ZS6PEER"));
    match h.call(AppCommand::Heard).await.unwrap() {
        CommandOk::Heard { stations } => assert!(stations.is_empty(), "{stations:?}"),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn a_beacon_is_not_a_general_broadcast() {
    let net = FakeNet::start().await;
    let dir = tempfile::tempdir().unwrap();
    let (h, mut rx, _) = ready(dir.path(), &net).await;
    air(&net, &Beacon::new(7, "ZS6PEER", None, CAP_CHAT).unwrap().encode());
    wait_for(&mut rx, 5, |e| matches!(e, AppEvent::Heard { station } if station.callsign == "ZS6PEER").then_some(()))
        .await;
    assert_eq!(listed(&h).await, vec![]);
}

#[tokio::test]
async fn other_broadcasts_are_only_logged() {
    let net = FakeNet::start().await;
    let dir = tempfile::tempdir().unwrap();
    let (h, mut rx, _) = ready(dir.path(), &net).await;
    air(&net, &[0x01, 0xff, 0x00, 0x42]);
    assert!(activity_line(&mut rx, "unsupported", 5).await.contains("another application"));
    assert_eq!(listed(&h).await, vec![]);
    match h.call(AppCommand::Snapshot).await.unwrap() {
        CommandOk::Snapshot(s) => assert_eq!(s.discovery.unsupported, 1),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn no_monitor_report_leaves_snr_blank() {
    let net = FakeNet::start().await;
    net.set_mute_reports(0, true);
    let dir = tempfile::tempdir().unwrap();
    let (h, mut rx, _) = ready(dir.path(), &net).await;
    air(&net, b"ZS6PEER: quiet");
    broadcast_event(&mut rx, 5).await;
    no_more_broadcast_events(&mut rx, 5).await;
    let list = listed(&h).await;
    assert_eq!((list.len(), list[0].snr.clone(), list[0].mode.clone()), (1, None, None));
}

#[tokio::test]
async fn two_at_once_are_both_blank() {
    let net = FakeNet::start().await;
    let dir = tempfile::tempdir().unwrap();
    let (h, mut rx, _) = ready(dir.path(), &net).await;
    air(&net, b"ZS6PEER: first");
    air(&net, b"ZS1XYZ: second");
    broadcast_event(&mut rx, 5).await;
    broadcast_event(&mut rx, 5).await;
    no_more_broadcast_events(&mut rx, 6).await;
    let list = listed(&h).await;
    assert_eq!(list.len(), 2);
    assert!(list.iter().all(|b| b.snr.is_none()), "{list:?}");
}

#[tokio::test]
async fn survives_a_restart() {
    let net = FakeNet::start().await;
    let dir = tempfile::tempdir().unwrap();
    let app_db = dir.path().join("app.db");
    let h = spawn_app(Store::open(&app_db).unwrap(), opts(dir.path()));
    let id = save(&h, existing(&net)).await;
    let mut rx = h.subscribe();
    select(&h, &mut rx, &id).await;
    air(&net, b"ZS6PEER: remember me");
    broadcast_event(&mut rx, 5).await;
    let settled = broadcast_event(&mut rx, 5).await;
    h.call(AppCommand::Shutdown).await.unwrap();
    drop(h);

    // The saved selection opens its station store again.
    let h = spawn_app(Store::open(&app_db).unwrap(), opts(dir.path()));
    assert_eq!(listed(&h).await, vec![settled]);
}

#[tokio::test]
async fn without_a_station_nothing_happens() {
    let dir = tempfile::tempdir().unwrap();
    let h = spawn_app(Store::open_in_memory().unwrap(), opts(dir.path()));
    let err = h.call(AppCommand::HeardBroadcasts).await.unwrap_err();
    assert!(format!("{err:?}").contains("no station is selected"), "{err:?}");
}

#[tokio::test]
async fn each_station_has_its_own_list() {
    let net = FakeNet::start().await;
    let dir = tempfile::tempdir().unwrap();
    let (h, mut rx, first) = ready(dir.path(), &net).await;
    air(&net, b"ZS6PEER: for the first station");
    broadcast_event(&mut rx, 5).await;

    let second = save(&h, existing(&net)).await;
    let mut rx = h.subscribe();
    select(&h, &mut rx, &second).await;
    assert_eq!(listed(&h).await, vec![]);

    let mut rx = h.subscribe();
    select(&h, &mut rx, &first).await;
    assert_eq!(listed(&h).await.len(), 1);
}
