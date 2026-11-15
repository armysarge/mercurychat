//! MercuryChat desktop shell: typed commands only; the UI has no raw
//! sockets, shell or filesystem access (docs/02 §2, §7). Every UI action goes
//! through `app_call` to the core's service actor; its events arrive on
//! `mc://event`.

mod startup;

use mc_core::app::{spawn_app, AppCommand, AppError, AppHandle as Core, AppOptions, CommandOk, EventEnvelope, Since};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use tauri::{Emitter, Manager};
use tokio::sync::broadcast;

/// The one event channel the UI listens on.
pub const EVENT_CHANNEL: &str = "mc://event";

static STARTED: OnceLock<Instant> = OnceLock::new();

fn since_start_ms() -> u128 {
    STARTED.get().map(|t| t.elapsed().as_millis()).unwrap_or(0)
}

/// What start-up produced: a running core, or the reason it could not start
/// (shown in the window; the app exits when the operator closes it).
enum Shell {
    Running(Box<Running>),
    Failed(String),
}

struct Running {
    core: Core,
    data_root: PathBuf,
    db_path: PathBuf,
    /// Endpoint locks (MOD-02), shared by every instance on this machine.
    locks_dir: PathBuf,
    schema: i64,
    _log: Option<mc_core::logs::LogGuard>,
    _lock: mc_core::lock::LockFile,
}

impl Shell {
    fn core(&self) -> Result<Core, AppError> {
        match self {
            Shell::Running(r) => Ok(r.core.clone()),
            Shell::Failed(message) => Err(AppError::Internal { message: message.clone() }),
        }
    }
}

#[derive(Serialize)]
struct AppInfo {
    app_version: String,
    protocol: String,
    os: String,
    arch: String,
    data_root: String,
    db_path: String,
    db_schema: i64,
    locks_dir: String,
    startup_ms: u128,
    /// Set when start-up failed; the UI then shows only this.
    startup_error: Option<String>,
}

#[tauri::command]
fn app_info(shell: tauri::State<'_, Shell>) -> AppInfo {
    let (data_root, db_path, db_schema, locks_dir, startup_error) = match shell.inner() {
        Shell::Running(r) => (
            r.data_root.display().to_string(),
            r.db_path.display().to_string(),
            r.schema,
            r.locks_dir.display().to_string(),
            None,
        ),
        Shell::Failed(m) => (String::new(), String::new(), 0, String::new(), Some(m.clone())),
    };
    AppInfo {
        app_version: env!("CARGO_PKG_VERSION").into(),
        protocol: format!("MCH/{}.{} draft", mch_protocol::MAJOR, mch_protocol::MINOR),
        os: std::env::consts::OS.into(),
        arch: std::env::consts::ARCH.into(),
        data_root,
        db_path,
        db_schema,
        locks_dir,
        startup_ms: since_start_ms(),
        startup_error,
    }
}

/// Every UI action (docs/02 §2): one typed command, one typed reply.
#[tauri::command]
async fn app_call(shell: tauri::State<'_, Shell>, cmd: AppCommand) -> Result<CommandOk, AppError> {
    let core = shell.core()?;
    core.call(cmd).await
}

/// Events after `seq`, or `snapshotNeeded` when the ring no longer holds them.
#[tauri::command]
async fn events_since(shell: tauri::State<'_, Shell>, seq: u64) -> Result<Since, AppError> {
    let core = shell.core()?;
    core.events_since(seq).await
}

/// Relay core events to the window. A lagging relay drops events; the UI
/// sees the gap in `seq` and asks `events_since`.
fn forward_events(app: tauri::AppHandle, mut rx: broadcast::Receiver<EventEnvelope>) {
    tauri::async_runtime::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(env) => {
                    let _ = app.emit(EVENT_CHANNEL, &env);
                }
                Err(broadcast::error::RecvError::Lagged(n)) => tracing::warn!(missed = n, "UI event relay lagged"),
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

/// Called by the UI after its first data render. Prints a parseable line for
/// the Pi/CI smoke scripts and optionally exits (MC_EXIT_AFTER_READY_MS).
/// Returns true when MC_SELF_MEASURE is set: the UI then measures IPC
/// round trips and reports them through `report_ipc` (unattended Pi runs).
#[tauri::command]
fn ui_ready(app: tauri::AppHandle) -> bool {
    let ms = since_start_ms();
    println!("MC_UI_READY_MS={ms}");
    let self_measure = std::env::var_os("MC_SELF_MEASURE").is_some();
    if let Ok(v) = std::env::var("MC_EXIT_AFTER_READY_MS") {
        let delay = v.parse::<u64>().unwrap_or(0);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(delay));
            app.exit(0);
        });
    }
    self_measure
}

#[tauri::command]
fn report_ipc(p50: f64, p95: f64, max: f64, samples: u32) {
    println!("MC_IPC_MS p50={p50:.3} p95={p95:.3} max={max:.3} n={samples}");
}

#[tauri::command]
fn ping() {}

#[derive(Serialize)]
struct ProbeResult {
    ok: bool,
    summary: String,
    lines: Vec<String>,
}

/// Read-only modem probe: attach the control/data pair, ask VERSION and
/// BUFFER, record 3 s of status lines, detach. Sends no MYCALL/LISTEN, so it
/// does not change the modem's configuration.
#[tauri::command]
async fn modem_probe(host: String, port: u16) -> ProbeResult {
    use mercury_adapter::{AdapterEvent, Endpoint, MercuryClient, Timeouts};
    if port == 0 || port == u16::MAX || host.trim().is_empty() || host.len() > 253 {
        return ProbeResult { ok: false, summary: "invalid host or port".into(), lines: vec![] };
    }
    let ep = Endpoint::base(host.trim(), port);
    let mut att = match MercuryClient::attach(&ep, Timeouts::default()).await {
        Ok(a) => a,
        Err(e) => return ProbeResult { ok: false, summary: e.to_string(), lines: vec![] },
    };
    let _ = att.client.query("VERSION").await;
    let _ = att.client.query("BUFFER").await;
    let mut lines = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while let Ok(Some(ev)) = tokio::time::timeout_at(deadline, att.events.recv()).await {
        match ev {
            AdapterEvent::RawLine(l) => lines.push(l),
            AdapterEvent::ControlClosed => {
                lines.push("(control connection closed by modem)".into());
                break;
            }
            _ => {}
        }
    }
    let vara = lines.iter().any(|l| l.starts_with("VARA"));
    ProbeResult {
        ok: true,
        summary: format!(
            "Attached control {} + data {}. {}",
            ep.control_port,
            ep.data_port,
            if vara {
                "VERSION answered with the VARA compatibility string (not Mercury's identity)."
            } else {
                "No VERSION answer within 3 s."
            }
        ),
        lines,
    }
}

/// Start-up order (docs/02 §6): data-root lock, logging, log pruning, crash
/// report, store (all in `startup::prepare`), then the service actor.
fn start(app: &tauri::App) -> Result<Shell, Box<dyn std::error::Error>> {
    // Lab mode (MOD-02): MC_DATA_DIR gives each instance its own data root.
    let root = match std::env::var_os("MC_DATA_DIR") {
        Some(d) => PathBuf::from(d),
        None => app.path().app_data_dir()?,
    };
    let p = match startup::prepare(&root) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("MercuryChat cannot start: {e}");
            return Ok(Shell::Failed(e.to_string()));
        }
    };
    // Endpoint locks ignore MC_DATA_DIR so lab instances on one machine share them.
    let locks_dir = app.path().app_local_data_dir()?.join("locks");
    std::fs::create_dir_all(&locks_dir)?;
    let startup::Prepared { root, db_path, schema, store, log, lock } = p;
    let opts = AppOptions { sim_root: Some(root.clone()), ..AppOptions::default() };
    // spawn_app needs a runtime context; Tauri's runtime keeps the actor alive.
    let core = tauri::async_runtime::block_on(async move { spawn_app(store, opts) });
    forward_events(app.handle().clone(), core.subscribe());
    Ok(Shell::Running(Box::new(Running { core, data_root: root, db_path, locks_dir, schema, _log: log, _lock: lock })))
}

pub fn run() {
    STARTED.get_or_init(Instant::now);
    tauri::Builder::default()
        .setup(|app| {
            let shell = start(app)?;
            app.manage(shell);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            app_info,
            app_call,
            events_since,
            ui_ready,
            report_ipc,
            ping,
            modem_probe
        ])
        .run(tauri::generate_context!())
        .expect("error while running MercuryChat");
}
