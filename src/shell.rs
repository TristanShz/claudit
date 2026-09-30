//! Bash command lines, as far as claudit reads them: split into simple
//! commands, their **leading command** (the program a line is about:
//! `git`, `cargo`, `pnpm`) and each simple command's **command key** (its
//! program and meaningful subcommand: `cargo test`, `pnpm exec vitest`,
//! `docker compose up`). The one shell reader of the crate: ingest derives
//! `tool_calls.bash_command` with [`leading_command`], and
//! [`crate::activities`] matches its rules against [`simple_commands`] and
//! names a Bash call by [`command_key`].
//!
//! **Splitting** (a light tokenization, no expansion): the line is cut at
//! `&&`, `||`, `;`, `|`, `&`, newlines and parentheses outside quotes;
//! quotes and backslash escapes are respected, `# comments` and
//! here-document bodies skipped, redirections like `2>&1` kept in their
//! word. In each simple command, leading `NAME=value` assignments, the
//! shell keywords that introduce a command (`{`, `!`, `if`, `then`, `else`,
//! `elif`, `while`, `until`, `do`) and transparent wrappers with their
//! options (`sudo`, `env` and its assignments, `time`, `nohup`, `nice`,
//! `timeout <duration>`, `command`, `exec`) are removed, and the program is
//! reduced to its file name (`/usr/bin/git` → `git`). Pieces that are not
//! commands (`for x in …`, `case …`, `done`, `fi`, `esac`, `}`) are dropped.
//!
//! **Leading command**: the program of the first simple command that is not
//! a *setup* command. Setup commands (directory changes `cd` / `pushd` /
//! `popd`, `export`, `set`, `unset`, `source` / `.`, `echo`, `printf`,
//! `sleep`, `true`, `false`, `:`, `[` / `[[` / `test`) only prepare or
//! label what follows, so `cd web && pnpm build` leads with `pnpm`; a line
//! of setup commands only leads with its `sleep` if it has one
//! (`echo a; sleep 5` waits), else its first one that is not a condition,
//! else its first. A line that opens with a polling loop
//! (`until grep -q done log; do sleep 5; done`) leads with the loop's
//! keyword (`until`, `while`), and its command key names the loop and its
//! condition (`until grep`): its time is spent waiting.

/// Programs that only run another command, and their options taking a
/// value.
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

/// Shell keywords that introduce the command after them.
const KEYWORD_PREFIXES: &[&str] = &[
    "{", "!", "if", "then", "else", "elif", "while", "until", "do",
];

/// Shell keywords that start something that is not a command.
const NOT_COMMANDS: &[&str] = &[
    "for", "select", "case", "esac", "done", "fi", "}", "function", "in",
];

/// Commands that only prepare or label the real work (see the module docs).
const SETUP_COMMANDS: &[&str] = &[
    "cd", "pushd", "popd", "export", "set", "unset", "source", ".", "echo", "printf", "sleep",
    "true", "false", ":", "[", "[[", "test",
];

/// Setup commands that only test a condition: the weakest candidates.
const CONDITIONS: &[&str] = &["true", "false", ":", "[", "[[", "test"];

/// One simple command of a line, prefixes and wrappers removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimpleCommand {
    /// Unquoted words, the program reduced to its file name. Never empty.
    pub words: Vec<String>,
    /// The program's file name, then the rest of the original text (quotes
    /// kept): what activity rule patterns are matched against.
    pub text: String,
    /// `until` or `while` when it is the condition of such a loop.
    pub loop_keyword: Option<&'static str>,
}

impl SimpleCommand {
    pub fn program(&self) -> &str {
        &self.words[0]
    }

    /// What it counts as when a line leads with it: the loop keyword for
    /// the condition of an `until` / `while` loop, else its program.
    pub fn leading_name(&self) -> &str {
        self.loop_keyword.unwrap_or(self.program())
    }

    /// Whether it only prepares or labels what follows (`cd`, `export`, …).
    pub fn is_setup(&self) -> bool {
        SETUP_COMMANDS.contains(&self.program())
    }
}

/// The leading command of `command_line` (see the module docs).
pub fn leading_command(command_line: &str) -> Option<String> {
    leading(&simple_commands(command_line)).map(|c| c.leading_name().to_owned())
}

/// The simple command a line leads with: the condition of the loop the line
/// opens with (`until grep -q done log; do sleep 5; done` polls with
/// `grep`), else the first that is not a setup command, else its first
/// `sleep` (`echo waiting; sleep 60` waits), else the first that is not a
/// condition (`[ -f x ]`, `true`), else the first.
pub fn leading(commands: &[SimpleCommand]) -> Option<&SimpleCommand> {
    commands
        .first()
        .filter(|c| c.loop_keyword.is_some())
        .or_else(|| commands.iter().find(|c| !c.is_setup()))
        .or_else(|| commands.iter().find(|c| c.program() == "sleep"))
        .or_else(|| commands.iter().find(|c| !CONDITIONS.contains(&c.program())))
        .or_else(|| commands.first())
}

/// Programs whose first argument is a subcommand worth keeping
/// (`git status`, `cargo test`, `brew install`).
const SUBCOMMAND_PROGRAMS: &[&str] = &[
    "apt",
    "apt-get",
    "astro",
    "aws",
    "biome",
    "brew",
    "bun",
    "bunx",
    "bundle",
    "cargo",
    "colima",
    "composer",
    "conda",
    "deno",
    "docker",
    "docker-compose",
    "dotnet",
    "drizzle-kit",
    "flyctl",
    "gcloud",
    "gem",
    "gh",
    "git",
    "glab",
    "go",
    "gradle",
    "gradlew",
    "hatch",
    "helm",
    "just",
    "kubectl",
    "launchctl",
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
    "playwright",
    "pnpm",
    "pod",
    "podman",
    "poetry",
    "prisma",
    "rake",
    "ruff",
    "rustup",
    "supabase",
    "swift",
    "systemctl",
    "terraform",
    "turbo",
    "uv",
    "uvx",
    "vite",
    "wrangler",
    "yarn",
    "zig",
];

/// Options that take a value, skipped with it before a subcommand or a
/// runner's target (`pnpm --filter web test`, `git -C repo status`,
/// `make -C sub test`, `docker compose -f dev.yml up`).
const VALUED_OPTIONS: &[&str] = &[
    "-C",
    "-c",
    "-F",
    "-f",
    "-w",
    "-p",
    "--filter",
    "--dir",
    "--cwd",
    "--prefix",
    "--workspace",
    "--project",
    "--directory",
    "--with",
    "--package",
    "--manifest-path",
    "--file",
    "--env-file",
    "--profile",
    "--context",
    "--config",
    "--git-dir",
    "--work-tree",
];

/// Subcommands that run something named by the next word
/// (`npm run build`, `pnpm exec vitest`, `bundle exec rspec`), and the
/// package managers they do so in (`docker exec <container>` does not).
const RUNNERS: &[&str] = &["run", "exec", "dlx", "x"];
const RUNNER_PROGRAMS: &[&str] = &[
    "npm", "pnpm", "yarn", "bun", "bundle", "uv", "poetry", "pdm", "pipenv", "hatch",
];

/// Subcommands that group further subcommands, kept with the next one
/// (`docker compose up`).
const GROUPS: &[(&str, &str)] = &[
    ("docker", "compose"),
    ("docker", "buildx"),
    ("podman", "compose"),
];

/// The command key of one simple command: its program, then for programs
/// with subcommands the subcommand, a runner's target (`npm run build`,
/// `uv run pytest`) or a group's subcommand (`docker compose up`);
/// `python -m <module>` for Python; `until <program>` / `while <program>`
/// for the condition of a polling loop (`[` counts as `test`). Options (and the values of the common
/// ones that take one) are skipped before a subcommand; paths, files,
/// assignments and other arguments are never kept: a kept word starts with
/// a letter and has no `/`, `.`, `=`, quote, `$` or bracket.
pub fn command_key(command: &SimpleCommand) -> String {
    let words = &command.words;
    let program = words[0].as_str();
    if let Some(keyword) = command.loop_keyword {
        let condition = if matches!(program, "[" | "[[") {
            "test"
        } else {
            program
        };
        return format!("{keyword} {condition}");
    }
    let mut out = vec![program];
    let is_python = program.starts_with("python") || program == "py";
    if is_python {
        if words.get(1).is_some_and(|w| w == "-m")
            && let Some(module) = words.get(2).filter(|w| is_key_word(w))
        {
            out.extend(["-m", module.as_str()]);
        }
        return out.join(" ");
    }
    if !SUBCOMMAND_PROGRAMS.contains(&program) {
        return out.join(" ");
    }
    let rest: Vec<&str> = words[1..].iter().map(String::as_str).collect();
    let mut at = 0;
    let Some(sub) = next_key_word(program, &rest, &mut at) else {
        return out.join(" ");
    };
    out.push(sub);
    if (RUNNERS.contains(&sub) && RUNNER_PROGRAMS.contains(&program))
        || GROUPS.contains(&(program, sub))
    {
        out.extend(next_key_word(program, &rest, &mut at));
    }
    out.join(" ")
}

/// The next word of `rest` from `*at` after options (and the values of
/// those in [`VALUED_OPTIONS`]), a `+toolchain` (`cargo +nightly test`) and
/// yarn's `workspace <name>`, if it is worth keeping; advances `*at` past
/// it.
fn next_key_word<'w>(program: &str, rest: &[&'w str], at: &mut usize) -> Option<&'w str> {
    while let Some(word) = rest.get(*at) {
        if word.starts_with('-') && word.len() > 1 {
            *at += if VALUED_OPTIONS.contains(word) { 2 } else { 1 };
        } else if word.starts_with('+') || (program == "yarn" && *word == "workspace") {
            *at += if *word == "workspace" { 2 } else { 1 };
        } else {
            break;
        }
    }
    let word = rest.get(*at)?;
    *at += 1;
    is_key_word(word).then_some(*word)
}

/// A word worth keeping in a command key: a subcommand-like name, never a
/// path, a file, an assignment, a number or quoted text.
fn is_key_word(word: &str) -> bool {
    word.len() <= 40
        && word.starts_with(|c: char| c.is_ascii_alphabetic())
        && !word.contains([
            '/', '=', '.', '\'', '"', '$', '*', '@', '[', ']', '{', '}', '<', '>', '(', ')', '`',
            ',', '~',
        ])
}

/// A word of a line: its unquoted text and where it ends in the line.
struct Word {
    text: String,
    end: usize,
}

/// Splits a command line into simple commands (see the module docs).
pub fn simple_commands(line: &str) -> Vec<SimpleCommand> {
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
                // A backslash-newline continues the line.
                match chars.next() {
                    Some((_, '\n')) | None => {}
                    Some((_, escaped)) => word.push(escaped),
                }
            }
            '#' if !in_word => {
                // A comment runs to the end of the line.
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
/// its assignments, keywords and wrappers; `None` when nothing is left or
/// it is not a command.
fn simple_command(line: &str, words: Vec<Word>, end: usize) -> Option<SimpleCommand> {
    let mut rest = words.into_iter().peekable();
    let mut loop_keyword = None;
    while let Some(word) = rest.peek() {
        match word.text.as_str() {
            "until" => loop_keyword = Some("until"),
            "while" => loop_keyword = Some("while"),
            w if is_assignment(w) || KEYWORD_PREFIXES.contains(&w) => {}
            _ => break,
        }
        rest.next();
    }
    loop {
        let first = rest.next()?;
        if NOT_COMMANDS.contains(&first.text.as_str()) {
            return None;
        }
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
            return Some(SimpleCommand {
                words,
                text,
                loop_keyword,
            });
        };
        // Skip the wrapper's options (and their values), plus `env`'s
        // assignments and `timeout`'s duration.
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
