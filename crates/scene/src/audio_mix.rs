//! Mixing rules for scene audio in the browser: layer gains (enable, mute, solo), validation of a scene's play
//! request, constant-power stereo balance, and the voice limit. Lua decides what plays; the host applies these.

use serde_json::Value;

/// Most voices playing at once; the oldest stops when another starts.
pub const MAX_VOICES: usize = 32;

/// A mixer layer's settings from a scene `audio.layer` event.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Layer {
    pub gain: f64,
    pub muted: bool,
    pub solo: bool,
    pub enabled: bool,
}

fn finite(value: Option<&Value>, low: f64, high: f64) -> Option<f64> {
    value.and_then(Value::as_f64).filter(|v| v.is_finite() && (low..=high).contains(v))
}

/// The layer named by an `audio.layer` event (`{name, enabled, mix: {gain, muted, solo}}`), or `None` if invalid.
pub fn layer_event(event: &Value) -> Option<(String, Layer)> {
    let name = event.get("name")?.as_str().filter(|n| !n.is_empty() && n.len() <= 64)?;
    let mix = event.get("mix")?;
    Some((
        name.to_string(),
        Layer {
            gain: finite(mix.get("gain"), 0.0, 2.0)?,
            muted: mix.get("muted")?.as_bool()?,
            solo: mix.get("solo")?.as_bool()?,
            enabled: event.get("enabled")?.as_bool()?,
        },
    ))
}

/// Effective gain of each layer: zero when disabled, muted, or when another enabled layer is soloed.
pub fn layer_gains<'a>(layers: impl Iterator<Item = &'a Layer> + Clone) -> Vec<f64> {
    let solo = layers.clone().any(|layer| layer.enabled && layer.solo);
    layers
        .map(|layer| if layer.enabled && !layer.muted && (!solo || layer.solo) { layer.gain } else { 0.0 })
        .collect()
}

/// A validated play request.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Voice {
    /// Left and right gains (constant-power balance).
    pub left: f64,
    pub right: f64,
    pub rate: f64,
    pub looped: bool,
}

/// The voice for an `audio.play` event's `settings` (`gain` 0..2, `balance` -1..1, `rate` 0.25..4, `looped`).
pub fn voice(settings: &Value) -> Option<Voice> {
    let gain = finite(settings.get("gain"), 0.0, 2.0)?;
    let balance = finite(settings.get("balance"), -1.0, 1.0)?;
    let rate = finite(settings.get("rate"), 0.25, 4.0)?;
    let looped = settings.get("looped")?.as_bool()?;
    let angle = (balance + 1.0) * std::f64::consts::FRAC_PI_4;
    Some(Voice {
        left: gain * std::f64::consts::SQRT_2 * angle.cos(),
        right: gain * std::f64::consts::SQRT_2 * angle.sin(),
        rate,
        looped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn layers_solo_mute_and_validation() {
        let (_, sound) = layer_event(&json!({"name":"sound","enabled":true,"mix":{"gain":0.8,"muted":false,"solo":false}})).unwrap();
        let (_, music) = layer_event(&json!({"name":"music","enabled":true,"mix":{"gain":0.5,"muted":false,"solo":true}})).unwrap();
        assert_eq!(layer_gains([sound, music].iter()), [0.0, 0.5]);
        let muted = Layer { muted: true, ..music };
        assert_eq!(layer_gains([sound, muted].iter()), [0.0, 0.0]);
        assert_eq!(layer_gains([sound].iter()), [0.8]);
        assert!(layer_event(&json!({"name":"","enabled":true,"mix":{"gain":0.8,"muted":false,"solo":false}})).is_none());
        assert!(layer_event(&json!({"name":"x","enabled":true,"mix":{"gain":3,"muted":false,"solo":false}})).is_none());
    }

    #[test]
    fn constant_power_balance_and_bounds() {
        let centre = voice(&json!({"gain":0.5,"balance":0,"rate":1,"looped":false})).unwrap();
        assert!((centre.left - 0.5).abs() < 1e-12 && (centre.right - 0.5).abs() < 1e-12);
        let left = voice(&json!({"gain":0.5,"balance":-1,"rate":1,"looped":false})).unwrap();
        assert!(left.right.abs() < 1e-12 && left.left > 0.7);
        assert!(voice(&json!({"gain":0.5,"balance":0,"rate":"fast","looped":false})).is_none());
        assert!(voice(&json!({"gain":0.5,"balance":0,"rate":10,"looped":false})).is_none());
    }
}
