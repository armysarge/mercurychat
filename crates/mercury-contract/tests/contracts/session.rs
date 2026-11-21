//! A2 Session: CONNECT leads to CONNECTED on both sides, the callee seeing
//! PENDING first (Mercury's TNC.md: PENDING is an incoming connect, or a CQ
//! going out, never the caller's own call); DISCONNECT and ABORT end it; an
//! aborted call to nobody ends; a call to nobody times out; BW and MODE.

use mercury_adapter::{Ack, Bandwidth, Callsign, Command, ControlEvent};
use mercury_contract::{attach, contract, link, next_event, Tag};

#[tokio::test(flavor = "multi_thread")]
async fn connect_pending_then_connected() {
    contract("connect_pending_then_connected", Tag::Both, |t| async move {
        let mut a = attach(&t.a, &t).await;
        let mut b = attach(&t.b, &t).await;
        assert_eq!(a.client.command(&Command::MyCall(t.a.call.clone())).await, Ok(Ack::Ok));
        assert_eq!(b.client.command(&Command::MyCall(t.b.call.clone())).await, Ok(Ack::Ok));
        assert_eq!(b.client.command(&Command::Listen(true)).await, Ok(Ack::Ok));
        let connect = Command::Connect { mycall: t.a.call.clone(), theircall: t.b.call.clone() };
        assert_eq!(a.client.command(&connect).await, Ok(Ack::Ok));
        next_event(&mut b, t.arq_timeout(), "PENDING on the callee", |e| *e == ControlEvent::Pending).await;
        let (ca, cb) = (t.a.call.as_str().to_string(), t.b.call.as_str().to_string());
        let ev = next_event(&mut a, t.arq_timeout(), "CONNECTED on A", |e| matches!(e, ControlEvent::Connected { .. }))
            .await;
        match ev {
            ControlEvent::Connected { source, dest, bandwidth } => {
                assert_eq!((source.as_str(), dest.as_str()), (ca.as_str(), cb.as_str()));
                assert!(!bandwidth.is_empty());
            }
            other => panic!("{other:?}"),
        }
        next_event(&mut b, t.arq_timeout(), "CONNECTED on B", |e| matches!(e, ControlEvent::Connected { .. })).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn disconnect_gives_disconnected() {
    contract("disconnect_gives_disconnected", Tag::Both, |t| async move {
        let (mut a, mut b) = link(&t).await;
        assert_eq!(a.client.command(&Command::Disconnect).await, Ok(Ack::Ok));
        next_event(&mut a, t.arq_timeout(), "DISCONNECTED on A", |e| *e == ControlEvent::Disconnected).await;
        next_event(&mut b, t.arq_timeout(), "DISCONNECTED on B", |e| *e == ControlEvent::Disconnected).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn abort_ends_session() {
    contract("abort_ends_session", Tag::Both, |t| async move {
        let (mut a, _b) = link(&t).await;
        assert_eq!(a.client.command(&Command::Abort).await, Ok(Ack::Ok));
        next_event(&mut a, t.arq_timeout(), "DISCONNECTED after ABORT", |e| *e == ControlEvent::Disconnected).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn abort_ends_unanswered_call() {
    contract("abort_ends_unanswered_call", Tag::Both, |t| async move {
        if let Some(net) = t.fake() {
            net.set_unanswered_calls(true);
        }
        let mut a = attach(&t.a, &t).await;
        assert_eq!(a.client.command(&Command::MyCall(t.a.call.clone())).await, Ok(Ack::Ok));
        // Nobody listens as this call: the call stays pending until aborted.
        let nobody = Callsign::parse("N0NONE").unwrap();
        assert_eq!(
            a.client.command(&Command::Connect { mycall: t.a.call.clone(), theircall: nobody }).await,
            Ok(Ack::Ok)
        );
        // The caller gets no PENDING for its own call: give it time to start calling.
        tokio::time::sleep(t.reply()).await;
        assert_eq!(a.client.command(&Command::Abort).await, Ok(Ack::Ok));
        next_event(&mut a, t.arq_timeout(), "the call ended", |e| {
            matches!(e, ControlEvent::CancelPending | ControlEvent::Disconnected)
        })
        .await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn call_to_nobody_times_out() {
    contract("call_to_nobody_times_out", Tag::RealOnly, |t| async move {
        let mut a = attach(&t.a, &t).await;
        assert_eq!(a.client.command(&Command::MyCall(t.a.call.clone())).await, Ok(Ack::Ok));
        let nobody = Callsign::parse("N0NONE").unwrap();
        assert_eq!(
            a.client.command(&Command::Connect { mycall: t.a.call.clone(), theircall: nobody }).await,
            Ok(Ack::Ok)
        );
        next_event(&mut a, std::time::Duration::from_secs(300), "the call timing out", |e| {
            matches!(e, ControlEvent::Disconnected | ControlEvent::CancelPending)
        })
        .await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn bw_and_mode_ok() {
    contract("bw_and_mode_ok", Tag::Both, |t| async move {
        let a = attach(&t.a, &t).await;
        assert_eq!(a.client.command(&Command::Bw(Bandwidth::Bw500)).await, Ok(Ack::Ok));
        assert_eq!(a.client.command(&Command::Bw(Bandwidth::Bw2300)).await, Ok(Ack::Ok));
        assert_eq!(a.client.command(&Command::Mode(0)).await, Ok(Ack::Ok));
    })
    .await;
}
