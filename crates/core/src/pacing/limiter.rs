//! Optional strict minimum spacing between frame requests on a monotonic clock.
use super::NonMonotonicTime;
use crate::rate::Rate;
use std::time::Duration;

// Round upwards so a capped request never precedes its exact rational period.
fn minimum_period(rate: Rate) -> Duration {
    let units = 1_000_000_000_u64 * u64::from(rate.denominator());
    Duration::from_nanos(units.div_ceil(u64::from(rate.numerator())))
}

/// One frame-limit decision and, when capped, the next time a request may be made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FramePermit {
    pub due: bool,
    pub next_deadline: Option<Duration>,
}

/// A host-driven limiter. `None` means uncapped; this never controls display presentation itself.
/// Missed time is not replayed: each accepted request starts a new minimum interval.
#[derive(Debug, Clone, Default)]
pub struct FrameLimiter {
    cap: Option<Rate>,
    last_observed: Option<Duration>,
    last_granted: Option<Duration>,
}
impl FrameLimiter {
    pub fn new(cap: Option<Rate>) -> Self {
        Self {
            cap,
            ..Default::default()
        }
    }
    pub fn cap(&self) -> Option<Rate> {
        self.cap
    }
    /// Change the cap while retaining the last granted time, preventing a burst on transitions.
    pub fn set_cap(&mut self, cap: Option<Rate>) {
        self.cap = cap;
    }
    /// Forget previous timing, for example after changing the clock origin.
    pub fn reset(&mut self) {
        self.last_observed = None;
        self.last_granted = None;
    }
    /// Whether a capped deadline cannot be represented by `Duration`.
    pub fn is_exhausted(&self) -> bool {
        self.cap
            .zip(self.last_granted)
            .is_some_and(|(rate, last)| last.checked_add(minimum_period(rate)).is_none())
    }
    /// Next permitted request, or `None` when uncapped, not started, or exhausted.
    pub fn next_deadline(&self) -> Option<Duration> {
        self.cap.and_then(|rate| {
            self.last_granted
                .and_then(|last| last.checked_add(minimum_period(rate)))
        })
    }
    /// Poll before requesting a frame. A backwards timestamp leaves all state unchanged.
    pub fn poll(&mut self, now: Duration) -> Result<FramePermit, NonMonotonicTime> {
        if self.last_observed.is_some_and(|last| now < last) {
            return Err(NonMonotonicTime);
        }
        let due = match (self.cap, self.last_granted) {
            (Some(rate), Some(last)) => last
                .checked_add(minimum_period(rate))
                .is_some_and(|next| now >= next),
            _ => true,
        };
        self.last_observed = Some(now);
        if due {
            self.last_granted = Some(now);
        }
        let next_deadline = self.next_deadline();
        Ok(FramePermit { due, next_deadline })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn caps_without_catchup_bursts_and_can_be_disabled() {
        assert_eq!(
            minimum_period(Rate::new(30, 1).unwrap()),
            Duration::from_nanos(33_333_334)
        );
        let mut exhausted = FrameLimiter::new(Some(Rate::new(60, 1).unwrap()));
        assert!(exhausted.poll(Duration::MAX).unwrap().due);
        assert!(exhausted.is_exhausted());
        assert!(!exhausted.poll(Duration::MAX).unwrap().due);
        let mut fractional = FrameLimiter::new(Some(Rate::new(30, 1).unwrap()));
        assert!(fractional.poll(Duration::ZERO).unwrap().due);
        assert!(
            !fractional
                .poll(Duration::from_nanos(33_333_333))
                .unwrap()
                .due
        );
        assert!(
            fractional
                .poll(Duration::from_nanos(33_333_334))
                .unwrap()
                .due
        );
        let mut limiter = FrameLimiter::new(Some(Rate::new(60, 1).unwrap()));
        assert!(limiter.poll(Duration::ZERO).unwrap().due);
        assert!(!limiter.poll(Duration::from_millis(16)).unwrap().due);
        assert!(limiter.poll(Duration::from_millis(100)).unwrap().due);
        assert!(!limiter.poll(Duration::from_millis(101)).unwrap().due);
        assert!(limiter.poll(Duration::from_millis(99)).is_err());
        limiter.set_cap(None);
        assert!(limiter.poll(Duration::from_millis(101)).unwrap().due);
        assert!(limiter.poll(Duration::from_millis(101)).unwrap().due);
        limiter.set_cap(Some(Rate::new(30, 1).unwrap()));
        assert!(!limiter.poll(Duration::from_millis(102)).unwrap().due);
        assert!(limiter.poll(Duration::from_millis(135)).unwrap().due);
    }
}
