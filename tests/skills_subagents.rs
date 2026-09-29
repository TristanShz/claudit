//! Skills and subagents (#9).
//!
//! Seams under test (seam 1 of `tests/common`): hook fixtures replayed
//! through `claudit::hook::run`, fixture transcripts (main + subagent)
//! dropped in place, `claudit::ingest::run`, then assertions on the typed
//! reports `stats::skills::skill_ranking`, `stats::subagents::{
//! subagent_ranking, subagent_runs}` and `stats::time::turn_times` only.

mod common;

use chrono::{DateTime, Duration, Utc};
use claudit::stats::Filter;
use claudit::stats::consumption::TokenTotals;
use claudit::stats::skills::SkillStat;
use claudit::stats::subagents::SubagentTypeStat;
use common::{TestEnv, t0};
use serde_json::json;

const ACME: &str = "-Users-alice-code-acme-api";
const SESSION_A: &str = "8d0c5a3e-1b2f-4c6d-9e7a-0f1b2c3d4e5f";
const TURN_A2: &str = "a1b2c3d4-0002-4000-8000-000000000002";
const AGENT: &str = "a1f3c5e7b9d2c4e6f";
const HAIKU: &str = "claude-haiku-4-5-20251001";

fn at(offset_ms: i64) -> DateTime<Utc> {
    t0() + Duration::milliseconds(offset_ms)
}

fn drop_session_a_transcripts(env: &TestEnv) {
    env.drop_transcript_fixture(&format!("{ACME}/{SESSION_A}.jsonl"));
    env.drop_transcript_fixture(&format!("{ACME}/{SESSION_A}/subagents/agent-{AGENT}.jsonl"));
}

/// Session A with hooks and transcripts.
fn session_a() -> TestEnv {
    let env = TestEnv::new();
    drop_session_a_transcripts(&env);
    env.replay_session_a_hooks();
    env.ingest();
    env
}

/// Tokens of the fixture subagent transcript (see its README).
fn subagent_tokens() -> TokenTotals {
    TokenTotals {
        input: 6,
        output: 300,
        cache_write: 4300,
        cache_read: 4000,
    }
}

fn skill<'a>(skills: &'a [SkillStat], name: &str) -> &'a SkillStat {
    skills
        .iter()
        .find(|s| s.skill == name)
        .unwrap_or_else(|| panic!("skill {name} missing from {skills:#?}"))
}

#[test]
fn a_typed_slash_skill_and_a_model_skill_tool_call_are_counted_with_their_trigger() {
    let env = session_a();
    // Another session: Claude calls the Skill tool during a turn 0–20 s.
    let turn = |event: &str, at_ms: i64| {
        env.at(at(at_ms)).hook(&json!({
            "session_id": "3f2b8c1e-7d4a-4e5b-9c6f-1a2b3c4d5e6f",
            "prompt_id": "b1e2c3d4-5f60-4a7b-8c9d-0e1f2a3b4c5d",
            "cwd": "/Users/alice/code/acme-api",
            "hook_event_name": event,
            "prompt": "write the release notes",
        }));
    };
    turn("UserPromptSubmit", 0);
    env.hook_fixture_at(at(5_000), "post_tool_use_skill.json");
    turn("Stop", 20_000);
    env.ingest();

    let skills = env.skills(&Filter::default());
    assert_eq!(skills.len(), 2, "{skills:#?}");

    let typed = skill(&skills, "code-review");
    assert_eq!((typed.user_invocations, typed.model_invocations), (1, 0));
    let invoked = skill(&skills, "release-notes");
    assert_eq!(
        (invoked.user_invocations, invoked.model_invocations),
        (0, 1)
    );
}

#[test]
fn a_skill_is_attributed_the_tokens_and_time_after_its_invocation() {
    let env = session_a();
    let turn = |event: &str, at_ms: i64| {
        env.at(at(at_ms)).hook(&json!({
            "session_id": "3f2b8c1e-7d4a-4e5b-9c6f-1a2b3c4d5e6f",
            "prompt_id": "b1e2c3d4-5f60-4a7b-8c9d-0e1f2a3b4c5d",
            "hook_event_name": event,
        }));
    };
    turn("UserPromptSubmit", 0);
    env.hook_fixture_at(at(5_000), "post_tool_use_skill.json");
    turn("Stop", 20_000);
    env.ingest();

    let skills = env.skills(&Filter::default());
    // Typed at the start of turn 2 (submit 300 s, stop 380.3 s): the whole
    // turn; its tokens are the two `attributionSkill: code-review` messages.
    let typed = skill(&skills, "code-review");
    assert_eq!(typed.attributed_time, Duration::milliseconds(80_300));
    assert_eq!(
        typed.tokens,
        TokenTotals {
            input: 3,
            output: 650,
            cache_write: 2100,
            cache_read: 43300,
        }
    );
    // Invoked by Claude at 5 s in a turn that stops at 20 s.
    let invoked = skill(&skills, "release-notes");
    assert_eq!(invoked.attributed_time, Duration::seconds(15));
    assert_eq!(invoked.tokens, TokenTotals::default());
}

#[test]
fn tool_calls_inside_a_subagent_are_attributed_to_it_not_to_the_main_thread() {
    let env = TestEnv::new();
    drop_session_a_transcripts(&env);
    // Without Claude's own tool count, the count comes from the hooks.
    env.replay_session_a_hooks_with(|name, p| {
        if name == "post_tool_use_agent.json" {
            p["tool_response"]
                .as_object_mut()
                .unwrap()
                .remove("totalToolUseCount");
        }
    });
    env.ingest();

    let runs = env.subagent_runs(&Filter::default());
    assert_eq!(runs.len(), 1, "{runs:#?}");
    assert_eq!(runs[0].agent_id, AGENT);
    assert_eq!(runs[0].tool_calls, 1);

    let turns = env.turn_times(&Filter::default());
    let t2 = turns.iter().find(|t| t.prompt_id == TURN_A2).unwrap();
    assert_eq!(t2.split.tool, Duration::zero());
    assert_eq!(t2.split.subagent, Duration::milliseconds(63_400));
}

#[test]
fn subagents_are_ranked_by_type_with_duration_model_and_transcript_tokens() {
    let env = session_a();

    assert_eq!(
        env.subagents(&Filter::default()),
        [SubagentTypeStat {
            agent_type: "general-purpose".into(),
            runs: 1,
            total_duration: Duration::milliseconds(63_400),
            tool_calls: 1,
            model: Some(HAIKU.into()),
            tokens: subagent_tokens(),
        }]
    );
    let runs = env.subagent_runs(&Filter::default());
    assert_eq!(
        runs[0].parent_tool_use_id.as_deref(),
        Some("toolu_01AcmeAgent0002")
    );
    assert_eq!(runs[0].prompt_id.as_deref(), Some(TURN_A2));
    assert_eq!(runs[0].started_at, Some(at(302_200)));
}

#[test]
fn a_subagent_known_only_from_its_transcript_is_still_reported() {
    let env = TestEnv::new();
    drop_session_a_transcripts(&env);
    env.ingest();

    // First to last subagent entry: 09:05:03 → 09:06:05.
    assert_eq!(
        env.subagents(&Filter::default()),
        [SubagentTypeStat {
            agent_type: "general-purpose".into(),
            runs: 1,
            total_duration: Duration::seconds(62),
            tool_calls: 0,
            model: Some(HAIKU.into()),
            tokens: subagent_tokens(),
        }]
    );
}

#[test]
fn replaying_the_same_hooks_twice_changes_nothing() {
    let env = session_a();
    let skills = env.skills(&Filter::default());
    let subagents = env.subagents(&Filter::default());

    env.replay_session_a_hooks();
    env.ingest();

    assert_eq!(env.skills(&Filter::default()), skills);
    assert_eq!(env.subagents(&Filter::default()), subagents);
}

#[test]
fn filters_apply_to_skills_and_subagents() {
    let env = session_a();
    let elsewhere = Filter {
        project: Some("/Users/alice/code/web-app".into()),
        ..Filter::default()
    };
    assert!(env.skills(&elsewhere).is_empty());
    assert!(env.subagents(&elsewhere).is_empty());

    let other_model = Filter {
        model: Some("claude-sonnet-5".into()),
        ..Filter::default()
    };
    assert!(env.subagents(&other_model).is_empty());
    let haiku = Filter {
        model: Some(HAIKU.into()),
        ..Filter::default()
    };
    assert_eq!(env.subagents(&haiku).len(), 1);
}
