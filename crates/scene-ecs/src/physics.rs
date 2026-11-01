//! Validated, script-visible per-entity physics components.
use countersplat_ecs::{ComponentAdapter, Entity, Value, World};
use countersplat_scene::{PhysicsBody, Transform};

macro_rules! adapter {
    ($name:ident, $component:ty) => {
        struct $name;
        impl ComponentAdapter for $name {
            fn read(&self, world: &World, entity: Entity) -> Result<Option<Value>, String> {
                world
                    .get::<$component>(entity)
                    .map(serde_json::to_value)
                    .transpose()
                    .map_err(|e| e.to_string())
            }
            fn write_json(
                &self,
                world: &World,
                entity: Entity,
                output: &mut Vec<u8>,
            ) -> Result<bool, String> {
                let Some(value) = world.get::<$component>(entity) else {
                    return Ok(false);
                };
                serde_json::to_writer(output, value).map_err(|error| error.to_string())?;
                Ok(true)
            }
            fn write(&self, world: &mut World, entity: Entity, value: Value) -> Result<(), String> {
                let component: $component =
                    serde_json::from_value(value).map_err(|e| e.to_string())?;
                component.validate().map_err(str::to_owned)?;
                world.insert(entity, component)
            }
            fn remove(&self, world: &mut World, entity: Entity) -> Result<Option<Value>, String> {
                let previous = self.read(world, entity)?;
                world.remove::<$component>(entity);
                Ok(previous)
            }
            fn contains(&self, world: &World, entity: Entity) -> bool {
                world.has::<$component>(entity)
            }
            fn indexed_entities<'a>(
                &self,
                world: &'a World,
            ) -> Option<countersplat_ecs::EntityIndex<'a>> {
                Some(world.iter_with::<$component>())
            }
        }
    };
}
adapter!(PhysicsBodyAdapter, PhysicsBody);
adapter!(TransformAdapter, Transform);
pub fn register(world: &mut World) -> Result<(), String> {
    world.register_component_adapter("PhysicsBody", PhysicsBodyAdapter)?;
    world.register_component_adapter("Transform", TransformAdapter)
}
