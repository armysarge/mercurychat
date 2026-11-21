//! A2 Attach: control and data connect; MYCALL, LISTEN and PUBLIC are
//! acknowledged; status lines parse; an unknown control line is tolerated.

use mercury_adapter::{Ack, Command, ControlEvent};
use mercury_contract::{attach, contract, next_event, Tag};

#[tokio::test(flavor = "multi_thread")]
async fn attach_control_and_data() {
    contract("attach_control_and_data", Tag::Both, |t| async move {
        let a = attach(&t.a, &t).await;
        assert!(!a.client.is_closed());
        assert!(a.data.peer_addr().is_ok());
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn mycall_listen_public_ok() {
    contract("mycall_listen_public_ok", Tag::Both, |t| async move {
        let a = attach(&t.a, &t).await;
        assert_eq!(a.client.command(&Command::MyCall(t.a.call.clone())).await, Ok(Ack::Ok));
        for cmd in [Command::Listen(true), Command::Listen(false), Command::Public(true), Command::Public(false)] {
            assert_eq!(a.client.command(&cmd).await, Ok(Ack::Ok), "{cmd:?}");
        }
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn status_lines_parse() {
    contract("status_lines_parse", Tag::Both, |t| async move {
        let mut a = attach(&t.a, &t).await;
        assert_eq!(a.client.command(&Command::MyCall(t.a.call.clone())).await, Ok(Ack::Ok));
        let call = t.a.call.as_str().to_string();
        next_event(&mut a, t.reply(), "REGISTERED", |e| *e == ControlEvent::Registered(call.clone())).await;
        a.client.query("BUFFER").await.unwrap();
        next_event(&mut a, t.reply(), "BUFFER", |e| matches!(e, ControlEvent::Buffer(_))).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn iamalive_seen() {
    // Mercury sends IAMALIVE on its own while a client is attached; the fake does not.
    contract("iamalive_seen", Tag::RealOnly, |t| async move {
        let mut a = attach(&t.a, &t).await;
        next_event(&mut a, std::time::Duration::from_secs(90), "IAMALIVE", |e| *e == ControlEvent::IAmAlive).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn unknown_control_line_tolerated() {
    contract("unknown_control_line_tolerated", Tag::FakeOnly, |t| async move {
        let mut a = attach(&t.a, &t).await;
        t.fake().unwrap().send_control(0, "SOMETHINGNEW 1 2 3");
        next_event(&mut a, t.reply(), "the unknown line", |e| matches!(e, ControlEvent::Unknown(_))).await;
        assert_eq!(a.client.command(&Command::Listen(true)).await, Ok(Ack::Ok), "still answering after it");
    })
    .await;
}
