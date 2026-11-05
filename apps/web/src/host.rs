//! Browser bindings for host logic that lives in Rust crates: profiling counters, gameplay pointer state and pointer
//! lock, world-line layers, stored preferences, gameplay event channels, and asset-loading policy. Each type keeps
//! the JavaScript name the page already used, so the page only forwards browser events and values.

use crate::{js_error, Viewer};
use countersplat_core::profile;
use countersplat_scene::{asset_policy, host_preferences, line_layers};
use countersplat_ui::pointer::{GameplayButton, PointerLock, PointerSample};
use js_sys::{Array, Function, Reflect, JSON};
use wasm_bindgen::{prelude::*, JsCast};

fn now() -> f64 {
    web_sys::window()
        .and_then(|window| window.performance())
        .map_or_else(js_sys::Date::now, |performance| performance.now())
}

/// A JavaScript value as JSON (`undefined` and functions become null).
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

fn get(object: &JsValue, key: &str) -> JsValue {
    Reflect::get(object, &JsValue::from_str(key)).unwrap_or(JsValue::UNDEFINED)
}

fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

// ------------------------------------------------------------------------------------------------- profiling

#[wasm_bindgen]
pub struct FrameProfiler(profile::FrameProfiler);

#[wasm_bindgen]
impl FrameProfiler {
    #[wasm_bindgen(constructor)]
    pub fn new(capacity: Option<u32>) -> Result<FrameProfiler, JsValue> {
        profile::FrameProfiler::new(capacity.unwrap_or(600) as usize).map(Self).map_err(js_error)
    }
    pub fn reset(&mut self) {
        self.0.reset();
    }
    pub fn record(&mut self, timestamp: f64, submitted: bool, render_ms: f64, scripts_ms: f64, callback_ms: f64, stages: JsValue) {
        let stages = to_json(&stages);
        let empty = serde_json::Map::new();
        self.0.record(timestamp, submitted, render_ms, scripts_ms, callback_ms, stages.as_object().unwrap_or(&empty));
    }
    pub fn snapshot(&self, state: JsValue, lua: JsValue) -> JsValue {
        from_json(&self.0.snapshot(&to_json(&state), to_json(&lua)))
    }
}

#[wasm_bindgen]
pub struct InputFrameLatency(profile::InputFrameLatency);

#[wasm_bindgen]
impl InputFrameLatency {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Result<InputFrameLatency, JsValue> {
        profile::InputFrameLatency::new(600, 512).map(Self).map_err(js_error)
    }
    #[wasm_bindgen(js_name = recordInput)]
    pub fn record_input(&mut self) {
        self.0.record_input(now());
    }
    #[wasm_bindgen(js_name = markDispatched)]
    pub fn mark_dispatched(&mut self) {
        self.0.mark_dispatched(now());
    }
    #[wasm_bindgen(js_name = markSubmitted)]
    pub fn mark_submitted(&mut self) {
        self.0.mark_submitted(now());
    }
    #[wasm_bindgen(js_name = clearPending)]
    pub fn clear_pending(&mut self) {
        self.0.clear_pending();
    }
    pub fn reset(&mut self) {
        self.0.reset();
    }
    pub fn snapshot(&self) -> JsValue {
        from_json(&self.0.snapshot())
    }
}

// --------------------------------------------------------------------------------------------- pointer state

fn sample(event: &JsValue) -> (String, i32, u32, String) {
    (
        get(event, "type").as_string().unwrap_or_default(),
        get(event, "button").as_f64().unwrap_or(-1.0) as i32,
        get(event, "buttons").as_f64().unwrap_or(0.0) as u32,
        get(event, "pointerType").as_string().unwrap_or_default(),
    )
}

/// One gameplay mouse button (right: zoom, left: fire); `emit(pressed)` reports changes.
struct ButtonBinding {
    state: GameplayButton,
    emit: Function,
}

impl ButtonBinding {
    fn update(&mut self, event: &JsValue, allow_press: bool) -> bool {
        let (kind, button, buttons, pointer_type) = sample(event);
        let change = self.state.update(
            PointerSample { kind: &kind, button, buttons, pointer_type: &pointer_type },
            allow_press,
        );
        if let Some(pressed) = change {
            let _ = self.emit.call1(&JsValue::NULL, &pressed.into());
        }
        change == Some(true)
    }
    fn reset(&mut self) {
        if let Some(pressed) = self.state.reset() {
            let _ = self.emit.call1(&JsValue::NULL, &pressed.into());
        }
    }
}

#[wasm_bindgen]
pub struct GameplayMouseButtons(ButtonBinding);

#[wasm_bindgen]
impl GameplayMouseButtons {
    #[wasm_bindgen(constructor)]
    pub fn new(emit: Function) -> Self {
        Self(ButtonBinding { state: GameplayButton::right(), emit })
    }
    pub fn update(&mut self, event: JsValue, allow_press: bool) -> bool {
        self.0.update(&event, allow_press)
    }
    pub fn reset(&mut self) {
        self.0.reset();
    }
    #[wasm_bindgen(getter, js_name = rightDown)]
    pub fn right_down(&self) -> bool {
        self.0.state.down
    }
}

#[wasm_bindgen]
pub struct GameplayFireButton(ButtonBinding);

#[wasm_bindgen]
impl GameplayFireButton {
    #[wasm_bindgen(constructor)]
    pub fn new(emit: Function) -> Self {
        Self(ButtonBinding { state: GameplayButton::left(), emit })
    }
    pub fn update(&mut self, event: JsValue, allow_press: bool) {
        self.0.update(&event, allow_press);
    }
    pub fn reset(&mut self) {
        self.0.reset();
    }
}

/// Pointer lock on the canvas, including cancelling a request still in flight.
#[wasm_bindgen]
pub struct MouseCapture {
    canvas: web_sys::Element,
    document: web_sys::Document,
    state: std::rc::Rc<std::cell::RefCell<PointerLock>>,
}

#[wasm_bindgen]
impl MouseCapture {
    #[wasm_bindgen(constructor)]
    pub fn new(canvas: web_sys::Element, document: web_sys::Document) -> Self {
        Self { canvas, document, state: Default::default() }
    }
    #[wasm_bindgen(getter)]
    pub fn locked(&self) -> bool {
        self.document.pointer_lock_element().is_some_and(|element| element == self.canvas)
    }
    #[wasm_bindgen(getter)]
    pub fn requested(&self) -> bool {
        self.state.borrow().requested
    }
    #[wasm_bindgen(getter)]
    pub fn failed(&self) -> bool {
        self.state.borrow().failed
    }
    /// Must stay synchronous with the user's click.
    pub fn request(&mut self) {
        let sequence = self.state.borrow_mut().request();
        let method = get(&self.canvas, "requestPointerLock");
        let Some(method) = method.dyn_ref::<Function>() else {
            self.state.borrow_mut().error();
            return;
        };
        match method.call0(&self.canvas) {
            Ok(result) => {
                if let Ok(promise) = result.dyn_into::<js_sys::Promise>() {
                    let state = self.state.clone();
                    let on_reject = Closure::<dyn FnMut(JsValue)>::new(move |_: JsValue| state.borrow_mut().rejected(sequence));
                    let _ = promise.catch(&on_reject);
                    // One small closure per click-driven request; it must outlive this call.
                    on_reject.forget();
                }
            }
            Err(_) => self.state.borrow_mut().error(),
        }
    }
    pub fn error(&mut self) {
        self.state.borrow_mut().error();
    }
    pub fn release(&mut self) {
        let locked = self.locked();
        if self.state.borrow_mut().release(locked) {
            self.document.exit_pointer_lock();
        }
    }
    /// On `pointerlockchange`: `locked`, `cancelled`, `unlocked`, `released` or `unexpected`.
    pub fn changed(&mut self, allow_capture: bool) -> String {
        let change = self.state.borrow_mut().changed(self.locked(), allow_capture);
        if change == countersplat_ui::pointer::LockChange::Cancelled {
            self.document.exit_pointer_lock();
        }
        change.name().to_string()
    }
}

// --------------------------------------------------------------------------------------------- line layers

#[wasm_bindgen]
pub struct LineLayers(line_layers::LineLayers);

#[wasm_bindgen]
impl LineLayers {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self(Default::default())
    }
    pub fn clear(&mut self) {
        self.0.clear();
    }
    pub fn update(&mut self, snapshot: JsValue) -> bool {
        self.0.update(&to_json(&snapshot))
    }
    pub fn packed(&self) -> Vec<f32> {
        self.0.packed()
    }
}

impl Default for LineLayers {
    fn default() -> Self {
        Self::new()
    }
}

// --------------------------------------------------------------------------------------------- preferences

#[wasm_bindgen(js_name = layoutKey)]
pub fn layout_key(scene: &str) -> String {
    host_preferences::layout_key(scene)
}

#[wasm_bindgen(js_name = migrateGameplayLayout)]
pub fn migrate_gameplay_layout(raw: Option<String>) -> Option<String> {
    host_preferences::migrate_gameplay_layout(raw.as_deref())
}

/// The saved player controls from storage, `{settings, migrated}` or null.
#[wasm_bindgen(js_name = loadControlsPreference)]
pub fn load_controls_preference(scene_sprint_default: Option<f64>) -> Result<JsValue, JsValue> {
    let storage = storage();
    let read = |key: &str| storage.as_ref().and_then(|s| s.get_item(key).ok().flatten());
    let current = read(host_preferences::PLAYER_CONTROLS_KEY);
    let legacy = read(host_preferences::LEGACY_PLAYER_CONTROLS_KEY);
    Ok(match host_preferences::read_controls_preference(current.as_deref(), legacy.as_deref(), scene_sprint_default)
        .map_err(js_error)?
    {
        Some((settings, migrated)) => from_json(&serde_json::json!({"settings": settings, "migrated": migrated})),
        None => JsValue::NULL,
    })
}

#[wasm_bindgen(js_name = saveControlsPreference)]
pub fn save_controls_preference(settings: JsValue) {
    if let Some(storage) = storage() {
        let _ = storage.set_item(host_preferences::PLAYER_CONTROLS_KEY, &to_json(&settings).to_string());
    }
}

#[wasm_bindgen(js_name = gameplayInputChannels)]
pub fn gameplay_input_channels() -> Array {
    host_preferences::GAMEPLAY_INPUT_CHANNELS.iter().map(|name| JsValue::from_str(name)).collect()
}

#[wasm_bindgen(js_name = gameplayEventChannels)]
pub fn gameplay_event_channels() -> Array {
    host_preferences::GAMEPLAY_EVENT_CHANNELS.iter().map(|name| JsValue::from_str(name)).collect()
}

impl Viewer {
    fn set_gameplay_resource(&mut self, name: &str, value: serde_json::Value) -> Result<(), String> {
        let request = serde_json::json!({"op": "set_resource", "name": name, "value": value}).to_string();
        self.ui.script_operation(&request).map(|_| ())
    }
}

#[wasm_bindgen]
impl Viewer {
    /// Publishes the selected level's player scale and its saved rig (from storage) to the gameplay scene. Lua
    /// decides what they mean.
    #[wasm_bindgen(js_name = publishPlayerRigData)]
    pub fn publish_player_rig_data(&mut self, level: JsValue) {
        let level = to_json(&level);
        let result = (|| -> Result<(), String> {
            if let Some(scale) = level.get("player_units_per_meter").filter(|v| !v.is_null()) {
                self.set_gameplay_resource("player.level_units_per_meter", scale.clone())?;
            }
            let id = level.get("id").and_then(serde_json::Value::as_str).unwrap_or_default();
            let raw = storage().and_then(|s| s.get_item(&host_preferences::player_rig_key(id)).ok().flatten());
            if let Some(raw) = raw {
                if raw.len() > 4096 {
                    return Err("Saved player rig is too large.".into());
                }
                let rig: serde_json::Value = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
                self.set_gameplay_resource("player.saved_rig", rig)?;
            }
            Ok(())
        })();
        if let Err(error) = result {
            web_sys::console::warn_1(&format!("Could not restore player rig: {error}").into());
        }
    }
}

#[wasm_bindgen(js_name = savePlayerRig)]
pub fn save_player_rig(level: JsValue, rig: JsValue) {
    let id = get(&level, "id").as_string().unwrap_or_default();
    if let Some(storage) = storage() {
        if storage.set_item(&host_preferences::player_rig_key(&id), &to_json(&rig).to_string()).is_err() {
            web_sys::console::warn_1(&"Could not save player rig".into());
        }
    }
}

// ------------------------------------------------------------------------------------------- asset policy

#[wasm_bindgen(js_name = loadPolicy)]
pub fn load_policy(value: JsValue) -> Result<String, JsValue> {
    let value = to_json(&value);
    asset_policy::load_policy(Some(&value)).map(str::to_string).map_err(js_error)
}

#[wasm_bindgen(js_name = levelPolicies)]
pub fn level_policies(value: JsValue) -> Result<JsValue, JsValue> {
    let value = to_json(&value);
    let p = asset_policy::level_policies(Some(&value)).map_err(js_error)?;
    Ok(from_json(&serde_json::json!({"manifest": p.manifest, "viewer_settings": p.viewer_settings,
        "chunk_metadata": p.chunk_metadata, "splat_chunks": p.splat_chunks})))
}

/// The scene's scripts to fetch: `[{id, url, execute}]`, each URL resolved against `base` and required to be on
/// `origin`.
#[wasm_bindgen(js_name = planScripts)]
pub fn plan_scripts(scene: JsValue, base: &web_sys::Url, origin: &str) -> Result<Array, JsValue> {
    let plan = asset_policy::plan_scripts(&to_json(&scene)).map_err(js_error)?;
    let out = Array::new();
    for script in plan {
        let url = web_sys::Url::new_with_base(&script.uri, &base.href())?;
        if url.origin() != origin {
            return Err(js_error(format!("Script {} must be local to this preview.", script.id)));
        }
        let item = js_sys::Object::new();
        Reflect::set(&item, &"id".into(), &script.id.into())?;
        Reflect::set(&item, &"url".into(), &url)?;
        Reflect::set(&item, &"execute".into(), &script.execute.into())?;
        out.push(&item);
    }
    Ok(out)
}

/// Resolves a manifest-rooted path inside this deployment's project directory (localhost, a repository subpath or
/// a custom domain): `/x` is relative to the project root, anything else to `relative_to`. Escaping the root fails.
#[wasm_bindgen(js_name = projectResourceUrl)]
pub fn project_resource_url(uri: &str, project_base: &str, relative_to: Option<String>) -> Result<web_sys::Url, JsValue> {
    let root = web_sys::Url::new(project_base)?;
    if uri.is_empty() || uri.starts_with("//") || !root.pathname().ends_with('/') {
        return Err(js_error("Invalid project resource URI."));
    }
    let url = match uri.strip_prefix('/') {
        Some(rest) => web_sys::Url::new_with_base(rest, &root.href())?,
        None => web_sys::Url::new_with_base(uri, relative_to.as_deref().unwrap_or(project_base))?,
    };
    if url.origin() != root.origin() || !url.pathname().starts_with(&root.pathname()) {
        return Err(js_error("Project resource escapes its deployment root."));
    }
    Ok(url)
}

/// A scene choice: a file name in the project's scene directory, never an arbitrary URL.
/// The base URL of catalog level `id`: its validated publisher hosting (`hosted`), or `assets/levels/<id>/` beside
/// the page at `page_base`.
#[wasm_bindgen(js_name = levelBaseUrl)]
pub fn level_base_url(catalog: &[u8], id: &str, page_base: &str) -> Result<String, JsValue> {
    let catalog = countersplat_scene::LevelCatalog::from_json(catalog).map_err(js_error)?;
    let level = catalog.get(id).ok_or_else(|| js_error("That level is not in the catalog."))?;
    let site = web_sys::Url::new_with_base(".", page_base)?.href();
    Ok(level.base_url(&site))
}

#[wasm_bindgen(js_name = localSceneUrl)]
pub fn local_scene_url(filename: &str, project_base: &str) -> Result<web_sys::Url, JsValue> {
    let valid = filename.len() <= 85
        && filename.strip_suffix(".json").is_some_and(|stem| {
            (1..=80).contains(&stem.len()) && stem.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        });
    if !valid {
        return Err(js_error("Invalid scene filename."));
    }
    let base = web_sys::Url::new_with_base(".", project_base)?;
    project_resource_url(&format!("assets/scenes/{filename}"), &base.href(), None)
}

/// Monotonic loading progress; `report(phase, progress, completed, total)` receives each update.
#[wasm_bindgen]
pub struct LoadingProgress {
    inner: asset_policy::LoadingProgress,
    report: Function,
}

#[wasm_bindgen]
impl LoadingProgress {
    #[wasm_bindgen(constructor)]
    pub fn new(report: Function) -> Self {
        Self { inner: Default::default(), report }
    }
    pub fn update(&mut self, phase: &str, start: f64, end: f64, completed: f64, total: f64) -> Result<(), JsValue> {
        if completed.fract() != 0.0 || total.fract() != 0.0 || completed < 0.0 || total < 0.0 {
            return Err(js_error("Invalid asset-loading progress."));
        }
        let progress = self.inner.update(phase, start, end, completed as u64, total as u64).map_err(js_error)?;
        let args = Array::of4(&phase.into(), &progress.into(), &completed.into(), &total.into());
        self.report.apply(&JsValue::NULL, &args)?;
        Ok(())
    }
}
