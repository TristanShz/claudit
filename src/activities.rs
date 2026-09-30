//! Activities: what a tool call was for (running tests, building, git, …).
//!
//! Each call is classified by an ordered list of rules: the built-in ones
//! (`activities/rules.toml`, compiled in), preceded by the user's own
//! (`$CLAUDIT_HOME/activities.toml`, same format) when that file exists.
//! The first rule matching the call wins; a call no rule matches is
//! [`OTHER_SHELL`] (Bash) or [`OTHER`]. The file format is documented at
//! the top of `activities/rules.toml` and in the README.
//!
//! Nothing classified is stored: `stats::activities` classifies the
//! archived tool calls at query time, so a rule change applies to the whole
//! history without re-ingesting (like prices, see `pricing`).
//!
//! Besides its activity, each call gets a **detail**, the group it is
//! counted in within the activity: for Bash, its command key
//! ([`crate::shell::command_key`]: `cargo test`, `pnpm exec vitest`,
//! `python -m pytest`), which `stats::commands` ranks too; for an MCP tool,
//! its server; otherwise the tool name. Command lines are read by
//! [`crate::shell`].

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{Context, Result, bail};
use regex::Regex;
use serde::Deserialize;

use crate::logfile;
use crate::paths::Paths;
use crate::shell::{self, SimpleCommand};

const BUILTIN: &str = include_str!("../activities/rules.toml");

/// The activity of a Bash call no rule matches.
pub const OTHER_SHELL: &str = "Other shell";
/// The activity of any other call no rule matches.
pub const OTHER: &str = "Other";
/// The built-in activity of Bash lines that only wait: polling loops and
/// bare `sleep`s.
pub const WAITING: &str = "Waiting & polling";

/// A tool call, as far as classification is concerned.
#[derive(Debug, Clone, Copy, Default)]
pub struct ToolCall<'a> {
    pub tool_name: &'a str,
    /// The server of an `mcp__<server>__<tool>` call.
    pub mcp_server: Option<&'a str>,
    /// The leading command of a Bash call, as ingest derived it.
    pub bash_command: Option<&'a str>,
    /// The full command line of a Bash call (`tool_input.command`).
    pub command: Option<&'a str>,
}

/// The activity of a call and its detail (see the module docs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Classification<'r, 'c> {
    pub activity: &'r str,
    pub detail: Cow<'c, str>,
}

/// An ordered list of rules. Cheap to clone (compiled regexes are shared),
/// optionally with a classification cache shared by its clones (see
/// [`ActivityRules::with_classification_cache`]).
#[derive(Debug, Clone)]
pub struct ActivityRules {
    /// `version` of the built-in file, plus the user file's when merged.
    pub version: String,
    rules: Vec<Rule>,
    cache: Option<Arc<Mutex<HashMap<String, Cached>>>>,
}

/// A cached classification: the index of the rule that matched (`None`
/// for [`OTHER_SHELL`] / [`OTHER`]) and the detail.
#[derive(Debug, Clone)]
struct Cached {
    rule: Option<usize>,
    detail: String,
}

#[derive(Debug, Clone)]
struct Rule {
    activity: String,
    tools: Vec<Glob>,
    commands: Vec<Glob>,
    pattern: Option<Regex>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RulesFile {
    #[serde(default)]
    version: String,
    #[serde(default, rename = "rule")]
    rules: Vec<RuleEntry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RuleEntry {
    activity: String,
    tools: Vec<String>,
    #[serde(default)]
    commands: Vec<String>,
    pattern: Option<String>,
}

/// Rules loaded for the dashboard: the user's file merged over the
/// built-in rules, or the built-in rules alone when that file is invalid.
#[derive(Debug, Clone)]
pub struct LoadedRules {
    pub rules: ActivityRules,
    /// Why `$CLAUDIT_HOME/activities.toml` was ignored, if it was.
    pub problem: Option<String>,
}

impl ActivityRules {
    /// The rules compiled in from `activities/rules.toml`.
    pub fn builtin() -> &'static ActivityRules {
        static RULES: OnceLock<ActivityRules> = OnceLock::new();
        RULES.get_or_init(|| {
            ActivityRules::from_toml(BUILTIN).expect("activities/rules.toml is a valid rule file")
        })
    }

    /// Parses rules in the format of `activities/rules.toml`.
    pub fn from_toml(text: &str) -> Result<Self> {
        let file: RulesFile = toml::from_str(text).context("invalid activity rules")?;
        let rules = file
            .rules
            .into_iter()
            .enumerate()
            .map(|(i, entry)| {
                let n = i + 1;
                if entry.activity.trim().is_empty() {
                    bail!("rule {n}: `activity` is empty");
                }
                if entry.tools.is_empty() {
                    bail!("rule {n} ({}): `tools` is empty", entry.activity);
                }
                let pattern = entry
                    .pattern
                    .map(|p| Regex::new(&p))
                    .transpose()
                    .with_context(|| format!("rule {n} ({}): invalid pattern", entry.activity))?;
                Ok(Rule {
                    activity: entry.activity,
                    tools: entry.tools.iter().map(|t| Glob::new(t)).collect(),
                    commands: entry.commands.iter().map(|c| Glob::new(c)).collect(),
                    pattern,
                })
            })
            .collect::<Result<_>>()?;
        Ok(Self {
            version: file.version,
            rules,
            cache: None,
        })
    }

    /// These rules with a fresh classification cache: each distinct call
    /// is classified once, however many reports classify it, until this
    /// value and its clones are dropped. The dashboard makes one per page
    /// (`web::frame`), so the activities, commands and trace reports of a
    /// page share their work; the cache is never kept across pages, so it
    /// cannot grow with the archive.
    pub fn with_classification_cache(&self) -> ActivityRules {
        ActivityRules {
            cache: Some(Arc::default()),
            ..self.clone()
        }
    }

    /// `overrides` first, then these rules.
    pub fn with_overrides(&self, overrides: &ActivityRules) -> ActivityRules {
        ActivityRules {
            version: if overrides.version.is_empty() {
                format!("{} + user rules", self.version)
            } else {
                format!("{} + user rules {}", self.version, overrides.version)
            },
            rules: overrides.rules.iter().chain(&self.rules).cloned().collect(),
            cache: None,
        }
    }

    /// The built-in rules, preceded by `$CLAUDIT_HOME/activities.toml` when
    /// it exists. An unreadable or invalid file is ignored and reported in
    /// [`LoadedRules::problem`], and logged (once per distinct problem).
    pub fn load(paths: &Paths) -> LoadedRules {
        let path = paths.activity_rules_file();
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return LoadedRules {
                    rules: Self::builtin().clone(),
                    problem: None,
                };
            }
            Err(err) => return Self::ignored(paths, format!("{}: {err}", path.display())),
        };

        // The dashboard loads the rules on every page: reuse the last
        // compiled user file while its text is unchanged.
        static LAST: Mutex<Option<(String, ActivityRules)>> = Mutex::new(None);
        let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((cached, rules)) = last.as_ref()
            && *cached == text
        {
            return LoadedRules {
                rules: rules.clone(),
                problem: None,
            };
        }
        match Self::from_toml(&text) {
            Ok(user) => {
                let rules = Self::builtin().with_overrides(&user);
                *last = Some((text, rules.clone()));
                LoadedRules {
                    rules,
                    problem: None,
                }
            }
            Err(err) => {
                drop(last);
                Self::ignored(paths, format!("{}: {err:#}", path.display()))
            }
        }
    }

    fn ignored(paths: &Paths, problem: String) -> LoadedRules {
        static LOGGED: Mutex<Option<String>> = Mutex::new(None);
        let mut logged = LOGGED.lock().unwrap_or_else(|e| e.into_inner());
        if logged.as_deref() != Some(problem.as_str()) {
            logfile::error(
                paths,
                "activities",
                format!("ignoring the user activity rules: {problem}"),
            );
            *logged = Some(problem.clone());
        }
        LoadedRules {
            rules: Self::builtin().clone(),
            problem: Some(problem),
        }
    }

    /// The activity of `call` and its detail.
    pub fn classify<'r, 'c>(&'r self, call: &ToolCall<'c>) -> Classification<'r, 'c> {
        let Some(cache) = &self.cache else {
            return self.classify_uncached(call);
        };
        let key = format!(
            "{}\0{}\0{}\0{}",
            call.tool_name,
            call.mcp_server.unwrap_or("\u{1}"),
            call.bash_command.unwrap_or("\u{1}"),
            call.command.unwrap_or("\u{1}"),
        );
        let mut cache = cache.lock().unwrap_or_else(|e| e.into_inner());
        let cached = cache.entry(key).or_insert_with(|| {
            let c = self.classify_uncached(call);
            Cached {
                rule: self
                    .rules
                    .iter()
                    .position(|r| std::ptr::eq(r.activity.as_str(), c.activity)),
                detail: c.detail.into_owned(),
            }
        });
        Classification {
            activity: match cached.rule {
                Some(i) => &self.rules[i].activity,
                None if call.tool_name == "Bash" => OTHER_SHELL,
                None => OTHER,
            },
            detail: Cow::Owned(cached.detail.clone()),
        }
    }

    fn classify_uncached<'r, 'c>(&'r self, call: &ToolCall<'c>) -> Classification<'r, 'c> {
        let is_bash = call.tool_name == "Bash";
        let commands = match (is_bash, call.command) {
            (true, Some(command)) => shell::simple_commands(command),
            _ => Vec::new(),
        };
        let leading: Option<Cow<'c, str>> = call
            .bash_command
            .map(Cow::Borrowed)
            .or_else(|| shell::leading(&commands).map(|c| Cow::Owned(c.leading_name().to_owned())));

        for rule in &self.rules {
            if !rule.tools.iter().any(|g| g.matches(call.tool_name)) {
                continue;
            }
            if !rule.commands.is_empty() {
                let Some(leading) = leading.as_deref() else {
                    continue;
                };
                if !rule.commands.iter().any(|g| g.matches(leading)) {
                    continue;
                }
            }
            let mut matched = None;
            if let Some(pattern) = &rule.pattern {
                matched = commands.iter().position(|c| pattern.is_match(&c.text));
                if matched.is_none() {
                    continue;
                }
            }
            return Classification {
                activity: &rule.activity,
                detail: detail(call, &commands, matched, leading.as_deref()),
            };
        }
        Classification {
            activity: if is_bash { OTHER_SHELL } else { OTHER },
            detail: detail(call, &commands, None, leading.as_deref()),
        }
    }
}

/// The detail of a call: see the module docs. `matched` is the simple
/// command a rule's pattern matched, if any.
///
/// A Bash line of several simple commands is named after its most
/// significant one: the one the matching rule's pattern matched (rules go
/// tests, lint, build, git, … in that order, so `cargo build && cargo
/// test` is `cargo test`), else the one it leads with (`cd web && pnpm
/// dev` is `pnpm dev`, see [`shell`]).
fn detail<'c>(
    call: &ToolCall<'c>,
    commands: &[SimpleCommand],
    matched: Option<usize>,
    leading: Option<&str>,
) -> Cow<'c, str> {
    if let Some(server) = call.mcp_server {
        return Cow::Borrowed(server);
    }
    if call.tool_name != "Bash" {
        return Cow::Borrowed(call.tool_name);
    }
    let command = matched
        .map(|i| &commands[i])
        .or_else(|| leading.and_then(|leading| commands.iter().find(|c| c.program() == leading)))
        .or_else(|| shell::leading(commands));
    match (command, leading) {
        (Some(command), _) => Cow::Owned(shell::command_key(command)),
        (None, Some(leading)) => Cow::Owned(leading.to_owned()),
        (None, None) => Cow::Borrowed(call.tool_name),
    }
}

/// A tool-name pattern: `*` matches any run of characters, `?` any one.
#[derive(Debug, Clone)]
struct Glob(Vec<char>);

impl Glob {
    fn new(pattern: &str) -> Self {
        Self(pattern.chars().collect())
    }

    fn matches(&self, text: &str) -> bool {
        let text: Vec<char> = text.chars().collect();
        let (p, t) = (&self.0, &text);
        // Iterative wildcard matching with backtracking to the last `*`.
        let (mut pi, mut ti) = (0, 0);
        let mut star: Option<(usize, usize)> = None;
        while ti < t.len() {
            if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
                pi += 1;
                ti += 1;
            } else if pi < p.len() && p[pi] == '*' {
                star = Some((pi, ti));
                pi += 1;
            } else if let Some((sp, st)) = star {
                pi = sp + 1;
                ti = st + 1;
                star = Some((sp, st + 1));
            } else {
                return false;
            }
        }
        p[pi..].iter().all(|c| *c == '*')
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bash(command: &str) -> (String, String) {
        // Without the leading command ingest derives, classify derives it
        // from the command line by the same rule.
        let c = ActivityRules::builtin().classify(&ToolCall {
            tool_name: "Bash",
            mcp_server: None,
            bash_command: None,
            command: Some(command),
        });
        (c.activity.to_owned(), c.detail.into_owned())
    }

    fn tool(name: &str) -> (String, String) {
        let server = name
            .strip_prefix("mcp__")
            .and_then(|rest| rest.split("__").next());
        let c = ActivityRules::builtin().classify(&ToolCall {
            tool_name: name,
            mcp_server: server,
            ..ToolCall::default()
        });
        (c.activity.to_owned(), c.detail.into_owned())
    }

    #[test]
    fn builtin_rules_classify_representative_shell_commands() {
        let table: &[(&str, &str, &str)] = &[
            // Tests
            ("cargo test", "Tests", "cargo test"),
            (
                "cargo test --workspace -- --nocapture",
                "Tests",
                "cargo test",
            ),
            ("cargo nextest run", "Tests", "cargo nextest"),
            ("cd crates/api && cargo test", "Tests", "cargo test"),
            ("RUST_LOG=debug cargo test -p api", "Tests", "cargo test"),
            ("npm test", "Tests", "npm test"),
            ("npm run test:unit", "Tests", "npm run test:unit"),
            ("pnpm test", "Tests", "pnpm test"),
            ("pnpm --filter web test", "Tests", "pnpm test"),
            ("yarn test", "Tests", "yarn test"),
            ("bun test", "Tests", "bun test"),
            ("npx jest src/app.test.ts", "Tests", "npx jest"),
            ("npx vitest run", "Tests", "npx vitest"),
            ("pnpm exec playwright test", "Tests", "pnpm exec playwright"),
            ("npx mocha", "Tests", "npx mocha"),
            ("pytest -x tests/", "Tests", "pytest"),
            ("python -m pytest -q", "Tests", "python -m pytest"),
            (
                "python3 -m unittest discover",
                "Tests",
                "python3 -m unittest",
            ),
            ("uv run pytest", "Tests", "uv run pytest"),
            ("go test ./...", "Tests", "go test"),
            ("mix test", "Tests", "mix test"),
            (
                "bundle exec rspec spec/models",
                "Tests",
                "bundle exec rspec",
            ),
            ("rspec", "Tests", "rspec"),
            ("vendor/bin/phpunit", "Tests", "phpunit"),
            ("dotnet test", "Tests", "dotnet test"),
            ("mvn -q test", "Tests", "mvn test"),
            ("./gradlew test", "Tests", "gradlew test"),
            ("make test", "Tests", "make test"),
            ("just test", "Tests", "just test"),
            ("deno test", "Tests", "deno test"),
            ("swift test", "Tests", "swift test"),
            ("ctest --output-on-failure", "Tests", "ctest"),
            ("cargo build && cargo test", "Tests", "cargo test"),
            ("cargo test 2>&1 | tail -20", "Tests", "cargo test"),
            ("timeout 60 cargo test", "Tests", "cargo test"),
            // Lint & format
            (
                "cargo clippy --all-targets -- -D warnings",
                "Lint & format",
                "cargo clippy",
            ),
            ("cargo fmt --all --check", "Lint & format", "cargo fmt"),
            ("npx eslint .", "Lint & format", "npx eslint"),
            ("prettier --write src", "Lint & format", "prettier"),
            ("pnpm lint", "Lint & format", "pnpm lint"),
            ("biome check .", "Lint & format", "biome check"),
            ("ruff check .", "Lint & format", "ruff check"),
            ("black .", "Lint & format", "black"),
            ("golangci-lint run", "Lint & format", "golangci-lint"),
            // Build & typecheck
            ("cargo build --release", "Build & typecheck", "cargo build"),
            ("cargo check", "Build & typecheck", "cargo check"),
            ("tsc --noEmit", "Build & typecheck", "tsc"),
            ("npx tsc --noEmit", "Build & typecheck", "npx tsc"),
            ("npm run build", "Build & typecheck", "npm run build"),
            ("go build ./...", "Build & typecheck", "go build"),
            ("make", "Build & typecheck", "make"),
            ("vite build", "Build & typecheck", "vite build"),
            ("next build", "Build & typecheck", "next build"),
            (
                "xcodebuild -scheme App build",
                "Build & typecheck",
                "xcodebuild",
            ),
            // Git & GitHub
            ("git status", "Git & GitHub", "git status"),
            ("cd web && git diff --stat", "Git & GitHub", "git diff"),
            (
                "git commit -m \"fix: run cargo test in CI\"",
                "Git & GitHub",
                "git commit",
            ),
            ("gh pr create --fill", "Git & GitHub", "gh pr"),
            // Dependencies
            ("npm install", "Dependencies", "npm install"),
            ("pnpm add -D vitest", "Dependencies", "pnpm add"),
            ("yarn add react", "Dependencies", "yarn add"),
            ("cargo add serde", "Dependencies", "cargo add"),
            (
                "pip install -r requirements.txt",
                "Dependencies",
                "pip install",
            ),
            ("uv sync", "Dependencies", "uv sync"),
            ("brew install jq", "Dependencies", "brew install"),
            // Run & scripts
            ("npm run dev", "Run & scripts", "npm run dev"),
            ("pnpm start", "Run & scripts", "pnpm start"),
            ("cargo run -- serve", "Run & scripts", "cargo run"),
            ("node scripts/seed.js", "Run & scripts", "node"),
            ("python scripts/report.py", "Run & scripts", "python"),
            ("./scripts/deploy.sh staging", "Run & scripts", "deploy.sh"),
            // Search, read, edit
            ("rg -n 'fn main' src", "Search code", "rg"),
            ("grep -rn TODO src", "Search code", "grep"),
            ("find . -name '*.rs'", "Search code", "find"),
            ("fd config", "Search code", "fd"),
            ("ls -la", "Search code", "ls"),
            ("cat Cargo.toml", "Read files", "cat"),
            ("head -50 src/main.rs", "Read files", "head"),
            ("tail -f logs/app.log", "Read files", "tail"),
            ("sed -n '10,40p' src/lib.rs", "Read files", "sed"),
            ("sed -i '' 's/a/b/' src/lib.rs", "Edit files", "sed"),
            ("curl -s https://example.com", "Web", "curl"),
            // Fallback
            ("echo hello", "Other shell", "echo"),
            // Waiting & polling: what the whole line does is wait.
            ("sleep 5", "Waiting & polling", "sleep"),
            (
                "echo 'waiting for CI'; sleep 60",
                "Waiting & polling",
                "sleep",
            ),
            (
                "until grep -q 'Test Files' /tmp/run.log; do sleep 10; done",
                "Waiting & polling",
                "until grep",
            ),
            (
                "while pgrep -f vitest >/dev/null; do sleep 2; done",
                "Waiting & polling",
                "while pgrep",
            ),
            (
                "until [ -f /tmp/done ]; do sleep 5; done",
                "Waiting & polling",
                "until test",
            ),
            // A sleep before real work does not make the line a wait.
            ("sleep 30 && gh run view 123", "Git & GitHub", "gh run"),
            ("sleep 2; cargo test", "Tests", "cargo test"),
        ];
        let mut wrong = Vec::new();
        for (command, activity, detail) in table {
            let got = bash(command);
            if got != (activity.to_string(), detail.to_string()) {
                wrong.push(format!(
                    "{command:?} → {got:?}, expected ({activity:?}, {detail:?})"
                ));
            }
        }
        assert!(wrong.is_empty(), "misclassified:\n{}", wrong.join("\n"));
    }

    /// A Bash call's detail is its command key (`/commands`): program and
    /// meaningful subcommand, never a path, a file or an argument. Shapes
    /// after real command lines, all content invented.
    #[test]
    fn a_bash_calls_detail_is_its_command_key() {
        let table: &[(&str, &str)] = &[
            // Program + subcommand, arguments dropped.
            ("git status --short", "git status"),
            (
                "git -C /Users/alice/code/acme-api log --oneline -5",
                "git log",
            ),
            ("git --no-pager diff --stat", "git diff"),
            ("gh pr view 42 --json title,body", "gh pr"),
            ("gh issue view 795 --json title -q .title", "gh issue"),
            ("cargo test --workspace -- --nocapture", "cargo test"),
            ("cargo +nightly fmt --all", "cargo fmt"),
            ("cargo clippy --all-targets -- -D warnings", "cargo clippy"),
            ("go test ./internal/cli/ -run TestLogin -count=1", "go test"),
            ("make -C services/api test", "make test"),
            ("make -j4 build", "make build"),
            // Package managers and their runners.
            ("pnpm test", "pnpm test"),
            ("pnpm run build", "pnpm run build"),
            ("pnpm --filter @acme/api test", "pnpm test"),
            (
                "pnpm -F web exec vitest run src/app.test.ts",
                "pnpm exec vitest",
            ),
            (
                "pnpm exec vitest run test/login.test.ts",
                "pnpm exec vitest",
            ),
            ("npx vitest run src/foo.test.ts", "npx vitest"),
            ("npx -y prettier --write src", "npx prettier"),
            ("npm run test:unit -- --watch=false", "npm run test:unit"),
            ("npm --prefix web run lint", "npm run lint"),
            (
                "yarn workspace @acme/api test test/login.test.ts",
                "yarn test",
            ),
            ("yarn test:e2e --project chromium", "yarn test:e2e"),
            ("uv run pytest -x tests/test_api.py", "uv run pytest"),
            (
                "uv run --with rich python scripts/report.py",
                "uv run python",
            ),
            ("python -m pytest -q tests/", "python -m pytest"),
            ("python3 - <<'EOF'\nprint(1)\nEOF", "python3"),
            ("python3 scripts/seed.py --count 10", "python3"),
            (
                "docker compose -f docker/dev.yml up -d db",
                "docker compose up",
            ),
            (
                "docker exec acme-db-1 psql -U acme -c 'select 1'",
                "docker exec",
            ),
            ("biome check --write src", "biome check"),
            // Programs without subcommands: the program alone.
            ("vitest run src/foo.test.ts", "vitest"),
            ("pytest -x tests/test_login.py::test_ok", "pytest"),
            ("node -e 'console.log(1)'", "node"),
            ("/usr/local/bin/rg -n 'fn main' src", "rg"),
            ("./scripts/deploy.sh staging", "deploy.sh"),
            // Chains: the simple command the matching rule matched, else
            // the leading one (setup commands skipped).
            ("cd apps/web && pnpm test 2>&1 | tail -30", "pnpm test"),
            ("cargo build && cargo test", "cargo test"),
            (
                "export PATH=\"$HOME/.nvm/bin:$PATH\" && npx tsc --noEmit -p apps/api",
                "npx tsc",
            ),
            ("echo '=== api' && git log --oneline | head -5", "git log"),
            ("sleep 30 && gh run view 123 --log-failed", "gh run"),
            ("set -e; source .venv/bin/activate; pytest", "pytest"),
            (
                "for f in a.ts b.ts; do npx biome check $f; done",
                "npx biome",
            ),
            // A polling loop is named after its loop and condition.
            ("until [ -f /tmp/done ]; do sleep 5; done", "until test"),
            (
                "until grep -q 'Test Files' /tmp/run.log; do sleep 10; done",
                "until grep",
            ),
            (
                "while pgrep -f vitest >/dev/null; do sleep 2; done",
                "while pgrep",
            ),
            ("if [ -d node_modules ]; then yarn build; fi", "yarn build"),
            ("cd /Users/alice/code/acme-api", "cd"),
            // Secrets are redacted before storage; assignments never kept.
            ("GITHUB_TOKEN=[REDACTED] gh api repos/acme/api", "gh api"),
            (
                "curl -H 'Authorization: Bearer [REDACTED]' https://x.test",
                "curl",
            ),
        ];
        let mut wrong = Vec::new();
        for (command, key) in table {
            let got = bash(command).1;
            if got != *key {
                wrong.push(format!("{command:?} → {got:?}, expected {key:?}"));
            }
        }
        assert!(
            wrong.is_empty(),
            "wrong command keys:\n{}",
            wrong.join("\n")
        );
    }

    #[test]
    fn a_classification_cache_gives_the_same_answers() {
        let plain = ActivityRules::builtin();
        let cached = plain.with_classification_cache();
        let calls = [
            ("Bash", None, None, Some("cd web && pnpm test 2>&1 | tail")),
            ("Bash", None, Some("git"), Some("git status")),
            (
                "Bash",
                None,
                None,
                Some("until grep -q ok log; do sleep 1; done"),
            ),
            ("Read", None, None, None),
            ("mcp__github__get_issue", Some("github"), None, None),
        ];
        for _ in 0..2 {
            for (tool_name, mcp_server, bash_command, command) in calls {
                let call = ToolCall {
                    tool_name,
                    mcp_server,
                    bash_command,
                    command,
                };
                assert_eq!(cached.classify(&call), plain.classify(&call), "{call:?}");
            }
        }
    }

    #[test]
    fn builtin_rules_classify_tools_by_name() {
        let table: &[(&str, &str, &str)] = &[
            ("Grep", "Search code", "Grep"),
            ("Glob", "Search code", "Glob"),
            ("Read", "Read files", "Read"),
            ("Edit", "Edit files", "Edit"),
            ("MultiEdit", "Edit files", "MultiEdit"),
            ("Write", "Edit files", "Write"),
            ("NotebookEdit", "Edit files", "NotebookEdit"),
            ("WebFetch", "Web", "WebFetch"),
            ("WebSearch", "Web", "WebSearch"),
            ("Agent", "Subagents", "Agent"),
            ("Task", "Subagents", "Task"),
            ("Skill", "Skills", "Skill"),
            ("mcp__github__create_issue", "MCP", "github"),
            ("mcp__claude-in-chrome__navigate", "MCP", "claude-in-chrome"),
            ("TodoWrite", "Planning & todos", "TodoWrite"),
            ("TaskCreate", "Planning & todos", "TaskCreate"),
            ("TaskUpdate", "Planning & todos", "TaskUpdate"),
            ("ExitPlanMode", "Planning & todos", "ExitPlanMode"),
            ("AskUserQuestion", "Planning & todos", "AskUserQuestion"),
            ("SomeFutureTool", "Other", "SomeFutureTool"),
        ];
        for (name, activity, detail) in table {
            assert_eq!(
                tool(name),
                (activity.to_string(), detail.to_string()),
                "{name}"
            );
        }
    }

    #[test]
    fn quoted_text_and_here_documents_are_not_commands() {
        for (command, activity) in [
            ("echo \"cargo test && pnpm test\"", "Other shell"),
            (
                "git commit -m \"$(cat <<'EOF'\nfix: cargo test\n\nnpm test\nEOF\n)\"",
                "Git & GitHub",
            ),
            (
                "cat <<EOF > notes.md\ncargo test\nEOF\necho done",
                "Read files",
            ),
            ("cargo build 2>&1 && echo ok", "Build & typecheck"),
            ("sudo -u alice env FOO=1 pytest", "Tests"),
            ("# run the suite\ncargo test", "Tests"),
        ] {
            assert_eq!(bash(command).0, activity, "{command:?}");
        }
    }

    #[test]
    fn tool_globs_match_whole_names() {
        let rules = ActivityRules::from_toml(
            r#"
            [[rule]]
            activity = "GitHub MCP"
            tools = ["mcp__github__*"]

            [[rule]]
            activity = "Notebook"
            tools = ["Notebook?dit"]
            "#,
        )
        .unwrap();
        let classify = |name: &str| {
            rules
                .classify(&ToolCall {
                    tool_name: name,
                    ..ToolCall::default()
                })
                .activity
                .to_owned()
        };
        assert_eq!(classify("mcp__github__create_issue"), "GitHub MCP");
        assert_eq!(classify("mcp__gitlab__create_issue"), OTHER);
        assert_eq!(classify("NotebookEdit"), "Notebook");
        assert_eq!(classify("NotebookEditor"), OTHER);
    }

    #[test]
    fn user_rules_come_before_the_builtin_ones() {
        let user = ActivityRules::from_toml(
            r#"
            version = "mine-1"

            [[rule]]
            activity = "E2E tests"
            tools = ["Bash"]
            pattern = '^pnpm\s+(?:run\s+)?e2e\b'

            [[rule]]
            activity = "Build & typecheck"
            tools = ["Bash"]
            commands = ["cargo"]
            pattern = '^cargo\s+clippy\b'
            "#,
        )
        .unwrap();
        let rules = ActivityRules::builtin().with_overrides(&user);
        let classify = |command: &str| {
            rules
                .classify(&ToolCall {
                    tool_name: "Bash",
                    command: Some(command),
                    ..ToolCall::default()
                })
                .activity
                .to_owned()
        };

        assert_eq!(classify("pnpm e2e"), "E2E tests");
        assert_eq!(classify("cargo clippy"), "Build & typecheck");
        assert_eq!(
            classify("cargo test"),
            "Tests",
            "built-in rules still apply"
        );
        assert!(
            rules.version.ends_with("user rules mine-1"),
            "{}",
            rules.version
        );
    }

    #[test]
    fn invalid_rules_are_rejected_with_the_rule_named() {
        for (text, expected) in [
            (
                "[[rule]]\nactivity = \"X\"\ntools = []",
                "rule 1 (X): `tools` is empty",
            ),
            (
                "[[rule]]\nactivity = \"X\"\ntools = [\"Bash\"]\npattern = \"(\"",
                "rule 1 (X): invalid pattern",
            ),
            (
                "[[rule]]\nactivity = \"X\"\ntool = [\"Bash\"]",
                "invalid activity rules",
            ),
        ] {
            let err = format!("{:#}", ActivityRules::from_toml(text).unwrap_err());
            assert!(err.contains(expected), "{err}");
        }
    }

    #[test]
    fn the_builtin_file_is_versioned() {
        assert!(!ActivityRules::builtin().version.is_empty());
    }
}
