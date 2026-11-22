//! M3c Task 6 (B1): MCH/1 minor 1 capabilities. A new frame type goes on the
//! wire only when both HELLOs list its capability; an old peer never sees one.
#![cfg(feature = "sim")]

use mc_core::session::{default_capabilities, PeerMsg};
use mc_core::station::{Station, StationConfig};
use mc_core::{SessionCommand, SessionEnd, SessionEvent};
use mch_protocol::payload::*;
use mch_protocol::{encode, Decoder, FrameType, ObjectId};
use mercury_adapter::fake::FakeNet;
use mercury_adapter::{Callsign, Endpoint};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

const A: &str = "ZS6AAA";
const B: &str = "ZS6BBB";

/// One side of a running session, driven by the test.
struct Side {
    cmds: mpsc::Sender<SessionCommand>,
    events: mpsc::UnboundedReceiver<SessionEvent>,
    end: JoinHandle<SessionEnd>,
}

impl Side {
    async fn next(&mut self, what: &str, pred: impl Fn(&SessionEvent) -> bool) -> SessionEvent {
        let end = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            match tokio::time::timeout_at(end, self.events.recv()).await {
                Ok(Some(e)) if pred(&e) => return e,
                Ok(Some(_)) => {}
                Ok(None) => panic!("waiting for {what}: the session ended"),
                Err(_) => panic!("no {what} within 10 s"),
            }
        }
    }

    async fn negotiated(&mut self) -> (u8, Vec<String>) {
        match self.next("Negotiated", |e| matches!(e, SessionEvent::Negotiated { .. })).await {
            SessionEvent::Negotiated { minor, caps, .. } => (minor, caps),
            _ => unreachable!(),
        }
    }

    async fn send(&self, m: PeerMsg) {
        self.cmds.send(SessionCommand::Peer(m)).await.unwrap();
    }
}

fn station(net: &FakeNet, call: &str, base: u16, dir: &Path, minor: u8, caps: &[&str]) -> Station {
    let mut cfg = StationConfig::defaults(Callsign::parse(call).unwrap(), Endpoint::base("127.0.0.1", base), dir);
    cfg.proto_minor = minor;
    cfg.capabilities = caps.iter().map(|c| c.to_string()).collect();
    let _ = net;
    Station::open(cfg).unwrap()
}

fn run(mut st: Station, info: mc_core::ConnectInfo) -> Side {
    let (cmds, mut cmd_rx) = mpsc::channel(32);
    let (ev_tx, events) = mpsc::unbounded_channel();
    let end = tokio::spawn(async move {
        let (_req_tx, mut req_rx) = mpsc::channel(4);
        st.run_session(&info, &mut cmd_rx, &mut req_rx, ev_tx).await
    });
    Side { cmds, events, end }
}

/// A calls B, which listens; both sessions run. B speaks `b_minor` with `b_caps`.
async fn pair(dir: &Path, b_minor: u8, b_caps: &[&str]) -> (Arc<FakeNet>, Side, Side) {
    let net = FakeNet::start().await;
    let defaults = default_capabilities();
    let a_caps: Vec<&str> = defaults.iter().map(String::as_str).collect();
    let mut a = station(&net, A, net.a_base, &dir.join("a"), mch_protocol::MINOR, &a_caps);
    let mut b = station(&net, B, net.b_base, &dir.join("b"), b_minor, b_caps);
    a.attach().await.unwrap();
    b.attach().await.unwrap();
    b.set_listen(true).await.unwrap();
    let callee = Callsign::parse(B).unwrap();
    let (ia, ib) = tokio::join!(a.call(&callee, Duration::from_secs(10)), b.wait_incoming(Duration::from_secs(10)));
    (net, run(a, ia.unwrap()), run(b, ib.unwrap()))
}

fn every_peer_msg(id: ObjectId) -> Vec<PeerMsg> {
    let cand = QsyCandidate { freq_hz: 7_105_000, mode: "USB".into(), passband_hz: 0, name: "40m".into() };
    vec![
        PeerMsg::QsyPropose(id, QsyPropose { candidates: vec![cand], note: None }),
        PeerMsg::QsyAnswer(id, QsyAnswer::Decline { reason: Some("busy".into()) }),
        PeerMsg::InfoRequest(id, InfoRequest { what: InfoWhat::Snr }),
        PeerMsg::InfoResponse(
            id,
            Box::new(InfoResponse {
                snr: Some(SnrReport { db: 4.5, age_s: 3, source: "telemetry".into() }),
                station: None,
                reason: None,
            }),
        ),
    ]
}

#[tokio::test(flavor = "multi_thread")]
async fn old_peer_never_receives_new_frames() {
    let dir = tempfile::tempdir().unwrap();
    let (net, mut a, mut b) = pair(dir.path(), 0, &["chat", "file"]).await;
    let (minor, caps) = a.negotiated().await;
    assert_eq!(minor, 0, "the peer's minor is recorded, not ours");
    // M4: the old "file" is not ours any more (file/2 is).
    assert_eq!(caps, ["chat"]);
    b.negotiated().await;

    net.set_hold_from(0, true);
    for m in every_peer_msg(ObjectId([3; 16])) {
        a.send(m).await;
        let refused = a.next("PeerSendRefused", |e| matches!(e, SessionEvent::PeerSendRefused { .. })).await;
        let SessionEvent::PeerSendRefused { reason, .. } = refused else { unreachable!() };
        assert_eq!(reason, "not supported by this station");
    }
    a.cmds.send(SessionCommand::SendChat { id: ObjectId([4; 16]), text: "still chatting".into() }).await.unwrap();
    a.next("ChatSubmitted", |e| matches!(e, SessionEvent::ChatSubmitted { .. })).await;

    // Everything A put on the wire after negotiation, decoded: no minor-1 type.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while net.held_len() == 0 {
        assert!(tokio::time::Instant::now() < deadline, "A sent nothing");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    let held = net.take_held();
    let bytes: Vec<u8> = held.iter().flat_map(|(_, b)| b.clone()).collect();
    let mut d = Decoder::new();
    d.push(&bytes);
    let mut types = Vec::new();
    while let Some(f) = d.next_frame().unwrap() {
        types.push(f.ftype as u8);
    }
    assert!(types.iter().all(|t| !(0x30..=0x41).contains(t)), "a new frame type reached the old peer: {types:x?}");
    assert!(types.contains(&(FrameType::Chat as u8)), "{types:x?}");
    net.set_hold_from(0, false);
    net.inject(0, bytes);
    b.next("the chat on B", |e| matches!(e, SessionEvent::ChatReceived { .. })).await;
    assert!(!b.end.is_finished(), "the old peer's session is still up");
}

#[tokio::test(flavor = "multi_thread")]
async fn negotiated_caps_are_the_intersection() {
    let dir = tempfile::tempdir().unwrap();
    let (_net, mut a, mut b) = pair(dir.path(), 1, &["chat", "file", "info/1", "future/9"]).await;
    let (minor, caps) = a.negotiated().await;
    assert_eq!((minor, caps), (1, vec!["chat".to_string(), "info/1".into()]));
    let (_, caps) = b.negotiated().await;
    assert_eq!(caps, ["chat", "info/1"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn new_peer_exchanges_qsy_and_info() {
    let dir = tempfile::tempdir().unwrap();
    let defaults = default_capabilities();
    let caps: Vec<&str> = defaults.iter().map(String::as_str).collect();
    let (_net, mut a, mut b) = pair(dir.path(), mch_protocol::MINOR, &caps).await;
    a.negotiated().await;
    b.negotiated().await;
    for (n, m) in every_peer_msg(ObjectId([9; 16])).into_iter().enumerate() {
        let (from, to) = if n % 2 == 0 { (&a, &mut b) } else { (&b, &mut a) };
        from.send(m.clone()).await;
        let got = to.next("the peer message", |e| matches!(e, SessionEvent::Peer(_))).await;
        assert_eq!(got, SessionEvent::Peer(m));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn unnegotiated_frame_ends_session() {
    let dir = tempfile::tempdir().unwrap();
    let (net, mut a, mut b) = pair(dir.path(), 0, &["chat", "file"]).await;
    a.negotiated().await;
    b.negotiated().await;
    let cand = QsyCandidate { freq_hz: 7_105_000, mode: "".into(), passband_hz: 0, name: "".into() };
    let raw = QsyPropose { candidates: vec![cand], note: None }.to_frame(ObjectId([1; 16])).unwrap();
    net.inject(0, encode(&raw).unwrap());
    let end = tokio::time::timeout(Duration::from_secs(10), b.end).await.expect("B's session ended").unwrap();
    assert!(matches!(end, SessionEnd::Protocol(_)), "{end:?}");
}

/// M4 C1: a minor-1 peer (no file/2) is refused before the wire and never
/// receives 0x20..=0x26.
#[tokio::test(flavor = "multi_thread")]
async fn minor1_peer_never_receives_file_frames() {
    let dir = tempfile::tempdir().unwrap();
    let (net, mut a, mut b) = pair(dir.path(), 1, &["chat", "file", "qsy/1", "info/1"]).await;
    a.negotiated().await;
    b.negotiated().await;
    let src = dir.path().join("note.txt");
    std::fs::write(&src, b"hello over the air").unwrap();
    net.set_hold_from(0, true);
    a.cmds.send(SessionCommand::SendFile { path: src, opts: Default::default() }).await.unwrap();
    let r = a
        .next("FileRejected", |e| matches!(e, SessionEvent::FileRejected { .. } | SessionEvent::FileFailed { .. }))
        .await;
    assert!(
        matches!(&r, SessionEvent::FileRejected { reason, .. } if reason == "not supported by this station"),
        "{r:?}"
    );
    a.cmds.send(SessionCommand::SendChat { id: ObjectId([5; 16]), text: "chat still goes".into() }).await.unwrap();
    a.next("ChatSubmitted", |e| matches!(e, SessionEvent::ChatSubmitted { .. })).await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while net.held_len() == 0 {
        assert!(tokio::time::Instant::now() < deadline, "A sent nothing");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    let bytes: Vec<u8> = net.take_held().iter().flat_map(|(_, b)| b.clone()).collect();
    let mut d = Decoder::new();
    d.push(&bytes);
    while let Some(f) = d.next_frame().unwrap() {
        assert!(!(0x20..=0x26).contains(&(f.ftype as u8)), "a file frame reached the minor-1 peer: {:?}", f.ftype);
    }
}
