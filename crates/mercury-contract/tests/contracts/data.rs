//! A2 Data plane: bytes written on A's data port arrive byte-exact on B's,
//! including the largest MCH/1 chat frame (4096 bytes of text) split
//! across writes; BUFFER drains to 0.

use mch_protocol::payload::chat_frame;
use mch_protocol::{encode, ObjectId};
use mercury_adapter::ControlEvent;
use mercury_contract::{contract, link, next_event, Tag};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test(flavor = "multi_thread")]
async fn data_byte_exact() {
    contract("data_byte_exact", Tag::Both, |t| async move {
        let (mut a, mut b) = link(&t).await;
        // A full-size chat: MCH/1 caps chat text at 4096 bytes.
        let text = "73 ✓ ".repeat(585); // 4095 bytes of UTF-8
        let bytes = encode(&chat_frame(ObjectId([7; 16]), &text).unwrap()).unwrap();
        let (one, rest) = bytes.split_at(13);
        let (two, three) = rest.split_at(rest.len() / 2 + 5);
        for part in [one, two, three] {
            a.data.write_all(part).await.unwrap();
        }
        let mut got = vec![0u8; bytes.len()];
        tokio::time::timeout(t.arq_timeout(), b.data.read_exact(&mut got)).await.expect("delivered in time").unwrap();
        assert_eq!(got, bytes);
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn buffer_drains_to_zero() {
    contract("buffer_drains_to_zero", Tag::Both, |t| async move {
        let (mut a, mut b) = link(&t).await;
        let payload: Vec<u8> = (0..=255u8).cycle().take(2000).collect();
        a.data.write_all(&payload).await.unwrap();
        let mut got = vec![0u8; payload.len()];
        tokio::time::timeout(t.arq_timeout(), b.data.read_exact(&mut got)).await.expect("delivered in time").unwrap();
        // After delivery Mercury reports an empty queue (asked, or on its own).
        let end = tokio::time::Instant::now() + t.arq_timeout();
        loop {
            a.client.query("BUFFER").await.unwrap();
            let ev = next_event(&mut a, t.reply(), "BUFFER", |e| matches!(e, ControlEvent::Buffer(_))).await;
            if ev == ControlEvent::Buffer(0) {
                break;
            }
            assert!(tokio::time::Instant::now() < end, "BUFFER never drained: {ev:?}");
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
    })
    .await;
}
