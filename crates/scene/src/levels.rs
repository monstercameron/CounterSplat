//! A portable catalog of local level assets. It owns no filesystem or renderer state.
use crate::AssetLoadPolicy;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Level formats supported by the example browser host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LevelFormat {
    StreamedSog,
    SingleSog,
}

/// Policies for assets discovered from the level manifest and catalog entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LevelAssetPolicies {
    pub manifest: AssetLoadPolicy,
    pub viewer_settings: AssetLoadPolicy,
    pub chunk_metadata: AssetLoadPolicy,
    pub splat_chunks: AssetLoadPolicy,
}
impl Default for LevelAssetPolicies {
    fn default() -> Self {
        Self {
            manifest: AssetLoadPolicy::Preload,
            viewer_settings: AssetLoadPolicy::Preload,
            chunk_metadata: AssetLoadPolicy::Preload,
            splat_chunks: AssetLoadPolicy::Streamable,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LevelEntry {
    pub id: String,
    pub name: String,
    pub creator: String,
    pub format: LevelFormat,
    /// Project-relative path; checked against the ID and format before use.
    pub manifest: String,
    /// Rotate source splats into the viewer camera coordinate system (default: true).
    #[serde(default = "default_splat_rotation")]
    pub rotate_z_180: bool,
    /// Optional authored calibration: level coordinate units per physical metre.
    #[serde(default)]
    pub player_units_per_meter: Option<f32>,
    #[serde(default)]
    pub lod_levels: Option<u32>,
    #[serde(default)]
    pub verified_bytes: u64,
    #[serde(default)]
    pub assets: LevelAssetPolicies,
}

fn default_splat_rotation() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LevelCatalog {
    pub version: u32,
    pub levels: Vec<LevelEntry>,
}
impl LevelCatalog {
    /// Parse untrusted catalog bytes and validate IDs, paths, and display strings.
    pub fn from_json(bytes: &[u8]) -> Result<Self, String> {
        if bytes.is_empty() || bytes.len() > 4 * 1024 * 1024 {
            return Err("level catalog must be between 1 byte and 4 MiB".into());
        }
        let catalog: Self =
            serde_json::from_slice(bytes).map_err(|error| format!("level catalog: {error}"))?;
        if catalog.version != 1 || catalog.levels.is_empty() || catalog.levels.len() > 256 {
            return Err("level catalog version or number of levels is invalid".into());
        }
        let mut seen = BTreeSet::new();
        for level in &catalog.levels {
            if level.id.len() != 8
                || !level
                    .id
                    .bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
                || !seen.insert(&level.id)
            {
                return Err(format!("invalid or duplicate level ID `{}`", level.id));
            }
            if [level.name.as_str(), level.creator.as_str()]
                .iter()
                .any(|v| v.trim().is_empty() || v.len() > 512 || v.chars().any(char::is_control))
            {
                return Err(format!("invalid level name or creator for `{}`", level.id));
            }
            let filename = match level.format {
                LevelFormat::StreamedSog => "lod-meta.json",
                LevelFormat::SingleSog => "meta.json",
            };
            if level.manifest != format!("assets/levels/{}/{filename}", level.id) {
                return Err(format!("invalid manifest path for `{}`", level.id));
            }
            if level.assets.manifest != AssetLoadPolicy::Preload
                || level.assets.viewer_settings != AssetLoadPolicy::Preload
                || level.assets.chunk_metadata != AssetLoadPolicy::Preload
            {
                return Err(format!(
                    "level `{}` requires preloaded manifest, viewer settings, and chunk metadata",
                    level.id
                ));
            }
            if level
                .player_units_per_meter
                .is_some_and(|scale| !scale.is_finite() || !(0.5..=2.5).contains(&scale))
            {
                return Err(format!("invalid player scale for `{}`", level.id));
            }
            if level.lod_levels.is_some_and(|n| n == 0 || n > 64) {
                return Err(format!("invalid LOD level count for `{}`", level.id));
            }
        }
        Ok(catalog)
    }
    pub fn get(&self, id: &str) -> Option<&LevelEntry> {
        self.levels.iter().find(|entry| entry.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn catalog() -> serde_json::Value {
        json!({"version":1,"levels":[{"id":"1234abcd","name":"Example","creator":"Artist",
            "format":"streamed_sog","manifest":"assets/levels/1234abcd/lod-meta.json","lod_levels":3}]})
    }
    #[test]
    fn validates_explicit_level_loading_policies() {
        let parse =
            |v: serde_json::Value| LevelCatalog::from_json(&serde_json::to_vec(&v).unwrap());
        let mut value = catalog();
        value["levels"][0]["assets"] = json!({
            "manifest": "preload", "viewer_settings": "preload", "chunk_metadata": "preload",
            "splat_chunks": "preload"
        });
        assert_eq!(
            parse(value.clone()).unwrap().levels[0].assets.splat_chunks,
            AssetLoadPolicy::Preload
        );
        value["levels"][0]["assets"]["splat_chunks"] = json!("streamable");
        assert_eq!(
            parse(value.clone()).unwrap().levels[0].assets.splat_chunks,
            AssetLoadPolicy::Streamable
        );
        value["levels"][0]["assets"]["chunk_metadata"] = json!("streamable");
        assert!(parse(value).is_err());
    }
    #[test]
    fn validates_level_ids_and_manifest_paths() {
        let parse =
            |v: serde_json::Value| LevelCatalog::from_json(&serde_json::to_vec(&v).unwrap());
        assert!(
            parse(catalog())
                .unwrap()
                .get("1234abcd")
                .unwrap()
                .rotate_z_180
        );
        let mut no_rotation = catalog();
        no_rotation["levels"][0]["rotate_z_180"] = json!(false);
        assert!(
            !parse(no_rotation)
                .unwrap()
                .get("1234abcd")
                .unwrap()
                .rotate_z_180
        );
        let mut bad = catalog();
        bad["levels"][0]["manifest"] = json!("assets/levels/../../secret");
        assert!(parse(bad).is_err());
        let mut bad = catalog();
        let entry = bad["levels"][0].clone();
        bad["levels"].as_array_mut().unwrap().push(entry);
        assert!(parse(bad).is_err());
        let mut scaled = catalog();
        scaled["levels"][0]["player_units_per_meter"] = json!(1.5);
        assert_eq!(
            parse(scaled.clone()).unwrap().levels[0].player_units_per_meter,
            Some(1.5)
        );
        scaled["levels"][0]["player_units_per_meter"] = json!(0.01);
        assert!(parse(scaled).is_err());
        let mut bad = catalog();
        bad["levels"][0]["id"] = json!("../etc");
        assert!(parse(bad).is_err());
    }
}
