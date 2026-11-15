//! Start-up (docs/02 §6, MOD-02): lock the data root, start logging, prune
//! old logs, install the crash report, then open the store.

use mc_core::lock::{lock_data_root, LockError, LockFile};
use mc_core::logs::{init_logging, install_crash_report, prune_logs, LogGuard, LOG_MAX_AGE, LOG_MAX_TOTAL};
use mc_storage::{StorageError, Store};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Debug, thiserror::Error)]
pub enum StartupError {
    /// Another instance holds this data root; the operator closes the window.
    #[error("{0}")]
    Busy(LockError),
    #[error("cannot create the data folder {}: {source}", path.display())]
    Io { path: PathBuf, source: std::io::Error },
    /// A failed migration names the backup that still holds the old data.
    #[error("cannot open the station database: {0}")]
    Storage(StorageError),
}

/// Everything start-up hands to the shell. `lock` and `log` must live as
/// long as the app: dropping them releases the data root and stops the log.
pub struct Prepared {
    pub root: PathBuf,
    pub db_path: PathBuf,
    pub schema: i64,
    pub store: Store,
    pub log: Option<LogGuard>,
    pub lock: LockFile,
}

pub fn prepare(root: &Path) -> Result<Prepared, StartupError> {
    std::fs::create_dir_all(root).map_err(|source| StartupError::Io { path: root.to_path_buf(), source })?;
    let lock = lock_data_root(root).map_err(StartupError::Busy)?;
    let logs = root.join("logs");
    let log = match init_logging(&logs) {
        Ok(g) => Some(g),
        Err(e) => {
            eprintln!("MercuryChat: cannot log to {} ({e}); logging to stderr only", logs.display());
            None
        }
    };
    match prune_logs(&logs, SystemTime::now(), LOG_MAX_AGE, LOG_MAX_TOTAL) {
        Ok(r) if r.removed > 0 => tracing::info!(removed = r.removed, kept_bytes = r.kept_bytes, "pruned old logs"),
        Ok(_) => {}
        Err(e) => tracing::warn!("log pruning failed: {e}"),
    }
    install_crash_report(logs, env!("CARGO_PKG_VERSION"));
    let db_path = root.join("mercurychat.db");
    let store = Store::open(&db_path).map_err(StartupError::Storage)?;
    let schema = store.schema_version().map_err(StartupError::Storage)?;
    tracing::info!(root = %root.display(), schema, "station data opened");
    Ok(Prepared { root: root.to_path_buf(), db_path, schema, store, log, lock })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mc_core::lock::LockError;

    #[test]
    fn second_instance_reports_busy() {
        let dir = tempfile::tempdir().unwrap();
        let _first = prepare(dir.path()).unwrap();
        match prepare(dir.path()) {
            Err(StartupError::Busy(LockError::DataRootBusy { .. })) => {}
            Err(other) => panic!("expected busy, got {other}"),
            Ok(_) => panic!("a second instance on the same data root must be refused"),
        }
    }

    #[test]
    fn prepare_lays_out_the_data_root() {
        let dir = tempfile::tempdir().unwrap();
        let p = prepare(dir.path()).unwrap();
        assert_eq!(p.db_path, dir.path().join("mercurychat.db"));
        assert!(p.db_path.is_file());
        assert!(dir.path().join("logs").is_dir());
        assert_eq!(p.schema, mc_storage::MIGRATIONS.last().unwrap().to);
    }
}
