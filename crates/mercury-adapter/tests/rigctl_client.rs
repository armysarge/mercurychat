//! The rigctld client task against the fake rigctld (M3b Task 2).

use mercury_adapter::fake_rig::FakeRigctld;
use mercury_adapter::rigctl::{
    Level, PollRate, PttReading, RigCaps, RigClient, RigClientCmd, RigLink, RigMsg, RigReply, RigRequest,
};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::{timeout, Instant};

struct Rig {
    fake: FakeRigctld,
    cmds: mpsc::Sender<RigClientCmd>,
    out: mpsc::Receiver<RigMsg>,
    _task: tokio::task::AbortHandle,
}

async fn start_with(fake: FakeRigctld) -> Rig {
    let (cmds, cmd_rx) = mpsc::channel(16);
    let (out_tx, out) = mpsc::channel(256);
    // Reconnect quickly in tests: a fiftieth of the real backoff.
    let task = RigClient::spawn_with(fake.addr.to_string(), cmd_rx, out_tx, |d| d / 50);
    Rig { fake, cmds, out, _task: task }
}

async fn start() -> Rig {
    start_with(FakeRigctld::start().await).await
}

/// The next message matching `pred` within `within`, or panic.
async fn until(rig: &mut Rig, within: Duration, mut pred: impl FnMut(&RigMsg) -> bool) -> RigMsg {
    let end = Instant::now() + within;
    loop {
        match timeout(end.saturating_duration_since(Instant::now()), rig.out.recv()).await {
            Ok(Some(m)) if pred(&m) => return m,
            Ok(Some(_)) => {}
            Ok(None) => panic!("client ended"),
            Err(_) => panic!("nothing matching within {within:?}"),
        }
    }
}

/// Every message for `d`.
async fn collect(rig: &mut Rig, d: Duration) -> Vec<RigMsg> {
    let end = Instant::now() + d;
    let mut v = vec![];
    while let Ok(Some(m)) = timeout(end.saturating_duration_since(Instant::now()), rig.out.recv()).await {
        v.push(m);
    }
    v
}

async fn up(rig: &mut Rig) {
    until(rig, Duration::from_secs(3), |m| matches!(m, RigMsg::Link(RigLink::Up))).await;
}

fn count(log: &[String], prefix: &str) -> usize {
    log.iter().filter(|l| l.starts_with(prefix)).count()
}

#[tokio::test]
async fn connect_reads_caps_then_reports_up() {
    let mut rig = start().await;
    let first = collect(&mut rig, Duration::from_millis(400)).await;
    let kinds: Vec<&str> = first
        .iter()
        .filter_map(|m| match m {
            RigMsg::Link(RigLink::Connecting) => Some("connecting"),
            RigMsg::Caps(_) => Some("caps"),
            RigMsg::Link(RigLink::Up) => Some("up"),
            _ => None,
        })
        .collect();
    assert_eq!(kinds, ["connecting", "caps", "up"]);
    assert_eq!(rig.fake.log().first().map(String::as_str), Some("+\\dump_caps"));
}

#[tokio::test]
async fn idle_polls_t_every_second_and_active_every_250ms() {
    let mut rig = start().await;
    up(&mut rig).await;
    let before = count(&rig.fake.log(), "+t");
    collect(&mut rig, Duration::from_millis(2000)).await;
    let idle = count(&rig.fake.log(), "+t") - before;
    assert!((1..=3).contains(&idle), "idle: {idle} polls in 2 s");
    rig.cmds.send(RigClientCmd::Rate(PollRate::Active)).await.unwrap();
    collect(&mut rig, Duration::from_millis(100)).await;
    let before = count(&rig.fake.log(), "+t");
    collect(&mut rig, Duration::from_millis(1000)).await;
    let active = count(&rig.fake.log(), "+t") - before;
    assert!((3..=5).contains(&active), "active: {active} polls in 1 s");
}

#[tokio::test]
async fn a_slow_reply_is_unknown_not_off() {
    let mut rig = start().await;
    up(&mut rig).await;
    rig.fake.delay(Duration::from_millis(300));
    rig.cmds.send(RigClientCmd::Rate(PollRate::Active)).await.unwrap();
    // Let anything sent before the switch (idle timeout 500 ms) finish.
    collect(&mut rig, Duration::from_millis(1200)).await;
    let msgs = collect(&mut rig, Duration::from_millis(1200)).await;
    let ptt: Vec<&PttReading> =
        msgs.iter().filter_map(|m| if let RigMsg::Ptt(p) = m { Some(p) } else { None }).collect();
    assert!(ptt.iter().any(|p| matches!(p, PttReading::Unknown { .. })), "{ptt:?}");
    assert!(!ptt.iter().any(|p| matches!(p, PttReading::Off)), "a late reply must never read as off: {ptt:?}");
}

#[tokio::test]
async fn a_slow_rigctld_is_never_overloaded_into_a_lost_link() {
    // Final review #4: one request in flight per client (design §5). A
    // rigctld answering in 300 ms during activity must not build a backlog
    // until the link is taken as lost.
    let mut rig = start().await;
    up(&mut rig).await;
    rig.fake.delay(Duration::from_millis(300));
    rig.cmds.send(RigClientCmd::Rate(PollRate::Active)).await.unwrap();
    let msgs = collect(&mut rig, Duration::from_secs(10)).await;
    assert!(!msgs.iter().any(|m| matches!(m, RigMsg::Link(RigLink::Lost { .. }))), "{msgs:?}");
    // No backlog: a request now is answered within its own timeout.
    rig.cmds.send(RigClientCmd::Request { id: 99, req: RigRequest::GetFreq }).await.unwrap();
    let done = until(&mut rig, Duration::from_secs(2), |m| matches!(m, RigMsg::Done { id: 99, .. })).await;
    assert!(matches!(done, RigMsg::Done { result: Ok(RigReply::Freq(_)), .. }), "{done:?}");
}

#[tokio::test]
async fn rprt_to_t_is_unknown() {
    let mut rig = start().await;
    up(&mut rig).await;
    rig.fake.ptt_error(Some(-11));
    let m = until(&mut rig, Duration::from_secs(3), |m| matches!(m, RigMsg::Ptt(PttReading::Unknown { .. }))).await;
    let RigMsg::Ptt(PttReading::Unknown { reason }) = m else { unreachable!() };
    assert!(reason.contains("-11"), "{reason}");
}

#[tokio::test]
async fn requests_queue_and_complete() {
    let mut rig = start().await;
    up(&mut rig).await;
    rig.cmds.send(RigClientCmd::Request { id: 7, req: RigRequest::SetFreq(7_105_000) }).await.unwrap();
    rig.cmds.send(RigClientCmd::Request { id: 8, req: RigRequest::GetFreq }).await.unwrap();
    let done = until(&mut rig, Duration::from_secs(2), |m| matches!(m, RigMsg::Done { id: 7, .. })).await;
    assert!(matches!(done, RigMsg::Done { result: Ok(RigReply::Ok), .. }), "{done:?}");
    let done = until(&mut rig, Duration::from_secs(2), |m| matches!(m, RigMsg::Done { id: 8, .. })).await;
    assert!(matches!(done, RigMsg::Done { result: Ok(RigReply::Freq(7_105_000)), .. }), "{done:?}");
    assert_eq!(rig.fake.freq(), 7_105_000);
    // A successful set is followed by a fresh reading.
    until(&mut rig, Duration::from_secs(2), |m| matches!(m, RigMsg::Freq(7_105_000))).await;
}

#[tokio::test]
async fn request_while_down_fails_fast() {
    let mut rig = start().await;
    up(&mut rig).await;
    rig.fake.kill();
    until(&mut rig, Duration::from_secs(3), |m| matches!(m, RigMsg::Link(RigLink::Lost { .. }))).await;
    let sent = Instant::now();
    rig.cmds.send(RigClientCmd::Request { id: 1, req: RigRequest::SetFreq(7_000_000) }).await.unwrap();
    let done = until(&mut rig, Duration::from_secs(1), |m| matches!(m, RigMsg::Done { id: 1, .. })).await;
    assert!(sent.elapsed() < Duration::from_millis(50), "{:?}", sent.elapsed());
    assert!(matches!(&done, RigMsg::Done { result: Err(e), .. } if e == "rig link down"), "{done:?}");
}

#[tokio::test]
async fn restart_reconnects_on_backoff() {
    let mut rig = start().await;
    up(&mut rig).await;
    rig.fake.kill();
    until(&mut rig, Duration::from_secs(3), |m| matches!(m, RigMsg::Link(RigLink::Lost { .. }))).await;
    // Several retries fail while it is down: still only one Lost.
    let down = collect(&mut rig, Duration::from_millis(700)).await;
    assert!(!down.iter().any(|m| matches!(m, RigMsg::Link(RigLink::Lost { .. }))), "{down:?}");
    rig.fake.restart().await;
    up(&mut rig).await;
}

#[tokio::test]
async fn meters_poll_only_while_keyed() {
    let caps = RigCaps {
        model: "Fake".into(),
        can_set_freq: true,
        can_set_mode: true,
        can_get_ptt: true,
        get_levels: vec![Level::RfPower, Level::Swr],
        set_levels: vec![Level::RfPower],
        ..RigCaps::default()
    };
    let fake = FakeRigctld::start().await;
    fake.caps(caps);
    let mut rig = start_with(fake).await;
    up(&mut rig).await;
    rig.cmds.send(RigClientCmd::Rate(PollRate::Active)).await.unwrap();
    rig.fake.set_ptt(true);
    until(&mut rig, Duration::from_secs(2), |m| matches!(m, RigMsg::Meter(Level::Swr, _))).await;
    assert_eq!(count(&rig.fake.log(), "+l ALC"), 0, "ALC is not in the caps");
    rig.fake.set_ptt(false);
    until(&mut rig, Duration::from_secs(2), |m| matches!(m, RigMsg::Ptt(PttReading::Off))).await;
    collect(&mut rig, Duration::from_millis(100)).await;
    let swr = count(&rig.fake.log(), "+l SWR");
    collect(&mut rig, Duration::from_millis(800)).await;
    assert_eq!(count(&rig.fake.log(), "+l SWR"), swr, "no meter polls while unkeyed");
}

#[tokio::test]
async fn the_client_never_sends_t_1() {
    let mut rig = start().await;
    up(&mut rig).await;
    rig.cmds.send(RigClientCmd::Rate(PollRate::Active)).await.unwrap();
    for (id, req) in [
        RigRequest::SetFreq(7_100_000),
        RigRequest::SetMode { mode: "PKTUSB".into(), passband_hz: 3000 },
        RigRequest::SetVfo("VFOB".into()),
        RigRequest::SetSplit { on: true, tx_vfo: "VFOA".into() },
        RigRequest::SetLevel(Level::RfPower, 0.5),
        RigRequest::PttOff,
    ]
    .into_iter()
    .enumerate()
    {
        rig.cmds.send(RigClientCmd::Request { id: id as u64, req }).await.unwrap();
    }
    rig.fake.set_ptt(true);
    collect(&mut rig, Duration::from_millis(1500)).await;
    let log = rig.fake.log();
    assert_eq!(count(&log, "+T 0"), 1);
    assert_eq!(count(&log, "+T 1"), 0, "{log:?}");
    assert_eq!(count(&log, "T 1"), 0);
}
