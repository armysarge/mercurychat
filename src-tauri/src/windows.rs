//! The two windows: a small splash that appears at once, and the main window,
//! which stays hidden until the UI has its first snapshot. The main window's
//! position and size are remembered (`window_state`).

use crate::window_state::{self, MonitorArea, Size, WindowRect};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{
    AppHandle, Manager, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent,
};

pub const SPLASH: &str = "splash";
pub const MAIN: &str = "main";
/// The splash stays at least this long, so the bar is seen rather than flashed.
/// `MC_SPLASH_MIN_MS` overrides it (the lab holds the splash to inspect it).
const SPLASH_MIN: Duration = Duration::from_millis(700);
/// If nothing has been shown by then, the splash says start-up is taking too
/// long and offers Close. `MC_STARTUP_TIMEOUT_MS` overrides it (lab and tests).
const STARTUP_TIMEOUT: Duration = Duration::from_secs(20);
/// Moves and resizes are saved once the window has been still for this long.
const SAVE_DELAY: Duration = Duration::from_millis(500);

/// How far start-up has got, as the splash shows it.
#[derive(Clone, Serialize)]
#[serde(tag = "stage", rename_all = "camelCase")]
pub enum Stage {
    /// Opening the station data.
    Opening,
    /// The core is running; the window is loading and the modem starting.
    Modem,
    /// Start-up failed: the splash shows the message and a Close button.
    Failed { message: String },
    /// Nothing was shown in time (a stalled core or window): the splash says so, with Close.
    TimedOut,
}

/// Shared by the splash page (it polls `splash_status`) and the hand-over.
pub struct Splash {
    stage: Mutex<Stage>,
    opened: Instant,
    revealed: AtomicBool,
}

pub fn open_splash(app: &AppHandle) -> tauri::Result<()> {
    app.manage(Splash { stage: Mutex::new(Stage::Opening), opened: Instant::now(), revealed: AtomicBool::new(false) });
    WebviewWindowBuilder::new(app, SPLASH, WebviewUrl::App("splash.html".into()))
        .title("MercuryChat")
        .inner_size(360.0, 180.0)
        .resizable(false)
        .decorations(false)
        .center()
        .always_on_top(true)
        .focused(true)
        .build()?;
    Ok(())
}

pub fn set_stage(app: &AppHandle, stage: Stage) {
    if let Some(s) = app.try_state::<Splash>() {
        *s.stage.lock().unwrap_or_else(|e| e.into_inner()) = stage;
    }
}

#[tauri::command]
pub fn splash_status(splash: tauri::State<'_, Splash>) -> Stage {
    splash.stage.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The splash's Close button after a start-up failure.
#[tauri::command]
pub fn quit_app(app: AppHandle) {
    app.exit(0);
}

/// The monitors' work areas, and the primary monitor's scale factor.
fn monitors(app: &AppHandle) -> (Vec<MonitorArea>, f64) {
    let primary = app.primary_monitor().ok().flatten();
    let areas = app
        .available_monitors()
        .unwrap_or_default()
        .iter()
        .map(|m| {
            let w = m.work_area();
            let is_primary = primary.as_ref().is_some_and(|p| p.position() == m.position() && p.size() == m.size());
            MonitorArea {
                x: w.position.x,
                y: w.position.y,
                width: w.size.width,
                height: w.size.height,
                primary: is_primary,
            }
        })
        .collect();
    (areas, primary.map(|p| p.scale_factor()).unwrap_or(1.0))
}

/// What is known about the window's rectangle, kept for the next start.
struct Tracker {
    dir: PathBuf,
    last: Mutex<WindowRect>,
    changes: AtomicU64,
    timer: AtomicBool,
}

impl Tracker {
    /// Read the window now and save it. A maximised window keeps its last
    /// normal rectangle; a minimised one is not recorded at all.
    fn capture(&self, win: &WebviewWindow) {
        if win.is_minimized().unwrap_or(false) {
            return;
        }
        let maximized = win.is_maximized().unwrap_or(false);
        let mut last = self.last.lock().unwrap_or_else(|e| e.into_inner());
        if !maximized {
            if let (Ok(p), Ok(s)) = (win.outer_position(), win.outer_size()) {
                if p.x > -30000 && s.width > 0 && s.height > 0 {
                    *last = WindowRect { x: p.x, y: p.y, width: s.width, height: s.height, maximized: false };
                }
            }
        }
        last.maximized = maximized;
        window_state::save(&self.dir, &last);
    }

    /// A move or resize happened: save once it has been quiet for a while.
    fn changed(self: &Arc<Self>, win: &WebviewWindow) {
        self.changes.fetch_add(1, Ordering::SeqCst);
        if self.timer.swap(true, Ordering::SeqCst) {
            return;
        }
        let (me, win) = (self.clone(), win.clone());
        std::thread::spawn(move || {
            loop {
                let seen = me.changes.load(Ordering::SeqCst);
                std::thread::sleep(SAVE_DELAY);
                if me.changes.load(Ordering::SeqCst) == seen {
                    break;
                }
            }
            me.timer.store(false, Ordering::SeqCst);
            me.capture(&win);
        });
    }
}

/// Build the main window hidden, put it where it was, and watch it.
pub fn create_main(app: &AppHandle, data_dir: &Path) -> tauri::Result<()> {
    let (areas, scale) = monitors(app);
    let default = Size {
        width: (window_state::DEFAULT_LOGICAL.width as f64 * scale).round() as u32,
        height: (window_state::DEFAULT_LOGICAL.height as f64 * scale).round() as u32,
    };
    let placement = window_state::restore_rect(window_state::load(data_dir), &areas, default);
    let r = placement.rect;
    let win = WebviewWindowBuilder::new(app, MAIN, WebviewUrl::default())
        .title("MercuryChat")
        .min_inner_size(window_state::MIN_SIZE.width as f64, window_state::MIN_SIZE.height as f64)
        .decorations(false)
        .shadow(false)
        .visible(false)
        .build()?;
    // `set_size` sets the inner size but the saved rectangle is the outer one
    // (Windows counts its invisible resize border in it): take the difference off.
    let (outer, inner) = (win.outer_size()?, win.inner_size()?);
    let (dw, dh) = (outer.width.saturating_sub(inner.width), outer.height.saturating_sub(inner.height));
    win.set_size(PhysicalSize::new(r.width.saturating_sub(dw), r.height.saturating_sub(dh)))?;
    win.set_position(PhysicalPosition::new(r.x, r.y))?;
    if placement.maximized {
        win.maximize()?;
    }
    let tracker = Arc::new(Tracker {
        dir: data_dir.to_path_buf(),
        last: Mutex::new(WindowRect { maximized: placement.maximized, ..r }),
        changes: AtomicU64::new(0),
        timer: AtomicBool::new(false),
    });
    let watched = win.clone();
    win.on_window_event(move |event| match event {
        WindowEvent::Moved(_) | WindowEvent::Resized(_) => tracker.changed(&watched),
        WindowEvent::CloseRequested { .. } => tracker.capture(&watched),
        _ => {}
    });
    Ok(())
}

/// Arm the start-up timeout. It lives apart from `boot`, so it also covers a
/// core that hangs or a UI that never reports ready.
pub fn arm_startup_timeout(app: &AppHandle) {
    let wait = std::env::var("MC_STARTUP_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .map(Duration::from_millis)
        .unwrap_or(STARTUP_TIMEOUT);
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(wait);
        let Some(splash) = app.try_state::<Splash>() else { return };
        if splash.revealed.load(Ordering::SeqCst) {
            return;
        }
        let mut stage = splash.stage.lock().unwrap_or_else(|e| e.into_inner());
        if !matches!(*stage, Stage::Failed { .. }) {
            *stage = Stage::TimedOut;
        }
    });
}

/// Show the main window and close the splash (once).
pub fn reveal(app: &AppHandle) {
    let Some(splash) = app.try_state::<Splash>() else { return };
    if splash.revealed.swap(true, Ordering::SeqCst) {
        return;
    }
    if let Some(main) = app.get_webview_window(MAIN) {
        let _ = main.show();
        let _ = main.set_focus();
    }
    if let Some(s) = app.get_webview_window(SPLASH) {
        let _ = s.close();
    }
}

/// The UI has its first snapshot: reveal the window once the splash has been up long enough.
pub fn reveal_when_due(app: &AppHandle) {
    let min = std::env::var("MC_SPLASH_MIN_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .map(Duration::from_millis)
        .unwrap_or(SPLASH_MIN);
    let wait = app.try_state::<Splash>().map(|s| min.saturating_sub(s.opened.elapsed())).unwrap_or_default();
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(wait);
        reveal(&app);
    });
}
