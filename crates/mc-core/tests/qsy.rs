//! M3c Task 9 (B3, B4): QSY during a session, through the service. Two apps
//! over the fake Mercury pair, each with its own fake rigctld.
#![cfg(feature = "sim")]

use mc_core::app::qsy::{QsyView, QsyWindows};
use mc_core::app::{
    spawn_app, AppCommand, AppError, AppEvent, AppHandle, AppOptions, AppSnapshot, CommandOk, EventEnvelope,
    FrequencyEntry, ModemState, RigConfig, RigLinkView, RigPttView,
};
use mc_core::profile::{ModemTarget, StationProfile};
use mc_core::station::{Station, StationConfig};
use mc_core::SessionCommand;
use mc_storage::Store;
use mercury_adapter::fake::FakeNet;
use mercury_adapter::fake_rig::FakeRigctld;
use mercury_adapter::{Callsign, Endpoint};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast::Receiver;
use tokio::sync::mpsc;
use tokio::time::Instant;

const A: &str = "ZS6ABC";
const B: &str = "ZS6PEER";
/// The candidate every test proposes (a displayed frequency).
const NEW_HZ: u64 = 7_105_000;
/// FakeRigctld's starting dial.
const START_HZ: u64 = 14_074_000;

fn windows() -> QsyWindows {
    QsyWindows { answer: Duration::from_secs(20), recall: Duration::from_secs(6), grace: Duration::from_secs(3) }
}

struct App {
    h: AppHandle,
    _rx: Receiver<EventEnvelope>,
}

impl App {
    async fn snap(&self) -> AppSnapshot {
        match self.h.call(AppCommand::Snapshot).await.unwrap() {
            CommandOk::Snapshot(s) => s,
            other => panic!("{other:?}"),
        }
    }

    async fn wait(&self, secs: u64, what: &str, pred: impl Fn(&AppSnapshot) -> bool) -> AppSnapshot {
        let end = Instant::now() + Duration::from_secs(secs);
        loop {
            let s = self.snap().await;
            if pred(&s) {
                return s;
            }
            assert!(Instant::now() < end, "timed out waiting for {what}: qsy {:?}, modem {:?}", s.qsy, s.modem);
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    async fn entry(&self, name: &str, hz: u64) -> String {
        let entry = FrequencyEntry {
            name: name.into(),
            freq_hz: hz,
            mode: "USB".into(),
            passband_hz: Some(2400),
            purpose: vec!["chat".into()],
            ..FrequencyEntry::default()
        };
        match self.h.call(AppCommand::SaveFrequency { entry }).await.unwrap() {
            CommandOk::Frequencies { entries } => entries.into_iter().find(|e| e.name == name).unwrap().id,
            other => panic!("{other:?}"),
        }
    }
}

fn stage(s: &AppSnapshot) -> Option<&str> {
    s.qsy.as_ref().map(|q: &QsyView| q.stage.as_str())
}

/// An app on station `i` of `net`, listening, with `rig` (and `offset_hz`) if given.
async fn app(root: &Path, net: &FakeNet, i: usize, call: &str, rig: Option<(&FakeRigctld, i32)>) -> App {
    let base = if i == 0 { net.a_base } else { net.b_base };
    let mut p = StationProfile::new_existing(call);
    p.modem = ModemTarget::Existing {
        host: "127.0.0.1".into(),
        control_port: base,
        data_port: base + 1,
        broadcast_port: None,
        telemetry_url: None,
    };
    let opts = AppOptions {
        sim_root: Some(root.join(call)),
        stations_root: Some(root.join(call).join("stations")),
        qsy_windows: Some(windows()),
        ..AppOptions::default()
    };
    let h = spawn_app(Store::open_in_memory().unwrap(), opts);
    let id = match h.call(AppCommand::SaveProfile { profile: p }).await.unwrap() {
        CommandOk::Profile(p) => p.id,
        other => panic!("{other:?}"),
    };
    let mut rx = h.subscribe();
    h.call(AppCommand::SelectProfile { id: Some(id) }).await.unwrap();
    loop {
        let ev = tokio::time::timeout(Duration::from_secs(15), rx.recv()).await.expect("listening").unwrap();
        if let AppEvent::ModemState { state: ModemState::Listening, .. } = ev.event {
            break;
        }
    }
    let a = App { h, _rx: rx };
    if let Some((fake, offset_hz)) = rig {
        let config = RigConfig { offset_hz, ..RigConfig::external("127.0.0.1", fake.addr.port()) };
        assert_eq!(a.h.call(AppCommand::SetRigConfig { config }).await.unwrap(), CommandOk::Done);
        a.wait(5, "the rig link", |s| s.rig.link == RigLinkView::Up && s.rig.ptt != RigPttView::Unknown).await;
    }
    a
}

/// A calls B; both see the session, with qsy/1 negotiated.
async fn connect(a: &App, b: &App) -> String {
    a.h.call(AppCommand::Call { peer: B.into() }).await.unwrap();
    let s = a.wait(15, "A's session", |s| s.session.is_some()).await;
    b.wait(15, "B's session", |s| s.session.is_some()).await;
    let session = s.session.unwrap();
    assert!(session.caps.iter().any(|c| c == "qsy/1"), "{:?}", session.caps);
    session.session_id
}

/// A proposes the entry; B accepts candidate 0.
async fn propose_and_accept(a: &App, b: &App) {
    let id = a.entry("40m chat", NEW_HZ).await;
    a.h.call(AppCommand::ProposeQsy { entry_ids: vec![id], note: None }).await.unwrap();
    b.wait(10, "B's offer", |s| stage(s) == Some("offered")).await;
    b.h.call(AppCommand::AnswerQsy { accept: Some(0) }).await.unwrap();
}

struct Two {
    net: Arc<FakeNet>,
    a: App,
    b: App,
    ra: FakeRigctld,
    rb: Option<FakeRigctld>,
}

async fn two(root: &Path, offset_b: i32, b_rig: bool) -> Two {
    let net = FakeNet::start().await;
    let ra = FakeRigctld::start().await;
    let rb = if b_rig { Some(FakeRigctld::start().await) } else { None };
    let a = app(root, &net, 0, A, Some((&ra, 0))).await;
    let b = app(root, &net, 1, B, rb.as_ref().map(|r| (r, offset_b))).await;
    Two { net, a, b, ra, rb }
}

async fn wait_freq(rig: &FakeRigctld, hz: u64, secs: u64, what: &str) {
    let end = Instant::now() + Duration::from_secs(secs);
    while rig.freq() != hz {
        assert!(Instant::now() < end, "{what}: the rig is on {} Hz, not {hz}", rig.freq());
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn qsy_end_to_end() {
    let dir = tempfile::tempdir().unwrap();
    let t = two(dir.path(), 0, true).await;
    let first = connect(&t.a, &t.b).await;
    let first_qso = t.a.snap().await.current_qso.unwrap().id;
    propose_and_accept(&t.a, &t.b).await;
    wait_freq(&t.ra, NEW_HZ, 10, "A retuned").await;
    wait_freq(t.rb.as_ref().unwrap(), NEW_HZ, 10, "B retuned").await;
    assert_eq!(t.ra.mode().0, "USB");
    // A calls again on the new frequency; the new session is a new QSO linked to the first.
    let s = t.a.wait(15, "the re-call", |s| s.session.as_ref().is_some_and(|x| x.session_id != first)).await;
    assert_eq!(s.session.unwrap().bandwidth, t.b.snap().await.session.unwrap().bandwidth);
    let qso = t.a.wait(5, "the new QSO", |s| s.current_qso.as_ref().is_some_and(|q| q.id != first_qso)).await;
    assert_eq!(qso.current_qso.unwrap().notes.as_deref(), Some(format!("QSY from {first_qso}").as_str()));
    t.a.wait(5, "A's QSY done", |s| s.qsy.is_none()).await;
    t.b.wait(5, "B's QSY done", |s| s.qsy.is_none()).await;
    // Well past both windows: nobody went back.
    tokio::time::sleep(windows().recall + windows().grace + Duration::from_secs(1)).await;
    assert_eq!((t.ra.freq(), t.rb.as_ref().unwrap().freq()), (NEW_HZ, NEW_HZ));
}

#[tokio::test(flavor = "multi_thread")]
async fn qsy_offsets_applied_per_station() {
    let dir = tempfile::tempdir().unwrap();
    let t = two(dir.path(), 1500, true).await;
    connect(&t.a, &t.b).await;
    propose_and_accept(&t.a, &t.b).await;
    wait_freq(&t.ra, NEW_HZ, 10, "A (offset 0)").await;
    wait_freq(t.rb.as_ref().unwrap(), NEW_HZ - 1500, 10, "B (offset +1500)").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn qsy_fallback_restores_original() {
    let dir = tempfile::tempdir().unwrap();
    let t = two(dir.path(), 0, true).await;
    connect(&t.a, &t.b).await;
    // B stops answering once the session ends, so A's re-calls go unanswered.
    t.b.h.call(AppCommand::SetReceiveOnly { on: true }).await.unwrap();
    propose_and_accept(&t.a, &t.b).await;
    wait_freq(t.rb.as_ref().unwrap(), NEW_HZ, 10, "B retuned").await;
    wait_freq(&t.ra, NEW_HZ, 10, "A retuned").await;
    wait_freq(&t.ra, START_HZ, 20, "A back after its window").await;
    wait_freq(t.rb.as_ref().unwrap(), START_HZ, 20, "B back after its window").await;
    let a = t.a.wait(5, "A's QSY over", |s| s.qsy.is_none()).await;
    assert!(a.session.is_none() && a.modem != ModemState::Calling, "no automatic re-call: {:?}", a.modem);
    t.b.wait(5, "B's QSY over", |s| s.qsy.is_none()).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn qsy_refused_without_capability() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let a = app(dir.path(), &net, 0, A, None).await;
    // A minor-0 peer (no qsy/1) calls A.
    let mut cfg = StationConfig::defaults(
        Callsign::parse(B).unwrap(),
        Endpoint::base("127.0.0.1", net.b_base),
        &dir.path().join("peer"),
    );
    cfg.proto_minor = 0;
    cfg.capabilities = vec!["chat".into(), "file".into()];
    let mut peer = Station::open(cfg).unwrap();
    peer.attach().await.unwrap();
    let info = peer.call(&Callsign::parse(A).unwrap(), Duration::from_secs(10)).await.unwrap();
    let (_cmds, mut cmd_rx) = mpsc::channel::<SessionCommand>(4);
    let (ev_tx, _ev_rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        let (_req_tx, mut req_rx) = mpsc::channel(4);
        peer.run_session(&info, &mut cmd_rx, &mut req_rx, ev_tx).await
    });
    a.wait(10, "the session", |s| s.session.is_some()).await;
    let id = a.entry("40m chat", NEW_HZ).await;
    let e = a.h.call(AppCommand::ProposeQsy { entry_ids: vec![id], note: None }).await.unwrap_err();
    assert!(matches!(&e, AppError::Refused { reason: "qsy.refused.unsupported", .. }), "{e:?}");
    assert!(a.snap().await.qsy.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn qsy_no_rig_station_shows_manual() {
    let dir = tempfile::tempdir().unwrap();
    let t = two(dir.path(), 0, false).await;
    connect(&t.a, &t.b).await;
    propose_and_accept(&t.a, &t.b).await;
    let s = t.b.wait(10, "B's manual path", |s| s.qsy.as_ref().is_some_and(|q| q.manual.is_some())).await;
    let manual = s.qsy.unwrap().manual.unwrap();
    assert_eq!((manual.freq_hz, manual.mode.as_str()), (NEW_HZ, "USB"));
    wait_freq(&t.ra, NEW_HZ, 10, "A retuned").await;
    assert_eq!(t.b.h.call(AppCommand::QsyTuned).await.unwrap(), CommandOk::Done);
    // B still listens, and A's re-call finds it.
    t.a.wait(15, "the re-call", |s| s.session.is_some()).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn no_retune_while_keyed() {
    let dir = tempfile::tempdir().unwrap();
    let t = two(dir.path(), 0, true).await;
    connect(&t.a, &t.b).await;
    // B answers no re-call (the fake ignores frequency, so one would end B's QSY).
    t.b.h.call(AppCommand::SetReceiveOnly { on: true }).await.unwrap();
    // A's DISCONNECT does not end the link; B's Mercury is keyed when it does end.
    t.net.set_hold_disconnect(true);
    propose_and_accept(&t.a, &t.b).await;
    t.a.wait(10, "A agreed", |s| stage(s) == Some("agreed")).await;
    t.b.wait(10, "B agreed", |s| stage(s) == Some("agreed")).await;
    t.net.send_control(1, "PTT ON");
    tokio::time::sleep(Duration::from_millis(500)).await;
    t.net.drop_link();
    t.b.wait(10, "B moving", |s| stage(s) == Some("moving")).await;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(t.rb.as_ref().unwrap().freq(), START_HZ, "no retune while Mercury is keyed");
    t.net.send_control(1, "PTT OFF");
    wait_freq(t.rb.as_ref().unwrap(), NEW_HZ, 10, "B retuned once unkeyed").await;
}
