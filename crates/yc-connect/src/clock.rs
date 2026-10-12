//! Injectable time source so scheduling is testable with a fake clock.

use std::sync::{Arc, Mutex};

use chrono::{DateTime, Duration, Utc};

/// Source of the current time.
pub trait Clock: Send + Sync {
    /// Current instant.
    fn now(&self) -> DateTime<Utc>;
}

/// The wall clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

/// A clock that only moves when told to; clones share the same instant.
#[derive(Debug, Clone)]
pub struct ManualClock(Arc<Mutex<DateTime<Utc>>>);

impl ManualClock {
    /// Starts at `start`.
    pub fn new(start: DateTime<Utc>) -> Self {
        Self(Arc::new(Mutex::new(start)))
    }

    /// Jumps to `to`.
    pub fn set(&self, to: DateTime<Utc>) {
        if let Ok(mut now) = self.0.lock() {
            *now = to;
        }
    }

    /// Moves forward by `by`.
    pub fn advance(&self, by: Duration) {
        if let Ok(mut now) = self.0.lock() {
            *now += by;
        }
    }
}

impl Clock for ManualClock {
    fn now(&self) -> DateTime<Utc> {
        self.0.lock().map(|now| *now).unwrap_or_default()
    }
}
