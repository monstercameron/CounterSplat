//! Frame pacing primitives for different host environments.
//!
//! [`DeadlinePacer`] schedules phase-locked software frame requests from monotonic time.
//! [`AdaptivePacer`] can recommend a refresh-aligned target from real workload measurements.
//! [`FramePacer`] is a callback-driven helper for hosts with one callback per display refresh.
//! None of these APIs alone guarantees presentation timing; that depends on the windowing system,
//! GPU, presentation mode, and operating-system scheduling.

mod adaptive;
mod deadline;
mod limiter;
pub use adaptive::{AdaptiveChange, AdaptivePacer, WorkSample};
pub use deadline::{DeadlinePacer, FrameDecision, NonMonotonicTime};
pub use limiter::{FrameLimiter, FramePermit};

use std::collections::VecDeque;

/// How frames are paced (`render.frame_pacing`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PacingMode {
    Off,
    Auto,
    /// A fixed rate in frames per second.
    Fixed(f64),
}

impl PacingMode {
    pub fn parse(text: &str) -> Self {
        match text.trim().to_ascii_lowercase().as_str() {
            "off" | "none" => Self::Off,
            "auto" | "" => Self::Auto,
            other => other
                .parse::<f64>()
                .ok()
                .filter(|hz| hz.is_finite() && *hz > 0.0)
                .map_or(Self::Auto, Self::Fixed),
        }
    }
}

/// Recent samples kept for the refresh and GPU estimates.
const SAMPLES: usize = 90;
/// The GPU time is given this much slack before a span counts as held.
const HEADROOM: f64 = 1.1;
/// Seconds over budget before spanning more refreshes, and with room to spare before spanning fewer.
const RAISE_AFTER_MS: f64 = 500.0;
const LOWER_AFTER_MS: f64 = 2000.0;
/// The longest span: 15 frames a second on a 60 Hz display.
const MAX_SPAN: u32 = 4;

#[derive(Debug, Clone)]
pub struct FramePacer {
    mode: PacingMode,
    last_callback: Option<f64>,
    intervals: VecDeque<f64>,
    gpu: VecDeque<f64>,
    span: u32,
    last_render: Option<f64>,
    over_since: Option<f64>,
    under_since: Option<f64>,
    /// The display's refresh interval when the host knows it (a native window's monitor), instead of estimating it.
    refresh_hint: Option<f64>,
}

impl FramePacer {
    pub fn new(mode: PacingMode) -> Self {
        let mode = valid_mode(mode);
        Self {
            mode,
            last_callback: None,
            intervals: VecDeque::new(),
            gpu: VecDeque::new(),
            span: 1,
            last_render: None,
            over_since: None,
            under_since: None,
            refresh_hint: None,
        }
    }

    pub fn set_mode(&mut self, mode: PacingMode) {
        let mode = valid_mode(mode);
        if self.mode != mode {
            self.mode = mode;
            self.span = 1;
            self.last_render = None;
            self.over_since = None;
            self.under_since = None;
            self.gpu.clear();
        }
    }

    /// Uses this refresh interval (milliseconds) instead of estimating one from the callbacks: a native window
    /// does not get one callback per refresh.
    pub fn set_refresh_hint(&mut self, refresh_ms: Option<f64>) {
        self.refresh_hint = refresh_ms.filter(|r| r.is_finite() && *r > 1.0);
    }

    /// When the next frame is due (milliseconds on the callers' clock), for a host that sleeps until then.
    pub fn next_due_ms(&self) -> f64 {
        let refresh = self.refresh_ms();
        self.last_render
            .map_or(0.0, |last| last + f64::from(self.span) * refresh)
    }

    /// The display's refresh interval in milliseconds: the host's hint, or a low percentile of the recent callback
    /// intervals (16.7 until there are enough).
    pub fn refresh_ms(&self) -> f64 {
        if let Some(hint) = self.refresh_hint {
            return hint;
        }
        if self.intervals.len() < 10 {
            return 1000.0 / 60.0;
        }
        let mut sorted: Vec<f64> = self.intervals.iter().copied().collect();
        sorted.sort_by(f64::total_cmp);
        sorted[sorted.len() / 10].max(1.0)
    }

    /// Refreshes each frame spans now.
    pub fn span(&self) -> u32 {
        self.span
    }

    /// The median GPU time of recent frames, milliseconds.
    pub fn gpu_ms(&self) -> f64 {
        let mut sorted: Vec<f64> = self.gpu.iter().copied().collect();
        sorted.sort_by(f64::total_cmp);
        sorted.get(sorted.len() / 2).copied().unwrap_or(0.0)
    }

    /// A rendered frame's GPU time (0 or less is ignored: not measured yet).
    pub fn record_gpu_ms(&mut self, ms: f64) {
        if ms.is_finite() && ms > 0.0 {
            push(&mut self.gpu, ms);
        }
    }

    /// Call on every display callback at `now_ms`; true when this one should render.
    pub fn tick(&mut self, now_ms: f64) -> bool {
        if !now_ms.is_finite() || self.last_callback.is_some_and(|last| now_ms < last) {
            return false;
        }
        if let Some(last) = self.last_callback.replace(now_ms) {
            let interval = now_ms - last;
            // A long gap (a hidden tab, a breakpoint) says nothing about the display.
            if interval > 0.0 && interval < 100.0 {
                push(&mut self.intervals, interval);
            }
        }
        let refresh = self.refresh_ms();
        self.span = match self.mode {
            PacingMode::Off => 1,
            PacingMode::Fixed(hz) => ((1000.0 / hz / refresh).round() as u32).max(1),
            PacingMode::Auto => self.auto_span(now_ms, refresh),
        };
        if self.mode == PacingMode::Off {
            self.last_render = Some(now_ms);
            return true;
        }
        // Render once the span has passed, within half a refresh (callbacks jitter).
        let due = self
            .last_render
            .is_none_or(|last| now_ms - last >= f64::from(self.span) * refresh - refresh * 0.5);
        if due {
            self.last_render = Some(now_ms);
        }
        due
    }

    fn auto_span(&mut self, now_ms: f64, refresh: f64) -> u32 {
        if self.gpu.len() < 5 {
            return self.span;
        }
        let need = ((self.gpu_ms() * HEADROOM / refresh).ceil() as u32).clamp(1, MAX_SPAN);
        if need > self.span {
            self.under_since = None;
            let since = *self.over_since.get_or_insert(now_ms);
            if now_ms - since >= RAISE_AFTER_MS {
                // One refresh at a time: while frames queue up behind each other the measured time includes the
                // wait, so it overstates the cost until a longer span lets the queue drain.
                self.over_since = None;
                self.gpu.clear();
                return self.span + 1;
            }
        } else if need < self.span {
            self.over_since = None;
            let since = *self.under_since.get_or_insert(now_ms);
            if now_ms - since >= LOWER_AFTER_MS {
                self.under_since = None;
                self.gpu.clear();
                return self.span - 1;
            }
        } else {
            self.over_since = None;
            self.under_since = None;
        }
        self.span
    }
}

fn valid_mode(mode: PacingMode) -> PacingMode {
    match mode {
        PacingMode::Fixed(hz) if !hz.is_finite() || hz <= 0.0 => PacingMode::Auto,
        _ => mode,
    }
}

fn push(samples: &mut VecDeque<f64>, value: f64) {
    samples.push_back(value);
    if samples.len() > SAMPLES {
        samples.pop_front();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs `seconds` of 60 Hz callbacks; the GPU takes `gpu_ms` per rendered frame. Returns the intervals between
    /// rendered frames over the last second.
    fn run(pacer: &mut FramePacer, seconds: f64, gpu_ms: f64, start: f64) -> Vec<f64> {
        let refresh = 1000.0 / 60.0;
        let mut rendered = Vec::new();
        let n = (seconds * 60.0) as usize;
        for k in 0..n {
            let now = start + k as f64 * refresh;
            if pacer.tick(now) {
                pacer.record_gpu_ms(gpu_ms);
                rendered.push(now);
            }
        }
        let tail: Vec<f64> = rendered
            .iter()
            .copied()
            .filter(|t| *t >= start + (seconds - 1.0) * 1000.0)
            .collect();
        tail.windows(2).map(|w| w[1] - w[0]).collect()
    }

    #[test]
    fn a_gpu_that_holds_60_renders_every_refresh() {
        let mut pacer = FramePacer::new(PacingMode::Auto);
        let intervals = run(&mut pacer, 4.0, 9.0, 0.0);
        assert_eq!(pacer.span(), 1);
        assert!(
            intervals.iter().all(|i| (i - 16.67).abs() < 1.0),
            "{intervals:?}"
        );
    }

    #[test]
    fn it_steps_up_one_refresh_at_a_time() {
        // A backed-up queue reports 60 ms; the pacer goes to two refreshes, not straight to four.
        let mut pacer = FramePacer::new(PacingMode::Auto);
        run(&mut pacer, 0.8, 60.0, 0.0);
        assert_eq!(pacer.span(), 2);
    }

    #[test]
    fn a_gpu_just_over_a_refresh_paces_at_a_steady_30() {
        let mut pacer = FramePacer::new(PacingMode::Auto);
        let intervals = run(&mut pacer, 4.0, 21.0, 0.0);
        assert_eq!(pacer.span(), 2);
        assert!(
            intervals.iter().all(|i| (i - 33.33).abs() < 1.0),
            "every frame two refreshes: {intervals:?}"
        );
    }

    #[test]
    fn it_returns_to_60_once_the_gpu_has_room_for_a_while() {
        let mut pacer = FramePacer::new(PacingMode::Auto);
        run(&mut pacer, 3.0, 21.0, 0.0);
        assert_eq!(pacer.span(), 2);
        run(&mut pacer, 1.0, 9.0, 3000.0);
        assert_eq!(pacer.span(), 2, "not after one second");
        run(&mut pacer, 3.0, 9.0, 4000.0);
        assert_eq!(pacer.span(), 1);
    }

    #[test]
    fn fixed_and_off_modes() {
        let mut fixed = FramePacer::new(PacingMode::parse("30"));
        let intervals = run(&mut fixed, 2.0, 5.0, 0.0);
        assert!(
            intervals.iter().all(|i| (i - 33.33).abs() < 1.0),
            "{intervals:?}"
        );
        let mut off = FramePacer::new(PacingMode::parse("off"));
        assert!((0..10).all(|k| off.tick(k as f64 * 16.67)));
        assert_eq!(PacingMode::parse("auto"), PacingMode::Auto);
    }
}
