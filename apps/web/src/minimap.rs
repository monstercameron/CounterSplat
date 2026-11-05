//! Level map in the browser. A dedicated worker owns its own copy of the level's voxels (or, without a collider,
//! its splat centres), so a map request never blocks the collision worker or the render thread. The page side
//! coalesces requests and uploads the finished raster. Rules: `countersplat_scene::minimap`; the worker's only
//! JavaScript is its bootstrap (`minimap-worker.js`).

use crate::{js_error, VoxelCollisionData};
use countersplat_scene::minimap::{self, Coalescer, Request};
use js_sys::{Array, Function, Object, Promise, Reflect, Uint8Array, JSON};
use std::{cell::RefCell, rc::Rc};
use wasm_bindgen::{prelude::*, JsCast};
use wasm_bindgen_futures::{spawn_local, JsFuture};
use web_sys::{DedicatedWorkerGlobalScope, MessageEvent, RequestCache, RequestInit, Response, Worker, WorkerGlobalScope};

const MAP_SPLAT_BUDGET: u64 = 3_000_000;
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
        .or_else(|| error.dyn_ref::<js_sys::Error>().map(|e| String::from(e.message())))
        .unwrap_or_else(|| format!("{error:?}"))
}

fn to_json(value: &JsValue) -> serde_json::Value {
    JSON::stringify(value).ok().and_then(|t| t.as_string()).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(serde_json::Value::Null)
}

fn url(relative: &str, base: &str) -> Result<String, JsValue> {
    Ok(web_sys::Url::new_with_base(relative, base)?.href())
}

// ---------------------------------------------------------------------------------------------- worker side

enum Source {
    Voxels(VoxelCollisionData),
    Splats(Vec<Vec<f32>>),
}

thread_local! {
    static SOURCE: RefCell<Option<Source>> = const { RefCell::new(None) };
}

/// Starts an uncached fetch; the request runs while other work is awaited.
fn start_fetch(url: &str) -> Promise {
    let scope: WorkerGlobalScope = js_sys::global().unchecked_into();
    let init = RequestInit::new();
    init.set_cache(RequestCache::NoStore);
    scope.fetch_with_str_and_init(url, &init)
}

async fn response(fetch: Promise) -> Result<Response, JsValue> {
    Ok(JsFuture::from(fetch).await?.unchecked_into())
}

/// The body of a successful response, refusing anything over `limit` bytes.
async fn body(response: Response, limit: f64, what: &str) -> Result<Vec<u8>, JsValue> {
    if !response.ok() {
        return Err(js_error(format!("{what} HTTP {}", response.status())));
    }
    let declared = response.headers().get("content-length").ok().flatten().and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
    let too_large = || js_error(format!("{what} exceeds size limit"));
    if declared > limit {
        return Err(too_large());
    }
    let bytes = Uint8Array::new(&JsFuture::from(response.array_buffer()?).await?);
    if f64::from(bytes.length()) > limit {
        return Err(too_large());
    }
    Ok(bytes.to_vec())
}

/// Splat centres of the level's finest LOD within the map budget; a missing LOD manifest means a single SOG.
async fn load_splats(base: &str, flip_xy: bool) -> Result<Vec<Vec<f32>>, JsValue> {
    let manifest = response(start_fetch(&url("lod-meta.json", base)?)).await?;
    let manifest = if manifest.status() == 404 {
        None
    } else {
        Some(serde_json::from_slice::<serde_json::Value>(&body(manifest, 16.0 * MIB, "map LOD manifest").await?).map_err(js_error)?)
    };
    let files = minimap::select_splat_map_files(manifest.as_ref()).map_err(js_error)?;
    let mut chunks = Vec::with_capacity(files.len());
    let mut total = 0;
    for file in files {
        let meta_url = url(&file, base)?;
        let meta: serde_json::Value =
            serde_json::from_slice(&body(response(start_fetch(&meta_url)).await?, MIB, "map asset").await?).map_err(js_error)?;
        total += meta.get("count").and_then(serde_json::Value::as_u64).filter(|&c| c >= 1).unwrap_or(u64::MAX / 2);
        if total > MAP_SPLAT_BUDGET {
            return Err(js_error("map splat count exceeds budget"));
        }
        let (low, high) = (start_fetch(&url("means_l.webp", &meta_url)?), start_fetch(&url("means_u.webp", &meta_url)?));
        let low = body(response(low).await?, 32.0 * MIB, "map asset").await?;
        let high = body(response(high).await?, 32.0 * MIB, "map asset").await?;
        let (mut low, mut high) = (crate::decode_texture(0, low)?, crate::decode_texture(1, high)?);
        if low.width() != high.width() || low.height() != high.height() {
            return Err(js_error("map means dimensions differ"));
        }
        chunks.push(minimap::decode_splat_centres(&meta, &low.take_pixels(), &high.take_pixels(), flip_xy).map_err(js_error)?);
    }
    Ok(chunks)
}

/// Loads the level's voxels (`scene.voxel.json` / `.bin`), or its splat centres when it has none. `Ok(false)`: no
/// map source (the level has neither).
async fn load(base: &str, flip_xy: bool) -> Result<bool, JsValue> {
    let metadata = response(start_fetch(&url("scene.voxel.json", base)?)).await?;
    if metadata.status() == 404 {
        return Ok(match load_splats(base, flip_xy).await {
            Ok(splats) => {
                SOURCE.with(|s| *s.borrow_mut() = Some(Source::Splats(splats)));
                true
            }
            Err(error) => {
                web_sys::console::warn_1(&format!("Splat map unavailable: {}", message_of(&error)).into());
                false
            }
        });
    }
    let info = body(metadata, MIB, "voxel metadata").await?;
    let bytes = body(response(start_fetch(&url("scene.voxel.bin", base)?)).await?, 128.0 * MIB, "voxel binary").await?;
    let collider = VoxelCollisionData::new(info, bytes)?;
    SOURCE.with(|s| *s.borrow_mut() = Some(Source::Voxels(collider)));
    Ok(true)
}

/// The map image for a request with the radius and slab height it covers.
fn slice(request: &Request) -> Result<(Vec<u8>, f64, f64), JsValue> {
    SOURCE.with(|source| {
        let source = source.borrow();
        let source = source.as_ref().ok_or_else(|| js_error("map is not loaded"))?;
        let [x, _, z] = request.position;
        // Use a real floor when available; keep the calibrated foot height when the probe finds too little
        // support (for example while flying).
        let ground = match source {
            Source::Voxels(collider) => {
                let (reach, drop) = request.probe_reach();
                collider.ground_height(request.probe().to_vec(), reach, drop)?.as_f64().filter(|g| g.is_finite())
            }
            Source::Splats(_) => None,
        };
        let height = ground.unwrap_or(request.feet) + request.height;
        let (mask, radius) = match source {
            Source::Voxels(collider) => {
                let radius = minimap::bounded_voxel_radius(request.radius, request.thickness, f64::from(collider.resolution()));
                let mask = collider.horizontal_slice(vec![x as f32, z as f32], height as f32, radius as f32, request.thickness as f32, request.size)?;
                (mask, radius)
            }
            Source::Splats(chunks) => {
                (minimap::rasterize_splat_slice(chunks, request.position, height, request.radius, request.thickness, request.size), request.radius)
            }
        };
        if mask.len() != (request.size * request.size) as usize {
            return Err(js_error("invalid map raster size"));
        }
        Ok((minimap::colorize(&mask, request.size), radius, height))
    })
}

/// Handles a message in the map worker: `{type: 'load', base, flipXY}` replies `ready` or `unavailable`;
/// `{type: 'slice', id, ...request}` replies `{type: 'slice', id, center, radius, height, size, pixels}`. Failures
/// reply `{type: 'error', operation, id, message}`.
#[wasm_bindgen]
pub fn minimap_worker_handle(data: JsValue) {
    spawn_local(async move {
        let scope: DedicatedWorkerGlobalScope = js_sys::global().unchecked_into();
        let operation = get(&data, "type").as_string().unwrap_or_default();
        let reply = Object::new();
        let result: Result<Option<Array>, JsValue> = async {
            match operation.as_str() {
                "load" => {
                    let base = get(&data, "base").as_string().unwrap_or_default();
                    let ready = load(&base, get(&data, "flipXY").as_bool() != Some(false)).await?;
                    set(&reply, "type", &(if ready { "ready" } else { "unavailable" }).into());
                    Ok(None)
                }
                "slice" => {
                    let request = Request::parse(&to_json(&data)).ok_or_else(|| js_error("invalid map request"))?;
                    let (pixels, radius, height) = slice(&request)?;
                    let pixels = Uint8Array::from(pixels.as_slice());
                    set(&reply, "type", &"slice".into());
                    set(&reply, "id", &get(&data, "id"));
                    set(&reply, "center", &Array::of2(&request.position[0].into(), &request.position[2].into()));
                    set(&reply, "radius", &radius.into());
                    set(&reply, "height", &height.into());
                    set(&reply, "size", &request.size.into());
                    set(&reply, "pixels", &pixels);
                    Ok(Some(Array::of1(&pixels.buffer())))
                }
                _ => Err(js_error("unknown map message")),
            }
        }
        .await;
        match result {
            Ok(transfer) => {
                let _ = match transfer {
                    Some(transfer) => scope.post_message_with_transfer(&reply, &transfer),
                    None => scope.post_message(&reply),
                };
            }
            Err(error) => {
                let failure = Object::new();
                set(&failure, "type", &"error".into());
                set(&failure, "operation", &operation.into());
                set(&failure, "id", &get(&data, "id"));
                set(&failure, "message", &message_of(&error).chars().take(256).collect::<String>().into());
                let _ = scope.post_message(&failure);
            }
        }
    });
}

// ------------------------------------------------------------------------------------------------ page side

struct Page {
    map: Coalescer,
    worker: Worker,
    /// The gameplay viewer; uploads go through its `set_gameplay_ui_image`.
    game: JsValue,
    is_current: Function,
    on_result: Function,
    /// Results for the scene, delivered once the page state is no longer borrowed (a scene may request again
    /// from its handler).
    outbox: Vec<serde_json::Value>,
}

impl Page {
    fn current(&self) -> bool {
        self.is_current.call0(&JsValue::NULL).ok().and_then(|v| v.as_bool()).unwrap_or(false)
    }

    fn result(&mut self, value: serde_json::Value) {
        self.outbox.push(value);
    }

    fn pump(&mut self) {
        if !self.current() {
            return;
        }
        let Some((id, request)) = self.map.next() else {
            return;
        };
        let message = serde_json::json!({"type": "slice", "id": id, "position": request.position, "feet": request.feet,
            "units": request.units, "radius": request.radius, "height": request.height, "thickness": request.thickness,
            "size": request.size});
        if let Ok(message) = JSON::parse(&message.to_string()) {
            let _ = self.worker.post_message(&message);
        }
    }

    fn fail(&mut self, message: &str) {
        if !self.current() {
            return;
        }
        self.map.fail();
        self.result(serde_json::json!({"status": "unavailable"}));
        web_sys::console::warn_1(&format!("Level map: {message}").into());
    }

    fn on_message(&mut self, data: &JsValue) {
        if !self.current() {
            return;
        }
        let id = get(data, "id").as_f64().filter(|v| v.fract() == 0.0 && *v >= 0.0).map(|v| v as u64);
        match get(data, "type").as_string().as_deref() {
            Some("ready") => self.map.ready(),
            Some("unavailable") => {
                self.map.fail();
                self.result(serde_json::json!({"status": "unavailable"}));
                return;
            }
            Some("error") => {
                let operation = get(data, "operation").as_string();
                if operation.as_deref() == Some("load") {
                    self.fail(&get(data, "message").as_string().unwrap_or_default());
                    return;
                }
                if !self.map.reply(id) {
                    return;
                }
                self.result(serde_json::json!({"status": "error"}));
            }
            Some("slice") => {
                if !self.map.reply(id) {
                    return;
                }
                let status = self.upload(data);
                self.result(status);
            }
            _ => return,
        }
        self.pump();
    }

    /// Validates a slice reply and uploads it; the result for the scene.
    fn upload(&self, data: &JsValue) -> serde_json::Value {
        let error = serde_json::json!({"status": "error"});
        let pixels = get(data, "pixels");
        let size = get(data, "size").as_f64().filter(|s| s.fract() == 0.0 && (16.0..=256.0).contains(s));
        let center: Vec<f64> = if Array::is_array(&get(data, "center")) {
            Array::from(&get(data, "center")).iter().filter_map(|v| v.as_f64()).filter(|v| v.is_finite()).collect()
        } else {
            Vec::new()
        };
        let radius = get(data, "radius").as_f64().filter(|r| r.is_finite() && *r > 0.0);
        let height = get(data, "height").as_f64().filter(|h| h.is_finite());
        let (Some(size), Some(radius), Some(height)) = (size, radius, height) else {
            return error;
        };
        if !pixels.is_instance_of::<Uint8Array>() || f64::from(Uint8Array::new(&pixels).length()) != size * size * 4.0 || center.len() != 2 {
            return error;
        }
        let upload = Reflect::get(&self.game, &"set_gameplay_ui_image".into()).ok().and_then(|f| f.dyn_into::<Function>().ok());
        let arguments = Array::of4(&"level-minimap".into(), &size.into(), &size.into(), &pixels);
        match upload.map(|f| Reflect::apply(&f, &self.game, &arguments)) {
            Some(Ok(_)) => serde_json::json!({"status": "ready", "center": center, "radius": radius, "height": height}),
            Some(Err(failure)) => {
                web_sys::console::warn_1(&format!("Could not upload minimap: {}", message_of(&failure)).into());
                error
            }
            None => error,
        }
    }
}

fn deliver(page: &Rc<RefCell<Page>>) {
    let (results, on_result) = {
        let mut page = page.borrow_mut();
        (std::mem::take(&mut page.outbox), page.on_result.clone())
    };
    for value in results {
        let value = JSON::parse(&value.to_string()).unwrap_or(JsValue::NULL);
        let _ = on_result.call1(&JsValue::NULL, &value);
    }
}

/// Map requests for one gameplay scene. `is_current()` says whether that scene is still active (an old scene can
/// never install a new texture); `on_result(result)` receives `{status: ready|error|unavailable, ...}`.
#[wasm_bindgen]
pub struct MinimapRuntime {
    page: Rc<RefCell<Page>>,
    _on_message: Closure<dyn FnMut(MessageEvent)>,
    _on_error: Closure<dyn FnMut(web_sys::ErrorEvent)>,
}

#[wasm_bindgen]
impl MinimapRuntime {
    #[wasm_bindgen(constructor)]
    pub fn new(
        worker_url: &str,
        game: JsValue,
        base: &str,
        is_current: Function,
        on_result: Function,
        flip_xy: bool,
    ) -> Result<MinimapRuntime, JsValue> {
        let options = web_sys::WorkerOptions::new();
        options.set_type(web_sys::WorkerType::Module);
        let worker = Worker::new_with_options(worker_url, &options)?;
        let page = Rc::new(RefCell::new(Page { map: Coalescer::default(), worker: worker.clone(), game, is_current, on_result, outbox: Vec::new() }));
        let weak = Rc::downgrade(&page);
        let on_message = Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
            if let Some(page) = weak.upgrade() {
                page.borrow_mut().on_message(&event.data());
                deliver(&page);
            }
        });
        let weak = Rc::downgrade(&page);
        let on_error = Closure::<dyn FnMut(web_sys::ErrorEvent)>::new(move |event: web_sys::ErrorEvent| {
            if let Some(page) = weak.upgrade() {
                let message = event.message();
                page.borrow_mut().fail(if message.is_empty() { "Map worker failed" } else { &message });
                deliver(&page);
            }
        });
        worker.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
        worker.set_onerror(Some(on_error.as_ref().unchecked_ref()));
        let load = serde_json::json!({"type": "load", "base": base, "flipXY": flip_xy});
        worker.post_message(&JSON::parse(&load.to_string())?)?;
        Ok(Self { page, _on_message: on_message, _on_error: on_error })
    }

    /// Queues a scene's `hud.map_request` (invalid requests are ignored).
    pub fn request(&self, value: JsValue) {
        let Some(request) = Request::parse(&to_json(&value)) else {
            return;
        };
        {
            let mut page = self.page.borrow_mut();
            if page.map.request(request) {
                page.pump();
            } else {
                page.result(serde_json::json!({"status": "unavailable"}));
            }
        }
        deliver(&self.page);
    }

    /// Terminates the worker; later replies are dropped.
    pub fn stop(&self) {
        let mut page = self.page.borrow_mut();
        page.worker.set_onmessage(None);
        page.worker.set_onerror(None);
        page.worker.terminate();
        page.map.stop();
    }
}

impl Drop for MinimapRuntime {
    fn drop(&mut self) {
        self.stop();
    }
}
