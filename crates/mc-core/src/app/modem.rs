//! The modem task (M2): one per selected profile. It owns the `Station`, does
//! the attach and everything after it, and reports back to the service actor
//! by message, tagged with the generation it was started for. The actor never
//! awaits it, so a slow or hung attach cannot stall commands or snapshots, and
//! a report from a modem the actor has moved on from is simply dropped.
//!
//! A real Mercury (an Existing profile) is attached only while this task holds
//! its endpoint lock (MOD-02). An occupied endpoint is reported once, with the
//! lock's own text, and the task ends: no takeover loop.

use super::events::ModemState;
use crate::profile::{ModemTarget, StationProfile};
use crate::station::{Station, StationConfig, StationError};
use mch_protocol::ObjectId;
use mercury_adapter::{Callsign, Endpoint};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::sync::mpsc;

/// Where a modem task keeps its data and takes its locks.
#[derive(Debug, Clone, Default)]
pub struct ModemOptions {
    /// `{data root}/stations`: each profile's station data lives in `{id}/`.
    pub stations_root: Option<PathBuf>,
    /// The shared endpoint lock folder; `None` takes no endpoint lock (tests).
    pub locks_dir: Option<PathBuf>,
}

/// What the actor asks of the modem.
#[derive(Debug)]
pub enum ModemCmd {
    SetListen(bool),
    Call(Callsign),
    CancelCall,
    Disconnect,
    Abort,
    Submit { id: ObjectId },
    Shutdown,
}

/// What the modem tells the actor, for the generation it was started in.
#[derive(Debug)]
pub struct ModemReport {
    pub generation: u64,
    pub kind: ReportKind,
}

#[derive(Debug)]
pub enum ReportKind {
    State { state: ModemState, detail: Option<String> },
}

/// The actor's end of one modem task. Dropping it stops the task.
pub struct ModemHandle {
    pub generation: u64,
    cmds: mpsc::Sender<ModemCmd>,
}

impl ModemHandle {
    /// Queue a command; false if the task has ended or is backed up.
    #[allow(dead_code)] // first used by listening (M2 Task 4)
    pub fn send(&self, cmd: ModemCmd) -> bool {
        self.cmds.try_send(cmd).is_ok()
    }
}

impl Drop for ModemHandle {
    fn drop(&mut self) {
        let _ = self.cmds.try_send(ModemCmd::Shutdown);
    }
}

pub fn spawn(
    profile: StationProfile,
    generation: u64,
    opts: ModemOptions,
    reports: mpsc::Sender<ModemReport>,
) -> ModemHandle {
    let (tx, rx) = mpsc::channel(64);
    tokio::spawn(run(profile, opts, rx, Reporter { generation, tx: reports }));
    ModemHandle { generation, cmds: tx }
}

struct Reporter {
    generation: u64,
    tx: mpsc::Sender<ModemReport>,
}

impl Reporter {
    async fn state(&self, state: ModemState, detail: Option<String>) {
        let report = ModemReport { generation: self.generation, kind: ReportKind::State { state, detail } };
        let _ = self.tx.send(report).await;
    }

    async fn fault(&self, detail: String) {
        self.state(ModemState::Faulted, Some(detail)).await;
    }
}

/// Resolves when the actor stops this task (Shutdown, or the handle dropped).
/// Anything else that arrives before the modem is up is ignored.
async fn stopped(cmds: &mut mpsc::Receiver<ModemCmd>) {
    while let Some(cmd) = cmds.recv().await {
        if matches!(cmd, ModemCmd::Shutdown) {
            return;
        }
    }
}

/// Open the station store. A just-stopped task for the same profile may still
/// be releasing its data-root lock, so a busy lock is retried briefly; that is
/// our own predecessor finishing, not another instance.
async fn open_station(cfg: StationConfig) -> Result<Station, StationError> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        match Station::open(cfg.clone()) {
            Err(StationError::Lock(crate::lock::LockError::DataRootBusy { .. }))
                if tokio::time::Instant::now() < deadline =>
            {
                tokio::time::sleep(Duration::from_millis(50)).await
            }
            other => return other,
        }
    }
}

async fn run(profile: StationProfile, opts: ModemOptions, mut cmds: mpsc::Receiver<ModemCmd>, r: Reporter) {
    let Some(stations) = opts.stations_root.clone() else {
        return r.fault("no data folder for the station".into()).await;
    };
    let station_dir = stations.join(&profile.id);
    let call = match Callsign::parse(&profile.callsign) {
        Ok(c) => c,
        Err(e) => return r.fault(format!("callsign: {e}")).await,
    };
    match profile.modem.clone() {
        ModemTarget::Existing { host, control_port, data_port, .. } => {
            let _lock = match &opts.locks_dir {
                Some(dir) => match crate::lock::lock_endpoint(dir, &host, control_port) {
                    Ok(l) => Some(l),
                    Err(e) => return r.fault(e.to_string()).await,
                },
                None => None,
            };
            r.state(ModemState::Attaching, None).await;
            let endpoint = Endpoint { host: host.clone(), control_port, data_port };
            let mut station = match open_station(StationConfig::defaults(call, endpoint, &station_dir)).await {
                Ok(s) => s,
                Err(e) => return r.fault(format!("station data: {e}")).await,
            };
            tokio::select! {
                res = station.attach() => match res {
                    Ok(()) => r.state(ModemState::Ready, None).await,
                    Err(e) => return r.fault(format!("could not attach to Mercury at {host}:{control_port}: {e}")).await,
                },
                _ = stopped(&mut cmds) => return,
            }
            serve(station, cmds).await;
        }
        ModemTarget::Simulator => simulator(&station_dir, call, cmds, r).await,
    }
}

#[cfg(feature = "sim")]
async fn simulator(station_dir: &Path, call: Callsign, mut cmds: mpsc::Receiver<ModemCmd>, r: Reporter) {
    r.state(ModemState::Attaching, None).await;
    let peer_dir = station_dir.join("sim-peer");
    let start = async {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        loop {
            match crate::sim::SimModem::start(station_dir, &peer_dir, call.as_str()).await {
                Err(StationError::Lock(crate::lock::LockError::DataRootBusy { .. }))
                    if tokio::time::Instant::now() < deadline =>
                {
                    tokio::time::sleep(Duration::from_millis(50)).await
                }
                other => return other,
            }
        }
    };
    let sim = tokio::select! {
        res = start => match res {
            Ok(sim) => sim,
            Err(e) => return r.fault(format!("simulator: {e}")).await,
        },
        _ = stopped(&mut cmds) => return,
    };
    r.state(ModemState::Ready, Some("simulator".into())).await;
    serve_sim(sim, cmds).await;
}

#[cfg(not(feature = "sim"))]
async fn simulator(_dir: &Path, _call: Callsign, _cmds: mpsc::Receiver<ModemCmd>, r: Reporter) {
    r.fault("the simulator is not part of this build".into()).await;
}

/// The attached modem: holds the station (and its locks) until stopped.
/// Listening, calls and sessions are added by later M2 tasks.
async fn serve(station: Station, cmds: mpsc::Receiver<ModemCmd>) {
    stay(cmds).await;
    drop(station);
}

#[cfg(feature = "sim")]
async fn serve_sim(sim: crate::sim::SimModem, cmds: mpsc::Receiver<ModemCmd>) {
    stay(cmds).await;
    drop(sim);
}

async fn stay(mut cmds: mpsc::Receiver<ModemCmd>) {
    stopped(&mut cmds).await;
}
