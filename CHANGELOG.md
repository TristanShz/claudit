# Changelog

All notable changes to claudit are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). Until 1.0, the
database schema and the dashboard may change in any minor release; claudit
migrates the archive automatically.

## [Unreleased]

## [0.1.0] - Unreleased

First public release.

### Added

- **Capture**: `claudit hook`, an async Claude Code command hook on 12 events
  (`SessionStart`, `SessionEnd`, `UserPromptSubmit`, `UserPromptExpansion`,
  `PreToolUse`, `PostToolUse`, `PostToolUseFailure`, `PermissionRequest`,
  `Notification`, `Stop`, `SubagentStart`, `SubagentStop`) that appends
  each timestamped payload to a per-session spool and never blocks, prints
  or fails.
- **Automatic ingest**: a detached `claudit ingest` after every turn and at
  session end; incremental (byte offsets per file, partial trailing lines
  left for later), deduplicated on natural keys, and coalesced behind a
  single-writer lock. Spool files are purged once archived.
- **Transcript backfill**: main-session and subagent transcripts in
  `~/.claude/projects` are ingested, including everything already on disk at
  first run. Unknown line shapes are skipped, counted and logged.
- **Permanent archive** in SQLite (`~/.claudit/claudit.db`, WAL), with the
  redacted raw hook payloads kept as the replay source.
- `claudit reingest`: rebuilds every derived table from the archived hook
  events and the transcripts still on disk, re-applying the current
  redaction patterns.
- **Privacy**: tool outputs and assistant responses are dropped at ingest
  (only a numeric/identifier allow-list of the Agent tool's run summary is
  kept); secrets (`sk-…`, GitHub and AWS keys, bearer tokens, secret-named
  JSON members and `KEY=value` assignments) are redacted with a versioned
  pattern list; every file is owner-only (0600/0700).
- **Time decomposition** of every turn into model, tool, waiting-on-you and
  subagent time, with parallel calls unioned so the four always sum to the
  wall time; waiting time and permission prompts per tool.
- **Tool analytics**: ranking by calls, total, median and p95 duration and
  failure rate; Bash calls by leading command; MCP calls by server.
- **Skills** by trigger (you typing `/skill` or Claude calling the Skill
  tool), with attributed time and tokens; **subagents** by type with runs,
  duration, tool calls, model and tokens.
- **Tokens and API-equivalent cost** per session, model, skill, subagent
  type and day, from a versioned price table compiled into the binary and
  applied at query time (1-hour cache writes priced at their own rate).
- **Dashboard** (`claudit serve [--port N]`, `127.0.0.1` only, all assets
  embedded, works offline): an overview with global filters (date range,
  project, branch, model), KPIs, "where the time goes", tools, skills,
  subagents and sessions, and a session detail page with a turn timeline.
- `claudit install` / `claudit uninstall`: add and remove claudit's hooks in
  Claude Code's user settings, with a timestamped backup, foreign hooks left
  untouched, idempotent re-install, and `cleanupPeriodDays` raised to 365
  (and restored on uninstall).
- `CLAUDIT_HOME` and `CLAUDE_CONFIG_DIR` overrides; errors logged to
  `~/.claudit/logs/claudit.log`.
- Prebuilt macOS binaries (arm64 and x86_64) published on each tag, and
  `cargo install --git https://github.com/TristanShz/claudit`.

[Unreleased]: https://github.com/TristanShz/claudit/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/TristanShz/claudit/releases/tag/v0.1.0
