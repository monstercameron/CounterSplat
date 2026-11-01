//! Validated player components exposed through ordinary ECS adapters.
use countersplat_ecs::{ComponentAdapter, Entity, Value, World};
use countersplat_scene::{PlayerCamera, PlayerControls};

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
            fn write(&self, world: &mut World, entity: Entity, value: Value) -> Result<(), String> {
                let component: $component =
                    serde_json::from_value(value).map_err(|e| e.to_string())?;
                component.validate().map_err(str::to_owned)?;
                world.insert(entity, component)
            }
            fn remove(&self, world: &mut World, entity: Entity) -> Result<Option<Value>, String> {
                let old = self.read(world, entity)?;
                world.remove::<$component>(entity);
                Ok(old)
            }
            fn contains(&self, world: &World, entity: Entity) -> bool {
                world.has::<$component>(entity)
            }
        }
    };
}
adapter!(CameraAdapter, PlayerCamera);
adapter!(ControlsAdapter, PlayerControls);
pub fn register(world: &mut World) -> Result<(), String> {
    world.register_component_adapter("PlayerCamera", CameraAdapter)?;
    world.register_component_adapter("PlayerControls", ControlsAdapter)
}
