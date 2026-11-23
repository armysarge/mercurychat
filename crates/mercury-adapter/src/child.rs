//! Programs MercuryChat starts for the built-in modem (Mercury, rigctld) live
//! no longer than the app (M5 §5.4): on Windows they join a job object that ends them when the
//! app's last handle to it closes (also on a crash); on Linux they get a
//! parent-death signal. Both are set up here, at spawn.

use std::path::Path;
use std::process::Stdio;
use tokio::process::{Child, Command};

/// A command for `program` with no console window, no stdio, and the
/// parent-death signal on Linux. Spawn it with [`spawn`].
pub fn command(program: &Path) -> Command {
    let mut cmd = Command::new(program);
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).kill_on_drop(true);
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(target_os = "linux")]
    {
        let parent = std::process::id() as libc::pid_t;
        // SAFETY: prctl, getppid and _exit are async-signal-safe.
        unsafe {
            cmd.pre_exec(move || {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                // The app died between fork and prctl: the signal never comes.
                if libc::getppid() != parent {
                    libc::_exit(1);
                }
                Ok(())
            });
        }
    }
    cmd
}

/// Spawn `cmd` and tie the child to the app.
pub fn spawn(cmd: &mut Command) -> std::io::Result<Child> {
    let child = cmd.spawn()?;
    #[cfg(windows)]
    if let Err(e) = job::assign(&child) {
        let mut child = child;
        let _ = child.start_kill();
        return Err(e);
    }
    Ok(child)
}

#[cfg(windows)]
mod job {
    use std::sync::OnceLock;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };

    /// The app's one job, created on first use and never closed: Windows
    /// closes it when the app ends, and that ends every process in it.
    fn job() -> std::io::Result<isize> {
        static JOB: OnceLock<Result<isize, i32>> = OnceLock::new();
        let r = JOB.get_or_init(|| unsafe {
            let h = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if h.is_null() {
                return Err(std::io::Error::last_os_error().raw_os_error().unwrap_or(0));
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = SetInformationJobObject(
                h,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            if ok == 0 {
                return Err(std::io::Error::last_os_error().raw_os_error().unwrap_or(0));
            }
            Ok(h as isize)
        });
        r.map_err(std::io::Error::from_raw_os_error)
    }

    pub fn assign(child: &tokio::process::Child) -> std::io::Result<()> {
        let job = job()?;
        let Some(h) = child.raw_handle() else {
            return Err(std::io::Error::other("the child has already exited"));
        };
        // SAFETY: both handles are open: the job for the app's life, the child's while `child` lives.
        if unsafe { AssignProcessToJobObject(job as _, h as _) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }
}
