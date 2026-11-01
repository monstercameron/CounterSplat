//! Small, GPU-free world for measuring the browser Lua/ECS boundary.
#![cfg(target_arch = "wasm32")]
use countersplat_ecs::{ComponentAdapter, Entity, World};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use wasm_bindgen::prelude::*;

#[derive(Serialize, Deserialize)]
struct Controls {
    enabled: bool,
    speed: f32,
    sprint: f32,
    bindings: Vec<String>,
}
struct ControlsAdapter;
impl ComponentAdapter for ControlsAdapter {
    fn read(&self, world: &World, entity: Entity) -> Result<Option<Value>, String> {
        world
            .get::<Controls>(entity)
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
        let Some(value) = world.get::<Controls>(entity) else {
            return Ok(false);
        };
        serde_json::to_writer(output, value).map_err(|error| error.to_string())?;
        Ok(true)
    }
    fn read_field(
        &self,
        world: &World,
        entity: Entity,
        field: &str,
    ) -> Result<Option<Value>, String> {
        if field != "speed" {
            return Ok(self
                .read(world, entity)?
                .and_then(|value| value.get(field).cloned()));
        }
        world
            .get::<Controls>(entity)
            .map(|value| serde_json::to_value(value.speed).map_err(|e| e.to_string()))
            .transpose()
    }
    fn write_field_json(
        &self,
        world: &World,
        entity: Entity,
        field: &str,
        output: &mut Vec<u8>,
    ) -> Result<bool, String> {
        if field != "speed" {
            return countersplat_ecs::write_object_field_json_via_adapter(
                self, world, entity, field, output,
            );
        }
        let Some(value) = world.get::<Controls>(entity) else {
            return Ok(false);
        };
        serde_json::to_writer(output, &value.speed).map_err(|error| error.to_string())?;
        Ok(true)
    }
    fn write(&self, world: &mut World, entity: Entity, value: Value) -> Result<(), String> {
        world.insert(
            entity,
            serde_json::from_value::<Controls>(value).map_err(|e| e.to_string())?,
        )
    }
    fn remove(&self, world: &mut World, entity: Entity) -> Result<Option<Value>, String> {
        world
            .remove::<Controls>(entity)
            .map(serde_json::to_value)
            .transpose()
            .map_err(|e| e.to_string())
    }
    fn contains(&self, world: &World, entity: Entity) -> bool {
        world.has::<Controls>(entity)
    }
    fn content_revision(&self, world: &World) -> Option<u64> {
        world.component_revision::<Controls>()
    }
}
fn js_error(error: impl std::fmt::Display) -> JsValue {
    JsValue::from_str(&error.to_string())
}

/// A tracked typed component on the same ECS implementation as the game.
#[wasm_bindgen]
pub struct BenchWorld {
    world: World,
    entity: Entity,
}
#[wasm_bindgen]
impl BenchWorld {
    #[wasm_bindgen(constructor)]
    pub fn new() -> BenchWorld {
        let mut world = World::new();
        world
            .register_component_adapter("PlayerControls", ControlsAdapter)
            .unwrap();
        world.track_component_changes::<Controls>();
        let entity = world.spawn();
        world
            .insert(
                entity,
                Controls {
                    enabled: true,
                    speed: 4.0,
                    sprint: 1.5,
                    bindings: vec!["KeyW".into(), "KeyS".into()],
                },
            )
            .unwrap();
        world.set_dynamic_resource("scene.entities", json!({"player": entity.to_string()}));
        world.set_dynamic_resource("player.spawn_probe_pending", json!(false));
        world.set_dynamic_resource("nullable", Value::Null);
        BenchWorld { world, entity }
    }
    pub fn id(&self) -> String {
        self.entity.to_string()
    }
    pub fn revision(&self) -> String {
        self.world
            .read_component_if_changed(self.entity, "PlayerControls", None)
            .unwrap()
            .unwrap()
            .revision
            .unwrap()
    }
    pub fn set_speed(&mut self, value: f32) {
        self.world.get_mut::<Controls>(self.entity).unwrap().speed = value;
    }
    pub fn ecs_get_field(
        &self,
        layer: &str,
        id: &str,
        name: &str,
        field: &str,
    ) -> Result<Option<String>, JsValue> {
        if layer != "gameplay" {
            return Err(js_error("unknown script world"));
        }
        self.world
            .script_component_field_json(id, name, field)
            .map_err(js_error)
    }
    pub fn ecs_operation(&mut self, layer: &str, request: &str) -> Result<String, JsValue> {
        if layer != "gameplay" {
            return Err(js_error("unknown script world"));
        }
        self.world.apply_script_json(request).map_err(js_error)
    }
    pub fn ecs_get_resource(&self, layer: &str, name: &str) -> Result<Option<String>, JsValue> {
        if layer != "gameplay" {
            return Err(js_error("unknown script world"));
        }
        self.world
            .script_resource_value_json(name)
            .map_err(js_error)
    }
    pub fn ecs_get_resource_field(
        &self,
        layer: &str,
        name: &str,
        field: &str,
    ) -> Result<Option<String>, JsValue> {
        if layer != "gameplay" {
            return Err(js_error("unknown script world"));
        }
        self.world
            .script_resource_field_json(name, field)
            .map_err(js_error)
    }
    pub fn ecs_is_alive(&self, layer: &str, id: &str) -> Result<bool, JsValue> {
        if layer != "gameplay" {
            return Err(js_error("unknown script world"));
        }
        self.world.script_entity_alive(id).map_err(js_error)
    }
    pub fn ecs_has_component(&self, layer: &str, id: &str, name: &str) -> Result<bool, JsValue> {
        if layer != "gameplay" {
            return Err(js_error("unknown script world"));
        }
        self.world.script_has_component(id, name).map_err(js_error)
    }
    pub fn ecs_get_if_changed(
        &self,
        layer: &str,
        id: &str,
        name: &str,
        revision: Option<String>,
    ) -> Result<Option<String>, JsValue> {
        if layer != "gameplay" {
            return Err(js_error("unknown script world"));
        }
        self.world
            .script_component_change_json(id, name, revision.as_deref())
            .map_err(js_error)
    }
}
