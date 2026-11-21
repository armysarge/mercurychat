//! MCB/1 kinds 2 and 3 (M3c B7, B9) on Mercury's broadcast plane: each
//! crosses intact at the capacity of the listen mode in use, and the encoder
//! refuses a kind 2 one byte over it before it reaches the port.

use mch_protocol::beacon::{self, *};
use mercury_adapter::kiss;
use mercury_contract::{broadcast_across, contract, Tag, LAB_MODE};

/// A kind 2 beacon of exactly `len` bytes: the three known fields, then an
/// unknown TLV (which receivers skip) to fill the rest.
fn extended_of(len: usize) -> ExtendedBeacon {
    let b = Beacon::new(0x0a0b_0c0d, "N0CTA", Some("KG44dg"), CAP_CHAT).unwrap();
    let mut tlvs = vec![Tlv::Name("n".repeat(24)), Tlv::Qth("q".repeat(24)), Tlv::Status("s".repeat(40))];
    let so_far = ExtendedBeacon::new(b.clone(), tlvs.clone()).unwrap().encode().len();
    assert!(len >= so_far + 3, "{len} bytes is too small for this shape ({so_far} + a filler)");
    tlvs.push(Tlv::Unknown(9, vec![0x7e; len - so_far - 2]));
    ExtendedBeacon::new(b, tlvs).unwrap()
}

#[test]
fn capacity_tables_match() {
    for mode in 0..=20u8 {
        assert_eq!(beacon::listen_capacity(mode), kiss::wrapped_capacity(mode), "listen mode {mode}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn kind2_at_capacity_crosses() {
    contract("kind2_at_capacity_crosses", Tag::Both, |t| async move {
        let cap = listen_capacity(LAB_MODE).unwrap();
        let e = extended_of(cap);
        let bytes = e.encode_for(LAB_MODE).expect("at capacity it fits");
        assert_eq!(bytes.len(), cap);
        let got = broadcast_across(&t, &bytes).await;
        assert_eq!(got, bytes);
        assert_eq!(classify(&got), Received::Extended(e));
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn kind3_max_crosses() {
    contract("kind3_max_crosses", Tag::Both, |t| async move {
        let q = QsySuggestion::new(u32::MAX, "ABCDEFGHIJKLMNO", "N0CTB-15/ABCDEF", vec![(60_000_000, Mode::PktFm); 3])
            .unwrap();
        let bytes = q.encode_for(LAB_MODE).expect("58 bytes fit DATAC3");
        assert_eq!(bytes.len(), 58);
        let got = broadcast_across(&t, &bytes).await;
        assert_eq!(got, bytes);
        assert_eq!(classify(&got), Received::Qsy(q));
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn kind2_one_over_refused() {
    contract("kind2_one_over_refused", Tag::Both, |_t| async move {
        let cap = listen_capacity(LAB_MODE).unwrap();
        let over = extended_of(cap + 1);
        assert_eq!(over.encode().len(), cap + 1);
        assert_eq!(over.encode_for(LAB_MODE), Err(BeaconError::TooLong { len: cap + 1, cap }));
    })
    .await;
}
