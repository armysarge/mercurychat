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
    QsyWindows {
        answer: Duration::from_secs(20),
        recall: Duration::from_secs(12),
        grace: Duration::from_secs(4),
        agreed: Duration::from_secs(6),
    }
}

struct App {
    h: AppHandle,
    rx: Receiver<EventEnvelope>,
}

impl App {
    /// The first event matching `pred` within `secs`, if any.
    async fn event(&mut self, secs: u64, pred: impl Fn(&AppEvent) -> bool) -> Option<AppEvent> {
        let end = Instant::now() + Duration::from_secs(secs);
        loop {
            match tokio::time::timeout_at(end, self.rx.recv()).await {
                Ok(Ok(e)) if pred(&e.event) => return Some(e.event),
                Ok(Ok(_)) | Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => {}
                Ok(Err(e)) => panic!("{e}"),
                Err(_) => return None,
            }
        }
    }

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
    app_with(root, net, i, call, rig, None).await
}

async fn app_with(
    root: &Path,
    net: &FakeNet,
    i: usize,
    call: &str,
    rig: Option<(&FakeRigctld, i32)>,
    telemetry_url: Option<String>,
) -> App {
    let (base, bcast) = if i == 0 { (net.a_base, net.a_bcast) } else { (net.b_base, net.b_bcast) };
    let mut p = StationProfile::new_existing(call);
    p.modem = ModemTarget::Existing {
        host: "127.0.0.1".into(),
        control_port: base,
        data_port: base + 1,
        broadcast_port: Some(bcast),
        telemetry_url,
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
    let a = App { h, rx };
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
    let s = t.a.wait(25, "the re-call", |s| s.session.as_ref().is_some_and(|x| x.session_id != first)).await;
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
    wait_freq(&t.ra, START_HZ, 30, "A back after its window").await;
    wait_freq(t.rb.as_ref().unwrap(), START_HZ, 30, "B back after its window").await;
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
    t.a.wait(25, "the re-call", |s| s.session.is_some()).await;
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

// --- Task 10 (B5): QSY suggestions before a connection (MCB/1 kind 3) ---

use mch_protocol::beacon::{self as mcb, Mode, QsySuggestion, Received};

const OTHER: &str = "ZS1XYZ";

fn suggestion(from: &str, to: &str) -> Vec<u8> {
    QsySuggestion::new(7, from, to, vec![(NEW_HZ as u32, Mode::Usb)]).unwrap().encode()
}

fn is_suggested(e: &AppEvent) -> bool {
    matches!(e, AppEvent::QsySuggested { .. })
}

fn kind3_sent_by(net: &FakeNet, side: usize) -> usize {
    net.broadcast_log().iter().filter(|(s, p)| *s == side && matches!(mcb::classify(p), Received::Qsy(_))).count()
}

#[tokio::test(flavor = "multi_thread")]
async fn suggestion_reaches_addressee_only() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let a = app(dir.path(), &net, 0, A, None).await;
    let mut b = app(dir.path(), &net, 1, B, None).await;
    let id = a.entry("40m chat", NEW_HZ).await;
    a.h.call(AppCommand::SuggestQsy { to: B.into(), entry_ids: vec![id] }).await.unwrap();
    match b.event(10, is_suggested).await {
        Some(AppEvent::QsySuggested { from, candidates }) => {
            assert_eq!(from, A);
            assert_eq!((candidates[0].freq_hz, candidates[0].mode.as_str()), (NEW_HZ, "USB"));
        }
        other => panic!("B was not shown the suggestion: {other:?}"),
    }
    assert_eq!(kind3_sent_by(&net, 0), 1);
    // One addressed to a third station is not shown to B.
    assert!(net.air_broadcast(0, suggestion(OTHER, "ZS6OTHR")));
    assert!(b.event(2, is_suggested).await.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn suggestion_counts_as_heard_for_others() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let mut a = app(dir.path(), &net, 0, A, None).await;
    assert!(net.air_broadcast(1, suggestion(OTHER, "ZS6OTHR")));
    let heard = a.event(10, |e| matches!(e, AppEvent::Heard { station } if station.callsign == OTHER)).await;
    assert!(heard.is_some(), "the sender is heard");
    assert!(a.event(1, is_suggested).await.is_none(), "not addressed to A");
}

fn status_on_mode(listen_mode: u8) -> String {
    format!(
        r#"{{"type":"status","bitrate":0,"snr":0.0,"direction":"rx","tx_gain_db":0.0,"audio_ok":true,"audio_error":"","arq_tx_mode":"","arq_rx_mode":"","radio_frequency_hz":null,"radio_frequency_age_ms":null,"freq_offset_hz":0.0,"freq_offset_age_ms":0,"listen_mode":{listen_mode},"busy":""}}"#
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn suggestion_over_capacity_refused() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let ws = mercury_adapter::fake_ws::FakeWs::start().await;
    ws.set_status(Some(&status_on_mode(7))); // DATAC15: 27 bytes
    let a = app_with(dir.path(), &net, 0, A, None, Some(ws.url.clone())).await;
    a.wait(10, "DATAC15 from telemetry", |s| s.telemetry.listen_mode == Some(7)).await;
    let id = a.entry("40m chat", NEW_HZ).await;
    let e = a.h.call(AppCommand::SuggestQsy { to: B.into(), entry_ids: vec![id] }).await.unwrap_err();
    assert!(format!("{e:?}").contains("27"), "the refusal names the capacity: {e:?}");
    assert_eq!(kind3_sent_by(&net, 0), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn suggestion_tune_refused_while_busy() {
    let dir = tempfile::tempdir().unwrap();
    let t = two(dir.path(), 0, true).await;
    let mut a = t.a;
    assert!(t.net.air_broadcast(1, suggestion(OTHER, A)));
    assert!(a.event(10, is_suggested).await.is_some());
    connect(&a, &t.b).await;
    let e = a.h.call(AppCommand::TuneSuggested { index: 0 }).await.unwrap_err();
    assert!(matches!(&e, AppError::Refused { reason: "rig.refused.connected", .. }), "{e:?}");
    assert_eq!(t.ra.freq(), START_HZ);
    a.h.call(AppCommand::Disconnect).await.unwrap();
    a.wait(10, "idle", |s| s.session.is_none() && matches!(s.modem, ModemState::Listening | ModemState::Ready)).await;
    // Mercury keys the teardown frame after DISCONNECTED: retry until the gate allows it.
    let end = Instant::now() + Duration::from_secs(5);
    while a.h.call(AppCommand::TuneSuggested { index: 0 }).await.is_err() {
        assert!(Instant::now() < end, "Tune and listen still refused once idle");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    wait_freq(&t.ra, NEW_HZ, 10, "tuned to the suggestion").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn suggestion_never_sent_automatically() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let mut a = app(dir.path(), &net, 0, A, None).await;
    a.h.call(AppCommand::SetBeacon { on: true }).await.unwrap();
    a.h.call(AppCommand::SendBeaconNow).await.unwrap();
    assert!(net.air_broadcast(1, suggestion(OTHER, A)));
    assert!(a.event(10, is_suggested).await.is_some());
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(net.broadcast_log().iter().any(|(s, p)| *s == 0 && matches!(mcb::classify(p), Received::Beacon(_))));
    assert_eq!(kind3_sent_by(&net, 0), 0, "no kind 3 without the command");
}

// --- Final review: the real rig gate through the service, and the manual path's dial ---

/// B's answer is refused by its own gate: `decline rig` to A, no disconnect, no rig change.
async fn declined_by_bs_rig(t: &mut Two) {
    let before = [t.a.snap().await.session.map(|x| x.session_id), t.b.snap().await.session.map(|x| x.session_id)];
    let id = t.a.entry("40m chat", NEW_HZ).await;
    t.a.h.call(AppCommand::ProposeQsy { entry_ids: vec![id], note: None }).await.unwrap();
    t.b.wait(10, "B's offer", |s| stage(s) == Some("offered")).await;
    let e = t.b.h.call(AppCommand::AnswerQsy { accept: Some(0) }).await.unwrap_err();
    assert!(matches!(&e, AppError::Refused { reason: "qsy.refused.rig", .. }), "{e:?}");
    let told = t.a.event(10, |e| matches!(e, AppEvent::QsyNotice { key, .. } if key == "qsy.notify.declinedRig")).await;
    assert!(told.is_some(), "A is told B's rig declined");
    t.a.wait(5, "A's QSY over", |s| s.qsy.is_none()).await;
    assert!(t.b.snap().await.qsy.is_none());
    tokio::time::sleep(Duration::from_secs(2)).await;
    for (app, was) in [&t.a, &t.b].into_iter().zip(before) {
        assert!(was.is_some());
        assert_eq!(app.snap().await.session.map(|x| x.session_id), was, "the session goes on");
    }
    assert_eq!(t.ra.freq(), START_HZ);
    assert_eq!(t.rb.as_ref().unwrap().freq(), START_HZ);
}

#[tokio::test(flavor = "multi_thread")]
async fn gate_offset_below_zero_declines_rig() {
    let dir = tempfile::tempdir().unwrap();
    // B's dial would be 7 105 000 - 8 000 000 < 0.
    let mut t = two(dir.path(), 8_000_000, true).await;
    connect(&t.a, &t.b).await;
    declined_by_bs_rig(&mut t).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn gate_ptt_unknown_declines_rig() {
    let dir = tempfile::tempdir().unwrap();
    let mut t = two(dir.path(), 0, true).await;
    connect(&t.a, &t.b).await;
    // rigctld can't read PTT (as without -P): the link stays up, PTT unknown.
    t.rb.as_ref().unwrap().ptt_error(Some(-11));
    t.b.wait(10, "B's PTT unknown", |s| s.rig.link == RigLinkView::Up && s.rig.ptt == RigPttView::Unknown).await;
    declined_by_bs_rig(&mut t).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn link_down_manual_path_shows_the_dial() {
    let dir = tempfile::tempdir().unwrap();
    let t = two(dir.path(), 1500, true).await;
    connect(&t.a, &t.b).await;
    // B's rigctld goes away: no rig link, so B accepts and tunes by hand.
    t.rb.as_ref().unwrap().kill();
    t.b.wait(10, "B's link down", |s| s.rig.link != RigLinkView::Up).await;
    propose_and_accept(&t.a, &t.b).await;
    let s = t.b.wait(10, "B's manual path", |s| s.qsy.as_ref().is_some_and(|q| q.manual.is_some())).await;
    let manual = s.qsy.unwrap().manual.unwrap();
    assert_eq!(manual.freq_hz, NEW_HZ - 1500, "the dial: the candidate less B's own offset");
}

// --- M4: the deferred M3c minors through the service ---

fn notice(e: &AppEvent, want: &str) -> bool {
    matches!(e, AppEvent::QsyNotice { key, .. } if key == want)
}

#[tokio::test(flavor = "multi_thread")]
async fn tune_refused_while_qsy_moving() {
    let dir = tempfile::tempdir().unwrap();
    let t = two(dir.path(), 0, true).await;
    let other = t.a.entry("30m elsewhere", 10_140_000).await;
    connect(&t.a, &t.b).await;
    t.b.h.call(AppCommand::SetReceiveOnly { on: true }).await.unwrap();
    propose_and_accept(&t.a, &t.b).await;
    t.a.wait(10, "A moving", |s| stage(s) == Some("moving") && s.session.is_none()).await;
    wait_freq(&t.ra, NEW_HZ, 10, "A retuned").await;
    let e = t.a.h.call(AppCommand::TuneToEntry { id: other }).await.unwrap_err();
    assert!(matches!(&e, AppError::Refused { reason: "qsy.refused.tuneDuringQsy", .. }), "{e:?}");
    assert!(t.net.air_broadcast(1, suggestion(OTHER, A)));
    tokio::time::sleep(Duration::from_millis(500)).await;
    let e = t.a.h.call(AppCommand::TuneSuggested { index: 0 }).await.unwrap_err();
    assert!(matches!(&e, AppError::Refused { reason: "qsy.refused.tuneDuringQsy", .. }), "{e:?}");
    assert_eq!(t.ra.freq(), NEW_HZ, "nothing moved the rig");
}

#[tokio::test(flavor = "multi_thread")]
async fn abandoned_qsy_keeps_the_session() {
    let dir = tempfile::tempdir().unwrap();
    let mut t = two(dir.path(), 0, true).await;
    let session = connect(&t.a, &t.b).await;
    // A's DISCONNECT is acknowledged but the link stays up.
    t.net.set_hold_disconnect(true);
    propose_and_accept(&t.a, &t.b).await;
    assert!(t.a.event(windows().agreed.as_secs() + 10, |e| notice(e, "qsy.notify.abandoned")).await.is_some());
    assert!(t.b.event(10, |e| notice(e, "qsy.notify.abandoned")).await.is_some());
    t.a.wait(5, "A's QSY over", |s| s.qsy.is_none()).await;
    t.b.wait(5, "B's QSY over", |s| s.qsy.is_none()).await;
    let s = t.a.snap().await;
    assert_eq!(s.session.map(|x| x.session_id), Some(session), "the session goes on");
    assert_eq!((t.ra.freq(), t.rb.as_ref().unwrap().freq()), (START_HZ, START_HZ));
}

#[tokio::test(flavor = "multi_thread")]
async fn no_reading_fallback_says_return_by_hand() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let rb = FakeRigctld::start().await;
    // A has no rig and no telemetry: its dial is unknown.
    let mut a = app(dir.path(), &net, 0, A, None).await;
    let b = app(dir.path(), &net, 1, B, Some((&rb, 0))).await;
    connect(&a, &b).await;
    b.h.call(AppCommand::SetReceiveOnly { on: true }).await.unwrap();
    propose_and_accept(&a, &b).await;
    let secs = (windows().recall + windows().agreed).as_secs() + 15;
    assert!(a.event(secs, |e| notice(e, "qsy.notify.returnByHand")).await.is_some());
    a.wait(5, "A's QSY over", |s| s.qsy.is_none()).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn no_beacon_during_qsy_move() {
    let dir = tempfile::tempdir().unwrap();
    let t = two(dir.path(), 0, true).await;
    t.a.h.call(AppCommand::SetBeacon { on: true }).await.unwrap();
    connect(&t.a, &t.b).await;
    t.b.h.call(AppCommand::SetReceiveOnly { on: true }).await.unwrap();
    propose_and_accept(&t.a, &t.b).await;
    t.a.wait(10, "A moving", |s| stage(s) == Some("moving") && s.session.is_none()).await;
    let before = t.net.broadcast_log().iter().filter(|(s, _)| *s == 0).count();
    let e = t.a.h.call(AppCommand::SendBeaconNow).await.unwrap_err();
    assert!(matches!(&e, AppError::Refused { reason: "qsy.refused.tuneDuringQsy", .. }), "{e:?}");
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(t.net.broadcast_log().iter().filter(|(s, _)| *s == 0).count(), before, "nothing keyed");
}
