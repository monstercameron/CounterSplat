//! Display resolution and presentation policy: the scene's render resolution against the window, aspect-ratio
//! presets, pixel-perfect and fill/fit presentation, and the persisted display and HUD preferences. Independent of
//! the browser and renderer; the web host persists the JSON and applies the results.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Largest scene a user may choose, in pixels.
pub const MAX_RENDER_PIXELS: f64 = 16_777_216.0;
/// Largest render dimension before the device's own limit.
pub const MAX_RENDER_DIMENSION: u32 = 8192;
/// A viewport-sized scene is scaled down to this many pixels, keeping its aspect (about 1790x1010 at 16:9). The
/// frame costs roughly in proportion to pixels: about 9 ms a megapixel in the browser on the development GPU, and
/// the splat draw alone measured 3.4 ms static at 720p, 4.6 at 1080p, 8.2 at 1440p and 18.4 at 4K on an
/// 8.9-million-splat scene. 1.8 megapixels keeps that GPU near 60 frames a second. The UI is drawn at full viewport
/// resolution regardless.
pub const MAX_VIEWPORT_SCENE_PIXELS: f64 = 1_800_000.0;

/// Aspect presets (`id`, `label`). The 21:9 family uses common marketed ultrawide sizes, which are not all exactly
/// one ratio (3440x1440 and 2560x1080).
pub const ASPECT_RATIOS: [&str; 4] = ["16:9", "16:10", "21:9", "custom"];

const CUSTOM_HEIGHTS: [u32; 19] = [
    180, 270, 360, 432, 480, 540, 600, 720, 800, 900, 1080, 1200, 1440, 1600, 1800, 2160, 2400, 2880, 3200,
];

fn common_resolutions(aspect: &str) -> &'static [(u32, u32)] {
    match aspect {
        "16:9" => &[(640, 360), (960, 540), (1280, 720), (1600, 900), (1920, 1080), (2560, 1440), (3200, 1800), (3840, 2160), (5120, 2880)],
        "16:10" => &[(640, 400), (960, 600), (1280, 800), (1440, 900), (1680, 1050), (1920, 1200), (2560, 1600), (2880, 1800), (3840, 2400)],
        "21:9" => &[(1280, 540), (1920, 810), (2560, 1080), (3440, 1440), (3840, 1600), (5120, 2160)],
        _ => &[],
    }
}

fn valid_aspect(id: &str) -> bool {
    ASPECT_RATIOS.contains(&id)
}

/// A width and height in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Size {
    pub width: u32,
    pub height: u32,
}

/// The persisted display preferences (`mode` fixed or viewport, `scaling` fill, fit or pixel). A custom aspect keeps
/// its reference ratio separately, so stepping through rounded resolutions cannot drift it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DisplayPreferences {
    pub mode: String,
    pub width: u32,
    pub height: u32,
    pub scaling: String,
    pub aspect: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ratio_width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ratio_height: Option<u32>,
}

impl Default for DisplayPreferences {
    /// The scene follows the window's own pixels by default: a fixed 1280x720 scene would be scaled up to fill a
    /// larger or high-DPI canvas, which softens the picture. Width and height are placeholders in viewport mode.
    fn default() -> Self {
        Self {
            mode: "viewport".into(),
            width: 1280,
            height: 720,
            scaling: "fill".into(),
            aspect: "16:9".into(),
            ratio_width: None,
            ratio_height: None,
        }
    }
}

fn valid_ratio(width: Option<u32>, height: Option<u32>) -> bool {
    matches!((width, height), (Some(w), Some(h)) if w > 0 && h > 0 && w <= MAX_RENDER_DIMENSION && h <= MAX_RENDER_DIMENSION)
}

/// Whether `width`x`height` belongs to the aspect preset `id` (any size is custom).
pub fn aspect_matches(id: &str, width: f64, height: f64) -> bool {
    if !(width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0) {
        return false;
    }
    let target = match id {
        "custom" => return true,
        "16:9" => 16.0 / 9.0,
        "16:10" => 16.0 / 10.0,
        "21:9" => 43.0 / 18.0,
        _ => return false,
    };
    let tolerance = if id == "21:9" { 0.035 } else { 0.006 };
    ((width / height) / target - 1.0).abs() <= tolerance
}

/// The preset a size belongs to, or `custom`.
pub fn infer_aspect(width: f64, height: f64) -> &'static str {
    ASPECT_RATIOS
        .iter()
        .copied()
        .find(|&id| id != "custom" && aspect_matches(id, width, height))
        .unwrap_or("custom")
}

/// Why a resolution cannot be used, or `None`. `device_limit` is the GPU's largest texture side.
pub fn validate_resolution(width: f64, height: f64, device_limit: f64) -> Option<String> {
    let max = if device_limit.is_finite() {
        f64::from(MAX_RENDER_DIMENSION).min(device_limit.floor())
    } else {
        f64::from(MAX_RENDER_DIMENSION)
    };
    if width.fract() != 0.0 || height.fract() != 0.0 || !(width >= 64.0) || !(height >= 64.0) {
        return Some("Width and height must be whole numbers of at least 64 pixels.".into());
    }
    if width > max || height > max {
        return Some(format!("The maximum dimension on this device is {max} pixels."));
    }
    if width * height > MAX_RENDER_PIXELS {
        return Some("The resolution exceeds the 16.8 megapixel render limit.".into());
    }
    None
}

fn number(value: &Value, key: &str) -> f64 {
    value.get(key).and_then(Value::as_f64).unwrap_or(f64::NAN)
}

fn text<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

fn whole(value: &Value, key: &str) -> Option<u32> {
    value.get(key).and_then(Value::as_u64).and_then(|v| u32::try_from(v).ok())
}

/// Stored preferences, repaired: anything invalid becomes the default.
pub fn normalize(value: &Value) -> DisplayPreferences {
    let mode = text(value, "mode").filter(|m| matches!(*m, "fixed" | "viewport"));
    let scaling = text(value, "scaling").filter(|s| matches!(*s, "fill" | "fit" | "pixel"));
    let (width, height) = (number(value, "width"), number(value, "height"));
    let (Some(mode), Some(scaling)) = (mode, scaling) else {
        return DisplayPreferences::default();
    };
    if validate_resolution(width, height, f64::from(MAX_RENDER_DIMENSION)).is_some() {
        return DisplayPreferences::default();
    }
    let given = text(value, "aspect").filter(|a| valid_aspect(a));
    let aspect = match given {
        Some(a) if mode == "viewport" || aspect_matches(a, width, height) => a,
        _ => infer_aspect(width, height),
    };
    let (w, h) = (width as u32, height as u32);
    let (ratio_width, ratio_height) = if aspect == "custom" {
        let (rw, rh) = (whole(value, "ratio_width"), whole(value, "ratio_height"));
        if valid_ratio(rw, rh) { (rw, rh) } else { (Some(w), Some(h)) }
    } else {
        (None, None)
    };
    DisplayPreferences {
        mode: mode.into(),
        width: w,
        height: h,
        scaling: scaling.into(),
        aspect: aspect.into(),
        ratio_width,
        ratio_height,
    }
}

/// `width:height` in lowest terms.
pub fn aspect_label(width: u32, height: u32) -> String {
    let (mut a, mut b) = (width, height);
    while b != 0 {
        (a, b) = (b, a % b);
    }
    let a = a.max(1);
    format!("{}:{}", width / a, height / a)
}

/// The canvas's pixel size for a CSS size and device pixel ratio, bounded proportionally (never one axis alone) by
/// the device's texture limit and the render pixel limit, so pixels still map uniformly to pointer coordinates.
pub fn viewport_size(css_width: f64, css_height: f64, device_pixel_ratio: f64, device_limit: f64) -> Size {
    if !(css_width.is_finite() && css_width > 0.0 && css_height.is_finite() && css_height > 0.0) {
        return Size { width: 1, height: 1 };
    }
    let limit = if device_limit.is_finite() { device_limit } else { f64::from(MAX_RENDER_DIMENSION) };
    let limit = limit.min(f64::from(MAX_RENDER_DIMENSION)).max(1.0);
    let dpr = if device_pixel_ratio.is_finite() && device_pixel_ratio > 0.0 { device_pixel_ratio } else { 1.0 };
    let scale = dpr
        .min(limit / css_width)
        .min(limit / css_height)
        .min((MAX_RENDER_PIXELS / (css_width * css_height)).sqrt());
    Size {
        width: ((css_width * scale).floor() as u32).max(1),
        height: ((css_height * scale).floor() as u32).max(1),
    }
}

/// The scene's render size: the fixed resolution, or the viewport scaled down uniformly to at most `max_pixels`.
pub fn scene_resolution(preferences: &DisplayPreferences, viewport: Size, max_pixels: f64) -> Size {
    if preferences.mode != "viewport" {
        return Size { width: preferences.width, height: preferences.height };
    }
    let pixels = f64::from(viewport.width) * f64::from(viewport.height);
    if !(pixels > max_pixels) {
        return viewport;
    }
    let scale = (max_pixels / pixels).sqrt();
    Size {
        width: ((f64::from(viewport.width) * scale).floor() as u32).max(1),
        height: ((f64::from(viewport.height) * scale).floor() as u32).max(1),
    }
}

/// How the scene image maps onto the output: UV scale and offset, the whole-pixel factor in pixel mode, and whether
/// pixel mode crops. Scene UI scripts get the same mapping, so callouts follow the image.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Mapping {
    pub scale: [f64; 2],
    pub offset: [f64; 2],
    pub factor: Option<u32>,
    pub cropped: bool,
}

pub fn presentation_mapping(source: Size, output: Size, mode: &str) -> Mapping {
    let (sw, sh) = (f64::from(source.width.max(1)), f64::from(source.height.max(1)));
    let (ow, oh) = (f64::from(output.width.max(1)), f64::from(output.height.max(1)));
    if mode == "pixel" {
        let factor = ((ow / sw).floor().min((oh / sh).floor())).max(1.0);
        let (iw, ih) = (sw * factor, sh * factor);
        let (dx, dy) = (ow - iw, oh - ih);
        return Mapping {
            scale: [ow / iw, oh / ih],
            offset: [(dx / 2.0 - (dx / 2.0).floor()) / iw, (dy / 2.0 - (dy / 2.0).floor()) / ih],
            factor: Some(factor as u32),
            cropped: iw > ow || ih > oh,
        };
    }
    let ratio = (ow / oh) / (sw / sh);
    let scale = if mode == "fill" {
        if ratio < 1.0 { [ratio, 1.0] } else { [1.0, 1.0 / ratio] }
    } else if ratio < 1.0 {
        [1.0, 1.0 / ratio]
    } else {
        [ratio, 1.0]
    };
    Mapping { scale, offset: [0.0, 0.0], factor: None, cropped: false }
}

/// A selectable resolution.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ResolutionOption {
    pub id: String,
    pub label: String,
    pub width: u32,
    pub height: u32,
}

/// Presets for `aspect` that fit the device, smallest first, plus the reference size. Custom sizes scale from the
/// reference ratio (`ratio_reference`, else `reference`), rounded by at most one pixel.
pub fn resolution_options(
    aspect: &str,
    reference: Option<Size>,
    device_limit: f64,
    ratio_reference: Option<Size>,
) -> Vec<ResolutionOption> {
    if !valid_aspect(aspect) {
        return Vec::new();
    }
    let reference = reference
        .filter(|r| r.width > 0 && r.height > 0)
        .unwrap_or(Size { width: 1280, height: 720 });
    let ratio = ratio_reference
        .filter(|r| valid_ratio(Some(r.width), Some(r.height)))
        .unwrap_or(reference);
    let pairs: Vec<(u32, u32)> = if aspect == "custom" {
        CUSTOM_HEIGHTS
            .iter()
            .map(|&h| ((f64::from(h) * f64::from(ratio.width) / f64::from(ratio.height)).round() as u32, h))
            .collect()
    } else {
        common_resolutions(aspect).to_vec()
    };
    let mut found: Vec<ResolutionOption> = Vec::new();
    let mut add = |width: u32, height: u32| {
        let (w, h) = (f64::from(width), f64::from(height));
        let id = format!("{width}x{height}");
        if validate_resolution(w, h, device_limit).is_none()
            && (aspect == "custom" || aspect_matches(aspect, w, h))
            && !found.iter().any(|o| o.id == id)
        {
            found.push(ResolutionOption { id, label: format!("{width} × {height}"), width, height });
        }
    };
    for (width, height) in pairs {
        add(width, height);
    }
    add(reference.width, reference.height);
    found.sort_by_key(|o| u64::from(o.width) * u64::from(o.height));
    found
}

/// The display preferences being edited, validated against the device. The host persists `preferences` as JSON.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DisplaySettings {
    pub preferences: DisplayPreferences,
}

impl DisplaySettings {
    /// From stored JSON (anything unreadable gives the defaults).
    pub fn load(stored: Option<&str>) -> Self {
        let preferences = stored
            .and_then(|json| serde_json::from_str::<Value>(json).ok())
            .map_or_else(DisplayPreferences::default, |value| normalize(&value));
        Self { preferences }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(&self.preferences).unwrap_or_default()
    }

    /// Applies a settings request (`mode`, `width`, `height`, `scaling`, optional `aspect`, `ratio_width`,
    /// `ratio_height`, `reset_ratio`). `device_limit` bounds fixed resolutions.
    pub fn set(&mut self, next: &Value, device_limit: f64) -> Result<(), String> {
        let invalid = || "Invalid display setting.".to_string();
        let mode = text(next, "mode").filter(|m| matches!(*m, "fixed" | "viewport")).ok_or_else(invalid)?;
        let scaling = text(next, "scaling").filter(|s| matches!(*s, "fill" | "fit" | "pixel")).ok_or_else(invalid)?;
        let requested_aspect = next.get("aspect");
        if requested_aspect.is_some_and(|a| !a.as_str().is_some_and(valid_aspect)) {
            return Err(invalid());
        }
        let (width, height) = (number(next, "width"), number(next, "height"));
        let limit = if mode == "fixed" { device_limit } else { f64::from(MAX_RENDER_DIMENSION) };
        if let Some(error) = validate_resolution(width, height, limit) {
            return Err(error);
        }
        let (w, h) = (width as u32, height as u32);
        let aspect = match requested_aspect.and_then(Value::as_str) {
            Some(a) if mode == "viewport" || aspect_matches(a, width, height) => a,
            _ => infer_aspect(width, height),
        };
        let (mut ratio_width, mut ratio_height) = (None, None);
        if aspect == "custom" {
            let current = &self.preferences;
            let same_size = current.mode == "fixed" && current.aspect == "custom" && current.width == w && current.height == h;
            let reset = next.get("reset_ratio").and_then(Value::as_bool).unwrap_or(false);
            let previous = valid_ratio(current.ratio_width, current.ratio_height) && same_size && !reset;
            let requested = |key: &str| next.get(key).filter(|v| !v.is_null());
            let pick = |key: &str, kept: Option<u32>, size: u32| -> Option<u32> {
                match requested(key) {
                    Some(value) => value.as_u64().and_then(|v| u32::try_from(v).ok()).or(Some(0)),
                    None => Some(if previous { kept.unwrap_or(size) } else { size }),
                }
            };
            ratio_width = pick("ratio_width", current.ratio_width, w);
            ratio_height = pick("ratio_height", current.ratio_height, h);
            if !valid_ratio(ratio_width, ratio_height) {
                return Err("Invalid custom aspect ratio.".into());
            }
        }
        self.preferences = DisplayPreferences {
            mode: mode.into(),
            width: w,
            height: h,
            scaling: scaling.into(),
            aspect: aspect.into(),
            ratio_width,
            ratio_height,
        };
        Ok(())
    }

    /// Switches to the preset size of `aspect` nearest (in pixels) to the current scene size, or to a custom ratio of
    /// the current size.
    pub fn set_aspect(&mut self, aspect: &str, viewport: Size, device_limit: f64) -> Result<(), String> {
        if !valid_aspect(aspect) {
            return Err("Invalid aspect ratio.".into());
        }
        let reference = scene_resolution(&self.preferences, viewport, MAX_VIEWPORT_SCENE_PIXELS);
        if aspect == "custom" {
            let usable = validate_resolution(f64::from(reference.width), f64::from(reference.height), device_limit).is_none();
            let source = if usable {
                reference
            } else {
                Size { width: self.preferences.width, height: self.preferences.height }
            };
            self.preferences = DisplayPreferences {
                mode: "fixed".into(),
                width: source.width,
                height: source.height,
                aspect: aspect.into(),
                ratio_width: Some(source.width),
                ratio_height: Some(source.height),
                ..self.preferences.clone()
            };
            return Ok(());
        }
        let mut options = resolution_options(aspect, Some(reference), device_limit, None);
        if options.is_empty() {
            return Err("No supported resolutions for this aspect ratio.".into());
        }
        let pixels = (f64::from(reference.width) * f64::from(reference.height)).max(1.0);
        let distance = |o: &ResolutionOption| (f64::from(o.width) * f64::from(o.height) / pixels).ln().abs();
        options.sort_by(|a, b| distance(a).total_cmp(&distance(b)));
        let best = &options[0];
        self.preferences = DisplayPreferences {
            mode: "fixed".into(),
            width: best.width,
            height: best.height,
            scaling: self.preferences.scaling.clone(),
            aspect: aspect.into(),
            ratio_width: None,
            ratio_height: None,
        };
        Ok(())
    }

    /// The display part of the pause menu's settings: the preferences with the actual scene size and ratio, the
    /// aspect list, the resolution presets, and a pixel-perfect note.
    pub fn menu_state(&self, viewport: Size, device_limit: f64) -> Value {
        let p = &self.preferences;
        let current = scene_resolution(p, viewport, MAX_VIEWPORT_SCENE_PIXELS);
        let aspect = if p.mode == "viewport" {
            infer_aspect(f64::from(current.width), f64::from(current.height)).to_string()
        } else {
            p.aspect.clone()
        };
        let mapping = presentation_mapping(current, viewport, &p.scaling);
        let warning = if p.scaling == "pixel" {
            if mapping.cropped {
                "Pixel perfect: 1× cropped · choose a smaller resolution to fit.".to_string()
            } else {
                format!("Pixel perfect: {}× · whole pixels", mapping.factor.unwrap_or(1))
            }
        } else {
            String::new()
        };
        let ratio = if p.mode == "viewport" {
            current
        } else {
            Size {
                width: p.ratio_width.unwrap_or(current.width),
                height: p.ratio_height.unwrap_or(current.height),
            }
        };
        let mut display = serde_json::to_value(p).unwrap_or(Value::Null);
        if let Some(object) = display.as_object_mut() {
            object.insert("aspect".into(), aspect.clone().into());
            object.insert("actual_width".into(), current.width.into());
            object.insert("actual_height".into(), current.height.into());
            object.insert("ratio_width".into(), ratio.width.into());
            object.insert("ratio_height".into(), ratio.height.into());
        }
        let aspects: Vec<Value> = ASPECT_RATIOS
            .iter()
            .map(|id| serde_json::json!({"id": id, "label": if *id == "custom" { "Custom" } else { id }}))
            .collect();
        serde_json::json!({
            "display": display,
            "aspects": aspects,
            "presets": resolution_options(&aspect, Some(current), device_limit, Some(ratio)),
            "warning": warning,
        })
    }
}

/// Whether the performance overlay is shown (persisted as `"true"`/`"false"`, default shown).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HudPreferences {
    pub show_performance: bool,
}

impl HudPreferences {
    pub fn load(stored: Option<&str>) -> Self {
        Self {
            show_performance: stored != Some("false"),
        }
    }

    /// Accepts only a boolean.
    pub fn set(&mut self, value: &Value) -> bool {
        match value.as_bool() {
            Some(show) => {
                self.show_performance = show;
                true
            }
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const VIEWPORT: Size = Size { width: 1920, height: 1080 };

    fn size(width: u32, height: u32) -> Size {
        Size { width, height }
    }

    #[test]
    fn aspects_and_validation() {
        assert_eq!(ASPECT_RATIOS, ["16:9", "16:10", "21:9", "custom"]);
        assert_eq!(infer_aspect(1920.0, 1080.0), "16:9");
        assert_eq!(infer_aspect(1920.0, 1200.0), "16:10");
        assert_eq!(infer_aspect(3440.0, 1440.0), "21:9");
        assert_eq!(infer_aspect(2560.0, 1080.0), "21:9");
        assert_eq!(infer_aspect(1024.0, 768.0), "custom");
        assert_eq!(validate_resolution(1920.0, 1080.0, 8192.0), None);
        assert!(validate_resolution(20.0, 1080.0, 8192.0).unwrap().contains("at least 64"));
        assert!(validate_resolution(8192.0, 8192.0, 8192.0).unwrap().contains("megapixel"));
        assert!(validate_resolution(4096.0, 2160.0, 2048.0).unwrap().contains("maximum dimension"));
        assert_eq!(normalize(&json!({"mode":"fixed","width":0,"height":1080,"scaling":"fill"})), DisplayPreferences::default());
        assert_eq!(normalize(&json!({"mode":"fixed","width":1024,"height":768,"scaling":"fit"})).aspect, "custom");
        assert_eq!(aspect_label(1920, 1080), "16:9");
        assert_eq!(aspect_label(1024, 768), "4:3");
    }

    #[test]
    fn viewport_and_scene_sizes() {
        assert_eq!(viewport_size(1920.0, 1080.0, 2.0, 8192.0), size(3840, 2160));
        let bounded = viewport_size(12000.0, 3000.0, 2.0, 4096.0);
        assert!(bounded.width <= 4096 && bounded.height <= 4096);
        assert!(f64::from(bounded.width) * f64::from(bounded.height) <= MAX_RENDER_PIXELS);
        assert!((f64::from(bounded.width) / f64::from(bounded.height) - 4.0).abs() < 0.01);
        let viewport = DisplayPreferences::default();
        assert_eq!(viewport.mode, "viewport");
        assert!(scene_resolution(&viewport, VIEWPORT, MAX_VIEWPORT_SCENE_PIXELS).width < 1920);
        for (w, h) in [(3840, 2160), (2560, 1440), (3440, 1440), (1674, 1439), (5120, 2880)] {
            let scene = scene_resolution(&viewport, size(w, h), MAX_VIEWPORT_SCENE_PIXELS);
            assert!(f64::from(scene.width) * f64::from(scene.height) <= MAX_VIEWPORT_SCENE_PIXELS, "{w}x{h}");
            assert!(scene.width <= w && scene.height <= h);
            assert!((f64::from(scene.width) / f64::from(scene.height) - f64::from(w) / f64::from(h)).abs() < 0.01);
        }
        assert_eq!(scene_resolution(&viewport, size(1600, 900), MAX_VIEWPORT_SCENE_PIXELS), size(1600, 900));
        assert_eq!(scene_resolution(&viewport, size(1280, 720), MAX_VIEWPORT_SCENE_PIXELS), size(1280, 720));
        let fixed = DisplayPreferences { mode: "fixed".into(), width: 3840, height: 2160, ..Default::default() };
        assert_eq!(scene_resolution(&fixed, size(800, 600), MAX_VIEWPORT_SCENE_PIXELS), size(3840, 2160));
        assert_eq!(scene_resolution(&viewport, size(1000, 1000), 250_000.0), size(500, 500));
    }

    #[test]
    fn resolution_presets_and_presentation() {
        for aspect in ["16:9", "16:10", "21:9"] {
            let options = resolution_options(aspect, Some(VIEWPORT), 4096.0, None);
            assert!(options.len() >= 4);
            assert!(options.iter().all(|o| aspect_matches(aspect, f64::from(o.width), f64::from(o.height))
                && validate_resolution(f64::from(o.width), f64::from(o.height), 4096.0).is_none()));
            assert!(options.iter().any(|o| o.width * o.height < 1920 * 1080));
            assert!(options.iter().any(|o| o.width * o.height > 1920 * 1080));
        }
        let custom = resolution_options("custom", Some(size(1600, 1200)), 4096.0, None);
        assert!(custom.iter().any(|o| o.width == 1600 && o.height == 1200));
        assert!(custom.iter().all(|o| (f64::from(o.width) / f64::from(o.height) - 4.0 / 3.0).abs() < 0.002));
        assert!(!custom.iter().any(|o| o.id == "viewport"));
        assert_eq!(
            presentation_mapping(size(2, 2), size(5, 5), "pixel"),
            Mapping { scale: [1.25, 1.25], offset: [0.125, 0.125], factor: Some(2), cropped: false }
        );
        assert_eq!(
            presentation_mapping(size(8, 6), size(5, 5), "pixel"),
            Mapping { scale: [0.625, 5.0 / 6.0], offset: [0.5 / 8.0, 0.5 / 6.0], factor: Some(1), cropped: true }
        );
        assert_eq!(presentation_mapping(size(2, 2), size(5, 5), "fit").offset, [0.0, 0.0]);
    }

    #[test]
    fn settings_requests_persist_and_keep_custom_ratios() {
        let mut settings = DisplaySettings::load(None);
        assert_eq!(settings.preferences, DisplayPreferences::default());
        settings.set(&json!({"mode":"fixed","width":1920,"height":1080,"scaling":"pixel","aspect":"16:9"}), 4096.0).unwrap();
        assert!(settings.set(&json!({"mode":"fixed","width":5000,"height":1000,"scaling":"fit"}), 4096.0).unwrap_err().contains("maximum dimension"));
        assert_eq!(settings.preferences.width, 1920);
        settings.set_aspect("16:10", VIEWPORT, 4096.0).unwrap();
        assert_eq!(settings.preferences.aspect, "16:10");
        assert!(aspect_matches("16:10", f64::from(settings.preferences.width), f64::from(settings.preferences.height)));
        settings.set_aspect("21:9", VIEWPORT, 4096.0).unwrap();
        assert!(aspect_matches("21:9", f64::from(settings.preferences.width), f64::from(settings.preferences.height)));
        settings.set_aspect("custom", VIEWPORT, 4096.0).unwrap();
        settings.set(&json!({"mode":"fixed","width":1600,"height":1200,"scaling":"pixel","aspect":"custom"}), 4096.0).unwrap();
        assert_eq!(settings.preferences.aspect, "custom");
        // A native 1280x605 ratio must not drift when stepping through rounded sizes.
        settings.set(&json!({"mode":"fixed","width":1280,"height":605,"scaling":"pixel","aspect":"custom"}), 4096.0).unwrap();
        for height in [600u32, 270, 720, 600] {
            let p = settings.preferences.clone();
            let ratio = size(p.ratio_width.unwrap(), p.ratio_height.unwrap());
            let choices = resolution_options("custom", Some(size(p.width, p.height)), 4096.0, Some(ratio));
            let width = (f64::from(height) * 1280.0 / 605.0).round() as u32;
            assert!(choices.iter().any(|c| c.width == width && c.height == height));
            settings.set(&json!({"mode":"fixed","width":width,"height":height,"scaling":"pixel","aspect":"custom",
                "ratio_width":ratio.width,"ratio_height":ratio.height}), 4096.0).unwrap();
            assert_eq!((settings.preferences.ratio_width, settings.preferences.ratio_height), (Some(1280), Some(605)));
        }
        let (w, h) = (settings.preferences.width, settings.preferences.height);
        settings.set(&json!({"mode":"fixed","width":w,"height":h,"scaling":"fit","aspect":"custom"}), 4096.0).unwrap();
        assert_eq!((settings.preferences.ratio_width, settings.preferences.ratio_height), (Some(1280), Some(605)));
        settings.set(&json!({"mode":"fixed","width":w,"height":h,"scaling":"fit","aspect":"custom","reset_ratio":true}), 4096.0).unwrap();
        assert_eq!((settings.preferences.ratio_width, settings.preferences.ratio_height), (Some(1269), Some(600)));
        assert!(settings.set(&json!({"mode":"fixed","width":1920,"height":1080,"scaling":"fit","aspect":"bad"}), 4096.0).is_err());
        settings.set_aspect("16:9", VIEWPORT, 4096.0).unwrap();
        assert!(!settings.to_json().contains("ratio_width"));
        assert_eq!(DisplaySettings::load(Some(&settings.to_json())), settings);
        let mut hud = HudPreferences::load(None);
        assert!(hud.show_performance);
        assert!(hud.set(&json!(false)));
        assert!(!HudPreferences::load(Some("false")).show_performance);
        assert!(!hud.set(&json!("false")));
        assert!(!hud.show_performance);
    }
}
