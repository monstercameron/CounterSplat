use countersplat_ecs::{Entity, World, WorldOperation, WorldOutcome, WorldReply};
use countersplat_scripting_lua::{
    ChannelBridge, LuaRuntime, ScriptOptions, ScriptWorker, WorldBridge, WorldRequest,
};
use serde_json::json;
use std::{str::FromStr, sync::mpsc, thread, time::Duration};

struct NoWorld;
impl WorldBridge for NoWorld {
    fn request(&mut self, _: WorldOperation) -> Result<WorldOutcome, String> {
        Err("no ECS available".into())
    }
}

fn finish<T>(reply: countersplat_scripting_lua::ScriptReply<T>) -> T {
    reply
        .recv_timeout(Duration::from_secs(5))
        .expect("script worker did not reply")
        .expect("script failed")
}

#[test]
fn scripting_worker_operates_on_a_separate_ecs_owner() {
    let (sender, receiver) = mpsc::sync_channel::<WorldRequest>(16);
    let ecs = thread::spawn(move || {
        let mut world = World::new();
        world.register_serde_component::<i32>("Health").unwrap();
        while let Ok(request) = receiver.recv() {
            let result = world.apply_command(request.command);
            let _ = request.reply.send(result);
        }
        world
    });
    let worker = ScriptWorker::spawn(ChannelBridge::new(sender), ScriptOptions::default()).unwrap();

    finish(
        worker
            .execute(
                r#"
        assert(io == nil and os == nil and package == nil and debug == nil)
        assert(require == nil and dofile == nil and loadfile == nil and coroutine == nil)
        assert(not pcall(function() ecs.set_resource("bad_number", 0/0) end))
        local cycle = {}; cycle.self = cycle
        assert(not pcall(function() ecs.set_resource("cycle", cycle) end))
        local graph = {leaf = true}
        for i = 1, 18 do graph = {graph, graph} end
        assert(not pcall(function() ecs.set_resource("graph", graph) end))
        root = ecs.spawn()
        child = ecs.spawn()
        assert(root ~= child and ecs.is_alive(root))
        ecs.set(root, "Health", 50)
        ecs.set(root, "Position", {x=1, y=2})
        ecs.set(child, "Position", {x=9, y=3})
        ecs.set_parent(child, root)
        assert(ecs.parent(child) == root and ecs.children(root)[1] == child)
        assert(#ecs.entities() == 2 and #ecs.query({"Position"}) == 2)
        assert(#ecs.query({"Health", "Position"}) == 1)
        assert(ecs.has(root, "Health") and not ecs.has(child, "Health"))
        assert(#ecs.components(root) == 2)
        ecs.set_resource("Game", {score=2, items=ecs.array(), marker=ecs.null})
        local game = ecs.get_resource("Game")
        assert(game.score == 2 and #game.items == 0 and game.marker == ecs.null)
        local snapshot = ecs.snapshot()
        assert(#snapshot.entities == 2 and snapshot.entities[1].id == root)
        assert(snapshot.entities[2].parent == root and snapshot.entities[1].components.Health == 50)
        assert(snapshot.resources.Game.score == 2)
        ecs.emit("hit", {damage=3})
        local events = ecs.drain("hit")
        assert(#events == 1 and events[1].damage == 3 and #ecs.drain("hit") == 0)
        ecs.system("move", function(dt)
            local position = ecs.get(root, "Position")
            position.x = position.x + dt
            ecs.set(root, "Position", position)
            ecs.set_resource("order", "move")
        end)
        ecs.system("finish", function(dt)
            assert(ecs.get_resource("order") == "move")
            ecs.set_resource("order", "done")
        end)
        ecs.on("heal", function(event)
            ecs.set(root, "Health", ecs.get(root, "Health") + event.amount)
        end)
        ecs.on("heal", function(event)
            ecs.set_resource("last_heal", event.amount)
        end)
    "#,
            )
            .unwrap(),
    );
    assert_eq!(finish(worker.systems().unwrap()), vec!["move", "finish"]);
    finish(worker.run_systems(0.5).unwrap());
    finish(worker.dispatch("heal", json!({"amount": 7})).unwrap());
    finish(
        worker
            .execute(
                r#"
        assert(ecs.get(root, "Position").x == 1.5)
        assert(ecs.get(root, "Health") == 57 and ecs.get_resource("last_heal") == 7)
        assert(ecs.get_resource("order") == "done")
        assert(ecs.remove(child, "Position").x == 9)
        assert(ecs.get(child, "Position") == nil)
        assert(ecs.despawn(child) == 1 and not ecs.is_alive(child))
        assert(not pcall(function() ecs.set(child, "Position", {x=1}) end))
        ecs.set_resource("root_id", root)
        assert(ecs.remove_resource("order") == "done")
        ecs.off("heal")
        assert(ecs.remove_system("finish"))
    "#,
            )
            .unwrap(),
    );
    assert_eq!(finish(worker.systems().unwrap()), vec!["move"]);
    finish(worker.dispatch("heal", json!({"amount": 99})).unwrap());
    worker.shutdown().unwrap();
    let world = ecs.join().unwrap();
    let id = Entity::from_str(
        world
            .get_dynamic_resource("root_id")
            .unwrap()
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(world.get::<i32>(id), Some(&57));
    assert_eq!(
        world.get_dynamic(id, "Position"),
        Some(&json!({"x": 1.5, "y": 2}))
    );
    assert_eq!(
        world.get_dynamic_resource("Game").unwrap(),
        &json!({"score": 2, "items": [], "marker": null})
    );
}

#[test]
fn lua_errors_and_limits_are_reported_and_reset() {
    let options = ScriptOptions {
        instruction_limit: Some(10_000),
        ..ScriptOptions::default()
    };
    let runtime = LuaRuntime::new(NoWorld, options).unwrap();
    assert!(runtime
        .execute("while true do end")
        .unwrap_err()
        .to_string()
        .contains("instruction budget"));
    runtime
        .execute("answer = 42; assert(answer == 42)")
        .unwrap();
    assert!(runtime
        .execute("ecs.spawn()")
        .unwrap_err()
        .to_string()
        .contains("no ECS available"));
    assert!(runtime
        .run_system("missing", 0.5)
        .unwrap_err()
        .to_string()
        .contains("unknown Lua system"));
    assert!(runtime.run_systems(f64::NAN).is_err());
    assert!(runtime.execute("ecs.system('', function() end)").is_err());
    assert!(runtime
        .execute("ecs.set_resource('bad', function() end)")
        .is_err());
}

#[test]
fn channel_bridge_validates_correlation_and_reports_disconnect() {
    let (sender, receiver) = mpsc::sync_channel::<WorldRequest>(16);
    let responder = thread::spawn(move || {
        let request = receiver.recv().unwrap();
        request
            .reply
            .send(WorldReply {
                request_id: request.command.request_id + 1,
                outcome: Ok(WorldOutcome::Applied),
            })
            .unwrap();
    });
    let mut bridge = ChannelBridge::new(sender);
    assert!(bridge
        .request(WorldOperation::Spawn)
        .unwrap_err()
        .contains("mismatch"));
    responder.join().unwrap();
    assert!(bridge
        .request(WorldOperation::Spawn)
        .unwrap_err()
        .contains("unavailable"));
}

#[test]
fn worker_queue_reports_backpressure() {
    struct PausedBridge {
        started: mpsc::Sender<()>,
        proceed: mpsc::Receiver<()>,
    }
    impl WorldBridge for PausedBridge {
        fn request(&mut self, _: WorldOperation) -> Result<WorldOutcome, String> {
            self.started.send(()).unwrap();
            self.proceed
                .recv_timeout(Duration::from_secs(5))
                .map_err(|e| e.to_string())?;
            Ok(WorldOutcome::Spawned(Entity::from_str("1:0.0").unwrap()))
        }
    }
    let (started, start_signal) = mpsc::channel();
    let (continue_signal, proceed) = mpsc::channel();
    let options = ScriptOptions {
        worker_queue_capacity: 1,
        ..ScriptOptions::default()
    };
    let worker = ScriptWorker::spawn(PausedBridge { started, proceed }, options).unwrap();
    let running = worker.execute("ecs.spawn()").unwrap();
    start_signal.recv_timeout(Duration::from_secs(5)).unwrap();
    let queued = worker.execute("assert(true)").unwrap();
    assert!(worker
        .execute("assert(true)")
        .unwrap_err()
        .contains("queue is full"));
    continue_signal.send(()).unwrap();
    finish(running);
    finish(queued);
    worker.shutdown().unwrap();
}
