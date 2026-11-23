//! Files over Mercury's ARQ link through MercuryChat's own session engine
//! (M4): a compressed file arrives byte-exact with its receipt, and at BW500
//! a chat sent during a file is not held behind it (C4: file data in the
//! modem queue is bounded by airtime).

use mc_core::files::{prepare, PhotoSize, Prepared};
use mc_core::station::{Station, StationConfig, RADIO_HELLO_TIMEOUT};
use mc_core::{SessionCommand, SessionEvent};
use mch_protocol::ObjectId;
use mercury_adapter::Bandwidth;
use mercury_contract::{contract, Tag, Target, TargetKind};
use std::path::Path;
use std::time::Duration;
use tokio::sync::mpsc;

struct Side {
    cmds: mpsc::Sender<SessionCommand>,
    events: mpsc::UnboundedReceiver<SessionEvent>,
}

impl Side {
    async fn next(&mut self, within: Duration, what: &str, pred: impl Fn(&SessionEvent) -> bool) -> SessionEvent {
        let end = tokio::time::Instant::now() + within;
        loop {
            match tokio::time::timeout_at(end, self.events.recv()).await {
                Ok(Some(e)) if pred(&e) => return e,
                Ok(Some(_)) => {}
                Ok(None) => panic!("waiting for {what}: the session ended"),
                Err(_) => panic!("no {what} within {within:?}"),
            }
        }
    }
}

fn run(mut st: Station, info: mc_core::ConnectInfo) -> Side {
    let (cmds, mut cmd_rx) = mpsc::channel(32);
    let (ev_tx, events) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        let (_req_tx, mut req_rx) = mpsc::channel(4);
        st.run_session(&info, &mut cmd_rx, &mut req_rx, ev_tx).await
    });
    Side { cmds, events }
}

/// A calls B at `bw`, both running MercuryChat's session; B takes files
/// without asking (the lab peer's setting).
async fn engines(t: &Target, root: &Path, bw: Bandwidth) -> (Side, Side) {
    let cfg = |st: &mercury_contract::Station, dir: &Path| {
        let mut c = StationConfig::defaults(st.call.clone(), st.ep.clone(), dir);
        c.bandwidth = bw;
        c.hello_timeout = if t.kind == TargetKind::Real { RADIO_HELLO_TIMEOUT } else { Duration::from_secs(10) };
        c.timeouts = t.t;
        c
    };
    let mut a = Station::open(cfg(&t.a, &root.join("a"))).unwrap();
    let mut bc = cfg(&t.b, &root.join("b"));
    bc.auto_accept_files = true;
    let mut b = Station::open(bc).unwrap();
    a.attach().await.expect("A attaches");
    b.attach().await.expect("B attaches");
    b.set_listen(true).await.expect("B listens");
    let (ia, ib) = tokio::join!(a.call(&t.b.call, t.arq_timeout()), b.wait_incoming(t.arq_timeout()));
    let (mut a, mut b) = (run(a, ia.expect("A's call connects")), run(b, ib.expect("B sees the call")));
    let hello = if t.kind == TargetKind::Real { RADIO_HELLO_TIMEOUT } else { Duration::from_secs(10) };
    for s in [&mut a, &mut b] {
        s.next(hello, "Negotiated", |e| matches!(e, SessionEvent::Negotiated { .. })).await;
    }
    (a, b)
}

fn prepared(root: &Path, name: &str, bytes: &[u8]) -> Prepared {
    let src = root.join(name);
    std::fs::write(&src, bytes).unwrap();
    prepare(&src, PhotoSize::Original, &root.join("prep"), 1 << 20).unwrap()
}

/// Bytes zstd can't shrink (a SHA-256 chain).
fn noise(n: usize) -> Vec<u8> {
    use sha2::{Digest, Sha256};
    let mut out = Vec::with_capacity(n + 32);
    let mut h = Sha256::digest(b"chat during a file");
    while out.len() < n {
        out.extend_from_slice(&h);
        h = Sha256::digest(h);
    }
    out.truncate(n);
    out
}

#[tokio::test(flavor = "multi_thread")]
async fn compressed_file_crosses_arq() {
    contract("compressed_file_crosses_arq", Tag::Both, |t| async move {
        let dir = tempfile::tempdir().unwrap();
        let (a, mut b) = engines(&t, dir.path(), Bandwidth::Bw2300).await;
        let text = "MercuryChat over HF: a 16 KiB log of plain text, 73.\n".repeat(16 * 1024 / 52 + 1);
        let text = &text.as_bytes()[..16 * 1024];
        let p = prepared(dir.path(), "log.txt", text);
        assert!(p.encoding.is_some() && p.wire_size < p.size / 4, "zstd shrinks text: {} of {}", p.wire_size, p.size);
        let id = ObjectId::random();
        a.cmds.send(SessionCommand::SendFile { id, prepared: p }).await.unwrap();
        let within = if t.kind == TargetKind::Real { Duration::from_secs(300) } else { Duration::from_secs(20) };
        let r = b
            .next(within, "FileReceived", |e| {
                matches!(e, SessionEvent::FileReceived { .. } | SessionEvent::FileFailed { .. })
            })
            .await;
        let SessionEvent::FileReceived { path, sha256, .. } = r else { panic!("{r:?}") };
        assert_eq!(std::fs::read(&path).unwrap(), text, "byte-identical after decompression");
        use sha2::Digest;
        assert_eq!(sha256, hex::encode(sha2::Sha256::digest(text)));
        let mut a = a;
        a.next(within, "FileDelivered", |e| matches!(e, SessionEvent::FileDelivered { id: got } if *got == id)).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn chat_during_file_at_bw500() {
    contract("chat_during_file_at_bw500", Tag::Both, |t| async move {
        let real = t.kind == TargetKind::Real;
        // The fake moves 200 B/s each way: slow enough that the file's 4 KiB
        // is more than its 12 s window, so chat can only win on order.
        if let Some(net) = t.fake() {
            net.set_drain_rate(0, 200.0);
            net.set_drain_rate(1, 200.0);
        }
        let dir = tempfile::tempdir().unwrap();
        let (mut a, mut b) = engines(&t, dir.path(), Bandwidth::Bw500).await;
        let p = prepared(dir.path(), "noise.bin", &noise(4096));
        assert_eq!(p.encoding, None, "incompressible");
        a.cmds.send(SessionCommand::SendFile { id: ObjectId::random(), prepared: p }).await.unwrap();
        let first = if real { Duration::from_secs(300) } else { Duration::from_secs(30) };
        a.next(first, "the first chunk", |e| matches!(e, SessionEvent::FileProgress { .. })).await;
        let chat = ObjectId::random();
        let sent = tokio::time::Instant::now();
        a.cmds.send(SessionCommand::SendChat { id: chat, text: "73, chat during a file".into() }).await.unwrap();
        a.next(
            Duration::from_secs(120),
            "the chat's receipt",
            move |e| matches!(e, SessionEvent::ChatDelivered { id, .. } if *id == chat),
        )
        .await;
        let waited = sent.elapsed();
        println!("NOTE chat_during_file_at_bw500 chat receipted {} ms after it was sent, mid-file", waited.as_millis());
        if real {
            assert!(waited < Duration::from_secs(30), "the chat waited {waited:?} behind the file (C4: under 30 s)");
        } else {
            // The chat overtook the rest of the file: B has it before the file ends.
            let mut chat_first = false;
            loop {
                match b
                    .next(Duration::from_secs(60), "B's chat and file", |e| {
                        matches!(
                            e,
                            SessionEvent::ChatReceived { .. }
                                | SessionEvent::FileReceived { .. }
                                | SessionEvent::FileFailed { .. }
                        )
                    })
                    .await
                {
                    SessionEvent::ChatReceived { .. } => chat_first = true,
                    SessionEvent::FileReceived { .. } => break,
                    other => panic!("{other:?}"),
                }
            }
            assert!(chat_first, "the chat arrived after the whole file");
        }
    })
    .await;
}
