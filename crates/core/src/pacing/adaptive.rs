//! Optional adaptive frame target based on real CPU and/or GPU workload measurements.
//!
//! The controller selects an integer number of known display refresh periods per frame. It uses a
//! bounded rolling 90th percentile, safety margins, and asymmetric hysteresis. It never invents GPU
//! timings: only pass GPU execution duration when the backend actually measured it. A host chooses
//! whether to apply the recommendation to a [`super::DeadlinePacer`] or scheduler.

use super::NonMonotonicTime;
use crate::rate::{ceil_div, Rate};
use std::{collections::VecDeque, time::Duration};

const SAMPLE_WINDOW: usize = 60;
const MIN_SAMPLES: usize = 8;
const OVER_FOR: Duration = Duration::from_millis(500);
const UNDER_FOR: Duration = Duration::from_secs(2);
const STALE_AFTER: Duration = Duration::from_secs(2);
const MAX_SAMPLE: Duration = Duration::from_secs(10);

/// Actual per-frame execution measurements. Use `None` when a measurement is unavailable.
///
/// CPU and GPU may overlap, so the controller uses the largest available measurement rather than
/// adding them. Pipeline time is useful when a host allows only one frame in flight. Do not pass sleep time, a queued frame's total latency, or an estimated value as GPU time.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WorkSample {
    /// CPU work for this frame, excluding intentional sleeping.
    pub cpu: Option<Duration>,
    /// Measured GPU execution for this frame, if timestamp queries are available.
    pub gpu: Option<Duration>,
    /// End-to-end service time for a single in-flight frame, including any readback or encoding.
    /// This is not a GPU timestamp or a measure of display presentation.
    pub pipeline: Option<Duration>,
}

/// A changed integer refresh span and its resulting target frame rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdaptiveChange {
    /// Number of display refresh periods per requested frame.
    pub span: u32,
    /// Requested frame rate after applying the span.
    pub rate: Rate,
}

/// Bounded, hysteretic adaptation for hosts with a known fixed display refresh rate.
#[derive(Debug, Clone)]
pub struct AdaptivePacer {
    refresh: Rate,
    max_span: u32,
    span: u32,
    samples: VecDeque<Duration>,
    over_since: Option<Duration>,
    under_since: Option<Duration>,
    last_now: Option<Duration>,
}

impl AdaptivePacer {
    /// Create a controller, initially targeting every refresh.
    ///
    /// `max_span` is the largest supported number of refreshes per frame. A change in monitor or
    /// refresh configuration should create or reset a controller using the new rate.
    pub fn new(refresh: Rate, max_span: u32) -> Result<Self, &'static str> {
        if max_span == 0 || refresh.denominator().checked_mul(max_span).is_none() {
            return Err("adaptive refresh span is invalid or unrepresentable");
        }
        Ok(Self {
            refresh,
            max_span,
            span: 1,
            samples: VecDeque::new(),
            over_since: None,
            under_since: None,
            last_now: None,
        })
    }
    /// Current refresh periods per frame.
    pub fn span(&self) -> u32 {
        self.span
    }
    /// Current recommended target.
    pub fn rate(&self) -> Rate {
        Rate::new(
            self.refresh.numerator(),
            self.refresh.denominator() * self.span,
        )
        .expect("adaptive configuration validates every target")
    }
    /// Forget measurements and hysteresis after a pause, resize, or device change.
    pub fn reset_measurements(&mut self) {
        self.samples.clear();
        self.over_since = None;
        self.under_since = None;
        self.last_now = None;
    }
    /// Record a measured frame and optionally return a changed target.
    ///
    /// Isolated spikes are resisted by the percentile; sustained overload raises the span after
    /// 500 ms, directly to the measured span when necessary. Recovery needs two seconds of headroom. Zero/unavailable samples are ignored. A
    /// gap longer than two seconds clears stale history. Backwards timestamps return an error.
    pub fn record(
        &mut self,
        now: Duration,
        sample: WorkSample,
    ) -> Result<Option<AdaptiveChange>, NonMonotonicTime> {
        if self.last_now.is_some_and(|last| now < last) {
            return Err(NonMonotonicTime);
        }
        if self.last_now.is_some_and(|last| now - last > STALE_AFTER) {
            self.samples.clear();
            self.over_since = None;
            self.under_since = None;
        }
        self.last_now = Some(now);
        let Some(work) = sample
            .cpu
            .into_iter()
            .chain(sample.gpu)
            .chain(sample.pipeline)
            .filter(|value| !value.is_zero())
            .max()
        else {
            return Ok(None);
        };
        self.samples.push_back(work.min(MAX_SAMPLE));
        if self.samples.len() > SAMPLE_WINDOW {
            self.samples.pop_front();
        }
        if self.samples.len() < MIN_SAMPLES {
            return Ok(None);
        }
        let mut sorted: Vec<_> = self.samples.iter().copied().collect();
        sorted.sort_unstable();
        let p90 = sorted[(sorted.len() * 9).div_ceil(10) - 1].as_nanos();
        let work_units = p90 * self.refresh.scale();
        let refresh_units = self.refresh.period_units();
        let over = work_units * 110 > refresh_units * u128::from(self.span) * 100;
        let required_span =
            ceil_div(work_units * 110, refresh_units * 100).min(u128::from(self.max_span)) as u32;
        let under =
            self.span > 1 && work_units * 125 < refresh_units * u128::from(self.span - 1) * 100;
        let mut changed = false;
        if over && self.span < self.max_span {
            self.under_since = None;
            let since = *self.over_since.get_or_insert(now);
            if now - since >= OVER_FOR {
                self.span = required_span.max(self.span + 1).min(self.max_span);
                changed = true;
            }
        } else if under {
            self.over_since = None;
            let since = *self.under_since.get_or_insert(now);
            if now - since >= UNDER_FOR {
                self.span -= 1;
                changed = true;
            }
        } else {
            self.over_since = None;
            self.under_since = None;
        }
        if changed {
            self.samples.clear();
            self.over_since = None;
            self.under_since = None;
            return Ok(Some(AdaptiveChange {
                span: self.span,
                rate: self.rate(),
            }));
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sustained_load_degrades_and_recovers_with_hysteresis() {
        let mut p = AdaptivePacer::new(Rate::new(60, 1).unwrap(), 4).unwrap();
        for n in 0..180 {
            p.record(
                Duration::from_millis(n * 17),
                WorkSample {
                    cpu: Some(Duration::from_millis(21)),
                    gpu: None,
                    pipeline: None,
                },
            )
            .unwrap();
        }
        assert_eq!(p.span(), 2);
        assert_eq!(p.rate(), Rate::new(30, 1).unwrap());
        for n in 180..450 {
            p.record(
                Duration::from_millis(n * 17),
                WorkSample {
                    cpu: Some(Duration::from_millis(8)),
                    gpu: None,
                    pipeline: None,
                },
            )
            .unwrap();
        }
        assert_eq!(p.span(), 1);
    }
    #[test]
    fn severe_sustained_overload_reaches_the_supported_span_promptly() {
        let mut p = AdaptivePacer::new(Rate::new(60, 1).unwrap(), 16).unwrap();
        for n in 0..14 {
            p.record(
                Duration::from_millis(n * 250),
                WorkSample {
                    cpu: None,
                    gpu: None,
                    pipeline: Some(Duration::from_millis(250)),
                },
            )
            .unwrap();
        }
        assert_eq!(p.span(), 16);
    }
    #[test]
    fn no_measurements_or_isolated_spikes_do_not_invent_a_slowdown() {
        let mut p = AdaptivePacer::new(Rate::new(60, 1).unwrap(), 4).unwrap();
        for n in 0..180 {
            let sample = if n % 30 == 0 {
                Duration::from_millis(100)
            } else {
                Duration::from_millis(5)
            };
            p.record(
                Duration::from_millis(n * 17),
                WorkSample {
                    cpu: Some(sample),
                    gpu: None,
                    pipeline: None,
                },
            )
            .unwrap();
        }
        assert_eq!(p.span(), 1);
        p.record(Duration::from_secs(10), WorkSample::default())
            .unwrap();
        assert_eq!(p.span(), 1);
        assert!(p.record(Duration::ZERO, WorkSample::default()).is_err());
    }
    #[test]
    fn validates_limits_and_supports_fractional_display_rates() {
        let refresh = Rate::new(60_000, 1_001).unwrap();
        assert!(AdaptivePacer::new(refresh, 0).is_err());
        assert!(AdaptivePacer::new(refresh, u32::MAX).is_err());
        let mut p = AdaptivePacer::new(refresh, 4).unwrap();
        for n in 0..180 {
            p.record(
                Duration::from_millis(n * 17),
                WorkSample {
                    cpu: None,
                    gpu: Some(Duration::from_millis(20)),
                    pipeline: None,
                },
            )
            .unwrap();
        }
        assert_eq!(p.rate(), Rate::new(30_000, 1_001).unwrap());
    }
}
