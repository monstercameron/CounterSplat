# CounterSplat ECS

A thread-owned entity/component world with typed Rust components and explicit,
language-neutral interfaces. The ECS crate has no Lua or renderer dependency.

## Typed components and script-visible values

Ordinary Rust components stay private to native code. Register a serializable
component when scripts or other subsystems should be able to read and write it.
Dynamic components are useful for data defined at runtime.

```rust
use countersplat_ecs::{World, WorldOperation, WorldOutcome};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, Serialize, Deserialize)]
struct Position {
    x: f32,
    y: f32,
}

fn main() -> Result<(), String> {
    let mut world = World::new();
    world.register_serde_component::<Position>("Position")?;

    let car = world.spawn();
    world.insert(car, Position { x: 1.0, y: 2.0 })?;
    world.set_dynamic(car, "Health", json!(100))?;

    // Both registered native and dynamic components participate in queries.
    assert_eq!(world.query_components(&["Position", "Health"])?, vec![car]);

    // This write updates the native Position; there is no separate script copy.
    world.write_component_value(car, "Position", json!({"x": 3.0, "y": 4.0}))?;
    assert_eq!(world.get::<Position>(car).unwrap().x, 3.0);

    let result = world.apply_operation(WorldOperation::Query {
        components: vec!["Health".into()],
    })?;
    assert_eq!(result, WorldOutcome::Entities(vec![car]));
    Ok(())
}
```

Implement `ComponentAdapter` for custom validation or restricted access.
Entity handles include a world identifier and generation: a despawned handle
or one from another world cannot address a living entity. Their string form is
process-local and should not be used as a persistent save identifier.

## Commands across threads

Create and own the world on the simulation thread. Other threads exchange
owned `WorldCommand` and `WorldReply` values over a transport of their choice;
request IDs correlate replies. The world is never shared in this example.

```rust
use countersplat_ecs::{World, WorldCommand, WorldOperation, WorldOutcome, WorldReply};
use std::{sync::mpsc, thread};

fn main() -> Result<(), String> {
    let (commands, incoming) = mpsc::channel::<WorldCommand>();
    let (outgoing, replies) = mpsc::channel::<WorldReply>();
    let worker = thread::spawn(move || {
        let mut world = World::new();
        while let Ok(command) = incoming.recv() {
            if outgoing.send(world.apply_command(command)).is_err() {
                break;
            }
        }
    });

    commands.send(WorldCommand {
        request_id: 1,
        operation: WorldOperation::Spawn,
    }).map_err(|error| error.to_string())?;

    let reply = replies.recv().map_err(|error| error.to_string())?;
    assert_eq!(reply.request_id, 1);
    let entity = match reply.outcome? {
        WorldOutcome::Spawned(entity) => entity,
        other => return Err(format!("unexpected result: {other:?}")),
    };
    println!("spawned {entity}");

    drop(commands);
    worker.join().map_err(|_| "simulation thread panicked".to_owned())?;
    Ok(())
}
```

Commands also cover component reads and writes, queries, resources, events,
hierarchy, and snapshots. An empty query matches every living entity. Snapshots
own their data and include dynamic values and registered native components;
native resources remain private unless explicitly exposed as dynamic resources.
Event queues are drained separately.

## API reference

- [World storage, adapters, resources, events, and hierarchy](src/world.rs)
- [Commands, outcomes, and snapshots](src/bridge.rs)
- [Entity handles](src/entity.rs)
- [Ordered systems](src/schedule.rs)

Generate searchable API documentation with `cargo doc -p countersplat-ecs --open`.
Run the crate tests with `cargo test -p countersplat-ecs`.