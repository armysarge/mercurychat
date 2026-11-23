//! The first-run wizard (M5 §6): its progress is kept by the core, so a
//! restart resumes it, and a station it is still setting up never answers a
//! call (Review Focus 4): Receive only holds until the "How you operate" step
//! is answered.

use mc_core::app::{
    spawn_app, AppCommand, AppError, AppEvent, AppHandle, AppOptions, CommandOk, EventEnvelope, ModemState,
    WizardState, WizardStep,
};
use mc_core::profile::{from_row, to_row, ModemTarget, StationProfile};
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

async fn until(rx: &mut Receiver<EventEnvelope>, want: ModemState) {
    loop {
        let ev = tokio::time::timeout(Duration::from_secs(60), rx.recv()).await.expect("a modem state").unwrap();
        if let AppEvent::ModemState { state, detail } = ev.event {
            if state == want {
                return;
            }
            assert_ne!(state, ModemState::Faulted, "faulted: {detail:?}");
        }
    }
}

async fn snapshot(h: &AppHandle) -> mc_core::app::AppSnapshot {
    match h.call(AppCommand::Snapshot).await.unwrap() {
        CommandOk::Snapshot(s) => s,
        other => panic!("{other:?}"),
    }
}

async fn wizard(h: &AppHandle) -> WizardState {
    match h.call(AppCommand::GetWizard).await.unwrap() {
        CommandOk::Wizard(w) => w,
        other => panic!("{other:?}"),
    }
}

async fn set_wizard(h: &AppHandle, state: WizardState) -> WizardState {
    match h.call(AppCommand::SetWizard { state }).await.unwrap() {
        CommandOk::Wizard(w) => w,
        other => panic!("{other:?}"),
    }
}

fn at(step: WizardStep, draft: Option<String>) -> WizardState {
    WizardState { step, draft_profile_id: draft, ..WizardState::default() }
}

fn listen_ons(net: &FakeNet) -> usize {
    net.log().iter().filter(|l| l.as_str() == "A> LISTEN ON").count()
}

#[tokio::test(flavor = "multi_thread")]
async fn wizard_resumes_and_nothing_transmits() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("mercurychat.db");
    let net = FakeNet::start().await;
    let id = {
        let h = spawn_app(Store::open(&db).unwrap(), opts(dir.path()));
        assert_eq!(wizard(&h).await, WizardState::default(), "a new install starts at Welcome");
        // A draft that says "answering": the wizard holds it in Receive only.
        let id = match h.call(AppCommand::SaveProfile { profile: existing(net.a_base) }).await.unwrap() {
            CommandOk::Profile(p) => p.id,
            other => panic!("{other:?}"),
        };
        let w = set_wizard(&h, at(WizardStep::Rig, Some(id.clone()))).await;
        assert_eq!(w.step, WizardStep::Rig);
        let mut rx = h.subscribe();
        h.call(AppCommand::SelectProfile { id: Some(id.clone()) }).await.unwrap();
        until(&mut rx, ModemState::Ready).await;
        assert!(snapshot(&h).await.receive_only);
        id
    }; // the app stops halfway through the wizard

    // Even a draft found answering at start is held (the app was stopped
    // between saving the profile and saving the wizard's progress).
    {
        let mut store = Store::open(&db).unwrap();
        let row = store.profiles().unwrap().into_iter().find(|r| r.id == id).unwrap();
        let mut p = from_row(&row).unwrap();
        p.receive_only = false;
        store.save_profile(&to_row(&p)).unwrap();
    }

    let h = spawn_app(Store::open(&db).unwrap(), opts(dir.path()));
    let mut rx = h.subscribe();
    until(&mut rx, ModemState::Ready).await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(wizard(&h).await.step, WizardStep::Rig, "the wizard resumes where it stopped");
    let s = snapshot(&h).await;
    assert!(s.receive_only && !s.listening, "a half-made station must not answer calls");
    assert_eq!(listen_ons(&net), 0, "LISTEN ON reached the modem during the wizard");

    match h.call(AppCommand::SetReceiveOnly { on: false }).await {
        Err(AppError::Refused { reason, .. }) => assert_eq!(reason, "wizard.receiveOnly"),
        other => panic!("answering calls before the operate step: {other:?}"),
    }

    // The operate step answered: the wizard moves on, and the choice is the operator's.
    set_wizard(&h, at(WizardStep::Tour, Some(id))).await;
    h.call(AppCommand::SetReceiveOnly { on: false }).await.unwrap();
    until(&mut rx, ModemState::Listening).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn skipped_steps_are_listed_in_settings() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("mercurychat.db");
    {
        let h = spawn_app(Store::open(&db).unwrap(), opts(dir.path()));
        let state = WizardState {
            step: WizardStep::Done,
            done: true,
            skipped: vec![WizardStep::Rig, WizardStep::Audio, WizardStep::Rig],
            ..WizardState::default()
        };
        let w = set_wizard(&h, state).await;
        assert_eq!(w.skipped, [WizardStep::Audio, WizardStep::Rig], "in step order, once each");
    }
    let h = spawn_app(Store::open(&db).unwrap(), opts(dir.path()));
    let w = wizard(&h).await;
    assert!(w.done);
    assert_eq!(w.skipped, [WizardStep::Audio, WizardStep::Rig]);
}

/// An install from before the wizard has stations: it is not asked again.
#[tokio::test(flavor = "multi_thread")]
async fn an_install_with_stations_reads_as_done() {
    let dir = tempfile::tempdir().unwrap();
    let h = spawn_app(Store::open_in_memory().unwrap(), opts(dir.path()));
    h.call(AppCommand::SaveProfile { profile: existing(9) }).await.unwrap();
    let w = wizard(&h).await;
    assert!(w.done && w.skipped.is_empty(), "{w:?}");
}

/// SET-03: My own Mercury's endpoints are checked one by one, read-only (a
/// connect, then closed): each is named, with what went wrong in words.
#[tokio::test(flavor = "multi_thread")]
async fn my_own_mercury_endpoints_are_checked_one_by_one() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let h = spawn_app(Store::open_in_memory().unwrap(), opts(dir.path()));
    let modem = ModemTarget::Existing {
        host: "127.0.0.1".into(),
        control_port: net.a_base,
        data_port: net.a_base + 1,
        broadcast_port: Some(closed),
        telemetry_url: Some(format!("ws://127.0.0.1:{closed}/websocket")),
    };
    let results = match h.call(AppCommand::CheckEndpoints { modem }).await.unwrap() {
        CommandOk::Endpoints { results } => results,
        other => panic!("{other:?}"),
    };
    let names: Vec<_> = results.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, ["control", "data", "broadcast", "telemetry"]);
    assert!(results[0].ok && results[1].ok, "{results:?}");
    assert!(!results[2].ok && !results[3].ok, "{results:?}");
    assert_eq!(results[2].address, format!("127.0.0.1:{closed}"));
    assert!(results[2].detail.contains("Nothing answers"), "{results:?}");
    assert!(snapshot(&h).await.active_profile_id.is_none(), "a check saves and selects nothing");
}

/// The wizard's rig step lists the serial ports from the system's records
/// (nothing is opened): the same list the adapter gives, in its order.
#[tokio::test(flavor = "multi_thread")]
async fn serial_ports_come_from_the_system() {
    let dir = tempfile::tempdir().unwrap();
    let h = spawn_app(Store::open_in_memory().unwrap(), opts(dir.path()));
    match h.call(AppCommand::SerialPorts).await.unwrap() {
        CommandOk::SerialPorts { ports } => assert_eq!(ports, mercury_adapter::serial::ports()),
        other => panic!("{other:?}"),
    }
}
