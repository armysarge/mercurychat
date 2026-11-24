//! M3a Task 8: one draft QSO record per session (LOG-01), with the frequency's
//! provenance. The modem is the FakeNet pair (side A is the app's), telemetry
//! a fake Mercury WebSocket. A frequency that is not known is `None`, never 0.
#![cfg(feature = "sim")]

use mc_core::app::{
    spawn_app, AppCommand, AppError, AppEvent, AppHandle, AppOptions, AppSnapshot, CommandOk, EventEnvelope,
    ModemState, Qso, QsoEdit, TelemetryLinkView, TelemetryView,
};
use mc_core::profile::{ModemTarget, StationProfile};
use mc_core::station::{Station, StationConfig};
use mc_storage::Store;
use mch_protocol::beacon::{Beacon, CAP_CHAT};
use mercury_adapter::broadcast::BroadcastPort;
use mercury_adapter::fake::FakeNet;
use mercury_adapter::fake_ws::FakeWs;
use mercury_adapter::{Callsign, Endpoint};
use std::path::Path;
use std::time::Duration;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::broadcast::Receiver;
use tokio::sync::mpsc;
use tokio::time::Instant;

const PEER: &str = "SIM1";
const TELEMETRY: &str = "mercury telemetry";

const LIVE: &str = r#"{"type":"status","bitrate":1200,"snr":9.5,"direction":"rx","audio_ok":true,"audio_error":"",
  "arq_tx_mode":"DATAC3","arq_rx_mode":"DATAC1","radio_frequency_hz":14105000,"radio_frequency_age_ms":200,
  "peer_snr":-99.9,"peer_snr_valid":false,"peer_hears_mode":"","freq_offset_hz":12.5,"freq_offset_age_ms":100,
  "listen_mode":1,"listen_mode_name":"DATAC3","busy":""}"#;

fn opts(root: &Path) -> AppOptions {
    AppOptions {
        sim_root: Some(root.to_path_buf()),
        stations_root: Some(root.join("stations")),
        ..AppOptions::default()
    }
}

fn existing(net: &FakeNet, telemetry_url: Option<String>) -> StationProfile {
    let mut p = StationProfile::new_existing("ZS6ABC");
    p.modem = ModemTarget::Existing {
        host: "127.0.0.1".into(),
        control_port: net.a_base,
        data_port: net.a_base + 1,
        broadcast_port: Some(net.a_bcast),
        telemetry_url,
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
        if let AppEvent::ModemState { state: ModemState::Faulted, detail } = &ev {
            panic!("faulted: {detail:?}");
        }
        if let Some(t) = want(&ev) {
            return t;
        }
    }
}

async fn until_state(rx: &mut Receiver<EventEnvelope>, want: ModemState, secs: u64) {
    wait_for(rx, secs, |e| matches!(e, AppEvent::ModemState { state, .. } if *state == want).then_some(())).await
}

async fn telemetry(
    rx: &mut Receiver<EventEnvelope>,
    secs: u64,
    want: impl Fn(&TelemetryView) -> bool,
) -> TelemetryView {
    wait_for(rx, secs, |e| match e {
        AppEvent::Telemetry { telemetry } if want(telemetry) => Some(telemetry.clone()),
        _ => None,
    })
    .await
}

/// The next QSO event `want` accepts.
async fn qso(rx: &mut Receiver<EventEnvelope>, secs: u64, want: impl Fn(&Qso) -> bool) -> Qso {
    wait_for(rx, secs, |e| match e {
        AppEvent::Qso { qso } if want(qso) => Some(qso.clone()),
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

async fn qsos(h: &AppHandle) -> Vec<Qso> {
    match h.call(AppCommand::Qsos { limit: 50 }).await.unwrap() {
        CommandOk::Qsos { qsos } => qsos,
        other => panic!("{other:?}"),
    }
}

async fn start(root: &Path, profile: StationProfile) -> (AppHandle, Receiver<EventEnvelope>) {
    let h = spawn_app(Store::open_in_memory().unwrap(), opts(root));
    let mut rx = h.subscribe();
    let id = match h.call(AppCommand::SaveProfile { profile }).await.unwrap() {
        CommandOk::Profile(p) => p.id,
        other => panic!("{other:?}"),
    };
    h.call(AppCommand::SelectProfile { id: Some(id) }).await.unwrap();
    until_state(&mut rx, ModemState::Listening, 15).await;
    (h, rx)
}

fn spawn_peer(net: &FakeNet, dir: &Path) -> tokio::task::JoinHandle<()> {
    let call = Callsign::parse(PEER).unwrap();
    let mut st = Station::open(StationConfig::defaults(call, Endpoint::base("127.0.0.1", net.b_base), dir)).unwrap();
    tokio::spawn(async move {
        st.attach().await.unwrap();
        st.set_listen(true).await.unwrap();
        loop {
            let Ok(info) = st.wait_incoming(Duration::from_secs(3600)).await else { return };
            let (_cmd_tx, mut cmd_rx) = mpsc::channel(64);
            let (_req_tx, mut req_rx) = mpsc::channel(1);
            let (ev_tx, _ev_rx) = mpsc::unbounded_channel();
            let end = st.run_session(&info, &mut cmd_rx, &mut req_rx, ev_tx).await;
            st.end_link_after(&end, Duration::from_secs(10)).await.unwrap();
        }
    })
}

/// Call SIM1 and return the session's draft record.
async fn call(h: &AppHandle, rx: &mut Receiver<EventEnvelope>) -> Qso {
    h.call(AppCommand::Call { peer: PEER.into() }).await.unwrap();
    qso(rx, 20, |q| q.peer == PEER).await
}

/// Hear a beacon from `call` with a locator, so the heard list has its grid.
async fn hear_beacon(net: &FakeNet, rx: &mut Receiver<EventEnvelope>, call: &str) {
    let port = BroadcastPort::connect("127.0.0.1", net.b_bcast, Duration::from_secs(2)).await.unwrap().0;
    let payload = Beacon::new(7, call, Some("KG33xi"), CAP_CHAT).unwrap().encode();
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        port.send(&payload).await.unwrap();
        let got = tokio::time::timeout(
            Duration::from_secs(1),
            wait_for(rx, 5, |e| matches!(e, AppEvent::Heard { station } if station.callsign == call).then_some(())),
        )
        .await;
        if got.is_ok() {
            return;
        }
        assert!(Instant::now() < deadline, "A never heard a beacon");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn session_creates_one_draft_with_bandwidth_and_peer() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let _peer = spawn_peer(&net, &dir.path().join("peer"));
    let (h, mut rx) = start(dir.path(), existing(&net, None)).await;
    hear_beacon(&net, &mut rx, PEER).await;

    let q = call(&h, &mut rx).await;
    assert_eq!((q.peer.as_str(), q.state.as_str(), q.end_utc.as_ref()), (PEER, "draft", None));
    assert!(q.bandwidth.as_deref().is_some_and(|b| !b.is_empty()), "{q:?}");
    assert_eq!(q.grid.as_deref(), Some("KG33xi"), "the peer's grid comes from the heard list");
    assert!(q.session_id.is_some());

    // Later changes in the same session (an ack, a status) make no second draft.
    h.call(AppCommand::SendChat { peer: PEER.into(), text: "hi".into() }).await.unwrap();
    wait_for(&mut rx, 20, |e| match e {
        AppEvent::MessageChanged { message } if message.state == "delivered" => Some(()),
        _ => None,
    })
    .await;
    let all = qsos(&h).await;
    assert_eq!(all.len(), 1, "{all:?}");
    assert_eq!(snapshot(&h).await.current_qso.as_ref().map(|c| &c.id), Some(&q.id));
}

#[tokio::test(flavor = "multi_thread")]
async fn draft_takes_frequency_with_provenance_when_telemetry_has_it() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let ws = FakeWs::start().await;
    ws.set_status(Some(LIVE));
    let _peer = spawn_peer(&net, &dir.path().join("peer"));
    let (h, mut rx) = start(dir.path(), existing(&net, Some(ws.url.clone()))).await;
    telemetry(&mut rx, 10, |t| t.link == TelemetryLinkView::Up && t.frequency.is_some()).await;

    let q = call(&h, &mut rx).await;
    let f = q.freq.as_ref().expect("a fresh frequency is recorded");
    assert_eq!((f.value, f.source.as_str()), (14_105_000, TELEMETRY));
    assert!(f.age_ms.is_some_and(|a| (200..=10_000).contains(&a)), "{f:?}");
    assert_eq!(q.band.as_deref(), Some("20m"));
    // The stored record says the same.
    assert_eq!(qsos(&h).await[0].freq, q.freq);
}

#[tokio::test(flavor = "multi_thread")]
async fn draft_without_telemetry_has_no_frequency_not_zero() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let _peer = spawn_peer(&net, &dir.path().join("peer"));
    let (h, mut rx) = start(dir.path(), existing(&net, None)).await;
    let q = call(&h, &mut rx).await;
    assert_eq!((q.freq.as_ref(), q.band.as_ref()), (None, None), "{q:?}");

    // A frequency 15 s old is too old to say where the session was.
    let stale = LIVE.replace(r#""radio_frequency_age_ms":200"#, r#""radio_frequency_age_ms":15000"#);
    let dir2 = tempfile::tempdir().unwrap();
    let net2 = FakeNet::start().await;
    let ws = FakeWs::start().await;
    ws.set_status(Some(&stale));
    let _peer2 = spawn_peer(&net2, &dir2.path().join("peer"));
    let (h2, mut rx2) = start(dir2.path(), existing(&net2, Some(ws.url.clone()))).await;
    telemetry(&mut rx2, 10, |t| t.link == TelemetryLinkView::Up && t.frequency.is_some()).await;
    let q2 = call(&h2, &mut rx2).await;
    assert_eq!((q2.freq, q2.band), (None, None));
}

#[tokio::test(flavor = "multi_thread")]
async fn observed_modes_accumulate_without_duplicates() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let ws = FakeWs::start().await;
    ws.set_status(Some(LIVE));
    let _peer = spawn_peer(&net, &dir.path().join("peer"));
    let (h, mut rx) = start(dir.path(), existing(&net, Some(ws.url.clone()))).await;
    telemetry(&mut rx, 10, |t| t.link == TelemetryLinkView::Up && t.frequency.is_some()).await;
    let q = call(&h, &mut rx).await;
    assert_eq!((q.tx_modes.as_deref(), q.rx_modes.as_deref()), (Some("DATAC3"), Some("DATAC1")));

    // The link moves to DATAC1 and back to DATAC3: each mode is listed once.
    ws.set_status(Some(&LIVE.replace(r#""arq_tx_mode":"DATAC3""#, r#""arq_tx_mode":"DATAC1""#)));
    let q = qso(&mut rx, 10, |q| q.tx_modes.as_deref() == Some("DATAC3,DATAC1")).await;
    assert_eq!(q.rx_modes.as_deref(), Some("DATAC1"));
    ws.set_status(Some(LIVE));
    tokio::time::sleep(Duration::from_secs(2)).await;
    let stored = &qsos(&h).await[0];
    assert_eq!(stored.tx_modes.as_deref(), Some("DATAC3,DATAC1"), "{stored:?}");
    assert_eq!(stored.rx_modes.as_deref(), Some("DATAC1"));
}

#[tokio::test(flavor = "multi_thread")]
async fn disconnect_ends_but_does_not_finalise_by_default() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let _peer = spawn_peer(&net, &dir.path().join("peer"));
    let (h, mut rx) = start(dir.path(), existing(&net, None)).await;
    assert!(!snapshot(&h).await.finalise_on_disconnect, "R1: off by default");
    let q = call(&h, &mut rx).await;
    h.call(AppCommand::Disconnect).await.unwrap();
    let ended = qso(&mut rx, 30, |q| q.end_utc.is_some()).await;
    assert_eq!((ended.id.as_str(), ended.state.as_str()), (q.id.as_str(), "draft"));
    // Still editable afterwards.
    let edit = QsoEdit { name: Some("Sam".into()), ..Default::default() };
    match h.call(AppCommand::UpdateQso { id: q.id.clone(), edit: Box::new(edit) }).await.unwrap() {
        CommandOk::Qso(u) => assert_eq!(u.name.as_deref(), Some("Sam")),
        other => panic!("{other:?}"),
    }
    assert_eq!(snapshot(&h).await.current_qso.and_then(|c| c.name).as_deref(), Some("Sam"));
}

#[tokio::test(flavor = "multi_thread")]
async fn finalise_on_disconnect_preference() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let _peer = spawn_peer(&net, &dir.path().join("peer"));
    let (h, mut rx) = start(dir.path(), existing(&net, None)).await;
    h.call(AppCommand::SetFinaliseOnDisconnect { on: true }).await.unwrap();
    assert!(snapshot(&h).await.finalise_on_disconnect);
    let q = call(&h, &mut rx).await;
    h.call(AppCommand::Disconnect).await.unwrap();
    let ended = qso(&mut rx, 30, |q| q.end_utc.is_some()).await;
    assert_eq!(ended.state, "final");
    let edit = QsoEdit { name: Some("Sam".into()), ..Default::default() };
    let err = h.call(AppCommand::UpdateQso { id: q.id, edit: Box::new(edit) }).await.unwrap_err();
    assert!(matches!(err, AppError::Conflict { .. }), "{err:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn operator_frequency_is_marked_operator() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let ws = FakeWs::start().await;
    ws.set_status(Some(LIVE));
    let _peer = spawn_peer(&net, &dir.path().join("peer"));
    let (h, mut rx) = start(dir.path(), existing(&net, Some(ws.url.clone()))).await;
    telemetry(&mut rx, 10, |t| t.link == TelemetryLinkView::Up && t.frequency.is_some()).await;
    let q = call(&h, &mut rx).await;
    assert_eq!(q.freq.as_ref().map(|f| f.source.as_str()), Some(TELEMETRY));

    let edit = QsoEdit { freq_hz: Some(7_074_000), ..Default::default() };
    let CommandOk::Qso(u) = h.call(AppCommand::UpdateQso { id: q.id.clone(), edit: Box::new(edit) }).await.unwrap()
    else {
        panic!("not a record")
    };
    let f = u.freq.as_ref().expect("a frequency");
    assert_eq!((f.value, f.source.as_str(), f.age_ms), (7_074_000, "operator", None), "no telemetry age is kept");
    assert_eq!(u.band.as_deref(), Some("40m"));
    assert_eq!(qsos(&h).await[0].freq, u.freq);

    // A nonsense frequency is refused, not stored.
    let bad = QsoEdit { freq_hz: Some(0), ..Default::default() };
    let err = h.call(AppCommand::UpdateQso { id: q.id, edit: Box::new(bad) }).await.unwrap_err();
    assert!(matches!(err, AppError::Validation { .. }), "{err:?}");
}

/// M5 §7: the logbook and a conversation are written where the operator
/// chose; "finalised only" leaves the draft out.
#[tokio::test(flavor = "multi_thread")]
async fn exports_write_the_logbook_and_the_conversation() {
    use mc_core::app::export::ExportFormat;
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let _peer = spawn_peer(&net, &dir.path().join("peer"));
    let (h, mut rx) = start(dir.path(), existing(&net, None)).await;
    call(&h, &mut rx).await;
    h.call(AppCommand::SendChat { peer: PEER.into(), text: "hello, José".into() }).await.unwrap();
    wait_for(&mut rx, 20, |e| match e {
        AppEvent::MessageChanged { message } if message.state == "delivered" => Some(()),
        _ => None,
    })
    .await;

    let export = |format, finalised_only, name: &str| {
        let path = dir.path().join(name).to_string_lossy().into_owned();
        (AppCommand::ExportLogbook { format, finalised_only, path: path.clone() }, path)
    };
    let (cmd, path) = export(ExportFormat::Adif, false, "log.adi");
    assert_eq!(h.call(cmd).await.unwrap(), CommandOk::Exported { count: 1, path: path.clone() });
    let adi = std::fs::read_to_string(&path).unwrap();
    assert!(adi.contains("<CALL:4>SIM1") && adi.contains("<EOR>"), "{adi}");

    let (cmd, path) = export(ExportFormat::Adif, true, "final.adi");
    assert_eq!(h.call(cmd).await.unwrap(), CommandOk::Exported { count: 0, path: path.clone() });
    assert!(!std::fs::read_to_string(&path).unwrap().contains("<EOR>"), "the draft is not finalised");

    let (cmd, path) = export(ExportFormat::Csv, false, "log.csv");
    assert_eq!(h.call(cmd).await.unwrap(), CommandOk::Exported { count: 1, path: path.clone() });
    assert_eq!(std::fs::read_to_string(&path).unwrap().lines().count(), 2, "a header and one row");

    let path = dir.path().join("chat.txt").to_string_lossy().into_owned();
    let r = h.call(AppCommand::ExportConversation { peer: PEER.into(), path: path.clone() }).await.unwrap();
    assert_eq!(r, CommandOk::Exported { count: 1, path: path.clone() });
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.starts_with("MercuryChat conversation with SIM1") && text.contains("hello, José"), "{text}");
}

/// The ADIF mode is the operator's (owner, 24 Nov: Log4OM does not know
/// DYNAMIC): stored, shown in the snapshot, used by the export, and checked.
#[tokio::test]
async fn the_adif_mode_is_chosen_and_used() {
    use mc_core::app::export::ExportFormat;
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let _peer = spawn_peer(&net, &dir.path().join("peer"));
    let (h, mut rx) = start(dir.path(), existing(&net, None)).await;
    assert_eq!(snapshot(&h).await.adif_mode, "DYNAMIC", "DYNAMIC by default");
    call(&h, &mut rx).await;

    assert_eq!(h.call(AppCommand::SetAdifMode { mode: " mercury ".into() }).await.unwrap(), CommandOk::Done);
    assert_eq!(snapshot(&h).await.adif_mode, "MERCURY");
    let path = dir.path().join("log.adi").to_string_lossy().into_owned();
    h.call(AppCommand::ExportLogbook { format: ExportFormat::Adif, finalised_only: false, path: path.clone() })
        .await
        .unwrap();
    let adi = std::fs::read_to_string(&path).unwrap();
    assert!(adi.contains("<MODE:7>MERCURY"), "{adi}");

    let refused = h.call(AppCommand::SetAdifMode { mode: "A B".into() }).await;
    assert!(matches!(refused, Err(AppError::Validation { .. })), "{refused:?}");
    assert_eq!(snapshot(&h).await.adif_mode, "MERCURY", "a refused mode changes nothing");
}
