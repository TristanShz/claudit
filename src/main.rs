//! The `claudit` binary: a thin CLI adapter over the library.

use std::process::ExitCode;

use anyhow::{Result, bail};
use clap::{Parser, Subcommand};
use claudit::clock::SystemClock;
use claudit::paths::Paths;

#[derive(Parser)]
#[command(version, about = "Local analytics for Claude Code sessions")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Record one hook payload from stdin (run by Claude Code, not by hand).
    Hook,
    /// Load new spooled events and transcripts into the archive.
    Ingest,
    /// Rebuild every derived table from the raw archive.
    Reingest,
    /// Serve the local dashboard.
    Serve {
        /// Port to listen on (always bound to 127.0.0.1).
        #[arg(long, default_value_t = claudit::web::DEFAULT_PORT)]
        port: u16,
    },
    /// Add claudit's hooks to Claude Code's user settings.
    Install,
    /// Remove claudit's hooks from Claude Code's user settings.
    Uninstall,
}

fn main() -> ExitCode {
    // `claudit hook` runs inside Claude Code: it must never print or fail,
    // so it bypasses argument parsing and error reporting entirely.
    if std::env::args().nth(1).as_deref() == Some("hook") {
        run_hook();
        return ExitCode::SUCCESS;
    }
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("claudit: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn run_hook() {
    // Silence the default panic message (stderr); `hook::run` logs panics.
    std::panic::set_hook(Box::new(|_| {}));
    // Without resolvable paths there is no log to write to either.
    if let Ok(paths) = Paths::from_env() {
        claudit::hook::run(&paths, &SystemClock, std::io::stdin().lock());
    }
}

fn run(cli: Cli) -> Result<()> {
    let paths = Paths::from_env()?;
    match cli.command {
        Command::Hook => unreachable!("handled before argument parsing"),
        Command::Ingest => {
            let report = claudit::ingest::run(&paths)?;
            println!(
                "ingested {} events ({} skipped lines, {} unprojected events)",
                report.events, report.skipped_lines, report.unprojected_events
            );
            Ok(())
        }
        Command::Serve { port } => {
            let runtime = tokio::runtime::Runtime::new()?;
            runtime.block_on(claudit::web::serve(paths, port))
        }
        Command::Reingest => not_yet_implemented("reingest"),
        Command::Install => not_yet_implemented("install"),
        Command::Uninstall => not_yet_implemented("uninstall"),
    }
}

fn not_yet_implemented(command: &str) -> Result<()> {
    bail!("`claudit {command}` is not yet implemented")
}
