//! The claudit error log (`$CLAUDIT_HOME/logs/claudit.log`).
//!
//! Components that must never surface errors to Claude Code (the hook, the
//! detached ingest) report failures here instead.

use std::io::Write;

use chrono::{SecondsFormat, Utc};

use crate::paths::Paths;
use crate::redact;
use crate::secure_fs;

/// Appends one line `<timestamp> ERROR [<component>] <message>` to the log.
/// Logging itself never fails loudly: if the log is unwritable, the error is
/// dropped, since there is nowhere left to report it.
pub fn error(paths: &Paths, component: &str, message: impl std::fmt::Display) {
    write(paths, "ERROR", component, message);
}

/// Appends one line `<timestamp> INFO [<component>] <message>`: a notable
/// event that is not a failure (e.g. a one-time rebuild after an upgrade).
pub fn info(paths: &Paths, component: &str, message: impl std::fmt::Display) {
    write(paths, "INFO", component, message);
}

fn write(paths: &Paths, level: &str, component: &str, message: impl std::fmt::Display) {
    let line = format!(
        "{} {level} [{component}] {}\n",
        Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        // Keep one entry per line even for multi-line error chains; error
        // messages can quote payload fragments, so they are redacted too.
        redact::redact_str(&message.to_string()).replace('\n', " | ")
    );
    if let Ok(mut file) = secure_fs::open_append(&paths.log_file()) {
        let _ = file.write_all(line.as_bytes());
    }
}
