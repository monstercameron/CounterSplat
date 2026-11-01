//! Serializable UI layout overrides keyed by stable scene IDs, not transient ECS handles.
use countersplat_ecs::{Entity, World};
use countersplat_ui::UiElement;
use std::collections::BTreeMap;

const MAX_LAYOUT_BYTES: usize = 128 * 1024;
const MAX_ENTRIES: usize = 1024;

pub struct UiLayout {
    entities: BTreeMap<String, Entity>,
    defaults: BTreeMap<String, ([f32; 2], Option<[f32; 2]>)>,
    changed: bool,
}
impl UiLayout {
    pub fn new(world: &World, entities: BTreeMap<String, Entity>) -> Self {
        let defaults = entities
            .iter()
            .filter_map(|(id, entity)| {
                world.get::<UiElement>(*entity).map(|element| {
                    (
                        id.clone(),
                        (
                            element.drag_offset,
                            element
                                .resize
                                .as_ref()
                                .map(|_| [element.rect.width, element.rect.height]),
                        ),
                    )
                })
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
        let mut offsets = BTreeMap::new();
        let mut sizes = BTreeMap::new();
        for (id, entity) in &self.entities {
            let (Some(element), Some(default)) =
                (world.get::<UiElement>(*entity), self.defaults.get(id))
            else {
                continue;
            };
            if element.drag_offset != default.0 {
                offsets.insert(id.clone(), element.drag_offset);
            }
            if element.resize.is_some() {
                let size = [element.rect.width, element.rect.height];
                if Some(size) != default.1 {
                    sizes.insert(id.clone(), size);
                }
            }
        }
        serde_json::to_string(
            &serde_json::json!({ "version": 2, "offsets": offsets, "sizes": sizes }),
        )
        .map_err(|error| error.to_string())
    }

    pub fn restore(&mut self, world: &mut World, encoded: &str) -> Result<(), String> {
        if encoded.len() > MAX_LAYOUT_BYTES {
            return Err("saved UI layout is too large".into());
        }
        let saved: serde_json::Value =
            serde_json::from_str(encoded).map_err(|error| error.to_string())?;
        let (offsets, sizes): (BTreeMap<String, [f32; 2]>, BTreeMap<String, [f32; 2]>) =
            if let Some(version) = saved.get("version").filter(|value| value.is_number()) {
                if version.as_u64() != Some(2) {
                    return Err("unsupported UI layout version".into());
                }
                let offsets = serde_json::from_value(
                    saved.get("offsets").cloned().ok_or("missing UI offsets")?,
                )
                .map_err(|error| error.to_string())?;
                let sizes =
                    serde_json::from_value(saved.get("sizes").cloned().ok_or("missing UI sizes")?)
                        .map_err(|error| error.to_string())?;
                (offsets, sizes)
            } else {
                (
                    serde_json::from_value(saved).map_err(|error| error.to_string())?,
                    BTreeMap::new(),
                )
            };
        if offsets.len() + sizes.len() > MAX_ENTRIES
            || offsets
                .values()
                .chain(sizes.values())
                .flatten()
                .any(|value| !value.is_finite() || value.abs() > 1_000_000.0)
        {
            return Err("saved UI layout contains invalid values".into());
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
        for (id, size) in sizes {
            if let Some(element) = self
                .entities
                .get(&id)
                .and_then(|entity| world.get_mut::<UiElement>(*entity))
            {
                if let Some(limits) = &element.resize {
                    if (0..2).all(|axis| {
                        size[axis] >= limits.min[axis] && size[axis] <= limits.max[axis]
                    }) {
                        element.rect.width = size[0];
                        element.rect.height = size[1];
                    }
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
            if let (Some(element), Some((offset, size))) =
                (world.get_mut::<UiElement>(*entity), self.defaults.get(id))
            {
                element.drag_offset = *offset;
                if let (Some(size), Some(_)) = (size, &element.resize) {
                    element.rect.width = size[0];
                    element.rect.height = size[1];
                }
            }
        }
    }
}
