//! Asset-loading policy for a host: which assets preload or stream, the order scene scripts run in, monotonic
//! loading progress, and the portable inventory of a level's files. URL resolution is the host's (it must follow the
//! browser's URL rules); everything here is data and is tested natively.

use serde_json::{json, Value};

/// `preload` (the default) or `streamable`.
pub fn load_policy(value: Option<&Value>) -> Result<&'static str, String> {
    match value {
        None | Some(Value::Null) => Ok("preload"),
        Some(Value::String(s)) if s == "preload" => Ok("preload"),
        Some(Value::String(s)) if s == "streamable" => Ok("streamable"),
        Some(other) => Err(format!(
            "Unknown asset load policy: {}",
            other.as_str().map_or_else(|| other.to_string(), str::to_string)
        )),
    }
}

/// A level's load policies. The manifest, viewer settings and chunk metadata must preload; splat chunks stream by
/// default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LevelPolicies {
    pub manifest: &'static str,
    pub viewer_settings: &'static str,
    pub chunk_metadata: &'static str,
    pub splat_chunks: &'static str,
}

pub fn level_policies(value: Option<&Value>) -> Result<LevelPolicies, String> {
    let empty = json!({});
    let value = match value {
        None | Some(Value::Null) => &empty,
        Some(v) if v.is_object() => v,
        Some(_) => return Err("Invalid level asset policies.".into()),
    };
    let policies = LevelPolicies {
        manifest: load_policy(value.get("manifest"))?,
        viewer_settings: load_policy(value.get("viewer_settings"))?,
        chunk_metadata: load_policy(value.get("chunk_metadata"))?,
        splat_chunks: match value.get("splat_chunks") {
            None | Some(Value::Null) => "streamable",
            some => load_policy(some)?,
        },
    };
    if policies.manifest != "preload" || policies.viewer_settings != "preload" || policies.chunk_metadata != "preload" {
        return Err("The level manifest, viewer settings, and chunk metadata must be preloaded.".into());
    }
    Ok(policies)
}

/// A scene script to fetch: its id, its URI relative to the scene, and whether it runs at load.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptPlan {
    pub id: String,
    pub uri: String,
    pub execute: bool,
}

/// The scene's preloaded Lua scripts. Scripts listed in `resources.preview.browser_scripts` run in that order; other
/// autoload scripts follow in catalog order; no script runs twice. Streamable scripts are skipped.
pub fn plan_scripts(scene: &Value) -> Result<Vec<ScriptPlan>, String> {
    let empty = serde_json::Map::new();
    let scripts = scene.pointer("/resources/scripts").and_then(Value::as_object).unwrap_or(&empty);
    let explicit: Vec<&str> = match scene.pointer("/resources/preview/browser_scripts") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(ids)) => ids
            .iter()
            .map(|id| id.as_str().filter(|id| scripts.contains_key(*id)))
            .collect::<Option<Vec<_>>>()
            .ok_or("Invalid browser script references.")?,
        Some(_) => return Err("Invalid browser script references.".into()),
    };
    let mut ordered: Vec<&str> = Vec::new();
    for id in explicit.iter().copied().chain(scripts.keys().map(String::as_str)) {
        if !ordered.contains(&id) {
            ordered.push(id);
        }
    }
    let mut plan = Vec::new();
    for id in ordered {
        let spec = &scripts[id];
        let execute = spec.get("autoload").and_then(Value::as_bool).unwrap_or(false) || explicit.contains(&id);
        let policy = load_policy(spec.get("load"))?;
        if execute && policy != "preload" {
            return Err(format!("Required script {id} must be preloaded."));
        }
        if policy == "streamable" {
            continue;
        }
        if spec.get("language").and_then(Value::as_str) != Some("lua") {
            return Err(format!("Script {id} is not Lua."));
        }
        let uri = spec.get("uri").and_then(Value::as_str).ok_or(format!("Script {id} has no URI."))?;
        plan.push(ScriptPlan { id: id.to_string(), uri: uri.to_string(), execute });
    }
    Ok(plan)
}

/// Monotonic overall loading progress across phases, keeping each phase's real counts.
#[derive(Debug, Clone, Copy, Default)]
pub struct LoadingProgress {
    pub progress: f64,
}

impl LoadingProgress {
    /// The phase spans `start..end` of the bar; returns the overall progress.
    pub fn update(&mut self, phase: &str, start: f64, end: f64, completed: u64, total: u64) -> Result<f64, String> {
        if phase.len() > 256
            || !start.is_finite()
            || !end.is_finite()
            || start < 0.0
            || end > 1.0
            || end < start
            || total < 1
            || completed > total
        {
            return Err("Invalid asset-loading progress.".into());
        }
        self.progress = self.progress.max(start + (end - start) * completed as f64 / total as f64);
        Ok(self.progress)
    }
}

/// A relative asset path with no root, drive, backslash, control characters, or empty, `.` or `..` segments.
pub fn safe_relative_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 1024
        && !path.starts_with('/')
        && !path.contains('\\')
        && !path.contains(':')
        && !path.chars().any(|c| c.is_control())
        && path.split('/').all(|part| !part.is_empty() && part != "." && part != "..")
}

/// The portable inventory of a level's files from its chunk metadata and, when present, its download inventory:
/// references only (path, kind, load policy).
#[allow(clippy::too_many_arguments)]
pub fn describe_level_assets(
    level: &Value,
    lod_count: u32,
    chunk_metadata: &[Value],
    filenames: &[String],
    viewer_settings: bool,
    inventory: Option<&Value>,
    has_source: bool,
    source: &str,
) -> Result<Value, String> {
    if !(1..=64).contains(&lod_count) || filenames.len() != chunk_metadata.len() {
        return Err("Invalid level asset metadata.".into());
    }
    let policies = level_policies(level.get("assets"))?;
    let mut files: Vec<Value> = Vec::new();
    let mut add = |path: &str, kind: &str, load: &str| -> Result<(), String> {
        if !safe_relative_path(path) {
            return Err("A level asset has an invalid path.".into());
        }
        if !files.iter().any(|f| f["path"] == path) {
            files.push(json!({"path": path, "kind": kind, "load": load}));
        }
        Ok(())
    };
    let format = level.get("format").and_then(Value::as_str);
    let single = format == Some("single_sog");
    if !single && format != Some("streamed_sog") {
        return Err("Unsupported level format.".into());
    }
    add(if single { "meta.json" } else { "lod-meta.json" }, "manifest", policies.manifest)?;
    if viewer_settings {
        add("viewer-settings.json", "viewer_settings", policies.viewer_settings)?;
    }
    for (index, meta) in chunk_metadata.iter().enumerate() {
        let metadata_path = if single { "meta.json" } else { filenames[index].as_str() };
        if !single {
            add(metadata_path, "chunk_metadata", policies.chunk_metadata)?;
        }
        let folder = if single {
            String::new()
        } else {
            let parts: Vec<&str> = metadata_path.split('/').collect();
            format!("{}/", parts[..parts.len() - 1].join("/"))
        };
        if !meta.is_object() {
            return Err("Invalid chunk metadata.".into());
        }
        for name in ["means", "quats", "scales", "sh0", "shN"] {
            let references = match meta.get(name).and_then(|m| m.get("files")) {
                None | Some(Value::Null) => continue,
                Some(Value::Array(list)) => list,
                Some(_) => return Err("Invalid chunk asset list.".into()),
            };
            for path in references {
                let path = path.as_str().ok_or("Invalid chunk asset path.")?;
                add(&format!("{folder}{path}"), "splat_data", policies.splat_chunks)?;
            }
        }
    }
    if let Some(inventory) = inventory {
        let entries = inventory.get("files").and_then(Value::as_array);
        if inventory.get("scene_id") != level.get("id") || entries.is_none() {
            return Err("Level download inventory does not match the selected level.".into());
        }
        for item in entries.into_iter().flatten() {
            let path = item.get("path").and_then(Value::as_str).ok_or("Invalid level inventory entry.")?;
            let is_collision = path.split('/').next_back().is_some_and(|name| name.starts_with("scene.voxel."));
            add(path, if is_collision { "collision" } else { "other" }, "streamable")?;
        }
        add("download-inventory.json", "other", "preload")?;
    }
    if has_source {
        add("source.json", "other", "preload")?;
    }
    Ok(json!({"source": source, "level_id": level.get("id"), "name": level.get("name"), "format": format,
        "lod_count": lod_count, "files": files}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policies_scripts_and_progress() {
        assert_eq!(load_policy(None), Ok("preload"));
        assert_eq!(level_policies(None).unwrap().splat_chunks, "streamable");
        assert_eq!(level_policies(Some(&json!({"splat_chunks":"preload"}))).unwrap().splat_chunks, "preload");
        assert!(level_policies(Some(&json!({"chunk_metadata":"streamable"}))).is_err());
        assert!(load_policy(Some(&json!("unknown"))).is_err());
        let scene = json!({"resources":{"scripts":{"main":{"language":"lua","uri":"../scripts/main.lua","load":"preload","autoload":true},
            "optional":{"language":"lua","uri":"../scripts/optional.lua","load":"streamable"}}}});
        assert_eq!(plan_scripts(&scene).unwrap().iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["main"]);
        assert!(plan_scripts(&json!({"resources":{"scripts":{"bad":{"language":"lua","uri":"x","load":"streamable","autoload":true}}}})).is_err());
        let ordered = json!({"resources":{"scripts":{"late":{"language":"lua","uri":"late.lua","load":"preload"},
            "early":{"language":"lua","uri":"early.lua","load":"preload"},"auto":{"language":"lua","uri":"auto.lua","load":"preload","autoload":true}},
            "preview":{"browser_scripts":["early","late","early"]}}});
        let executed: Vec<String> = plan_scripts(&ordered).unwrap().into_iter().filter(|s| s.execute).map(|s| s.id).collect();
        assert_eq!(executed, ["early", "late", "auto"]);
        let mut tracker = LoadingProgress::default();
        assert!((tracker.update("Metadata", 0.1, 0.5, 1, 2).unwrap() - 0.3).abs() < 1e-9);
        assert_eq!(tracker.update("Metadata", 0.1, 0.5, 2, 2).unwrap(), 0.5);
        assert_eq!(tracker.update("Cached", 0.0, 0.1, 1, 1).unwrap(), 0.5);
        assert!(tracker.update("Bad", 0.0, 1.0, 2, 1).is_err());
    }

    #[test]
    fn level_inventory() {
        let example = json!({"id":"1234abcd","name":"Example","format":"streamed_sog","assets":{"splat_chunks":"streamable"}});
        let inventory = json!({"scene_id":"1234abcd","files":[{"path":"lod-meta.json"},{"path":"scene.voxel.bin"}]});
        let metas = [json!({"means":{"files":["means_l.webp","means_u.webp"]}})];
        let names = ["0_0/meta.json".to_string()];
        let described = describe_level_assets(&example, 2, &metas, &names, true, Some(&inventory), true, "level").unwrap();
        assert_eq!(described["lod_count"], 2);
        let files = described["files"].as_array().unwrap();
        assert!(files.contains(&json!({"path":"0_0/means_l.webp","kind":"splat_data","load":"streamable"})));
        assert_eq!(files.iter().find(|f| f["path"] == "scene.voxel.bin").unwrap()["kind"], "collision");
        assert_eq!(files.iter().filter(|f| f["path"] == "lod-meta.json").count(), 1);
        assert!(describe_level_assets(&example, 2, &[json!({"means":{"files":["../secret"]}})], &names, false, None, false, "level").is_err());
        assert!(describe_level_assets(&example, 2, &[json!({"means":{"files":[null]}})], &names, false, None, false, "level").is_err());
        assert!(safe_relative_path("0_0/meta.json") && !safe_relative_path("/x") && !safe_relative_path("a//b"));
    }
}
