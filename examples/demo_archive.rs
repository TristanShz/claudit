//! Builds a demo archive from the test fixtures, for screenshots and manual
//! browser checks (see "Screenshots" in CONTRIBUTING.md):
//!
//! ```sh
//! cargo run --example demo_archive -- /tmp/claudit-demo
//! CLAUDIT_HOME=/tmp/claudit-demo cargo run -- serve
//! ```
//!
//! It reuses the seam-1 test harness, so the archive holds exactly what the
//! dashboard smoke checks see, plus a few more tool calls.

#[path = "../tests/common/mod.rs"]
mod common;

use std::path::PathBuf;

fn main() {
    let Some(target) = std::env::args_os().nth(1).map(PathBuf::from) else {
        eprintln!("usage: cargo run --example demo_archive -- <target CLAUDIT_HOME>");
        std::process::exit(2);
    };
    let env = common::TestEnv::new();
    env.populate_fixture_archive();
    env.replay_activities_session();
    for name in [
        "post_tool_use_bash.json",
        "post_tool_use_read.json",
        "post_tool_use_mcp.json",
        "post_tool_use_failure_bash.json",
        "post_tool_use_skill.json",
    ] {
        env.hook_fixture(name);
    }
    env.ingest();
    std::fs::create_dir_all(&target).expect("create the target directory");
    for entry in std::fs::read_dir(env.paths.home()).expect("read the demo home") {
        let entry = entry.expect("read the demo home");
        if entry.file_type().expect("file type").is_file() {
            std::fs::copy(entry.path(), target.join(entry.file_name())).expect("copy");
        }
    }
    println!(
        "demo archive written to {}; run: CLAUDIT_HOME={} claudit serve",
        target.display(),
        target.display()
    );
}
