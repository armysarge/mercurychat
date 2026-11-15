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
