//! Exercise the scene's real Lua weapon controller against the ECS bridge.
use countersplat_ecs::{Entity, World, WorldOperation, WorldOutcome};
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
struct Fixture {
    world: Rc<RefCell<World>>,
    lua: LuaRuntime,
    entity: Entity,
}
impl Fixture {
    fn new() -> Self {
        Self::with_config(None)
    }
    fn with_config(config: Option<Value>) -> Self {
        Self::with_controller(config, true)
    }
    fn with_controller(config: Option<Value>, load_controller: bool) -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut scene = SceneManifest::from_json(
            &std::fs::read(root.join("assets/scenes/gameplay.json")).unwrap(),
        )
        .unwrap();
        if let Some(config) = config {
            scene
                .entities
                .iter_mut()
                .find(|entity| entity.id == "first_person")
                .unwrap()
                .components
                .insert("WeaponConfig".into(), config);
        }
        let world = Rc::new(RefCell::new(World::new()));
        countersplat_ui_ecs::register(&mut world.borrow_mut()).unwrap();
        countersplat_scene_ecs::register(&mut world.borrow_mut()).unwrap();
        let ids = countersplat_scene_ecs::instantiate_layers(
            &mut world.borrow_mut(),
            &scene,
            &["ui", "player"],
        )
        .unwrap();
        let entity = ids["first_person"];
        let lua = LuaRuntime::new(Bridge(world.clone()), ScriptOptions::default()).unwrap();
        if load_controller {
            lua.execute(
                &std::fs::read_to_string(root.join("assets/scripts/first_person_controller.lua"))
                    .unwrap(),
            )
            .unwrap();
        }
        Self { world, lua, entity }
    }
    fn tick(&self, n: usize) {
        for _ in 0..n {
            self.lua.run_systems(1.0 / 60.0).unwrap();
        }
    }
    fn state(&self) -> Value {
        self.world
            .borrow()
            .get_dynamic(self.entity, "WeaponState")
            .unwrap()
            .clone()
    }
    fn mouse(&self, button: &str, pressed: bool) {
        self.lua
            .dispatch(
                "input.mouse_button",
                &json!({"button":button,"pressed":pressed,"gameplay":true}),
            )
            .unwrap();
    }
    fn key(&self, code: &str, pressed: bool) {
        self.lua
            .dispatch(
                "input.key",
                &json!({"code":code,"pressed":pressed,"is_repeat":false,"gameplay":true}),
            )
            .unwrap();
    }
}
#[test]
fn weapon_input_and_state_transitions_require_the_lua_controller() {
    let f = Fixture::with_controller(None, false);
    let initial = f.state();
    f.mouse("left", true);
    f.mouse("left", false);
    f.tick(40);
    assert_eq!(f.state(), initial);
    assert!(f.world.borrow_mut().drain_events("weapon.shot").is_empty());

    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    f.lua
        .execute(
            &std::fs::read_to_string(root.join("assets/scripts/first_person_controller.lua"))
                .unwrap(),
        )
        .unwrap();
    f.tick(1);
    f.mouse("left", true);
    f.mouse("left", false);
    f.tick(40);
    assert_eq!(f.state()["ammo"], 29);
    assert_eq!(f.world.borrow_mut().drain_events("weapon.shot").len(), 1);
}

#[test]
fn shot_reload_and_repack_preserve_rounds_and_delay_the_swap() {
    let f = Fixture::new();
    f.tick(1);
    f.mouse("left", true);
    f.mouse("left", false);
    f.tick(40);
    assert_eq!(f.state()["ammo"], 29);
    assert_eq!(f.state()["reserve"], 90);
    assert_eq!(f.world.borrow_mut().drain_events("weapon.shot").len(), 1);
    f.key("KeyR", true);
    f.key("KeyR", false);
    f.tick(120);
    assert_eq!(f.state()["state"], "reload");
    assert_eq!(f.state()["ammo"], 29);
    f.tick(125);
    assert_eq!(f.state()["ammo"], 30);
    assert_eq!(f.state()["reserve"], 89);
    assert_eq!(f.state()["magazines"], json!([29, 30, 30]));
    // Another partially spent magazine makes consolidation possible.
    for _ in 0..3 {
        f.mouse("left", true);
        f.mouse("left", false);
        f.tick(50);
    }
    f.key("KeyR", true);
    f.key("KeyR", false);
    f.tick(245);
    assert_eq!(f.state()["ammo"], 30);
    assert_eq!(f.state()["reserve"], 86);
    f.key("KeyR", true);
    f.tick(100);
    f.key("KeyR", false);
    f.tick(1);
    assert_eq!(
        f.state()["ammo"].as_u64().unwrap() + f.state()["reserve"].as_u64().unwrap(),
        116
    );
    assert_eq!(f.state()["state"], "lowReady");
    assert_eq!(f.state()["magazines"], json!([30, 26, 30]));
}
#[test]
fn aim_reversal_burst_stow_and_input_reset_are_scripted() {
    let f = Fixture::new();
    f.tick(1);
    f.mouse("right", true);
    f.tick(7);
    let before = f.world.borrow_mut().drain_events("first_person.frame");
    let prior = before.last().unwrap()["index"].as_u64().unwrap();
    f.mouse("right", false);
    f.tick(1);
    let after = f.world.borrow_mut().drain_events("first_person.frame");
    let now = after
        .last()
        .map(|event| event["index"].as_u64().unwrap())
        .unwrap_or(prior);
    assert!(
        prior.abs_diff(now) <= 2,
        "reversal must continue from the current frame"
    );
    f.tick(30);
    assert_eq!(f.state()["state"], "lowReady");
    f.key("KeyV", true);
    f.key("KeyV", false);
    f.tick(1);
    assert_eq!(f.state()["mode"], "burst");
    f.mouse("left", true);
    f.mouse("left", false);
    f.tick(150);
    assert_eq!(f.state()["shots"], 3);
    assert_eq!(f.state()["ammo"], 27);
    // Posture is driven by a collision-aware gameplay system, not by the renderer.
    f.lua
        .dispatch("weapon.posture", &json!({"crouch":0.5,"moving":false}))
        .unwrap();
    f.tick(1);
    let posture = f.world.borrow_mut().drain_events("first_person.frame");
    assert_eq!(posture.last().unwrap()["clip"], "crouchEnterLow");
    f.lua
        .dispatch("weapon.posture", &json!({"crouch":0,"moving":false}))
        .unwrap();
    f.tick(1);
    f.key("KeyF", true);
    f.key("KeyF", false);
    f.tick(35);
    assert_eq!(f.state()["state"], "noGun");
    f.mouse("left", true);
    f.tick(60);
    assert_eq!(f.state()["ammo"], 27);
    f.lua.dispatch("input.reset", &json!({})).unwrap();
    f.key("KeyF", true);
    f.key("KeyF", false);
    f.tick(45);
    assert_eq!(f.state()["state"], "lowReady");
}

#[test]
fn automatic_fire_keeps_cadence_across_tick_rates_and_stops_on_reset() {
    let mut results = Vec::new();
    for rate in [30, 60, 144] {
        let f = Fixture::new();
        f.mouse("right", true);
        for _ in 0..rate {
            f.lua.run_systems(1.0 / rate as f64).unwrap();
        }
        for _ in 0..2 {
            f.key("KeyV", true);
            f.key("KeyV", false);
        }
        f.mouse("left", true);
        for _ in 0..rate {
            f.lua.run_systems(1.0 / rate as f64).unwrap();
        }
        let shots = f.state()["shots"].as_u64().unwrap();
        results.push(shots);
        f.lua.dispatch("input.reset", &json!({})).unwrap();
        f.tick(60);
        assert_eq!(
            f.state()["shots"],
            shots,
            "focus reset must release the trigger"
        );
    }
    assert!(
        results.iter().all(|shots| (10..=11).contains(shots)),
        "unexpected firing cadence: {results:?}"
    );
    assert!(results.iter().max().unwrap() - results.iter().min().unwrap() <= 1);
}

#[test]
fn initial_magazines_and_timing_come_from_the_scene() {
    let f = Fixture::with_config(Some(json!({
        "magazine_capacity":12,"initial_loaded":4,"initial_spares":[8,6,0],
        "fire_interval":0.2,"reload_seconds":1.0,"hip_raise_seconds":0.22,
        "hip_recovery_seconds":1.0,"repack_round_seconds":0.4
    })));
    f.tick(1);
    assert_eq!(f.state()["ammo"], 4);
    assert_eq!(f.state()["reserve"], 14);
    f.key("KeyR", true);
    f.key("KeyR", false);
    f.tick(61);
    assert_eq!(f.state()["ammo"], 8);
    assert_eq!(f.state()["reserve"], 10);
}

#[test]
fn hit_reacts_and_death_interrupts_a_carry_transition() {
    let f = Fixture::new();
    f.tick(1);
    f.lua
        .dispatch("weapon.hit", &json!({"near":false}))
        .unwrap();
    f.tick(1);
    assert!(f.state()["reaction"].as_f64().unwrap() > 0.3);
    f.mouse("right", true);
    f.tick(4);
    f.lua.dispatch("weapon.die", &json!({})).unwrap();
    f.tick(55);
    assert_eq!(f.state()["state"], "dead");
    let frames = f.world.borrow_mut().drain_events("first_person.frame");
    assert_eq!(frames.last().unwrap()["visible"], false);
    f.mouse("left", true);
    f.tick(60);
    assert_eq!(f.state()["shots"], 0);
}

#[test]
fn standing_idle_animates_and_blends_into_the_aim_transition() {
    let f = Fixture::new();
    f.tick(1);
    let first = f
        .world
        .borrow_mut()
        .drain_events("first_person.frame")
        .pop()
        .unwrap();
    assert_eq!(first["clip"], "lowReady");
    assert_eq!(first["loop"], true);
    f.tick(60);
    let idle = f.world.borrow_mut().drain_events("first_person.frame");
    let last = idle.last().unwrap();
    assert_eq!(last["clip"], "lowReady");
    assert_ne!(
        first["index"], last["index"],
        "idle must advance through the source footage"
    );
    assert_ne!(
        first["motion_x"], last["motion_x"],
        "breathing should move the held weapon"
    );
    f.mouse("right", true);
    f.tick(1);
    let raise = f
        .world
        .borrow_mut()
        .drain_events("first_person.frame")
        .pop()
        .unwrap();
    assert_eq!(raise["clip"], "raising");
    assert!(raise["transition_seconds"].as_f64().unwrap() > 0.0);
    f.tick(6);
    let before = f
        .world
        .borrow_mut()
        .drain_events("first_person.frame")
        .pop()
        .unwrap();
    f.mouse("right", false);
    f.tick(1);
    let reverse = f
        .world
        .borrow_mut()
        .drain_events("first_person.frame")
        .pop()
        .unwrap();
    assert_eq!(reverse["clip"], "raising");
    assert_eq!(reverse["direction"], -1);
    assert!(
        before["index"]
            .as_i64()
            .unwrap()
            .abs_diff(reverse["index"].as_i64().unwrap())
            <= 1,
        "aim reversal must preserve the visible pose"
    );
}

#[test]
fn sprint_uses_the_authored_bridge_and_can_reverse_without_a_cut() {
    let f = Fixture::new();
    f.tick(1);
    f.key("KeyW", true);
    f.key("ShiftLeft", true);
    f.tick(6);
    let before = f
        .world
        .borrow_mut()
        .drain_events("first_person.frame")
        .pop()
        .unwrap();
    assert_eq!(before["clip"], "runFromLow");
    f.key("ShiftLeft", false);
    f.tick(1);
    let reverse = f
        .world
        .borrow_mut()
        .drain_events("first_person.frame")
        .pop()
        .unwrap();
    assert_eq!(reverse["clip"], "runFromLow");
    assert_eq!(reverse["direction"], -1);
    assert!(
        before["index"]
            .as_i64()
            .unwrap()
            .abs_diff(reverse["index"].as_i64().unwrap())
            <= 1
    );
    f.tick(30);
    assert_eq!(f.state()["state"], "lowReady");
}

#[test]
fn sprint_interrupts_passive_hip_recovery_after_the_visible_shot() {
    let f = Fixture::new();
    f.mouse("left", true);
    f.mouse("left", false);
    f.tick(30);
    assert_eq!(f.state()["state"], "hipFire");
    assert_eq!(f.state()["shots"], 1);
    f.key("KeyW", true);
    f.key("ShiftLeft", true);
    f.tick(30);
    assert_eq!(
        f.state()["state"],
        "run",
        "sprint should not wait through the long idle recovery"
    );
}

#[test]
fn ads_uses_the_approved_pose_and_procedural_recoil_when_its_clip_is_disabled() {
    let f = Fixture::new();
    f.mouse("right", true);
    f.tick(30);
    assert_eq!(f.state()["state"], "ads");
    f.world.borrow_mut().drain_events("first_person.frame");
    f.mouse("left", true);
    f.mouse("left", false);
    f.tick(6);
    let frames = f.world.borrow_mut().drain_events("first_person.frame");
    assert!(frames.iter().all(|event| event["clip"] != "fireADS"));
    assert!(
        frames
            .iter()
            .any(|event| event["motion_y"].as_f64().unwrap().abs() > 0.001),
        "recoil must animate the viewmodel"
    );
    assert_eq!(f.state()["shots"], 1);
}

#[test]
fn stowing_and_equipping_lower_and_fade_the_viewmodel() {
    let f = Fixture::new();
    f.tick(1);
    f.key("KeyF", true);
    f.key("KeyF", false);
    f.tick(15);
    let frames = f.world.borrow_mut().drain_events("first_person.frame");
    let mid = frames.last().unwrap();
    assert!(mid["motion_y"].as_f64().unwrap() > 0.05);
    assert!(mid["opacity"].as_f64().unwrap() < 0.9);
    f.tick(20);
    assert_eq!(f.state()["state"], "noGun");
    f.key("KeyF", true);
    f.key("KeyF", false);
    f.tick(10);
    let frames = f.world.borrow_mut().drain_events("first_person.frame");
    assert!(frames.last().unwrap()["opacity"].as_f64().unwrap() < 1.0);
    f.tick(40);
    assert_eq!(f.state()["state"], "lowReady");
}

#[test]
fn stow_and_death_preserve_the_current_aim_pose() {
    let f = Fixture::new();
    f.mouse("right", true);
    f.tick(30);
    f.world.borrow_mut().drain_events("first_person.frame");
    f.key("KeyF", true);
    f.key("KeyF", false);
    f.tick(1);
    let frames = f.world.borrow_mut().drain_events("first_person.frame");
    let stow = frames.last().unwrap();
    assert_eq!(stow["clip"], "raising");
    assert_eq!(
        stow["index"], 33,
        "stowing from ADS should start on the ADS pose"
    );

    let g = Fixture::new();
    g.mouse("right", true);
    g.tick(7);
    let before = g
        .world
        .borrow_mut()
        .drain_events("first_person.frame")
        .pop()
        .unwrap();
    g.lua.dispatch("weapon.die", &json!({})).unwrap();
    g.tick(1);
    let after = g
        .world
        .borrow_mut()
        .drain_events("first_person.frame")
        .pop()
        .unwrap();
    assert_eq!(
        before["index"], after["index"],
        "death must lower the visible pose"
    );
}

#[test]
fn an_interrupted_aim_transition_can_continue_into_sprint() {
    let f = Fixture::new();
    f.mouse("right", true);
    f.tick(13);
    f.world.borrow_mut().drain_events("first_person.frame");
    f.mouse("right", false);
    f.key("KeyW", true);
    f.key("ShiftLeft", true);
    f.tick(45);
    assert_eq!(f.state()["state"], "run");
    let frames = f.world.borrow_mut().drain_events("first_person.frame");
    assert!(
        frames.iter().any(|frame| frame["clip"] == "runFromADS"),
        "the near-ADS interruption should use the ADS-to-run bridge"
    );
}

#[test]
fn quick_hip_taps_survive_a_raise_but_released_aim_taps_are_cancelled() {
    let hip = Fixture::new();
    hip.mouse("left", true);
    hip.mouse("left", false);
    hip.tick(30);
    assert_eq!(
        hip.state()["shots"],
        1,
        "a hip tap commits its delayed shot"
    );

    let aimed = Fixture::new();
    aimed.mouse("right", true);
    aimed.tick(3);
    aimed.mouse("left", true);
    aimed.mouse("left", false);
    aimed.tick(30);
    assert_eq!(aimed.state()["state"], "ads");
    assert_eq!(
        aimed.state()["shots"],
        0,
        "a released ADS tap cannot fire later"
    );

    aimed.mouse("left", true);
    aimed.mouse("left", false);
    aimed.tick(1);
    assert_eq!(
        aimed.state()["shots"],
        1,
        "a ready ADS tap fires immediately"
    );
}

#[test]
fn stow_freezes_the_current_sprint_frame_instead_of_snapping_to_idle() {
    let f = Fixture::new();
    f.key("KeyW", true);
    f.key("ShiftLeft", true);
    f.tick(35);
    assert_eq!(f.state()["state"], "run");
    let before = f
        .world
        .borrow_mut()
        .drain_events("first_person.frame")
        .pop()
        .unwrap();
    assert_eq!(before["clip"], "runLateral");
    f.key("KeyF", true);
    f.key("KeyF", false);
    f.tick(1);
    let after = f
        .world
        .borrow_mut()
        .drain_events("first_person.frame")
        .pop()
        .unwrap();
    assert_eq!(after["clip"], before["clip"]);
    assert_eq!(after["index"], before["index"]);
    f.tick(30);
    assert_eq!(f.state()["state"], "noGun");
}
