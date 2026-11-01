//! Browser host: the CounterSplat renderer presents directly to a WebGPU canvas.
//!
//! The browser owns the animation callback and asynchronous asset fetches. This crate owns the
//! GPU surface, streaming pool, and fixed-step scheduler. It does not create native threads or
//! read files from disk; the host supplies asset bytes through the public API.

#![cfg(target_arch = "wasm32")]

mod menu;
mod selector;
mod ui_layer;
mod ui_layout;
pub use selector::Selector;

use countersplat_assets::{environment, lod::LodMeta, sog, viewer::CameraHint};
use countersplat_core::{rate::Rate, scheduler::Scheduler, time::FixedClock};
use countersplat_ecs::{Entity, World};
use countersplat_render::{
    camera, env,
    scene::{self, GsplatScene, Pipelines, View},
    stream::{LevelStream, StreamSettings},
    worker_decode::StagedChunk,
    DEPTH_FORMAT,
};
use countersplat_scene::{SceneManifest, Skybox};
use countersplat_ui::{InputEvent, Modifiers, PointerButton};
use js_sys::{Array, Uint8Array};
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use wasm_bindgen::prelude::*;
use web_sys::HtmlCanvasElement;

fn js_error(error: impl std::fmt::Display) -> JsValue {
    JsValue::from_str(&error.to_string())
}

fn depth_view(device: &wgpu::Device, width: u32, height: u32) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("CounterSplat canvas depth"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
        .create_view(&Default::default())
}

/// A live WebGPU renderer attached to the supplied HTML canvas.
#[wasm_bindgen]
pub struct Viewer {
    canvas: HtmlCanvasElement,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    surface_view_format: wgpu::TextureFormat,
    depth: wgpu::TextureView,
    pipelines: Pipelines,
    pool: GsplatScene,
    stream: LevelStream,
    scheduler: Scheduler,
    frame_limit: Option<u32>,
    pending_gpu: Arc<AtomicBool>,
    pending_resize: Option<(u32, u32)>,
    script_ticks: u32,
    far: f32,
    model: camera::Mat4,
    submitted_frames: u64,
    busy_skips: u64,
    missed_deadlines: u128,
    discarded_time: Duration,
    total_ticks: u128,
    ui: ui_layer::SceneUi,
    menu: menu::MenuUi,
    menu_visible: bool,
    scene_world: World,
    skybox_entity: Entity,
    sky_assets: BTreeMap<String, env::Sky>,
    ui_css_width: f32,
}

impl Viewer {
    fn route_ui_input(&mut self, event: InputEvent) -> Result<(), JsValue> {
        if self.menu_visible {
            self.menu.input(event)
        } else {
            self.ui.input(event)
        }
        .map_err(js_error)
    }
}

#[wasm_bindgen]
impl Viewer {
    /// Create the canvas and streaming pool. Scene manifests are gameplay, then menu JSON bytes.
    pub async fn create(
        canvas: HtmlCanvasElement,
        lod_json: Vec<u8>,
        chunk_metas: Array,
        viewer_settings: Vec<u8>,
        scene_manifests: Array,
        capacity: u32,
        rotate_z_180: bool,
        retain_preloaded_chunks: bool,
    ) -> Result<Viewer, JsValue> {
        if capacity == 0 {
            return Err(js_error("streaming capacity must be positive"));
        }
        if scene_manifests.length() != 2 {
            return Err(js_error("expected gameplay and menu scene manifests"));
        }
        let gameplay_json = Uint8Array::new(&scene_manifests.get(0)).to_vec();
        let menu_json = Uint8Array::new(&scene_manifests.get(1)).to_vec();
        let manifest = SceneManifest::from_json(&gameplay_json).map_err(js_error)?;
        let menu_manifest = SceneManifest::from_json(&menu_json).map_err(js_error)?;
        let meta = LodMeta::parse(&lod_json).map_err(js_error)?;
        let first = meta
            .leaves
            .first()
            .ok_or_else(|| js_error("level has no leaves"))?;
        let (mut min, mut max) = (first.min, first.max);
        for leaf in &meta.leaves {
            for axis in 0..3 {
                min[axis] = min[axis].min(leaf.min[axis]);
                max[axis] = max[axis].max(leaf.max[axis]);
            }
        }
        let size = (0..3)
            .map(|axis| (max[axis] - min[axis]).abs())
            .fold(0.0f32, f32::max)
            .max(1.0);
        let metas: Vec<Option<Vec<u8>>> = chunk_metas
            .iter()
            .map(|value| {
                if value.is_null() || value.is_undefined() {
                    None
                } else {
                    Some(Uint8Array::new(&value).to_vec())
                }
            })
            .collect();
        let settings = StreamSettings {
            capacity,
            max_distance_m: (size * 4.0).max(150.0),
            // Keep explicitly preloaded chunks available for the lifetime of this level.
            cache_chunks: if retain_preloaded_chunks {
                metas.len()
            } else {
                StreamSettings::default().cache_chunks
            },
            ..Default::default()
        };
        let mut stream = LevelStream::new(meta, &metas, settings).map_err(js_error)?;

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let surface = instance
            .create_surface(wgpu::SurfaceTarget::Canvas(canvas.clone()))
            .map_err(js_error)?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                compatible_surface: Some(&surface),
                ..Default::default()
            })
            .await
            .map_err(js_error)?;
        let limits = adapter.limits();
        let required_limits = wgpu::Limits {
            max_texture_dimension_2d: limits.max_texture_dimension_2d.clamp(8192, 16384),
            max_storage_buffer_binding_size: limits
                .max_storage_buffer_binding_size
                .clamp(128 << 20, 1 << 30),
            max_buffer_size: limits.max_buffer_size.clamp(256 << 20, 1 << 30),
            ..wgpu::Limits::default()
        };
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                required_limits,
                ..Default::default()
            })
            .await
            .map_err(js_error)?;
        let capabilities = surface.get_capabilities(&adapter);
        let base = capabilities
            .formats
            .iter()
            .copied()
            .find(|format| format.is_srgb())
            .or_else(|| capabilities.formats.first().copied())
            .ok_or_else(|| js_error("canvas has no supported format"))?;
        let surface_view_format = base.add_srgb_suffix();
        let (width, height) = (canvas.width().max(1), canvas.height().max(1));
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: base,
            color_space: Default::default(),
            width,
            height,
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: *capabilities
                .alpha_modes
                .first()
                .ok_or_else(|| js_error("canvas has no alpha mode"))?,
            view_formats: if surface_view_format == base {
                vec![]
            } else {
                vec![surface_view_format]
            },
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);
        let depth = depth_view(&device, width, height);
        let pipelines = Pipelines::new(&device, surface_view_format).map_err(js_error)?;
        let rows = stream.palette_texture_rows(device.limits().max_texture_dimension_2d);
        let pool = scene::pool_with_centroid_rows(
            &device,
            &queue,
            &pipelines,
            stream.shared_meta(),
            capacity,
            0.5,
            rows,
        )
        .map_err(js_error)?;
        stream.init_pool(&pool, &queue);
        let mut ui =
            ui_layer::SceneUi::new(&device, surface_view_format, &manifest).map_err(js_error)?;
        let menu =
            menu::MenuUi::new(&device, surface_view_format, &menu_manifest).map_err(js_error)?;
        let mut scene_world = World::new();
        countersplat_scene_ecs::register(&mut scene_world).map_err(js_error)?;
        let entities =
            countersplat_scene_ecs::instantiate_layer(&mut scene_world, &manifest, "scene")
                .map_err(js_error)?;
        let sky_id = manifest
            .resources
            .values
            .get("preview")
            .and_then(|value| value.get("skybox_entity"))
            .and_then(|value| value.as_str())
            .ok_or_else(|| {
                js_error("scene resource `preview.skybox_entity` must be an entity ID")
            })?;
        let skybox_entity = *entities.get(sky_id).ok_or_else(|| {
            js_error(format!(
                "skybox entity `{sky_id}` is missing from the scene layer"
            ))
        })?;
        if !scene_world.has::<Skybox>(skybox_entity) {
            return Err(js_error(format!(
                "scene entity `{sky_id}` lacks a Skybox component"
            )));
        }
        let hint = CameraHint::from_json(&viewer_settings).ok();
        let center = [0, 1, 2].map(|axis| (min[axis] + max[axis]) * 0.5);
        let fallback_eye = [center[0], center[1] + size * 0.2, center[2] + size * 1.8];
        let model = if rotate_z_180 {
            [
                -1.0, 0.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
            ]
        } else {
            camera::IDENTITY
        };
        let eye = hint.map_or_else(
            || camera::transform_point(&model, fallback_eye),
            |hint| hint.position,
        );
        let target = hint.map_or_else(
            || camera::transform_point(&model, center),
            |hint| hint.target,
        );
        let fov = hint.map_or(60.0, |hint| hint.fov_degrees);
        let spawn_source = manifest
            .resources
            .values
            .get("preview")
            .and_then(|value| value.get("player_spawn"))
            .and_then(|value| value.as_str())
            .unwrap_or("level");
        let use_level_camera = match spawn_source {
            "level" => true,
            "scene" => false,
            _ => return Err(js_error("preview.player_spawn must be `level` or `scene`")),
        };
        ui.set_camera_spawn(eye, target, fov, use_level_camera)
            .map_err(js_error)?;
        let rate = Rate::new(60, 1).map_err(js_error)?;
        let fixed = FixedClock::with_rate(rate, Duration::from_millis(250), 4).map_err(js_error)?;
        Ok(Self {
            canvas,
            surface,
            device,
            queue,
            config,
            surface_view_format,
            depth,
            pipelines,
            pool,
            stream,
            scheduler: Scheduler::new(fixed, rate),
            frame_limit: None,
            pending_gpu: Arc::new(AtomicBool::new(false)),
            pending_resize: None,
            script_ticks: 0,
            far: size * 20.0 + 100.0,
            model,
            submitted_frames: 0,
            busy_skips: 0,
            missed_deadlines: 0,
            discarded_time: Duration::ZERO,
            total_ticks: 0,
            ui,
            menu,
            menu_visible: false,
            scene_world,
            skybox_entity,
            sky_assets: BTreeMap::new(),
            ui_css_width: width as f32,
        })
    }

    /// Queue a resize; it is applied only when the previous frame has finished.
    pub fn resize(&mut self, width: u32, height: u32) {
        let limit = self.device.limits().max_texture_dimension_2d;
        let size = (width.clamp(1, limit), height.clamp(1, limit));
        self.pending_resize = (size != (self.config.width, self.config.height)).then_some(size);
    }

    /// Set the canvas width in CSS points for high-DPI UI and pointer mapping.
    pub fn ui_set_width(&mut self, width: f32) {
        if width.is_finite() && (1.0..=1_000_000.0).contains(&width) {
            self.ui_css_width = width;
        }
    }
    /// Set which ECS UI scene owns input and is drawn over the game.
    pub fn set_menu_visible(&mut self, visible: bool) {
        self.menu_visible = visible;
    }
    /// Apply an owned script command to the specified ECS world.
    pub fn ecs_operation(&mut self, layer: &str, request: &str) -> Result<String, JsValue> {
        match layer {
            "gameplay" => self.ui.script_operation(request),
            "menu" => self.menu.script_operation(request),
            _ => Err("unknown script world".into()),
        }
        .map_err(js_error)
    }
    /// Whether a canvas point overlaps the current gameplay UI.
    pub fn ui_hit_test(&self, x: f32, y: f32) -> bool {
        self.menu_visible || self.ui.hit_test(x, y)
    }
    pub fn player_mouse_look_enabled(&self) -> bool {
        !self.menu_visible && self.ui.mouse_look_enabled()
    }
    /// Consume fixed simulation ticks produced by the most recent frame poll.
    pub fn take_script_ticks(&mut self) -> u32 {
        std::mem::take(&mut self.script_ticks)
    }
    pub fn script_tick_seconds(&self) -> f64 {
        self.scheduler.tick_seconds()
    }
    /// Allow dragging individual ECS UI elements rather than activating controls.
    pub fn ui_set_arrange_mode(&mut self, enabled: bool) {
        self.ui.set_arrange_mode(enabled);
        self.menu.set_arrange_mode(enabled);
    }
    pub fn ui_layout(&self) -> Result<String, JsValue> {
        if self.menu_visible {
            self.menu.layout()
        } else {
            self.ui.layout()
        }
        .map_err(js_error)
    }
    pub fn ui_restore_layout(&mut self, encoded: String) -> Result<(), JsValue> {
        if self.menu_visible {
            self.menu.restore_layout(&encoded)
        } else {
            self.ui.restore_layout(&encoded)
        }
        .map_err(js_error)
    }
    pub fn ui_reset_layout(&mut self) {
        if self.menu_visible {
            self.menu.reset_layout()
        } else {
            self.ui.reset_layout()
        }
    }
    pub fn ui_take_layout_changed(&mut self) -> bool {
        if self.menu_visible {
            self.menu.take_layout_changed()
        } else {
            self.ui.take_layout_changed()
        }
    }
    pub fn ui_pointer_move(&mut self, x: f32, y: f32) -> Result<(), JsValue> {
        self.route_ui_input(InputEvent::PointerMoved([x, y]))
    }
    pub fn ui_pointer_button(
        &mut self,
        x: f32,
        y: f32,
        button: u8,
        pressed: bool,
        modifiers: u8,
    ) -> Result<(), JsValue> {
        let button = match button {
            0 => PointerButton::Primary,
            1 => PointerButton::Secondary,
            2 => PointerButton::Middle,
            _ => return Err(js_error("unknown pointer button")),
        };
        self.route_ui_input(InputEvent::PointerButton {
            position: [x, y],
            button,
            pressed,
            modifiers: ui_modifiers(modifiers),
        })
    }
    pub fn ui_pointer_gone(&mut self) -> Result<(), JsValue> {
        self.route_ui_input(InputEvent::PointerGone)
    }
    pub fn ui_scroll(&mut self, x: f32, y: f32) -> Result<(), JsValue> {
        self.route_ui_input(InputEvent::Scroll([x, y]))
    }
    pub fn ui_key(
        &mut self,
        name: String,
        pressed: bool,
        repeat: bool,
        modifiers: u8,
    ) -> Result<(), JsValue> {
        self.route_ui_input(InputEvent::Key {
            name,
            pressed,
            repeat,
            modifiers: ui_modifiers(modifiers),
        })
    }
    pub fn ui_text(&mut self, text: String) -> Result<(), JsValue> {
        self.route_ui_input(InputEvent::Text(text))
    }
    pub fn ui_focus(&mut self, focused: bool) -> Result<(), JsValue> {
        self.route_ui_input(InputEvent::Focus(focused))
    }
    pub fn ui_wants_keyboard(&self) -> bool {
        if self.menu_visible {
            self.menu.wants_keyboard()
        } else {
            self.ui.wants_keyboard()
        }
    }
    pub fn ui_wants_pointer(&self) -> bool {
        if self.menu_visible {
            self.menu.wants_pointer()
        } else {
            self.ui.wants_pointer()
        }
    }

    /// Install a decoded cube from a worker under an opaque scene asset ID.
    pub fn install_skybox(
        &mut self,
        asset: String,
        face_size: u32,
        rgba16f: Vec<u8>,
    ) -> Result<(), JsValue> {
        Skybox::new(&asset).map_err(js_error)?;
        if !self.sky_assets.contains_key(&asset) && self.sky_assets.len() >= 8 {
            return Err(js_error("skybox cache is full"));
        }
        let sky = env::Sky::from_rgba16f(
            &self.device,
            &self.queue,
            face_size as usize,
            &rgba16f,
            self.surface_view_format,
        )
        .map_err(js_error)?;
        self.sky_assets.insert(asset, sky);
        Ok(())
    }
    /// Select a resident sky asset without replacing the existing ECS layer on failure.
    pub fn select_skybox(&mut self, asset: String) -> Result<(), JsValue> {
        Skybox::new(&asset).map_err(js_error)?;
        if !self.sky_assets.contains_key(&asset) {
            return Err(js_error("skybox asset is not loaded"));
        }
        let skybox = self
            .scene_world
            .get_mut::<Skybox>(self.skybox_entity)
            .ok_or_else(|| js_error("skybox entity is missing"))?;
        skybox.asset = asset;
        Ok(())
    }
    /// Pause simulation when the browser hides the page; frames may resume at the next callback.
    pub fn set_paused(&mut self, paused: bool) {
        self.scheduler.set_paused(paused);
        if paused {
            self.ui.reset_timing();
        }
    }

    /// Advance fixed ticks and, when due, present one frame directly to the canvas.
    pub fn frame(&mut self, now_ms: f64) -> Result<bool, JsValue> {
        let now = Duration::try_from_secs_f64(now_ms / 1000.0).map_err(js_error)?;
        let busy = self.pending_gpu.load(Ordering::Acquire);
        if !busy {
            if let Some((width, height)) = self.pending_resize.take() {
                self.canvas.set_width(width);
                self.canvas.set_height(height);
                self.config.width = width;
                self.config.height = height;
                self.surface.configure(&self.device, &self.config);
                self.depth = depth_view(&self.device, width, height);
            }
        }
        let requested_limit = self.ui.frame_limit();
        if requested_limit != self.frame_limit {
            if let Some(fps) = requested_limit {
                self.scheduler
                    .set_frame_rate(Rate::new(fps, 1).map_err(js_error)?, now)
                    .map_err(js_error)?;
            }
            self.frame_limit = requested_limit;
        }
        let plan = self.scheduler.poll(now).map_err(js_error)?;
        self.total_ticks = plan.total_ticks;
        self.script_ticks = plan.simulation.ticks;
        self.discarded_time = self.discarded_time.saturating_add(plan.simulation.dropped);
        if self.frame_limit.is_some() {
            self.missed_deadlines = self.missed_deadlines.saturating_add(plan.frame.missed);
        }
        let edits = self.ui.take_sky_edits();
        if let Some(skybox) = self.scene_world.get_mut::<Skybox>(self.skybox_entity) {
            if let Some(value) = edits.enabled {
                skybox.enabled = value;
            }
            if let Some(value) = edits.exposure {
                skybox.exposure = value;
            }
            if let Some(value) = edits.rotation_degrees {
                skybox.rotation_degrees = value;
            }
            self.ui
                .show_sky_settings(skybox.enabled, skybox.exposure, skybox.rotation_degrees);
        }
        let selected_sky = countersplat_scene_ecs::extract(&self.scene_world);
        let seconds = (plan.total_ticks as f64 + f64::from(plan.simulation.alpha))
            * self.scheduler.tick_seconds();
        let angle = self.ui.angle(seconds);
        let (sin, cos) = angle.sin_cos();
        let player_camera = self.ui.player_camera().map_err(js_error)?;
        let eye = player_camera.position;
        let direction = player_camera.direction();
        let target = [
            eye[0] + direction[0] * cos - direction[2] * sin,
            eye[1] + direction[1],
            eye[2] + direction[0] * sin + direction[2] * cos,
        ];
        let view = View {
            model: self.model,
            view: camera::look_at(eye, target, [0.0, 1.0, 0.0]),
            projection: camera::perspective(
                player_camera.fov_degrees.to_radians(),
                self.config.width as f32 / self.config.height as f32,
                0.05,
                self.far,
            ),
            width: self.config.width,
            height: self.config.height,
            near: 0.05,
            far: self.far,
        };
        let inverse =
            camera::inverse(&self.model).ok_or_else(|| js_error("invalid scene transform"))?;
        let local_eye = camera::transform_point(&inverse, eye);
        let local_target = camera::transform_point(&inverse, target);
        let local_direction = [0, 1, 2].map(|axis| local_target[axis] - local_eye[axis]);
        let clip = camera::mul(&view.projection, &camera::mul(&view.view, &view.model));
        self.stream
            .set_focal_px(view.projection[5] * self.config.height as f32);
        self.stream
            .select(local_eye, local_direction, Some(&clip), seconds as f32);
        self.stream.apply(&self.pool, &self.queue);
        // Keep menu input responsive even when gameplay has a very low frame cap.
        if !self.menu_visible && self.frame_limit.is_some() && !plan.frame.due {
            return Ok(false);
        }
        if busy {
            self.busy_skips = self.busy_skips.saturating_add(1);
            return Ok(false);
        }
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return Ok(false)
            }
            wgpu::CurrentSurfaceTexture::Outdated => {
                self.surface.configure(&self.device, &self.config);
                return Ok(false);
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                return Err(js_error(
                    "WebGPU canvas surface was lost; reload to recreate it",
                ))
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                return Err(js_error("WebGPU canvas validation failed"))
            }
        };
        let color = frame.texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(self.surface_view_format),
            ..Default::default()
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        self.pool
            .prepare(&self.queue, &mut encoder, &view, &view.model);
        {
            let mut pass = scene::color_pass(
                &mut encoder,
                &color,
                &self.depth,
                true,
                true,
                "CounterSplat canvas",
            );
            if let Some((snapshot, sky)) = selected_sky.as_ref().and_then(|snapshot| {
                self.sky_assets
                    .get(&snapshot.skybox.asset)
                    .map(|sky| (snapshot, sky))
            }) {
                sky.draw_rotated(
                    &self.queue,
                    &mut pass,
                    &view.view,
                    &view.projection,
                    snapshot.skybox.exposure,
                    snapshot.skybox.rotation_degrees,
                );
            }
            self.pool.draw_color(&mut pass, &self.pipelines);
        }
        let pixels_per_point = (self.config.width as f32 / self.ui_css_width).clamp(0.25, 8.0);
        let ui_target = countersplat_ui_wgpu::UiTarget {
            device: &self.device,
            queue: &self.queue,
            encoder: &mut encoder,
            color: &color,
            size: [self.config.width, self.config.height],
            pixels_per_point,
            time_seconds: now.as_secs_f64(),
        };
        let ui_buffers = if self.menu_visible {
            self.menu.draw(ui_target)
        } else {
            self.ui.draw(ui_target)
        }
        .map_err(js_error)?;
        self.pending_gpu.store(true, Ordering::Release);
        self.queue.submit(
            ui_buffers
                .into_iter()
                .chain(std::iter::once(encoder.finish())),
        );
        // Sample the actual CPU submission time rather than the earlier animation callback time.
        let ui_submitted_at = web_sys::window()
            .and_then(|window| window.performance())
            .and_then(|clock| Duration::try_from_secs_f64(clock.now() / 1000.0).ok())
            .unwrap_or(now);
        if self.menu_visible {
            self.menu.after_submit();
        } else {
            self.ui.after_submit();
        }
        let pending = Arc::clone(&self.pending_gpu);
        self.queue.on_submitted_work_done(move || {
            pending.store(false, Ordering::Release);
        });
        self.queue.present(frame);
        self.ui.observe_frame(ui_submitted_at).map_err(js_error)?;
        self.submitted_frames = self.submitted_frames.saturating_add(1);
        Ok(true)
    }

    /// Chunk folders requested by the streaming system, as JSON `[file, folder]` pairs.
    pub fn requests(&mut self) -> String {
        serde_json::to_string(&self.stream.requests()).unwrap_or_else(|_| "[]".into())
    }

    /// Validate and insert a chunk decoded by the browser's asset worker.
    pub fn insert_decoded_chunk(
        &mut self,
        file: usize,
        metadata: Vec<u8>,
        dimensions: Vec<u32>,
        pixels: Array,
    ) -> Result<(), JsValue> {
        if dimensions.len() != sog::streams().len() * 2
            || pixels.length() as usize != sog::streams().len()
        {
            return Err(js_error("incorrect decoded chunk stream count"));
        }
        let meta = sog::parse_meta_json(&metadata).map_err(js_error)?;
        if !self.stream.accepts_chunk_meta(file, &meta) {
            return Err(js_error("chunk metadata does not match the selected level"));
        }
        let mut staged = StagedChunk::new(file, meta);
        for index in 0..sog::streams().len() {
            let rgba = Uint8Array::new(&pixels.get(index as u32)).to_vec();
            staged
                .push(
                    index,
                    dimensions[index * 2],
                    dimensions[index * 2 + 1],
                    rgba,
                )
                .map_err(js_error)?;
        }
        let (file, bundle) = staged.finish().map_err(js_error)?;
        self.stream.insert(file, bundle);
        Ok(())
    }

    /// Report a fetch or decode failure so the streamer can retry it.
    pub fn chunk_failed(&mut self, file: usize) {
        self.stream.failed(file);
    }

    /// Small diagnostic snapshot for the host UI. Submitted frames are not display-present feedback.
    pub fn status(&self) -> String {
        let stats = self.stream.stats();
        let (ui_animate, ui_sway) = self.ui.values();
        let skybox = self.scene_world.get::<Skybox>(self.skybox_entity);
        let sky_loaded = skybox.is_some_and(|sky| self.sky_assets.contains_key(&sky.asset));
        let timing = self.ui.frame_measurement();
        let history_seconds = self.ui.history_seconds();
        serde_json::json!({ "sky_loaded": sky_loaded, "sky_enabled": skybox.is_some_and(|sky| sky.enabled),
            "sky_exposure": skybox.map(|sky| sky.exposure), "sky_rotation": skybox.map(|sky| sky.rotation_degrees),
            "sky_asset": skybox.map(|sky| sky.asset.as_str()), "ui_history_seconds": history_seconds, "ui_frame_fps": timing.map(|value| value.fps), "ui_frame_ms": timing.map(|value| value.frame_time_ms),
            "ui_animate": ui_animate, "ui_sway": ui_sway, "ticks": self.total_ticks.to_string(), "submitted_frames": self.submitted_frames,
            "resident_splats": stats.resident_splats, "busy_skips": self.busy_skips,
            "missed_deadlines": self.missed_deadlines.to_string(), "discarded_ms": self.discarded_time.as_secs_f64() * 1000.0,
            "target_fps": self.frame_limit,
            "presentation": "browser_vsync",
            "gpu_pending": self.pending_gpu.load(Ordering::Acquire) }).to_string()
    }
}

fn ui_modifiers(mask: u8) -> Modifiers {
    Modifiers {
        alt: mask & 1 != 0,
        ctrl: mask & 2 != 0,
        shift: mask & 4 != 0,
        command: mask & 8 != 0,
    }
}

/// An HDR cubemap prepared on an asset worker for upload by the GPU owner.
#[wasm_bindgen]
pub struct DecodedSkybox {
    size: u32,
    rgba16f: Vec<u8>,
}
#[wasm_bindgen]
impl DecodedSkybox {
    pub fn size(&self) -> u32 {
        self.size
    }
    pub fn take_pixels(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.rgba16f)
    }
}
/// Decode a Radiance HDR panorama and convert it to unrotated cube faces off the render thread.
#[wasm_bindgen]
pub fn decode_skybox_hdr(bytes: Vec<u8>, face_size: u32) -> Result<DecodedSkybox, JsValue> {
    if face_size == 0 || face_size > 512 {
        return Err(js_error("browser sky face size must be 1 to 512"));
    }
    let panorama = environment::decode_hdr(&bytes).map_err(js_error)?;
    let cube = env::try_to_cubemap(&panorama, face_size as usize, 0.0).map_err(js_error)?;
    Ok(DecodedSkybox {
        size: face_size,
        rgba16f: env::cubemap_rgba16f(&cube),
    })
}

/// A CPU-decoded SOG texture, used by the separate browser asset worker.
#[wasm_bindgen]
pub struct DecodedTexture {
    inner: sog::Texture,
}
#[wasm_bindgen]
impl DecodedTexture {
    pub fn width(&self) -> u32 {
        self.inner.width
    }
    pub fn height(&self) -> u32 {
        self.inner.height
    }
    pub fn take_pixels(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.inner.rgba)
    }
}

/// Decode a SOG texture without touching the GPU; safe to call in a Web Worker.
#[wasm_bindgen]
pub fn decode_texture(index: usize, bytes: Vec<u8>) -> Result<DecodedTexture, JsValue> {
    let name = sog::streams()
        .get(index)
        .ok_or_else(|| js_error("unknown SOG texture"))?
        .0;
    Ok(DecodedTexture {
        inner: sog::decode_stream(name, &bytes).map_err(js_error)?,
    })
}
