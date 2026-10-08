//! The dashboard in the background: `claudit serve --detach` and
//! `claudit kill`.
//!
//! Every `claudit serve`, in the foreground or detached, holds a file lock
//! ([`File::try_lock`]: `flock` on Unix, `LockFileEx` on Windows) on
//! `$CLAUDIT_HOME/serve.lock` for as long as it runs, and writes
//! `<pid> <port>` to `$CLAUDIT_HOME/serve.pid` once its port is bound. The
//! kernel releases the lock when the process exits, even if it crashes, and
//! `serve.pid` is only read while the lock is held, so it always names a
//! live dashboard: `claudit kill` only ever stops a process that is still
//! that dashboard.
//!
//! A detached dashboard is the same `claudit serve`, started detached
//! ([`process::detach`], so closing the terminal does not stop it) with stdin
//! on the null device and stdout/stderr appended to `logs/serve.log`.

use std::fs::{self, File, TryLockError};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use crate::paths::Paths;
use crate::process;
use crate::secure_fs;

/// How long `serve --detach` waits for the dashboard to bind its port.
const START_TIMEOUT: Duration = Duration::from_secs(10);
/// How long `kill` waits after asking the dashboard to stop before killing
/// it. A dashboard only lingers while its start-up catch-up ingest finishes.
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
    _file: File,
    pid_file: std::path::PathBuf,
}

impl ServeLock {
    /// Takes the serve lock, or fails naming the dashboard already running.
    pub fn acquire(paths: &Paths) -> Result<Self> {
        // A `running` probe holds the lock for a few microseconds: retry
        // briefly so a probe never makes a starting dashboard give up.
        for _ in 0..10 {
            let file = open_lock_file(paths)?;
            if try_lock(paths, &file, Lock::Exclusive)? {
                // Whatever is there was left by a dashboard that crashed
                // (in the lock file itself before 0.9).
                file.set_len(0).context("truncate serve lock file")?;
                remove_pid_file(paths)?;
                return Ok(Self {
                    _file: file,
                    pid_file: paths.serve_pid_file(),
                });
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
        secure_fs::owner_only(
            fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true),
        )
        .open(&self.pid_file)
        .and_then(|mut file| file.write_all(line.as_bytes()))
        .with_context(|| format!("write {}", self.pid_file.display()))
    }
}

impl Drop for ServeLock {
    fn drop(&mut self) {
        // Leave no pid behind once stopped (the lock itself is released when
        // the file closes).
        let _ = fs::remove_file(&self.pid_file);
    }
}

/// The dashboard currently running, if any.
pub fn running(paths: &Paths) -> Result<Option<RunningServer>> {
    let file = open_lock_file(paths)?;
    // A shared probe: it never holds the lock against a dashboard for longer
    // than this call.
    if try_lock(paths, &file, Lock::Shared)? {
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
    let mut child = process::detach(&mut command)
        .spawn()
        .context("spawn claudit serve")?;

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
    /// Stopped when asked ([`process::terminate`]).
    Stopped(RunningServer),
    /// Did not stop within the timeout and was killed ([`process::kill`]).
    Killed(RunningServer),
}

/// Stops the running dashboard, whether it was started detached or not.
pub fn kill(paths: &Paths) -> Result<KillOutcome> {
    let Some(server) = running(paths)? else {
        return Ok(KillOutcome::NotRunning);
    };
    process::terminate(server.pid)?;
    if wait_until_stopped(paths, STOP_TIMEOUT)? {
        return Ok(KillOutcome::Stopped(server));
    }
    process::kill(server.pid)?;
    if wait_until_stopped(paths, STOP_TIMEOUT)? {
        return Ok(KillOutcome::Killed(server));
    }
    bail!("the dashboard (pid {}) did not stop", server.pid)
}

fn wait_until_stopped(paths: &Paths, timeout: Duration) -> Result<bool> {
    let deadline = Instant::now() + timeout;
    loop {
        let file = open_lock_file(paths)?;
        if try_lock(paths, &file, Lock::Shared)? {
            return Ok(true);
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        thread::sleep(POLL_INTERVAL);
    }
}

fn open_lock_file(paths: &Paths) -> Result<File> {
    let path = paths.serve_lock_file();
    secure_fs::open_lock(&path).with_context(|| format!("open {}", path.display()))
}

#[derive(Debug, Clone, Copy)]
enum Lock {
    Exclusive,
    Shared,
}

/// Takes `lock` on `file` without blocking: false if another process holds
/// the lock.
fn try_lock(paths: &Paths, file: &File, lock: Lock) -> Result<bool> {
    let taken = match lock {
        Lock::Exclusive => file.try_lock(),
        Lock::Shared => file.try_lock_shared(),
    };
    match taken {
        Ok(()) => Ok(true),
        Err(TryLockError::WouldBlock) => Ok(false),
        Err(TryLockError::Error(err)) => {
            Err(err).with_context(|| format!("lock {}", paths.serve_lock_file().display()))
        }
    }
}

fn remove_pid_file(paths: &Paths) -> Result<()> {
    let path = paths.serve_pid_file();
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err).with_context(|| format!("remove {}", path.display())),
    }
}

/// The `<pid> <port>` the holder published; `None` before it did (or while
/// it is writing the line, which only counts once complete).
fn read_holder(paths: &Paths) -> Result<Option<RunningServer>> {
    if let Some(text) = read_if_exists(&paths.serve_pid_file())? {
        return Ok(parse_holder(&text));
    }
    // A dashboard started before 0.9 published in the lock file itself,
    // which only Unix lets another process read while it is locked.
    #[cfg(unix)]
    if let Some(text) = read_if_exists(&paths.serve_lock_file())? {
        return Ok(parse_holder(&text));
    }
    Ok(None)
}

fn parse_holder(text: &str) -> Option<RunningServer> {
    let mut fields = text.strip_suffix('\n')?.split_whitespace();
    let (pid, port) = (fields.next()?, fields.next()?);
    Some(RunningServer {
        pid: pid.parse().ok()?,
        port: port.parse().ok()?,
    })
}

fn read_if_exists(path: &std::path::Path) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err).with_context(|| format!("read {}", path.display())),
    }
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
