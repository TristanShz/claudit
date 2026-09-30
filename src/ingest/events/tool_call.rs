//! Every write to `tool_calls`: the completed-call hooks (`PostToolUse`,
//! `PostToolUseFailure`), `PreToolUse`, and the transcripts' `tool_use` /
//! `tool_result` blocks, all upserting one row per `tool_use_id`.
//!
//! Each source records its own timing (`hook_*`, `transcript_*` columns);
//! [`resolve_timing`] then derives the effective `pre_at_us`, `post_at_us`,
//! `duration_ms`, `success` and `timing_source` from them, after every
//! write. The rule, a pure function of the stored facts, so any ingest order
//! (and a reingest) gives the same row:
//! - the hooks saw the call complete (`PostToolUse(Failure)`): hook timing;
//! - else the transcript has its `tool_result`: transcript timing, the
//!   duration estimated as result time − tool_use time (permission prompts
//!   included, so coarser than the hook's `duration_ms`);
//! - else the call is still running (or was interrupted): only a `pre`.

use anyhow::Result;
use rusqlite::{Connection, params};
use serde::Deserialize;
use serde_json::Value;

use super::{Projection, RawEvent};

/// The fields of a completed tool call (success or failure).
#[derive(Debug, Deserialize)]
struct CompletedCall {
    tool_use_id: String,
    tool_name: String,
    #[serde(default)]
    tool_input: Option<Value>,
    #[serde(default)]
    duration_ms: Option<i64>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    prompt_id: Option<String>,
    #[serde(default)]
    agent_id: Option<String>,
    /// `PostToolUseFailure` only.
    #[serde(default)]
    error: Option<String>,
}

/// How the call ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Outcome {
    Success,
    Failure,
}

/// Upserts the call on `tool_use_id`. On duplicate delivery the earliest
/// receive time wins, so replay order never changes the result.
pub(super) fn project(conn: &Connection, event: &RawEvent, outcome: Outcome) -> Result<Projection> {
    let call: CompletedCall = match event.parse() {
        Ok(call) => call,
        Err(malformed) => return Ok(malformed),
    };
    let (bash_command, mcp_server) = derived_columns(&call.tool_name, call.tool_input.as_ref());
    let (success, error) = match outcome {
        Outcome::Success => (1, None),
        Outcome::Failure => (0, call.error),
    };
    conn.execute(
        "INSERT INTO tool_calls (
             tool_use_id, session_id, prompt_id, agent_id, tool_name, mcp_server,
             bash_command, tool_input, cwd, hook_post_at_us, hook_duration_ms, hook_success, error
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
         ON CONFLICT(tool_use_id) DO UPDATE SET
             session_id       = excluded.session_id,
             prompt_id        = COALESCE(excluded.prompt_id, prompt_id),
             agent_id         = COALESCE(excluded.agent_id, agent_id),
             tool_name        = excluded.tool_name,
             mcp_server       = excluded.mcp_server,
             bash_command     = excluded.bash_command,
             tool_input       = COALESCE(excluded.tool_input, tool_input),
             cwd              = COALESCE(excluded.cwd, cwd),
             hook_post_at_us  = MIN(COALESCE(hook_post_at_us, excluded.hook_post_at_us),
                                    excluded.hook_post_at_us),
             hook_duration_ms = COALESCE(excluded.hook_duration_ms, hook_duration_ms),
             hook_success     = excluded.hook_success,
             error            = excluded.error",
        params![
            call.tool_use_id,
            event.session_id,
            call.prompt_id,
            call.agent_id,
            call.tool_name,
            mcp_server,
            bash_command,
            call.tool_input.map(|input| input.to_string()),
            call.cwd,
            event.received_at_us(),
            call.duration_ms,
            success,
            error,
        ],
    )?;
    resolve_timing(conn, &call.tool_use_id)?;
    Ok(Projection::Applied)
}

/// A `PreToolUse`: the call was announced at `at_us` (earliest delivery
/// wins). Identity columns are only filled when still empty, so either
/// source may come first.
pub(crate) struct AnnouncedCall<'a> {
    pub tool_use_id: &'a str,
    pub session_id: &'a str,
    pub prompt_id: Option<&'a str>,
    pub agent_id: Option<&'a str>,
    pub tool_name: &'a str,
    /// Redacted input JSON.
    pub tool_input: Option<&'a Value>,
    pub cwd: Option<&'a str>,
    pub at_us: i64,
}

/// Records a `PreToolUse` hook.
pub(super) fn record_hook_pre(conn: &Connection, call: &AnnouncedCall) -> Result<()> {
    upsert_announced(conn, call, "hook_pre_at_us")
}

/// Records a transcript `tool_use` block (the entry's timestamp as `at_us`).
pub(crate) fn record_transcript_use(conn: &Connection, call: &AnnouncedCall) -> Result<()> {
    upsert_announced(conn, call, "transcript_pre_at_us")
}

/// Records a transcript `tool_result` block, received at `at_us`. Only
/// whether it is an error is kept, never its content. A result whose
/// `tool_use` was never seen has nothing to attach to and is ignored.
pub(crate) fn record_transcript_result(
    conn: &Connection,
    tool_use_id: &str,
    at_us: i64,
    is_error: bool,
) -> Result<()> {
    conn.prepare_cached(
        "UPDATE tool_calls SET
             transcript_post_at_us = MIN(COALESCE(transcript_post_at_us, ?2), ?2),
             transcript_success    = MIN(COALESCE(transcript_success, ?3), ?3)
         WHERE tool_use_id = ?1",
    )?
    .execute(params![tool_use_id, at_us, i64::from(!is_error)])?;
    resolve_timing(conn, tool_use_id)
}

/// Upserts an announced call, `pre_column` being the source's own `pre`
/// column (a trusted name).
fn upsert_announced(conn: &Connection, call: &AnnouncedCall, pre_column: &str) -> Result<()> {
    let (bash_command, mcp_server) = derived_columns(call.tool_name, call.tool_input);
    conn.prepare_cached(&format!(
        "INSERT INTO tool_calls (
                 tool_use_id, session_id, prompt_id, agent_id, tool_name, mcp_server,
                 bash_command, tool_input, cwd, {pre_column}
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(tool_use_id) DO UPDATE SET
                 prompt_id    = COALESCE(prompt_id, excluded.prompt_id),
                 agent_id     = COALESCE(agent_id, excluded.agent_id),
                 mcp_server   = COALESCE(mcp_server, excluded.mcp_server),
                 bash_command = COALESCE(bash_command, excluded.bash_command),
                 tool_input   = COALESCE(tool_input, excluded.tool_input),
                 cwd          = COALESCE(cwd, excluded.cwd),
                 {pre_column} = MIN(COALESCE({pre_column}, excluded.{pre_column}),
                                    excluded.{pre_column})"
    ))?
    .execute(params![
        call.tool_use_id,
        call.session_id,
        call.prompt_id,
        call.agent_id,
        call.tool_name,
        mcp_server,
        bash_command,
        call.tool_input.map(Value::to_string),
        call.cwd,
        call.at_us,
    ])?;
    resolve_timing(conn, call.tool_use_id)
}

/// Derives the effective timing of a call from what each source recorded
/// (see the module docs).
fn resolve_timing(conn: &Connection, tool_use_id: &str) -> Result<()> {
    conn.prepare_cached(
        "UPDATE tool_calls SET
             timing_source = CASE
                 WHEN hook_post_at_us IS NOT NULL THEN 'hook'
                 WHEN transcript_post_at_us IS NOT NULL THEN 'transcript'
                 WHEN hook_pre_at_us IS NOT NULL THEN 'hook'
                 ELSE 'transcript' END,
             pre_at_us = CASE
                 WHEN hook_post_at_us IS NOT NULL THEN hook_pre_at_us
                 WHEN transcript_post_at_us IS NOT NULL THEN transcript_pre_at_us
                 ELSE COALESCE(hook_pre_at_us, transcript_pre_at_us) END,
             post_at_us = COALESCE(hook_post_at_us, transcript_post_at_us),
             duration_ms = CASE
                 WHEN hook_post_at_us IS NOT NULL THEN hook_duration_ms
                 WHEN transcript_post_at_us IS NOT NULL
                  AND transcript_pre_at_us IS NOT NULL
                     THEN MAX(0, (transcript_post_at_us - transcript_pre_at_us) / 1000) END,
             success = CASE
                 WHEN hook_post_at_us IS NOT NULL THEN hook_success
                 ELSE transcript_success END
         WHERE tool_use_id = ?1",
    )?
    .execute([tool_use_id])?;
    Ok(())
}

/// The Bash leading command and the MCP server of a call, derived from its
/// name and input.
fn derived_columns<'a>(
    tool_name: &'a str,
    tool_input: Option<&Value>,
) -> (Option<String>, Option<&'a str>) {
    let bash_command = match tool_name {
        "Bash" => tool_input
            .and_then(|input| input.get("command"))
            .and_then(Value::as_str)
            .and_then(crate::shell::leading_command),
        _ => None,
    };
    (bash_command, mcp_server(tool_name))
}

/// The server of an MCP tool, from its name `mcp__<server>__<tool>`. Server
/// names may contain single underscores (`mcp__plugin_x_y__tool`), never a
/// double one, so the server is everything up to the next `__`.
fn mcp_server(tool_name: &str) -> Option<&str> {
    let rest = tool_name.strip_prefix("mcp__")?;
    let (server, _tool) = rest.split_once("__")?;
    (!server.is_empty()).then_some(server)
}
