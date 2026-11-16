use mercury_adapter::broadcast::BroadcastPort;
use mercury_adapter::fake::FakeNet;
use mercury_adapter::kiss::{self, KissFrame, CMD_DATA_STD};
use mercury_adapter::monitor::parse_cq;
use mercury_adapter::*;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;

const T: Duration = Duration::from_secs(3);

async fn attach(net: &FakeNet, side: usize, call: &str) -> Attachment {
    let base = [net.a_base, net.b_base][side];
    let a = MercuryClient::attach(&Endpoint::base("127.0.0.1", base), Timeouts::default()).await.unwrap();
    let c = Callsign::parse(call).unwrap();
    assert_eq!(a.client.command(&Command::MyCall(c)).await, Ok(Ack::Ok));
    a
}

async fn bcast(net: &FakeNet, side: usize) -> (BroadcastPort, mpsc::Receiver<KissFrame>) {
    let port = [net.a_bcast, net.b_bcast][side];
    BroadcastPort::connect("127.0.0.1", port, T).await.unwrap()
}

/// Next control event matching `f`, skipping unrelated lines.
async fn control_until(a: &mut Attachment, mut f: impl FnMut(&ControlEvent) -> bool) -> ControlEvent {
    loop {
        match tokio::time::timeout(T, a.events.recv()).await.expect("timed out").expect("closed") {
            AdapterEvent::Control(e) if f(&e) => return e,
            _ => continue,
        }
    }
}

async fn quiet(rx: &mut mpsc::Receiver<KissFrame>) {
    assert!(tokio::time::timeout(Duration::from_millis(1200), rx.recv()).await.is_err(), "unexpected frame");
}

#[tokio::test]
async fn cq_reaches_the_other_side_with_monitor_snr() {
    let net = FakeNet::start().await;
    let mut a = attach(&net, 0, "LABA").await;
    let mut b = attach(&net, 1, "LABB").await;
    assert_eq!(b.client.command(&Command::Monitor(true)).await, Ok(Ack::Ok));
    let cq = Command::CqFrame { source: Callsign::parse("LABA").unwrap(), bw: Bandwidth::Bw2300 };
    assert_eq!(a.client.command(&cq).await, Ok(Ack::Ok));
    // Sender: PENDING, PTT ON, PTT OFF, CANCELPENDING, in that order.
    let mut seen = Vec::new();
    while seen.len() < 4 {
        let ev = tokio::time::timeout(T, a.events.recv()).await.unwrap().unwrap();
        if let AdapterEvent::Control(e @ (ControlEvent::Pending | ControlEvent::Ptt(_) | ControlEvent::CancelPending)) =
            ev
        {
            seen.push(e);
        }
    }
    assert_eq!(
        seen,
        vec![ControlEvent::Pending, ControlEvent::Ptt(true), ControlEvent::Ptt(false), ControlEvent::CancelPending]
    );
    // Receiver: the CQFRAME report and the MONITOR line with SNR.
    let mut got_cq = false;
    let mon = loop {
        match control_until(&mut b, |e| matches!(e, ControlEvent::Monitor(_) | ControlEvent::CqFrame { .. })).await {
            ControlEvent::Monitor(m) => break m,
            ControlEvent::CqFrame { source, bandwidth } => {
                assert_eq!((source.as_str(), bandwidth.as_str()), ("LABA", "2300"));
                got_cq = true;
            }
            _ => unreachable!(),
        }
    };
    let cq = parse_cq(&mon).expect("parse");
    assert_eq!((cq.from.as_str(), cq.bw.as_str(), cq.snr_db), ("LABA", "2300", 7.5));
    assert!(got_cq, "CQFRAME precedes the MONITOR line");
    assert!(net.log().iter().any(|l| l == "A> CQFRAME LABA 2300"));
}

#[tokio::test]
async fn beacon_crosses_only_matching_listen_modes() {
    let net = FakeNet::start().await;
    let mut a = attach(&net, 0, "LABA").await;
    let b = attach(&net, 1, "LABB").await;
    let (pa, _ra) = bcast(&net, 0).await;
    let (_pb, mut rb) = bcast(&net, 1).await;
    // Default modes (1 and 1): delivered, with PTT around it on the sender.
    pa.send(b"MCB1-one").await.unwrap();
    let f = tokio::time::timeout(T, rb.recv()).await.unwrap().unwrap();
    assert_eq!(f, KissFrame { cmd: CMD_DATA_STD, payload: b"MCB1-one".to_vec() });
    control_until(&mut a, |e| *e == ControlEvent::Ptt(true)).await;
    control_until(&mut a, |e| *e == ControlEvent::Ptt(false)).await;
    // B moves to MFSK (11): A's DATAC3 beacon is no longer heard.
    assert_eq!(b.client.command(&Command::Mode(11)).await, Ok(Ack::Ok));
    pa.send(b"MCB1-two").await.unwrap();
    quiet(&mut rb).await;
    // Matching again on both: heard.
    assert_eq!(a.client.command(&Command::Mode(11)).await, Ok(Ack::Ok));
    pa.send(b"MCB1-three").await.unwrap();
    let f = tokio::time::timeout(T, rb.recv()).await.unwrap().unwrap();
    assert_eq!(f.payload, b"MCB1-three");
    assert_eq!(net.broadcast_log().len(), 3); // the mismatched one was still transmitted
}

#[tokio::test]
async fn over_capacity_payload_is_truncated_by_the_fake() {
    let net = FakeNet::start().await;
    let _a = attach(&net, 0, "LABA").await;
    let _b = attach(&net, 1, "LABB").await;
    let (pa, _ra) = bcast(&net, 0).await;
    let (_pb, mut rb) = bcast(&net, 1).await;
    let cap = kiss::wrapped_capacity(1).unwrap();
    assert_eq!(cap, 123);
    let exact: Vec<u8> = (0..cap as u32).map(|i| (i % 251) as u8).collect();
    pa.send(&exact).await.unwrap();
    assert_eq!(tokio::time::timeout(T, rb.recv()).await.unwrap().unwrap().payload, exact);
    let mut over = exact.clone();
    over.push(0xC0);
    pa.send(&over).await.unwrap();
    assert_eq!(tokio::time::timeout(T, rb.recv()).await.unwrap().unwrap().payload, exact);
    assert_eq!(net.broadcast_log().last().unwrap(), &(0, exact));
}

#[tokio::test]
async fn broadcast_during_session_is_dropped() {
    let net = FakeNet::start().await;
    let mut a = attach(&net, 0, "LABA").await;
    let (pa, _ra) = bcast(&net, 0).await;
    let (_pb, mut rb) = bcast(&net, 1).await;
    // Side B is driven raw so the bare BUSY answer to MODE can be read.
    let mut bc = tokio::net::TcpStream::connect(("127.0.0.1", net.b_base)).await.unwrap();
    bc.write_all(b"MYCALL LABB\rLISTEN ON\r").await.unwrap();
    let ca = Callsign::parse("LABA").unwrap();
    let cb = Callsign::parse("LABB").unwrap();
    assert_eq!(a.client.command(&Command::Connect { mycall: ca, theircall: cb }).await, Ok(Ack::Ok));
    control_until(&mut a, |e| matches!(e, ControlEvent::Connected { .. })).await;
    assert!(net.session_up());
    bc.write_all(b"MODE 1\r").await.unwrap();
    let mut seen = String::new();
    let mut buf = [0u8; 256];
    while !seen.contains("BUSY") {
        let n = tokio::time::timeout(T, bc.read(&mut buf)).await.unwrap().unwrap();
        assert!(n > 0);
        seen.push_str(&String::from_utf8_lossy(&buf[..n]));
    }
    pa.send(b"MCB1-nope").await.unwrap();
    quiet(&mut rb).await;
    assert!(net.broadcast_log().is_empty());
}

#[tokio::test]
async fn kiss_split_at_every_boundary_yields_one_frame() {
    let payload: Vec<u8> = vec![0x4D, 0xC0, 0x01, 0xDB, 0xFF, 0xC0, 0xDB];
    let wire = kiss::encode(CMD_DATA_STD, &payload);
    // A local listener stands in for Mercury's broadcast port.
    let l = std::sync::Arc::new(tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap());
    let port = l.local_addr().unwrap().port();
    for cut in 1..wire.len() {
        let acc = tokio::spawn({
            let l = l.clone();
            async move { l.accept().await.unwrap().0 }
        });
        let (_port, mut rx) = BroadcastPort::connect("127.0.0.1", port, T).await.unwrap();
        let mut s = acc.await.unwrap();
        s.write_all(&wire[..cut]).await.unwrap();
        s.flush().await.unwrap();
        tokio::time::sleep(Duration::from_millis(5)).await;
        s.write_all(&wire[cut..]).await.unwrap();
        let f = tokio::time::timeout(T, rx.recv()).await.unwrap().unwrap();
        assert_eq!(f.payload, payload, "cut at {cut}");
        assert!(tokio::time::timeout(Duration::from_millis(20), rx.recv()).await.is_err());
    }
}

#[test]
fn parse_cq_rejects_other_monitor_lines() {
    let ok = parse_cq("DATAC16 CQ FROM=ZS6ABC BW=2300 SNR=7.5").unwrap();
    assert_eq!((ok.from.as_str(), ok.bw.as_str(), ok.snr_db), ("ZS6ABC", "2300", 7.5));
    assert_eq!(parse_cq("DATAC16 CQ FROM=A BW=500 SNR=-3").unwrap().snr_db, -3.0);
    for bad in [
        "",
        "DATAC16 CQ",
        "DATAC3 CQ FROM=A BW=2300 SNR=1",
        "DATAC16 BEACON FROM=A BW=2300 SNR=1",
        "DATAC16 CQ FROM=A BW=2300",
        "DATAC16 CQ FROM=A BW=2300 SNR=x",
        "DATAC16 CQ FROM=A BW=2300 SNR=1 EXTRA",
        "DATAC16 CQ BW=2300 FROM=A SNR=1",
        "DATAC16 CQ FROM= BW=2300 SNR=1",
        "DATAC16 CQ FROM=A BW=2300 SNR=NaN",
    ] {
        assert!(parse_cq(bad).is_none(), "{bad:?}");
    }
}
