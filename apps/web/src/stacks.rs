//! Opt-in CPU stack capture over gameplay frames with the browser's JS Self-Profiling API. The warm-up, frame
//! records and summary are `countersplat_core::frame_stacks`; this owns the browser profiler and clock.

use countersplat_core::frame_stacks::{self, Begin, Capture, CapturedFrame, LuaSnapshot};
use js_sys::{Function, Reflect, JSON};
use serde_json::{json, Value};
use std::{cell::RefCell, rc::Rc};
use wasm_bindgen::{prelude::*, JsCast};
use wasm_bindgen_futures::{spawn_local, JsFuture};

#[wasm_bindgen]
extern "C" {
    /// `Profiler` from the JS Self-Profiling API (needs the `Document-Policy: js-profiling` header).
    type Profiler;
    #[wasm_bindgen(catch, constructor)]
    fn new(options: &JsValue) -> Result<Profiler, JsValue>;
    #[wasm_bindgen(method)]
    fn stop(this: &Profiler) -> js_sys::Promise;
    #[wasm_bindgen(method, getter, js_name = sampleInterval)]
    fn sample_interval(this: &Profiler) -> JsValue;
}

fn to_json(value: &JsValue) -> Value {
    if value.is_undefined() || value.is_null() {
        return Value::Null;
    }
    JSON::stringify(value).ok().and_then(|t| t.as_string()).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null)
}

fn to_js(value: &Value) -> JsValue {
    JSON::parse(&value.to_string()).unwrap_or(JsValue::NULL)
}

fn now() -> f64 {
    web_sys::window().and_then(|w| w.performance()).map_or(0.0, |p| p.now())
}

fn lua_report(lua: &JsValue) -> Value {
    let report = Reflect::get(lua, &"profileReport".into())
        .ok()
        .and_then(|f| f.dyn_into::<Function>().ok())
        .and_then(|f| f.call0(lua).ok())
        .unwrap_or(JsValue::NULL);
    to_json(&report)
}

struct Pending {
    start: f64,
    before: Option<LuaSnapshot>,
}

struct State {
    capture: Capture,
    profiler: Option<Profiler>,
    pending: Option<Pending>,
    on_complete: Function,
    gpu_timing: Function,
}

/// One-shot capture: warms up for 120 gameplay frames, records 180, then calls `on_complete(summary, raw)` with
/// raw `{trace, frames}` (or `({error}, null)`). `gpu_timing()` returns the viewer's GPU timing JSON or null; the
/// summary's selected frame is the one matching its median GPU sample.
#[wasm_bindgen]
pub struct FrameStackCapture(Rc<RefCell<State>>);

#[wasm_bindgen]
impl FrameStackCapture {
    #[wasm_bindgen(constructor)]
    pub fn new(on_complete: Function, gpu_timing: Function, detail_every: usize) -> Self {
        Self(Rc::new(RefCell::new(State {
            capture: Capture::new(120, 180, detail_every),
            profiler: None,
            pending: None,
            on_complete,
            gpu_timing,
        })))
    }

    /// Starts a frame; `active` is "gameplay with Lua", `lua` the gameplay Lua scene (for its profile report).
    #[wasm_bindgen(js_name = beginFrame)]
    pub fn begin_frame(&self, active: bool, lua: JsValue) -> bool {
        let mut s = self.0.borrow_mut();
        s.pending = None;
        let detail = match s.capture.begin(active && !lua.is_null() && !lua.is_undefined()) {
            Begin::Skip => return false,
            Begin::Cancel => {
                if let Some(profiler) = s.profiler.take() {
                    let stopped = profiler.stop();
                    spawn_local(async move {
                        let _ = JsFuture::from(stopped).await;
                    });
                }
                return false;
            }
            Begin::Start { detail } => {
                let options = to_js(&json!({"sampleInterval": 1, "maxBufferSize": 10000}));
                match Profiler::new(&options) {
                    Ok(profiler) => s.profiler = Some(profiler),
                    Err(error) => {
                        s.capture.fail();
                        let message = error.as_string().or_else(|| js_sys::Error::from(error).to_string().as_string());
                        let _ = s.on_complete.call2(&JsValue::NULL, &to_js(&json!({"error": message})), &JsValue::NULL);
                        return false;
                    }
                }
                detail
            }
            Begin::Capture { detail } => detail,
        };
        let before = detail.then(|| frame_stacks::snapshot_lua(&lua_report(&lua)));
        s.pending = Some(Pending { start: now(), before });
        true
    }

    /// Whether the frame in progress records detailed timings (Lua, stages and the renderer's own timing).
    #[wasm_bindgen(getter)]
    pub fn detailed(&self) -> bool {
        self.0.borrow().pending.as_ref().is_some_and(|p| p.before.is_some())
    }

    /// Ends the frame started by `beginFrame` with its timings; stops the profiler when the capture is full.
    #[allow(clippy::too_many_arguments)]
    #[wasm_bindgen(js_name = endFrame)]
    pub fn end_frame(
        &self,
        submitted: bool,
        render_ms: f64,
        scripts_ms: f64,
        stages: JsValue,
        lua: JsValue,
        rust_timing: JsValue,
        animation_timestamp: Option<f64>,
    ) {
        let mut s = self.0.borrow_mut();
        let Some(pending) = s.pending.take() else {
            return;
        };
        if s.profiler.is_none() {
            return;
        }
        let end = now();
        let details = pending.before.map(|before| {
            json!({"render_cpu_ms": render_ms, "scripts_cpu_ms": scripts_ms, "stages": to_json(&stages),
                "rust": to_json(&rust_timing),
                "lua": frame_stacks::lua_delta(&before, &frame_stacks::snapshot_lua(&lua_report(&lua)))})
        });
        let frame = CapturedFrame { start: pending.start, end, submitted, details, animation_timestamp_ms: animation_timestamp };
        if !s.capture.end(frame) {
            return;
        }
        let Some(profiler) = s.profiler.take() else {
            return;
        };
        let state = self.0.clone();
        drop(s);
        spawn_local(async move {
            let interval = profiler.sample_interval().as_f64();
            let result = JsFuture::from(profiler.stop()).await;
            let s = state.borrow();
            let (summary, raw) = match result {
                Ok(trace) => {
                    let trace = to_json(&trace);
                    let frames = &s.capture.frames;
                    let gpu = s.gpu_timing.call0(&JsValue::NULL).ok().and_then(|v| v.as_string()).and_then(|t| serde_json::from_str::<Value>(&t).ok());
                    let selected = gpu.as_ref().and_then(|g| frame_stacks::median_gpu_sample(&g["samples"]));
                    let preferred = selected.as_ref().and_then(|v| v["frame_timestamp_ms"].as_f64());
                    let mut summary = frame_stacks::summarize(&trace, frames, 1.0, interval, preferred);
                    summary["gpu"] = match gpu {
                        Some(Value::Object(mut gpu)) => {
                            gpu.insert("selected".into(), selected.unwrap_or(Value::Null));
                            Value::Object(gpu)
                        }
                        _ => Value::Null,
                    };
                    let frames: Vec<Value> = frames.iter().map(CapturedFrame::to_json).collect();
                    (summary, json!({"trace": trace, "frames": frames}))
                }
                Err(error) => (json!({"error": format!("{error:?}")}), Value::Null),
            };
            let on_complete = s.on_complete.clone();
            drop(s);
            let _ = on_complete.call2(&JsValue::NULL, &to_js(&summary), &to_js(&raw));
        });
    }
}
