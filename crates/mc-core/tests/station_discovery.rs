//! M3a Task 3: the Station's observation tap, DATAC3 listen mode and CQ.
//! Driven against the FakeNet modem pair; no RF.
#![cfg(feature = "sim")]

use mc_core::session::SessionEvent;
use mc_core::sim::serve_peer;
use mc_core::station::{Observation, Station, StationConfig};
use mc_core::{SessionCommand, StationRequest};
use mch_protocol::ObjectId;
use mercury_adapter::fake::FakeNet;
use mercury_adapter::{Callsign, Endpoint};
use std::path::Path;
use std::time::Duration;
use tokio::sync::mpsc;

const A_CALL: &str = "ZS6ABC";
const B_CALL: &str = "ZS6PEER";

fn station(net: &FakeNet, side: usize, call: &str, dir: &Path) -> Station {
    let base = [net.a_base, net.b_base][side];
    let call = Callsign::parse(call).unwrap();
    Station::open(StationConfig::defaults(call, Endpoint::base("127.0.0.1", base), dir)).unwrap()
}

/// Pump `st`'s event stream for `ms` so its observer sees what arrived.
async fn pump(st: &mut Station, ms: u64) {
    let _ = st.wait_incoming(Duration::from_millis(ms)).await;
}

#[tokio::test]
async fn observer_sees_cq_monitor_busy_and_ptt() {
    let net = FakeNet::start().await;
    let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let mut a = station(&net, 0, A_CALL, da.path());
    let mut b = station(&net, 1, B_CALL, db.path());
    let (tx, mut rx) = mpsc::channel(256);
    b.set_observer(tx);
    a.attach().await.unwrap();
    b.attach().await.unwrap();
    assert_eq!(b.listen_mode(), Some(1));
    a.cq().await.unwrap();
    pump(&mut b, 800).await;
    net.send_control(1, "BUSY ON");
    net.send_control(1, "BUSY OFF");
    net.send_control(1, "SN 12.5");
    net.send_control(1, "SN fish");
    net.send_control(1, "MONITOR DATAC1 garbage");
    pump(&mut b, 400).await;
    let mut seen = Vec::new();
    while let Ok(o) = rx.try_recv() {
        seen.push(o);
    }
    assert!(seen.iter().any(|o| matches!(o, Observation::Cq { source, .. } if source == A_CALL)), "{seen:?}");
    assert!(
        seen.iter()
            .any(|o| matches!(o, Observation::MonitorCq(m) if m.from == A_CALL && (m.snr_db - 7.5).abs() < 0.01)),
        "{seen:?}"
    );
    assert!(seen.contains(&Observation::Busy(true)) && seen.contains(&Observation::Busy(false)), "{seen:?}");
    assert!(seen.contains(&Observation::Snr(12.5)), "{seen:?}");
    assert!(seen.contains(&Observation::Unknown("SN fish".into())), "{seen:?}");
    assert!(seen.contains(&Observation::Unknown("MONITOR DATAC1 garbage".into())), "{seen:?}");
    assert_eq!(b.observations_dropped, 0);
    // A's own CQ transmission keyed its PTT.
    let (txa, mut rxa) = mpsc::channel(256);
    a.set_observer(txa);
    a.cq().await.unwrap();
    pump(&mut a, 800).await;
    let mut mine = Vec::new();
    while let Ok(o) = rxa.try_recv() {
        mine.push(o);
    }
    assert!(mine.contains(&Observation::Ptt(true)) && mine.contains(&Observation::Ptt(false)), "{mine:?}");
}

#[tokio::test]
async fn incoming_call_during_cq_is_not_lost() {
    let net = FakeNet::start().await;
    let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let mut a = station(&net, 0, A_CALL, da.path());
    let mut b = station(&net, 1, B_CALL, db.path());
    a.attach().await.unwrap();
    b.attach().await.unwrap();
    a.set_listen(true).await.unwrap();
    a.cq().await.unwrap();
    let acall = Callsign::parse(A_CALL).unwrap();
    let (incoming, call) =
        tokio::join!(a.wait_incoming(Duration::from_secs(5)), b.call(&acall, Duration::from_secs(5)));
    let info = incoming.expect("the incoming call is still delivered after a CQ");
    assert_eq!(info.peer, B_CALL);
    call.unwrap();
}

#[tokio::test]
async fn mode_wrong_leaves_listen_mode_unknown() {
    let net = FakeNet::start().await;
    net.set_mode_wrong(0, true);
    let da = tempfile::tempdir().unwrap();
    let mut a = station(&net, 0, A_CALL, da.path());
    a.attach().await.expect("MODE WRONG must not fail attach");
    assert_eq!(a.listen_mode(), None);
    assert!(a.is_attached());
}

#[tokio::test]
async fn full_observer_never_blocks_a_session() {
    let net = FakeNet::start().await;
    let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let mut a = station(&net, 0, A_CALL, da.path());
    let mut b = station(&net, 1, B_CALL, db.path());
    let (tx, _rx_never_read) = mpsc::channel(1);
    a.set_observer(tx);
    a.attach().await.unwrap();
    b.attach().await.unwrap();
    b.set_listen(true).await.unwrap();
    let peer = tokio::spawn(serve_peer(b, true, None));

    let info = a.call(&Callsign::parse(B_CALL).unwrap(), Duration::from_secs(5)).await.unwrap();
    let (cmd_tx, mut cmd_rx) = mpsc::channel(8);
    let (_req_tx, mut req_rx) = mpsc::channel::<StationRequest>(1);
    let (ev_tx, mut ev_rx) = mpsc::unbounded_channel();
    let id = ObjectId::random();
    let driver = async {
        cmd_tx.send(SessionCommand::SendChat { id, text: "hello".into() }).await.unwrap();
        loop {
            match ev_rx.recv().await {
                Some(SessionEvent::ChatDelivered { id: got, .. }) if got == id => return,
                Some(_) => {}
                None => panic!("session ended before delivery"),
            }
        }
    };
    let delivered = tokio::time::timeout(Duration::from_secs(20), async {
        tokio::select! {
            end = a.run_session(&info, &mut cmd_rx, &mut req_rx, ev_tx) => panic!("session ended: {end:?}"),
            _ = driver => {}
        }
    })
    .await;
    assert!(delivered.is_ok(), "the chat round-trips although nobody reads the observer");
    assert!(a.observations_dropped > 0, "the full channel dropped observations");
    peer.abort();
}
