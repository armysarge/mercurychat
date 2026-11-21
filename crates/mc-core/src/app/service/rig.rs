//! The actor's side of rig control (M3b design §7.1): it owns the rigctld
//! client task, checks every change against the gate at the moment it is
//! forwarded, holds a call while a frequency change is in flight, feeds the
//! policy what the client and Mercury report, and carries out its actions.

use super::{conflict, storage, App};
use crate::app::contract::{AppError, CommandOk};
use crate::app::discovery::Reading;
use crate::app::events::{AppEvent, ModemState};
use crate::app::modem::{ModemCmd, TuneResult};
use crate::app::rig::{
    rig_gate, Activity, OwnerCheck, RigAction, RigBanner, RigChange, RigPolicy, RigRefusal, RigTestStep, TuneView,
    TxLevelResult, RIG_SOURCE,
};
use crate::app::rigconfig::{
    rig_key, rigctld_spec_for, BandLevels, FreqControl, HamlibModelView, RigConfig, TUNE_RANGE,
};
use crate::app::telemetry::Tuning;
use crate::band::{band_for, BANDS};
use mc_storage::now_utc;
use mercury_adapter::flrig::FlrigClient;
use mercury_adapter::rigctl::{
    find_rigctld, parse_model_list, probe, DaemonState, Level, PollRate, RigClient, RigClientCmd, RigLink, RigMsg,
    RigReply, RigRequest, RigctldDaemon, LOG_LINES,
};
use mercury_adapter::telemetry::{RadioList, TelemetryCmd, TelemetryMsg};
use mercury_adapter::{Callsign, TuneArg};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::AbortHandle;
use tokio::time::Instant;

/// A rig reading this fresh is the heard list's and the logbook's frequency.
pub const RIG_FRESH: Duration = Duration::from_secs(2);
/// The longest a call waits for a frequency change already sent to the rig.
pub const CALL_HOLD: Duration = Duration::from_secs(1);
/// Rig events at most this often; the snapshot always has the latest.
pub const RIG_EVENT_GAP: Duration = Duration::from_millis(500);
/// The last frequency is stored at most this often.
pub const LAST_SAVE_GAP: Duration = Duration::from_secs(5);
/// How long Mercury has to confirm a new TX level in its status (RIG-04).
pub const TX_LEVEL_CONFIRM: Duration = Duration::from_secs(5);
/// Two levels this close are the same.
pub const TX_LEVEL_TOLERANCE: f32 = 0.01;
/// How long each step of the Settings link test may take.
pub const RIG_TEST_TIMEOUT: Duration = Duration::from_secs(3);

/// MercuryChat stops Tune after this (RIG-05); Mercury's own stop is 60 s.
pub const TUNE_LIMIT: Duration = Duration::from_secs(30);

/// Highest frequency a rig change may ask for (6 m tops out at 54 MHz).
pub const MAX_FREQ_HZ: u64 = 60_000_000;

/// Where MercuryChat's own rigctld listens (M3b.1 §5.3).
pub const MANAGED_LISTEN: &str = "127.0.0.1:4532";
/// How long `rigctld -l` may take.
pub const MODEL_LIST_TIMEOUT: Duration = Duration::from_secs(10);

/// The frequency shown for the rig's dial reading: dial + offset (M3b.1 §5.1).
pub fn to_display(dial: u64, offset_hz: i32) -> u64 {
    (dial as i64).saturating_add(i64::from(offset_hz)).max(0) as u64
}

/// The dial frequency for a frequency shown: shown - offset; refused below 1 Hz.
pub fn to_dial(shown: u64, offset_hz: i32) -> Result<u64, AppError> {
    let dial = (shown as i64).saturating_sub(i64::from(offset_hz));
    if dial < 1 {
        return Err(invalid("frequency", "With the frequency offset, that is below 0 Hz on the radio's dial"));
    }
    Ok(dial as u64)
}

struct AbortOnDrop(AbortHandle);
impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// A call held until the frequency change before it reaches the rig.
pub(super) struct HeldCall {
    peer: Callsign,
    before: ModemState,
    until: Instant,
}

/// The last tuning, restored on startup when the operator opts in (RIG-03).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LastTuning {
    freq_hz: u64,
    mode: Option<String>,
    passband_hz: Option<i32>,
}

/// Mercury's tuning carrier, as the actor tracks it.
#[derive(Default)]
pub(super) struct TuneState {
    supported: Option<bool>,
    pub(super) active: bool,
    level: Option<f32>,
    /// When MercuryChat stops it, and that wall-clock time for the UI.
    until: Option<(Instant, String)>,
    off_sent: bool,
    shown: Option<TuneView>,
}

impl TuneState {
    pub(super) fn view(&self) -> TuneView {
        TuneView {
            supported: self.supported,
            active: self.active,
            ends_utc: self.until.as_ref().map(|u| u.1.clone()),
            level_dbfs: self.level.unwrap_or(TuneView::default().level_dbfs),
        }
    }

    fn stopped(&mut self) {
        self.active = false;
        self.until = None;
        self.off_sent = false;
    }
}

pub(super) struct RigSide {
    pub(super) policy: RigPolicy,
    profile_id: Option<String>,
    tx: Option<mpsc::Sender<RigClientCmd>>,
    task: Option<AbortOnDrop>,
    /// A new client's message stream, for the actor loop to take.
    pub(super) inbox: Option<mpsc::Receiver<RigMsg>>,
    pub(super) inbox_new: bool,
    rate: Option<PollRate>,
    seq: u64,
    /// Changes sent and not yet answered; a call waits for all of them.
    in_flight: Vec<(u64, RigChange)>,
    held_call: Option<HeldCall>,
    dirty: bool,
    emitted_at: Option<Instant>,
    restore_pending: bool,
    last_saved: Option<LastTuning>,
    last_saved_at: Option<Instant>,
    /// The rig settings the link runs on.
    cfg: RigConfig,
    /// Commands for the modem's telemetry client (set_tx_gain), if it has one.
    tele_tx: Option<mpsc::Sender<TelemetryCmd>>,
    /// Mercury's last `radio_list`, for the two-owner check.
    radio_list: Option<RadioList>,
    /// The band the rig was last on, and a TX level waiting to be applied.
    band: Option<String>,
    tx_pending: Option<(String, f32)>,
    /// A TX level sent, awaiting Mercury's status: (band, dB, deadline).
    tx_wait: Option<(String, f32, Instant)>,
    pub(super) tune: TuneState,
    tune_limit: Duration,
    /// The simulator's fake rigctld, while the simulator runs.
    sim: Option<std::net::SocketAddr>,
    /// MercuryChat's own rigctld, and the task forwarding what it does.
    daemon: Option<RigctldDaemon>,
    daemon_bridge: Option<AbortOnDrop>,
    /// The rigctld program (tests); `None`: look for it.
    rigctld_program: Option<Option<PathBuf>>,
    managed_listen: SocketAddr,
    /// MercuryChat's own rig link errors, for the CAT log.
    cat_log: VecDeque<String>,
}

impl RigSide {
    pub(super) fn with_options(
        limit: Option<Duration>,
        rigctld_program: Option<Option<PathBuf>>,
        managed_listen: Option<SocketAddr>,
    ) -> RigSide {
        RigSide {
            tune_limit: limit.unwrap_or(TUNE_LIMIT),
            rigctld_program,
            managed_listen: managed_listen.unwrap_or_else(|| MANAGED_LISTEN.parse().unwrap()),
            ..RigSide::default()
        }
    }

    fn cat_note(&mut self, line: String) {
        if self.cat_log.len() == LOG_LINES {
            self.cat_log.pop_front();
        }
        self.cat_log.push_back(format!("{} {line}", now_utc()));
    }

    /// The rigctld to start: the test's, else on the PATH or in `hamlib_dir`.
    fn rigctld_program(&self, hamlib_dir: &str) -> Option<PathBuf> {
        match &self.rigctld_program {
            Some(p) => p.clone(),
            None => {
                let dir = hamlib_dir.trim();
                find_rigctld((!dir.is_empty()).then(|| std::path::Path::new(dir)))
            }
        }
    }
}

impl Default for RigSide {
    fn default() -> Self {
        RigSide {
            policy: RigPolicy::new(false),
            profile_id: None,
            tx: None,
            task: None,
            inbox: None,
            inbox_new: false,
            rate: None,
            seq: 0,
            in_flight: Vec::new(),
            held_call: None,
            dirty: false,
            emitted_at: None,
            restore_pending: false,
            last_saved: None,
            last_saved_at: None,
            cfg: RigConfig::default(),
            tele_tx: None,
            radio_list: None,
            band: None,
            tx_pending: None,
            tx_wait: None,
            tune: TuneState { shown: Some(TuneView::default()), ..TuneState::default() },
            tune_limit: TUNE_LIMIT,
            sim: None,
            daemon: None,
            daemon_bridge: None,
            rigctld_program: None,
            managed_listen: MANAGED_LISTEN.parse().unwrap(),
            cat_log: VecDeque::new(),
        }
    }
}

/// Does Mercury key through the rigctld MercuryChat reads (RIG-01; design §2.11)?
fn owner_check(cfg: &RigConfig, rl: Option<&RadioList>, managed: SocketAddr) -> OwnerCheck {
    let Some(rl) = rl else { return OwnerCheck::Unknown };
    let ours = match cfg.freq_control {
        FreqControl::ExternalRigctld => format!("{}:{}", cfg.external.host, cfg.external.port),
        FreqControl::ManagedRigctld => managed.to_string(),
        FreqControl::Flrig | FreqControl::None => return OwnerCheck::Ok,
    };
    if rl.ptt_method != "hamlib" {
        return OwnerCheck::Ok;
    }
    let norm = |a: &str| a.trim().replace("localhost", "127.0.0.1");
    match rl.selected_model {
        Some(2) if norm(&rl.device_path) == norm(&ours) => OwnerCheck::Ok,
        Some(2) => OwnerCheck::Warn {
            reason: format!(
                "Mercury keys through the rigctld at {}, MercuryChat reads the one at {ours}: the PTT check only works if both reach the same rigctld",
                rl.device_path
            ),
        },
        Some(model) => OwnerCheck::Refused {
            reason: format!(
                "Mercury keys the radio through its own CAT connection (Hamlib model {model} on {}), so the CAT port would have two owners: set the PTT in Mercury to Hamlib model 2 (rigctld) at {ours}",
                rl.device_path
            ),
        },
        None => OwnerCheck::Warn { reason: "Mercury keys through Hamlib, but which rig it uses could not be read".into() },
    }
}

fn refused(r: crate::app::rig::RigRefusal) -> AppError {
    AppError::Refused { reason: r.key(), message: r.message().to_string() }
}

fn invalid(field: &str, message: &str) -> AppError {
    AppError::Validation { errors: crate::app::chat::field(field, message) }
}

fn request_for(change: &RigChange) -> Option<RigRequest> {
    Some(match change {
        RigChange::Frequency { hz } => RigRequest::SetFreq(*hz),
        RigChange::Mode { mode, passband_hz } => RigRequest::SetMode { mode: mode.clone(), passband_hz: *passband_hz },
        RigChange::Vfo { vfo } => RigRequest::SetVfo(vfo.clone()),
        RigChange::Split { on, tx_vfo } => RigRequest::SetSplit { on: *on, tx_vfo: tx_vfo.clone() },
        RigChange::RfPower { level } => RigRequest::SetLevel(Level::RfPower, *level),
        RigChange::TxLevel | RigChange::Tune => return None,
    })
}

fn token_ok(s: &str) -> bool {
    !s.is_empty() && s.len() <= 16 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Reject a change that is malformed whatever the rig's state.
fn validate(change: &RigChange) -> Result<(), AppError> {
    match change {
        RigChange::Frequency { hz } if *hz == 0 || *hz > MAX_FREQ_HZ => {
            Err(invalid("frequency", "Enter a frequency between 1 Hz and 60 MHz"))
        }
        RigChange::Mode { mode, passband_hz } if !token_ok(mode) || !(0..=20_000).contains(passband_hz) => {
            Err(invalid("mode", "Choose a mode and a passband of 0 to 20000 Hz"))
        }
        RigChange::Vfo { vfo } if !token_ok(vfo) => Err(invalid("vfo", "Choose a VFO")),
        RigChange::Split { tx_vfo, .. } if !token_ok(tx_vfo) => Err(invalid("vfo", "Choose a VFO")),
        RigChange::RfPower { level } if !level.is_finite() || !(0.0..=1.0).contains(level) => {
            Err(invalid("power", "RF power is 0 to 100 %"))
        }
        RigChange::TxLevel | RigChange::Tune => Err(invalid("change", "use the TX level or Tune controls")),
        _ => Ok(()),
    }
}

impl App {
    /// What the station is doing, for the rig gate.
    pub(super) fn activity(&self) -> Activity {
        match self.modem {
            ModemState::Calling | ModemState::Negotiating => Activity::Calling,
            ModemState::Connected => Activity::Connected,
            ModemState::Draining | ModemState::Recovering => Activity::Draining,
            _ if self.rig.held_call.is_some() => Activity::Calling,
            _ if self.rig.tune.active => Activity::Tuning,
            _ if self.broadcasts.deadline().is_some() => Activity::Broadcasting,
            _ => Activity::Idle,
        }
    }

    /// The modem is attached (Tune and TX level go through it).
    pub(super) fn attached(&self) -> bool {
        matches!(
            self.modem,
            ModemState::Ready
                | ModemState::Listening
                | ModemState::Calling
                | ModemState::Negotiating
                | ModemState::Connected
                | ModemState::Draining
        )
    }

    /// Start (or stop) the rig link for a profile's settings. `None`: no profile.
    pub(super) fn start_rig(&mut self, profile_id: Option<&str>) {
        let cfg = match (self.rig.sim, profile_id) {
            // The simulator's own rig, whatever the profile stores.
            (Some(addr), Some(_)) => RigConfig::external(&addr.ip().to_string(), addr.port()),
            _ => profile_id.map(|id| RigConfig::load(&self.store, id)).unwrap_or_default(),
        };
        let r = &mut self.rig;
        r.task = None;
        r.tx = None;
        r.rate = None;
        r.in_flight.clear();
        // Stops MercuryChat's own rigctld, if it started one.
        r.daemon_bridge = None;
        r.daemon = None;
        let same_station = profile_id.is_some() && r.profile_id.as_deref() == profile_id;
        r.profile_id = profile_id.map(String::from);
        let configured = profile_id.is_some() && cfg.freq_control != FreqControl::None;
        // No rig before and none now: nothing to tell the UI.
        let was_configured = r.policy.configured();
        let old = std::mem::replace(&mut r.policy, RigPolicy::new(configured));
        if same_station && configured && was_configured {
            r.policy.carry_safety(&old);
        }
        r.policy.set_activity(Activity::Idle);
        let check = owner_check(&cfg, r.radio_list.as_ref(), r.managed_listen);
        let refused = matches!(check, OwnerCheck::Refused { .. });
        r.policy.set_owner_check(check);
        r.cfg = cfg.clone();
        r.restore_pending = configured && cfg.restore_on_startup;
        r.inbox_new = true;
        r.inbox = None;
        r.dirty = configured || was_configured;
        if configured && refused {
            // Two owners of the CAT port: no rig link until Mercury is changed.
            let reason = "not connected: Mercury owns the CAT port".to_string();
            r.policy.on_msg(&RigMsg::Link(RigLink::Lost { reason }), Instant::now());
        } else if configured {
            let (cmd_tx, cmd_rx) = mpsc::channel(32);
            let (out_tx, out_rx) = mpsc::channel(256);
            let read_every = Duration::from_secs(u64::from(cfg.read_every_s.max(1)));
            let task = match cfg.freq_control {
                FreqControl::None => None,
                FreqControl::ExternalRigctld => Some(RigClient::spawn_every(
                    format!("{}:{}", cfg.external.host, cfg.external.port),
                    read_every,
                    cmd_rx,
                    out_tx,
                )),
                FreqControl::Flrig => Some(FlrigClient::spawn(
                    format!("{}:{}", cfg.flrig.host, cfg.flrig.port),
                    read_every,
                    cmd_rx,
                    out_tx,
                )),
                FreqControl::ManagedRigctld => {
                    let listen = r.managed_listen;
                    match (r.rigctld_program(&cfg.hamlib_dir), rigctld_spec_for(&cfg, listen)) {
                        (Some(exe), Some(spec)) => {
                            let (dtx, mut drx) = mpsc::channel(8);
                            r.daemon = Some(RigctldDaemon::start(exe, spec, dtx));
                            let fwd = out_tx.clone();
                            let bridge = tokio::spawn(async move {
                                while let Some(state) = drx.recv().await {
                                    if fwd.send(RigMsg::Daemon(state)).await.is_err() {
                                        break;
                                    }
                                }
                            });
                            r.daemon_bridge = Some(AbortOnDrop(bridge.abort_handle()));
                            r.policy.set_daemon(Some("starting".into()));
                            Some(RigClient::spawn_every(listen.to_string(), read_every, cmd_rx, out_tx))
                        }
                        (None, _) => {
                            r.policy.set_daemon(Some("notFound".into()));
                            let reason = "rigctld not found: install Hamlib or choose its folder".to_string();
                            r.cat_note(reason.clone());
                            r.policy.on_msg(&RigMsg::Link(RigLink::Lost { reason }), Instant::now());
                            None
                        }
                        (_, None) => {
                            let reason = "no rig model chosen".to_string();
                            r.policy.set_daemon(Some(format!("failed: {reason}")));
                            r.policy.on_msg(&RigMsg::Link(RigLink::Lost { reason }), Instant::now());
                            None
                        }
                    }
                }
            };
            if let Some(task) = task {
                r.task = Some(AbortOnDrop(task));
                r.tx = Some(cmd_tx);
                r.inbox = Some(out_rx);
            }
        }
    }

    /// Hamlib's rig models (`rigctld -l`), from its own task.
    pub(super) fn hamlib_models(&mut self, hamlib_dir: String, reply: super::Reply<CommandOk>) {
        let Some(exe) = self.rig.rigctld_program(&hamlib_dir) else {
            let _ = reply.send(Ok(CommandOk::HamlibModels { models: vec![], found: false }));
            return;
        };
        tokio::spawn(async move {
            let mut cmd = tokio::process::Command::new(&exe);
            cmd.arg("-l").stdin(std::process::Stdio::null()).kill_on_drop(true);
            #[cfg(windows)]
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
            let result = match tokio::time::timeout(MODEL_LIST_TIMEOUT, cmd.output()).await {
                Ok(Ok(out)) => {
                    let models = parse_model_list(&String::from_utf8_lossy(&out.stdout))
                        .into_iter()
                        .map(|m| HamlibModelView { id: m.id, mfg: m.mfg, model: m.model, status: m.status })
                        .collect();
                    Ok(CommandOk::HamlibModels { models, found: true })
                }
                Ok(Err(e)) => Err(conflict(format!("{}: {e}", exe.display()))),
                Err(_) => Err(conflict("rigctld -l did not finish within 10 s".into())),
            };
            let _ = reply.send(result);
        });
    }

    /// The CAT log: rigctld's stderr, then MercuryChat's own rig link errors.
    pub(super) fn cat_log(&self) -> Result<CommandOk, AppError> {
        let mut lines = self.rig.daemon.as_ref().map(|d| d.log()).unwrap_or_default();
        lines.extend(self.rig.cat_log.iter().cloned());
        Ok(CommandOk::CatLog { lines })
    }

    /// What MercuryChat's own rigctld is doing.
    fn on_daemon(&mut self, state: DaemonState) {
        let shown = match &state {
            DaemonState::Reused => "reused".to_string(),
            DaemonState::Starting => "starting".to_string(),
            DaemonState::Running => "running".to_string(),
            DaemonState::Restarting => "restarting".to_string(),
            DaemonState::Failed { reason } => format!("failed: {reason}"),
        };
        match &state {
            DaemonState::Reused => {
                let line = self.discovery.note("rig", "using the rigctld already running on this computer");
                self.emit(line);
            }
            DaemonState::Restarting | DaemonState::Failed { .. } => {
                self.rig.cat_note(format!("rigctld {shown}"));
                let line = self.discovery.note("rig", format!("MercuryChat's rigctld: {shown}"));
                self.emit(line);
            }
            _ => {}
        }
        self.rig.policy.set_daemon(Some(shown));
        self.rig.dirty = true;
    }

    /// The Settings link test: a separate connection that only reads, run in
    /// its own task so the actor never waits on the network; plus whether
    /// Mercury keys through the same rigctld (from its last radio_list).
    pub(super) fn test_rig(&mut self, config: RigConfig, reply: super::Reply<CommandOk>) {
        let host = config.external.host.trim().to_string();
        if host.is_empty() || host.chars().any(|c| c.is_whitespace() || c.is_control()) || config.external.port == 0 {
            let _ = reply.send(Err(invalid("host", "Enter the rigctld host and port, e.g. 127.0.0.1 and 4532")));
            return;
        }
        let cfg = RigConfig::external(&host, config.external.port);
        let owner = match owner_check(&cfg, self.rig.radio_list.as_ref(), self.rig.managed_listen) {
            OwnerCheck::Ok => {
                RigTestStep { name: "owner".into(), ok: Some(true), detail: "Mercury keys through this rigctld".into() }
            }
            OwnerCheck::Warn { reason } => RigTestStep { name: "owner".into(), ok: None, detail: reason },
            OwnerCheck::Refused { reason } => RigTestStep { name: "owner".into(), ok: Some(false), detail: reason },
            OwnerCheck::Unknown => RigTestStep {
                name: "owner".into(),
                ok: None,
                detail: "Mercury's PTT settings are not known: it has no telemetry connection".into(),
            },
        };
        let addr = format!("{}:{}", cfg.external.host, cfg.external.port);
        tokio::spawn(async move {
            let mut steps: Vec<RigTestStep> = probe(&addr, RIG_TEST_TIMEOUT)
                .await
                .into_iter()
                .map(|s| RigTestStep { name: s.name.into(), ok: s.ok, detail: s.detail })
                .collect();
            steps.push(owner);
            let _ = reply.send(Ok(CommandOk::RigTest { steps }));
        });
    }

    pub(super) fn set_rig_config(&mut self, config: RigConfig) -> Result<CommandOk, AppError> {
        let Some(id) = self.active_profile().map(|p| p.id) else {
            return Err(conflict("no station is selected".into()));
        };
        if let Some(e) = config.validate().into_iter().next() {
            return Err(invalid(&e.field, &e.message));
        }
        if config.freq_control == FreqControl::ExternalRigctld {
            let host = config.external.host.trim();
            if host.is_empty() || host.len() > 253 || host.chars().any(|c| c.is_whitespace() || c.is_control()) {
                return Err(invalid("host", "Enter the rigctld host, e.g. 127.0.0.1"));
            }
            if config.external.port == 0 {
                return Err(invalid("port", "Enter the rigctld port, e.g. 4532"));
            }
        }
        let mut config = config;
        config.external.host = config.external.host.trim().to_string();
        config.save(&mut self.store, &id).map_err(|message| AppError::Storage { message })?;
        self.start_rig(Some(&id));
        Ok(CommandOk::Done)
    }

    fn rig_request(&mut self, req: RigRequest) -> Option<u64> {
        let tx = self.rig.tx.as_ref()?;
        self.rig.seq += 1;
        let id = self.rig.seq;
        tx.try_send(RigClientCmd::Request { id, req }).ok()?;
        Some(id)
    }

    pub(super) fn rig_set(&mut self, change: RigChange) -> Result<CommandOk, AppError> {
        self.rig_set_all(std::slice::from_ref(&change))
    }

    /// Several changes as one: every one is checked before any is sent, so
    /// there is one refusal for all. The first is the one a call waits for.
    pub(super) fn rig_set_all(&mut self, changes: &[RigChange]) -> Result<CommandOk, AppError> {
        let activity = self.activity();
        for change in changes {
            validate(change)?;
            rig_gate(activity, &self.rig.policy, change, self.attached()).map_err(refused)?;
            request_for(change).ok_or_else(|| invalid("change", "not a rig change"))?;
        }
        let offset = self.rig.cfg.offset_hz;
        let mut reqs = Vec::with_capacity(changes.len());
        for change in changes {
            reqs.push(match request_for(change).ok_or_else(|| invalid("change", "not a rig change"))? {
                RigRequest::SetFreq(hz) => RigRequest::SetFreq(to_dial(hz, offset)?),
                other => other,
            });
        }
        for (change, req) in changes.iter().zip(reqs) {
            let id = self.rig_request(req).ok_or_else(|| conflict("the rig link is not accepting commands".into()))?;
            // Tracked as soon as it is sent, so a later failure leaves none untracked.
            self.rig.in_flight.push((id, change.clone()));
        }
        Ok(CommandOk::Done)
    }

    pub(super) fn dismiss_rig_banner(&mut self, banner: RigBanner) -> Result<CommandOk, AppError> {
        if !self.rig.policy.dismiss(banner) {
            return Err(conflict(match banner {
                RigBanner::LostWhileKeyed => "not yet: the rig is not known to be unkeyed".into(),
                _ => "not yet: the rig still disagrees with the modem".into(),
            }));
        }
        self.rig.dirty = true;
        Ok(CommandOk::Done)
    }

    /// A call arrives: hold it while rig changes are on their way to the
    /// rig, so none lands after `CONNECT` (Review Focus 1).
    /// Returns true if it was held (the caller then only shows Calling).
    pub(super) fn hold_call(&mut self, peer: &Callsign) -> bool {
        if self.rig.in_flight.is_empty() {
            return false;
        }
        self.rig.held_call =
            Some(HeldCall { peer: peer.clone(), before: self.modem, until: Instant::now() + CALL_HOLD });
        true
    }

    /// Cancel a held call (CancelCall or Abort before it went out).
    pub(super) fn cancel_held_call(&mut self) -> bool {
        let Some(h) = self.rig.held_call.take() else { return false };
        self.set_modem(h.before, None);
        true
    }

    /// The rig changes a held call waited for are done. `failed`: the rig
    /// did not confirm one (it may still land later), so the call is given
    /// up rather than made with the rig possibly retuning under it.
    fn release_held_call(&mut self, failed: Option<&str>) {
        let Some(h) = self.rig.held_call.take() else { return };
        if self.modem != ModemState::Calling {
            return; // the state moved on (a fault, a new selection)
        }
        if let Some(why) = failed {
            self.set_modem(h.before, None);
            let reason = format!("the rig did not confirm the frequency change ({why}); call again");
            let line = self.discovery.note("rig", format!("call to {} not made: {reason}", h.peer.as_str()));
            self.emit(line);
            self.emit(AppEvent::CallFailed { peer: h.peer.as_str().to_string(), reason });
            return;
        }
        if self.command_modem(ModemCmd::Call(h.peer.clone())).is_err() {
            self.set_modem(h.before, None);
            self.emit(AppEvent::CallFailed {
                peer: h.peer.as_str().to_string(),
                reason: "the modem is not accepting commands".into(),
            });
        }
    }

    /// Something the rigctld client said.
    pub(super) fn on_rig(&mut self, msg: RigMsg) {
        let msg = match msg {
            RigMsg::Daemon(state) => return self.on_daemon(state),
            RigMsg::Freq(dial) => RigMsg::Freq(to_display(dial, self.rig.cfg.offset_hz)),
            other => other,
        };
        match &msg {
            RigMsg::Link(RigLink::Lost { reason }) => self.rig.cat_note(format!("rig link lost: {reason}")),
            RigMsg::Done { result: Err(e), .. } => self.rig.cat_note(format!("request failed: {e}")),
            _ => {}
        }
        let now = Instant::now();
        if let RigMsg::Done { id, result } = &msg {
            self.rig_done(*id, result.clone(), now);
        }
        let came_up = matches!(msg, RigMsg::Link(RigLink::Up));
        let lost = matches!(msg, RigMsg::Link(RigLink::Lost { .. }));
        let actions = self.rig.policy.on_msg(&msg, now);
        self.rig_actions(actions);
        if lost {
            self.stop_tune_unchecked("the rig link was lost");
        }
        if came_up && self.rig.restore_pending {
            self.rig.restore_pending = false;
            self.restore_last();
        }
        if matches!(msg, RigMsg::Freq(_) | RigMsg::Mode { .. }) {
            self.save_last(now);
        }
        self.rig.dirty = true;
    }

    /// Mercury's `PTT ON` / `PTT OFF`.
    pub(super) fn on_mercury_ptt(&mut self, on: bool) {
        let actions = self.rig.policy.on_mercury_ptt(on, Instant::now());
        self.rig_actions(actions);
        self.rig.dirty |= self.rig.policy.configured();
    }

    fn rig_done(&mut self, id: u64, result: Result<RigReply, String>, now: Instant) {
        let Some(i) = self.rig.in_flight.iter().position(|(want, _)| *want == id) else { return };
        let (_, change) = self.rig.in_flight.remove(i);
        match result {
            Ok(_) => {
                // The operator's own choice is the "last frequency", stored at once.
                let v = self.rig.policy.view(now);
                let chosen = match &change {
                    RigChange::Frequency { hz } => {
                        Some(LastTuning { freq_hz: *hz, mode: v.mode, passband_hz: v.passband_hz })
                    }
                    RigChange::Mode { mode, passband_hz } => v.frequency.map(|f| LastTuning {
                        freq_hz: f.value,
                        mode: Some(mode.clone()),
                        passband_hz: Some(*passband_hz),
                    }),
                    RigChange::RfPower { level } => {
                        self.rig.policy.set_rf_power(*level, now);
                        None
                    }
                    _ => None,
                };
                if let Some(last) = chosen {
                    self.store_last(last, now);
                }
            }
            Err(e) => {
                let line = self.discovery.note("rig", format!("the rig did not take the change ({e})"));
                self.emit(line);
                // A timed-out change may still land: never call after it.
                self.rig.in_flight.clear();
                self.release_held_call(Some(&e));
                return;
            }
        }
        if self.rig.in_flight.is_empty() {
            self.release_held_call(None);
        }
    }

    fn rig_actions(&mut self, actions: Vec<RigAction>) {
        for a in actions {
            let text = match a {
                RigAction::SendPttOff => {
                    let sent = self.rig_request(RigRequest::PttOff).is_some();
                    if sent {
                        "the rig link is back: sent PTT off once".to_string()
                    } else {
                        "the rig link is back, but PTT off could not be sent: unkey the radio by hand".to_string()
                    }
                }
                RigAction::Banner(RigBanner::PttNotFollowing) => {
                    self.stop_tune_unchecked("the rig's PTT does not follow the modem");
                    "the rig's PTT does not follow the modem: check the radio and its CAT link".into()
                }
                RigAction::Banner(RigBanner::LostWhileKeyed) => {
                    "rig link lost while transmitting: unkey the radio by hand".into()
                }
                RigAction::Banner(RigBanner::RestartModem) => {
                    "Mercury reports PTT on but the rig is not keyed: restart the modem to key again".into()
                }
            };
            let line = self.discovery.note("rig", text);
            self.emit(line);
        }
    }

    fn restore_last(&mut self) {
        let Some(id) = self.rig.profile_id.clone() else { return };
        let Some(last) = self
            .store
            .setting(&rig_key(&id, "last"))
            .ok()
            .flatten()
            .and_then(|j| serde_json::from_str::<LastTuning>(&j).ok())
        else {
            return;
        };
        let change = RigChange::Frequency { hz: last.freq_hz };
        if rig_gate(self.activity(), &self.rig.policy, &change, self.attached()).is_err() {
            let line = self.discovery.note("rig", "the last frequency was not restored: the station is busy");
            self.emit(line);
            return;
        }
        let Ok(dial) = to_dial(last.freq_hz, self.rig.cfg.offset_hz) else { return };
        if let Some(id) = self.rig_request(RigRequest::SetFreq(dial)) {
            self.rig.in_flight.push((id, change));
        }
        if let (Some(mode), Some(pb)) = (last.mode, last.passband_hz) {
            let change = RigChange::Mode { mode: mode.clone(), passband_hz: pb };
            if let Some(id) = self.rig_request(RigRequest::SetMode { mode, passband_hz: pb }) {
                self.rig.in_flight.push((id, change));
            }
        }
        let line = self.discovery.note("rig", format!("restored the last frequency, {} Hz", last.freq_hz));
        self.emit(line);
    }

    fn save_last(&mut self, now: Instant) {
        let v = self.rig.policy.view(now);
        let Some(f) = v.frequency else { return };
        let last = LastTuning { freq_hz: f.value, mode: v.mode, passband_hz: v.passband_hz };
        if self.rig.last_saved.as_ref() == Some(&last)
            || self.rig.last_saved_at.is_some_and(|t| now.saturating_duration_since(t) < LAST_SAVE_GAP)
        {
            return;
        }
        self.store_last(last, now);
    }

    fn store_last(&mut self, last: LastTuning, now: Instant) {
        let Some(id) = self.rig.profile_id.clone() else { return };
        if let Ok(json) = serde_json::to_string(&last) {
            if let Err(e) = self.store.set_setting(&rig_key(&id, "last"), &json).map_err(storage) {
                tracing::warn!("could not store the last frequency: {e}");
            }
        }
        self.rig.last_saved = Some(last);
        self.rig.last_saved_at = Some(now);
    }

    /// The frequency (and offset) to record with a heard station or a QSO:
    /// the rig's, when fresh and its link is up; else Mercury's telemetry.
    pub(super) fn tuning(&self, now: Instant) -> Tuning {
        let mut t = self.telemetry.tuning(now);
        if self.rig.policy.link_up() {
            if let Some((hz, at)) = self.rig.policy.frequency() {
                let age = now.saturating_duration_since(at);
                if age <= RIG_FRESH {
                    t.freq = Some(Reading {
                        value: hz,
                        source: RIG_SOURCE.into(),
                        at_utc: now_utc(),
                        age_ms: Some(age.as_millis() as u64),
                    });
                }
            }
        }
        t
    }

    /// After every turn of the loop: the activity and poll rate, a held
    /// call's deadline, and the throttled rig event.
    pub(super) fn sync_rig(&mut self) {
        let now = Instant::now();
        if self.rig.held_call.as_ref().is_some_and(|h| now >= h.until) {
            self.rig.in_flight.clear();
            self.release_held_call(Some("no answer within 1 s"));
        }
        self.sync_tx_level(now);
        self.sync_tune(now);
        self.emit_tune();
        self.sync_schedule(now);
        let activity = self.activity();
        self.rig.policy.set_activity(activity);
        let rate = self.rig.policy.poll_rate(activity);
        if self.rig.rate != Some(rate) {
            if let Some(tx) = &self.rig.tx {
                if tx.try_send(RigClientCmd::Rate(rate)).is_ok() {
                    self.rig.rate = Some(rate);
                }
            }
        }
        if self.rig.dirty && self.rig.emitted_at.is_none_or(|t| now.saturating_duration_since(t) >= RIG_EVENT_GAP) {
            self.rig.dirty = false;
            self.rig.emitted_at = Some(now);
            let rig = self.rig.policy.view(now);
            self.emit(AppEvent::Rig { rig });
        }
    }

    /// When the loop must next wake for the rig: a throttled event, a held
    /// call or a TX level awaiting confirmation.
    pub(super) fn rig_wake(&self) -> Option<Instant> {
        let event = self.rig.dirty.then(|| self.rig.emitted_at.map(|t| t + RIG_EVENT_GAP)).flatten();
        let held = self.rig.held_call.as_ref().map(|h| h.until);
        let tx = self.rig.tx_wait.as_ref().map(|w| w.2);
        let tune = self.rig.tune.until.as_ref().filter(|_| !self.rig.tune.off_sent).map(|u| u.0);
        [event, held, tx, tune, self.freq.wake()].into_iter().flatten().min()
    }

    /// The settings the rig link runs on, when they are not the stored ones
    /// (the simulator's own rig).
    pub(super) fn rig_config_override(&self) -> Option<RigConfig> {
        self.rig.sim.map(|_| self.rig.cfg.clone())
    }

    /// The simulator's rig is up: link to it, with Mercury keying through it
    /// as the simulator's PTT does (Hamlib model 2 at its address).
    pub(super) fn rig_sim(&mut self, addr: std::net::SocketAddr) {
        self.rig.sim = Some(addr);
        self.rig.radio_list =
            Some(RadioList { ptt_method: "hamlib".into(), selected_model: Some(2), device_path: addr.to_string() });
        let id = self.rig.profile_id.clone();
        self.start_rig(id.as_deref());
    }

    /// A new modem (or none): its telemetry client, and what it said, are gone.
    pub(super) fn rig_modem_changed(&mut self) {
        self.rig.sim = None;
        self.rig.tele_tx = None;
        self.rig.radio_list = None;
        self.rig.tx_wait = None;
        self.rig.tx_pending = None;
        self.rig.band = None;
        self.rig.policy.set_tx_level(None);
        // The new modem is asked again; the old one's Tune ends with it
        // (and Mercury stops it by itself after 60 s regardless).
        self.rig.tune.supported = None;
        self.rig.tune.stopped(); // published at the end of the turn, after the state
    }

    /// Calls and broadcasts wait while Tune is on.
    pub(super) fn refuse_if_tuning(&self) -> Result<(), AppError> {
        if self.rig.tune.active {
            return Err(refused(RigRefusal::Tuning));
        }
        Ok(())
    }

    /// Tune keys a carrier only while the cross-check can watch it: a lost
    /// rig link or a PTT fault stops it.
    fn stop_tune_unchecked(&mut self, why: &str) {
        if self.rig.tune.active && self.tune(false).is_ok() {
            let line = self.discovery.note("rig", format!("Tune stopped: {why}"));
            self.emit(line);
        }
    }

    /// Abort also stops Tune. True if it did.
    pub(super) fn stop_tune_for_abort(&mut self) -> bool {
        self.rig.tune.active && self.tune(false).is_ok()
    }

    /// Publish the Tune state if it changed.
    fn emit_tune(&mut self) {
        let tune = self.rig.tune.view();
        if self.rig.tune.shown.as_ref() != Some(&tune) {
            self.rig.tune.shown = Some(tune.clone());
            self.emit(AppEvent::Tune { tune });
        }
    }

    /// The tune level for the band the rig is on, or the default.
    fn band_tune_level(&self) -> (Option<String>, f32) {
        let band = self.tuning(Instant::now()).freq.and_then(|f| band_for(f.value)).map(String::from);
        let level = match (self.rig.profile_id.as_deref(), band.as_deref()) {
            (Some(id), Some(b)) => BandLevels::for_band(&self.store, id, b).tune_dbfs,
            _ => BandLevels::default().tune_dbfs,
        };
        (band, level)
    }

    pub(super) fn tune(&mut self, on: bool) -> Result<CommandOk, AppError> {
        if !on {
            // Always allowed, and sent even if MercuryChat thinks it is off.
            self.command_modem(ModemCmd::Tune(TuneArg::Off))?;
            if self.rig.tune.active {
                self.rig.tune.off_sent = true;
            }
            return Ok(CommandOk::Done);
        }
        if self.rig.tune.active {
            return Err(conflict("Tune is already on".into()));
        }
        rig_gate(self.activity(), &self.rig.policy, &RigChange::Tune, self.attached()).map_err(refused)?;
        match self.rig.tune.supported {
            Some(true) => {}
            Some(false) => {
                return Err(conflict("this Mercury has no TUNE: update Mercury to tune from MercuryChat".into()))
            }
            None => return Err(conflict("not yet: the modem has not said whether it can tune".into())),
        }
        let (_, level) = self.band_tune_level();
        self.command_modem(ModemCmd::Tune(TuneArg::Level(level)))?;
        let limit = self.rig.tune_limit;
        let ahead = chrono::Duration::from_std(limit).unwrap_or_default();
        let ends = (chrono::Utc::now() + ahead).format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string();
        let t = &mut self.rig.tune;
        t.active = true;
        t.level = Some(level);
        t.until = Some((Instant::now() + limit, ends));
        t.off_sent = false;
        self.emit_tune();
        Ok(CommandOk::Done)
    }

    pub(super) fn set_tune_level(&mut self, dbfs: f32) -> Result<CommandOk, AppError> {
        if !self.rig.tune.active {
            return Err(conflict("Tune is not on".into()));
        }
        if !dbfs.is_finite() || !(TUNE_RANGE.0..=TUNE_RANGE.1).contains(&dbfs) {
            let m = format!("Tune level must be {} to {} dBFS", TUNE_RANGE.0, TUNE_RANGE.1);
            return Err(invalid("dbfs", &m));
        }
        self.command_modem(ModemCmd::Tune(TuneArg::Level(dbfs)))?;
        self.rig.tune.level = Some(dbfs);
        // Remembered for the band, as the Settings levels table shows.
        if let (Some(id), (Some(band), _)) = (self.rig.profile_id.clone(), self.band_tune_level()) {
            let levels = BandLevels { tune_dbfs: dbfs, ..BandLevels::for_band(&self.store, &id, &band) };
            BandLevels::save(&mut self.store, &id, &band, levels).map_err(|message| AppError::Storage { message })?;
        }
        self.emit_tune();
        Ok(CommandOk::Done)
    }

    /// What became of a TUNE command, or the probe.
    pub(super) fn tune_report(&mut self, result: TuneResult) {
        match result {
            TuneResult::Supported(s) => self.rig.tune.supported = Some(s),
            TuneResult::Started => {}
            TuneResult::Stopped => self.rig.tune.stopped(),
            TuneResult::Refused(why) => {
                let line = self.discovery.note("rig", format!("Tune refused: {why}"));
                self.emit(line);
                if self.rig.tune.active && !self.rig.tune.off_sent {
                    // It may still be on at the old level: never leave it.
                    let _ = self.command_modem(ModemCmd::Tune(TuneArg::Off));
                }
                self.rig.tune.stopped();
            }
        }
        self.emit_tune();
    }

    /// Stop Tune once its time is up.
    fn sync_tune(&mut self, now: Instant) {
        let t = &self.rig.tune;
        if !t.active || t.off_sent || t.until.as_ref().is_none_or(|u| now < u.0) {
            return;
        }
        let secs = self.rig.tune_limit.as_secs_f32();
        let line = self.discovery.note("rig", format!("Tune stopped after {secs:.0} s"));
        self.emit(line);
        if self.command_modem(ModemCmd::Tune(TuneArg::Off)).is_ok() {
            self.rig.tune.off_sent = true;
        } else {
            self.rig.tune.stopped();
            self.emit_tune();
        }
    }

    /// The modem's telemetry client takes commands here.
    pub(super) fn rig_telemetry(&mut self, tx: mpsc::Sender<TelemetryCmd>) {
        self.rig.tele_tx = Some(tx);
    }

    /// What the rig side reads from Mercury's telemetry.
    pub(super) fn rig_telemetry_msg(&mut self, msg: &TelemetryMsg) {
        match msg {
            TelemetryMsg::RadioList(rl) => {
                let before = self.rig.policy.owner_check().clone();
                self.rig.radio_list = Some(rl.clone());
                let after = owner_check(&self.rig.cfg, Some(rl), self.rig.managed_listen);
                if after != before {
                    let was_refused = matches!(before, OwnerCheck::Refused { .. });
                    let refused = matches!(after, OwnerCheck::Refused { .. });
                    if let OwnerCheck::Refused { reason } | OwnerCheck::Warn { reason } = &after {
                        let line = self.discovery.note("rig", reason.clone());
                        self.emit(line);
                    }
                    if refused != was_refused {
                        // Stop the link, or start it again now that Mercury has changed.
                        let id = self.rig.profile_id.clone();
                        self.start_rig(id.as_deref());
                    } else {
                        self.rig.policy.set_owner_check(after);
                    }
                    self.rig.dirty = true;
                }
            }
            TelemetryMsg::Status(st) => {
                if let (Some((band, db, _)), Some(now)) = (&self.rig.tx_wait, st.tx_gain_db) {
                    if (now - db).abs() <= TX_LEVEL_TOLERANCE {
                        let result = TxLevelResult::Applied { band: band.clone(), db: *db };
                        self.rig.tx_wait = None;
                        self.tx_level_done(result);
                    }
                }
            }
            TelemetryMsg::CommandFailed { reason } => {
                if let Some((band, _, _)) = self.rig.tx_wait.take() {
                    self.tx_level_done(TxLevelResult::NotApplied { band, reason: reason.clone() });
                }
            }
            TelemetryMsg::Link(_) => {}
        }
    }

    fn tx_level_done(&mut self, result: TxLevelResult) {
        let text = match &result {
            TxLevelResult::Applied { band, db } => format!("TX level for {band} applied: {db:+.1} dB"),
            TxLevelResult::NotApplied { band, reason } => format!("TX level for {band} not applied: {reason}"),
            TxLevelResult::Pending { .. } => return,
        };
        let line = self.discovery.note("rig", text);
        self.emit(line);
        self.rig.policy.set_tx_level(Some(result));
        self.rig.dirty = true;
    }

    pub(super) fn band_levels(&self) -> Result<CommandOk, AppError> {
        let id = self.active_profile().map(|p| p.id).ok_or_else(|| conflict("no station is selected".into()))?;
        Ok(CommandOk::BandLevels { levels: BandLevels::load_all(&self.store, &id) })
    }

    pub(super) fn set_band_levels(&mut self, band: String, levels: BandLevels) -> Result<CommandOk, AppError> {
        let Some(id) = self.active_profile().map(|p| p.id) else {
            return Err(conflict("no station is selected".into()));
        };
        if !BANDS.iter().any(|(_, _, b)| *b == band) {
            return Err(invalid("band", "Choose an amateur band, e.g. 40m"));
        }
        levels.validate().map_err(|m| invalid("levels", &m))?;
        BandLevels::save(&mut self.store, &id, &band, levels).map_err(|message| AppError::Storage { message })?;
        if self.rig.band.as_deref() == Some(band.as_str()) {
            self.rig.tx_pending = Some((band, levels.tx_gain_db));
        }
        Ok(CommandOk::Done)
    }

    /// Follow the band, and apply a stored TX level once the station is idle.
    fn sync_tx_level(&mut self, now: Instant) {
        if let Some((band, _, deadline)) = &self.rig.tx_wait {
            if now >= *deadline {
                let band = band.clone();
                self.rig.tx_wait = None;
                let reason = format!("Mercury did not confirm the new level within {} s", TX_LEVEL_CONFIRM.as_secs());
                self.tx_level_done(TxLevelResult::NotApplied { band, reason });
            }
        }
        let band = self.tuning(now).freq.and_then(|f| band_for(f.value)).map(String::from);
        if band.is_some() && band != self.rig.band {
            self.rig.band = band.clone();
            let stored = match (self.rig.profile_id.as_deref(), band.as_deref()) {
                (Some(id), Some(b)) => BandLevels::load_all(&self.store, id).get(b).copied(),
                _ => None,
            };
            // A band without stored levels leaves Mercury's own level alone.
            self.rig.tx_pending = stored.map(|l| (band.clone().unwrap_or_default(), l.tx_gain_db));
        }
        let Some((band, db)) = self.rig.tx_pending.clone() else { return };
        if self.rig.tx_wait.is_some()
            || rig_gate(self.activity(), &self.rig.policy, &RigChange::TxLevel, self.attached()).is_err()
        {
            return; // waits until the station is idle
        }
        self.rig.tx_pending = None;
        let sent = self.rig.tele_tx.as_ref().is_some_and(|t| t.try_send(TelemetryCmd::SetTxGain(db)).is_ok());
        if sent {
            self.rig.tx_wait = Some((band.clone(), db, now + TX_LEVEL_CONFIRM));
            self.rig.policy.set_tx_level(Some(TxLevelResult::Pending { band, db }));
            self.rig.dirty = true;
        } else {
            let reason = "no telemetry connection to Mercury".to_string();
            self.tx_level_done(TxLevelResult::NotApplied { band, reason });
        }
    }
}
