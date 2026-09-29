//! Human-readable formatting shared by templates.

/// `850 ms`, `4.2 s`, `3 min 05 s`, `1 h 02 min`.
pub(super) fn duration_ms(ms: u64) -> String {
    match ms {
        0..1_000 => format!("{ms} ms"),
        1_000..60_000 => format!("{:.1} s", ms as f64 / 1_000.0),
        60_000..3_600_000 => format!("{} min {:02} s", ms / 60_000, (ms % 60_000) / 1_000),
        _ => format!("{} h {:02} min", ms / 3_600_000, (ms % 3_600_000) / 60_000),
    }
}

/// `950`, `12.3 k`, `4.56 M`, `1.20 B`.
pub(super) fn count(n: u64) -> String {
    match n {
        0..1_000 => n.to_string(),
        1_000..1_000_000 => format!("{:.1} k", n as f64 / 1e3),
        1_000_000..1_000_000_000 => format!("{:.2} M", n as f64 / 1e6),
        _ => format!("{:.2} B", n as f64 / 1e9),
    }
}

/// A ratio in `[0, 1]` as a percentage: `0 %`, `12.5 %`, `100 %`.
pub(super) fn percent(ratio: f64) -> String {
    let pct = ratio * 100.0;
    if pct == pct.round() {
        format!("{pct:.0} %")
    } else {
        format!("{pct:.1} %")
    }
}

/// Serializes `value` for embedding in a `<script type="application/json">`
/// element: `<` is escaped so data can never close the script tag.
pub(super) fn script_json<T: serde::Serialize>(value: &T) -> anyhow::Result<String> {
    Ok(serde_json::to_string(value)?.replace('<', "\\u003c"))
}
