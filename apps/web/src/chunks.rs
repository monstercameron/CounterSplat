//! Level chunk decoding in Web Workers and streaming into the renderer.
//!
//! All of it is Rust: the worker side (`chunk_worker_handle`: fetch a chunk's WebP streams, decode them and post the
//! pixels back), the pool on the page (`WorkerPool`), level preloading (`preload_level_chunks`) and the per-frame
//! streaming in `Viewer::pump_chunks`. The only JavaScript is the worker bootstrap, `decoder.js`, which loads this
//! module and forwards each message to `chunk_worker_handle`. The planning rules (worker count, splitting a level
//! with fewer chunks than workers, the per-frame insert budget) live in `countersplat_core::chunk_jobs`.

use crate::js_error;
use countersplat_assets::sog;
use countersplat_core::chunk_jobs::{self, Merger, STREAMS};
use js_sys::{Array, Object, Promise, Reflect, Uint32Array, Uint8Array};
use std::{
    cell::RefCell,
    collections::{BTreeSet, VecDeque},
    rc::Rc,
};
use wasm_bindgen::{prelude::*, JsCast};
use wasm_bindgen_futures::{spawn_local, JsFuture};
use web_sys::{
    DedicatedWorkerGlobalScope, ErrorEvent, MessageEvent, RequestCache, RequestInit, Response,
    Worker, WorkerGlobalScope, WorkerOptions, WorkerType,
};

/// A failed chunk is offered to the streamer again after this long.
const RETRY_MS: f64 = 3000.0;
/// Time per frame for copying decoded chunks into the renderer (always at least one chunk).
const INSERT_BUDGET_MS: f64 = 3.0;

fn set(object: &Object, key: &str, value: &JsValue) {
    let _ = Reflect::set(object, &JsValue::from_str(key), value);
}

fn get(object: &JsValue, key: &str) -> JsValue {
    Reflect::get(object, &JsValue::from_str(key)).unwrap_or(JsValue::UNDEFINED)
}

fn now_ms() -> f64 {
    js_sys::Date::now()
}

/// `base` + the chunk's stream file, each path segment URL-encoded (`base` ends with `/`).
fn stream_url(base: &str, folder: &str, file: &str, single: bool) -> String {
    let relative = if single {
        file.to_string()
    } else {
        format!("{folder}/{file}")
    };
    let encoded: Vec<String> = relative
        .split('/')
        .map(|segment| String::from(js_sys::encode_uri_component(segment)))
        .collect();
    format!("{base}{}", encoded.join("/"))
}

// ---------------------------------------------------------------------------------------------- worker side

/// Handles one decode request inside a chunk worker and posts the reply: `{ok, job, metadata, dimensions,
/// pixels}` with the pixel buffers transferred, or `{ok: false, job, error}`.
#[wasm_bindgen]
pub fn chunk_worker_handle(request: JsValue) {
    spawn_local(async move {
        let job = get(&request, "job");
        let (reply, transfer) = match decode_request(&request).await {
            Ok(pair) => pair,
            Err(error) => {
                let reply = Object::new();
                set(&reply, "ok", &false.into());
                set(
                    &reply,
                    "error",
                    &error.as_string().map_or(error.clone(), JsValue::from).into(),
                );
                (reply, Array::new())
            }
        };
        set(&reply, "job", &job);
        let scope: DedicatedWorkerGlobalScope = js_sys::global().unchecked_into();
        if scope.post_message_with_transfer(&reply, &transfer).is_err() {
            let _ = scope.post_message(&reply);
        }
    });
}

async fn decode_request(request: &JsValue) -> Result<(Object, Array), JsValue> {
    let base = get(request, "base")
        .as_string()
        .ok_or_else(|| js_error("decode request has no base URL"))?;
    let folder = get(request, "folder").as_string().unwrap_or_default();
    let single = get(request, "single").as_bool().unwrap_or(false);
    let metadata = Uint8Array::new(&get(request, "metadata")).to_vec();
    let parts = get(request, "parts");
    let parts: Option<Vec<usize>> = (!parts.is_undefined() && !parts.is_null()).then(|| {
        Array::from(&parts)
            .iter()
            .filter_map(|value| value.as_f64())
            .map(|value| value as usize)
            .collect()
    });
    let wanted = |stream: usize| parts.as_ref().is_none_or(|parts| parts.contains(&stream));
    let has_sh = sog::parse_meta_json(&metadata)
        .map_err(js_error)?
        .sh_n_count
        > 0;

    // Fetch every wanted stream at once, then decode them one at a time (each decode is single-threaded).
    let scope: WorkerGlobalScope = js_sys::global().unchecked_into();
    let init = RequestInit::new();
    init.set_cache(RequestCache::NoStore);
    let fetches = Array::new();
    let mut fetched = Vec::new();
    for (stream, (name, file)) in sog::streams().iter().enumerate() {
        if wanted(stream) && (has_sh || !sog::is_sh_stream(name)) {
            fetches.push(&scope.fetch_with_str_and_init(&stream_url(&base, &folder, file, single), &init));
            fetched.push(stream);
        }
    }
    let responses: Array = JsFuture::from(Promise::all(&fetches)).await?.unchecked_into();
    let buffers = Array::new();
    for response in responses.iter() {
        let response: Response = response.unchecked_into();
        if !response.ok() {
            return Err(js_error(format!("{}: HTTP {}", response.url(), response.status())));
        }
        let buffer: Promise = response.array_buffer()?;
        buffers.push(&buffer);
    }
    let buffers: Array = JsFuture::from(Promise::all(&buffers)).await?.unchecked_into();

    let dimensions = Uint32Array::new_with_length((STREAMS * 2) as u32);
    let pixels = Array::new();
    let transfer = Array::new();
    for (stream, (name, _)) in sog::streams().iter().enumerate() {
        let texture = if wanted(stream) {
            let bytes = fetched
                .iter()
                .position(|&s| s == stream)
                .map(|at| Uint8Array::new(&buffers.get(at as u32)).to_vec())
                .unwrap_or_default();
            sog::decode_stream(name, &bytes).map_err(js_error)?
        } else {
            sog::Texture {
                width: 0,
                height: 0,
                rgba: Vec::new(),
            }
        };
        dimensions.set_index((stream * 2) as u32, texture.width);
        dimensions.set_index((stream * 2 + 1) as u32, texture.height);
        let array = Uint8Array::from(texture.rgba.as_slice());
        transfer.push(&array.buffer());
        pixels.push(&array);
    }
    let metadata = Uint8Array::from(metadata.as_slice());
    transfer.push(&metadata.buffer());
    transfer.push(&dimensions.buffer());
    let reply = Object::new();
    set(&reply, "ok", &true.into());
    set(&reply, "metadata", &metadata);
    set(&reply, "dimensions", &dimensions);
    set(&reply, "pixels", &pixels);
    Ok((reply, transfer))
}

// ------------------------------------------------------------------------------------------------ page side

enum Reply {
    Message(usize, JsValue),
    Crashed(usize, String),
}

struct Slot {
    worker: Option<Worker>,
    job: Option<u32>,
    _handlers: Option<(Closure<dyn FnMut(MessageEvent)>, Closure<dyn FnMut(ErrorEvent)>)>,
}

/// Decode workers on the page. Work goes only to idle workers (no hidden queue), so the streamer can choose fresh
/// priorities whenever one frees up.
pub(crate) struct WorkerPool {
    url: String,
    slots: Vec<Slot>,
    inbox: Rc<RefCell<VecDeque<Reply>>>,
    /// Resolves a pending `next_reply` wait.
    waker: Rc<RefCell<Option<js_sys::Function>>>,
}

fn wake(waker: &Rc<RefCell<Option<js_sys::Function>>>) {
    if let Some(resolve) = waker.borrow_mut().take() {
        let _ = resolve.call0(&JsValue::NULL);
    }
}

impl WorkerPool {
    pub(crate) fn new(url: &str, size: u32) -> Result<Self, JsValue> {
        if !(1..=chunk_jobs::MAX_DECODERS).contains(&size) {
            return Err(js_error("Invalid chunk worker count."));
        }
        let mut pool = Self {
            url: url.to_string(),
            slots: (0..size)
                .map(|_| Slot {
                    worker: None,
                    job: None,
                    _handlers: None,
                })
                .collect(),
            inbox: Rc::default(),
            waker: Rc::default(),
        };
        for slot in 0..size as usize {
            pool.spawn(slot)?;
        }
        Ok(pool)
    }

    fn spawn(&mut self, slot: usize) -> Result<(), JsValue> {
        let options = WorkerOptions::new();
        options.set_type(WorkerType::Module);
        let worker = Worker::new_with_options(&self.url, &options)?;
        let (inbox, waker) = (self.inbox.clone(), self.waker.clone());
        let on_message = Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
            inbox.borrow_mut().push_back(Reply::Message(slot, event.data()));
            wake(&waker);
        });
        let (inbox, waker) = (self.inbox.clone(), self.waker.clone());
        let on_error = Closure::<dyn FnMut(ErrorEvent)>::new(move |event: ErrorEvent| {
            event.prevent_default();
            inbox.borrow_mut().push_back(Reply::Crashed(slot, event.message()));
            wake(&waker);
        });
        worker.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
        worker.set_onerror(Some(on_error.as_ref().unchecked_ref()));
        let previous = &mut self.slots[slot];
        if let Some(old) = previous.worker.take() {
            old.terminate();
        }
        *previous = Slot {
            worker: Some(worker),
            job: None,
            _handlers: Some((on_message, on_error)),
        };
        Ok(())
    }

    pub(crate) fn size(&self) -> usize {
        self.slots.iter().filter(|slot| slot.worker.is_some()).count()
    }

    pub(crate) fn idle(&self) -> usize {
        self.slots
            .iter()
            .filter(|slot| slot.worker.is_some() && slot.job.is_none())
            .count()
    }

    /// Sends `request` (with `job`) to an idle worker; false when none is idle or the post fails.
    pub(crate) fn submit(&mut self, job: u32, request: &Object) -> bool {
        let Some(slot) = self
            .slots
            .iter_mut()
            .find(|slot| slot.worker.is_some() && slot.job.is_none())
        else {
            return false;
        };
        set(request, "job", &job.into());
        if slot.worker.as_ref().is_some_and(|worker| worker.post_message(request).is_ok()) {
            slot.job = Some(job);
            true
        } else {
            false
        }
    }

    /// Finished jobs since the last call: the reply object, or the error. A worker that crashed is replaced (one
    /// that fails to start is retired; the others carry on).
    pub(crate) fn take_replies(&mut self) -> Vec<(u32, Result<JsValue, String>)> {
        let replies: Vec<Reply> = self.inbox.borrow_mut().drain(..).collect();
        let mut done = Vec::new();
        for reply in replies {
            match reply {
                Reply::Message(slot, data) => {
                    let Some(job) = self.slots[slot].job.take() else {
                        continue;
                    };
                    if get(&data, "ok").as_bool() == Some(true) {
                        done.push((job, Ok(data)));
                    } else {
                        let error = get(&data, "error")
                            .as_string()
                            .unwrap_or_else(|| "Chunk decode failed.".into());
                        done.push((job, Err(error)));
                    }
                }
                Reply::Crashed(slot, message) => {
                    let job = self.slots[slot].job.take();
                    if self.spawn(slot).is_err() {
                        if let Some(worker) = self.slots[slot].worker.take() {
                            worker.terminate();
                        }
                    }
                    if let Some(job) = job {
                        done.push((job, Err(format!("Chunk worker failed: {message}"))));
                    }
                }
            }
        }
        done
    }

    /// Waits until a reply arrives or `timeout_ms` passes.
    async fn wait(&self, timeout_ms: i32) {
        if !self.inbox.borrow().is_empty() {
            return;
        }
        let waker = self.waker.clone();
        let promise = Promise::new(&mut |resolve, _| {
            if let Some(window) = web_sys::window() {
                let _ = window
                    .set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, timeout_ms);
            }
            *waker.borrow_mut() = Some(resolve);
        });
        let _ = JsFuture::from(promise).await;
    }
}

impl Drop for WorkerPool {
    fn drop(&mut self) {
        for slot in &mut self.slots {
            if let Some(worker) = slot.worker.take() {
                worker.terminate();
            }
        }
    }
}

/// A decode request for `folder`'s streams (or `parts` of them).
fn request(base: &str, folder: &str, single: bool, metadata: &[u8], parts: Option<&[usize]>) -> Object {
    let request = Object::new();
    set(&request, "base", &base.into());
    set(&request, "folder", &folder.into());
    set(&request, "single", &single.into());
    set(&request, "metadata", &Uint8Array::from(metadata));
    if let Some(parts) = parts {
        let array: Array = parts.iter().map(|&part| JsValue::from(part as u32)).collect();
        set(&request, "parts", &array);
    }
    request
}

/// The streams of a decoded reply: (width, height, pixels) each.
pub(crate) fn reply_streams(reply: &JsValue) -> Result<(Vec<u8>, Vec<(u32, u32, Uint8Array)>), JsValue> {
    let metadata = Uint8Array::new(&get(reply, "metadata")).to_vec();
    let dimensions = Uint32Array::new(&get(reply, "dimensions")).to_vec();
    let pixels = Array::from(&get(reply, "pixels"));
    if dimensions.len() != STREAMS * 2 || pixels.length() as usize != STREAMS {
        return Err(js_error("Decoder returned invalid chunk data."));
    }
    let streams = (0..STREAMS)
        .map(|stream| {
            (
                dimensions[stream * 2],
                dimensions[stream * 2 + 1],
                Uint8Array::new(&pixels.get(stream as u32)),
            )
        })
        .collect();
    Ok((metadata, streams))
}

fn reply_bytes(reply: &JsValue) -> f64 {
    let pixels = Array::from(&get(reply, "pixels"));
    let pixel_bytes: f64 = pixels
        .iter()
        .map(|array| f64::from(Uint8Array::new(&array).byte_length()))
        .sum();
    pixel_bytes
        + f64::from(Uint8Array::new(&get(reply, "metadata")).byte_length())
        + (STREAMS * 8) as f64
}

/// Streaming state of a live level: which chunks are being decoded, decoded chunks waiting for a frame's insert
/// budget, and failed chunks waiting to be offered again.
pub(crate) struct ChunkStreaming {
    pub(crate) pool: WorkerPool,
    base: String,
    single: bool,
    pub(crate) in_flight: BTreeSet<usize>,
    pub(crate) ready: VecDeque<(usize, JsValue)>,
    retry: Vec<(usize, f64)>,
}

impl ChunkStreaming {
    pub(crate) fn new(base: &str, worker_url: &str, single: bool, workers: u32) -> Result<Self, JsValue> {
        Ok(Self {
            pool: WorkerPool::new(worker_url, workers)?,
            base: base.to_string(),
            single,
            in_flight: BTreeSet::new(),
            ready: VecDeque::new(),
            retry: Vec::new(),
        })
    }

    /// Collects finished decodes; failed ones are retried after `RETRY_MS`.
    pub(crate) fn collect(&mut self) {
        let now = now_ms();
        for (job, result) in self.pool.take_replies() {
            let file = job as usize;
            match result {
                Ok(reply) => self.ready.push_back((file, reply)),
                Err(error) => self.fail(file, &error, now),
            }
        }
    }

    pub(crate) fn fail(&mut self, file: usize, error: &str, now: f64) {
        web_sys::console::warn_1(&format!("Chunk {file} failed: {error}").into());
        self.retry.push((file, now + RETRY_MS));
    }

    /// Chunks whose retry delay has passed: report them to the streamer and forget them.
    pub(crate) fn due_retries(&mut self) -> Vec<usize> {
        let now = now_ms();
        let (due, waiting): (Vec<_>, Vec<_>) = self.retry.drain(..).partition(|&(_, at)| at <= now);
        self.retry = waiting;
        due.into_iter()
            .map(|(file, _)| {
                self.in_flight.remove(&file);
                file
            })
            .collect()
    }

    /// Workers free for new requests: idle ones, less decoded chunks still waiting to be inserted (so memory stays
    /// bounded when inserting falls behind).
    pub(crate) fn capacity(&self) -> usize {
        self.pool.idle().saturating_sub(self.ready.len())
    }

    /// Sends a request for chunk `file` to an idle worker.
    pub(crate) fn submit(&mut self, file: usize, folder: &str, metadata: &[u8]) -> bool {
        if self.in_flight.contains(&file) {
            return true;
        }
        let request = request(&self.base, folder, self.single, metadata, None);
        if self.pool.submit(file as u32, &request) {
            self.in_flight.insert(file);
            true
        } else {
            false
        }
    }

    pub(crate) fn drain_ready(&mut self, mut insert: impl FnMut(usize, &JsValue) -> Result<(), JsValue>) {
        let clock = web_sys::window().and_then(|window| window.performance());
        let now = || clock.as_ref().map_or_else(now_ms, |performance| performance.now());
        let mut ready = std::mem::take(&mut self.ready);
        let mut failed = Vec::new();
        chunk_jobs::drain_within(&mut ready, INSERT_BUDGET_MS, now, |(file, reply)| {
            match insert(file, &reply) {
                Ok(()) => {
                    self.in_flight.remove(&file);
                }
                Err(error) => failed.push((file, error.as_string().unwrap_or_default())),
            }
        });
        self.ready = ready;
        let now = now_ms();
        for (file, error) in failed {
            self.fail(file, &error, now);
        }
    }
}

/// Chunks decoded before a level starts (`preload_level_chunks`), in file order.
#[wasm_bindgen]
pub struct PreloadedChunks {
    pub(crate) chunks: Vec<(usize, JsValue)>,
}

/// Decodes every chunk of a level before it starts, `workers` at a time, reporting `(done, total)` to `on_progress`.
/// A level with fewer chunks than workers decodes each chunk's streams on separate workers and merges them. Fails
/// past `max_bytes` of decoded data, or with an `AbortError` when `signal` aborts.
#[wasm_bindgen]
#[allow(clippy::too_many_arguments)]
pub async fn preload_level_chunks(
    base: String,
    worker_url: String,
    metas: Array,
    filenames: Array,
    single: bool,
    workers: u32,
    max_bytes: f64,
    on_progress: js_sys::Function,
    signal: web_sys::AbortSignal,
) -> Result<PreloadedChunks, JsValue> {
    let count = metas.length() as usize;
    if count == 0 || count != filenames.length() as usize || !(max_bytes >= 1.0) {
        return Err(js_error("Invalid splat preload request."));
    }
    let aborted = || {
        web_sys::DomException::new_with_message_and_name("Loading cancelled", "AbortError")
            .map(JsValue::from)
            .unwrap_or_else(|_| js_error("Loading cancelled"))
    };
    let jobs = chunk_jobs::plan_jobs(count, workers);
    let mut pool = WorkerPool::new(&worker_url, workers)?;
    let mut merger: Merger<(u32, u32, Uint8Array)> = Merger::new(count);
    let mut chunks: Vec<Option<JsValue>> = vec![None; count];
    let (mut next, mut done, mut retained) = (0usize, 0usize, 0.0f64);
    while done < count {
        if signal.aborted() {
            return Err(aborted());
        }
        while next < jobs.len() && pool.idle() > 0 {
            let job = &jobs[next];
            let metadata = Uint8Array::new(&metas.get(job.chunk as u32)).to_vec();
            let filename = filenames.get(job.chunk as u32).as_string().unwrap_or_default();
            let folder = if single {
                "single".to_string()
            } else {
                filename.split('/').next().unwrap_or_default().to_string()
            };
            let request = request(&base, &folder, single, &metadata, job.parts);
            if !pool.submit(next as u32, &request) {
                return Err(js_error("Chunk workers failed."));
            }
            next += 1;
        }
        if pool.size() == 0 {
            return Err(js_error("Chunk workers failed: no worker could start."));
        }
        pool.wait(100).await;
        for (id, result) in pool.take_replies() {
            let job = &jobs[id as usize];
            let reply = result.map_err(|error| js_error(format!("Chunk {}: {error}", job.chunk)))?;
            retained += reply_bytes(&reply);
            if retained > max_bytes {
                return Err(js_error(
                    "Decoded splat preload exceeds 512 MiB. Mark splat chunks streamable.",
                ));
            }
            let (metadata, streams) = reply_streams(&reply)?;
            let pieces: Vec<Option<(u32, u32, Uint8Array)>> = streams.into_iter().map(Some).collect();
            let Some(streams) = merger.add(job, pieces) else {
                continue;
            };
            let dimensions = Uint32Array::new_with_length((STREAMS * 2) as u32);
            let pixels = Array::new();
            for (stream, (width, height, array)) in streams.into_iter().enumerate() {
                dimensions.set_index((stream * 2) as u32, width);
                dimensions.set_index((stream * 2 + 1) as u32, height);
                pixels.push(&array);
            }
            let chunk = Object::new();
            set(&chunk, "metadata", &Uint8Array::from(metadata.as_slice()));
            set(&chunk, "dimensions", &dimensions);
            set(&chunk, "pixels", &pixels);
            chunks[job.chunk] = Some(chunk.into());
            done += 1;
            let _ = on_progress.call2(&JsValue::NULL, &(done as u32).into(), &(count as u32).into());
        }
    }
    Ok(PreloadedChunks {
        chunks: chunks
            .into_iter()
            .enumerate()
            .filter_map(|(file, chunk)| chunk.map(|chunk| (file, chunk)))
            .collect(),
    })
}

/// Decode workers for this machine (`logical_cores` from `navigator.hardwareConcurrency`) or the scene's
/// `chunk_decoder_workers` setting (`"auto"`, absent, or an integer from 1 to 8).
#[wasm_bindgen]
pub fn chunk_decoder_count(logical_cores: f64, configured: JsValue) -> Result<u32, JsValue> {
    let cores = (logical_cores.is_finite() && logical_cores >= 1.0).then_some(logical_cores as u32);
    let configured = if configured.is_undefined() || configured.is_null() || configured.as_string().as_deref() == Some("auto") {
        None
    } else {
        match configured.as_f64() {
            Some(value) if value.fract() == 0.0 && value >= 0.0 => Some(value as u32),
            _ => return Err(js_error("Chunk decoder workers must be auto or an integer from 1 to 8.")),
        }
    };
    chunk_jobs::decoder_count(cores, configured).map_err(js_error)
}
