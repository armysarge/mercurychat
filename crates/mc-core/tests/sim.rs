//! M1 exit: the UI runs on simulated modem events, with no audio and no RF.
#![cfg(feature = "sim")]

use mc_core::app::{
    spawn_app, AppCommand, AppEvent, AppHandle, AppOptions, CommandOk, EventEnvelope, ModemState, OwnerCheck,
    RigChange, RigLinkView, RigPttView, RIG_SOURCE,
};
use mc_core::profile::StationProfile;
use mc_core::sim::SimModem;
use mc_storage::Store;
use std::time::Duration;
use tokio::sync::broadcast::Receiver;

/// The next event other than an activity line (those are covered in
/// tests/discovery.rs; these tests pin the state and profile sequence).
async fn next(rx: &mut Receiver<EventEnvelope>) -> EventEnvelope {
    loop {
        let env = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.expect("event within 5 s").unwrap();
        if !matches!(env.event, AppEvent::Activity { .. }) {
            return env;
        }
    }
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
    // While attaching, the modem is asked whether it can tune (M3b).
    let t = next(&mut rx).await;
    assert!(matches!(&t.event, AppEvent::Tune { tune } if tune.supported == Some(true)), "{:?}", t.event);
    let c = next(&mut rx).await;
    assert_eq!(c.event, AppEvent::ModemState { state: ModemState::Ready, detail: Some("simulator".into()) });
    assert!(a.generation >= 1 && b.generation == a.generation && c.generation == a.generation);
    // Listening is on by default (D18): it follows Ready at once.
    let d = next(&mut rx).await;
    assert_eq!(d.event, AppEvent::ModemState { state: ModemState::Listening, detail: None });
    assert_eq!(modem(&h).await, ModemState::Listening);
}

#[tokio::test(flavor = "multi_thread")]
async fn deselect_goes_unavailable() {
    let dir = tempfile::tempdir().unwrap();
    let (h, id) = sim_app(dir.path()).await;
    let mut rx = h.subscribe();
    h.call(AppCommand::SelectProfile { id: Some(id) }).await.unwrap();
    // Listening follows Ready at once (D18): wait for it, or it races the deselect's events.
    while !matches!(next(&mut rx).await.event, AppEvent::ModemState { state: ModemState::Listening, .. }) {}

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
    let sim = SimModem::start(&dir.path().join("station"), &dir.path().join("peer"), "ZS6ABC").await.unwrap();
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
    while modem(&h).await != ModemState::Listening {
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
    while !matches!(next(&mut rx).await.event, AppEvent::ModemState { state: ModemState::Listening, .. }) {}
    let s = snap(&h).await;
    assert_eq!((s.modem, s.modem_detail.as_deref()), (ModemState::Listening, None));

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
        if matches!(e.event, AppEvent::ModemState { state: ModemState::Listening, .. }) {
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
        if matches!(e.event, AppEvent::ModemState { state: ModemState::Listening, .. }) {
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

// ---- M3b Task 9: a fake rig behind the simulator ----

/// The simulator with its rig on, as the desktop app runs it, selected and listening.
async fn sim_with_rig(root: &std::path::Path) -> AppHandle {
    let h = spawn_app(
        Store::open_in_memory().unwrap(),
        AppOptions { sim_root: Some(root.to_path_buf()), sim_rig: true, ..AppOptions::default() },
    );
    let id = match h.call(AppCommand::SaveProfile { profile: StationProfile::new_simulator("ZS6ABC") }).await.unwrap() {
        CommandOk::Profile(p) => p.id,
        other => panic!("{other:?}"),
    };
    h.call(AppCommand::SelectProfile { id: Some(id) }).await.unwrap();
    wait_snap(&h, 15, "the simulator's rig", |s| {
        s.modem == ModemState::Listening && s.rig.link == RigLinkView::Up && s.rig.ptt == RigPttView::Off
    })
    .await;
    h
}

async fn wait_snap(
    h: &AppHandle,
    secs: u64,
    what: &str,
    pred: impl Fn(&mc_core::app::AppSnapshot) -> bool,
) -> mc_core::app::AppSnapshot {
    let end = tokio::time::Instant::now() + Duration::from_secs(secs);
    loop {
        let s = snap(h).await;
        if pred(&s) {
            return s;
        }
        assert!(tokio::time::Instant::now() < end, "timed out waiting for {what}: {:?} {:?}", s.modem, s.rig);
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn sim_has_a_working_rig() {
    let dir = tempfile::tempdir().unwrap();
    let h = sim_with_rig(dir.path()).await;
    let s = wait_snap(&h, 5, "a rig frequency", |s| s.rig.frequency.is_some()).await;
    assert_eq!(s.rig.frequency.as_ref().unwrap().source, RIG_SOURCE);
    assert!(matches!(s.rig.owner_check, OwnerCheck::Ok), "{:?}", s.rig.owner_check);
    assert_eq!(s.tune.supported, Some(true));
    h.call(AppCommand::RigSet { change: RigChange::Frequency { hz: 7_047_000 } }).await.unwrap();
    wait_snap(&h, 3, "the new frequency", |s| s.rig.frequency.as_ref().is_some_and(|f| f.value == 7_047_000)).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn sim_tune_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let h = sim_with_rig(dir.path()).await;
    h.call(AppCommand::Tune { on: true }).await.unwrap();
    wait_snap(&h, 3, "the rig keyed by Tune", |s| s.tune.active && s.rig.ptt == RigPttView::On).await;
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert_eq!(snap(&h).await.rig.fault, None, "the rig follows Mercury's PTT");
    h.call(AppCommand::Tune { on: false }).await.unwrap();
    wait_snap(&h, 3, "unkeyed", |s| !s.tune.active && s.rig.ptt == RigPttView::Off).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn sim_cq_keys_and_the_cross_check_agrees() {
    let dir = tempfile::tempdir().unwrap();
    let h = sim_with_rig(dir.path()).await;
    h.call(AppCommand::SendCq).await.unwrap();
    tokio::time::sleep(Duration::from_secs(2)).await;
    let s = snap(&h).await;
    assert_eq!(s.rig.fault, None);
    assert!(s.rig.banners.is_empty(), "{:?}", s.rig.banners);
}

#[tokio::test(flavor = "multi_thread")]
async fn sim_rig_settings_show_the_sim_rig_and_test_it() {
    let dir = tempfile::tempdir().unwrap();
    let h = sim_with_rig(dir.path()).await;
    let CommandOk::RigSettings { config, .. } = h.call(AppCommand::RigSettings).await.unwrap() else { panic!() };
    assert_eq!(config.freq_control, mc_core::app::FreqControl::ExternalRigctld);
    let CommandOk::RigTest { steps } = h.call(AppCommand::TestRig { config }).await.unwrap() else { panic!() };
    assert_eq!(steps.len(), 6);
    assert!(steps.iter().all(|s| s.ok == Some(true)), "{steps:?}");
}

// ---- M4 Task 10: files on the simulator -----------------------------------

use mc_core::files::{prepare, PhotoSize};
use mc_core::sim::SimPeer;
use mc_core::{SessionCommand, SessionEvent};
use mch_protocol::ObjectId;
use mercury_adapter::Callsign;
use tokio::sync::mpsc;

/// Our station on the simulator calls SIM1 and runs the session.
struct SimSession {
    _sim: tokio::task::JoinHandle<()>,
    cmds: mpsc::Sender<SessionCommand>,
    events: mpsc::UnboundedReceiver<SessionEvent>,
    dir: std::path::PathBuf,
}

impl SimSession {
    async fn start(dir: &std::path::Path, peer: SimPeer) -> (SimSession, Vec<String>) {
        let mut sim = SimModem::start_with(&dir.join("station"), &dir.join("peer"), "ZS6ABC", peer).await.unwrap();
        let info = sim.station.call(&Callsign::parse("SIM1").unwrap(), Duration::from_secs(10)).await.unwrap();
        let (cmds, mut cmd_rx) = mpsc::channel(16);
        let (ev_tx, mut events) = mpsc::unbounded_channel();
        let task = tokio::spawn(async move {
            let (_req_tx, mut req_rx) = mpsc::channel(4);
            sim.station.run_session(&info, &mut cmd_rx, &mut req_rx, ev_tx).await;
            drop(sim);
        });
        let caps = loop {
            match tokio::time::timeout(Duration::from_secs(10), events.recv()).await.expect("negotiated") {
                Some(SessionEvent::Negotiated { caps, .. }) => break caps,
                Some(_) => {}
                None => panic!("the session ended"),
            }
        };
        (SimSession { _sim: task, cmds, events, dir: dir.to_path_buf() }, caps)
    }

    async fn next(&mut self, secs: u64, what: &str, pred: impl Fn(&SessionEvent) -> bool) -> SessionEvent {
        let end = tokio::time::Instant::now() + Duration::from_secs(secs);
        loop {
            match tokio::time::timeout_at(end, self.events.recv()).await {
                Ok(Some(e)) if pred(&e) => return e,
                Ok(Some(_)) => {}
                Ok(None) => panic!("waiting for {what}: the session ended"),
                Err(_) => panic!("no {what} within {secs} s"),
            }
        }
    }

    async fn send_file(&self, name: &str, bytes: &[u8]) -> ObjectId {
        let src = self.dir.join(name);
        std::fs::write(&src, bytes).unwrap();
        let prepared = prepare(&src, PhotoSize::Original, &self.dir.join("tmp"), 1 << 20).unwrap();
        let id = ObjectId::random();
        self.cmds.send(SessionCommand::SendFile { id, prepared }).await.unwrap();
        id
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn sim_peer_accepts_a_file() {
    let dir = tempfile::tempdir().unwrap();
    let (mut s, _) = SimSession::start(dir.path(), SimPeer::default()).await;
    let t = tokio::time::Instant::now();
    let id = s.send_file("hello.txt", &b"hello SIM1, 73\n".repeat(50)).await;
    let done = s
        .next(15, "FileDelivered", |e| {
            matches!(
                e,
                SessionEvent::FileDelivered { .. }
                    | SessionEvent::FileRejected { .. }
                    | SessionEvent::FileFailed { .. }
            )
        })
        .await;
    assert_eq!(done, SessionEvent::FileDelivered { id });
    assert!(t.elapsed() >= Duration::from_secs(2), "SIM1 answers after about 2 s, like an operator");
    assert!(dir.path().join("peer").join("files").join("hello.txt").exists(), "SIM1 saved it");
}

#[tokio::test(flavor = "multi_thread")]
async fn sim_peer_offers_when_asked() {
    let dir = tempfile::tempdir().unwrap();
    let (mut s, _) = SimSession::start(dir.path(), SimPeer { offers: true, ..SimPeer::default() }).await;
    for (name, check) in [("notes.txt", 0usize), ("photo.jpg", 1)] {
        let o = s.next(20, "FileOffered", |e| matches!(e, SessionEvent::FileOffered { .. })).await;
        let SessionEvent::FileOffered { id, name: got, .. } = o else { unreachable!() };
        assert_eq!(got, name);
        s.cmds.send(SessionCommand::AcceptFile { id }).await.unwrap();
        let r = s
            .next(20, "FileReceived", |e| {
                matches!(e, SessionEvent::FileReceived { .. } | SessionEvent::FileFailed { .. })
            })
            .await;
        let SessionEvent::FileReceived { path, .. } = r else { panic!("{r:?}") };
        let bytes = std::fs::read(&path).unwrap();
        if check == 0 {
            assert!((1500..=2500).contains(&bytes.len()), "about 2 KB of text: {}", bytes.len());
            assert!(std::str::from_utf8(&bytes).is_ok());
        } else {
            let img = image::load_from_memory(&bytes).unwrap();
            assert_eq!(img.width().max(img.height()), 640, "a 640-px photo");
            assert_eq!(image::guess_format(&bytes).unwrap(), image::ImageFormat::Jpeg);
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn old_sim_peer_has_no_files() {
    let dir = tempfile::tempdir().unwrap();
    let (mut s, caps) = SimSession::start(dir.path(), SimPeer { minor0: true, ..SimPeer::default() }).await;
    assert!(!caps.iter().any(|c| c == "file/2"), "{caps:?}");
    let id = s.send_file("hello.txt", b"hello").await;
    let r = s
        .next(10, "refusal", |e| matches!(e, SessionEvent::FileRejected { .. } | SessionEvent::FileOffered { .. }))
        .await;
    assert_eq!(r, SessionEvent::FileRejected { id, reason: "not supported by this station".into() });
}
