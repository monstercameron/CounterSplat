//! Exercise sustained-fire vision through the real Lua/ECS and weapon event path.
mod common;
use countersplat_ecs::{Entity, World, WorldOperation, WorldOutcome};
use countersplat_scene::SceneManifest;
use countersplat_scripting_lua::{LuaRuntime, ScriptOptions, WorldBridge};
use serde_json::json;
use std::{cell::RefCell, path::PathBuf, rc::Rc};

struct Bridge(Rc<RefCell<World>>);
impl WorldBridge for Bridge {
    fn request(&mut self, op: WorldOperation) -> Result<WorldOutcome, String> {
        self.0.borrow_mut().apply_operation(op)
    }
}
struct Fixture {
    world: Rc<RefCell<World>>,
    lua: LuaRuntime,
    player: Entity,
    weapon: Entity,
    root: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let scene = SceneManifest::from_json(
            &common::gameplay_with_ready_weapon(&root),
        )
        .unwrap();
        let world = Rc::new(RefCell::new(World::new()));
        countersplat_ui_ecs::register(&mut world.borrow_mut()).unwrap();
        countersplat_scene_ecs::register(&mut world.borrow_mut()).unwrap();
        let ids = countersplat_scene_ecs::instantiate_layers(
            &mut world.borrow_mut(),
            &scene,
            &["ui", "player"],
        )
        .unwrap();
        let lua = LuaRuntime::new(Bridge(world.clone()), ScriptOptions::default()).unwrap();
        Self {
            world,
            lua,
            player: ids["player"],
            weapon: ids["first_person"],
            root,
        }
    }
    fn load(&self, file: &str) {
        self.lua
            .execute(&std::fs::read_to_string(self.root.join("assets/scripts").join(file)).unwrap())
            .unwrap();
    }
    fn step(&self, n: usize, dt: f64) {
        for _ in 0..n {
            self.lua.run_systems(dt).unwrap();
        }
    }
    fn shot(&self) {
        self.lua
            .dispatch(
                "weapon.shot",
                &json!({"player":self.player.to_string(),"entity":self.weapon.to_string()}),
            )
            .unwrap();
    }
    fn blur(&self) -> f64 {
        self.world
            .borrow()
            .get_dynamic_resource("camera.combat_vision")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0)
    }
    fn set(&self, field: &str, value: serde_json::Value) {
        self.world
            .borrow_mut()
            .write_component_field(self.player, "CombatVision", field, value)
            .unwrap();
    }
}

#[test]
fn frequent_shots_gradually_build_bounded_blur_then_smoothly_recover() {
    let single = Fixture::new();
    single.load("combat_vision.lua");
    single.shot();
    single.step(60, 1.0 / 60.0);
    assert_eq!(
        single.blur(),
        0.0,
        "one isolated shot stays below the threshold"
    );
    let f = Fixture::new();
    f.load("combat_vision.lua");
    let mut levels = Vec::new();
    for _ in 0..18 {
        f.shot();
        f.step(6, 1.0 / 60.0);
        levels.push(f.blur());
    }
    assert!(
        levels[3] > 0.0 && levels[9] > levels[3] + 0.15 && levels[17] > levels[9],
        "buildup: {levels:?}"
    );
    assert!(
        levels.iter().all(|v| *v <= 0.48),
        "strength must remain bounded: {levels:?}"
    );
    let peak = f.blur();
    f.step(30, 1.0 / 60.0);
    let fading = f.blur();
    assert!(
        fading > 0.0 && fading < peak,
        "recovery should be gradual: {peak} -> {fading}"
    );
    f.step(600, 1.0 / 60.0);
    assert_eq!(f.blur(), 0.0, "effect should fully settle");
}

#[test]
fn only_owned_shots_apply_and_toggle_and_reset_clear_the_effect() {
    let f = Fixture::new();
    f.load("combat_vision.lua");
    for _ in 0..20 {
        f.lua
            .dispatch(
                "weapon.shot",
                &json!({"player":"another-player","entity":"enemy"}),
            )
            .unwrap();
        f.lua
            .dispatch("weapon.dry_fire", &json!({"player":f.player.to_string()}))
            .unwrap();
        f.step(6, 1.0 / 60.0);
    }
    assert_eq!(f.blur(), 0.0);
    f.set("enabled", json!(false));
    for _ in 0..20 {
        f.shot();
        f.step(6, 1.0 / 60.0);
    }
    f.set("enabled", json!(true));
    f.step(1, 1.0 / 60.0);
    assert_eq!(f.blur(), 0.0);
    for _ in 0..10 {
        f.shot();
        f.step(6, 1.0 / 60.0);
    }
    assert!(f.blur() > 0.2);
    f.lua.dispatch("input.reset", &json!({})).unwrap();
    assert_eq!(f.blur(), 0.0);
    for _ in 0..10 {
        f.shot();
        f.step(6, 1.0 / 60.0);
    }
    f.lua
        .dispatch("combat.death", &json!({"target":"another-player"}))
        .unwrap();
    assert!(f.blur() > 0.2);
    f.lua
        .dispatch("combat.death", &json!({"target":f.player.to_string()}))
        .unwrap();
    assert_eq!(f.blur(), 0.0);
    for _ in 0..10 {
        f.shot();
        f.step(6, 1.0 / 60.0);
    }
    f.set("enabled", json!(false));
    f.step(1, 1.0 / 60.0);
    assert_eq!(f.blur(), 0.0);
}

#[test]
fn buildup_is_consistent_across_fixed_tick_rates() {
    let mut values = Vec::new();
    for hz in [30usize, 60, 120] {
        let f = Fixture::new();
        f.load("combat_vision.lua");
        for _ in 0..15 {
            f.shot();
            f.step(hz / 10, 1.0 / hz as f64);
        }
        values.push(f.blur());
    }
    assert!(
        (values[0] - values[1]).abs() < 0.02 && (values[1] - values[2]).abs() < 0.02,
        "tick rates: {values:?}"
    );
}

#[test]
fn partial_config_is_normalized_and_inspector_exports_are_live() {
    let f = Fixture::new();
    f.lua.execute("local p=ecs.get_resource('scene.entities').player; ecs.set(p,'CombatVision',{enabled=false,max_blur=2})").unwrap();
    f.load("combat_vision.lua");
    f.lua.execute("local p=ecs.get_resource('scene.entities').player; local v=ecs.get(p,'CombatVision'); assert(not v.enabled and v.max_blur==1 and v.heat_per_shot==.12); engine.set_export(p,'combat_vision_enabled',true); engine.set_export(p,'combat_vision_strength',.25)").unwrap();
    for _ in 0..20 {
        f.shot();
        f.step(6, 1.0 / 60.0);
    }
    assert!(
        f.blur() > 0.18 && f.blur() <= 0.25,
        "inspector strength should apply: {}",
        f.blur()
    );
}

#[test]
fn real_automatic_weapon_shots_reach_the_vision_system_while_aiming() {
    let f = Fixture::new();
    f.load("combat_vision.lua");
    f.load("first_person_controller.lua");
    f.lua.dispatch("weapon.mode", &json!({})).unwrap();
    f.lua.dispatch("weapon.mode", &json!({})).unwrap();
    f.lua
        .dispatch("weapon.aim", &json!({"pressed":true}))
        .unwrap();
    f.lua
        .dispatch("weapon.trigger", &json!({"pressed":true}))
        .unwrap();
    let mut count = 0;
    for _ in 0..180 {
        f.step(1, 1.0 / 60.0);
        let shots = f.world.borrow_mut().drain_events("weapon.shot");
        for shot in shots {
            count += 1;
            f.lua.dispatch("weapon.shot", &shot).unwrap();
        }
    }
    f.lua
        .dispatch("weapon.trigger", &json!({"pressed":false}))
        .unwrap();
    assert!(count >= 15, "weapon should really fire: {count}");
    assert!(
        f.blur() > 0.25,
        "ADS automatic fire should build the effect: {}",
        f.blur()
    );
}
