//! RF-02: listening is an explicit, persistent preference, but a restart (or a
//! recovery) never arms it by itself. Checked against a fake Mercury whose
//! command log shows exactly what reached the modem.

use mc_core::app::{
    spawn_app, AppCommand, AppError, AppEvent, AppHandle, AppOptions, CommandOk, EventEnvelope, ModemState,
};
use mc_core::profile::{ModemTarget, StationProfile};
use mc_storage::Store;
use mercury_adapter::fake::FakeNet;
use std::path::Path;
use std::time::Duration;
use tokio::sync::broadcast::Receiver;

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
        let ev = tokio::time::timeout(Duration::from_secs(10), rx.recv()).await.expect("a modem state").unwrap();
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

fn listen_ons(net: &FakeNet) -> usize {
    net.log().iter().filter(|l| l.as_str() == "A> LISTEN ON").count()
}

#[tokio::test(flavor = "multi_thread")]
async fn set_listen_on_reaches_listening() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let h = spawn_app(Store::open_in_memory().unwrap(), opts(dir.path()));
    let id = match h.call(AppCommand::SaveProfile { profile: existing(net.a_base) }).await.unwrap() {
        CommandOk::Profile(p) => p.id,
        other => panic!("{other:?}"),
    };
    let mut rx = h.subscribe();
    h.call(AppCommand::SelectProfile { id: Some(id) }).await.unwrap();
    until(&mut rx, ModemState::Ready).await;
    assert!(!snapshot(&h).await.listen_preferred);

    assert_eq!(h.call(AppCommand::SetListen { on: true }).await.unwrap(), CommandOk::Done);
    until(&mut rx, ModemState::Listening).await;
    assert_eq!(listen_ons(&net), 1);
    assert!(snapshot(&h).await.listen_preferred);

    h.call(AppCommand::SetListen { on: false }).await.unwrap();
    until(&mut rx, ModemState::Ready).await;
    assert!(!snapshot(&h).await.listen_preferred);
}

#[tokio::test(flavor = "multi_thread")]
async fn preference_survives_restart_but_does_not_arm() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("mercurychat.db");
    let net = FakeNet::start().await;
    {
        let h = spawn_app(Store::open(&db).unwrap(), opts(dir.path()));
        let id = match h.call(AppCommand::SaveProfile { profile: existing(net.a_base) }).await.unwrap() {
            CommandOk::Profile(p) => p.id,
            other => panic!("{other:?}"),
        };
        let mut rx = h.subscribe();
        h.call(AppCommand::SelectProfile { id: Some(id) }).await.unwrap();
        until(&mut rx, ModemState::Ready).await;
        h.call(AppCommand::SetListen { on: true }).await.unwrap();
        until(&mut rx, ModemState::Listening).await;
    } // the app stops: the actor ends and its modem task is told to stop
    let before = listen_ons(&net);

    let h = spawn_app(Store::open(&db).unwrap(), opts(dir.path()));
    let mut rx = h.subscribe();
    let detail = until(&mut rx, ModemState::Ready).await;
    assert_eq!(detail.as_deref(), Some("listening is off until you turn it on"));
    let s = snapshot(&h).await;
    assert!(s.listen_preferred, "the preference is kept");
    assert_eq!(s.modem, ModemState::Ready, "but not armed");
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(listen_ons(&net), before, "a restart sent LISTEN ON by itself");
    assert!(net.log().iter().rev().any(|l| l == "A> LISTEN OFF"), "attach leaves the modem not listening");
}

#[tokio::test(flavor = "multi_thread")]
async fn set_listen_while_unavailable_is_conflict() {
    let h = spawn_app(Store::open_in_memory().unwrap(), AppOptions::default());
    match h.call(AppCommand::SetListen { on: true }).await {
        Err(AppError::Conflict { message }) => assert!(message.contains("unavailable"), "{message}"),
        other => panic!("expected Conflict, got {other:?}"),
    }
}
