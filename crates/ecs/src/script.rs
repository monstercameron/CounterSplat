//! JSON transport for the standard ECS operations, with opaque string entity IDs.
//!
//! Hosts can use this boundary when the caller cannot exchange Rust values directly. All
//! operations still execute on the world owner and use the same component adapters as native Lua.
use crate::{ComponentFields, Entity, World, WorldOperation, WorldOutcome};
use serde::{ser::SerializeSeq, Deserialize, Serialize, Serializer};
use serde_json::{json, Value};
use std::{borrow::Cow, collections::HashSet, str::FromStr};

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

struct ScriptEntityId(Entity);
impl Serialize for ScriptEntityId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(&self.0)
    }
}

struct ScriptEntities<'a>(&'a World);
impl Serialize for ScriptEntities<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut sequence = serializer.serialize_seq(Some(self.0.len()))?;
        for entity in self.0.iter_entities() {
            sequence.serialize_element(&ScriptEntityId(entity))?;
        }
        sequence.end()
    }
}

struct ScriptEntityList<'a>(&'a [Entity]);
impl Serialize for ScriptEntityList<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut sequence = serializer.serialize_seq(Some(self.0.len()))?;
        for &entity in self.0 {
            sequence.serialize_element(&ScriptEntityId(entity))?;
        }
        sequence.end()
    }
}

#[derive(Serialize)]
struct ScriptHierarchyEntry {
    id: ScriptEntityId,
    parent: Option<ScriptEntityId>,
}

struct ScriptHierarchy<'a>(&'a World);
impl Serialize for ScriptHierarchy<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut sequence = serializer.serialize_seq(Some(self.0.len()))?;
        for entity in self.0.iter_entities() {
            sequence.serialize_element(&ScriptHierarchyEntry {
                id: ScriptEntityId(entity),
                parent: self.0.parent(entity).map(ScriptEntityId),
            })?;
        }
        sequence.end()
    }
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

fn script_components_result(
    world: &World,
    entity: Entity,
    names: &[RequestName<'_>],
) -> Result<String, String> {
    world.require_alive(entity)?;
    // Preserve repeated adapter reads and last-value semantics for duplicates.
    // Avoid quadratic work for large, untrusted requests.
    let duplicates = if names.len() <= 16 {
        names.iter().enumerate().any(|(index, name)| {
            names[..index]
                .iter()
                .any(|earlier| earlier.as_ref() == name.as_ref())
        })
    } else {
        let mut seen = HashSet::with_capacity(names.len());
        names.iter().any(|name| !seen.insert(name.as_ref()))
    };
    if duplicates {
        let owned = names
            .iter()
            .map(|name| name.as_ref().to_owned())
            .collect::<Vec<_>>();
        return script_result(Some(world.read_component_values(entity, &owned)?));
    }

    let mut output = Vec::with_capacity(256);
    output.extend_from_slice(br#"{"found":true,"value":{"#);
    let mut first = true;
    for name in names {
        // Serialize directly into the response. If absent, remove the key and any
        // partial bytes an adapter wrote before returning false.
        let start = output.len();
        if !first {
            output.push(b',');
        }
        serde_json::to_writer(&mut output, name.as_ref()).map_err(|error| error.to_string())?;
        output.push(b':');
        if world.write_component_json(entity, name.as_ref(), &mut output)? {
            first = false;
        } else {
            output.truncate(start);
        }
    }
    output.extend_from_slice(b"}}");
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
        #[serde(borrow)]
        id: Cow<'a, str>,
    },
    Entities,
    Hierarchy,
    StructureRevision,
    Components {
        id: String,
    },
    Query {
        #[serde(borrow)]
        components: Vec<RequestName<'a>>,
    },
    Get {
        #[serde(borrow)]
        id: Cow<'a, str>,
        #[serde(borrow)]
        name: Cow<'a, str>,
    },
    GetMany {
        #[serde(borrow)]
        id: Cow<'a, str>,
        #[serde(borrow)]
        names: Vec<RequestName<'a>>,
    },
    Has {
        #[serde(borrow)]
        id: Cow<'a, str>,
        #[serde(borrow)]
        name: Cow<'a, str>,
    },
    Set {
        #[serde(borrow)]
        id: Cow<'a, str>,
        #[serde(borrow)]
        name: Cow<'a, str>,
        value: Value,
    },
    SetField {
        #[serde(borrow)]
        id: Cow<'a, str>,
        #[serde(borrow)]
        name: Cow<'a, str>,
        #[serde(borrow)]
        field: Cow<'a, str>,
        value: Value,
    },
    SetFields {
        #[serde(borrow)]
        id: Cow<'a, str>,
        #[serde(borrow)]
        name: Cow<'a, str>,
        fields: ComponentFields,
    },
    Remove {
        #[serde(borrow)]
        id: Cow<'a, str>,
        #[serde(borrow)]
        name: Cow<'a, str>,
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
        #[serde(borrow)]
        name: Cow<'a, str>,
        value: Value,
    },
    RemoveResource {
        #[serde(borrow)]
        name: Cow<'a, str>,
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
        #[serde(borrow)]
        name: Cow<'a, str>,
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
            S::IsAlive { id } => return script_result(Some(self.is_alive(Entity::from_str(&id)?))),
            S::Entities => return script_result(Some(ScriptEntities(self))),
            S::Hierarchy => return script_result(Some(ScriptHierarchy(self))),
            S::StructureRevision => {
                return script_result(Some(self.structure_revision().to_string()))
            }
            S::Components { id } => O::Components {
                entity: Entity::from_str(&id)?,
            },
            S::Query { components } => {
                if components.is_empty() {
                    return script_result(Some(ScriptEntities(self)));
                }
                let names: Vec<&str> = components.iter().map(AsRef::as_ref).collect();
                let entities = self.query_components(&names)?;
                return script_result(Some(ScriptEntityList(&entities)));
            }
            S::Get { id, name } => {
                return script_component_result(self, Entity::from_str(&id)?, &name);
            }
            S::GetMany { id, names } => {
                return script_components_result(self, Entity::from_str(&id)?, &names);
            }
            S::Has { id, name } => {
                return script_result(Some(
                    self.has_component_value(Entity::from_str(&id)?, &name)?,
                ));
            }
            S::Set { id, name, value } => {
                self.write_component_value(Entity::from_str(&id)?, &name, value)?;
                return script_result::<Value>(None);
            }
            S::SetField {
                id,
                name,
                field,
                value,
            } => {
                self.write_component_field(Entity::from_str(&id)?, &name, &field, value)?;
                return script_result::<Value>(None);
            }
            S::SetFields { id, name, fields } => {
                self.write_component_fields(Entity::from_str(&id)?, &name, fields)?;
                return script_result::<Value>(None);
            }
            S::Remove { id, name } => {
                return script_result(self.remove_component_value(Entity::from_str(&id)?, &name)?);
            }
            S::GetResource { name } => return script_result(self.get_dynamic_resource(&name)),
            S::GetResourceField { name, field } => {
                return script_result(
                    self.get_dynamic_resource(&name)
                        .and_then(Value::as_object)
                        .and_then(|object| object.get(field.as_ref())),
                );
            }
            S::SetResource { name, value } => {
                self.set_dynamic_resource(&name, value);
                return script_result::<Value>(None);
            }
            S::RemoveResource { name } => {
                return script_result(self.remove_dynamic_resource(&name))
            }
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
            S::Emit { name, value } => {
                self.emit_event(&name, value);
                return script_result::<Value>(None);
            }
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

        let set = json!({"op":"set","id":entity.to_string(),"name":name,"value":null});
        let result: Value =
            serde_json::from_str(&world.apply_script_json(&set.to_string()).unwrap()).unwrap();
        assert_eq!(result, json!({"found":false,"value":null}));
        assert_eq!(world.get_dynamic(entity, name), Some(&Value::Null));
        let remove = json!({"op":"remove","id":entity.to_string(),"name":name});
        let result: Value =
            serde_json::from_str(&world.apply_script_json(&remove.to_string()).unwrap()).unwrap();
        assert_eq!(result, json!({"found":true,"value":null}));
        let resource = "quoted\"resource";
        world
            .apply_script_json(&json!({"op":"set_resource","name":resource,"value":7}).to_string())
            .unwrap();
        assert_eq!(world.get_dynamic_resource(resource), Some(&json!(7)));
        world
            .apply_script_json(&json!({"op":"remove_resource","name":resource}).to_string())
            .unwrap();
        assert!(world.get_dynamic_resource(resource).is_none());

        let channel = "quoted\"channel";
        world
            .apply_script_json(&json!({"op":"emit","name":channel,"value":null}).to_string())
            .unwrap();
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
