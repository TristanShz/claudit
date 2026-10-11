//! Readable labels for turn prompts.
//!
//! Not every turn starts with something the user typed: Claude Code also
//! opens turns with text it injects, wrapped in tags. A background task
//! finishing sends a `<task-notification>`, a subagent handing back sends
//! an `<agent-message from="…">`, a typed slash command is stored as
//! `<command-message>` / `<command-name>` / `<command-args>`, and local
//! commands leave `<local-command-stdout>` and similar. Reports label them
//! instead of showing the raw markup, and a session's "first prompt" is its
//! first prompt the user typed (a slash command included), falling back to
//! its first prompt of any kind.

use std::collections::HashMap;

use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{Connection, params_from_iter};
use serde::Serialize;

use super::Filter;
use crate::clock;

/// Where a turn's prompt came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptKind {
    /// Text the user typed.
    Typed,
    /// A slash command (`/name args`) or a `!` shell command the user typed.
    Command,
    /// A subagent's message (its hand-back) to the main thread.
    SubagentMessage,
    /// A background task (command or subagent) finishing.
    TaskNotification,
    /// The output of a local command (`/model`, `!ls`, …).
    LocalOutput,
    /// A scheduled task firing.
    Scheduled,
}

impl PromptKind {
    /// Injected by Claude Code rather than typed by the user.
    pub fn injected(self) -> bool {
        !matches!(self, PromptKind::Typed | PromptKind::Command)
    }
}

/// A prompt as shown: its kind and a one-line text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PromptLabel {
    pub kind: PromptKind,
    /// The typed text as is; for the other kinds a label such as
    /// `/review 12`, `Task notification: …` or `Message from subagent: …`.
    pub text: String,
}

/// SQL over a `turns` alias `t`: 1 when its prompt was injected by Claude
/// Code (the kinds [`PromptKind::injected`] names), else 0. Sorting on it
/// first picks a session's first typed prompt when there is one.
pub(super) const INJECTED_SQL: &str =
    "(CASE WHEN ltrim(t.prompt_text, ' ' || char(9) || char(10) || char(13))
       GLOB '<task-notification>*'
    OR ltrim(t.prompt_text, ' ' || char(9) || char(10) || char(13))
       GLOB '<agent-message*'
    OR t.prompt_text GLOB 'Another Claude session sent a message:*'
    OR ltrim(t.prompt_text, ' ' || char(9) || char(10) || char(13))
       GLOB '<local-command-*'
    OR ltrim(t.prompt_text, ' ' || char(9) || char(10) || char(13))
       GLOB '<bash-std*'
    OR ltrim(t.prompt_text, ' ' || char(9) || char(10) || char(13))
       GLOB '<scheduled-task*'
    THEN 1 ELSE 0 END)";

/// A prompt the user typed (a slash or `!` command included).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypedPrompt {
    pub session_id: String,
    /// The session's working directory.
    pub cwd: Option<String>,
    /// When the turn started (`UserPromptSubmit`, else its first transcript
    /// entry).
    pub at: DateTime<Utc>,
    /// As [`label`] gives it: typed text as is, a command as `/name args`.
    pub text: String,
}

/// The prompts the user typed in the filtered turns, oldest first; the
/// prompts Claude Code injected are left out. Turns are filtered as in
/// [`super::consumption::consumption`].
pub fn typed_prompts(conn: &Connection, filter: &Filter) -> Result<Vec<TypedPrompt>> {
    let where_ = filter.sql(&super::consumption::TURN_COLUMNS)?;
    let sql = format!(
        "SELECT t.session_id, s.cwd, COALESCE(t.submit_at_us, t.start_at_us) AS at, t.prompt_text
         FROM turns t
         LEFT JOIN sessions s ON s.session_id = t.session_id
         LEFT JOIN api_messages m
                ON m.session_id = t.session_id AND m.prompt_id = t.prompt_id
         WHERE t.prompt_text IS NOT NULL AND at IS NOT NULL
           AND NOT {INJECTED_SQL} AND {}
         GROUP BY t.session_id, t.prompt_id
         ORDER BY at, t.session_id, t.prompt_id",
        where_.clause
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(where_.params), |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, String>(3)?,
        ))
    })?;
    let mut prompts = Vec::new();
    for row in rows {
        let (session_id, cwd, at, raw) = row?;
        let label = label(&raw, &HashMap::new());
        if label.kind.injected() {
            continue;
        }
        prompts.push(TypedPrompt {
            session_id,
            cwd,
            at: clock::from_micros(at),
            text: label.text,
        });
    }
    Ok(prompts)
}

/// The label of prompt `raw`. `subagents` maps agent ids to a name (the
/// description of the Agent call, or the agent type) for subagent
/// messages; an unknown agent is named by its id's first 8 characters.
pub fn label(raw: &str, subagents: &HashMap<String, String>) -> PromptLabel {
    let text = raw.trim_start();
    let text = text
        .strip_prefix("Another Claude session sent a message:")
        .map_or(text, str::trim_start);
    let make = |kind, text: String| PromptLabel { kind, text };

    if let Some(rest) = text.strip_prefix("<agent-message") {
        let from = attribute(rest, "from");
        let name = from.map(|id| {
            subagents
                .get(id)
                .cloned()
                .unwrap_or_else(|| id.chars().take(8).collect())
        });
        return make(
            PromptKind::SubagentMessage,
            match name {
                Some(name) => format!("Message from subagent: {name}"),
                None => "Message from a subagent".to_owned(),
            },
        );
    }
    if text.starts_with("<task-notification>") {
        let summary = tag(text, "summary").map(|s| one_line(&unescape(s)));
        return make(
            PromptKind::TaskNotification,
            match summary {
                Some(summary) if !summary.is_empty() => format!("Task notification: {summary}"),
                _ => "Task notification".to_owned(),
            },
        );
    }
    if let Some(rest) = text.strip_prefix("<scheduled-task") {
        return make(
            PromptKind::Scheduled,
            match attribute(rest, "name") {
                Some(name) => format!("Scheduled task: {name}"),
                None => "Scheduled task".to_owned(),
            },
        );
    }
    if text.starts_with("<local-command-") || text.starts_with("<bash-std") {
        return make(PromptKind::LocalOutput, "Local command output".to_owned());
    }
    if let Some(command) = tag(text, "bash-input") {
        return make(PromptKind::Command, format!("! {}", one_line(command)));
    }
    if let Some(name) = tag(text, "command-name") {
        let args = tag(text, "command-args").map(one_line).unwrap_or_default();
        let name = if name.starts_with('/') {
            name.to_owned()
        } else {
            format!("/{name}")
        };
        return make(
            PromptKind::Command,
            if args.is_empty() {
                name
            } else {
                format!("{name} {args}")
            },
        );
    }
    make(PromptKind::Typed, raw.to_owned())
}

/// The trimmed content of the first `<name>…</name>` in `text`.
fn tag<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = text.find(&open)? + open.len();
    let end = start + text[start..].find(&close)?;
    Some(text[start..end].trim())
}

/// The value of `name="…"` in the rest of an opening tag.
fn attribute<'a>(rest: &'a str, name: &str) -> Option<&'a str> {
    let head = &rest[..rest.find('>').unwrap_or(rest.len())];
    let key = format!("{name}=\"");
    let start = head.find(&key)? + key.len();
    let end = start + head[start..].find('"')?;
    Some(&head[start..end]).filter(|v| !v.is_empty())
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Undoes the XML escaping Claude Code applies inside notification tags.
fn unescape(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}
