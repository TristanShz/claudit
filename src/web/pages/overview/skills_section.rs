//! The skills and subagents ranked lists (overview).

use crate::stats::skills::SkillStat;
use crate::stats::subagents::SubagentTypeStat;
use crate::web::format;

/// Rows shown per list.
const TOP: usize = 8;

/// A row of the skills list.
pub(in crate::web) struct SkillRow {
    pub name: String,
    /// Who triggered it: `you`, `Claude` or `you + Claude` (empty when only
    /// tokens are attributed to it in range).
    pub pill: &'static str,
    /// CSS modifier for the pill: `user`, `model` or `both`.
    pub pill_kind: &'static str,
    pub invocations: u64,
    pub tokens: String,
    pub time: String,
}

/// A row of the subagents list.
pub(in crate::web) struct SubagentRow {
    pub agent_type: String,
    pub runs: u64,
    pub time: String,
    pub model: String,
    pub tokens: String,
}

pub(in crate::web) fn skill_rows(stats: Vec<SkillStat>) -> Vec<SkillRow> {
    stats
        .into_iter()
        .take(TOP)
        .map(|s| {
            let (pill, pill_kind) = match (s.user_invocations > 0, s.model_invocations > 0) {
                (true, true) => ("you + Claude", "both"),
                (true, false) => ("you", "user"),
                (false, true) => ("Claude", "model"),
                (false, false) => ("", ""),
            };
            SkillRow {
                invocations: s.invocations(),
                tokens: format::count(s.tokens.total()),
                time: format::duration_ms(s.attributed_time.num_milliseconds().max(0) as u64),
                name: s.skill,
                pill,
                pill_kind,
            }
        })
        .collect()
}

pub(in crate::web) fn subagent_rows(stats: Vec<SubagentTypeStat>) -> Vec<SubagentRow> {
    stats
        .into_iter()
        .take(TOP)
        .map(|s| SubagentRow {
            runs: s.runs,
            time: format::duration_ms(s.total_duration.num_milliseconds().max(0) as u64),
            model: s.model.unwrap_or_default(),
            tokens: format::count(s.tokens.total()),
            agent_type: s.agent_type,
        })
        .collect()
}
