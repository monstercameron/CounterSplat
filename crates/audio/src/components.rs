use crate::{config::validate_layer_name, AudioError, LayerMix, PlaySettings};
use serde::{Deserialize, Serialize};

/// Initial mixer-layer state for a scene or script. Hosts reconcile it with an audio owner.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AudioLayer {
    pub name: String,
    pub mix: LayerMix,
    pub enabled: bool,
}
impl Default for AudioLayer {
    fn default() -> Self {
        Self {
            name: "sound".into(),
            mix: LayerMix::default(),
            enabled: true,
        }
    }
}
impl AudioLayer {
    pub fn validate(&self) -> Result<(), AudioError> {
        validate_layer_name(&self.name)?;
        self.mix.validate()
    }
}

/// A script-neutral sound source referring to a scene asset by ID, never by device handle.
/// The host decides how and when to resolve assets and start/stop the indicated voice.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AudioEmitter {
    pub asset: String,
    pub layer: String,
    pub settings: PlaySettings,
    pub enabled: bool,
    pub autoplay: bool,
}
impl Default for AudioEmitter {
    fn default() -> Self {
        Self {
            asset: String::new(),
            layer: "sound".into(),
            settings: PlaySettings::default(),
            enabled: true,
            autoplay: false,
        }
    }
}
impl AudioEmitter {
    pub fn validate(&self) -> Result<(), AudioError> {
        if self.asset.is_empty()
            || self.asset.len() > 256
            || self.asset.trim() != self.asset
            || self.asset.chars().any(char::is_control)
        {
            return Err(AudioError::Invalid(
                "audio asset ID must be nonempty, trimmed, and at most 256 bytes",
            ));
        }
        validate_layer_name(&self.layer)?;
        self.settings.validate()
    }
}
