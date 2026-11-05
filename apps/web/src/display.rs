//! Display and HUD preferences for the browser host: the policy is `countersplat_scene::display`; this persists it
//! in `localStorage` and applies it to the viewer (render resolution, presentation and the viewport resources scene
//! scripts read).

use crate::{js_error, Viewer};
use countersplat_scene::display::{self as policy, Size};
use wasm_bindgen::prelude::*;

const DISPLAY_KEY: &str = "countersplat.display.v1";
const HUD_KEY: &str = "countersplat.hud.show-performance.v1";

fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

fn load(key: &str) -> Option<String> {
    storage()?.get_item(key).ok().flatten()
}

fn save(key: &str, value: &str) {
    // Live settings still apply when storage is unavailable (private windows, blocked site data).
    if let Some(storage) = storage() {
        let _ = storage.set_item(key, value);
    }
}

fn parse(request: &str) -> Result<serde_json::Value, JsValue> {
    serde_json::from_str(request).map_err(js_error)
}

/// The display preferences (persisted) and the HUD's performance-overlay switch.
#[wasm_bindgen]
pub struct DisplaySettings {
    pub(crate) inner: policy::DisplaySettings,
    hud: policy::HudPreferences,
}

#[wasm_bindgen]
impl DisplaySettings {
    /// Loads both from `localStorage` (defaults when absent or unreadable).
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        Self {
            inner: policy::DisplaySettings::load(load(DISPLAY_KEY).as_deref()),
            hud: policy::HudPreferences::load(load(HUD_KEY).as_deref()),
        }
    }

    /// The preferences as JSON.
    pub fn preferences(&self) -> String {
        self.inner.to_json()
    }

    /// Applies a settings request (JSON); returns the error message, empty when accepted.
    pub fn set(&mut self, request: &str, device_limit: f64) -> Result<String, JsValue> {
        Ok(match self.inner.set(&parse(request)?, device_limit) {
            Ok(()) => {
                save(DISPLAY_KEY, &self.inner.to_json());
                String::new()
            }
            Err(error) => error,
        })
    }

    /// Switches aspect preset against the current viewport; returns the error message, empty when accepted.
    pub fn set_aspect(&mut self, aspect: &str, viewport_width: u32, viewport_height: u32, device_limit: f64) -> String {
        let viewport = Size { width: viewport_width, height: viewport_height };
        match self.inner.set_aspect(aspect, viewport, device_limit) {
            Ok(()) => {
                save(DISPLAY_KEY, &self.inner.to_json());
                String::new()
            }
            Err(error) => error,
        }
    }

    pub fn show_performance(&self) -> bool {
        self.hud.show_performance
    }

    /// Sets the overlay switch from a JSON value; false (and unchanged) unless it is a boolean.
    pub fn set_show_performance(&mut self, value: &str) -> bool {
        let accepted = serde_json::from_str(value).is_ok_and(|value| self.hud.set(&value));
        if accepted {
            save(HUD_KEY, if self.hud.show_performance { "true" } else { "false" });
        }
        accepted
    }
}

impl Default for DisplaySettings {
    fn default() -> Self {
        Self::new()
    }
}

/// The canvas pixel size for a CSS size: `[width, height]`.
#[wasm_bindgen]
pub fn display_viewport_size(css_width: f64, css_height: f64, device_pixel_ratio: f64, device_limit: f64) -> Vec<u32> {
    let size = policy::viewport_size(css_width, css_height, device_pixel_ratio, device_limit);
    vec![size.width, size.height]
}

/// Viewport state last published to the gameplay scene, so it is written only when it changes.
#[derive(Default, PartialEq, Clone)]
pub(crate) struct PublishedViewport {
    aspect: Option<f64>,
    viewport: Option<serde_json::Value>,
}

impl Viewer {
    fn publish_resource(&mut self, name: &str, value: serde_json::Value) {
        let request = serde_json::json!({"op": "set_resource", "name": name, "value": value}).to_string();
        let _ = self.ui.script_operation(&request);
    }

    /// A gameplay component of the entity with `role` in the scene's `scene.entities` map, or null.
    fn read_component(&mut self, entities: &serde_json::Value, role: &str, component: &str) -> serde_json::Value {
        let Some(id) = entities.get(role) else {
            return serde_json::Value::Null;
        };
        let request = serde_json::json!({"op": "get", "id": id, "name": component}).to_string();
        self.ui
            .script_operation(&request)
            .ok()
            .and_then(|json| serde_json::from_str::<serde_json::Value>(&json).ok())
            .and_then(|reply| reply.get("value").cloned())
            .unwrap_or(serde_json::Value::Null)
    }
}

#[wasm_bindgen]
impl Viewer {
    /// Applies the display preferences for a canvas of `output_width`x`output_height` pixels
    /// (`css_width`x`css_height` CSS points): the scene's render resolution and presentation, and, in gameplay, the
    /// `render.scene_aspect` and `render.viewport` resources scene scripts project UI with.
    #[allow(clippy::too_many_arguments)]
    pub fn apply_display(
        &mut self,
        settings: &DisplaySettings,
        css_width: f64,
        css_height: f64,
        output_width: u32,
        output_height: u32,
        gameplay: bool,
    ) -> Result<(), JsValue> {
        let output = Size { width: output_width, height: output_height };
        let preferences = &settings.inner.preferences;
        let render = policy::scene_resolution(preferences, output, policy::MAX_VIEWPORT_SCENE_PIXELS);
        self.set_render_resolution(render.width, render.height)?;
        self.set_viewport_scaling(&preferences.scaling)?;
        if !gameplay {
            return Ok(());
        }
        // Lua projects the filmed weapon sight through this render aspect.
        let aspect = f64::from(render.width) / f64::from(render.height);
        if self.published_viewport.aspect != Some(aspect) {
            self.publish_resource("render.scene_aspect", aspect.into());
            self.published_viewport.aspect = Some(aspect);
        }
        let mapping = policy::presentation_mapping(render, output, &preferences.scaling);
        let viewport = serde_json::json!({
            "width": css_width, "height": css_height, "scene_aspect": aspect, "scaling": preferences.scaling,
            "output_width": output.width, "output_height": output.height,
            "render_width": render.width, "render_height": render.height,
            "uv_scale": mapping.scale, "uv_offset": mapping.offset,
        });
        if self.published_viewport.viewport.as_ref() != Some(&viewport) {
            self.publish_resource("render.viewport", viewport.clone());
            self.published_viewport.viewport = Some(viewport);
        }
        Ok(())
    }

    /// The pause menu's settings payload (JSON): display and HUD preferences with resolution presets, the level and
    /// session summary, and the gameplay settings the menu edits, read from the gameplay scene.
    pub fn menu_settings(
        &mut self,
        settings: &DisplaySettings,
        viewport_width: u32,
        viewport_height: u32,
        level_name: Option<String>,
        level_format: Option<String>,
        error: &str,
    ) -> String {
        let viewport = Size { width: viewport_width, height: viewport_height };
        let display = settings
            .inner
            .menu_state(viewport, f64::from(self.max_texture_dimension()));
        let resource = |viewer: &Self, name: &str| -> serde_json::Value {
            viewer
                .ui
                .script_resource_value(name)
                .ok()
                .flatten()
                .and_then(|json| serde_json::from_str(&json).ok())
                .unwrap_or(serde_json::Value::Null)
        };
        let entities = resource(self, "scene.entities");
        let mode_state = resource(self, "game.mode_state");
        let level = self.read_component(&entities, "active_level", "LevelSettings");
        let assets = self.read_component(&entities, "active_level", "LevelAssets");
        let sky = self.read_component(&entities, "skybox", "Skybox");
        let layer = self.read_component(&entities, "audio_root", "AudioLayer");
        let controls = self.read_component(&entities, "player", "PlayerControls");
        let mode_config = self.read_component(&entities, "pursuit_mode", "PursuitConfig");
        let ballistics = self.read_component(&entities, "first_person", "Ballistics");
        let stats = self.stream.stats();
        let playtime = level_name
            .as_ref()
            .map(|_| self.total_ticks as f64 * self.scheduler.tick_seconds());
        let warning = display["warning"].as_str().unwrap_or_default().to_string();
        serde_json::json!({
            "display": display["display"],
            "show_performance": settings.hud.show_performance,
            "game": {"name": level_name, "format": level_format, "resident_splats": stats.resident_splats,
                "chunks_loaded": stats.chunks_loaded, "playtime_seconds": playtime},
            "controls": controls,
            "graphics": {"lod": level.get("lod"), "draw_distance": level.get("draw_distance"),
                "lod_count": assets.get("lod_count"), "sky_enabled": sky.get("enabled")},
            "audio": {"gain": layer.pointer("/mix/gain"), "muted": layer.pointer("/mix/muted")},
            "mode_state": mode_state,
            "mode_config": mode_config,
            "weapon_ballistics": ballistics,
            "aspects": display["aspects"],
            "presets": display["presets"],
            "error": if error.is_empty() { warning } else { error.to_string() },
        })
        .to_string()
    }
}
