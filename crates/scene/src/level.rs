//! Portable, script-visible descriptions of the active level and its render settings.
//! Asset paths are level-relative references; bytes and GPU objects stay with their owners.
use crate::{AssetLoadPolicy, LevelFormat};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// The purpose of a file in the level's asset catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LevelFileKind {
    Manifest,
    ViewerSettings,
    ChunkMetadata,
    SplatData,
    Collision,
    Other,
}

/// One level-relative file and the loading policy used by its subsystem.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LevelFile {
    pub path: String,
    pub kind: LevelFileKind,
    pub load: AssetLoadPolicy,
}

/// The active level's asset references, published by the host after resolving its manifest.
/// `source` names a `sog_level` scene asset. An unbound scene template has no `format` or files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LevelAssets {
    pub source: String,
    pub level_id: String,
    pub name: String,
    pub format: Option<LevelFormat>,
    pub lod_count: u32,
    pub files: Vec<LevelFile>,
}
impl Default for LevelAssets {
    fn default() -> Self {
        Self {
            source: "level".into(),
            level_id: String::new(),
            name: String::new(),
            format: None,
            lod_count: 1,
            files: Vec::new(),
        }
    }
}
impl LevelAssets {
    /// Validate script and manifest writes before publishing them to the world.
    pub fn validate(&self) -> Result<(), &'static str> {
        let valid_text =
            |s: &str, limit: usize| s.len() <= limit && !s.chars().any(char::is_control);
        if self.source.trim().is_empty()
            || !valid_text(&self.source, 256)
            || !valid_text(&self.level_id, 128)
            || !valid_text(&self.name, 512)
            || !(1..=64).contains(&self.lod_count)
            || self.files.len() > 8192
            || (self.format.is_some()
                && (self.level_id.trim().is_empty() || self.name.trim().is_empty()))
        {
            return Err("level asset identity, LOD count, or file count is invalid");
        }
        let mut unique = BTreeSet::new();
        let mut path_bytes = 0usize;
        for file in &self.files {
            let path = file.path.as_str();
            path_bytes += path.len();
            if path.is_empty()
                || path.len() > 1024
                || path.starts_with('/')
                || path.contains('\\')
                || path.contains(':')
                || path.chars().any(char::is_control)
                || path
                    .split('/')
                    .any(|part| part.is_empty() || part == "." || part == "..")
                || !unique.insert(path)
                || path_bytes > 512 * 1024
            {
                return Err("level asset paths must be unique, confined relative paths");
            }
        }
        Ok(())
    }
}

/// `None` selects LOD automatically; `Some(0)` requests the finest available LOD.
/// The renderer may choose a coarser level to stay within its GPU pool budget.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LevelSettings {
    pub lod: Option<u32>,
    /// Forward reach of the browser's streaming region in world units.
    pub draw_distance: f32,
}
impl Default for LevelSettings {
    fn default() -> Self {
        Self {
            lod: None,
            draw_distance: 150.0,
        }
    }
}
impl LevelSettings {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.lod.is_some_and(|lod| lod >= 64) {
            return Err("level LOD must be between zero and 63, or null for automatic selection");
        }
        if !self.draw_distance.is_finite() || !(10.0..=1000.0).contains(&self.draw_distance) {
            return Err("draw distance must be finite and between 10 and 1000 world units");
        }
        Ok(())
    }
    pub fn validate_for(&self, assets: &LevelAssets) -> Result<(), &'static str> {
        self.validate()?;
        if self.lod.is_some_and(|lod| lod >= assets.lod_count) {
            return Err("selected LOD does not exist in this level");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn level_asset_paths_and_lod_are_validated() {
        let mut assets = LevelAssets::default();
        assert!(assets.validate().is_ok());
        assets.files.push(LevelFile {
            path: "../escape".into(),
            kind: LevelFileKind::Other,
            load: AssetLoadPolicy::Streamable,
        });
        assert!(assets.validate().is_err());
        assets.files[0].path = "0_0/meta.json".into();
        assert!(assets.validate().is_ok());
        assets.files.push(assets.files[0].clone());
        assert!(assets.validate().is_err());
        assert!(LevelSettings {
            lod: Some(1),
            ..Default::default()
        }
        .validate_for(&LevelAssets::default())
        .is_err());
    }
}
