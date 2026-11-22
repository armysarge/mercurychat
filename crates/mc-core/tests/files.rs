//! M4 Task 7 (C2, C3): files through the session engine. Two stations over
//! the fake Mercury pair; one prepares a file, the other receives it.
#![cfg(feature = "sim")]

use mc_core::files::{prepare, PhotoSize, Prepared};
use mc_core::station::{FreeSpace, Station, StationConfig};
use mc_core::{SessionCommand, SessionEnd, SessionEvent};
use mc_storage::{Direction, Store, TransferState};
use mch_protocol::payload::Reason;
use mch_protocol::{encode, Decoder, FrameType, ObjectId};
use mercury_adapter::fake::FakeNet;
use mercury_adapter::{Callsign, Endpoint};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

const A: &str = "ZS6AAA";
const B: &str = "ZS6BBB";

struct Side {
    cmds: mpsc::Sender<SessionCommand>,
    events: mpsc::UnboundedReceiver<SessionEvent>,
    end: JoinHandle<SessionEnd>,
    dir: PathBuf,
}

impl Side {
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

    /// Nothing matching `pred` within `secs`.
    async fn none(&mut self, secs: u64, pred: impl Fn(&SessionEvent) -> bool) {
        let end = tokio::time::Instant::now() + Duration::from_secs(secs);
        while let Ok(Some(e)) = tokio::time::timeout_at(end, self.events.recv()).await {
            assert!(!pred(&e), "unexpected {e:?}");
        }
    }

    async fn send(&self, c: SessionCommand) {
        self.cmds.send(c).await.unwrap();
    }

    fn rx_dir(&self) -> PathBuf {
        self.dir.join("files")
    }

    /// The station database, opened beside the running station.
    fn store(&self) -> Store {
        Store::open(&self.dir.join("station.db")).unwrap()
    }

    fn row(&self, dir: Direction, id: ObjectId) -> mc_storage::TransferRow {
        self.store().transfer(dir, &id.0).unwrap().expect("a transfer row")
    }
}

/// The visible files in the receive folder (not `.partial`).
fn received(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).collect())
        .unwrap_or_default();
    v.retain(|n| !n.starts_with('.'));
    v.sort();
    v
}

/// Nothing left in a folder (temps, wire files).
fn empty_or_missing(dir: &Path) -> bool {
    std::fs::read_dir(dir).map(|mut rd| rd.next().is_none()).unwrap_or(true)
}

fn station(call: &str, base: u16, dir: &Path, tweak: &dyn Fn(&mut StationConfig)) -> Station {
    let mut cfg = StationConfig::defaults(Callsign::parse(call).unwrap(), Endpoint::base("127.0.0.1", base), dir);
    tweak(&mut cfg);
    let st = Station::open(cfg).unwrap();
    std::fs::create_dir_all(dir.join("files")).unwrap();
    st
}

fn run(mut st: Station, info: mc_core::ConnectInfo, dir: PathBuf) -> Side {
    let (cmds, mut cmd_rx) = mpsc::channel(32);
    let (ev_tx, events) = mpsc::unbounded_channel();
    let end = tokio::spawn(async move {
        let (_req_tx, mut req_rx) = mpsc::channel(4);
        st.run_session(&info, &mut cmd_rx, &mut req_rx, ev_tx).await
    });
    Side { cmds, events, end, dir }
}

/// A calls B; `tweak_b` sets B's station config.
async fn pair(root: &Path, tweak_b: &dyn Fn(&mut StationConfig)) -> (Arc<FakeNet>, Side, Side) {
    let net = FakeNet::start().await;
    let (da, db) = (root.join("a"), root.join("b"));
    let mut a = station(A, net.a_base, &da, &|_| {});
    let mut b = station(B, net.b_base, &db, tweak_b);
    a.attach().await.unwrap();
    b.attach().await.unwrap();
    b.set_listen(true).await.unwrap();
    let callee = Callsign::parse(B).unwrap();
    let (ia, ib) = tokio::join!(a.call(&callee, Duration::from_secs(10)), b.wait_incoming(Duration::from_secs(10)));
    let (mut a, mut b) = (run(a, ia.unwrap(), da), run(b, ib.unwrap(), db));
    for s in [&mut a, &mut b] {
        s.next(10, "Negotiated", |e| matches!(e, SessionEvent::Negotiated { .. })).await;
    }
    (net, a, b)
}

/// Bytes zstd can't shrink (a SHA-256 chain).
fn noise(n: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(n + 32);
    let mut h = Sha256::digest(b"MercuryChat");
    while out.len() < n {
        out.extend_from_slice(&h);
        h = Sha256::digest(h);
    }
    out.truncate(n);
    out
}

fn text(n: usize) -> Vec<u8> {
    "CQ CQ de ZS6AAA, the quick brown fox, 73.\n".repeat(n).into_bytes()
}

/// Prepare `bytes` as `name` from A's side.
fn prepared(side: &Side, name: &str, bytes: &[u8]) -> Prepared {
    let src = side.dir.join(name.rsplit(['/', '\\']).next().unwrap());
    std::fs::write(&src, bytes).unwrap();
    let mut p = prepare(&src, PhotoSize::Original, &side.dir.join("tmp"), 1 << 20).unwrap();
    p.name = name.to_string();
    p
}

async fn offer(a: &Side, b: &mut Side, p: Prepared) -> ObjectId {
    let id = ObjectId::random();
    a.send(SessionCommand::SendFile { id, prepared: p }).await;
    let o = b.next(10, "FileOffered", |e| matches!(e, SessionEvent::FileOffered { .. })).await;
    let SessionEvent::FileOffered { id: got, .. } = o else { unreachable!() };
    assert_eq!(got, id, "the offer carries the sender's transfer id");
    id
}

async fn received_ok(b: &mut Side) -> (PathBuf, String) {
    let r = b
        .next(30, "FileReceived", |e| matches!(e, SessionEvent::FileReceived { .. } | SessionEvent::FileFailed { .. }))
        .await;
    let SessionEvent::FileReceived { path, sha256, .. } = r else { panic!("{r:?}") };
    (path, sha256)
}

#[tokio::test(flavor = "multi_thread")]
async fn compressed_file_round_trip_sha256() {
    let dir = tempfile::tempdir().unwrap();
    let (_net, mut a, mut b) = pair(dir.path(), &|_| {}).await;
    let bytes = text(80);
    let p = prepared(&a, "notes.txt", &bytes);
    assert!(p.encoding.is_some() && p.wire_size < p.size);
    let id = offer(&a, &mut b, p).await;
    let o = b.row(Direction::In, id);
    assert_eq!((o.size, o.encoding.as_deref()), (bytes.len() as u64, Some("zstd")));
    b.send(SessionCommand::AcceptFile { id }).await;
    let (path, sha) = received_ok(&mut b).await;
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert_eq!(sha, hex::encode(Sha256::digest(&bytes)));
    a.next(10, "FileDelivered", |e| matches!(e, SessionEvent::FileDelivered { .. })).await;
    assert_eq!(a.row(Direction::Out, id).state, TransferState::Complete);
    assert!(empty_or_missing(&b.dir.join("tmp")), "no wire file left");
    assert!(empty_or_missing(&b.rx_dir().join(".partial")), "no partial file left");
    assert!(empty_or_missing(&a.dir.join("tmp")), "the sender's prepared file is gone");
}

#[tokio::test(flavor = "multi_thread")]
async fn plain_file_round_trip_sha256() {
    let dir = tempfile::tempdir().unwrap();
    let (_net, a, mut b) = pair(dir.path(), &|_| {}).await;
    let bytes = noise(3000);
    let p = prepared(&a, "random.bin", &bytes);
    assert_eq!(p.encoding, None);
    let id = offer(&a, &mut b, p).await;
    b.send(SessionCommand::AcceptFile { id }).await;
    let (path, _) = received_ok(&mut b).await;
    assert_eq!(std::fs::read(path).unwrap(), bytes);
}

#[tokio::test(flavor = "multi_thread")]
async fn truncated_never_completes() {
    let dir = tempfile::tempdir().unwrap();
    let (net, mut a, mut b) = pair(dir.path(), &|_| {}).await;
    let bytes = noise(20_000);
    let id = offer(&a, &mut b, prepared(&a, "big.bin", &bytes)).await;
    b.send(SessionCommand::AcceptFile { id }).await;
    b.next(10, "progress", |e| matches!(e, SessionEvent::FileProgress { .. })).await;
    net.drop_link();
    let _ = (&mut b.end).await;
    let _ = (&mut a.end).await;
    let row = b.row(Direction::In, id);
    assert_ne!(row.state, TransferState::Complete);
    assert_eq!(row.state, TransferState::Interrupted);
    assert!(received(&b.rx_dir()).is_empty(), "nothing published");
    assert!(empty_or_missing(&b.dir.join("tmp")) && empty_or_missing(&b.rx_dir().join(".partial")));
    assert_eq!(a.row(Direction::Out, id).state, TransferState::Interrupted);
}

#[tokio::test(flavor = "multi_thread")]
async fn hash_mismatch_no_receipt() {
    let dir = tempfile::tempdir().unwrap();
    let (_net, mut a, mut b) = pair(dir.path(), &|_| {}).await;
    let mut p = prepared(&a, "notes.txt", &text(40));
    p.sha256 = hex::encode(Sha256::digest(b"not the file"));
    let id = offer(&a, &mut b, p).await;
    b.send(SessionCommand::AcceptFile { id }).await;
    let r = b
        .next(30, "FileFailed", |e| matches!(e, SessionEvent::FileFailed { .. } | SessionEvent::FileReceived { .. }))
        .await;
    assert!(matches!(&r, SessionEvent::FileFailed { reason, .. } if reason.starts_with("hash_mismatch")), "{r:?}");
    a.none(3, |e| matches!(e, SessionEvent::FileDelivered { .. })).await;
    assert!(received(&b.rx_dir()).is_empty());
    assert_eq!(b.row(Direction::In, id).state, TransferState::Failed);
}

#[tokio::test(flavor = "multi_thread")]
async fn disk_full_declines() {
    let dir = tempfile::tempdir().unwrap();
    let (_net, mut a, mut b) = pair(dir.path(), &|c| c.free_space = FreeSpace::fixed(10)).await;
    let id = ObjectId::random();
    a.send(SessionCommand::SendFile { id, prepared: prepared(&a, "notes.txt", &text(40)) }).await;
    let r = a.next(10, "FileRejected", |e| matches!(e, SessionEvent::FileRejected { .. })).await;
    assert!(matches!(&r, SessionEvent::FileRejected { reason, .. } if reason.contains("no space")), "{r:?}");
    b.none(1, |e| matches!(e, SessionEvent::FileOffered { .. })).await;
    assert_eq!(b.row(Direction::In, id).state, TransferState::Rejected);
}

#[tokio::test(flavor = "multi_thread")]
async fn write_error_fails_cleanly() {
    let dir = tempfile::tempdir().unwrap();
    // B's wire folder is a file: nothing can be written there.
    let blocker = dir.path().join("b-tmp-is-a-file");
    std::fs::write(&blocker, b"x").unwrap();
    let tmp = blocker.clone();
    let (_net, mut a, mut b) = pair(dir.path(), &move |c| c.tmp_dir = Some(tmp.clone())).await;
    let id = offer(&a, &mut b, prepared(&a, "notes.txt", &text(40))).await;
    b.send(SessionCommand::AcceptFile { id }).await;
    let r = a
        .next(10, "refused", |e| matches!(e, SessionEvent::FileRejected { .. } | SessionEvent::FileFailed { .. }))
        .await;
    assert!(!matches!(r, SessionEvent::FileDelivered { .. }));
    assert!(received(&b.rx_dir()).is_empty());
    assert!(!b.row(Direction::In, id).state.eq(&TransferState::Complete));
}

#[tokio::test(flavor = "multi_thread")]
async fn oversize_declined_before_operator() {
    let dir = tempfile::tempdir().unwrap();
    let (_net, mut a, mut b) = pair(dir.path(), &|c| c.max_file = 1000).await;
    let id = ObjectId::random();
    a.send(SessionCommand::SendFile { id, prepared: prepared(&a, "notes.txt", &text(40)) }).await;
    let r = a.next(10, "FileRejected", |e| matches!(e, SessionEvent::FileRejected { .. })).await;
    assert!(matches!(&r, SessionEvent::FileRejected { reason, .. } if reason.contains("too large")), "{r:?}");
    b.none(1, |e| matches!(e, SessionEvent::FileOffered { .. })).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn traversal_name_stays_in_folder() {
    let dir = tempfile::tempdir().unwrap();
    let (_net, a, mut b) = pair(dir.path(), &|_| {}).await;
    let id = offer(&a, &mut b, prepared(&a, "../../evil.txt", &text(5))).await;
    b.send(SessionCommand::AcceptFile { id }).await;
    let (path, _) = received_ok(&mut b).await;
    assert_eq!(path.parent().unwrap(), b.rx_dir());
    assert_eq!(received(&b.rx_dir()), ["evil.txt"]);
    assert!(!dir.path().join("evil.txt").exists() && !b.dir.join("evil.txt").exists());
    let row = b.row(Direction::In, id);
    assert_eq!((row.original_name.as_str(), row.saved_name.as_deref()), ("../../evil.txt", Some("evil.txt")));
}

#[tokio::test(flavor = "multi_thread")]
async fn offer_times_out() {
    let dir = tempfile::tempdir().unwrap();
    let (_net, mut a, mut b) = pair(dir.path(), &|c| c.offer_timeout = Duration::from_secs(2)).await;
    let id = offer(&a, &mut b, prepared(&a, "notes.txt", &text(5))).await;
    let t0 = tokio::time::Instant::now();
    let r = a.next(10, "FileRejected", |e| matches!(e, SessionEvent::FileRejected { .. })).await;
    assert!(matches!(&r, SessionEvent::FileRejected { reason, .. } if reason.contains("timeout")), "{r:?}");
    assert!(t0.elapsed() >= Duration::from_millis(1500), "not before the window");
    assert_eq!(b.row(Direction::In, id).state, TransferState::Rejected);
}

#[tokio::test(flavor = "multi_thread")]
async fn extend_once_then_times_out() {
    let dir = tempfile::tempdir().unwrap();
    let (_net, mut a, mut b) = pair(dir.path(), &|c| c.offer_timeout = Duration::from_secs(2)).await;
    let id = offer(&a, &mut b, prepared(&a, "notes.txt", &text(5))).await;
    let t0 = tokio::time::Instant::now();
    tokio::time::sleep(Duration::from_secs(1)).await;
    b.send(SessionCommand::ExtendFile { id }).await;
    b.send(SessionCommand::ExtendFile { id }).await; // a second one is ignored
    let r = a.next(10, "FileRejected", |e| matches!(e, SessionEvent::FileRejected { .. })).await;
    assert!(matches!(&r, SessionEvent::FileRejected { reason, .. } if reason.contains("timeout")), "{r:?}");
    let took = t0.elapsed();
    assert!(took >= Duration::from_millis(3500) && took < Duration::from_millis(5800), "{took:?}");
}

/// Review Focus 2: a cancel right behind FILE_END, while B is verifying.
#[tokio::test(flavor = "multi_thread")]
async fn cancel_before_publish_discards() {
    let dir = tempfile::tempdir().unwrap();
    let (net, mut a, mut b) = pair(dir.path(), &|_| {}).await;
    let id = offer(&a, &mut b, prepared(&a, "notes.txt", &text(200))).await;
    net.set_hold_from(0, true);
    b.send(SessionCommand::AcceptFile { id }).await;
    a.next(10, "sending", |e| matches!(e, SessionEvent::FileProgress { .. })).await;
    // Wait until everything A sends (the chunks and FILE_END) is held.
    let mut bytes = Vec::new();
    let end = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        bytes.extend(net.take_held().into_iter().flat_map(|(_, b)| b));
        let mut d = Decoder::new();
        d.push(&bytes);
        let mut ended = false;
        while let Ok(Some(f)) = d.next_frame() {
            ended |= f.ftype == FrameType::FileEnd;
        }
        if ended {
            break;
        }
        assert!(tokio::time::Instant::now() < end, "A never sent FILE_END");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let cancel = Reason::new("user", "cancelled").to_frame(FrameType::FileCancel, id).unwrap();
    bytes.extend(encode(&cancel).unwrap());
    net.set_hold_from(0, false);
    net.inject(0, bytes);
    let r = b
        .next(10, "FileCancelled", |e| {
            matches!(e, SessionEvent::FileCancelled { .. } | SessionEvent::FileReceived { .. })
        })
        .await;
    assert!(matches!(r, SessionEvent::FileCancelled { by_peer: true, .. }), "{r:?}");
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(received(&b.rx_dir()).is_empty(), "nothing published");
    assert_eq!(b.row(Direction::In, id).state, TransferState::Cancelled);
    a.none(2, |e| matches!(e, SessionEvent::FileDelivered { .. })).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn receipt_only_after_complete_row() {
    let dir = tempfile::tempdir().unwrap();
    let (_net, mut a, mut b) = pair(dir.path(), &|_| {}).await;
    let id = offer(&a, &mut b, prepared(&a, "notes.txt", &text(30))).await;
    b.send(SessionCommand::AcceptFile { id }).await;
    a.next(30, "FileDelivered", |e| matches!(e, SessionEvent::FileDelivered { .. })).await;
    let row = b.row(Direction::In, id);
    assert_eq!(row.state, TransferState::Complete, "complete before the sender saw the receipt");
    assert!(row.saved_path.is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn sender_cancel_is_seen_by_both() {
    let dir = tempfile::tempdir().unwrap();
    let (_net, mut a, mut b) = pair(dir.path(), &|_| {}).await;
    let id = offer(&a, &mut b, prepared(&a, "notes.txt", &text(30))).await;
    a.send(SessionCommand::CancelFile { id }).await;
    let r = b.next(10, "FileCancelled", |e| matches!(e, SessionEvent::FileCancelled { .. })).await;
    assert!(matches!(r, SessionEvent::FileCancelled { by_peer: true, .. }));
    a.next(10, "own FileCancelled", |e| matches!(e, SessionEvent::FileCancelled { by_peer: false, .. })).await;
    assert_eq!(a.row(Direction::Out, id).state, TransferState::Cancelled);
    assert_eq!(b.row(Direction::In, id).state, TransferState::Cancelled);
}
