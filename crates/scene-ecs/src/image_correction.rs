//! Validated ECS image-correction adapter and deterministic selection.
use countersplat_ecs::{ComponentAdapter, Entity, Value, World};
use countersplat_scene::ImageCorrection;

struct Adapter;
impl ComponentAdapter for Adapter {
    fn read(&self, world: &World, entity: Entity) -> Result<Option<Value>, String> {
        world
            .get::<ImageCorrection>(entity)
            .map(serde_json::to_value)
            .transpose()
            .map_err(|e| e.to_string())
    }
    fn write(&self, world: &mut World, entity: Entity, value: Value) -> Result<(), String> {
        let correction: ImageCorrection =
            serde_json::from_value(value).map_err(|e| e.to_string())?;
        correction.validate().map_err(str::to_owned)?;
        world.insert(entity, correction)
    }
    fn remove(&self, world: &mut World, entity: Entity) -> Result<Option<Value>, String> {
        let old = self.read(world, entity)?;
        world.remove::<ImageCorrection>(entity);
        Ok(old)
    }
    fn contains(&self, world: &World, entity: Entity) -> bool {
        world.has::<ImageCorrection>(entity)
    }
}
pub fn register(world: &mut World) -> Result<(), String> {
    world.register_component_adapter("ImageCorrection", Adapter)
}
/// Select an owned, valid grade without sharing an ECS world with the render thread.
pub fn extract(world: &World) -> Option<ImageCorrection> {
    world
        .component_view::<ImageCorrection>()?
        .iter()
        .filter(|(_, grade)| grade.enabled && grade.validate().is_ok())
        .max_by(|(a_id, a), (b_id, b)| a.priority.cmp(&b.priority).then_with(|| b_id.cmp(a_id)))
        .map(|(_, grade)| grade.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selection_and_invalid_script_writes_are_safe() {
        let mut world = World::new();
        register(&mut world).unwrap();
        let first = world.spawn();
        let second = world.spawn();
        let a = ImageCorrection::default();
        let mut b = a.clone();
        b.priority = 10;
        world.insert(first, a.clone()).unwrap();
        world.insert(second, b.clone()).unwrap();
        assert_eq!(extract(&world), Some(b.clone()));
        b.enabled = false;
        world.insert(second, b).unwrap();
        assert_eq!(extract(&world), Some(a.clone()));
        let mut invalid = serde_json::to_value(&a).unwrap();
        invalid["contrast"] = serde_json::json!(-1.0);
        assert!(world
            .write_component_value(first, "ImageCorrection", invalid)
            .is_err());
        assert_eq!(world.get::<ImageCorrection>(first), Some(&a));
    }
}
