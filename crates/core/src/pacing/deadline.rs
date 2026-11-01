//! Phase-locked software frame deadlines. The host supplies monotonic timestamps.
//!
//! This schedules frame *requests*, not GPU completion or display presentation. Integrate actual
//! vblank/presentation feedback separately. A late host skips obsolete deadlines instead of issuing
//! a burst of old frames, and the next deadline remains anchored to the original phase.

use crate::rate::{ceil_div, duration_from_nanos, Rate};
use std::{error::Error, fmt, time::Duration};

/// The host supplied an earlier timestamp than it supplied previously.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NonMonotonicTime;
impl fmt::Display for NonMonotonicTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("timestamp moved backwards")
    }
}
impl Error for NonMonotonicTime {}

/// One pacing decision. A due request does not imply that the GPU presented a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameDecision {
    /// Whether one new frame should be requested now.
    pub due: bool,
    /// Deadline of this request, or `None` when no request is due.
    pub scheduled_at: Option<Duration>,
    /// Earlier deadlines skipped since the last request; never request them retroactively.
    pub missed: u128,
    /// Next deadline, or `None` if it lies beyond representable `Duration`.
    pub next_deadline: Option<Duration>,
}

/// Phase-locked frame requests at a rational rate, independent of simulation ticks.
#[derive(Debug, Clone)]
pub struct DeadlinePacer {
    rate: Rate,
    epoch: Option<Duration>,
    next_index: u128,
    last_now: Option<Duration>,
}

impl DeadlinePacer {
    /// Create a pacer. Its first poll is due immediately and establishes the phase.
    pub fn new(rate: Rate) -> Self {
        Self {
            rate,
            epoch: None,
            next_index: 0,
            last_now: None,
        }
    }
    /// The current target rate.
    pub fn rate(&self) -> Rate {
        self.rate
    }
    /// Change target rate and rebase the phase on the next poll, which is immediately due.
    pub fn set_rate(&mut self, rate: Rate) {
        self.rate = rate;
        self.epoch = None;
        self.next_index = 0;
        // Preserve last_now so a rate change cannot hide a backwards timestamp.
    }
    /// Change target and start a new phase at `now` without requesting an immediate extra frame.
    pub fn rebase_rate(&mut self, rate: Rate, now: Duration) -> Result<(), NonMonotonicTime> {
        if self.last_now.is_some_and(|last| now < last) {
            return Err(NonMonotonicTime);
        }
        self.rate = rate;
        self.epoch = Some(now);
        self.next_index = 1;
        self.last_now = Some(now);
        Ok(())
    }
    /// Forget the phase and timestamp, for a host that intentionally changes its clock origin.
    pub fn reset(&mut self) {
        self.epoch = None;
        self.next_index = 0;
        self.last_now = None;
    }
    /// The next scheduled deadline after a successful poll.
    pub fn next_deadline(&self) -> Option<Duration> {
        let epoch = self.epoch?;
        let units = self.next_index.checked_mul(self.rate.period_units())?;
        let offset = duration_from_nanos(ceil_div(units, self.rate.scale()))?;
        epoch.checked_add(offset)
    }
    /// Poll using time measured from a consistent monotonic origin.
    ///
    /// A backwards timestamp returns an error without advancing state. Call `reset` only when the
    /// host intentionally changes origins. If several deadlines passed, one request is returned and
    /// all older deadlines are counted as missed; the next one stays aligned to the original phase.
    pub fn poll(&mut self, now: Duration) -> Result<FrameDecision, NonMonotonicTime> {
        if self.last_now.is_some_and(|last| now < last) {
            return Err(NonMonotonicTime);
        }
        self.last_now = Some(now);
        let Some(epoch) = self.epoch else {
            self.epoch = Some(now);
            self.next_index = 1;
            return Ok(FrameDecision {
                due: true,
                scheduled_at: Some(now),
                missed: 0,
                next_deadline: self.next_deadline(),
            });
        };
        let next = self.next_deadline();
        if next.is_none_or(|deadline| now < deadline) {
            return Ok(FrameDecision {
                due: false,
                scheduled_at: None,
                missed: 0,
                next_deadline: next,
            });
        }
        // The largest frame index whose ceil-rounded deadline is <= now.
        let elapsed = now.saturating_sub(epoch).as_nanos();
        let latest = elapsed * self.rate.scale() / self.rate.period_units();
        let missed = latest.saturating_sub(self.next_index);
        let scheduled_offset = duration_from_nanos(ceil_div(
            latest * self.rate.period_units(),
            self.rate.scale(),
        ))
        .expect("a deadline at or before now is representable");
        let scheduled_at = epoch
            .checked_add(scheduled_offset)
            .expect("deadline is at or before now");
        self.next_index = latest + 1;
        Ok(FrameDecision {
            due: true,
            scheduled_at: Some(scheduled_at),
            missed,
            next_deadline: self.next_deadline(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn deadlines_are_phase_locked_and_skip_old_frames() {
        let mut p = DeadlinePacer::new(Rate::new(60, 1).unwrap());
        assert!(p.poll(Duration::ZERO).unwrap().due);
        assert!(!p.poll(Duration::from_millis(16)).unwrap().due);
        let first = p.poll(Duration::from_nanos(16_666_667)).unwrap();
        assert!(first.due);
        assert_eq!(first.missed, 0);
        let late = p.poll(Duration::from_millis(65)).unwrap();
        assert_eq!(late.missed, 1); // the 33.3 ms deadline was skipped; 50 ms is requested
        assert_eq!(late.scheduled_at, Some(Duration::from_millis(50)));
        assert_eq!(late.next_deadline, Some(Duration::from_nanos(66_666_667)));
        assert!(!p.poll(Duration::from_millis(65)).unwrap().due);
    }
    #[test]
    fn fractional_rate_keeps_long_term_phase() {
        let mut p = DeadlinePacer::new(Rate::new(60_000, 1_001).unwrap());
        p.poll(Duration::ZERO).unwrap();
        let result = p.poll(Duration::from_secs(1_001)).unwrap();
        assert!(result.due);
        assert_eq!(result.scheduled_at, Some(Duration::from_secs(1_001)));
        assert_eq!(result.missed, 59_999);
        assert_eq!(
            result.next_deadline,
            Some(Duration::from_nanos(1_001_016_683_334))
        );
    }
    #[test]
    fn rejects_backwards_time_without_mutation_and_rebases_rate_changes() {
        let mut p = DeadlinePacer::new(Rate::new(60, 1).unwrap());
        p.poll(Duration::from_secs(1)).unwrap();
        assert!(p.poll(Duration::ZERO).is_err());
        assert_eq!(p.next_deadline(), Some(Duration::from_nanos(1_016_666_667)));
        p.set_rate(Rate::new(30, 1).unwrap());
        assert!(p.poll(Duration::ZERO).is_err());
        assert!(p.poll(Duration::from_secs(2)).unwrap().due);
        assert_eq!(p.next_deadline(), Some(Duration::from_nanos(2_033_333_334)));
    }
    #[test]
    fn representable_end_of_time_does_not_panic_or_spin() {
        let mut p = DeadlinePacer::new(Rate::new(60, 1).unwrap());
        let last = p.poll(Duration::MAX).unwrap();
        assert!(last.due);
        assert_eq!(last.next_deadline, None);
        assert!(!p.poll(Duration::MAX).unwrap().due);
    }
}
