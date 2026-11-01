//! Renderer-independent scene components.
//!
//! A host owns assets separately from scene state. Components reference opaque asset IDs rather
//! than filesystem paths or GPU objects, so snapshots can cross subsystem and scripting boundaries.
mod levels;
mod loading;
mod manifest;
mod player;
pub use levels::{LevelAssetPolicies, LevelCatalog, LevelEntry, LevelFormat};
pub use loading::{AssetLoader, AssetLoadingStatus};
pub use manifest::{
    AssetDefinition, AssetLoadPolicy, ComponentDefinition, ComponentStorage, EntityDefinition,
    SceneManifest, SceneResources, ScriptDefinition,
};
pub use player::{PlayerBindings, PlayerCamera, PlayerControls};

use serde::{Deserialize, Serialize};

/// An environment background layer referencing an equirectangular HDR asset by opaque ID.
///
/// Multiple enabled skyboxes may coexist; the scene adapter selects the greatest `priority`.
/// `exposure` is a linear intensity multiplier and `rotation_degrees` rotates around world +Y.
/// This controls the visible background only; it does not yet provide image-based scene lighting.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Skybox {
    pub asset: String,
    pub enabled: bool,
    pub priority: i32,
    pub exposure: f32,
    pub rotation_degrees: f32,
}
impl Default for Skybox {
    fn default() -> Self {
        Self {
            asset: "default".into(),
            enabled: true,
            priority: 0,
            exposure: 1.0,
            rotation_degrees: 0.0,
        }
    }
}
impl Skybox {
    pub fn new(asset: impl Into<String>) -> Result<Self, &'static str> {
        let skybox = Self {
            asset: asset.into(),
            ..Self::default()
        };
        skybox.validate()?;
        Ok(skybox)
    }
    /// Reject malformed untrusted/script-provided configuration before storing or rendering it.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.asset.trim().is_empty()
            || self.asset.len() > 256
            || self.asset.chars().any(char::is_control)
        {
            return Err("skybox asset ID must be nonempty, at most 256 bytes, and contain no control characters");
        }
        if !self.exposure.is_finite() || !(0.0..=64.0).contains(&self.exposure) {
            return Err("skybox exposure must be finite and between 0 and 64");
        }
        if !self.rotation_degrees.is_finite() || self.rotation_degrees.abs() > 1_000_000.0 {
            return Err("skybox rotation must be finite and within one million degrees");
        }
        Ok(())
    }
}

/// An owned selection that can be sent from an ECS owner to a render worker.
#[derive(Debug, Clone, PartialEq)]
pub struct SkyboxSnapshot {
    pub entity_id: String,
    pub skybox: Skybox,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn skybox_configuration_is_validated() {
        assert!(Skybox::new("").is_err());
        let mut sky = Skybox::new("outdoor/sunset").unwrap();
        sky.exposure = f32::NAN;
        assert!(sky.validate().is_err());
        sky.exposure = 1.0;
        sky.rotation_degrees = f32::INFINITY;
        assert!(sky.validate().is_err());
    }
}
