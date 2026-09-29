//! Parsing one transcript line into the few facts claudit keeps.
//!
//! The transcript format is undocumented, so parsing is tolerant and
//! selective. Observed on Claude Code 2.1.x:
//! - Entries of type `user`, `assistant`, `system` and `attachment` share an
//!   envelope: `uuid`, `parentUuid`, `sessionId`, `timestamp`, `cwd`,
//!   `gitBranch`, `version`, and in subagent transcripts `agentId`.
//! - `promptId` (the turn) is on `user` entries only; other entries belong
//!   to their parent's turn.
//! - An API response may be split over several `assistant` entries sharing
//!   `message.id`, each repeating `message.usage`.
//! - Many other line types (`mode`, `last-prompt`, `ai-title`,
//!   `file-history-snapshot`, `queue-operation`, …) carry nothing claudit
//!   uses and are ignored.
//!
//! Assistant response text and tool results are never extracted; prompt
//! text is redacted (`crate::redact`) as soon as it is extracted.

use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::Value;

use crate::redact;

/// What a line turned out to be.
#[derive(Debug)]
pub(super) enum Line {
    Entry(Box<Entry>),
    /// A well-formed line of a type claudit has no use for.
    Ignored,
    /// A line claudit could not make sense of (the reason, for the log).
    Unknown(String),
}

/// The envelope fields of a conversation entry, plus its kind.
#[derive(Debug)]
pub(super) struct Entry {
    pub uuid: String,
    pub parent_uuid: Option<String>,
    pub session_id: String,
    pub at: DateTime<Utc>,
    pub cwd: Option<String>,
    pub git_branch: Option<String>,
    pub version: Option<String>,
    pub prompt_id: Option<String>,
    pub agent_id: Option<String>,
    pub permission_mode: Option<String>,
    pub effort: Option<String>,
    pub kind: Kind,
}

#[derive(Debug)]
pub(super) enum Kind {
    /// A `user` entry: a prompt (text is `None` for meta entries such as
    /// skill expansions) or a tool result.
    User { prompt_text: Option<String> },
    /// One `assistant` entry of an API response.
    Assistant(ApiMessage),
    /// `system` and `attachment` entries.
    Other,
}

#[derive(Debug)]
pub(super) struct ApiMessage {
    pub message_id: String,
    pub model: String,
    pub usage: Usage,
    pub skill: Option<String>,
    pub agent_type: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct Usage {
    pub input_tokens: i64,
    pub output_tokens: i64,
    #[serde(default)]
    pub cache_creation_input_tokens: i64,
    #[serde(default)]
    pub cache_read_input_tokens: i64,
    /// The TTL split of `cache_creation_input_tokens`, when reported.
    #[serde(default)]
    pub cache_creation: Option<CacheCreation>,
}

impl Usage {
    /// The cache-write tokens written with a 1-hour TTL; 0 without a split,
    /// so every write then counts as a 5-minute write.
    pub fn cache_write_1h_tokens(&self) -> i64 {
        self.cache_creation
            .as_ref()
            .map_or(0, |split| split.ephemeral_1h_input_tokens)
    }
}

#[derive(Debug, Deserialize)]
pub(super) struct CacheCreation {
    #[serde(default)]
    pub ephemeral_1h_input_tokens: i64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Envelope {
    uuid: String,
    #[serde(default)]
    parent_uuid: Option<String>,
    session_id: String,
    timestamp: DateTime<Utc>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    git_branch: Option<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    prompt_id: Option<String>,
    #[serde(default)]
    agent_id: Option<String>,
    #[serde(default)]
    is_meta: Option<bool>,
    #[serde(default)]
    permission_mode: Option<String>,
    #[serde(default)]
    effort: Option<String>,
    #[serde(default)]
    attribution_skill: Option<String>,
    #[serde(default)]
    attribution_agent: Option<String>,
    #[serde(default)]
    message: Option<Value>,
}

#[derive(Deserialize)]
struct AssistantMessage {
    id: String,
    model: String,
    usage: Usage,
}

/// Parses one (non-empty) transcript line.
pub(super) fn parse(line: &str) -> Line {
    let value: Value = match serde_json::from_str(line) {
        Ok(value) => value,
        Err(err) => return Line::Unknown(format!("not JSON: {err}")),
    };
    let Some(entry_type) = value.get("type").and_then(Value::as_str) else {
        return Line::Unknown("no `type` field".to_owned());
    };
    let entry_type = entry_type.to_owned();
    if !matches!(
        entry_type.as_str(),
        "user" | "assistant" | "system" | "attachment"
    ) {
        return Line::Ignored;
    }
    let envelope: Envelope = match Envelope::deserialize(value) {
        Ok(envelope) => envelope,
        Err(err) => return Line::Unknown(format!("`{entry_type}` entry: {err}")),
    };
    let kind = match entry_type.as_str() {
        "user" => Kind::User {
            prompt_text: if envelope.is_meta == Some(true) {
                None
            } else {
                envelope
                    .message
                    .as_ref()
                    .and_then(prompt_text)
                    .map(|text| redact::redact_str(&text).into_owned())
            },
        },
        "assistant" => {
            let message = envelope
                .message
                .as_ref()
                .map(AssistantMessage::deserialize)
                .transpose();
            match message {
                Ok(Some(message)) => Kind::Assistant(ApiMessage {
                    message_id: message.id,
                    model: message.model,
                    usage: message.usage,
                    skill: envelope.attribution_skill.clone(),
                    agent_type: envelope.attribution_agent.clone(),
                }),
                Ok(None) => return Line::Unknown("`assistant` entry without message".into()),
                Err(err) => return Line::Unknown(format!("`assistant` message: {err}")),
            }
        }
        _ => Kind::Other,
    };
    Line::Entry(Box::new(Entry {
        uuid: envelope.uuid,
        parent_uuid: envelope.parent_uuid,
        session_id: envelope.session_id,
        at: envelope.timestamp,
        cwd: envelope.cwd,
        git_branch: envelope.git_branch,
        version: envelope.version,
        prompt_id: envelope.prompt_id,
        agent_id: envelope.agent_id,
        permission_mode: envelope.permission_mode,
        effort: envelope.effort,
        kind,
    }))
}

/// The text a user typed: a string content, or the text blocks of a list
/// content. Tool results are not prompts.
fn prompt_text(message: &Value) -> Option<String> {
    match message.get("content")? {
        Value::String(text) => Some(text.clone()),
        Value::Array(blocks) => {
            let mut texts = Vec::new();
            for block in blocks {
                match block.get("type").and_then(Value::as_str) {
                    Some("tool_result") => return None,
                    Some("text") => {
                        texts.extend(block.get("text").and_then(Value::as_str));
                    }
                    _ => {}
                }
            }
            (!texts.is_empty()).then(|| texts.join("\n"))
        }
        _ => None,
    }
}
