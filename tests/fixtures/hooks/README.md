# Hook payload fixtures

One JSON file per hook payload, named `<event>_<variant>.json` in snake case
(e.g. `post_tool_use_bash.json`). Tests replay them through the same entry
point `claudit hook` uses (`TestEnv::hook_fixture`), optionally overriding
fields (`TestEnv::hook_fixture_with`) to vary ids, tools or durations.

There are two kinds of fixtures here:

- **Synthetic** (the `*.json` files at the top level): written by hand after
  the documented shapes (https://code.claude.com/docs/en/hooks) of Claude
  Code 2.1.284, with invented content: user `alice`, project
  `/Users/alice/code/acme-api`, made-up ids and timings. They were not
  captured from a real session; they exist to script precise scenarios.
- **Captured** (`captured-<version>/`): payloads a real Claude Code
  `<version>` sent to `claudit hook`, anonymized (see below).

When upstream shapes change, add new fixtures next to these (suffix the
version, e.g. `post_tool_use_bash.v2_3.json`, or a new `captured-<version>/`)
instead of replacing them.

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

## `activities/`: session `9f4c2b7a-…`

A synthetic session known only from hooks (no transcript), in
`/Users/alice/code/acme-api` on 2026-03-05, of thirteen completed calls,
numbered in arrival order with their receive times in `received_at.json`
(`TestEnv::replay_activities_session`). It is not part of the default
fixture archive, so the totals above are unchanged; `tests/activities.rs`
documents what each activity adds up to.

| # | Call | `duration_ms` | Outcome |
| --- | --- | --- | --- |
| 01 | Bash `cargo test` | 12000 | ok |
| 02 | Bash `cargo test -p api` | 8000 | failed |
| 03 | Edit | 50 | ok |
| 04 | Bash `cd crates/api && cargo nextest run` | 5000 | ok |
| 05 | Bash `pnpm test` | 3000 | failed |
| 06 | Edit | 70 | ok |
| 07 | Write | 30 | ok |
| 08 | Bash `RUST_LOG=debug cargo build` | 6000 | ok |
| 09 | Bash `cargo check` | 2000 | ok |
| 10 | Bash `echo hello` | 10 | ok |
| 11 | Bash `git status` | 100 | ok |
| 12 | Bash `git commit -m "fix: flaky test"` | 300 | ok |
| 13 | Bash `gh pr create --fill` | 1600 | ok |

## `trace/`: session `c4e8a2f0-…`

A synthetic session in `/Users/alice/code/acme-api` (branch
`fix/flaky-login`) on 2026-03-07, for the session trace
(`tests/session_trace.rs`). Its transcript is
`tests/fixtures/transcripts/trace/`; `TestEnv::populate_trace_session`
drops it and replays these payloads, numbered in arrival order, at the
receive times of `received_at.json` (offsets below from 10:00:00 UTC). Agent
`a5d7f9b1c3e5a7b9d` is an Explore subagent run **in the background**.

| Offset (s) | Payload | Notes |
| --- | --- | --- |
| 0 | `UserPromptSubmit` | turn `…0101`, "Fix the flaky login test" |
| 2.0 / 2.1 | `PreToolUse` Bash `cargo test`, `PermissionRequest` | |
| 3.0 / 3.1 | `PreToolUse` Read, Grep | in parallel |
| 3.5 / 3.6 | `PostToolUse` Read (200 ms), Grep (300 ms) | ran 3.3 → 3.5, 3.3 → 3.6 |
| 12.0 | `PostToolUse` Bash (4000 ms) | waited 2 → 8, ran 8 → 12 |
| 14.0 / 14.05 | `PreToolUse` / `PostToolUse` Agent (30 ms) | `status: async_launched` |
| 14.1 | `SubagentStart` | |
| 18.0 | `Stop` | the turn ends, the subagent runs on |
| 20.0 / 25.0 | subagent Bash `cargo test login -- --nocapture` | `PostToolUseFailure`, 4800 ms |
| 30.0 / 31.0 | subagent WebFetch (900 ms) | |
| 40.0 | `SubagentStop` | |
| 41.0 | `UserPromptSubmit` | turn `…0102`, a `<task-notification>` |
| 56.0 | `PreToolUse` TaskOutput (`block: true`, the agent's id) | |
| 60.0 / 70.0 | `SubagentStart` / `SubagentStop` | the run resumes |
| 71.0 | `PostToolUse` TaskOutput (15000 ms) | the main thread waited on the subagent 56 → 71 |
| 72.0 / 72.05 | Edit (50 ms) | |
| 75.0 | `Stop` | |

Expected: turn `…0101` (18 s) splits into model 7.95 s, waiting 5.72 s,
tools 4.3 s, subagent 30 ms (only the Agent call: the background run is not
main-thread time); turn `…0102` (34 s) into model 18.95 s, subagent 15 s
(the blocking TaskOutput), tools 50 ms. The run is active 14.1 → 40 and
60 → 70 s: 35.9 s.

## `captured-2.1.284/`: a real session

Captured from **Claude Code 2.1.284** running headless
(`claude -p --settings <file> --model haiku`) with `claudit hook` registered
as an async command hook on all twelve events. The prompt asked Claude to run
`ls -la`, read `notes.txt`, run `cat does-not-exist.txt` (which fails), and
launch a general-purpose subagent that runs `echo hi`. Files are numbered in
arrival order; `received_at.json` holds each payload's real receive time, and
`TestEnv::replay_captured_hooks` replays them at those times. The matching
transcripts are in `tests/fixtures/transcripts/captured-2.1.284/`.

Anonymization: paths moved to `/Users/alice/code/scratch` (and
`/Users/alice/.claude/projects/-Users-alice-code-scratch`), the user name
replaced by `alice`, the prompt and the `ls` output replaced by invented
text. Ids, event order, timings, `duration_ms` and every other field are as
captured.

Worth knowing from this capture (2.1.284):

- the Agent tool ran the subagent **in the background**: its `PostToolUse`
  arrived 4 ms after launch, with `tool_response.status = "async_launched"`
  and no totals (`totalTokens`, `totalDurationMs` absent); the subagent's
  tokens come from its transcript;
- the main thread's `Stop` (`14_stop.json`) arrived before `SubagentStop`,
  and the subagent's completion then opened a second turn whose
  `UserPromptSubmit` prompt is a `<task-notification>` (`16_…`, `17_stop`);
- `Stop` and `SubagentStop` carry `background_tasks` and `session_crons`;
  `SessionEnd` carried `reason: "other"` for a headless run.
