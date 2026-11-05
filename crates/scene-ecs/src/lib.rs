//! Script-neutral ECS integration for scene layers.
//!
//! The ECS owner extracts owned snapshots; GPU resources and asset I/O remain in the host.
mod image_correction;
mod level;
mod loading;
mod physics;
mod player;
mod voxel;

use countersplat_ecs::{ComponentAdapter, Entity, Value, World};
use countersplat_scene::{
    weapon_profile, ComponentStorage, LevelAssets, SceneManifest, Skybox, SkyboxSnapshot,
};
use serde::Deserialize;
use std::collections::BTreeMap;

pub const SKYBOX_COMPONENT: &str = "Skybox";
struct SkyboxAdapter;
impl ComponentAdapter for SkyboxAdapter {
    fn read(&self, world: &World, entity: Entity) -> Result<Option<Value>, String> {
        world
            .get::<Skybox>(entity)
            .map(serde_json::to_value)
            .transpose()
            .map_err(|e| e.to_string())
    }
    fn write(&self, world: &mut World, entity: Entity, value: Value) -> Result<(), String> {
        let sky: Skybox = serde_json::from_value(value).map_err(|e| e.to_string())?;
        sky.validate().map_err(str::to_owned)?;
        world.insert(entity, sky)
    }
    fn remove(&self, world: &mut World, entity: Entity) -> Result<Option<Value>, String> {
        let previous = self.read(world, entity)?;
        world.remove::<Skybox>(entity);
        Ok(previous)
    }
    fn contains(&self, world: &World, entity: Entity) -> bool {
        world.has::<Skybox>(entity)
    }
}

/// Expose validated `Skybox` reads/writes through the generic ECS and Lua bridge.
pub fn register(world: &mut World) -> Result<(), String> {
    world.register_component_adapter(SKYBOX_COMPONENT, SkyboxAdapter)?;
    image_correction::register(world)?;
    player::register(world)?;
    physics::register(world)?;
    level::register(world)?;
    voxel::register(world)?;
    loading::register(world)
}

/// Select the highest-priority enabled, valid scene color grade.
pub use image_correction::extract as extract_image_correction;

/// Create a validated skybox entity on the world owner.
pub fn spawn(world: &mut World, skybox: Skybox) -> Result<Entity, String> {
    skybox.validate().map_err(str::to_owned)?;
    let entity = world.spawn();
    if let Err(error) = world.insert(entity, skybox) {
        world.despawn(entity);
        return Err(error);
    }
    Ok(entity)
}

/// Select the enabled, valid skybox with highest priority. Ties prefer the lowest entity ID.
/// Invalid components inserted directly through Rust are ignored. The result owns its data.
pub fn extract(world: &World) -> Option<SkyboxSnapshot> {
    world
        .component_view::<Skybox>()?
        .iter()
        .filter(|(_, sky)| sky.enabled && sky.validate().is_ok())
        .max_by(|(a_id, a), (b_id, b)| a.priority.cmp(&b.priority).then_with(|| b_id.cmp(a_id)))
        .map(|(entity, skybox)| SkyboxSnapshot {
            entity_id: entity.to_string(),
            skybox: skybox.clone(),
        })
}

/// Instantiate one manifest layer on the world owner and return stable scene IDs mapped to runtime handles.
///
/// All registered native adapters must already exist. Failed writes or parent assignments despawn
/// every entity created by this call. Resources are published only after all entities succeed.
/// Existing resources are never overwritten. Adapters should not perform unrelated side effects.
pub fn instantiate_layer(
    world: &mut World,
    manifest: &SceneManifest,
    layer: &str,
) -> Result<BTreeMap<String, Entity>, String> {
    instantiate_layers(world, manifest, &[layer])
}

/// Instantiate several independent manifest layers into one ECS world in a single transaction.
/// Stable IDs from all selected layers share one `scene.entities` resource.
pub fn instantiate_layers(
    world: &mut World,
    manifest: &SceneManifest,
    layers: &[&str],
) -> Result<BTreeMap<String, Entity>, String> {
    manifest.validate()?;
    let definitions: Vec<_> = manifest
        .entities
        .iter()
        .filter(|item| layers.contains(&item.layer.as_str()))
        .collect();
    let registered = world.registered_component_names();
    for item in &definitions {
        for (name, value) in &item.components {
            let native = registered.contains(&name.as_str());
            let storage = manifest.component_types[name].storage;
            if (storage == ComponentStorage::Native) != native {
                return Err(format!(
                    "entity `{}` component `{name}`: declared {storage:?}, adapter {}",
                    item.id,
                    if native {
                        "is registered"
                    } else {
                        "is not registered"
                    }
                ));
            }
            if name == "LevelAssets" {
                let level = LevelAssets::deserialize(value)
                    .map_err(|error| format!("entity `{}` LevelAssets: {error}", item.id))?;
                level
                    .validate()
                    .map_err(|error| format!("entity `{}` LevelAssets: {error}", item.id))?;
                if !manifest.resources.assets.contains_key(&level.source) {
                    return Err(format!(
                        "entity `{}` references missing level asset `{}`",
                        item.id, level.source
                    ));
                }
            }
            if name == SKYBOX_COMPONENT {
                let sky = Skybox::deserialize(value)
                    .map_err(|error| format!("entity `{}` Skybox: {error}", item.id))?;
                sky.validate()
                    .map_err(|error| format!("entity `{}` Skybox: {error}", item.id))?;
                if !manifest.resources.assets.contains_key(&sky.asset) {
                    return Err(format!(
                        "entity `{}` references missing skybox asset `{}`",
                        item.id, sky.asset
                    ));
                }
            }
        }
    }
    // Weapon profiles: every entity with a `WeaponProfile` gets its profile's resolved component values merged
    // over its authored components, and `weapon_stats` publishes the physics of every profile in the scene.
    let (profiles, tuning) = weapon_profile::scene_profiles(&manifest.resources.values)?;
    let mut weapon_overrides: BTreeMap<&str, BTreeMap<String, serde_json::Value>> = BTreeMap::new();
    for item in &definitions {
        if let Some(selection) = item.components.get("WeaponProfile") {
            let id = weapon_profile::selected_profile(selection)
                .map_err(|error| format!("entity `{}`: {error}", item.id))?;
            let profile = profiles
                .get(id)
                .ok_or_else(|| format!("entity `{}` selects unknown weapon profile `{id}`", item.id))?;
            let resolved = weapon_profile::resolve(profile, &tuning)
                .map_err(|error| format!("entity `{}` weapon profile `{id}`: {error}", item.id))?;
            weapon_overrides.insert(&item.id, resolved.components);
        }
    }
    let mut resources = manifest.resources.values.clone();
    if !profiles.is_empty() {
        let mut stats = serde_json::Map::new();
        for (id, profile) in &profiles {
            let resolved = weapon_profile::resolve(profile, &tuning)
                .map_err(|error| format!("weapon profile `{id}`: {error}"))?;
            stats.insert(id.clone(), serde_json::to_value(&resolved.stats).map_err(|e| e.to_string())?);
        }
        resources.insert("weapon_stats".into(), serde_json::Value::Object(stats));
    }
    resources.insert(
        "scene.assets".into(),
        serde_json::to_value(&manifest.resources.assets).map_err(|e| e.to_string())?,
    );
    resources.insert(
        "scene.scripts".into(),
        serde_json::to_value(&manifest.resources.scripts).map_err(|e| e.to_string())?,
    );
    for name in resources
        .keys()
        .chain(std::iter::once(&"scene.entities".to_owned()))
    {
        if world.get_dynamic_resource(name).is_some() {
            return Err(format!(
                "scene resource `{name}` already exists in this world"
            ));
        }
    }
    let entities: BTreeMap<_, _> = definitions
        .iter()
        .map(|item| (item.id.clone(), world.spawn()))
        .collect();
    let result: Result<(), String> = (|| {
        for item in &definitions {
            let entity = entities[&item.id];
            let overrides = weapon_overrides.get(item.id.as_str());
            for (name, value) in &item.components {
                let value = match overrides.and_then(|o| o.get(name)) {
                    Some(patch) => weapon_profile::merge_fields(Some(value), patch),
                    None => value.clone(),
                };
                world
                    .write_component_value(entity, name, value)
                    .map_err(|error| format!("entity `{}` component `{name}`: {error}", item.id))?;
            }
            // Profile components the entity did not author (its derived `WeaponStats`, for one).
            for (name, patch) in overrides.into_iter().flatten() {
                if !item.components.contains_key(name) {
                    world
                        .write_component_value(entity, name, patch.clone())
                        .map_err(|error| format!("entity `{}` component `{name}`: {error}", item.id))?;
                }
            }
            if let Some(parent) = &item.parent {
                world
                    .set_parent(entity, Some(entities[parent]))
                    .map_err(|error| format!("entity `{}` parent: {error}", item.id))?;
            }
        }
        Ok(())
    })();
    if let Err(error) = result {
        for entity in entities.values() {
            world.despawn(*entity);
        }
        return Err(error);
    }
    let ids: BTreeMap<_, _> = entities
        .iter()
        .map(|(name, entity)| (name, entity.to_string()))
        .collect();
    resources.insert(
        "scene.entities".into(),
        serde_json::to_value(ids).map_err(|e| e.to_string())?,
    );
    for (name, value) in resources {
        world.set_dynamic_resource(&name, value);
    }
    Ok(entities)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn selection_is_deterministic_and_respects_visibility_and_priority() {
        let mut world = World::new();
        let first = spawn(&mut world, Skybox::new("first").unwrap()).unwrap();
        let second = spawn(&mut world, Skybox::new("second").unwrap()).unwrap();
        assert_eq!(extract(&world).unwrap().entity_id, first.to_string());
        world.get_mut::<Skybox>(second).unwrap().priority = 5;
        assert_eq!(extract(&world).unwrap().entity_id, second.to_string());
        world.get_mut::<Skybox>(second).unwrap().enabled = false;
        assert_eq!(extract(&world).unwrap().entity_id, first.to_string());
        world.despawn(first);
        assert!(extract(&world).is_none());
    }
    #[test]
    fn script_adapter_rejects_invalid_writes_without_mutating_the_old_value() {
        let mut world = World::new();
        register(&mut world).unwrap();
        let entity = spawn(&mut world, Skybox::new("day").unwrap()).unwrap();
        let original = world
            .read_component_value(entity, SKYBOX_COMPONENT)
            .unwrap()
            .unwrap();
        let mut invalid = original.clone();
        invalid["exposure"] = json!(-1);
        assert!(world
            .write_component_value(entity, SKYBOX_COMPONENT, invalid)
            .is_err());
        assert_eq!(
            world
                .read_component_value(entity, SKYBOX_COMPONENT)
                .unwrap()
                .unwrap(),
            original
        );
    }
}
