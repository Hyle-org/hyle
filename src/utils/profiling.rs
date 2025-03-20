use std::time::{Duration, Instant};

use tracing::warn;

pub struct BranchTimer {
    start: Instant,
    threshold: Duration,
    location: &'static str,
    description: &'static str,
}

impl BranchTimer {
    pub fn new(threshold_ms: u64, location: &'static str, description: &'static str) -> Self {
        Self {
            start: Instant::now(),
            threshold: Duration::from_millis(threshold_ms),
            location,
            description,
        }
    }
}

impl Drop for BranchTimer {
    fn drop(&mut self) {
        let elapsed = self.start.elapsed();
        if elapsed > self.threshold {
            warn!(
                "Select branch '{}' at {} took {:?} (threshold: {:?})",
                self.description, self.location, elapsed, self.threshold
            );
        }
    }
}

macro_rules! time_branch {
    ($threshold_ms:expr, $description:expr) => {
        let _timer = $crate::utils::profiling::BranchTimer::new(
            $threshold_ms,
            concat!(file!(), ":", line!()),
            $description,
        );
    };
    ($threshold_ms:expr) => {
        let _timer = $crate::utils::profiling::BranchTimer::new(
            $threshold_ms,
            concat!(file!(), ":", line!()),
            "",
        );
    };
}
pub(crate) use time_branch;
