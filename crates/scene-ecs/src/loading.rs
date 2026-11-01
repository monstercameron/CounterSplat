//! Validated ECS adapter for host-owned asset loading state.
use countersplat_ecs::{ComponentAdapter, Entity, Value, World};
use countersplat_scene::AssetLoader;
struct LoaderAdapter;
impl ComponentAdapter for LoaderAdapter {
    fn read(&self, world: &World, entity: Entity) -> Result<Option<Value>, String> {
        world
            .get::<AssetLoader>(entity)
            .map(serde_json::to_value)
            .transpose()
            .map_err(|e| e.to_string())
    }
    fn write(&self, world: &mut World, entity: Entity, value: Value) -> Result<(), String> {
        let state: AssetLoader = serde_json::from_value(value).map_err(|e| e.to_string())?;
        state.validate().map_err(str::to_owned)?;
        world.insert(entity, state)
    }
    fn remove(&self, world: &mut World, entity: Entity) -> Result<Option<Value>, String> {
        let old = self.read(world, entity)?;
        world.remove::<AssetLoader>(entity);
        Ok(old)
    }
    fn contains(&self, world: &World, entity: Entity) -> bool {
        world.has::<AssetLoader>(entity)
    }
}
pub fn register(world: &mut World) -> Result<(), String> {
    world.register_component_adapter("AssetLoader", LoaderAdapter)
}
