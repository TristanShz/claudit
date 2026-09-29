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

/// Serializes `value` for embedding in a `<script type="application/json">`
/// element: `<` is escaped so data can never close the script tag.
pub(super) fn script_json<T: serde::Serialize>(value: &T) -> anyhow::Result<String> {
    Ok(serde_json::to_string(value)?.replace('<', "\\u003c"))
}
