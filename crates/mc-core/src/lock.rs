//! Exclusive ownership (MOD-02): one MercuryChat process per station data root
//! and per modem endpoint, so two instances never drive the same modem or
//! write the same database. Lab mode is two instances with distinct data roots
//! and endpoints, which lock independently.
//!
//! The operating system lock decides, not the lock file's existence: a file
//! left behind by a crashed process does not block the next start. The
//! holder's pid is written to a `.pid` file beside the lock (on Windows a
//! locked file cannot be read through another handle) and is only a hint for
//! the error message.

use std::fs::{File, OpenOptions, TryLockError};
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum LockError {
    #[error("another MercuryChat instance is using this station's data ({}){}", path.display(), pid_suffix(*holder))]
    DataRootBusy { path: PathBuf, holder: Option<u32> },
    #[error("another MercuryChat instance is using the modem at {endpoint}{}", pid_suffix(*holder))]
    EndpointBusy { endpoint: String, holder: Option<u32> },
    #[error("lock file: {0}")]
    Io(#[from] std::io::Error),
}

fn pid_suffix(holder: Option<u32>) -> String {
    holder.map(|p| format!(" (pid {p})")).unwrap_or_default()
}

/// A held lock; dropping it releases the lock.
#[derive(Debug)]
pub struct LockFile {
    _file: File,
}

enum Taken {
    Held(LockFile),
    Busy(Option<u32>),
}

fn take(lock_path: &Path) -> Result<Taken, std::io::Error> {
    if let Some(dir) = lock_path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let file = OpenOptions::new().create(true).truncate(false).read(true).write(true).open(lock_path)?;
    let pid_path = pid_path(lock_path);
    match file.try_lock() {
        Ok(()) => {
            let mut pid = File::create(&pid_path)?;
            write!(pid, "{}", std::process::id())?;
            Ok(Taken::Held(LockFile { _file: file }))
        }
        Err(TryLockError::WouldBlock) => {
            let holder = std::fs::read_to_string(&pid_path).ok().and_then(|s| s.trim().parse().ok());
            Ok(Taken::Busy(holder))
        }
        Err(TryLockError::Error(e)) => Err(e),
    }
}

fn pid_path(lock_path: &Path) -> PathBuf {
    let mut name = lock_path.file_name().unwrap_or_default().to_os_string();
    name.push(".pid");
    lock_path.with_file_name(name)
}

/// Lock `root/station.lock` for this process.
pub fn lock_data_root(root: &Path) -> Result<LockFile, LockError> {
    match take(&root.join("station.lock"))? {
        Taken::Held(l) => Ok(l),
        Taken::Busy(holder) => Err(LockError::DataRootBusy { path: root.to_path_buf(), holder }),
    }
}

/// Lock the modem endpoint `host:control_port` in a directory shared by every
/// MercuryChat instance of this user (not a per-instance data root).
pub fn lock_endpoint(locks_dir: &Path, host: &str, control_port: u16) -> Result<LockFile, LockError> {
    let safe: String =
        host.chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '_' }).collect();
    match take(&locks_dir.join(format!("endpoint-{safe}-{control_port}.lock")))? {
        Taken::Held(l) => Ok(l),
        Taken::Busy(holder) => Err(LockError::EndpointBusy { endpoint: format!("{host}:{control_port}"), holder }),
    }
}
