//! M3a Task 7: Mercury telemetry as the app shows it (OBS-01, OBS-02), and the
//! frequency, band and offset on heard stations. The modem is the FakeNet
//! pair (side A is the app's), telemetry a fake Mercury WebSocket. Losing
//! telemetry never ends a session, and what is missing is `null`, never 0.
#![cfg(feature = "sim")]

use mc_core::app::{
    spawn_app, AppCommand, AppEvent, AppHandle, AppOptions, AppSnapshot, ChatMessage, CommandOk, EventEnvelope,
    HeardStation, ModemState, TelemetryLinkView, TelemetryView,
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

/// What Mercury publishes every 500 ms while a session is up.
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

/// The next event `want` accepts, within `secs`.
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

/// The next telemetry view `want` accepts.
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

/// An app on side A of `net`, listening.
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

/// SIM1 on side B of `net`: listens and runs every session.
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

async fn connect(h: &AppHandle, rx: &mut Receiver<EventEnvelope>) {
    h.call(AppCommand::Call { peer: PEER.into() }).await.unwrap();
    until_state(rx, ModemState::Connected, 15).await;
}

/// Send `text` to SIM1 and wait until it is delivered.
async fn deliver(h: &AppHandle, rx: &mut Receiver<EventEnvelope>, text: &str) -> ChatMessage {
    let m = match h.call(AppCommand::SendChat { peer: PEER.into(), text: text.into() }).await.unwrap() {
        CommandOk::Message(m) => m,
        other => panic!("{other:?}"),
    };
    wait_for(rx, 20, |e| match e {
        AppEvent::MessageChanged { message } if message.id == m.id && message.state == "delivered" => {
            Some(message.clone())
        }
        AppEvent::MessageChanged { message } if message.id == m.id => {
            assert!(!matches!(message.state.as_str(), "failed" | "uncertain"), "{message:?}");
            None
        }
        _ => None,
    })
    .await
}

/// Nothing in the view came from telemetry.
fn no_telemetry_readings(t: &TelemetryView) {
    assert!(t.snr_db.as_ref().is_none_or(|r| r.source != TELEMETRY), "{t:?}");
    assert_eq!(t.tx_mode, None);
    assert_eq!(t.rx_mode, None);
    assert_eq!(t.audio, None);
    assert_eq!(t.frequency, None);
    assert_eq!(t.listen_mode, None);
}

fn up_with_status(t: &TelemetryView) -> bool {
    t.link == TelemetryLinkView::Up && t.frequency.is_some()
}

#[tokio::test(flavor = "multi_thread")]
async fn telemetry_loss_mid_session_leaves_chat_intact() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let _peer = spawn_peer(&net, &dir.path().join("peer"));
    let ws = FakeWs::start().await;
    ws.set_status(Some(LIVE));
    let (h, mut rx) = start(dir.path(), existing(&net, Some(ws.url.clone()))).await;
    let t = telemetry(&mut rx, 10, up_with_status).await;
    assert_eq!(t.snr_db.as_ref().map(|r| (r.value, r.source.as_str())), Some((9.5, TELEMETRY)));
    assert_eq!(t.tx_mode.as_deref(), Some("DATAC3"));
    assert_eq!(t.rx_mode.as_deref(), Some("DATAC1"));
    assert_eq!(t.listen_mode, Some(1));
    let f = t.frequency.as_ref().unwrap();
    assert_eq!((f.value, f.source.as_str()), (14_105_000, TELEMETRY));
    assert!(f.age_ms.is_some_and(|a| a >= 200), "{f:?}");

    connect(&h, &mut rx).await;
    deliver(&h, &mut rx, "before").await;

    // Mercury's WebSocket dies; the connection to its TNC ports does not.
    drop(ws);
    let t = telemetry(&mut rx, 10, |t| matches!(t.link, TelemetryLinkView::Lost { .. })).await;
    no_telemetry_readings(&t);
    match &t.link {
        TelemetryLinkView::Lost { since_utc, reason } => assert!(!since_utc.is_empty() && !reason.is_empty()),
        other => panic!("{other:?}"),
    }

    // The session goes on: chat still delivers, and nothing stale is live.
    deliver(&h, &mut rx, "after").await;
    let s = snapshot(&h).await;
    assert_eq!(s.modem, ModemState::Connected);
    assert_eq!(s.session.as_ref().map(|s| s.peer.as_str()), Some(PEER));
    assert!(matches!(s.telemetry.link, TelemetryLinkView::Lost { .. }), "{:?}", s.telemetry);
    no_telemetry_readings(&s.telemetry);
    assert!(s.telemetry.acked_bps.is_some_and(|b| b > 0.0), "app throughput is not telemetry: {:?}", s.telemetry);
}

#[tokio::test(flavor = "multi_thread")]
async fn garbage_frames_do_not_end_the_session() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let _peer = spawn_peer(&net, &dir.path().join("peer"));
    let ws = FakeWs::start().await;
    ws.set_status(Some(LIVE));
    let (h, mut rx) = start(dir.path(), existing(&net, Some(ws.url.clone()))).await;
    let spectrum = h.spectrum();
    telemetry(&mut rx, 10, up_with_status).await;
    connect(&h, &mut rx).await;

    // Non-JSON, an unknown type, a status of the wrong shape, NaN bins, a
    // short binary frame, a bad N, then a 1 MiB text frame.
    let header = |n: u16| {
        let mut b = 0x4D43_5259u32.to_le_bytes().to_vec();
        b.extend_from_slice(&n.to_le_bytes());
        b.extend_from_slice(&8000u16.to_le_bytes());
        b
    };
    let mut nan = header(64);
    for i in 0..64 {
        nan.extend_from_slice(&(if i == 5 { f32::NAN } else { -90.0f32 }).to_le_bytes());
    }
    ws.send_text("this is not json".into());
    ws.send_text(r#"{"type":"frobnicate","snr":99}"#.into());
    ws.send_text(r#"{"type":"status","snr":[1,2]}"#.into());
    ws.send_binary(nan);
    ws.send_binary(vec![0x59, 0x52, 0x43]);
    ws.send_binary(header(100));
    ws.send_text(format!(r#"{{"type":"status","snr":1.0,"audio_error":"{}"}}"#, "z".repeat(1024 * 1024)));
    deliver(&h, &mut rx, "through the garbage").await;
    assert!(spectrum.borrow().is_none(), "no invalid spectrum is ever published");

    // Telemetry comes back by itself, with the good values only.
    let t = telemetry(&mut rx, 20, up_with_status).await;
    assert_eq!(t.snr_db.map(|r| r.value), Some(9.5));
    let mut good = header(64);
    for _ in 0..64 {
        good.extend_from_slice(&(-80.0f32).to_le_bytes());
    }
    let mut feed = h.spectrum();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        ws.send_binary(good.clone());
        let got = tokio::time::timeout(Duration::from_millis(500), feed.wait_for(|s| s.is_some())).await;
        if let Ok(Ok(s)) = got {
            let s = s.clone().unwrap();
            assert_eq!((s.fft_size, s.sample_rate, s.bins.len()), (64, 8000, 64));
            break;
        }
        assert!(Instant::now() < deadline, "the good spectrum never arrived");
    }
    let s = snapshot(&h).await;
    assert_eq!(s.modem, ModemState::Connected, "garbage never touches the session");
    deliver(&h, &mut rx, "still here").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn unconfigured_telemetry_shows_unavailable_not_zero() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let (h, _rx) = start(dir.path(), existing(&net, None)).await;
    let t = snapshot(&h).await.telemetry;
    assert_eq!(t.link, TelemetryLinkView::Unconfigured);
    assert_eq!(t.snr_db, None);
    assert_eq!(t.acked_bps, None);
    no_telemetry_readings(&t);
    let json = serde_json::to_value(&t).unwrap();
    for field in ["snrDb", "txMode", "rxMode", "ackedBps", "audio", "frequency", "listenMode"] {
        assert!(json[field].is_null(), "{field} must be null, not a made-up value: {json}");
    }
    assert_eq!(json["link"]["type"], "unconfigured");
}

#[tokio::test(flavor = "multi_thread")]
async fn snr_falls_back_to_control_sn_with_its_source() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let ws = FakeWs::start().await;
    ws.set_status(Some(LIVE));
    let (h, mut rx) = start(dir.path(), existing(&net, Some(ws.url.clone()))).await;
    telemetry(&mut rx, 10, up_with_status).await;
    // While telemetry reports an SNR, it is the one shown.
    net.send_control(0, "SN 4.5");
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        let t = snapshot(&h).await.telemetry;
        assert_eq!(t.snr_db.map(|r| (r.value, r.source)), Some((9.5, TELEMETRY.to_string())));
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    // Without telemetry, the control connection's SN, labelled as such.
    drop(ws);
    let t = telemetry(&mut rx, 10, |t| matches!(t.link, TelemetryLinkView::Lost { .. })).await;
    let snr = t.snr_db.expect("the control SN is still a reading");
    assert_eq!((snr.value, snr.source.as_str()), (4.5, "modem control"));
    net.send_control(0, "SN -3.0");
    let t = telemetry(&mut rx, 10, |t| t.snr_db.as_ref().is_some_and(|r| r.value == -3.0)).await;
    assert_eq!(t.snr_db.unwrap().source, "modem control");
}

#[tokio::test(flavor = "multi_thread")]
async fn acked_throughput_only_after_first_receipt() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let _peer = spawn_peer(&net, &dir.path().join("peer"));
    let (h, mut rx) = start(dir.path(), existing(&net, None)).await;
    assert_eq!(snapshot(&h).await.telemetry.acked_bps, None, "no session");
    connect(&h, &mut rx).await;
    assert_eq!(snapshot(&h).await.telemetry.acked_bps, None, "a session, but nothing acknowledged yet");
    deliver(&h, &mut rx, "the first receipt").await;
    let t = telemetry(&mut rx, 5, |t| t.acked_bps.is_some()).await;
    let bps = t.acked_bps.unwrap();
    assert!(bps > 0.0 && bps <= "the first receipt".len() as f32, "{bps}");
    h.call(AppCommand::Disconnect).await.unwrap();
    until_state(&mut rx, ModemState::Listening, 30).await;
    assert_eq!(snapshot(&h).await.telemetry.acked_bps, None, "no session, no session throughput");
}

/// Send valid beacons from `call` until A hears one.
async fn heard_beacon(net: &FakeNet, rx: &mut Receiver<EventEnvelope>, call: &str) -> HeardStation {
    let port = BroadcastPort::connect("127.0.0.1", net.b_bcast, Duration::from_secs(2)).await.unwrap().0;
    let payload = Beacon::new(7, call, Some("KG33xi"), CAP_CHAT).unwrap().encode();
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        port.send(&payload).await.unwrap();
        let got = tokio::time::timeout(
            Duration::from_secs(1),
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

async fn heard_with(status: &str) -> (HeardStation, HeardStation) {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let ws = FakeWs::start().await;
    ws.set_status(Some(status));
    let (h, mut rx) = start(dir.path(), existing(&net, Some(ws.url.clone()))).await;
    telemetry(&mut rx, 10, |t| t.link == TelemetryLinkView::Up && t.snr_db.is_some()).await;
    let ev = heard_beacon(&net, &mut rx, "ZS6PEER").await;
    let row = heard(&h).await.into_iter().find(|s| s.callsign == "ZS6PEER").expect("listed");
    (ev, row)
}

#[tokio::test(flavor = "multi_thread")]
async fn heard_row_takes_fresh_frequency_and_band_with_source() {
    let (ev, row) = heard_with(LIVE).await;
    for st in [&ev, &row] {
        let f = st.freq.as_ref().expect("a fresh frequency is recorded");
        assert_eq!((f.value, f.source.as_str()), (14_105_000, TELEMETRY));
        assert!(f.age_ms.is_some_and(|a| (200..=10_000).contains(&a)), "{f:?}");
        assert_eq!(st.band.as_deref(), Some("20m"));
        let o = st.offset_hz.as_ref().expect("a fresh offset is attached");
        assert_eq!((o.value, o.source.as_str()), (12.5, TELEMETRY));
        assert!(o.age_ms.is_some_and(|a| a < 2000), "{o:?}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn heard_row_without_fresh_frequency_has_none() {
    // Mercury's cached frequency is 15 s old: too old to say where it was heard.
    let stale = LIVE.replace(r#""radio_frequency_age_ms":200"#, r#""radio_frequency_age_ms":15000"#);
    let (ev, row) = heard_with(&stale).await;
    for st in [&ev, &row] {
        assert_eq!(st.freq, None, "{st:?}");
        assert_eq!(st.band, None);
    }
    // No frequency at all.
    let none = LIVE
        .replace(r#""radio_frequency_hz":14105000"#, r#""radio_frequency_hz":null"#)
        .replace(r#""radio_frequency_age_ms":200"#, r#""radio_frequency_age_ms":null"#);
    let (ev, row) = heard_with(&none).await;
    assert_eq!((ev.freq, ev.band), (None, None));
    assert_eq!((row.freq, row.band), (None, None));

    // And no telemetry at all.
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let (_h, mut rx) = start(dir.path(), existing(&net, None)).await;
    let ev = heard_beacon(&net, &mut rx, "ZS6PEER").await;
    assert_eq!((ev.freq, ev.band, ev.offset_hz), (None, None, None));
}

#[tokio::test(flavor = "multi_thread")]
async fn stale_offset_is_not_attached() {
    let stale = LIVE.replace(r#""freq_offset_age_ms":100"#, r#""freq_offset_age_ms":5000"#);
    let (ev, row) = heard_with(&stale).await;
    for st in [&ev, &row] {
        assert_eq!(st.offset_hz, None, "a 5 s old offset is not this frame's: {st:?}");
        assert!(st.freq.is_some(), "the frequency is still fresh");
    }
}

/// The beacon gate's answer to one SendBeaconNow: Ok, or the refusal's text.
async fn beacon_now(h: &AppHandle) -> Result<(), String> {
    match h.call(AppCommand::SendBeaconNow).await {
        Ok(_) => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

/// Fix round 1 (review Important 1): after one good status the socket sends
/// only garbage and spectrum frames. That is not telemetry: the link is lost,
/// every reading goes, and the beacon gate falls back to the attach listen
/// mode (ruling P2) instead of a stale live one.
#[tokio::test(flavor = "multi_thread")]
async fn a_socket_without_statuses_is_lost_and_the_gate_falls_back_to_attach() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let ws = FakeWs::start().await;
    // A live listen mode the gate has no capacity for: beacons are refused.
    let odd = LIVE.replace(r#""listen_mode":1,"#, r#""listen_mode":200,"#);
    ws.set_status(Some(&odd));
    let (h, mut rx) = start(dir.path(), existing(&net, Some(ws.url.clone()))).await;
    telemetry(&mut rx, 10, |t| t.listen_mode == Some(200)).await;
    let why = beacon_now(&h).await.expect_err("live mode 200 has no known capacity");
    assert!(why.contains("listen mode unknown"), "{why}");

    // From now on: no status, only garbage and good spectrum frames.
    ws.set_status(None);
    let mut spectrum = 0x4D43_5259u32.to_le_bytes().to_vec();
    spectrum.extend_from_slice(&64u16.to_le_bytes());
    spectrum.extend_from_slice(&8000u16.to_le_bytes());
    for _ in 0..64 {
        spectrum.extend_from_slice(&(-90.0f32).to_le_bytes());
    }
    let deadline = Instant::now() + Duration::from_secs(15);
    let lost = loop {
        ws.send_text("not json".into());
        ws.send_text(r#"{"type":"history","messages":[]}"#.into());
        ws.send_binary(spectrum.clone());
        let got = tokio::time::timeout(
            Duration::from_millis(200),
            telemetry(&mut rx, 5, |t| matches!(t.link, TelemetryLinkView::Lost { .. })),
        )
        .await;
        if let Ok(t) = got {
            break t;
        }
        assert!(Instant::now() < deadline, "a status-less socket stayed up");
    };
    no_telemetry_readings(&lost);
    let s = snapshot(&h).await;
    assert_eq!(s.telemetry.listen_mode, None);
    assert_eq!(s.telemetry.snr_db, None, "no control SN either: nothing is shown");
    // The gate is back on the attach value (MODE 1, DATAC3): the beacon goes.
    beacon_now(&h).await.expect("the attach listen mode applies again");
}

/// Fix round 1 (review Important 2): the modem task faults while telemetry is
/// up. The Faulted report alone shows the link lost with every reading gone.
#[tokio::test(flavor = "multi_thread")]
async fn a_modem_fault_with_telemetry_up_shows_it_lost() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let ws = FakeWs::start().await;
    ws.set_status(Some(LIVE));
    let (h, mut rx) = start(dir.path(), existing(&net, Some(ws.url.clone()))).await;
    telemetry(&mut rx, 10, up_with_status).await;
    // A refused LISTEN OFF faults the modem (Mercury may still answer calls).
    net.set_listen_off_wrong(0, true);
    h.call(AppCommand::SetReceiveOnly { on: true }).await.unwrap();
    // wait_for treats Faulted as a failure: here it is the point.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match tokio::time::timeout_at(deadline, rx.recv()).await.expect("Faulted within 10 s") {
            Ok(env) if matches!(env.event, AppEvent::ModemState { state: ModemState::Faulted, .. }) => break,
            Ok(_) | Err(RecvError::Lagged(_)) => continue,
            Err(RecvError::Closed) => panic!("the event stream closed"),
        }
    }
    let s = snapshot(&h).await;
    assert_eq!(s.modem, ModemState::Faulted);
    assert!(matches!(s.telemetry.link, TelemetryLinkView::Lost { .. }), "{:?}", s.telemetry.link);
    no_telemetry_readings(&s.telemetry);
}
