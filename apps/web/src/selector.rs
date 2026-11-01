//! ECS-owned level selection scene for the browser host.
use crate::{js_error, ui_modifiers};
use countersplat_ecs::{Entity, World};
use countersplat_scene::{
    AssetLoader, AssetLoadingStatus, LevelCatalog, SceneCatalog, SceneManifest,
};
use countersplat_ui::{
    Color, InputEvent, PointerButton, Rect, Style, TextAlign, UiElement, UiEvent, UiEventKind,
    Widget,
};
use countersplat_ui_ecs::UiLayout;
use countersplat_ui_ecs::{apply_events, extract, register, spawn, EVENT_CHANNEL};
use countersplat_ui_wgpu::{UiRenderer, UiTarget};
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use wasm_bindgen::prelude::*;
use web_sys::HtmlCanvasElement;

#[derive(Clone, Copy)]
struct Controls {
    title: Entity,
    subtitle: Entity,
    list: Entity,
    page: Entity,
    previous: Entity,
    next: Entity,
    detail: Entity,
    preview: Entity,
    name: Entity,
    info: Entity,
    launch: Entity,
    scene_panel: Entity,
    scene_title: Entity,
    scene_help: Entity,
    scene_name: Entity,
    scene_info: Entity,
    scene_previous: Entity,
    scene_next: Entity,
    scene_open: Entity,
    notice: Entity,
    loader_overlay: Entity,
    loader_card: Entity,
    loader_title: Entity,
    loader_phase: Entity,
    loader_progress: Entity,
    loader_count: Entity,
    loader_cancel: Entity,
}
fn required(ids: &BTreeMap<String, Entity>, id: &str) -> Result<Entity, JsValue> {
    ids.get(id)
        .copied()
        .ok_or_else(|| js_error(format!("selector entity `{id}` is missing")))
}
fn short_name(text: &str, limit: usize) -> String {
    // Long location names often include a readable romanized form in parentheses.
    let text = if text.chars().count() > limit {
        text.split_once('(')
            .map_or(text, |(_, suffix)| suffix.trim_end_matches(')'))
    } else {
        text
    };
    if text.chars().count() <= limit {
        return text.to_owned();
    }
    let mut result: String = text.chars().take(limit.saturating_sub(1)).collect();
    result.push('…');
    result
}

#[derive(Clone, Copy)]
struct LevelCard {
    root: Entity,
    image: Entity,
    title: Entity,
}

#[wasm_bindgen]
pub struct Selector {
    canvas: HtmlCanvasElement,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    format: wgpu::TextureFormat,
    renderer: UiRenderer,
    world: World,
    layout: UiLayout,
    controls: Controls,
    catalog: LevelCatalog,
    scenes: SceneCatalog,
    rows: Vec<LevelCard>,
    selected_style: Style,
    unselected_style: Style,
    selected: usize,
    selected_scene: usize,
    page: usize,
    per_page: usize,
    busy: bool,
    launch: Option<String>,
    open_scene: Option<String>,
    cancel: bool,
    css_width: f32,
    pending_resize: Option<(u32, u32)>,
    pending_gpu: Arc<AtomicBool>,
}

#[wasm_bindgen]
impl Selector {
    /// Create the selector from a scene manifest and an independently validated level catalog.
    pub async fn create(
        canvas: HtmlCanvasElement,
        scene_json: Vec<u8>,
        catalog_json: Vec<u8>,
        scene_catalog_json: Vec<u8>,
        selected_id: String,
        selected_scene_id: String,
    ) -> Result<Selector, JsValue> {
        let scene = SceneManifest::from_json(&scene_json).map_err(js_error)?;
        let catalog = LevelCatalog::from_json(&catalog_json).map_err(js_error)?;
        let scenes = SceneCatalog::from_json(&scene_catalog_json).map_err(js_error)?;
        let config = scene
            .resources
            .values
            .get("selector")
            .ok_or_else(|| js_error("selector settings are missing"))?;
        let template: UiElement = serde_json::from_value(
            config
                .get("item_template")
                .cloned()
                .ok_or_else(|| js_error("selector item template is missing"))?,
        )
        .map_err(js_error)?;
        template.validate().map_err(js_error)?;
        if !matches!(template.widget, Widget::Button { .. }) {
            return Err(js_error("selector item template must be a button"));
        }
        let selected_style: Style = serde_json::from_value(
            config
                .get("selected_style")
                .cloned()
                .ok_or_else(|| js_error("selected style is missing"))?,
        )
        .map_err(js_error)?;
        let unselected_style: Style = serde_json::from_value(
            config
                .get("unselected_style")
                .cloned()
                .ok_or_else(|| js_error("unselected style is missing"))?,
        )
        .map_err(js_error)?;
        let mut world = World::new();
        register(&mut world).map_err(js_error)?;
        countersplat_scene_ecs::register(&mut world).map_err(js_error)?;
        let mut ids = countersplat_scene_ecs::instantiate_layer(&mut world, &scene, "ui")
            .map_err(js_error)?;
        let controls = Controls {
            title: required(&ids, "title")?,
            subtitle: required(&ids, "subtitle")?,
            list: required(&ids, "list_panel")?,
            page: required(&ids, "page_info")?,
            previous: required(&ids, "previous")?,
            next: required(&ids, "next")?,
            detail: required(&ids, "detail_panel")?,
            preview: required(&ids, "selected_preview")?,
            name: required(&ids, "selected_name")?,
            info: required(&ids, "selected_info")?,
            launch: required(&ids, "launch")?,
            scene_panel: required(&ids, "scene_panel")?,
            scene_title: required(&ids, "scene_title")?,
            scene_help: required(&ids, "scene_help")?,
            scene_name: required(&ids, "scene_name")?,
            scene_info: required(&ids, "scene_info")?,
            scene_previous: required(&ids, "scene_previous")?,
            scene_next: required(&ids, "scene_next")?,
            scene_open: required(&ids, "scene_open")?,
            notice: required(&ids, "notice")?,
            loader_overlay: required(&ids, "loader_overlay")?,
            loader_card: required(&ids, "loader_card")?,
            loader_title: required(&ids, "loader_title")?,
            loader_phase: required(&ids, "loader_phase")?,
            loader_progress: required(&ids, "loader_progress")?,
            loader_count: required(&ids, "loader_count")?,
            loader_cancel: required(&ids, "loader_cancel")?,
        };
        if !world.has::<AssetLoader>(controls.loader_overlay) {
            return Err(js_error("loading overlay needs an AssetLoader component"));
        }
        if !world
            .get::<UiElement>(controls.loader_progress)
            .is_some_and(|e| matches!(e.widget, Widget::Progress { .. }))
        {
            return Err(js_error(
                "loading progress entity must be a progress widget",
            ));
        }
        for id in [
            controls.previous,
            controls.next,
            controls.launch,
            controls.scene_previous,
            controls.scene_next,
            controls.scene_open,
            controls.loader_cancel,
        ] {
            if !world
                .get::<UiElement>(id)
                .is_some_and(|e| matches!(e.widget, Widget::Button { .. }))
            {
                return Err(js_error("selector action must be a button"));
            }
        }
        let mut rows = Vec::with_capacity(catalog.levels.len());
        for level in &catalog.levels {
            let entity =
                spawn(&mut world, Some(controls.list), template.clone()).map_err(js_error)?;
            world
                .set_dynamic(entity, "LevelChoice", serde_json::json!({"id":level.id}))
                .map_err(js_error)?;
            if ids.insert(format!("@level/{}", level.id), entity).is_some() {
                return Err(js_error("selector layout ID collision"));
            }
            let image = spawn(
                &mut world,
                Some(entity),
                UiElement {
                    z: 4,
                    draggable: false,
                    widget: Widget::Image {
                        source: level.id.clone(),
                    },
                    style: Style {
                        fill: Color([13, 30, 39, 255]),
                        border: Color::TRANSPARENT,
                        text: Color([151, 185, 187, 255]),
                        font_size: 12.0,
                        rounding: 4.0,
                        ..Default::default()
                    },
                    ..Default::default()
                },
            )
            .map_err(js_error)?;
            let title = spawn(
                &mut world,
                Some(entity),
                UiElement {
                    z: 5,
                    draggable: false,
                    widget: Widget::Text {
                        text: level.name.clone(),
                        align: TextAlign::Left,
                    },
                    style: Style {
                        fill: Color::TRANSPARENT,
                        border: Color::TRANSPARENT,
                        text: Color([235, 245, 241, 255]),
                        font_size: 14.0,
                        ..Default::default()
                    },
                    ..Default::default()
                },
            )
            .map_err(js_error)?;
            rows.push(LevelCard {
                root: entity,
                image,
                title,
            });
        }
        let layout = UiLayout::new(&world, ids);
        let selected = catalog
            .levels
            .iter()
            .position(|level| level.id == selected_id)
            .unwrap_or(0);
        let selected_scene = scenes
            .scenes
            .iter()
            .position(|scene| scene.id == selected_scene_id)
            .unwrap_or(0);
        world.set_dynamic_resource(
            "selector.selected_level",
            serde_json::json!(catalog.levels[selected].id),
        );
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
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default())
            .await
            .map_err(js_error)?;
        let caps = surface.get_capabilities(&adapter);
        let base = caps
            .formats
            .iter()
            .copied()
            .find(|format| format.is_srgb())
            .or_else(|| caps.formats.first().copied())
            .ok_or_else(|| js_error("canvas has no supported color format"))?;
        let format = base.add_srgb_suffix();
        let width = canvas.width().max(1);
        let height = canvas.height().max(1);
        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: base,
            color_space: Default::default(),
            width,
            height,
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: *caps
                .alpha_modes
                .first()
                .ok_or_else(|| js_error("canvas has no alpha mode"))?,
            view_formats: if base == format { vec![] } else { vec![format] },
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &surface_config);
        let renderer = UiRenderer::new(&device, format);
        Ok(Self {
            canvas,
            surface,
            device,
            queue,
            config: surface_config,
            format,
            renderer,
            world,
            layout,
            controls,
            catalog,
            scenes,
            rows,
            selected_style,
            unselected_style,
            selected,
            selected_scene,
            page: 0,
            per_page: 0,
            busy: false,
            launch: None,
            open_scene: None,
            cancel: false,
            css_width: width as f32,
            pending_resize: None,
            pending_gpu: Arc::new(AtomicBool::new(false)),
        })
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        let limit = self.device.limits().max_texture_dimension_2d;
        let size = (width.clamp(1, limit), height.clamp(1, limit));
        self.pending_resize = (size != (self.config.width, self.config.height)).then_some(size);
    }
    /// Maximum supported 2D texture dimension for viewport sizing.
    pub fn max_texture_dimension(&self) -> u32 {
        self.device.limits().max_texture_dimension_2d
    }
    pub fn ui_set_width(&mut self, width: f32) {
        if width.is_finite() && (1.0..=1_000_000.0).contains(&width) {
            self.css_width = width;
        }
    }
    pub fn ui_set_arrange_mode(&mut self, enabled: bool) {
        self.renderer.set_arrange_mode(enabled);
    }
    pub fn ui_layout(&self) -> Result<String, JsValue> {
        self.layout.save(&self.world).map_err(js_error)
    }
    pub fn ui_restore_layout(&mut self, encoded: String) -> Result<(), JsValue> {
        self.layout
            .restore(&mut self.world, &encoded)
            .map_err(js_error)
    }
    pub fn ui_reset_layout(&mut self) {
        self.layout.reset(&mut self.world);
    }
    pub fn ui_take_layout_changed(&mut self) -> bool {
        self.layout.take_changed()
    }
    pub fn ui_pointer_move(&mut self, x: f32, y: f32) -> Result<(), JsValue> {
        self.renderer
            .push_input(InputEvent::PointerMoved([x, y]))
            .map_err(js_error)
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
        self.renderer
            .push_input(InputEvent::PointerButton {
                position: [x, y],
                button,
                pressed,
                modifiers: ui_modifiers(modifiers),
            })
            .map_err(js_error)
    }
    pub fn ui_pointer_gone(&mut self) -> Result<(), JsValue> {
        self.renderer
            .push_input(InputEvent::PointerGone)
            .map_err(js_error)
    }
    pub fn ui_scroll(&mut self, x: f32, y: f32) -> Result<(), JsValue> {
        self.renderer
            .push_input(InputEvent::Scroll([x, y]))
            .map_err(js_error)
    }
    pub fn ui_key(
        &mut self,
        name: String,
        pressed: bool,
        repeat: bool,
        modifiers: u8,
    ) -> Result<(), JsValue> {
        self.renderer
            .push_input(InputEvent::Key {
                name,
                pressed,
                repeat,
                modifiers: ui_modifiers(modifiers),
            })
            .map_err(js_error)
    }
    pub fn ui_text(&mut self, text: String) -> Result<(), JsValue> {
        self.renderer
            .push_input(InputEvent::Text(text))
            .map_err(js_error)
    }
    pub fn ui_focus(&mut self, focused: bool) -> Result<(), JsValue> {
        self.renderer
            .push_input(InputEvent::Focus(focused))
            .map_err(js_error)
    }
    pub fn ui_wants_keyboard(&self) -> bool {
        self.renderer.wants_keyboard()
    }
    pub fn ui_wants_pointer(&self) -> bool {
        self.renderer.wants_pointer()
    }
    pub fn set_busy(&mut self, busy: bool) {
        self.busy = busy;
        if let Some(element) = self
            .world
            .get_mut::<UiElement>(self.controls.loader_overlay)
        {
            element.visible = busy;
        }
    }
    /// Show the ECS loading overlay while preloading assets for a selected level.
    pub fn begin_loading(&mut self, level_name: String) -> Result<(), JsValue> {
        self.cancel = false;
        self.set_busy(true);
        self.set_text(
            self.controls.loader_title,
            &format!("Loading {}", short_name(&level_name, 36)),
        );
        if let Some(element) = self.world.get_mut::<UiElement>(self.controls.loader_cancel) {
            element.enabled = true;
        }
        self.update_loading("Preparing assets".into(), 0.0, 0, 1)
    }
    /// Publish actual progress through the AssetLoader ECS component and its UI widgets.
    pub fn update_loading(
        &mut self,
        phase: String,
        progress: f32,
        completed: u32,
        total: u32,
    ) -> Result<(), JsValue> {
        let state = AssetLoader {
            status: AssetLoadingStatus::Loading,
            phase: phase.clone(),
            progress,
            completed,
            total,
        };
        state.validate().map_err(js_error)?;
        self.world
            .insert(self.controls.loader_overlay, state)
            .map_err(js_error)?;
        self.set_text(self.controls.loader_phase, &phase);
        self.set_text(
            self.controls.loader_count,
            &format!("{:.0}%  ·  {} / {}", progress * 100.0, completed, total),
        );
        if let Some(UiElement {
            widget: Widget::Progress { value },
            ..
        }) = self
            .world
            .get_mut::<UiElement>(self.controls.loader_progress)
        {
            *value = progress;
        }
        Ok(())
    }
    pub fn finish_loading(&mut self, outcome: &str) -> Result<(), JsValue> {
        let status = match outcome {
            "complete" => AssetLoadingStatus::Complete,
            "cancelled" => AssetLoadingStatus::Cancelled,
            "failed" => AssetLoadingStatus::Failed,
            _ => return Err(js_error("unknown asset-loading outcome")),
        };
        let state = self
            .world
            .get_mut::<AssetLoader>(self.controls.loader_overlay)
            .ok_or_else(|| js_error("asset loader is missing"))?;
        state.status = status;
        if status == AssetLoadingStatus::Complete {
            state.progress = 1.0;
        }
        Ok(())
    }
    /// Disable cancellation once all preloads are ready and the renderer is starting.
    pub fn set_loading_cancellable(&mut self, enabled: bool) {
        if let Some(element) = self.world.get_mut::<UiElement>(self.controls.loader_cancel) {
            element.enabled = enabled;
        }
    }
    pub fn take_cancel_loading(&mut self) -> bool {
        std::mem::take(&mut self.cancel)
    }
    /// Expose the loading component and scene resources through the standard ECS transport.
    pub fn ecs_operation(&mut self, request: &str) -> Result<String, JsValue> {
        self.world.apply_script_json(request).map_err(js_error)
    }
    pub fn set_notice(&mut self, message: String) {
        self.set_text(self.controls.notice, &message);
    }
    pub fn take_launch(&mut self) -> String {
        self.launch.take().unwrap_or_default()
    }
    /// Return a selected scene/level request for the host to validate and open.
    pub fn take_open_scene(&mut self) -> String {
        self.open_scene.take().unwrap_or_default()
    }
    /// Register a decoded image for a catalog level; missing images use a placeholder.
    pub fn install_level_preview(
        &mut self,
        id: String,
        width: u32,
        height: u32,
        rgba: &[u8],
    ) -> Result<(), JsValue> {
        if self.catalog.get(&id).is_none() {
            return Err(js_error("unknown preview level"));
        }
        self.renderer
            .set_image_rgba(&id, [width as usize, height as usize], rgba)
            .map_err(js_error)
    }
    pub fn selected_id(&self) -> String {
        self.catalog.levels[self.selected].id.clone()
    }
    pub fn set_paused(&mut self, _paused: bool) {}

    /// Draw directly to the canvas and apply accepted UI events on the ECS owner.
    pub fn frame(&mut self, now_ms: f64) -> Result<bool, JsValue> {
        if !now_ms.is_finite() || now_ms < 0.0 {
            return Err(js_error("invalid frame time"));
        }
        if self.pending_gpu.load(Ordering::Acquire) {
            return Ok(false);
        }
        if let Some((width, height)) = self.pending_resize.take() {
            self.canvas.set_width(width);
            self.canvas.set_height(height);
            self.config.width = width;
            self.config.height = height;
            self.surface.configure(&self.device, &self.config);
        }
        let scale = self.config.width as f32 / self.css_width;
        let width = self.css_width;
        let height = self.config.height as f32 / scale;
        self.layout(width, height);
        let frame_ui =
            extract(&self.world, Rect::new(0.0, 0.0, width, height)).map_err(js_error)?;
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
            wgpu::CurrentSurfaceTexture::Lost => return Err(js_error("selector surface was lost")),
            wgpu::CurrentSurfaceTexture::Validation => {
                return Err(js_error("selector surface validation failed"))
            }
        };
        let color = frame.texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(self.format),
            ..Default::default()
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Level selector"),
            });
        {
            let _clear = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Clear selector"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &color,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.012,
                            g: 0.026,
                            b: 0.033,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
        }
        let output = self
            .renderer
            .draw(
                UiTarget {
                    device: &self.device,
                    queue: &self.queue,
                    encoder: &mut encoder,
                    color: &color,
                    size: [self.config.width, self.config.height],
                    pixels_per_point: scale,
                    time_seconds: now_ms / 1000.0,
                },
                &frame_ui,
            )
            .map_err(js_error)?;
        self.handle_actions(output.events);
        self.pending_gpu.store(true, Ordering::Release);
        self.queue.submit(
            output
                .command_buffers
                .into_iter()
                .chain(std::iter::once(encoder.finish())),
        );
        self.renderer.after_submit();
        let pending = Arc::clone(&self.pending_gpu);
        self.queue
            .on_submitted_work_done(move || pending.store(false, Ordering::Release));
        self.queue.present(frame);
        Ok(true)
    }
}

impl Selector {
    fn set_rect(&mut self, id: Entity, rect: Rect) {
        if let Some(element) = self.world.get_mut::<UiElement>(id) {
            element.rect = rect;
        }
    }
    fn set_text(&mut self, id: Entity, value: &str) {
        if let Some(UiElement {
            widget: Widget::Text { text, .. },
            ..
        }) = self.world.get_mut::<UiElement>(id)
        {
            *text = value.to_owned();
        }
    }
    fn layout(&mut self, width: f32, height: f32) {
        self.set_rect(
            self.controls.loader_overlay,
            Rect::new(0.0, 0.0, width, height),
        );
        let card_width = (width - 24.0).clamp(20.0, 480.0);
        self.set_rect(
            self.controls.loader_card,
            Rect::new(0.0, 0.0, card_width, 220.0),
        );
        let content_width = (card_width - 48.0).max(1.0);
        self.set_rect(
            self.controls.loader_title,
            Rect::new(24.0, 18.0, content_width, 35.0),
        );
        self.set_rect(
            self.controls.loader_phase,
            Rect::new(24.0, 63.0, content_width, 23.0),
        );
        self.set_rect(
            self.controls.loader_progress,
            Rect::new(24.0, 101.0, content_width, 20.0),
        );
        self.set_rect(
            self.controls.loader_count,
            Rect::new(24.0, 130.0, content_width, 20.0),
        );
        self.set_rect(
            self.controls.loader_cancel,
            Rect::new(24.0, 167.0, content_width.min(150.0), 34.0),
        );
        // Wide screens pair the image gallery with independent level and scene panels.
        // The same layout compresses to fewer cards and smaller controls on narrow canvases.
        let compact = height < 480.0;
        let margin = if width < 540.0 { 12.0 } else { 24.0 };
        let list_y = if compact { 60.0 } else { 84.0 };
        let side_width = (width * 0.30)
            .clamp(if width < 650.0 { 135.0 } else { 220.0 }, 340.0)
            .min(((width - 3.0 * margin) * 0.48).max(30.0));
        let list_width = (width - 3.0 * margin - side_width).max(40.0);
        let side_x = margin + list_width + margin;
        let pager_y = (height - margin - 25.0).max(list_y + 32.0);
        let list_height = (pager_y - list_y - 8.0).max(24.0);
        self.set_rect(
            self.controls.title,
            Rect::new(margin, 10.0, width - 2.0 * margin, 34.0),
        );
        self.set_rect(
            self.controls.subtitle,
            Rect::new(margin, 43.0, width - 2.0 * margin, 20.0),
        );
        self.set_rect(
            self.controls.list,
            Rect::new(margin, list_y, list_width, list_height),
        );
        let columns = if list_width >= 650.0 {
            3
        } else if list_width >= 330.0 {
            2
        } else {
            1
        };
        let minimum_height = if compact { 88.0 } else { 155.0 };
        let rows = (((list_height - 16.0) / minimum_height).floor() as usize).clamp(1, 3);
        let per_page = columns * rows;
        if self.per_page == 0 {
            self.page = self.selected / per_page;
        }
        self.per_page = per_page;
        let pages = self.catalog.levels.len().div_ceil(per_page);
        self.page = self.page.min(pages - 1);
        let start = self.page * per_page;
        let end = (start + per_page).min(self.rows.len());
        let gap = 8.0;
        let card_width =
            ((list_width - 20.0 - gap * (columns - 1) as f32) / columns as f32).max(20.0);
        let card_height = ((list_height - 16.0 - gap * (rows - 1) as f32) / rows as f32).max(42.0);
        for (index, card) in self.rows.iter().copied().enumerate() {
            let visible = (start..end).contains(&index);
            if let Some(element) = self.world.get_mut::<UiElement>(card.root) {
                element.visible = visible;
                element.enabled = !self.busy;
                if visible {
                    let slot = index - start;
                    element.rect = Rect::new(
                        10.0 + (slot % columns) as f32 * (card_width + gap),
                        8.0 + (slot / columns) as f32 * (card_height + gap),
                        card_width,
                        card_height,
                    );
                    element.style = if index == self.selected {
                        self.selected_style.clone()
                    } else {
                        self.unselected_style.clone()
                    };
                    if let Widget::Button { label } = &mut element.widget {
                        label.clear();
                    }
                }
            }
            if let Some(element) = self.world.get_mut::<UiElement>(card.image) {
                element.rect = Rect::new(
                    6.0,
                    6.0,
                    (card_width - 12.0).max(8.0),
                    (card_height - 39.0).max(12.0),
                );
            }
            if let Some(element) = self.world.get_mut::<UiElement>(card.title) {
                element.rect = Rect::new(
                    10.0,
                    (card_height - 30.0).max(8.0),
                    (card_width - 20.0).max(8.0),
                    22.0,
                );
                if let Widget::Text { text, .. } = &mut element.widget {
                    *text = short_name(
                        &self.catalog.levels[index].name,
                        (card_width / 7.5).floor().max(8.0) as usize,
                    );
                }
            }
        }
        self.set_rect(
            self.controls.page,
            Rect::new(margin + 2.0, pager_y, (list_width - 160.0).max(30.0), 22.0),
        );
        self.set_rect(
            self.controls.previous,
            Rect::new(margin + list_width - 150.0, pager_y, 70.0, 24.0),
        );
        self.set_rect(
            self.controls.next,
            Rect::new(margin + list_width - 74.0, pager_y, 74.0, 24.0),
        );
        self.set_text(
            self.controls.page,
            &format!("{}–{} of {} levels", start + 1, end, self.rows.len()),
        );
        for (id, enabled) in [
            (self.controls.previous, self.page > 0),
            (self.controls.next, self.page + 1 < pages),
        ] {
            if let Some(element) = self.world.get_mut::<UiElement>(id) {
                element.visible = pages > 1;
                element.enabled = enabled && !self.busy;
            }
        }

        let available = (height - list_y - margin - 18.0).max(80.0);
        let detail_height = if compact {
            (available * 0.48).max(60.0)
        } else {
            (available * 0.55).clamp(150.0, 320.0)
        };
        let scene_y = list_y + detail_height + 12.0;
        let scene_height = (height - margin - scene_y - 18.0).max(45.0);
        self.set_rect(
            self.controls.detail,
            Rect::new(side_x, list_y, side_width, detail_height),
        );
        self.set_rect(
            self.controls.scene_panel,
            Rect::new(side_x, scene_y, side_width, scene_height),
        );
        let show_preview = detail_height >= 220.0;
        let preview_height = (detail_height - 126.0).max(20.0);
        self.set_rect(
            self.controls.preview,
            Rect::new(12.0, 12.0, (side_width - 24.0).max(8.0), preview_height),
        );
        if let Some(element) = self.world.get_mut::<UiElement>(self.controls.preview) {
            element.visible = show_preview;
            element.widget = Widget::Image {
                source: self.catalog.levels[self.selected].id.clone(),
            };
        }
        let name_y = if show_preview {
            preview_height + 20.0
        } else {
            10.0
        };
        self.set_rect(
            self.controls.name,
            Rect::new(14.0, name_y, (side_width - 28.0).max(8.0), 23.0),
        );
        self.set_rect(
            self.controls.info,
            Rect::new(14.0, name_y + 25.0, (side_width - 28.0).max(8.0), 18.0),
        );
        self.set_rect(
            self.controls.launch,
            Rect::new(
                12.0,
                (detail_height - 46.0).max(48.0),
                (side_width - 24.0).max(30.0),
                34.0,
            ),
        );
        let level = &self.catalog.levels[self.selected];
        let max_chars = ((side_width - 28.0).max(20.0) / 8.0).floor().max(6.0) as usize;
        let name = short_name(&level.name, max_chars);
        let detail = match level.lod_levels {
            Some(n) => format!(
                "{}  |  {} detail levels",
                short_name(&level.creator, max_chars.saturating_sub(18).max(6)),
                n
            ),
            None => format!(
                "{}  |  single scan",
                short_name(&level.creator, max_chars.saturating_sub(16).max(6))
            ),
        };
        self.set_text(self.controls.name, &name);
        self.set_text(self.controls.info, &detail);
        if let Some(element) = self.world.get_mut::<UiElement>(self.controls.launch) {
            element.enabled = !self.busy;
        }

        let show_help = scene_height >= 190.0;
        self.set_rect(
            self.controls.scene_title,
            Rect::new(14.0, 8.0, (side_width - 28.0).max(8.0), 26.0),
        );
        self.set_rect(
            self.controls.scene_help,
            Rect::new(14.0, 38.0, (side_width - 28.0).max(8.0), 19.0),
        );
        if let Some(element) = self.world.get_mut::<UiElement>(self.controls.scene_help) {
            element.visible = show_help;
        }
        let scene_name_y = if show_help { 72.0 } else { 36.0 };
        self.set_rect(
            self.controls.scene_name,
            Rect::new(14.0, scene_name_y, (side_width - 28.0).max(8.0), 22.0),
        );
        self.set_rect(
            self.controls.scene_info,
            Rect::new(
                14.0,
                scene_name_y + 23.0,
                (side_width - 28.0).max(8.0),
                18.0,
            ),
        );
        if let Some(element) = self.world.get_mut::<UiElement>(self.controls.scene_info) {
            element.visible = scene_height >= 130.0;
        }
        let scene_action_y = (scene_height - 46.0).max(62.0);
        self.set_rect(
            self.controls.scene_previous,
            Rect::new(12.0, scene_action_y, 32.0, 34.0),
        );
        self.set_rect(
            self.controls.scene_next,
            Rect::new(48.0, scene_action_y, 32.0, 34.0),
        );
        self.set_rect(
            self.controls.scene_open,
            Rect::new(88.0, scene_action_y, (side_width - 100.0).max(30.0), 34.0),
        );
        let selected_scene = self.scenes.scenes.get(self.selected_scene);
        let scene_name = selected_scene
            .map(|scene| short_name(&scene.name, max_chars))
            .unwrap_or_else(|| "No playable scenes".into());
        let scene_info = selected_scene
            .map(|scene| short_name(&scene.id, max_chars))
            .unwrap_or_else(|| "Add a scene JSON".into());
        let has_scene = selected_scene.is_some();
        self.set_text(self.controls.scene_name, &scene_name);
        self.set_text(self.controls.scene_info, &scene_info);
        for (id, enabled) in [
            (self.controls.scene_previous, self.selected_scene > 0),
            (
                self.controls.scene_next,
                self.selected_scene + 1 < self.scenes.scenes.len(),
            ),
            (self.controls.scene_open, has_scene),
        ] {
            if let Some(element) = self.world.get_mut::<UiElement>(id) {
                element.enabled = enabled && !self.busy;
            }
        }
        self.set_rect(
            self.controls.notice,
            Rect::new(margin, height - 17.0, width - 2.0 * margin, 16.0),
        );
    }
    fn handle_actions(&mut self, events: Vec<UiEvent>) {
        for event in apply_events(&mut self.world, events) {
            if matches!(event.kind, UiEventKind::MovedBy(_)) {
                self.layout.changed();
            }
            if event.kind == UiEventKind::Clicked
                && self.busy
                && event.id == self.controls.loader_cancel.to_string()
            {
                self.cancel = true;
                continue;
            }
            if event.kind != UiEventKind::Clicked || self.busy {
                continue;
            }
            if let Some(index) = self
                .rows
                .iter()
                .position(|card| card.root.to_string() == event.id)
            {
                self.selected = index;
                self.world.set_dynamic_resource(
                    "selector.selected_level",
                    serde_json::json!(self.catalog.levels[index].id),
                );
                self.set_text(self.controls.notice, "");
            } else if event.id == self.controls.previous.to_string() && self.page > 0 {
                self.page -= 1;
            } else if event.id == self.controls.next.to_string()
                && (self.page + 1) * self.per_page < self.rows.len()
            {
                self.page += 1;
            } else if event.id == self.controls.scene_previous.to_string()
                && self.selected_scene > 0
            {
                self.selected_scene -= 1;
            } else if event.id == self.controls.scene_next.to_string()
                && self.selected_scene + 1 < self.scenes.scenes.len()
            {
                self.selected_scene += 1;
            } else if event.id == self.controls.scene_open.to_string() {
                if let Some(scene) = self.scenes.scenes.get(self.selected_scene) {
                    let request = serde_json::json!({"scene":scene.id, "level":self.catalog.levels[self.selected].id});
                    self.world.set_dynamic_resource(
                        "selector.selected_scene",
                        serde_json::json!(scene.id),
                    );
                    self.world
                        .emit_event("selector.open_scene", request.clone());
                    self.open_scene = Some(request.to_string());
                }
            } else if event.id == self.controls.launch.to_string() {
                let id = self.catalog.levels[self.selected].id.clone();
                self.world
                    .emit_event("selector.launch", serde_json::json!({"level":id}));
                self.launch = Some(id);
            }
        }
        self.world.drain_events(EVENT_CHANNEL);
    }
}
