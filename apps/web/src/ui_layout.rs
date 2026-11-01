//! Browser layout overrides keyed by stable scene IDs, not transient ECS handles.
use countersplat_ecs::{Entity, World};
use countersplat_ui::UiElement;
use std::collections::BTreeMap;

const MAX_LAYOUT_BYTES: usize = 128 * 1024;
const MAX_ENTRIES: usize = 1024;

pub struct UiLayout {
    entities: BTreeMap<String, Entity>,
    defaults: BTreeMap<String, [f32; 2]>,
    changed: bool,
}
impl UiLayout {
    pub fn new(world: &World, entities: BTreeMap<String, Entity>) -> Self {
        let defaults = entities
            .iter()
            .filter_map(|(id, entity)| {
                world
                    .get::<UiElement>(*entity)
                    .map(|element| (id.clone(), element.drag_offset))
            })
            .collect();
        Self {
            entities,
            defaults,
            changed: false,
        }
    }
    pub fn changed(&mut self) {
        self.changed = true;
    }
    pub fn take_changed(&mut self) -> bool {
        std::mem::take(&mut self.changed)
    }
    pub fn save(&self, world: &World) -> Result<String, String> {
        let offsets: BTreeMap<_, _> = self
            .entities
            .iter()
            .filter_map(|(id, entity)| {
                let element = world.get::<UiElement>(*entity)?;
                (Some(&element.drag_offset) != self.defaults.get(id))
                    .then_some((id, element.drag_offset))
            })
            .collect();
        serde_json::to_string(&offsets).map_err(|error| error.to_string())
    }
    pub fn restore(&mut self, world: &mut World, encoded: &str) -> Result<(), String> {
        if encoded.len() > MAX_LAYOUT_BYTES {
            return Err("saved UI layout is too large".into());
        }
        let offsets: BTreeMap<String, [f32; 2]> =
            serde_json::from_str(encoded).map_err(|error| error.to_string())?;
        if offsets.len() > MAX_ENTRIES
            || offsets
                .values()
                .flatten()
                .any(|value| !value.is_finite() || value.abs() > 1_000_000.0)
        {
            return Err("saved UI layout contains invalid offsets".into());
        }
        self.reset_inner(world);
        for (id, offset) in offsets {
            if let Some(element) = self
                .entities
                .get(&id)
                .and_then(|entity| world.get_mut::<UiElement>(*entity))
            {
                if element.draggable {
                    element.drag_offset = offset;
                }
            }
        }
        self.changed = false;
        Ok(())
    }
    pub fn reset(&mut self, world: &mut World) {
        self.reset_inner(world);
        self.changed = true;
    }
    fn reset_inner(&self, world: &mut World) {
        for (id, entity) in &self.entities {
            if let (Some(element), Some(offset)) =
                (world.get_mut::<UiElement>(*entity), self.defaults.get(id))
            {
                element.drag_offset = *offset;
            }
        }
    }
}
