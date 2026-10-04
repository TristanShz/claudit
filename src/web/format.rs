//! Human-readable formatting shared by templates.

/// `850 ms`, `4.2 s`, `3 min 05 s`, `1 h 02 min`.
pub(crate) fn duration_ms(ms: u64) -> String {
    match ms {
        0..1_000 => format!("{ms} ms"),
        1_000..60_000 => format!("{:.1} s", ms as f64 / 1_000.0),
        60_000..3_600_000 => format!("{} min {:02} s", ms / 60_000, (ms % 60_000) / 1_000),
        _ => format!("{} h {:02} min", ms / 3_600_000, (ms % 3_600_000) / 60_000),
    }
}

/// `950`, `12.3 k`, `4.56 M`, `1.20 B`.
pub(crate) fn count(n: u64) -> String {
    match n {
        0..1_000 => n.to_string(),
        1_000..1_000_000 => format!("{:.1} k", n as f64 / 1e3),
        1_000_000..1_000_000_000 => format!("{:.2} M", n as f64 / 1e6),
        _ => format!("{:.2} B", n as f64 / 1e9),
    }
}

/// A ratio in `[0, 1]` as a percentage: `0 %`, `12.5 %`, `100 %`.
pub(crate) fn percent(ratio: f64) -> String {
    let pct = ratio * 100.0;
    if pct == pct.round() {
        format!("{pct:.0} %")
    } else {
        format!("{pct:.1} %")
    }
}

/// Serializes `value` for embedding in a `<script type="application/json">`
/// element: `<` is escaped so data can never close the script tag.
pub(crate) fn script_json<T: serde::Serialize>(value: &T) -> anyhow::Result<String> {
    Ok(serde_json::to_string(value)?.replace('<', "\\u003c"))
}

/// A chrono duration, as [`duration_ms`] (negative durations count as 0).
pub(crate) fn duration(d: chrono::Duration) -> String {
    duration_ms(d.num_milliseconds().max(0) as u64)
}

/// An instant in the machine's time zone: `2026-03-02 09:00`.
pub(crate) fn local_time(at: chrono::DateTime<chrono::Utc>) -> String {
    at.with_timezone(&chrono::Local)
        .format("%Y-%m-%d %H:%M")
        .to_string()
}

/// An instant in the machine's time zone, to the second:
/// `2026-03-02 09:00:05`.
pub(crate) fn local_time_s(at: chrono::DateTime<chrono::Utc>) -> String {
    at.with_timezone(&chrono::Local)
        .format("%Y-%m-%d %H:%M:%S")
        .to_string()
}

/// A cost: `$12.34`, `$12.34+` when some models are unpriced, `unknown`
/// when none is (never `$0` for unpriced tokens).
pub(crate) fn cost(cost: &crate::pricing::Cost) -> String {
    if cost.is_complete() {
        cost.known.to_string()
    } else if cost.known.picos() == 0 {
        "unknown".to_owned()
    } else {
        format!("{}+", cost.known)
    }
}

/// `text` on one line (whitespace runs collapsed), cut to at most `max`
/// characters with an ellipsis when cut.
pub(crate) fn truncate(text: &str, max: usize) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let text = text.as_str();
    match text.char_indices().nth(max) {
        Some((at, _)) => format!("{}…", text[..at].trim_end()),
        None => text.to_owned(),
    }
}
