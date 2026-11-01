//! Compose fixed simulation ticks with phase-locked software frame requests.
//!
//! The host supplies timestamps from one monotonic origin and performs the returned work in order:
//! run simulation ticks first, then request a frame using the interpolation fraction. This module
//! does not create threads, sleep, touch a window, or claim a frame was presented by a GPU.

use crate::{
    pacing::{DeadlinePacer, FrameDecision, NonMonotonicTime},
    rate::Rate,
    time::{FixedClock, FrameStep},
};
use std::time::Duration;

/// Work due from a single scheduler poll.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScheduleStep {
    /// Actual elapsed host time since the previous poll, or zero for the first poll.
    pub elapsed: Duration,
    /// Fixed simulation work and the interpolation fraction after that work.
    pub simulation: FrameStep,
    /// Sequence number of the first due tick. Meaningful only when `simulation.ticks > 0`.
    pub first_tick: u128,
    /// Total simulation ticks issued since construction.
    pub total_ticks: u128,
    /// Software frame request decision; this is not presentation feedback.
    pub frame: FrameDecision,
    /// Earliest next tick or frame deadline, for a host that can sleep until then.
    pub next_wake: Option<Duration>,
}

/// Host-driven simulation and render-request schedule.
#[derive(Debug, Clone)]
pub struct Scheduler {
    simulation: FixedClock,
    frames: DeadlinePacer,
    last: Option<Duration>,
    tick_index: u128,
    paused: bool,
    skip_elapsed: bool,
}

impl Scheduler {
    /// Construct from a configured fixed-step simulation clock and a target frame-request rate.
    pub fn new(simulation: FixedClock, frame_rate: Rate) -> Self {
        Self {
            simulation,
            frames: DeadlinePacer::new(frame_rate),
            last: None,
            tick_index: 0,
            paused: false,
            skip_elapsed: false,
        }
    }
    /// Fixed simulation tick in seconds, for systems such as physics.
    pub fn tick_seconds(&self) -> f64 {
        self.simulation.step_seconds()
    }
    /// Whether simulation is paused. Frame requests may continue while paused.
    pub fn is_paused(&self) -> bool {
        self.paused
    }
    /// Pause or resume simulation. Time between this transition and the next poll is discarded.
    ///
    /// Fractional simulation time is cleared on each transition. This avoids simulating time spent
    /// paused; a caller that needs a final tick before pausing should poll immediately beforehand.
    pub fn set_paused(&mut self, paused: bool) {
        if self.paused != paused {
            self.paused = paused;
            self.skip_elapsed = true;
            self.simulation.reset();
        }
    }
    /// Change the frame target and rebase its phase without issuing an immediate extra frame.
    /// The timestamp must use the same origin and must not precede the previous poll.
    pub fn set_frame_rate(&mut self, rate: Rate, now: Duration) -> Result<(), NonMonotonicTime> {
        if self.last.is_some_and(|last| now < last) {
            return Err(NonMonotonicTime);
        }
        self.frames.rebase_rate(rate, now)
    }
    /// Poll with a monotonic timestamp. Backwards timestamps return an error and change no state.
    pub fn poll(&mut self, now: Duration) -> Result<ScheduleStep, NonMonotonicTime> {
        let elapsed = match self.last {
            Some(last) => now.checked_sub(last).ok_or(NonMonotonicTime)?,
            None => Duration::ZERO,
        };
        // This is the only fallible operation after validating `now`; do it before changing clock state.
        let frame = self.frames.poll(now)?;
        let simulation = if self.paused || self.skip_elapsed {
            FrameStep {
                ticks: 0,
                alpha: 0.0,
                dropped: Duration::ZERO,
            }
        } else {
            self.simulation.advance(elapsed)
        };
        self.skip_elapsed = false;
        self.last = Some(now);
        let first_tick = self.tick_index.saturating_add(1);
        self.tick_index = self.tick_index.saturating_add(u128::from(simulation.ticks));
        let next_tick = if self.paused {
            None
        } else {
            now.checked_add(self.simulation.time_until_tick())
        };
        let next_wake = match (next_tick, frame.next_deadline) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        };
        Ok(ScheduleStep {
            elapsed,
            simulation,
            first_tick,
            total_ticks: self.tick_index,
            frame,
            next_wake,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn scheduler() -> Scheduler {
        Scheduler::new(
            FixedClock::new(60.0, Duration::from_millis(250), 4).unwrap(),
            Rate::new(30, 1).unwrap(),
        )
    }
    #[test]
    fn independent_simulation_and_frame_rates_have_ordered_tick_ids() {
        let mut s = scheduler();
        let initial = s.poll(Duration::ZERO).unwrap();
        assert!(initial.frame.due);
        assert_eq!(initial.simulation.ticks, 0);
        assert_eq!(initial.next_wake, Some(Duration::from_nanos(16_666_667)));
        let tick = s.poll(Duration::from_nanos(16_666_667)).unwrap();
        assert_eq!(
            (tick.first_tick, tick.total_ticks, tick.simulation.ticks),
            (1, 1, 1)
        );
        assert!(!tick.frame.due);
        let next = s.poll(Duration::from_nanos(33_333_334)).unwrap();
        assert_eq!(
            (next.first_tick, next.total_ticks, next.simulation.ticks),
            (2, 2, 1)
        );
        assert!(next.frame.due);
    }
    #[test]
    fn long_stall_bounds_ticks_and_does_not_burst_frames() {
        let mut s = scheduler();
        s.poll(Duration::ZERO).unwrap();
        let late = s.poll(Duration::from_secs(2)).unwrap();
        assert_eq!(late.simulation.ticks, 4);
        assert!(late.simulation.dropped > Duration::from_secs(1));
        assert_eq!(late.frame.missed, 59);
        assert!(late.frame.due);
        assert!(!s.poll(Duration::from_secs(2)).unwrap().frame.due);
    }
    #[test]
    fn pause_and_resume_drop_paused_time_but_continue_frame_requests() {
        let mut s = scheduler();
        s.poll(Duration::ZERO).unwrap();
        s.set_paused(true);
        let paused = s.poll(Duration::from_secs(10)).unwrap();
        assert_eq!(paused.simulation.ticks, 0);
        assert!(paused.frame.due);
        s.set_paused(false);
        assert_eq!(s.poll(Duration::from_secs(20)).unwrap().simulation.ticks, 0);
        assert_eq!(
            s.poll(Duration::from_nanos(20_016_666_667))
                .unwrap()
                .simulation
                .ticks,
            1
        );
    }
    #[test]
    fn backward_timestamp_does_not_consume_time() {
        let mut s = scheduler();
        s.poll(Duration::from_secs(1)).unwrap();
        assert!(s.poll(Duration::ZERO).is_err());
        assert_eq!(
            s.poll(Duration::from_nanos(1_016_666_667))
                .unwrap()
                .simulation
                .ticks,
            1
        );
    }
}
