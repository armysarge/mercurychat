//! The simulator modem (M1): the fake Mercury pair from `mercury-adapter`
//! running in-process on ephemeral loopback ports, with no audio and no RF.
//! Our station attaches on side A; a peer station `SIM1` listens on side B so
//! M2 can call it. Each side has its own data root under the app's, so their
//! locks never meet the app's own.

use crate::station::{Station, StationConfig, StationError};
use mercury_adapter::fake::FakeNet;
use mercury_adapter::{Bandwidth, Callsign, Endpoint, Timeouts};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

pub const PEER_CALLSIGN: &str = "SIM1";

pub struct SimModem {
    pub net: Arc<FakeNet>,
    pub peer_callsign: String,
    pub station: Station,
    _peer: Station,
}

fn config(call: Callsign, base: u16, dir: &Path) -> StationConfig {
    StationConfig {
        callsign: call,
        endpoint: Endpoint::base("127.0.0.1", base),
        data_dir: dir.to_path_buf(),
        bandwidth: Bandwidth::Bw2300,
        max_file: 256 * 1024,
        chunk: 1024,
        auto_accept_files: false,
        hello_timeout: Duration::from_secs(5),
        modem_window: 2 * 1024 + 256,
        timeouts: Timeouts::default(),
    }
}

impl SimModem {
    pub async fn start(data_root: &Path, callsign: &str) -> Result<SimModem, StationError> {
        let ours = Callsign::parse(callsign).map_err(|e| StationError::Wrong(e.to_string()))?;
        let net = FakeNet::start().await;
        let peer_call = Callsign::parse(PEER_CALLSIGN).expect("valid simulator callsign");
        let mut peer = Station::open(config(peer_call, net.b_base, &data_root.join("sim-peer")))?;
        peer.attach().await?;
        peer.set_listen(true).await?;
        let mut station = Station::open(config(ours, net.a_base, &data_root.join("sim-station")))?;
        station.attach().await?;
        Ok(SimModem { net, peer_callsign: PEER_CALLSIGN.into(), station, _peer: peer })
    }
}
