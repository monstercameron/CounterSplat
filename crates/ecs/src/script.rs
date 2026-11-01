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
    // Missing reads and write acknowledgements are common and have a fixed
    // wire representation. Avoid invoking the JSON serializer for them.
    if value.is_none() {
        return Ok(r#"{"found":false,"value":null}"#.to_owned());
    }
    serde_json::to_string(&ScriptResult { found: true, value }).map_err(|error| error.to_string())
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

// Stream one-component query IDs straight into the JSON response. This
// avoids an intermediate Vec<Entity> and keeps adapter iteration order.
fn script_single_query_result(world: &World, name: &str) -> Result<String, String> {
    let mut output = Vec::with_capacity(128);
    output.extend_from_slice(br#"{"found":true,"value":"#);
    {
        let mut serializer = serde_json::Serializer::new(&mut output);
        let mut sequence = serializer.serialize_seq(None).map_err(|e| e.to_string())?;
        world.visit_component_entities(name, |entity| {
            sequence
                .serialize_element(&ScriptEntityId(entity))
                .map_err(|e| e.to_string())
        })?;
        sequence.end().map_err(|e| e.to_string())?;
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

// The browser's fixed event-channel lists always send this field order. A
// direct struct avoids buffering the internally tagged enum's other variants.
#[derive(Deserialize)]
struct FastDrainNext<'a> {
    #[serde(rename = "op")]
    _op: serde::de::IgnoredAny,
    #[serde(borrow)]
    names: Vec<RequestName<'a>>,
    #[serde(default)]
    start: usize,
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
    GetIfChanged {
        #[serde(borrow)]
        id: Cow<'a, str>,
        #[serde(borrow)]
        name: Cow<'a, str>,
        #[serde(borrow, default)]
        revision: Option<Cow<'a, str>>,
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
        if request.starts_with(r#"{"op":"drain_next","#) {
            let command: FastDrainNext =
                serde_json::from_str(request).map_err(|e| e.to_string())?;
            return script_result(
                self.drain_next_events(&command.names, command.start)
                    .map(|(index, events)| EventBatch { index, events }),
            );
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
                if let [component] = components.as_slice() {
                    return script_single_query_result(self, component.as_ref());
                }
                let names: Vec<&str> = components.iter().map(AsRef::as_ref).collect();
                let entities = self.query_components(&names)?;
                return script_result(Some(ScriptEntityList(&entities)));
            }
            S::Get { id, name } => {
                return script_component_result(self, Entity::from_str(&id)?, &name);
            }
            S::GetIfChanged { id, name, revision } => {
                return script_result(self.read_component_if_changed(
                    Entity::from_str(&id)?,
                    &name,
                    revision.as_deref(),
                )?);
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
    fn fast_drain_parser_matches_general_protocol_for_valid_and_invalid_requests() {
        fn general(text: &str) -> Result<(Vec<String>, usize), String> {
            match serde_json::from_str::<ScriptOperation<'_>>(text).map_err(|e| e.to_string())? {
                ScriptOperation::DrainNext { names, start } => Ok((
                    names.into_iter().map(|name| name.0.into_owned()).collect(),
                    start,
                )),
                _ => Err("wrong operation".into()),
            }
        }
        fn fast(text: &str) -> Result<(Vec<String>, usize), String> {
            let value: FastDrainNext<'_> = serde_json::from_str(text).map_err(|e| e.to_string())?;
            Ok((
                value
                    .names
                    .into_iter()
                    .map(|name| name.0.into_owned())
                    .collect(),
                value.start,
            ))
        }
        for text in [
            r#"{"op":"drain_next","names":[]}"#,
            r#"{"op":"drain_next","names":["a","b"],"start":1}"#,
            r#"{"op":"drain_next","names":["quoted\"name","\u00e9"],"start":0,"extra":{"x":[1,null]}}"#,
            r#"{"op":"drain_next","names":["a"],"start":999999}"#,
            r#"{"op":"drain_next","names":["a"],"start":-1}"#,
            r#"{"op":"drain_next","names":["a"],"start":null}"#,
            r#"{"op":"drain_next","names":[1]}"#,
            r#"{"op":"drain_next","names":null}"#,
            r#"{"op":"drain_next","start":0}"#,
            r#"{"op":"drain_next","names":[],"names":[]}"#,
            r#"{"op":"drain_next","names":[],"start":0,"start":1}"#,
            r#"{"op":"drain_next","names":[],"op":"other"}"#,
            r#"{"op":"drain_next","names":[],"extra":[}"#,
        ] {
            assert_eq!(general(text).ok(), fast(text).ok(), "{text}");
        }
    }

    #[test]
    #[ignore = "manual CPU microbenchmark"]
    fn benchmark_drain_next_parsing() {
        use std::{hint::black_box, time::Instant};
        let text = r#"{"op":"drain_next","names":["input.key","input.mouse_delta","input.mouse_button","input.reset","ui.action","editor.entity_selected","physics.ready","physics.ground_result","physics.clearance_result","physics.trace_result","physics.motion_resolved","physics.motion_error","physics.collision","physics.error","weapon.trigger","weapon.aim","weapon.reload","weapon.equip","weapon.mode","weapon.hit","weapon.die","weapon.posture"],"start":0}"#;
        let mut samples = [Vec::new(), Vec::new()];
        for round in 0..9 {
            for mode in if round % 2 == 0 { [0, 1] } else { [1, 0] } {
                let start = Instant::now();
                for _ in 0..100_000 {
                    if mode == 0 {
                        black_box(
                            serde_json::from_str::<ScriptOperation<'_>>(black_box(text)).unwrap(),
                        );
                    } else {
                        black_box(
                            serde_json::from_str::<FastDrainNext<'_>>(black_box(text)).unwrap(),
                        );
                    }
                }
                samples[mode].push(start.elapsed().as_nanos() as f64 / 100_000.0);
            }
        }
        samples[0].sort_by(f64::total_cmp);
        samples[1].sort_by(f64::total_cmp);
        println!(
            "drain-next parser: general {:.0} ns, direct {:.0} ns",
            samples[0][4], samples[1][4]
        );
    }

    #[test]
    fn fast_and_general_drain_paths_preserve_order_and_later_events() {
        let fast = r#"{"op":"drain_next","names":["early","hit","later"],"start":0}"#;
        let general = r#"{"names":["early","hit","later"],"start":0,"op":"drain_next"}"#;
        let mut a = World::new();
        let mut b = World::new();
        for world in [&mut a, &mut b] {
            world.emit_event("hit", serde_json::json!(1));
            world.emit_event("hit", serde_json::json!(2));
            world.emit_event("later", serde_json::json!(3));
        }
        assert_eq!(
            a.apply_script_json(fast).unwrap(),
            b.apply_script_json(general).unwrap()
        );
        assert_eq!(a.drain_events("later"), b.drain_events("later"));
        assert_eq!(
            a.apply_script_json(fast).unwrap(),
            b.apply_script_json(general).unwrap()
        );
    }

    #[test]
    #[ignore = "manual CPU microbenchmark"]
    fn benchmark_drain_next_bridge() {
        use std::{hint::black_box, time::Instant};
        let names = [
            "input.key",
            "input.mouse_delta",
            "input.mouse_button",
            "input.reset",
            "ui.action",
            "editor.entity_selected",
            "physics.ready",
            "physics.ground_result",
            "physics.clearance_result",
            "physics.trace_result",
            "physics.motion_resolved",
            "physics.motion_error",
            "physics.collision",
            "physics.error",
            "weapon.trigger",
            "weapon.aim",
            "weapon.reload",
            "weapon.equip",
            "weapon.mode",
            "weapon.hit",
            "weapon.die",
            "weapon.posture",
        ];
        let names_json = serde_json::to_string(&names).unwrap();
        let fast = format!(r#"{{"op":"drain_next","names":{names_json},"start":0}}"#);
        let general = format!(r#"{{"names":{names_json},"start":0,"op":"drain_next"}}"#);
        let mut world = World::new();
        assert_eq!(
            world.apply_script_json(&fast).unwrap(),
            world.apply_script_json(&general).unwrap()
        );
        let mut samples = [Vec::new(), Vec::new()];
        for round in 0..9 {
            for mode in if round % 2 == 0 { [0, 1] } else { [1, 0] } {
                let request = if mode == 0 { &general } else { &fast };
                let start = Instant::now();
                for _ in 0..100_000 {
                    black_box(world.apply_script_json(black_box(request)).unwrap());
                }
                samples[mode].push(start.elapsed().as_nanos() as f64 / 100_000.0);
            }
        }
        samples[0].sort_by(f64::total_cmp);
        samples[1].sort_by(f64::total_cmp);
        println!(
            "drain-next bridge: general {:.0} ns, direct {:.0} ns",
            samples[0][4], samples[1][4]
        );
    }

    #[test]
    fn streamed_single_query_matches_allocated_query_json() {
        let mut world = World::new();
        world.register_serde_component::<u32>("Typed").unwrap();
        for index in 0..1536 {
            let entity = world.spawn();
            if index % 2 == 0 {
                world.set_dynamic(entity, "Marker", Value::Null).unwrap();
            }
            if index % 3 == 0 {
                world.insert(entity, index as u32).unwrap();
            }
        }
        for name in ["Marker", "Typed", "Missing"] {
            let expected = world.query_components(&[name]).unwrap();
            let reference = script_result(Some(ScriptEntityList(&expected))).unwrap();
            assert_eq!(script_single_query_result(&world, name).unwrap(), reference);
        }
        assert!(script_single_query_result(&world, "").is_err());
    }

    #[test]
    #[ignore = "manual CPU microbenchmark"]
    fn benchmark_streamed_single_query() {
        use std::{hint::black_box, time::Instant};
        for size in [1usize, 1000, 10000] {
            let mut world = World::new();
            for _ in 0..size {
                let entity = world.spawn();
                world.set_dynamic(entity, "Marker", Value::Null).unwrap();
            }
            let iterations = if size == 1 {
                100_000
            } else if size == 1000 {
                1000
            } else {
                100
            };
            let mut samples = [Vec::new(), Vec::new()];
            for round in 0..9 {
                for mode in if round % 2 == 0 { [0, 1] } else { [1, 0] } {
                    let start = Instant::now();
                    for _ in 0..iterations {
                        let result = if mode == 0 {
                            let names = vec![black_box("Marker")];
                            let entities = world.query_components(&names).unwrap();
                            script_result(Some(ScriptEntityList(&entities))).unwrap()
                        } else {
                            script_single_query_result(black_box(&world), black_box("Marker"))
                                .unwrap()
                        };
                        black_box(result);
                    }
                    samples[mode].push(start.elapsed().as_nanos() as f64 / iterations as f64);
                }
            }
            samples[0].sort_by(f64::total_cmp);
            samples[1].sort_by(f64::total_cmp);
            println!(
                "entities={size} allocated_ns={:.0} streamed_ns={:.0}",
                samples[0][4], samples[1][4]
            );
        }
    }

    #[test]
    fn empty_script_results_match_serialization_and_keep_present_null_distinct() {
        let reference = serde_json::to_string(&ScriptResult::<Value> {
            found: false,
            value: None,
        })
        .unwrap();
        assert_eq!(script_result::<Value>(None).unwrap(), reference);
        assert_eq!(
            script_result(Some(Value::Null)).unwrap(),
            r#"{"found":true,"value":null}"#
        );
        assert_eq!(
            script_result(Some(false)).unwrap(),
            r#"{"found":true,"value":false}"#
        );
    }

    #[test]
    #[ignore = "manual CPU microbenchmark"]
    fn benchmark_empty_script_result() {
        use std::{hint::black_box, time::Instant};
        let mut samples = [Vec::new(), Vec::new()];
        for round in 0..11 {
            for mode in if round % 2 == 0 { [0, 1] } else { [1, 0] } {
                let start = Instant::now();
                for _ in 0..500_000 {
                    let value = if mode == 0 {
                        serde_json::to_string(&ScriptResult::<Value> {
                            found: false,
                            value: None,
                        })
                        .unwrap()
                    } else {
                        script_result::<Value>(None).unwrap()
                    };
                    black_box(value);
                }
                samples[mode].push(start.elapsed().as_nanos() as f64 / 500_000.0);
            }
        }
        samples[0].sort_by(f64::total_cmp);
        samples[1].sort_by(f64::total_cmp);
        println!(
            "empty JSON response: serialized {:.1} ns, constant {:.1} ns",
            samples[0][5], samples[1][5]
        );
    }

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
