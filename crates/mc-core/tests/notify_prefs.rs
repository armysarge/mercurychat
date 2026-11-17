//! M3a Task 12b: the desktop notification preferences are app settings (not
//! per profile), checked by the core and kept across a restart. The window
//! decides what notifies (apps/desktop/src/notify.ts); the core only keeps
//! the choices.

use mc_core::app::{spawn_app, AppCommand, AppError, AppHandle, AppOptions, CommandOk, NotifyPrefs};
use mc_storage::Store;
use std::path::Path;
use std::time::Duration;

fn opts(root: &Path) -> AppOptions {
    AppOptions { sim_root: Some(root.join("sim")), stations_root: Some(root.join("stations")), ..AppOptions::default() }
}

async fn prefs(h: &AppHandle) -> NotifyPrefs {
    match h.call(AppCommand::Snapshot).await.unwrap() {
        CommandOk::Snapshot(s) => s.notify_prefs,
        other => panic!("{other:?}"),
    }
}

async fn set(h: &AppHandle, p: NotifyPrefs) -> Result<CommandOk, AppError> {
    h.call(AppCommand::SetNotifyPrefs { prefs: Box::new(p) }).await
}

#[tokio::test(flavor = "multi_thread")]
async fn defaults_follow_the_plan() {
    let dir = tempfile::tempdir().unwrap();
    let h = spawn_app(Store::open_in_memory().unwrap(), opts(dir.path()));
    let p = prefs(&h).await;
    assert!(
        p.enabled && p.message && p.incoming && p.call_failed && p.delivery && p.heard_favourite && p.fault && p.sound
    );
    assert!(!p.heard_any && !p.beacon_refused && !p.hide_text && !p.lab_enabled);
    assert_eq!((p.quiet_start.as_deref(), p.quiet_end.as_deref()), (None, None));
}

#[tokio::test(flavor = "multi_thread")]
async fn prefs_persist_across_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("app.db");
    let p = NotifyPrefs {
        heard_any: true,
        hide_text: true,
        quiet_start: Some("22:00".into()),
        quiet_end: Some("06:30".into()),
        ..NotifyPrefs::default()
    };
    {
        let h = spawn_app(Store::open(&db).unwrap(), opts(dir.path()));
        assert_eq!(set(&h, p.clone()).await.unwrap(), CommandOk::Done);
        assert_eq!(prefs(&h).await, p);
    }
    tokio::time::sleep(Duration::from_millis(200)).await; // the first service has stopped
    let h = spawn_app(Store::open(&db).unwrap(), opts(dir.path()));
    assert_eq!(prefs(&h).await, p);
}

#[tokio::test(flavor = "multi_thread")]
async fn quiet_hours_must_be_hh_mm_and_come_as_a_pair() {
    let dir = tempfile::tempdir().unwrap();
    let h = spawn_app(Store::open_in_memory().unwrap(), opts(dir.path()));
    for (start, end) in [
        (Some("25:00"), Some("06:00")),
        (Some("7:00"), Some("08:00")),
        (Some("22:00"), None),
        (Some("22:60"), Some("06:00")),
    ] {
        let p = NotifyPrefs {
            quiet_start: start.map(Into::into),
            quiet_end: end.map(Into::into),
            ..NotifyPrefs::default()
        };
        match set(&h, p).await {
            Err(AppError::Validation { errors }) => {
                assert!(errors.iter().any(|e| e.field.starts_with("quiet")), "{errors:?}")
            }
            other => panic!("{start:?}-{end:?} accepted: {other:?}"),
        }
    }
    assert_eq!(prefs(&h).await, NotifyPrefs::default(), "a refused change stores nothing");
}
