//! Privacy at ingest: secret redaction and dropping of outputs.
//!
//! Nothing claudit stores may contain a known secret, a tool's output or
//! the assistant's response text. Ingest therefore passes every hook payload
//! through [`sanitize_hook_payload`] and every transcript-derived text
//! through [`redact_str`] before anything reaches SQLite.
//!
//! The secret patterns live in `redaction/patterns.toml`, versioned in the
//! repository and compiled into the binary; see that file for how to edit
//! them.

use std::borrow::Cow;
use std::sync::LazyLock;

use regex::Regex;
use serde::Deserialize;
use serde_json::{Map, Value};

/// What every secret is replaced with.
pub const REDACTED: &str = "[REDACTED]";

/// `meta` key recording the version of the pattern list ingest last used.
pub const META_PATTERNS_VERSION: &str = "redaction_patterns_version";

/// Fields of a `tool_response` kept at ingest; everything else in it (the
/// tool's actual output) is dropped. These are the Agent (`Task`) tool's
/// run summary, used for subagent statistics (#9): identifiers, status,
/// totals and the model the subagent resolved to. `usage` keeps only its
/// numeric members (token counts). Add a field here only if it is a number,
/// a flag or an identifier, never free text.
pub const TOOL_RESPONSE_ALLOWLIST: &[&str] = &[
    "agentId",
    "agentType",
    "status",
    "resolvedModel",
    "totalDurationMs",
    "totalTokens",
    "totalToolUseCount",
    "usage",
];

/// Payload members carrying the assistant's response text, dropped at
/// ingest (`Stop`, `SubagentStop`).
const ASSISTANT_TEXT_FIELDS: &[&str] = &["last_assistant_message"];

const PATTERNS_TOML: &str = include_str!("../redaction/patterns.toml");

#[derive(Deserialize)]
struct PatternFile {
    version: u32,
    pattern: Vec<PatternDef>,
}

#[derive(Deserialize)]
struct PatternDef {
    name: String,
    regex: String,
    #[serde(default)]
    replacement: Option<String>,
}

struct Patterns {
    version: u32,
    rules: Vec<(Regex, String)>,
    /// Object member names whose string value is a secret as a whole
    /// (e.g. `{"password": "…"}` in a tool input).
    secret_name: Regex,
}

static PATTERNS: LazyLock<Patterns> = LazyLock::new(|| {
    // The file is compiled in and covered by tests: a bad pattern is a bug.
    let file: PatternFile =
        toml::from_str(PATTERNS_TOML).expect("redaction/patterns.toml is valid TOML");
    let rules = file
        .pattern
        .into_iter()
        .map(|def| {
            let regex = Regex::new(&def.regex)
                .unwrap_or_else(|err| panic!("redaction pattern {}: {err}", def.name));
            (
                regex,
                def.replacement.unwrap_or_else(|| REDACTED.to_owned()),
            )
        })
        .collect();
    Patterns {
        version: file.version,
        rules,
        secret_name: Regex::new("(?i)secret|key|token|password|passwd").expect("valid regex"),
    }
});

/// The version of the compiled-in pattern list.
pub fn patterns_version() -> u32 {
    PATTERNS.version
}

/// Replaces every known secret in `text` with [`REDACTED`]. Idempotent.
pub fn redact_str(text: &str) -> Cow<'_, str> {
    let mut out = Cow::Borrowed(text);
    for (regex, replacement) in &PATTERNS.rules {
        if let Cow::Owned(replaced) = regex.replace_all(&out, replacement.as_str()) {
            out = Cow::Owned(replaced);
        }
    }
    out
}

/// Redacts every string inside `value`, recursively. A string member whose
/// name suggests a secret (`password`, `api_key`, …) is redacted whole.
pub fn redact_value(value: &mut Value) {
    match value {
        Value::String(text) => {
            if let Cow::Owned(redacted) = redact_str(text) {
                *text = redacted;
            }
        }
        Value::Array(items) => items.iter_mut().for_each(redact_value),
        Value::Object(members) => {
            for (name, member) in members.iter_mut() {
                match member {
                    Value::String(text) if PATTERNS.secret_name.is_match(name) => {
                        *text = REDACTED.to_owned();
                    }
                    _ => redact_value(member),
                }
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

/// What ingest keeps of a hook payload: outputs dropped (`tool_response`
/// reduced to [`TOOL_RESPONSE_ALLOWLIST`], also inside `PostToolBatch`'s
/// `tool_calls`; assistant response text removed), then every string
/// redacted. Idempotent, so replaying archived payloads is safe.
pub fn sanitize_hook_payload(mut payload: Value) -> Value {
    if let Value::Object(members) = &mut payload {
        drop_outputs(members);
        if let Some(Value::Array(calls)) = members.get_mut("tool_calls") {
            for call in calls {
                if let Value::Object(call) = call {
                    drop_outputs(call);
                }
            }
        }
    }
    redact_value(&mut payload);
    payload
}

fn drop_outputs(members: &mut Map<String, Value>) {
    for field in ASSISTANT_TEXT_FIELDS {
        members.remove(*field);
    }
    let Some(response) = members.remove("tool_response") else {
        return;
    };
    let Value::Object(response) = response else {
        // A bare string or array is output as a whole: drop it.
        return;
    };
    let kept: Map<String, Value> = response
        .into_iter()
        .filter(|(name, _)| TOOL_RESPONSE_ALLOWLIST.contains(&name.as_str()))
        .filter_map(|(name, value)| summary_value(value).map(|value| (name, value)))
        .collect();
    if !kept.is_empty() {
        members.insert("tool_response".to_owned(), Value::Object(kept));
    }
}

/// Keeps scalars and, for objects, their numeric members only.
fn summary_value(value: Value) -> Option<Value> {
    match value {
        Value::Object(members) => {
            let numbers: Map<String, Value> =
                members.into_iter().filter(|(_, v)| v.is_number()).collect();
            (!numbers.is_empty()).then_some(Value::Object(numbers))
        }
        Value::Array(_) => None,
        scalar => Some(scalar),
    }
}
