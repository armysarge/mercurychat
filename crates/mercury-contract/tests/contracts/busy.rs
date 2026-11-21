//! A2 Busy: BUSYDETECT ON/OFF is acknowledged; with it on, a peer's
//! transmission gives BUSY ON (real Mercury only: the fake has no channel).

use mercury_adapter::{Ack, Bandwidth, Command, ControlEvent};
use mercury_contract::{attach, contract, next_event, Tag};

#[tokio::test(flavor = "multi_thread")]
async fn busydetect_toggles() {
    contract("busydetect_toggles", Tag::Both, |t| async move {
        let a = attach(&t.a, &t).await;
        assert_eq!(a.client.command(&Command::BusyDetect(true)).await, Ok(Ack::Ok));
        assert_eq!(a.client.command(&Command::BusyDetect(false)).await, Ok(Ack::Ok));
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn busy_while_peer_transmits() {
    contract("busy_while_peer_transmits", Tag::RealOnly, |t| async move {
        let mut a = attach(&t.a, &t).await;
        let b = attach(&t.b, &t).await;
        assert_eq!(a.client.command(&Command::BusyDetect(true)).await, Ok(Ack::Ok));
        // The detector measures its noise floor first.
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        let cq = Command::CqFrame { source: t.b.call.clone(), bw: Bandwidth::Bw2300 };
        assert_eq!(b.client.command(&cq).await, Ok(Ack::Ok));
        next_event(&mut a, t.arq_timeout(), "BUSY ON", |e| *e == ControlEvent::Busy(true)).await;
        assert_eq!(a.client.command(&Command::BusyDetect(false)).await, Ok(Ack::Ok));
    })
    .await;
}
