//! Browser persistence of gameplay settings and the packing of script visuals for the renderer. Lua and the ECS own
//! what the settings mean; this stores accepted values and restores them into the scene, and turns script
//! snapshots (impacts, projectile traces, hitbox lines) into the renderer's packed arrays.

use crate::Viewer;
use countersplat_scene::host_preferences;
use js_sys::{Array, JSON};
use serde_json::{json, Value};
use wasm_bindgen::prelude::*;

/// Saved settings: (kind, storage key, ECS component restored on the player or its own entity).
const SETTINGS: [(&str, &str); 6] = [
    ("player_controls", host_preferences::PLAYER_CONTROLS_KEY),
    ("camera_lens", "countersplat.camera-lens.v1"),
    ("camera_zoom_lens", "countersplat.camera-zoom-lens.v1"),
    ("camera_controller", "countersplat.camera-controller.v1"),
    ("image_correction", "countersplat.image-correction.v1"),
    ("weapon_optic", "countersplat.weapon-optic.first-person.v1"),
];
const MAX_SAVED_BYTES: usize = 16384;

fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

fn load(key: &str) -> Option<String> {
    storage()?.get_item(key).ok().flatten()
}

fn store(key: &str, value: &str) -> bool {
    storage().is_some_and(|s| s.set_item(key, value).is_ok())
}

fn key_for(kind: &str) -> Option<&'static str> {
    SETTINGS.iter().find(|(name, _)| *name == kind).map(|(_, key)| *key)
}

fn to_json(value: &JsValue) -> Value {
    JSON::stringify(value)
        .ok()
        .and_then(|text| text.as_string())
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(Value::Null)
}

fn warn(message: String) {
    web_sys::console::warn_1(&message.into());
}

/// Stores an accepted setting from a scene output event (`kind` from the settings table). The weapon optic keeps
/// only its kind and magnification, and only for the equipped weapon (`active_weapon`).
#[wasm_bindgen(js_name = saveSetting)]
pub fn save_setting(kind: &str, value: JsValue, active_weapon: JsValue) {
    let Some(key) = key_for(kind) else {
        return;
    };
    let mut value = to_json(&value);
    if kind == "weapon_optic" {
        if value.get("entity") != Some(&to_json(&active_weapon)) {
            return;
        }
        value = json!({"kind": value.get("kind"), "scope_magnification": value.get("scope_magnification")});
    }
    if !store(key, &value.to_string()) {
        warn(format!("Could not save {kind}"));
    }
}

/// The saved weapon optic (dispatched to Lua, which validates it), or null.
#[wasm_bindgen(js_name = savedWeaponOptic)]
pub fn saved_weapon_optic() -> JsValue {
    load(key_for("weapon_optic").unwrap_or_default())
        .filter(|raw| raw.len() <= 4096)
        .and_then(|raw| JSON::parse(&raw).ok())
        .unwrap_or(JsValue::NULL)
}

/// The saved UI layout for `scene` (`gameplay`, `menu`, `selector`), migrating the old gameplay layout once.
#[wasm_bindgen(js_name = savedLayout)]
pub fn saved_layout(scene: &str) -> Option<String> {
    let key = host_preferences::layout_key(scene);
    if let Some(saved) = load(&key) {
        return Some(saved);
    }
    if scene != "gameplay" {
        return None;
    }
    let migrated = host_preferences::migrate_gameplay_layout(load("countersplat.ui-layout.gameplay.v1").as_deref())?;
    store(&key, &migrated);
    Some(migrated)
}

#[wasm_bindgen(js_name = storeLayout)]
pub fn store_layout(scene: &str, layout: &str) {
    if !store(&host_preferences::layout_key(scene), layout) {
        warn("Could not save UI layout".into());
    }
}

impl Viewer {
    fn gameplay(&mut self, request: Value) -> Result<Value, String> {
        let reply = self.ui.script_operation(&request.to_string())?;
        Ok(serde_json::from_str::<Value>(&reply).ok().and_then(|v| v.get("value").cloned()).unwrap_or(Value::Null))
    }

    fn entity(&mut self, role: &str) -> Option<Value> {
        self.gameplay(json!({"op": "get_resource", "name": "scene.entities"}))
            .ok()?
            .get(role)
            .filter(|id| !id.is_null())
            .cloned()
    }

    fn saved_object(key: &str) -> Result<Option<Value>, String> {
        let Some(raw) = load(key).filter(|raw| !raw.is_empty()) else {
            return Ok(None);
        };
        if raw.len() > MAX_SAVED_BYTES {
            return Err("saved settings are too large".into());
        }
        let value: Value = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
        if !value.is_object() {
            return Err("saved settings must be an object".into());
        }
        Ok(Some(value))
    }

    fn restore_component(&mut self, role: &str, component: &str, key: &str, merge: fn(Value, Value) -> Value) -> Result<(), String> {
        let Some(saved) = Self::saved_object(key)? else {
            return Ok(());
        };
        let Some(id) = self.entity(role) else {
            return Ok(());
        };
        let current = self.gameplay(json!({"op": "get", "id": id, "name": component}))?;
        if current.is_null() && component != "CameraLens" && component != "CameraZoomLens" {
            return Err(format!("no current {component}"));
        }
        let value = merge(current, saved);
        self.gameplay(json!({"op": "set", "id": id, "name": component, "value": value}))?;
        Ok(())
    }

    fn restore_controls(&mut self) -> Result<(), String> {
        let current = load(host_preferences::PLAYER_CONTROLS_KEY);
        let legacy = if current.is_none() { load(host_preferences::LEGACY_PLAYER_CONTROLS_KEY) } else { None };
        if current.is_none() && legacy.is_none() {
            return Ok(());
        }
        let defaults = self.gameplay(json!({"op": "get_resource", "name": "player.default_controls"}))?;
        let sprint = defaults.get("sprint_multiplier").and_then(Value::as_f64);
        let Some((settings, migrated)) =
            host_preferences::read_controls_preference(current.as_deref(), legacy.as_deref(), sprint)?
        else {
            return Ok(());
        };
        let Some(player) = self.entity("player") else {
            return Ok(());
        };
        self.gameplay(json!({"op": "set", "id": player, "name": "PlayerControls", "value": settings}))?;
        // Keep version 1 intact; write version 2 only after the ECS accepted the restored component.
        if migrated {
            store(host_preferences::PLAYER_CONTROLS_KEY, &settings.to_string());
        }
        Ok(())
    }
}

fn saved_wins(_current: Value, saved: Value) -> Value {
    saved
}

/// Camera tuning is a preference; the target stays the scene's.
fn keep_target(current: Value, mut saved: Value) -> Value {
    if let Some(object) = saved.as_object_mut() {
        object.insert("target".into(), current.get("target").cloned().unwrap_or(Value::Null));
    }
    saved
}

/// Older saved grades predate fields such as sharpness: keep their values and take missing ones from the scene.
fn fill_missing(current: Value, saved: Value) -> Value {
    let mut merged = current;
    if let (Some(base), Some(saved)) = (merged.as_object_mut(), saved.as_object()) {
        for (key, value) in saved {
            base.insert(key.clone(), value.clone());
        }
    }
    merged
}

#[wasm_bindgen]
impl Viewer {
    /// Restores the saved player controls, camera lens and zoom lens, camera controller tuning and image correction
    /// into the gameplay scene. Each one that fails is reported and skipped.
    #[wasm_bindgen(js_name = restoreSavedSettings)]
    pub fn restore_saved_settings(&mut self) {
        if let Err(error) = self.restore_controls() {
            warn(format!("Could not restore player controls: {error}"));
        }
        let restores: [(&str, &str, &str, fn(Value, Value) -> Value); 4] = [
            ("player", "CameraLens", "camera_lens", saved_wins),
            ("player", "CameraZoomLens", "camera_zoom_lens", saved_wins),
            ("camera_controller", "CameraController", "camera_controller", keep_target),
            ("image_correction", "ImageCorrection", "image_correction", fill_missing),
        ];
        for (role, component, kind, merge) in restores {
            let key = key_for(kind).unwrap_or_default();
            if let Err(error) = self.restore_component(role, component, key, merge) {
                warn(format!("Could not restore {component}: {error}"));
            }
        }
    }

    /// Packs the latest impact snapshot (`{decals, smoke}`) for the impact renderer; returns `{decals, smoke}`
    /// counts.
    #[wasm_bindgen(js_name = applyImpactVisuals)]
    pub fn apply_impact_visuals(&mut self, snapshot: JsValue) -> Result<JsValue, JsValue> {
        let snapshot = to_json(&snapshot);
        let list = |name: &str, max: usize| -> Vec<Value> {
            snapshot.get(name).and_then(Value::as_array).map_or_else(Vec::new, |a| a.iter().take(max).cloned().collect())
        };
        let (decals, smoke) = (list("decals", 96), list("smoke", 160));
        let mut packed: Vec<f32> = Vec::new();
        for (kind, sprites) in [(0.0f32, &decals), (1.0, &smoke)] {
            for sprite in sprites {
                let vec3 = |name: &str| -> Option<[f32; 3]> {
                    let a = sprite.get(name)?.as_array()?;
                    if a.len() != 3 {
                        return None;
                    }
                    Some([a[0].as_f64()? as f32, a[1].as_f64()? as f32, a[2].as_f64()? as f32])
                };
                let scalar = |name: &str| sprite.get(name).and_then(Value::as_f64).map(|v| v as f32);
                let (Some(p), Some(n), Some(radius), Some(opacity), Some(variation)) =
                    (vec3("position"), vec3("normal"), scalar("radius"), scalar("opacity"), scalar("variation"))
                else {
                    continue;
                };
                if !p.iter().chain(&n).chain([radius, opacity, variation].iter()).all(|v| v.is_finite()) {
                    continue;
                }
                packed.extend_from_slice(&[p[0], p[1], p[2], radius, n[0], n[1], n[2], kind, opacity, variation, 0.0, 0.0]);
            }
        }
        self.set_impact_sprites(packed)?;
        Ok(JSON::parse(&json!({"decals": decals.len(), "smoke": smoke.len()}).to_string()).unwrap_or(JsValue::NULL))
    }

    /// Packs the latest projectile trace snapshot (`{segments: [{start, finish, alpha}]}`, at most 256).
    #[wasm_bindgen(js_name = applyProjectileVisuals)]
    pub fn apply_projectile_visuals(&mut self, snapshot: JsValue) -> Result<(), JsValue> {
        let snapshot = to_json(&snapshot);
        let mut packed = Vec::new();
        for segment in snapshot.get("segments").and_then(Value::as_array).into_iter().flatten().take(256) {
            let number = |v: &Value| v.as_f64().unwrap_or(f64::NAN) as f32;
            for name in ["start", "finish"] {
                for v in segment.get(name).and_then(Value::as_array).into_iter().flatten() {
                    packed.push(number(v));
                }
            }
            packed.push(segment.get("alpha").map_or(f32::NAN, number));
        }
        self.set_projectile_traces(packed)
    }

    /// Applies hitbox line snapshots from scene scripts to the world-line layers and redraws them when one changed.
    #[wasm_bindgen(js_name = applyHitboxVisuals)]
    pub fn apply_hitbox_visuals(&mut self, events: Array) -> Result<(), JsValue> {
        let mut changed = false;
        for event in events.iter() {
            changed |= self.line_layers.update(&to_json(&event));
        }
        if changed {
            let packed = self.line_layers.packed();
            self.set_hitbox_debug_lines_colored(packed)?;
        }
        Ok(())
    }

    #[wasm_bindgen(js_name = clearHitboxVisuals)]
    pub fn clear_hitbox_visuals(&mut self) {
        self.line_layers.clear();
    }
}

