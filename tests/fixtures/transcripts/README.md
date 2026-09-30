# Transcript fixtures

`projects/` and `cost/` are **synthetic** transcripts: written by hand after
the shape of real transcripts from **Claude Code 2.1.284** (user `alice`,
projects under `/Users/alice/code/`), not captured. All prompt, tool and
response content is invented; ids are synthetic.

`captured-2.1.284/` is a **captured** transcript, anonymized (see the end of
this file).

`projects/` mirrors `~/.claude/projects/`:

| File | What it covers |
| --- | --- |
| `-Users-alice-code-acme-api/8d0c5a3e-….jsonl` | Main session on `main`, two turns (Opus). Turn 1: one API message split over two entries sharing `message.id` (output tokens grow from 150 to 212 between them), a Bash call. Turn 2: skill-attributed (`attributionSkill: code-review`) Agent call. Also carries metadata lines (`mode`, `file-history-snapshot`, `last-prompt`, `ai-title`, `queue-operation`, `cost-state`), attachments, system entries, and **one line of unknown shape** (no `type`). |
| `-Users-alice-code-acme-api/8d0c5a3e-…/subagents/agent-a1f3c5e7b9d2c4e6f.jsonl` | The turn-2 subagent (Haiku): `agentId`, `isSidechain: true`, the parent's `promptId`, `attributionAgent`. Its `.meta.json` sits next to it, as Claude Code writes it. |
| `-Users-alice-code-acme-api/2b7e4f10-….jsonl` | Second session, branch `feat/login`, one turn (Sonnet). |
| `-Users-alice-code-web-app/5c9d1e22-….jsonl` | Third session in another project, one turn. |

Expected totals (tokens counted once per `message.id`):

| Session | Turns | Input | Output | Cache write | Cache read |
| --- | --- | --- | --- | --- | --- |
| `8d0c5a3e` main thread | 2 | 7 | 1162 | 15000 | 71300 |
| `8d0c5a3e` subagent | – | 6 | 300 | 4300 | 4000 |
| `2b7e4f10` | 1 | 5 | 150 | 3200 | 3000 |
| `5c9d1e22` | 1 | 2 | 500 | 5000 | 1000 |

Cost fixtures live in `cost/`, outside `projects/`, so `drop_projects_fixture`
and the totals above are unaffected; `tests/cost.rs` drops them explicitly:

| File | What it covers |
| --- | --- |
| `cost/7e3a9c51-….jsonl` | One-turn session in `acme-api` (2026-03-05) on a model absent from the price table, `claude-nebula-9`: input 10, output 400, cache write 2000, cache read 6000. Its cost must be reported as unknown. |
| `cost/4f6b8d02-….jsonl` | One-turn session in `acme-api` (2026-03-06), Opus 5.5, one response streamed over two entries whose cache writes are split by TTL (`cache_creation`: 1000 5-minute + 3000 1-hour): input 10, output 100, cache write 4000, cache read 2000. The two TTLs must be priced at their own rates. |

Every cache write in `projects/` is a 1-hour write (`ephemeral_1h_input_tokens`).

Since tool calls are read from transcripts too, `projects/` yields, without
any hook: session `8d0c5a3e` a Bash call (09:00:04 → 09:00:20) and an Agent
call (09:05:02 → 09:06:10) on the main thread, and a Read call (09:05:06 →
09:05:07) in its subagent; session `2b7e4f10` an Edit call (14:00:02 →
14:00:03). The hook fixtures of session `8d0c5a3e` carry the same
`tool_use_id`s, so with hooks those calls count once, with hook timing.

`tool_calls/` holds a session for tool calls read from transcripts only
(a backfilled session), kept out of `projects/` so the results documented
above stay valid; `TestEnv::drop_tool_calls_fixture` copies it into place
and `tests/transcript_tool_calls.rs` uses it:

| File | What it covers |
| --- | --- |
| `tool_calls/-Users-alice-code-toolbox/6e2d9b47-….jsonl` | One turn (2026-03-04 10:00:00 → 10:00:21, Sonnet), one `tool_use` per assistant entry and one `tool_result` per user entry: Bash `git status --short` 02.0 → 03.5 s (1500 ms), Bash `cargo test --all` 05.0 → 12.0 s (7000 ms, `is_error: true`), Read 13.0 → 13.2 s (200 ms, no `is_error` field, list content), Agent 14.0 → 20.0 s (6000 ms). |
| `tool_calls/…/6e2d9b47-…/subagents/agent-a7c9e1b3d5f2a4c6e.jsonl` | The Explore subagent (Haiku): Grep 15.0 → 15.4 s (400 ms). Its `.meta.json` names the Agent call. |

Expected time split of the turn: tools 8.7 s (1.5 + 7 + 0.2), subagent 6 s,
waiting 0 (no hooks), model 6.3 s.

The files are generated for readability, not captured verbatim. When the
upstream format changes, add fixtures for the new version next to these
rather than editing them.

## `captured-2.1.284/`: a real session

The transcripts Claude Code 2.1.284 wrote for the session whose hook payloads
are in `tests/fixtures/hooks/captured-2.1.284/` (see the README there for the
scenario): `projects/-Users-alice-code-scratch/8b7fcd89-….jsonl` (two turns,
Haiku) and its background subagent `…/subagents/agent-ab3adec90281dd9a2.jsonl`
with its `.meta.json` (`requestShape: "background"`).
`TestEnv::drop_captured_transcripts` copies them into place.

Anonymization: paths and the user name as for the hooks; the prompt, tool
outputs and thinking signatures replaced by invented text; `attachment`
entries reduced to their envelope (uuids, timestamps, cwd, …) and their
`attachment.type`, because their bodies carry the local setup (system prompt,
tool and MCP server lists, account context); `system` hook summaries point at
an invented command. Everything else (ids, timestamps, usage, models, line
types and order) is as captured.
