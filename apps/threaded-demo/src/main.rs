//! A small native host demonstrating data flowing between real subsystem owners.
//!
//! This is a one-shot integration example, not the interactive game loop. Lua
//! creates an ECS entity, physics simulates a body from its position, and the
//! host publishes the resulting pose back to ECS for Lua to observe.

mod live;

use countersplat_ecs::{World as EcsWorld, WorldOperation, WorldOutcome};
use countersplat_physics::world::{BodyKind, BoxBody, World as PhysicsWorld};
use countersplat_runtime::native::{Client, Worker};
use countersplat_scripting_lua::{ScriptOptions, ScriptWorker, WorldBridge};
use serde_json::{json, Value};
use std::{
    sync::mpsc,
    thread::{self, ThreadId},
    time::Duration,
};

const TIMEOUT: Duration = Duration::from_secs(10);
const QUEUE_CAPACITY: usize = 16;
const SCRIPT: &str = r#"
    actor = ecs.spawn()
    ecs.set(actor, "Position", { x = 0.0, y = 5.0, z = 0.0 })
    ecs.system("observe_physics", function(dt)
        local position = ecs.get(actor, "Position")
        ecs.set_resource("script_observed_y", position.y)
    end)
"#;

/// A bounded, request/reply bridge. Lua holds a client, never a world or lock.
struct EcsBridge {
    client: Client<WorldOperation, WorldOutcome>,
    lua_thread: Option<mpsc::Sender<ThreadId>>,
}

impl WorldBridge for EcsBridge {
    fn request(&mut self, operation: WorldOperation) -> Result<WorldOutcome, String> {
        if let Some(sender) = self.lua_thread.take() {
            let _ = sender.send(thread::current().id());
        }
        call_ecs(&self.client, operation)
    }
}

fn call_ecs(
    client: &Client<WorldOperation, WorldOutcome>,
    operation: WorldOperation,
) -> Result<WorldOutcome, String> {
    client
        .try_submit(operation)
        .map_err(|e| e.to_string())?
        .wait_timeout(TIMEOUT)
        .map_err(|e| e.to_string())
}

/// A physics request and reply are owned values rather than backend references.
#[derive(Debug)]
struct SimulateBody {
    start: [f32; 3],
    ticks: u32,
}

fn simulate(world: &mut PhysicsWorld, command: SimulateBody) -> Result<[f32; 3], String> {
    let body = world
        .add_box(BoxBody {
            kind: BodyKind::Dynamic,
            center: command.start,
            half_extents: [0.5; 3],
            friction: 0.5,
            restitution: 0.0,
        })
        .map_err(str::to_owned)?;
    for _ in 0..command.ticks {
        world.step(1.0 / 60.0).map_err(str::to_owned)?;
    }
    let position = world.position(body).ok_or("physics body disappeared")?;
    if !world.remove(body) {
        return Err("could not remove demo body".into());
    }
    Ok(position)
}

fn physics_world() -> Result<PhysicsWorld, String> {
    let mut world = PhysicsWorld::new([0.0, -9.81, 0.0], 2).map_err(str::to_owned)?;
    world
        .add_box(BoxBody {
            kind: BodyKind::Fixed,
            center: [0.0, -0.5, 0.0],
            half_extents: [10.0, 0.5, 10.0],
            friction: 0.5,
            restitution: 0.0,
        })
        .map_err(str::to_owned)?;
    Ok(world)
}

fn position(value: &Value) -> Result<[f32; 3], String> {
    let coordinate = |name| {
        value
            .get(name)
            .and_then(Value::as_f64)
            .map(|n| n as f32)
            .ok_or_else(|| format!("missing numeric position.{name}"))
    };
    Ok([coordinate("x")?, coordinate("y")?, coordinate("z")?])
}

#[derive(Debug)]
struct Report {
    host: ThreadId,
    ecs: ThreadId,
    lua: ThreadId,
    physics: ThreadId,
    start: [f32; 3],
    end: [f32; 3],
    script_observed_y: f64,
}

fn run() -> Result<Report, String> {
    // The world and physics state are each created, used and dropped on their
    // own worker. The host remains free to coordinate without borrowing either.
    let mut ecs = Worker::spawn(
        "countersplat-simulation",
        QUEUE_CAPACITY,
        || Ok(EcsWorld::new()),
        |world, operation| world.apply_operation(operation),
    )
    .map_err(|e| e.to_string())?;
    let mut physics = Worker::spawn(
        "countersplat-physics",
        QUEUE_CAPACITY,
        physics_world,
        simulate,
    )
    .map_err(|e| e.to_string())?;
    let ecs_thread = ecs.wait_ready(TIMEOUT).map_err(|e| e.to_string())?;
    let physics_thread = physics.wait_ready(TIMEOUT).map_err(|e| e.to_string())?;
    let ecs_client = ecs.client();
    let (thread_sender, thread_receiver) = mpsc::channel();
    let lua = ScriptWorker::spawn(
        EcsBridge {
            client: ecs_client.clone(),
            lua_thread: Some(thread_sender),
        },
        ScriptOptions::default(),
    )?;

    // ECS continues serving bridge requests while the host waits for Lua.
    lua.execute(SCRIPT)?
        .recv_timeout(TIMEOUT)
        .map_err(|e| e.to_string())??;
    let lua_thread = thread_receiver
        .recv_timeout(TIMEOUT)
        .map_err(|e| e.to_string())?;
    let entities = match call_ecs(
        &ecs_client,
        WorldOperation::Query {
            components: vec!["Position".into()],
        },
    )? {
        WorldOutcome::Entities(entities) => entities,
        result => return Err(format!("unexpected query result: {result:?}")),
    };
    let [entity] = entities.as_slice() else {
        return Err("expected one scripted actor".into());
    };
    let initial = match call_ecs(
        &ecs_client,
        WorldOperation::ReadComponent {
            entity: *entity,
            name: "Position".into(),
        },
    )? {
        WorldOutcome::Value(Some(value)) => position(&value)?,
        result => return Err(format!("unexpected position result: {result:?}")),
    };
    let end = physics
        .client()
        .try_submit(SimulateBody {
            start: initial,
            ticks: 120,
        })
        .map_err(|e| e.to_string())?
        .wait_timeout(TIMEOUT)
        .map_err(|e| e.to_string())?;
    match call_ecs(
        &ecs_client,
        WorldOperation::WriteComponent {
            entity: *entity,
            name: "Position".into(),
            value: json!({"x": end[0], "y": end[1], "z": end[2]}),
        },
    )? {
        WorldOutcome::Applied => (),
        result => return Err(format!("unexpected write result: {result:?}")),
    }
    lua.run_system("observe_physics", 1.0 / 60.0)?
        .recv_timeout(TIMEOUT)
        .map_err(|e| e.to_string())??;
    let observed = match call_ecs(
        &ecs_client,
        WorldOperation::ReadResource {
            name: "script_observed_y".into(),
        },
    )? {
        WorldOutcome::Value(Some(value)) => {
            value.as_f64().ok_or("script observation is not a number")?
        }
        result => return Err(format!("unexpected script observation: {result:?}")),
    };

    // Lua may synchronously ask the ECS owner for data, so stop Lua first.
    lua.shutdown()?;
    physics.shutdown().map_err(|e| e.to_string())?;
    ecs.shutdown().map_err(|e| e.to_string())?;
    Ok(Report {
        host: thread::current().id(),
        ecs: ecs_thread,
        lua: lua_thread,
        physics: physics_thread,
        start: initial,
        end,
        script_observed_y: observed,
    })
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.as_slice() {
        [] => run().map(|report| println!("Host {:?}; ECS {:?}; Lua {:?}; physics {:?}\nBody {:?} -> {:?}; Lua observed y = {:.4}",
            report.host, report.ecs, report.lua, report.physics, report.start, report.end, report.script_observed_y)),
        [flag] if flag == "--realtime" => live::run_seconds(3.0),
        [flag, seconds] if flag == "--realtime" => seconds.parse::<f64>()
            .map_err(|_| "duration must be a positive finite number of seconds".to_string())
            .and_then(live::run_seconds),
        _ => Err("usage: countersplat-threaded-demo [--realtime [seconds]]".to_string()),
    };
    if let Err(error) = result {
        eprintln!("threaded demo failed: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn real_subsystems_exchange_state_on_distinct_threads() {
        let report = run().expect("threaded integration should complete");
        let owners = [report.host, report.ecs, report.lua, report.physics];
        for (index, owner) in owners.iter().enumerate() {
            assert!(
                !owners[..index].contains(owner),
                "owner threads must be distinct: {report:?}"
            );
        }
        assert_eq!(report.start, [0.0, 5.0, 0.0]);
        assert!(
            (0.45..0.56).contains(&report.end[1]),
            "body should land on floor: {report:?}"
        );
        assert!(
            (report.script_observed_y - f64::from(report.end[1])).abs() < 1e-6,
            "Lua should observe the physics pose published to ECS: {report:?}"
        );
    }
}
