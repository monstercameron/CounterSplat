//! Real scene ECS + Lua event/lifetime coverage for impact effects.
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
fn fixture() -> (Rc<RefCell<World>>, LuaRuntime, String) {
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
    let entity = ids["impact_effects"].to_string();
    let lua = LuaRuntime::new(Bridge(world.clone()), ScriptOptions::default()).unwrap();
    lua.execute(&std::fs::read_to_string(root.join("assets/scripts/impact_effects.lua")).unwrap())
        .unwrap();
    (world, lua, entity)
}
fn snapshot(world: &Rc<RefCell<World>>) -> Value {
    world
        .borrow_mut()
        .drain_events("impact.visuals")
        .pop()
        .unwrap()
}
#[test]
fn level_hit_spawns_surface_mark_and_fading_smoke_with_bounded_lifetime() {
    let (world, lua, _) = fixture();
    lua.dispatch(
        "projectile.impact",
        &json!({"kind":"level","position":[2,3,4],"normal":[0,0,2],"shot":4}),
    )
    .unwrap();
    lua.run_systems(1.0 / 60.0).unwrap();
    let first = snapshot(&world);
    assert_eq!(first["decals"].as_array().unwrap().len(), 1);
    assert_eq!(first["smoke"].as_array().unwrap().len(), 5);
    let decal = &first["decals"][0];
    assert_eq!(decal["normal"], json!([0.0, 0.0, 1.0]));
    assert!(decal["position"][2].as_f64().unwrap() > 4.0);
    assert!(decal["radius"].as_f64().unwrap() > 0.08);
    let start = &first["smoke"][0];
    lua.run_systems(0.25).unwrap();
    let later = snapshot(&world);
    assert!(later["smoke"][0]["radius"].as_f64().unwrap() > start["radius"].as_f64().unwrap());
    assert!(
        later["smoke"][0]["position"][1].as_f64().unwrap() > start["position"][1].as_f64().unwrap()
    );
    for _ in 0..8 {
        lua.run_systems(0.25).unwrap();
    }
    assert!(snapshot(&world)["smoke"].as_array().unwrap().is_empty());
    for _ in 0..120 {
        lua.run_systems(0.25).unwrap();
    }
    let cleared = snapshot(&world);
    assert!(cleared["decals"].as_array().unwrap().is_empty());
    assert!(cleared["smoke"].as_array().unwrap().is_empty());
}
#[test]
fn ignores_non_level_and_invalid_hits_and_honors_live_toggles() {
    let (world, lua, entity) = fixture();
    for value in [
        json!({"kind":"entity","position":[0,0,0],"normal":[0,1,0]}),
        json!({"kind":"level","position":[0,0,0],"normal":[0,0,0]}),
    ] {
        lua.dispatch("projectile.impact", &value).unwrap();
    }
    lua.run_systems(0.02).unwrap();
    assert!(world.borrow_mut().drain_events("impact.visuals").is_empty());
    lua.dispatch(
        "projectile.impact",
        &json!({"kind":"level","position":[0,0,0],"normal":[0,1,0]}),
    )
    .unwrap();
    lua.run_systems(0.02).unwrap();
    assert_eq!(snapshot(&world)["decals"].as_array().unwrap().len(), 1);
    let mut w = world.borrow_mut();
    let id = w.query_dynamic(&["ImpactEffects"])[0];
    assert_eq!(id.to_string(), entity);
    let mut cfg = w.get_dynamic(id, "ImpactEffects").unwrap().clone();
    cfg["enabled"] = json!(false);
    w.set_dynamic(id, "ImpactEffects", cfg).unwrap();
    drop(w);
    lua.run_systems(0.02).unwrap();
    let cleared = snapshot(&world);
    assert!(cleared["decals"].as_array().unwrap().is_empty());
    assert!(cleared["smoke"].as_array().unwrap().is_empty());
}
#[test]
fn effects_are_bounded_and_smoke_can_be_disabled_independently() {
    let (world, lua, _) = fixture();
    for shot in 0..200 {
        lua.dispatch(
            "projectile.impact",
            &json!({"kind":"level","position":[1,2,3],"normal":[1,0,0],"shot":shot}),
        )
        .unwrap();
    }
    lua.run_systems(0.02).unwrap();
    let visuals = snapshot(&world);
    assert_eq!(visuals["decals"].as_array().unwrap().len(), 96);
    assert_eq!(visuals["smoke"].as_array().unwrap().len(), 160);
    let mut w = world.borrow_mut();
    let id = w.query_dynamic(&["ImpactEffects"])[0];
    let mut cfg = w.get_dynamic(id, "ImpactEffects").unwrap().clone();
    cfg["smoke_enabled"] = json!(false);
    w.set_dynamic(id, "ImpactEffects", cfg).unwrap();
    drop(w);
    lua.run_systems(0.02).unwrap();
    let visuals = snapshot(&world);
    assert_eq!(visuals["decals"].as_array().unwrap().len(), 96);
    assert!(visuals["smoke"].as_array().unwrap().is_empty());
}
