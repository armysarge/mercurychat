//! M3a Task 4: the heard list, favourites and the activity log (DSC-01,
//! DSC-04), through the service against the FakeNet modem pair. Side A is the
//! app's station; the tests play the air from side B or inject control lines.
#![cfg(feature = "sim")]

use mc_core::app::{
    spawn_app, ActivityEntry, AppCommand, AppError, AppEvent, AppHandle, AppOptions, AppSnapshot, CommandOk,
    EventEnvelope, HeardSource, HeardStation, ModemState,
};
use mc_core::profile::{ModemTarget, StationProfile};
use mc_core::station::{Station, StationConfig};
use mc_storage::{HeardRow, Store};
use mch_protocol::beacon::{Beacon, CAP_CHAT, CAP_FILE};
use mercury_adapter::broadcast::BroadcastPort;
use mercury_adapter::fake::FakeNet;
use mercury_adapter::{Callsign, Endpoint};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::broadcast::Receiver;
use tokio::time::Instant;

const PEER: &str = "ZS6PEER";

fn opts(root: &Path) -> AppOptions {
    AppOptions {
        sim_root: Some(root.to_path_buf()),
        stations_root: Some(root.join("stations")),
        ..AppOptions::default()
    }
}

/// The app's station on side A of `net`, with its broadcast port.
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

/// The next event that `want` accepts, within `secs`. A lagging receiver
/// skips what it missed; the caller waits for state, never for a sequence.
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

async fn heard_event(rx: &mut Receiver<EventEnvelope>, call: &str, secs: u64) -> HeardStation {
    wait_for(rx, secs, |e| match e {
        AppEvent::Heard { station } if station.callsign == call => Some(station.clone()),
        _ => None,
    })
    .await
}

async fn activity_event(rx: &mut Receiver<EventEnvelope>, kind: &str, secs: u64) -> ActivityEntry {
    wait_for(rx, secs, |e| match e {
        AppEvent::Activity { entry } if entry.kind == kind => Some(entry.clone()),
        _ => None,
    })
    .await
}

async fn snapshot(h: &AppHandle) -> AppSnapshot {
    match h.call(AppCommand::Snapshot).await.unwrap() {
        CommandOk::Snapshot(s) => s,
        other => panic!("{other:?}"),
    }
}

async fn heard(h: &AppHandle) -> Vec<HeardStation> {
    match h.call(AppCommand::Heard).await.unwrap() {
        CommandOk::Heard { stations } => stations,
        other => panic!("{other:?}"),
    }
}

async fn activity(h: &AppHandle) -> Vec<ActivityEntry> {
    match h.call(AppCommand::Activity).await.unwrap() {
        CommandOk::Activity { entries } => entries,
        other => panic!("{other:?}"),
    }
}

async fn save(h: &AppHandle, profile: StationProfile) -> String {
    match h.call(AppCommand::SaveProfile { profile }).await.unwrap() {
        CommandOk::Profile(p) => p.id,
        other => panic!("{other:?}"),
    }
}

/// Select `id` and wait until its modem is Ready.
async fn select(h: &AppHandle, rx: &mut Receiver<EventEnvelope>, id: &str) {
    h.call(AppCommand::SelectProfile { id: Some(id.to_string()) }).await.unwrap();
    until_state(rx, ModemState::Ready).await;
}

/// An app on side A of `net`, Ready.
async fn ready(root: &Path, net: &FakeNet) -> (AppHandle, Receiver<EventEnvelope>, String) {
    let h = spawn_app(Store::open_in_memory().unwrap(), opts(root));
    let id = save(&h, existing(net)).await;
    let mut rx = h.subscribe();
    select(&h, &mut rx, &id).await;
    (h, rx, id)
}

fn station_db(root: &Path, id: &str) -> PathBuf {
    root.join("stations").join(id).join("station.db")
}

/// The peer's broadcast port on side B: what it sends, A hears.
async fn peer_broadcast(net: &FakeNet) -> BroadcastPort {
    BroadcastPort::connect("127.0.0.1", net.b_bcast, Duration::from_secs(2)).await.unwrap().0
}

fn beacon(call: &str) -> Vec<u8> {
    Beacon::new(7, call, Some("KG33xi"), CAP_CHAT | CAP_FILE).unwrap().encode()
}

/// Send a valid beacon from `call` until A has heard it: from then on A's
/// broadcast port is known to be connected.
async fn sync_broadcast(rx: &mut Receiver<EventEnvelope>, port: &BroadcastPort, call: &str) -> HeardStation {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        port.send(&beacon(call)).await.unwrap();
        let got = tokio::time::timeout(
            Duration::from_secs(1),
            // The outer timeout ends each try; this deadline is only a backstop.
            wait_for(rx, 5, |e| match e {
                AppEvent::Heard { station } if station.callsign == call => Some(station.clone()),
                _ => None,
            }),
        )
        .await;
        if let Ok(st) = got {
            return st;
        }
        assert!(Instant::now() < deadline, "A never heard a beacon");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn cq_from_peer_appears_in_heard_with_monitor_snr() {
    let net = FakeNet::start().await;
    let dir = tempfile::tempdir().unwrap();
    let (h, mut rx, _) = ready(dir.path(), &net).await;
    let pdir = tempfile::tempdir().unwrap();
    let mut b = Station::open(StationConfig::defaults(
        Callsign::parse(PEER).unwrap(),
        Endpoint::base("127.0.0.1", net.b_base),
        pdir.path(),
    ))
    .unwrap();
    b.attach().await.unwrap();
    b.cq().await.unwrap();
    // CQFRAME first, then MONITOR's SNR merges into the same row.
    let st = wait_for(&mut rx, 10, |e| match e {
        AppEvent::Heard { station } | AppEvent::HeardUpdated { station }
            if station.callsign == PEER && station.snr.is_some() =>
        {
            Some(station.clone())
        }
        _ => None,
    })
    .await;
    assert_eq!(st.source, Some(HeardSource::Cq));
    let snr = st.snr.unwrap();
    assert!((snr.value - 7.5).abs() < 0.01, "{snr:?}");
    assert_eq!(snr.source, "modem monitor");
    assert_eq!(snr.age_ms, None);
    let list = heard(&h).await;
    let row = list.iter().find(|s| s.callsign == PEER).expect("the peer is heard");
    assert_eq!(row.count, 1, "the MONITOR line merges into the CQ row: {row:?}");
    assert!(row.bandwidth.is_some());
    assert!(row.snr.is_some());
    assert!(!row.favourite);
    let log = activity(&h).await;
    assert!(log.iter().any(|e| e.kind == "heard" && e.text.contains(PEER)), "{log:?}");
}

/// Send `lines` as Mercury's control stream, then a marker CQ, and wait
/// until A has heard the marker: control lines are handled in order, so
/// everything before it has been handled too.
/// Returns every event published before the marker was heard.
async fn control_lines(net: &FakeNet, rx: &mut Receiver<EventEnvelope>, lines: &[String]) -> Vec<AppEvent> {
    for l in lines {
        net.send_control(0, l);
    }
    net.send_control(0, "CQFRAME ZS9END 500");
    let mut seen = Vec::new();
    wait_for(rx, 10, |e| {
        if matches!(e, AppEvent::Heard { station } if station.callsign == "ZS9END") {
            return Some(());
        }
        seen.push(e.clone());
        None
    })
    .await;
    seen
}

/// The `heard` events and the heard activity lines `call` got in `events`.
fn heard_counts(events: &[AppEvent], call: &str) -> (usize, usize) {
    let heard = events.iter().filter(|e| matches!(e, AppEvent::Heard { station } if station.callsign == call)).count();
    let lines = events
        .iter()
        .filter(|e| matches!(e, AppEvent::Activity { entry } if entry.kind == "heard" && entry.text.contains(call)))
        .count();
    (heard, lines)
}

#[tokio::test(flavor = "multi_thread")]
async fn one_heard_event_and_one_heard_line_per_cq_in_either_order() {
    // Lab F1b: the line that completes a CQ's row is an update of the same
    // observation, not a second hearing.
    let net = FakeNet::start().await;
    let dir = tempfile::tempdir().unwrap();
    let (h, mut rx, _) = ready(dir.path(), &net).await;
    for (i, lines) in [[monitor(PEER, 25.0), cqframe(PEER)], [cqframe(PEER), monitor(PEER, 20.0)]].iter().enumerate() {
        let events = control_lines(&net, &mut rx, lines).await;
        assert_eq!(heard_counts(&events, PEER), (1, 1), "order {i}: {events:?}");
        let updates: Vec<HeardStation> = events
            .iter()
            .filter_map(|e| match e {
                AppEvent::HeardUpdated { station } if station.callsign == PEER => Some(station.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(updates.len(), 1, "the completing line publishes one update: {events:?}");
        assert!(updates[0].snr.is_some() && updates[0].bandwidth.is_some(), "{:?}", updates[0]);
        assert_eq!(updates[0].count, (i + 1) as u32, "the update is the same row, not a new one");
    }
    assert_eq!(heard(&h).await.iter().find(|s| s.callsign == PEER).unwrap().count, 2);
}

fn monitor(call: &str, snr: f32) -> String {
    format!("MONITOR DATAC16 CQ FROM={call} BW=500 SNR={snr:.1}")
}

fn cqframe(call: &str) -> String {
    format!("CQFRAME {call} 500")
}

/// The heard row of `call`: one observation, from a CQ.
async fn one_cq(h: &AppHandle, call: &str) -> HeardStation {
    let list = heard(h).await;
    let row = list.into_iter().find(|s| s.callsign == call).expect("heard");
    assert_eq!(row.count, 1, "one CQ is one observation: {row:?}");
    assert_eq!(row.source, Some(HeardSource::Cq));
    assert_eq!(row.bandwidth.as_deref(), Some("500"));
    row
}

fn monitor_snr(row: &HeardStation) -> f32 {
    let snr = row.snr.clone().unwrap_or_else(|| panic!("the MONITOR SNR was lost: {row:?}"));
    assert_eq!(snr.source, "modem monitor");
    snr.value
}

#[tokio::test(flavor = "multi_thread")]
async fn monitor_before_cqframe_is_one_cq_with_the_monitor_snr() {
    // Lab Q3: both lines come in one read, MONITOR first in up to 4 of 5 CQs.
    let net = FakeNet::start().await;
    let dir = tempfile::tempdir().unwrap();
    let (h, mut rx, _) = ready(dir.path(), &net).await;
    control_lines(&net, &mut rx, &[monitor(PEER, 25.0), cqframe(PEER)]).await;
    let row = one_cq(&h, PEER).await;
    assert!((monitor_snr(&row) - 25.0).abs() < 0.01, "{row:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn cqframe_before_monitor_is_one_cq_with_the_monitor_snr() {
    let net = FakeNet::start().await;
    let dir = tempfile::tempdir().unwrap();
    let (h, mut rx, _) = ready(dir.path(), &net).await;
    control_lines(&net, &mut rx, &[cqframe(PEER), monitor(PEER, 25.0)]).await;
    let row = one_cq(&h, PEER).await;
    assert!((monitor_snr(&row) - 25.0).abs() < 0.01, "{row:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn cqframe_without_monitor_has_no_snr_never_zero() {
    let net = FakeNet::start().await;
    let dir = tempfile::tempdir().unwrap();
    let (h, mut rx, _) = ready(dir.path(), &net).await;
    control_lines(&net, &mut rx, &[cqframe(PEER)]).await;
    let row = one_cq(&h, PEER).await;
    assert_eq!(row.snr, None, "no MONITOR line, no SNR: {row:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn two_callsigns_interleaved_keep_their_own_snr() {
    let net = FakeNet::start().await;
    let dir = tempfile::tempdir().unwrap();
    let (h, mut rx, _) = ready(dir.path(), &net).await;
    let lines = [monitor(PEER, 25.0), monitor("ZS1TWO", 12.0), cqframe("ZS1TWO"), cqframe(PEER)];
    control_lines(&net, &mut rx, &lines).await;
    assert!((monitor_snr(&one_cq(&h, PEER).await) - 25.0).abs() < 0.01);
    assert!((monitor_snr(&one_cq(&h, "ZS1TWO").await) - 12.0).abs() < 0.01);
    // And the other way round, each CQ still one row more.
    let lines = [cqframe(PEER), cqframe("ZS1TWO"), monitor("ZS1TWO", 9.0), monitor(PEER, 3.0)];
    control_lines(&net, &mut rx, &lines).await;
    let list = heard(&h).await;
    for (call, snr) in [(PEER, 3.0), ("ZS1TWO", 9.0)] {
        let row = list.iter().find(|s| s.callsign == call).unwrap();
        assert_eq!(row.count, 2, "{row:?}");
        assert!((monitor_snr(row) - snr).abs() < 0.01, "{row:?}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn beacon_from_peer_appears_with_grid_and_caps() {
    let net = FakeNet::start().await;
    let dir = tempfile::tempdir().unwrap();
    let (h, mut rx, _) = ready(dir.path(), &net).await;
    let port = peer_broadcast(&net).await;
    let st = sync_broadcast(&mut rx, &port, PEER).await;
    assert_eq!(st.source, Some(HeardSource::Beacon));
    assert_eq!(st.grid.as_deref(), Some("KG33xi"));
    assert_eq!(st.caps, Some(CAP_CHAT | CAP_FILE));
    assert!(st.snr.is_none(), "a beacon carries no SNR of its own");
    let list = heard(&h).await;
    let row = list.iter().find(|s| s.callsign == PEER).unwrap();
    assert_eq!(row.grid.as_deref(), Some("KG33xi"));
    assert_eq!(row.caps, Some(CAP_CHAT | CAP_FILE));
    assert_eq!(snapshot(&h).await.discovery.malformed, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn truncated_beacon_is_malformed_not_heard() {
    let net = FakeNet::start().await;
    let dir = tempfile::tempdir().unwrap();
    let (h, mut rx, _) = ready(dir.path(), &net).await;
    let port = peer_broadcast(&net).await;
    sync_broadcast(&mut rx, &port, "ZS6SYNC").await;
    let mut cut = beacon("ZS6BAD");
    cut.pop();
    port.send(&cut).await.unwrap();
    let entry = activity_event(&mut rx, "malformed", 10).await;
    assert!(!entry.text.is_empty());
    let snap = snapshot(&h).await;
    assert_eq!(snap.discovery.malformed, 1);
    assert_eq!(snap.discovery.unsupported, 0);
    assert!(heard(&h).await.iter().all(|s| s.callsign != "ZS6BAD"), "a malformed beacon is never a station");
}

#[tokio::test(flavor = "multi_thread")]
async fn foreign_kiss_frame_counts_as_unsupported() {
    let net = FakeNet::start().await;
    let dir = tempfile::tempdir().unwrap();
    let (h, mut rx, _) = ready(dir.path(), &net).await;
    let port = peer_broadcast(&net).await;
    sync_broadcast(&mut rx, &port, "ZS6SYNC").await;
    port.send(b"RNS\x01 another application's traffic").await.unwrap();
    activity_event(&mut rx, "unsupported", 10).await;
    let snap = snapshot(&h).await;
    assert_eq!(snap.discovery.unsupported, 1);
    assert_eq!(snap.discovery.malformed, 0);
    let list = heard(&h).await;
    assert_eq!(list.len(), 1, "only the sync beacon is a station: {list:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn invalid_callsign_from_the_air_is_dropped() {
    let net = FakeNet::start().await;
    let dir = tempfile::tempdir().unwrap();
    let (h, mut rx, _) = ready(dir.path(), &net).await;
    let sixteen = "ZS6ABCDEFGHIJKLM";
    assert_eq!(sixteen.len(), 16);
    net.send_control(0, "CQFRAME zs6 abc 500");
    net.send_control(0, &format!("CQFRAME {sixteen} 500"));
    net.send_control(0, &format!("MONITOR DATAC16 CQ FROM={sixteen} BW=500 SNR=3.0"));
    for _ in 0..3 {
        activity_event(&mut rx, "malformed", 10).await;
    }
    assert_eq!(snapshot(&h).await.discovery.malformed, 3);
    let list = heard(&h).await;
    assert!(list.is_empty(), "nothing from an invalid callsign is stored: {list:?}");
    // A valid call on the same path is still heard.
    net.send_control(0, "CQFRAME zs6ok 500");
    let st = heard_event(&mut rx, "ZS6OK", 10).await;
    assert_eq!(st.source, Some(HeardSource::Cq));
}

#[tokio::test(flavor = "multi_thread")]
async fn heard_ages_from_receiver_clock() {
    let net = FakeNet::start().await;
    let dir = tempfile::tempdir().unwrap();
    let (h, mut rx, _) = ready(dir.path(), &net).await;
    let before = mc_storage::now_utc();
    net.send_control(0, &format!("CQFRAME {PEER} 500"));
    let st = heard_event(&mut rx, PEER, 10).await;
    let after = mc_storage::now_utc();
    let at = st.last_heard_utc.clone().expect("a heard station has a time");
    assert!(before <= at && at <= after, "{before} <= {at} <= {after}");
    let row = heard(&h).await.into_iter().find(|s| s.callsign == PEER).unwrap();
    assert_eq!(row.last_heard_utc, Some(at));
}

#[tokio::test(flavor = "multi_thread")]
async fn heard_older_than_30_days_is_purged_on_open() {
    let net = FakeNet::start().await;
    let dir = tempfile::tempdir().unwrap();
    let h = spawn_app(Store::open_in_memory().unwrap(), opts(dir.path()));
    let id = save(&h, existing(&net)).await;
    let ago =
        |days: i64| (chrono::Utc::now() - chrono::Duration::days(days)).format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();
    {
        let db = station_db(dir.path(), &id);
        std::fs::create_dir_all(db.parent().unwrap()).unwrap();
        let mut store = Store::open(&db).unwrap();
        let row = |call: &str, at: String| HeardRow {
            id: 0,
            callsign: call.into(),
            observed_utc: at,
            kind: "cq".into(),
            bandwidth: Some("500".into()),
            grid: None,
            caps: None,
            snr_db: None,
            snr_source: None,
            freq_hz: None,
            freq_source: None,
            freq_age_ms: None,
        };
        store.record_heard(&row("ZS6OLD", ago(40))).unwrap();
        store.record_heard(&row("ZS6OLD", ago(31))).unwrap();
        store.record_heard(&row("ZS6NEW", ago(29))).unwrap();
    }
    let mut rx = h.subscribe();
    select(&h, &mut rx, &id).await;
    let list = heard(&h).await;
    let calls: Vec<&str> = list.iter().map(|s| s.callsign.as_str()).collect();
    assert_eq!(calls, ["ZS6NEW"]);
    let log = activity(&h).await;
    assert!(log.iter().any(|e| e.kind == "purge" && e.text.contains('2')), "{log:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn session_peer_is_recorded_as_heard() {
    let dir = tempfile::tempdir().unwrap();
    let h = spawn_app(Store::open_in_memory().unwrap(), opts(dir.path()));
    let id = save(&h, StationProfile::new_simulator("ZS6ABC")).await;
    let mut rx = h.subscribe();
    select(&h, &mut rx, &id).await;
    h.call(AppCommand::Call { peer: "sim1".into() }).await.unwrap();
    let st = wait_for(&mut rx, 20, |e| match e {
        AppEvent::Heard { station } if station.callsign == "SIM1" => Some(station.clone()),
        _ => None,
    })
    .await;
    assert_eq!(st.source, Some(HeardSource::Session));
    let list = heard(&h).await;
    let row = list.iter().find(|s| s.callsign == "SIM1").unwrap();
    assert_eq!(row.source, Some(HeardSource::Session));
    assert_eq!(row.count, 1);
}

/// M3a Task 10: on the simulator with SIM1 on the air (the desktop app's
/// setting), SIM1's CQ and its beacon with a locator reach the heard list
/// without RF, and our own Beacon now goes out through the fake's broadcast
/// port. Tests that pin event sequences leave this off (the default).
#[tokio::test(flavor = "multi_thread")]
async fn simulator_peer_calls_cq_and_beacons() {
    let dir = tempfile::tempdir().unwrap();
    let h = spawn_app(Store::open_in_memory().unwrap(), AppOptions { sim_peer_broadcasts: true, ..opts(dir.path()) });
    let id = save(&h, StationProfile::new_simulator("ZS6ABC")).await;
    let mut rx = h.subscribe();
    select(&h, &mut rx, &id).await;
    let cq = heard_event(&mut rx, "SIM1", 10).await;
    assert_eq!(cq.source, Some(HeardSource::Cq), "{cq:?}");
    let beacon = wait_for(&mut rx, 10, |e| match e {
        AppEvent::Heard { station } | AppEvent::HeardUpdated { station }
            if station.callsign == "SIM1" && station.source == Some(HeardSource::Beacon) =>
        {
            Some(station.clone())
        }
        _ => None,
    })
    .await;
    assert_eq!(beacon.grid.as_deref(), Some(mc_core::sim::PEER_GRID));
    h.call(AppCommand::SendBeaconNow).await.unwrap();
    let outcome = wait_for(&mut rx, 10, |e| match e {
        AppEvent::Broadcast { outcome, .. } if !matches!(outcome, mc_core::app::BroadcastOutcome::Submitted) => {
            Some(outcome.clone())
        }
        _ => None,
    })
    .await;
    assert_eq!(outcome, mc_core::app::BroadcastOutcome::Transmitted, "our beacon goes out on the simulator");
}

#[tokio::test(flavor = "multi_thread")]
async fn favourite_is_listed_without_recent_observation() {
    let net = FakeNet::start().await;
    let dir = tempfile::tempdir().unwrap();
    let (h, mut rx, _) = ready(dir.path(), &net).await;
    let ok = h.call(AppCommand::SetFavourite { callsign: "zs6fav".into(), on: true }).await.unwrap();
    assert_eq!(ok, CommandOk::Done);
    let list = heard(&h).await;
    let fav = list.iter().find(|s| s.callsign == "ZS6FAV").expect("a favourite is always listed");
    assert!(fav.favourite);
    assert_eq!(fav.count, 0);
    assert_eq!(fav.last_heard_utc, None, "never heard: no time is made up");
    assert_eq!(fav.source, None);
    assert!(fav.snr.is_none());
    // A heard station marked favourite keeps its observation.
    net.send_control(0, &format!("CQFRAME {PEER} 500"));
    heard_event(&mut rx, PEER, 10).await;
    h.call(AppCommand::SetFavourite { callsign: PEER.into(), on: true }).await.unwrap();
    let st = heard_event(&mut rx, PEER, 5).await;
    assert!(st.favourite);
    assert_eq!(st.source, Some(HeardSource::Cq));
    // Unmarked and never heard: gone from the list.
    h.call(AppCommand::SetFavourite { callsign: "ZS6FAV".into(), on: false }).await.unwrap();
    assert!(heard(&h).await.iter().all(|s| s.callsign != "ZS6FAV"));
    let bad = h.call(AppCommand::SetFavourite { callsign: "not a call".into(), on: true }).await;
    assert!(matches!(bad, Err(AppError::Validation { .. })), "{bad:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn activity_ring_is_bounded_at_500() {
    let net = FakeNet::start().await;
    let dir = tempfile::tempdir().unwrap();
    let (h, mut rx, _) = ready(dir.path(), &net).await;
    let text = |n: u64| format!("modem buffer: {n} bytes");
    // In batches, so the observation channel never overflows.
    for batch in 0..12u64 {
        for i in 1..=50u64 {
            net.send_control(0, &format!("BUFFER {}", batch * 50 + i));
        }
        let last = text(batch * 50 + 50);
        wait_for(&mut rx, 10, |e| matches!(e, AppEvent::Activity { entry } if entry.text == last).then_some(())).await;
    }
    // A repeated value is not a change.
    net.send_control(0, "BUFFER 600");
    net.send_control(0, "BUSY ON");
    activity_event(&mut rx, "busy", 10).await;
    let log = activity(&h).await;
    assert_eq!(log.len(), 500);
    assert_eq!(log.last().unwrap().kind, "busy");
    assert_eq!(log[log.len() - 2].text, text(600));
    assert!(log.iter().all(|e| e.text != text(1)), "the oldest entries have left the ring");
    assert_eq!(log.iter().filter(|e| e.text == text(600)).count(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn occupied_broadcast_port_is_logged_not_faulted() {
    let net = FakeNet::start().await;
    let dir = tempfile::tempdir().unwrap();
    // Another client holds A's broadcast port; Mercury takes one.
    let _other = BroadcastPort::connect("127.0.0.1", net.a_bcast, Duration::from_secs(2)).await.unwrap();
    let h = spawn_app(Store::open_in_memory().unwrap(), opts(dir.path()));
    let id = save(&h, existing(&net)).await;
    let mut rx = h.subscribe();
    h.call(AppCommand::SelectProfile { id: Some(id) }).await.unwrap();
    // Ready and the activity line come in either order; a fault fails.
    let (mut ready, mut line) = (false, None);
    while !(ready && line.is_some()) {
        wait_for(&mut rx, 15, |e| match e {
            AppEvent::ModemState { state: ModemState::Ready, .. } => {
                ready = true;
                Some(())
            }
            AppEvent::Activity { entry } if entry.kind == "broadcast" => {
                line = Some(entry.clone());
                Some(())
            }
            _ => None,
        })
        .await;
    }
    let line = line.unwrap();
    assert!(line.text.contains("closed"), "{line:?}");
    // Optional plane: the modem stays up and CQ is still heard. Listening
    // starts by itself after attach (D18), so either idle state counts.
    let modem = snapshot(&h).await.modem;
    assert!(matches!(modem, ModemState::Ready | ModemState::Listening), "{modem:?}");
    net.send_control(0, &format!("CQFRAME {PEER} 500"));
    heard_event(&mut rx, PEER, 10).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn broadcast_port_reopens_after_recovery() {
    let net = FakeNet::start().await;
    let dir = tempfile::tempdir().unwrap();
    let (h, mut rx, _) = ready(dir.path(), &net).await;
    let port = peer_broadcast(&net).await;
    sync_broadcast(&mut rx, &port, "ZS6ONE").await;
    // Mercury drops our sockets (as on a restart): the modem recovers.
    net.kill_client(0);
    until_state(&mut rx, ModemState::Recovering).await;
    wait_for(&mut rx, 60, |e| matches!(e, AppEvent::ModemState { state: ModemState::Ready, .. }).then_some(())).await;
    // Beacons are heard again without selecting the profile again.
    let st = sync_broadcast(&mut rx, &port, "ZS6TWO").await;
    assert_eq!(st.source, Some(HeardSource::Beacon));
    let log = activity(&h).await;
    let lines: Vec<&str> = log.iter().filter(|e| e.kind == "broadcast").map(|e| e.text.as_str()).collect();
    assert!(lines.iter().any(|t| t.contains("reopened")), "{lines:?}");
    assert!(lines.iter().all(|t| !t.contains("another client")), "no guessed causes: {lines:?}");
}
