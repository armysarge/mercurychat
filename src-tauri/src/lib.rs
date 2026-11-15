//! MercuryChat desktop shell (M0): typed commands only; the UI has no raw
//! sockets, shell or filesystem access (docs/02 §2, §7).

use serde::Serialize;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tauri::Manager;

static STARTED: OnceLock<Instant> = OnceLock::new();

fn since_start_ms() -> u128 {
    STARTED.get().map(|t| t.elapsed().as_millis()).unwrap_or(0)
}

struct AppState {
    db_path: PathBuf,
    store: Mutex<mc_storage::Store>,
}

#[derive(Serialize)]
struct AppInfo {
    app_version: String,
    protocol: String,
    os: String,
    arch: String,
    db_path: String,
    db_schema: i64,
    startup_ms: u128,
}

#[tauri::command]
fn app_info(state: tauri::State<'_, AppState>) -> Result<AppInfo, String> {
    let schema = state.store.lock().unwrap().schema_version().map_err(|e| e.to_string())?;
    Ok(AppInfo {
        app_version: env!("CARGO_PKG_VERSION").into(),
        protocol: format!("MCH/{}.{} draft", mch_protocol::MAJOR, mch_protocol::MINOR),
        os: std::env::consts::OS.into(),
        arch: std::env::consts::ARCH.into(),
        db_path: state.db_path.display().to_string(),
        db_schema: schema,
        startup_ms: since_start_ms(),
    })
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

pub fn run() {
    STARTED.get_or_init(Instant::now);
    tauri::Builder::default()
        .setup(|app| {
            // Lab mode (MOD-02): MC_DATA_DIR gives each instance its own data root.
            let dir = match std::env::var_os("MC_DATA_DIR") {
                Some(d) => PathBuf::from(d),
                None => app.path().app_data_dir()?,
            };
            std::fs::create_dir_all(&dir)?;
            let db_path = dir.join("mercurychat.db");
            let store = mc_storage::Store::open(&db_path)?;
            app.manage(AppState { db_path, store: Mutex::new(store) });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![app_info, ui_ready, report_ipc, ping, modem_probe])
        .run(tauri::generate_context!())
        .expect("error while running MercuryChat");
}
