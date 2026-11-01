//! Validated, script-visible level metadata and settings adapters.
use countersplat_ecs::{ComponentAdapter, Entity, Value, World};
use countersplat_scene::{LevelAssets, LevelSettings};

struct AssetsAdapter;
impl ComponentAdapter for AssetsAdapter {
    fn read(&self, world: &World, entity: Entity) -> Result<Option<Value>, String> {
        world
            .get::<LevelAssets>(entity)
            .map(serde_json::to_value)
            .transpose()
            .map_err(|e| e.to_string())
    }
    fn write(&self, world: &mut World, entity: Entity, value: Value) -> Result<(), String> {
        let assets: LevelAssets = serde_json::from_value(value).map_err(|e| e.to_string())?;
        assets.validate().map_err(str::to_owned)?;
        if let Some(settings) = world.get::<LevelSettings>(entity) {
            settings.validate_for(&assets).map_err(str::to_owned)?;
        }
        world.insert(entity, assets)
    }
    fn remove(&self, world: &mut World, entity: Entity) -> Result<Option<Value>, String> {
        let old = self.read(world, entity)?;
        world.remove::<LevelAssets>(entity);
        Ok(old)
    }
    fn contains(&self, world: &World, entity: Entity) -> bool {
        world.has::<LevelAssets>(entity)
    }
}
struct SettingsAdapter;
impl ComponentAdapter for SettingsAdapter {
    fn read(&self, world: &World, entity: Entity) -> Result<Option<Value>, String> {
        world
            .get::<LevelSettings>(entity)
            .map(serde_json::to_value)
            .transpose()
            .map_err(|e| e.to_string())
    }
    fn write(&self, world: &mut World, entity: Entity, value: Value) -> Result<(), String> {
        let settings: LevelSettings = serde_json::from_value(value).map_err(|e| e.to_string())?;
        settings.validate().map_err(str::to_owned)?;
        if let Some(assets) = world.get::<LevelAssets>(entity) {
            settings.validate_for(assets).map_err(str::to_owned)?;
        }
        world.insert(entity, settings)
    }
    fn remove(&self, world: &mut World, entity: Entity) -> Result<Option<Value>, String> {
        let old = self.read(world, entity)?;
        world.remove::<LevelSettings>(entity);
        Ok(old)
    }
    fn contains(&self, world: &World, entity: Entity) -> bool {
        world.has::<LevelSettings>(entity)
    }
}
pub fn register(world: &mut World) -> Result<(), String> {
    world.register_component_adapter("LevelAssets", AssetsAdapter)?;
    world.register_component_adapter("LevelSettings", SettingsAdapter)
}
