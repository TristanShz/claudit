//! The leading command of a Bash tool call: the program the command line is
//! really about (`git`, `cargo`, `pnpm`, …).
//!
//! The rule, applied to a light shell tokenization (quotes and backslash
//! escapes respected, `# comments` dropped, no expansion):
//!
//! 1. The line is split into simple commands at `&&`, `||`, `;`, `|`, `&`,
//!    newlines and subshell parentheses; they are considered in order.
//! 2. In a simple command, leading `NAME=value` assignments and the `{` / `!`
//!    keywords are skipped, then transparent wrappers with their options
//!    (`sudo`, `env` and its assignments, `time`, `nohup`, `nice`,
//!    `timeout <duration>`, `command`, `exec`).
//! 3. The first remaining word, reduced to its file name
//!    (`/usr/bin/git` → `git`), is the command.
//! 4. Directory changes (`cd`, `pushd`, `popd`) only set up what follows, so
//!    they are skipped in favour of the next simple command
//!    (`cd web && pnpm build` → `pnpm`); a line that only changes directory
//!    counts as `cd`.
//!
//! A line with no command at all (empty, only assignments) has none.

/// Programs that only run another command, and whose options take a value.
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

/// Commands that only change directory before the real work.
const DIRECTORY_CHANGES: [&str; 3] = ["cd", "pushd", "popd"];

/// The leading command of `command_line` (see the module docs for the rule).
pub(crate) fn leading_command(command_line: &str) -> Option<String> {
    let mut directory_change = None;
    for words in simple_commands(command_line) {
        let Some(program) = program_of(&words) else {
            continue;
        };
        if DIRECTORY_CHANGES.contains(&program.as_str()) {
            directory_change.get_or_insert(program);
            continue;
        }
        return Some(program);
    }
    directory_change
}

/// The program of one simple command, skipping assignments and wrappers.
fn program_of(words: &[String]) -> Option<String> {
    let mut rest = words.iter().map(String::as_str).peekable();
    while let Some(&word) = rest.peek() {
        if is_assignment(word) || word == "{" || word == "!" {
            rest.next();
        } else {
            break;
        }
    }
    loop {
        let word = rest.next()?;
        let program = word.rsplit('/').next().unwrap_or(word);
        let Some((wrapper, valued_options)) = WRAPPERS.iter().find(|(name, _)| *name == program)
        else {
            return (!program.is_empty()).then(|| program.to_owned());
        };
        // Skip the wrapper's options (and their values), plus `env`'s
        // assignments and `timeout`'s duration.
        while let Some(&next) = rest.peek() {
            if next.starts_with('-') && next.len() > 1 {
                rest.next();
                if valued_options.contains(&next) {
                    rest.next();
                }
            } else if *wrapper == "env" && is_assignment(next) {
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

/// Splits a command line into simple commands, each a list of words with
/// quotes removed.
fn simple_commands(line: &str) -> Vec<Vec<String>> {
    let mut commands = Vec::new();
    let mut words: Vec<String> = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut chars = line.chars().peekable();

    let end_word = |word: &mut String, in_word: &mut bool, words: &mut Vec<String>| {
        if *in_word {
            words.push(std::mem::take(word));
            *in_word = false;
        }
    };

    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                in_word = true;
                for q in chars.by_ref() {
                    if q == '\'' {
                        break;
                    }
                    word.push(q);
                }
            }
            '"' => {
                in_word = true;
                while let Some(q) = chars.next() {
                    match q {
                        '"' => break,
                        '\\' => word.extend(chars.next()),
                        _ => word.push(q),
                    }
                }
            }
            '\\' => {
                in_word = true;
                // A backslash-newline continues the line.
                match chars.next() {
                    Some('\n') | None => {}
                    Some(escaped) => word.push(escaped),
                }
            }
            '#' if !in_word => {
                // A comment runs to the end of the line.
                for rest in chars.by_ref() {
                    if rest == '\n' {
                        break;
                    }
                }
                end_word(&mut word, &mut in_word, &mut words);
                commands.push(std::mem::take(&mut words));
            }
            // Redirections `2>&1`, `>&2` and `&>file` are not separators.
            '&' if (in_word && word.ends_with('>')) || chars.peek() == Some(&'>') => {
                in_word = true;
                word.push(c);
            }
            ';' | '&' | '|' | '\n' | '(' | ')' => {
                end_word(&mut word, &mut in_word, &mut words);
                commands.push(std::mem::take(&mut words));
            }
            c if c.is_whitespace() => end_word(&mut word, &mut in_word, &mut words),
            c => {
                in_word = true;
                word.push(c);
            }
        }
    }
    end_word(&mut word, &mut in_word, &mut words);
    commands.push(words);
    commands.retain(|words| !words.is_empty());
    commands
}
