//! The `claudit` binary: a thin CLI adapter over the library.

use std::process::ExitCode;

use anyhow::Result;
use clap::{Parser, Subcommand};
use claudit::clock::SystemClock;
use claudit::ingest::IngestOutcome;
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
        /// Run in the background, detached from the terminal (stop it with
        /// `claudit kill`).
        #[arg(short, long)]
        detach: bool,
    },
    /// Stop the running dashboard (started with `serve --detach` or not).
    #[command(visible_alias = "stop")]
    Kill,
    /// Add claudit's hooks to Claude Code's user settings.
    Install,
    /// Remove claudit's hooks from Claude Code's user settings.
    Uninstall,
    /// Export one session as Markdown (to read, or to give to an AI).
    Export {
        /// The session id, or a unique prefix of it.
        session: String,
        /// Add every turn in detail: its whole prompt and every tool call.
        #[arg(long)]
        full: bool,
        /// Write to this file instead of standard output.
        #[arg(short, long)]
        output: Option<std::path::PathBuf>,
    },
    /// Replace this binary with the latest release from GitHub.
    Update {
        /// Only report whether a newer release exists.
        #[arg(long)]
        check: bool,
    },
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
        claudit::hook::run(
            &paths,
            &SystemClock,
            &claudit::hook::DetachedIngest,
            std::io::stdin().lock(),
        );
    }
}

fn run(cli: Cli) -> Result<()> {
    let paths = Paths::from_env()?;
    match cli.command {
        Command::Hook => unreachable!("handled before argument parsing"),
        Command::Ingest => catch_up(&paths),
        Command::Serve { port, detach: true } => serve_detached(&paths, port),
        Command::Serve {
            port,
            detach: false,
        } => {
            // Serve catches up once in the background at startup; the
            // dashboard never polls afterwards.
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
        Command::Kill => kill(&paths),
        Command::Install => install(&paths),
        Command::Uninstall => uninstall(&paths),
        Command::Export {
            session,
            full,
            output,
        } => export(&paths, &session, full, output.as_deref()),
        Command::Update { check } => update(check),
    }
}

fn serve_detached(paths: &Paths, port: u16) -> Result<()> {
    let server = claudit::daemon::spawn_detached(paths, port)?;
    println!(
        "claudit dashboard: {} (running in the background, pid {}).",
        server.url(),
        server.pid
    );
    println!("Output goes to {}.", paths.serve_log_file().display());
    println!("Stop it with `claudit kill`.");
    Ok(())
}

fn kill(paths: &Paths) -> Result<()> {
    use claudit::daemon::KillOutcome;
    match claudit::daemon::kill(paths)? {
        KillOutcome::NotRunning => println!("No claudit dashboard is running."),
        KillOutcome::Stopped(server) => println!(
            "Stopped the claudit dashboard at {} (pid {}).",
            server.url(),
            server.pid
        ),
        KillOutcome::Killed(server) => println!(
            "The claudit dashboard at {} (pid {}) did not stop in time and was killed.",
            server.url(),
            server.pid
        ),
    }
    Ok(())
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

fn export(
    paths: &Paths,
    session: &str,
    full: bool,
    output: Option<&std::path::Path>,
) -> Result<()> {
    use claudit::export::{self, ExportLevel};
    // Export what is pending too; stdout carries the export, so quietly.
    if let Err(err) = claudit::ingest::catch_up(paths, &SystemClock) {
        eprintln!("claudit: ingest failed, exporting the archive as it is: {err:#}");
    }
    let conn = claudit::db::open(paths)?;
    let session_id = export::resolve_session(&conn, session)?;
    let loaded = claudit::activities::ActivityRules::load(paths);
    if let Some(problem) = &loaded.problem {
        eprintln!("claudit: activity rules file ignored: {problem}");
    }
    let level = if full {
        ExportLevel::Full
    } else {
        ExportLevel::Summary
    };
    let Some(markdown) = export::session_markdown(
        &conn,
        &session_id,
        level,
        &loaded.rules,
        claudit::pricing::PriceTable::builtin(),
    )?
    else {
        anyhow::bail!("nothing is known about session {session_id}");
    };
    match output {
        Some(path) => {
            std::fs::write(path, markdown)?;
            eprintln!("Exported session {session_id} to {}.", path.display());
        }
        None => print!("{markdown}"),
    }
    Ok(())
}

fn update(check_only: bool) -> Result<()> {
    let check = claudit::update::check()?;
    if !check.is_newer() {
        println!("claudit {} is up to date.", check.current);
        return Ok(());
    }
    if check_only {
        println!(
            "claudit {} is available (installed: {}). Run `claudit update` to install it.",
            check.latest, check.current
        );
        return Ok(());
    }
    let exe = std::env::current_exe()?;
    let exe = exe.canonicalize().unwrap_or(exe);
    claudit::update::ensure_self_managed(&exe)?;
    println!("Updating claudit {} → {}…", check.current, check.latest);
    claudit::update::install_release(&exe, check.latest)?;
    println!("Installed claudit {} at {}.", check.latest, exe.display());
    println!(
        "Changes: {}/releases/tag/{}",
        claudit::update::REPOSITORY,
        check.latest.tag()
    );
    Ok(())
}

/// Runs the locked catch-up ingest. Losing the lock is not an error: the
/// running ingest will pick up everything pending.
fn catch_up(paths: &Paths) -> Result<()> {
    match claudit::ingest::catch_up(paths, &SystemClock) {
        Ok(IngestOutcome::Ran(report)) => {
            if report.rebuilt {
                println!(
                    "rebuilt the archive once (derived by an older claudit), as `claudit reingest` would"
                );
            }
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
        Ok(IngestOutcome::AlreadyRunning) => {
            println!("another ingest is running; it will pick up pending input");
            Ok(())
        }
        Err(err) => {
            // A detached ingest has no terminal: the log is its only output.
            claudit::logfile::error(paths, "ingest", format!("{err:#}"));
            Err(err)
        }
    }
}
