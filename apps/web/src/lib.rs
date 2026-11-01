//! Browser host: the CounterSplat renderer presents directly to a WebGPU canvas.
//!
//! The browser owns the animation callback and asynchronous asset fetches. This crate owns the
//! GPU surface, streaming pool, and fixed-step scheduler. It does not create native threads or
//! read files from disk; the host supplies asset bytes through the public API.

#![cfg(target_arch = "wasm32")]

mod menu;
mod selector;
mod ui_layer;
pub use selector::Selector;

use countersplat_assets::{environment, lod::LodMeta, sog, viewer::CameraHint};
use countersplat_core::{inflight::InFlight, rate::Rate, scheduler::Scheduler, time::FixedClock};
use countersplat_render::{
    camera, env, first_person,
    gpu_timing::{GpuFrameTimer, GpuTiming},
    postprocess,
    scene::{self, GsplatScene, Pipelines, View},
    stream::{DrawRegion, LevelStream, StreamSettings},
    tracer::TracerRenderer,
    voxel_debug::VoxelDebug,
    worker_decode::StagedChunk,
    DEPTH_FORMAT,
};
use countersplat_scene::{LevelAssets, SceneManifest, Skybox};
use countersplat_ui::{InputEvent, Modifiers, PointerButton};
use js_sys::{Array, Uint8Array};
use std::{collections::BTreeMap, time::Duration};
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

/// Previous submitted camera, used only for bounded camera-motion blur.
struct PreviousCamera {
    time: Duration,
    clip: camera::Mat4,
    position: [f32; 3],
    direction: [f32; 3],
}

// A one-shot CPU breakdown for the opt-in browser profiler. It measures
// command preparation and submission, not asynchronous GPU execution.
struct FrameCpuProfile {
    clock: web_sys::Performance,
    previous_ms: f64,
    spans: Vec<(&'static str, f64)>,
}

impl FrameCpuProfile {
    fn new() -> Option<Self> {
        let clock = web_sys::window()?.performance()?;
        let previous_ms = clock.now();
        Some(Self {
            clock,
            previous_ms,
            spans: Vec::with_capacity(12),
        })
    }

    fn mark(&mut self, name: &'static str) {
        let now = self.clock.now();
        self.spans.push((name, (now - self.previous_ms).max(0.0)));
        self.previous_ms = now;
    }
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
    postprocess: postprocess::PostProcess,
    first_person: first_person::FirstPersonLayer,
    first_person_settings: first_person::Settings,
    pipelines: Pipelines,
    pool: GsplatScene,
    stream: LevelStream,
    voxel_debug: VoxelDebug,
    tracers: TracerRenderer,
    scheduler: Scheduler,
    frame_limit: Option<u32>,
    gpu_frames: InFlight,
    pending_resize: Option<(u32, u32)>,
    render_size: (u32, u32),
    pending_render_size: Option<(u32, u32)>,
    viewport_scale: postprocess::ViewportScale,
    script_ticks: u32,
    far: f32,
    model: camera::Mat4,
    previous_camera: Option<PreviousCamera>,
    submitted_frames: u64,
    busy_skips: u64,
    missed_deadlines: u128,
    discarded_time: Duration,
    total_ticks: u128,
    ui: ui_layer::SceneUi,
    menu: menu::MenuUi,
    menu_visible: bool,
    sky_assets: BTreeMap<String, env::Sky>,
    ui_css_width: f32,
    capture_frame_timing: bool,
    last_frame_timing: Option<FrameCpuProfile>,
    gpu_timing_requested: bool,
    gpu_timers: Vec<GpuFrameTimer>,
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
        level_assets_json: String,
        gpu_timing_requested: bool,
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
        let level_assets: LevelAssets =
            serde_json::from_str(&level_assets_json).map_err(js_error)?;
        level_assets.validate().map_err(js_error)?;
        if level_assets.lod_count as usize != meta.levels() {
            return Err(js_error(
                "level asset LOD count does not match the manifest",
            ));
        }
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
            max_distance_m: 150.0,
            draw_region: DrawRegion::ForwardEllipse {
                rear_fraction: 0.3,
                side_fraction: 0.55,
            },
            fade_s: 0.45,
            keep_s: 2.5,
            near_lod0_m: 24.0,
            center_lod0_m: 1000.0,
            offscreen_keep_m: 12.0,
            allow_fade_cuts: false,
            // The host starts at most four decoders and requests only for idle workers.
            max_in_flight: 4,
            // Keep explicitly preloaded chunks available for the lifetime of this level.
            cache_chunks: if retain_preloaded_chunks {
                metas.len()
            } else {
                16
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
        let timing_feature = if gpu_timing_requested
            && adapter.features().contains(wgpu::Features::TIMESTAMP_QUERY)
        {
            wgpu::Features::TIMESTAMP_QUERY
        } else {
            wgpu::Features::empty()
        };
        let requested = adapter
            .request_device(&wgpu::DeviceDescriptor {
                required_limits: required_limits.clone(),
                required_features: timing_feature,
                ..Default::default()
            })
            .await;
        // Profiling is optional: retry without timestamp queries if the
        // advertised feature cannot be enabled on this device.
        let (device, queue) = match requested {
            Ok(device) => device,
            Err(_) if !timing_feature.is_empty() => adapter
                .request_device(&wgpu::DeviceDescriptor {
                    required_limits,
                    ..Default::default()
                })
                .await
                .map_err(js_error)?,
            Err(error) => return Err(js_error(error)),
        };
        let gpu_timers = if gpu_timing_requested {
            (0..6).filter_map(|_| GpuFrameTimer::new(&device)).collect()
        } else {
            Vec::new()
        };
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
        let postprocess =
            postprocess::PostProcess::new(&device, surface_view_format, width, height);
        let first_person = first_person::FirstPersonLayer::new(&device, postprocess::SCENE_FORMAT);
        let voxel_debug = VoxelDebug::new(&device, postprocess::SCENE_FORMAT);
        let tracers = TracerRenderer::new(&device, postprocess::SCENE_FORMAT);
        let pipelines = Pipelines::new(&device, postprocess::SCENE_FORMAT).map_err(js_error)?;
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
        ui.set_level_assets(level_assets).map_err(js_error)?;
        let menu =
            menu::MenuUi::new(&device, surface_view_format, &menu_manifest).map_err(js_error)?;
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
        // Supply the level's camera hint as data. The scene's Lua player
        // script chooses whether to use it or the authored scene camera.
        ui.publish_level_view(eye, target, fov);
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
            postprocess,
            first_person,
            first_person_settings: first_person::Settings::default(),
            pipelines,
            pool,
            stream,
            voxel_debug,
            tracers,
            scheduler: Scheduler::new(fixed, rate),
            frame_limit: None,
            gpu_frames: InFlight::new(2).expect("positive in-flight frame limit"),
            pending_resize: None,
            render_size: (width, height),
            pending_render_size: None,
            viewport_scale: postprocess::ViewportScale::Fill,
            script_ticks: 0,
            far: size * 20.0 + 100.0,
            model,
            previous_camera: None,
            submitted_frames: 0,
            busy_skips: 0,
            missed_deadlines: 0,
            discarded_time: Duration::ZERO,
            total_ticks: 0,
            ui,
            menu,
            menu_visible: false,
            sky_assets: BTreeMap::new(),
            ui_css_width: width as f32,
            capture_frame_timing: false,
            last_frame_timing: None,
            gpu_timing_requested,
            gpu_timers,
        })
    }

    /// Queue a resize; it is applied after all submitted frames have finished.
    pub fn resize(&mut self, width: u32, height: u32) {
        let limit = self.device.limits().max_texture_dimension_2d;
        let size = (width.clamp(1, limit), height.clamp(1, limit));
        self.pending_resize = (size != (self.config.width, self.config.height)).then_some(size);
    }

    /// Maximum supported 2D texture dimension on the active device.
    pub fn max_texture_dimension(&self) -> u32 {
        self.device.limits().max_texture_dimension_2d
    }

    /// Set the 3D scene resolution independently of the display canvas and UI.
    /// The limit bounds HDR/depth/bloom memory while still supporting 4K and ultrawide.
    pub fn set_render_resolution(&mut self, width: u32, height: u32) -> Result<(), JsValue> {
        const MAX_PIXELS: u64 = 16_777_216;
        let limit = self.device.limits().max_texture_dimension_2d;
        if width == 0
            || height == 0
            || width > limit
            || height > limit
            || u64::from(width) * u64::from(height) > MAX_PIXELS
        {
            return Err(js_error(
                "render resolution exceeds the device or pixel limit",
            ));
        }
        self.pending_render_size = ((width, height) != self.render_size).then_some((width, height));
        Ok(())
    }

    /// Fill crops the scene; fit shows the whole scene with bars. UI remains visible.
    pub fn set_viewport_fill(&mut self, fill: bool) {
        self.viewport_scale = if fill {
            postprocess::ViewportScale::Fill
        } else {
            postprocess::ViewportScale::Fit
        };
    }

    /// Set the canvas width in CSS points for high-DPI UI and pointer mapping.
    pub fn ui_set_width(&mut self, width: f32) {
        if width.is_finite() && (1.0..=1_000_000.0).contains(&width) {
            self.ui_css_width = width;
        }
    }
    /// Set which ECS UI scene owns input and is drawn over the game.
    pub fn set_menu_visible(&mut self, visible: bool) {
        if self.menu_visible != visible {
            self.previous_camera = None;
        }
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
    /// Read a resource value without encoding a JSON request.
    pub fn ecs_get_resource(&self, layer: &str, name: &str) -> Result<Option<String>, JsValue> {
        match layer {
            "gameplay" => self.ui.script_resource_value(name),
            "menu" => self.menu.script_resource_value(name),
            _ => Err("unknown script world".into()),
        }
        .map_err(js_error)
    }
    /// Read one resource field without encoding a JSON request.
    pub fn ecs_get_resource_field(
        &self,
        layer: &str,
        name: &str,
        field: &str,
    ) -> Result<Option<String>, JsValue> {
        match layer {
            "gameplay" => self.ui.script_resource_field(name, field),
            "menu" => self.menu.script_resource_field(name, field),
            _ => Err("unknown script world".into()),
        }
        .map_err(js_error)
    }
    /// Check a script-visible entity handle without a JSON request.
    pub fn ecs_is_alive(&self, layer: &str, id: &str) -> Result<bool, JsValue> {
        match layer {
            "gameplay" => self.ui.script_entity_alive(id),
            "menu" => self.menu.script_entity_alive(id),
            _ => Err("unknown script world".into()),
        }
        .map_err(js_error)
    }
    /// Check script-visible component presence without a JSON request.
    pub fn ecs_has_component(&self, layer: &str, id: &str, name: &str) -> Result<bool, JsValue> {
        match layer {
            "gameplay" => self.ui.script_has_component(id, name),
            "menu" => self.menu.script_has_component(id, name),
            _ => Err("unknown script world".into()),
        }
        .map_err(js_error)
    }
    /// Read a changed component without encoding an intermediate JSON request.
    /// An absent result means the caller's revision is still current.
    pub fn ecs_get_if_changed(
        &self,
        layer: &str,
        id: &str,
        name: &str,
        revision: Option<String>,
    ) -> Result<Option<String>, JsValue> {
        match layer {
            "gameplay" => self
                .ui
                .script_component_change(id, name, revision.as_deref()),
            "menu" => self
                .menu
                .script_component_change(id, name, revision.as_deref()),
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

    /// Upload a decoded scene frame directly to the WebGPU texture. The browser
    /// retains ownership of the ImageBitmap and may close it after this call.
    pub fn install_first_person_frame(
        &mut self,
        image: web_sys::ImageBitmap,
    ) -> Result<(), JsValue> {
        let (width, height) = (image.width(), image.height());
        let limit = self.device.limits().max_texture_dimension_2d.min(4096);
        if width == 0 || height == 0 || width > limit || height > limit {
            return Err(js_error("invalid first-person frame dimensions"));
        }
        let texture = self.first_person.frame_texture(&self.device, width, height);
        self.queue.copy_external_image_to_texture(
            &wgpu::CopyExternalImageSourceInfo {
                source: wgpu::ExternalImageSource::ImageBitmap(image),
                origin: wgpu::Origin2d::ZERO,
                flip_y: false,
            },
            wgpu::CopyExternalImageDestInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
                color_space: wgpu::PredefinedColorSpace::Srgb,
                premultiplied_alpha: false,
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        self.first_person.mark_ready();
        Ok(())
    }
    /// Presentation settings are read from the script-owned ECS component.
    pub fn set_first_person_settings(
        &mut self,
        visible: bool,
        opacity: f32,
        key_threshold: f32,
        key_softness: f32,
        despill: f32,
        vertical_offset: f32,
    ) {
        self.first_person_settings = first_person::Settings {
            visible,
            opacity,
            key_threshold,
            key_softness,
            despill,
            vertical_offset,
            ..self.first_person_settings
        };
    }
    /// A renderer-only frame capture used to blend discontinuous footage clips.
    pub fn capture_first_person_frame(&mut self) -> bool {
        self.first_person
            .capture_previous(&self.device, &self.queue)
    }
    /// Apply a Lua-authored visual transform without changing player physics.
    pub fn set_first_person_motion(&mut self, x: f32, y: f32, roll: f32, blend: f32) {
        self.first_person_settings.motion_x = x;
        self.first_person_settings.motion_y = y;
        self.first_person_settings.motion_roll = roll;
        self.first_person_settings.blend = blend;
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
            postprocess::SCENE_FORMAT,
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
            .ui
            .skybox_mut()
            .ok_or_else(|| js_error("skybox entity is missing"))?;
        skybox.asset = asset;
        Ok(())
    }
    /// Pause simulation when the browser hides the page; frames may resume at the next callback.
    pub fn set_paused(&mut self, paused: bool) {
        self.scheduler.set_paused(paused);
        if paused {
            self.ui.reset_timing();
            self.previous_camera = None;
        }
    }

    /// Arm a one-frame CPU breakdown for local diagnostics.
    pub fn arm_frame_timing(&mut self) {
        self.capture_frame_timing = true;
        self.last_frame_timing = None;
    }

    /// Return and clear the most recent breakdown, or JSON null if no frame submitted.
    pub fn take_frame_timing(&mut self) -> String {
        self.last_frame_timing
            .take()
            .and_then(|profile| serde_json::to_string(&profile.spans).ok())
            .unwrap_or_else(|| "null".to_owned())
    }

    /// Return nonblocking GPU samples with the animation timestamp of each frame.
    pub fn gpu_frame_timing(&self) -> String {
        if !self.gpu_timing_requested {
            return serde_json::json!({"status":"disabled"}).to_string();
        }
        if self.gpu_timers.is_empty() {
            return serde_json::json!({"status":"unsupported"}).to_string();
        }
        let mut samples = Vec::new();
        let mut pending = 0;
        let mut unused = 0;
        let mut errors = Vec::new();
        for timer in &self.gpu_timers {
            match timer.timing() {
                GpuTiming::Ready => unused += 1,
                GpuTiming::Pending { .. } => pending += 1,
                GpuTiming::Complete {
                    token,
                    milliseconds,
                    spans,
                } => samples.push(serde_json::json!({
                    "frame_timestamp_ms":f64::from_bits(token),"gpu_ms":milliseconds,"stages":spans
                })),
                GpuTiming::Failed { token, reason } => errors.push(serde_json::json!({
                    "frame_timestamp_ms":f64::from_bits(token),"reason":reason
                })),
            }
        }
        let status = if !samples.is_empty() {
            "complete"
        } else if pending > 0 {
            "pending"
        } else if !errors.is_empty() {
            "failed"
        } else {
            "ready"
        };
        serde_json::json!({"status":status,"samples":samples,"pending":pending,"unused":unused,"errors":errors}).to_string()
    }

    /// Advance fixed ticks and, when due, present one frame directly to the canvas.
    pub fn frame(&mut self, now_ms: f64) -> Result<bool, JsValue> {
        let mut profile = if std::mem::take(&mut self.capture_frame_timing) {
            FrameCpuProfile::new()
        } else {
            None
        };
        let now = Duration::try_from_secs_f64(now_ms / 1000.0).map_err(js_error)?;
        let pending_frames = self.gpu_frames.count();
        let resizing = self.pending_resize.is_some() || self.pending_render_size.is_some();
        // Drain the queue before reconfiguring the surface or replacing targets.
        // Otherwise allow two ordered submissions so a delayed completion callback
        // does not unnecessarily halve the presentation cadence.
        let busy = self.gpu_frames.is_full() || (resizing && pending_frames > 0);
        if pending_frames == 0 {
            if let Some((width, height)) = self.pending_resize.take() {
                self.canvas.set_width(width);
                self.canvas.set_height(height);
                self.config.width = width;
                self.config.height = height;
                self.surface.configure(&self.device, &self.config);
            }
            if let Some((width, height)) = self.pending_render_size.take() {
                self.depth = depth_view(&self.device, width, height);
                self.postprocess.resize(&self.device, width, height);
                self.render_size = (width, height);
                self.previous_camera = None;
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
        let sky_settings = self.ui.skybox_mut().map(|skybox| {
            if let Some(value) = edits.enabled {
                skybox.enabled = value;
            }
            if let Some(value) = edits.exposure {
                skybox.exposure = value;
            }
            if let Some(value) = edits.rotation_degrees {
                skybox.rotation_degrees = value;
            }
            (skybox.enabled, skybox.exposure, skybox.rotation_degrees)
        });
        if let Some((enabled, exposure, rotation)) = sky_settings {
            self.ui.show_sky_settings(enabled, exposure, rotation);
        }
        let selected_sky = self.ui.selected_sky();
        let seconds = (plan.total_ticks as f64 + f64::from(plan.simulation.alpha))
            * self.scheduler.tick_seconds();
        let angle = self.ui.angle(seconds);
        let (sin, cos) = angle.sin_cos();
        let player_camera = self.ui.player_camera().map_err(js_error)?;
        // Lua may add visual-only camera motion without moving the player collider.
        let eye = player_camera.render_position();
        let direction = player_camera.render_direction();
        let camera_up = player_camera.render_up();
        let up = [
            camera_up[0] * cos - camera_up[2] * sin,
            camera_up[1],
            camera_up[2] * cos + camera_up[0] * sin,
        ];
        let lens = self.ui.camera_lens().map_err(js_error)?;
        let target = [
            eye[0] + direction[0] * cos - direction[2] * sin,
            eye[1] + direction[1],
            eye[2] + direction[0] * sin + direction[2] * cos,
        ];
        let view = View {
            model: self.model,
            view: camera::look_at(eye, target, up),
            projection: camera::perspective(
                player_camera.render_fov_degrees().to_radians(),
                self.render_size.0 as f32 / self.render_size.1 as f32,
                0.05,
                self.far,
            ),
            width: self.render_size.0,
            height: self.render_size.1,
            near: 0.05,
            far: self.far,
        };
        let inverse =
            camera::inverse(&self.model).ok_or_else(|| js_error("invalid scene transform"))?;
        // Keep streaming distance anchored to the player, not the bobbing view.
        let local_eye = camera::transform_point(&inverse, player_camera.position);
        let base_target =
            [0, 1, 2].map(|axis| player_camera.position[axis] + target[axis] - eye[axis]);
        let local_target = camera::transform_point(&inverse, base_target);
        let local_direction = [0, 1, 2].map(|axis| local_target[axis] - local_eye[axis]);
        let clip = camera::mul(&view.projection, &camera::mul(&view.view, &view.model));
        if let Some(profile) = profile.as_mut() {
            profile.mark("scheduler_camera");
        }
        let level_lod = self.ui.level_lod().map_err(js_error)?;
        self.stream.settings.max_distance_m = self.ui.level_draw_distance().map_err(js_error)?;
        self.stream
            .set_fixed_lod(level_lod.map(|lod| lod as usize))
            .map_err(js_error)?;
        self.stream
            .set_focal_px(view.projection[5] * self.render_size.1 as f32);
        self.stream
            .select(local_eye, local_direction, Some(&clip), seconds as f32);
        if let Some(profile) = profile.as_mut() {
            profile.mark("lod_selection");
        }
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
        if let Some(profile) = profile.as_mut() {
            profile.mark("surface_acquire");
        }
        // Upload and retire ranges only when this frame can actually be submitted.
        // Advancing fades while the GPU is still busy can skip visible transition
        // steps and pile streaming work onto an already overloaded frame.
        self.stream.apply(&self.pool, &self.queue);
        if let Some(profile) = profile.as_mut() {
            profile.mark("stream_upload");
        }
        let color = frame.texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(self.surface_view_format),
            ..Default::default()
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        // Each one-shot timer surrounds the main encoded scene work only
        // when the local profiler arms a detailed frame.
        let capture_gpu = profile
            .as_ref()
            .and_then(|_| {
                self.gpu_timers
                    .iter()
                    .position(|timer| matches!(timer.timing(), GpuTiming::Ready))
            })
            .filter(|&index| self.gpu_timers[index].begin(&mut encoder, now_ms.to_bits()));
        let units_per_meter = self.ui.player_rig().map_or(1.0, |rig| rig.units_per_meter);
        let camera_direction = [0, 1, 2].map(|axis| target[axis] - eye[axis]);
        let shutter_fraction = self
            .previous_camera
            .as_ref()
            .and_then(|previous| {
                let delta = now.checked_sub(previous.time)?.as_secs_f32();
                let displacement = (0..3)
                    .map(|i| (eye[i] - previous.position[i]).powi(2))
                    .sum::<f32>()
                    .sqrt();
                let direction_dot: f32 = (0..3)
                    .map(|i| camera_direction[i] * previous.direction[i])
                    .sum();
                (lens.motion_blur
                    && !self.menu_visible
                    && (0.001..=0.2).contains(&delta)
                    && displacement <= 20.0 * units_per_meter
                    && direction_dot > 0.5)
                    .then_some((lens.shutter_seconds() / delta).clamp(0.0, 1.0))
            })
            .unwrap_or(0.0);
        let previous_clip = self
            .previous_camera
            .as_ref()
            .map_or(&clip, |previous| &previous.clip);
        self.pool.set_camera_effects(
            &self.queue,
            [
                lens.exposure_multiplier().log2(),
                lens.focus_distance_m * units_per_meter,
                lens.blur_scale_pixels(
                    player_camera.render_fov_degrees(),
                    self.render_size.1 as f32,
                ),
                shutter_fraction,
            ],
            previous_clip,
        );
        self.pool
            .prepare(&self.queue, &mut encoder, &view, &view.model);
        if let Some(index) = capture_gpu {
            self.gpu_timers[index].mark(&mut encoder, "splat_prepare");
        }
        if let Some(profile) = profile.as_mut() {
            profile.mark("splat_prepare");
        }
        {
            let mut pass = scene::color_pass(
                &mut encoder,
                self.postprocess.scene_view(),
                &self.depth,
                true,
                true,
                "CounterSplat HDR scene",
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
                    snapshot.skybox.exposure * lens.exposure_multiplier(),
                    snapshot.skybox.rotation_degrees,
                );
            }
            self.pool.draw_color(&mut pass, &self.pipelines);
            if !self.menu_visible {
                self.tracers
                    .draw(&self.queue, &mut pass, &view.view, &view.projection);
            }
            if !self.menu_visible && self.ui.show_voxels() {
                self.voxel_debug
                    .draw(&self.queue, &mut pass, &view.view, &view.projection);
            }
        }
        if let Some(index) = capture_gpu {
            self.gpu_timers[index].mark(&mut encoder, "scene_draw");
        }
        self.first_person.encode(
            &self.queue,
            &mut encoder,
            self.postprocess.scene_view(),
            self.render_size,
            self.first_person_settings,
        );
        // The render crate consumes a plain snapshot, never ECS or Lua state.
        let grade = self
            .ui
            .image_correction()
            .map(|grade| postprocess::Settings {
                enabled: grade.enabled,
                filmic: grade.filmic,
                exposure_ev: grade.exposure_ev,
                contrast: grade.contrast,
                saturation: grade.saturation,
                temperature: grade.temperature,
                tint: grade.tint,
                shadows: grade.shadows,
                highlights: grade.highlights,
                bloom_intensity: grade.bloom_intensity,
                bloom_threshold: grade.bloom_threshold,
                vignette: grade.vignette,
                grain: grade.grain,
                sharpness: grade.sharpness,
            })
            .unwrap_or_default();
        self.postprocess.encode_to_viewport(
            &self.queue,
            &mut encoder,
            &color,
            grade,
            (self.config.width, self.config.height),
            self.viewport_scale,
        );
        if let Some(index) = capture_gpu {
            self.gpu_timers[index].mark(&mut encoder, "first_person_post");
        }
        if let Some(profile) = profile.as_mut() {
            profile.mark("scene_post_encode");
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
            let result = self.menu.draw(ui_target);
            if let Some(profile) = profile.as_mut() {
                profile.mark("ui_menu");
            }
            result
        } else {
            self.ui.draw(ui_target, profile.as_mut())
        }
        .map_err(js_error)?;
        if let Some(index) = capture_gpu {
            self.gpu_timers[index].end(&mut encoder, "ui");
        }
        let permit = self
            .gpu_frames
            .try_acquire()
            .ok_or_else(|| js_error("GPU frame limit reached"))?;
        self.queue.submit(
            ui_buffers
                .into_iter()
                .chain(std::iter::once(encoder.finish())),
        );
        if let Some(index) = capture_gpu {
            self.gpu_timers[index].submitted(&self.queue);
        }
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
        self.queue.on_submitted_work_done(move || drop(permit));
        self.queue.present(frame);
        self.previous_camera = Some(PreviousCamera {
            time: now,
            clip,
            position: eye,
            direction: camera_direction,
        });
        if let Some(profile) = profile.as_mut() {
            profile.mark("submit_present");
        }
        self.ui.observe_frame(ui_submitted_at).map_err(js_error)?;
        if let Some(profile) = profile.as_mut() {
            profile.mark("frame_history");
        }
        self.submitted_frames = self.submitted_frames.saturating_add(1);
        self.last_frame_timing = profile;
        Ok(true)
    }

    /// Replace the script-provided projectile visual snapshot; no game rules live here.
    pub fn set_projectile_traces(&mut self, segments: Vec<f32>) -> Result<(), JsValue> {
        self.tracers.set(&self.queue, &segments).map_err(js_error)
    }

    /// Install a bounded, world-space snapshot of occupied voxel centers for the debug overlay.
    pub fn set_voxel_debug(&mut self, centers: Vec<f32>, size: f32) -> Result<(), JsValue> {
        self.voxel_debug
            .set(&self.queue, &centers, size)
            .map_err(js_error)
    }
    pub fn clear_voxel_debug(&mut self) {
        self.voxel_debug.clear();
    }

    /// Chunk folders requested for currently idle workers, as JSON `[file, folder]` pairs.
    pub fn requests(&mut self, available: usize) -> String {
        serde_json::to_string(&self.stream.requests_up_to(available))
            .unwrap_or_else(|_| "[]".into())
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
        let skybox = self.ui.skybox();
        let sky_loaded = skybox.is_some_and(|sky| self.sky_assets.contains_key(&sky.asset));
        let timing = self.ui.frame_measurement();
        let history_seconds = self.ui.history_seconds();
        let mut status = serde_json::json!({ "sky_loaded": sky_loaded, "sky_enabled": skybox.is_some_and(|sky| sky.enabled),
            "sky_exposure": skybox.map(|sky| sky.exposure), "sky_rotation": skybox.map(|sky| sky.rotation_degrees),
            "sky_asset": skybox.map(|sky| sky.asset.as_str()), "ui_history_seconds": history_seconds, "ui_frame_fps": timing.map(|value| value.fps), "ui_frame_ms": timing.map(|value| value.frame_time_ms),
            "ui_animate": ui_animate, "ui_sway": ui_sway, "ticks": self.total_ticks.to_string(), "submitted_frames": self.submitted_frames,
            "resident_splats": stats.resident_splats, "busy_skips": self.busy_skips,
            "player_camera": self.ui.player_camera().ok(), "camera_lens": self.ui.camera_lens().ok(),
            "player_physics": self.ui.player_physics(), "player_rig": self.ui.player_rig(),
            "level_lod": self.ui.level_lod().ok().flatten(), "level_asset_count": self.ui.level_file_count(),
            "leaves_per_lod": stats.leaves_per_lod,
            "octree_nodes": stats.octree_nodes, "lod_pending": stats.leaves_pending + stats.leaves_empty,
            "lod_wanted_splats": stats.wanted_splats, "lod_active_slots": stats.active_slots,
            "lod_free_slots": stats.free_slots, "lod_largest_free": stats.largest_free_range,
            "lod_pending_details": self.stream.pending_details(8),
            "lod_chunks_loaded": stats.chunks_loaded, "lod_chunks_in_flight": stats.chunks_in_flight,
            "lod_fading": stats.fading, "lod_switches": stats.switches, "fade_cuts": stats.fades_cut, "hard_swaps": stats.hard_swaps,
            "missed_deadlines": self.missed_deadlines.to_string(), "discarded_ms": self.discarded_time.as_secs_f64() * 1000.0,
            "target_fps": self.frame_limit,
            "presentation": "browser_vsync",
            "gpu_pending": !self.gpu_frames.is_empty(),
            "gpu_in_flight": self.gpu_frames.count(), "gpu_in_flight_limit": self.gpu_frames.limit() });
        status["display"] = serde_json::json!({
            "render": [self.render_size.0, self.render_size.1],
            "viewport": [self.config.width, self.config.height],
        });
        status.to_string()
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
/// Worker-owned PlayCanvas voxel collision data; never stores GPU or ECS objects.
#[wasm_bindgen]
pub struct VoxelCollisionData {
    inner: countersplat_physics::voxel_collision::VoxelCollider,
}
#[wasm_bindgen]
impl VoxelCollisionData {
    #[wasm_bindgen(constructor)]
    pub fn new(metadata: Vec<u8>, data: Vec<u8>) -> Result<VoxelCollisionData, JsValue> {
        let inner = countersplat_physics::voxel_collision::VoxelCollider::from_json_and_bin(
            &metadata, &data,
        )
        .map_err(js_error)?;
        Ok(Self { inner })
    }
    pub fn resolution(&self) -> f32 {
        self.inner.resolution()
    }
    pub fn move_sphere(
        &self,
        position: Vec<f32>,
        delta: Vec<f32>,
        radius: f32,
    ) -> Result<Vec<f32>, JsValue> {
        let position: [f32; 3] = position
            .try_into()
            .map_err(|_| js_error("expected a 3D position"))?;
        let delta: [f32; 3] = delta
            .try_into()
            .map_err(|_| js_error("expected a 3D movement"))?;
        Ok(self
            .inner
            .move_sphere(position, delta, radius)
            .map_err(js_error)?
            .to_vec())
    }
    /// Move an upright capsule through static voxel geometry.
    pub fn move_capsule(
        &self,
        position: Vec<f32>,
        delta: Vec<f32>,
        radius: f32,
        half_height: f32,
    ) -> Result<Vec<f32>, JsValue> {
        let position: [f32; 3] = position
            .try_into()
            .map_err(|_| js_error("expected a 3D position"))?;
        let delta: [f32; 3] = delta
            .try_into()
            .map_err(|_| js_error("expected a 3D movement"))?;
        Ok(self
            .inner
            .move_capsule(position, delta, radius, half_height)
            .map_err(js_error)?
            .to_vec())
    }
    /// Sweep a finite world-space segment against the worker-owned level octree.
    /// An empty array means no hit; otherwise values are fraction, position and normal.
    pub fn trace_segment(&self, start: Vec<f32>, end: Vec<f32>) -> Result<Vec<f32>, JsValue> {
        let start = start
            .try_into()
            .map_err(|_| js_error("expected a 3D segment start"))?;
        let end = end
            .try_into()
            .map_err(|_| js_error("expected a 3D segment end"))?;
        Ok(
            match self.inner.trace_segment(start, end).map_err(js_error)? {
                Some(hit) => vec![
                    hit.fraction,
                    hit.position[0],
                    hit.position[1],
                    hit.position[2],
                    hit.normal[0],
                    hit.normal[1],
                    hit.normal[2],
                ],
                None => Vec::new(),
            },
        )
    }

    /// Check a capsule volume without moving the player.
    pub fn capsule_clear(
        &self,
        center: Vec<f32>,
        radius: f32,
        half_height: f32,
    ) -> Result<bool, JsValue> {
        let center: [f32; 3] = center
            .try_into()
            .map_err(|_| js_error("expected a 3D capsule center"))?;
        self.inner
            .capsule_clear(center, radius, half_height)
            .map_err(js_error)
    }
    /// Probe voxel ground below a camera without moving the collider.
    pub fn ground_height(
        &self,
        position: Vec<f32>,
        sample_radius: f32,
        max_drop: f32,
    ) -> Result<JsValue, JsValue> {
        let position: [f32; 3] = position
            .try_into()
            .map_err(|_| js_error("expected a 3D position"))?;
        Ok(
            match self
                .inner
                .ground_height(position, sample_radius, max_drop)
                .map_err(js_error)?
            {
                Some(height) => JsValue::from_f64(f64::from(height)),
                None => JsValue::NULL,
            },
        )
    }
    pub fn nearby(
        &self,
        position: Vec<f32>,
        radius: f32,
        limit: usize,
    ) -> Result<Vec<f32>, JsValue> {
        let position: [f32; 3] = position
            .try_into()
            .map_err(|_| js_error("expected a 3D position"))?;
        Ok(self
            .inner
            .nearby_centers(position, radius, limit)
            .map_err(js_error)?
            .into_iter()
            .flatten()
            .collect())
    }
}

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
