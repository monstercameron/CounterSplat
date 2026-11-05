//! Level voxel collision in the browser. A dedicated worker owns the sparse collision data and its CPU queries,
//! separate from the WebGPU owner; the Viewer coordinates per-entity ECS bodies with it
//! (`countersplat_scene::voxel_host`). The worker's only JavaScript is its bootstrap (`voxel-worker.js`).

use crate::{js_error, Viewer, VoxelCollisionData};
use countersplat_core::profile;
use countersplat_scene::voxel_host::{self, Host, Reply, VoxelRuntime};
use js_sys::{Array, Float32Array, Object, Reflect, JSON};
use std::{cell::RefCell, collections::VecDeque, rc::Rc};
use wasm_bindgen::{prelude::*, JsCast};
use wasm_bindgen_futures::{spawn_local, JsFuture};
use web_sys::{DedicatedWorkerGlobalScope, MessageEvent, RequestCache, RequestInit, Response, Worker, WorkerGlobalScope};

const MIB: f64 = 1024.0 * 1024.0;

fn get(object: &JsValue, key: &str) -> JsValue {
    Reflect::get(object, &JsValue::from_str(key)).unwrap_or(JsValue::UNDEFINED)
}

fn set(object: &Object, key: &str, value: &JsValue) {
    let _ = Reflect::set(object, &JsValue::from_str(key), value);
}

fn message_of(error: &JsValue) -> String {
    error
        .as_string()
        .or_else(|| error.dyn_ref::<js_sys::Error>().map(|e| format!("Error: {}", String::from(e.message()))))
        .unwrap_or_else(|| format!("{error:?}"))
}

fn to_json(value: &JsValue) -> serde_json::Value {
    JSON::stringify(value).ok().and_then(|t| t.as_string()).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(serde_json::Value::Null)
}

fn floats(value: &JsValue) -> Option<Vec<f32>> {
    Array::is_array(value).then(|| Array::from(value).iter().map(|v| v.as_f64().unwrap_or(f64::NAN) as f32).collect())
}

// ---------------------------------------------------------------------------------------------- worker side

thread_local! {
    static COLLIDER: RefCell<Option<VoxelCollisionData>> = const { RefCell::new(None) };
}

fn worker_now() -> f64 {
    js_sys::global().unchecked_into::<WorkerGlobalScope>().performance().map_or(0.0, |p| p.now())
}

async fn fetch(url: &str) -> Result<Response, JsValue> {
    let scope: WorkerGlobalScope = js_sys::global().unchecked_into();
    let init = RequestInit::new();
    init.set_cache(RequestCache::NoStore);
    Ok(JsFuture::from(scope.fetch_with_str_and_init(url, &init)).await?.unchecked_into())
}

async fn body(response: Response, limit: f64, what: &str) -> Result<Vec<u8>, JsValue> {
    if !response.ok() {
        return Err(js_error(format!("{what} HTTP {}", response.status())));
    }
    let declared = response.headers().get("content-length").ok().flatten().and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
    if declared > limit {
        return Err(js_error(format!("{what} exceeds {} MiB", limit / MIB)));
    }
    let bytes = js_sys::Uint8Array::new(&JsFuture::from(response.array_buffer()?).await?);
    if f64::from(bytes.length()) > limit {
        return Err(js_error(format!("{what} exceeds {} MiB", limit / MIB)));
    }
    Ok(bytes.to_vec())
}

/// The reply to one worker request and the buffers it transfers.
async fn answer(data: &JsValue) -> Result<Option<(Object, Option<Array>)>, JsValue> {
    let reply = Object::new();
    let kind = get(data, "type").as_string().unwrap_or_default();
    if kind == "load" {
        let base = get(data, "base").as_string().unwrap_or_default();
        let metadata = fetch(&web_sys::Url::new_with_base("scene.voxel.json", &base)?.href()).await?;
        if metadata.status() == 404 {
            set(&reply, "type", &"unavailable".into());
            return Ok(Some((reply, None)));
        }
        let info = body(metadata, MIB, "voxel metadata").await?;
        let bytes = body(fetch(&web_sys::Url::new_with_base("scene.voxel.bin", &base)?.href()).await?, 128.0 * MIB, "voxel binary").await?;
        let collider = VoxelCollisionData::new(info, bytes)?;
        set(&reply, "type", &"ready".into());
        set(&reply, "resolution", &f64::from(collider.resolution()).into());
        COLLIDER.with(|c| *c.borrow_mut() = Some(collider));
        return Ok(Some((reply, None)));
    }
    COLLIDER.with(|collider| {
        let collider = collider.borrow();
        let Some(collider) = collider.as_ref() else {
            return Ok(None);
        };
        let vector = |key: &str| floats(&get(data, key)).ok_or_else(|| js_error(format!("expected {key}")));
        let number = |key: &str| get(data, key).as_f64().unwrap_or(f64::NAN) as f32;
        set(&reply, "type", &kind.as_str().into());
        set(&reply, "id", &get(data, "id"));
        let mut transfer = None;
        match kind.as_str() {
            "move" => {
                let half_height = get(data, "halfHeight").as_f64().unwrap_or(0.0) as f32;
                let mut position = vector("position")?;
                for delta in Array::from(&get(data, "deltas")).iter() {
                    let delta = floats(&delta).ok_or_else(|| js_error("expected a 3D movement"))?;
                    position = collider.move_capsule(position, delta, number("radius"), half_height)?;
                }
                let position: Array = position.iter().map(|&v| JsValue::from(f64::from(v))).collect();
                set(&reply, "position", &position);
            }
            "trace" => {
                let hit: Array = collider.trace_segment(vector("start")?, vector("end")?)?.iter().map(|&v| JsValue::from(f64::from(v))).collect();
                set(&reply, "hit", &hit);
            }
            "clearance" => {
                let clear = collider.capsule_clear(vector("center")?, number("radius"), number("halfHeight"))?;
                set(&reply, "clear", &clear.into());
            }
            "ground" => set(&reply, "height", &collider.ground_height(vector("position")?, number("radius"), number("maxDrop"))?),
            "debug" => {
                let limit = get(data, "limit").as_f64().unwrap_or(0.0) as usize;
                let points = Float32Array::from(collider.nearby(vector("position")?, number("radius"), limit)?.as_slice());
                transfer = Some(Array::of1(&points.buffer()));
                set(&reply, "points", &points);
            }
            _ => return Ok(None),
        }
        Ok(Some((reply, transfer)))
    })
}

/// Handles one message in the voxel worker. Timing is added only for the local profiler (`profile: true`); the
/// measured span includes the query and result packing, not time queued on either side.
#[wasm_bindgen]
pub fn voxel_worker_handle(data: JsValue) {
    spawn_local(async move {
        let scope: DedicatedWorkerGlobalScope = js_sys::global().unchecked_into();
        let started = (get(&data, "profile").as_bool() == Some(true)).then(worker_now);
        let (reply, transfer) = match answer(&data).await {
            Ok(Some(reply)) => reply,
            Ok(None) => return,
            Err(error) => {
                let failure = Object::new();
                set(&failure, "type", &"error".into());
                set(&failure, "operation", &get(&data, "type"));
                set(&failure, "id", &get(&data, "id"));
                set(&failure, "message", &message_of(&error).into());
                (failure, None)
            }
        };
        if let Some(started) = started {
            set(&reply, "worker_ms", &(worker_now() - started).max(0.0).into());
        }
        let _ = match transfer {
            Some(transfer) => scope.post_message_with_transfer(&reply, &transfer),
            None => scope.post_message(&reply),
        };
    });
}

// ------------------------------------------------------------------------------------------------ page side

enum Inbound {
    Reply(Reply),
    Failed(String),
}

/// The gameplay viewer's collision worker, its unapplied replies and the coordinator.
pub(crate) struct VoxelLink {
    worker: Worker,
    inbox: Rc<RefCell<VecDeque<Inbound>>>,
    runtime: VoxelRuntime,
    latency: Option<profile::WorkerLatency>,
    _on_message: Closure<dyn FnMut(MessageEvent)>,
    _on_error: Closure<dyn FnMut(web_sys::ErrorEvent)>,
}

impl Drop for VoxelLink {
    fn drop(&mut self) {
        self.worker.set_onmessage(None);
        self.worker.set_onerror(None);
        self.worker.terminate();
    }
}

fn page_now() -> f64 {
    web_sys::window().and_then(|w| w.performance()).map_or(0.0, |p| p.now())
}

/// The Viewer parts the coordinator uses.
struct Bridge<'a> {
    ui: &'a mut crate::ui_layer::SceneUi,
    debug: &'a mut countersplat_render::voxel_debug::VoxelDebug,
    queue: &'a wgpu::Queue,
    worker: &'a Worker,
    latency: &'a mut Option<profile::WorkerLatency>,
}

impl Host for Bridge<'_> {
    fn ecs(&mut self, operation: serde_json::Value) -> Result<serde_json::Value, String> {
        voxel_host::reply_value(&self.ui.script_operation(&operation.to_string())?)
    }
    fn send(&mut self, message: serde_json::Value) {
        if let Ok(message) = JSON::parse(&message.to_string()) {
            let _ = self.worker.post_message(&message);
        }
    }
    fn set_debug(&mut self, points: &[f32], size: f32) -> Result<(), String> {
        self.debug.set(self.queue, points, size).map_err(|e| e.to_string())
    }
    fn clear_debug(&mut self) {
        self.debug.clear();
    }
    fn warn(&mut self, message: &str) {
        web_sys::console::warn_1(&message.into());
    }
    fn latency_start(&mut self, id: u64, operation: &str, batch: Option<usize>) -> bool {
        let Some(latency) = self.latency.as_mut() else {
            return false;
        };
        latency.start(id as i64, operation, batch.map(|b| b as f64), page_now());
        true
    }
    fn latency_finish(&mut self, id: u64, worker_ms: Option<f64>, failed: bool) -> Option<f64> {
        self.latency.as_mut()?.finish(id as i64, worker_ms, failed, page_now())
    }
    fn latency_applied(&mut self, started: Option<f64>) {
        if let (Some(latency), Some(started)) = (self.latency.as_mut(), started) {
            latency.mark_applied(started, page_now());
        }
    }
    fn latency_clear(&mut self) {
        if let Some(latency) = self.latency.as_mut() {
            latency.clear_applied();
        }
    }
}

impl Viewer {
    /// Runs `f` with the coordinator and its host, after applying the worker replies received so far.
    fn with_voxels<T>(&mut self, f: impl FnOnce(&mut VoxelRuntime, &mut Bridge) -> T) -> Option<T> {
        let link = self.voxels.as_mut()?;
        let mut bridge = Bridge { ui: &mut self.ui, debug: &mut self.voxel_debug, queue: &self.queue, worker: &link.worker, latency: &mut link.latency };
        loop {
            let next = link.inbox.borrow_mut().pop_front();
            match next {
                Some(Inbound::Reply(reply)) => link.runtime.receive(&mut bridge, reply),
                Some(Inbound::Failed(message)) => link.runtime.fail(&mut bridge, &message),
                None => break,
            }
        }
        Some(f(&mut link.runtime, &mut bridge))
    }
}

#[wasm_bindgen]
impl Viewer {
    /// Starts level collision for this gameplay scene: a worker (`worker_url`, the bootstrap) loads the voxels
    /// under `base`. Fails when the scene has no level or player entity.
    pub fn start_voxels(&mut self, worker_url: &str, base: &str) -> Result<(), JsValue> {
        self.voxels = None;
        let options = web_sys::WorkerOptions::new();
        options.set_type(web_sys::WorkerType::Module);
        let worker = Worker::new_with_options(worker_url, &options)?;
        let inbox: Rc<RefCell<VecDeque<Inbound>>> = Rc::default();
        let on_message = {
            let inbox = inbox.clone();
            Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
                let data = event.data();
                // Debug points are a typed array; take them before the JSON copy.
                let points = get(&data, "points");
                let points = points.is_instance_of::<Float32Array>().then(|| {
                    let _ = Reflect::delete_property(data.unchecked_ref::<Object>(), &"points".into());
                    Float32Array::new(&points).to_vec()
                });
                inbox.borrow_mut().push_back(Inbound::Reply(Reply { data: to_json(&data), points }));
            })
        };
        let on_error = {
            let inbox = inbox.clone();
            Closure::<dyn FnMut(web_sys::ErrorEvent)>::new(move |event: web_sys::ErrorEvent| {
                let message = event.message();
                inbox.borrow_mut().push_back(Inbound::Failed(if message.is_empty() { "Voxel worker failed.".into() } else { message }));
            })
        };
        worker.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
        worker.set_onerror(Some(on_error.as_ref().unchecked_ref()));
        let mut latency = None;
        let mut bridge = Bridge { ui: &mut self.ui, debug: &mut self.voxel_debug, queue: &self.queue, worker: &worker, latency: &mut latency };
        let runtime = VoxelRuntime::new(&mut bridge, base).map_err(js_error)?;
        self.voxels = Some(VoxelLink { worker, inbox, runtime, latency, _on_message: on_message, _on_error: on_error });
        Ok(())
    }

    /// Times worker requests for the local profiler (`voxel_latency_report`).
    pub fn voxel_enable_latency(&mut self) -> Result<(), JsValue> {
        if let Some(link) = self.voxels.as_mut() {
            link.latency = Some(profile::WorkerLatency::new(600, 512).map_err(js_error)?);
        }
        Ok(())
    }

    /// The worker latency profile, or null.
    pub fn voxel_latency_report(&self) -> JsValue {
        self.voxels
            .as_ref()
            .and_then(|link| link.latency.as_ref())
            .and_then(|latency| JSON::parse(&latency.snapshot().to_string()).ok())
            .unwrap_or(JsValue::NULL)
    }

    /// A gameplay frame was submitted (latency profile).
    pub fn voxel_mark_frame(&mut self) {
        if let Some(latency) = self.voxels.as_mut().and_then(|link| link.latency.as_mut()) {
            latency.mark_submitted(page_now());
        }
    }

    /// Applies the collision replies received since the last call; call at the start of each frame.
    pub fn voxel_receive(&mut self) {
        self.with_voxels(|_, _| ());
    }

    /// After the frame's scripts: routes their physics queries and movement and updates the debug view.
    pub fn voxel_tick(&mut self, now_ms: f64) {
        self.with_voxels(|runtime, host| runtime.tick(host, now_ms));
    }

    /// Gameplay paused.
    pub fn voxel_suspend(&mut self) {
        self.with_voxels(|runtime, host| runtime.suspend(host));
    }

    /// The scene ended: terminates the worker.
    pub fn voxel_stop(&mut self) {
        if let Some(mut link) = self.voxels.take() {
            let mut bridge = Bridge { ui: &mut self.ui, debug: &mut self.voxel_debug, queue: &self.queue, worker: &link.worker, latency: &mut link.latency };
            link.runtime.stop(&mut bridge);
        }
    }

    /// The scene's player entity while collision runs.
    pub fn voxel_player(&self) -> Option<String> {
        self.voxels.as_ref().map(|link| link.runtime.player.clone())
    }
}
