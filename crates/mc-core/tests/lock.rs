//! MOD-02: one MercuryChat process per station data root and per modem endpoint.

use mc_core::lock::{lock_data_root, lock_endpoint, LockError};
use mc_core::{Station, StationConfig, StationError};
use mercury_adapter::{Bandwidth, Callsign, Endpoint, Timeouts};
use std::time::Duration;

#[test]
fn second_lock_on_same_root_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let _first = lock_data_root(dir.path()).unwrap();
    match lock_data_root(dir.path()) {
        Err(LockError::DataRootBusy { holder, .. }) => assert_eq!(holder, Some(std::process::id())),
        other => panic!("expected DataRootBusy, got {other:?}"),
    }
}

#[test]
fn lock_is_released_on_drop() {
    let dir = tempfile::tempdir().unwrap();
    drop(lock_data_root(dir.path()).unwrap());
    lock_data_root(dir.path()).unwrap();
}

#[test]
fn distinct_roots_lock_independently() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    let _la = lock_data_root(a.path()).unwrap();
    let _lb = lock_data_root(b.path()).unwrap();
}

#[test]
fn stale_lock_file_does_not_block() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("station.lock"), "99999 garbage").unwrap();
    lock_data_root(dir.path()).unwrap();
}

#[test]
fn busy_message_names_the_path_and_holder() {
    let dir = tempfile::tempdir().unwrap();
    let _first = lock_data_root(dir.path()).unwrap();
    let msg = lock_data_root(dir.path()).unwrap_err().to_string();
    assert!(msg.starts_with("another MercuryChat instance is using this station's data ("), "{msg}");
    assert!(msg.ends_with(&format!(" (pid {})", std::process::id())), "{msg}");
}

#[test]
fn endpoint_lock_conflicts_on_same_host_and_port() {
    let dir = tempfile::tempdir().unwrap();
    let _a = lock_endpoint(dir.path(), "127.0.0.1", 8300).unwrap();
    match lock_endpoint(dir.path(), "127.0.0.1", 8300) {
        Err(LockError::EndpointBusy { endpoint, .. }) => assert_eq!(endpoint, "127.0.0.1:8300"),
        other => panic!("expected EndpointBusy, got {other:?}"),
    }
    let _b = lock_endpoint(dir.path(), "127.0.0.1", 8400).unwrap();
    assert_eq!(
        lock_endpoint(dir.path(), "127.0.0.1", 8300).unwrap_err().to_string(),
        format!("another MercuryChat instance is using the modem at 127.0.0.1:8300 (pid {})", std::process::id())
    );
}

#[test]
fn endpoint_host_is_sanitised_into_the_file_name() {
    let dir = tempfile::tempdir().unwrap();
    let _l = lock_endpoint(dir.path(), "fe80::1%eth0", 8300).unwrap();
    assert!(dir.path().join("endpoint-fe80__1_eth0-8300.lock").exists());
}

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
        offer_timeout: std::time::Duration::from_secs(120),
        free_space: mc_core::station::FreeSpace::system(),
        hello_timeout: Duration::from_secs(5),
        modem_window: 2304,
        timeouts: Timeouts::default(),
        proto_minor: mch_protocol::MINOR,
        capabilities: mc_core::session::default_capabilities(),
    }
}

#[test]
fn station_open_takes_the_data_root_lock() {
    let dir = tempfile::tempdir().unwrap();
    let _first = Station::open(cfg(dir.path())).unwrap();
    match Station::open(cfg(dir.path())) {
        Err(StationError::Lock(LockError::DataRootBusy { .. })) => {}
        Err(e) => panic!("expected a lock error, got {e}"),
        Ok(_) => panic!("second Station::open on the same data root succeeded"),
    }
}
