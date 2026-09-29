//! The `claudit` binary: a thin CLI adapter over the library.

use std::process::ExitCode;

use anyhow::Result;
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
            println!(
                "read {} transcript lines ({} skipped)",
                report.transcript_lines, report.skipped_transcript_lines
            );
            Ok(())
        }
        Command::Serve { port } => {
            let runtime = tokio::runtime::Runtime::new()?;
            runtime.block_on(claudit::web::serve(paths, port))
        }
        Command::Reingest => {
            let report = claudit::ingest::reingest(&paths)?;
            println!(
                "replayed {} archived events ({} re-redacted, {} unprojected)",
                report.replay.events, report.replay.resanitized, report.replay.unprojected_events
            );
            println!(
                "ingested {} new events; read {} transcript lines ({} skipped)",
                report.ingest.events,
                report.ingest.transcript_lines,
                report.ingest.skipped_transcript_lines
            );
            Ok(())
        }
        Command::Install => install(&paths),
        Command::Uninstall => uninstall(&paths),
    }
}

fn install(paths: &Paths) -> Result<()> {
    let exe = std::env::current_exe()?;
    let exe = exe.canonicalize().unwrap_or(exe);
    let command = claudit::install::hook_command(&exe);
    let report = claudit::install::install_settings(paths, &command, &SystemClock)?;
    let file = report.settings_file.display();
    if !report.changed {
        println!("claudit is already installed in {file}; nothing changed.");
        return Ok(());
    }
    println!("Installed claudit's hooks in {file} (command: {command}).");
    if let Some(backup) = &report.backup {
        println!("Previous settings backed up to {}.", backup.display());
    }
    if report.raised_cleanup_from.is_some() {
        println!(
            "Raised cleanupPeriodDays to {} so transcripts are kept long enough to ingest.",
            claudit::install::MIN_CLEANUP_PERIOD_DAYS
        );
    }
    println!("New Claude Code sessions will now be recorded.");
    Ok(())
}

fn uninstall(paths: &Paths) -> Result<()> {
    let report = claudit::install::uninstall_settings(paths, &SystemClock)?;
    let file = report.settings_file.display();
    if !report.changed {
        println!("claudit's hooks were not found in {file}; nothing changed.");
        return Ok(());
    }
    println!(
        "Removed claudit's hooks from {file} (cleanupPeriodDays restored if claudit raised it)."
    );
    if let Some(backup) = &report.backup {
        println!("Previous settings backed up to {}.", backup.display());
    }
    println!("Your recorded data in {} was kept.", paths.home().display());
    Ok(())
}
