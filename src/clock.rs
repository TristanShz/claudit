//! Injectable time source. The hook stamps every payload with a
//! high-resolution receive time; tests substitute a manual clock.

use std::sync::Mutex;

use chrono::{DateTime, Duration, Utc};

/// A source of the current time.
pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}

/// The real wall clock (nanosecond resolution where the OS provides it).
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

/// A clock that only moves when told to. Used by tests.
#[derive(Debug)]
pub struct ManualClock {
    now: Mutex<DateTime<Utc>>,
}

impl ManualClock {
    pub fn new(start: DateTime<Utc>) -> Self {
        Self {
            now: Mutex::new(start),
        }
    }

    pub fn set(&self, at: DateTime<Utc>) {
        *self.now.lock().expect("clock lock") = at;
    }

    pub fn advance(&self, by: Duration) {
        *self.now.lock().expect("clock lock") += by;
    }
}

impl Clock for ManualClock {
    fn now(&self) -> DateTime<Utc> {
        *self.now.lock().expect("clock lock")
    }
}

/// Microseconds since the Unix epoch: the storage format of every timestamp
/// in the database (sortable, and cheap to do interval arithmetic on).
pub fn to_micros(at: DateTime<Utc>) -> i64 {
    at.timestamp_micros()
}

/// When a call started executing, in µs: its completion time `post_us`
/// minus the execution time Claude Code reports (`duration_ms`, which
/// excludes permission prompts; a negative one counts as 0). The same rule
/// in SQL is `stats::tools::EXECUTION_START`.
pub fn execution_start_us(post_us: i64, duration_ms: i64) -> i64 {
    post_us - duration_ms.max(0) * 1000
}

/// Inverse of [`to_micros`].
pub fn from_micros(us: i64) -> DateTime<Utc> {
    DateTime::from_timestamp_micros(us).unwrap_or_default()
}
