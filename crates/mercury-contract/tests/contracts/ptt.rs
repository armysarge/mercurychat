//! A2 PTT: Mercury frames each transmission with PTT ON / PTT OFF; the lines
//! alternate, starting ON, and none is left open once the session is over.

use mch_protocol::payload::chat_frame;
use mch_protocol::{encode, ObjectId};
use mercury_adapter::{Ack, Command, ControlEvent};
use mercury_contract::{collect_for, contract, link, next_event, Tag};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn ptt_or_down(e: &ControlEvent) -> bool {
    matches!(e, ControlEvent::Disconnected | ControlEvent::Ptt(_))
}

#[tokio::test(flavor = "multi_thread")]
async fn ptt_frames_each_transmission() {
    contract("ptt_frames_each_transmission", Tag::Both, |t| async move {
        let (mut a, mut b) = link(&t).await;
        // A chat each way.
        for n in 0..2u8 {
            let bytes = encode(&chat_frame(ObjectId([n + 1; 16]), "ptt contract").unwrap()).unwrap();
            let (from, to) = if n == 0 { (&mut a.data, &mut b.data) } else { (&mut b.data, &mut a.data) };
            from.write_all(&bytes).await.unwrap();
            let mut got = vec![0u8; bytes.len()];
            tokio::time::timeout(t.arq_timeout(), to.read_exact(&mut got)).await.expect("delivered in time").unwrap();
        }
        assert_eq!(a.client.command(&Command::Disconnect).await, Ok(Ack::Ok));
        // Every PTT line up to DISCONNECTED, then the teardown's keydown after it.
        let mut ptt = Vec::new();
        while let ControlEvent::Ptt(on) = next_event(&mut a, t.arq_timeout(), "DISCONNECTED on A", ptt_or_down).await {
            ptt.push(on);
        }
        ptt.extend(collect_for(&mut a, t.reply()).await.into_iter().filter_map(|e| match e {
            ControlEvent::Ptt(on) => Some(on),
            _ => None,
        }));
        assert!(!ptt.is_empty(), "no PTT line at all");
        for (i, on) in ptt.iter().enumerate() {
            assert_eq!(*on, i % 2 == 0, "PTT lines do not alternate from ON: {ptt:?}");
        }
        assert_eq!(ptt.last(), Some(&false), "a PTT ON was left open: {ptt:?}");
    })
    .await;
}
