//! Team comparison: the period reports of a team (`claudit report`), read
//! from a folder, lined up member by member to show where usage diverges.
//!
//! A team shares its reports in a folder (a synced drive, a repository, or
//! files dropped by hand); [`load_dir`] reads every report in it. Reports
//! are compared over one period at a time ([`periods`]): a member's latest
//! report of that exact period counts.
//!
//! Members differ in how much they use Claude Code, so [`compare`] puts
//! them side by side on rates (per turn, per active day, shares of time)
//! rather than on raw totals, against the team median. A value far from
//! the median ([`DEVIATION`]) is flagged: that is where practices differ,
//! the place to look at first.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::paths::Paths;
use crate::report::{self, Period, Report};
use crate::secure_fs;

/// How far from the team median a value is flagged: at least this many
/// times the median, or at most its inverse.
pub const DEVIATION: f64 = 1.5;

/// The folder a team shares its reports in, as the dashboard remembers it
/// (`$CLAUDIT_HOME/team.json`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamSettings {
    pub dir: Option<PathBuf>,
}

impl TeamSettings {
    /// The saved settings; none saved (or unreadable) is the default.
    pub fn load(paths: &Paths) -> Self {
        std::fs::read_to_string(paths.team_settings_file())
            .ok()
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, paths: &Paths) -> Result<()> {
        let path = paths.team_settings_file();
        secure_fs::create_dir_all(paths.home())?;
        let mut file = secure_fs::owner_only(
            std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true),
        )
        .open(&path)
        .with_context(|| format!("write {}", path.display()))?;
        serde_json::to_writer_pretty(&mut file, self)?;
        Ok(())
    }
}

/// The reports of a folder.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TeamReports {
    pub reports: Vec<Report>,
    /// JSON files that are not readable reports, with why.
    pub skipped: Vec<Skipped>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skipped {
    pub file: String,
    pub reason: String,
}

/// Every report among the `.json` files of `dir` (not its subfolders).
/// A file that is not a report, or one of a newer schema, is skipped and
/// listed with why; an unreadable folder is an error.
pub fn load_dir(dir: &Path) -> Result<TeamReports> {
    let entries =
        std::fs::read_dir(dir).with_context(|| format!("read the folder {}", dir.display()))?;
    let mut files: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
        })
        .collect();
    files.sort();
    let mut loaded = TeamReports::default();
    for path in files {
        let file = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        match std::fs::read_to_string(&path)
            .map_err(anyhow::Error::from)
            .and_then(|json| report::read(&json))
        {
            Ok(report) => loaded.reports.push(report),
            Err(err) => loaded.skipped.push(Skipped {
                file,
                reason: format!("{err:#}"),
            }),
        }
    }
    Ok(loaded)
}

/// A period some reports cover.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeriodChoice {
    pub period: Period,
    /// Distinct members with a report of it.
    pub members: usize,
}

/// The periods `reports` cover, the one most members reported first (then
/// the most recent).
pub fn periods(reports: &[Report]) -> Vec<PeriodChoice> {
    let mut members: BTreeMap<(chrono::NaiveDate, chrono::NaiveDate), BTreeSet<&str>> =
        BTreeMap::new();
    for r in reports {
        members
            .entry((r.period.from, r.period.to))
            .or_default()
            .insert(r.author.as_str());
    }
    let mut choices: Vec<PeriodChoice> = members
        .into_iter()
        .map(|((from, to), authors)| PeriodChoice {
            period: Period { from, to },
            members: authors.len(),
        })
        .collect();
    choices.sort_by(|a, b| {
        b.members
            .cmp(&a.members)
            .then(b.period.to.cmp(&a.period.to))
            .then(b.period.from.cmp(&a.period.from))
    });
    choices
}

/// How a metric reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    /// A count or a rate: `12`, `3.4`.
    Number,
    /// A ratio in `[0, 1]`.
    Share,
    /// Milliseconds.
    Duration,
    /// US dollars.
    Usd,
}

/// Where a value stands against the team median.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Deviation {
    /// At least [`DEVIATION`] times the median.
    Above,
    /// At most the median divided by [`DEVIATION`].
    Below,
    /// In between, or nothing to compare with.
    Typical,
}

/// One value per member, in [`Comparison::members`] order; `None` when the
/// member's report cannot tell (no timed turn for a time metric, say).
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub label: String,
    /// What it measures, for a tooltip.
    pub help: String,
    pub unit: Unit,
    pub values: Vec<Option<f64>>,
    /// Median of the known values.
    pub median: Option<f64>,
    pub deviations: Vec<Deviation>,
}

impl Row {
    fn new(label: &str, help: &str, unit: Unit, values: Vec<Option<f64>>) -> Self {
        let median = median(&values);
        let deviations = values.iter().map(|v| deviation(*v, median)).collect();
        Self {
            label: label.to_owned(),
            help: help.to_owned(),
            unit,
            values,
            median,
            deviations,
        }
    }

    /// Members with a positive value.
    pub fn users(&self) -> usize {
        self.values
            .iter()
            .filter(|v| v.is_some_and(|v| v > 0.0))
            .count()
    }
}

/// The team over one period.
#[derive(Debug, Clone, PartialEq)]
pub struct Comparison {
    pub period: Period,
    /// Authors, sorted.
    pub members: Vec<String>,
    /// Each member's report, in `members` order.
    pub reports: Vec<Report>,
    /// Volume, time, tools, tokens and cost, as rates.
    pub metrics: Vec<Row>,
    /// Skills: invocations (typed or by Claude) per 100 turns.
    pub skills: Vec<Row>,
    /// Subagent types: runs per 100 turns.
    pub subagents: Vec<Row>,
    /// Tools: calls per turn.
    pub tools: Vec<Row>,
    /// Activities: share of hook-timed tool time.
    pub activities: Vec<Row>,
    /// Models: share of the priced cost.
    pub models: Vec<Row>,
}

/// The members' reports of `period` side by side (see the module docs);
/// each member's latest report of it counts.
pub fn compare(reports: &[Report], period: Period) -> Comparison {
    let mut latest: BTreeMap<&str, &Report> = BTreeMap::new();
    for r in reports.iter().filter(|r| r.period == period) {
        let keep = latest
            .get(r.author.as_str())
            .is_none_or(|kept| r.generated_at > kept.generated_at);
        if keep {
            latest.insert(r.author.as_str(), r);
        }
    }
    let members: Vec<String> = latest.keys().map(|a| (*a).to_owned()).collect();
    let reports: Vec<Report> = latest.into_values().cloned().collect();

    let per = |n: f64, d: f64| (d > 0.0).then(|| n / d);
    let metric = |label: &str, help: &str, unit: Unit, f: &dyn Fn(&Report) -> Option<f64>| {
        Row::new(label, help, unit, reports.iter().map(f).collect())
    };
    let wall = |r: &Report| {
        let t = r.time;
        (t.model + t.tool + t.waiting + t.subagent + t.background) as f64
    };
    let share_of_time = |part: fn(&Report) -> u64| {
        move |r: &Report| {
            (r.totals.timed_turns > 0)
                .then(|| per(part(r) as f64, wall(r)))
                .flatten()
        }
    };
    let turns = |r: &Report| r.totals.turns as f64;
    let active_days = |r: &Report| r.by_day.len() as f64;

    let metrics = vec![
        metric(
            "Sessions",
            "Sessions started in the period",
            Unit::Number,
            &|r| Some(r.totals.sessions as f64),
        ),
        metric("Turns", "Prompts, typed or injected", Unit::Number, &|r| {
            Some(r.totals.turns as f64)
        }),
        metric(
            "Active days",
            "Days with a session or tokens",
            Unit::Number,
            &|r| Some(active_days(r)),
        ),
        metric(
            "Turns per active day",
            "Turns ÷ active days",
            Unit::Number,
            &|r| per(turns(r), active_days(r)),
        ),
        metric(
            "Time per turn",
            "Wall time of the timed turns ÷ timed turns (background included)",
            Unit::Duration,
            &|r| per(wall(r), r.totals.timed_turns as f64),
        ),
        metric(
            "Model",
            "Share of the timed time the model spent generating",
            Unit::Share,
            &share_of_time(|r| r.time.model),
        ),
        metric(
            "Tools",
            "Share of the timed time spent running tools",
            Unit::Share,
            &share_of_time(|r| r.time.tool),
        ),
        metric(
            "Waiting on you",
            "Share of the timed time spent on permission prompts",
            Unit::Share,
            &share_of_time(|r| r.time.waiting),
        ),
        metric(
            "Subagents",
            "Share of the timed time the main thread waited on subagents",
            Unit::Share,
            &share_of_time(|r| r.time.subagent),
        ),
        metric(
            "Background subagents",
            "Share of the timed time subagents ran between turns",
            Unit::Share,
            &share_of_time(|r| r.time.background),
        ),
        metric(
            "Waiting per turn",
            "Permission-prompt time ÷ timed turns",
            Unit::Duration,
            &|r| per(r.time.waiting as f64, r.totals.timed_turns as f64),
        ),
        metric(
            "Tool calls per turn",
            "Completed tool calls (subagents' included) ÷ turns",
            Unit::Number,
            &|r| per(r.totals.tool_calls as f64, turns(r)),
        ),
        metric(
            "Tool failure rate",
            "Failed tool calls ÷ tool calls",
            Unit::Share,
            &|r| per(r.totals.tool_failures as f64, r.totals.tool_calls as f64),
        ),
        metric(
            "Skills per 100 turns",
            "Skill invocations, typed or by Claude, per 100 turns",
            Unit::Number,
            &|r| {
                let n: u64 = r
                    .skills
                    .iter()
                    .map(|s| s.user_invocations + s.model_invocations)
                    .sum();
                per(n as f64 * 100.0, turns(r))
            },
        ),
        metric(
            "Subagent runs per 100 turns",
            "Subagent runs per 100 turns",
            Unit::Number,
            &|r| {
                let n: u64 = r.subagents.iter().map(|s| s.runs).sum();
                per(n as f64 * 100.0, turns(r))
            },
        ),
        metric(
            "Tokens per turn",
            "All tokens ÷ turns",
            Unit::Number,
            &|r| {
                let t = r.totals.tokens;
                per(
                    (t.input + t.output + t.cache_write + t.cache_read) as f64,
                    turns(r),
                )
            },
        ),
        metric(
            "Cache reads",
            "Share of input tokens read from the prompt cache",
            Unit::Share,
            &|r| {
                let t = r.totals.tokens;
                per(
                    t.cache_read as f64,
                    (t.input + t.cache_write + t.cache_read) as f64,
                )
            },
        ),
        metric(
            "Cost per turn",
            "API-equivalent cost ÷ turns",
            Unit::Usd,
            &|r| per(r.totals.cost.usd, turns(r)),
        ),
        metric(
            "Cost per active day",
            "API-equivalent cost ÷ active days",
            Unit::Usd,
            &|r| per(r.totals.cost.usd, active_days(r)),
        ),
        metric(
            "Cost",
            "API-equivalent cost over the period",
            Unit::Usd,
            &|r| Some(r.totals.cost.usd),
        ),
    ];

    let skills = items(&reports, Unit::Number, |r| {
        r.skills
            .iter()
            .map(|s| {
                let n = (s.user_invocations + s.model_invocations) as f64;
                (s.skill.clone(), per(n * 100.0, turns(r)))
            })
            .collect()
    });
    let subagents = items(&reports, Unit::Number, |r| {
        r.subagents
            .iter()
            .map(|s| (s.agent_type.clone(), per(s.runs as f64 * 100.0, turns(r))))
            .collect()
    });
    let tools = items(&reports, Unit::Number, |r| {
        r.tools
            .iter()
            .map(|t| (t.name.clone(), per(t.calls.calls as f64, turns(r))))
            .collect()
    });
    let activities = items(&reports, Unit::Share, |r| {
        let total: u64 = r.activities.iter().map(|a| a.calls.total_ms).sum();
        r.activities
            .iter()
            .map(|a| {
                (
                    a.activity.clone(),
                    per(a.calls.total_ms as f64, total as f64),
                )
            })
            .collect()
    });
    let models = items(&reports, Unit::Share, |r| {
        let total: f64 = r.models.iter().map(|m| m.cost.usd).sum();
        r.models
            .iter()
            .map(|m| {
                let name = if m.model.is_empty() {
                    "(no model)".to_owned()
                } else {
                    m.model.clone()
                };
                (name, per(m.cost.usd, total))
            })
            .collect()
    });

    Comparison {
        period,
        members,
        reports,
        metrics,
        skills,
        subagents,
        tools,
        activities,
        models,
    }
}

/// One row per item any member has, its value `0` for a member whose report
/// lacks it (that member does not use it), `None` when `values` cannot tell.
/// Most widely used first, then highest median, then name.
fn items(
    reports: &[Report],
    unit: Unit,
    values: impl Fn(&Report) -> Vec<(String, Option<f64>)>,
) -> Vec<Row> {
    let per_member: Vec<BTreeMap<String, Option<f64>>> = reports
        .iter()
        .map(|r| values(r).into_iter().collect())
        .collect();
    let names: BTreeSet<&String> = per_member.iter().flat_map(|m| m.keys()).collect();
    let mut rows: Vec<Row> = names
        .into_iter()
        .map(|name| {
            let values = per_member
                .iter()
                .map(|m| m.get(name).copied().unwrap_or(Some(0.0)))
                .collect();
            Row::new(name, "", unit, values)
        })
        .collect();
    rows.sort_by(|a, b| {
        b.users()
            .cmp(&a.users())
            .then(b.median.unwrap_or(0.0).total_cmp(&a.median.unwrap_or(0.0)))
            .then_with(|| a.label.cmp(&b.label))
    });
    rows
}

/// The median of the known values (the mean of the middle two for an even
/// count).
fn median(values: &[Option<f64>]) -> Option<f64> {
    let mut known: Vec<f64> = values.iter().flatten().copied().collect();
    if known.is_empty() {
        return None;
    }
    known.sort_by(f64::total_cmp);
    let mid = known.len() / 2;
    Some(if known.len() % 2 == 0 {
        (known[mid - 1] + known[mid]) / 2.0
    } else {
        known[mid]
    })
}

fn deviation(value: Option<f64>, median: Option<f64>) -> Deviation {
    match (value, median) {
        (Some(v), Some(m)) if m > 0.0 => {
            if v >= m * DEVIATION {
                Deviation::Above
            } else if v <= m / DEVIATION {
                Deviation::Below
            } else {
                Deviation::Typical
            }
        }
        // Everyone else at zero: any use stands out.
        (Some(v), Some(m)) if m == 0.0 && v > 0.0 => Deviation::Above,
        _ => Deviation::Typical,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_median_and_deviations() {
        assert_eq!(median(&[Some(3.0), None, Some(1.0), Some(2.0)]), Some(2.0));
        assert_eq!(median(&[Some(1.0), Some(4.0)]), Some(2.5));
        assert_eq!(median(&[None]), None);

        assert_eq!(deviation(Some(3.0), Some(2.0)), Deviation::Above);
        assert_eq!(deviation(Some(1.0), Some(2.0)), Deviation::Below);
        assert_eq!(deviation(Some(2.5), Some(2.0)), Deviation::Typical);
        assert_eq!(deviation(Some(1.0), Some(0.0)), Deviation::Above);
        assert_eq!(deviation(Some(0.0), Some(0.0)), Deviation::Typical);
        assert_eq!(deviation(None, Some(2.0)), Deviation::Typical);
    }
}
