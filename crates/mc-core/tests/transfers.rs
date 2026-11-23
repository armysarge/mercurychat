//! M4 Task 9: files through the service. Two apps over the fake Mercury
//! pair (or an app and a bare minor-1 station), as the UI drives them.
#![cfg(feature = "sim")]

use mc_core::app::files::{FileSettings, PreparedView, TransferDirection, TransferState, TransferView};
use mc_core::app::{spawn_app, AppCommand, AppError, AppEvent, AppHandle, AppOptions, AppSnapshot, CommandOk};
use mc_core::app::{EventEnvelope, FrequencyEntry, ModemState};
use mc_core::files::PhotoSize;
use mc_core::profile::{ModemTarget, StationProfile};
use mc_core::station::{Station, StationConfig};
use mc_core::SessionEvent;
use mc_storage::{Direction, Store, TransferRow};
use mercury_adapter::fake::FakeNet;
use mercury_adapter::{Callsign, Endpoint};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast::Receiver;
use tokio::sync::mpsc;
use tokio::time::Instant;

const A: &str = "ZS6ABC";
const B: &str = "ZS6PEER";

struct App {
    h: AppHandle,
    rx: Receiver<EventEnvelope>,
    /// `{root}/{call}`: the app's data root.
    root: PathBuf,
    profile_id: String,
}

impl App {
    async fn call(&self, cmd: AppCommand) -> Result<CommandOk, AppError> {
        self.h.call(cmd).await
    }

    async fn snap(&self) -> AppSnapshot {
        match self.call(AppCommand::Snapshot).await.unwrap() {
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
            assert!(Instant::now() < end, "timed out waiting for {what}: modem {:?}", s.modem);
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    /// The next Transfer event whose view matches `pred`.
    async fn transfer(&mut self, secs: u64, what: &str, pred: impl Fn(&TransferView) -> bool) -> TransferView {
        let end = Instant::now() + Duration::from_secs(secs);
        loop {
            match tokio::time::timeout_at(end, self.rx.recv()).await {
                Ok(Ok(EventEnvelope { event: AppEvent::Transfer { transfer }, .. })) if pred(&transfer) => {
                    return transfer
                }
                Ok(Ok(_)) | Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => {}
                Ok(Err(e)) => panic!("{e}"),
                Err(_) => panic!("no Transfer event for {what} within {secs} s"),
            }
        }
    }

    async fn transfers(&self) -> Vec<TransferView> {
        match self.call(AppCommand::Transfers).await.unwrap() {
            CommandOk::Transfers { transfers } => transfers,
            other => panic!("{other:?}"),
        }
    }

    async fn view(&self, id: &str) -> TransferView {
        self.transfers().await.into_iter().find(|t| t.id == id).expect("a transfer row")
    }

    async fn prepare(&self, path: &Path) -> PreparedView {
        let cmd = AppCommand::PrepareFile { path: path.to_string_lossy().into_owned(), photo: PhotoSize::Original };
        match self.call(cmd).await.unwrap() {
            CommandOk::Prepared(p) => p,
            other => panic!("{other:?}"),
        }
    }

    /// Prepare and send `path`; the transfer id from the first event.
    async fn send(&mut self, path: &Path) -> String {
        let p = self.prepare(path).await;
        assert_eq!(self.call(AppCommand::SendFile { prep_id: p.prep_id }).await.unwrap(), CommandOk::Done);
        self.transfer(10, "the outgoing row", |t| t.direction == TransferDirection::Out).await.id
    }

    fn station_dir(&self) -> PathBuf {
        self.root.join("stations").join(&self.profile_id)
    }
}

fn profile(net: &FakeNet, i: usize, call: &str) -> StationProfile {
    let (base, bcast) = if i == 0 { (net.a_base, net.a_bcast) } else { (net.b_base, net.b_bcast) };
    let mut p = StationProfile::new_existing(call);
    p.modem = ModemTarget::Existing {
        host: "127.0.0.1".into(),
        control_port: base,
        data_port: base + 1,
        broadcast_port: Some(bcast),
        telemetry_url: None,
    };
    p
}

/// Start an app on `root/call` (its store on disk, so a restart keeps it).
async fn start(root: &Path, call: &str, select: Option<StationProfile>) -> App {
    let dir = root.join(call);
    std::fs::create_dir_all(&dir).unwrap();
    let opts =
        AppOptions { sim_root: Some(dir.clone()), stations_root: Some(dir.join("stations")), ..AppOptions::default() };
    let h = spawn_app(Store::open(&dir.join("app.db")).unwrap(), opts);
    let mut rx = h.subscribe();
    let profile_id = match select {
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
    loop {
        let ev = tokio::time::timeout(Duration::from_secs(15), rx.recv()).await.expect("listening").unwrap();
        if let AppEvent::ModemState { state: ModemState::Listening, .. } = ev.event {
            break;
        }
    }
    App { h, rx, root: dir, profile_id }
}

struct Two {
    net: Arc<FakeNet>,
    a: App,
    b: App,
}

/// Two apps; `rate` (bytes a second each way) slows the link, so a transfer
/// takes long enough to act on mid-way.
async fn two(root: &Path, rate: Option<f64>) -> Two {
    let net = FakeNet::start().await;
    if let Some(r) = rate {
        net.set_drain_rate(0, r);
        net.set_drain_rate(1, r);
    }
    let a = start(root, A, Some(profile(&net, 0, A))).await;
    let b = start(root, B, Some(profile(&net, 1, B))).await;
    Two { net, a, b }
}

async fn connect(a: &App, b: &App) {
    a.call(AppCommand::Call { peer: B.into() }).await.unwrap();
    let s = a.wait(15, "A's session", |s| s.session.is_some()).await;
    b.wait(15, "B's session", |s| s.session.is_some()).await;
    assert!(s.session.unwrap().files, "file/2 negotiated");
}

/// Bytes zstd can't shrink.
fn noise(n: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(n + 32);
    let mut h = Sha256::digest(b"transfers");
    while out.len() < n {
        out.extend_from_slice(&h);
        h = Sha256::digest(h);
    }
    out.truncate(n);
    out
}

fn source(root: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let p = root.join("src");
    std::fs::create_dir_all(&p).unwrap();
    let p = p.join(name);
    std::fs::write(&p, bytes).unwrap();
    p
}

fn refusal(e: &AppError) -> &str {
    match e {
        AppError::Refused { message, .. } | AppError::Conflict { message } => message,
        other => panic!("not a refusal: {other:?}"),
    }
}

fn files_in(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).collect())
        .unwrap_or_default();
    v.retain(|n| !n.starts_with('.'));
    v.sort();
    v
}

#[tokio::test(flavor = "multi_thread")]
async fn send_and_receive_through_the_service() {
    let dir = tempfile::tempdir().unwrap();
    let mut t = two(dir.path(), None).await;
    connect(&t.a, &t.b).await;
    let text = "73 de ZS6ABC, a log line\n".repeat(200).into_bytes();
    let src = source(dir.path(), "log.txt", &text);
    let p = t.a.prepare(&src).await;
    assert!(p.compressed && p.wire_size < p.size && p.airtime_s > 0, "{p:?}");
    t.a.call(AppCommand::SendFile { prep_id: p.prep_id }).await.unwrap();
    let offer = t.b.transfer(10, "B's offer", |v| v.state == TransferState::Offered).await;
    assert_eq!((offer.direction, offer.original_name.as_str(), offer.size), (TransferDirection::In, "log.txt", 5000));
    assert!(offer.offer_expires_s.is_some_and(|s| s > 100 && s <= 120), "{offer:?}");
    t.b.call(AppCommand::AcceptFile { id: offer.id.clone() }).await.unwrap();
    let done = t.b.transfer(15, "B complete", |v| v.state == TransferState::Complete).await;
    t.a.transfer(15, "A complete", |v| v.state == TransferState::Complete).await;
    let saved = t.b.station_dir().join("files").join(done.saved_name.clone().unwrap());
    assert_eq!(hex::encode(Sha256::digest(std::fs::read(saved).unwrap())), hex::encode(Sha256::digest(&text)));
    assert_eq!(t.a.view(&offer.id).await.state, TransferState::Complete, "the same id on both sides");
}

#[tokio::test(flavor = "multi_thread")]
async fn decline_is_seen_by_both() {
    let dir = tempfile::tempdir().unwrap();
    let mut t = two(dir.path(), None).await;
    connect(&t.a, &t.b).await;
    let id = t.a.send(&source(dir.path(), "x.bin", &noise(500))).await;
    t.b.transfer(10, "offer", |v| v.state == TransferState::Offered).await;
    t.b.call(AppCommand::DeclineFile { id: id.clone() }).await.unwrap();
    // The decliner's own card changes too (an event, not only the stored row).
    let b = t.b.transfer(10, "B rejected", |v| v.id == id && v.state == TransferState::Rejected).await;
    assert_eq!(b.reason.as_deref(), Some("declined"));
    let a = t.a.transfer(10, "A rejected", |v| v.state == TransferState::Rejected).await;
    assert_eq!(a.reason.as_deref(), Some("declined"));
    assert_eq!(t.b.view(&id).await.state, TransferState::Rejected);
}

#[tokio::test(flavor = "multi_thread")]
async fn cancel_by_sender_and_by_receiver() {
    let dir = tempfile::tempdir().unwrap();
    // 1 KB/s: the cancel lands while most of the file is still to come.
    let mut t = two(dir.path(), Some(1000.0)).await;
    connect(&t.a, &t.b).await;
    for by_sender in [true, false] {
        let id = t.a.send(&source(dir.path(), "big.bin", &noise(60_000))).await;
        t.b.transfer(10, "offer", |v| v.state == TransferState::Offered).await;
        t.b.call(AppCommand::AcceptFile { id: id.clone() }).await.unwrap();
        t.b.transfer(15, "receiving", |v| v.state == TransferState::Receiving && v.bytes_done > 0).await;
        let who = if by_sender { &t.a } else { &t.b };
        who.call(AppCommand::CancelFile { id: id.clone() }).await.unwrap();
        t.a.transfer(30, "A cancelled", |v| v.id == id && v.state == TransferState::Cancelled).await;
        t.b.transfer(30, "B cancelled", |v| v.id == id && v.state == TransferState::Cancelled).await;
        assert!(files_in(&t.b.station_dir().join("files")).is_empty(), "nothing published");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn restart_marks_interrupted() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let a = start(dir.path(), A, Some(profile(&net, 0, A))).await;
    let station = a.station_dir();
    a.call(AppCommand::Shutdown).await.unwrap();
    drop(a);
    tokio::time::sleep(Duration::from_millis(300)).await;
    {
        let mut s = Store::open(&station.join("station.db")).unwrap();
        s.transfer_upsert(&TransferRow {
            id: [7; 16],
            direction: Direction::Out,
            peer: B.into(),
            session_id: None,
            original_name: "half.bin".into(),
            saved_name: None,
            size: 100,
            wire_size: 100,
            sha256: "ab".into(),
            encoding: None,
            state: mc_storage::TransferState::Sending,
            bytes_done: 40,
            reason: None,
            created_utc: String::new(),
            updated_utc: String::new(),
            saved_path: None,
        })
        .unwrap();
    }
    std::fs::create_dir_all(station.join("tmp")).unwrap();
    std::fs::write(station.join("tmp").join("left.prep"), b"x").unwrap();
    let a = start(dir.path(), A, None).await;
    let v = a.view(&hex::encode([7u8; 16])).await;
    assert_eq!(v.state, TransferState::Interrupted);
    assert!(files_in(&station.join("tmp")).is_empty(), "tmp emptied");
}

#[tokio::test(flavor = "multi_thread")]
async fn discarded_and_leftover_temps_removed() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let a = start(dir.path(), A, Some(profile(&net, 0, A))).await;
    let tmp = a.station_dir().join("tmp");
    let p = a.prepare(&source(dir.path(), "n.txt", b"hello hello hello hello")).await;
    assert_eq!(files_in(&tmp).len(), 1, "the prepared file waits in tmp");
    a.call(AppCommand::DiscardPrepared { prep_id: p.prep_id.clone() }).await.unwrap();
    assert!(files_in(&tmp).is_empty(), "discarded");
    // Sending a discarded one is refused.
    assert!(a.call(AppCommand::SendFile { prep_id: p.prep_id }).await.is_err());
    // Quitting mid-prepare: a leftover is cleared on the next start.
    a.prepare(&source(dir.path(), "m.txt", b"left behind")).await;
    a.call(AppCommand::Shutdown).await.unwrap();
    drop(a);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let _a = start(dir.path(), A, None).await;
    assert!(files_in(&tmp).is_empty(), "leftovers cleared at start");
}

#[tokio::test(flavor = "multi_thread")]
async fn missing_receive_folder_declines() {
    let dir = tempfile::tempdir().unwrap();
    let mut t = two(dir.path(), None).await;
    let folder = dir.path().join("inbox");
    std::fs::create_dir_all(&folder).unwrap();
    let settings = FileSettings {
        max_incoming: 65_536,
        receive_dir: Some(folder.to_string_lossy().into_owned()),
        photo_default: PhotoSize::Medium,
    };
    t.b.call(AppCommand::SetFileSettings { settings }).await.unwrap();
    std::fs::remove_dir_all(&folder).unwrap();
    connect(&t.a, &t.b).await;
    let id = t.a.send(&source(dir.path(), "x.bin", &noise(500))).await;
    let a = t.a.transfer(10, "A rejected", |v| v.state == TransferState::Rejected).await;
    assert_eq!(a.reason.as_deref(), Some("no folder"));
    // The receiving operator sees the declined offer, with its reason.
    let b = t.b.transfer(10, "B rejected", |v| v.id == id && v.state == TransferState::Rejected).await;
    assert_eq!(b.reason.as_deref(), Some("no folder"));
    assert!(!folder.exists(), "nothing created or landed elsewhere");
}

#[tokio::test(flavor = "multi_thread")]
async fn resend_after_interruption_is_a_new_transfer() {
    let dir = tempfile::tempdir().unwrap();
    let mut t = two(dir.path(), Some(4000.0)).await;
    connect(&t.a, &t.b).await;
    let src = source(dir.path(), "big.bin", &noise(60_000));
    let first = t.a.send(&src).await;
    t.b.transfer(10, "offer", |v| v.state == TransferState::Offered).await;
    t.b.call(AppCommand::AcceptFile { id: first.clone() }).await.unwrap();
    t.b.transfer(15, "receiving", |v| v.state == TransferState::Receiving && v.bytes_done > 0).await;
    t.net.drop_link();
    t.a.transfer(15, "A interrupted", |v| v.id == first && v.state == TransferState::Interrupted).await;
    t.b.transfer(15, "B interrupted", |v| v.id == first && v.state == TransferState::Interrupted).await;
    t.a.wait(15, "A idle", |s| s.session.is_none() && s.modem == ModemState::Listening).await;
    t.b.wait(15, "B idle", |s| s.session.is_none() && s.modem == ModemState::Listening).await;
    connect(&t.a, &t.b).await;
    let second = t.a.send(&src).await;
    assert_ne!(first, second, "a new transfer id");
    t.b.transfer(10, "offer", |v| v.id == second && v.state == TransferState::Offered).await;
    t.b.call(AppCommand::AcceptFile { id: second.clone() }).await.unwrap();
    let done = t.b.transfer(60, "B complete", |v| v.id == second && v.state == TransferState::Complete).await;
    assert_eq!(done.saved_name.as_deref(), Some("big.bin"));
    assert_eq!(t.a.view(&first).await.state, TransferState::Interrupted, "the old one stays interrupted");
    assert_eq!(t.b.view(&first).await.state, TransferState::Interrupted);
}

#[tokio::test(flavor = "multi_thread")]
async fn qsy_refused_during_transfer() {
    let dir = tempfile::tempdir().unwrap();
    let mut t = two(dir.path(), Some(4000.0)).await;
    connect(&t.a, &t.b).await;
    let id = t.a.send(&source(dir.path(), "big.bin", &noise(60_000))).await;
    t.b.transfer(10, "offer", |v| v.state == TransferState::Offered).await;
    t.b.call(AppCommand::AcceptFile { id: id.clone() }).await.unwrap();
    t.b.transfer(15, "receiving", |v| v.state == TransferState::Receiving && v.bytes_done > 0).await;
    let entry = FrequencyEntry {
        name: "40m".into(),
        freq_hz: 7_105_000,
        mode: "USB".into(),
        passband_hz: Some(2400),
        purpose: vec!["chat".into()],
        ..FrequencyEntry::default()
    };
    let entry_id = match t.a.call(AppCommand::SaveFrequency { entry }).await.unwrap() {
        CommandOk::Frequencies { entries } => entries.into_iter().find(|e| e.name == "40m").unwrap().id,
        other => panic!("{other:?}"),
    };
    let e = t.a.call(AppCommand::ProposeQsy { entry_ids: vec![entry_id], note: None }).await.unwrap_err();
    assert_eq!(refusal(&e), "a file transfer is in progress");
    let e = t.b.call(AppCommand::AnswerQsy { accept: Some(0) }).await.unwrap_err();
    assert_eq!(refusal(&e), "a file transfer is in progress");
    t.b.transfer(60, "B complete", |v| v.id == id && v.state == TransferState::Complete).await;
    assert!(t.a.snap().await.session.is_some(), "the session went on");
}

#[tokio::test(flavor = "multi_thread")]
async fn send_refused_to_minor1_peer() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let mut a = start(dir.path(), A, Some(profile(&net, 0, A))).await;
    let mut cfg = StationConfig::defaults(
        Callsign::parse(B).unwrap(),
        Endpoint::base("127.0.0.1", net.b_base),
        &dir.path().join("peer"),
    );
    cfg.proto_minor = 1;
    cfg.capabilities = vec!["chat".into(), "file".into(), "qsy/1".into(), "info/1".into()];
    let mut peer = Station::open(cfg).unwrap();
    peer.attach().await.unwrap();
    let info = peer.call(&Callsign::parse(A).unwrap(), Duration::from_secs(10)).await.unwrap();
    let (_peer_cmds, mut cmd_rx) = mpsc::channel(16);
    let (ev_tx, mut peer_events) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        let (_req_tx, mut req_rx) = mpsc::channel(4);
        peer.run_session(&info, &mut cmd_rx, &mut req_rx, ev_tx).await
    });
    let s = a.wait(15, "the session", |s| s.session.is_some()).await;
    assert!(!s.session.unwrap().files);
    let p = a.prepare(&source(dir.path(), "x.txt", b"hello")).await;
    let e = a.call(AppCommand::SendFile { prep_id: p.prep_id }).await.unwrap_err();
    assert_eq!(refusal(&e), "not supported by this station");
    tokio::time::sleep(Duration::from_millis(500)).await;
    while let Ok(ev) = peer_events.try_recv() {
        assert!(!matches!(ev, SessionEvent::FileOffered { .. }), "{ev:?}");
    }
    while let Ok(ev) = a.rx.try_recv() {
        assert!(!matches!(ev.event, AppEvent::Transfer { .. }), "{ev:?}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn over_peer_max_file_refused_before_offer() {
    let dir = tempfile::tempdir().unwrap();
    let mut t = two(dir.path(), None).await;
    let settings = FileSettings { max_incoming: 1_024, receive_dir: None, photo_default: PhotoSize::Medium };
    t.b.call(AppCommand::SetFileSettings { settings }).await.unwrap();
    connect(&t.a, &t.b).await;
    assert_eq!(t.a.snap().await.session.unwrap().peer_max_file, 1_024);
    let p = t.a.prepare(&source(dir.path(), "x.bin", &noise(2_000))).await;
    let e = t.a.call(AppCommand::SendFile { prep_id: p.prep_id }).await.unwrap_err();
    assert!(refusal(&e).contains("1024"), "the limit is shown: {e:?}");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(t.b.transfers().await.is_empty(), "nothing offered");
    while let Ok(ev) = t.a.rx.try_recv() {
        assert!(!matches!(ev.event, AppEvent::Transfer { .. }), "{ev:?}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn settings_bounds() {
    let dir = tempfile::tempdir().unwrap();
    let net = FakeNet::start().await;
    let a = start(dir.path(), A, Some(profile(&net, 0, A))).await;
    match a.call(AppCommand::FileSettings).await.unwrap() {
        CommandOk::FileSettings(s) => {
            assert_eq!(s, FileSettings { max_incoming: 65_536, receive_dir: None, photo_default: PhotoSize::Medium })
        }
        other => panic!("{other:?}"),
    }
    for bad in [500, 2 << 20] {
        let settings = FileSettings { max_incoming: bad, receive_dir: None, photo_default: PhotoSize::Medium };
        let e = a.call(AppCommand::SetFileSettings { settings }).await.unwrap_err();
        assert!(matches!(e, AppError::Validation { .. }), "{bad}: {e:?}");
    }
    let ok = FileSettings { max_incoming: 1 << 20, receive_dir: None, photo_default: PhotoSize::Small };
    a.call(AppCommand::SetFileSettings { settings: ok.clone() }).await.unwrap();
    assert_eq!(a.call(AppCommand::FileSettings).await.unwrap(), CommandOk::FileSettings(ok));
}

/// Final review 8 (Review Focus 1, the other order): while a QSY is being
/// arranged, no file starts, so the QSY's disconnect can't cut one short.
#[tokio::test(flavor = "multi_thread")]
async fn no_file_starts_while_a_qsy_is_arranged() {
    let dir = tempfile::tempdir().unwrap();
    let t = two(dir.path(), None).await;
    connect(&t.a, &t.b).await;
    let entry = FrequencyEntry {
        name: "40m".into(),
        freq_hz: 7_105_000,
        mode: "USB".into(),
        passband_hz: Some(2400),
        purpose: vec!["chat".into()],
        ..FrequencyEntry::default()
    };
    let entry_id = match t.a.call(AppCommand::SaveFrequency { entry }).await.unwrap() {
        CommandOk::Frequencies { entries } => entries.into_iter().find(|e| e.name == "40m").unwrap().id,
        other => panic!("{other:?}"),
    };
    t.a.call(AppCommand::ProposeQsy { entry_ids: vec![entry_id], note: None }).await.unwrap();
    // The proposer can't send a file while it waits for the answer.
    let p = t.a.prepare(&source(dir.path(), "a.txt", b"hello")).await;
    let e = t.a.call(AppCommand::SendFile { prep_id: p.prep_id }).await.unwrap_err();
    assert_eq!(refusal(&e), "not while a QSY is being arranged");
    // Nor can the station it was offered to.
    t.b.wait(10, "B offered the QSY", |s| s.qsy.is_some()).await;
    let p = t.b.prepare(&source(dir.path(), "b.txt", b"hello")).await;
    let e = t.b.call(AppCommand::SendFile { prep_id: p.prep_id }).await.unwrap_err();
    assert_eq!(refusal(&e), "not while a QSY is being arranged");
}
