//! Level metadata loading off the page thread: a short-lived Web Worker fetches every chunk descriptor (bounded
//! concurrency and size), parses them and builds the level's asset inventory; the original bytes go back to the page
//! for the renderer. Both sides are Rust; the worker's only JavaScript is its bootstrap (`metadata-worker.js`).

use crate::js_error;
use countersplat_scene::asset_policy;
use js_sys::{Array, Object, Promise, Reflect, Uint8Array, JSON};
use std::{cell::RefCell, rc::Rc};
use wasm_bindgen::{prelude::*, JsCast};
use wasm_bindgen_futures::{spawn_local, JsFuture};
use web_sys::{DedicatedWorkerGlobalScope, MessageEvent, RequestCache, RequestInit, Response, Worker, WorkerGlobalScope};

const MAX_METADATA_BYTES: f64 = 512.0 * 1024.0 * 1024.0;
const MAX_FILE_BYTES: f64 = 16.0 * 1024.0 * 1024.0;

fn get(object: &JsValue, key: &str) -> JsValue {
    Reflect::get(object, &JsValue::from_str(key)).unwrap_or(JsValue::UNDEFINED)
}

fn set(object: &Object, key: &str, value: &JsValue) {
    let _ = Reflect::set(object, &JsValue::from_str(key), value);
}

fn to_json(value: &JsValue) -> serde_json::Value {
    if value.is_undefined() {
        return serde_json::Value::Null;
    }
    JSON::stringify(value)
        .ok()
        .and_then(|text| text.as_string())
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(serde_json::Value::Null)
}

/// A chunk descriptor's URL under `base`, refusing unsafe relative paths.
fn metadata_url(relative: &str, base: &str) -> Result<String, JsValue> {
    if !asset_policy::safe_relative_path(relative) {
        return Err(js_error("The level contains an invalid asset path."));
    }
    let encoded: Vec<String> = relative.split('/').map(|s| String::from(js_sys::encode_uri_component(s))).collect();
    Ok(web_sys::Url::new_with_base(&encoded.join("/"), base)?.href())
}

// ---------------------------------------------------------------------------------------------- worker side

/// Handles one metadata request in the metadata worker: posts `{type: 'progress', completed, total}` (at most every
/// 50 ms), then `{type: 'done', result: {metas, levelAssets}}` with the bytes transferred, or `{type: 'error'}`.
#[wasm_bindgen]
pub fn metadata_worker_handle(request: JsValue) {
    spawn_local(async move {
        let scope: DedicatedWorkerGlobalScope = js_sys::global().unchecked_into();
        match prepare(&request, &scope).await {
            Ok((result, transfer)) => {
                let message = Object::new();
                set(&message, "type", &"done".into());
                set(&message, "result", &result);
                let _ = scope.post_message_with_transfer(&message, &transfer);
            }
            Err(error) => {
                let message = Object::new();
                set(&message, "type", &"error".into());
                let text = error
                    .as_string()
                    .or_else(|| error.dyn_ref::<js_sys::Error>().map(|e| String::from(e.message())))
                    .unwrap_or_else(|| "Metadata worker failed.".into());
                set(&message, "error", &text.into());
                let _ = scope.post_message(&message);
            }
        }
    });
}

async fn prepare(request: &JsValue, scope: &DedicatedWorkerGlobalScope) -> Result<(Object, Array), JsValue> {
    let level = to_json(&get(request, "level"));
    let filenames: Vec<String> = Array::from(&get(request, "filenames")).iter().filter_map(|v| v.as_string()).collect();
    let base = get(request, "base").as_string().unwrap_or_default();
    let concurrency = get(request, "concurrency").as_f64().unwrap_or(12.0);
    let lod_count = get(request, "lodCount").as_f64().unwrap_or(0.0);
    if level.is_null() || filenames.is_empty() || filenames.len() > 10_000 || !(1.0..=32.0).contains(&concurrency) || concurrency.fract() != 0.0 {
        return Err(js_error("Invalid level metadata request."));
    }
    let format = level.get("format").and_then(serde_json::Value::as_str);
    let single = format == Some("single_sog");
    if !single && format != Some("streamed_sog") {
        return Err(js_error("This level format is not supported."));
    }
    let single_metadata = get(request, "singleMetadata");
    if single && (filenames.len() != 1 || !single_metadata.is_instance_of::<Uint8Array>()) {
        return Err(js_error("Invalid single scan metadata."));
    }
    let worker_scope: &WorkerGlobalScope = scope.unchecked_ref();
    let init = RequestInit::new();
    init.set_cache(RequestCache::NoStore);
    let total = filenames.len();
    let mut metas: Vec<Uint8Array> = Vec::with_capacity(total);
    let mut parsed: Vec<serde_json::Value> = Vec::with_capacity(total);
    let mut retained = 0.0;
    let mut last_update = f64::NEG_INFINITY;
    let clock = || worker_scope.performance().map_or(0.0, |p| p.now());
    for batch in filenames.chunks(concurrency as usize) {
        let bytes: Vec<Uint8Array> = if single {
            vec![Uint8Array::new(&single_metadata)]
        } else {
            let fetches = Array::new();
            let mut urls = Vec::new();
            for name in batch {
                let url = metadata_url(name, &base)?;
                fetches.push(&worker_scope.fetch_with_str_and_init(&url, &init));
                urls.push(url);
            }
            let responses: Array = JsFuture::from(Promise::all(&fetches)).await?.unchecked_into();
            let buffers = Array::new();
            for (response, url) in responses.iter().zip(&urls) {
                let response: Response = response.unchecked_into();
                if !response.ok() {
                    return Err(js_error(format!("Could not load {url} (HTTP {}).", response.status())));
                }
                let declared = response.headers().get("content-length").ok().flatten().and_then(|v| v.parse::<f64>().ok());
                if declared.is_some_and(|size| size > MAX_FILE_BYTES) {
                    return Err(js_error("Chunk metadata exceeds the 16 MiB limit."));
                }
                let buffer: Promise = response.array_buffer()?;
                buffers.push(&buffer);
            }
            let buffers: Array = JsFuture::from(Promise::all(&buffers)).await?.unchecked_into();
            buffers.iter().map(|b| Uint8Array::new(&b)).collect()
        };
        for array in bytes {
            let size = f64::from(array.byte_length());
            if size > MAX_FILE_BYTES {
                return Err(js_error("Chunk metadata exceeds the 16 MiB limit."));
            }
            retained += size;
            if retained > MAX_METADATA_BYTES {
                return Err(js_error("Level metadata exceeds the 512 MiB limit."));
            }
            parsed.push(serde_json::from_slice(&array.to_vec()).map_err(js_error)?);
            metas.push(array);
            let now = clock();
            if metas.len() == total || now - last_update >= 50.0 {
                last_update = now;
                let progress = Object::new();
                set(&progress, "type", &"progress".into());
                set(&progress, "completed", &(metas.len() as u32).into());
                set(&progress, "total", &(total as u32).into());
                let _ = scope.post_message(&progress);
            }
        }
    }
    let inventory_bytes = get(request, "inventoryBytes");
    let inventory = if inventory_bytes.is_instance_of::<Uint8Array>() && Uint8Array::new(&inventory_bytes).length() > 0 {
        Some(serde_json::from_slice::<serde_json::Value>(&Uint8Array::new(&inventory_bytes).to_vec()).map_err(js_error)?)
    } else {
        None
    };
    let assets = asset_policy::describe_level_assets(
        &level,
        lod_count as u32,
        &parsed,
        &filenames,
        get(request, "viewerSettings").as_bool().unwrap_or(false),
        inventory.as_ref(),
        get(request, "hasSource").as_bool().unwrap_or(false),
        get(request, "source").as_string().as_deref().unwrap_or("level"),
    )
    .map_err(js_error)?;
    let result = Object::new();
    let metas_array = Array::new();
    let transfer = Array::new();
    for array in &metas {
        // A single scan's bytes came from the page; copy so each entry owns its buffer.
        let owned = Uint8Array::new(&array.slice(0, array.length()));
        transfer.push(&owned.buffer());
        metas_array.push(&owned);
    }
    set(&result, "metas", &metas_array);
    set(&result, "levelAssets", &JSON::parse(&assets.to_string())?);
    Ok((result, transfer))
}

// ------------------------------------------------------------------------------------------------ page side

/// Loads a level's chunk metadata in a fresh worker (`worker_url`, the bootstrap module) and resolves to `{metas,
/// levelAssets}`. Progress goes to `on_progress(completed, total)`; aborting `signal` rejects with an `AbortError`
/// and terminates the worker, which cancels its fetches.
#[wasm_bindgen(js_name = loadLevelMetadata)]
pub fn load_level_metadata(
    worker_url: &str,
    request: JsValue,
    signal: web_sys::AbortSignal,
    on_progress: js_sys::Function,
) -> Result<Promise, JsValue> {
    let options = web_sys::WorkerOptions::new();
    options.set_type(web_sys::WorkerType::Module);
    let worker = Worker::new_with_options(worker_url, &options)?;
    let expected = Array::from(&get(&request, "filenames")).length();
    let transfer = Array::new();
    for key in ["singleMetadata", "inventoryBytes"] {
        let bytes = get(&request, key);
        if bytes.is_instance_of::<Uint8Array>() {
            transfer.push(&Uint8Array::new(&bytes).buffer());
        }
    }
    let promise = Promise::new(&mut |resolve, reject| {
        let settled = Rc::new(RefCell::new(false));
        let finish = {
            let (settled, worker, signal) = (settled.clone(), worker.clone(), signal.clone());
            let (resolve, reject) = (resolve.clone(), reject.clone());
            Rc::new(move |result: Result<JsValue, JsValue>| {
                if settled.replace(true) {
                    return;
                }
                worker.set_onmessage(None);
                worker.set_onerror(None);
                worker.terminate();
                signal.set_onabort(None);
                match result {
                    Ok(value) => drop(resolve.call1(&JsValue::NULL, &value)),
                    Err(error) => drop(reject.call1(&JsValue::NULL, &error)),
                }
            })
        };
        let completed = Rc::new(RefCell::new(0u32));
        let on_message = {
            let (finish, on_progress, completed) = (finish.clone(), on_progress.clone(), completed.clone());
            Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
                let data = event.data();
                match get(&data, "type").as_string().as_deref() {
                    Some("progress") => {
                        let done = get(&data, "completed").as_f64().unwrap_or(-1.0);
                        let total = get(&data, "total").as_f64().unwrap_or(-1.0);
                        if done < f64::from(*completed.borrow()) || total != f64::from(expected) || done > total {
                            finish(Err(js_error("Metadata worker returned invalid progress.")));
                            return;
                        }
                        *completed.borrow_mut() = done as u32;
                        let _ = on_progress.call2(&JsValue::NULL, &done.into(), &total.into());
                    }
                    Some("done") => {
                        let result = get(&data, "result");
                        let metas = get(&result, "metas");
                        let valid = Array::is_array(&metas)
                            && Array::from(&metas).length() == expected
                            && Array::from(&metas).iter().all(|m| m.is_instance_of::<Uint8Array>())
                            && !get(&result, "levelAssets").is_undefined();
                        finish(if valid { Ok(result) } else { Err(js_error("Metadata worker returned invalid results.")) });
                    }
                    Some("error") => finish(Err(js_error(get(&data, "error").as_string().unwrap_or_else(|| "Metadata worker failed.".into())))),
                    _ => finish(Err(js_error("Metadata worker returned an unexpected message."))),
                }
            })
        };
        let on_error = {
            let finish = finish.clone();
            Closure::<dyn FnMut(web_sys::ErrorEvent)>::new(move |event: web_sys::ErrorEvent| {
                let message = event.message();
                finish(Err(js_error(if message.is_empty() { "Metadata worker failed.".into() } else { message })));
            })
        };
        let on_abort = {
            let finish = finish.clone();
            Closure::<dyn FnMut()>::new(move || {
                let error = web_sys::DomException::new_with_message_and_name("Loading cancelled", "AbortError")
                    .map(JsValue::from)
                    .unwrap_or_else(|_| js_error("Loading cancelled"));
                finish(Err(error));
            })
        };
        if signal.aborted() {
            (on_abort.as_ref().unchecked_ref::<js_sys::Function>()).call0(&JsValue::NULL).ok();
            return;
        }
        worker.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
        worker.set_onerror(Some(on_error.as_ref().unchecked_ref()));
        signal.set_onabort(Some(on_abort.as_ref().unchecked_ref()));
        // Three small handlers per level load; they must outlive this call (a settled load ignores them).
        on_message.forget();
        on_error.forget();
        on_abort.forget();
        if let Err(error) = worker.post_message_with_transfer(&request, &transfer) {
            finish(Err(error));
        }
    });
    Ok(promise)
}
