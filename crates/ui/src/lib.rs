//! Renderer-independent UI data for CounterSplat.
//!
//! Elements are owned values: a game can keep them in an ECS, send resolved frames to a render
//! worker, and send [`UiEvent`] values back. This crate has no ECS, browser, GPU, or Lua dependency.
//! Coordinates are logical points with a top-left origin; the host supplies pixels-per-point.

mod timing;
pub use timing::{FrameCounter, FrameHistory, FrameMeasurement, FrameMeter, FrameUpdate};

use serde::{Deserialize, Serialize};

/// An RGBA color in sRGB byte values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Color(pub [u8; 4]);
impl Color {
    pub const TRANSPARENT: Self = Self([0, 0, 0, 0]);
    pub const WHITE: Self = Self([255, 255, 255, 255]);
}

/// A rectangle in logical points.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}
impl Rect {
    pub fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
    /// Intersection; empty or non-finite rectangles do not produce visible content.
    pub fn intersection(self, other: Self) -> Option<Self> {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let right = (self.x + self.width).min(other.x + other.width);
        let bottom = (self.y + self.height).min(other.y + other.height);
        (right > x && bottom > y).then_some(Self::new(x, y, right - x, bottom - y))
    }
    pub fn valid(self) -> bool {
        [self.x, self.y, self.width, self.height]
            .iter()
            .all(|v| v.is_finite() && v.abs() <= 1_000_000.0)
            && self.width > 0.0
            && self.height > 0.0
    }
}
impl Default for Rect {
    fn default() -> Self {
        Self::new(0.0, 0.0, 100.0, 32.0)
    }
}

/// Which corner or center of a parent rectangle positions an element.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Anchor {
    #[default]
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    Center,
}

/// Position an element inside its parent. Positive offsets are insets for right/bottom anchors.
pub fn place(parent: Rect, local: Rect, anchor: Anchor) -> Rect {
    let (x, y) = match anchor {
        Anchor::TopLeft => (parent.x + local.x, parent.y + local.y),
        Anchor::TopRight => (
            parent.x + parent.width - local.width - local.x,
            parent.y + local.y,
        ),
        Anchor::BottomLeft => (
            parent.x + local.x,
            parent.y + parent.height - local.height - local.y,
        ),
        Anchor::BottomRight => (
            parent.x + parent.width - local.width - local.x,
            parent.y + parent.height - local.height - local.y,
        ),
        Anchor::Center => (
            parent.x + (parent.width - local.width) * 0.5 + local.x,
            parent.y + (parent.height - local.height) * 0.5 + local.y,
        ),
    };
    Rect::new(x, y, local.width, local.height)
}

/// Built-in font families or a family registered with the UI renderer.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Font {
    #[default]
    Proportional,
    Monospace,
    Named(String),
}
/// Horizontal alignment of text inside its rectangle.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextAlign {
    #[default]
    Left,
    Center,
    Right,
}

/// Shared colors and font size. Per-game themes can supply their own values.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Style {
    pub fill: Color,
    pub text: Color,
    pub border: Color,
    pub accent: Color,
    pub rounding: f32,
    pub font_size: f32,
    pub font: Font,
}
impl Default for Style {
    fn default() -> Self {
        Self {
            fill: Color([21, 39, 52, 235]),
            text: Color([238, 245, 241, 255]),
            border: Color([99, 139, 145, 180]),
            accent: Color([232, 188, 119, 255]),
            rounding: 6.0,
            font_size: 15.0,
            font: Font::Proportional,
        }
    }
}

/// Data for built-in shapes and controls. Game behavior belongs in event consumers.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Widget {
    #[default]
    Panel,
    Rectangle,
    Circle,
    Line {
        end: [f32; 2],
        thickness: f32,
    },
    /// A bounded, normalized-to-range line chart. Points are ordered left to right.
    LineGraph {
        points: Vec<[f32; 2]>,
        x_min: f32,
        x_max: f32,
        y_min: f32,
        y_max: f32,
    },
    Text {
        text: String,
        align: TextAlign,
    },
    Button {
        label: String,
    },
    Checkbox {
        label: String,
        checked: bool,
    },
    Slider {
        label: String,
        value: f32,
        min: f32,
        max: f32,
    },
    TextInput {
        text: String,
        max_chars: usize,
    },
    Progress {
        value: f32,
    },
}

/// Optional panel header and ECS-owned collapse state.
///
/// A collapsed panel keeps only its header visible; descendants retain their state and reappear
/// when expanded. Child coordinates are relative to the content area below the header.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CollapsiblePanel {
    pub title: String,
    pub collapsed: bool,
    /// Header height in logical points, smaller than the panel's expanded height.
    pub header_height: f32,
}
impl Default for CollapsiblePanel {
    fn default() -> Self {
        Self {
            title: "Panel".into(),
            collapsed: false,
            header_height: 24.0,
        }
    }
}

/// One layout element. Child rectangles are relative to their UI parent's rectangle.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiElement {
    pub rect: Rect,
    pub anchor: Anchor,
    /// Larger values draw later and receive pointer input above lower values.
    pub z: i32,
    pub visible: bool,
    pub enabled: bool,
    pub clip_children: bool,
    /// Whether users may reposition this element in an arranging UI.
    pub draggable: bool,
    /// User translation in logical screen points, applied after anchor-based layout.
    pub drag_offset: [f32; 2],
    /// Optional, interactive header for `Widget::Panel`; absent panels behave as before.
    pub collapsible: Option<CollapsiblePanel>,
    pub style: Style,
    pub widget: Widget,
}
impl Default for UiElement {
    fn default() -> Self {
        Self {
            rect: Rect::default(),
            anchor: Anchor::TopLeft,
            z: 0,
            visible: true,
            enabled: true,
            clip_children: true,
            draggable: true,
            drag_offset: [0.0, 0.0],
            collapsible: None,
            style: Style::default(),
            widget: Widget::Panel,
        }
    }
}
impl UiElement {
    /// Check untrusted or script-provided values before storing or drawing them.
    pub fn validate(&self) -> Result<(), &'static str> {
        if !self.rect.valid() {
            return Err("UI rectangle must have finite coordinates and positive size");
        }
        if !self
            .drag_offset
            .iter()
            .all(|v| v.is_finite() && v.abs() <= 1_000_000.0)
        {
            return Err("UI drag offset must be finite and within one million points");
        }
        if !self.style.rounding.is_finite()
            || !(0.0..=255.0).contains(&self.style.rounding)
            || !self.style.font_size.is_finite()
            || !(1.0..=256.0).contains(&self.style.font_size)
        {
            return Err("UI rounding or font size is invalid");
        }
        if let Some(panel) = &self.collapsible {
            if !matches!(self.widget, Widget::Panel)
                || panel.title.trim().is_empty()
                || panel.title.len() > 512
                || panel.title.chars().any(char::is_control)
                || !panel.header_height.is_finite()
                || !(18.0..=128.0).contains(&panel.header_height)
                || panel.header_height >= self.rect.height
            {
                return Err("collapsible panel needs a title and a valid header smaller than its expanded height");
            }
        }
        let text_valid = |s: &str| s.len() <= 16_384;
        match &self.widget {
            Widget::Text { text, .. } if !text_valid(text) => Err("UI text is too long"),
            Widget::Button { label }
            | Widget::Checkbox { label, .. }
            | Widget::Slider { label, .. }
                if !text_valid(label) =>
            {
                Err("UI label is too long")
            }
            Widget::Line { end, thickness }
                if !end.iter().all(|v| v.is_finite())
                    || !thickness.is_finite()
                    || *thickness <= 0.0
                    || *thickness > 256.0 =>
            {
                Err("UI line is invalid")
            }
            Widget::LineGraph {
                points,
                x_min,
                x_max,
                y_min,
                y_max,
            } if points.len() > 2048
                || ![x_min, x_max, y_min, y_max]
                    .iter()
                    .all(|v| v.is_finite() && v.abs() <= 1_000_000.0)
                || x_min >= x_max
                || y_min >= y_max
                || !points
                    .iter()
                    .flatten()
                    .all(|v| v.is_finite() && v.abs() <= 1_000_000.0) =>
            {
                Err("UI line graph has invalid points or ranges")
            }
            Widget::Slider {
                value, min, max, ..
            } if !value.is_finite()
                || !min.is_finite()
                || !max.is_finite()
                || min >= max
                || value < min
                || value > max =>
            {
                Err("UI slider range or value is invalid")
            }
            Widget::Progress { value } if !value.is_finite() || !(0.0..=1.0).contains(value) => {
                Err("UI progress must be between zero and one")
            }
            Widget::TextInput { text, max_chars }
                if *max_chars == 0 || *max_chars > 16_384 || text.chars().count() > *max_chars =>
            {
                Err("UI text input exceeds its limit")
            }
            _ => Ok(()),
        }
    }
}

/// A resolved node ready to send to a rendering worker. IDs are opaque to this crate.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedNode {
    pub id: String,
    pub rect: Rect,
    pub clip: Rect,
    /// Visible parent bounds, used to keep dragged elements reachable.
    pub drag_bounds: Rect,
    /// Adjustment from the authored position to the visible position after a resize.
    /// Include this when starting a drag so the stored offset catches up to the displayed position.
    pub drag_correction: [f32; 2],
    pub z: i32,
    pub enabled: bool,
    pub element: UiElement,
}
/// A snapshot of the UI layer for one viewport.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UiFrame {
    pub nodes: Vec<ResolvedNode>,
}

/// A control event sent from the UI renderer back to the owner of the UI state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiEvent {
    pub id: String,
    pub kind: UiEventKind,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum UiEventKind {
    Clicked,
    /// The requested collapsed state of a panel header.
    CollapsedChanged(bool),
    /// Incremental movement in logical screen points; independent of the element anchor.
    MovedBy([f32; 2]),
    BoolChanged(bool),
    NumberChanged(f32),
    TextChanged(String),
}

/// Pointer/keyboard modifiers at the time an event was generated.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Modifiers {
    pub alt: bool,
    pub ctrl: bool,
    pub shift: bool,
    pub command: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerButton {
    Primary,
    Secondary,
    Middle,
}
/// Platform-neutral input in logical points. The host handles coordinate conversion and focus.
#[derive(Debug, Clone, PartialEq)]
pub enum InputEvent {
    PointerMoved([f32; 2]),
    PointerButton {
        position: [f32; 2],
        button: PointerButton,
        pressed: bool,
        modifiers: Modifiers,
    },
    PointerGone,
    Scroll([f32; 2]),
    Key {
        name: String,
        pressed: bool,
        repeat: bool,
        modifiers: Modifiers,
    },
    Text(String),
    Focus(bool),
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn anchors_and_clipping_are_relative_to_parent() {
        let parent = Rect::new(10.0, 20.0, 300.0, 200.0);
        assert_eq!(
            place(parent, Rect::new(5.0, 6.0, 50.0, 30.0), Anchor::BottomRight),
            Rect::new(255.0, 184.0, 50.0, 30.0)
        );
        assert_eq!(
            parent.intersection(Rect::new(300.0, 200.0, 20.0, 40.0)),
            Some(Rect::new(300.0, 200.0, 10.0, 20.0))
        );
        assert!(parent
            .intersection(Rect::new(500.0, 0.0, 10.0, 10.0))
            .is_none());
    }
    #[test]
    fn rejects_invalid_geometry_and_control_values() {
        let mut node = UiElement::default();
        node.rect.width = f32::NAN;
        assert!(node.validate().is_err());
        node.rect.width = 100.0;
        node.widget = Widget::Slider {
            label: "value".into(),
            value: 3.0,
            min: 0.0,
            max: 1.0,
        };
        assert!(node.validate().is_err());
    }
    #[test]
    fn collapsible_panel_configuration_is_validated() {
        let default_panel = UiElement {
            collapsible: Some(CollapsiblePanel::default()),
            ..Default::default()
        };
        assert!(default_panel.validate().is_ok());
        let mut node = UiElement {
            rect: Rect::new(0.0, 0.0, 200.0, 100.0),
            collapsible: Some(CollapsiblePanel {
                title: "Settings".into(),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert!(node.validate().is_ok());
        node.collapsible.as_mut().unwrap().header_height = 100.0;
        assert!(node.validate().is_err());
        node.collapsible.as_mut().unwrap().header_height = f32::NAN;
        assert!(node.validate().is_err());
        node.collapsible.as_mut().unwrap().header_height = 32.0;
        node.widget = Widget::Rectangle;
        assert!(node.validate().is_err());
    }
}
