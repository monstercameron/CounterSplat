//! Serializable scene descriptions. Hosts decide how to load assets and execute scripts.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// Maximum encoded scene size, before JSON is parsed.
pub const MAX_SCENE_BYTES: usize = 4 * 1024 * 1024;

/// A versioned, backend-independent scene configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SceneManifest {
    pub version: u32,
    pub name: String,
    /// Each component used by an entity must be explicitly declared here.
    pub component_types: BTreeMap<String, ComponentDefinition>,
    #[serde(default)]
    pub resources: SceneResources,
    pub entities: Vec<EntityDefinition>,
}

/// Native components require a host-registered adapter; dynamic components store JSON values.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ComponentDefinition {
    pub storage: ComponentStorage,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComponentStorage {
    Native,
    Dynamic,
}

/// Asset and script catalogs plus arbitrary named, JSON-compatible game resources.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SceneResources {
    #[serde(default)]
    pub assets: BTreeMap<String, AssetDefinition>,
    #[serde(default)]
    pub scripts: BTreeMap<String, ScriptDefinition>,
    #[serde(flatten)]
    pub values: BTreeMap<String, Value>,
}

/// Whether an asset must be ready before a scene transition or may be loaded later.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetLoadPolicy {
    #[default]
    Preload,
    Streamable,
}

/// An opaque asset reference. A host resolves `uri` relative to the manifest location.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssetDefinition {
    pub kind: String,
    pub uri: String,
    #[serde(default)]
    pub load: AssetLoadPolicy,
}

/// A script reference; `autoload` asks a supporting host to execute it at scene startup.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScriptDefinition {
    pub language: String,
    pub uri: String,
    #[serde(default)]
    pub autoload: bool,
    #[serde(default)]
    pub load: AssetLoadPolicy,
}

/// Stable manifest ID, layer, optional parent ID, and component values.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntityDefinition {
    pub id: String,
    pub layer: String,
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default)]
    pub components: BTreeMap<String, Value>,
}

fn valid_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value.trim() == value
        && !value.chars().any(char::is_control)
}
fn valid_uri(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 4096
        && value.trim() == value
        && !value.chars().any(char::is_control)
}

impl SceneManifest {
    /// Parse and structurally validate a manifest without creating any runtime objects.
    pub fn from_json(bytes: &[u8]) -> Result<Self, String> {
        if bytes.is_empty() || bytes.len() > MAX_SCENE_BYTES {
            return Err("scene JSON must be between 1 byte and 4 MiB".into());
        }
        let scene: Self =
            serde_json::from_slice(bytes).map_err(|e| format!("invalid scene JSON: {e}"))?;
        scene.validate()?;
        Ok(scene)
    }

    /// Check references, component declarations, layer-local parentage and cycles.
    pub fn validate(&self) -> Result<(), String> {
        if self.version != 1 {
            return Err(format!("unsupported scene version {}", self.version));
        }
        if !valid_name(&self.name) {
            return Err("invalid scene name".into());
        }
        if self.component_types.len() > 1024
            || self.entities.len() > 10_000
            || self.resources.assets.len() > 4096
            || self.resources.scripts.len() > 4096
        {
            return Err("scene exceeds its component, entity, asset or script limit".into());
        }
        for name in self.component_types.keys() {
            if !valid_name(name) {
                return Err(format!("invalid component type name `{name}`"));
            }
        }
        for (id, asset) in &self.resources.assets {
            if !valid_name(id) || !valid_name(&asset.kind) || !valid_uri(&asset.uri) {
                return Err(format!("invalid asset definition `{id}`"));
            }
        }
        for (id, script) in &self.resources.scripts {
            if !valid_name(id) || !valid_name(&script.language) || !valid_uri(&script.uri) {
                return Err(format!("invalid script definition `{id}`"));
            }
            if script.autoload && script.load != AssetLoadPolicy::Preload {
                return Err(format!("autoload script `{id}` must be preloaded"));
            }
        }
        for name in self.resources.values.keys() {
            if !valid_name(name)
                || name == "scene.assets"
                || name == "scene.scripts"
                || name == "scene.entities"
            {
                return Err(format!("invalid or reserved resource name `{name}`"));
            }
        }
        let mut by_id = BTreeMap::new();
        for entity in &self.entities {
            if !valid_name(&entity.id) || !valid_name(&entity.layer) {
                return Err(format!("invalid entity ID or layer for `{}`", entity.id));
            }
            if by_id.insert(entity.id.as_str(), entity).is_some() {
                return Err(format!("duplicate entity ID `{}`", entity.id));
            }
            for name in entity.components.keys() {
                if !self.component_types.contains_key(name) {
                    return Err(format!(
                        "entity `{}` uses undeclared component `{name}`",
                        entity.id
                    ));
                }
            }
        }
        for entity in &self.entities {
            if let Some(parent) = &entity.parent {
                let Some(parent_entity) = by_id.get(parent.as_str()) else {
                    return Err(format!(
                        "entity `{}` references missing parent `{parent}`",
                        entity.id
                    ));
                };
                if parent_entity.layer != entity.layer {
                    return Err(format!(
                        "entity `{}` has a parent in another layer",
                        entity.id
                    ));
                }
            }
        }
        let mut completed = BTreeSet::new();
        for entity in &self.entities {
            let mut path = BTreeSet::new();
            let mut cursor = Some(entity.id.as_str());
            while let Some(id) = cursor {
                if completed.contains(id) {
                    break;
                }
                if !path.insert(id) {
                    return Err(format!("entity hierarchy contains a cycle at `{id}`"));
                }
                cursor = by_id[id].parent.as_deref();
            }
            completed.extend(path);
        }
        Ok(())
    }
}
