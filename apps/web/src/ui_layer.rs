//! Example ECS-owned in-scene UI. Game behavior is kept in this host, not in the UI crates.

use crate::ui_layout::UiLayout;
use countersplat_ecs::{Entity, World};
use countersplat_scene::{
    LevelAssets, LevelSettings, PhysicsBody, PlayerCamera, PlayerControls, PlayerRig,
    SceneManifest, Skybox, SkyboxSnapshot, VoxelCollider, VoxelColliderState, VoxelStatus,
};
use countersplat_ui::{
    FrameCounter, FrameHistory, FrameMeasurement, FrameMeter, FrameUpdate, InputEvent, Rect,
    UiElement, UiEventKind, Widget,
};
use countersplat_ui_ecs::{apply_events, extract, register, update_frame_counters};
use countersplat_ui_wgpu::{UiRenderer, UiTarget};
use std::time::Duration;

/// Only controls changed by the user are sent back to the scene owner.
#[derive(Default)]
pub struct SkyEdits {
    pub enabled: Option<bool>,
    pub exposure: Option<f32>,
    pub rotation_degrees: Option<f32>,
}

pub struct SceneUi {
    world: World,
    renderer: UiRenderer,
    layout: UiLayout,
    player: Entity,
    level: Entity,
    skybox: Entity,
    viewport: Rect,
    animate: Entity,
    sway: Entity,
    reset_animate: bool,
    reset_sway: f32,
    reset: Entity,
    limit_enabled: Entity,
    frame_limit: Entity,
    sky_visible: Entity,
    sky_exposure: Entity,
    sky_rotation: Entity,
    sky_edits: SkyEdits,
    counter: Entity,
    graph: Entity,
    graph_scale: Entity,
    frame_meter: FrameMeter,
    frame_history: FrameHistory,
}

/// Resolve a configured UI role and check its widget type at startup.
fn binding(
    world: &World,
    entities: &std::collections::BTreeMap<String, Entity>,
    bindings: &serde_json::Map<String, serde_json::Value>,
    role: &str,
) -> Result<Entity, String> {
    let id = bindings
        .get(role)
        .and_then(|value| value.as_str())
        .ok_or_else(|| format!("preview UI binding `{role}` must be an entity ID"))?;
    let entity = *entities
        .get(id)
        .ok_or_else(|| format!("preview UI entity `{id}` is missing"))?;
    let element = world
        .get::<UiElement>(entity)
        .ok_or_else(|| format!("preview UI entity `{id}` lacks UiElement"))?;
    let valid = match role {
        "animate" | "limit_enabled" | "sky_visible" => {
            matches!(element.widget, Widget::Checkbox { .. })
        }
        "sway" | "frame_limit" | "sky_exposure" | "sky_rotation" => {
            matches!(element.widget, Widget::Slider { .. })
        }
        "reset" | "back_to_levels" => matches!(element.widget, Widget::Button { .. }),
        "counter" | "graph_scale" => matches!(element.widget, Widget::Text { .. }),
        "graph" => matches!(element.widget, Widget::LineGraph { .. }),
        _ => false,
    };
    if !valid || (role == "counter" && !world.has::<FrameCounter>(entity)) {
        return Err(format!(
            "preview UI entity `{id}` has the wrong components for `{role}`"
        ));
    }
    if let Widget::Slider { min, max, .. } = element.widget {
        let allowed = match role {
            "frame_limit" => min >= 1.0 && max <= 1000.0,
            "sky_exposure" => min >= 0.0 && max <= 64.0,
            "sky_rotation" => min >= -1_000_000.0 && max <= 1_000_000.0,
            _ => true,
        };
        if !allowed {
            return Err(format!(
                "preview UI slider `{id}` exceeds the allowed range for `{role}`"
            ));
        }
    }
    Ok(entity)
}

impl SceneUi {
    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        manifest: &SceneManifest,
    ) -> Result<Self, String> {
        let mut world = World::new();
        register(&mut world)?;
        countersplat_scene_ecs::register(&mut world)?;
        let entities = countersplat_scene_ecs::instantiate_layers(
            &mut world,
            manifest,
            &["ui", "player", "level", "scene"],
        )?;
        let player = *entities
            .get("player")
            .ok_or("gameplay scene needs a player entity")?;
        if !world.has::<PlayerCamera>(player) || !world.has::<PlayerControls>(player) {
            return Err("player entity needs PlayerCamera and PlayerControls".into());
        }
        let level_id = manifest
            .resources
            .values
            .get("preview")
            .and_then(|value| value.get("level_entity"))
            .and_then(|value| value.as_str())
            .ok_or("preview.level_entity must name a level entity")?;
        let level = *entities
            .get(level_id)
            .ok_or("gameplay scene needs the configured level entity")?;
        let assets = world
            .get::<LevelAssets>(level)
            .ok_or("level entity needs LevelAssets")?;
        if !world.has::<LevelSettings>(level)
            || !manifest
                .resources
                .assets
                .get(&assets.source)
                .is_some_and(|asset| asset.kind == "sog_level")
        {
            return Err("level entity needs LevelSettings and a declared sog_level asset".into());
        }
        let sky_id = manifest
            .resources
            .values
            .get("preview")
            .and_then(|value| value.get("skybox_entity"))
            .and_then(|value| value.as_str())
            .ok_or("preview.skybox_entity must name a scene entity")?;
        let skybox = *entities
            .get(sky_id)
            .ok_or("configured skybox entity is missing")?;
        if !world.has::<Skybox>(skybox) {
            return Err("configured skybox entity needs Skybox".into());
        }
        if !world.has::<PlayerRig>(player) {
            world.insert(player, PlayerRig::default())?;
        }
        if !world.has::<PhysicsBody>(player) {
            world.insert(player, PhysicsBody::default())?;
        }
        let default_rig =
            serde_json::to_value(world.get::<PlayerRig>(player).expect("inserted player rig"))
                .map_err(|error| error.to_string())?;
        world.set_dynamic_resource("player.default_rig", default_rig);
        let default_controls = world
            .get::<PlayerControls>(player)
            .expect("checked player controls");
        let default_controls = serde_json::to_value(default_controls).map_err(|e| e.to_string())?;
        world.set_dynamic_resource("player.default_controls", default_controls);
        let bindings = manifest
            .resources
            .values
            .get("preview")
            .and_then(|value| value.get("ui"))
            .and_then(|value| value.as_object())
            .ok_or("scene resource `preview.ui` must contain UI bindings")?;
        let get = |role| binding(&world, &entities, bindings, role);
        let animate = get("animate")?;
        let sway = get("sway")?;
        let reset = get("reset")?;
        if bindings.contains_key("back_to_levels") {
            get("back_to_levels")?;
        }
        let limit_enabled = get("limit_enabled")?;
        let frame_limit = get("frame_limit")?;
        let sky_visible = get("sky_visible")?;
        let sky_exposure = get("sky_exposure")?;
        let sky_rotation = get("sky_rotation")?;
        let counter = get("counter")?;
        let graph = get("graph")?;
        let graph_scale = get("graph_scale")?;
        let reset_animate = matches!(
            world.get::<UiElement>(animate).unwrap().widget,
            Widget::Checkbox { checked: true, .. }
        );
        let reset_sway = match world.get::<UiElement>(sway).unwrap().widget {
            Widget::Slider { value, .. } => value,
            _ => unreachable!("binding checked the widget"),
        };
        let layout = UiLayout::new(&world, entities);
        Ok(Self {
            world,
            renderer: UiRenderer::new(device, format),
            layout,
            player,
            level,
            skybox,
            viewport: Rect::new(0.0, 0.0, 1.0, 1.0),
            animate,
            sway,
            reset_animate,
            reset_sway,
            reset,
            limit_enabled,
            frame_limit,
            sky_visible,
            sky_exposure,
            sky_rotation,
            sky_edits: SkyEdits::default(),
            counter,
            graph,
            graph_scale,
            frame_meter: FrameMeter::new(),
            frame_history: FrameHistory::new(),
        })
    }

    /// Publish selected level references without moving asset bytes into the ECS world.
    pub fn set_level_assets(&mut self, assets: LevelAssets) -> Result<(), String> {
        assets.validate().map_err(str::to_owned)?;
        let previous = self
            .world
            .get::<LevelAssets>(self.level)
            .ok_or("level assets were removed")?;
        if previous.source != assets.source {
            return Err("level asset source does not match the scene".into());
        }
        self.world
            .get::<LevelSettings>(self.level)
            .ok_or("level settings were removed")?
            .validate_for(&assets)
            .map_err(str::to_owned)?;
        self.world.insert(self.level, assets)
    }
    pub fn level_lod(&self) -> Result<Option<u32>, String> {
        let assets = self
            .world
            .get::<LevelAssets>(self.level)
            .ok_or("level assets were removed")?;
        let settings = self
            .world
            .get::<LevelSettings>(self.level)
            .ok_or("level settings were removed")?;
        settings.validate_for(assets).map_err(str::to_owned)?;
        Ok(settings.lod)
    }
    pub fn level_draw_distance(&self) -> Result<f32, String> {
        let assets = self
            .world
            .get::<LevelAssets>(self.level)
            .ok_or("level assets were removed")?;
        let settings = self
            .world
            .get::<LevelSettings>(self.level)
            .ok_or("level settings were removed")?;
        settings.validate_for(assets).map_err(str::to_owned)?;
        Ok(settings.draw_distance)
    }
    pub fn show_voxels(&self) -> bool {
        self.world
            .get::<VoxelCollider>(self.level)
            .is_some_and(|settings| settings.enabled && settings.visible)
            && self
                .world
                .get::<VoxelColliderState>(self.level)
                .is_some_and(|state| state.status == VoxelStatus::Ready)
    }
    pub fn level_file_count(&self) -> usize {
        self.world
            .get::<LevelAssets>(self.level)
            .map_or(0, |assets| assets.files.len())
    }

    /// Seed the player pose from the selected level and publish the reset position to Lua.
    pub fn set_camera_spawn(
        &mut self,
        eye: [f32; 3],
        target: [f32; 3],
        fov: f32,
        use_level_camera: bool,
    ) -> Result<(), String> {
        let camera = if use_level_camera {
            PlayerCamera::from_view(eye, target, fov).map_err(str::to_owned)?
        } else {
            self.player_camera()?
        };
        self.world.insert(self.player, camera.clone())?;
        self.world.set_dynamic_resource(
            "player.spawn_camera",
            serde_json::to_value(camera).map_err(|e| e.to_string())?,
        );
        Ok(())
    }
    pub fn player_camera(&self) -> Result<PlayerCamera, String> {
        let camera = self
            .world
            .get::<PlayerCamera>(self.player)
            .ok_or("player camera was removed")?;
        camera.validate().map_err(str::to_owned)?;
        Ok(camera.clone())
    }
    pub fn skybox(&self) -> Option<&Skybox> {
        self.world.get::<Skybox>(self.skybox)
    }
    pub fn skybox_mut(&mut self) -> Option<&mut Skybox> {
        self.world.get_mut::<Skybox>(self.skybox)
    }
    pub fn selected_sky(&self) -> Option<SkyboxSnapshot> {
        countersplat_scene_ecs::extract(&self.world)
    }

    pub fn player_rig(&self) -> Option<PlayerRig> {
        self.world.get::<PlayerRig>(self.player).cloned()
    }
    pub fn player_physics(&self) -> Option<PhysicsBody> {
        self.world.get::<PhysicsBody>(self.player).cloned()
    }
    pub fn mouse_look_enabled(&self) -> bool {
        self.world
            .get::<PlayerControls>(self.player)
            .is_some_and(|c| c.enabled && c.mouse_look)
    }
    /// Check resolved UI rectangles before capturing the pointer for camera look.
    pub fn hit_test(&self, x: f32, y: f32) -> bool {
        if !x.is_finite() || !y.is_finite() {
            return false;
        }
        extract(&self.world, self.viewport)
            .ok()
            .is_some_and(|frame| {
                frame.nodes.iter().any(|node| {
                    x >= node.clip.x
                        && x < node.clip.x + node.clip.width
                        && y >= node.clip.y
                        && y < node.clip.y + node.clip.height
                })
            })
    }
    pub fn script_operation(&mut self, request: &str) -> Result<String, String> {
        self.world.apply_script_json(request)
    }

    pub fn input(&mut self, event: InputEvent) -> Result<(), &'static str> {
        self.renderer.push_input(event)
    }
    pub fn set_arrange_mode(&mut self, enabled: bool) {
        self.renderer.set_arrange_mode(enabled);
    }
    pub fn layout(&self) -> Result<String, String> {
        self.layout.save(&self.world)
    }
    pub fn restore_layout(&mut self, encoded: &str) -> Result<(), String> {
        self.layout.restore(&mut self.world, encoded)
    }
    pub fn reset_layout(&mut self) {
        self.layout.reset(&mut self.world);
    }
    pub fn take_layout_changed(&mut self) -> bool {
        self.layout.take_changed()
    }
    pub fn wants_keyboard(&self) -> bool {
        self.renderer.wants_keyboard()
    }
    pub fn wants_pointer(&self) -> bool {
        self.renderer.wants_pointer()
    }
    pub fn values(&self) -> (bool, f32) {
        let animate = self
            .world
            .get::<UiElement>(self.animate)
            .is_some_and(|item| matches!(item.widget, Widget::Checkbox { checked: true, .. }));
        let sway = self
            .world
            .get::<UiElement>(self.sway)
            .and_then(|item| match item.widget {
                Widget::Slider { value, .. } => Some(value),
                _ => None,
            })
            .unwrap_or(self.reset_sway);
        (animate, sway)
    }
    /// Browser frame cap. None submits on every available animation callback.
    pub fn frame_limit(&self) -> Option<u32> {
        let enabled = self
            .world
            .get::<UiElement>(self.limit_enabled)
            .is_some_and(|item| matches!(item.widget, Widget::Checkbox { checked: true, .. }));
        if !enabled {
            return None;
        }
        self.world
            .get::<UiElement>(self.frame_limit)
            .and_then(|item| match item.widget {
                Widget::Slider { value, .. } => Some(value.round().clamp(1.0, 1000.0) as u32),
                _ => None,
            })
    }
    /// Consume only user edits, so script/game changes are not overwritten every frame.
    pub fn take_sky_edits(&mut self) -> SkyEdits {
        std::mem::take(&mut self.sky_edits)
    }
    /// Reflect scene-owned values in the controls without producing UI events.
    pub fn show_sky_settings(&mut self, enabled: bool, exposure: f32, rotation: f32) {
        if let Some(element) = self.world.get_mut::<UiElement>(self.sky_visible) {
            if let Widget::Checkbox { checked, .. } = &mut element.widget {
                *checked = enabled;
            }
        }
        for (id, value) in [(self.sky_exposure, exposure), (self.sky_rotation, rotation)] {
            if let Some(element) = self.world.get_mut::<UiElement>(id) {
                if let Widget::Slider {
                    value: displayed,
                    min,
                    max,
                    ..
                } = &mut element.widget
                {
                    *displayed = value.clamp(*min, *max);
                }
            }
        }
    }
    pub fn frame_measurement(&self) -> Option<FrameMeasurement> {
        self.world
            .get::<FrameCounter>(self.counter)
            .and_then(|counter| counter.measurement)
    }
    pub fn history_seconds(&self) -> f32 {
        let Some(element) = self.world.get::<UiElement>(self.graph) else {
            return 0.0;
        };
        let Widget::LineGraph { points, .. } = &element.widget else {
            return 0.0;
        };
        match (points.first(), points.last()) {
            (Some(first), Some(last)) => (last[0] - first[0]).max(0.0),
            _ => 0.0,
        }
    }
    fn refresh_graph(&mut self, now: Duration) {
        let points = self.frame_history.points(now);
        let peak = points.iter().map(|point| point[1]).fold(50.0_f32, f32::max);
        let ceiling = (peak / 25.0).ceil() * 25.0;
        if let Some(element) = self.world.get_mut::<UiElement>(self.graph) {
            if let Widget::LineGraph {
                points: current,
                y_max,
                ..
            } = &mut element.widget
            {
                *current = points;
                *y_max = ceiling;
            }
        }
        if let Some(element) = self.world.get_mut::<UiElement>(self.graph_scale) {
            if let Widget::Text { text, .. } = &mut element.widget {
                *text = format!("0-{ceiling:.0} ms");
            }
        }
    }
    pub fn observe_frame(&mut self, now: Duration) -> Result<(), &'static str> {
        self.frame_history.record(now);
        self.refresh_graph(now);
        match self.frame_meter.record(now) {
            FrameUpdate::Unchanged => Ok(()),
            FrameUpdate::Reset => update_frame_counters(&mut self.world, None),
            FrameUpdate::Measured(measurement) => {
                update_frame_counters(&mut self.world, Some(measurement))
            }
        }
    }
    pub fn reset_timing(&mut self) {
        self.frame_meter.reset();
        self.frame_history.reset();
        self.refresh_graph(Duration::ZERO);
        // Clearing a measurement cannot fail validation.
        let _ = update_frame_counters(&mut self.world, None);
    }
    pub fn angle(&self, seconds: f64) -> f32 {
        let (enabled, amount) = self.values();
        if enabled {
            ((seconds * 0.6).sin() as f32) * amount
        } else {
            0.0
        }
    }
    fn handle_actions(&mut self, events: Vec<countersplat_ui::UiEvent>) {
        for event in apply_events(&mut self.world, events) {
            if matches!(event.kind, UiEventKind::MovedBy(_)) {
                self.layout.changed();
            }
            if event.id == self.sky_visible.to_string() {
                if let UiEventKind::BoolChanged(value) = event.kind {
                    self.sky_edits.enabled = Some(value);
                }
            } else if event.id == self.sky_exposure.to_string() {
                if let UiEventKind::NumberChanged(value) = event.kind {
                    self.sky_edits.exposure = Some(value);
                }
            } else if event.id == self.sky_rotation.to_string() {
                if let UiEventKind::NumberChanged(value) = event.kind {
                    self.sky_edits.rotation_degrees = Some(value);
                }
            }
            if event.id == self.reset.to_string() && event.kind == UiEventKind::Clicked {
                if let Some(element) = self.world.get_mut::<UiElement>(self.animate) {
                    if let Widget::Checkbox { checked, .. } = &mut element.widget {
                        *checked = self.reset_animate;
                    }
                }
                if let Some(element) = self.world.get_mut::<UiElement>(self.sway) {
                    if let Widget::Slider { value, .. } = &mut element.widget {
                        *value = self.reset_sway;
                    }
                }
            }
        }
    }
    pub fn draw(&mut self, target: UiTarget<'_>) -> Result<Vec<wgpu::CommandBuffer>, String> {
        let viewport = Rect::new(
            0.0,
            0.0,
            target.size[0] as f32 / target.pixels_per_point,
            target.size[1] as f32 / target.pixels_per_point,
        );
        self.viewport = viewport;
        let frame = extract(&self.world, viewport).map_err(str::to_owned)?;
        let output = self.renderer.draw(target, &frame).map_err(str::to_owned)?;
        self.handle_actions(output.events);
        Ok(output.command_buffers)
    }
    pub fn after_submit(&mut self) {
        self.renderer.after_submit();
    }
}
