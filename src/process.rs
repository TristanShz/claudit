//! Process control, the part of claudit that differs most between
//! platforms: starting a child that outlives the terminal (and the Claude
//! Code session) that started it, and stopping another process.

use std::process::Command;

use anyhow::Result;

/// Makes `command` start detached: in a new session on Unix (`setsid`, so
/// closing the terminal does not stop it), in a new process group with no
/// console on Windows. Its standard streams are left to the caller.
pub fn detach(command: &mut Command) -> &mut Command {
    platform::detach(command)
}

/// Asks process `pid` to stop: SIGTERM on Unix, which the dashboard answers
/// with a graceful shutdown. Windows has no such request: the process is
/// terminated at once (every archive write is a SQLite transaction, so
/// nothing is left half-written). A process already gone is not an error.
pub fn terminate(pid: u32) -> Result<()> {
    platform::terminate(pid)
}

/// Stops process `pid` at once: SIGKILL on Unix, `TerminateProcess` on
/// Windows. A process already gone is not an error.
pub fn kill(pid: u32) -> Result<()> {
    platform::kill(pid)
}

#[cfg(unix)]
mod platform {
    use std::io;
    use std::os::unix::process::CommandExt;
    use std::process::Command;

    use anyhow::{Context, Result};

    pub fn detach(command: &mut Command) -> &mut Command {
        // SAFETY: `setsid` is async-signal-safe, as required between fork
        // and exec; it only fails if the child already leads a process
        // group, which a freshly forked child never does.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            })
        }
    }

    pub fn terminate(pid: u32) -> Result<()> {
        signal(pid, libc::SIGTERM)
    }

    pub fn kill(pid: u32) -> Result<()> {
        signal(pid, libc::SIGKILL)
    }

    fn signal(pid: u32, signal: libc::c_int) -> Result<()> {
        let pid = libc::pid_t::try_from(pid).context("pid out of range")?;
        // SAFETY: `kill` has no memory-safety preconditions.
        if unsafe { libc::kill(pid, signal) } == -1 {
            let err = io::Error::last_os_error();
            // Already gone between the lock probe and the signal.
            if err.raw_os_error() != Some(libc::ESRCH) {
                return Err(err).with_context(|| format!("signal pid {pid}"));
            }
        }
        Ok(())
    }
}

#[cfg(windows)]
mod platform {
    use std::io;
    use std::os::windows::process::CommandExt;
    use std::process::Command;

    use anyhow::{Context, Result};
    use windows_sys::Win32::Foundation::{CloseHandle, ERROR_INVALID_PARAMETER};
    use windows_sys::Win32::System::Threading::{
        CREATE_NEW_PROCESS_GROUP, DETACHED_PROCESS, OpenProcess, PROCESS_TERMINATE,
        TerminateProcess,
    };

    pub fn detach(command: &mut Command) -> &mut Command {
        command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP)
    }

    pub fn terminate(pid: u32) -> Result<()> {
        kill(pid)
    }

    pub fn kill(pid: u32) -> Result<()> {
        // SAFETY: `OpenProcess` has no memory-safety preconditions; the
        // handle it returns is closed below and used only in between.
        let handle = unsafe { OpenProcess(PROCESS_TERMINATE, 0, pid) };
        if handle.is_null() {
            let err = io::Error::last_os_error();
            // Already gone between the lock probe and this call.
            if err.raw_os_error() == Some(ERROR_INVALID_PARAMETER as i32) {
                return Ok(());
            }
            return Err(err).with_context(|| format!("open process {pid}"));
        }
        // SAFETY: `handle` is a valid process handle opened above.
        let terminated = unsafe { TerminateProcess(handle, 1) };
        let err = io::Error::last_os_error();
        // SAFETY: `handle` is valid and not used after this call.
        unsafe { CloseHandle(handle) };
        if terminated == 0 {
            return Err(err).with_context(|| format!("terminate process {pid}"));
        }
        Ok(())
    }
}
