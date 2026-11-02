//! The real Lua event adapter and transient-light service, exercised through ECS.
use countersplat_ecs::{World, WorldOperation, WorldOutcome};
use countersplat_scene::SceneManifest;
use countersplat_scripting_lua::{LuaRuntime, ScriptOptions, WorldBridge};
use serde_json::{json, Value};
use std::{cell::RefCell, path::PathBuf, rc::Rc};

struct Bridge(Rc<RefCell<World>>);
impl WorldBridge for Bridge {
    fn request(&mut self, op: WorldOperation) -> Result<WorldOutcome, String> {
        self.0.borrow_mut().apply_operation(op)
    }
}

fn fixture() -> (Rc<RefCell<World>>, LuaRuntime, String, String) {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let scene =
        SceneManifest::from_json(&std::fs::read(root.join("assets/scenes/gameplay.json")).unwrap())
            .unwrap();
    let world = Rc::new(RefCell::new(World::new()));
    countersplat_ui_ecs::register(&mut world.borrow_mut()).unwrap();
    countersplat_scene_ecs::register(&mut world.borrow_mut()).unwrap();
    let ids = countersplat_scene_ecs::instantiate_layers(
        &mut world.borrow_mut(),
        &scene,
        &["ui", "player", "level", "scene"],
    )
    .unwrap();
    let owner = ids["first_person"].to_string();
    let player = ids["player"].to_string();
    let lua = LuaRuntime::new(Bridge(world.clone()), ScriptOptions::default()).unwrap();
    for script in ["muzzle_light.lua", "flash_lighting.lua"] {
        lua.execute(&std::fs::read_to_string(root.join("assets/scripts").join(script)).unwrap())
            .unwrap();
    }
    (world, lua, owner, player)
}
fn light(world: &Rc<RefCell<World>>) -> Option<Value> {
    world
        .borrow()
        .get_dynamic_resource("render.local_light")
        .cloned()
}
fn deliver(world: &Rc<RefCell<World>>, lua: &LuaRuntime, name: &str) {
    let events = world.borrow_mut().drain_events(name);
    for event in events {
        lua.dispatch(name, &event).unwrap();
    }
}

#[test]
fn weapon_shot_emits_reusable_light_event_and_ages_after_presentation() {
    let (world, lua, owner, player) = fixture();
    lua.dispatch(
        "weapon.shot",
        &json!({"entity":owner,"player":player,"shot":1,
        "origin":[1,2,3],"direction":[0,0,-2]}),
    )
    .unwrap();
    let envelope = world
        .borrow_mut()
        .drain_events("engine.event")
        .pop()
        .unwrap();
    assert_eq!(envelope["name"], "lighting.flash");
    let flash = &envelope["payload"];
    assert_eq!(flash["source"], owner);
    assert!((flash["position"][2].as_f64().unwrap() - 2.35).abs() < 1e-5);
    assert!(flash["intensity"].as_f64().unwrap() <= 0.4);
    lua.dispatch("engine.event", &envelope).unwrap();
    let started = world
        .borrow_mut()
        .drain_events("lighting.flash_started")
        .pop()
        .unwrap();
    let first = light(&world).unwrap();
    assert_eq!(first["position"], flash["position"]);
    // Catch-up ticks must not consume a flash that hasn't reached the canvas.
    for _ in 0..4 {
        lua.run_systems(0.1).unwrap();
    }
    assert_eq!(light(&world).unwrap(), first);
    lua.dispatch("lighting.flash_presented", &started).unwrap();
    lua.run_systems(0.03).unwrap();
    let fading = light(&world).unwrap();
    assert!(fading["intensity"].as_f64().unwrap() < first["intensity"].as_f64().unwrap());
    lua.run_systems(0.1).unwrap();
    assert!(light(&world).is_none());
}

#[test]
fn generic_events_validate_replace_and_clear_without_weapon_dependency() {
    let (world, lua, _, _) = fixture();
    let flash = json!({"position":[4,5,6],"radius":2,"color":[1,0.5,0.25],"intensity":0.4,"duration_s":0.1});
    lua.dispatch(
        "engine.event",
        &json!({"name":"lighting.flash","payload":{"position":[0,0,0],"radius":0}}),
    )
    .unwrap();
    assert!(light(&world).is_none());
    lua.dispatch(
        "engine.event",
        &json!({"name":"lighting.flash","payload":flash}),
    )
    .unwrap();
    let first = world
        .borrow_mut()
        .drain_events("lighting.flash_started")
        .pop()
        .unwrap();
    lua.dispatch(
        "engine.event",
        &json!({"name":"lighting.flash","payload":flash}),
    )
    .unwrap();
    let second = world
        .borrow_mut()
        .drain_events("lighting.flash_started")
        .pop()
        .unwrap();
    assert_ne!(first["id"], second["id"]);
    lua.dispatch("lighting.flash_presented", &first).unwrap();
    lua.run_systems(0.2).unwrap();
    assert!(
        light(&world).is_some(),
        "an old acknowledgement must not clear a newer flash"
    );
    lua.dispatch("input.reset", &json!({})).unwrap();
    assert!(light(&world).is_none());
}

#[test]
fn disabled_muzzle_light_does_not_emit_and_other_shots_are_ignored() {
    let (world, lua, owner, player) = fixture();
    lua.dispatch(
        "weapon.shot",
        &json!({"entity":"other","player":player,"origin":[0,0,0],"direction":[0,0,-1]}),
    )
    .unwrap();
    assert!(world.borrow_mut().drain_events("engine.event").is_empty());
    let entity = world.borrow().query_dynamic(&["MuzzleLight"])[0];
    let mut cfg = world
        .borrow()
        .get_dynamic(entity, "MuzzleLight")
        .unwrap()
        .clone();
    cfg["enabled"] = json!(false);
    world
        .borrow_mut()
        .set_dynamic(entity, "MuzzleLight", cfg)
        .unwrap();
    lua.dispatch(
        "weapon.shot",
        &json!({"entity":owner,"player":player,"origin":[0,0,0],"direction":[0,0,-1]}),
    )
    .unwrap();
    deliver(&world, &lua, "engine.event");
    assert!(light(&world).is_none());
}

#[test]
fn arbitrary_named_events_are_queued_and_subscribers_can_be_removed() {
    let (world, lua, _, _) = fixture();
    lua.execute(r#"
        callback = engine.events.on('test.signal', function(event)
            ecs.set_resource('test.value', event.value)
            engine.events.emit('test.followup', {value=event.value+1})
        end)
        engine.events.on('test.followup', function(event) ecs.set_resource('test.followup', event.value) end)
        engine.events.emit('test.signal', {value=7})
    "#).unwrap();
    deliver(&world, &lua, "engine.event");
    assert_eq!(
        world.borrow().get_dynamic_resource("test.value"),
        Some(&json!(7))
    );
    assert!(world
        .borrow()
        .get_dynamic_resource("test.followup")
        .is_none());
    deliver(&world, &lua, "engine.event");
    assert_eq!(
        world.borrow().get_dynamic_resource("test.followup"),
        Some(&json!(8))
    );
    lua.execute(r#"assert(engine.events.off('test.signal', callback)); engine.events.emit('test.signal', {value=9})"#).unwrap();
    deliver(&world, &lua, "engine.event");
    assert_eq!(
        world.borrow().get_dynamic_resource("test.value"),
        Some(&json!(7))
    );
}
