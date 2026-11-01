//! Validated player components exposed through ordinary ECS adapters.
use countersplat_ecs::{ComponentAdapter, Entity, Value, World};
use countersplat_scene::{
    CameraController, CameraLens, CameraZoomLens, PlayerCamera, PlayerControls, PlayerRig,
};

// Only plain value types whose changes flow through World opt into revisions.
macro_rules! tracked_revision {
    (tracked, $component:ty) => {
        fn content_revision(&self, world: &World) -> Option<u64> {
            world.component_revision::<$component>()
        }
    };
}

macro_rules! adapter {
    ($name:ident, $component:ty $(, [$($fast_field:ident),+])? $(; $tracked:ident)?) => {
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
            $(
            fn write_field(
                &self,
                world: &mut World,
                entity: Entity,
                field: &str,
                value: Value,
            ) -> Result<(), String> {
                if !matches!(field, $(stringify!($fast_field))|+) {
                    return countersplat_ecs::write_object_field_via_adapter(
                        self, world, entity, field, value,
                    );
                }
                let mut component = world
                    .get::<$component>(entity)
                    .cloned()
                    .ok_or("component is missing")?;
                match field {
                    $(stringify!($fast_field) => {
                        component.$fast_field =
                            serde_json::from_value(value).map_err(|e| e.to_string())?;
                    },)+
                    _ => unreachable!(),
                }
                component.validate().map_err(str::to_owned)?;
                world.insert(entity, component)
            }
            fn write_fields(
                &self,
                world: &mut World,
                entity: Entity,
                fields: countersplat_ecs::ComponentFields,
            ) -> Result<(), String> {
                if fields
                    .keys()
                    .any(|field| !matches!(field.as_str(), $(stringify!($fast_field))|+))
                {
                    return countersplat_ecs::write_object_fields_via_adapter(
                        self, world, entity, fields,
                    );
                }
                // Merge and validate a copy before replacing the component;
                // one bad field cannot leave a partially updated component.
                let mut component = world
                    .get::<$component>(entity)
                    .cloned()
                    .ok_or("component is missing")?;
                for (field, value) in fields {
                    match field.as_str() {
                        $(stringify!($fast_field) => {
                            component.$fast_field =
                                serde_json::from_value(value).map_err(|e| e.to_string())?;
                        },)+
                        _ => unreachable!(),
                    }
                }
                component.validate().map_err(str::to_owned)?;
                world.insert(entity, component)
            }
            )?
            fn remove(&self, world: &mut World, entity: Entity) -> Result<Option<Value>, String> {
                let old = self.read(world, entity)?;
                world.remove::<$component>(entity);
                Ok(old)
            }
            fn contains(&self, world: &World, entity: Entity) -> bool {
                world.has::<$component>(entity)
            }
            $(tracked_revision!($tracked, $component);)?
            fn indexed_entities<'a>(
                &self,
                world: &'a World,
            ) -> Option<countersplat_ecs::EntityIndex<'a>> {
                Some(world.iter_with::<$component>())
            }
        }
    };
}
adapter!(
    CameraAdapter,
    PlayerCamera,
    [
        position,
        view_offset,
        view_posture_offset_y,
        view_bob_pitch,
        view_bob_roll,
        view_turn_roll,
        yaw,
        pitch,
        roll,
        fov_degrees,
        view_fov_degrees
    ]
);
adapter!(LensAdapter, CameraLens; tracked);
adapter!(ZoomLensAdapter, CameraZoomLens; tracked);
adapter!(ControllerAdapter, CameraController);
adapter!(ControlsAdapter, PlayerControls);
adapter!(RigAdapter, PlayerRig);
pub fn register(world: &mut World) -> Result<(), String> {
    world.register_component_adapter("PlayerCamera", CameraAdapter)?;
    world.register_component_adapter("CameraLens", LensAdapter)?;
    world.register_component_adapter("CameraZoomLens", ZoomLensAdapter)?;
    world.track_component_changes::<CameraLens>();
    world.track_component_changes::<CameraZoomLens>();
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
