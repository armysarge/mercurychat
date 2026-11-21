//! MCH/1 minor 1 frames (M3c B2) cross Mercury's ARQ link byte-exact: Mercury
//! carries the data stream opaquely, so a new frame type needs nothing of it.

use mch_protocol::payload::*;
use mch_protocol::{encode, ObjectId};
use mercury_contract::{contract, link, Tag};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test(flavor = "multi_thread")]
async fn new_frames_cross_arq_unchanged() {
    contract("new_frames_cross_arq_unchanged", Tag::Both, |t| async move {
        let (mut a, mut b) = link(&t).await;
        let id = ObjectId([0x5a; 16]);
        let cand = QsyCandidate { freq_hz: 7_105_000, mode: "PKTUSB".into(), passband_hz: 3000, name: "40m".into() };
        let frames = [
            QsyPropose { candidates: vec![cand], note: Some("QRM".into()) }.to_frame(id).unwrap(),
            QsyAnswer::Accept { index: 0 }.to_frame(id).unwrap(),
            InfoRequest { what: InfoWhat::Station }.to_frame(id).unwrap(),
            InfoResponse {
                snr: Some(SnrReport { db: -3.5, age_s: 4, source: "arq".into() }),
                station: Some(StationShare { locator: Some("KG44".into()), ..StationShare::default() }),
                reason: None,
            }
            .to_frame(id)
            .unwrap(),
        ];
        let bytes: Vec<u8> = frames.iter().flat_map(|f| encode(f).unwrap()).collect();
        a.data.write_all(&bytes).await.unwrap();
        let mut got = vec![0u8; bytes.len()];
        tokio::time::timeout(t.arq_timeout(), b.data.read_exact(&mut got)).await.expect("delivered in time").unwrap();
        assert_eq!(got, bytes);
    })
    .await;
}
