//! Where the main window sits and how big it is, remembered between runs.
//!
//! The placement rules are pure functions over plain numbers (physical pixels,
//! the monitors' work areas), so they are tested without a window system. The
//! saved state itself is a small JSON file in the data root (`window.json`),
//! written from Rust: the window is placed before the UI exists, so it cannot
//! wait for the core's settings store.

use serde::{Deserialize, Serialize};
use std::path::Path;

/// The smallest the window is restored at, unless the screen itself is smaller.
pub const MIN_SIZE: Size = Size { width: 960, height: 640 };
/// The size on first run, in logical pixels (scaled by the primary monitor).
pub const DEFAULT_LOGICAL: Size = Size { width: 1180, height: 760 };
/// A saved rectangle is restored only when this much of it (percent) lies on one monitor.
const VISIBLE_PERCENT: i64 = 80;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Size {
    pub width: u32,
    pub height: u32,
}

/// The window's outer rectangle in physical pixels, and whether it was maximised.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    #[serde(default)]
    pub maximized: bool,
}

/// One connected monitor's work area (the screen minus the taskbar), physical pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MonitorArea {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub primary: bool,
}

/// What to do with the window at start-up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placement {
    /// Where to put it (when maximised: where it returns to when restored).
    pub rect: WindowRect,
    /// Maximise it after placing it.
    pub maximized: bool,
    /// The saved rectangle was used (false: centred at the default size).
    pub restored: bool,
}

/// Fit `rect` into `area`: never larger than the area, never smaller than
/// [`MIN_SIZE`] unless the area itself is, and moved so it lies inside.
pub fn clamp_to(area: MonitorArea, rect: WindowRect) -> WindowRect {
    let fit = |wanted: u32, room: u32, minimum: u32| wanted.max(minimum.min(room)).min(room);
    let width = fit(rect.width, area.width, MIN_SIZE.width);
    let height = fit(rect.height, area.height, MIN_SIZE.height);
    let place = |at: i32, start: i32, room: u32, size: u32| {
        let last = start as i64 + room as i64 - size as i64;
        (at as i64).clamp(start as i64, last) as i32
    };
    WindowRect {
        x: place(rect.x, area.x, area.width, width),
        y: place(rect.y, area.y, area.height, height),
        width,
        height,
        maximized: rect.maximized,
    }
}

/// How much of `rect` lies inside `area`, in square pixels.
fn overlap(area: &MonitorArea, rect: &WindowRect) -> i64 {
    let span = |a: i32, alen: u32, r: i32, rlen: u32| {
        let lo = (a as i64).max(r as i64);
        let hi = (a as i64 + alen as i64).min(r as i64 + rlen as i64);
        (hi - lo).max(0)
    };
    span(area.x, area.width, rect.x, rect.width) * span(area.y, area.height, rect.y, rect.height)
}

/// The placement for a saved rectangle on the monitors that are connected now.
/// `default` is the first-run size (physical pixels).
pub fn restore_rect(saved: Option<WindowRect>, monitors: &[MonitorArea], default: Size) -> Placement {
    if let Some(saved) = saved {
        let whole = saved.width as i64 * saved.height as i64;
        let home = monitors.iter().find(|m| whole > 0 && overlap(m, &saved) * 100 >= whole * VISIBLE_PERCENT);
        if let Some(monitor) = home {
            let rect = clamp_to(*monitor, saved);
            return Placement { rect, maximized: saved.maximized, restored: true };
        }
    }
    let first_run = WindowRect { x: 0, y: 0, width: default.width, height: default.height, maximized: false };
    let Some(primary) = monitors.iter().find(|m| m.primary).or(monitors.first()) else {
        return Placement { rect: first_run, maximized: false, restored: false };
    };
    let fitted = clamp_to(*primary, first_run);
    let centred = WindowRect {
        x: primary.x + ((primary.width - fitted.width) / 2) as i32,
        y: primary.y + ((primary.height - fitted.height) / 2) as i32,
        ..fitted
    };
    Placement { rect: centred, maximized: false, restored: false }
}

/// Read the saved state; a missing or unreadable file is no state.
pub fn load(dir: &Path) -> Option<WindowRect> {
    let text = std::fs::read_to_string(dir.join("window.json")).ok()?;
    serde_json::from_str(&text).ok()
}

/// Save the state (best effort: a window must never fail to close over this).
pub fn save(dir: &Path, rect: &WindowRect) {
    let Ok(text) = serde_json::to_string(rect) else { return };
    let tmp = dir.join("window.json.tmp");
    if std::fs::write(&tmp, text).is_ok() {
        let _ = std::fs::rename(&tmp, dir.join("window.json"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEFAULT: Size = Size { width: 1180, height: 760 };

    fn area(x: i32, y: i32, width: u32, height: u32, primary: bool) -> MonitorArea {
        MonitorArea { x, y, width, height, primary }
    }
    fn rect(x: i32, y: i32, width: u32, height: u32) -> WindowRect {
        WindowRect { x, y, width, height, maximized: false }
    }
    /// A 1920x1080 screen with a 40 px taskbar.
    fn main_screen() -> MonitorArea {
        area(0, 0, 1920, 1040, true)
    }

    #[test]
    fn off_screen_rect_recentres() {
        let p = restore_rect(Some(rect(5000, 300, 1180, 760)), &[main_screen()], DEFAULT);
        assert!(!p.restored);
        assert_eq!((p.rect.x, p.rect.y, p.rect.width, p.rect.height), (370, 140, 1180, 760));
        // Mostly off screen counts as off screen: only 40 % of it is visible.
        let p = restore_rect(Some(rect(1500, 100, 1000, 700)), &[main_screen()], DEFAULT);
        assert!(!p.restored);
    }

    #[test]
    fn oversized_rect_is_clamped_to_work_area() {
        // 91 % of it is on the screen, so it is restored, but cut to fit.
        let p = restore_rect(Some(rect(0, 0, 2000, 1100)), &[main_screen()], DEFAULT);
        assert!(p.restored);
        assert_eq!((p.rect.x, p.rect.y, p.rect.width, p.rect.height), (0, 0, 1920, 1040));
        let c = clamp_to(main_screen(), rect(-50, -20, 4000, 3000));
        assert_eq!((c.x, c.y, c.width, c.height), (0, 0, 1920, 1040));
        // Pushed back inside when it sticks out to the right and below.
        let c = clamp_to(main_screen(), rect(1200, 700, 1180, 760));
        assert_eq!((c.x, c.y, c.width, c.height), (740, 280, 1180, 760));
    }

    #[test]
    fn removed_monitor_recentres_on_primary() {
        // It was on a second screen to the right; only the primary is left.
        let p = restore_rect(Some(rect(2000, 100, 1200, 800)), &[main_screen()], DEFAULT);
        assert!(!p.restored);
        assert_eq!((p.rect.x, p.rect.y, p.rect.width, p.rect.height), (370, 140, 1180, 760));
        // The primary is the one flagged, wherever it is in the list.
        let left = area(-1280, 0, 1280, 984, false);
        let p = restore_rect(Some(rect(9000, 0, 1000, 700)), &[left, main_screen()], DEFAULT);
        assert!(p.rect.x >= 0 && p.rect.x + p.rect.width as i32 <= 1920);
    }

    #[test]
    fn negative_coordinates_on_left_monitor_are_kept() {
        let left = area(-1920, -100, 1920, 1040, false);
        let saved = rect(-1700, 50, 1200, 800);
        let p = restore_rect(Some(saved), &[main_screen(), left], DEFAULT);
        assert!(p.restored);
        assert_eq!(p.rect, saved);
        assert!(!p.maximized);
    }

    #[test]
    fn first_run_centres_default_size() {
        let p = restore_rect(None, &[main_screen()], DEFAULT);
        assert!(!p.restored && !p.maximized);
        assert_eq!((p.rect.x, p.rect.y, p.rect.width, p.rect.height), (370, 140, 1180, 760));
        // The default never exceeds a small screen.
        let small = area(0, 0, 1024, 728, true);
        let p = restore_rect(None, &[small], DEFAULT);
        assert_eq!((p.rect.x, p.rect.y, p.rect.width, p.rect.height), (0, 0, 1024, 728));
    }

    #[test]
    fn smaller_new_resolution_shrinks_and_keeps_the_window_on_screen() {
        // Saved on a big screen, now a 1366x728 laptop panel: the window is mostly
        // off it, so it recentres at the default size cut to the screen.
        let laptop = area(0, 0, 1366, 728, true);
        let p = restore_rect(Some(rect(300, 100, 1800, 1000)), &[laptop], DEFAULT);
        assert_eq!((p.rect.x, p.rect.y, p.rect.width, p.rect.height), (93, 0, 1180, 728));
        // A window that mostly fits is kept, cut to fit.
        let p = restore_rect(Some(rect(0, 0, 1400, 760)), &[laptop], DEFAULT);
        assert!(p.restored);
        assert_eq!((p.rect.width, p.rect.height), (1366, 728));
    }

    #[test]
    fn maximised_state_comes_back_with_the_normal_rectangle() {
        let saved = WindowRect { x: 100, y: 80, width: 1300, height: 800, maximized: true };
        let p = restore_rect(Some(saved), &[main_screen()], DEFAULT);
        assert!(p.restored && p.maximized);
        assert_eq!(p.rect, saved);
        // On a monitor that is gone it is not maximised on the wrong screen's size.
        let p = restore_rect(Some(WindowRect { x: 4000, ..saved }), &[main_screen()], DEFAULT);
        assert!(!p.restored && !p.maximized);
    }

    #[test]
    fn minimum_size_holds_unless_the_screen_is_smaller() {
        let c = clamp_to(main_screen(), rect(10, 10, 300, 200));
        assert_eq!((c.width, c.height), (960, 640));
        let tiny = area(0, 0, 800, 600, true);
        let c = clamp_to(tiny, rect(10, 10, 300, 200));
        assert_eq!((c.x, c.y, c.width, c.height), (0, 0, 800, 600));
    }

    #[test]
    fn no_monitors_and_empty_rect_fall_back_safely() {
        let p = restore_rect(Some(rect(0, 0, 0, 0)), &[main_screen()], DEFAULT);
        assert!(!p.restored);
        let p = restore_rect(Some(rect(10, 10, 900, 700)), &[], DEFAULT);
        assert!(!p.restored);
        assert_eq!((p.rect.width, p.rect.height), (1180, 760));
    }

    #[test]
    fn saved_state_round_trips_and_a_damaged_file_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load(dir.path()), None);
        let r = WindowRect { x: -20, y: 5, width: 1000, height: 700, maximized: true };
        save(dir.path(), &r);
        assert_eq!(load(dir.path()), Some(r));
        std::fs::write(dir.path().join("window.json"), "{ not json").unwrap();
        assert_eq!(load(dir.path()), None);
    }
}
