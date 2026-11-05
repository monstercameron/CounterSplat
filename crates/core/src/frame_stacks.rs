//! Summaries of an opt-in CPU sampling capture (the browser's JS Self-Profiling trace) over a run of frames: picks a
//! representative submitted frame, its top sampled stacks and per-frame Lua deltas. Sample counts are not exact
//! per-function durations, and GPU execution and other threads are not measured.

use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, HashMap, HashSet};

const PROFILE_GROUPS: [&str; 2] = ["systems", "operations"];

/// One captured frame (CPU interval) with optional detailed timing.
#[derive(Debug, Clone, PartialEq)]
pub struct CapturedFrame {
    pub start: f64,
    pub end: f64,
    pub submitted: bool,
    /// Detailed timing for every `detail_every`-th frame (object), otherwise `None`.
    pub details: Option<Value>,
    pub animation_timestamp_ms: Option<f64>,
}

impl CapturedFrame {
    pub fn to_json(&self) -> Value {
        json!({"start": self.start, "end": self.end, "submitted": self.submitted, "details": self.details,
            "animation_timestamp_ms": self.animation_timestamp_ms})
    }

    pub fn from_json(value: &Value) -> Option<Self> {
        Some(Self {
            start: value.get("start")?.as_f64()?,
            end: value.get("end")?.as_f64()?,
            submitted: value.get("submitted").and_then(Value::as_bool).unwrap_or(false),
            details: value.get("details").filter(|d| d.is_object()).cloned(),
            animation_timestamp_ms: value.get("animation_timestamp_ms").and_then(Value::as_f64),
        })
    }

    fn cpu_ms(&self) -> f64 {
        self.end - self.start
    }
}

/// Counts and totals of a Lua profile report's groups.
pub type LuaSnapshot = BTreeMap<&'static str, BTreeMap<String, (f64, f64)>>;

pub fn snapshot_lua(profile: &Value) -> LuaSnapshot {
    PROFILE_GROUPS
        .iter()
        .map(|group| {
            let entries = profile.get(group).and_then(Value::as_object).map_or_else(BTreeMap::new, |values| {
                values
                    .iter()
                    .map(|(name, value)| {
                        let field = |key| value.get(key).and_then(Value::as_f64).unwrap_or(0.0);
                        (name.clone(), (field("count"), field("total_ms")))
                    })
                    .collect()
            });
            (*group, entries)
        })
        .collect()
}

/// Per-entry Lua work between two snapshots, busiest first; entries that did not run are omitted.
pub fn lua_delta(before: &LuaSnapshot, after: &LuaSnapshot) -> Value {
    let mut result = Map::new();
    for group in PROFILE_GROUPS {
        let empty = BTreeMap::new();
        let old = before.get(group).unwrap_or(&empty);
        let mut entries: Vec<(String, f64, f64)> = after
            .get(group)
            .unwrap_or(&empty)
            .iter()
            .map(|(name, (count, total))| {
                let (old_count, old_total) = old.get(name).copied().unwrap_or((0.0, 0.0));
                (name.clone(), count - old_count, (total - old_total).max(0.0))
            })
            .filter(|(_, count, _)| *count > 0.0)
            .collect();
        entries.sort_by(|a, b| b.2.total_cmp(&a.2));
        result.insert(
            group.into(),
            entries.into_iter().map(|(name, count, total_ms)| json!({"name": name, "count": count, "total_ms": total_ms})).collect(),
        );
    }
    Value::Object(result)
}

fn array<'a>(trace: &'a Value, key: &str) -> &'a [Value] {
    trace.get(key).and_then(Value::as_array).map_or(&[], Vec::as_slice)
}

fn stack_frames(trace: &Value, sample: &Value) -> Vec<Value> {
    let (stacks, frames, resources) = (array(trace, "stacks"), array(trace, "frames"), array(trace, "resources"));
    let mut result = Vec::new();
    let mut seen = HashSet::new();
    let mut id = sample.get("stackId").and_then(Value::as_u64);
    while let Some(index) = id.map(|id| id as usize).filter(|&i| i < stacks.len() && seen.insert(i)) {
        let stack = &stacks[index];
        let Some(frame) = stack.get("frameId").and_then(Value::as_u64).and_then(|f| frames.get(f as usize)) else {
            break;
        };
        let resource = frame.get("resourceId").and_then(Value::as_u64).and_then(|r| resources.get(r as usize)).cloned();
        let name = frame.get("name").and_then(Value::as_str).filter(|n| !n.is_empty()).unwrap_or("(anonymous)");
        result.push(json!({"name": name, "resource": resource, "line": frame.get("line").cloned().unwrap_or(Value::Null),
            "column": frame.get("column").cloned().unwrap_or(Value::Null)}));
        id = stack.get("parentId").and_then(Value::as_u64);
    }
    result.reverse();
    result
}

fn top_stacks<'a>(trace: &Value, samples: impl Iterator<Item = &'a Value>) -> Value {
    let mut order: Vec<(usize, Vec<Value>)> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    for sample in samples {
        let frames = stack_frames(trace, sample);
        if frames.is_empty() {
            continue;
        }
        let key = Value::Array(frames.clone()).to_string();
        match index.get(&key) {
            Some(&at) => order[at].0 += 1,
            None => {
                index.insert(key, order.len());
                order.push((1, frames));
            }
        }
    }
    // Stable: equal counts keep first-seen order.
    order.sort_by(|a, b| b.0.cmp(&a.0));
    order.into_iter().take(20).map(|(count, frames)| json!({"count": count, "frames": frames})).collect()
}

/// The median-GPU-time sample with a frame timestamp (`{gpu_ms, frame_timestamp_ms}`), without reordering the input.
pub fn median_gpu_sample(samples: &Value) -> Option<Value> {
    let mut valid: Vec<&Value> = samples
        .as_array()?
        .iter()
        .filter(|s| {
            s.get("gpu_ms").and_then(Value::as_f64).is_some_and(|g| g.is_finite() && g >= 0.0)
                && s.get("frame_timestamp_ms").and_then(Value::as_f64).is_some_and(f64::is_finite)
        })
        .collect();
    valid.sort_by(|a, b| a["gpu_ms"].as_f64().unwrap_or(0.0).total_cmp(&b["gpu_ms"].as_f64().unwrap_or(0.0)));
    valid.get(valid.len() / 2).map(|s| (*s).clone())
}

/// Summary of a capture. The selected frame is the one whose animation timestamp is `preferred_timestamp` (to match a
/// GPU-timed frame) or otherwise a detailed, submitted frame closest to the median submitted CPU time.
pub fn summarize(
    trace: &Value,
    frames: &[CapturedFrame],
    requested_interval_ms: f64,
    actual_interval_ms: Option<f64>,
    preferred_timestamp: Option<f64>,
) -> Value {
    let samples = array(trace, "samples");
    let timestamp = |s: &Value| s.get("timestamp").and_then(Value::as_f64).unwrap_or(f64::NAN);
    let by_frame: Vec<Vec<&Value>> = frames
        .iter()
        .map(|f| samples.iter().filter(|s| timestamp(s) >= f.start && timestamp(s) < f.end).collect())
        .collect();
    let submitted: Vec<&CapturedFrame> = frames.iter().filter(|f| f.submitted).collect();
    let mut durations: Vec<f64> =
        if submitted.is_empty() { frames.iter().collect::<Vec<_>>() } else { submitted }.iter().map(|f| f.cpu_ms()).collect();
    durations.sort_by(f64::total_cmp);
    let median = durations.get(durations.len() / 2).copied().unwrap_or(0.0);
    let candidates = |keep: &dyn Fn(usize) -> bool| -> Vec<usize> { (0..frames.len()).filter(|&i| keep(i)).collect() };
    let detailed = |i: usize| frames[i].details.is_some();
    let mut chosen = candidates(&|i| detailed(i) && frames[i].submitted && by_frame[i].len() >= 2);
    if chosen.is_empty() {
        chosen = candidates(&|i| detailed(i) && frames[i].submitted && !by_frame[i].is_empty());
    }
    if chosen.is_empty() {
        chosen = candidates(&detailed);
    }
    chosen.sort_by(|&a, &b| (frames[a].cpu_ms() - median).abs().total_cmp(&(frames[b].cpu_ms() - median).abs()));
    let matching = preferred_timestamp.and_then(|t| {
        frames.iter().position(|f| f.animation_timestamp_ms == Some(t) && f.details.is_some() && f.submitted)
    });
    let selected = matching.or(chosen.first().copied()).map(|index| {
        let frame = &frames[index];
        let mut out = Map::new();
        out.insert("index".into(), index.into());
        out.insert("cpu_ms".into(), frame.cpu_ms().into());
        out.insert("submitted".into(), frame.submitted.into());
        if let Some(Value::Object(details)) = &frame.details {
            out.extend(details.clone());
        }
        out.insert("sample_count".into(), by_frame[index].len().into());
        out.insert("stacks".into(), top_stacks(trace, by_frame[index].iter().copied()));
        Value::Object(out)
    });
    json!({
        "requested_interval_ms": requested_interval_ms,
        "actual_interval_ms": actual_interval_ms,
        "captured_frames": frames.len(),
        "captured_samples": samples.len(),
        "samples_within_frames": by_frame.iter().map(Vec::len).sum::<usize>(),
        "median_submitted_cpu_ms": median,
        "selected_frame": selected,
        "aggregate_stacks": top_stacks(trace, by_frame.iter().flatten().copied()),
    })
}

/// What a frame's capture step asks the host to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Begin {
    /// Not capturing this frame.
    Skip,
    /// Stop and discard a running profiler (gameplay ended before the capture filled).
    Cancel,
    /// Start the profiler, then capture this frame.
    Start { detail: bool },
    /// Capture this frame (`detail`: also snapshot Lua and stage timings).
    Capture { detail: bool },
}

/// Warm-up, capture length and detail cadence of a one-shot capture; the host owns the profiler and clock.
#[derive(Debug, Clone)]
pub struct Capture {
    pub warm_frames: u32,
    pub capture_frames: usize,
    pub detail_every: usize,
    warm: u32,
    running: bool,
    done: bool,
    pub frames: Vec<CapturedFrame>,
}

impl Capture {
    pub fn new(warm_frames: u32, capture_frames: usize, detail_every: usize) -> Self {
        Self { warm_frames, capture_frames: capture_frames.max(1), detail_every: detail_every.max(1), warm: 0, running: false, done: false, frames: Vec::new() }
    }

    pub fn done(&self) -> bool {
        self.done
    }

    /// Called at the start of a frame; `active` is "gameplay with Lua is running".
    pub fn begin(&mut self, active: bool) -> Begin {
        if self.done {
            return Begin::Skip;
        }
        if !active {
            self.warm = 0;
            if std::mem::take(&mut self.running) {
                self.frames.clear();
                return Begin::Cancel;
            }
            return Begin::Skip;
        }
        let detail = self.frames.len() % self.detail_every == 0;
        if self.running {
            return Begin::Capture { detail };
        }
        self.warm += 1;
        if self.warm < self.warm_frames {
            return Begin::Skip;
        }
        self.running = true;
        Begin::Start { detail }
    }

    /// The profiler could not start; the capture ends with an error.
    pub fn fail(&mut self) {
        self.running = false;
        self.done = true;
    }

    /// Records a captured frame; true when the capture is full and the profiler should stop.
    pub fn end(&mut self, frame: CapturedFrame) -> bool {
        if !self.running || self.done {
            return false;
        }
        self.frames.push(frame);
        if self.frames.len() < self.capture_frames {
            return false;
        }
        self.running = false;
        self.done = true;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(start: f64, end: f64, submitted: bool, render: f64, ts: f64) -> CapturedFrame {
        CapturedFrame { start, end, submitted, details: Some(json!({"render_cpu_ms": render})), animation_timestamp_ms: Some(ts) }
    }

    #[test]
    fn summary_selects_representative_frame_and_stacks() {
        let trace = json!({
            "resources": ["local.js"],
            "frames": [{"name":"tick","resourceId":0,"line":1}, {"name":"render","resourceId":0,"line":10}, {"name":"script","resourceId":0,"line":20}],
            "stacks": [{"frameId":0}, {"frameId":1,"parentId":0}, {"frameId":2,"parentId":0}],
            "samples": [{"timestamp":0.5,"stackId":1}, {"timestamp":3.2,"stackId":1}, {"timestamp":3.8,"stackId":1},
                {"timestamp":6.5,"stackId":2}, {"timestamp":10,"stackId":2}]
        });
        let frames = [frame(0.0, 2.0, true, 1.0, 100.0), frame(3.0, 7.0, true, 2.0, 200.0), frame(8.0, 14.0, false, 3.0, 300.0)];
        let result = summarize(&trace, &frames, 1.0, Some(16.0), None);
        assert_eq!(result["actual_interval_ms"], 16.0);
        assert_eq!(result["captured_frames"], 3);
        assert_eq!(result["samples_within_frames"], 5);
        let selected = &result["selected_frame"];
        assert_eq!(selected["index"], 1);
        assert_eq!(selected["render_cpu_ms"], 2.0);
        assert_eq!(selected["sample_count"], 3);
        assert_eq!(selected["stacks"][0]["count"], 2);
        let names: Vec<&str> = selected["stacks"][0]["frames"].as_array().unwrap().iter().map(|f| f["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["tick", "render"]);
        let gpu = summarize(&trace, &frames, 1.0, Some(16.0), Some(100.0));
        assert_eq!(gpu["selected_frame"]["index"], 0);
        assert_eq!(gpu["selected_frame"]["sample_count"], 1);
        // An unsubmitted frame cannot be the GPU match.
        assert_eq!(summarize(&trace, &frames, 1.0, Some(16.0), Some(300.0))["selected_frame"]["index"], 1);
        let empty = json!({"resources":[],"frames":[],"stacks":[],"samples":[]});
        assert!(summarize(&empty, &[], 1.0, None, None)["selected_frame"].is_null());
    }

    #[test]
    fn median_gpu_sample_ignores_invalid() {
        let samples = json!([{"gpu_ms":3,"frame_timestamp_ms":1}, {"gpu_ms":1,"frame_timestamp_ms":2},
            {"gpu_ms":2,"frame_timestamp_ms":3}, {"gpu_ms":null,"frame_timestamp_ms":4}]);
        assert_eq!(median_gpu_sample(&samples).unwrap()["frame_timestamp_ms"], 3);
        assert_eq!(samples[0]["gpu_ms"], 3);
        assert!(median_gpu_sample(&json!([])).is_none());
    }

    #[test]
    fn lua_delta_reports_work_between_snapshots() {
        let before = snapshot_lua(&json!({"systems":{"camera":{"count":0,"total_ms":0}},"operations":{}}));
        let after = snapshot_lua(&json!({"systems":{"camera":{"count":1,"total_ms":0.25},"idle":{"count":0,"total_ms":0}},"operations":{}}));
        let delta = lua_delta(&before, &after);
        assert_eq!(delta["systems"], json!([{"name":"camera","count":1.0,"total_ms":0.25}]));
        assert_eq!(delta["operations"], json!([]));
    }

    #[test]
    fn capture_warms_up_fills_and_cancels() {
        let mut capture = Capture::new(2, 2, 1);
        assert_eq!(capture.begin(true), Begin::Skip);
        assert_eq!(capture.begin(true), Begin::Start { detail: true });
        assert!(!capture.end(frame(0.0, 1.0, true, 0.0, 0.0)));
        assert_eq!(capture.begin(true), Begin::Capture { detail: true });
        assert!(capture.end(frame(1.0, 2.0, true, 0.0, 1.0)));
        assert_eq!(capture.frames.len(), 2);
        assert_eq!(capture.begin(true), Begin::Skip, "one-shot");

        let mut cancelled = Capture::new(1, 180, 10);
        assert_eq!(cancelled.begin(true), Begin::Start { detail: true });
        cancelled.end(frame(0.0, 1.0, true, 0.0, 0.0));
        assert_eq!(cancelled.begin(true), Begin::Capture { detail: false });
        assert_eq!(cancelled.begin(false), Begin::Cancel);
        assert!(cancelled.frames.is_empty());
        assert_eq!(cancelled.begin(false), Begin::Skip);

        let mut failed = Capture::new(1, 2, 1);
        assert_eq!(failed.begin(true), Begin::Start { detail: true });
        failed.fail();
        assert!(failed.done() && failed.begin(true) == Begin::Skip);
    }
}
