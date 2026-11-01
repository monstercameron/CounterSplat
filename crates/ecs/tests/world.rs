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
