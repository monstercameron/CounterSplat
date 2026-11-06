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
    player: Entity,
}
impl Fixture {
    fn new() -> Self {
        Self::with_config(None)
    }
    fn with_config(config: Option<Value>) -> Self {
        Self::with_controller(config, true)
    }
    /// The scene exactly as shipped, including its spawn settings and weapon profile.
    fn as_shipped() -> Self {
        Self::build(None, true, true, None)
    }
    /// The shipped scene with another of its weapon profiles selected.
    fn with_profile(profile: &str) -> Self {
        Self::build(None, true, true, Some(profile))
    }
    fn with_controller(config: Option<Value>, load_controller: bool) -> Self {
        Self::build(config, load_controller, false, None)
    }
    /// Weapon behaviour tests start from a ready weapon that aims from its presented sight: unless `as_shipped`, the
    /// scene's `start_stowed` and `aim_from_camera` settings are removed (the shipped-scene tests cover them).
    fn build(config: Option<Value>, load_controller: bool, as_shipped: bool, profile: Option<&str>) -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut scene = SceneManifest::from_json(
            &std::fs::read(root.join("assets/scenes/gameplay.json")).unwrap(),
        )
        .unwrap();
        let first_person = scene
            .entities
            .iter_mut()
            .find(|entity| entity.id == "first_person")
            .unwrap();
        if let Some(profile) = profile {
            first_person.components.insert("WeaponProfile".into(), json!({ "profile": profile }));
        }
        if let Some(config) = config {
            first_person.components.insert("WeaponConfig".into(), config);
        } else if !as_shipped {
            if let Some(Value::Object(fields)) = first_person.components.get_mut("WeaponConfig") {
                fields.remove("start_stowed");
            }
        }
        if !as_shipped {
            if let Some(Value::Object(fields)) = first_person.components.get_mut("FirstPersonView") {
                fields.remove("aim_from_camera");
            }
            first_person.components.remove("WeaponProfile");
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
        Self {
            world,
            lua,
            entity,
            player: ids["player"],
        }
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
/// The shipped gameplay scene spawns with the weapon ready and drawn (revolver concept stills stand in for footage).
/// The stills have no sight landmarks, so a hip shot goes straight through the camera centre.
#[test]
fn spawns_ready_and_drawn_and_aims_through_the_camera_centre() {
    let f = Fixture::as_shipped();
    f.tick(2);
    assert_eq!(f.state()["state"], "lowReady");
    let frames = f.world.borrow_mut().drain_events("first_person.frame");
    assert!(!frames.is_empty() && frames.iter().all(|frame| frame["visible"] == true), "{frames:?}");
    f.tick(30);
    let (yaw, pitch) = {
        let world = f.world.borrow();
        let camera = world.get::<countersplat_scene::PlayerCamera>(f.player).unwrap();
        (camera.yaw as f64, camera.pitch as f64)
    };
    f.mouse("left", true);
    f.mouse("left", false);
    f.tick(40);
    assert_eq!(f.state()["ammo"], 4, "the revolver fires from the hip");
    let shots = f.world.borrow_mut().drain_events("weapon.shot");
    assert_eq!(shots.len(), 1, "{shots:?}");
    let d = &shots[0]["direction"];
    let forward = [yaw.sin() * pitch.cos(), pitch.sin(), -yaw.cos() * pitch.cos()];
    let dot: f64 = (0..3).map(|i| d[i].as_f64().unwrap() * forward[i]).sum();
    assert!(dot > 0.9999, "the shot leaves through the camera centre: {d} vs {forward:?}");
}

fn component(f: &Fixture, name: &str) -> Value {
    f.world.borrow().get_dynamic(f.entity, name).unwrap().clone()
}
fn shoot_once(f: &Fixture) {
    f.mouse("left", true);
    f.mouse("left", false);
    f.tick(70);
}
fn tap(f: &Fixture, code: &str) {
    f.key(code, true);
    f.key(code, false);
}

/// The shipped weapon is the .500 Magnum revolver profile: its physics and limits reach the weapon components.
#[test]
fn the_shipped_revolver_profile_sets_the_weapon_components() {
    let f = Fixture::as_shipped();
    let config = component(&f, "WeaponConfig");
    assert_eq!((config["magazine_capacity"].clone(), config["modes"].clone()), (json!(5), json!(["single"])));
    assert_eq!((config["reload_kind"].clone(), config["initial_spares"].clone()), (json!("swap"), json!([5, 5, 5])));
    assert!((config["fire_interval"].as_f64().unwrap() - 0.5).abs() < 1e-9);
    let ballistics = component(&f, "Ballistics");
    assert!((ballistics["speed_mps"].as_f64().unwrap() - 579.12).abs() < 0.01);
    assert_eq!(ballistics["gravity_mps2"], 9.81, "authored fields the profile does not set are kept");
    let stats = component(&f, "WeaponStats");
    assert!((stats["muzzle_energy_ftlbf"].as_f64().unwrap() - 2805.0).abs() < 2.0, "{stats}");
    assert!(component(&f, "WeaponRecoil")["vertical_degrees"].as_f64().unwrap() > 1.5, "a magnum kicks hard");
    let all = f.world.borrow().get_dynamic_resource("weapon_stats").unwrap().clone();
    assert_eq!(all.as_object().unwrap().len(), 6, "every profile's physics is published");

    f.tick(30);
    assert_eq!((f.state()["ammo"].clone(), f.state()["reserve"].clone()), (json!(5), json!(15)));
    tap(&f, "KeyV");
    f.tick(2);
    assert_eq!(f.state()["mode"], "single", "a revolver has no other fire mode");
}

/// A tube-fed pump shotgun reloads shell by shell from loose reserve rounds, only as many as are missing.
#[test]
fn a_tube_fed_shotgun_tops_up_one_shell_at_a_time() {
    let f = Fixture::with_profile("shotgun_12");
    assert_eq!(component(&f, "Ballistics")["projectiles"], 9);
    f.tick(30);
    assert_eq!((f.state()["ammo"].clone(), f.state()["reserve"].clone()), (json!(5), json!(20)));
    shoot_once(&f);
    shoot_once(&f);
    assert_eq!(f.state()["ammo"], 3);
    tap(&f, "KeyR");
    f.tick(40);
    assert_eq!(f.state()["state"], "reload", "the hip pose lowers first, then two shells take 1.1 s");
    f.tick(80);
    assert_eq!((f.state()["ammo"].clone(), f.state()["reserve"].clone()), (json!(5), json!(18)));
}

/// A select-fire carbine cycles exactly its profile's modes, and burst fires its profile's round count.
#[test]
fn a_select_fire_carbine_cycles_its_profile_modes() {
    let f = Fixture::with_profile("carbine_556");
    f.tick(30);
    assert_eq!(f.state()["mode"], "single");
    tap(&f, "KeyV");
    f.tick(2);
    assert_eq!(f.state()["mode"], "burst");
    shoot_once(&f);
    assert_eq!(f.state()["ammo"], 27, "a three-round burst");
    tap(&f, "KeyV");
    f.tick(2);
    assert_eq!(f.state()["mode"], "auto");
    tap(&f, "KeyV");
    f.tick(2);
    assert_eq!(f.state()["mode"], "single");
}

/// I turns the weapon over from an idle low ready and returns; aiming cancels it.
#[test]
fn inspect_plays_from_idle_low_ready_and_aiming_cancels_it() {
    let f = Fixture::as_shipped();
    f.tick(30);
    f.world.borrow_mut().drain_events("first_person.frame");
    f.key("KeyI", true);
    f.key("KeyI", false);
    f.tick(10);
    assert_eq!(f.state()["state"], "inspectEnter");
    f.tick(20);
    assert_eq!(f.state()["state"], "inspecting");
    let frames = f.world.borrow_mut().drain_events("first_person.frame");
    assert!(frames.iter().any(|frame| frame["clip"] == "inspect"), "{frames:?}");
    f.tick(100);
    assert_eq!(f.state()["state"], "inspectExit", "I's hold ends on its own");
    f.tick(30);
    assert_eq!(f.state()["state"], "lowReady", "inspect returns to low ready after 2.4 s");

    f.key("KeyI", true);
    f.key("KeyI", false);
    f.tick(30);
    assert_eq!(f.state()["state"], "inspecting");
    f.mouse("right", true);
    f.tick(2);
    assert!(!f.state()["state"].as_str().unwrap().starts_with("inspect"), "aiming cancels inspect");
    f.tick(40);
    assert_eq!(f.state()["state"], "ads");
}

/// The inspection HUD (Q) holds the weapon in the inspect pose for as long as it is open.
#[test]
fn the_inspection_hud_holds_the_inspect_pose_until_it_closes() {
    let f = Fixture::as_shipped();
    f.tick(30);
    let hud = |name: &str| {
        f.lua.dispatch("engine.event", &json!({"name": name, "payload": {}})).unwrap();
    };
    hud("hud.inspect_started");
    f.tick(30);
    assert_eq!(f.state()["state"], "inspecting");
    f.tick(400);
    assert_eq!(f.state()["state"], "inspecting", "held well past inspect_seconds while the HUD is open");
    hud("hud.inspect_ended");
    f.tick(2);
    assert_eq!(f.state()["state"], "inspectExit");
    f.tick(30);
    assert_eq!(f.state()["state"], "lowReady");
}

/// Aiming mid-sprint is its own transition state into ADS, and releasing aim goes back the other way.
#[test]
fn run_to_ads_and_back_are_named_transitions() {
    let f = Fixture::new();
    f.key("KeyW", true);
    f.key("ShiftLeft", true);
    f.tick(35);
    assert_eq!(f.state()["state"], "runForward");
    f.mouse("right", true);
    f.tick(2);
    assert_eq!(f.state()["state"], "runToADS");
    assert_eq!(f.world.borrow_mut().drain_events("first_person.frame").pop().unwrap()["clip"], "runFromADS");
    f.tick(30);
    assert_eq!(f.state()["state"], "ads");
    f.mouse("right", false);
    f.tick(2);
    assert_eq!(f.state()["state"], "adsToRun");
    f.tick(30);
    assert_eq!(f.state()["state"], "runForward");
    f.key("ShiftLeft", false);
    f.tick(2);
    assert_eq!(f.state()["state"], "runToLow");
}

/// A crouch slide (player.slide events from the movement script) takes the weapon into its slide pose; aiming
/// leaves it, and the slide's end settles back to low ready.
#[test]
fn a_crouch_slide_drives_the_weapon_slide_state() {
    let f = Fixture::new();
    f.key("KeyW", true);
    f.key("ShiftLeft", true);
    f.tick(35);
    f.lua.dispatch("player.slide", &json!({"active": true})).unwrap();
    f.tick(2);
    assert_eq!(f.state()["state"], "slide");
    let entering = f.world.borrow_mut().drain_events("first_person.frame").pop().unwrap();
    assert_eq!(entering["clip"], "slide");
    // The drop into the slide plays over the clip's duration, then holds on its last frame.
    f.tick(40);
    let frames = f.world.borrow_mut().drain_events("first_person.frame");
    let held = frames.iter().rev().find(|e| e["clip"] == "slide").unwrap_or(&entering);
    assert!(held["index"].as_u64() > entering["index"].as_u64(), "the slide entry plays: {entering} -> {held}");
    f.lua.dispatch("player.slide", &json!({"active": false})).unwrap();
    f.key("ShiftLeft", false);
    f.key("KeyW", false);
    f.tick(2);
    assert_eq!(f.state()["state"], "slideExit", "getting up plays the slide exit");
    f.tick(30);
    assert_eq!(f.state()["state"], "lowReady");

    f.lua.dispatch("player.slide", &json!({"active": true})).unwrap();
    f.tick(2);
    assert_eq!(f.state()["state"], "slide");
    f.mouse("right", true);
    f.tick(2);
    assert_ne!(f.state()["state"], "slide", "aiming leaves the slide pose");
    f.tick(40);
    assert_eq!(f.state()["state"], "ads");
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
    f.tick(40);
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
        "runForward",
        "sprint should not wait through the long idle recovery"
    );
}

#[test]
fn ads_shots_show_authored_flash_then_return_to_the_aimed_pose() {
    let f = Fixture::new();
    f.mouse("right", true);
    f.tick(30);
    assert_eq!(f.state()["state"], "ads");
    f.world.borrow_mut().drain_events("first_person.frame");
    f.mouse("left", true);
    f.mouse("left", false);
    f.tick(6);
    let frames = f.world.borrow_mut().drain_events("first_person.frame");
    assert!(
        frames
            .iter()
            .any(|event| event["clip"] == "fireADS" && event["index"] == 1),
        "each aimed shot must display the muzzle-flash frame"
    );
    f.tick(12);
    let recovered = f.world.borrow_mut().drain_events("first_person.frame");
    assert!(
        recovered.iter().any(|event| event["clip"] == "raising"),
        "the aimed pose must return after the flash"
    );
    assert!(
        frames
            .iter()
            .any(|event| event["motion_y"].as_f64().unwrap().abs() > 0.001),
        "recoil must animate the viewmodel"
    );
    assert_eq!(f.state()["shots"], 1);
}

/// From an idle low ready, stowing plays the holster footage out of frame and equipping plays it backwards; the
/// footage does the lowering, so the view model is neither faded nor pushed down.
#[test]
fn stowing_and_equipping_from_low_ready_play_the_holster_clip() {
    let f = Fixture::new();
    f.tick(1);
    f.key("KeyF", true);
    f.key("KeyF", false);
    f.tick(15);
    let frames = f.world.borrow_mut().drain_events("first_person.frame");
    let mid = frames.last().unwrap();
    assert_eq!(mid["clip"], "holster");
    let stowing = mid["index"].as_u64().unwrap();
    assert!(stowing > 1 && stowing < 17, "mid-holster: {mid}");
    assert!(mid["motion_y"].as_f64().unwrap().abs() < 0.02, "{mid}");
    assert_eq!(mid["opacity"].as_f64().unwrap(), 1.0);
    f.tick(25);
    assert_eq!(f.state()["state"], "noGun");
    f.key("KeyF", true);
    f.key("KeyF", false);
    f.tick(10);
    let frames = f.world.borrow_mut().drain_events("first_person.frame");
    let drawing = frames.last().unwrap();
    assert_eq!(drawing["clip"], "holster");
    assert_eq!(drawing["direction"], -1, "equipping plays the holster backwards");
    assert_eq!(drawing["opacity"].as_f64().unwrap(), 1.0);
    f.tick(40);
    assert_eq!(f.state()["state"], "lowReady");
}

/// Stowing from any other pose (here ADS) freezes that pose and lowers and fades it procedurally.
#[test]
fn stowing_from_ads_lowers_and_fades_the_viewmodel() {
    let f = Fixture::new();
    f.mouse("right", true);
    f.tick(30);
    f.key("KeyF", true);
    f.key("KeyF", false);
    f.tick(15);
    let frames = f.world.borrow_mut().drain_events("first_person.frame");
    let mid = frames.last().unwrap();
    assert!(mid["motion_y"].as_f64().unwrap() > 0.05);
    assert!(mid["opacity"].as_f64().unwrap() < 0.9);
    f.tick(20);
    assert_eq!(f.state()["state"], "noGun");
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
    assert_eq!(f.state()["state"], "runForward");
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
    assert_eq!(f.state()["state"], "runForward");
    let before = f
        .world
        .borrow_mut()
        .drain_events("first_person.frame")
        .pop()
        .unwrap();
    assert_eq!(before["clip"], "runForward");
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

#[test]
fn cached_weapon_settings_follow_live_ecs_edits() {
    let f = Fixture::new();
    f.tick(2); // Populate the readers with the authored settings.
    f.world.borrow_mut().drain_events("first_person.frame");
    {
        let mut world = f.world.borrow_mut();
        world
            .write_component_field(f.entity, "FirstPersonView", "opacity", json!(0.35))
            .unwrap();
        world
            .write_component_field(f.entity, "WeaponAnimation", "idle_enabled", json!(false))
            .unwrap();
        world
            .write_component_field(f.entity, "WeaponConfig", "hip_raise_seconds", json!(0.8))
            .unwrap();
    }
    f.tick(1);
    let frame = f
        .world
        .borrow_mut()
        .drain_events("first_person.frame")
        .pop()
        .unwrap();
    assert_eq!(frame["opacity"], json!(0.35));
    assert_eq!(
        frame["loop"], false,
        "idle footage should stop when disabled"
    );
    f.mouse("left", true);
    f.mouse("left", false);
    f.tick(20);
    assert_eq!(
        f.state()["state"],
        "hipRaise",
        "new raise timing must take effect"
    );
    f.tick(40);
    assert_eq!(f.state()["shots"], 1);
}
#[test]
fn aim_takes_priority_over_sprint_and_returns_to_sprint_after_release() {
    let f = Fixture::new();
    f.key("KeyW", true);
    f.key("ShiftLeft", true);
    f.tick(35);
    assert_eq!(f.state()["state"], "runForward");
    f.mouse("right", true);
    f.tick(45);
    assert_eq!(
        f.state()["state"],
        "ads",
        "aiming while running must enter ADS"
    );
    f.tick(30);
    assert_eq!(
        f.state()["state"],
        "ads",
        "held Shift must not interrupt ADS"
    );
    f.mouse("right", false);
    f.tick(45);
    assert_eq!(
        f.state()["state"],
        "runForward",
        "held Shift may sprint after ADS is released"
    );
}

#[test]
fn sprinting_single_tap_fires_promptly_and_returns_to_running() {
    let f = Fixture::new();
    f.key("KeyW", true);
    f.key("ShiftLeft", true);
    f.tick(35);
    assert_eq!(f.state()["state"], "runForward");
    f.mouse("left", true);
    f.mouse("left", false);
    f.tick(20);
    let shots = f.world.borrow_mut().drain_events("weapon.shot");
    assert_eq!(
        shots.len(),
        1,
        "a quick tap while running must fire without a long double transition"
    );
    assert_eq!(shots[0]["stance"], "hip");
    f.tick(45);
    assert_eq!(f.state()["state"], "runForward");
    assert_eq!(f.state()["shots"], 1);
}

#[test]
fn sprinting_auto_and_burst_do_not_drop_shots_or_interrupt_the_run() {
    for mode in ["burst", "auto"] {
        let f = Fixture::new();
        f.key("KeyW", true);
        f.key("ShiftLeft", true);
        f.tick(35);
        f.key("KeyV", true);
        f.key("KeyV", false);
        if mode == "auto" {
            f.key("KeyV", true);
            f.key("KeyV", false);
        }
        f.mouse("left", true);
        if mode == "burst" {
            f.mouse("left", false);
        }
        f.tick(20);
        assert!(
            f.state()["shots"].as_u64().unwrap() >= 1,
            "{mode} should begin promptly while sprinting"
        );
        f.tick(55);
        if mode == "auto" {
            f.mouse("left", false);
        }
        let shots = f.state()["shots"].as_u64().unwrap();
        if mode == "burst" {
            assert_eq!(shots, 3);
        } else {
            assert!(shots >= 9, "automatic fire stopped during sprint: {shots}");
        }
        f.tick(45);
        assert_eq!(
            f.state()["state"],
            "runForward",
            "{mode} should return to the held sprint"
        );
        assert_eq!(f.state()["shots"], shots, "release should stop shooting");
    }
}

#[test]
fn a_new_hip_tap_reverses_a_lowering_weapon_without_losing_the_shot() {
    let f = Fixture::new();
    f.mouse("left", true);
    f.mouse("left", false);
    for _ in 0..180 {
        f.tick(1);
        if f.state()["state"] == "hipLower" {
            break;
        }
    }
    assert_eq!(f.state()["state"], "hipLower");
    f.tick(10);
    f.mouse("left", true);
    f.mouse("left", false);
    f.tick(12);
    assert_eq!(
        f.state()["shots"],
        2,
        "the new tap should reverse the lowering animation promptly"
    );
}

#[test]
fn stow_interrupts_a_carry_transition_and_equips_again() {
    let f = Fixture::new();
    f.key("KeyW", true);
    f.key("ShiftLeft", true);
    f.tick(3);
    assert_eq!(f.state()["state"], "lowToRun");
    f.key("KeyF", true);
    f.key("KeyF", false);
    f.tick(40);
    assert_eq!(
        f.state()["state"],
        "noGun",
        "stow must not be ignored while entering a sprint"
    );
    f.key("KeyW", false);
    f.key("ShiftLeft", false);
    f.key("KeyF", true);
    f.key("KeyF", false);
    f.tick(50);
    assert_eq!(f.state()["state"], "lowReady");
}

#[test]
fn resetting_input_cancels_a_queued_reload() {
    let f = Fixture::new();
    f.mouse("left", true);
    f.mouse("left", false);
    f.tick(35);
    assert_eq!(f.state()["ammo"], 29);
    f.key("KeyW", true);
    f.key("ShiftLeft", true);
    f.tick(3);
    f.key("KeyR", true);
    f.key("KeyR", false);
    f.lua.dispatch("input.reset", &json!({})).unwrap();
    f.tick(150);
    assert_eq!(
        f.state()["ammo"],
        29,
        "a queued reload must not resume after a focus reset"
    );
    assert!(f
        .world
        .borrow_mut()
        .drain_events("weapon.reload_started")
        .is_empty());
}

#[test]
fn death_with_a_stowed_weapon_does_not_show_the_weapon_again() {
    let f = Fixture::new();
    f.key("KeyF", true);
    f.key("KeyF", false);
    f.tick(40);
    assert_eq!(f.state()["state"], "noGun");
    f.world.borrow_mut().drain_events("first_person.frame");
    f.lua.dispatch("weapon.die", &json!({})).unwrap();
    f.tick(1);
    assert_eq!(f.state()["state"], "dead");
    assert!(f
        .world
        .borrow_mut()
        .drain_events("first_person.frame")
        .iter()
        .all(|frame| frame["visible"] == false));
}

#[test]
fn continuous_fire_transitions_between_sprint_hip_and_ads() {
    let f = Fixture::new();
    f.key("KeyW", true);
    f.key("ShiftLeft", true);
    f.tick(35);
    for _ in 0..2 {
        f.key("KeyV", true);
        f.key("KeyV", false);
    }
    f.mouse("left", true);
    f.tick(45);
    let hip = f.world.borrow_mut().drain_events("weapon.shot");
    assert!(hip.len() >= 2 && hip.iter().all(|shot| shot["stance"] == "hip"));

    f.mouse("right", true);
    f.tick(45);
    let aimed = f.world.borrow_mut().drain_events("weapon.shot");
    assert!(
        aimed.iter().any(|shot| shot["stance"] == "ads"),
        "held fire must continue after entering ADS"
    );
    assert_eq!(f.state()["state"], "ads");

    f.mouse("right", false);
    f.tick(45);
    let hip_again = f.world.borrow_mut().drain_events("weapon.shot");
    assert!(
        hip_again.iter().any(|shot| shot["stance"] == "hip"),
        "held fire must resume hip shots after ADS"
    );
    f.mouse("left", false);
    let fired = f.state()["shots"].as_u64().unwrap();
    f.tick(45);
    assert_eq!(f.state()["state"], "runForward");
    assert_eq!(f.state()["shots"], fired);
}

#[test]
fn sprint_fire_keeps_player_speed_but_ads_reduces_it() {
    use countersplat_scene::{PhysicsBody, PlayerCamera, PlayerControls};
    let f = Fixture::new();
    {
        let mut world = f.world.borrow_mut();
        world.get_mut::<PhysicsBody>(f.player).unwrap().enabled = false;
        let camera = world.get_mut::<PlayerCamera>(f.player).unwrap();
        camera.yaw = 0.0;
        camera.pitch = 0.0;
        world.set_dynamic_resource(
            "player.default_controls",
            serde_json::to_value(PlayerControls::default()).unwrap(),
        );
        world.set_dynamic_resource(
            "player.spawn_camera",
            serde_json::to_value(PlayerCamera::default()).unwrap(),
        );
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    f.lua
        .execute(&std::fs::read_to_string(root.join("assets/scripts/player_controls.lua")).unwrap())
        .unwrap();
    let position = || {
        f.world
            .borrow()
            .get::<PlayerCamera>(f.player)
            .unwrap()
            .position
    };
    let distance =
        |a: [f32; 3], b: [f32; 3]| ((b[0] - a[0]).powi(2) + (b[2] - a[2]).powi(2)).sqrt();

    f.key("KeyW", true);
    f.key("ShiftLeft", true);
    f.tick(35);
    let start = position();
    f.tick(30);
    let running = distance(start, position());
    for _ in 0..2 {
        f.key("KeyV", true);
        f.key("KeyV", false);
    }
    f.mouse("left", true);
    let start = position();
    f.tick(60);
    let firing = distance(start, position());
    assert!(f.state()["shots"].as_u64().unwrap() >= 6);
    assert!(
        (firing / (2.0 * running) - 1.0).abs() < 0.03,
        "hip fire must preserve running speed"
    );

    f.mouse("right", true);
    f.tick(45);
    let start = position();
    f.tick(30);
    let aiming = distance(start, position());
    assert!(
        (aiming / running
            - 1.0
                / f.world
                    .borrow()
                    .get::<PlayerControls>(f.player)
                    .unwrap()
                    .sprint_multiplier)
            .abs()
            < 0.03,
        "ADS must still block sprint speed"
    );
    f.mouse("left", false);
    f.mouse("right", false);
}

#[test]
fn stowing_mid_reload_preserves_ammunition_and_allows_a_later_reload() {
    let f = Fixture::new();
    f.mouse("left", true);
    f.mouse("left", false);
    f.tick(35);
    f.key("KeyR", true);
    f.key("KeyR", false);
    f.tick(20);
    assert_eq!(f.state()["state"], "reload");
    f.key("KeyF", true);
    f.key("KeyF", false);
    f.tick(40);
    assert_eq!(f.state()["state"], "noGun");
    assert_eq!(
        f.state()["ammo"],
        29,
        "interrupted reload must not swap magazines"
    );
    f.key("KeyF", true);
    f.key("KeyF", false);
    f.tick(50);
    f.key("KeyR", true);
    f.key("KeyR", false);
    f.tick(245);
    assert_eq!(f.state()["ammo"], 30);
    assert_eq!(f.state()["reserve"], 89);
}

#[test]
fn opposing_movement_keys_do_not_enter_run_carry() {
    let f = Fixture::new();
    f.key("KeyW", true);
    f.key("KeyS", true);
    f.key("ShiftLeft", true);
    f.tick(50);
    assert_eq!(f.state()["state"], "lowReady");
}

#[test]
fn running_tap_fires_across_simulation_tick_rates() {
    for rate in [30_usize, 60, 144] {
        let f = Fixture::new();
        f.key("KeyW", true);
        f.key("ShiftLeft", true);
        for _ in 0..rate {
            f.lua.run_systems(1.0 / rate as f64).unwrap();
        }
        f.mouse("left", true);
        f.mouse("left", false);
        for _ in 0..((rate as f64 * 0.35).ceil() as usize) {
            f.lua.run_systems(1.0 / rate as f64).unwrap();
        }
        assert_eq!(f.state()["shots"], 1, "running tap must fire at {rate} Hz");
    }
}

#[test]
fn immediate_reload_or_stow_does_not_erase_the_last_muzzle_flash() {
    for key in ["KeyR", "KeyF"] {
        let f = Fixture::new();
        f.mouse("right", true);
        f.tick(30);
        f.world.borrow_mut().drain_events("first_person.frame");
        f.mouse("left", true);
        f.mouse("left", false);
        f.key(key, true);
        f.key(key, false);
        f.tick(1);
        let frames = f.world.borrow_mut().drain_events("first_person.frame");
        assert!(
            frames
                .iter()
                .any(|frame| frame["clip"] == "fireADS" && frame["index"] == 1),
            "{key} must not skip the shot's first flash frame"
        );
        f.tick(45);
        if key == "KeyR" {
            assert_eq!(f.state()["state"], "reloadADS");
            assert_eq!(f.state()["ammo"], 29);
        } else {
            assert_eq!(f.state()["state"], "noGun");
            assert_eq!(f.state()["ammo"], 29);
        }
    }
}

#[test]
fn a_tap_interrupts_every_phase_of_entering_sprint() {
    for elapsed in [1, 4, 8, 12] {
        let f = Fixture::new();
        f.key("KeyW", true);
        f.key("ShiftLeft", true);
        f.tick(elapsed);
        f.mouse("left", true);
        f.mouse("left", false);
        f.tick(21);
        assert_eq!(
            f.state()["shots"],
            1,
            "fire was lost during sprint transition at tick {elapsed}"
        );
        f.tick(45);
        assert_eq!(f.state()["state"], "runForward");
    }
}

#[test]
fn reset_cancels_a_stow_queued_behind_a_flash() {
    let f = Fixture::new();
    f.mouse("right", true);
    f.tick(30);
    f.mouse("left", true);
    f.mouse("left", false);
    f.key("KeyF", true);
    f.key("KeyF", false);
    f.lua.dispatch("input.reset", &json!({})).unwrap();
    f.tick(100);
    assert_ne!(
        f.state()["state"],
        "noGun",
        "focus reset must cancel a queued stow"
    );
    assert_eq!(f.state()["shots"], 1);
}

#[test]
fn holding_aim_through_stow_and_equip_restores_ads() {
    use countersplat_scene::PlayerCamera;
    let f = Fixture::new();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    for name in ["weapon_optics.lua", "camera_zoom.lua"] {
        f.lua
            .execute(&std::fs::read_to_string(root.join("assets/scripts").join(name)).unwrap())
            .unwrap();
    }
    let camera = || {
        f.world
            .borrow()
            .get::<PlayerCamera>(f.player)
            .unwrap()
            .clone()
    };
    f.mouse("right", true);
    f.tick(30);
    assert_eq!(f.state()["state"], "ads");
    assert!(camera().view_fov_degrees.unwrap() < camera().fov_degrees);
    f.key("KeyF", true);
    f.key("KeyF", false);
    f.tick(70);
    assert_eq!(f.state()["state"], "noGun");
    let stowed = camera();
    assert!(
        (stowed.view_fov_degrees.unwrap_or(stowed.fov_degrees) - stowed.fov_degrees).abs() < 0.1
    );
    f.key("KeyF", true);
    f.key("KeyF", false);
    f.tick(90);
    assert_eq!(
        f.state()["state"],
        "ads",
        "held aim must agree with the zoom after equipping"
    );
    assert!(camera().view_fov_degrees.unwrap() < camera().fov_degrees);
    f.mouse("right", false);
    f.tick(30);
    assert_eq!(f.state()["state"], "lowReady");
}

#[test]
fn holding_aim_with_a_stowed_weapon_does_not_block_sprint() {
    use countersplat_scene::{PhysicsBody, PlayerCamera, PlayerControls};
    let f = Fixture::new();
    {
        let mut w = f.world.borrow_mut();
        w.get_mut::<PhysicsBody>(f.player).unwrap().enabled = false;
        let c = w.get_mut::<PlayerCamera>(f.player).unwrap();
        c.yaw = 0.0;
        c.pitch = 0.0;
        w.set_dynamic_resource(
            "player.default_controls",
            serde_json::to_value(PlayerControls::default()).unwrap(),
        );
        w.set_dynamic_resource(
            "player.spawn_camera",
            serde_json::to_value(PlayerCamera::default()).unwrap(),
        );
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    f.lua
        .execute(&std::fs::read_to_string(root.join("assets/scripts/player_controls.lua")).unwrap())
        .unwrap();
    f.key("KeyF", true);
    f.key("KeyF", false);
    f.tick(40);
    assert_eq!(f.state()["state"], "noGun");
    f.key("KeyW", true);
    f.key("ShiftLeft", true);
    f.tick(6);
    let pos = || {
        f.world
            .borrow()
            .get::<PlayerCamera>(f.player)
            .unwrap()
            .position[2]
    };
    let start = pos();
    f.tick(30);
    let sprint = start - pos();
    f.mouse("right", true);
    let start = pos();
    f.tick(30);
    let held_aim = start - pos();
    assert!(
        (held_aim / sprint - 1.0).abs() < 0.03,
        "a stowed weapon cannot enter ADS or block sprint"
    );
    f.key("KeyF", true);
    f.key("KeyF", false);
    f.tick(70);
    assert_eq!(f.state()["state"], "ads");
    let start = pos();
    f.tick(30);
    let aimed = start - pos();
    assert!(
        (aimed / sprint
            - 1.0
                / f.world
                    .borrow()
                    .get::<PlayerControls>(f.player)
                    .unwrap()
                    .sprint_multiplier)
            .abs()
            < 0.03,
        "sprint is blocked again once ADS is available"
    );
}

#[test]
fn death_during_stow_or_equip_keeps_the_current_weapon_visibility() {
    for during_equip in [false, true] {
        let f = Fixture::new();
        f.tick(1);
        f.key("KeyF", true);
        f.key("KeyF", false);
        if during_equip {
            f.tick(40);
            f.key("KeyF", true);
            f.key("KeyF", false);
        }
        f.tick(12);
        let before = f
            .world
            .borrow_mut()
            .drain_events("first_person.frame")
            .pop()
            .unwrap();
        // Mid-holster: the footage is part-way out of frame at full opacity.
        assert_eq!(before["clip"], "holster");
        let opacity = before["opacity"].as_f64().unwrap();
        assert_eq!(opacity, 1.0);
        f.lua.dispatch("weapon.die", &json!({})).unwrap();
        f.tick(1);
        let after = f
            .world
            .borrow_mut()
            .drain_events("first_person.frame")
            .pop()
            .unwrap();
        let next = after["opacity"].as_f64().unwrap();
        assert!(
            next <= opacity + 0.01 && next >= opacity - 0.1,
            "death should continue from the visible pose: before={opacity}, after={next}"
        );
        assert_eq!(after["clip"], "holster", "death freezes the visible holster frame");
        assert_eq!(after["index"], before["index"]);
    }
}

#[test]
fn weapon_reset_restores_ammo_and_revives_without_changing_fire_mode() {
    let f = Fixture::new();
    f.lua.dispatch("weapon.mode", &json!({})).unwrap();
    f.lua.dispatch("weapon.mode", &json!({})).unwrap();
    f.mouse("left", true);
    f.tick(70);
    f.mouse("left", false);
    f.tick(1);
    assert!(f.state()["ammo"].as_u64().unwrap() < 30);
    f.lua.dispatch("weapon.die", &json!({})).unwrap();
    f.tick(60);
    assert_eq!(f.state()["state"], "dead");
    f.lua
        .dispatch("weapon.reset", &json!({"entity":f.entity.to_string()}))
        .unwrap();
    f.tick(1);
    assert_eq!(f.state()["state"], "lowReady");
    assert_eq!(f.state()["ammo"], 30);
    assert_eq!(f.state()["reserve"], 90);
    assert_eq!(f.state()["mode"], "auto");
    f.mouse("left", true);
    f.tick(35);
    f.mouse("left", false);
    f.tick(1);
    assert!(
        f.state()["shots"].as_u64().unwrap() > 0,
        "the reset weapon must fire again"
    );
}

/// Each sprint direction is its own state with its own clip: the carry from low ready lands in the direction the
/// keys choose, changing direction mid-sprint switches state with a crossfade, and releasing sprint carries back.
#[test]
fn each_sprint_direction_is_its_own_state_with_its_own_clip() {
    let f = Fixture::new();
    let last_frame = |f: &Fixture| f.world.borrow_mut().drain_events("first_person.frame").pop().unwrap();
    f.key("KeyA", true);
    f.key("ShiftLeft", true);
    f.tick(35);
    assert_eq!(f.state()["state"], "runLeft", "a strafing sprint carries straight into the left run");
    assert_eq!(last_frame(&f)["clip"], "runLeft");

    for (release, press, state) in [
        ("KeyA", "KeyD", "runRight"),
        ("KeyD", "KeyS", "runBackward"),
        ("KeyS", "KeyW", "runForward"),
    ] {
        f.key(release, false);
        f.key(press, true);
        f.tick(1);
        assert_eq!(f.state()["state"], state);
        let frame = last_frame(&f);
        assert_eq!(frame["clip"], state);
        if state != "runForward" {
            assert!(frame["transition_seconds"].as_f64().unwrap() > 0.0, "{state} crossfades in: {frame}");
        }
    }
    f.key("KeyA", true);
    f.tick(2);
    assert_eq!(f.state()["state"], "runForward", "a forward diagonal still runs forward");
    f.key("KeyW", false);
    f.tick(2);
    assert_eq!(f.state()["state"], "runLeft");

    f.key("ShiftLeft", false);
    f.tick(2);
    assert_eq!(last_frame(&f)["clip"], "runFromLow", "releasing sprint carries back to low ready");
    f.tick(40);
    assert_eq!(f.state()["state"], "lowReady");
}

/// Aimed shots in the shipped scene leave through the revolver's front sight (FirstPersonView.ads_sight, the
/// fiber-optic dot in the ADS frame) as the renderer draws it, not through the screen centre.
#[test]
fn ads_shots_leave_through_the_drawn_front_sight() {
    let f = Fixture::as_shipped();
    f.tick(10);
    f.mouse("right", true);
    f.tick(40);
    assert_eq!(f.state()["state"], "ads");
    f.lua
        .dispatch(
            "first_person.presentation",
            &json!({"visible": true, "clip": "raising", "index": 33,
                    "motion_x": 0.0, "motion_y": 0.0, "motion_roll": 0.0, "vertical_offset": 0.055}),
        )
        .unwrap();
    let (yaw, pitch, fov) = {
        let world = f.world.borrow();
        let camera = world.get::<countersplat_scene::PlayerCamera>(f.player).unwrap();
        (camera.yaw as f64, camera.pitch as f64, camera.view_fov_degrees.unwrap_or(camera.fov_degrees) as f64)
    };
    assert!(yaw.abs() < 1e-9 && pitch.abs() < 1e-9, "the projection below assumes a level camera facing -Z");
    f.mouse("left", true);
    f.mouse("left", false);
    f.tick(1);
    let shot = f.world.borrow_mut().drain_events("weapon.shot").pop().expect("an aimed shot");
    let d: Vec<f64> = (0..3).map(|i| shot["direction"][i].as_f64().unwrap()).collect();
    let tangent = (fov.to_radians() * 0.5).tan();
    let screen_x = 0.5 + d[0] / (-d[2]) / (2.0 * 16.0 / 9.0 * tangent);
    let screen_y = 0.5 - d[1] / (-d[2]) / (2.0 * tangent);
    let sight = f.world.borrow().get_dynamic(f.entity, "FirstPersonView").unwrap()["ads_sight"].clone();
    // A 16:9 view shows the frame whole; the renderer pushes it down by vertical_offset.
    let expected_x = sight[0].as_f64().unwrap() / 1280.0;
    let expected_y = sight[1].as_f64().unwrap() / 720.0 + 0.055;
    assert!((screen_x - expected_x).abs() < 1e-6, "x {screen_x} vs {expected_x}");
    assert!((screen_y - expected_y).abs() < 1e-6, "y {screen_y} vs {expected_y}");
    assert!(screen_y < 0.5, "the sight is drawn above the screen centre, so the shot goes there");
}
