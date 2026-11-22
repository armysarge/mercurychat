//! M3c Task 7 (B6, B10): information requests. Answers are automatic, from
//! what the operator shares; the email only ever goes out on its own command;
//! a peer's report fills the QSO card's empty fields.
#![cfg(feature = "sim")]

use mc_core::app::info::{station_share, ShareConfig, ShareField};
use mc_core::app::{
    spawn_app, AppCommand, AppError, AppEvent, AppHandle, AppOptions, AppSnapshot, CommandOk, EventEnvelope,
    ModemState, QsoEdit,
};
use mc_core::profile::{ModemTarget, StationProfile};
use mc_core::session::PeerMsg;
use mc_core::station::{Station, StationConfig};
use mc_core::{SessionCommand, SessionEvent};
use mc_storage::Store;
use mch_protocol::payload::*;
use mch_protocol::ObjectId;
use mercury_adapter::fake::FakeNet;
use mercury_adapter::fake_ws::FakeWs;
use mercury_adapter::{Callsign, Endpoint};
use std::path::Path;
use std::time::Duration;
use tokio::sync::broadcast::Receiver;
use tokio::sync::mpsc;

const US: &str = "ZS6ABC";
const PEER: &str = "ZS6PEER";

fn status(snr: f32, age_ms: u64) -> String {
    format!(
        r#"{{"type":"status","bitrate":0,"snr":{snr},"direction":"rx","tx_gain_db":0.0,"audio_ok":true,"audio_error":"","arq_tx_mode":"","arq_rx_mode":"","radio_frequency_hz":null,"radio_frequency_age_ms":null,"freq_offset_hz":3.0,"freq_offset_age_ms":{age_ms},"listen_mode":1,"busy":""}}"#
    )
}

/// The app's side and the peer's side of one session.
struct Pair {
    h: AppHandle,
    rx: Receiver<EventEnvelope>,
    peer_cmds: mpsc::Sender<SessionCommand>,
    peer_events: mpsc::UnboundedReceiver<SessionEvent>,
    _net: std::sync::Arc<FakeNet>,
    _ws: FakeWs,
}

impl Pair {
    async fn peer_sends(&self, m: PeerMsg) {
        self.peer_cmds.send(SessionCommand::Peer(m)).await.unwrap();
    }

    /// The next minor-1 message the app sent the peer.
    async fn peer_gets(&mut self, within: Duration) -> Option<PeerMsg> {
        let end = tokio::time::Instant::now() + within;
        loop {
            match tokio::time::timeout_at(end, self.peer_events.recv()).await {
                Ok(Some(SessionEvent::Peer(m))) => return Some(m),
                Ok(Some(_)) => {}
                Ok(None) => panic!("the peer's session ended"),
                Err(_) => return None,
            }
        }
    }

    async fn answer_to(&mut self, request: ObjectId) -> InfoResponse {
        match self.peer_gets(Duration::from_secs(10)).await {
            Some(PeerMsg::InfoResponse(id, r)) => {
                assert_eq!(id, request, "the answer carries the request id");
                *r
            }
            other => panic!("no answer: {other:?}"),
        }
    }

    async fn snapshot(&self) -> AppSnapshot {
        match self.h.call(AppCommand::Snapshot).await.unwrap() {
            CommandOk::Snapshot(s) => s,
            other => panic!("{other:?}"),
        }
    }

    async fn peer_info(&mut self, within: Duration) -> Option<AppEvent> {
        let end = tokio::time::Instant::now() + within;
        loop {
            match tokio::time::timeout_at(end, self.rx.recv()).await {
                Ok(Ok(e)) if matches!(e.event, AppEvent::PeerInfo { .. }) => return Some(e.event),
                Ok(Ok(_)) => {}
                Ok(Err(e)) => panic!("{e}"),
                Err(_) => return None,
            }
        }
    }
}

/// The app (ZS6ABC, listening) answers a call from a Station peer running
/// MCH/1 minor `peer_minor`; the session is negotiated on both sides.
async fn pair(root: &Path, profile: impl FnOnce(&mut StationProfile), peer_minor: u8, snr_age_ms: u64) -> Pair {
    let net = FakeNet::start().await;
    let ws = FakeWs::start().await;
    ws.set_status(Some(&status(-3.4, snr_age_ms)));
    let mut p = StationProfile::new_existing(US);
    p.modem = ModemTarget::Existing {
        host: "127.0.0.1".into(),
        control_port: net.a_base,
        data_port: net.a_base + 1,
        broadcast_port: None,
        telemetry_url: Some(ws.url.clone()),
    };
    profile(&mut p);
    let opts = AppOptions {
        sim_root: Some(root.to_path_buf()),
        stations_root: Some(root.join("stations")),
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

    let mut cfg = StationConfig::defaults(
        Callsign::parse(PEER).unwrap(),
        Endpoint::base("127.0.0.1", net.b_base),
        &root.join("peer"),
    );
    cfg.proto_minor = peer_minor;
    if peer_minor == 0 {
        cfg.capabilities = vec!["chat".into(), "file".into()];
    }
    let mut peer = Station::open(cfg).unwrap();
    peer.attach().await.unwrap();
    let info = peer.call(&Callsign::parse(US).unwrap(), Duration::from_secs(10)).await.unwrap();
    let (peer_cmds, mut cmd_rx) = mpsc::channel(16);
    let (ev_tx, mut peer_events) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        let (_req_tx, mut req_rx) = mpsc::channel(4);
        peer.run_session(&info, &mut cmd_rx, &mut req_rx, ev_tx).await
    });
    loop {
        match tokio::time::timeout(Duration::from_secs(10), peer_events.recv()).await.expect("negotiated") {
            Some(SessionEvent::Negotiated { .. }) => break,
            Some(_) => {}
            None => panic!("the peer's session ended"),
        }
    }
    let pair = Pair { h, rx, peer_cmds, peer_events, _net: net, _ws: ws };
    let end = tokio::time::Instant::now() + Duration::from_secs(10);
    while pair.snapshot().await.session.is_none() || pair.snapshot().await.telemetry.snr_db.is_none() {
        assert!(tokio::time::Instant::now() < end, "no session or no telemetry SNR on the app");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    pair
}

fn request(what: InfoWhat, n: u8) -> (ObjectId, PeerMsg) {
    let id = ObjectId([n; 16]);
    (id, PeerMsg::InfoRequest(id, InfoRequest { what }))
}

fn refusal(e: &AppError) -> &'static str {
    match e {
        AppError::Refused { reason, .. } => reason,
        other => panic!("not a refusal: {other:?}"),
    }
}

fn full_station(p: &mut StationProfile) {
    p.name = Some("Pat".into());
    p.grid = Some("KG44dg".into());
    p.qth = Some("Pretoria".into());
    p.rig = Some("IC-7300".into());
    p.power = Some("100 W".into());
    p.antenna = Some("EFHW".into());
    p.about = Some("Chats on 40m".into());
    p.email = Some("zs6abc@example.com".into());
}

#[tokio::test(flavor = "multi_thread")]
async fn snr_request_answered_from_telemetry() {
    let dir = tempfile::tempdir().unwrap();
    let mut p = pair(dir.path(), |_| {}, 1, 800).await;
    let (id, m) = request(InfoWhat::Snr, 1);
    p.peer_sends(m).await;
    let r = p.answer_to(id).await;
    let snr = r.snr.expect("an SNR answer");
    assert_eq!((snr.db, snr.source.as_str()), (-3.4, "telemetry"));
    assert!(snr.age_s <= 5, "{}", snr.age_s);
    assert_eq!(r.reason, None);
}

#[tokio::test(flavor = "multi_thread")]
async fn snr_no_reading_after_60s() {
    let dir = tempfile::tempdir().unwrap();
    let mut p = pair(dir.path(), |_| {}, 1, 61_000).await;
    let (id, m) = request(InfoWhat::Snr, 2);
    p.peer_sends(m).await;
    let r = p.answer_to(id).await;
    assert_eq!((r.snr, r.reason.as_deref()), (None, Some("no-reading")));
}

#[tokio::test(flavor = "multi_thread")]
async fn station_answer_has_only_shared_fields() {
    let dir = tempfile::tempdir().unwrap();
    let mut p = pair(dir.path(), full_station, 1, 800).await;
    let (id, m) = request(InfoWhat::Station, 3);
    p.peer_sends(m).await;
    let s = p.answer_to(id).await.station.expect("a station answer");
    // The defaults: name, QTH and locator.
    assert_eq!(
        s,
        StationShare {
            name: Some("Pat".into()),
            qth: Some("Pretoria".into()),
            locator: Some("KG44dg".into()),
            ..StationShare::default()
        }
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn email_never_in_an_answer() {
    let dir = tempfile::tempdir().unwrap();
    let mut p = pair(dir.path(), full_station, 1, 800).await;
    use ShareField::*;
    let config = ShareConfig { fields: vec![Name, Qth, Locator, Rig, Power, Antenna, About] };
    p.h.call(AppCommand::SetShareSettings { config: config.clone() }).await.unwrap();
    assert_eq!(p.h.call(AppCommand::ShareSettings).await.unwrap(), CommandOk::Share(config));
    let (id, m) = request(InfoWhat::Station, 4);
    p.peer_sends(m).await;
    let s = p.answer_to(id).await.station.expect("a station answer");
    assert_eq!((s.rig.as_deref(), s.about.as_deref()), (Some("IC-7300"), Some("Chats on 40m")));
    assert_eq!(s.email, None, "the email is never in an answer");
}

#[tokio::test(flavor = "multi_thread")]
async fn send_email_once_unsolicited() {
    let dir = tempfile::tempdir().unwrap();
    let mut p = pair(dir.path(), full_station, 1, 800).await;
    p.h.call(AppCommand::SendEmail).await.unwrap();
    let r = p.answer_to(ObjectId::ZERO).await;
    assert_eq!(
        r,
        InfoResponse {
            snr: None,
            station: Some(StationShare { email: Some("zs6abc@example.com".into()), ..StationShare::default() }),
            reason: None,
        }
    );
    assert_eq!(p.peer_gets(Duration::from_secs(1)).await, None, "sent once");
}

#[tokio::test(flavor = "multi_thread")]
async fn rate_limited_second_request() {
    let dir = tempfile::tempdir().unwrap();
    let mut p = pair(dir.path(), |_| {}, 1, 800).await;
    let (first, m) = request(InfoWhat::Snr, 5);
    p.peer_sends(m).await;
    assert!(p.answer_to(first).await.snr.is_some());
    let (second, m) = request(InfoWhat::Snr, 6);
    p.peer_sends(m).await;
    let r = p.answer_to(second).await;
    assert_eq!((r.snr, r.station, r.reason.as_deref()), (None, None, Some("rate")));
    // The other kind has its own allowance.
    let (third, m) = request(InfoWhat::Station, 7);
    p.peer_sends(m).await;
    assert!(p.answer_to(third).await.station.is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn peer_report_fills_empty_fields_only() {
    let dir = tempfile::tempdir().unwrap();
    let mut p = pair(dir.path(), |_| {}, 1, 800).await;
    let qso = p.snapshot().await.current_qso.expect("a draft");
    let edit = Box::new(QsoEdit { name: Some("Typed Name".into()), ..QsoEdit::default() });
    p.h.call(AppCommand::UpdateQso { id: qso.id.clone(), edit }).await.unwrap();

    for what in [InfoWhat::Station, InfoWhat::Snr] {
        p.h.call(AppCommand::RequestInfo { what }).await.unwrap();
        let Some(PeerMsg::InfoRequest(id, req)) = p.peer_gets(Duration::from_secs(10)).await else {
            panic!("no request reached the peer")
        };
        assert_eq!(req.what, what);
        let answer = match what {
            InfoWhat::Station => InfoResponse {
                snr: None,
                station: Some(StationShare {
                    name: Some("Pat".into()),
                    qth: Some("Pretoria".into()),
                    ..StationShare::default()
                }),
                reason: None,
            },
            InfoWhat::Snr => InfoResponse {
                snr: Some(SnrReport { db: -3.4, age_s: 12, source: "telemetry".into() }),
                station: None,
                reason: None,
            },
        };
        p.peer_sends(PeerMsg::InfoResponse(id, Box::new(answer))).await;
        assert!(p.peer_info(Duration::from_secs(10)).await.is_some(), "PeerInfo for {what:?}");
    }
    let qso = p.snapshot().await.current_qso.expect("the draft");
    assert_eq!((qso.name.as_deref(), qso.name_source.as_deref()), (Some("Typed Name"), None), "typed is kept");
    assert_eq!((qso.qth.as_deref(), qso.qth_source.as_deref()), (Some("Pretoria"), Some("peer report")));
    assert_eq!((qso.rst_rcvd.as_deref(), qso.rst_rcvd_source.as_deref()), (Some("-3 dB"), Some("peer report")));
}

#[tokio::test(flavor = "multi_thread")]
async fn unknown_response_id_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let mut p = pair(dir.path(), |_| {}, 1, 800).await;
    let stray = InfoResponse {
        snr: None,
        station: Some(StationShare { qth: Some("Nowhere".into()), ..StationShare::default() }),
        reason: None,
    };
    p.peer_sends(PeerMsg::InfoResponse(ObjectId([0x77; 16]), Box::new(stray))).await;
    assert_eq!(p.peer_info(Duration::from_secs(2)).await, None, "an answer to nothing we asked");
    assert_eq!(p.snapshot().await.current_qso.unwrap().qth, None);
}

/// M4 B6: an unsolicited answer (id zero) is the email share only; SNR and
/// station fields in it fill nothing.
#[tokio::test(flavor = "multi_thread")]
async fn unsolicited_answer_only_fills_email() {
    let dir = tempfile::tempdir().unwrap();
    let mut p = pair(dir.path(), |_| {}, 1, 800).await;
    let share = InfoResponse {
        snr: Some(SnrReport { db: 7.0, age_s: 3, source: "telemetry".into() }),
        station: Some(StationShare {
            name: Some("Sneaky".into()),
            qth: Some("Elsewhere".into()),
            email: Some("pat@example.org".into()),
            ..StationShare::default()
        }),
        reason: Some("rate".into()),
    };
    p.peer_sends(PeerMsg::InfoResponse(ObjectId::ZERO, Box::new(share))).await;
    let Some(AppEvent::PeerInfo { snr, station, reason, .. }) = p.peer_info(Duration::from_secs(10)).await else {
        panic!("the email share is shown")
    };
    assert_eq!((snr, reason), (None, None));
    let station = station.expect("the email");
    assert_eq!(station.email.as_deref(), Some("pat@example.org"));
    assert_eq!((station.name, station.qth), (None, None));
    let qso = p.snapshot().await.current_qso.unwrap();
    assert_eq!((qso.name, qso.qth, qso.rst_rcvd), (None, None, None));
}

#[test]
fn over_limit_value_left_out() {
    let mut p = StationProfile::new_existing(US);
    p.name = Some("Pat".into());
    p.qth = Some("q".repeat(49)); // B10: QTH at most 48 bytes
    let s = station_share(&p, &ShareConfig::default());
    assert_eq!((s.name.as_deref(), s.qth), (Some("Pat"), None), "left out, never truncated");
}

#[tokio::test(flavor = "multi_thread")]
async fn request_refused_without_capability() {
    let dir = tempfile::tempdir().unwrap();
    let p = pair(dir.path(), full_station, 0, 800).await;
    let e = p.h.call(AppCommand::RequestInfo { what: InfoWhat::Snr }).await.unwrap_err();
    assert_eq!(refusal(&e), "info.refused.unsupported");
    let e = p.h.call(AppCommand::SendEmail).await.unwrap_err();
    assert_eq!(refusal(&e), "info.refused.unsupported");
}
