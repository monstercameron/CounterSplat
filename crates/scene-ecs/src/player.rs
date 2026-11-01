//! Validated player components exposed through ordinary ECS adapters.
use countersplat_ecs::{ComponentAdapter, Entity, Value, World};
use countersplat_scene::{
    CameraController, CameraLens, CameraZoomLens, PlayerCamera, PlayerControls, PlayerRig,
};

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
            fn indexed_entities<'a>(
                &self,
                world: &'a World,
            ) -> Option<countersplat_ecs::EntityIndex<'a>> {
                Some(world.iter_with::<$component>())
            }
        }
    };
}
adapter!(CameraAdapter, PlayerCamera);
adapter!(LensAdapter, CameraLens);
adapter!(ZoomLensAdapter, CameraZoomLens);
adapter!(ControllerAdapter, CameraController);
adapter!(ControlsAdapter, PlayerControls);
adapter!(RigAdapter, PlayerRig);
pub fn register(world: &mut World) -> Result<(), String> {
    world.register_component_adapter("PlayerCamera", CameraAdapter)?;
    world.register_component_adapter("CameraLens", LensAdapter)?;
    world.register_component_adapter("CameraZoomLens", ZoomLensAdapter)?;
    world.register_component_adapter("CameraController", ControllerAdapter)?;
    world.register_component_adapter("PlayerControls", ControlsAdapter)?;
    world.register_component_adapter("PlayerRig", RigAdapter)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn zoom_lens_is_independent_and_validated_through_the_ecs_bridge() {
        let mut world = World::new();
        register(&mut world).unwrap();
        let entity = world.spawn();
        world.insert(entity, CameraLens::default()).unwrap();
        world.insert(entity, CameraZoomLens::default()).unwrap();
        let mut zoom = world
            .read_component_value(entity, "CameraZoomLens")
            .unwrap()
            .unwrap();
        zoom["aperture_f_number"] = json!(8.0);
        world
            .write_component_value(entity, "CameraZoomLens", zoom.clone())
            .unwrap();
        assert_eq!(
            world.get::<CameraLens>(entity).unwrap().aperture_f_number,
            5.6
        );
        assert_eq!(
            world
                .get::<CameraZoomLens>(entity)
                .unwrap()
                .0
                .aperture_f_number,
            8.0
        );
        zoom["aperture_f_number"] = json!(0.0);
        assert!(world
            .write_component_value(entity, "CameraZoomLens", zoom)
            .is_err());
        assert_eq!(
            world
                .get::<CameraZoomLens>(entity)
                .unwrap()
                .0
                .aperture_f_number,
            8.0
        );
    }
}
