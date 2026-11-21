//! A2 Broadcast: a KISS payload at the listen mode's capacity arrives intact;
//! a longer one arrives cut to the capacity; CQFRAME and MONITOR lines parse.

use mercury_adapter::kiss;
use mercury_adapter::monitor::parse_cq;
use mercury_adapter::{Ack, Bandwidth, Command, ControlEvent};
use mercury_contract::{attach, broadcast_across, contract, next_event, Tag, Target, LAB_MODE as MODE};

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
