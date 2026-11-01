//! ECS-owned level selection scene for the browser host.
use crate::{js_error, ui_layout::UiLayout, ui_modifiers};
use countersplat_ecs::{Entity, World};
use countersplat_scene::{AssetLoader, AssetLoadingStatus, LevelCatalog, SceneManifest};
use countersplat_ui::{
    InputEvent, PointerButton, Rect, Style, UiElement, UiEvent, UiEventKind, Widget,
};
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
    name: Entity,
    info: Entity,
    launch: Entity,
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
    rows: Vec<Entity>,
    selected_style: Style,
    unselected_style: Style,
    selected: usize,
    page: usize,
    per_page: usize,
    busy: bool,
    launch: Option<String>,
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
        selected_id: String,
    ) -> Result<Selector, JsValue> {
        let scene = SceneManifest::from_json(&scene_json).map_err(js_error)?;
        let catalog = LevelCatalog::from_json(&catalog_json).map_err(js_error)?;
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
            name: required(&ids, "selected_name")?,
            info: required(&ids, "selected_info")?,
            launch: required(&ids, "launch")?,
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
            rows.push(entity);
        }
        let layout = UiLayout::new(&world, ids);
        let selected = catalog
            .levels
            .iter()
            .position(|level| level.id == selected_id)
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
            rows,
            selected_style,
            unselected_style,
            selected,
            page: 0,
            per_page: 0,
            busy: false,
            launch: None,
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
        let margin = if width < 540.0 { 12.0 } else { 28.0 };
        let footer_h = if width < 540.0 { 76.0 } else { 64.0 };
        let footer_y = (height - margin - footer_h).max(110.0);
        let list_y = 84.0;
        let list_height = (footer_y - list_y - 34.0).max(28.0);
        let list_width = (width - 2.0 * margin).max(40.0);
        self.set_rect(
            self.controls.title,
            Rect::new(margin, 13.0, list_width, 34.0),
        );
        self.set_rect(
            self.controls.subtitle,
            Rect::new(margin, 48.0, list_width, 20.0),
        );
        self.set_rect(
            self.controls.list,
            Rect::new(margin, list_y, list_width, list_height),
        );
        self.set_rect(
            self.controls.detail,
            Rect::new(margin, footer_y, list_width, footer_h),
        );
        let columns = if width >= 540.0 { 2 } else { 1 };
        let rows = (((list_height - 16.0) / 36.0).floor() as usize).clamp(1, 6);
        let per_page = columns * rows;
        if self.per_page == 0 {
            // When returning from gameplay, open the page containing the selected level.
            self.page = self.selected / per_page;
        }
        self.per_page = per_page;
        let pages = self.catalog.levels.len().div_ceil(self.per_page);
        self.page = self.page.min(pages - 1);
        let start = self.page * self.per_page;
        let end = (start + self.per_page).min(self.rows.len());
        let gap = 7.0;
        let row_width =
            ((list_width - 20.0 - gap * (columns - 1) as f32) / columns as f32).max(20.0);
        let row_height =
            ((list_height - 16.0 - gap * (rows - 1) as f32) / rows as f32).clamp(18.0, 42.0);
        for (index, &entity) in self.rows.iter().enumerate() {
            if let Some(element) = self.world.get_mut::<UiElement>(entity) {
                element.visible = (start..end).contains(&index);
                element.enabled = !self.busy;
                if element.visible {
                    let slot = index - start;
                    let col = slot % columns;
                    let row = slot / columns;
                    element.rect = Rect::new(
                        10.0 + col as f32 * (row_width + gap),
                        8.0 + row as f32 * (row_height + gap),
                        row_width,
                        row_height,
                    );
                    element.style = if index == self.selected {
                        self.selected_style.clone()
                    } else {
                        self.unselected_style.clone()
                    };
                    if let Widget::Button { label } = &mut element.widget {
                        *label = short_name(
                            &self.catalog.levels[index].name,
                            (row_width / 7.5).floor().max(8.0) as usize,
                        );
                    }
                }
            }
        }
        let pager_y = footer_y - 29.0;
        self.set_rect(
            self.controls.page,
            Rect::new(margin + 2.0, pager_y, (list_width - 190.0).max(30.0), 22.0),
        );
        self.set_rect(
            self.controls.previous,
            Rect::new(width - margin - 184.0, pager_y, 86.0, 24.0),
        );
        self.set_rect(
            self.controls.next,
            Rect::new(width - margin - 90.0, pager_y, 90.0, 24.0),
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
        let button_width = if width < 540.0 { 120.0 } else { 155.0 };
        self.set_rect(
            self.controls.name,
            Rect::new(
                14.0,
                7.0,
                (list_width - button_width - 34.0).max(20.0),
                23.0,
            ),
        );
        self.set_rect(
            self.controls.info,
            Rect::new(
                14.0,
                33.0,
                (list_width - button_width - 34.0).max(20.0),
                18.0,
            ),
        );
        self.set_rect(
            self.controls.launch,
            Rect::new(
                (list_width - button_width - 12.0).max(0.0),
                (footer_h - 38.0) * 0.5,
                button_width,
                38.0,
            ),
        );
        self.set_rect(
            self.controls.notice,
            Rect::new(margin, height - 17.0, list_width, 16.0),
        );
        let level = &self.catalog.levels[self.selected];
        let max_chars = ((list_width - button_width - 34.0).max(20.0) / 8.0)
            .floor()
            .max(6.0) as usize;
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
            if let Some(index) = self.rows.iter().position(|id| id.to_string() == event.id) {
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
