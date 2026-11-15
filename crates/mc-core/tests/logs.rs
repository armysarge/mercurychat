//! docs/02 §6: diagnostics are kept 7 days or 25 MiB; crashes leave a report.

use filetime::{set_file_mtime, FileTime};
use mc_core::logs::{install_crash_report, prune_logs, LOG_MAX_AGE, LOG_MAX_TOTAL};
use std::path::Path;
use std::time::{Duration, SystemTime};

const DAY: u64 = 24 * 3600;

fn file(dir: &Path, name: &str, bytes: usize, age_secs: u64) {
    let p = dir.join(name);
    std::fs::write(&p, vec![b'x'; bytes]).unwrap();
    set_file_mtime(&p, FileTime::from_system_time(SystemTime::now() - Duration::from_secs(age_secs))).unwrap();
}

#[test]
fn retention_matches_the_spec() {
    assert_eq!(LOG_MAX_AGE, Duration::from_secs(7 * DAY));
    assert_eq!(LOG_MAX_TOTAL, 25 * 1024 * 1024);
}

#[test]
fn old_files_are_removed() {
    let d = tempfile::tempdir().unwrap();
    file(d.path(), "mercurychat.log.2026-11-01", 10, 8 * DAY);
    file(d.path(), "crash-20261101T000000Z.txt", 10, 8 * DAY);
    file(d.path(), "mercurychat.log.2026-11-14", 10, DAY);
    let r = prune_logs(d.path(), SystemTime::now(), LOG_MAX_AGE, LOG_MAX_TOTAL).unwrap();
    assert_eq!(r.removed, 2);
    assert!(!d.path().join("mercurychat.log.2026-11-01").exists());
    assert!(!d.path().join("crash-20261101T000000Z.txt").exists());
    assert!(d.path().join("mercurychat.log.2026-11-14").exists());
    assert_eq!(r.kept_bytes, 10);
}

#[test]
fn total_cap_removes_oldest_first() {
    let d = tempfile::tempdir().unwrap();
    file(d.path(), "mercurychat.log.a", 10, 3 * 3600);
    file(d.path(), "mercurychat.log.b", 10, 2 * 3600);
    file(d.path(), "mercurychat.log.c", 10, 3600);
    let r = prune_logs(d.path(), SystemTime::now(), LOG_MAX_AGE, 25).unwrap();
    assert_eq!(r.removed, 1);
    assert!(!d.path().join("mercurychat.log.a").exists());
    assert!(d.path().join("mercurychat.log.b").exists() && d.path().join("mercurychat.log.c").exists());
}

#[test]
fn newest_log_is_never_removed() {
    let d = tempfile::tempdir().unwrap();
    file(d.path(), "mercurychat.log.2026-11-15", 100, 30 * DAY);
    let r = prune_logs(d.path(), SystemTime::now(), LOG_MAX_AGE, 10).unwrap();
    assert_eq!(r.removed, 0);
    assert!(d.path().join("mercurychat.log.2026-11-15").exists());
}

#[test]
fn unrelated_files_are_untouched() {
    let d = tempfile::tempdir().unwrap();
    file(d.path(), "notes.txt", 10, 30 * DAY);
    file(d.path(), "mercurychat.log.x", 10, 0);
    prune_logs(d.path(), SystemTime::now(), LOG_MAX_AGE, LOG_MAX_TOTAL).unwrap();
    assert!(d.path().join("notes.txt").exists());
}

#[test]
fn missing_directory_prunes_nothing() {
    let d = tempfile::tempdir().unwrap();
    let r = prune_logs(&d.path().join("absent"), SystemTime::now(), LOG_MAX_AGE, LOG_MAX_TOTAL).unwrap();
    assert_eq!((r.removed, r.kept_bytes), (0, 0));
}

#[test]
fn crash_report_is_written() {
    let d = tempfile::tempdir().unwrap();
    install_crash_report(d.path().to_path_buf(), "9.9.9-test");
    let _ = std::panic::catch_unwind(|| panic!("boom at the test"));
    let reports: Vec<_> = std::fs::read_dir(d.path())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.file_name().unwrap().to_string_lossy().starts_with("crash-"))
        .collect();
    assert_eq!(reports.len(), 1, "{reports:?}");
    let text = std::fs::read_to_string(&reports[0]).unwrap();
    assert!(text.contains("boom at the test"), "{text}");
    assert!(text.contains("9.9.9-test"), "{text}");
    assert!(text.contains("logs.rs"), "location: {text}");
}
