//! Adapters for the voxel settings and its host-owned status.
use countersplat_ecs::{ComponentAdapter, Entity, Value, World};
use countersplat_scene::{VoxelCollider, VoxelColliderState};
macro_rules! adapter {
    ($name:ident, $ty:ty) => {
        struct $name;
        impl ComponentAdapter for $name {
            fn read(&self, world: &World, entity: Entity) -> Result<Option<Value>, String> {
                world
                    .get::<$ty>(entity)
                    .map(serde_json::to_value)
                    .transpose()
                    .map_err(|e| e.to_string())
            }
            fn write(&self, world: &mut World, entity: Entity, value: Value) -> Result<(), String> {
                let value: $ty = serde_json::from_value(value).map_err(|e| e.to_string())?;
                value.validate().map_err(str::to_owned)?;
                world.insert(entity, value)
            }
            fn remove(&self, world: &mut World, entity: Entity) -> Result<Option<Value>, String> {
                let old = self.read(world, entity)?;
                world.remove::<$ty>(entity);
                Ok(old)
            }
            fn contains(&self, world: &World, entity: Entity) -> bool {
                world.has::<$ty>(entity)
            }
        }
    };
}
adapter!(ColliderAdapter, VoxelCollider);
adapter!(StateAdapter, VoxelColliderState);
pub fn register(world: &mut World) -> Result<(), String> {
    world.register_component_adapter("VoxelCollider", ColliderAdapter)?;
    world.register_component_adapter("VoxelColliderState", StateAdapter)
}
