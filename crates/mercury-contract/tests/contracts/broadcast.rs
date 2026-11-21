//! A2 Broadcast: a KISS payload at the listen mode's capacity arrives intact;
//! a longer one arrives cut to the capacity; CQFRAME and MONITOR lines parse.

use mercury_adapter::broadcast::BroadcastPort;
use mercury_adapter::kiss::{self, KissFrame};
use mercury_adapter::monitor::parse_cq;
use mercury_adapter::{Ack, Bandwidth, Command, ControlEvent};
use mercury_contract::{attach, contract, next_event, Station, Tag, Target};
use std::time::Duration;
use tokio::sync::mpsc;

/// The listen mode both stations use here: DATAC3 (126-byte frames).
const MODE: u8 = 1;

async fn port(st: &Station) -> (BroadcastPort, mpsc::Receiver<KissFrame>) {
    BroadcastPort::connect(&st.bcast.ip().to_string(), st.bcast.port(), Duration::from_secs(3))
        .await
        .unwrap_or_else(|e| panic!("broadcast port of {}: {e}", st.call))
}

async fn received(rx: &mut mpsc::Receiver<KissFrame>, within: Duration) -> Vec<u8> {
    let end = tokio::time::Instant::now() + within;
    loop {
        let f =
            tokio::time::timeout_at(end, rx.recv()).await.expect("a broadcast within the timeout").expect("port open");
        if f.cmd == kiss::CMD_DATA_STD {
            return f.payload;
        }
    }
}

/// A sends `len` bytes on its broadcast port; what B's broadcast port delivers.
async fn send_and_receive(t: &Target, len: usize) -> (Vec<u8>, Vec<u8>) {
    let a = attach(&t.a, t).await;
    let b = attach(&t.b, t).await;
    for x in [&a, &b] {
        assert_eq!(x.client.command(&Command::Mode(MODE)).await, Ok(Ack::Ok));
    }
    let (pa, _ra) = port(&t.a).await;
    let (_pb, mut rb) = port(&t.b).await;
    let payload: Vec<u8> = (0..len).map(|i| (i * 7 + 3) as u8).collect();
    pa.send(&payload).await.unwrap();
    let got = received(&mut rb, t.arq_timeout()).await;
    (payload, got)
}

#[tokio::test(flavor = "multi_thread")]
async fn kiss_payload_at_capacity_intact() {
    contract("kiss_payload_at_capacity_intact", Tag::Both, |t| async move {
        let cap = kiss::wrapped_capacity(MODE).unwrap();
        let (sent, got) = send_and_receive(&t, cap).await;
        assert_eq!(got, sent);
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn over_capacity_payload_truncated() {
    contract("over_capacity_payload_truncated", Tag::Both, |t| async move {
        let cap = kiss::wrapped_capacity(MODE).unwrap();
        let (sent, got) = send_and_receive(&t, cap + 20).await;
        assert_eq!(got, sent[..cap], "cut to the {cap}-byte capacity");
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn cqframe_and_monitor_lines_parse() {
    contract("cqframe_and_monitor_lines_parse", Tag::Both, |t| async move {
        let a = attach(&t.a, &t).await;
        let mut b = attach(&t.b, &t).await;
        assert_eq!(b.client.command(&Command::Monitor(true)).await, Ok(Ack::Ok));
        let cq = Command::CqFrame { source: t.a.call.clone(), bw: Bandwidth::Bw2300 };
        assert_eq!(a.client.command(&cq).await, Ok(Ack::Ok));
        let call = t.a.call.as_str().to_string();
        let ev =
            next_event(&mut b, t.arq_timeout(), "CQFRAME on B", |e| matches!(e, ControlEvent::CqFrame { .. })).await;
        assert_eq!(ev, ControlEvent::CqFrame { source: call.clone(), bandwidth: "2300".into() });
        let ev = next_event(
            &mut b,
            t.arq_timeout(),
            "MONITOR ... CQ on B",
            |e| matches!(e, ControlEvent::Monitor(m) if m.contains(" CQ ")),
        )
        .await;
        let ControlEvent::Monitor(rest) = ev else { unreachable!() };
        let cq = parse_cq(&rest).unwrap_or_else(|| panic!("MONITOR line does not parse: {rest:?}"));
        assert_eq!(cq.from, call);
        assert_eq!(b.client.command(&Command::Monitor(false)).await, Ok(Ack::Ok));
    })
    .await;
}
