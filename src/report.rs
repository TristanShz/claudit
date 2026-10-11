//! Period report: one person's Claude Code usage over a period, as a
//! versioned JSON document to share with a team and compare with the
//! others' (`claudit report`).
//!
//! A report holds aggregates only: totals, where the time went, models,
//! tools, MCP servers, Bash commands, activities, skills, subagents, per
//! project and per day. The prompts the user typed are added only on
//! request ([`ReportOptions::include_prompts`]); like the archive, a report
//! never holds Claude's responses nor tool outputs.
//!
//! Reports are meant to be compared across machines, so they never hold a
//! local path: a project is named by its git `origin` remote
//! (`github.com/acme/api`), else by its directory's name (see
//! [`git_project_name`]).
//!
//! The document is [`Report`]. Its layout is versioned by
//! [`SCHEMA_VERSION`]; a change that breaks a reader bumps it.
//!
//! Durations are in milliseconds, costs in US dollars at API list prices,
//! days are UTC days.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use anyhow::{Result, bail};
use chrono::{DateTime, Duration, NaiveDate, TimeZone, Utc};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::activities::ActivityRules;
use crate::pricing::{self, PriceTable};
use crate::stats::commands::{self, CommandSort};
use crate::stats::consumption::{self, TokenTotals};
use crate::stats::time::{self, TimeSplit};
use crate::stats::tools::{self, CallStats, RankedCalls};
use crate::stats::{Filter, activities, cost, models, prompt, sessions, skills, subagents};

/// [`Report::format`]: tells a report from any other JSON file.
pub const FORMAT: &str = "claudit-report";

/// [`Report::schema_version`] of the reports this claudit writes.
pub const SCHEMA_VERSION: u32 = 1;

/// The project of a session whose working directory is unknown.
pub const UNKNOWN_PROJECT: &str = "(unknown)";

/// Days a report covers when neither `from` nor `days` is given.
pub const DEFAULT_DAYS: u32 = 30;

/// A period report (see the module docs).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Report {
    /// Always [`FORMAT`].
    pub format: String,
    pub schema_version: u32,
    /// The claudit that wrote it.
    pub claudit_version: String,
    pub generated_at: DateTime<Utc>,
    /// Whose usage it is.
    pub author: String,
    pub period: Period,
    /// The filters beyond the period, if any.
    pub filter: ReportFilter,
    pub coverage: Coverage,
    pub totals: Totals,
    /// Where the time of the timed turns went.
    pub time: TimeMs,
    /// Days with activity, oldest first.
    pub by_day: Vec<Day>,
    /// Most sessions first.
    pub projects: Vec<Project>,
    /// Most expensive first.
    pub models: Vec<Model>,
    /// Most calls first.
    pub tools: Vec<NamedCalls>,
    /// Most calls first.
    pub mcp_servers: Vec<NamedCalls>,
    /// Bash commands by command key, most time first.
    pub commands: Vec<Command>,
    /// Most time first.
    pub activities: Vec<Activity>,
    /// Most invocations first.
    pub skills: Vec<Skill>,
    /// Most runs first.
    pub subagents: Vec<Subagent>,
    /// The prompts the user typed, oldest first; absent unless asked for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompts: Option<Vec<Prompt>>,
}

/// The days a report covers, both included (UTC).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Period {
    pub from: NaiveDate,
    pub to: NaiveDate,
}

impl Period {
    /// The period from `from` (else `days` days, else [`DEFAULT_DAYS`],
    /// before `to`) to `to` (else `today`).
    pub fn resolve(
        from: Option<NaiveDate>,
        to: Option<NaiveDate>,
        days: Option<u32>,
        today: NaiveDate,
    ) -> Result<Self> {
        let to = to.unwrap_or(today);
        let from = match (from, days) {
            (Some(_), Some(_)) => bail!("give either a start day or a number of days, not both"),
            (Some(from), None) => from,
            (None, days) => {
                let days = days.unwrap_or(DEFAULT_DAYS);
                if days == 0 {
                    bail!("a report covers at least one day");
                }
                to - Duration::days(i64::from(days) - 1)
            }
        };
        if from > to {
            bail!("the period starts ({from}) after it ends ({to})");
        }
        Ok(Self { from, to })
    }

    /// Its first instant and the instant after its last day.
    fn bounds(self) -> (DateTime<Utc>, DateTime<Utc>) {
        let start = |day: NaiveDate| Utc.from_utc_datetime(&day.and_hms_opt(0, 0, 0).unwrap());
        (start(self.from), start(self.to) + Duration::days(1))
    }
}

/// [`Report::filter`]. The project is named as in [`Report::projects`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportFilter {
    pub project: Option<String>,
    pub branch: Option<String>,
    pub model: Option<String>,
}

/// Which sessions the time figures cover (see
/// [`crate::stats::time::TimeCoverage`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Coverage {
    /// Sessions the hooks recorded: their turns are timed.
    pub recorded_sessions: u64,
    /// Sessions known only from their transcripts: counted, not timed.
    pub imported_sessions: u64,
    /// When the hooks started recording on this machine.
    pub hooks_since: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Totals {
    pub sessions: u64,
    pub turns: u64,
    /// Turns with a start and an end recorded by the hooks: [`Report::time`]
    /// is measured on these.
    pub timed_turns: u64,
    /// Completed tool calls, subagents' included.
    pub tool_calls: u64,
    pub tool_failures: u64,
    pub tokens: Tokens,
    pub cost: Cost,
}

/// Token counts by class.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tokens {
    /// Uncached input tokens.
    pub input: u64,
    pub output: u64,
    pub cache_write: u64,
    pub cache_read: u64,
}

impl From<TokenTotals> for Tokens {
    fn from(t: TokenTotals) -> Self {
        Self {
            input: t.input,
            output: t.output,
            cache_write: t.cache_write,
            cache_read: t.cache_read,
        }
    }
}

/// An API-equivalent cost.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Cost {
    /// The cost of the tokens of the priced models.
    pub usd: f64,
    /// Models without a price: their tokens are not in `usd`.
    pub unpriced_models: Vec<String>,
}

impl From<&pricing::Cost> for Cost {
    fn from(c: &pricing::Cost) -> Self {
        Self {
            usd: c.known.dollars(),
            unpriced_models: c.unknown_models.clone(),
        }
    }
}

/// A wall time split by what it was spent on, in milliseconds (see
/// `ARCHITECTURE.md`, "Time decomposition").
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeMs {
    /// The model generating.
    pub model: u64,
    /// Main-thread tools executing.
    pub tool: u64,
    /// Waiting on the user (permission prompts).
    pub waiting: u64,
    /// The main thread waiting on a subagent.
    pub subagent: u64,
    /// Subagents running between turns, the main thread idle.
    pub background: u64,
}

impl From<TimeSplit> for TimeMs {
    fn from(s: TimeSplit) -> Self {
        Self {
            model: ms(s.model),
            tool: ms(s.tool),
            waiting: ms(s.waiting),
            subagent: ms(s.subagent),
            background: ms(s.background),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Day {
    pub day: NaiveDate,
    /// Sessions started that day.
    pub sessions: u64,
    /// Timed turns started that day.
    pub timed_turns: u64,
    pub time: TimeMs,
    pub tokens: Tokens,
    pub cost: Cost,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Project {
    /// See the module docs.
    pub name: String,
    pub sessions: u64,
    pub turns: u64,
    pub tool_calls: u64,
    pub time: TimeMs,
    pub tokens: Tokens,
    pub cost: Cost,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Model {
    /// As the API reports it (empty when a response had none).
    pub model: String,
    pub sessions: u64,
    /// API responses, subagents' included.
    pub api_messages: u64,
    /// Of which in subagent threads.
    pub subagent_api_messages: u64,
    pub tokens: Tokens,
    pub cost: Cost,
}

/// Calls aggregated (see [`crate::stats::tools::CallStats`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Calls {
    pub calls: u64,
    pub failures: u64,
    /// Calls the hooks timed: the durations are measured on these.
    pub timed_calls: u64,
    pub total_ms: u64,
    pub median_ms: Option<u64>,
    pub p95_ms: Option<u64>,
}

impl From<&CallStats> for Calls {
    fn from(s: &CallStats) -> Self {
        Self {
            calls: s.calls,
            failures: s.failures,
            timed_calls: s.timed_calls,
            total_ms: s.total_duration_ms,
            median_ms: s.median_duration_ms,
            p95_ms: s.p95_duration_ms,
        }
    }
}

/// A tool or an MCP server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NamedCalls {
    pub name: String,
    #[serde(flatten)]
    pub calls: Calls,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Command {
    /// The command key, e.g. `pnpm exec vitest`.
    pub command: String,
    pub activity: String,
    #[serde(flatten)]
    pub calls: Calls,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Activity {
    pub activity: String,
    #[serde(flatten)]
    pub calls: Calls,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Skill {
    pub skill: String,
    /// Typed by the user (`/skill`).
    pub user_invocations: u64,
    /// Called by Claude (the Skill tool).
    pub model_invocations: u64,
    /// Time of the turns that used it (see `stats::skills`).
    pub attributed_ms: u64,
    pub tokens: Tokens,
    pub cost: Cost,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Subagent {
    pub agent_type: String,
    pub runs: u64,
    /// Summed active time of the runs.
    pub total_ms: u64,
    pub tool_calls: u64,
    /// The model most runs used.
    pub model: Option<String>,
    pub tokens: Tokens,
    pub cost: Cost,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Prompt {
    pub at: DateTime<Utc>,
    pub project: String,
    /// As typed (secrets redacted); a command as `/name args`.
    pub text: String,
}

/// What a report covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportOptions {
    pub author: String,
    pub period: Period,
    /// A project working directory (as the dashboard's filter).
    pub project: Option<String>,
    pub branch: Option<String>,
    pub model: Option<String>,
    pub include_prompts: bool,
}

/// The report of `options` over the archive. `project_name` names a
/// project from a session's working directory ([`git_project_name`]
/// outside tests); it is called once per directory.
pub fn build(
    conn: &Connection,
    options: &ReportOptions,
    rules: &ActivityRules,
    prices: &PriceTable,
    now: DateTime<Utc>,
    project_name: &dyn Fn(&str) -> String,
) -> Result<Report> {
    if options.author.trim().is_empty() {
        bail!("a report needs an author");
    }
    let (from, to) = options.period.bounds();
    let filter = Filter {
        from: Some(from),
        to: Some(to),
        project: options.project.clone(),
        branch: options.branch.clone(),
        model: options.model.clone(),
    };
    let mut names: HashMap<String, String> = HashMap::new();
    let mut name_of = |cwd: Option<&str>| -> String {
        let Some(cwd) = cwd.filter(|c| !c.is_empty()) else {
            return UNKNOWN_PROJECT.to_owned();
        };
        names
            .entry(cwd.to_owned())
            .or_insert_with(|| project_name(cwd))
            .clone()
    };

    let usage = consumption::consumption(conn, &filter)?;
    let coverage = time::time_coverage(conn, &filter)?;
    let turn_times = time::turn_times(conn, &filter)?;
    let tool_ranking = tools::tool_ranking(conn, &filter)?;
    let session_list = sessions::session_list(conn, &filter)?;
    let session_costs: HashMap<String, pricing::Cost> =
        cost::cost_by_session(conn, &filter, prices)?
            .into_iter()
            .map(|line| (line.key, line.cost))
            .collect();

    let mut total_time = TimeSplit::default();
    for turn in &turn_times {
        total_time += turn.split;
    }
    let by_day = day_rows(
        &turn_times,
        &session_list,
        &cost::daily_series(conn, &filter, prices)?,
    );

    let mut projects: BTreeMap<String, (Project, pricing::Cost)> = BTreeMap::new();
    for session in &session_list {
        let name = name_of(session.cwd.as_deref());
        let (project, cost) = projects.entry(name.clone()).or_insert_with(|| {
            (
                Project {
                    name,
                    sessions: 0,
                    turns: 0,
                    tool_calls: 0,
                    time: TimeMs::default(),
                    tokens: Tokens::default(),
                    cost: Cost::default(),
                },
                pricing::Cost::default(),
            )
        });
        project.sessions += 1;
        project.turns += session.turns;
        project.tool_calls += session.tool_calls;
        add_time(&mut project.time, session.time);
        add_tokens(&mut project.tokens, session.tokens);
        if let Some(c) = session_costs.get(&session.session_id) {
            cost.add(c);
        }
    }
    let mut projects: Vec<Project> = projects
        .into_values()
        .map(|(mut project, cost)| {
            project.cost = Cost::from(&cost);
            project
        })
        .collect();
    projects.sort_by(|a, b| {
        b.sessions
            .cmp(&a.sessions)
            .then_with(|| b.cost.usd.total_cmp(&a.cost.usd))
            .then_with(|| a.name.cmp(&b.name))
    });

    let models_report = models::model_usage(conn, &filter, prices)?;
    let models = models_report
        .models
        .iter()
        .map(|m| Model {
            model: m.model.clone(),
            sessions: m.sessions,
            api_messages: m.api_messages,
            subagent_api_messages: m.subagents.api_messages,
            tokens: m.tokens.into(),
            cost: Cost::from(&m.cost),
        })
        .collect();

    let mut command_ranking = commands::command_ranking(conn, &filter, rules, CommandSort::Total)?;
    command_ranking.sort(CommandSort::Total);
    let commands = command_ranking
        .commands
        .iter()
        .map(|c| Command {
            command: c.command.clone(),
            activity: c.activity.clone(),
            calls: Calls::from(&c.stats),
        })
        .collect();

    let activities = activities::activity_breakdown(conn, &filter, rules)?
        .activities
        .iter()
        .map(|a| Activity {
            activity: a.activity.clone(),
            calls: Calls::from(&a.stats),
        })
        .collect();

    let skill_costs = cost_map(cost::cost_by_skill(conn, &filter, prices)?);
    let skills = skills::skill_ranking(conn, &filter)?
        .into_iter()
        .map(|s| Skill {
            cost: skill_costs.get(&s.skill).cloned().unwrap_or_default(),
            user_invocations: s.user_invocations,
            model_invocations: s.model_invocations,
            attributed_ms: ms(s.attributed_time),
            tokens: s.tokens.into(),
            skill: s.skill,
        })
        .collect();

    let agent_costs = cost_map(cost::cost_by_agent_type(conn, &filter, prices)?);
    let subagents = subagents::subagent_ranking(conn, &filter)?
        .into_iter()
        .map(|s| Subagent {
            cost: agent_costs.get(&s.agent_type).cloned().unwrap_or_default(),
            runs: s.runs,
            total_ms: ms(s.total_duration),
            tool_calls: s.tool_calls,
            model: s.model,
            tokens: s.tokens.into(),
            agent_type: s.agent_type,
        })
        .collect();

    let prompts = if options.include_prompts {
        Some(
            prompt::typed_prompts(conn, &filter)?
                .into_iter()
                .map(|p| Prompt {
                    at: p.at,
                    project: name_of(p.cwd.as_deref()),
                    text: p.text,
                })
                .collect(),
        )
    } else {
        None
    };

    let project_filter = options.project.as_deref().map(|p| name_of(Some(p)));
    Ok(Report {
        format: FORMAT.to_owned(),
        schema_version: SCHEMA_VERSION,
        claudit_version: env!("CARGO_PKG_VERSION").to_owned(),
        generated_at: now,
        author: options.author.trim().to_owned(),
        period: options.period,
        filter: ReportFilter {
            project: project_filter,
            branch: options.branch.clone(),
            model: options.model.clone(),
        },
        coverage: Coverage {
            recorded_sessions: coverage.recorded_sessions,
            imported_sessions: coverage.imported_sessions,
            hooks_since: coverage.hooks_since,
        },
        totals: Totals {
            sessions: usage.sessions,
            turns: usage.turns,
            timed_turns: turn_times.len() as u64,
            tool_calls: tool_ranking.iter().map(|t| t.stats.calls).sum(),
            tool_failures: tool_ranking.iter().map(|t| t.stats.failures).sum(),
            tokens: usage.tokens.into(),
            cost: Cost::from(&models_report.total_cost),
        },
        time: total_time.into(),
        by_day,
        projects,
        models,
        tools: named_calls(&tool_ranking),
        mcp_servers: named_calls(&tools::mcp_server_ranking(conn, &filter)?),
        commands,
        activities,
        skills,
        subagents,
        prompts,
    })
}

/// One row per day with a session, a timed turn or tokens, oldest first.
fn day_rows(
    turns: &[time::TurnTime],
    sessions: &[sessions::SessionSummary],
    usage: &[cost::DailyUsage],
) -> Vec<Day> {
    type Acc = (Day, TimeSplit, pricing::Cost);
    fn day(days: &mut BTreeMap<NaiveDate, Acc>, date: NaiveDate) -> &mut Acc {
        days.entry(date).or_insert_with(|| {
            (
                Day {
                    day: date,
                    sessions: 0,
                    timed_turns: 0,
                    time: TimeMs::default(),
                    tokens: Tokens::default(),
                    cost: Cost::default(),
                },
                TimeSplit::default(),
                pricing::Cost::default(),
            )
        })
    }
    let mut days: BTreeMap<NaiveDate, Acc> = BTreeMap::new();
    for turn in turns {
        let (row, split, _) = day(&mut days, turn.start.date_naive());
        row.timed_turns += 1;
        *split += turn.split;
    }
    for session in sessions {
        day(&mut days, session.started_at.date_naive()).0.sessions += 1;
    }
    for line in usage {
        let (row, _, cost) = day(&mut days, line.day);
        add_tokens(&mut row.tokens, line.tokens);
        cost.add(&line.cost);
    }
    days.into_values()
        .map(|(mut row, split, cost)| {
            row.time = split.into();
            row.cost = Cost::from(&cost);
            row
        })
        .collect()
}

fn named_calls(ranking: &[RankedCalls]) -> Vec<NamedCalls> {
    ranking
        .iter()
        .map(|r| NamedCalls {
            name: r.name.clone(),
            calls: Calls::from(&r.stats),
        })
        .collect()
}

fn cost_map(lines: Vec<cost::CostLine>) -> HashMap<String, Cost> {
    lines
        .into_iter()
        .map(|line| (line.key, Cost::from(&line.cost)))
        .collect()
}

fn add_tokens(into: &mut Tokens, more: TokenTotals) {
    into.input += more.input;
    into.output += more.output;
    into.cache_write += more.cache_write;
    into.cache_read += more.cache_read;
}

fn add_time(into: &mut TimeMs, more: TimeSplit) {
    let more = TimeMs::from(more);
    into.model += more.model;
    into.tool += more.tool;
    into.waiting += more.waiting;
    into.subagent += more.subagent;
    into.background += more.background;
}

fn ms(d: Duration) -> u64 {
    d.num_milliseconds().max(0) as u64
}

/// The name of the project in `cwd`: its git `origin` remote, normalized
/// ([`normalize_remote`]), else the directory's name. Runs `git`. A
/// directory that is gone (a removed worktree, say) is looked up from its
/// nearest existing parent, so `<repo>/.claude/worktrees/<name>` still
/// names the repository; a directory not in a repository, or in one
/// without an `origin`, is named by its last path component.
pub fn git_project_name(cwd: &str) -> String {
    use std::process::{Command, Stdio};
    let existing = Path::new(cwd).ancestors().find(|dir| dir.is_dir());
    existing
        .and_then(|dir| {
            Command::new("git")
                .arg("-C")
                .arg(dir)
                .args(["remote", "get-url", "origin"])
                .stdin(Stdio::null())
                .stderr(Stdio::null())
                .output()
                .ok()
        })
        .filter(|out| out.status.success())
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .and_then(|url| normalize_remote(&url))
        .unwrap_or_else(|| dir_name(cwd))
}

/// `git config user.name`, when set: a report's default author.
pub fn git_user_name() -> Option<String> {
    use std::process::{Command, Stdio};
    let out = Command::new("git")
        .args(["config", "user.name"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let name = String::from_utf8(out.stdout).ok()?;
    let name = name.trim();
    (out.status.success() && !name.is_empty()).then(|| name.to_owned())
}

/// A git remote URL as `host/path`, without scheme, credentials, port nor
/// `.git`: `git@github.com:acme/api.git` and
/// `https://token@github.com/acme/api` are both `github.com/acme/api`.
/// `None` for a local remote (a path, `file://`).
pub fn normalize_remote(url: &str) -> Option<String> {
    let url = url.trim().trim_end_matches('/');
    let url = url.strip_suffix(".git").unwrap_or(url);
    let (authority, path) = match url.split_once("://") {
        Some((_, rest)) => rest.split_once('/')?,
        // scp-like `[user@]host:path`; a Windows path (`C:\…`) or a path
        // with a colon further on is not one.
        None => {
            let (authority, path) = url.split_once(':')?;
            if authority.len() <= 1 || authority.contains(['/', '\\']) {
                return None;
            }
            (authority, path)
        }
    };
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let host = host.split(':').next().unwrap_or_default().to_lowercase();
    let path = path.trim_matches('/');
    if host.is_empty() || path.is_empty() {
        return None;
    }
    Some(format!("{host}/{path}"))
}

/// The last component of `path` (`/` or `\` separated).
fn dir_name(path: &str) -> String {
    path.trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or(UNKNOWN_PROJECT)
        .to_owned()
}

/// The default file name of a report: `claudit-report-<author>-<from>_<to>.json`,
/// the author reduced to safe characters.
pub fn file_name(report: &Report) -> String {
    let mut author = String::new();
    for c in report.author.chars() {
        if c.is_alphanumeric() {
            author.extend(c.to_lowercase());
        } else if !author.ends_with('-') {
            author.push('-');
        }
    }
    let author = author.trim_matches('-');
    format!(
        "claudit-report-{}-{}_{}.json",
        if author.is_empty() {
            "anonymous"
        } else {
            author
        },
        report.period.from,
        report.period.to
    )
}

/// Writes `report` as pretty JSON to `output`: that file, or, when it is an
/// existing directory, [`file_name`] in it. Returns the path written.
pub fn write(report: &Report, output: &Path) -> Result<std::path::PathBuf> {
    let path = if output.is_dir() {
        output.join(file_name(report))
    } else {
        output.to_owned()
    };
    let mut json = serde_json::to_string_pretty(report)?;
    json.push('\n');
    std::fs::write(&path, json)?;
    Ok(path)
}

/// Reads a report, refusing any other JSON and a newer schema.
pub fn read(json: &str) -> Result<Report> {
    #[derive(Deserialize)]
    struct Header {
        format: Option<String>,
        schema_version: Option<u32>,
    }
    let header: Header = serde_json::from_str(json)?;
    if header.format.as_deref() != Some(FORMAT) {
        bail!("not a claudit report");
    }
    match header.schema_version {
        Some(v) if v > SCHEMA_VERSION => bail!(
            "report schema {v} is newer than this claudit reads ({SCHEMA_VERSION}); run `claudit update`"
        ),
        Some(_) => Ok(serde_json::from_str(json)?),
        None => bail!("not a claudit report: no schema version"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remotes_are_normalized_to_host_and_path() {
        for (url, want) in [
            ("git@github.com:acme/api.git", Some("github.com/acme/api")),
            (
                "https://github.com/acme/api.git\n",
                Some("github.com/acme/api"),
            ),
            (
                // Fake credentials, split so secret scanners don't flag the test.
                concat!("https://x-token:", "s3cr3t", "@GitHub.com/acme/api/"),
                Some("github.com/acme/api"),
            ),
            (
                "ssh://git@gitlab.example.com:2222/team/sub/app.git",
                Some("gitlab.example.com/team/sub/app"),
            ),
            ("/srv/git/api.git", None),
            ("file:///srv/git/api.git", None),
            ("C:\\repos\\api", None),
        ] {
            assert_eq!(normalize_remote(url).as_deref(), want, "{url}");
        }
    }

    #[test]
    fn a_directory_is_named_by_its_last_component() {
        assert_eq!(dir_name("/Users/alice/code/acme-api"), "acme-api");
        assert_eq!(dir_name("/Users/alice/code/acme-api/"), "acme-api");
        assert_eq!(dir_name("C:\\code\\acme-api"), "acme-api");
        assert_eq!(dir_name("/"), UNKNOWN_PROJECT);
    }

    #[test]
    fn a_period_defaults_to_the_last_thirty_days() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 11).unwrap();
        let day = |s: &str| NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap();

        let p = Period::resolve(None, None, None, today).unwrap();
        assert_eq!((p.from, p.to), (day("2026-09-12"), today));
        let p = Period::resolve(None, Some(day("2026-09-30")), Some(7), today).unwrap();
        assert_eq!((p.from, p.to), (day("2026-09-24"), day("2026-09-30")));
        let p = Period::resolve(Some(day("2026-09-01")), None, None, today).unwrap();
        assert_eq!((p.from, p.to), (day("2026-09-01"), today));

        assert!(Period::resolve(Some(day("2026-10-12")), None, None, today).is_err());
        assert!(Period::resolve(Some(day("2026-09-01")), None, Some(3), today).is_err());
        assert!(Period::resolve(None, None, Some(0), today).is_err());
    }
}
