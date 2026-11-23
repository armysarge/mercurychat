//! The built-in modem (M5 §5): MercuryChat's own Mercury and rigctld.
//!
//! [`Supervisor`] starts one private Mercury: ports checked free (never
//! adopting a listener already there), the ini written, the process tied to
//! the app, and "started" meaning the control port answers `VERSION`. It
//! stops only the process it started. Restarting after an exit is the
//! caller's rule (M5 §5.4); the supervisor reports the exit and the log tail.

pub mod child;
pub mod ini;
pub mod ports;

use crate::app::PttLine;
use crate::bundle::BundleError;
use crate::profile::BuiltInPorts;
use ports::PortTaken;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Child;

/// Mercury's PTT, from the profile's rig settings (M5 §5.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MercuryPtt {
    None,
    /// CAT: Hamlib model 2 to the managed rigctld on 127.0.0.1.
    Rigctld {
        port: u16,
    },
    Serial {
        port: String,
        line: PttLine,
    },
    Cm108 {
        device: String,
    },
}

#[derive(Debug, Clone)]
pub struct BuiltInStart {
    /// The verified Mercury program (`Bundle::verify`).
    pub mercury: PathBuf,
    /// The profile's Mercury folder: `mercury.ini` and `mercury.log`.
    pub dir: PathBuf,
    pub ports: BuiltInPorts,
    pub ptt: MercuryPtt,
    /// No sound card chosen yet: start with Mercury's `null` audio (M5 §5.5).
    pub audio_null: bool,
}

#[derive(Debug)]
pub enum StartError {
    Bundle(BundleError),
    PortTaken(PortTaken),
    Spawn(String),
    /// Started, but the control port did not answer within 10 s.
    NoAnswer {
        log_tail: String,
    },
}

impl std::fmt::Display for StartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StartError::Bundle(e) => write!(f, "{e}"),
            StartError::PortTaken(t) => write!(f, "Port {} is already in use on this computer.", t.port),
            StartError::Spawn(e) => write!(f, "Mercury could not be started: {e}"),
            StartError::NoAnswer { .. } => write!(f, "Mercury started but did not answer."),
        }
    }
}

impl std::error::Error for StartError {}

/// How Mercury ended: its exit code and the last lines of its log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExitInfo {
    pub code: Option<i32>,
    pub log_tail: String,
}

const ANSWER_WITHIN: Duration = Duration::from_secs(10);
const QUIT_WITHIN: Duration = Duration::from_secs(5);
const TAIL_LINES: usize = 40;

/// The running built-in Mercury.
pub struct Supervisor {
    child: Child,
    log: PathBuf,
}

/// The last 40 lines of Mercury's log.
pub fn log_tail(log: &Path) -> String {
    let Ok(bytes) = std::fs::read(log) else {
        return String::new();
    };
    let text = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(TAIL_LINES)..].join("\n")
}

/// The control port accepts and replies to `VERSION` (after Mercury's greeting).
async fn answers(port: u16) -> bool {
    let probe = async {
        let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.ok()?;
        s.write_all(b"VERSION\r").await.ok()?;
        let mut r = BufReader::new(s);
        loop {
            let mut line = Vec::new();
            if r.read_until(b'\r', &mut line).await.ok()? == 0 {
                return None;
            }
            if line.trim_ascii_start().starts_with(b"VERSION") {
                return Some(());
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(2), probe).await.ok().flatten().is_some()
}

impl Supervisor {
    pub async fn start(cfg: BuiltInStart) -> Result<Supervisor, StartError> {
        ports::check_free(&cfg.ports).map_err(StartError::PortTaken)?;
        let ini = ini::write_ini(&cfg.dir, &cfg.ports, &cfg.ptt)
            .map_err(|e| StartError::Spawn(format!("cannot write mercury.ini: {e}")))?;
        let log = cfg.dir.join("mercury.log");
        let p = &cfg.ports;
        let mut cmd = child::command(&cfg.mercury);
        cmd.arg("-C").arg(&ini);
        cmd.args(["-p", &p.control.to_string(), "-b", &p.broadcast.to_string(), "-G", "-U", &p.ui.to_string()]);
        cmd.arg("-L").arg(&log);
        if cfg.audio_null {
            cmd.args(["-x", "null"]);
        }
        cmd.current_dir(&cfg.dir);
        let mut child = child::spawn(&mut cmd).map_err(|e| StartError::Spawn(e.to_string()))?;

        let end = Instant::now() + ANSWER_WITHIN;
        loop {
            if let Ok(Some(_)) = child.try_wait() {
                return Err(StartError::NoAnswer { log_tail: log_tail(&log) });
            }
            if answers(p.control).await {
                return Ok(Supervisor { child, log });
            }
            if Instant::now() >= end {
                let _ = child.start_kill();
                let _ = child.wait().await;
                return Err(StartError::NoAnswer { log_tail: log_tail(&log) });
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// The process id MercuryChat started (the only one it ever stops).
    pub fn pid(&self) -> Option<u32> {
        self.child.id()
    }

    /// Ask Mercury to quit, then end it if it hasn't gone within 5 s.
    pub async fn stop(mut self) {
        #[cfg(target_os = "linux")]
        if let Some(pid) = self.child.id() {
            // SAFETY: a signal to the child we started and still hold.
            unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
            if tokio::time::timeout(QUIT_WITHIN, self.child.wait()).await.is_ok() {
                return;
            }
        }
        let _ = self.child.start_kill();
        let _ = tokio::time::timeout(QUIT_WITHIN, self.child.wait()).await;
    }

    /// Resolves when Mercury exits, with its exit code and log tail.
    pub async fn exited(&mut self) -> ExitInfo {
        let status = self.child.wait().await;
        ExitInfo { code: status.ok().and_then(|s| s.code()), log_tail: log_tail(&self.log) }
    }
}
