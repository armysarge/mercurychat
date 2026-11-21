//! Byte-exact MCB/1 beacon vectors (docs/03 §6-7). `MCH_BLESS=1` regenerates
//! `tests/fixtures/mcb1/*.hex` after an intentional, reviewed format change.

use mch_protocol::beacon::*;
use std::path::PathBuf;

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/mcb1")
}

fn vectors() -> Vec<(&'static str, Beacon)> {
    vec![
        ("beacon_min", Beacon::new(0, "K1", None, 0).unwrap()),
        ("beacon_grid4", Beacon::new(0x0000_0007, "N0CALL", Some("FN42"), CAP_CHAT).unwrap()),
        ("beacon_portable_grid6", Beacon::new(0x8000_0001, "N0CALL/P", Some("JJ00aa"), CAP_CHAT | CAP_FILE).unwrap()),
        ("beacon_max", Beacon::new(u32::MAX, "ABCDEFGHIJKLMNO", Some("RR99xx99"), 0xFFFF).unwrap()),
    ]
}

#[test]
fn mcb1_vectors_match() {
    let dir = fixtures();
    let bless = std::env::var("MCH_BLESS").is_ok();
    if bless {
        std::fs::create_dir_all(&dir).unwrap();
    }
    for (name, b) in vectors() {
        let bytes = b.encode();
        assert!(bytes.len() <= MAX_LEN);
        let path = dir.join(format!("{name}.hex"));
        if bless {
            std::fs::write(&path, hex::encode(&bytes) + "\n").unwrap();
            continue;
        }
        let want = std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("missing {path:?}; run with MCH_BLESS=1"));
        assert_eq!(hex::encode(&bytes), want.trim(), "vector {name} changed");
        assert_eq!(Beacon::decode(&hex::decode(want.trim()).unwrap()).unwrap(), b, "decode {name}");
    }
}

/// Kinds 2 and 3 (M3c B9), in the same fixture directory.
fn new_kinds() -> Vec<(&'static str, Vec<u8>, Received)> {
    let b = Beacon::new(0x0102_0304, "ZS6ABC", Some("KG44dg"), CAP_CHAT | CAP_FILE).unwrap();
    let all = ExtendedBeacon::new(
        b.clone(),
        vec![Tlv::Name("Pat".into()), Tlv::Qth("Pretoria".into()), Tlv::Status("QRV 40m, 73 ✓".into())],
    )
    .unwrap();
    let status = ExtendedBeacon::new(b, vec![Tlv::Status("on the air".into())]).unwrap();
    let three = QsySuggestion::new(
        7,
        "ZS6ABC",
        "N0CALL/P",
        vec![(7_105_000, Mode::PktUsb), (14_105_000, Mode::Usb), (3_585_000, Mode::Lsb)],
    )
    .unwrap();
    let one = QsySuggestion::new(8, "ZS6ABC", "K1", vec![(28_120_000, Mode::Unspecified)]).unwrap();
    vec![
        ("extended_all_fields", all.encode(), Received::Extended(all)),
        ("extended_status_only", status.encode(), Received::Extended(status)),
        ("qsy_three_candidates", three.encode(), Received::Qsy(three)),
        ("qsy_one_unspecified", one.encode(), Received::Qsy(one)),
    ]
}

#[test]
fn mcb1_new_kind_vectors_match() {
    let dir = fixtures();
    let bless = std::env::var("MCH_BLESS").is_ok();
    for (name, bytes, want) in new_kinds() {
        let path = dir.join(format!("{name}.hex"));
        if bless {
            std::fs::write(&path, hex::encode(&bytes) + "\n").unwrap();
            continue;
        }
        let stored =
            std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("missing {path:?}; run with MCH_BLESS=1"));
        assert_eq!(hex::encode(&bytes), stored.trim(), "vector {name} changed");
        assert_eq!(classify(&hex::decode(stored.trim()).unwrap()), want, "decode {name}");
    }
}
