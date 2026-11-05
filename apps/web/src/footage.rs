//! Browser side of first-person footage streaming: the scheduling is `countersplat_scene::first_person_frames`; this
//! fetches and decodes frames as `ImageBitmap`s and applies the scheduler's effects to the viewer.

use countersplat_scene::first_person_frames::{Effect, FirstPersonFrames as Scheduler, FrameRequest};
use js_sys::{Array, Function, Object, Reflect, JSON};
use std::{cell::RefCell, rc::Rc};
use wasm_bindgen::{prelude::*, JsCast};
use wasm_bindgen_futures::{spawn_local, JsFuture};

fn get(object: &JsValue, key: &str) -> JsValue {
    Reflect::get(object, &JsValue::from_str(key)).unwrap_or(JsValue::UNDEFINED)
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

fn from_json(value: &serde_json::Value) -> JsValue {
    JSON::parse(&value.to_string()).unwrap_or(JsValue::NULL)
}

fn call(target: &JsValue, method: &str, args: &[JsValue]) -> JsValue {
    let Ok(function) = get(target, method).dyn_into::<Function>() else {
        return JsValue::UNDEFINED;
    };
    function.apply(target, &args.iter().collect::<Array>()).unwrap_or(JsValue::UNDEFINED)
}

struct Inner {
    scheduler: Scheduler<JsValue>,
    assets: JsValue,
    base: String,
    origin: String,
    preloaded: JsValue,
    viewer: JsValue,
    abort: web_sys::AbortController,
}

impl Inner {
    fn apply(&mut self, effects: Vec<Effect<JsValue>>) {
        let mut followups = Vec::new();
        for effect in effects {
            match effect {
                Effect::Settings([opacity, threshold, softness, despill, offset], visible) => {
                    call(&self.viewer, "set_first_person_settings",
                        &[visible.into(), opacity.into(), threshold.into(), softness.into(), despill.into(), offset.into()]);
                }
                Effect::Install(bitmap) => {
                    call(&self.viewer, "install_first_person_frame", &[bitmap]);
                }
                Effect::Capture => {
                    if call(&self.viewer, "capture_first_person_frame", &[]).as_bool() != Some(true) {
                        self.scheduler.capture_failed(&mut followups);
                    }
                }
                Effect::Motion([x, y, roll, blend]) => {
                    call(&self.viewer, "set_first_person_motion", &[x.into(), y.into(), roll.into(), blend.into()]);
                }
                Effect::Close(bitmap) => {
                    call(&bitmap, "close", &[]);
                }
            }
        }
        if !followups.is_empty() {
            self.apply(followups);
        }
    }

    /// The asset URL of frame `id`, which must be a declared first-person frame on this origin.
    fn url(&self, id: &str) -> Result<String, String> {
        let asset = get(&self.assets, id);
        if asset.is_undefined() || get(&asset, "kind").as_string().as_deref() != Some("first_person_frame") {
            return Err(format!("Undeclared first-person frame {id}"));
        }
        let uri = get(&asset, "uri").as_string().unwrap_or_default();
        let url = web_sys::Url::new_with_base(&uri, &self.base).map_err(|_| "Invalid frame URL".to_string())?;
        if url.origin() != self.origin {
            return Err("First-person frames must be local scene assets".into());
        }
        Ok(url.href())
    }
}

async fn decode(url: String, preloaded: JsValue, signal: web_sys::AbortSignal) -> Result<JsValue, JsValue> {
    let global = js_sys::global();
    let options = Object::new();
    Reflect::set(&options, &"premultiplyAlpha".into(), &"none".into())?;
    let bytes = call(&preloaded, "get", &[url.clone().into()]);
    let blob = if bytes.is_undefined() || bytes.is_null() {
        let init = Object::new();
        Reflect::set(&init, &"signal".into(), &signal)?;
        let response = JsFuture::from(js_sys::Promise::from(call(&global, "fetch", &[url.clone().into(), init.into()]))).await?;
        if get(&response, "ok").as_bool() != Some(true) {
            let path = web_sys::Url::new(&url).map(|u| u.pathname()).unwrap_or(url.clone());
            return Err(JsValue::from_str(&format!("HTTP {} for {path}", get(&response, "status").as_f64().unwrap_or(0.0))));
        }
        JsFuture::from(js_sys::Promise::from(call(&response, "blob", &[]))).await?
    } else {
        let parts = Array::of1(&bytes);
        let kind = Object::new();
        Reflect::set(&kind, &"type".into(), &"image/webp".into())?;
        let constructor: Function = get(&global, "Blob").dyn_into()?;
        Reflect::construct(&constructor, &Array::of2(&parts, &kind))?
    };
    JsFuture::from(js_sys::Promise::from(call(&global, "createImageBitmap", &[blob, options.into()]))).await
}

fn pump(inner: &Rc<RefCell<Inner>>) {
    let loads = inner.borrow_mut().scheduler.take_loads();
    for id in loads {
        let (url, preloaded, signal) = {
            let state = inner.borrow();
            (state.url(&id), state.preloaded.clone(), state.abort.signal())
        };
        let inner = inner.clone();
        spawn_local(async move {
            let result = match url {
                Ok(url) => decode(url, preloaded, signal).await.map_err(|error| {
                    error.as_string().or_else(|| error.dyn_ref::<js_sys::Error>().map(|e| String::from(e.message()))).unwrap_or_default()
                }),
                Err(error) => Err(error),
            };
            if let Err(error) = &result {
                if !error.contains("abort") {
                    web_sys::console::warn_1(&format!("First-person frame: {error}").into());
                }
            }
            let mut effects = Vec::new();
            inner.borrow_mut().scheduler.loaded(&id, result, &mut effects);
            inner.borrow_mut().apply(effects);
            pump(&inner);
        });
    }
}

/// First-person footage for the gameplay viewer: `scene` declares the frame assets, `manifest` the clips,
/// `preloaded` maps URLs to bytes already fetched, and `viewer` receives the frames.
#[wasm_bindgen]
pub struct FirstPersonFrames(Rc<RefCell<Inner>>);

#[wasm_bindgen]
impl FirstPersonFrames {
    #[wasm_bindgen(constructor)]
    pub fn new(scene: JsValue, base: &str, manifest: JsValue, preloaded: JsValue, viewer: JsValue) -> Result<FirstPersonFrames, JsValue> {
        let base_url = web_sys::Url::new(base)?;
        let inner = Rc::new(RefCell::new(Inner {
            scheduler: Scheduler::new(&to_json(&manifest)),
            assets: get(&get(&scene, "resources"), "assets"),
            base: base_url.href(),
            origin: web_sys::window().map(|w| w.location().origin().unwrap_or_default()).unwrap_or_default(),
            preloaded,
            viewer,
            abort: web_sys::AbortController::new()?,
        }));
        pump(&inner);
        Ok(Self(inner))
    }

    /// The installed frame's presentation for the HUD.
    pub fn presentation(&self) -> JsValue {
        from_json(&self.0.borrow().scheduler.presentation())
    }

    /// Advance clip blending (milliseconds).
    pub fn update(&mut self, now: f64) {
        let mut effects = Vec::new();
        self.0.borrow_mut().scheduler.update(now, &mut effects);
        self.0.borrow_mut().apply(effects);
    }

    /// Lua's frames since the last submission; returns the frame requested now, or null.
    #[wasm_bindgen(js_name = requestBatch)]
    pub fn request_batch(&mut self, frames: Array) -> JsValue {
        let frames: Vec<FrameRequest> = frames.iter().map(|frame| FrameRequest { raw: to_json(&frame) }).collect();
        let mut effects = Vec::new();
        let chosen = self.0.borrow_mut().scheduler.request_batch(frames, &mut effects);
        self.0.borrow_mut().apply(effects);
        pump(&self.0);
        chosen.map_or(JsValue::NULL, |frame| from_json(&frame.raw))
    }

    /// After a real canvas submission; returns the newest frame requested once a held flash was drawn, or null.
    pub fn presented(&mut self) -> JsValue {
        let mut effects = Vec::new();
        let next = self.0.borrow_mut().scheduler.presented(&mut effects);
        self.0.borrow_mut().apply(effects);
        pump(&self.0);
        next.map_or(JsValue::NULL, |frame| from_json(&frame.raw))
    }

    pub fn close(&mut self) {
        let mut effects = Vec::new();
        let mut state = self.0.borrow_mut();
        state.abort.abort();
        state.scheduler.close(&mut effects);
        state.apply(effects);
    }
}
