//! M2 Task 6: the outbox and chat through the service (CHT-01 to CHT-04).
//! The simulator's SIM1 echoes; the FakeNet tests run a SIM1 of their own on
//! side B so the link can be held, dropped and replayed.
#![cfg(feature = "sim")]

use mc_core::app::{
    spawn_app, AppCommand, AppError, AppEvent, AppHandle, AppOptions, ChatMessage, CommandOk, EventEnvelope, ModemState,
};
use mc_core::profile::{ModemTarget, StationProfile};
use mc_core::station::{Station, StationConfig};
use mc_core::SessionCommand;
use mc_storage::Store;
use mch_protocol::ObjectId;
use mercury_adapter::fake::FakeNet;
use mercury_adapter::{Callsign, Endpoint};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::sync::broadcast::Receiver;
use tokio::sync::mpsc;

const PEER: &str = "SIM1";

fn opts(root: &Path) -> AppOptions {
    AppOptions {
        sim_root: Some(root.to_path_buf()),
        stations_root: Some(root.join("stations")),
        ..AppOptions::default()
    }
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

async fn next(rx: &mut Receiver<EventEnvelope>, secs: u64) -> AppEvent {
    tokio::time::timeout(Duration::from_secs(secs), rx.recv()).await.expect("an event in time").unwrap().event
}

async fn until(rx: &mut Receiver<EventEnvelope>, want: ModemState, secs: u64) {
    loop {
        if let AppEvent::ModemState { state, detail } = next(rx, secs).await {
            if state == want {
                return;
            }
            assert_ne!(state, ModemState::Faulted, "faulted: {detail:?}");
        }
    }
}

/// The next change to message `id` (or any incoming message when `id` is None).
async fn message(rx: &mut Receiver<EventEnvelope>, id: Option<&str>, secs: u64) -> ChatMessage {
    loop {
        if let AppEvent::MessageChanged { message } = next(rx, secs).await {
            match id {
                Some(id) if message.id == id => return message,
                None if message.direction == "in" => return message,
                _ => {}
            }
        }
    }
}

/// Follow message `id` until it reaches `last`; every state seen, in order.
async fn states_of(rx: &mut Receiver<EventEnvelope>, id: &str, last: &str, secs: u64) -> Vec<String> {
    let mut seen = Vec::new();
    loop {
        let m = message(rx, Some(id), secs).await;
        seen.push(m.state.clone());
        if m.state == last {
            return seen;
        }
    }
}

struct App {
    h: AppHandle,
    rx: Receiver<EventEnvelope>,
    id: String,
}

async fn start(db: &Path, root: &Path, profile: Option<StationProfile>) -> App {
    let h = spawn_app(Store::open(db).unwrap(), opts(root));
    let mut rx = h.subscribe();
    let id = match profile {
        Some(p) => {
            let id = match h.call(AppCommand::SaveProfile { profile: p }).await.unwrap() {
                CommandOk::Profile(p) => p.id,
                other => panic!("{other:?}"),
            };
            h.call(AppCommand::SelectProfile { id: Some(id.clone()) }).await.unwrap();
            id
        }
        None => match h.call(AppCommand::Snapshot).await.unwrap() {
            CommandOk::Snapshot(s) => s.active_profile_id.expect("a selected profile"),
            other => panic!("{other:?}"),
        },
    };
    until(&mut rx, ModemState::Ready, 15).await;
    App { h, rx, id }
}

async fn sim(root: &Path) -> App {
    start(&root.join("app.db"), root, Some(StationProfile::new_simulator("ZS6ABC"))).await
}

async fn send(h: &AppHandle, peer: &str, text: &str) -> Result<ChatMessage, AppError> {
    match h.call(AppCommand::SendChat { peer: peer.into(), text: text.into() }).await? {
        CommandOk::Message(m) => Ok(m),
        other => panic!("{other:?}"),
    }
}

async fn connect(app: &mut App) {
    app.h.call(AppCommand::Call { peer: PEER.into() }).await.unwrap();
    until(&mut app.rx, ModemState::Connected, 15).await;
}

async fn history(h: &AppHandle, peer: &str) -> Vec<ChatMessage> {
    match h.call(AppCommand::History { peer: peer.into(), before_seq: None, limit: 200 }).await.unwrap() {
        CommandOk::History { messages } => messages,
        other => panic!("{other:?}"),
    }
}

fn text_error(r: Result<ChatMessage, AppError>) -> String {
    match r {
        Err(AppError::Validation { errors }) => {
            assert_eq!(errors[0].field, "text");
            errors[0].message.clone()
        }
        other => panic!("expected a text Validation error, got {other:?}"),
    }
}

/// What a peer station's store holds from ZS6ABC.
fn received_by(peer_db: &Path) -> Vec<(String, [u8; 16])> {
    Store::open(peer_db)
        .unwrap()
        .messages("ZS6ABC")
        .unwrap()
        .into_iter()
        .filter(|m| m.direction == "in")
        .map(|m| (m.body, m.wire_id))
        .collect()
}

fn sim_peer_db(root: &Path, id: &str) -> PathBuf {
    root.join("stations").join(id).join("sim-peer").join("station.db")
}

/// SIM1 on side B of `net`: listens, runs every session, and at the start of
/// each one sends `chats` (ids may repeat).
fn spawn_peer(net: &FakeNet, dir: &Path, chats: Vec<(ObjectId, &'static str)>) -> tokio::task::JoinHandle<()> {
    let call = Callsign::parse(PEER).unwrap();
    let mut st = Station::open(StationConfig::defaults(call, Endpoint::base("127.0.0.1", net.b_base), dir)).unwrap();
    tokio::spawn(async move {
        st.attach().await.unwrap();
        st.set_listen(true).await.unwrap();
        loop {
            let Ok(info) = st.wait_incoming(Duration::from_secs(3600)).await else { return };
            let (cmd_tx, mut cmd_rx) = mpsc::channel(64);
            for (id, text) in &chats {
                cmd_tx.send(SessionCommand::SendChat { id: *id, text: text.to_string() }).await.unwrap();
            }
            let (_req_tx, mut req_rx) = mpsc::channel(1);
            let (ev_tx, _ev_rx) = mpsc::unbounded_channel();
            st.run_session(&info, &mut cmd_rx, &mut req_rx, ev_tx).await;
        }
    })
}

async fn wait_for<F: Fn() -> bool>(what: &str, f: F) {
    let end = tokio::time::Instant::now() + Duration::from_secs(10);
    while !f() {
        assert!(tokio::time::Instant::now() < end, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn send_while_connected_reaches_delivered() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = sim(dir.path()).await;
    connect(&mut app).await;
    let m = send(&app.h, PEER, "hi").await.unwrap();
    assert_eq!((m.state.as_str(), m.direction.as_str(), m.peer.as_str()), ("queued", "out", PEER));
    assert_eq!(m.id.len(), 32);
    // The queued event is on the stream too, before the modem takes it.
    let seen = states_of(&mut app.rx, &m.id, "delivered", 15).await;
    assert_eq!(seen, ["queued", "submitting", "awaiting_receipt", "delivered"]);
    let echo = message(&mut app.rx, None, 15).await;
    assert_eq!((echo.body.as_str(), echo.peer.as_str()), ("echo: hi", PEER));
    let h = history(&app.h, PEER).await;
    assert_eq!(h.iter().map(|m| m.body.as_str()).collect::<Vec<_>>(), ["hi", "echo: hi"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn unicode_and_newlines_survive() {
    let text = "héllo 👋\r\nline two\n\tשלום";
    let dir = tempfile::tempdir().unwrap();
    let mut app = sim(dir.path()).await;
    connect(&mut app).await;
    let m = send(&app.h, PEER, text).await.unwrap();
    states_of(&mut app.rx, &m.id, "delivered", 15).await;
    let echo = message(&mut app.rx, None, 15).await;
    assert_eq!(echo.body.as_bytes(), format!("echo: {text}").as_bytes());
    let at_sim1 = received_by(&sim_peer_db(dir.path(), &app.id));
    assert_eq!(at_sim1.len(), 1);
    assert_eq!(at_sim1[0].0.as_bytes(), text.as_bytes());
}

#[tokio::test(flavor = "multi_thread")]
async fn limit_4096_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let app = sim(dir.path()).await;
    assert!(send(&app.h, PEER, &"a".repeat(4096)).await.is_ok());
    assert!(send(&app.h, PEER, &"é".repeat(2048)).await.is_ok(), "2,048 two-byte characters are 4,096 bytes");
    let e = text_error(send(&app.h, PEER, &format!("{}a", "é".repeat(2048))).await);
    assert!(e.contains("4096") || e.contains("4,096"), "{e}");
    text_error(send(&app.h, PEER, "").await);
    text_error(send(&app.h, PEER, " \n ").await);
    assert_eq!(history(&app.h, PEER).await.len(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn queue_quota_per_peer() {
    let dir = tempfile::tempdir().unwrap();
    let app = sim(dir.path()).await;
    for i in 0..100 {
        send(&app.h, "ZS1AAA", &format!("m{i}")).await.unwrap();
    }
    let e = text_error(send(&app.h, "ZS1AAA", "one too many").await);
    assert!(e.contains("100"), "{e}");
    // The quota is per peer, and counts bytes too: 64 × 4 KiB is 256 KiB.
    let big = "b".repeat(4096);
    for _ in 0..64 {
        send(&app.h, "ZS1BBB", &big).await.unwrap();
    }
    let e = text_error(send(&app.h, "ZS1BBB", "x").await);
    assert!(e.contains("256 KiB"), "{e}");
    assert!(send(&app.h, "ZS1CCC", "another peer is fine").await.is_ok());
}

#[tokio::test(flavor = "multi_thread")]
async fn send_while_calling_waits_for_hello() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = sim(dir.path()).await;
    app.h.call(AppCommand::Call { peer: PEER.into() }).await.unwrap();
    let m = send(&app.h, PEER, "early").await.unwrap();
    assert_eq!(m.state, "queued");
    // Nothing of it may be submitted before the session is negotiated.
    let mut connected = false;
    loop {
        match next(&mut app.rx, 15).await {
            AppEvent::ModemState { state: ModemState::Connected, .. } => connected = true,
            AppEvent::MessageChanged { message } if message.id == m.id => {
                assert!(connected || message.state == "queued", "{} before negotiation", message.state);
                if message.state == "delivered" {
                    break;
                }
            }
            _ => {}
        }
    }
    assert_eq!(received_by(&sim_peer_db(dir.path(), &app.id)).len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn restart_keeps_queued_and_sends_once() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("app.db");
    let (id, msg) = {
        let app = start(&db, dir.path(), Some(StationProfile::new_simulator("ZS6ABC"))).await;
        let m = send(&app.h, PEER, "survives").await.unwrap();
        (app.id, m)
    }; // the service stops before anything was submitted
    tokio::time::sleep(Duration::from_millis(300)).await;
    let mut app = start(&db, dir.path(), None).await;
    assert_eq!(app.id, id);
    let before = history(&app.h, PEER).await;
    assert_eq!(before.len(), 1);
    assert_eq!((before[0].id.as_str(), before[0].state.as_str()), (msg.id.as_str(), "queued"));
    connect(&mut app).await;
    states_of(&mut app.rx, &msg.id, "delivered", 15).await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    let at_sim1 = received_by(&sim_peer_db(dir.path(), &id));
    assert_eq!(at_sim1.iter().filter(|(b, _)| b == "survives").count(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn drop_then_late_receipt_is_delivered() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let _peer = spawn_peer(&net, &dir.path().join("peer"), vec![]);
    let mut app = start(&dir.path().join("app.db"), dir.path(), Some(existing(net.a_base))).await;
    connect(&mut app).await;
    net.set_hold_from(1, true); // SIM1's receipt stays in its modem
    let m = send(&app.h, PEER, "late").await.unwrap();
    states_of(&mut app.rx, &m.id, "awaiting_receipt", 10).await;
    wait_for("the receipt to be held", || net.held_len() > 0).await;
    let receipt = net.take_held();
    net.set_hold_from(1, false);
    net.kill_client(0); // our sockets drop before the receipt arrives
    states_of(&mut app.rx, &m.id, "uncertain", 15).await;
    until(&mut app.rx, ModemState::Ready, 60).await;

    connect(&mut app).await;
    for (from, bytes) in receipt {
        net.inject(from, bytes); // the old receipt reaches us in the new session
    }
    let last = message(&mut app.rx, Some(&m.id), 10).await;
    assert_eq!(last.state, "delivered", "a late receipt is the truth");
    assert_eq!(received_by(&dir.path().join("peer").join("station.db")).len(), 1, "and nothing was resent");
}

#[tokio::test(flavor = "multi_thread")]
async fn uncertain_is_not_resent_on_reconnect() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let _peer = spawn_peer(&net, &dir.path().join("peer"), vec![]);
    let mut app = start(&dir.path().join("app.db"), dir.path(), Some(existing(net.a_base))).await;
    connect(&mut app).await;
    net.set_hold_from(0, true); // our chat never leaves our modem
    let m = send(&app.h, PEER, "lost?").await.unwrap();
    states_of(&mut app.rx, &m.id, "awaiting_receipt", 10).await;
    net.take_held();
    net.set_hold_from(0, false);
    net.kill_client(0);
    states_of(&mut app.rx, &m.id, "uncertain", 15).await;
    until(&mut app.rx, ModemState::Ready, 60).await;
    connect(&mut app).await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(received_by(&dir.path().join("peer").join("station.db")).is_empty(), "an uncertain message was resent");
    assert_eq!(history(&app.h, PEER).await[0].state, "uncertain");
}

#[tokio::test(flavor = "multi_thread")]
async fn retry_uses_the_same_id_and_appears_once_remotely() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let _peer = spawn_peer(&net, &dir.path().join("peer"), vec![]);
    let mut app = start(&dir.path().join("app.db"), dir.path(), Some(existing(net.a_base))).await;
    connect(&mut app).await;
    net.set_hold_from(1, true); // SIM1 stores it, but its receipt is lost
    let m = send(&app.h, PEER, "once").await.unwrap();
    states_of(&mut app.rx, &m.id, "awaiting_receipt", 10).await;
    wait_for("the receipt to be held", || net.held_len() > 0).await;
    net.take_held();
    net.set_hold_from(1, false);
    net.kill_client(0);
    states_of(&mut app.rx, &m.id, "uncertain", 15).await;
    until(&mut app.rx, ModemState::Ready, 60).await;
    connect(&mut app).await;

    let r = app.h.call(AppCommand::RetryChat { peer: PEER.into(), id: m.id.clone() }).await.unwrap();
    let CommandOk::Message(r) = r else { panic!("{r:?}") };
    assert_eq!(r.id, m.id);
    let seen = states_of(&mut app.rx, &m.id, "delivered", 15).await;
    assert_eq!(seen.first().map(String::as_str), Some("queued"));
    let at_sim1 = received_by(&dir.path().join("peer").join("station.db"));
    assert_eq!(at_sim1.len(), 1, "the retry was stored once at SIM1");
    assert_eq!(hex::encode(at_sim1[0].1), m.id);
    match app.h.call(AppCommand::RetryChat { peer: PEER.into(), id: m.id.clone() }).await {
        Err(AppError::Conflict { .. }) => {}
        other => panic!("a delivered message cannot be retried: {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn duplicate_incoming_shows_once() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let id = ObjectId::random();
    let twice = vec![(id, "same"), (id, "same")];
    let _peer = spawn_peer(&net, &dir.path().join("peer"), twice);
    let mut app = start(&dir.path().join("app.db"), dir.path(), Some(existing(net.a_base))).await;
    connect(&mut app).await;
    let first = message(&mut app.rx, None, 10).await;
    assert_eq!(first.id, id.to_hex());
    tokio::time::sleep(Duration::from_secs(1)).await;
    while let Ok(env) = app.rx.try_recv() {
        if let AppEvent::MessageChanged { message } = env.event {
            assert_ne!(message.direction, "in", "the duplicate was shown: {message:?}");
        }
    }
    let h = history(&app.h, PEER).await;
    assert_eq!(h.iter().filter(|m| m.direction == "in").count(), 1);
}

/// Every chat reply crosses the IPC boundary as tagged JSON (docs/02 section 2).
#[tokio::test(flavor = "multi_thread")]
async fn chat_replies_serialize_for_the_window() {
    let dir = tempfile::tempdir().unwrap();
    let app = sim(dir.path()).await;
    let m = send(&app.h, PEER, "json").await.unwrap();
    let replies = [
        CommandOk::Message(m),
        app.h.call(AppCommand::Conversations).await.unwrap(),
        app.h.call(AppCommand::History { peer: PEER.into(), before_seq: None, limit: 10 }).await.unwrap(),
    ];
    for r in replies {
        let v = serde_json::to_value(&r).unwrap_or_else(|e| panic!("{r:?} does not serialize: {e}"));
        assert!(v.get("type").is_some(), "{v}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn draft_survives_restart_and_is_never_sent() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("app.db");
    let id = {
        let app = start(&db, dir.path(), Some(StationProfile::new_simulator("ZS6ABC"))).await;
        let r = app.h.call(AppCommand::SaveDraft { peer: PEER.into(), text: "not yet\nsent".into() }).await;
        assert_eq!(r.unwrap(), CommandOk::Done);
        app.id
    };
    tokio::time::sleep(Duration::from_millis(300)).await;
    let mut app = start(&db, dir.path(), None).await;
    let convs = match app.h.call(AppCommand::Conversations).await.unwrap() {
        CommandOk::Conversations { conversations } => conversations,
        other => panic!("{other:?}"),
    };
    let c = convs.iter().find(|c| c.peer == PEER).expect("the draft's conversation");
    assert_eq!(c.draft.as_deref(), Some("not yet\nsent"));
    connect(&mut app).await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(received_by(&sim_peer_db(dir.path(), &id)).is_empty(), "a draft was transmitted");
    assert!(history(&app.h, PEER).await.is_empty());
    // An empty draft clears it.
    app.h.call(AppCommand::SaveDraft { peer: PEER.into(), text: String::new() }).await.unwrap();
    let convs = match app.h.call(AppCommand::Conversations).await.unwrap() {
        CommandOk::Conversations { conversations } => conversations,
        other => panic!("{other:?}"),
    };
    assert!(convs.iter().all(|c| c.draft.is_none()));
}
