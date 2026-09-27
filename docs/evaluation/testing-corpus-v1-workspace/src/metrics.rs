//! Seed `untested-concurrency`: a counter shared across threads has no tests.

use std::sync::atomic::{AtomicU64, Ordering};

/// Monotonic counter safe to share between threads.
#[derive(Debug, Default)]
pub struct Counter {
    value: AtomicU64,
}

impl Counter {
    /// Adds `amount` and returns the new total.
    pub fn increment_by(&self, amount: u64) -> u64 {
        self.value.fetch_add(amount, Ordering::Relaxed) + amount
    }

    /// Current total.
    #[must_use]
    pub fn total(&self) -> u64 {
        self.value.load(Ordering::Relaxed)
    }
}
