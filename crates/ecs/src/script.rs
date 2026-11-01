//! JSON transport for the standard ECS operations, with opaque string entity IDs.
//!
//! Hosts can use this boundary when the caller cannot exchange Rust values directly. All
//! operations still execute on the world owner and use the same component adapters as native Lua.
use crate::{Entity, World, WorldOperation, WorldOutcome};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{borrow::Cow, str::FromStr};

const MAX_REQUEST_BYTES: usize = 1024 * 1024;

#[derive(Serialize)]
struct ScriptResult<T> {
    found: bool,
    value: Option<T>,
}

fn script_result<T: Serialize>(value: Option<T>) -> Result<String, String> {
    serde_json::to_string(&ScriptResult {
        found: value.is_some(),
        value,
    })
    .map_err(|error| error.to_string())
}

fn script_component_result(world: &World, entity: Entity, name: &str) -> Result<String, String> {
    if !world.is_adapted_component(name) {
        return script_result(world.borrow_component_value(entity, name)?);
    }
    let mut output = Vec::with_capacity(128);
    output.extend_from_slice(br#"{"found":true,"value":"#);
    if !world.write_component_json(entity, name, &mut output)? {
        return script_result::<Value>(None);
    }
    output.push(b'}');
    String::from_utf8(output).map_err(|error| error.to_string())
}

#[derive(Deserialize)]
struct RequestName<'a>(#[serde(borrow)] Cow<'a, str>);

impl AsRef<str> for RequestName<'_> {
    fn as_ref(&self) -> &str {
        self.0.as_ref()
    }
}

#[derive(Serialize)]
struct EventBatch {
    index: usize,
    events: Vec<Value>,
}

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum ScriptOperation<'a> {
    Spawn,
    Despawn {
        id: String,
        #[serde(default)]
        recursive: bool,
    },
    IsAlive {
        id: String,
    },
    Entities,
    Hierarchy,
    Components {
        id: String,
    },
    Query {
        components: Vec<String>,
    },
    Get {
        #[serde(borrow)]
        id: Cow<'a, str>,
        #[serde(borrow)]
        name: Cow<'a, str>,
    },
    Has {
        #[serde(borrow)]
        id: Cow<'a, str>,
        #[serde(borrow)]
        name: Cow<'a, str>,
    },
    Set {
        id: String,
        name: String,
        value: Value,
    },
    Remove {
        id: String,
        name: String,
    },
    GetResource {
        #[serde(borrow)]
        name: Cow<'a, str>,
    },
    GetResourceField {
        #[serde(borrow)]
        name: Cow<'a, str>,
        #[serde(borrow)]
        field: Cow<'a, str>,
    },
    SetResource {
        name: String,
        value: Value,
    },
    RemoveResource {
        name: String,
    },
    SetParent {
        child: String,
        parent: Option<String>,
    },
    Parent {
        id: String,
    },
    Children {
        id: String,
    },
    Emit {
        name: String,
        value: Value,
    },
    Drain {
        #[serde(borrow)]
        name: Cow<'a, str>,
    },
    DrainNext {
        #[serde(borrow)]
        names: Vec<RequestName<'a>>,
        #[serde(default)]
        start: usize,
    },
    Snapshot,
}

impl World {
    /// Apply one JSON operation and return `{found, value}`. `found` distinguishes a missing
    /// value from JSON null; all entity handles use the same opaque strings as native Lua.
    pub fn apply_script_json(&mut self, request: &str) -> Result<String, String> {
        if request.len() > MAX_REQUEST_BYTES {
            return Err("ECS script request is too large".into());
        }
        let command: ScriptOperation = serde_json::from_str(request).map_err(|e| e.to_string())?;
        use ScriptOperation as S;
        use WorldOperation as O;
        let operation = match command {
            S::Spawn => O::Spawn,
            S::Despawn { id, recursive } => O::Despawn {
                entity: Entity::from_str(&id)?,
                recursive,
            },
            S::IsAlive { id } => O::IsAlive {
                entity: Entity::from_str(&id)?,
            },
            S::Entities => O::Query {
                components: Vec::new(),
            },
            S::Hierarchy => O::Hierarchy,
            S::Components { id } => O::Components {
                entity: Entity::from_str(&id)?,
            },
            S::Query { components } => O::Query { components },
            S::Get { id, name } => {
                return script_component_result(self, Entity::from_str(&id)?, &name);
            }
            S::Has { id, name } => {
                return script_result(Some(
                    self.has_component_value(Entity::from_str(&id)?, &name)?,
                ));
            }
            S::Set { id, name, value } => O::WriteComponent {
                entity: Entity::from_str(&id)?,
                name,
                value,
            },
            S::Remove { id, name } => O::RemoveComponent {
                entity: Entity::from_str(&id)?,
                name,
            },
            S::GetResource { name } => return script_result(self.get_dynamic_resource(&name)),
            S::GetResourceField { name, field } => {
                return script_result(
                    self.get_dynamic_resource(&name)
                        .and_then(Value::as_object)
                        .and_then(|object| object.get(field.as_ref())),
                );
            }
            S::SetResource { name, value } => O::WriteResource { name, value },
            S::RemoveResource { name } => O::RemoveResource { name },
            S::SetParent { child, parent } => O::SetParent {
                child: Entity::from_str(&child)?,
                parent: parent.map(|id| Entity::from_str(&id)).transpose()?,
            },
            S::Parent { id } => O::Parent {
                entity: Entity::from_str(&id)?,
            },
            S::Children { id } => O::Children {
                entity: Entity::from_str(&id)?,
            },
            S::Emit { name, value } => O::EmitEvent { name, value },
            S::Drain { name } => return script_result(Some(self.drain_events(&name))),
            S::DrainNext { names, start } => {
                return script_result(
                    self.drain_next_events(&names, start)
                        .map(|(index, events)| EventBatch { index, events }),
                );
            }
            S::Snapshot => O::Snapshot,
        };
        let result = self.apply_operation(operation)?;
        let value = match result {
            WorldOutcome::Spawned(id) => Some(json!(id.to_string())),
            WorldOutcome::Bool(value) => Some(json!(value)),
            WorldOutcome::Count(value) => Some(json!(value)),
            WorldOutcome::Value(value) => value,
            WorldOutcome::Names(value) => Some(json!(value)),
            WorldOutcome::Entity(value) => value.map(|id| json!(id.to_string())),
            WorldOutcome::Entities(value) => Some(json!(value
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>())),
            WorldOutcome::Events(value) => Some(json!(value)),
            WorldOutcome::Snapshot(snapshot) => Some(json!({
                "entities": snapshot.entities.into_iter().map(|item| json!({
                    "id": item.entity.to_string(),
                    "parent": item.parent.map(|id| id.to_string()),
                    "components": item.components,
                })).collect::<Vec<_>>(),
                "resources": snapshot.resources,
            })),
            WorldOutcome::Applied => None,
        };
        script_result(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn request_names_borrow_when_possible_and_support_escaped_text() {
        let request =
            json!({"op":"drain_next","names":["plain","quoted\"name"],"start":0}).to_string();
        let operation: ScriptOperation<'_> = serde_json::from_str(&request).unwrap();
        match operation {
            ScriptOperation::DrainNext { names, .. } => {
                assert!(matches!(names[0].0, Cow::Borrowed("plain")));
                assert!(matches!(&names[1].0, Cow::Owned(name) if name == "quoted\"name"));
            }
            _ => panic!("expected ordered event drain"),
        }

        let mut world = World::new();
        let entity = world.spawn();
        let name = "quoted\"component";
        world
            .set_dynamic(entity, name, json!({"text":"works"}))
            .unwrap();
        let request = json!({"op":"get","id":entity.to_string(),"name":name});
        let response: Value =
            serde_json::from_str(&world.apply_script_json(&request.to_string()).unwrap()).unwrap();
        assert_eq!(response, json!({"found":true,"value":{"text":"works"}}));

        let channel = "quoted\"channel";
        world.emit_event(channel, Value::Null);
        let request = json!({"op":"drain_next","names":["other",channel],"start":0});
        let response: Value =
            serde_json::from_str(&world.apply_script_json(&request.to_string()).unwrap()).unwrap();
        assert_eq!(
            response,
            json!({"found":true,"value":{"index":1,"events":[null]}})
        );
    }

    #[test]
    fn json_bridge_uses_real_world_operations_and_preserves_null_and_handles() {
        let mut world = World::new();
        let run = |world: &mut World, value: Value| -> Value {
            serde_json::from_str(&world.apply_script_json(&value.to_string()).unwrap()).unwrap()
        };
        let id = run(&mut world, json!({"op":"spawn"}))["value"]
            .as_str()
            .unwrap()
            .to_owned();
        run(
            &mut world,
            json!({"op":"set", "id":id, "name":"Health", "value":{"hp":10}}),
        );
        assert_eq!(
            run(&mut world, json!({"op":"get", "id":id, "name":"Health"}))["value"]["hp"],
            10
        );
        run(
            &mut world,
            json!({"op":"set_resource", "name":"nil-value", "value":null}),
        );
        assert_eq!(
            run(&mut world, json!({"op":"get_resource", "name":"nil-value"})),
            json!({"found":true,"value":null})
        );
        assert_eq!(
            run(&mut world, json!({"op":"get_resource", "name":"missing"}))["found"],
            false
        );
        run(
            &mut world,
            json!({"op":"set_resource", "name":"bindings", "value":{"player":id,"optional":null}}),
        );
        assert_eq!(
            run(
                &mut world,
                json!({"op":"get_resource_field","name":"bindings","field":"player"})
            )["value"],
            id
        );
        assert_eq!(
            run(
                &mut world,
                json!({"op":"get_resource_field","name":"bindings","field":"optional"})
            ),
            json!({"found":true,"value":null})
        );
        assert_eq!(
            run(
                &mut world,
                json!({"op":"get_resource_field","name":"bindings","field":"missing"})
            )["found"],
            false
        );
        run(
            &mut world,
            json!({"op":"emit", "name":"input.key", "value":{"key":"Escape"}}),
        );
        assert_eq!(
            run(&mut world, json!({"op":"drain", "name":"input.key"}))["value"][0]["key"],
            "Escape"
        );
        assert_eq!(run(&mut world, json!({"op":"entities"}))["value"][0], id);
        assert!(world
            .apply_script_json(&json!({"op":"set","id":"bad","name":"x","value":1}).to_string())
            .is_err());
    }
}
