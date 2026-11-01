//! Fixed-step simulation time independent of rendering and the platform clock.
//!
//! Supply elapsed monotonic time to [`FixedClock::advance`]. The host runs the returned number of
//! ticks, each at [`FixedClock::step_seconds`], and can interpolate using the returned fraction.
//! Long stalls are limited explicitly so an overloaded application cannot enter an endless catch-up loop.

use crate::rate::{ceil_div, duration_from_nanos, Rate};
use std::time::Duration;

/// Simulation work to perform for one host update.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameStep {
    /// Number of fixed simulation ticks to run.
    pub ticks: u32,
    /// Fraction of a tick remaining after simulation, always in `[0, 1)`.
    pub alpha: f32,
    /// Wall time discarded to bound catch-up after a stall, rounded down to whole nanoseconds.
    pub dropped: Duration,
}

/// Accumulates host time with an exact rational rate and bounded catch-up work.
#[derive(Debug, Clone)]
pub struct FixedClock {
    rate: Rate,
    remainder: u128,
    max_frame: Duration,
    max_ticks: u32,
}

impl FixedClock {
    /// Create a clock from a frequency. Use [`Self::with_rate`] for an exact fractional rate.
    pub fn new(hz: f64, max_frame: Duration, max_ticks: u32) -> Result<Self, &'static str> {
        Self::with_rate(Rate::from_hz(hz)?, max_frame, max_ticks)
    }

    /// Create a clock using an exact rational frequency and nonzero catch-up limits.
    pub fn with_rate(
        rate: Rate,
        max_frame: Duration,
        max_ticks: u32,
    ) -> Result<Self, &'static str> {
        if max_ticks == 0 || max_frame.is_zero() {
            return Err("clock requires nonzero frame and tick limits");
        }
        Ok(Self {
            rate,
            remainder: 0,
            max_frame,
            max_ticks,
        })
    }

    /// Simulation frequency.
    pub fn rate(&self) -> Rate {
        self.rate
    }
    /// One tick rounded to the closest nanosecond; useful for APIs accepting `Duration`.
    pub fn step(&self) -> Duration {
        self.rate.period()
    }
    /// One tick in seconds, without first rounding it to an integer number of nanoseconds.
    pub fn step_seconds(&self) -> f64 {
        self.rate.period_seconds()
    }
    /// Time until the next tick, rounded up to the next representable nanosecond.
    pub fn time_until_tick(&self) -> Duration {
        duration_from_nanos(ceil_div(
            self.rate.period_units() - self.remainder,
            self.rate.scale(),
        ))
        .expect("validated rate has representable period")
    }

    /// Add elapsed time. Work is computed in constant time even for extremely large stalls.
    ///
    /// At most `max_frame` is accepted from this update and at most `max_ticks` are returned.
    /// Whole ticks above the limit are discarded; the fractional remainder is retained. If total
    /// discarded time cannot fit in `Duration`, the reported amount saturates at `Duration::MAX`.
    pub fn advance(&mut self, elapsed: Duration) -> FrameStep {
        let accepted = elapsed.min(self.max_frame);
        let scale = self.rate.scale();
        let period = self.rate.period_units();
        // Duration's largest nanosecond count multiplied by a u32 rate fits in u128.
        let units = self.remainder + accepted.as_nanos() * scale;
        let available = units / period;
        let ticks = available.min(u128::from(self.max_ticks)) as u32;
        self.remainder = units % period;
        let discarded_ticks = available - u128::from(ticks);
        let dropped_ns =
            elapsed.saturating_sub(accepted).as_nanos() + discarded_ticks * period / scale;
        let dropped = duration_from_nanos(dropped_ns).unwrap_or(Duration::MAX);
        let alpha = ((self.remainder as f64 / period as f64) as f32)
            .min(f32::from_bits(1.0f32.to_bits() - 1));
        FrameStep {
            ticks,
            alpha,
            dropped,
        }
    }

    /// Clear fractional time, for example when explicitly pausing or resetting the simulation.
    pub fn reset(&mut self) {
        self.remainder = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn accumulates_partial_ticks_and_interpolates() {
        let mut clock = FixedClock::new(10.0, Duration::from_secs(1), 8).unwrap();
        let first = clock.advance(Duration::from_millis(150));
        assert_eq!(first.ticks, 1);
        assert!((first.alpha - 0.5).abs() < 1e-6);
        assert_eq!(clock.advance(Duration::from_millis(50)).ticks, 1);
    }
    #[test]
    fn bounds_catchup_and_reports_discarded_time() {
        let mut clock = FixedClock::new(10.0, Duration::from_millis(550), 2).unwrap();
        let frame = clock.advance(Duration::from_secs(1));
        assert_eq!(frame.ticks, 2);
        assert!((frame.alpha - 0.5).abs() < 1e-6);
        assert_eq!(frame.dropped, Duration::from_millis(750));
    }
    #[test]
    fn exact_rational_clock_does_not_lose_ticks_to_rounded_periods() {
        let mut clock = FixedClock::with_rate(
            Rate::new(60, 1).unwrap(),
            Duration::from_secs(86_400),
            u32::MAX,
        )
        .unwrap();
        let day = clock.advance(Duration::from_secs(86_400));
        assert_eq!(day.ticks, 5_184_000);
        assert_eq!(day.alpha, 0.0);
        let mut fractional = FixedClock::with_rate(
            Rate::new(60_000, 1_001).unwrap(),
            Duration::from_secs(1_001),
            u32::MAX,
        )
        .unwrap();
        assert_eq!(fractional.advance(Duration::from_secs(1_001)).ticks, 60_000);
    }
    #[test]
    fn interpolation_never_rounds_to_one() {
        let mut clock = FixedClock::new(1.0, Duration::from_secs(1), 1).unwrap();
        let frame = clock.advance(Duration::from_nanos(999_999_999));
        assert_eq!(frame.ticks, 0);
        assert!(frame.alpha < 1.0);
        assert_eq!(clock.time_until_tick(), Duration::from_nanos(1));
    }
    #[test]
    fn huge_stalls_are_bounded_and_do_not_overflow() {
        let mut clock =
            FixedClock::with_rate(Rate::new(1_000_000_000, 1).unwrap(), Duration::MAX, 3).unwrap();
        let result = clock.advance(Duration::MAX);
        assert_eq!(result.ticks, 3);
        assert_eq!(result.dropped, Duration::MAX - Duration::from_nanos(3));
    }
    #[test]
    fn rejects_invalid_configuration() {
        assert!(FixedClock::new(f64::NAN, Duration::from_secs(1), 1).is_err());
        assert!(FixedClock::new(60.0, Duration::ZERO, 1).is_err());
        assert!(FixedClock::new(60.0, Duration::from_secs(1), 0).is_err());
    }
}
