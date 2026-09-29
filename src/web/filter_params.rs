//! The global filter bar's query string (`?from=&to=&project=&branch=&model=`),
//! shared by every page.

use chrono::{NaiveDate, TimeZone, Utc};
use serde::Deserialize;

use crate::stats::Filter;

/// Raw query parameters; empty strings mean "no filter".
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub(super) struct FilterParams {
    /// First day included (`YYYY-MM-DD`, UTC).
    pub from: String,
    /// Last day included (`YYYY-MM-DD`, UTC).
    pub to: String,
    pub project: String,
    pub branch: String,
    pub model: String,
}

impl FilterParams {
    pub fn to_filter(&self) -> Filter {
        let day_start = |day: &str| {
            NaiveDate::parse_from_str(day.trim(), "%Y-%m-%d")
                .ok()
                .and_then(|d| d.and_hms_opt(0, 0, 0))
                .map(|dt| Utc.from_utc_datetime(&dt))
        };
        let non_empty = |s: &str| Some(s.trim().to_owned()).filter(|s| !s.is_empty());
        Filter {
            from: day_start(&self.from),
            to: day_start(&self.to).map(|d| d + chrono::Duration::days(1)),
            project: non_empty(&self.project),
            branch: non_empty(&self.branch),
            model: non_empty(&self.model),
        }
    }
}
