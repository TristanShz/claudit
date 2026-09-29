# Hook payload fixtures

One JSON file per hook payload, named `<event>_<variant>.json` in snake case
(e.g. `post_tool_use_bash.json`). Tests replay them through the same entry
point `claudit hook` uses (`TestEnv::hook_fixture`), optionally overriding
fields (`TestEnv::hook_fixture_with`) to vary ids, tools or durations.

Shapes follow Claude Code 2.1.284 (https://code.claude.com/docs/en/hooks),
anonymized: user `alice`, project `/Users/alice/code/acme-api`, made-up ids.
When upstream shapes change, add new fixtures next to these (suffix the
version, e.g. `post_tool_use_bash.v2_3.json`) instead of replacing them.

## Session `8d0c5a3e-…` (hooks for the transcript fixture)

These fixtures are the hook side of the transcript fixture session
`8d0c5a3e-1b2f-4c6d-9e7a-0f1b2c3d4e5f` (same session, prompt, tool-use and
agent ids). `TestEnv::replay_session_a_hooks` feeds them with these receive
times (offsets from `t0()` = 2026-03-02 09:00:00 UTC):

| Offset (s) | Fixture | Notes |
| --- | --- | --- |
| −0.5 | `session_start_startup.json` | |
| 0 | `user_prompt_submit.json` | turn 1 |
| 4.1 | `pre_tool_use_bash.json` | |
| 4.2 | `permission_request_bash.json` | |
| 10.0 | `notification_permission_prompt.json` | |
| 19.9 | `post_tool_use_bash_after_prompt.json` | `duration_ms` 4000: ran 15.9 → 19.9 s |
| 30.3 | `stop_first_turn.json` | |
| 299.99 | `user_prompt_expansion_skill.json` | turn 2: typed `/code-review` |
| 300.0 | `user_prompt_submit_skill.json` | |
| 302.1 | `pre_tool_use_agent.json` | |
| 302.2 | `subagent_start.json` | agent `a1f3c5e7b9d2c4e6f` |
| 306.1 | `pre_tool_use_subagent_read.json` | inside the subagent (`agent_id`) |
| 306.9 | `post_tool_use_subagent_read.json` | `duration_ms` 700 |
| 365.5 | `subagent_stop.json` | |
| 365.6 | `post_tool_use_agent_review.json` | `duration_ms` 63400; `tool_response` totals |
| 380.3 | `stop_after_subagent.json` | |
| 1800 | `session_end_exit.json` | |

Expected split — turn 1: model 14.5 s, tools 4 s, waiting 11.8 s; turn 2:
model 16.8 s, waiting 0.1 s, subagent 63.4 s.

`post_tool_use_skill.json` (session `3f2b8c1e-…`) is a model-invoked Skill
tool call.
