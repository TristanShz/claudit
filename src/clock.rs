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

/// Inverse of [`to_micros`].
pub fn from_micros(us: i64) -> DateTime<Utc> {
    DateTime::from_timestamp_micros(us).unwrap_or_default()
}
