use std::time::{Duration, Instant, SystemTime};

pub(crate) struct LocalObservation {
    monotonic: Instant,
    wall: SystemTime,
}

impl LocalObservation {
    pub(crate) fn now() -> Self {
        Self {
            monotonic: Instant::now(),
            wall: SystemTime::now(),
        }
    }

    #[cfg(test)]
    pub(crate) fn aged_for_test(age: Duration) -> Self {
        Self {
            monotonic: Instant::now(),
            wall: SystemTime::now() - age,
        }
    }

    pub(crate) fn elapsed(&self) -> Duration {
        // A suspended Mac may stop its uptime clock. Wall-clock rollback is
        // uncertain and must not make an old process observation look fresh.
        self.monotonic
            .elapsed()
            .max(self.wall.elapsed().unwrap_or(Duration::MAX))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suspension_and_clock_rollback_do_not_refresh_observations() {
        let suspended = LocalObservation {
            monotonic: Instant::now(),
            wall: SystemTime::now() - Duration::from_secs(16),
        };
        assert!(suspended.elapsed() >= Duration::from_secs(15));
        let rolled_back = LocalObservation {
            monotonic: Instant::now(),
            wall: SystemTime::now() + Duration::from_secs(16),
        };
        assert_eq!(rolled_back.elapsed(), Duration::MAX);
    }
}
