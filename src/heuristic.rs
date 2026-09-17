use std::time::Duration;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptOutcome {
    SuccessHeuristic,
    FailedFatalDialog,
    FailedEarlyExit,
    FailedWerFault,
    UnknownTimeout,
    InternalError,
}

impl AttemptOutcome {
    pub fn is_failure(self) -> bool {
        matches!(
            self,
            Self::FailedFatalDialog
                | Self::FailedEarlyExit
                | Self::FailedWerFault
                | Self::InternalError
        )
    }
}

#[derive(Debug, Clone, Default)]
pub struct Observation {
    pub process_exists: bool,
    pub fatal_dialog: Option<String>,
    pub werfault: bool,
    pub visible_window: bool,
    pub window_hung: bool,
    pub process_cpu_percent: f32,
}

#[derive(Debug)]
pub struct AttemptTracker {
    success_after: Duration,
    hard_stop: Duration,
    responsive_since: Option<Duration>,
    saw_cpu_activity: bool,
    fatal_match: Option<String>,
}

impl AttemptTracker {
    pub fn new(success_after: Duration, hard_stop: Duration) -> Self {
        Self {
            success_after,
            hard_stop,
            responsive_since: None,
            saw_cpu_activity: false,
            fatal_match: None,
        }
    }

    pub fn fatal_match(&self) -> Option<&str> {
        self.fatal_match.as_deref()
    }

    pub fn observe(
        &mut self,
        elapsed: Duration,
        observation: &Observation,
    ) -> Option<AttemptOutcome> {
        if let Some(text) = &observation.fatal_dialog {
            self.fatal_match = Some(text.clone());
            return Some(AttemptOutcome::FailedFatalDialog);
        }
        if observation.werfault {
            return Some(AttemptOutcome::FailedWerFault);
        }
        if !observation.process_exists {
            return Some(if elapsed < self.success_after {
                AttemptOutcome::FailedEarlyExit
            } else {
                AttemptOutcome::UnknownTimeout
            });
        }

        self.saw_cpu_activity |= observation.process_cpu_percent > 0.01;
        if observation.visible_window && !observation.window_hung {
            self.responsive_since.get_or_insert(elapsed);
        } else {
            self.responsive_since = None;
        }

        let responsive_long_enough = self
            .responsive_since
            .is_some_and(|since| elapsed.saturating_sub(since) >= Duration::from_secs(10));
        if elapsed >= self.success_after && responsive_long_enough && self.saw_cpu_activity {
            return Some(AttemptOutcome::SuccessHeuristic);
        }
        if elapsed >= self.hard_stop {
            return Some(AttemptOutcome::UnknownTimeout);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn healthy() -> Observation {
        Observation {
            process_exists: true,
            visible_window: true,
            process_cpu_percent: 1.0,
            ..Observation::default()
        }
    }

    #[test]
    fn fatal_dialog_wins_immediately() {
        let mut tracker = AttemptTracker::new(Duration::from_secs(45), Duration::from_secs(60));
        let observation = Observation {
            process_exists: true,
            fatal_dialog: Some("Fatal Error in GC: SuspendThread Loop failed".into()),
            ..Observation::default()
        };
        assert_eq!(
            tracker.observe(Duration::from_secs(2), &observation),
            Some(AttemptOutcome::FailedFatalDialog)
        );
    }

    #[test]
    fn early_exit_is_failure() {
        let mut tracker = AttemptTracker::new(Duration::from_secs(45), Duration::from_secs(60));
        assert_eq!(
            tracker.observe(Duration::from_secs(20), &Observation::default()),
            Some(AttemptOutcome::FailedEarlyExit)
        );
    }

    #[test]
    fn success_needs_ten_responsive_seconds_and_cpu() {
        let mut tracker = AttemptTracker::new(Duration::from_secs(45), Duration::from_secs(60));
        assert_eq!(tracker.observe(Duration::from_secs(34), &healthy()), None);
        assert_eq!(
            tracker.observe(Duration::from_secs(45), &healthy()),
            Some(AttemptOutcome::SuccessHeuristic)
        );
    }

    #[test]
    fn hung_window_reaches_unknown_timeout() {
        let mut tracker = AttemptTracker::new(Duration::from_secs(40), Duration::from_secs(50));
        let observation = Observation {
            process_exists: true,
            visible_window: true,
            window_hung: true,
            process_cpu_percent: 1.0,
            ..Observation::default()
        };
        assert_eq!(
            tracker.observe(Duration::from_secs(50), &observation),
            Some(AttemptOutcome::UnknownTimeout)
        );
    }
}
