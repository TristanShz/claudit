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
//! counted in within the activity: for Bash, the command normalized to its
//! program and subcommand (`cargo test`, `pnpm exec vitest`,
//! `python -m pytest`); for an MCP tool, its server; otherwise the tool
//! name.

use std::borrow::Cow;
use std::sync::{Mutex, OnceLock};

use anyhow::{Context, Result, bail};
use regex::Regex;
use serde::Deserialize;

use crate::logfile;
use crate::paths::Paths;

const BUILTIN: &str = include_str!("../activities/rules.toml");

/// The activity of a Bash call no rule matches.
pub const OTHER_SHELL: &str = "Other shell";
/// The activity of any other call no rule matches.
pub const OTHER: &str = "Other";

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

/// An ordered list of rules. Cheap to clone (compiled regexes are shared).
#[derive(Debug, Clone)]
pub struct ActivityRules {
    /// `version` of the built-in file, plus the user file's when merged.
    pub version: String,
    rules: Vec<Rule>,
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
        })
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
        let is_bash = call.tool_name == "Bash";
        let segments = match (is_bash, call.command) {
            (true, Some(command)) => simple_commands(command),
            _ => Vec::new(),
        };
        let leading: Option<Cow<'c, str>> = call
            .bash_command
            .map(Cow::Borrowed)
            .or_else(|| leading_program(&segments).map(|p| Cow::Owned(p.to_owned())));

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
                matched = segments.iter().position(|s| pattern.is_match(&s.text));
                if matched.is_none() {
                    continue;
                }
            }
            return Classification {
                activity: &rule.activity,
                detail: detail(call, &segments, matched, leading.as_deref()),
            };
        }
        Classification {
            activity: if is_bash { OTHER_SHELL } else { OTHER },
            detail: detail(call, &segments, None, leading.as_deref()),
        }
    }
}

/// The detail of a call: see the module docs. `matched` is the simple
/// command a rule's pattern matched, if any.
fn detail<'c>(
    call: &ToolCall<'c>,
    segments: &[SimpleCommand],
    matched: Option<usize>,
    leading: Option<&str>,
) -> Cow<'c, str> {
    if let Some(server) = call.mcp_server {
        return Cow::Borrowed(server);
    }
    if call.tool_name != "Bash" {
        return Cow::Borrowed(call.tool_name);
    }
    let segment = matched
        .map(|i| &segments[i])
        .or_else(|| leading.and_then(|leading| segments.iter().find(|s| s.words[0] == leading)))
        .or_else(|| segments.iter().find(|s| !is_directory_change(&s.words[0])))
        .or(segments.first());
    match (segment, leading) {
        (Some(segment), _) => Cow::Owned(normalized(&segment.words)),
        (None, Some(leading)) => Cow::Owned(leading.to_owned()),
        (None, None) => Cow::Borrowed(call.tool_name),
    }
}

/// Programs whose first argument is a subcommand worth keeping
/// (`git status`, `cargo test`, `brew install`).
const SUBCOMMAND_PROGRAMS: &[&str] = &[
    "apt",
    "apt-get",
    "astro",
    "brew",
    "bun",
    "bunx",
    "bundle",
    "cargo",
    "composer",
    "conda",
    "deno",
    "docker",
    "docker-compose",
    "dotnet",
    "gem",
    "gh",
    "git",
    "glab",
    "go",
    "gradle",
    "gradlew",
    "hatch",
    "just",
    "kubectl",
    "make",
    "mix",
    "mvn",
    "mvnw",
    "next",
    "npm",
    "npx",
    "nuxt",
    "nx",
    "pdm",
    "pip",
    "pip3",
    "pipenv",
    "pipx",
    "pnpm",
    "pod",
    "podman",
    "poetry",
    "rake",
    "rustup",
    "swift",
    "terraform",
    "turbo",
    "uv",
    "uvx",
    "vite",
    "yarn",
    "zig",
];

/// Subcommands that run something named by the next word
/// (`npm run build`, `pnpm exec vitest`, `bundle exec rspec`).
const RUNNERS: &[&str] = &["run", "exec", "dlx", "x"];

/// A command's program and subcommand words: `cargo test`,
/// `npm run build`, `python -m pytest`, or the program alone.
fn normalized(words: &[String]) -> String {
    let program = words[0].as_str();
    let rest = &words[1..];
    let is_word = |w: &&String| {
        !w.is_empty()
            && !w.starts_with('-')
            && !w.contains(['/', '=', '.', '\'', '"', '$', '*', '@'])
    };
    let mut out = vec![program];
    let is_python = program.starts_with("python") || program == "py";
    if is_python && rest.first().is_some_and(|w| w == "-m") {
        out.push("-m");
        out.extend(rest.get(1).map(String::as_str));
    } else if SUBCOMMAND_PROGRAMS.contains(&program)
        && let Some(sub) = rest.first().filter(is_word)
    {
        out.push(sub);
        if RUNNERS.contains(&sub.as_str())
            && let Some(target) = rest.get(1).filter(is_word)
        {
            out.push(target);
        }
    }
    out.join(" ")
}

fn is_directory_change(program: &str) -> bool {
    matches!(program, "cd" | "pushd" | "popd")
}

/// The leading command of a line (the rule of `ingest::bash_command`): the
/// first simple command that is not a directory change, else `cd`.
fn leading_program(segments: &[SimpleCommand]) -> Option<&str> {
    segments
        .iter()
        .map(|s| s.words[0].as_str())
        .find(|p| !is_directory_change(p))
        .or_else(|| segments.first().map(|s| s.words[0].as_str()))
}

/// Programs that only run another command, and their options taking a
/// value (as in `ingest::bash_command`).
const WRAPPERS: [(&str, &[&str]); 8] = [
    (
        "sudo",
        &["-u", "-g", "-C", "-D", "-h", "-p", "-r", "-t", "-U"],
    ),
    ("env", &["-u", "-C", "-S"]),
    ("time", &["-f", "-o"]),
    ("nohup", &[]),
    ("nice", &["-n"]),
    ("timeout", &["-k", "-s"]),
    ("command", &[]),
    ("exec", &["-a"]),
];

/// One simple command of a line, prefixes and wrappers removed.
#[derive(Debug)]
struct SimpleCommand {
    /// Unquoted words, the program reduced to its file name. Never empty.
    words: Vec<String>,
    /// The program's file name, then the rest of the original text (quotes
    /// kept): what rule patterns are matched against.
    text: String,
}

/// A word of a line: its unquoted text and where it ends in the line.
struct Word {
    text: String,
    end: usize,
}

/// Splits a command line into simple commands, at `&&`, `||`, `;`, `|`,
/// `&`, newlines and parentheses outside quotes, skipping here-document
/// bodies.
fn simple_commands(line: &str) -> Vec<SimpleCommand> {
    let mut commands = Vec::new();
    let mut words: Vec<Word> = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut heredoc: Option<String> = None;
    let mut chars = line.char_indices().peekable();

    let finish = |words: &mut Vec<Word>, commands: &mut Vec<SimpleCommand>, end: usize| {
        if let Some(command) = simple_command(line, std::mem::take(words), end) {
            commands.push(command);
        }
    };

    while let Some((at, c)) = chars.next() {
        let end_word = |word: &mut String, in_word: &mut bool, words: &mut Vec<Word>| {
            if *in_word {
                words.push(Word {
                    text: std::mem::take(word),
                    end: at,
                });
                *in_word = false;
            }
        };
        match c {
            '\'' => {
                in_word = true;
                for (_, q) in chars.by_ref() {
                    if q == '\'' {
                        break;
                    }
                    word.push(q);
                }
            }
            '"' => {
                in_word = true;
                while let Some((_, q)) = chars.next() {
                    match q {
                        '"' => break,
                        '\\' => word.extend(chars.next().map(|(_, e)| e)),
                        _ => word.push(q),
                    }
                }
            }
            '\\' => {
                in_word = true;
                match chars.next() {
                    Some((_, '\n')) | None => {}
                    Some((_, escaped)) => word.push(escaped),
                }
            }
            '#' if !in_word => {
                for (_, rest) in chars.by_ref() {
                    if rest == '\n' {
                        break;
                    }
                }
                finish(&mut words, &mut commands, at);
            }
            '<' if !in_word && line[at..].starts_with("<<") && !line[at..].starts_with("<<<") => {
                chars.next();
                let rest = &line[at + 2..];
                let delimiter: String = rest
                    .trim_start_matches('-')
                    .trim_start()
                    .chars()
                    .take_while(|c| !c.is_whitespace() && !matches!(c, ';' | '&' | '|' | ')'))
                    .filter(|c| !matches!(c, '\'' | '"' | '\\'))
                    .collect();
                if !delimiter.is_empty() {
                    heredoc = Some(delimiter);
                }
            }
            // Redirections `2>&1`, `>&2` and `&>file` are not separators.
            '&' if (in_word && word.ends_with('>'))
                || chars.peek().map(|(_, c)| *c) == Some('>') =>
            {
                in_word = true;
                word.push(c);
            }
            ';' | '&' | '|' | '\n' | '(' | ')' => {
                end_word(&mut word, &mut in_word, &mut words);
                finish(&mut words, &mut commands, at);
                if c == '\n'
                    && let Some(delimiter) = heredoc.take()
                {
                    // Skip the body, up to and including the delimiter line.
                    let mut current = String::new();
                    for (_, b) in chars.by_ref() {
                        if b == '\n' {
                            if current.trim() == delimiter {
                                break;
                            }
                            current.clear();
                        } else {
                            current.push(b);
                        }
                    }
                }
            }
            c if c.is_whitespace() => end_word(&mut word, &mut in_word, &mut words),
            c => {
                in_word = true;
                word.push(c);
            }
        }
    }
    if in_word {
        words.push(Word {
            text: word,
            end: line.len(),
        });
    }
    finish(&mut words, &mut commands, line.len());
    commands
}

/// The simple command of `words` (ending at byte `end` of `line`), without
/// its assignments, `{` / `!` and wrappers; `None` when nothing is left.
fn simple_command(line: &str, words: Vec<Word>, end: usize) -> Option<SimpleCommand> {
    let mut rest = words.into_iter().peekable();
    while rest
        .peek()
        .is_some_and(|w| is_assignment(&w.text) || w.text == "{" || w.text == "!")
    {
        rest.next();
    }
    loop {
        let first = rest.next()?;
        let program = first
            .text
            .rsplit('/')
            .next()
            .unwrap_or(&first.text)
            .to_owned();
        if program.is_empty() {
            return None;
        }
        let Some((wrapper, valued)) = WRAPPERS.iter().find(|(name, _)| *name == program) else {
            let text = format!("{program}{}", line[first.end..end].trim_end());
            let mut words = vec![program];
            words.extend(rest.map(|w| w.text));
            return Some(SimpleCommand { words, text });
        };
        while let Some(next) = rest.peek() {
            if next.text.starts_with('-') && next.text.len() > 1 {
                let option = rest.next().expect("peeked");
                if valued.contains(&option.text.as_str()) {
                    rest.next();
                }
            } else if *wrapper == "env" && is_assignment(&next.text) {
                rest.next();
            } else {
                break;
            }
        }
        if *wrapper == "timeout" {
            rest.next();
        }
    }
}

/// `NAME=value` with a valid shell variable name.
fn is_assignment(word: &str) -> bool {
    let Some((name, _value)) = word.split_once('=') else {
        return false;
    };
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
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
            ("pnpm --filter web test", "Tests", "pnpm"),
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
            ("mvn -q test", "Tests", "mvn"),
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
            ("biome check .", "Lint & format", "biome"),
            ("ruff check .", "Lint & format", "ruff"),
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
            ("sleep 5", "Other shell", "sleep"),
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
