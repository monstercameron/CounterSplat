//! Presentation-independent measurement and display settings for an ECS frame counter.
//!
//! The host records a monotonic timestamp once for each submitted frame. Measurements represent
//! the average interval between submissions; they are not GPU execution or display-present times.
use serde::{Deserialize, Serialize};
use std::{collections::VecDeque, time::Duration};

/// A completed window of submitted-frame intervals.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FrameMeasurement {
    pub fps: f64,
    pub frame_time_ms: f64,
    pub intervals: u32,
}
impl FrameMeasurement {
    pub fn validate(self) -> Result<(), &'static str> {
        if self.fps.is_finite()
            && self.fps > 0.0
            && self.fps <= 1_000_000_000.0
            && self.frame_time_ms.is_finite()
            && self.frame_time_ms > 0.0
            && self.frame_time_ms <= 2_000.0
            && self.intervals > 0
        {
            Ok(())
        } else {
            Err("invalid frame measurement")
        }
    }
}

/// Attach beside a text `UiElement` to display measured FPS and frame interval.
///
/// Settings and the last measurement are available to scripts through the `FrameCounter`
/// component adapter. Hosts own timing and replace `measurement` when a new window completes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FrameCounter {
    pub show_fps: bool,
    pub show_frame_time: bool,
    /// Number of decimal places, from zero to three.
    pub precision: u8,
    pub measurement: Option<FrameMeasurement>,
}
impl Default for FrameCounter {
    fn default() -> Self {
        Self {
            show_fps: true,
            show_frame_time: true,
            precision: 1,
            measurement: None,
        }
    }
}
impl FrameCounter {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.precision > 3 {
            return Err("frame counter precision must be between zero and three");
        }
        if let Some(measurement) = self.measurement {
            measurement.validate()?;
        }
        Ok(())
    }
    /// Format the current measurement for a UI text widget.
    pub fn label(&self) -> String {
        let precision = usize::from(self.precision.min(3));
        let fps = self
            .measurement
            .map_or_else(|| "--".to_owned(), |m| format!("{:.*}", precision, m.fps));
        let frame = self.measurement.map_or_else(
            || "--".to_owned(),
            |m| format!("{:.*}", precision, m.frame_time_ms),
        );
        match (self.show_fps, self.show_frame_time) {
            (true, true) => format!("{fps} FPS   /   {frame} ms"),
            (true, false) => format!("{fps} FPS"),
            (false, true) => format!("{frame} ms"),
            (false, false) => String::new(),
        }
    }
}

/// Result of observing a submitted frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FrameUpdate {
    /// Waiting for a full window, or the timestamp matched the previous frame.
    Unchanged,
    /// The clock moved backwards or a long interruption occurred; discard the old display value.
    Reset,
    /// A complete measurement window is ready.
    Measured(FrameMeasurement),
}

/// Counts intervals over half-second windows without keeping a history or allocating per frame.
/// Gaps longer than two seconds reset the measurement, avoiding misleading post-pause averages.
#[derive(Debug, Default, Clone)]
pub struct FrameMeter {
    last: Option<Duration>,
    elapsed: Duration,
    intervals: u32,
}
impl FrameMeter {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn reset(&mut self) {
        *self = Self::default();
    }
    /// Observe a monotonic submission timestamp, expressed on the host's clock.
    pub fn record(&mut self, now: Duration) -> FrameUpdate {
        let Some(previous) = self.last else {
            self.last = Some(now);
            return FrameUpdate::Unchanged;
        };
        let Some(delta) = now.checked_sub(previous) else {
            self.reset();
            self.last = Some(now);
            return FrameUpdate::Reset;
        };
        if delta.is_zero() {
            return FrameUpdate::Unchanged;
        }
        self.last = Some(now);
        if delta > Duration::from_secs(2) {
            self.elapsed = Duration::ZERO;
            self.intervals = 0;
            return FrameUpdate::Reset;
        }
        self.elapsed = self.elapsed.saturating_add(delta);
        self.intervals = self.intervals.saturating_add(1);
        if self.elapsed < Duration::from_millis(500) {
            return FrameUpdate::Unchanged;
        }
        let seconds = self.elapsed.as_secs_f64();
        let measurement = FrameMeasurement {
            fps: f64::from(self.intervals) / seconds,
            frame_time_ms: seconds * 1000.0 / f64::from(self.intervals),
            intervals: self.intervals,
        };
        self.elapsed = Duration::ZERO;
        self.intervals = 0;
        FrameUpdate::Measured(measurement)
    }
}

/// A bounded one-minute history of submitted-frame intervals.
///
/// Each quarter-second bucket keeps its slowest frame, retaining short spikes without storing
/// every frame. Gaps longer than the whole window and backwards clock movement clear stale history.
#[derive(Debug, Default, Clone)]
pub struct FrameHistory {
    last: Option<Duration>,
    buckets: VecDeque<FrameBucket>,
}
#[derive(Debug, Clone, Copy)]
struct FrameBucket {
    index: u128,
    at: Duration,
    max_ms: f32,
}
impl FrameHistory {
    pub const WINDOW_SECONDS: f32 = 60.0;
    const BUCKET_NANOS: u128 = 250_000_000;
    const MAX_BUCKETS: usize = 241;

    pub fn new() -> Self {
        Self::default()
    }
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Record one successful submission using the same monotonic clock as `FrameMeter`.
    pub fn record(&mut self, now: Duration) {
        let Some(previous) = self.last else {
            self.last = Some(now);
            return;
        };
        let Some(delta) = now.checked_sub(previous) else {
            self.reset();
            self.last = Some(now);
            return;
        };
        if delta.is_zero() {
            return;
        }
        self.last = Some(now);
        if delta > Duration::from_secs(60) {
            self.buckets.clear();
            return;
        }
        let index = now.as_nanos() / Self::BUCKET_NANOS;
        let ms = (delta.as_secs_f64() * 1000.0) as f32;
        if let Some(bucket) = self
            .buckets
            .back_mut()
            .filter(|bucket| bucket.index == index)
        {
            bucket.at = now;
            bucket.max_ms = bucket.max_ms.max(ms);
        } else {
            self.buckets.push_back(FrameBucket {
                index,
                at: now,
                max_ms: ms,
            });
        }
        while self
            .buckets
            .front()
            .is_some_and(|bucket| now.saturating_sub(bucket.at) > Duration::from_secs(60))
            || self.buckets.len() > Self::MAX_BUCKETS
        {
            self.buckets.pop_front();
        }
    }

    /// Points for a line graph: x is seconds from the left edge of a rolling 60-second window,
    /// y is the slowest submitted-frame interval in milliseconds in that quarter-second bucket.
    pub fn points(&self, now: Duration) -> Vec<[f32; 2]> {
        let mut points = Vec::with_capacity(self.buckets.len());
        self.write_points(now, &mut points);
        points
    }

    /// Replace a caller-owned buffer with the current graph points, reusing its allocation.
    pub fn write_points(&self, now: Duration, points: &mut Vec<[f32; 2]>) {
        points.clear();
        points.reserve(self.buckets.len());
        points.extend(self.buckets.iter().filter_map(|bucket| {
            let age = now.checked_sub(bucket.at)?.as_secs_f32();
            (age <= Self::WINDOW_SECONDS).then_some([Self::WINDOW_SECONDS - age, bucket.max_ms])
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn measures_submitted_intervals_and_resets_after_a_pause_or_clock_regression() {
        let mut meter = FrameMeter::new();
        for i in 0..25 {
            assert_eq!(
                meter.record(Duration::from_millis(i * 20)),
                FrameUpdate::Unchanged
            );
        }
        let FrameUpdate::Measured(value) = meter.record(Duration::from_millis(500)) else {
            panic!("measurement expected")
        };
        assert_eq!(value.intervals, 25);
        assert!((value.fps - 50.0).abs() < 1e-9);
        assert!((value.frame_time_ms - 20.0).abs() < 1e-9);
        assert_eq!(
            meter.record(Duration::from_millis(500)),
            FrameUpdate::Unchanged
        );
        assert_eq!(meter.record(Duration::from_secs(3)), FrameUpdate::Reset);
        assert_eq!(meter.record(Duration::from_secs(2)), FrameUpdate::Reset);
        assert_eq!(
            meter.record(Duration::from_millis(2500)),
            FrameUpdate::Measured(FrameMeasurement {
                fps: 2.0,
                frame_time_ms: 500.0,
                intervals: 1,
            })
        );
    }
    #[test]
    fn minute_history_is_bounded_and_keeps_peaks() {
        let mut history = FrameHistory::new();
        history.record(Duration::ZERO);
        history.record(Duration::from_millis(16));
        history.record(Duration::from_millis(196)); // A 180 ms spike in the same bucket.
        history.record(Duration::from_millis(212));
        let points = history.points(Duration::from_millis(212));
        assert_eq!(points.len(), 1);
        assert!((points[0][1] - 180.0).abs() < 0.001);
        for i in 3..=800 {
            history.record(Duration::from_millis(i * 100));
        }
        let points = history.points(Duration::from_secs(80));
        let mut reused = vec![[999.0, 999.0]];
        history.write_points(Duration::from_secs(80), &mut reused);
        assert_eq!(reused, points);
        let capacity = reused.capacity();
        history.write_points(Duration::from_secs(80), &mut reused);
        assert_eq!(reused.capacity(), capacity);
        assert_eq!(reused, points);
        assert!(points.len() <= 241 && points.len() >= 238);
        assert!(points.iter().all(|point| (0.0..=60.0).contains(&point[0])));
        assert!(
            points.iter().all(|point| point[1] <= 100.1),
            "expired spikes must leave the window"
        );
        assert!((points.last().unwrap()[0] - 60.0).abs() < 0.001);
        history.record(Duration::from_secs(90));
        assert_eq!(
            history.points(Duration::from_secs(90)).last().unwrap()[1],
            10_000.0,
            "a long frame within the window must appear as a spike"
        );
        history.record(Duration::from_secs(151));
        assert!(history.points(Duration::from_secs(151)).is_empty());
        history.record(Duration::from_secs(150));
        assert!(history.points(Duration::from_secs(150)).is_empty());
    }
    #[test]
    fn formatting_and_validation_are_script_safe() {
        let mut counter = FrameCounter::default();
        assert_eq!(counter.label(), "-- FPS   /   -- ms");
        counter.measurement = Some(FrameMeasurement {
            fps: 50.0,
            frame_time_ms: 20.0,
            intervals: 25,
        });
        assert_eq!(counter.label(), "50.0 FPS   /   20.0 ms");
        counter.show_frame_time = false;
        assert_eq!(counter.label(), "50.0 FPS");
        counter.precision = 4;
        assert!(counter.validate().is_err());
    }
}
