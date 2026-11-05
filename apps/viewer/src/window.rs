//! Native on-screen level host with real surface presentation and an optional frame cap.
//!
//! The browser cannot turn off its compositor's synchronization. This host uses the surface's
//! advertised FIFO or Immediate mode and never silently substitutes a different mode.
use super::{threaded, Result};
use countersplat_core::{pacing::FrameLimiter, rate::Rate};
use countersplat_ecs::{Entity, World};
use countersplat_render::{
    camera, env, postprocess,
    presentation::{select_mode, Vsync},
    scene::{self, Pipelines, View},
    DEPTH_FORMAT,
};
use countersplat_scene::{SceneManifest, Skybox};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use winit::{
    application::ApplicationHandler,
    event::{ElementState, WindowEvent},
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    keyboard::{KeyCode, PhysicalKey},
    window::{Window, WindowId},
};

#[derive(Clone, Copy, Debug, PartialEq)]
enum Cap {
    Unlimited,
    Rate(Rate),
    Monitor,
}
#[derive(Debug, Clone, PartialEq)]
enum SkySource {
    Default,
    Path(PathBuf),
    Disabled,
}
#[derive(Debug)]
struct Options {
    capacity: u32,
    rotate: bool,
    vsync: Vsync,
    cap: Cap,
    sky: SkySource,
    scene_path: PathBuf,
    sky_exposure: Option<f32>,
    sky_rotation: Option<f32>,
}
impl Options {
    fn parse(args: &[String]) -> Result<Self> {
        let mut result = Self {
            capacity: 300_000,
            rotate: false,
            vsync: Vsync::On,
            cap: Cap::Rate(Rate::new(120, 1)?),
            sky: SkySource::Default,
            scene_path: PathBuf::from("assets/scenes/gameplay.json"),
            sky_exposure: None,
            sky_rotation: None,
        };
        for arg in args {
            if arg == "--z180" {
                result.rotate = true;
            } else if arg == "--no-sky" {
                result.sky = SkySource::Disabled;
            } else if let Some(value) = arg.strip_prefix("--sky=") {
                if value.is_empty() {
                    return Err("--sky requires an HDR file path".into());
                }
                result.sky = SkySource::Path(PathBuf::from(value));
            } else if let Some(value) = arg.strip_prefix("--scene=") {
                if value.is_empty() {
                    return Err("--scene requires a JSON file path".into());
                }
                result.scene_path = PathBuf::from(value);
            } else if let Some(value) = arg.strip_prefix("--sky-exposure=") {
                result.sky_exposure = Some(value.parse()?);
            } else if let Some(value) = arg.strip_prefix("--sky-rotation=") {
                result.sky_rotation = Some(value.parse()?);
            } else if let Some(value) = arg.strip_prefix("--capacity=") {
                result.capacity = value.parse()?;
                if result.capacity == 0 {
                    return Err("capacity must be positive".into());
                }
            } else if let Some(value) = arg.strip_prefix("--vsync=") {
                result.vsync = match value {
                    "on" => Vsync::On,
                    "off" => Vsync::Off,
                    _ => return Err("--vsync must be on or off".into()),
                };
            } else if let Some(value) = arg.strip_prefix("--fps=") {
                result.cap = match value {
                    "unlimited" => Cap::Unlimited,
                    "monitor" => Cap::Monitor,
                    _ => {
                        let fps: f64 = value.parse()?;
                        if !(1.0..=1000.0).contains(&fps) {
                            return Err("FPS cap must be between 1 and 1000".into());
                        }
                        Cap::Rate(Rate::from_hz(fps)?)
                    }
                };
            } else {
                return Err(format!("unknown window option: {arg}").into());
            }
        }
        Skybox {
            exposure: result.sky_exposure.unwrap_or(1.0),
            rotation_degrees: result.sky_rotation.unwrap_or(0.0),
            ..Default::default()
        }
        .validate()?;
        Ok(result)
    }
}

pub(super) fn run(level: &Path, args: &[String]) -> Result<()> {
    let options = Options::parse(args)?;
    let event_loop = EventLoop::new()?;
    let mut app = App {
        path: level.to_owned(),
        options: Some(options),
        state: None,
        error: None,
    };
    event_loop.run_app(&mut app)?;
    match app.error {
        Some(error) => Err(error.into()),
        None => Ok(()),
    }
}

struct App {
    path: PathBuf,
    options: Option<Options>,
    state: Option<State>,
    error: Option<String>,
}
struct State {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    format: wgpu::TextureFormat,
    modes: Vec<wgpu::PresentMode>,
    depth: wgpu::TextureView,
    postprocess: postprocess::PostProcess,
    gpu: threaded::Gpu,
    level: super::LoadedLevel,
    scene_world: World,
    skybox_entity: Option<Entity>,
    skybox_assets: BTreeMap<String, env::Sky>,
    vsync: Vsync,
    requested_vsync: Option<Vsync>,
    requested_resize: Option<(u32, u32)>,
    cap: Cap,
    limiter: FrameLimiter,
    pending_gpu: Arc<AtomicBool>,
    start: Instant,
    last_report: Instant,
    frames: u64,
    note: String,
}

fn depth(device: &wgpu::Device, width: u32, height: u32) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("CounterSplat native depth"),
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

/// Load scene data before opening a window or allocating GPU resources.
struct SceneSetup {
    world: World,
    control: Option<Entity>,
    sky_paths: BTreeMap<String, PathBuf>,
}
fn load_scene(options: &Options) -> Result<SceneSetup> {
    let manifest = SceneManifest::from_json(&std::fs::read(&options.scene_path)?)?;
    if manifest
        .resources
        .scripts
        .values()
        .any(|script| script.autoload)
    {
        return Err("the native window does not support autoloading scene scripts yet".into());
    }
    let mut world = World::new();
    countersplat_scene_ecs::register(&mut world)?;
    let entities = countersplat_scene_ecs::instantiate_layer(&mut world, &manifest, "scene")?;
    let sky_id = manifest
        .resources
        .values
        .get("preview")
        .and_then(|value| value.get("skybox_entity"))
        .and_then(|value| value.as_str())
        .ok_or("scene resource `preview.skybox_entity` must be an entity ID")?;
    let entity = *entities
        .get(sky_id)
        .ok_or("configured skybox entity is missing")?;
    let sky = world
        .get_mut::<Skybox>(entity)
        .ok_or("configured entity lacks Skybox")?;
    if let Some(exposure) = options.sky_exposure {
        sky.exposure = exposure;
    }
    if let Some(rotation) = options.sky_rotation {
        sky.rotation_degrees = rotation;
    }
    sky.validate()?;
    let controlled_asset = sky.asset.clone();
    let mut sky_paths = BTreeMap::new();
    if matches!(options.sky, SkySource::Disabled) {
        let ids: Vec<_> = world
            .query::<Skybox>()
            .iter()
            .map(|(entity, _)| *entity)
            .collect();
        for id in ids {
            world.get_mut::<Skybox>(id).unwrap().enabled = false;
        }
    } else {
        for (_, sky) in world.query::<Skybox>() {
            if sky_paths.contains_key(&sky.asset) {
                continue;
            }
            let asset = manifest
                .resources
                .assets
                .get(&sky.asset)
                .ok_or("skybox asset is missing")?;
            if asset.kind != "radiance_hdr" {
                return Err("native skybox requires a radiance_hdr asset".into());
            }
            let path = if sky.asset == controlled_asset {
                match &options.sky {
                    SkySource::Path(path) => Some(path.clone()),
                    _ => None,
                }
            } else {
                None
            };
            let path = match path {
                Some(path) => path,
                None => {
                    if asset.uri.contains("://") {
                        return Err("native skybox asset must use a local file path".into());
                    }
                    options
                        .scene_path
                        .parent()
                        .unwrap_or_else(|| Path::new("."))
                        .join(&asset.uri)
                }
            };
            sky_paths.insert(sky.asset.clone(), path);
        }
        if sky_paths.len() > 8 {
            return Err("native window supports at most eight skybox assets".into());
        }
    }
    Ok(SceneSetup {
        world,
        control: (!matches!(options.sky, SkySource::Disabled)).then_some(entity),
        sky_paths,
    })
}

impl State {
    fn create(event_loop: &ActiveEventLoop, path: &Path, options: Options) -> Result<Self> {
        let SceneSetup {
            world: scene_world,
            control: skybox_entity,
            sky_paths,
        } = load_scene(&options)?;
        let window = Arc::new(
            event_loop.create_window(
                Window::default_attributes()
                    .with_title("CounterSplat - loading level")
                    .with_inner_size(winit::dpi::LogicalSize::new(960.0, 540.0)),
            )?,
        );
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let surface = instance.create_surface(window.clone())?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            compatible_surface: Some(&surface),
            // The splat draw and sort are GPU-bound: prefer the faster GPU where there are two.
            power_preference: wgpu::PowerPreference::HighPerformance,
            ..Default::default()
        }))?;
        let limits = adapter.limits();
        let required_limits = wgpu::Limits {
            max_texture_dimension_2d: limits.max_texture_dimension_2d.clamp(8192, 16384),
            max_storage_buffer_binding_size: limits
                .max_storage_buffer_binding_size
                .clamp(128 << 20, 1 << 30),
            max_buffer_size: limits.max_buffer_size.clamp(256 << 20, 1 << 30),
            ..Default::default()
        };
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                required_limits,
                ..Default::default()
            }))?;
        let capabilities = surface.get_capabilities(&adapter);
        let base = capabilities
            .formats
            .iter()
            .copied()
            .find(|format| format.is_srgb())
            .or_else(|| capabilities.formats.first().copied())
            .ok_or("surface has no color format")?;
        let format = base.add_srgb_suffix();
        let mode = select_mode(options.vsync, &capabilities.present_modes)?;
        let size = window.inner_size();
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: base,
            color_space: Default::default(),
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: mode,
            alpha_mode: *capabilities
                .alpha_modes
                .first()
                .ok_or("surface has no alpha mode")?,
            view_formats: if format == base { vec![] } else { vec![format] },
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);
        let depth = depth(&device, config.width, config.height);
        let postprocess =
            postprocess::PostProcess::new(&device, format, config.width, config.height);
        let pipelines = Pipelines::new(&device, postprocess::SCENE_FORMAT)?;
        let gpu = threaded::Gpu {
            device,
            queue,
            pipelines,
            level: None,
        };
        // Reuse the asset worker and public scene preparation used by the existing viewer.
        let worker = threaded::start_assets()?;
        let client = worker.client();
        let prepared: Result<(super::LoadedLevel, Vec<(String, env::Cubemap)>)> = (|| {
            let data =
                match threaded::request(&client, threaded::AssetCommand::Level(path.to_owned()))? {
                    threaded::AssetReply::Level(data) => data,
                    _ => return Err("asset worker returned an unexpected reply".into()),
                };
            let level =
                super::prepare_level(data, path, options.capacity, options.rotate, &client, &gpu)?;
            let mut cubes = Vec::new();
            for (id, path) in sky_paths {
                let cube = match threaded::request(&client, threaded::AssetCommand::Sky(path))? {
                    threaded::AssetReply::Sky(cube) => cube,
                    _ => return Err("asset worker returned an unexpected sky reply".into()),
                };
                cubes.push((id, cube));
            }
            Ok((level, cubes))
        })();
        drop(client);
        let shutdown = worker.shutdown();
        let (level, cubes) = prepared?;
        shutdown?;
        let mut skybox_assets = BTreeMap::new();
        for (id, cube) in cubes {
            skybox_assets.insert(
                id,
                env::Sky::try_new(&gpu.device, &gpu.queue, &cube, postprocess::SCENE_FORMAT)?,
            );
        }
        let mut state = Self {
            window,
            surface,
            config,
            format,
            modes: capabilities.present_modes,
            depth,
            postprocess,
            gpu,
            level,
            scene_world,
            skybox_entity,
            skybox_assets,
            vsync: options.vsync,
            requested_vsync: None,
            requested_resize: None,
            cap: options.cap,
            limiter: FrameLimiter::new(None),
            pending_gpu: Arc::new(AtomicBool::new(false)),
            start: Instant::now(),
            last_report: Instant::now(),
            frames: 0,
            note: String::new(),
        };
        state.update_limiter()?;
        state.update_title();
        Ok(state)
    }
    fn monitor_rate(&self) -> Option<Rate> {
        self.window
            .current_monitor()
            .and_then(|monitor| monitor.refresh_rate_millihertz())
            .and_then(|millihertz| Rate::new(millihertz, 1000).ok())
    }
    fn update_limiter(&mut self) -> Result<()> {
        let limit = if self.vsync == Vsync::On {
            None
        } else {
            match self.cap {
                Cap::Unlimited => None,
                Cap::Rate(rate) => Some(rate),
                Cap::Monitor => Some(
                    self.monitor_rate()
                        .ok_or("monitor refresh rate is unavailable")?,
                ),
            }
        };
        self.limiter.set_cap(limit);
        Ok(())
    }
    fn update_title(&self) {
        let sync = if self.vsync == Vsync::On {
            "VSync on"
        } else {
            "VSync off"
        };
        let cap = match self.cap {
            Cap::Unlimited => "uncapped".to_owned(),
            Cap::Rate(rate) => format!("{:.1} FPS cap", rate.hz()),
            Cap::Monitor => "monitor FPS cap".to_owned(),
        };
        let monitor = self.monitor_rate().map_or("unknown Hz".to_owned(), |rate| {
            format!("{:.1} Hz", rate.hz())
        });
        let visible_cap = if self.vsync == Vsync::On {
            "display paced"
        } else {
            &cap
        };
        let note = if self.note.is_empty() {
            String::new()
        } else {
            format!(" | {}", self.note)
        };
        let sky = self
            .skybox_entity
            .and_then(|id| self.scene_world.get::<Skybox>(id));
        let sky_text = sky.map_or("no HDR sky", |sky| {
            if sky.enabled {
                "HDR sky on"
            } else {
                "HDR sky off"
            }
        });
        self.window.set_title(&format!(
            "CounterSplat | {sync} | {visible_cap} | {monitor} | {sky_text} | V: sync  F: cap  M: monitor  S: sky{note}"
        ));
    }
    fn toggle_sky(&mut self) {
        if let Some(skybox) = self
            .skybox_entity
            .and_then(|id| self.scene_world.get_mut::<Skybox>(id))
        {
            skybox.enabled = !skybox.enabled;
            self.update_title();
        }
    }
    fn toggle_vsync(&mut self) {
        let current = self.requested_vsync.unwrap_or(self.vsync);
        let next = if current == Vsync::On {
            Vsync::Off
        } else {
            Vsync::On
        };
        match select_mode(next, &self.modes) {
            Ok(_) => {
                self.requested_vsync = Some(next);
                self.note.clear();
            }
            Err(error) => self.note = error.into(),
        }
        self.update_title();
    }
    fn cycle_cap(&mut self) {
        self.cap = match self.cap {
            Cap::Unlimited => Cap::Rate(Rate::new(30, 1).unwrap()),
            Cap::Rate(rate) if rate.hz() < 45.0 => Cap::Rate(Rate::new(60, 1).unwrap()),
            Cap::Rate(rate) if rate.hz() < 90.0 => Cap::Rate(Rate::new(120, 1).unwrap()),
            Cap::Rate(rate) if rate.hz() < 132.0 => Cap::Rate(Rate::new(144, 1).unwrap()),
            Cap::Rate(rate) if rate.hz() < 200.0 => Cap::Rate(Rate::new(240, 1).unwrap()),
            _ => Cap::Unlimited,
        };
        if let Err(error) = self.update_limiter() {
            self.note = error.to_string();
        } else {
            self.note.clear();
        }
        self.update_title();
    }
    fn use_monitor_cap(&mut self) {
        if self.monitor_rate().is_some() {
            self.cap = Cap::Monitor;
            if let Err(error) = self.update_limiter() {
                self.note = error.to_string();
            } else {
                self.note.clear();
            }
        } else {
            self.note = "monitor refresh unavailable".into();
        }
        self.update_title();
    }
    /// Reconfigure only once the previous GPU submission has completed and no surface frame is held.
    fn reconfigure(&mut self) -> Result<()> {
        let mut changed = false;
        if let Some(mode) = self.requested_vsync.take() {
            self.config.present_mode = select_mode(mode, &self.modes)?;
            self.vsync = mode;
            self.update_limiter()?;
            changed = true;
        }
        if let Some((width, height)) = self.requested_resize.take() {
            if width > 0 && height > 0 {
                self.config.width = width;
                self.config.height = height;
                self.depth = depth(&self.gpu.device, width, height);
                self.postprocess.resize(&self.gpu.device, width, height);
                changed = true;
            }
        }
        if changed {
            self.surface.configure(&self.gpu.device, &self.config);
            self.update_title();
        }
        Ok(())
    }
    fn draw(&mut self) -> Result<()> {
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return Ok(())
            }
            wgpu::CurrentSurfaceTexture::Outdated => {
                self.surface.configure(&self.gpu.device, &self.config);
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Lost => return Err("native surface lost".into()),
            wgpu::CurrentSurfaceTexture::Validation => {
                return Err("native surface validation failed".into())
            }
        };
        let color = frame.texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(self.format),
            ..Default::default()
        });
        let mut view: View = self.level.view;
        view.width = self.config.width;
        view.height = self.config.height;
        view.projection[0] = view.projection[5] / (view.width as f32 / view.height as f32);
        let angle = ((self.start.elapsed().as_secs_f64() * 0.6).sin() * 0.08) as f32;
        let (sin, cos) = angle.sin_cos();
        let direction = self.level.direction;
        let target = [
            self.level.eye[0] + direction[0] * cos - direction[2] * sin,
            self.level.eye[1] + direction[1],
            self.level.eye[2] + direction[0] * sin + direction[2] * cos,
        ];
        view.view = camera::look_at(self.level.eye, target, self.level.up);
        let mut encoder = self.gpu.device.create_command_encoder(&Default::default());
        self.level
            .pool
            .prepare(&self.gpu.queue, &mut encoder, &view, &view.model);
        {
            let mut pass = scene::color_pass(
                &mut encoder,
                self.postprocess.scene_view(),
                &self.depth,
                true,
                true,
                "CounterSplat native HDR scene",
            );
            if let Some(snapshot) = countersplat_scene_ecs::extract(&self.scene_world) {
                if let Some(sky) = self.skybox_assets.get(&snapshot.skybox.asset) {
                    sky.draw_rotated(
                        &self.gpu.queue,
                        &mut pass,
                        &view.view,
                        &view.projection,
                        snapshot.skybox.exposure,
                        snapshot.skybox.rotation_degrees,
                    );
                }
            }
            self.level.pool.draw_color(&mut pass, &self.gpu.pipelines);
        }
        let grade = countersplat_scene_ecs::extract_image_correction(&self.scene_world)
            .map(|grade| postprocess::Settings {
                enabled: grade.enabled,
                filmic: grade.filmic,
                exposure_ev: grade.exposure_ev,
                contrast: grade.contrast,
                saturation: grade.saturation,
                temperature: grade.temperature,
                tint: grade.tint,
                shadow_warmth: grade.shadow_warmth,
                highlight_warmth: grade.highlight_warmth,
                shadows: grade.shadows,
                highlights: grade.highlights,
                bloom_intensity: grade.bloom_intensity,
                bloom_threshold: grade.bloom_threshold,
                vignette: grade.vignette,
                grain: grade.grain,
                sharpness: grade.sharpness,
                peripheral_blur: 0.0,
                tunnel_vision: 0.0,
            })
            .unwrap_or_default();
        self.postprocess
            .encode(&self.gpu.queue, &mut encoder, &color, grade);
        self.pending_gpu.store(true, Ordering::Release);
        self.gpu.queue.submit(Some(encoder.finish()));
        let pending = Arc::clone(&self.pending_gpu);
        self.gpu
            .queue
            .on_submitted_work_done(move || pending.store(false, Ordering::Release));
        self.gpu.queue.present(frame);
        self.frames = self.frames.saturating_add(1);
        if self.last_report.elapsed() >= Duration::from_secs(1) {
            let fps = self.frames as f64 / self.last_report.elapsed().as_secs_f64();
            self.note = format!("{fps:.1} submitted FPS");
            self.frames = 0;
            self.last_report = Instant::now();
            self.update_title();
        }
        Ok(())
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
        let Some(options) = self.options.take() else {
            return;
        };
        match State::create(event_loop, &self.path, options) {
            Ok(state) => self.state = Some(state),
            Err(error) => {
                self.error = Some(error.to_string());
                event_loop.exit();
            }
        }
    }
    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let Some(state) = &mut self.state else {
            return;
        };
        if state.window.id() != id {
            return;
        }
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => state.requested_resize = Some((size.width, size.height)),
            WindowEvent::KeyboardInput { event, .. }
                if event.state == ElementState::Pressed && !event.repeat =>
            {
                match event.physical_key {
                    PhysicalKey::Code(KeyCode::Escape) => event_loop.exit(),
                    PhysicalKey::Code(KeyCode::KeyV) => state.toggle_vsync(),
                    PhysicalKey::Code(KeyCode::KeyF) => state.cycle_cap(),
                    PhysicalKey::Code(KeyCode::KeyM) => state.use_monitor_cap(),
                    PhysicalKey::Code(KeyCode::KeyS) => state.toggle_sky(),
                    _ => {}
                }
            }
            WindowEvent::RedrawRequested if !state.pending_gpu.load(Ordering::Acquire) => {
                match state.limiter.poll(state.start.elapsed()) {
                    Ok(permit) if permit.due => {
                        if let Err(error) = state.draw() {
                            self.error = Some(error.to_string());
                            event_loop.exit();
                        }
                    }
                    Ok(_) => {}
                    Err(error) => {
                        self.error = Some(error.to_string());
                        event_loop.exit();
                    }
                }
            }
            _ => {}
        }
    }
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let Some(state) = &mut self.state else {
            return;
        };
        if let Err(error) = state.gpu.device.poll(wgpu::PollType::Poll) {
            self.error = Some(error.to_string());
            event_loop.exit();
            return;
        }
        if state.pending_gpu.load(Ordering::Acquire) {
            // Do not impose a 1 ms timer ceiling on an explicitly uncapped run.
            if state.vsync == Vsync::Off && matches!(state.cap, Cap::Unlimited) {
                std::thread::yield_now();
                event_loop.set_control_flow(ControlFlow::Poll);
            } else {
                event_loop.set_control_flow(ControlFlow::WaitUntil(
                    Instant::now() + Duration::from_millis(1),
                ));
            }
            return;
        }
        if let Err(error) = state.reconfigure() {
            self.error = Some(error.to_string());
            event_loop.exit();
            return;
        }
        if state.window.inner_size().width == 0 || state.window.inner_size().height == 0 {
            event_loop.set_control_flow(ControlFlow::Wait);
            return;
        }
        if state.vsync == Vsync::Off && matches!(state.cap, Cap::Monitor) {
            if let Err(error) = state.update_limiter() {
                state.note = error.to_string();
                state.update_title();
            }
        }
        if state.limiter.is_exhausted() {
            event_loop.set_control_flow(ControlFlow::Wait);
            return;
        }
        let now = state.start.elapsed();
        let next = state.limiter.next_deadline();
        if let Some(deadline) = next.filter(|deadline| *deadline > now) {
            let wake = state.start.checked_add(deadline);
            event_loop.set_control_flow(wake.map_or(ControlFlow::Wait, ControlFlow::WaitUntil));
        } else {
            state.window.request_redraw();
            event_loop.set_control_flow(ControlFlow::Poll);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_presentation_and_caps() {
        let options =
            Options::parse(&["--vsync=off".into(), "--fps=144".into(), "--z180".into()]).unwrap();
        assert_eq!(options.vsync, Vsync::Off);
        assert_eq!(options.cap, Cap::Rate(Rate::new(144, 1).unwrap()));
        assert!(options.rotate);
        assert!(Options::parse(&["--fps=NaN".into()]).is_err());
        assert!(Options::parse(&["--fps=0".into()]).is_err());
        assert!(Options::parse(&["--vsync=maybe".into()]).is_err());
        let sky = Options::parse(&[
            "--sky=custom.hdr".into(),
            "--sky-exposure=2".into(),
            "--sky-rotation=90".into(),
        ])
        .unwrap();
        assert_eq!(sky.sky, SkySource::Path(PathBuf::from("custom.hdr")));
        assert_eq!(sky.sky_exposure, Some(2.0));
        assert_eq!(sky.sky_rotation, Some(90.0));
        assert!(Options::parse(&["--sky-exposure=-1".into()]).is_err());
    }
    #[test]
    fn scene_json_supplies_native_sky_settings_and_cli_overrides() {
        let path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/scenes/gameplay.json");
        let scene_arg = format!("--scene={}", path.display());
        let options = Options::parse(&[
            scene_arg.clone(),
            "--sky-exposure=2".into(),
            "--sky-rotation=90".into(),
        ])
        .unwrap();
        let setup = load_scene(&options).unwrap();
        let sky = setup.world.get::<Skybox>(setup.control.unwrap()).unwrap();
        assert_eq!(sky.asset, "example-sky");
        assert_eq!(sky.exposure, 2.0);
        assert_eq!(sky.rotation_degrees, 90.0);
        assert!(setup.sky_paths["example-sky"].is_file());
        let disabled = Options::parse(&[scene_arg, "--no-sky".into()]).unwrap();
        let setup = load_scene(&disabled).unwrap();
        assert!(setup.control.is_none());
        assert!(setup.sky_paths.is_empty());
        assert!(countersplat_scene_ecs::extract(&setup.world).is_none());
    }
}
