//! FLRig as the rig link (M3b.1 Task 4): frequency, mode and PTT reading
//! over XML-RPC, speaking the same RigClientCmd/RigMsg as the rigctld client.

use mercury_adapter::fake_flrig::FakeFlrig;
use mercury_adapter::flrig::FlrigClient;
use mercury_adapter::rigctl::{PttReading, RigClientCmd, RigLink, RigMsg, RigReply, RigRequest};
use std::time::Duration;
use tokio::sync::mpsc;

struct Client {
    tx: mpsc::Sender<RigClientCmd>,
    rx: mpsc::Receiver<RigMsg>,
    _task: tokio::task::AbortHandle,
}

fn spawn(fake: &FakeFlrig) -> Client {
    let (tx, crx) = mpsc::channel(8);
    let (otx, rx) = mpsc::channel(256);
    let task = FlrigClient::spawn(fake.addr.to_string(), Duration::from_millis(200), crx, otx);
    Client { tx, rx, _task: task }
}

async fn until<T>(rx: &mut mpsc::Receiver<RigMsg>, mut f: impl FnMut(&RigMsg) -> Option<T>) -> T {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let m = rx.recv().await.expect("client open");
            if let Some(t) = f(&m) {
                return t;
            }
        }
    })
    .await
    .expect("in time")
}

async fn ask(c: &mut Client, id: u64, req: RigRequest) -> Result<RigReply, String> {
    c.tx.send(RigClientCmd::Request { id, req }).await.unwrap();
    until(&mut c.rx, |m| match m {
        RigMsg::Done { id: got, result } if *got == id => Some(result.clone()),
        _ => None,
    })
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn reads_freq_mode_ptt() {
    let fake = FakeFlrig::start().await;
    fake.set_freq(14_105_000);
    fake.set_mode("USB-D");
    let mut c = spawn(&fake);
    until(&mut c.rx, |m| matches!(m, RigMsg::Link(RigLink::Up)).then_some(())).await;
    assert_eq!(until(&mut c.rx, |m| if let RigMsg::Freq(hz) = m { Some(*hz) } else { None }).await, 14_105_000);
    let mode = until(&mut c.rx, |m| if let RigMsg::Mode { mode, .. } = m { Some(mode.clone()) } else { None }).await;
    assert_eq!(mode, "USB-D");
    let ptt = until(&mut c.rx, |m| if let RigMsg::Ptt(p) = m { Some(p.clone()) } else { None }).await;
    assert_eq!(ptt, PttReading::Off);
    fake.set_ptt(true);
    until(&mut c.rx, |m| matches!(m, RigMsg::Ptt(PttReading::On)).then_some(())).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn caps_say_ptt_readable_only() {
    let fake = FakeFlrig::start().await;
    let mut c = spawn(&fake);
    let caps = until(&mut c.rx, |m| if let RigMsg::Caps(c) = m { Some(c.clone()) } else { None }).await;
    assert!(caps.can_get_ptt && caps.can_set_freq && caps.can_set_mode);
    assert!(!caps.can_set_vfo && !caps.can_split && caps.set_levels.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn set_freq_round_trip() {
    let fake = FakeFlrig::start().await;
    let mut c = spawn(&fake);
    until(&mut c.rx, |m| matches!(m, RigMsg::Link(RigLink::Up)).then_some(())).await;
    assert_eq!(ask(&mut c, 1, RigRequest::SetFreq(7_105_000)).await, Ok(RigReply::Ok));
    assert_eq!(fake.freq(), 7_105_000);
    let r = ask(&mut c, 2, RigRequest::SetMode { mode: "LSB".into(), passband_hz: 0 }).await;
    assert_eq!(r, Ok(RigReply::Ok));
    assert_eq!(fake.mode(), "LSB");
}

#[tokio::test(flavor = "multi_thread")]
async fn fault_is_error() {
    let fake = FakeFlrig::start().await;
    fake.fault_on("rig.set_mode", "bad mode");
    let mut c = spawn(&fake);
    until(&mut c.rx, |m| matches!(m, RigMsg::Link(RigLink::Up)).then_some(())).await;
    let r = ask(&mut c, 1, RigRequest::SetMode { mode: "XYZ".into(), passband_hz: 0 }).await;
    assert!(matches!(&r, Err(e) if e.contains("bad mode")), "{r:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn ptt_off_unsupported() {
    let fake = FakeFlrig::start().await;
    let mut c = spawn(&fake);
    until(&mut c.rx, |m| matches!(m, RigMsg::Link(RigLink::Up)).then_some(())).await;
    for (id, req) in [(1, RigRequest::PttOff), (2, RigRequest::SetVfo("VFOB".into()))] {
        let r = ask(&mut c, id, req).await;
        assert_eq!(r, Err("not supported by FLRig".to_string()));
    }
    assert!(!fake.log().iter().any(|(m, _)| m == "rig.set_ptt"), "MercuryChat never keys through FLRig");
}

#[tokio::test(flavor = "multi_thread")]
async fn lost_and_back() {
    let fake = FakeFlrig::start().await;
    let mut c = spawn(&fake);
    until(&mut c.rx, |m| matches!(m, RigMsg::Link(RigLink::Up)).then_some(())).await;
    fake.kill();
    until(&mut c.rx, |m| matches!(m, RigMsg::Link(RigLink::Lost { .. })).then_some(())).await;
    fake.restart().await;
    until(&mut c.rx, |m| matches!(m, RigMsg::Link(RigLink::Up)).then_some(())).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn xml_escaping() {
    let fake = FakeFlrig::start().await;
    let mut c = spawn(&fake);
    until(&mut c.rx, |m| matches!(m, RigMsg::Link(RigLink::Up)).then_some(())).await;
    let _ = ask(&mut c, 1, RigRequest::SetMode { mode: "A&B<".into(), passband_hz: 0 }).await;
    let raw = fake.log().into_iter().find(|(m, _)| m == "rig.set_mode").expect("sent").1;
    assert!(raw.contains("A&amp;B&lt;"), "{raw}");
    assert_eq!(fake.mode(), "A&B<", "the fake decodes it back");
}
