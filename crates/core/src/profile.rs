//! Opt-in, bounded frame and latency measurements for a host's profiler: sample summaries, per-frame CPU stage
//! times, input-to-submission latency, and worker round trips. Times are milliseconds from the caller's clock, so
//! everything here is deterministic and tested without a browser.

use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, VecDeque};

/// Summary of a sample window: count, mean, p50, p95, p99 and max (`Value::Null` when empty).
pub fn summarize(samples: &VecDeque<f64>) -> Value {
    if samples.is_empty() {
        return Value::Null;
    }
    let mut values: Vec<f64> = samples.iter().copied().collect();
    values.sort_by(f64::total_cmp);
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    let percentile = |p: f64| values[((values.len() - 1) as f64 * p).ceil() as usize];
    json!({"samples": values.len(), "mean": mean, "p50": percentile(0.5), "p95": percentile(0.95),
        "p99": percentile(0.99), "max": values[values.len() - 1]})
}

fn push_bounded(samples: &mut VecDeque<f64>, value: f64, capacity: usize) {
    samples.push_back(value);
    while samples.len() > capacity {
        samples.pop_front();
    }
}

fn check_bounds(capacity: usize, pending_limit: usize, what: &str) -> Result<(), String> {
    if !(1..=3600).contains(&capacity) || !(1..=4096).contains(&pending_limit) {
        return Err(format!("Invalid {what} bounds"));
    }
    Ok(())
}

/// The CPU stage names a host reports per frame, besides the frame-level times.
pub const FRAME_STAGES: [&str; 6] = [
    "event_dispatch_ms",
    "lua_systems_ms",
    "tick_events_ms",
    "output_ms",
    "voxel_ms",
    "stream_ms",
];

/// Rolling windows of animation-frame intervals, submission intervals and per-frame CPU stage times.
#[derive(Debug, Clone)]
pub struct FrameProfiler {
    capacity: usize,
    last_frame: Option<f64>,
    last_submit: Option<f64>,
    samples: BTreeMap<&'static str, VecDeque<f64>>,
}

impl FrameProfiler {
    pub fn new(capacity: usize) -> Result<Self, String> {
        if !(1..=3600).contains(&capacity) {
            return Err("Frame profile capacity must be an integer between 1 and 3600".into());
        }
        let mut profiler = Self {
            capacity,
            last_frame: None,
            last_submit: None,
            samples: BTreeMap::new(),
        };
        profiler.reset();
        Ok(profiler)
    }

    pub fn reset(&mut self) {
        self.last_frame = None;
        self.last_submit = None;
        self.samples = ["animation_ms", "submit_ms", "render_cpu_ms", "scripts_cpu_ms", "callback_cpu_ms"]
            .into_iter()
            .chain(FRAME_STAGES)
            .map(|name| (name, VecDeque::new()))
            .collect();
    }

    fn push(&mut self, name: &str, value: f64) {
        if !value.is_finite() || value < 0.0 {
            return;
        }
        if let Some(samples) = self.samples.get_mut(name) {
            push_bounded(samples, value, self.capacity);
        }
    }

    /// One animation frame at `timestamp`; `stages` maps stage names to milliseconds (missing ones count as 0).
    pub fn record(
        &mut self,
        timestamp: f64,
        submitted: bool,
        render_ms: f64,
        scripts_ms: f64,
        callback_ms: f64,
        stages: &Map<String, Value>,
    ) {
        if !timestamp.is_finite() {
            return;
        }
        if let Some(last) = self.last_frame {
            self.push("animation_ms", timestamp - last);
        }
        self.last_frame = Some(timestamp);
        if submitted {
            if let Some(last) = self.last_submit {
                self.push("submit_ms", timestamp - last);
            }
            self.last_submit = Some(timestamp);
        }
        self.push("render_cpu_ms", render_ms);
        self.push("scripts_cpu_ms", scripts_ms);
        self.push("callback_cpu_ms", callback_ms);
        for name in FRAME_STAGES {
            let value = stages.get(name).and_then(Value::as_f64).unwrap_or(0.0);
            self.push(name, value);
        }
    }

    /// Summaries plus the renderer's busy-skip fraction (`state` is its status, `lua` the script profile).
    pub fn snapshot(&self, state: &Value, lua: Value) -> Value {
        let mut out: Map<String, Value> = self
            .samples
            .iter()
            .map(|(name, samples)| (name.to_string(), summarize(samples)))
            .collect();
        let number = |key: &str| state.get(key).and_then(|v| v.as_f64().or_else(|| v.as_str()?.parse().ok())).unwrap_or(0.0);
        let (submitted, busy) = (number("submitted_frames"), number("busy_skips"));
        out.insert("gpu_busy_skips".into(), json!(busy));
        out.insert("submitted_frames".into(), json!(submitted));
        let fraction = if busy + submitted > 0.0 { busy / (busy + submitted) } else { 0.0 };
        out.insert("busy_fraction".into(), json!(fraction));
        out.insert("lua".into(), lua);
        Value::Object(out)
    }
}

/// Time from a browser input event to its dispatch to scripts and to the next submitted frame.
#[derive(Debug, Clone)]
pub struct InputFrameLatency {
    capacity: usize,
    pending_limit: usize,
    pending: VecDeque<f64>,
    dispatched: VecDeque<(f64, f64)>,
    event_to_submit: VecDeque<f64>,
    dispatch_to_submit: VecDeque<f64>,
    evicted: u64,
}

impl InputFrameLatency {
    pub fn new(capacity: usize, pending_limit: usize) -> Result<Self, String> {
        check_bounds(capacity, pending_limit, "input latency")?;
        Ok(Self {
            capacity,
            pending_limit,
            pending: VecDeque::new(),
            dispatched: VecDeque::new(),
            event_to_submit: VecDeque::new(),
            dispatch_to_submit: VecDeque::new(),
            evicted: 0,
        })
    }

    pub fn record_input(&mut self, now: f64) {
        if !now.is_finite() {
            return;
        }
        if self.pending.len() >= self.pending_limit {
            self.pending.pop_front();
            self.evicted += 1;
        }
        self.pending.push_back(now);
    }

    pub fn mark_dispatched(&mut self, now: f64) {
        if self.pending.is_empty() {
            return;
        }
        for started in std::mem::take(&mut self.pending) {
            if !now.is_finite() || now < started {
                continue;
            }
            if self.dispatched.len() >= self.pending_limit {
                self.dispatched.pop_front();
                self.evicted += 1;
            }
            self.dispatched.push_back((started, now));
        }
    }

    pub fn mark_submitted(&mut self, now: f64) {
        for (started, dispatched) in std::mem::take(&mut self.dispatched) {
            if !now.is_finite() || now < dispatched {
                continue;
            }
            push_bounded(&mut self.event_to_submit, now - started, self.capacity);
            push_bounded(&mut self.dispatch_to_submit, now - dispatched, self.capacity);
        }
    }

    pub fn clear_pending(&mut self) {
        self.pending.clear();
        self.dispatched.clear();
    }

    pub fn reset(&mut self) {
        self.clear_pending();
        self.event_to_submit.clear();
        self.dispatch_to_submit.clear();
        self.evicted = 0;
    }

    pub fn snapshot(&self) -> Value {
        json!({"pending": self.pending.len(), "dispatched": self.dispatched.len(), "evicted": self.evicted,
            "event_to_submit_ms": summarize(&self.event_to_submit),
            "dispatch_to_submit_ms": summarize(&self.dispatch_to_submit)})
    }
}

#[derive(Debug, Clone, Default)]
struct Operation {
    round_trip: VecDeque<f64>,
    worker: VecDeque<f64>,
    batch_size: VecDeque<f64>,
    completed: u64,
    errors: u64,
}

/// Worker round trips per operation, and request-to-frame latency for results applied to the scene.
#[derive(Debug, Clone)]
pub struct WorkerLatency {
    capacity: usize,
    pending_limit: usize,
    /// Insertion-ordered pending requests: (id, operation, batch size, started).
    pending: VecDeque<(i64, String, Option<f64>, f64)>,
    operations: BTreeMap<String, Operation>,
    evicted: u64,
    applied: VecDeque<(f64, f64)>,
    applied_evicted: u64,
    request_to_submit: VecDeque<f64>,
    applied_to_submit: VecDeque<f64>,
}

impl WorkerLatency {
    pub fn new(capacity: usize, pending_limit: usize) -> Result<Self, String> {
        check_bounds(capacity, pending_limit, "worker latency")?;
        Ok(Self {
            capacity,
            pending_limit,
            pending: VecDeque::new(),
            operations: BTreeMap::new(),
            evicted: 0,
            applied: VecDeque::new(),
            applied_evicted: 0,
            request_to_submit: VecDeque::new(),
            applied_to_submit: VecDeque::new(),
        })
    }

    pub fn start(&mut self, id: i64, operation: &str, batch_size: Option<f64>, now: f64) {
        if let Some(entry) = self.pending.iter_mut().find(|entry| entry.0 == id) {
            *entry = (id, operation.to_string(), batch_size, now);
            return;
        }
        if self.pending.len() >= self.pending_limit {
            self.pending.pop_front();
            self.evicted += 1;
        }
        self.pending.push_back((id, operation.to_string(), batch_size, now));
    }

    /// Completes request `id`; returns when it started (for `mark_applied`), or `None` for an unknown request.
    pub fn finish(&mut self, id: i64, worker_ms: Option<f64>, failed: bool, now: f64) -> Option<f64> {
        let at = self.pending.iter().position(|entry| entry.0 == id)?;
        let (_, operation, batch_size, started) = self.pending.remove(at)?;
        let elapsed = now - started;
        if !elapsed.is_finite() || elapsed < 0.0 {
            return None;
        }
        let capacity = self.capacity;
        let data = self.operations.entry(operation).or_default();
        data.completed += 1;
        if failed {
            data.errors += 1;
        }
        if let Some(size) = batch_size.filter(|size| size.fract() == 0.0 && *size > 0.0) {
            push_bounded(&mut data.batch_size, size, capacity);
        }
        push_bounded(&mut data.round_trip, elapsed, capacity);
        if let Some(ms) = worker_ms.filter(|ms| ms.is_finite() && *ms >= 0.0) {
            push_bounded(&mut data.worker, ms, capacity);
        }
        Some(started)
    }

    /// A result the scene accepted; it can show in the next submitted frame.
    pub fn mark_applied(&mut self, started: f64, now: f64) {
        if !started.is_finite() || !now.is_finite() || now < started {
            return;
        }
        if self.applied.len() >= self.pending_limit {
            self.applied.pop_front();
            self.applied_evicted += 1;
        }
        self.applied.push_back((started, now));
    }

    pub fn mark_submitted(&mut self, now: f64) {
        for (started, applied) in std::mem::take(&mut self.applied) {
            if !now.is_finite() || now < applied {
                continue;
            }
            push_bounded(&mut self.request_to_submit, now - started, self.capacity);
            push_bounded(&mut self.applied_to_submit, now - applied, self.capacity);
        }
    }

    pub fn clear_applied(&mut self) {
        self.applied.clear();
    }

    pub fn snapshot(&self) -> Value {
        let operations: Map<String, Value> = self
            .operations
            .iter()
            .map(|(name, data)| {
                (
                    name.clone(),
                    json!({"completed": data.completed, "errors": data.errors,
                        "round_trip_ms": summarize(&data.round_trip), "worker_ms": summarize(&data.worker),
                        "batch_size": summarize(&data.batch_size)}),
                )
            })
            .collect();
        json!({"pending": self.pending.len(), "evicted": self.evicted,
            "movement_submit": {"pending": self.applied.len(), "evicted": self.applied_evicted,
                "request_to_submit_ms": summarize(&self.request_to_submit),
                "applied_to_submit_ms": summarize(&self.applied_to_submit)},
            "operations": operations})
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_profile_windows_and_busy_fraction() {
        let mut profile = FrameProfiler::new(3).unwrap();
        for i in 0..6 {
            let i = f64::from(i);
            let stages = json!({"lua_systems_ms": i});
            profile.record(i * 10.0, i as u32 % 2 == 0, i, i * 2.0, i * 3.0, stages.as_object().unwrap());
        }
        let result = profile.snapshot(&json!({"submitted_frames": 3, "busy_skips": 1}), Value::Null);
        assert_eq!(result["animation_ms"]["samples"], 3);
        assert_eq!(result["animation_ms"]["mean"], 10.0);
        assert_eq!(result["submit_ms"]["mean"], 20.0);
        assert_eq!(result["lua_systems_ms"]["mean"], 4.0);
        assert_eq!(result["busy_fraction"], 0.25);
        profile.record(5100.0, true, 1.0, 2.0, 3.0, &Map::new());
        assert_eq!(profile.snapshot(&Value::Null, Value::Null)["animation_ms"]["max"], 5050.0, "long stalls remain visible");
        assert!(FrameProfiler::new(0).is_err());
        profile.reset();
        assert_eq!(profile.snapshot(&Value::Null, Value::Null)["animation_ms"], Value::Null);
    }

    #[test]
    fn input_latency_follows_events_to_submission() {
        let mut profile = InputFrameLatency::new(2, 2).unwrap();
        profile.record_input(1.0);
        profile.record_input(2.0);
        profile.mark_submitted(3.0);
        assert_eq!(profile.snapshot()["event_to_submit_ms"], Value::Null);
        profile.mark_dispatched(4.0);
        assert_eq!(profile.snapshot()["dispatched"], 2);
        profile.mark_submitted(10.0);
        let report = profile.snapshot();
        assert_eq!(report["event_to_submit_ms"]["samples"], 2);
        assert_eq!(report["event_to_submit_ms"]["p50"], 9.0);
        assert_eq!(report["dispatch_to_submit_ms"]["p50"], 6.0);
        for _ in 0..3 {
            profile.record_input(10.0);
        }
        assert_eq!(profile.snapshot()["evicted"], 1);
        profile.mark_dispatched(10.0);
        profile.clear_pending();
        assert_eq!(profile.snapshot()["dispatched"], 0);
        profile.reset();
        assert_eq!(profile.snapshot()["event_to_submit_ms"], Value::Null);
        assert_eq!(profile.snapshot()["evicted"], 0);
        assert!(InputFrameLatency::new(0, 2).is_err());
        assert!(InputFrameLatency::new(2, 0).is_err());
    }

    #[test]
    fn worker_latency_per_operation_and_movement() {
        let mut profile = WorkerLatency::new(2, 2).unwrap();
        profile.start(1, "move", Some(3.0), 0.0);
        profile.start(2, "trace", None, 1.0);
        let started = profile.finish(1, Some(2.0), false, 5.0).unwrap();
        profile.mark_applied(started, 5.0);
        profile.finish(2, Some(3.0), true, 8.0);
        profile.mark_submitted(8.0);
        let report = profile.snapshot();
        assert_eq!(report["operations"]["move"]["round_trip_ms"]["p50"], 5.0);
        assert_eq!(report["operations"]["move"]["worker_ms"]["p50"], 2.0);
        assert_eq!(report["operations"]["move"]["batch_size"]["p50"], 3.0);
        assert_eq!(report["operations"]["trace"]["batch_size"], Value::Null);
        assert_eq!(report["operations"]["trace"]["errors"], 1);
        assert_eq!(report["movement_submit"]["request_to_submit_ms"]["p50"], 8.0);
        assert_eq!(report["movement_submit"]["applied_to_submit_ms"]["p50"], 3.0);
        assert_eq!(report["movement_submit"]["pending"], 0);
        assert_eq!(report["pending"], 0);
        for id in [3, 4, 5] {
            profile.start(id, "move", None, 8.0);
        }
        assert_eq!(profile.snapshot()["evicted"], 1);
        profile.finish(3, Some(0.0), false, 9.0); // evicted: ignored
        profile.finish(4, None, false, 10.0);
        profile.finish(5, Some(0.5), false, 11.0);
        let report = profile.snapshot();
        assert_eq!(report["operations"]["move"]["completed"], 3);
        assert_eq!(report["operations"]["move"]["round_trip_ms"]["samples"], 2);
        assert_eq!(report["operations"]["move"]["worker_ms"]["samples"], 2);
        assert_eq!(report["pending"], 0);
        for _ in 0..3 {
            profile.mark_applied(9.0, 12.0);
        }
        assert_eq!(profile.snapshot()["movement_submit"]["evicted"], 1);
        profile.clear_applied();
        assert_eq!(profile.snapshot()["movement_submit"]["pending"], 0);
        assert!(WorkerLatency::new(0, 2).is_err());
        assert!(WorkerLatency::new(2, 0).is_err());
    }
}
