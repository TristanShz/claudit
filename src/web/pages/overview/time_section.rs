//! "Where the time goes" and "Waiting on you, by tool" (overview).

use serde::Serialize;

use crate::stats::time::{SegmentKind, TimeBreakdown, ToolWaiting};
use crate::web::format;

/// Both cards of the time row.
pub(in crate::web) struct TimeSection {
    /// False when no turn has both a start and an end yet.
    pub has_data: bool,
    /// Wall time over the filtered turns, formatted.
    pub total: String,
    pub parts: Vec<TimePart>,
    /// `TimeChart` as script JSON, shared by the split bar and the daily
    /// columns.
    pub chart_json: String,
    pub waiting: Vec<WaitingRow>,
}

/// One component of the split, for the legend.
pub(in crate::web) struct TimePart {
    /// `model`, `tool`, `waiting` or `subagent` (CSS hook).
    pub kind: &'static str,
    pub label: &'static str,
    pub value: String,
    /// e.g. `42%`.
    pub share: String,
}

/// A row of "Waiting on you, by tool".
pub(in crate::web) struct WaitingRow {
    pub name: String,
    pub waiting: String,
    pub calls_waited: u64,
    pub permission_requests: u64,
}

#[derive(Serialize)]
struct TimeChart {
    split: Vec<ChartPart>,
    days: Vec<ChartDay>,
}

#[derive(Serialize)]
struct ChartPart {
    kind: &'static str,
    label: &'static str,
    ms: i64,
}

#[derive(Serialize)]
struct ChartDay {
    day: String,
    model: i64,
    tool: i64,
    waiting: i64,
    subagent: i64,
}

impl TimeSection {
    pub fn build(breakdown: TimeBreakdown, waiting: Vec<ToolWaiting>) -> anyhow::Result<Self> {
        let wall = breakdown.total.wall();
        let wall_ms = wall.num_milliseconds();
        let parts = SegmentKind::ALL
            .into_iter()
            .map(|kind| {
                let d = breakdown.total.component(kind);
                TimePart {
                    kind: kind.name(),
                    label: kind.label(),
                    value: format::duration(d),
                    share: if wall_ms > 0 {
                        format!(
                            "{:.0}%",
                            d.num_milliseconds() as f64 * 100.0 / wall_ms as f64
                        )
                    } else {
                        String::new()
                    },
                }
            })
            .collect();
        let chart = TimeChart {
            split: SegmentKind::ALL
                .into_iter()
                .map(|kind| ChartPart {
                    kind: kind.name(),
                    label: kind.label(),
                    ms: breakdown.total.component(kind).num_milliseconds(),
                })
                .collect(),
            days: breakdown
                .by_day
                .iter()
                .map(|d| ChartDay {
                    day: d.day.format("%Y-%m-%d").to_string(),
                    model: d.split.model.num_milliseconds(),
                    tool: d.split.tool.num_milliseconds(),
                    waiting: d.split.waiting.num_milliseconds(),
                    subagent: d.split.subagent.num_milliseconds(),
                })
                .collect(),
        };
        Ok(Self {
            has_data: breakdown.turns > 0,
            total: format::duration(wall),
            parts,
            chart_json: format::script_json(&chart)?,
            waiting: waiting
                .into_iter()
                .map(|w| WaitingRow {
                    waiting: format::duration(w.waiting),
                    name: w.tool_name,
                    calls_waited: w.calls_waited,
                    permission_requests: w.permission_requests,
                })
                .collect(),
        })
    }
}
