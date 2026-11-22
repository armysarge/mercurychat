//! The simulator modem: the fake Mercury pair from `mercury-adapter` running
//! in-process on ephemeral loopback ports, with no audio and no RF. Our
//! station attaches on side A. A peer station `SIM1` listens on side B,
//! answers calls, runs MCH/1 sessions and echoes every chat it receives, so
//! the whole chat loop can be exercised without a radio. Each side has its own
//! data directory, so their locks never meet the app's own.

use crate::session::{PeerMsg, SessionCommand, SessionEvent};
use crate::station::{Station, StationConfig, StationError};
use mch_protocol::beacon::{Beacon, CAP_CHAT};
use mch_protocol::payload::{InfoResponse, InfoWhat, QsyAnswer, SnrReport, StationShare};
use mch_protocol::ObjectId;
use mercury_adapter::fake::FakeNet;
use mercury_adapter::{Callsign, Endpoint};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::AbortHandle;

pub const PEER_CALLSIGN: &str = "SIM1";
/// The locator SIM1's beacons carry (a demo square, not anyone's station).
pub const PEER_GRID: &str = "JJ00aa";
/// How long after attach SIM1 first calls CQ and beacons, and how often after.
pub const PEER_FIRST: Duration = Duration::from_secs(3);
pub const PEER_EVERY: Duration = Duration::from_secs(60);
/// The gap between SIM1's CQ and its beacon, so each is heard on its own.
const PEER_GAP: Duration = Duration::from_secs(2);

/// What SIM1 is like. The app reads it from the environment
/// (`SimPeer::from_env`); tests set it directly.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SimPeer {
    /// `MC_SIM_PEER_MINOR0=1`: an older MercuryChat (MCH/1 minor 0, no `file/2`).
    pub minor0: bool,
    /// `MC_SIM_PEER_OFFERS=1`: SIM1 offers `notes.txt` 5 s after a session
    /// starts and, once that ends, a 640-px photo.
    pub offers: bool,
}

impl SimPeer {
    pub fn from_env() -> SimPeer {
        let on = |k: &str| std::env::var_os(k).is_some_and(|v| v == "1");
        SimPeer { minor0: on("MC_SIM_PEER_MINOR0"), offers: on("MC_SIM_PEER_OFFERS") }
    }
}

pub struct SimModem {
    pub net: Arc<FakeNet>,
    pub peer_callsign: String,
    pub station: Station,
    peer: AbortHandle,
    on_air: Option<AbortHandle>,
}

impl Drop for SimModem {
    fn drop(&mut self) {
        self.peer.abort(); // drops the peer station, its sockets and its lock
        if let Some(t) = self.on_air.take() {
            t.abort();
        }
    }
}

impl SimModem {
    /// Start the pair: our station in `station_dir`, `SIM1` in `peer_dir`,
    /// SIM1 as the environment says.
    pub async fn start(station_dir: &Path, peer_dir: &Path, callsign: &str) -> Result<SimModem, StationError> {
        Self::start_with(station_dir, peer_dir, callsign, SimPeer::from_env()).await
    }

    pub async fn start_with(
        station_dir: &Path,
        peer_dir: &Path,
        callsign: &str,
        sim_peer: SimPeer,
    ) -> Result<SimModem, StationError> {
        let ours = Callsign::parse(callsign).map_err(|e| StationError::Wrong(e.to_string()))?;
        let net = FakeNet::start().await;
        let peer_call = Callsign::parse(PEER_CALLSIGN).expect("valid simulator callsign");
        let mut cfg = StationConfig::defaults(peer_call, Endpoint::base("127.0.0.1", net.b_base), peer_dir);
        // An older MercuryChat: MCH/1 minor 0, chat and the never-wired `file`.
        if sim_peer.minor0 {
            cfg.proto_minor = 0;
            cfg.capabilities = vec!["chat".into(), "file".into()];
        }
        let mut peer = Station::open(cfg)?;
        peer.attach().await?;
        peer.set_listen(true).await?;
        let peer = tokio::spawn(serve_peer_with(peer, true, None, sim_peer.offers)).abort_handle();
        let mut station =
            Station::open(StationConfig::defaults(ours, Endpoint::base("127.0.0.1", net.a_base), station_dir))?;
        station.attach().await?;
        Ok(SimModem { net, peer_callsign: PEER_CALLSIGN.into(), station, peer, on_air: None })
    }

    /// Our side's broadcast (KISS) port on the fake modem.
    pub fn broadcast_port(&self) -> u16 {
        self.net.a_bcast
    }

    /// Make SIM1 call CQ and send an MCB/1 beacon with its locator,
    /// [`PEER_FIRST`] after now and then every [`PEER_EVERY`], so the heard
    /// list has stations without a radio (M3a Task 10). As on the air, none
    /// goes out while a session is up; a skipped one is not made up later.
    pub fn start_peer_broadcasts(&mut self) {
        if self.on_air.is_some() {
            return;
        }
        let net = self.net.clone();
        let task = tokio::spawn(async move {
            let mut seq = rand::random::<u32>();
            let mut next = tokio::time::Instant::now() + PEER_FIRST;
            loop {
                tokio::time::sleep_until(next).await;
                net.air_cq(1, PEER_CALLSIGN, "2300");
                tokio::time::sleep(PEER_GAP).await;
                if let Ok(b) = Beacon::new(seq, PEER_CALLSIGN, Some(PEER_GRID), CAP_CHAT) {
                    if net.air_broadcast(1, b.encode()) {
                        seq = seq.wrapping_add(1);
                    }
                }
                next += PEER_EVERY;
            }
        });
        self.on_air = Some(task.abort_handle());
    }
}

/// SIM1's answer to a minor-1 message (M3c): it accepts the first QSY
/// candidate, and answers information requests from made-up values.
fn peer_reply(m: &PeerMsg) -> Option<PeerMsg> {
    match m {
        PeerMsg::QsyPropose(id, _) => Some(PeerMsg::QsyAnswer(*id, QsyAnswer::Accept { index: 0 })),
        PeerMsg::InfoRequest(id, req) => {
            let answer = match req.what {
                InfoWhat::Snr => InfoResponse {
                    snr: Some(SnrReport { db: 6.0, age_s: 1, source: "telemetry".into() }),
                    station: None,
                    reason: None,
                },
                InfoWhat::Station => InfoResponse {
                    snr: None,
                    station: Some(StationShare {
                        name: Some("Sim".into()),
                        qth: Some("Simulator".into()),
                        locator: Some(PEER_GRID.into()),
                        ..StationShare::default()
                    }),
                    reason: None,
                },
            };
            Some(PeerMsg::InfoResponse(*id, Box::new(answer)))
        }
        _ => None,
    }
}

/// SIM1 answers a file offer after this, like an operator would.
pub const PEER_ACCEPT_AFTER: Duration = Duration::from_secs(2);
/// With offers on, SIM1 offers its first file this long into a session.
pub const PEER_OFFER_AFTER: Duration = Duration::from_secs(5);

/// The text SIM1 offers: about 2 KB of made-up station notes.
fn sim_notes() -> Vec<u8> {
    let mut out = String::from("SIM1 station notes (simulator, made up)\n\n");
    let lines = [
        "Antenna: a wire dipole at 10 m, fed with ladder line.",
        "Rig: 100 W HF transceiver, USB data, ALC kept at zero.",
        "Mercury: BW2300 by day, BW500 when the band is poor.",
        "Schedule: 40 m in the evening, 20 m at midday.",
        "Notes: thanks for the QSO, 73 and good DX.",
    ];
    let mut i = 0;
    while out.len() < 2000 {
        out.push_str(&format!("{:02}. {}\n", i + 1, lines[i % lines.len()]));
        i += 1;
    }
    out.into_bytes()
}

/// The photo SIM1 offers: a 640 x 480 gradient made here, as JPEG (no
/// camera, no metadata, nobody's picture).
fn sim_photo() -> Vec<u8> {
    let img = image::RgbImage::from_fn(640, 480, |x, y| {
        image::Rgb([(x * 255 / 639) as u8, (y * 255 / 479) as u8, (((x + y) * 255) / 1118) as u8])
    });
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 80)
        .encode_image(&img)
        .expect("a JPEG encodes in memory");
    out
}

/// Write `bytes` as `name` under SIM1's own folder and prepare it to send.
fn sim_prepare(peer: &Station, name: &str, bytes: &[u8]) -> Option<crate::files::Prepared> {
    let dir = peer.cfg.data_dir.join("sim-src");
    std::fs::create_dir_all(&dir).ok()?;
    let src = dir.join(name);
    std::fs::write(&src, bytes).ok()?;
    let p = crate::files::prepare(&src, crate::files::PhotoSize::Original, &peer.tmp_dir(), 1 << 20);
    p.map_err(|e| tracing::warn!("SIM1 could not prepare {name}: {e:?}")).ok()
}

/// A chat a peer station stored: (sender, id, text).
pub type Received = (String, ObjectId, String);

/// A headless MercuryChat peer (the simulator's SIM1, and `m0-client peer`
/// in the real-Mercury labs): answer every call and run the session. Each
/// new chat is reported on `received` once it has been stored (its receipt
/// is already on the way); with `echo`, it is answered with `echo: {text}`.
/// Returns when the modem's control connection is gone.
pub async fn serve_peer(peer: Station, echo: bool, received: Option<mpsc::UnboundedSender<Received>>) {
    serve_peer_with(peer, echo, received, false).await
}

/// As `serve_peer`; every file offered is accepted after
/// `PEER_ACCEPT_AFTER`, and with `offers` SIM1 offers its own two files
/// in each session (M4).
pub async fn serve_peer_with(
    mut peer: Station,
    echo: bool,
    received: Option<mpsc::UnboundedSender<Received>>,
    offers: bool,
) {
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
        let notes = offers.then(|| sim_prepare(&peer, "notes.txt", &sim_notes())).flatten();
        let photo = offers.then(|| sim_prepare(&peer, "photo.jpg", &sim_photo())).flatten();
        let watch = async {
            // SIM1's own offers: the notes first, the photo once they end.
            let mut notes = notes;
            let mut photo = photo;
            let mut offered: Option<ObjectId> = None;
            while let Some(ev) = ev_rx.recv().await {
                match &ev {
                    SessionEvent::Negotiated { caps, .. } if caps.iter().any(|c| c == mch_protocol::CAP_FILE) => {
                        if let Some(prepared) = notes.take() {
                            let id = ObjectId::random();
                            offered = Some(id);
                            let tx = cmd_tx.clone();
                            tokio::spawn(async move {
                                tokio::time::sleep(PEER_OFFER_AFTER).await;
                                let _ = tx.send(SessionCommand::SendFile { id, prepared }).await;
                            });
                        }
                    }
                    SessionEvent::FileOffered { id, .. } => {
                        let (tx, id) = (cmd_tx.clone(), *id);
                        tokio::spawn(async move {
                            tokio::time::sleep(PEER_ACCEPT_AFTER).await;
                            let _ = tx.send(SessionCommand::AcceptFile { id }).await;
                        });
                    }
                    SessionEvent::FileDelivered { id }
                    | SessionEvent::FileRejected { id, .. }
                    | SessionEvent::FileFailed { id, .. }
                    | SessionEvent::FileCancelled { id, .. }
                        if offered == Some(*id) =>
                    {
                        offered = None;
                        if let Some(prepared) = photo.take() {
                            let id = ObjectId::random();
                            offered = Some(id);
                            let _ = cmd_tx.send(SessionCommand::SendFile { id, prepared }).await;
                        }
                    }
                    _ => {}
                }
                if let SessionEvent::Peer(m) = &ev {
                    if let Some(reply) = peer_reply(m) {
                        let _ = cmd_tx.send(SessionCommand::Peer(reply)).await;
                    }
                    continue;
                }
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
