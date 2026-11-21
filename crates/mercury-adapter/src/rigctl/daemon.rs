//! MercuryChat's own rigctld (M3b.1 design §5.3): its command line, Hamlib's
//! list of rig models, and (Task 3) the process that runs it.

use std::collections::VecDeque;
use std::ffi::OsStr;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

/// What MercuryChat starts rigctld with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RigctldSpec {
    /// Hamlib rig model (`-m`).
    pub model: u32,
    /// The radio: a serial port (`COM3`, `/dev/ttyUSB0`) or a network radio's `host:port` (`-r`).
    pub rig_path: String,
    /// Serial speed (`-s`); none for a network radio.
    pub baud: Option<u32>,
    /// Hamlib configuration parameters (`-C name=value,...`).
    pub conf: Vec<(String, String)>,
    /// PTT type (`-P`): RIG, DTR, RTS, CM108 or NONE.
    pub ptt_type: &'static str,
    /// A separate PTT device (`-p`); none: the rig's own port.
    pub ptt_path: Option<String>,
    /// Where rigctld listens (`-T`, `-t`).
    pub listen: SocketAddr,
}

/// rigctld's arguments for `spec`, in a fixed order.
pub fn rigctld_args(spec: &RigctldSpec) -> Vec<String> {
    let mut a = vec!["-m".to_string(), spec.model.to_string(), "-r".to_string(), spec.rig_path.clone()];
    if let Some(b) = spec.baud {
        a.extend(["-s".to_string(), b.to_string()]);
    }
    if !spec.conf.is_empty() {
        let c: Vec<String> = spec.conf.iter().map(|(k, v)| format!("{k}={v}")).collect();
        a.extend(["-C".to_string(), c.join(",")]);
    }
    a.extend(["-P".to_string(), spec.ptt_type.to_string()]);
    if let Some(p) = &spec.ptt_path {
        a.extend(["-p".to_string(), p.clone()]);
    }
    a.extend(["-T".to_string(), spec.listen.ip().to_string(), "-t".to_string(), spec.listen.port().to_string()]);
    a
}

/// One rig model Hamlib supports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HamlibModel {
    pub id: u32,
    pub mfg: String,
    pub model: String,
    pub status: String,
}

/// `rigctld -l`. Hamlib prints fixed-width columns under a header (`Rig #`,
/// `Mfg`, `Model`, `Version`, `Status`, `Macro`), and names contain spaces,
/// so each row is cut at the header's column starts.
pub fn parse_model_list(stdout: &str) -> Vec<HamlibModel> {
    let mut lines = stdout.lines();
    let cols = loop {
        let Some(l) = lines.next() else { return Vec::new() };
        if !l.contains("Rig #") {
            continue;
        }
        let at = |name: &str| l.find(name);
        match (at("Mfg"), at("Model"), at("Version"), at("Status")) {
            (Some(m), Some(mo), Some(v), Some(s)) if m < mo && mo < v && v < s => break (m, mo, v, s),
            _ => return Vec::new(),
        }
    };
    let (mfg, model, version, status) = cols;
    let cut = |l: &str, from: usize, to: usize| -> String {
        let b = l.as_bytes();
        let from = from.min(b.len());
        let to = to.min(b.len());
        String::from_utf8_lossy(&b[from..to]).trim().to_string()
    };
    lines
        .filter_map(|l| {
            let id = cut(l, 0, mfg).parse().ok()?;
            // The status is one word; the macro name follows it.
            let rest = cut(l, status, usize::MAX);
            Some(HamlibModel {
                id,
                mfg: cut(l, mfg, model),
                model: cut(l, model, version),
                status: rest.split_whitespace().next().unwrap_or("").to_string(),
            })
        })
        .collect()
}

// ---- The process (Task 3) ---------------------------------------------------

/// How long rigctld has to answer after it starts.
pub const READY_WITHIN: Duration = Duration::from_secs(5);
/// The wait before the one restart.
pub const RESTART_AFTER: Duration = Duration::from_secs(2);
/// Lines of rigctld's stderr kept for the CAT log.
pub const LOG_LINES: usize = 200;
const PROBE_TIMEOUT: Duration = Duration::from_secs(1);

const EXE: &str = if cfg!(windows) { "rigctld.exe" } else { "rigctld" };

/// rigctld on the PATH, else in `dir`.
pub fn find_rigctld(dir: Option<&Path>) -> Option<PathBuf> {
    find_rigctld_in(&std::env::var_os("PATH").unwrap_or_default(), dir)
}

/// As [`find_rigctld`], with the PATH given.
pub fn find_rigctld_in(path_var: &OsStr, dir: Option<&Path>) -> Option<PathBuf> {
    std::env::split_paths(path_var).chain(dir.map(Path::to_path_buf)).map(|d| d.join(EXE)).find(|p| p.is_file())
}

/// What MercuryChat's rigctld is doing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DaemonState {
    /// A rigctld was already answering at the address: used, never stopped.
    Reused,
    Starting,
    Running,
    /// It exited or did not answer; one more start follows.
    Restarting,
    Failed {
        reason: String,
    },
}

/// Does something answer at `addr` as rigctld does: a frequency for `+f`
/// (extended form: `get_freq:`, `Frequency: N`, `RPRT 0`)?
pub async fn answers_as_rigctld(addr: SocketAddr) -> bool {
    let attempt = async {
        let mut s = tokio::net::TcpStream::connect(addr).await.ok()?;
        s.write_all(b"+f\n").await.ok()?;
        let mut text = String::new();
        let mut buf = [0u8; 256];
        while !text.contains("RPRT") && text.len() < 1024 {
            let n = s.read(&mut buf).await.ok()?;
            if n == 0 {
                return None;
            }
            text.push_str(&String::from_utf8_lossy(&buf[..n]));
        }
        let freq = text.lines().find_map(|l| l.trim().strip_prefix("Frequency:"))?;
        freq.trim().parse::<u64>().ok().filter(|_| text.contains("RPRT 0"))
    };
    matches!(tokio::time::timeout(PROBE_TIMEOUT, attempt).await, Ok(Some(_)))
}

/// MercuryChat's own rigctld. [`RigctldDaemon::shutdown`] stops it and waits
/// until it has gone; dropping the handle stops it without waiting. Either
/// way only a rigctld this handle started is stopped; a reused one is never
/// touched.
pub struct RigctldDaemon {
    log: Arc<Mutex<VecDeque<String>>>,
    stop: watch::Sender<bool>,
    task: Option<JoinHandle<()>>,
}

impl Drop for RigctldDaemon {
    fn drop(&mut self) {
        // The task owns the child, spawned with kill_on_drop: aborting it stops rigctld.
        if let Some(t) = &self.task {
            t.abort();
        }
    }
}

impl RigctldDaemon {
    pub fn start(exe: PathBuf, spec: RigctldSpec, events: mpsc::Sender<DaemonState>) -> RigctldDaemon {
        Self::spawn(exe, spec, events, Vec::new(), None)
    }

    /// As [`RigctldDaemon::start`], once `previous` (the daemon this one
    /// replaces) has stopped: otherwise the new one could find the old one
    /// still answering on the same address and "reuse" it as it exits.
    pub fn start_after(
        exe: PathBuf,
        spec: RigctldSpec,
        events: mpsc::Sender<DaemonState>,
        previous: Option<RigctldDaemon>,
    ) -> RigctldDaemon {
        Self::spawn(exe, spec, events, Vec::new(), previous)
    }

    /// As [`RigctldDaemon::start`], with extra environment for the program (tests).
    pub fn start_with_env(
        exe: PathBuf,
        spec: RigctldSpec,
        events: mpsc::Sender<DaemonState>,
        env: Vec<(String, String)>,
    ) -> RigctldDaemon {
        Self::spawn(exe, spec, events, env, None)
    }

    fn spawn(
        exe: PathBuf,
        spec: RigctldSpec,
        events: mpsc::Sender<DaemonState>,
        env: Vec<(String, String)>,
        previous: Option<RigctldDaemon>,
    ) -> RigctldDaemon {
        let log = Arc::new(Mutex::new(VecDeque::with_capacity(LOG_LINES)));
        let (stop, stopped) = watch::channel(false);
        let l = log.clone();
        let task = tokio::spawn(async move {
            if let Some(p) = previous {
                p.shutdown().await;
            }
            supervise(exe, spec, events, env, l, stopped).await;
        });
        RigctldDaemon { log, stop, task: Some(task) }
    }

    /// Let rigctld outlive this handle: the app is exiting while Mercury may
    /// be keying through it, and a radio left keyed is worse than a rigctld
    /// left running. The handle is forgotten on purpose: dropping it, or its
    /// stop signal, would kill rigctld.
    pub fn leave_running(self) {
        std::mem::forget(self);
    }

    /// Stop rigctld, if this handle started it, and return once it has exited
    /// (its port is free). The app calls this on exit: a dropped handle is
    /// not enough then, as the runtime may stop before the kill is sent.
    pub async fn shutdown(mut self) {
        let _ = self.stop.send(true);
        if let Some(t) = self.task.take() {
            let _ = t.await;
        }
    }

    /// The last [`LOG_LINES`] lines rigctld wrote to stderr, oldest first.
    pub fn log(&self) -> Vec<String> {
        self.log.lock().unwrap().iter().cloned().collect()
    }
}

fn push_line(log: &Mutex<VecDeque<String>>, line: String) {
    let mut l = log.lock().unwrap();
    if l.len() == LOG_LINES {
        l.pop_front();
    }
    l.push_back(line);
}

async fn supervise(
    exe: PathBuf,
    spec: RigctldSpec,
    events: mpsc::Sender<DaemonState>,
    env: Vec<(String, String)>,
    log: Arc<Mutex<VecDeque<String>>>,
    mut stop: watch::Receiver<bool>,
) {
    if *stop.borrow() {
        return;
    }
    if answers_as_rigctld(spec.listen).await {
        let _ = events.send(DaemonState::Reused).await;
        return;
    }
    let args = rigctld_args(&spec);
    let _ = events.send(DaemonState::Starting).await;
    for attempt in 0..2 {
        if attempt == 1 {
            let _ = events.send(DaemonState::Restarting).await;
            tokio::select! {
                _ = tokio::time::sleep(RESTART_AFTER) => {}
                _ = stopped(&mut stop) => return,
            }
        }
        let Some(why) = run_once(&exe, &args, &env, spec.listen, &events, &log, &mut stop).await else {
            return; // stopped
        };
        if attempt == 1 {
            let _ = events.send(DaemonState::Failed { reason: why }).await;
        }
    }
}

/// Resolves once `stop` says stop (or its sender has gone).
async fn stopped(stop: &mut watch::Receiver<bool>) {
    let _ = stop.wait_for(|s| *s).await;
}

/// Start rigctld once and wait for it to stop; why it stopped, or None when
/// `stop` ended it (killed and waited for).
async fn run_once(
    exe: &Path,
    args: &[String],
    env: &[(String, String)],
    listen: SocketAddr,
    events: &mpsc::Sender<DaemonState>,
    log: &Arc<Mutex<VecDeque<String>>>,
    stop: &mut watch::Receiver<bool>,
) -> Option<String> {
    let mut cmd = tokio::process::Command::new(exe);
    cmd.args(args)
        .envs(env.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return Some(format!("{}: {e}", exe.display())),
    };
    let last = Arc::new(Mutex::new(None::<String>));
    let reader = child.stderr.take().map(|err| {
        let (log, last) = (log.clone(), last.clone());
        tokio::spawn(async move {
            let mut lines = BufReader::new(err).lines();
            while let Ok(Some(l)) = lines.next_line().await {
                *last.lock().unwrap() = Some(l.clone());
                push_line(&log, l);
            }
        })
    });
    let reason = |code: Option<i32>, last: &Mutex<Option<String>>| {
        last.lock().unwrap().clone().unwrap_or_else(|| match code {
            Some(c) => format!("rigctld exited (code {c})"),
            None => "rigctld exited".to_string(),
        })
    };
    // Ready: answering within READY_WITHIN, unless it exits first.
    let deadline = tokio::time::Instant::now() + READY_WITHIN;
    loop {
        if *stop.borrow() {
            let _ = child.kill().await; // kills and waits
            return None;
        }
        if let Ok(Some(status)) = child.try_wait() {
            if let Some(r) = reader {
                let _ = r.await; // the whole of stderr, for the reason
            }
            return Some(reason(status.code(), &last));
        }
        if answers_as_rigctld(listen).await {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            let _ = child.kill().await;
            return Some(format!("rigctld did not answer on {listen} within {} s", READY_WITHIN.as_secs()));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let _ = events.send(DaemonState::Running).await;
    let status = tokio::select! {
        s = child.wait() => s,
        _ = stopped(stop) => {
            let _ = child.kill().await;
            return None;
        }
    };
    if let Some(r) = reader {
        let _ = r.await;
    }
    Some(reason(status.ok().and_then(|s| s.code()), &last))
}
