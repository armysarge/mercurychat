//! The application service: typed commands through a bounded queue, one
//! event stream, and a snapshot that never fails on stale settings.

use mc_core::app::{spawn_app, AppCommand, AppError, AppEvent, AppOptions, CommandOk, Since};
use mc_core::profile::{to_row, StationProfile};
use mc_storage::Store;
use std::time::Duration;

fn app(store: Store) -> mc_core::app::AppHandle {
    spawn_app(store, AppOptions::default())
}

async fn snapshot(h: &mc_core::app::AppHandle) -> mc_core::app::AppSnapshot {
    match h.call(AppCommand::Snapshot).await.unwrap() {
        CommandOk::Snapshot(s) => s,
        other => panic!("expected snapshot, got {other:?}"),
    }
}

/// The next event that is not a modem state change (selection drives the
/// modem, whose events are covered in tests/sim.rs).
async fn next_app_event(rx: &mut tokio::sync::broadcast::Receiver<mc_core::app::EventEnvelope>) -> AppEvent {
    loop {
        let ev = rx.recv().await.unwrap().event;
        if !matches!(ev, AppEvent::ModemState { .. }) {
            return ev;
        }
    }
}

async fn save(h: &mc_core::app::AppHandle, p: StationProfile) -> StationProfile {
    match h.call(AppCommand::SaveProfile { profile: p }).await.unwrap() {
        CommandOk::Profile(p) => p,
        other => panic!("expected profile, got {other:?}"),
    }
}

#[tokio::test]
async fn save_emits_and_snapshot_reflects() {
    let h = app(Store::open_in_memory().unwrap());
    let mut rx = h.subscribe();
    let saved = save(&h, StationProfile::new_existing(" zs6abc ")).await;
    assert_eq!(saved.callsign, "ZS6ABC");
    let ev = rx.recv().await.unwrap();
    assert_eq!((ev.seq, ev.event), (1, AppEvent::ProfilesChanged));
    let s = snapshot(&h).await;
    assert_eq!(s.seq, 1);
    assert_eq!(s.profiles, vec![saved]);
    assert_eq!(s.active_profile_id, None);
    assert_eq!(s.modem, mc_core::app::ModemState::Unavailable);
}

#[tokio::test]
async fn validate_returns_normalised_without_saving() {
    let h = app(Store::open_in_memory().unwrap());
    let r = h.call(AppCommand::ValidateProfile { profile: StationProfile::new_simulator("zs6abc") }).await.unwrap();
    assert!(matches!(r, CommandOk::Profile(p) if p.callsign == "ZS6ABC"));
    assert!(snapshot(&h).await.profiles.is_empty());
}

#[tokio::test]
async fn invalid_profile_returns_errors_and_emits_nothing() {
    let h = app(Store::open_in_memory().unwrap());
    let mut rx = h.subscribe();
    match h.call(AppCommand::SaveProfile { profile: StationProfile::new_existing("") }).await {
        Err(AppError::Validation { errors }) => assert_eq!(errors[0].field, "callsign"),
        other => panic!("expected validation error, got {other:?}"),
    }
    assert!(tokio::time::timeout(Duration::from_millis(50), rx.recv()).await.is_err(), "no event expected");
    assert!(snapshot(&h).await.profiles.is_empty());
}

#[tokio::test]
async fn select_and_deselect() {
    let h = app(Store::open_in_memory().unwrap());
    let p = save(&h, StationProfile::new_simulator("ZS6ABC")).await;
    let mut rx = h.subscribe();
    assert_eq!(h.call(AppCommand::SelectProfile { id: Some(p.id.clone()) }).await.unwrap(), CommandOk::Done);
    assert_eq!(next_app_event(&mut rx).await, AppEvent::ActiveProfileChanged { id: Some(p.id.clone()) });
    assert_eq!(snapshot(&h).await.active_profile_id, Some(p.id.clone()));
    h.call(AppCommand::SelectProfile { id: None }).await.unwrap();
    assert_eq!(next_app_event(&mut rx).await, AppEvent::ActiveProfileChanged { id: None });
    assert_eq!(snapshot(&h).await.active_profile_id, None);
}

#[tokio::test]
async fn select_unknown_profile_is_not_found() {
    let h = app(Store::open_in_memory().unwrap());
    let r = h.call(AppCommand::SelectProfile { id: Some("nope".into()) }).await;
    assert_eq!(r, Err(AppError::NotFound { id: "nope".into() }));
}

#[tokio::test]
async fn delete_unknown_profile_is_not_found() {
    let h = app(Store::open_in_memory().unwrap());
    let r = h.call(AppCommand::DeleteProfile { id: "nope".into() }).await;
    assert_eq!(r, Err(AppError::NotFound { id: "nope".into() }));
}

#[tokio::test]
async fn deleting_active_clears_selection() {
    let h = app(Store::open_in_memory().unwrap());
    let p = save(&h, StationProfile::new_simulator("ZS6ABC")).await;
    h.call(AppCommand::SelectProfile { id: Some(p.id.clone()) }).await.unwrap();
    let mut rx = h.subscribe();
    h.call(AppCommand::DeleteProfile { id: p.id.clone() }).await.unwrap();
    let got: Vec<AppEvent> = vec![rx.recv().await.unwrap().event, rx.recv().await.unwrap().event];
    assert!(got.contains(&AppEvent::ProfilesChanged), "{got:?}");
    assert!(got.contains(&AppEvent::ActiveProfileChanged { id: None }), "{got:?}");
    let s = snapshot(&h).await;
    assert!(s.profiles.is_empty());
    assert_eq!(s.active_profile_id, None);
}

#[tokio::test]
async fn dangling_active_id_reads_as_none() {
    let mut store = Store::open_in_memory().unwrap();
    store.save_profile(&to_row(&StationProfile::new_simulator("ZS6ABC").validate().unwrap())).unwrap();
    store.set_setting("activeProfileId", "gone").unwrap();
    let h = app(store);
    let s = snapshot(&h).await;
    assert_eq!(s.profiles.len(), 1);
    assert_eq!(s.active_profile_id, None);
}

#[tokio::test]
async fn events_since_follows_the_log() {
    let h = app(Store::open_in_memory().unwrap());
    save(&h, StationProfile::new_simulator("ZS6ABC")).await;
    save(&h, StationProfile::new_simulator("ZS6ABD")).await;
    match h.events_since(1).await.unwrap() {
        Since::Events { events } => assert_eq!(events.iter().map(|e| e.seq).collect::<Vec<_>>(), [2]),
        Since::SnapshotNeeded => panic!("ring holds both"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn full_queue_is_busy_not_blocking() {
    let h = spawn_app(Store::open_in_memory().unwrap(), AppOptions { command_queue: 1, ..AppOptions::default() });
    let calls =
        (0..50).map(|i| {
            let h = h.clone();
            async move {
                h.call(AppCommand::SaveProfile { profile: StationProfile::new_simulator(&format!("ZS{i}A")) }).await
            }
        });
    let results = tokio::time::timeout(Duration::from_secs(1), futures::future::join_all(calls)).await.unwrap();
    let busy = results.iter().filter(|r| matches!(r, Err(AppError::Busy { .. }))).count();
    let ok = results.iter().filter(|r| r.is_ok()).count();
    assert!(busy >= 1, "a full queue must refuse at once");
    assert_eq!(ok + busy, 50, "every call is either served or refused: {results:?}");
    let msg = results.iter().find_map(|r| r.as_ref().err()).unwrap().to_string();
    assert_eq!(msg, "too many pending requests");
}
