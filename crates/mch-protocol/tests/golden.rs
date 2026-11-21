//! Byte-exact MCH/1 golden vectors (docs/03 §7). The same vectors must pass on
//! Windows x64, Linux amd64 and Linux arm64. Set `MCH_BLESS=1` to regenerate
//! `tests/fixtures/mch1/*.hex` after an intentional, reviewed format change.

use mch_protocol::payload::*;
use mch_protocol::*;
use std::path::PathBuf;

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/mch1")
}

fn vectors() -> Vec<(&'static str, Frame)> {
    let id = ObjectId([0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff]);
    let hello = Hello {
        app: "MercuryChat".into(),
        app_version: "0.0.1".into(),
        proto_minor: 0,
        nonce: "0f0e0d0c0b0a09080706050403020100".into(),
        callsign: "N0CALL".into(),
        capabilities: vec!["chat".into(), "file".into()],
        max_frame: 8192,
        max_file: 262144,
        max_chunk: 1024,
    };
    vec![
        ("hello", hello.to_frame().unwrap()),
        (
            "hello_ack",
            HelloAck { nonce: hello.nonce.clone(), major: 1, minor: 0, capabilities: vec!["chat".into()] }
                .to_frame()
                .unwrap(),
        ),
        ("chat_ascii", chat_frame(id, "CQ de N0CALL").unwrap()),
        ("chat_utf8_multiline", chat_frame(id, "73 ✓\r\nline two\nÅß€😀").unwrap()),
        ("chat_empty", chat_frame(id, "").unwrap()),
        ("chat_receipt", Frame::empty(FrameType::ChatReceipt, id)),
        (
            "file_offer",
            FileOffer {
                name: "log.adi".into(),
                size: 3,
                sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".into(),
                mime: Some("text/plain".into()),
            }
            .to_frame(id)
            .unwrap(),
        ),
        ("file_accept", FileAccept { chunk: 512 }.to_frame(id).unwrap()),
        ("file_chunk_kiss_bytes", chunk_frame(id, 0x0102030405060708, &[0xC0, 0xDB, 0xDC, 0xDD, 0x00, 0xFF]).unwrap()),
        ("file_end", Frame::empty(FrameType::FileEnd, id)),
        ("file_receipt", Frame::empty(FrameType::FileReceipt, id)),
        ("file_cancel", Reason::new("user", "cancelled").to_frame(FrameType::FileCancel, id).unwrap()),
        ("error", Reason::new("unsupported", "").to_frame(FrameType::Error, ObjectId::ZERO).unwrap()),
        // Minor 1 (M3c B2).
        (
            "qsy_propose",
            QsyPropose {
                candidates: vec![
                    QsyCandidate {
                        freq_hz: 7_105_000,
                        mode: "PKTUSB".into(),
                        passband_hz: 3000,
                        name: "40m chat".into(),
                    },
                    QsyCandidate { freq_hz: 14_105_000, mode: "".into(), passband_hz: 0, name: "".into() },
                ],
                note: Some("QRM here".into()),
            }
            .to_frame(id)
            .unwrap(),
        ),
        ("qsy_answer_accept", QsyAnswer::Accept { index: 1 }.to_frame(id).unwrap()),
        ("qsy_answer_decline", QsyAnswer::Decline { reason: Some("rig".into()) }.to_frame(id).unwrap()),
        ("info_request_snr", InfoRequest { what: InfoWhat::Snr }.to_frame(id).unwrap()),
        (
            "info_response_snr",
            InfoResponse {
                snr: Some(SnrReport { db: 7.5, age_s: 12, source: "monitor".into() }),
                station: None,
                reason: None,
            }
            .to_frame(id)
            .unwrap(),
        ),
        (
            "info_response_station",
            InfoResponse {
                snr: None,
                station: Some(StationShare {
                    name: Some("Pat".into()),
                    qth: Some("Pretoria".into()),
                    locator: Some("KG44dg".into()),
                    rig: Some("IC-7300".into()),
                    power: Some("100 W".into()),
                    antenna: Some("EFHW".into()),
                    about: None,
                    email: None,
                }),
                reason: None,
            }
            .to_frame(id)
            .unwrap(),
        ),
        (
            "info_response_unsolicited_email",
            InfoResponse {
                snr: None,
                station: Some(StationShare { email: Some("n0call@example.com".into()), ..StationShare::default() }),
                reason: None,
            }
            .to_frame(ObjectId::ZERO)
            .unwrap(),
        ),
    ]
}

#[test]
fn golden_vectors_match() {
    let dir = fixtures();
    let bless = std::env::var("MCH_BLESS").is_ok();
    if bless {
        std::fs::create_dir_all(&dir).unwrap();
    }
    for (name, frame) in vectors() {
        let bytes = encode(&frame).unwrap();
        let path = dir.join(format!("{name}.hex"));
        if bless {
            std::fs::write(&path, hex::encode(&bytes) + "\n").unwrap();
            continue;
        }
        let want = std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("missing {path:?}; run with MCH_BLESS=1"));
        assert_eq!(hex::encode(&bytes), want.trim(), "vector {name} changed");
        // Decoding the stored bytes must give back the same frame.
        let mut d = Decoder::new();
        d.push(&hex::decode(want.trim()).unwrap());
        assert_eq!(d.next_frame().unwrap().unwrap(), frame, "decode {name}");
    }
}
