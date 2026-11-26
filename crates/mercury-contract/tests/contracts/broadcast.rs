//! A2 Broadcast: a KISS payload at the listen mode's capacity arrives intact;
//! a longer one arrives cut to the capacity; CQFRAME and MONITOR lines parse.

use mercury_adapter::kiss;
use mercury_adapter::monitor::{parse_broadcast, parse_cq};
use mercury_adapter::{Ack, Bandwidth, Command, ControlEvent};
use mercury_contract::{attach, broadcast_across, broadcast_port, contract, next_event, Tag, Target, LAB_MODE as MODE};
use std::time::Duration;
use tokio::time::Instant;

/// A sends `len` bytes on its broadcast port; what B's broadcast port delivers.
async fn send_and_receive(t: &Target, len: usize) -> (Vec<u8>, Vec<u8>) {
    let payload: Vec<u8> = (0..len).map(|i| (i * 7 + 3) as u8).collect();
    let got = broadcast_across(t, &payload).await;
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
        // Both lines come for one CQ, in no fixed order (real Mercury has sent
        // the MONITOR line first).
        let (mut cqframe, mut monitor) = (None, None);
        while cqframe.is_none() || monitor.is_none() {
            let ev = next_event(&mut b, t.arq_timeout(), "CQFRAME and MONITOR ... CQ on B", |e| {
                matches!(e, ControlEvent::CqFrame { .. }) || matches!(e, ControlEvent::Monitor(m) if m.contains(" CQ "))
            })
            .await;
            match ev {
                ControlEvent::Monitor(rest) => monitor = Some(rest),
                other => cqframe = Some(other),
            }
        }
        assert_eq!(cqframe.unwrap(), ControlEvent::CqFrame { source: call.clone(), bandwidth: "2300".into() });
        let rest = monitor.unwrap();
        let cq = parse_cq(&rest).unwrap_or_else(|| panic!("MONITOR line does not parse: {rest:?}"));
        assert_eq!(cq.from, call);
        assert_eq!(b.client.command(&Command::Monitor(false)).await, Ok(Ack::Ok));
    })
    .await;
}

/// What the latest-broadcasts list rests on: a wrapped broadcast arrives
/// byte-exact with its command, and with MONITOR on, the receiver also gets a
/// BCAST_DATA report with the S/N, close enough in time to be paired with it.
#[tokio::test(flavor = "multi_thread")]
async fn wrapped_broadcast_has_monitor_report() {
    contract("wrapped_broadcast_has_monitor_report", Tag::Both, |t| async move {
        let a = attach(&t.a, &t).await;
        let mut b = attach(&t.b, &t).await;
        for x in [&a, &b] {
            assert_eq!(x.client.command(&Command::Mode(MODE)).await, Ok(Ack::Ok));
        }
        assert_eq!(b.client.command(&Command::Monitor(true)).await, Ok(Ack::Ok));
        let (pa, _ra) = broadcast_port(&t.a).await;
        let (_pb, mut rb) = broadcast_port(&t.b).await;
        let sent = b"N0CTA: contract".to_vec();
        pa.send(&sent).await.unwrap();
        let frame = async {
            let f = tokio::time::timeout(t.arq_timeout(), rb.recv()).await.expect("the frame").expect("port open");
            (Instant::now(), f)
        };
        let report = async {
            let ev = next_event(
                &mut b,
                t.arq_timeout(),
                "MONITOR ... BCAST_DATA on B",
                |e| matches!(e, ControlEvent::Monitor(m) if parse_broadcast(m).is_some()),
            )
            .await;
            let ControlEvent::Monitor(rest) = ev else { unreachable!() };
            (Instant::now(), parse_broadcast(&rest).unwrap())
        };
        let ((frame_at, f), (report_at, r)) = tokio::join!(frame, report);
        assert_eq!(f.cmd, kiss::CMD_DATA_STD, "delivered with the sender's command");
        assert_eq!(f.payload, sent, "byte-exact, no padding");
        assert_eq!(r.mode, "DATAC3");
        let gap = if frame_at > report_at { frame_at - report_at } else { report_at - frame_at };
        let first = if frame_at < report_at { "frame" } else { "report" };
        println!("NOTE wrapped_broadcast_has_monitor_report gap_ms={} first={first} snr={}", gap.as_millis(), r.snr_db);
        assert!(gap < Duration::from_secs(2), "the report came {gap:?} from the frame");
        assert_eq!(b.client.command(&Command::Monitor(false)).await, Ok(Ack::Ok));
    })
    .await;
}
