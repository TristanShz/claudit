//! The dashboard in the background: `claudit serve --detach` and
//! `claudit kill`.
//!
//! Every `claudit serve`, in the foreground or detached, holds an advisory
//! `flock` on `$CLAUDIT_HOME/serve.lock` for as long as it runs, and writes
//! `<pid> <port>` into it once its port is bound. The kernel releases the lock
//! when the process exits, even if it crashes, so a held lock always names a
//! live dashboard and the file can never go stale: `claudit kill` only ever
//! signals a process that is still that dashboard.
//!
//! A detached dashboard is the same `claudit serve`, started in a new session
//! (`setsid`, so closing the terminal does not stop it) with stdin on
//! `/dev/null` and stdout/stderr appended to `logs/serve.log`.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use crate::paths::Paths;
use crate::secure_fs;

/// How long `serve --detach` waits for the dashboard to bind its port.
const START_TIMEOUT: Duration = Duration::from_secs(10);
/// How long `kill` waits after SIGTERM before sending SIGKILL. A dashboard
/// only lingers while its start-up catch-up ingest finishes.
const STOP_TIMEOUT: Duration = Duration::from_secs(10);
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// A dashboard that holds the serve lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunningServer {
    pub pid: u32,
    pub port: u16,
}

impl RunningServer {
    /// The dashboard's address.
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }
}

/// Proof that this process is the only dashboard. Released on drop.
#[derive(Debug)]
pub struct ServeLock {
    file: File,
}

impl ServeLock {
    /// Takes the serve lock, or fails naming the dashboard already running.
    pub fn acquire(paths: &Paths) -> Result<Self> {
        // A `running` probe holds the lock for a few microseconds: retry
        // briefly so a probe never makes a starting dashboard give up.
        for _ in 0..10 {
            let file = open_lock_file(paths)?;
            if try_flock(paths, &file, libc::LOCK_EX)? {
                file.set_len(0).context("truncate serve lock file")?;
                return Ok(Self { file });
            }
            if let Some(server) = read_holder(paths)? {
                bail!(
                    "the dashboard is already running at {} (pid {}); stop it with `claudit kill`",
                    server.url(),
                    server.pid
                );
            }
            thread::sleep(Duration::from_millis(20));
        }
        bail!("another claudit dashboard is starting; stop it with `claudit kill`")
    }

    /// Records this process and the port it listens on, once bound.
    pub fn publish(&mut self, port: u16) -> Result<()> {
        let line = format!("{} {port}\n", std::process::id());
        self.file.set_len(0)?;
        self.file.seek(SeekFrom::Start(0))?;
        self.file
            .write_all(line.as_bytes())
            .context("write serve lock file")
    }
}

impl Drop for ServeLock {
    fn drop(&mut self) {
        // Leave no pid behind once stopped (the lock itself is released when
        // the file closes).
        let _ = self.file.set_len(0);
    }
}

/// The dashboard currently running, if any.
pub fn running(paths: &Paths) -> Result<Option<RunningServer>> {
    let file = open_lock_file(paths)?;
    // A shared probe: it never holds the lock against a dashboard for longer
    // than this call.
    if try_flock(paths, &file, libc::LOCK_SH)? {
        return Ok(None);
    }
    // Held, but the holder may not have bound its port yet.
    let deadline = Instant::now() + START_TIMEOUT;
    loop {
        if let Some(server) = read_holder(paths)? {
            return Ok(Some(server));
        }
        if Instant::now() >= deadline {
            bail!("a claudit dashboard holds the serve lock but never published its port");
        }
        thread::sleep(POLL_INTERVAL);
    }
}

/// Starts `claudit serve --port <port>` detached from the terminal and waits
/// until it listens.
pub fn spawn_detached(paths: &Paths, port: u16) -> Result<RunningServer> {
    if let Some(server) = running(paths)? {
        bail!(
            "the dashboard is already running at {} (pid {}); stop it with `claudit kill`",
            server.url(),
            server.pid
        );
    }
    let log_path = paths.serve_log_file();
    let log = secure_fs::open_append(&log_path)
        .with_context(|| format!("open {}", log_path.display()))?;
    // Only this start's output is quoted if it fails.
    let log_start = log.metadata().map(|m| m.len()).unwrap_or(0);

    let exe = std::env::current_exe().context("locate the claudit executable")?;
    let mut command = Command::new(exe);
    command
        .args(["serve", "--port", &port.to_string()])
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    // SAFETY: `setsid` is async-signal-safe, as required between fork and
    // exec; it only fails if the child already leads a process group, which a
    // freshly forked child never does.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn().context("spawn claudit serve")?;

    let deadline = Instant::now() + START_TIMEOUT;
    loop {
        // Never probe the lock here: the child must be free to take it.
        if let Some(server) = read_holder(paths)?
            && server.pid == child.id()
        {
            return Ok(server);
        }
        if child.try_wait()?.is_some() {
            let output = read_from(&log_path, log_start);
            bail!(
                "the dashboard did not start: {} (log: {})",
                last_line(&output).unwrap_or("no output"),
                log_path.display()
            );
        }
        if Instant::now() >= deadline {
            bail!(
                "the dashboard did not start within {}s (pid {}); see {}",
                START_TIMEOUT.as_secs(),
                child.id(),
                log_path.display()
            );
        }
        thread::sleep(POLL_INTERVAL);
    }
}

/// What `kill` did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KillOutcome {
    NotRunning,
    /// Stopped by SIGTERM (graceful shutdown).
    Stopped(RunningServer),
    /// Did not stop within the timeout and was sent SIGKILL.
    Killed(RunningServer),
}

/// Stops the running dashboard, whether it was started detached or not.
pub fn kill(paths: &Paths) -> Result<KillOutcome> {
    let Some(server) = running(paths)? else {
        return Ok(KillOutcome::NotRunning);
    };
    signal(server.pid, libc::SIGTERM)?;
    if wait_until_stopped(paths, STOP_TIMEOUT)? {
        return Ok(KillOutcome::Stopped(server));
    }
    signal(server.pid, libc::SIGKILL)?;
    if wait_until_stopped(paths, STOP_TIMEOUT)? {
        return Ok(KillOutcome::Killed(server));
    }
    bail!("the dashboard (pid {}) did not stop", server.pid)
}

fn wait_until_stopped(paths: &Paths, timeout: Duration) -> Result<bool> {
    let deadline = Instant::now() + timeout;
    loop {
        let file = open_lock_file(paths)?;
        if try_flock(paths, &file, libc::LOCK_SH)? {
            return Ok(true);
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        thread::sleep(POLL_INTERVAL);
    }
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

fn open_lock_file(paths: &Paths) -> Result<File> {
    let path = paths.serve_lock_file();
    if let Some(parent) = path.parent() {
        secure_fs::create_dir_all(parent)?;
    }
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(secure_fs::FILE_MODE)
        .open(&path)
        .with_context(|| format!("open {}", path.display()))
}

/// Takes `operation` (`LOCK_EX` or `LOCK_SH`) without blocking: false if
/// another process holds the lock.
fn try_flock(paths: &Paths, file: &File, operation: libc::c_int) -> Result<bool> {
    // SAFETY: `flock` only reads the descriptor, which `file` keeps open.
    if unsafe { libc::flock(file.as_raw_fd(), operation | libc::LOCK_NB) } == 0 {
        return Ok(true);
    }
    let err = io::Error::last_os_error();
    if err.kind() == io::ErrorKind::WouldBlock {
        Ok(false)
    } else {
        Err(err).with_context(|| format!("lock {}", paths.serve_lock_file().display()))
    }
}

/// The `<pid> <port>` the holder published; `None` before it did.
fn read_holder(paths: &Paths) -> Result<Option<RunningServer>> {
    let mut text = String::new();
    open_lock_file(paths)?.read_to_string(&mut text)?;
    let mut fields = text.split_whitespace();
    let (Some(pid), Some(port)) = (fields.next(), fields.next()) else {
        return Ok(None);
    };
    Ok(match (pid.parse(), port.parse()) {
        (Ok(pid), Ok(port)) => Some(RunningServer { pid, port }),
        _ => None,
    })
}

fn read_from(path: &std::path::Path, offset: u64) -> String {
    let mut text = String::new();
    if let Ok(mut file) = File::open(path)
        && file.seek(SeekFrom::Start(offset)).is_ok()
    {
        let _ = file.read_to_string(&mut text);
    }
    text
}

fn last_line(text: &str) -> Option<&str> {
    text.lines().map(str::trim).rfind(|line| !line.is_empty())
}
