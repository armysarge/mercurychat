//! M2 exit "restart does not blindly resend": opening a station turns every
//! message that may have reached the modem into `uncertain`. Messages never
//! handed to a session stay `queued`.

use mc_core::{Station, StationConfig};
use mc_storage::{OutState, Store};
use mercury_adapter::{Bandwidth, Callsign, Endpoint, Timeouts};
use std::time::Duration;

fn cfg(dir: &std::path::Path) -> StationConfig {
    StationConfig {
        callsign: Callsign::parse("ZS6ABC").unwrap(),
        endpoint: Endpoint::base("127.0.0.1", 8300),
        data_dir: dir.to_path_buf(),
        bandwidth: Bandwidth::Bw2300,
        max_file: 256 * 1024,
        chunk: 1024,
        auto_accept_files: false,
        receive_dir: None,
        tmp_dir: None,
        offer_timeout: Duration::from_secs(120),
        free_space: mc_core::station::FreeSpace::system(),
        hello_timeout: Duration::from_secs(5),
        timeouts: Timeouts::default(),
        proto_minor: mch_protocol::MINOR,
        capabilities: mc_core::session::default_capabilities(),
        busy_detect: true,
    }
}

fn id(n: u8) -> [u8; 16] {
    [n; 16]
}

fn seed(dir: &std::path::Path) {
    let mut s = Store::open(&dir.join("station.db")).unwrap();
    for (n, st) in
        [(1, OutState::Submitting), (2, OutState::AwaitingReceipt), (3, OutState::Queued), (4, OutState::Delivered)]
    {
        s.insert_outgoing("ZS6PEER", &id(n), "x").unwrap();
        s.set_out_state("ZS6PEER", &id(n), st, Some(1)).unwrap();
    }
}

fn states(st: &Station) -> Vec<Option<OutState>> {
    (1..=4).map(|n| st.store.out_state("ZS6PEER", &id(n)).unwrap()).collect()
}

#[test]
fn open_marks_in_flight_uncertain() {
    let dir = tempfile::tempdir().unwrap();
    seed(dir.path());
    let st = Station::open(cfg(dir.path())).unwrap();
    assert_eq!(
        states(&st),
        [Some(OutState::Uncertain), Some(OutState::Uncertain), Some(OutState::Queued), Some(OutState::Delivered)]
    );
}

#[test]
fn open_twice_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    seed(dir.path());
    drop(Station::open(cfg(dir.path())).unwrap());
    let st = Station::open(cfg(dir.path())).unwrap();
    assert_eq!(
        states(&st),
        [Some(OutState::Uncertain), Some(OutState::Uncertain), Some(OutState::Queued), Some(OutState::Delivered)]
    );
}
