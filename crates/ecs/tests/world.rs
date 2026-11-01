use countersplat_ecs::{
    ComponentAdapter, Entity, LifecycleEvent, Schedule, Value, World, WorldCommand, WorldOperation,
    WorldOutcome, WorldReply, WorldSnapshot,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::mpsc;
use std::thread;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Position {
    x: f32,
    y: f32,
}

#[test]
fn entity_handles_are_world_scoped_and_do_not_revive() {
    let mut first = World::new();
    let mut second = World::new();
    let original = first.spawn();
    let foreign = second.spawn();
    assert_ne!(original, foreign);
    assert!(!second.is_alive(original));
    assert!(second.insert(original, 7_u32).is_err());
    assert!(second.set_dynamic(original, "Health", json!(10)).is_err());
    assert_eq!(original.to_string().parse::<Entity>().unwrap(), original);
    assert!("7.2".parse::<Entity>().is_err());

    first.insert(original, 12_u32).unwrap();
    first.set_dynamic(original, "Health", json!(10)).unwrap();
    assert!(first.despawn(original));
    let replacement = first.spawn();
    assert_eq!(replacement.index(), original.index());
    assert_ne!(replacement.generation(), original.generation());
    assert!(!first.is_alive(original));
    assert_eq!(first.get::<u32>(replacement), None);
    assert_eq!(first.get_dynamic(replacement, "Health"), None);
    assert!(first.read_component_value(original, "Health").is_err());
}

#[test]
fn structure_revision_tracks_membership_and_parent_changes_only() {
    let mut world = World::new();
    let initial = world.structure_revision();
    let first = world.spawn();
    assert_ne!(world.structure_revision(), initial);
    let second = world.spawn();
    let mut revision = world.structure_revision();

    world.insert(first, 1_u32).unwrap();
    assert_ne!(world.structure_revision(), revision);
    revision = world.structure_revision();
    world.insert(first, 2_u32).unwrap();
    *world.get_mut::<u32>(first).unwrap() = 3;
    world.set_dynamic_resource("settings", json!(1));
    world.emit_event("event", Value::Null);
    assert_eq!(world.structure_revision(), revision);

    world.set_dynamic(first, "Tag", Value::Null).unwrap();
    assert_ne!(world.structure_revision(), revision);
    revision = world.structure_revision();
    world.set_dynamic(first, "Tag", json!(3)).unwrap();
    assert_eq!(world.structure_revision(), revision);
    assert!(world.remove_dynamic(first, "Missing").is_none());
    assert_eq!(world.structure_revision(), revision);
    world.remove_dynamic(first, "Tag");
    assert_ne!(world.structure_revision(), revision);
    revision = world.structure_revision();

    world.set_parent(second, Some(first)).unwrap();
    assert_ne!(world.structure_revision(), revision);
    revision = world.structure_revision();
    world.set_parent(second, Some(first)).unwrap();
    assert!(world.set_parent(first, Some(second)).is_err());
    assert_eq!(world.structure_revision(), revision);
    world.register_serde_component::<u32>("Number").unwrap();
    assert_ne!(world.structure_revision(), revision);
    revision = world.structure_revision();
    assert!(world.register_serde_component::<u32>("Number").is_err());
    assert_eq!(world.structure_revision(), revision);

    let expected = revision.to_string();
    assert_eq!(
        world
            .apply_operation(WorldOperation::StructureRevision)
            .unwrap(),
        WorldOutcome::Value(Some(json!(expected)))
    );
    let result: Value = serde_json::from_str(
        &world
            .apply_script_json(r#"{"op":"structure_revision"}"#)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(result, json!({"found":true,"value":expected}));
    assert!(world.remove::<u32>(first).is_some());
    assert_ne!(world.structure_revision(), revision);
    revision = world.structure_revision();
    assert!(world.despawn(second));
    assert_ne!(world.structure_revision(), revision);
    revision = world.structure_revision();
    assert!(!world.despawn(second));
    assert_eq!(world.structure_revision(), revision);
}

#[test]
fn streamed_script_hierarchy_matches_owned_world_operation() {
    let mut world = World::new();
    let root = world.spawn();
    let removed = world.spawn();
    let child = world.spawn();
    world.set_parent(child, Some(root)).unwrap();
    assert!(world.despawn(removed));
    let replacement = world.spawn();
    world.set_parent(replacement, Some(child)).unwrap();
    assert_eq!(world.iter_entities().collect::<Vec<_>>(), world.entities());

    let expected = match world.apply_operation(WorldOperation::Hierarchy).unwrap() {
        WorldOutcome::Value(Some(value)) => value,
        other => panic!("unexpected hierarchy: {other:?}"),
    };
    let hierarchy: Value =
        serde_json::from_str(&world.apply_script_json(r#"{"op":"hierarchy"}"#).unwrap()).unwrap();
    assert_eq!(hierarchy, json!({"found":true,"value":expected}));
    let entities: Value =
        serde_json::from_str(&world.apply_script_json(r#"{"op":"entities"}"#).unwrap()).unwrap();
    let ids = world
        .entities()
        .into_iter()
        .map(|entity| entity.to_string())
        .collect::<Vec<_>>();
    assert_eq!(entities, json!({"found":true,"value":ids}));

    let empty = World::new()
        .apply_script_json(r#"{"op":"hierarchy"}"#)
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&empty).unwrap(),
        json!({"found":true,"value":[]})
    );
}

#[test]
fn component_view_borrows_ordered_live_storage() {
    let mut world = World::new();
    assert!(world.component_view::<u32>().is_none());
    let first = world.spawn();
    let second = world.spawn();
    world.insert(second, 2_u32).unwrap();
    world.insert(first, 1_u32).unwrap();
    let view = world.component_view::<u32>().unwrap();
    assert_eq!(view.len(), 2);
    assert!(!view.is_empty());
    assert_eq!(view.entities().collect::<Vec<_>>(), vec![first, second]);
    assert_eq!(
        view.iter()
            .map(|(entity, value)| (entity, *value))
            .collect::<Vec<_>>(),
        vec![(first, 1), (second, 2)]
    );
    assert_eq!(view.get(first), Some(&1));
    assert!(view.contains(second));
    assert!(world.despawn(first));
    let view = world.component_view::<u32>().unwrap();
    assert!(!view.contains(first));
    assert_eq!(view.entities().collect::<Vec<_>>(), vec![second]);
}

#[test]
fn typed_components_and_dynamic_values_share_an_explicit_script_view() {
    let mut world = World::new();
    world
        .register_serde_component::<Position>("Position")
        .unwrap();
    let a = world.spawn();
    let b = world.spawn();
    world.insert(a, Position { x: 1.0, y: 2.0 }).unwrap();
    world.insert(b, Position { x: 3.0, y: 4.0 }).unwrap();
    world.set_dynamic(a, "Health", json!(75)).unwrap();

    assert_eq!(
        world.query_components(&["Position", "Health"]).unwrap(),
        vec![a]
    );
    assert_eq!(world.query_dynamic(&[]), vec![a, b]);
    assert_eq!(world.query_components(&[]).unwrap(), vec![a, b]);
    assert_eq!(
        world.component_value_names(a).unwrap(),
        vec!["Health", "Position"]
    );
    assert_eq!(
        world.read_component_value(a, "Position").unwrap(),
        Some(json!({"x": 1.0, "y": 2.0}))
    );
    world
        .write_component_value(a, "Position", json!({"x": 9.0, "y": 8.0}))
        .unwrap();
    assert_eq!(world.get::<Position>(a), Some(&Position { x: 9.0, y: 8.0 }));
    assert!(world
        .write_component_value(a, "Position", json!({"x": "invalid"}))
        .is_err());
    assert_eq!(world.get::<Position>(a), Some(&Position { x: 9.0, y: 8.0 }));
    world.get_mut::<Position>(a).unwrap().x = 6.0;
    assert_eq!(
        world.read_component_value(a, "Position").unwrap(),
        Some(json!({"x": 6.0, "y": 8.0}))
    );
    assert!(world.set_dynamic(a, "Position", json!({})).is_err());
    assert_eq!(
        world.remove_component_value(a, "Position").unwrap(),
        Some(json!({"x": 6.0, "y": 8.0}))
    );
    assert_eq!(
        world.query_components(&["Position", "Health"]).unwrap(),
        Vec::<Entity>::new()
    );
}

struct ReadOnlyHealth;
impl ComponentAdapter for ReadOnlyHealth {
    fn read(&self, world: &World, entity: Entity) -> Result<Option<Value>, String> {
        Ok(world.get::<u32>(entity).map(|value| json!(value)))
    }
    fn write(&self, _: &mut World, _: Entity, _: Value) -> Result<(), String> {
        Err("read only".into())
    }
    fn remove(&self, _: &mut World, _: Entity) -> Result<Option<Value>, String> {
        Err("read only".into())
    }
    fn contains(&self, world: &World, entity: Entity) -> bool {
        world.has::<u32>(entity)
    }
}

#[test]
fn custom_adapter_controls_script_access_without_copying_native_state() {
    let mut world = World::new();
    let entity = world.spawn();
    world.insert(entity, 30_u32).unwrap();
    world
        .register_component_adapter("ReadOnlyHealth", ReadOnlyHealth)
        .unwrap();
    assert!(world
        .register_component_adapter("ReadOnlyHealth", ReadOnlyHealth)
        .is_err());
    assert_eq!(
        world
            .read_component_value(entity, "ReadOnlyHealth")
            .unwrap(),
        Some(json!(30))
    );
    assert!(world
        .write_component_value(entity, "ReadOnlyHealth", json!(0))
        .is_err());
    assert!(world
        .remove_component_value(entity, "ReadOnlyHealth")
        .is_err());
    assert_eq!(world.get::<u32>(entity), Some(&30));
}

struct PresenceOnly;
impl ComponentAdapter for PresenceOnly {
    fn read(&self, _: &World, _: Entity) -> Result<Option<Value>, String> {
        Err("value cannot be serialized".into())
    }
    fn write(&self, _: &mut World, _: Entity, _: Value) -> Result<(), String> {
        Err("read only".into())
    }
    fn remove(&self, _: &mut World, _: Entity) -> Result<Option<Value>, String> {
        Err("read only".into())
    }
    fn contains(&self, world: &World, entity: Entity) -> bool {
        world.has::<u16>(entity)
    }
}

#[test]
fn borrowed_component_reads_preserve_adapter_and_json_semantics() {
    use std::borrow::Cow;

    let mut world = World::new();
    world
        .register_serde_component::<Position>("Position")
        .unwrap();
    let entity = world.spawn();
    world.insert(entity, Position { x: 1.0, y: 2.0 }).unwrap();
    let dynamic = json!({"items": [null, {"name": "one"}], "empty": []});
    world
        .set_dynamic(entity, "Inventory", dynamic.clone())
        .unwrap();

    let borrowed = world
        .borrow_component_value(entity, "Inventory")
        .unwrap()
        .unwrap();
    assert!(matches!(borrowed, Cow::Borrowed(_)));
    assert_eq!(*borrowed, dynamic);
    let native = world
        .borrow_component_value(entity, "Position")
        .unwrap()
        .unwrap();
    assert!(matches!(native, Cow::Owned(_)));
    assert_eq!(*native, json!({"x": 1.0, "y": 2.0}));

    // Direct JSON output agrees with the standard adapter and dynamic reads.
    for name in ["Position", "Inventory"] {
        let expected = world.read_component_value(entity, name).unwrap().unwrap();
        let mut bytes = Vec::new();
        assert!(world
            .write_component_json(entity, name, &mut bytes)
            .unwrap());
        assert_eq!(serde_json::from_slice::<Value>(&bytes).unwrap(), expected);
        let request = json!({"op":"get", "id":entity.to_string(), "name":name});
        let response: Value =
            serde_json::from_str(&world.apply_script_json(&request.to_string()).unwrap()).unwrap();
        assert_eq!(response, json!({"found":true, "value":expected}));
    }
    let mut unchanged = b"prefix".to_vec();
    assert!(!world
        .write_component_json(entity, "Missing", &mut unchanged)
        .unwrap());
    assert_eq!(unchanged, b"prefix");
    assert!(world
        .borrow_component_value(entity, "Missing")
        .unwrap()
        .is_none());

    let request = json!({"op":"get", "id":entity.to_string(), "name":"Inventory"});
    let response: Value =
        serde_json::from_str(&world.apply_script_json(&request.to_string()).unwrap()).unwrap();
    assert_eq!(response, json!({"found":true, "value":dynamic}));
    let missing = json!({"op":"get", "id":entity.to_string(), "name":"Missing"});
    let response: Value =
        serde_json::from_str(&world.apply_script_json(&missing.to_string()).unwrap()).unwrap();
    assert_eq!(response, json!({"found":false, "value":null}));
    assert!(world.despawn(entity));
    assert!(world.borrow_component_value(entity, "Inventory").is_err());
}

#[test]
fn batched_reads_preserve_missing_null_and_duplicate_semantics() {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    struct Counting(Arc<AtomicUsize>);
    impl ComponentAdapter for Counting {
        fn read(&self, _: &World, _: Entity) -> Result<Option<Value>, String> {
            Ok(Some(json!(self.0.fetch_add(1, Ordering::Relaxed))))
        }
        fn write(&self, _: &mut World, _: Entity, _: Value) -> Result<(), String> {
            Err("read only".into())
        }
        fn remove(&self, _: &mut World, _: Entity) -> Result<Option<Value>, String> {
            Err("read only".into())
        }
        fn contains(&self, _: &World, _: Entity) -> bool {
            true
        }
    }

    let mut world = World::new();
    world
        .register_serde_component::<Position>("Position")
        .unwrap();
    let count = Arc::new(AtomicUsize::new(0));
    world
        .register_component_adapter("Counter", Counting(count.clone()))
        .unwrap();
    let entity = world.spawn();
    world.insert(entity, Position { x: 1.0, y: 2.0 }).unwrap();
    world.set_dynamic(entity, "Nullable", Value::Null).unwrap();
    world
        .set_dynamic(entity, "quote\"key", json!("safe"))
        .unwrap();
    let names = vec![
        "Position".into(),
        "Nullable".into(),
        "Missing".into(),
        "quote\"key".into(),
    ];
    let expected = world.read_component_values(entity, &names).unwrap();
    let request = json!({"op":"get_many", "id":entity.to_string(), "names":names});
    let response: Value =
        serde_json::from_str(&world.apply_script_json(&request.to_string()).unwrap()).unwrap();
    assert_eq!(response, json!({"found":true,"value":expected}));
    let duplicates =
        json!({"op":"get_many", "id":entity.to_string(), "names":["Counter","Counter"]});
    let response: Value =
        serde_json::from_str(&world.apply_script_json(&duplicates.to_string()).unwrap()).unwrap();
    assert_eq!(response, json!({"found":true,"value":{"Counter":1}}));
    assert_eq!(count.load(Ordering::Relaxed), 2);
    let mut many = (0..32).map(|i| format!("Missing{i}")).collect::<Vec<_>>();
    many.extend(["Counter".into(), "Counter".into()]);
    let request = json!({"op":"get_many", "id":entity.to_string(), "names":many});
    let response: Value =
        serde_json::from_str(&world.apply_script_json(&request.to_string()).unwrap()).unwrap();
    assert_eq!(response, json!({"found":true,"value":{"Counter":3}}));
    assert_eq!(count.load(Ordering::Relaxed), 4);

    let empty = json!({"op":"get_many", "id":entity.to_string(), "names":[]});
    let response: Value =
        serde_json::from_str(&world.apply_script_json(&empty.to_string()).unwrap()).unwrap();
    assert_eq!(response, json!({"found":true,"value":{}}));
    assert!(world.despawn(entity));
    assert!(world.apply_script_json(&empty.to_string()).is_err());
}

#[test]
fn script_queries_and_liveness_match_world_operations() {
    let mut world = World::new();
    world
        .register_serde_component::<Position>("Position")
        .unwrap();
    world
        .register_component_adapter("ReadOnlyHealth", ReadOnlyHealth)
        .unwrap();
    let a = world.spawn();
    let b = world.spawn();
    let removed = world.spawn();
    world.insert(a, Position { x: 1.0, y: 2.0 }).unwrap();
    world.insert(b, Position { x: 3.0, y: 4.0 }).unwrap();
    world.insert(a, 42u32).unwrap();
    world.set_dynamic(a, "quoted\"name", Value::Null).unwrap();
    world
        .set_dynamic(removed, "quoted\"name", json!(true))
        .unwrap();
    assert!(world.despawn(removed));
    let reused = world.spawn();
    world
        .set_dynamic(reused, "quoted\"name", json!(true))
        .unwrap();

    for names in [
        vec![],
        vec!["Position"],
        vec!["ReadOnlyHealth"],
        vec!["quoted\"name"],
        vec!["Position", "quoted\"name"],
        vec!["Position", "Position"],
        vec!["Missing"],
        vec!["Missing", "Position"],
    ] {
        let components = names
            .iter()
            .map(|name| name.to_string())
            .collect::<Vec<_>>();
        let expected = match world
            .apply_operation(WorldOperation::Query {
                components: components.clone(),
            })
            .unwrap()
        {
            WorldOutcome::Entities(entities) => entities
                .into_iter()
                .map(|e| e.to_string())
                .collect::<Vec<_>>(),
            other => panic!("unexpected query result: {other:?}"),
        };
        let request = json!({"op":"query", "components":components});
        let actual: Value =
            serde_json::from_str(&world.apply_script_json(&request.to_string()).unwrap()).unwrap();
        assert_eq!(
            actual,
            json!({"found":true, "value":expected}),
            "query {names:?}"
        );
    }
    let invalid = json!({"op":"query", "components":["Missing",""]});
    assert!(world.apply_script_json(&invalid.to_string()).is_err());
    let invalid_single = json!({"op":"query", "components":[""]});
    assert!(world
        .apply_script_json(&invalid_single.to_string())
        .is_err());

    for (entity, alive) in [
        (a, true),
        (removed, false),
        (reused, true),
        (World::new().spawn(), false),
    ] {
        let request = json!({"op":"is_alive", "id":entity.to_string()});
        let actual: Value =
            serde_json::from_str(&world.apply_script_json(&request.to_string()).unwrap()).unwrap();
        assert_eq!(actual, json!({"found":true,"value":alive}));
    }
    let escaped = format!(
        r#"{{"op":"is_alive","id":"{}\u003a{}.{}"}}"#,
        a.world(),
        a.index(),
        a.generation()
    );
    let actual: Value = serde_json::from_str(&world.apply_script_json(&escaped).unwrap()).unwrap();
    assert_eq!(actual, json!({"found":true,"value":true}));
    assert!(world
        .apply_script_json(r#"{"op":"is_alive","id":"invalid"}"#)
        .is_err());
}

#[test]
fn component_field_writes_preserve_other_fields_and_validate_targets() {
    let mut world = World::new();
    world
        .register_serde_component::<Position>("Position")
        .unwrap();
    let entity = world.spawn();
    world.insert(entity, Position { x: 1.0, y: 2.0 }).unwrap();
    world
        .set_dynamic(entity, "State", json!({"keep":1,"nullable":null}))
        .unwrap();
    world.set_dynamic(entity, "Scalar", json!(5)).unwrap();
    let revision = world.structure_revision();

    world
        .write_component_field(entity, "State", "quoted\"field", json!([1, 2, 3]))
        .unwrap();
    assert_eq!(
        world.get_dynamic(entity, "State"),
        Some(&json!({"keep":1,"nullable":null,"quoted\"field":[1,2,3]}))
    );
    world
        .write_component_field(entity, "Position", "x", json!(8.0))
        .unwrap();
    assert_eq!(
        world.get::<Position>(entity),
        Some(&Position { x: 8.0, y: 2.0 })
    );
    assert!(world
        .write_component_field(entity, "Position", "y", json!("bad"))
        .is_err());
    assert_eq!(
        world.get::<Position>(entity),
        Some(&Position { x: 8.0, y: 2.0 })
    );
    assert!(world
        .write_component_field(entity, "Missing", "x", json!(1))
        .is_err());
    assert!(world
        .write_component_field(entity, "Scalar", "x", json!(1))
        .is_err());
    assert_eq!(world.get_dynamic(entity, "Scalar"), Some(&json!(5)));
    world
        .write_component_field(entity, "State", "__proto__", json!("safe"))
        .unwrap();
    assert_eq!(
        world.get_dynamic(entity, "State").unwrap()["__proto__"],
        "safe"
    );
    assert_eq!(world.structure_revision(), revision);

    let request = json!({"op":"set_field","id":entity.to_string(),"name":"State","field":"quoted\"field","value":null});
    let response: Value =
        serde_json::from_str(&world.apply_script_json(&request.to_string()).unwrap()).unwrap();
    assert_eq!(response, json!({"found":false,"value":null}));
    assert_eq!(
        world.get_dynamic(entity, "State").unwrap()["quoted\"field"],
        Value::Null
    );
    assert_eq!(
        world
            .apply_operation(WorldOperation::WriteComponentField {
                entity,
                name: "State".into(),
                field: "keep".into(),
                value: json!(9),
            })
            .unwrap(),
        WorldOutcome::Applied
    );
    assert_eq!(world.get_dynamic(entity, "State").unwrap()["keep"], 9);

    assert!(world.despawn(entity));
    assert!(world
        .write_component_field(entity, "State", "keep", json!(1))
        .is_err());
}

#[test]
fn batched_reads_discard_partial_output_from_an_absent_adapter() {
    struct Absent;
    impl ComponentAdapter for Absent {
        fn read(&self, _: &World, _: Entity) -> Result<Option<Value>, String> {
            Ok(None)
        }
        fn write_json(&self, _: &World, _: Entity, output: &mut Vec<u8>) -> Result<bool, String> {
            output.extend_from_slice(b"partial");
            Ok(false)
        }
        fn write(&self, _: &mut World, _: Entity, _: Value) -> Result<(), String> {
            Err("read only".into())
        }
        fn remove(&self, _: &mut World, _: Entity) -> Result<Option<Value>, String> {
            Err("read only".into())
        }
        fn contains(&self, _: &World, _: Entity) -> bool {
            false
        }
    }
    let mut world = World::new();
    world.register_component_adapter("Absent", Absent).unwrap();
    let entity = world.spawn();
    world
        .set_dynamic(entity, "Present", json!({"value": 42}))
        .unwrap();
    for names in [
        vec!["Absent", "Present"],
        vec!["Present", "Absent"],
        vec!["Absent"],
    ] {
        let expected = world
            .read_component_values(
                entity,
                &names
                    .iter()
                    .map(|name| name.to_string())
                    .collect::<Vec<_>>(),
            )
            .unwrap();
        let request = json!({"op":"get_many", "id":entity.to_string(), "names":names});
        let result: Value =
            serde_json::from_str(&world.apply_script_json(&request.to_string()).unwrap()).unwrap();
        assert_eq!(result, json!({"found":true, "value":expected}));
    }
}

#[test]
fn component_presence_does_not_copy_or_serialize_values() {
    let mut world = World::new();
    world
        .register_component_adapter("Native", PresenceOnly)
        .unwrap();
    let entity = world.spawn();
    world.insert(entity, 7_u16).unwrap();
    world.set_dynamic(entity, "Nullable", Value::Null).unwrap();

    assert!(world.has_component_value(entity, "Native").unwrap());
    assert!(world.read_component_value(entity, "Native").is_err());
    assert!(world
        .write_component_json(entity, "Native", &mut Vec::new())
        .is_err());
    let request = json!({"op":"get", "id":entity.to_string(), "name":"Native"});
    assert!(world.apply_script_json(&request.to_string()).is_err());
    assert!(world.has_component_value(entity, "Nullable").unwrap());
    assert!(!world.has_component_value(entity, "Missing").unwrap());
    assert!(world.has_component_value(entity, "").is_err());
    assert_eq!(
        world
            .apply_operation(WorldOperation::HasComponent {
                entity,
                name: "Native".into(),
            })
            .unwrap(),
        WorldOutcome::Bool(true)
    );
    let request = json!({"op": "has", "id": entity.to_string(), "name": "Nullable"});
    assert_eq!(
        world.apply_script_json(&request.to_string()).unwrap(),
        r#"{"found":true,"value":true}"#
    );

    world.remove_dynamic(entity, "Nullable");
    assert!(!world.has_component_value(entity, "Nullable").unwrap());
    assert!(world.despawn(entity));
    assert!(world.has_component_value(entity, "Native").is_err());
}

#[test]
fn sparse_queries_match_live_entities_across_components_and_reused_slots() {
    #[derive(Serialize, Deserialize)]
    struct Marker;
    let mut world = World::new();
    world
        .register_serde_component::<Position>("Position")
        .unwrap();
    world.register_serde_component::<Marker>("Marker").unwrap();
    world
        .register_component_adapter("ReadOnlyHealth", ReadOnlyHealth)
        .unwrap();
    let mut entities = Vec::new();
    for index in 0..180 {
        let entity = world.spawn();
        entities.push(entity);
        if index % 2 == 0 {
            world
                .insert(
                    entity,
                    Position {
                        x: index as f32,
                        y: 0.0,
                    },
                )
                .unwrap();
        }
        if index % 3 == 0 {
            world.insert(entity, index as u32).unwrap();
        }
        if index % 11 == 0 {
            world.insert(entity, Marker).unwrap();
        }
        if index % 5 == 0 {
            // Null is still a present dynamic component.
            world.set_dynamic(entity, "Rare", Value::Null).unwrap();
        }
        if index % 7 == 0 {
            world.set_dynamic(entity, "Tag", json!(true)).unwrap();
        }
    }
    assert!(world.despawn(entities[0]));
    let replacement = world.spawn();
    world
        .insert(replacement, Position { x: 1.0, y: 2.0 })
        .unwrap();
    world.set_dynamic(replacement, "Rare", Value::Null).unwrap();
    world.remove_dynamic(entities[35], "Rare");

    let queries: &[&[&str]] = &[
        &[],
        &["Position"],
        &["Marker"],
        &["ReadOnlyHealth"],
        &["Rare"],
        &["Missing"],
        &["Rare", "Rare"],
        &["Position", "Rare"],
        &["Rare", "Position"],
        &["Position", "ReadOnlyHealth", "Rare", "Tag"],
        &["ReadOnlyHealth", "Position"],
        &["Marker", "Position"],
        &["Position", "Marker"],
        &["Position", "Position"],
        &["Marker", "ReadOnlyHealth", "Position"],
        &["Tag", "Marker"],
        &["Marker", "Rare"],
        &["Rare", "Marker"],
    ];
    for names in queries {
        let expected = world
            .entities()
            .into_iter()
            .filter(|entity| {
                names.iter().all(|name| match *name {
                    "Position" => world.has::<Position>(*entity),
                    "ReadOnlyHealth" => world.has::<u32>(*entity),
                    "Marker" => world.has::<Marker>(*entity),
                    other => world.get_dynamic(*entity, other).is_some(),
                })
            })
            .collect::<Vec<_>>();
        assert_eq!(
            world.query_components(names).unwrap(),
            expected,
            "{names:?}"
        );
    }
    let expected_join = world.query_components(&["Position", "Marker"]).unwrap();
    let forward = world
        .query2::<Position, Marker>()
        .into_iter()
        .map(|(entity, _, _)| entity)
        .collect::<Vec<_>>();
    let reverse = world
        .query2::<Marker, Position>()
        .into_iter()
        .map(|(entity, _, _)| entity)
        .collect::<Vec<_>>();
    assert_eq!(forward, expected_join);
    assert_eq!(reverse, expected_join);
    assert!(world.query2::<Position, u64>().is_empty());
    assert_eq!(
        world
            .query2::<Position, Position>()
            .into_iter()
            .map(|(entity, _, _)| entity)
            .collect::<Vec<_>>(),
        world.with::<Position>()
    );

    assert_eq!(
        world.query_dynamic(&["Tag", "Rare"]),
        world
            .entities()
            .into_iter()
            .filter(|entity| world.get_dynamic(*entity, "Tag").is_some()
                && world.get_dynamic(*entity, "Rare").is_some())
            .collect::<Vec<_>>()
    );
    assert!(world.query_dynamic(&["Missing"]).is_empty());
    // Every name is validated, even after a missing component.
    assert!(world.query_components(&["Missing", ""]).is_err());
}

#[test]
fn hierarchy_rejects_cycles_and_defines_despawn_behavior() {
    let mut world = World::new();
    world.set_lifecycle_tracking(true);
    let parent = world.spawn();
    let child = world.spawn();
    let grandchild = world.spawn();
    world.set_parent(child, Some(parent)).unwrap();
    world.set_parent(grandchild, Some(child)).unwrap();
    assert!(world.set_parent(parent, Some(grandchild)).is_err());
    assert_eq!(world.children(parent), vec![child]);
    assert!(world.despawn(parent));
    assert_eq!(world.parent(child), None);
    assert_eq!(world.parent(grandchild), Some(child));
    assert!(world
        .drain_lifecycle()
        .contains(&LifecycleEvent::ParentChanged {
            child,
            parent: None
        }));
    assert_eq!(world.despawn_recursive(child), 2);
    assert!(world.is_empty());
}

#[test]
fn event_bursts_drain_in_emission_order() {
    let mut world = World::new();
    for index in 0..256 {
        world.emit_event("burst", json!({"index":index}));
    }
    world.emit_event("other", Value::Null);
    let burst = world.drain_events("burst");
    assert_eq!(burst.len(), 256);
    for (index, value) in burst.iter().enumerate() {
        assert_eq!(value["index"], index);
    }
    assert!(world.drain_events("burst").is_empty());
    assert_eq!(world.drain_events("other"), vec![Value::Null]);
    world.emit_event("burst", json!("again"));
    assert_eq!(world.drain_events("burst"), vec![json!("again")]);
}

#[test]
fn ordered_event_batches_preserve_later_and_reentrant_events() {
    let mut world = World::new();
    let names = ["earlier", "first", "later"];
    world.emit_event("first", json!(1));
    world.emit_event("first", json!(2));
    assert_eq!(
        world.drain_next_events(&names, 0),
        Some((1, vec![json!(1), json!(2)]))
    );

    // Events emitted while handling first are considered only if their channel
    // occurs later in this dispatch pass.
    world.emit_event("earlier", json!("next pass"));
    world.emit_event("first", json!("next pass"));
    world.emit_event("later", json!("same pass"));
    assert_eq!(
        world.drain_next_events(&names, 2),
        Some((2, vec![json!("same pass")]))
    );
    assert_eq!(world.drain_next_events(&names, 3), None);
    assert_eq!(
        world.drain_next_events(&names, 0),
        Some((0, vec![json!("next pass")]))
    );
    assert_eq!(
        world.drain_next_events(&names, 1),
        Some((1, vec![json!("next pass")]))
    );
    assert_eq!(world.drain_next_events(&names, 0), None);

    world.emit_event("later", Value::Null);
    let request = json!({"op":"drain_next", "names":names, "start":1});
    let result: Value =
        serde_json::from_str(&world.apply_script_json(&request.to_string()).unwrap()).unwrap();
    assert_eq!(
        result,
        json!({"found":true, "value":{"index":2, "events":[null]}})
    );
    let result: Value =
        serde_json::from_str(&world.apply_script_json(&request.to_string()).unwrap()).unwrap();
    assert_eq!(result, json!({"found":false, "value":null}));
}

#[test]
fn snapshots_are_owned_and_events_preserve_order() {
    let mut world = World::new();
    world
        .register_serde_component::<Position>("Position")
        .unwrap();
    let entity = world.spawn();
    world.insert(entity, Position { x: 1.0, y: 2.0 }).unwrap();
    world.set_dynamic(entity, "Label", json!("before")).unwrap();
    world.set_dynamic_resource("Clock", json!({"tick": 5}));
    world.emit_event("Hit", json!(1));
    world.emit_event("Hit", json!(2));
    let snapshot = world.snapshot().unwrap();
    world
        .write_component_value(entity, "Label", json!("after"))
        .unwrap();
    world.get_mut::<Position>(entity).unwrap().x = 10.0;
    assert_eq!(snapshot.entities[0].components["Label"], json!("before"));
    assert_eq!(snapshot.entities[0].components["Position"]["x"], json!(1.0));
    assert_eq!(snapshot.resources["Clock"], json!({"tick": 5}));
    assert_eq!(world.drain_events("Hit"), vec![json!(1), json!(2)]);
    assert!(world.drain_events("Hit").is_empty());
}

#[test]
fn schedule_runs_in_order_and_stops_after_error() {
    let mut world = World::new();
    world.insert_resource(Vec::<u8>::new());
    let mut schedule = Schedule::new();
    schedule.add("first", |w| w.resource_mut::<Vec<u8>>().unwrap().push(1));
    schedule.add_fallible("error", |w| {
        w.resource_mut::<Vec<u8>>().unwrap().push(2);
        Err("failed".into())
    });
    schedule.add("last", |w| w.resource_mut::<Vec<u8>>().unwrap().push(3));
    let error = schedule.run(&mut world).unwrap_err();
    assert_eq!(error.system, "error");
    assert_eq!(world.resource::<Vec<u8>>().unwrap(), &[1, 2]);
    schedule.set_enabled("error", false);
    schedule.run(&mut world).unwrap();
    assert_eq!(world.resource::<Vec<u8>>().unwrap(), &[1, 2, 1, 3]);
}

#[test]
fn commands_cross_threads_and_preserve_replies_and_snapshot_data() {
    fn assert_send<T: Send>() {}
    fn assert_serde<T: Serialize + for<'a> Deserialize<'a>>() {}
    assert_send::<World>();
    assert_send::<WorldCommand>();
    assert_send::<WorldReply>();
    assert_send::<WorldSnapshot>();
    assert_serde::<WorldCommand>();
    assert_serde::<WorldReply>();

    let (command_tx, command_rx) = mpsc::channel::<WorldCommand>();
    let (reply_tx, reply_rx) = mpsc::channel::<WorldReply>();
    let worker = thread::spawn(move || {
        let mut world = World::new();
        for command in command_rx {
            reply_tx.send(world.apply_command(command)).unwrap();
        }
    });
    let call = |id, operation| {
        command_tx
            .send(WorldCommand {
                request_id: id,
                operation,
            })
            .unwrap();
        let reply = reply_rx.recv().unwrap();
        assert_eq!(reply.request_id, id);
        reply.outcome.unwrap()
    };
    let entity = match call(1, WorldOperation::Spawn) {
        WorldOutcome::Spawned(entity) => entity,
        other => panic!("unexpected outcome: {other:?}"),
    };
    assert_eq!(
        call(
            2,
            WorldOperation::WriteComponent {
                entity,
                name: "Health".into(),
                value: json!(50)
            }
        ),
        WorldOutcome::Applied
    );
    assert_eq!(
        call(3, WorldOperation::Query { components: vec![] }),
        WorldOutcome::Entities(vec![entity])
    );
    assert_eq!(
        call(4, WorldOperation::Components { entity }),
        WorldOutcome::Names(vec!["Health".into()])
    );
    let snapshot = match call(5, WorldOperation::Snapshot) {
        WorldOutcome::Snapshot(snapshot) => snapshot,
        other => panic!("unexpected outcome: {other:?}"),
    };
    assert_eq!(snapshot.entities[0].components["Health"], json!(50));
    let encoded = serde_json::to_string(&WorldCommand {
        request_id: 6,
        operation: WorldOperation::IsAlive { entity },
    })
    .unwrap();
    let decoded: WorldCommand = serde_json::from_str(&encoded).unwrap();
    assert_eq!(
        call(decoded.request_id, decoded.operation),
        WorldOutcome::Bool(true)
    );
    drop(command_tx);
    worker.join().unwrap();
}

#[test]
fn multi_field_writes_apply_atomically_through_world_and_json() {
    let mut world = World::new();
    world
        .register_serde_component::<Position>("Position")
        .unwrap();
    let entity = world.spawn();
    world.insert(entity, Position { x: 1.0, y: 2.0 }).unwrap();
    world
        .set_dynamic(entity, "State", json!({"keep":1}))
        .unwrap();
    let revision = world.structure_revision();
    let fields = json!({"x":3.0,"y":4.0}).as_object().unwrap().clone();
    world
        .write_component_fields(entity, "Position", fields)
        .unwrap();
    assert_eq!(
        world.get::<Position>(entity),
        Some(&Position { x: 3.0, y: 4.0 })
    );
    let invalid = json!({"x":9.0,"y":"bad"}).as_object().unwrap().clone();
    assert!(world
        .write_component_fields(entity, "Position", invalid)
        .is_err());
    assert_eq!(
        world.get::<Position>(entity),
        Some(&Position { x: 3.0, y: 4.0 })
    );
    let request = json!({"op":"set_fields","id":entity.to_string(),"name":"State","fields":{"keep":7,"extra":null}});
    let result: Value =
        serde_json::from_str(&world.apply_script_json(&request.to_string()).unwrap()).unwrap();
    assert_eq!(result, json!({"found":false,"value":null}));
    assert_eq!(
        world.get_dynamic(entity, "State"),
        Some(&json!({"keep":7,"extra":null}))
    );
    let invalid = json!({"op":"set_fields","id":entity.to_string(),"name":"State","fields":[1,2]});
    assert!(world.apply_script_json(&invalid.to_string()).is_err());
    assert_eq!(
        world.get_dynamic(entity, "State"),
        Some(&json!({"keep":7,"extra":null}))
    );
    assert_eq!(world.structure_revision(), revision);
    assert_eq!(
        world
            .apply_operation(WorldOperation::WriteComponentFields {
                entity,
                name: "State".into(),
                fields: json!({"keep":9}).as_object().unwrap().clone()
            })
            .unwrap(),
        WorldOutcome::Applied
    );
    assert!(world.despawn(entity));
    assert!(world
        .write_component_fields(entity, "State", Default::default())
        .is_err());
}

#[test]
fn tracked_component_reads_invalidate_on_every_world_mutation_path() {
    struct Tracked;
    impl ComponentAdapter for Tracked {
        fn read(&self, world: &World, entity: Entity) -> Result<Option<Value>, String> {
            world
                .get::<Position>(entity)
                .map(serde_json::to_value)
                .transpose()
                .map_err(|e| e.to_string())
        }
        fn write(&self, world: &mut World, entity: Entity, value: Value) -> Result<(), String> {
            world.insert(
                entity,
                serde_json::from_value::<Position>(value).map_err(|e| e.to_string())?,
            )
        }
        fn remove(&self, world: &mut World, entity: Entity) -> Result<Option<Value>, String> {
            world
                .remove::<Position>(entity)
                .map(serde_json::to_value)
                .transpose()
                .map_err(|e| e.to_string())
        }
        fn contains(&self, world: &World, entity: Entity) -> bool {
            world.has::<Position>(entity)
        }
        fn content_revision(&self, world: &World) -> Option<u64> {
            world.component_revision::<Position>()
        }
    }
    let mut world = World::new();
    world
        .register_component_adapter("Tracked", Tracked)
        .unwrap();
    world.register_component_adapter("Other", Tracked).unwrap();
    let a = world.spawn();
    let b = world.spawn();
    world.insert(a, Position { x: 1.0, y: 2.0 }).unwrap();
    assert_eq!(world.component_revision::<Position>(), None);
    let untracked = world
        .read_component_if_changed(a, "Tracked", None)
        .unwrap()
        .unwrap();
    assert_eq!(untracked.revision, None);
    assert!(world
        .read_component_if_changed(a, "Tracked", Some("anything"))
        .unwrap()
        .is_some());

    world.track_component_changes::<Position>();
    let initial = world
        .read_component_if_changed(a, "Tracked", None)
        .unwrap()
        .unwrap();
    assert!(initial.present);
    let mut token = initial.revision.unwrap();
    assert!(world
        .read_component_if_changed(a, "Tracked", Some(&token))
        .unwrap()
        .is_none());
    assert!(world
        .read_component_if_changed(b, "Tracked", Some(&token))
        .unwrap()
        .is_some());
    assert!(world
        .read_component_if_changed(a, "Other", Some(&token))
        .unwrap()
        .is_some());

    // Mutably borrowing an existing value invalidates even if it is not changed.
    world.get_mut::<Position>(a).unwrap().x = 4.0;
    let changed = world
        .read_component_if_changed(a, "Tracked", Some(&token))
        .unwrap()
        .unwrap();
    assert_eq!(changed.value["x"], 4.0);
    token = changed.revision.unwrap();
    assert!(world.get_mut::<Position>(b).is_none());
    assert!(world
        .read_component_if_changed(a, "Tracked", Some(&token))
        .unwrap()
        .is_none());
    world.insert(b, Position { x: 8.0, y: 9.0 }).unwrap();
    let changed = world
        .read_component_if_changed(a, "Tracked", Some(&token))
        .unwrap()
        .unwrap();
    token = changed.revision.unwrap();
    world.remove::<Position>(a).unwrap();
    let missing = world
        .read_component_if_changed(a, "Tracked", Some(&token))
        .unwrap()
        .unwrap();
    assert!(!missing.present && missing.value == Value::Null);
    let missing_token = missing.revision.unwrap();
    assert!(world
        .read_component_if_changed(a, "Tracked", Some(&missing_token))
        .unwrap()
        .is_none());
    world.insert(a, Position { x: 2.0, y: 3.0 }).unwrap();
    let present = world
        .read_component_if_changed(a, "Tracked", Some(&missing_token))
        .unwrap()
        .unwrap();
    let revision = present.revision.unwrap();
    let request =
        json!({"op":"get_if_changed","id":a.to_string(),"name":"Tracked","revision":revision});
    let result: Value =
        serde_json::from_str(&world.apply_script_json(&request.to_string()).unwrap()).unwrap();
    assert_eq!(result, json!({"found":false,"value":null}));
    assert!(world.despawn(b));
    assert!(world
        .read_component_if_changed(a, "Tracked", Some(&revision))
        .unwrap()
        .is_some());
    assert!(world.despawn(a));
    assert!(world
        .read_component_if_changed(a, "Tracked", Some(&revision))
        .is_err());

    let c = world.spawn();
    world.set_dynamic(c, "Null", Value::Null).unwrap();
    let null = world
        .read_component_if_changed(c, "Null", None)
        .unwrap()
        .unwrap();
    assert!(null.present && null.value == Value::Null && null.revision.is_some());
    let roundtrip: countersplat_ecs::ComponentChange =
        serde_json::from_value(serde_json::to_value(&null).unwrap()).unwrap();
    assert_eq!(roundtrip, null);
    let missing = world
        .read_component_if_changed(c, "Missing", None)
        .unwrap()
        .unwrap();
    assert!(!missing.present && missing.value == Value::Null && missing.revision.is_none());
    assert!(world
        .read_component_if_changed(c, "Null", Some("any"))
        .unwrap()
        .is_some());
}

#[test]
fn dynamic_revisions_follow_all_mutations_and_never_alias_native_adapters() {
    struct FixedRevision(u64);
    impl ComponentAdapter for FixedRevision {
        fn read(&self, _: &World, _: Entity) -> Result<Option<Value>, String> {
            Ok(Some(json!(77)))
        }
        fn write(&self, _: &mut World, _: Entity, _: Value) -> Result<(), String> {
            Ok(())
        }
        fn remove(&self, _: &mut World, _: Entity) -> Result<Option<Value>, String> {
            Ok(None)
        }
        fn contains(&self, _: &World, _: Entity) -> bool {
            true
        }
        fn content_revision(&self, _: &World) -> Option<u64> {
            Some(self.0)
        }
    }

    let mut world = World::new();
    let a = world.spawn();
    let b = world.spawn();
    world
        .set_dynamic(a, "Settings", json!({"speed": 1}))
        .unwrap();
    world
        .set_dynamic(b, "Settings", json!({"speed": 2}))
        .unwrap();
    world.set_dynamic(a, "Other", json!(false)).unwrap();
    let token = |world: &World, entity, name| {
        world
            .read_component_if_changed(entity, name, None)
            .unwrap()
            .unwrap()
            .revision
            .unwrap()
    };
    let mut settings = token(&world, a, "Settings");
    let other = token(&world, a, "Other");
    assert!(world
        .read_component_if_changed(a, "Settings", Some(&settings))
        .unwrap()
        .is_none());

    // A mutation to another entity in the same store is conservative; a
    // different component name does not invalidate the cached value.
    world
        .set_dynamic(b, "Settings", json!({"speed": 3}))
        .unwrap();
    assert!(world
        .read_component_if_changed(a, "Settings", Some(&settings))
        .unwrap()
        .is_some());
    assert!(world
        .read_component_if_changed(a, "Other", Some(&other))
        .unwrap()
        .is_none());
    settings = token(&world, a, "Settings");
    world
        .write_component_field(a, "Settings", "speed", json!(4))
        .unwrap();
    assert_eq!(
        world
            .read_component_if_changed(a, "Settings", Some(&settings))
            .unwrap()
            .unwrap()
            .value["speed"],
        4
    );
    settings = token(&world, a, "Settings");
    world
        .write_component_fields(
            a,
            "Settings",
            json!({"speed": 5, "enabled": true})
                .as_object()
                .unwrap()
                .clone(),
        )
        .unwrap();
    assert_eq!(
        world
            .read_component_if_changed(a, "Settings", Some(&settings))
            .unwrap()
            .unwrap()
            .value["speed"],
        5
    );
    settings = token(&world, a, "Settings");
    assert!(world
        .write_component_field(a, "Other", "bad", json!(1))
        .is_err());
    assert!(world
        .read_component_if_changed(a, "Settings", Some(&settings))
        .unwrap()
        .is_none());
    assert!(world
        .read_component_if_changed(a, "Other", Some(&other))
        .unwrap()
        .is_none());
    assert!(world.despawn(b));
    assert!(world
        .read_component_if_changed(a, "Settings", Some(&settings))
        .unwrap()
        .is_some());
    settings = token(&world, a, "Settings");
    world.remove_dynamic(a, "Settings").unwrap();
    let missing = world
        .read_component_if_changed(a, "Settings", Some(&settings))
        .unwrap()
        .unwrap();
    assert!(!missing.present && missing.revision.is_none());
    world
        .set_dynamic(a, "Settings", json!({"speed": 6}))
        .unwrap();
    let recreated = token(&world, a, "Settings");
    assert_ne!(settings, recreated);
    assert_eq!(
        world
            .read_component_if_changed(a, "Settings", Some(&settings))
            .unwrap()
            .unwrap()
            .value["speed"],
        6
    );

    // Even an adapter with the exact same numeric revision cannot accept an
    // old token from the dynamic store that used this name.
    let sequence = recreated
        .rsplit('|')
        .next()
        .unwrap()
        .strip_prefix('d')
        .unwrap()
        .parse::<u64>()
        .unwrap();
    world.remove_dynamic(a, "Settings").unwrap();
    world
        .register_component_adapter("Settings", FixedRevision(sequence))
        .unwrap();
    let native = world
        .read_component_if_changed(a, "Settings", Some(&recreated))
        .unwrap()
        .unwrap();
    assert_eq!(native.value, json!(77));
    assert_ne!(native.revision.as_deref(), Some(recreated.as_str()));
}
