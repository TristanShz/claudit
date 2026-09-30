# claudit guide

The details behind the [README](../README.md): how claudit works, what it stores, the dashboard's rules, and how to query the archive yourself.

## How it works

Claude Code runs `claudit hook` as an
[async command hook](https://code.claude.com/docs/en/hooks) on twelve
events. The hook appends the raw payload, stamped with its receive time, to a
per-session spool file and exits; it never blocks your session, never prints
anything and never fails visibly. At the end of every turn it starts
`claudit ingest` in the background, which loads the spool and Claude Code's
transcripts (`~/.claude/projects`, including subagent transcripts) into
`~/.claudit/claudit.db`. The first ingest backfills every transcript still on
disk, so your history starts with whatever Claude Code has kept (30 days by
default) rather than from zero. From then on the archive keeps everything,
independently of Claude Code's own retention.

There is no daemon. `claudit serve` starts the dashboard when you want it,
catches up on pending ingestion first, and stops on Ctrl-C.

See [ARCHITECTURE.md](../ARCHITECTURE.md) for the full data flow and data model.

### Imported sessions vs sessions recorded with hooks

Sessions from before `claudit install` are **imported** from their
transcripts. They count everywhere consumption is reported: sessions, turns,
tokens, cost, models, skills' tokens, subagents' tokens, tool calls,
failures, Bash commands, MCP servers and activity call counts.

**Time is measured only on sessions recorded with the hooks.** A
transcript's timestamps include permission prompts, idle time and background
work (a turn left waiting for an hour reads as an hour of "model" time), so
imported sessions contribute no time at all: no "Where the time goes", no
waiting, no active time, no tool durations or percentiles, no activity time,
no subagent durations. The dashboard tags them `imported`, shows `–` for
their durations, and each time section says how many sessions it covers
("Time measured on N sessions recorded with hooks since …; X imported
sessions not included").

## What is captured, and what never is

**Stored** (after redaction, see below):

- hook payloads for `SessionStart`, `SessionEnd`, `UserPromptSubmit`,
  `UserPromptExpansion`, `PreToolUse`, `PostToolUse`, `PostToolUseFailure`,
  `PermissionRequest`, `Notification`, `Stop`, `SubagentStart` and
  `SubagentStop`, with their receive timestamps;
- your prompts (the text you type);
- tool **inputs** (the command Claude ran, the file it read, the pattern it
  searched for, …) and tool error messages;
- from transcripts: session metadata (working directory, git branch, Claude
  Code version), turn timings, permission mode and effort, and for each API
  response the model, timestamp, token counts and the skill or subagent it is
  attributed to; for each tool call its name, input, timestamps and whether
  it failed (not the error text).

**Never stored:**

- **tool outputs**: a hook's `tool_response` is dropped at ingest, except a
  fixed allow-list of summary fields the Agent tool reports about a subagent
  run (`agentId`, `agentType`, `status`, `resolvedModel`, `totalDurationMs`,
  `totalTokens`, `totalToolUseCount`, and the numeric members of `usage`).
  Only numbers, flags and identifiers are allowed there, never free text;
- **assistant responses**: the text Claude writes back
  (`last_assistant_message` on `Stop`/`SubagentStop`, and assistant message
  content in transcripts) is never read into the archive;
- tool results inside transcripts (only whether a result is an error is
  read).

The raw spool file holds the untouched payload only until it has been
ingested; it is deleted once its session has ended (or after 24 hours of
inactivity for sessions that never ended cleanly).

## Privacy

- **Secret redaction before anything is stored.** Every string of every hook
  payload, and every text taken from transcripts, goes through a versioned
  list of patterns ([`redaction/patterns.toml`](../redaction/patterns.toml)):
  `sk-…` API keys, GitHub tokens (`ghp_…`, `gho_…`, `github_pat_…`, …), AWS
  access key ids (`AKIA…`, `ASIA…`), `Bearer …` tokens, JSON members and
  `NAME=value` assignments whose name contains `secret`, `key`, `token` or
  `password`. Matches become `[REDACTED]`. When the list grows,
  `claudit reingest` re-applies it to the whole archive, raw events included.
  Redaction is pattern based: it catches common secret shapes, not every
  possible one.
- **Owner-only files.** Everything under `~/.claudit` is created with mode
  `0600` (files) and `0700` (directories), including the SQLite WAL files and
  the settings backups `claudit install` writes.
- **Localhost only.** `claudit serve` binds to `127.0.0.1`; there is no option
  to listen on another interface.
- **No network.** claudit makes no outgoing connections, except
  `claudit update` when you run it (to github.com only). The dashboard's
  JavaScript (htmx, ECharts) and CSS are embedded in the binary; nothing is
  loaded from a CDN.
- There is no encryption at rest: the archive is as private as your user
  account.

## Commands

| Command | What it does |
| --- | --- |
| `claudit install` | Adds claudit's hooks to Claude Code's user settings. |
| `claudit uninstall` | Removes them and restores `cleanupPeriodDays`. Your recorded data in `~/.claudit` is kept. |
| `claudit update [--check]` | Downloads the latest release from GitHub, checks its SHA-256 and replaces the binary in place (`--check` only reports whether one is available). For a binary installed with `cargo install`, it prints the `cargo` command to run instead. |
| `claudit serve [--port N]` | Catches up on pending ingestion, then serves the dashboard on `127.0.0.1:N` (default 8421) until Ctrl-C. |
| `claudit ingest` | Loads new spooled events and transcripts into the archive. Runs automatically after each turn; safe to run by hand at any time (it is incremental, deduplicated and exits at once if another ingest is running). |
| `claudit reingest` | Rebuilds every derived table from the archived hook events plus the transcripts still on disk, re-applying the current redaction patterns. After an upgrade that derives more from the archive (e.g. tool calls from transcripts), the next ingest runs it once by itself; run it by hand after an upgrade that changes the parser or the patterns. |
| `claudit hook` | Records one hook payload from stdin. Run by Claude Code, not by hand. |

### Environment variables

| Variable | Default | Meaning |
| --- | --- | --- |
| `CLAUDIT_HOME` | `~/.claudit` | Where claudit keeps its data: `claudit.db`, `spool/`, `logs/claudit.log`, `install-state.json`, `ingest.lock`, `ingest.pending`, and your optional `activities.toml`. |
| `CLAUDE_CONFIG_DIR` | `~/.claude` | Claude Code's configuration directory: `settings.json` and the `projects/` transcripts. Set it the same way you set it for Claude Code. |

Hooks inherit Claude Code's environment, so a `CLAUDIT_HOME` set in the
shell you start Claude Code from also applies to recording.

### When something looks wrong

Everything that fails inside the hook or a background ingest is appended to
`~/.claudit/logs/claudit.log` (redacted, one line per error). A hook payload
that is not valid JSON is logged and archived as raw text (redacted) instead
of being dropped. Transcript lines of an unknown shape are skipped, counted
and logged; the dashboard shows a banner when there are any.

## Activities

Every tool call is assigned an **activity** by an ordered list of rules: the
first rule that matches wins, and a call no rule matches is *Other shell*
(Bash) or *Other*. The built-in rules
([`activities/rules.toml`](../activities/rules.toml)) cover waiting and
polling (`until`/`while` loops, lines that only sleep), tests (`cargo
test`, `pnpm test`, `pytest`, `go test`, `npx vitest`, …), build and
typecheck, lint and format, git and GitHub, dependencies, running scripts,
searching, reading and editing files, the web, subagents, skills, MCP (by
server) and planning tools. Calls are classified when a page is shown, so
changing the rules reclassifies your whole history, without a reingest.

To add activities or reclassify calls, create
`$CLAUDIT_HOME/activities.toml` (`~/.claudit/activities.toml`) in the same
format. Your rules are tried **before** the built-in ones:

```toml
version = "1"              # optional, shown on the Activities page

# Your end-to-end suite gets its own activity.
[[rule]]
activity = "E2E tests"
tools = ["Bash"]
pattern = '^(?:pnpm|npm)\s+(?:run\s+)?e2e\b'

# Count `cargo clippy` as building rather than linting.
[[rule]]
activity = "Build & typecheck"
tools = ["Bash"]
commands = ["cargo"]
pattern = '^cargo\s+clippy\b'

# One MCP server as an activity of its own.
[[rule]]
activity = "Browser"
tools = ["mcp__claude-in-chrome__*"]
```

A rule has:

| Key | Required | Matches |
| --- | --- | --- |
| `activity` | yes | The activity name it assigns (a new one or a built-in one). |
| `tools` | yes | Tool names; `*` and `?` are wildcards (`mcp__*`). |
| `commands` | no | The Bash call's leading command: the program of its first simple command that is not a setup command (`cd`, `export`, `set`, `source`, `echo`, `printf`, `sleep`, `true`, `[`, …), e.g. `cd web && pnpm test` → `pnpm`; `until` / `while` for a line that opens with such a loop, and `sleep` for a line that only sleeps. Wildcards allowed. |
| `pattern` | no | A regular expression ([Rust syntax](https://docs.rs/regex/latest/regex/#syntax)) searched in each simple command of the Bash command line: the line is split at `&&`, `\|\|`, `;`, `\|`, `&` and newlines (outside quotes, here-documents skipped), `VAR=value` prefixes, shell keywords (`if`, `then`, `do`, `while`, `until`, …) and `sudo`/`env`/`time`/`timeout N`/`nohup` wrappers are removed, and the program is reduced to its file name (`./gradlew test` → `gradlew test`). Start it with `^` to mean "a command that starts with". |

A rule matches when the tool matches and, when given, the leading command
and the pattern match too. If the file cannot be read or is invalid, it is
ignored (the built-in rules still apply), the error is logged to
`logs/claudit.log`, and the dashboard shows a banner naming the problem.

Time is each call's own execution time (`duration_ms`), summed: parallel
calls add up, and calls made inside a subagent count in their activity as
well as in the Agent call's *Subagents* time.

## Bash commands

The Commands page (`/commands`), the overview's "Bash commands" block and
the session page's panel group Bash calls by **command key**: the program
and its meaningful subcommand, without paths, files or other arguments.

| Command line | Key |
| --- | --- |
| `pnpm exec vitest run src/cart.test.ts` | `pnpm exec vitest` |
| `pnpm --filter @acme/api test` | `pnpm test` |
| `npx vitest run`, `npm run build` | `npx vitest`, `npm run build` |
| `cargo test --workspace`, `cargo +nightly fmt` | `cargo test`, `cargo fmt` |
| `git -C repo status --short`, `gh pr view 42` | `git status`, `gh pr` |
| `docker compose -f dev.yml up -d` | `docker compose up` |
| `python -m pytest -q`, `uv run pytest` | `python -m pytest`, `uv run pytest` |
| `vitest run src/a.test.ts`, `./scripts/deploy.sh prod` | `vitest`, `deploy.sh` |
| `cd web && pnpm test 2>&1 \| tail -30` | `pnpm test` |
| `until grep -q done run.log; do sleep 5; done` | `until grep` (Waiting & polling) |
| `sleep 60`; `sleep 30 && gh run view 12` | `sleep` (Waiting & polling); `gh run` |

A line of several commands counts once, under its most significant command:
the one its activity rule matched (rules are tried in order: tests, lint,
build, git, …, so `cargo build && cargo test` is `cargo test`), else its
leading command (see `commands` under [Activities](#activities)). Each
command carries its activity. Runs imported from transcripts are counted
but not timed; the tables say "Time measured on N of M runs" when some
were.

Commands of the built-in **Waiting & polling** activity (polling loops and
lines that only sleep) wait on something else, such as tests running in the
background, so the overview's "Slowest" tab and the Commands page leave them
out by default and say how many were hidden; "Show them" includes them
(`/commands?polling=show`).

## Querying the archive with SQL

The archive is a plain SQLite database, and direct SQL is the supported way
to answer questions the dashboard doesn't:

```sh
sqlite3 ~/.claudit/claudit.db
```

Stick to `SELECT`: claudit owns the data, and `claudit reingest` rebuilds the
derived tables anyway. The database is in WAL mode, so reading it never
blocks a running ingest.

Timestamps are integers in microseconds since the Unix epoch (columns ending
in `_us`); `datetime(x / 1000000, 'unixepoch')` makes them readable. The
tables are described in [ARCHITECTURE.md](../ARCHITECTURE.md#data-model).

```sql
-- Most used tools, with failures and total execution time (hook-timed).
SELECT tool_name, COUNT(*) AS calls,
       SUM(success = 0) AS failures,
       ROUND(SUM(hook_duration_ms) / 1000.0, 1) AS total_s
FROM tool_calls
GROUP BY tool_name ORDER BY calls DESC LIMIT 15;

-- What Claude runs in your shell, by leading command.
SELECT bash_command, COUNT(*) AS calls, SUM(success = 0) AS failures
FROM tool_calls WHERE tool_name = 'Bash'
GROUP BY bash_command ORDER BY calls DESC LIMIT 15;

-- Tokens per day and model.
SELECT date(at_us / 1000000, 'unixepoch') AS day, model,
       SUM(input_tokens) AS input, SUM(output_tokens) AS output,
       SUM(cache_write_tokens) AS cache_write, SUM(cache_read_tokens) AS cache_read
FROM api_messages
GROUP BY day, model ORDER BY day DESC, output DESC;

-- Sessions per project and branch.
SELECT cwd, git_branch, COUNT(*) AS sessions,
       datetime(MAX(last_at_us) / 1000000, 'unixepoch') AS last_active
FROM sessions
GROUP BY cwd, git_branch ORDER BY sessions DESC;

-- Skills, by who triggered them.
SELECT skill, trigger, COUNT(*) AS invocations
FROM skill_invocations
GROUP BY skill, trigger ORDER BY invocations DESC;

-- Your last ten prompts (hook time, else transcript time).
SELECT datetime(COALESCE(submit_at_us, start_at_us) / 1000000, 'unixepoch') AS at,
       substr(prompt_text, 1, 80) AS prompt
FROM turns ORDER BY COALESCE(submit_at_us, start_at_us) DESC LIMIT 10;
```

Costs are not stored: they are computed at query time from token counts and
the compiled-in price table ([`pricing/prices.toml`](../pricing/prices.toml)).

The schema may change between releases (new migrations are applied
automatically when claudit opens the database). `raw_events` keeps every
redacted hook payload as JSON, so it is the most stable thing to query.

## Limitations

- **The cost is an estimate.** It is what the recorded tokens would cost at
  Anthropic's public API list prices (standard rates: no batch discount, no
  data-residency or partner-platform pricing), as of the price table's date.
  It is not what you pay on a subscription plan, and models missing from the
  table are reported as unpriced rather than guessed.
- **The transcript format is undocumented** and can change with any Claude
  Code release. Parsing is tolerant: lines of an unknown shape are skipped and
  counted rather than failing the ingest, and the raw hook events are kept so
  `claudit reingest` can recover fields a newer parser understands.
- **Only sessions recorded with the hooks have time.** Imported sessions
  (from before `claudit install`) have tokens, turns, costs and tool calls
  from their transcripts, but no time, permission waits or skill triggers
  (see [above](#imported-sessions-vs-sessions-recorded-with-hooks)). When
  a hook and a transcript both saw a call, it counts once, with the hook's
  timing.
- **Time is measured from hook receive times**, so it includes the small
  delay Claude Code takes to start each hook process.
- Redaction is pattern based and cannot recognize every secret. Prompts and
  tool inputs are stored; don't use claudit if that is unacceptable for your
  work.
- macOS is the only release target. Windows is not supported.
