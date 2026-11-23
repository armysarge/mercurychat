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
    pair_with(root, &|_| {}, tweak_b, |_| {}).await
}

/// A calls B with both configs set, and `net` set up before the call.
async fn pair_with(
    root: &Path,
    tweak_a: &dyn Fn(&mut StationConfig),
    tweak_b: &dyn Fn(&mut StationConfig),
    net_setup: impl FnOnce(&FakeNet),
) -> (Arc<FakeNet>, Side, Side) {
    let net = FakeNet::start().await;
    net_setup(&net);
    let (da, db) = (root.join("a"), root.join("b"));
    let mut a = station(A, net.a_base, &da, tweak_a);
    let mut b = station(B, net.b_base, &db, tweak_b);
    a.attach().await.unwrap();
    b.attach().await.unwrap();
    b.set_listen(true).await.unwrap();
    let callee = Callsign::parse(B).unwrap();
    let (ia, ib) = tokio::join!(a.call(&callee, Duration::from_secs(10)), b.wait_incoming(Duration::from_secs(10)));
    let (mut a, mut b) = (run(a, ia.unwrap(), da), run(b, ib.unwrap(), db));
    for s in [&mut a, &mut b] {
        s.next(60, "Negotiated", |e| matches!(e, SessionEvent::Negotiated { .. })).await;
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

/// M4 C4 / G5: at BW500 (19 B/s on the air, both ways) a chat sent while a
/// file is going is receipted within 30 s: file data in the modem queue is
/// bounded by airtime. Real time: the fake modem is real loopback I/O.
#[tokio::test(flavor = "multi_thread")]
async fn chat_waits_less_than_30s_at_bw500() {
    let dir = tempfile::tempdir().unwrap();
    let bw500 = |c: &mut StationConfig| {
        c.bandwidth = mercury_adapter::Bandwidth::Bw500;
        c.hello_timeout = mc_core::station::RADIO_HELLO_TIMEOUT;
    };
    let (_net, mut a, mut b) = pair_with(dir.path(), &bw500, &bw500, |n| {
        n.set_drain_rate(0, 19.0);
        n.set_drain_rate(1, 19.0);
    })
    .await;
    let id = ObjectId::random();
    a.send(SessionCommand::SendFile { id, prepared: prepared(&a, "photo.bin", &noise(8 * 1024)) }).await;
    let offered = b.next(30, "FileOffered", |e| matches!(e, SessionEvent::FileOffered { .. })).await;
    let SessionEvent::FileOffered { id: got, .. } = offered else { unreachable!() };
    b.send(SessionCommand::AcceptFile { id: got }).await;
    // The file is going: its first chunk is in A's modem queue.
    a.next(60, "FileProgress", |e| matches!(e, SessionEvent::FileProgress { .. })).await;
    let chat = ObjectId::random();
    let t = tokio::time::Instant::now();
    a.send(SessionCommand::SendChat { id: chat, text: "73, chat during a file".into() }).await;
    a.next(30, "ChatDelivered", move |e| matches!(e, SessionEvent::ChatDelivered { id, .. } if *id == chat)).await;
    let waited = t.elapsed();
    assert!(waited < Duration::from_secs(30), "chat waited {waited:?}");
}

/// Collect what station `from` sends while held, until a frame of type
/// `until` is among it.
async fn held_until(net: &FakeNet, from: usize, until: FrameType) -> Vec<u8> {
    let mut bytes = Vec::new();
    let end = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        bytes.extend(net.take_held().into_iter().filter(|(s, _)| *s == from).flat_map(|(_, b)| b));
        let mut d = Decoder::new();
        d.push(&bytes);
        let mut seen = false;
        while let Ok(Some(f)) = d.next_frame() {
            seen |= f.ftype == until;
        }
        if seen {
            return bytes;
        }
        assert!(tokio::time::Instant::now() < end, "never sent {until:?}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Final review 4: a receive folder that exists but cannot take files is
/// found at the offer, not after the whole file has used its airtime.
#[tokio::test(flavor = "multi_thread")]
async fn unwritable_receive_folder_declines_at_the_offer() {
    let dir = tempfile::tempdir().unwrap();
    let (_net, mut a, mut b) = pair(dir.path(), &|_| {}).await;
    // A file where the partial folder must go: nothing can be written there.
    std::fs::write(b.rx_dir().join(".partial"), b"x").unwrap();
    let id = ObjectId::random();
    a.send(SessionCommand::SendFile { id, prepared: prepared(&a, "notes.txt", &text(40)) }).await;
    let r = a
        .next(10, "refused", |e| matches!(e, SessionEvent::FileRejected { .. } | SessionEvent::FileDelivered { .. }))
        .await;
    assert!(matches!(&r, SessionEvent::FileRejected { reason, .. } if reason == "write failed"), "{r:?}");
    b.none(1, |e| matches!(e, SessionEvent::FileOffered { .. })).await;
}

/// Final review 2: bidi and invisible characters cannot disguise a name.
#[tokio::test(flavor = "multi_thread")]
async fn bidi_name_is_shown_and_saved_plainly() {
    let dir = tempfile::tempdir().unwrap();
    let (_net, a, mut b) = pair(dir.path(), &|_| {}).await;
    let id = ObjectId::random();
    a.send(SessionCommand::SendFile { id, prepared: prepared(&a, "holiday\u{202E}gpj.exe", &text(5)) }).await;
    let o = b.next(10, "FileOffered", |e| matches!(e, SessionEvent::FileOffered { .. })).await;
    let SessionEvent::FileOffered { name, .. } = o else { unreachable!() };
    assert_eq!(name, "holiday_gpj.exe", "the operator sees the name as it is");
    b.send(SessionCommand::AcceptFile { id }).await;
    let (path, _) = received_ok(&mut b).await;
    assert_eq!(path.file_name().unwrap().to_string_lossy(), "holiday_gpj.exe");
}

/// Final review 3: an offer that reuses a known transfer id is refused, and
/// the earlier record is left as it was.
#[tokio::test(flavor = "multi_thread")]
async fn reused_transfer_id_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (_net, mut a, mut b) = pair(dir.path(), &|_| {}).await;
    let id = offer(&a, &mut b, prepared(&a, "notes.txt", &text(5))).await;
    b.send(SessionCommand::AcceptFile { id }).await;
    received_ok(&mut b).await;
    a.next(10, "FileDelivered", |e| matches!(e, SessionEvent::FileDelivered { .. })).await;
    let before = b.row(Direction::In, id);
    a.send(SessionCommand::SendFile { id, prepared: prepared(&a, "other.txt", &text(7)) }).await;
    let r = a.next(10, "refused", |e| matches!(e, SessionEvent::FileRejected { .. })).await;
    assert!(matches!(&r, SessionEvent::FileRejected { reason, .. } if reason == "duplicate"), "{r:?}");
    b.none(1, |e| matches!(e, SessionEvent::FileOffered { .. })).await;
    let after = b.row(Direction::In, id);
    assert_eq!(after.state, TransferState::Complete);
    assert_eq!((after.original_name, after.saved_path), (before.original_name, before.saved_path));
}

/// Final review 6: the sender cancels after FILE_END, but the receiver had
/// already saved the file: its receipt wins, and both say complete.
#[tokio::test(flavor = "multi_thread")]
async fn receipt_after_own_cancel_marks_delivered() {
    let dir = tempfile::tempdir().unwrap();
    let (net, mut a, mut b) = pair(dir.path(), &|_| {}).await;
    let id = offer(&a, &mut b, prepared(&a, "notes.txt", &text(20))).await;
    net.set_hold_from(0, true);
    b.send(SessionCommand::AcceptFile { id }).await;
    let to_b = held_until(&net, 0, FrameType::FileEnd).await;
    a.send(SessionCommand::CancelFile { id }).await;
    a.next(10, "own FileCancelled", |e| matches!(e, SessionEvent::FileCancelled { by_peer: false, .. })).await;
    // B gets everything up to FILE_END, saves the file and sends its receipt;
    // A's cancel stays held.
    net.inject(0, to_b);
    received_ok(&mut b).await;
    a.next(10, "FileDelivered", |e| matches!(e, SessionEvent::FileDelivered { .. })).await;
    assert_eq!(a.row(Direction::Out, id).state, TransferState::Complete);
    assert_eq!(b.row(Direction::In, id).state, TransferState::Complete);
}

/// Final review 7: one transfer at a time per session. A file sent while one
/// is arriving waits for it.
#[tokio::test(flavor = "multi_thread")]
async fn send_waits_while_receiving() {
    let dir = tempfile::tempdir().unwrap();
    let auto = |c: &mut StationConfig| c.auto_accept_files = true;
    let (_net, mut a, b) = pair_with(dir.path(), &auto, &auto, |n| {
        n.set_drain_rate(0, 2000.0);
        n.set_drain_rate(1, 2000.0);
    })
    .await;
    let mut b = b;
    let first = ObjectId::random();
    a.send(SessionCommand::SendFile { id: first, prepared: prepared(&a, "first.bin", &noise(8000)) }).await;
    b.next(10, "B receiving", |e| matches!(e, SessionEvent::FileProgress { .. })).await;
    let second = ObjectId::random();
    b.send(SessionCommand::SendFile { id: second, prepared: prepared(&b, "second.bin", &noise(500)) }).await;
    let r = a
        .next(30, "A's first done or B's offer", |e| {
            matches!(e, SessionEvent::FileDelivered { .. } | SessionEvent::FileOffered { .. })
        })
        .await;
    assert!(matches!(r, SessionEvent::FileDelivered { id } if id == first), "B offered while receiving: {r:?}");
    a.next(30, "B's file after", |e| matches!(e, SessionEvent::FileReceived { .. })).await;
}

/// Final review 7: offers that cross on the air. One goes; the other is
/// refused as busy, the same way on both sides.
#[tokio::test(flavor = "multi_thread")]
async fn crossed_offers_one_goes() {
    let dir = tempfile::tempdir().unwrap();
    let auto = |c: &mut StationConfig| c.auto_accept_files = true;
    let (net, mut a, mut b) = pair_with(dir.path(), &auto, &auto, |_| {}).await;
    net.set_hold_from(0, true);
    net.set_hold_from(1, true);
    let (ia, ib) = (ObjectId::random(), ObjectId::random());
    a.send(SessionCommand::SendFile { id: ia, prepared: prepared(&a, "a.txt", &text(5)) }).await;
    b.send(SessionCommand::SendFile { id: ib, prepared: prepared(&b, "b.txt", &text(6)) }).await;
    let mut from_a = Vec::new();
    let mut from_b = Vec::new();
    let end = tokio::time::Instant::now() + Duration::from_secs(10);
    let has_offer = |bytes: &[u8]| {
        let mut d = Decoder::new();
        d.push(bytes);
        let mut seen = false;
        while let Ok(Some(f)) = d.next_frame() {
            seen |= f.ftype == FrameType::FileOffer;
        }
        seen
    };
    while !(has_offer(&from_a) && has_offer(&from_b)) {
        for (s, bytes) in net.take_held() {
            if s == 0 {
                from_a.extend(bytes)
            } else {
                from_b.extend(bytes)
            }
        }
        assert!(tokio::time::Instant::now() < end, "the offers were never sent");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    net.set_hold_from(0, false);
    net.set_hold_from(1, false);
    net.inject(0, from_a);
    net.inject(1, from_b);
    let (win, lose) = if ia.0 > ib.0 { (ia, ib) } else { (ib, ia) };
    let (winner, loser) = if win == ia { (&mut a, &mut b) } else { (&mut b, &mut a) };
    let r = loser
        .next(10, "the loser's answer", |e| matches!(e, SessionEvent::FileRejected { id, .. } if *id == lose))
        .await;
    assert!(matches!(&r, SessionEvent::FileRejected { reason, .. } if reason == "busy"), "{r:?}");
    winner
        .next(30, "the winner's file delivered", |e| matches!(e, SessionEvent::FileDelivered { id } if *id == win))
        .await;
}
