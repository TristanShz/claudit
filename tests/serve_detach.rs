//! `claudit serve --detach` and `claudit kill`: the dashboard keeps running
//! after the command that started it exits, only one runs at a time, and
//! `kill` stops it.
//!
//! Seam under test: the built binary's process contract (exit codes, output,
//! the port answering), like the `claudit hook` contract in `tests/hook.rs`.

mod common;

use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;

use common::TestEnv;

fn free_port() -> u16 {
    TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// The status line of `GET /assets/claudit.css`, or `None` if nothing listens.
fn status_line(port: u16) -> Option<String> {
    let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).ok()?;
    stream
        .write_all(
            b"GET /assets/claudit.css HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )
        .ok()?;
    let mut response = String::new();
    let _ = stream.read_to_string(&mut response);
    response.lines().next().map(str::to_owned)
}

fn stdout(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Stops whatever the test left running, even when an assertion failed.
struct KillOnDrop<'a>(&'a TestEnv);

impl Drop for KillOnDrop<'_> {
    fn drop(&mut self) {
        self.0.run_bin(&["kill"], b"");
    }
}

#[test]
fn a_detached_dashboard_serves_until_killed() {
    let env = TestEnv::new();
    let _guard = KillOnDrop(&env);
    let port = free_port();

    let out = env.run_bin(&["serve", "--detach", "--port", &port.to_string()], b"");

    assert_eq!(out.status.code(), Some(0), "stderr: {:?}", out.stderr);
    assert!(
        stdout(&out).contains(&format!("http://127.0.0.1:{port}")),
        "stdout: {}",
        stdout(&out)
    );
    // The command has exited; the dashboard it started still answers.
    assert_eq!(status_line(port).as_deref(), Some("HTTP/1.1 200 OK"));
    for file in [env.paths.serve_lock_file(), env.paths.serve_log_file()] {
        let mode = std::fs::metadata(&file).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "{}", file.display());
    }

    let out = env.run_bin(&["kill"], b"");

    assert_eq!(out.status.code(), Some(0));
    assert!(stdout(&out).contains("Stopped"), "stdout: {}", stdout(&out));
    assert_eq!(status_line(port), None, "the port is closed");
}

#[test]
fn a_second_dashboard_is_refused_while_one_runs() {
    let env = TestEnv::new();
    let _guard = KillOnDrop(&env);
    let port = free_port();
    let out = env.run_bin(&["serve", "-d", "--port", &port.to_string()], b"");
    assert_eq!(out.status.code(), Some(0), "stderr: {:?}", out.stderr);

    let other_port = free_port().to_string();
    for args in [
        vec!["serve", "-d", "--port", &other_port],
        vec!["serve", "--port", &other_port],
    ] {
        let out = env.run_bin(&args, b"");

        assert_eq!(out.status.code(), Some(1), "{args:?}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("already running"), "stderr: {stderr}");
        assert!(stderr.contains(&format!(":{port}")), "stderr: {stderr}");
    }
}

#[test]
fn a_detached_dashboard_that_cannot_bind_reports_why() {
    let env = TestEnv::new();
    let taken = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = taken.local_addr().unwrap().port().to_string();

    let out = env.run_bin(&["serve", "-d", "--port", &port], b"");

    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("did not start"), "stderr: {stderr}");
    assert!(stderr.contains("bind"), "stderr: {stderr}");
}

#[test]
fn kill_with_nothing_running_says_so() {
    let env = TestEnv::new();

    let out = env.run_bin(&["kill"], b"");

    assert_eq!(out.status.code(), Some(0));
    assert!(
        stdout(&out).contains("No claudit dashboard is running"),
        "stdout: {}",
        stdout(&out)
    );
}
