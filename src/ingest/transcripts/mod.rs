//! Loads Claude Code transcripts into `sessions`, `turns` and
//! `api_messages`.
//!
//! Inputs, under the Claude projects dir:
//! - main sessions: `<project>/<session_id>.jsonl`;
//! - subagents: `<project>/<session_id>/subagents/agent-<agent_id>.jsonl`.
//!
//! Each file is read incrementally from its byte offset (path + inode), one
//! IMMEDIATE transaction per file. Entries are deduplicated on their `uuid`
//! and API usage on `message.id`, so a first run backfills everything on
//! disk and later runs only add what is new. Lines of unknown shape are
//! skipped, counted (in the report and cumulatively in `meta`) and logged.

mod entry;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use self::entry::{Entry, Kind, Line};
use super::IngestReport;
use super::offsets::{self, FileId};
use crate::clock::{self, Clock, SystemClock};
use crate::logfile;
use crate::paths::Paths;

/// `meta` key: transcript lines skipped as unknown, over all runs.
pub const META_SKIPPED_LINES: &str = "transcripts_skipped_lines";
/// `meta` key: when the first complete pass over the transcripts on disk
/// (the backfill) finished, µs since the epoch.
pub const META_BACKFILLED_AT: &str = "transcripts_backfilled_at_us";

/// Model name Claude Code gives to messages it made up locally (e.g. API
/// errors): not an API call, so not counted.
const SYNTHETIC_MODEL: &str = "<synthetic>";

/// Ingests the new complete lines of every transcript.
pub fn ingest(conn: &mut Connection, paths: &Paths) -> Result<IngestReport> {
    let mut report = IngestReport::default();
    for file in transcript_files(&paths.claude_projects_dir())? {
        match ingest_file(conn, paths, &file) {
            Ok(file_report) => report.absorb(file_report),
            // A transcript can vanish (retention cleanup) between listing
            // and reading: log it and carry on with the others.
            Err(err) => logfile::error(
                paths,
                "ingest",
                format!("transcript {}: {err:#}", file.path.display()),
            ),
        }
    }
    conn.execute(
        "INSERT INTO meta (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO NOTHING",
        params![
            META_BACKFILLED_AT,
            clock::to_micros(SystemClock.now()).to_string()
        ],
    )?;
    Ok(report)
}

/// A transcript file and, for a subagent transcript, the agent it belongs to.
struct TranscriptFile {
    path: PathBuf,
    agent_id: Option<String>,
}

/// Every main and subagent transcript, in a deterministic order (main
/// transcripts of a project before its subagents).
fn transcript_files(projects: &Path) -> Result<Vec<TranscriptFile>> {
    let mut files = Vec::new();
    for project in sorted_entries(projects)? {
        if !project.is_dir() {
            continue;
        }
        let mut subagents = Vec::new();
        for path in sorted_entries(&project)? {
            if path.is_dir() {
                for sub in sorted_entries(&path.join("subagents"))? {
                    if let Some(agent) = jsonl_stem(&sub) {
                        let agent_id = agent.strip_prefix("agent-").unwrap_or(&agent).to_owned();
                        subagents.push(TranscriptFile {
                            path: sub,
                            agent_id: Some(agent_id),
                        });
                    }
                }
            } else if jsonl_stem(&path).is_some() {
                files.push(TranscriptFile {
                    path,
                    agent_id: None,
                });
            }
        }
        files.append(&mut subagents);
    }
    Ok(files)
}

fn sorted_entries(dir: &Path) -> Result<Vec<PathBuf>> {
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut paths = std::fs::read_dir(dir)
        .with_context(|| format!("list {}", dir.display()))?
        .map(|entry| entry.map(|e| e.path()))
        .collect::<Result<Vec<_>, _>>()?;
    paths.sort();
    Ok(paths)
}

fn jsonl_stem(path: &Path) -> Option<String> {
    if !path.is_file() || path.extension().is_none_or(|ext| ext != "jsonl") {
        return None;
    }
    Some(path.file_stem()?.to_string_lossy().into_owned())
}

fn ingest_file(
    conn: &mut Connection,
    paths: &Paths,
    file: &TranscriptFile,
) -> Result<IngestReport> {
    let id = FileId::of(&file.path)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let start = offsets::get(&tx, &id)?;
    let chunk = offsets::read_complete_lines(&file.path, start)?;

    let mut report = IngestReport::default();
    let mut first_skip = None;
    for line in chunk.lines.iter().filter(|line| !line.trim().is_empty()) {
        report.transcript_lines += 1;
        match entry::parse(line) {
            Line::Entry(entry) => project(&tx, &entry, file.agent_id.as_deref())?,
            Line::Ignored => {}
            Line::Unknown(reason) => {
                report.skipped_transcript_lines += 1;
                first_skip.get_or_insert(reason);
            }
        }
    }
    if let Some(reason) = first_skip {
        logfile::error(
            paths,
            "ingest",
            format!(
                "skipped {} transcript line(s) of unknown shape in {} (first: {reason})",
                report.skipped_transcript_lines,
                file.path.display()
            ),
        );
        tx.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE
             SET value = CAST(CAST(value AS INTEGER) + CAST(excluded.value AS INTEGER) AS TEXT)",
            params![
                META_SKIPPED_LINES,
                report.skipped_transcript_lines.to_string()
            ],
        )?;
    }
    offsets::set(&tx, &id, chunk.end_offset)?;
    tx.commit()?;
    Ok(report)
}

/// Derives rows from one entry. Entries already seen (same `uuid`, e.g.
/// repeated in a resumed session's file) are skipped.
fn project(conn: &Connection, entry: &Entry, file_agent: Option<&str>) -> Result<()> {
    let prompt_id = match &entry.prompt_id {
        Some(prompt_id) => Some(prompt_id.clone()),
        None => parent_prompt(conn, entry.parent_uuid.as_deref())?,
    };
    let inserted = conn.execute(
        "INSERT INTO transcript_entries (uuid, session_id, prompt_id) VALUES (?1, ?2, ?3)
         ON CONFLICT(uuid) DO NOTHING",
        params![entry.uuid, entry.session_id, prompt_id],
    )?;
    if inserted == 0 {
        return Ok(());
    }
    let at_us = clock::to_micros(entry.at);
    let agent_id = entry.agent_id.as_deref().or(file_agent);
    let main_thread = agent_id.is_none();

    if main_thread {
        upsert_session(conn, entry, at_us)?;
    }
    match &entry.kind {
        Kind::User { prompt_text } => {
            if let (true, Some(prompt_id)) = (main_thread, &prompt_id) {
                upsert_turn(conn, entry, prompt_id, at_us, prompt_text.as_deref())?;
            }
        }
        Kind::Assistant(message) => {
            if let (true, Some(prompt_id)) = (main_thread, &prompt_id) {
                upsert_turn(conn, entry, prompt_id, at_us, None)?;
            }
            if message.model != SYNTHETIC_MODEL {
                conn.execute(
                    "INSERT INTO api_messages (
                         message_id, session_id, prompt_id, agent_id, agent_type, model,
                         at_us, input_tokens, output_tokens, cache_write_tokens,
                         cache_read_tokens, skill, cache_write_1h_tokens
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
                     ON CONFLICT(message_id) DO UPDATE SET
                         prompt_id          = COALESCE(prompt_id, excluded.prompt_id),
                         agent_type         = COALESCE(agent_type, excluded.agent_type),
                         at_us              = MIN(at_us, excluded.at_us),
                         input_tokens       = MAX(input_tokens, excluded.input_tokens),
                         output_tokens      = MAX(output_tokens, excluded.output_tokens),
                         cache_write_tokens = MAX(cache_write_tokens, excluded.cache_write_tokens),
                         cache_read_tokens  = MAX(cache_read_tokens, excluded.cache_read_tokens),
                         skill              = COALESCE(skill, excluded.skill),
                         cache_write_1h_tokens =
                             MAX(cache_write_1h_tokens, excluded.cache_write_1h_tokens)",
                    params![
                        message.message_id,
                        entry.session_id,
                        prompt_id,
                        agent_id,
                        message.agent_type,
                        message.model,
                        at_us,
                        message.usage.input_tokens,
                        message.usage.output_tokens,
                        message.usage.cache_creation_input_tokens,
                        message.usage.cache_read_input_tokens,
                        message.skill,
                        message.usage.cache_write_1h_tokens(),
                    ],
                )?;
            }
        }
        Kind::Other => {}
    }
    Ok(())
}

fn parent_prompt(conn: &Connection, parent: Option<&str>) -> Result<Option<String>> {
    let Some(parent) = parent else {
        return Ok(None);
    };
    Ok(conn
        .query_row(
            "SELECT prompt_id FROM transcript_entries WHERE uuid = ?1",
            [parent],
            |row| row.get(0),
        )
        .optional()?
        .flatten())
}

/// Widens the session's time span; cwd is the first seen, branch and
/// version follow the latest entry.
fn upsert_session(conn: &Connection, entry: &Entry, at_us: i64) -> Result<()> {
    conn.execute(
        "INSERT INTO sessions (session_id, cwd, git_branch, version, first_at_us, last_at_us)
         VALUES (?1, ?2, ?3, ?4, ?5, ?5)
         ON CONFLICT(session_id) DO UPDATE SET
             cwd         = COALESCE(cwd, excluded.cwd),
             git_branch  = CASE WHEN last_at_us IS NULL OR excluded.last_at_us >= last_at_us
                                THEN COALESCE(excluded.git_branch, git_branch)
                                ELSE COALESCE(git_branch, excluded.git_branch) END,
             version     = CASE WHEN last_at_us IS NULL OR excluded.last_at_us >= last_at_us
                                THEN COALESCE(excluded.version, version)
                                ELSE COALESCE(version, excluded.version) END,
             first_at_us = MIN(COALESCE(first_at_us, excluded.first_at_us), excluded.first_at_us),
             last_at_us  = MAX(COALESCE(last_at_us, excluded.last_at_us), excluded.last_at_us)",
        params![
            entry.session_id,
            entry.cwd,
            entry.git_branch.as_deref().filter(|b| !b.is_empty()),
            entry.version,
            at_us
        ],
    )?;
    Ok(())
}

/// Widens the turn's transcript time span and fills its prompt details.
fn upsert_turn(
    conn: &Connection,
    entry: &Entry,
    prompt_id: &str,
    at_us: i64,
    prompt_text: Option<&str>,
) -> Result<()> {
    conn.execute(
        "INSERT INTO turns (session_id, prompt_id, prompt_text, permission_mode, effort,
                            start_at_us, end_at_us)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)
         ON CONFLICT(session_id, prompt_id) DO UPDATE SET
             prompt_text     = COALESCE(prompt_text, excluded.prompt_text),
             permission_mode = COALESCE(permission_mode, excluded.permission_mode),
             effort          = COALESCE(effort, excluded.effort),
             start_at_us     = MIN(COALESCE(start_at_us, excluded.start_at_us), excluded.start_at_us),
             end_at_us       = MAX(COALESCE(end_at_us, excluded.end_at_us), excluded.end_at_us)",
        params![
            entry.session_id,
            prompt_id,
            prompt_text,
            entry.permission_mode,
            entry.effort,
            at_us
        ],
    )?;
    Ok(())
}
