//! M1 exit: the UI runs on simulated modem events, with no audio and no RF.
#![cfg(feature = "sim")]

use mc_core::app::{spawn_app, AppCommand, AppEvent, AppHandle, AppOptions, CommandOk, EventEnvelope, ModemState};
use mc_core::profile::StationProfile;
use mc_core::sim::SimModem;
use mc_storage::Store;
use std::time::Duration;
use tokio::sync::broadcast::Receiver;

async fn next(rx: &mut Receiver<EventEnvelope>) -> EventEnvelope {
    tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.expect("event within 5 s").unwrap()
}

async fn sim_app(root: &std::path::Path) -> (AppHandle, String) {
    let h = spawn_app(
        Store::open_in_memory().unwrap(),
        AppOptions { sim_root: Some(root.to_path_buf()), ..AppOptions::default() },
    );
    let id = match h.call(AppCommand::SaveProfile { profile: StationProfile::new_simulator("ZS6ABC") }).await.unwrap() {
        CommandOk::Profile(p) => p.id,
        other => panic!("{other:?}"),
    };
    (h, id)
}

async fn modem(h: &AppHandle) -> ModemState {
    match h.call(AppCommand::Snapshot).await.unwrap() {
        CommandOk::Snapshot(s) => s.modem,
        other => panic!("{other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn simulator_profile_reaches_ready() {
    let dir = tempfile::tempdir().unwrap();
    let (h, id) = sim_app(dir.path()).await;
    let mut rx = h.subscribe();
    h.call(AppCommand::SelectProfile { id: Some(id.clone()) }).await.unwrap();

    let a = next(&mut rx).await;
    assert_eq!(a.event, AppEvent::ActiveProfileChanged { id: Some(id) });
    let b = next(&mut rx).await;
    assert_eq!(b.event, AppEvent::ModemState { state: ModemState::Attaching, detail: None });
    let c = next(&mut rx).await;
    assert_eq!(c.event, AppEvent::ModemState { state: ModemState::Ready, detail: Some("simulator".into()) });
    assert!(a.generation >= 1 && b.generation == a.generation && c.generation == a.generation);
    assert_eq!(modem(&h).await, ModemState::Ready);
}

#[tokio::test(flavor = "multi_thread")]
async fn deselect_goes_unavailable() {
    let dir = tempfile::tempdir().unwrap();
    let (h, id) = sim_app(dir.path()).await;
    let mut rx = h.subscribe();
    h.call(AppCommand::SelectProfile { id: Some(id) }).await.unwrap();
    while !matches!(next(&mut rx).await.event, AppEvent::ModemState { state: ModemState::Ready, .. }) {}

    h.call(AppCommand::SelectProfile { id: None }).await.unwrap();
    assert_eq!(next(&mut rx).await.event, AppEvent::ActiveProfileChanged { id: None });
    assert_eq!(next(&mut rx).await.event, AppEvent::ModemState { state: ModemState::Unavailable, detail: None });
    assert_eq!(modem(&h).await, ModemState::Unavailable);
}

#[tokio::test(flavor = "multi_thread")]
async fn reselecting_the_simulator_works() {
    let dir = tempfile::tempdir().unwrap();
    let (h, id) = sim_app(dir.path()).await;
    let mut rx = h.subscribe();
    for _ in 0..2 {
        h.call(AppCommand::SelectProfile { id: Some(id.clone()) }).await.unwrap();
        while !matches!(next(&mut rx).await.event, AppEvent::ModemState { state: ModemState::Ready, .. }) {}
        h.call(AppCommand::SelectProfile { id: None }).await.unwrap();
        while !matches!(next(&mut rx).await.event, AppEvent::ModemState { state: ModemState::Unavailable, .. }) {}
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn simulator_without_a_data_root_faults() {
    let h = spawn_app(Store::open_in_memory().unwrap(), AppOptions::default());
    let id = match h.call(AppCommand::SaveProfile { profile: StationProfile::new_simulator("ZS6ABC") }).await.unwrap() {
        CommandOk::Profile(p) => p.id,
        other => panic!("{other:?}"),
    };
    let mut rx = h.subscribe();
    h.call(AppCommand::SelectProfile { id: Some(id) }).await.unwrap();
    next(&mut rx).await; // ActiveProfileChanged
    match next(&mut rx).await.event {
        AppEvent::ModemState { state: ModemState::Faulted, detail: Some(d) } => assert!(d.contains("simulator"), "{d}"),
        other => panic!("expected Faulted, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn no_real_endpoint_is_touched() {
    let dir = tempfile::tempdir().unwrap();
    let sim = SimModem::start(dir.path(), "ZS6ABC").await.unwrap();
    for base in [sim.net.a_base, sim.net.b_base] {
        assert!(![8100, 8300, 8301, 8400, 8401].contains(&base), "simulator used a real Mercury port {base}");
    }
    assert_eq!(sim.peer_callsign, "SIM1");
}

#[tokio::test(flavor = "multi_thread")]
async fn restart_reattaches_the_selected_simulator() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("mercurychat.db");
    {
        let mut store = Store::open(&db).unwrap();
        let p = StationProfile::new_simulator("ZS6ABC").validate().unwrap();
        store.save_profile(&mc_core::profile::to_row(&p)).unwrap();
        store.set_setting("activeProfileId", &p.id).unwrap();
    }
    let h = spawn_app(
        Store::open(&db).unwrap(),
        AppOptions { sim_root: Some(dir.path().to_path_buf()), ..AppOptions::default() },
    );
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while modem(&h).await != ModemState::Ready {
        assert!(tokio::time::Instant::now() < deadline, "the selected simulator was not re-attached on start");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn snap(h: &AppHandle) -> mc_core::app::AppSnapshot {
    match h.call(AppCommand::Snapshot).await.unwrap() {
        CommandOk::Snapshot(s) => s,
        other => panic!("{other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_snapshot_carries_the_modem_detail() {
    // A UI that resyncs after the attach has finished still learns why the
    // modem is in its state (a fault's reason, "simulator").
    let dir = tempfile::tempdir().unwrap();
    let (h, id) = sim_app(dir.path()).await;
    let mut rx = h.subscribe();
    h.call(AppCommand::SelectProfile { id: Some(id) }).await.unwrap();
    while !matches!(next(&mut rx).await.event, AppEvent::ModemState { state: ModemState::Ready, .. }) {}
    let s = snap(&h).await;
    assert_eq!((s.modem, s.modem_detail.as_deref()), (ModemState::Ready, Some("simulator")));

    let h = spawn_app(Store::open_in_memory().unwrap(), AppOptions::default());
    let id = match h.call(AppCommand::SaveProfile { profile: StationProfile::new_simulator("ZS6ABC") }).await.unwrap() {
        CommandOk::Profile(p) => p.id,
        other => panic!("{other:?}"),
    };
    h.call(AppCommand::SelectProfile { id: Some(id) }).await.unwrap();
    let s = snap(&h).await;
    assert_eq!(s.modem, ModemState::Faulted);
    assert!(s.modem_detail.as_deref().is_some_and(|d| d.contains("simulator")), "{:?}", s.modem_detail);
}

#[tokio::test(flavor = "multi_thread")]
async fn editing_the_active_profile_reattaches_its_modem() {
    // A corrected callsign must reach the modem, not wait for a re-selection.
    let dir = tempfile::tempdir().unwrap();
    let (h, id) = sim_app(dir.path()).await;
    let mut rx = h.subscribe();
    h.call(AppCommand::SelectProfile { id: Some(id.clone()) }).await.unwrap();
    let ready = loop {
        let e = next(&mut rx).await;
        if matches!(e.event, AppEvent::ModemState { state: ModemState::Ready, .. }) {
            break e;
        }
    };
    let mut p = snap(&h).await.profiles.into_iter().find(|p| p.id == id).unwrap();
    p.callsign = "ZS6ABD".into();
    h.call(AppCommand::SaveProfile { profile: p.clone() }).await.unwrap();
    let mut seen = Vec::new();
    loop {
        let e = next(&mut rx).await;
        seen.push(e.event.clone());
        if matches!(e.event, AppEvent::ModemState { state: ModemState::Ready, .. }) {
            assert!(e.generation > ready.generation, "a re-attach starts a new generation");
            break;
        }
    }
    assert!(seen.contains(&AppEvent::ModemState { state: ModemState::Attaching, detail: None }), "{seen:?}");

    // Saving it again unchanged (a name edit, say) does not re-attach.
    p.name = Some("Home".into());
    h.call(AppCommand::SaveProfile { profile: p }).await.unwrap();
    assert_eq!(next(&mut rx).await.event, AppEvent::ProfilesChanged);
    assert!(tokio::time::timeout(Duration::from_millis(300), rx.recv()).await.is_err(), "no re-attach for a name edit");
}
