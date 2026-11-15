//! Diagnostics files (docs/02 §6, OPS-01): a daily-rolling log, retention of
//! 7 days or 25 MiB, and a crash report written by the panic hook. Message
//! bodies never go in the log; callers log state transitions and errors.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

pub const LOG_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 3600);
pub const LOG_MAX_TOTAL: u64 = 25 * 1024 * 1024;

const LOG_PREFIX: &str = "mercurychat.log";

/// Keeps the background log writer alive; logging stops when it is dropped.
pub struct LogGuard(#[allow(dead_code)] tracing_appender::non_blocking::WorkerGuard);

/// Log to `dir/mercurychat.log.YYYY-MM-DD` and to stderr, filtered by `MC_LOG`
/// (default `info`). On error the caller carries on with stderr only.
pub fn init_logging(dir: &Path) -> std::io::Result<LogGuard> {
    use tracing_subscriber::prelude::*;
    std::fs::create_dir_all(dir)?;
    let (file, guard) = tracing_appender::non_blocking(tracing_appender::rolling::daily(dir, LOG_PREFIX));
    let filter = tracing_subscriber::EnvFilter::try_from_env("MC_LOG")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    // A second init (tests, a restarted shell) keeps the first subscriber.
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer().with_ansi(false).with_writer(file))
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
        .try_init();
    Ok(LogGuard(guard))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PruneReport {
    pub removed: usize,
    pub kept_bytes: u64,
}

fn is_ours(name: &str) -> bool {
    name.starts_with(LOG_PREFIX) || (name.starts_with("crash-") && name.ends_with(".txt"))
}

/// Remove our diagnostics files older than `max_age`, then the oldest until
/// the rest fit in `max_total`. The newest log file (the one being written)
/// is never removed. Other files in `dir` are left alone.
pub fn prune_logs(dir: &Path, now: SystemTime, max_age: Duration, max_total: u64) -> std::io::Result<PruneReport> {
    let entries = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(PruneReport { removed: 0, kept_bytes: 0 }),
        Err(e) => return Err(e),
    };
    let mut files: Vec<(PathBuf, SystemTime, u64, bool)> = Vec::new();
    for e in entries {
        let e = e?;
        let name = e.file_name().to_string_lossy().into_owned();
        let meta = e.metadata()?;
        if meta.is_file() && is_ours(&name) {
            files.push((e.path(), meta.modified()?, meta.len(), name.starts_with(LOG_PREFIX)));
        }
    }
    files.sort_by_key(|f| f.1);
    let newest_log = files.iter().rposition(|f| f.3);

    let mut removed = 0;
    let mut keep = Vec::new();
    for (i, f) in files.into_iter().enumerate() {
        let protected = Some(i) == newest_log;
        let old = now.duration_since(f.1).unwrap_or_default() > max_age;
        if old && !protected {
            std::fs::remove_file(&f.0)?;
            removed += 1;
        } else {
            keep.push((f, protected));
        }
    }
    let mut total: u64 = keep.iter().map(|(f, _)| f.2).sum();
    for ((path, _, len, _), protected) in &keep {
        if total <= max_total {
            break;
        }
        if !protected {
            std::fs::remove_file(path)?;
            removed += 1;
            total -= len;
        }
    }
    Ok(PruneReport { removed, kept_bytes: total })
}

/// Write `dir/crash-YYYYMMDDTHHMMSSZ.txt` on any panic, then run the previous hook.
pub fn install_crash_report(dir: PathBuf, app_version: &'static str) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let message = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "(non-text panic payload)".into());
        let location = info.location().map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column())).unwrap_or_default();
        let now = chrono::Utc::now();
        let report = format!(
            "MercuryChat {app_version} crashed at {}\nos: {} {}\npanic: {message}\nat: {location}\n\nbacktrace:\n{}\n",
            now.to_rfc3339(),
            std::env::consts::OS,
            std::env::consts::ARCH,
            std::backtrace::Backtrace::force_capture()
        );
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join(format!("crash-{}.txt", now.format("%Y%m%dT%H%M%SZ"))), report);
        previous(info);
    }));
}
