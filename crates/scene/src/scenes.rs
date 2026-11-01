//! Local, playable scene manifests discoverable by a host.
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneChoice {
    /// A single JSON filename in the host's scenes directory.
    pub id: String,
    pub name: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneCatalog {
    pub version: u32,
    pub scenes: Vec<SceneChoice>,
}
impl SceneCatalog {
    pub fn from_json(bytes: &[u8]) -> Result<Self, String> {
        if bytes.is_empty() || bytes.len() > 1024 * 1024 {
            return Err("scene catalog size is invalid".into());
        }
        let catalog: Self =
            serde_json::from_slice(bytes).map_err(|e| format!("scene catalog: {e}"))?;
        if catalog.version != 1 || catalog.scenes.len() > 256 {
            return Err("scene catalog version or size is invalid".into());
        }
        let mut ids = BTreeSet::new();
        for scene in &catalog.scenes {
            let Some(stem) = scene.id.strip_suffix(".json") else {
                return Err("scene filename must end in .json".into());
            };
            if stem.is_empty()
                || stem.len() > 80
                || !stem
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
                || !ids.insert(&scene.id)
                || scene.name.trim().is_empty()
                || scene.name.len() > 512
                || scene.name.chars().any(char::is_control)
            {
                return Err("invalid scene catalog entry".into());
            }
        }
        Ok(catalog)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_paths_and_duplicate_scene_ids() {
        let parse = |v| SceneCatalog::from_json(serde_json::to_vec(&v).unwrap().as_slice());
        let mut v =
            serde_json::json!({"version":1,"scenes":[{"id":"gameplay.json","name":"Gameplay"}]});
        assert_eq!(parse(v.clone()).unwrap().scenes.len(), 1);
        v["scenes"][0]["id"] = serde_json::json!("../outside.json");
        assert!(parse(v).is_err());
        let v = serde_json::json!({"version":1,"scenes":[{"id":"a.json","name":"A"},{"id":"a.json","name":"B"}]});
        assert!(parse(v).is_err());
    }
}
