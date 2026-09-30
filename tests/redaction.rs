//! Secret redaction and dropping of tool outputs (#7).
//!
//! Seams under test:
//! - the pure redaction API `claudit::redact` (`redact_str` over the
//!   versioned pattern list, `sanitize_hook_payload` for what ingest keeps
//!   of a hook payload);
//! - seam 1 end to end (hook payloads + transcripts in, ingest). The one
//!   sanctioned look inside the database is `every_stored_text`, which dumps
//!   every text/blob cell of every table: "no secret anywhere in the
//!   database" cannot be observed through a stats report.

mod common;

use claudit::redact::{self, REDACTED};
use claudit::stats::Filter;
use common::TestEnv;
use serde_json::{Value, json};

/// One made-up secret per pattern family, as they would appear in a prompt.
const SECRETS: &[&str] = &[
    "sk-ant-api03-Zx9Qw8Er7Ty6Ui5Op4As3Df2Gh1Jk0LzXcVbNm-AbCdEf",
    "sk-proj-4f9KdL2mQ8rT1vW6yZ0aB3cE5gH7jN9p",
    "ghp_16C7e42F292c6912E7710c838347Ae178B4a",
    "gho_16C7e42F292c6912E7710c838347Ae178B4b",
    "ghs_16C7e42F292c6912E7710c838347Ae178B4c",
    "github_pat_11ABCDEFG0123456789_abcdefghijklmnopqrstuvwxyzABCDEF",
    "AKIAIOSFODNN7EXAMPLE",
    // A fake JWT, split so secret scanners don't flag the fixture itself.
    concat!(
        "eyJhbGciOiJIUzI1NiJ9",
        ".",
        "eyJzdWIiOiJhbGljZSJ9",
        ".",
        "c2lnbmF0dXJl"
    ),
    "hunter2-db-password",
    "s3cr3t-webhook-value",
    "tok-internal-9f8e7d",
    "json-password-value",
];

/// Text embedding every secret of [`SECRETS`] in its natural context.
fn secret_text() -> String {
    format!(
        "export ANTHROPIC_API_KEY={} && OPENAI_KEY={} \
         GITHUB_TOKEN={} gho {} ghs {} pat {} aws {} \
         curl -H 'Authorization: Bearer {}' DB_PASSWORD={} \
         WEBHOOK_SECRET=\"{}\" SLACK_TOKEN={} body {{\"password\": \"{}\"}}",
        SECRETS[0],
        SECRETS[1],
        SECRETS[2],
        SECRETS[3],
        SECRETS[4],
        SECRETS[5],
        SECRETS[6],
        SECRETS[7],
        SECRETS[8],
        SECRETS[9],
        SECRETS[10],
        SECRETS[11],
    )
}

fn assert_no_secret(text: &str) {
    for secret in SECRETS {
        assert!(
            !text.contains(secret),
            "secret {secret:?} survived in {text}"
        );
    }
}

// ---- the pattern list -------------------------------------------------------

#[test]
fn every_secret_pattern_is_replaced_with_the_redaction_marker() {
    let text = secret_text();
    let redacted = redact::redact_str(&text);
    assert_no_secret(&redacted);
    assert!(redacted.contains(REDACTED));
}

#[test]
fn known_token_shapes_are_redacted_standalone() {
    assert_eq!(
        redact::redact_str(&format!("key {} end", SECRETS[0])),
        "key [REDACTED] end"
    );
    assert_eq!(redact::redact_str(SECRETS[2]), "[REDACTED]");
    assert_eq!(redact::redact_str(SECRETS[5]), "[REDACTED]");
    assert_eq!(
        redact::redact_str(&format!("id={}", SECRETS[6])),
        "id=[REDACTED]"
    );
    assert_eq!(
        redact::redact_str(&format!("Authorization: Bearer {}", SECRETS[7])),
        "Authorization: Bearer [REDACTED]"
    );
}

#[test]
fn assignments_keep_the_name_and_hide_the_value() {
    assert_eq!(
        redact::redact_str("DB_PASSWORD=hunter2 cargo run"),
        "DB_PASSWORD=[REDACTED] cargo run"
    );
    assert_eq!(
        redact::redact_str("export api_key='abc def' && ls"),
        "export api_key=[REDACTED] && ls"
    );
    assert_eq!(
        redact::redact_str(r#"{"client_secret": "abc", "user": "alice"}"#),
        r#"{"client_secret": "[REDACTED]", "user": "alice"}"#
    );
}

#[test]
fn ordinary_text_is_left_untouched() {
    for text in [
        "cargo test --workspace",
        "Run the test suite and tell me what fails",
        "git commit -m 'task-list: fix the skeleton'",
        "/Users/alice/code/acme-api/src/login.rs",
        "The keyboard shortcut is ctrl+k",
    ] {
        assert_eq!(redact::redact_str(text), text);
    }
}

#[test]
fn redaction_is_idempotent() {
    let once = redact::redact_str(&secret_text()).into_owned();
    assert_eq!(redact::redact_str(&once), once);
}

#[test]
fn the_pattern_list_is_versioned() {
    assert!(redact::patterns_version() >= 1);
}

// ---- what ingest keeps of a hook payload ------------------------------------

#[test]
fn secrets_are_redacted_in_every_string_of_a_payload() {
    let payload = json!({
        "hook_event_name": "PreToolUse",
        "tool_input": {
            "command": format!("GITHUB_TOKEN={} gh pr list", SECRETS[2]),
            "env": [format!("AWS={}", SECRETS[6])],
            "nested": { "password": "plain-words" },
        },
    });
    let clean = redact::sanitize_hook_payload(payload);
    assert_no_secret(&clean.to_string());
    assert_eq!(
        clean["tool_input"]["command"],
        "GITHUB_TOKEN=[REDACTED] gh pr list"
    );
    assert_eq!(clean["tool_input"]["nested"]["password"], REDACTED);
    assert_eq!(clean["hook_event_name"], "PreToolUse");
}

#[test]
fn tool_outputs_are_dropped_but_input_and_error_are_kept() {
    let payload = common::hook_fixture("post_tool_use_bash.json");
    let clean = redact::sanitize_hook_payload(payload.clone());
    assert!(clean.get("tool_response").is_none(), "{clean}");
    assert_eq!(clean["tool_input"], payload["tool_input"]);
    assert_eq!(clean["duration_ms"], payload["duration_ms"]);

    let failure = json!({
        "hook_event_name": "PostToolUseFailure",
        "tool_response": null,
        "error": "Command failed with exit code 1",
    });
    let clean = redact::sanitize_hook_payload(failure);
    assert_eq!(clean["error"], "Command failed with exit code 1");
}

#[test]
fn only_allow_listed_agent_totals_survive_from_a_tool_response() {
    let payload = common::hook_fixture("post_tool_use_agent.json");
    let clean = redact::sanitize_hook_payload(payload);
    assert_eq!(
        clean["tool_response"],
        json!({
            "status": "completed",
            "agentId": "a1f3c5e7b9d2c4e6f",
            "totalDurationMs": 41250,
            "totalTokens": 18342,
            "totalToolUseCount": 7,
            "usage": {
                "input_tokens": 12,
                "cache_creation_input_tokens": 5400,
                "cache_read_input_tokens": 12100,
                "output_tokens": 830
            }
        })
    );
}

#[test]
fn the_subagent_type_and_model_survive_from_an_agent_response() {
    let payload = common::hook_fixture("post_tool_use_agent_review.json");
    let clean = redact::sanitize_hook_payload(payload);
    let response = &clean["tool_response"];
    assert_eq!(response["agentType"], "general-purpose");
    assert_eq!(response["resolvedModel"], "claude-haiku-4-5-20251001");
    assert!(response.get("toolStats").is_none(), "{response}");
    assert!(response.get("prompt").is_none(), "{response}");
}

#[test]
fn assistant_response_text_is_dropped_from_stop_events() {
    for event in ["Stop", "SubagentStop"] {
        let clean = redact::sanitize_hook_payload(json!({
            "hook_event_name": event,
            "session_id": "s",
            "last_assistant_message": "Here is the full answer",
        }));
        assert_eq!(
            clean,
            json!({ "hook_event_name": event, "session_id": "s" })
        );
    }
}

#[test]
fn batched_tool_outputs_are_dropped_too() {
    let clean = redact::sanitize_hook_payload(json!({
        "hook_event_name": "PostToolBatch",
        "tool_calls": [
            { "tool_name": "Read", "tool_input": { "file_path": "/a" }, "tool_response": "secret file" }
        ],
    }));
    assert_eq!(
        clean["tool_calls"],
        json!([{ "tool_name": "Read", "tool_input": { "file_path": "/a" } }])
    );
}

// ---- end to end: nothing reaches the database -------------------------------

const SESSION: &str = "3f2b8c1e-7d4a-4e5b-9c6f-1a2b3c4d5e6f";
const PROJECT: &str = "-Users-alice-code-acme-api";

/// A working directory and a branch name with secrets in them: transcript
/// entries carry both on every line.
fn secret_cwd() -> String {
    format!("/Users/alice/code/{}", SECRETS[6])
}

fn secret_branch() -> String {
    format!("fix/{}", SECRETS[2])
}

fn transcript_with_secret_prompt() -> String {
    let user = json!({
        "parentUuid": null, "isSidechain": false,
        "promptId": "b1e2c3d4-5f60-4a7b-8c9d-0e1f2a3b4c5d",
        "type": "user",
        "message": { "role": "user", "content": secret_text() },
        "permissionMode": "default",
        "uuid": "f0000001-0000-4000-8000-000000000001",
        "timestamp": "2026-03-02T09:00:00.000Z",
        "cwd": secret_cwd(), "sessionId": SESSION,
        "version": "2.1.284", "gitBranch": secret_branch()
    });
    let assistant = json!({
        "parentUuid": "f0000001-0000-4000-8000-000000000001", "isSidechain": false,
        "type": "assistant",
        "message": {
            "model": "claude-sonnet-4-6", "id": "msg_01Secret0001", "role": "assistant",
            "content": [{ "type": "text", "text": format!("Sure, using {}", SECRETS[0]) }],
            "usage": { "input_tokens": 3, "output_tokens": 40,
                       "cache_creation_input_tokens": 100, "cache_read_input_tokens": 0 }
        },
        "uuid": "f0000002-0000-4000-8000-000000000002",
        "timestamp": "2026-03-02T09:00:05.000Z",
        "cwd": secret_cwd(), "sessionId": SESSION,
        "version": "2.1.284", "gitBranch": secret_branch()
    });
    format!("{user}\n{assistant}\n")
}

/// Hook payloads carrying every secret in a prompt, a Bash command, a tool
/// input and a tool output, a permission request, a notification, skill
/// arguments (typed and model-invoked), a subagent's prompt and answer, a
/// working directory and a payload that is not valid JSON; plus a transcript
/// whose prompt, working directory and branch carry them.
fn record_a_leaky_session(env: &TestEnv) {
    env.hook(&json!({
        "session_id": SESSION,
        "prompt_id": "b1e2c3d4-5f60-4a7b-8c9d-0e1f2a3b4c5d",
        "cwd": "/Users/alice/code/acme-api",
        "hook_event_name": "UserPromptSubmit",
        "prompt": secret_text(),
    }));
    env.hook_fixture_with("post_tool_use_bash.json", |p| {
        p["tool_input"]["command"] = Value::String(secret_text());
        p["tool_response"]["stdout"] = Value::String(secret_text());
    });
    env.hook_fixture_with("post_tool_use_read.json", |p| {
        p["cwd"] = Value::String(secret_cwd());
        p["tool_input"]["file_path"] = Value::String(format!("/tmp/{}", SECRETS[2]));
        p["tool_input"]["api_key"] = Value::String(SECRETS[9].to_owned());
    });
    env.hook_fixture_with("pre_tool_use_bash.json", |p| {
        p["session_id"] = json!(SESSION);
        p["tool_input"]["command"] = Value::String(secret_text());
    });
    env.hook_fixture_with("permission_request_bash.json", |p| {
        p["session_id"] = json!(SESSION);
        p["tool_input"]["command"] = Value::String(secret_text());
    });
    env.hook_fixture_with("notification_permission_prompt.json", |p| {
        p["session_id"] = json!(SESSION);
        p["message"] = Value::String(secret_text());
    });
    env.hook_fixture_with("user_prompt_expansion_skill.json", |p| {
        p["session_id"] = json!(SESSION);
        p["command_args"] = Value::String(secret_text());
        p["prompt"] = Value::String(format!("/code-review {}", secret_text()));
    });
    env.hook_fixture_with("post_tool_use_skill.json", |p| {
        p["tool_input"]["args"] = Value::String(secret_text());
    });
    env.hook_fixture_with("post_tool_use_agent_review.json", |p| {
        p["session_id"] = json!(SESSION);
        p["tool_input"]["prompt"] = Value::String(secret_text());
        p["tool_response"]["prompt"] = Value::String(secret_text());
        p["tool_response"]["content"][0]["text"] = Value::String(secret_text());
    });
    env.hook(&json!({
        "session_id": SESSION,
        "agent_id": "a1f3c5e7b9d2c4e6f",
        "cwd": "/Users/alice/code/acme-api",
        "hook_event_name": "SubagentStop",
        "last_assistant_message": secret_text(),
    }));
    env.hook(&json!({
        "session_id": SESSION,
        "cwd": "/Users/alice/code/acme-api",
        "hook_event_name": "Stop",
        "last_assistant_message": secret_text(),
    }));
    env.hook_raw(format!("{{\"session_id\": \"{SESSION}\", {}", secret_text()).as_bytes());
    env.drop_transcript(
        &format!("{PROJECT}/{SESSION}.jsonl"),
        &transcript_with_secret_prompt(),
    );
}

/// Every text and blob cell of every table of the archive.
fn every_stored_text(env: &TestEnv) -> String {
    let conn = env.db();
    let tables: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let mut dump = String::new();
    for table in tables {
        let mut stmt = conn.prepare(&format!("SELECT * FROM \"{table}\"")).unwrap();
        let columns = stmt.column_count();
        let mut rows = stmt.query([]).unwrap();
        while let Some(row) = rows.next().unwrap() {
            for i in 0..columns {
                match row.get_ref(i).unwrap() {
                    rusqlite::types::ValueRef::Text(t) | rusqlite::types::ValueRef::Blob(t) => {
                        dump.push_str(&String::from_utf8_lossy(t));
                        dump.push('\n');
                    }
                    _ => {}
                }
            }
        }
    }
    dump
}

#[test]
fn no_secret_reaches_the_database() {
    let env = TestEnv::new();
    record_a_leaky_session(&env);

    env.ingest();

    let dump = every_stored_text(&env);
    assert!(dump.contains("hook_event_name"), "raw events were archived");
    assert!(dump.contains(REDACTED), "redacted text was stored");
    assert_no_secret(&dump);
    assert!(!dump.contains("running 12 tests"), "tool output was stored");
    // Redacted data is still analysed.
    let tools = env.tool_ranking(&Filter::default());
    assert_eq!(tools.len(), 4);
    let skills: Vec<_> = env
        .skills(&Filter::default())
        .into_iter()
        .map(|s| s.skill)
        .collect();
    assert_eq!(skills.len(), 2, "{skills:?}");
    let runs = env.subagent_runs(&Filter::default());
    assert_eq!(runs[0].agent_type, "general-purpose");
    let sessions = env.sessions(&Filter::default());
    assert!(
        sessions[0]
            .first_prompt
            .as_deref()
            .unwrap()
            .contains("ANTHROPIC_API_KEY=[REDACTED]")
    );
}

/// Output text only a tool result carries.
const TOOL_RESULT_MARKER: &str = "tool-result-body-4e1f9a";

/// A transcript whose Bash call carries every secret in its input, and whose
/// failed result carries them (and [`TOOL_RESULT_MARKER`]) in its output.
fn transcript_with_secret_tool_call() -> String {
    let envelope = |uuid: &str, parent: Option<&str>, timestamp: &str| {
        json!({
            "parentUuid": parent, "isSidechain": false, "uuid": uuid,
            "timestamp": timestamp, "cwd": "/Users/alice/code/acme-api",
            "sessionId": SESSION, "version": "2.1.284", "gitBranch": "main"
        })
    };
    let mut prompt = envelope(
        "f1000001-0000-4000-8000-000000000001",
        None,
        "2026-03-02T09:00:00.000Z",
    );
    prompt["type"] = json!("user");
    prompt["promptId"] = json!("b1e2c3d4-5f60-4a7b-8c9d-0e1f2a3b4c5d");
    prompt["message"] = json!({ "role": "user", "content": "Deploy it" });
    let mut call = envelope(
        "f1000002-0000-4000-8000-000000000002",
        Some("f1000001-0000-4000-8000-000000000001"),
        "2026-03-02T09:00:02.000Z",
    );
    call["type"] = json!("assistant");
    call["message"] = json!({
        "model": "claude-sonnet-4-6", "id": "msg_01SecretCall0001", "role": "assistant",
        "content": [{
            "type": "tool_use", "id": "toolu_01SecretBash0001", "name": "Bash",
            "input": { "command": secret_text(), "api_key": SECRETS[9] }
        }],
        "usage": { "input_tokens": 3, "output_tokens": 40,
                   "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0 }
    });
    let mut result = envelope(
        "f1000003-0000-4000-8000-000000000003",
        Some("f1000002-0000-4000-8000-000000000002"),
        "2026-03-02T09:00:04.000Z",
    );
    result["type"] = json!("user");
    result["promptId"] = json!("b1e2c3d4-5f60-4a7b-8c9d-0e1f2a3b4c5d");
    let output = format!("{TOOL_RESULT_MARKER} {}", secret_text());
    result["message"] = json!({ "role": "user", "content": [{
        "type": "tool_result", "tool_use_id": "toolu_01SecretBash0001",
        "is_error": true, "content": output
    }]});
    result["toolUseResult"] = json!(format!("Error: {output}"));
    format!("{prompt}\n{call}\n{result}\n")
}

#[test]
fn no_transcript_tool_input_secret_or_tool_result_reaches_the_database() {
    let env = TestEnv::new();
    env.drop_transcript(
        &format!("{PROJECT}/{SESSION}.jsonl"),
        &transcript_with_secret_tool_call(),
    );
    env.ingest();

    let dump = every_stored_text(&env);
    assert_no_secret(&dump);
    assert!(
        !dump.contains(TOOL_RESULT_MARKER),
        "tool result content was stored"
    );
    // The redacted call is still analysed, as a failure.
    let tools = env.tool_ranking(&Filter::default());
    assert_eq!(tools.len(), 1, "{tools:#?}");
    assert_eq!(
        (tools[0].name.as_str(), tools[0].stats.failures),
        ("Bash", 1)
    );

    claudit::ingest::reingest(&env.paths).expect("reingest succeeds");
    let dump = every_stored_text(&env);
    assert_no_secret(&dump);
    assert!(!dump.contains(TOOL_RESULT_MARKER));
}

#[test]
fn no_secret_reaches_the_database_after_a_reingest() {
    let env = TestEnv::new();
    record_a_leaky_session(&env);
    env.ingest();

    claudit::ingest::reingest(&env.paths).expect("reingest succeeds");

    assert_no_secret(&every_stored_text(&env));
}
