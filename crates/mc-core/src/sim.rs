//! The simulator modem: the fake Mercury pair from `mercury-adapter` running
//! in-process on ephemeral loopback ports, with no audio and no RF. Our
//! station attaches on side A. A peer station `SIM1` listens on side B,
//! answers calls, runs MCH/1 sessions and echoes every chat it receives, so
//! the whole chat loop can be exercised without a radio. Each side has its own
//! data directory, so their locks never meet the app's own.

use crate::session::{SessionCommand, SessionEvent};
use crate::station::{Station, StationConfig, StationError};
use mch_protocol::ObjectId;
use mercury_adapter::fake::FakeNet;
use mercury_adapter::{Callsign, Endpoint};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::AbortHandle;

pub const PEER_CALLSIGN: &str = "SIM1";

pub struct SimModem {
    pub net: Arc<FakeNet>,
    pub peer_callsign: String,
    pub station: Station,
    peer: AbortHandle,
}

impl Drop for SimModem {
    fn drop(&mut self) {
        self.peer.abort(); // drops the peer station, its sockets and its lock
    }
}

impl SimModem {
    /// Start the pair: our station in `station_dir`, `SIM1` in `peer_dir`.
    pub async fn start(station_dir: &Path, peer_dir: &Path, callsign: &str) -> Result<SimModem, StationError> {
        let ours = Callsign::parse(callsign).map_err(|e| StationError::Wrong(e.to_string()))?;
        let net = FakeNet::start().await;
        let peer_call = Callsign::parse(PEER_CALLSIGN).expect("valid simulator callsign");
        let mut peer =
            Station::open(StationConfig::defaults(peer_call, Endpoint::base("127.0.0.1", net.b_base), peer_dir))?;
        peer.attach().await?;
        peer.set_listen(true).await?;
        let peer = tokio::spawn(serve_peer(peer, true, None)).abort_handle();
        let mut station =
            Station::open(StationConfig::defaults(ours, Endpoint::base("127.0.0.1", net.a_base), station_dir))?;
        station.attach().await?;
        Ok(SimModem { net, peer_callsign: PEER_CALLSIGN.into(), station, peer })
    }
}

/// A chat a peer station stored: (sender, id, text).
pub type Received = (String, ObjectId, String);

/// A headless MercuryChat peer (the simulator's SIM1, and `m0-client peer`
/// in the real-Mercury labs): answer every call and run the session. Each
/// new chat is reported on `received` once it has been stored (its receipt
/// is already on the way); with `echo`, it is answered with `echo: {text}`.
/// Returns when the modem's control connection is gone.
pub async fn serve_peer(mut peer: Station, echo: bool, received: Option<mpsc::UnboundedSender<Received>>) {
    loop {
        let info = match peer.wait_incoming(Duration::from_secs(3600)).await {
            Ok(info) => info,
            Err(StationError::Timeout(_)) => continue,
            Err(StationError::BadPeer(p)) => {
                tracing::warn!("refusing a link from an invalid callsign {p:?}");
                if peer.abort().await.is_err() {
                    return;
                }
                continue;
            }
            Err(_) => return,
        };
        let (cmd_tx, mut cmd_rx) = mpsc::channel(64);
        let (_req_tx, mut req_rx) = mpsc::channel(1);
        let (ev_tx, mut ev_rx) = mpsc::unbounded_channel();
        let from = info.peer.clone();
        let watch = async {
            while let Some(ev) = ev_rx.recv().await {
                if let SessionEvent::ChatReceived { id, text, duplicate: false } = ev {
                    if let Some(tx) = &received {
                        let _ = tx.send((from.clone(), id, text.clone()));
                    }
                    if echo {
                        let _ = cmd_tx
                            .send(SessionCommand::SendChat { id: ObjectId::random(), text: format!("echo: {text}") })
                            .await;
                    }
                }
            }
            std::future::pending::<()>().await
        };
        let end = tokio::select! {
            end = peer.run_session(&info, &mut cmd_rx, &mut req_rx, ev_tx) => end,
            _ = watch => continue,
        };
        if let Err(e) = peer.end_link_after(&end, Duration::from_secs(60)).await {
            tracing::warn!("could not end the link after {end:?}: {e}");
            return;
        }
    }
}
