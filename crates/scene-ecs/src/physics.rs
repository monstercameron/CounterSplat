//! Validated, script-visible per-entity physics components.
use countersplat_ecs::{ComponentAdapter, Entity, Value, World};
use countersplat_scene::{PhysicsBody, Transform};

macro_rules! adapter {
    ($name:ident, $component:ty $(, [$($fast_field:ident),+])?) => {
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
adapter!(PhysicsBodyAdapter, PhysicsBody, [velocity, grounded]);
adapter!(TransformAdapter, Transform, [position]);
pub fn register(world: &mut World) -> Result<(), String> {
    world.register_component_adapter("PhysicsBody", PhysicsBodyAdapter)?;
    world.register_component_adapter("Transform", TransformAdapter)
}
