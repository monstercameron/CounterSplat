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
            fn write_json(
                &self,
                world: &World,
                entity: Entity,
                output: &mut Vec<u8>,
            ) -> Result<bool, String> {
                let Some(value) = world.get::<$ty>(entity) else {
                    return Ok(false);
                };
                serde_json::to_writer(output, value).map_err(|error| error.to_string())?;
                Ok(true)
            }
            fn content_revision(&self, world: &World) -> Option<u64> {
                world.component_revision::<$ty>()
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
    world.register_component_adapter("VoxelColliderState", StateAdapter)?;
    // These plain values change only through the world and are safe to cache.
    world.track_component_changes::<VoxelCollider>();
    world.track_component_changes::<VoxelColliderState>();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn voxel_revisions_follow_validated_edits_and_host_updates() {
        let mut world = World::new();
        register(&mut world).unwrap();
        let entity = world.spawn();
        world.insert(entity, VoxelCollider::default()).unwrap();
        world.insert(entity, VoxelColliderState::default()).unwrap();
        let old = world
            .read_component_if_changed(entity, "VoxelColliderState", None)
            .unwrap()
            .unwrap()
            .revision
            .unwrap();
        assert!(world
            .write_component_field(entity, "VoxelColliderState", "status", json!("ready"))
            .is_err());
        assert!(world
            .read_component_if_changed(entity, "VoxelColliderState", Some(&old))
            .unwrap()
            .is_none());
        world
            .write_component_fields(
                entity,
                "VoxelColliderState",
                json!({"status":"ready", "resolution":0.05})
                    .as_object()
                    .unwrap()
                    .clone(),
            )
            .unwrap();
        let changed = world
            .read_component_if_changed(entity, "VoxelColliderState", Some(&old))
            .unwrap()
            .unwrap();
        assert_eq!(changed.value["status"], "ready");
        let revision = changed.revision.unwrap();
        world
            .get_mut::<VoxelColliderState>(entity)
            .unwrap()
            .visible_voxels = 42;
        assert_eq!(
            world
                .read_component_if_changed(entity, "VoxelColliderState", Some(&revision))
                .unwrap()
                .unwrap()
                .value["visible_voxels"],
            42
        );
    }
}
