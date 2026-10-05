# Changelog

All notable changes to claudit are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). Until 1.0, the
database schema and the dashboard may change in any minor release; claudit
migrates the archive automatically.

## [Unreleased]

### Added

- Session export as Markdown, to read or to give to an AI (to improve the
  skills a session used, say): `claudit export <session> [--full] [-o FILE]`
  (the session id or a unique prefix of it), and *Summary* / *Full*
  download buttons on the session page. The summary holds the session's
  general statistics: where the time went, skills (invocations, attributed
  time, tokens), subagents, activities, tools, Bash commands and the list
  of turns. The full export adds every turn in detail: its whole prompt,
  its time split and every tool call, main thread and subagents, with its
  input, wait, duration and error. Like the archive, an export never holds
  Claude's responses or tool outputs.

### Fixed

- Session time no longer leaves out subagents running in the background.
  A session that hands its work to background subagents (the main thread
  stops at once and only wakes up for each hand-back) used to show only
  the few seconds of each hand-back: 43 minutes for a session that worked
  for 8 hours. "Where the time goes", session active time and the turn
  timeline now have a fifth component, *Background subagents*: the time
  subagents run between turns, while the main thread is idle. Parallel
  subagents count once, and a paused subagent does not count while it is
  paused.

## [0.6.0] - 2026-09-30

### Added

- `claudit serve -d` / `--detach`: runs the dashboard in the background,
  detached from the terminal, so closing the terminal no longer stops it.
  The command returns once the dashboard listens (or says why it could not
  start); its output goes to `~/.claudit/logs/serve.log`.
- `claudit kill` (alias `claudit stop`): stops the running dashboard,
  detached or not (SIGTERM, then SIGKILL after 10 seconds).

### Changed

- Only one `claudit serve` runs at a time: a second one is refused with the
  address of the running dashboard. The dashboard also shuts down cleanly on
  SIGTERM, not only on Ctrl-C.

## [0.5.0] - 2026-09-30

### Added

- `claudit update`: replaces the binary with the latest GitHub release
  (found from the `/releases/latest` redirect, SHA-256 checked, renamed over
  the running binary). `--check` only reports whether a newer release
  exists. A binary installed with `cargo install` is left alone, with the
  `cargo` command to run instead. This is the only command that uses the
  network, and only when you run it.

### Changed

- The README is now a short quick start; the details (commands, what is
  captured, custom activity rules, SQL queries, limitations) moved to
  `docs/GUIDE.md`.

## [0.4.0] - 2026-09-30

### Added

- **Bash commands ranking**: which commands Claude runs most and which take
  the most time, by command key: the program and its meaningful subcommand
  (`pnpm exec vitest`, `cargo test`, `git status`, `docker compose up`,
  `uv run pytest`), never a path, a file or another argument. A line of
  several commands counts once, under its most significant command (the one
  its activity rule matched, else the first that is not `cd`, `export`,
  `echo`, `sleep` or the like); a polling loop is named after its condition
  (`until grep`). Each command shows its activity, runs, total time, median,
  p95 and failures; durations come from hook-timed runs only, with a
  "time measured on N of M runs" note when some runs were imported
  (`stats::commands::{command_ranking, session_commands}`).
  - Overview: a "Bash commands" block with **Most used** and **Slowest** (by
    total time) tabs, next to "Where the time goes".
  - `/commands` (new, in the top navigation): every command, filtered by the
    filter bar, sortable by runs, total time, median, p95 or failures.
  - Session page: a "Bash commands" panel for the session, sortable too.
  - `/tools`: its Bash table now shows the top 10 command keys and links to
    `/commands`.
- **Waiting & polling** activity (built-in, tried first): Bash lines whose
  point is to wait, i.e. a line that opens with an `until` / `while` loop
  (keyed `until grep`, `while pgrep`, `until test`) or that only sleeps
  (`sleep 60`, `echo …; sleep 60`; a `sleep` before real work, as in
  `sleep 30 && gh run view`, does not count). Their time is spent waiting on
  something else (background tests, CI), so they are left out of the
  overview's "Slowest" tab and of `/commands` by default, with a note
  ("2 waiting and polling commands hidden (…)") and a link to show them
  (`/commands?polling=show`); they still count everywhere else.

### Changed

- The leading command of a Bash call (`tool_calls.bash_command`, and the
  `commands` key of activity rules) skips setup commands (`cd`, `export`,
  `set`, `source`, `echo`, `printf`, `sleep`, `true`, `[`, …) and shell
  keywords (`for`, `if`, `do`, …): `export PATH=… && npx tsc` leads with
  `npx`; a line that opens with an `until` / `while` loop leads with
  `until` / `while`. The first ingest after the upgrade rebuilds the archive once
  (`DERIVATION_VERSION` 3; about 2–3 minutes on a large archive).
- A page classifies each distinct tool call once, however many of its
  reports need it (a per-page classification cache,
  `ActivityRules::with_classification_cache`).
- Bash details on the Activities page are finer: options and their values
  are skipped before a subcommand (`pnpm --filter web test` → `pnpm test`,
  `git -C repo log` → `git log`, `cargo +nightly fmt` → `cargo fmt`).

### Removed

- The overview's "Waiting on you, by tool" block, and the
  `stats::time::waiting_by_tool` and `stats::tools::bash_command_ranking`
  reports (replaced by `stats::commands`). Waiting time is still part of
  "Where the time goes", the session KPIs and the turn traces.

## [0.3.0] - 2026-09-30

### Added

- **Turn list and turn traces on the session page**: every turn of the
  session is listed, timed or not, with its start, prompt, duration
  (hook-timed only), tool calls and failures, subagents, test runs, tokens
  and cost (`stats::trace::session_turns`). Opening a turn loads its trace
  (`/sessions/{id}/turns/{prompt_id}`, `stats::trace::turn_trace`): one
  swimlane for the main thread and one per subagent run (its active
  spans), each tool call drawn from execution start to end with its
  permission wait before it, failed calls outlined, calls known only from
  transcripts as ticks at their launch; a tooltip with the call's redacted
  input summary (command, file, pattern, URL, MCP server and tool, skill,
  subagent), timings, status and error; activity filter chips (Tests,
  Build, …, Failed); zoom; and the chronological call log below.
- Per-turn cost (`stats::cost::session_cost_by_turn`); subagent runs carry
  their Agent call's `description` and their active spans.

### Fixed

- **Subagent run time** comes from `SubagentStart` → `SubagentStop`: each
  start paired with the next stop, summed, so background runs (whose Agent
  call returns in milliseconds) and runs resumed later are measured as
  they ran; then `totalDurationMs`; never the Agent call's own duration.
  New table `subagent_events`; the archive is rebuilt once on upgrade.
- **Waiting on subagents**: a blocking `TaskOutput` on a subagent now
  counts as subagent time, like a foreground Agent call; a background run
  no longer shows as a few milliseconds of "Subagent runs" next to its real
  time (the session KPI shows runs and their active time).
- **Readable injected prompts**: task notifications, subagent hand-backs
  (`<agent-message>`), slash commands and local command output are
  labelled everywhere prompts show (session header, session list,
  timeline, turn list), and a session's first prompt is its first typed
  one. Hand-back turns imported from transcripts now have their text.

## [0.2.0] - 2026-09-30

### Added

- **Tool calls from transcripts**: sessions imported by the transcript
  backfill (everything before `claudit install`) now get their tool calls,
  Bash commands and failures, subagent calls included. They count as calls
  but carry no duration (see below); when both sources saw a call it counts
  once, with the hook timing, in any ingest order. Tool result content is
  never stored.
- **Imported sessions are flagged**: a session with no hook data is
  `imported` (`SessionSummary::imported`, `SessionDetail::imported`); the
  session table tags it and shows `–` for its duration and split, and its
  page explains that tokens, cost and tool calls are available but not the
  time breakdown. Time sections show how many sessions they cover
  (`stats::time::time_coverage`), or an empty state when no hook-recorded
  session matches.
- **One-time rebuild after an upgrade**: when the archive was derived by an
  older claudit (`derivation_version` in `meta`), the next `claudit ingest`
  or `claudit serve` rebuilds it once, as `claudit reingest` would, so
  existing archives get the tool calls of transcripts already read.
- **Models**: an overview block and a `/models` page with, per model, the
  sessions that used it, API responses, tokens, cache-read share and
  API-equivalent cost, its share of all tokens and cost, and the split
  between the main thread and subagents
  (`stats::models::model_usage`).
- **Activities**: what Claude's tools spend their time on. Every tool call
  is classified (tests, build & typecheck, lint & format, git & GitHub,
  dependencies, run & scripts, search, read and edit files, web, subagents,
  skills, MCP, planning) by versioned built-in rules
  (`activities/rules.toml`) at query time, so rule changes apply to the
  whole history. Shown as an overview section, an `/activities` page with
  each activity's top commands and a daily chart, and a panel on the
  session page (`stats::activities`). Add or override rules in
  `$CLAUDIT_HOME/activities.toml`; an invalid file is ignored and reported
  in a banner.

### Fixed

- **Time is measured from hook-recorded data only.** Turn spans read from
  transcripts include permission prompts, idle time and background tasks,
  so imported sessions inflated "Where the time goes" (hundreds of hours of
  "model" time). Time decomposition, active time, waiting, session
  durations, the turn timeline, skills' attributed time and subagent
  durations now use hook times only (a turn needs its `UserPromptSubmit`
  and `Stop`). Tool and activity durations and percentiles come from
  hook-timed calls only; `CallStats::estimated_duration_calls` is replaced
  by `timed_calls`. Consumption (sessions, turns, tokens, cost, models,
  tool calls) still covers every session.

## [0.1.0] - 2026-09-30

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
  single-writer lock (a run turned away by the lock leaves a marker, and the
  holder runs again after releasing it, so late input is never stranded).
  Spool files are purged once archived. Hook stdin that is not valid JSON is
  logged and archived raw rather than dropped.
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
  pattern list, in hook payloads and in transcript prompts, working
  directories and branch names; every file is owner-only (0600/0700).
- **Time decomposition** of every turn into model, tool, waiting-on-you and
  subagent time, with parallel calls unioned so the four always sum to the
  wall time; waiting time and permission prompts per tool. Subagent time is
  a fourth component of the partition (main-thread Agent/Task execution,
  priority subagent > tool > waiting > model), a deliberate refinement of
  the spec's three-way split with subagents reported apart; a turn missing
  its `UserPromptSubmit` or `Stop` hook falls back to its transcript
  timestamps.
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
  untouched, idempotent re-install, `cleanupPeriodDays` raised to 365 (and
  restored on uninstall), and an exact round trip: uninstall drops only the
  hook containers install created.
- `CLAUDIT_HOME` and `CLAUDE_CONFIG_DIR` overrides; errors logged to
  `~/.claudit/logs/claudit.log`.
- Prebuilt macOS binaries (arm64 and x86_64) published on each tag, and
  `cargo install --git https://github.com/TristanShz/claudit`.

[Unreleased]: https://github.com/TristanShz/claudit/compare/v0.6.0...HEAD
[0.6.0]: https://github.com/TristanShz/claudit/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/TristanShz/claudit/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/TristanShz/claudit/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/TristanShz/claudit/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/TristanShz/claudit/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/TristanShz/claudit/releases/tag/v0.1.0
