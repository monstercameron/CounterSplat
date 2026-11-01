//! egui-based wgpu renderer for CounterSplat UI snapshots.
//!
//! The renderer owns its font atlas and transient input state. It draws into a caller-provided
//! color target using a load pass, so the UI is composited over an existing scene. It consumes no
//! ECS types: the world owner produces `UiFrame` and receives owned `UiEvent` values.

use countersplat_ui::{
    Color, Font, InputEvent, Modifiers, PointerButton, Rect, TextAlign, UiEvent, UiEventKind,
    UiFrame, Widget,
};
use egui::{Color32, FontId, Pos2, Stroke, StrokeKind, Vec2};

const MAX_INPUT_EVENTS: usize = 1024;

/// Commands returned by a UI draw. Submit callback buffers before the main encoder, then call
/// [`UiRenderer::after_submit`] to release font/image textures no longer used by egui.
pub struct DrawOutput {
    pub events: Vec<UiEvent>,
    pub command_buffers: Vec<wgpu::CommandBuffer>,
}

/// Resources and viewport for compositing UI onto an existing scene target.
pub struct UiTarget<'a> {
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    pub encoder: &'a mut wgpu::CommandEncoder,
    pub color: &'a wgpu::TextureView,
    pub size: [u32; 2],
    pub pixels_per_point: f32,
    pub time_seconds: f64,
}

/// A reusable UI renderer and input context for one surface/device.
pub struct UiRenderer {
    context: egui::Context,
    renderer: egui_wgpu::Renderer,
    input: Vec<egui::Event>,
    focused: bool,
    modifiers: egui::Modifiers,
    textures_to_free: Vec<egui::TextureId>,
    fonts: std::collections::BTreeSet<String>,
    images: std::collections::BTreeMap<String, egui::TextureHandle>,
    arrange_mode: bool,
}

impl UiRenderer {
    /// Create a renderer for the same color format as the scene target.
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let context = egui::Context::default();
        let mut visuals = egui::Visuals::dark();
        visuals.selection.bg_fill = Color32::from_rgb(173, 119, 67);
        visuals.selection.stroke = Stroke::new(1.0, Color32::from_rgb(232, 188, 119));
        context.set_visuals(visuals);
        Self {
            context,
            renderer: egui_wgpu::Renderer::new(device, format, Default::default()),
            input: Vec::new(),
            focused: true,
            modifiers: egui::Modifiers::default(),
            textures_to_free: Vec::new(),
            fonts: Default::default(),
            images: Default::default(),
            arrange_mode: false,
        }
    }

    /// Install a custom TTF/OTF font under a named family, without removing the default fonts.
    pub fn add_font(&mut self, name: &str, bytes: Vec<u8>) -> Result<(), &'static str> {
        if name.is_empty() || name.len() > 128 || bytes.is_empty() {
            return Err("font name and bytes must be nonempty");
        }
        self.context.add_font(egui::FontInsert {
            name: name.to_owned(),
            data: egui::FontData::from_owned(bytes),
            families: vec![egui::InsertFontFamily {
                family: egui::FontFamily::Name(name.into()),
                priority: egui::FontPriority::Highest,
            }],
        });
        self.fonts.insert(name.to_owned());
        Ok(())
    }

    /// Register decoded RGBA pixels under an opaque UI image key.
    /// Replacing a key releases the previous texture when egui no longer needs it.
    pub fn set_image_rgba(
        &mut self,
        key: &str,
        size: [usize; 2],
        rgba: &[u8],
    ) -> Result<(), &'static str> {
        if key.trim().is_empty()
            || key.len() > 256
            || key.chars().any(char::is_control)
            || size.contains(&0)
            || size[0] > 4096
            || size[1] > 4096
            || size[0].checked_mul(size[1]).and_then(|n| n.checked_mul(4)) != Some(rgba.len())
        {
            return Err("invalid UI image key, dimensions, or RGBA bytes");
        }
        let image = egui::ColorImage::from_rgba_unmultiplied(size, rgba);
        let texture = self
            .context
            .load_texture(key, image, egui::TextureOptions::LINEAR);
        self.images.insert(key.to_owned(), texture);
        Ok(())
    }

    /// Remove a registered image; elements using it display a placeholder.
    pub fn remove_image(&mut self, key: &str) {
        self.images.remove(key);
    }

    /// Queue a platform input event. Pointer moves are coalesced; the queue is bounded.
    ///
    /// On overflow, input is cleared and focus is released to prevent a permanently held button/key.
    /// The host should send a new focus event before accepting more keyboard input.
    pub fn push_input(&mut self, event: InputEvent) -> Result<(), &'static str> {
        let converted = convert_input(event, &mut self.focused, &mut self.modifiers)?;
        let Some(converted) = converted else {
            return Ok(());
        };
        if matches!(converted, egui::Event::PointerMoved(_))
            && matches!(self.input.last(), Some(egui::Event::PointerMoved(_)))
        {
            *self.input.last_mut().expect("last element exists") = converted;
            return Ok(());
        }
        if self.input.len() >= MAX_INPUT_EVENTS {
            self.input.clear();
            self.focused = false;
            self.modifiers = egui::Modifiers::default();
            self.input.push(egui::Event::WindowFocused(false));
            self.input.push(egui::Event::PointerGone);
            return Err("UI input queue is full; focus was released");
        }
        self.input.push(converted);
        Ok(())
    }
    /// Enable arranging all draggable UI elements instead of activating controls.
    pub fn set_arrange_mode(&mut self, enabled: bool) {
        self.arrange_mode = enabled;
    }

    /// Whether UI currently wants keyboard input; use this to route game shortcuts.
    pub fn wants_keyboard(&self) -> bool {
        self.context.egui_wants_keyboard_input()
    }
    /// Whether UI is currently using the pointer; use this to route game input.
    pub fn wants_pointer(&self) -> bool {
        self.context.egui_is_using_pointer()
    }

    /// Draw the UI on top of a color target and return control events for the state owner.
    /// `pixels_per_point` maps logical UI coordinates to the physical target size.
    pub fn draw(
        &mut self,
        target: UiTarget<'_>,
        frame: &UiFrame,
    ) -> Result<DrawOutput, &'static str> {
        let UiTarget {
            device,
            queue,
            encoder,
            color,
            size,
            pixels_per_point,
            time_seconds,
        } = target;
        if size.contains(&0)
            || !pixels_per_point.is_finite()
            || !(0.25..=8.0).contains(&pixels_per_point)
            || !time_seconds.is_finite()
        {
            return Err("invalid UI viewport, scale, or clock");
        }
        let mut raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                Pos2::ZERO,
                Vec2::new(
                    size[0] as f32 / pixels_per_point,
                    size[1] as f32 / pixels_per_point,
                ),
            )),
            time: Some(time_seconds),
            focused: self.focused,
            events: std::mem::take(&mut self.input),
            ..Default::default()
        };
        if let Some(viewport) = raw.viewports.get_mut(&egui::ViewportId::ROOT) {
            viewport.native_pixels_per_point = Some(pixels_per_point);
        }
        let (mut output, events) = build_ui(
            &self.context,
            raw,
            frame,
            &self.fonts,
            &self.images,
            self.arrange_mode,
        );
        for (id, deltas) in &output.textures_delta.set {
            for delta in deltas {
                self.renderer.update_texture(device, queue, *id, delta);
            }
        }
        let paint_jobs = self
            .context
            .tessellate(std::mem::take(&mut output.shapes), output.pixels_per_point);
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: size,
            pixels_per_point: output.pixels_per_point,
        };
        let command_buffers =
            self.renderer
                .update_buffers(device, queue, encoder, &paint_jobs, &screen);
        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("CounterSplat UI layer"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: color,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            let mut pass = pass.forget_lifetime();
            self.renderer.render(&mut pass, &paint_jobs, &screen);
        }
        self.textures_to_free
            .extend(output.textures_delta.free.drain());
        output.textures_delta.clear();
        Ok(DrawOutput {
            events,
            command_buffers,
        })
    }

    /// Release textures after the encoded frame has been submitted.
    pub fn after_submit(&mut self) {
        for id in self.textures_to_free.drain(..) {
            self.renderer.free_texture(&id);
        }
    }
}

fn color(value: Color) -> Color32 {
    Color32::from_rgba_unmultiplied(value.0[0], value.0[1], value.0[2], value.0[3])
}
fn rect(value: Rect) -> egui::Rect {
    egui::Rect::from_min_size(
        Pos2::new(value.x, value.y),
        Vec2::new(value.width, value.height),
    )
}
fn font(value: &Font, size: f32, fonts: &std::collections::BTreeSet<String>) -> FontId {
    let family = match value {
        Font::Proportional => egui::FontFamily::Proportional,
        Font::Monospace => egui::FontFamily::Monospace,
        Font::Named(name) if fonts.contains(name) => egui::FontFamily::Name(name.as_str().into()),
        Font::Named(_) => egui::FontFamily::Proportional,
    };
    FontId::new(size, family)
}
fn mods(value: Modifiers) -> egui::Modifiers {
    egui::Modifiers {
        alt: value.alt,
        ctrl: value.ctrl,
        shift: value.shift,
        mac_cmd: value.command && !value.ctrl,
        command: value.command || value.ctrl,
    }
}
fn convert_input(
    event: InputEvent,
    focused: &mut bool,
    modifiers: &mut egui::Modifiers,
) -> Result<Option<egui::Event>, &'static str> {
    let point = |value: [f32; 2]| {
        if value
            .iter()
            .all(|n| n.is_finite() && n.abs() <= 1_000_000.0)
        {
            Ok(Pos2::new(value[0], value[1]))
        } else {
            Err("invalid pointer coordinate")
        }
    };
    Ok(match event {
        InputEvent::PointerMoved(position) => Some(egui::Event::PointerMoved(point(position)?)),
        InputEvent::PointerButton {
            position,
            button,
            pressed,
            modifiers: event_modifiers,
        } => {
            *modifiers = mods(event_modifiers);
            Some(egui::Event::PointerButton {
                pos: point(position)?,
                button: match button {
                    PointerButton::Primary => egui::PointerButton::Primary,
                    PointerButton::Secondary => egui::PointerButton::Secondary,
                    PointerButton::Middle => egui::PointerButton::Middle,
                },
                pressed,
                modifiers: *modifiers,
            })
        }
        InputEvent::PointerGone => Some(egui::Event::PointerGone),
        InputEvent::Scroll(delta) => {
            let p = point(delta)?;
            Some(egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: Vec2::new(p.x, p.y),
                phase: egui::TouchPhase::Move,
                source: egui::MouseWheelSource::Unknown,
                modifiers: *modifiers,
            })
        }
        InputEvent::Key {
            name,
            pressed,
            repeat,
            modifiers: event_modifiers,
        } => {
            if name.len() > 64 {
                return Err("key name is too long");
            }
            *modifiers = mods(event_modifiers);
            let normalized = match name.as_str() {
                " " => "Space",
                "ArrowLeft" => "ArrowLeft",
                "ArrowRight" => "ArrowRight",
                "ArrowUp" => "ArrowUp",
                "ArrowDown" => "ArrowDown",
                other => other,
            };
            egui::Key::from_name(normalized).map(|key| egui::Event::Key {
                key,
                physical_key: None,
                pressed,
                repeat,
                modifiers: *modifiers,
            })
        }
        InputEvent::Text(text) => {
            if text.len() > 16_384 {
                return Err("input text is too long");
            }
            Some(egui::Event::Text(text))
        }
        InputEvent::Focus(value) => {
            *focused = value;
            if !value {
                *modifiers = egui::Modifiers::default();
            }
            Some(egui::Event::WindowFocused(value))
        }
    })
}

fn push_drag(
    events: &mut Vec<UiEvent>,
    node: &countersplat_ui::ResolvedNode,
    response: &egui::Response,
    handle_height: f32,
) {
    if !response.dragged_by(egui::PointerButton::Primary) {
        return;
    }
    let delta = if response.drag_started() {
        response
            .total_drag_delta()
            .unwrap_or_else(|| response.drag_delta())
    } else {
        response.drag_delta()
    };
    let bounds = node.drag_bounds;
    let visible_width = node.rect.width.min(bounds.width).min(32.0);
    let visible_height = handle_height.min(bounds.height).min(24.0);
    let x = (node.rect.x + delta.x).clamp(
        bounds.x - node.rect.width + visible_width,
        bounds.x + bounds.width - visible_width,
    );
    let y = (node.rect.y + delta.y).clamp(bounds.y, bounds.y + bounds.height - visible_height);
    // A resized viewport may have clamped the drawn position without modifying the ECS.
    // Reconcile that difference on the next drag so the item follows the pointer immediately.
    let movement = [
        x - node.rect.x + node.drag_correction[0],
        y - node.rect.y + node.drag_correction[1],
    ];
    if movement != [0.0, 0.0] && movement.iter().all(|value| value.is_finite()) {
        events.push(UiEvent {
            id: node.id.clone(),
            kind: UiEventKind::MovedBy(movement),
        });
    }
}

fn build_ui(
    context: &egui::Context,
    raw: egui::RawInput,
    frame: &UiFrame,
    fonts: &std::collections::BTreeSet<String>,
    images: &std::collections::BTreeMap<String, egui::TextureHandle>,
    arrange_mode: bool,
) -> (egui::FullOutput, Vec<UiEvent>) {
    let mut events = Vec::new();
    let output = context.run_ui(raw, |ui| {
        events.clear();
        for node in &frame.nodes {
            if !node.rect.valid()
                || !node.clip.valid()
                || !node.drag_bounds.valid()
                || node.element.validate().is_err()
            {
                continue;
            }
            let area = rect(node.rect);
            let clip = rect(node.clip);
            let style = &node.element.style;
            let painter = ui.painter().with_clip_rect(clip);
            let stroke = Stroke::new(1.0, color(style.border));
            let rounding = style.rounding.round() as u8;
            match &node.element.widget {
                Widget::Panel | Widget::Rectangle => {
                    painter.rect(
                        area,
                        rounding,
                        color(style.fill),
                        stroke,
                        StrokeKind::Inside,
                    );
                    if let Some(panel) = &node.element.collapsible {
                        let header = egui::Rect::from_min_max(
                            area.min,
                            Pos2::new(area.right(), area.top() + panel.header_height),
                        );
                        let response = (!arrange_mode).then(|| {
                            ui.push_id(&node.id, |ui| {
                                ui.set_clip_rect(clip);
                                ui.add_enabled_ui(node.enabled, |ui| {
                                    let id = ui.make_persistent_id("panel-collapse");
                                    let sense = if node.element.draggable {
                                        egui::Sense::click_and_drag()
                                    } else {
                                        egui::Sense::click()
                                    };
                                    ui.interact(header, id, sense)
                                })
                                .inner
                            })
                            .inner
                        });
                        if response.as_ref().is_some_and(|r| r.hovered()) {
                            painter.rect_filled(
                                header.shrink(1.0),
                                rounding,
                                Color32::from_white_alpha(12),
                            );
                        }
                        painter.text(
                            Pos2::new(header.left() + 12.0, header.center().y),
                            egui::Align2::LEFT_CENTER,
                            &panel.title,
                            font(
                                &style.font,
                                style.font_size.min(panel.header_height * 0.6),
                                fonts,
                            ),
                            color(style.text),
                        );
                        let center = Pos2::new(header.right() - 17.0, header.center().y);
                        let points = if panel.collapsed {
                            vec![
                                center + Vec2::new(-3.0, -5.0),
                                center + Vec2::new(4.0, 0.0),
                                center + Vec2::new(-3.0, 5.0),
                            ]
                        } else {
                            vec![
                                center + Vec2::new(-5.0, -3.0),
                                center + Vec2::new(5.0, -3.0),
                                center + Vec2::new(0.0, 4.0),
                            ]
                        };
                        painter.add(egui::Shape::convex_polygon(
                            points,
                            color(style.accent),
                            Stroke::NONE,
                        ));
                        if !panel.collapsed {
                            painter.line_segment(
                                [header.left_bottom(), header.right_bottom()],
                                stroke,
                            );
                        }
                        if let Some(response) = response {
                            if node.element.draggable {
                                push_drag(&mut events, node, &response, panel.header_height);
                            }
                            if response.clicked() {
                                events.push(UiEvent {
                                    id: node.id.clone(),
                                    kind: UiEventKind::CollapsedChanged(!panel.collapsed),
                                });
                            }
                        }
                    }
                }
                Widget::Image { source } => {
                    painter.rect_filled(area, rounding, color(style.fill));
                    if let Some(texture) = images.get(source) {
                        let size = texture.size_vec2();
                        let image_ratio = size.x / size.y;
                        let area_ratio = area.width() / area.height();
                        let (u, v) = if image_ratio > area_ratio {
                            (area_ratio / image_ratio, 1.0)
                        } else {
                            (1.0, image_ratio / area_ratio)
                        };
                        let uv = egui::Rect::from_center_size(Pos2::new(0.5, 0.5), Vec2::new(u, v));
                        painter.image(texture.id(), area, uv, Color32::WHITE);
                    } else {
                        painter.text(
                            area.center(),
                            egui::Align2::CENTER_CENTER,
                            "Preview unavailable",
                            font(&style.font, style.font_size, fonts),
                            color(style.text),
                        );
                    }
                    painter.rect_stroke(area, rounding, stroke, StrokeKind::Inside);
                }
                Widget::Circle => {
                    painter.circle(
                        area.center(),
                        area.width().min(area.height()) * 0.5,
                        color(style.fill),
                        stroke,
                    );
                }
                Widget::Line { end, thickness } => {
                    painter.line_segment(
                        [area.min, area.min + Vec2::new(end[0], end[1])],
                        Stroke::new(*thickness, color(style.fill)),
                    );
                }
                Widget::LineGraph {
                    points,
                    x_min,
                    x_max,
                    y_min,
                    y_max,
                } => {
                    painter.rect(
                        area,
                        rounding,
                        color(style.fill),
                        stroke,
                        StrokeKind::Inside,
                    );
                    let grid = Stroke::new(1.0, color(style.border));
                    for fraction in [0.25, 0.5, 0.75] {
                        let y = area.top() + area.height() * fraction;
                        painter.line_segment(
                            [Pos2::new(area.left(), y), Pos2::new(area.right(), y)],
                            grid,
                        );
                    }
                    let positions: Vec<Pos2> = points
                        .iter()
                        .map(|point| {
                            let x = ((point[0] - x_min) / (x_max - x_min)).clamp(0.0, 1.0);
                            let y = ((point[1] - y_min) / (y_max - y_min)).clamp(0.0, 1.0);
                            Pos2::new(
                                area.left() + x * area.width(),
                                area.bottom() - y * area.height(),
                            )
                        })
                        .collect();
                    if positions.len() == 1 {
                        painter.circle_filled(positions[0], 2.0, color(style.accent));
                    }
                    if positions.len() > 1 {
                        painter.add(egui::Shape::line(
                            positions,
                            Stroke::new(1.5, color(style.accent)),
                        ));
                    }
                }
                Widget::Text { text, align } => {
                    let (position, anchor) = match align {
                        TextAlign::Left => (area.left_center(), egui::Align2::LEFT_CENTER),
                        TextAlign::Center => (area.center(), egui::Align2::CENTER_CENTER),
                        TextAlign::Right => (area.right_center(), egui::Align2::RIGHT_CENTER),
                    };
                    painter.text(
                        position,
                        anchor,
                        text,
                        font(&style.font, style.font_size, fonts),
                        color(style.text),
                    );
                }
                _ => {
                    ui.push_id(&node.id, |ui| {
                        ui.set_clip_rect(clip);
                        ui.add_enabled_ui(node.enabled && !arrange_mode, |ui| {
                            let kind = match &node.element.widget {
                                Widget::Button { label } => {
                                    let text = egui::RichText::new(label)
                                        .font(font(&style.font, style.font_size, fonts))
                                        .color(color(style.text));
                                    let response = ui.put(
                                        area,
                                        egui::Button::new(text)
                                            .fill(color(style.fill))
                                            .stroke(stroke)
                                            .corner_radius(rounding),
                                    );
                                    response.clicked().then_some(UiEventKind::Clicked)
                                }
                                Widget::Checkbox { label, checked } => {
                                    let mut value = *checked;
                                    let text = egui::RichText::new(label)
                                        .font(font(&style.font, style.font_size, fonts))
                                        .color(color(style.text));
                                    ui.put(area, egui::Checkbox::new(&mut value, text))
                                        .changed()
                                        .then_some(UiEventKind::BoolChanged(value))
                                }
                                Widget::Slider {
                                    label,
                                    value,
                                    min,
                                    max,
                                } => {
                                    let mut next = *value;
                                    ui.style_mut().visuals.selection.bg_fill = color(style.accent);
                                    let slider =
                                        egui::Slider::new(&mut next, *min..=*max).text(label);
                                    ui.put(area, slider)
                                        .changed()
                                        .then_some(UiEventKind::NumberChanged(next))
                                }
                                Widget::TextInput { text, max_chars } => {
                                    let mut next = text.clone();
                                    let input = egui::TextEdit::singleline(&mut next)
                                        .font(font(&style.font, style.font_size, fonts))
                                        .char_limit(*max_chars);
                                    ui.put(area, input)
                                        .changed()
                                        .then_some(UiEventKind::TextChanged(next))
                                }
                                Widget::Progress { value } => {
                                    ui.put(
                                        area,
                                        egui::ProgressBar::new(*value).fill(color(style.accent)),
                                    );
                                    None
                                }
                                _ => None,
                            };
                            if let Some(kind) = kind {
                                events.push(UiEvent {
                                    id: node.id.clone(),
                                    kind,
                                });
                            }
                        });
                    });
                }
            }
            if arrange_mode && node.enabled && node.element.draggable {
                let response = ui
                    .push_id(&node.id, |ui| {
                        ui.set_clip_rect(clip);
                        let id = ui.make_persistent_id("arrange-drag");
                        ui.interact(area, id, egui::Sense::drag())
                    })
                    .inner;
                let outline = if response.hovered() || response.dragged() {
                    Color32::from_rgb(239, 192, 112)
                } else {
                    Color32::from_rgba_unmultiplied(133, 207, 202, 110)
                };
                painter.rect_stroke(
                    area.shrink(1.0),
                    rounding,
                    Stroke::new(1.0, outline),
                    StrokeKind::Inside,
                );
                push_drag(&mut events, node, &response, area.height().min(24.0));
            }
        }
    });
    (output, events)
}

#[cfg(test)]
mod tests {
    use super::*;
    use countersplat_ui::{ResolvedNode, UiElement};
    fn frame() -> UiFrame {
        let rect = Rect::new(10.0, 10.0, 120.0, 30.0);
        UiFrame {
            nodes: vec![ResolvedNode {
                id: "button".into(),
                rect,
                clip: rect,
                drag_bounds: Rect::new(0.0, 0.0, 400.0, 300.0),
                drag_correction: [0.0, 0.0],
                z: 0,
                enabled: true,
                element: UiElement {
                    rect,
                    widget: Widget::Button {
                        label: "Click".into(),
                    },
                    ..Default::default()
                },
            }],
        }
    }
    fn run(context: &egui::Context, events: Vec<egui::Event>, t: f64) -> Vec<UiEvent> {
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                Pos2::ZERO,
                Vec2::new(400.0, 300.0),
            )),
            events,
            time: Some(t),
            ..Default::default()
        };
        let (output, events) = build_ui(
            context,
            raw,
            &frame(),
            &Default::default(),
            &Default::default(),
            false,
        );
        output.drop_without_applying_deltas();
        events
    }
    #[test]
    fn pointer_click_produces_an_owned_ui_event() {
        let context = egui::Context::default();
        run(&context, vec![], 0.0);
        let pos = Pos2::new(35.0, 20.0);
        let button = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        };
        assert!(run(
            &context,
            vec![egui::Event::PointerMoved(pos), button(true)],
            0.1
        )
        .is_empty());
        assert_eq!(
            run(&context, vec![button(false)], 0.2),
            vec![UiEvent {
                id: "button".into(),
                kind: UiEventKind::Clicked
            }]
        );
    }
    #[test]
    fn panel_header_emits_collapse_and_expand_events() {
        use countersplat_ui::CollapsiblePanel;
        let context = egui::Context::default();
        let expanded = Rect::new(10.0, 10.0, 120.0, 100.0);
        let mut frame = UiFrame {
            nodes: vec![ResolvedNode {
                id: "panel".into(),
                rect: expanded,
                clip: expanded,
                drag_bounds: Rect::new(0.0, 0.0, 400.0, 300.0),
                drag_correction: [0.0, 0.0],
                z: 0,
                enabled: true,
                element: UiElement {
                    rect: expanded,
                    collapsible: Some(CollapsiblePanel {
                        title: "Settings".into(),
                        header_height: 30.0,
                        collapsed: false,
                    }),
                    ..Default::default()
                },
            }],
        };
        let run_panel = |context: &egui::Context, frame: &UiFrame, input, time| {
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    Pos2::ZERO,
                    Vec2::new(400.0, 300.0),
                )),
                events: input,
                time: Some(time),
                ..Default::default()
            };
            let (output, events) = build_ui(
                context,
                raw,
                frame,
                &Default::default(),
                &Default::default(),
                false,
            );
            output.drop_without_applying_deltas();
            events
        };
        let pos = Pos2::new(30.0, 25.0);
        let button = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        };
        run_panel(&context, &frame, vec![], 0.0);
        assert!(run_panel(
            &context,
            &frame,
            vec![egui::Event::PointerMoved(pos), button(true)],
            0.1
        )
        .is_empty());
        assert_eq!(
            run_panel(&context, &frame, vec![button(false)], 0.2),
            vec![UiEvent {
                id: "panel".into(),
                kind: UiEventKind::CollapsedChanged(true)
            }]
        );
        frame.nodes[0]
            .element
            .collapsible
            .as_mut()
            .unwrap()
            .collapsed = true;
        frame.nodes[0].rect.height = 30.0;
        frame.nodes[0].clip.height = 30.0;
        assert!(run_panel(&context, &frame, vec![button(true)], 0.3).is_empty());
        assert_eq!(
            run_panel(&context, &frame, vec![button(false)], 0.4),
            vec![UiEvent {
                id: "panel".into(),
                kind: UiEventKind::CollapsedChanged(false)
            }]
        );
        frame.nodes[0].enabled = false;
        assert!(run_panel(&context, &frame, vec![button(true)], 0.5).is_empty());
        assert!(run_panel(&context, &frame, vec![button(false)], 0.6).is_empty());
    }

    fn test_frame(
        context: &egui::Context,
        frame: &UiFrame,
        input: Vec<egui::Event>,
        time: f64,
        arrange: bool,
    ) -> Vec<UiEvent> {
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                Pos2::ZERO,
                Vec2::new(400.0, 300.0),
            )),
            events: input,
            time: Some(time),
            ..Default::default()
        };
        let (output, events) = build_ui(
            context,
            raw,
            frame,
            &Default::default(),
            &Default::default(),
            arrange,
        );
        output.drop_without_applying_deltas();
        events
    }
    fn pointer(pos: Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        }
    }

    #[test]
    fn dragging_a_panel_header_moves_it_without_collapsing() {
        use countersplat_ui::CollapsiblePanel;
        let context = egui::Context::default();
        let area = Rect::new(10.0, 10.0, 120.0, 100.0);
        let mut frame = UiFrame {
            nodes: vec![ResolvedNode {
                id: "panel".into(),
                rect: area,
                clip: area,
                drag_bounds: Rect::new(0.0, 0.0, 400.0, 300.0),
                drag_correction: [0.0, 0.0],
                z: 0,
                enabled: true,
                element: UiElement {
                    rect: area,
                    collapsible: Some(CollapsiblePanel {
                        title: "Settings".into(),
                        header_height: 30.0,
                        collapsed: false,
                    }),
                    ..Default::default()
                },
            }],
        };
        let start = Pos2::new(30.0, 25.0);
        let end = Pos2::new(70.0, 55.0);
        test_frame(&context, &frame, vec![], 0.0, false);
        test_frame(
            &context,
            &frame,
            vec![egui::Event::PointerMoved(start), pointer(start, true)],
            0.1,
            false,
        );
        let events = test_frame(
            &context,
            &frame,
            vec![egui::Event::PointerMoved(end)],
            0.2,
            false,
        );
        assert_eq!(
            events,
            vec![UiEvent {
                id: "panel".into(),
                kind: UiEventKind::MovedBy([40.0, 30.0])
            }]
        );
        frame.nodes[0].rect.x += 40.0;
        frame.nodes[0].rect.y += 30.0;
        frame.nodes[0].clip = frame.nodes[0].rect;
        assert!(test_frame(&context, &frame, vec![pointer(end, false)], 0.3, false).is_empty());
    }

    #[test]
    fn dragging_reconciles_a_position_clamped_after_resize() {
        let context = egui::Context::default();
        let mut frame = frame();
        frame.nodes[0].drag_correction = [-100.0, -20.0];
        let start = Pos2::new(35.0, 20.0);
        let end = Pos2::new(85.0, 45.0);
        test_frame(&context, &frame, vec![], 0.0, true);
        test_frame(
            &context,
            &frame,
            vec![egui::Event::PointerMoved(start), pointer(start, true)],
            0.1,
            true,
        );
        assert_eq!(
            test_frame(
                &context,
                &frame,
                vec![egui::Event::PointerMoved(end)],
                0.2,
                true
            ),
            vec![UiEvent {
                id: "button".into(),
                kind: UiEventKind::MovedBy([-50.0, 5.0])
            }]
        );
    }

    #[test]
    fn arrange_mode_moves_a_button_without_activating_it() {
        let context = egui::Context::default();
        let mut frame = frame();
        let start = Pos2::new(35.0, 20.0);
        let end = Pos2::new(85.0, 45.0);
        test_frame(&context, &frame, vec![], 0.0, true);
        test_frame(
            &context,
            &frame,
            vec![egui::Event::PointerMoved(start), pointer(start, true)],
            0.1,
            true,
        );
        let events = test_frame(
            &context,
            &frame,
            vec![egui::Event::PointerMoved(end)],
            0.2,
            true,
        );
        assert_eq!(
            events,
            vec![UiEvent {
                id: "button".into(),
                kind: UiEventKind::MovedBy([50.0, 25.0])
            }]
        );
        frame.nodes[0].rect.x += 50.0;
        frame.nodes[0].rect.y += 25.0;
        frame.nodes[0].clip = frame.nodes[0].rect;
        assert!(test_frame(&context, &frame, vec![pointer(end, false)], 0.3, true).is_empty());
        test_frame(&context, &frame, vec![], 0.4, false);
        test_frame(&context, &frame, vec![pointer(end, true)], 0.5, false);
        assert_eq!(
            test_frame(&context, &frame, vec![pointer(end, false)], 0.6, false),
            vec![UiEvent {
                id: "button".into(),
                kind: UiEventKind::Clicked
            }]
        );
    }
}
