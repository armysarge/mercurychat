use mercury_adapter::fake::FakeNet;
use mercury_adapter::*;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn next_control(a: &mut Attachment) -> ControlEvent {
    loop {
        match tokio::time::timeout(Duration::from_secs(2), a.events.recv()).await.unwrap().unwrap() {
            AdapterEvent::Control(e) => return e,
            AdapterEvent::RawLine(_) => continue,
            other => panic!("unexpected {other:?}"),
        }
    }
}

#[tokio::test]
async fn second_client_is_reported_occupied() {
    let net = FakeNet::start().await;
    let ep = Endpoint::base("127.0.0.1", net.a_base);
    let _first = MercuryClient::attach(&ep, Timeouts::default()).await.unwrap();
    match MercuryClient::attach(&ep, Timeouts::default()).await {
        Err(AttachError::Occupied { plane: "control", .. }) => {}
        Err(e) => panic!("wrong error {e}"),
        Ok(_) => panic!("second attach must fail"),
    }
}

#[tokio::test]
async fn partial_attach_closes_both() {
    // Data port with nothing listening: control must not stay attached.
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    let ep = Endpoint { host: "127.0.0.1".into(), control_port: port, data_port: 1 };
    assert!(MercuryClient::attach(&ep, Timeouts::default()).await.is_err());
    let (mut s, _) = l.accept().await.unwrap();
    let mut b = [0u8; 1];
    assert_eq!(tokio::time::timeout(Duration::from_secs(2), s.read(&mut b)).await.unwrap().unwrap(), 0);
}

#[tokio::test]
async fn connect_exchange_binary_disconnect() {
    let net = FakeNet::start().await;
    let mut a = MercuryClient::attach(&Endpoint::base("127.0.0.1", net.a_base), Timeouts::default()).await.unwrap();
    let mut b = MercuryClient::attach(&Endpoint::base("127.0.0.1", net.b_base), Timeouts::default()).await.unwrap();
    let ca = Callsign::parse("LABA").unwrap();
    let cb = Callsign::parse("LABB").unwrap();
    assert_eq!(a.client.command(&Command::MyCall(ca.clone())).await, Ok(Ack::Ok));
    assert_eq!(next_control(&mut a).await, ControlEvent::Registered("LABA".into()));
    assert_eq!(b.client.command(&Command::MyCall(cb.clone())).await, Ok(Ack::Ok));
    assert_eq!(b.client.command(&Command::Listen(true)).await, Ok(Ack::Ok));
    assert_eq!(a.client.command(&Command::Connect { mycall: ca, theircall: cb }).await, Ok(Ack::Ok));
    let want = ControlEvent::Connected { source: "LABA".into(), dest: "LABB".into(), bandwidth: "2300".into() };
    assert_eq!(next_control(&mut a).await, want);
    loop {
        if next_control(&mut b).await == want { break; }
    }
    let payload: Vec<u8> = (0..=255u8).cycle().take(5000).collect();
    a.data.write_all(&payload).await.unwrap();
    let mut got = vec![0u8; payload.len()];
    tokio::time::timeout(Duration::from_secs(2), b.data.read_exact(&mut got)).await.unwrap().unwrap();
    assert_eq!(got, payload);
    assert_eq!(a.client.command(&Command::Disconnect).await, Ok(Ack::Ok));
    loop {
        if next_control(&mut b).await == ControlEvent::Disconnected { break; }
    }
}
