# Transcript fixtures

Anonymized Claude Code transcripts, shaped after real transcripts written by
**Claude Code 2.1.284** (user `alice`, projects under `/Users/alice/code/`).
All prompt, tool and response content is invented; ids are synthetic.

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

The files are generated for readability, not captured verbatim. When the
upstream format changes, add fixtures for the new version next to these
rather than editing them.
