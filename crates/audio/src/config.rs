use crate::AudioError;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Common layers. Applications can add their own independent layers.
pub const DEFAULT_LAYERS: [&str; 5] = ["music", "sound", "ambience", "voice", "ui"];

/// Mixer limits and the default de-click transition for gain and balance changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioConfig {
    pub max_layers: usize,
    pub voices_per_layer: usize,
    pub transition: Duration,
}
impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            max_layers: 32,
            voices_per_layer: 128,
            transition: Duration::from_millis(10),
        }
    }
}
impl AudioConfig {
    pub(crate) fn validate(self) -> Result<(), AudioError> {
        if !(DEFAULT_LAYERS.len()..=128).contains(&self.max_layers) {
            return Err(AudioError::Invalid("max_layers must be between 5 and 128"));
        }
        if !(1..=1024).contains(&self.voices_per_layer) {
            return Err(AudioError::Invalid(
                "voices_per_layer must be between 1 and 1024",
            ));
        }
        if self.transition > Duration::from_secs(30) {
            return Err(AudioError::Invalid("transition must not exceed 30 seconds"));
        }
        Ok(())
    }
}

/// Settings for a mixer layer. Solo silences other layers without changing their saved gains.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LayerMix {
    pub gain: f32,
    pub muted: bool,
    pub solo: bool,
}
impl Default for LayerMix {
    fn default() -> Self {
        Self {
            gain: 1.0,
            muted: false,
            solo: false,
        }
    }
}
impl LayerMix {
    pub fn validate(self) -> Result<(), AudioError> {
        validate_gain(self.gain)
    }
}

/// Initial settings for one playing instance; gain is linear and balance is -1..=1.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PlaySettings {
    pub gain: f32,
    pub balance: f32,
    pub rate: f64,
    pub looped: bool,
}
impl Default for PlaySettings {
    fn default() -> Self {
        Self {
            gain: 1.0,
            balance: 0.0,
            rate: 1.0,
            looped: false,
        }
    }
}
impl PlaySettings {
    pub fn validate(self) -> Result<(), AudioError> {
        validate_gain(self.gain)?;
        validate_balance(self.balance)?;
        validate_rate(self.rate)
    }
}
pub(crate) fn validate_gain(gain: f32) -> Result<(), AudioError> {
    if gain.is_finite() && (0.0..=2.0).contains(&gain) {
        Ok(())
    } else {
        Err(AudioError::Invalid(
            "gain must be finite and between 0 and 2",
        ))
    }
}
pub(crate) fn validate_balance(balance: f32) -> Result<(), AudioError> {
    if balance.is_finite() && (-1.0..=1.0).contains(&balance) {
        Ok(())
    } else {
        Err(AudioError::Invalid(
            "balance must be finite and between -1 and 1",
        ))
    }
}
pub(crate) fn validate_rate(rate: f64) -> Result<(), AudioError> {
    if rate.is_finite() && (0.25..=4.0).contains(&rate) {
        Ok(())
    } else {
        Err(AudioError::Invalid(
            "playback rate must be finite and between 0.25 and 4",
        ))
    }
}
pub(crate) fn validate_layer_name(name: &str) -> Result<(), AudioError> {
    if !name.is_empty()
        && name.len() <= 128
        && name.trim() == name
        && !name.chars().any(char::is_control)
    {
        Ok(())
    } else {
        Err(AudioError::Invalid(
            "layer name must be nonempty, trimmed, and at most 128 bytes",
        ))
    }
}
