# Contributing to claudit

Thanks for helping. Read [ARCHITECTURE.md](ARCHITECTURE.md) first: it
describes the data flow, the data model and the time decomposition this guide
refers to.

## Build and check

You need Rust 1.88 or newer (`rust-version` in `Cargo.toml`; CI uses current stable). SQLite is
bundled; there is no Node or Python toolchain, and the dashboard's assets are
checked in under `assets/`.

```sh
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --all --check
```

CI runs exactly these (with `--locked`) on Linux and macOS, with
`RUSTFLAGS=-D warnings`. All four must pass before a change is merged.

To try a build against a throwaway environment instead of your real
`~/.claudit` and `~/.claude`:

```sh
export CLAUDIT_HOME=/tmp/claudit-dev CLAUDE_CONFIG_DIR=/tmp/claude-dev
mkdir -p $CLAUDE_CONFIG_DIR && cp -R tests/fixtures/transcripts/projects $CLAUDE_CONFIG_DIR/
cargo run -- hook < tests/fixtures/hooks/post_tool_use_bash.json
cargo run -- serve
```

## Tests: two seams, assert on stats

Tests exercise external behavior through two public seams only:

1. **The core library, end to end** (`tests/common/mod.rs`, `TestEnv`). Each
   test gets a temp `CLAUDIT_HOME`, a temp Claude config dir and a
   `ManualClock`. Feed hook payloads through the same entry point
   `claudit hook` uses (`env.hook_fixture("…")`, `env.hook_fixture_with(…)`,
   `env.at(t).hook(…)`), drop transcripts in place
   (`env.drop_transcript_fixture(…)`, `env.append_transcript(…)`), run
   `env.ingest()`, then assert on the typed `claudit::stats` reports
   (`env.tool_ranking(&Filter::default())`, `env.time_breakdown(…)`, …).
2. **The settings transformation**: `claudit::install::install` and
   `uninstall` are pure functions from settings JSON to settings JSON
   (`tests/settings.rs`).

**Assert on stats, never on tables.** Tests must not query table layouts,
offsets or internal helpers, so the storage schema can change freely. If a
behavior can't be observed through a stats report, the report is probably
missing. The only sanctioned exceptions are properties that no report can
express: "no secret string survives anywhere in the database"
(`tests/redaction.rs` dumps every text cell), file permissions
(`tests/permissions.rs`), and the setup of "an archive an older claudit
derived" (`tests/derivation_upgrade.rs`), whose assertions stay on reports.

There are no tests on the HTTP layer or the templates: handlers only call
the stats API and render. One explicit exception: `tests/web_pages.rs` is a
render smoke check (issues #11 and #12 require every page to render). It
calls `claudit::web::router` in process and asserts only the HTTP status
and the presence of section ids, never page content; don't extend it
beyond that.

Conventions:

- One test file per feature (`tests/<feature>.rs`), each starting with a
  `//!` comment naming the seams it uses.
- Ingest is idempotent: where it matters, run `env.ingest()` twice and check
  the results don't change.
- Every new stats report gets a line in the `Reports` struct of
  `tests/reingest.rs`, which asserts that `claudit reingest` rebuilds the
  same results as incremental ingest.

## Fixtures policy

Fixtures live in `tests/fixtures/`:

- `hooks/<event>_<variant>.json`: one hook payload per file, snake case
  (e.g. `post_tool_use_bash.json`);
- `transcripts/projects/…`: a mirror of `~/.claude/projects`, including a
  subagent transcript and its `.meta.json`;
- `hooks/activities/`: a synthetic hook-only session of test, build, git and
  edit calls for `tests/activities.rs` (replayed by
  `TestEnv::replay_activities_session`, outside the default fixture archive);
- `transcripts/cost/…`: transcripts for cost tests, kept out of `projects/`
  so the totals documented for `projects/` stay valid;
- `transcripts/tool_calls/…`: a backfilled session for tool calls read from
  transcripts, kept out of `projects/` for the same reason;
- `settings/*.json`: Claude Code settings files for seam 2;
- `hooks/captured-<version>/` and `transcripts/captured-<version>/`: one real
  session captured from Claude Code `<version>`, anonymized
  (`tests/captured.rs` replays it).

Rules:

- **Synthetic or captured, and labelled as such.** Most fixtures are
  synthetic: written by hand after real shapes, with all content invented
  (user `alice`, projects under `/Users/alice/code/`, made-up ids). Captured
  fixtures live under `captured-<version>/` and must be anonymized before
  commit: paths and user name moved to `alice`, prompts and outputs
  replaced, anything describing the local setup (attachments' bodies, hook
  commands) removed; ids, timings and usage may stay. Never commit a
  captured file as is. To capture a new set, register
  `<path to claudit> hook` for all twelve events in a scratch settings file
  with `CLAUDIT_HOME` pointing to a scratch dir, and run
  `claude -p --settings <file> "<prompt>"` in a scratch project; never edit
  your real `~/.claude/settings.json` for it.
- **Tagged with the Claude Code version** they were shaped after. Each
  fixture directory has a `README.md` naming the version (currently 2.1.284)
  and documenting the scenario and expected results (timings, token totals)
  that tests rely on. Update the README when you add a fixture.
- **Added alongside, never replaced.** Claude Code's formats are
  undocumented and change between releases. When a shape changes, add new
  fixtures next to the old ones (suffix the version, e.g.
  `post_tool_use_bash.v2_3.json`) and keep the old ones and their tests:
  archives contain data from every version, and claudit must keep parsing
  all of them.

## Extension points

### A new hook event

1. Add `src/ingest/events/<event>.rs` with a `project(conn, event)` that
   parses the payload (`event.parse::<T>()`) and upserts on a natural key.
   It must be idempotent: the same event may be projected again by
   `reingest`. Return `Projection::Malformed(reason)` when the payload lacks
   what the event requires.
2. Add the `mod` line and one match arm in `events::project`
   (`src/ingest/events/mod.rs`).
3. If claudit does not yet subscribe to the event, add it to
   `install::EVENTS` (`src/install.rs`) and to the list in
   `tests/settings.rs`. Existing users get it by re-running
   `claudit install`. Events already archived before a projection existed
   are picked up by `claudit reingest`.
4. If the payload has output-like fields, make sure `redact.rs` drops them.
   Add a field to `TOOL_RESPONSE_ALLOWLIST` only if it is a number, a flag or
   an identifier, never free text.
5. Add a fixture (`tests/fixtures/hooks/<event>_<variant>.json`) and a test
   asserting on the report the event feeds.

### A new stats report

1. Add `src/stats/<report>.rs` and one `pub mod` line in `src/stats/mod.rs`.
   The report is a function `(&Connection, &Filter, …) -> Result<T>`
   returning plain structs. Build its `WHERE` clause with `Filter::sql`,
   declaring in a `FilterColumns` which expression implements each
   dimension (`None` for one the report cannot honour).
2. Add a `TestEnv` helper if convenient, tests in `tests/<feature>.rs`, and
   a line in the `Reports` struct of `tests/reingest.rs`.
3. To show it: a section template `templates/sections/<x>.html`, an include
   in the page template, and a field in the page struct under
   `src/web/pages/`. Charts are a `[data-chart]` element plus a JSON script
   (`format::script_json`) and a renderer in `assets/claudit.js`.

### A migration

Add `migrations/NNNN_<name>.sql`, where `NNNN` is the number of the GitHub
issue it implements (e.g. `0009_skills_subagents.sql` for issue #9), so
parallel work never collides. `build.rs` discovers the files; they are
applied in file-name order, each once, inside one transaction. **Never edit
a migration that has been merged:** add a new one. Store times as `INTEGER`
microseconds with a `_us` suffix, and comment every table and non-obvious
column.

### A new table

Every table must be classified for `claudit reingest` in
`src/ingest/reingest.rs`, or the `every_table_is_classified_for_reingest`
unit test fails:

- **`DERIVED_TABLES`** if it is computed from `raw_events` and/or the
  transcripts (almost always). It is emptied and rebuilt on reingest. List
  child tables before the tables they reference.
- **`KEPT_TABLES`** if it is a source of truth or bookkeeping that must
  survive a rebuild (rare: today only `meta`, `schema_migrations`,
  `raw_events`, `ingest_offsets`).

A derived `meta` key belongs in `DERIVED_META_KEYS`.

### Redaction patterns and prices

- `redaction/patterns.toml`: bump `version` on every change; patterns must be
  idempotent (a redacted text is unchanged by a second pass). Add a case to
  `tests/redaction.rs`.
- `pricing/prices.toml`: update `version` (the date prices were read) and
  `source`; prices have at most 6 decimals. Add aliases rather than fuzzy
  matching.
- `activities/rules.toml`: bump `version` (the date of the change); order
  matters (first match wins, so specific shell rules go before catch-alls
  like "Run & scripts"). Add the commands you meant to cover to the table
  in the unit tests of `src/activities.rs`.

## Screenshots

The README screenshots (`docs/screenshots/`) are taken from a demo archive
built from the test fixtures:

```sh
cargo run --example demo_archive -- /tmp/claudit-demo
CLAUDIT_HOME=/tmp/claudit-demo cargo run -- serve
```

`examples/demo_archive.rs` reuses the seam-1 harness (`tests/common`), so the
demo shows the same data the dashboard smoke checks render. Use it for manual
browser checks too.

## Pull requests

- Keep commits focused; use conventional prefixes (`feat:`, `fix:`,
  `docs:`, `chore:`), referencing the issue (`(#12)`).
- Add a line under `Unreleased` in `CHANGELOG.md` for user-visible changes.
- By contributing you agree that your contributions are dual licensed under
  MIT OR Apache-2.0, as the rest of the project.

## Releasing

1. Move the `Unreleased` entries of `CHANGELOG.md` under the new version.
2. Bump `version` in `Cargo.toml` (and `Cargo.lock` via `cargo build`).
3. Tag `vX.Y.Z` on `main` and push the tag. The release workflow
   (`.github/workflows/release.yml`) checks that the tag matches the crate
   version, builds macOS arm64 and x86_64 binaries, and publishes them with
   their SHA-256 checksums as a GitHub release.
