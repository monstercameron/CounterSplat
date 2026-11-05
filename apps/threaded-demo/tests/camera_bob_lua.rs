//! Exercise the actual Lua view-bob script against the scene ECS and inspector.
mod common;
use countersplat_ecs::{Entity, World, WorldOperation, WorldOutcome};
use countersplat_scene::{PhysicsBody, PlayerCamera, PlayerControls, SceneManifest};
use countersplat_scripting_lua::{LuaRuntime, ScriptOptions, WorldBridge};
use countersplat_ui::{UiElement, UiEvent, UiEventKind};
use serde_json::json;
use std::{cell::RefCell, collections::BTreeMap, path::PathBuf, rc::Rc};

struct LocalBridge(Rc<RefCell<World>>);
impl WorldBridge for LocalBridge {
    fn request(&mut self, op: WorldOperation) -> Result<WorldOutcome, String> {
        self.0.borrow_mut().apply_operation(op)
    }
}

struct Fixture {
    world: Rc<RefCell<World>>,
    ids: BTreeMap<String, Entity>,
    lua: LuaRuntime,
}
impl Fixture {
    fn new(scripts: &[&str]) -> Self {
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
            &["ui", "player", "level", "scene"],
        )
        .unwrap();
        world.borrow_mut().set_dynamic_resource(
            "player.default_controls",
            serde_json::to_value(PlayerControls::default()).unwrap(),
        );
        // The step-bob tests expect a still view at rest; breathing (always slightly alive) has its own tests.
        world.borrow_mut().get_mut::<PlayerControls>(ids["player"]).unwrap().breathing_enabled = false;
        world.borrow_mut().set_dynamic_resource(
            "player.spawn_camera",
            serde_json::to_value(PlayerCamera::default()).unwrap(),
        );
        world
            .borrow_mut()
            .get_mut::<UiElement>(ids["entity_inspector"])
            .unwrap()
            .visible = true;
        world
            .borrow_mut()
            .get_mut::<UiElement>(ids["entity_inspector"])
            .unwrap()
            .collapsible
            .as_mut()
            .unwrap()
            .collapsed = false;
        let lua = LuaRuntime::new(LocalBridge(world.clone()), ScriptOptions::default()).unwrap();
        for script in scripts {
            lua.execute(
                &std::fs::read_to_string(root.join("assets/scripts").join(script)).unwrap(),
            )
            .unwrap();
        }
        Self { world, ids, lua }
    }
    fn camera(&self) -> PlayerCamera {
        self.world
            .borrow()
            .get::<PlayerCamera>(self.ids["player"])
            .unwrap()
            .clone()
    }
    fn action(&self, name: &str, kind: UiEventKind) {
        let accepted = countersplat_ui_ecs::apply_events(
            &mut self.world.borrow_mut(),
            [UiEvent {
                id: self.ids[name].to_string(),
                kind,
            }],
        );
        assert_eq!(accepted.len(), 1, "action was not accepted: {name}");
        let events = self.world.borrow_mut().drain_events("ui.action");
        for event in events {
            self.lua.dispatch("ui.action", &event).unwrap();
        }
    }
}

#[test]
fn walking_bobs_only_the_visual_eye_then_smoothly_settles() {
    let f = Fixture::new(&["player_controls.lua", "camera_bob.lua"]);
    f.world
        .borrow_mut()
        .get_mut::<PhysicsBody>(f.ids["player"])
        .unwrap()
        .gravity_enabled = false;
    f.lua
        .dispatch(
            "input.key",
            &json!({"code":"KeyW", "pressed":true, "is_repeat":false, "gameplay":true}),
        )
        .unwrap();
    for _ in 0..12 {
        f.lua.run_systems(1.0 / 60.0).unwrap();
    }
    let walking = f.camera();
    assert!((walking.position[2] + 0.8).abs() < 0.001, "{walking:?}");
    assert_eq!(
        walking.position[1], 0.0,
        "bob must not move the collider or eye baseline"
    );
    assert!(
        walking.view_offset.iter().any(|v| v.abs() > 0.001),
        "{walking:?}"
    );
    assert_ne!(walking.render_position(), walking.position);

    f.lua
        .dispatch(
            "input.key",
            &json!({"code":"KeyW", "pressed":false, "is_repeat":false, "gameplay":true}),
        )
        .unwrap();
    for _ in 0..90 {
        f.lua.run_systems(1.0 / 60.0).unwrap();
    }
    let stopped = f.camera();
    assert_eq!(
        stopped.position, walking.position,
        "the visual offset must never accumulate into the player pose"
    );
    assert_eq!(
        stopped.view_offset, [0.0; 3],
        "the view should return to rest"
    );

    f.lua.execute("local p=ecs.get_resource('scene.entities').player; engine.set_export(p,'bob_enabled',false)").unwrap();
    f.lua
        .dispatch(
            "input.key",
            &json!({"code":"KeyW", "pressed":true, "is_repeat":false, "gameplay":true}),
        )
        .unwrap();
    for _ in 0..20 {
        f.lua.run_systems(1.0 / 60.0).unwrap();
    }
    let disabled = f.camera();
    assert!(disabled.position[2] < stopped.position[2]);
    assert_eq!(disabled.view_offset, [0.0; 3]);
    f.lua.dispatch("input.reset", &json!({})).unwrap();
    assert_eq!(f.camera().view_offset, [0.0; 3]);
}

#[test]
fn collision_resolved_travel_drives_bob_and_blocked_or_airborne_motion_settles() {
    let f = Fixture::new(&["camera_bob.lua"]);
    let player = f.ids["player"];
    f.world
        .borrow_mut()
        .set_dynamic_resource("voxel.capture_movement", json!(true));
    f.world
        .borrow_mut()
        .get_mut::<PhysicsBody>(player)
        .unwrap()
        .grounded = true;
    for _ in 0..10 {
        f.lua
            .dispatch(
                "physics.motion_resolved",
                &json!({"entity":player.to_string(), "moved":[0.0,0.0,-0.07]}),
            )
            .unwrap();
        f.lua.run_systems(1.0 / 60.0).unwrap();
    }
    let moving = f.camera();
    assert_eq!(moving.position, [0.0; 3]);
    assert!(moving.view_offset.iter().any(|v| v.abs() > 0.001));
    for _ in 0..90 {
        f.lua
            .dispatch(
                "physics.motion_resolved",
                &json!({"entity":player.to_string(), "moved":[0.0,0.0,0.0]}),
            )
            .unwrap();
        f.lua.run_systems(1.0 / 60.0).unwrap();
    }
    assert_eq!(
        f.camera().view_offset,
        [0.0; 3],
        "a blocked player must settle"
    );
    f.world
        .borrow_mut()
        .get_mut::<PhysicsBody>(player)
        .unwrap()
        .grounded = false;
    for _ in 0..90 {
        f.lua
            .dispatch(
                "physics.motion_resolved",
                &json!({"entity":player.to_string(), "moved":[0.0,0.0,-0.07]}),
            )
            .unwrap();
        f.lua.run_systems(1.0 / 60.0).unwrap();
    }
    assert_eq!(
        f.camera().view_offset,
        [0.0; 3],
        "airborne movement should not bob"
    );
}

#[test]
fn inspector_edits_and_persists_lua_bob_exports() {
    let f = Fixture::new(&[
        "player_controls.lua",
        "camera_bob.lua",
        "entity_inspector.lua",
    ]);
    f.action("inspector_export_next", UiEventKind::Clicked);
    f.action("inspector_export_3", UiEventKind::BoolChanged(false));
    f.action("inspector_export_4", UiEventKind::NumberChanged(3.5));
    f.action("inspector_export_next", UiEventKind::Clicked);
    f.action("inspector_export_1", UiEventKind::NumberChanged(0.08));
    f.action("inspector_export_2", UiEventKind::NumberChanged(0.04));
    f.action("inspector_export_3", UiEventKind::NumberChanged(18.0));
    let controls = f
        .world
        .borrow()
        .get::<PlayerControls>(f.ids["player"])
        .unwrap()
        .clone();
    assert!(!controls.bob_enabled);
    assert!((controls.bob_cadence - 3.5).abs() < 0.001);
    assert!((controls.bob_vertical - 0.08).abs() < 0.001);
    assert!((controls.bob_horizontal - 0.04).abs() < 0.001);
    assert!((controls.bob_smoothing - 18.0).abs() < 0.001);
    assert_eq!(
        f.world
            .borrow_mut()
            .drain_events("player.controls_changed")
            .len(),
        5
    );
    f.lua.execute("local p=ecs.get_resource('scene.entities').player; assert(not pcall(function() engine.set_export(p,'bob_vertical',1) end))").unwrap();
    f.lua
        .dispatch(
            "ui.action",
            &json!({"id":f.ids["controls_reset"].to_string(), "kind":{"type":"clicked"}}),
        )
        .unwrap();
    assert_eq!(
        *f.world
            .borrow()
            .get::<PlayerControls>(f.ids["player"])
            .unwrap(),
        PlayerControls::default()
    );
}

#[test]
fn bob_tracks_external_posture_changes_and_removal() {
    let standing = Fixture::new(&["camera_bob.lua"]);
    let crouched = Fixture::new(&["camera_bob.lua"]);
    for fixture in [&standing, &crouched] {
        let player = fixture.ids["player"];
        let mut world = fixture.world.borrow_mut();
        world.set_dynamic_resource("voxel.capture_movement", json!(true));
        world.get_mut::<PhysicsBody>(player).unwrap().grounded = true;
    }
    fn step(fixture: &Fixture) {
        fixture
            .lua
            .dispatch(
                "physics.motion_resolved",
                &json!({
                    "entity":fixture.ids["player"].to_string(), "moved":[0.0,0.0,-0.07]
                }),
            )
            .unwrap();
        fixture.lua.run_systems(1.0 / 60.0).unwrap();
    }
    for _ in 0..20 {
        step(&standing);
        step(&crouched);
    }
    let player = crouched.ids["player"];
    crouched
        .world
        .borrow_mut()
        .write_component_field(player, "PlayerPosture", "crouched", json!(true))
        .unwrap();
    for _ in 0..90 {
        step(&standing);
        step(&crouched);
    }
    let mut standing_energy = 0.0;
    let mut crouched_energy = 0.0;
    for _ in 0..60 {
        step(&standing);
        step(&crouched);
        standing_energy += standing
            .camera()
            .view_offset
            .iter()
            .map(|x| x * x)
            .sum::<f32>();
        crouched_energy += crouched
            .camera()
            .view_offset
            .iter()
            .map(|x| x * x)
            .sum::<f32>();
    }
    assert!(standing_energy > 0.0);
    assert!(
        crouched_energy < standing_energy * 0.3,
        "crouching should damp bob after an external edit"
    );
    crouched
        .world
        .borrow_mut()
        .remove_dynamic(player, "PlayerPosture");
    for _ in 0..120 {
        step(&standing);
        step(&crouched);
    }
    // Crouched steps come at the crouch cadence, so the two bobs are out of phase; removing posture restores the
    // standing bob's amplitude.
    let energy = |f: &Fixture| f.camera().view_offset.iter().map(|x| x * x).sum::<f32>();
    let (mut a, mut b) = (0.0, 0.0);
    for _ in 0..120 {
        step(&standing);
        step(&crouched);
        a += energy(&standing);
        b += energy(&crouched);
    }
    assert!((b / a - 1.0).abs() < 0.15, "removing posture restores standing bob: {b} vs {a}");
}

fn sprint_fixture() -> Fixture {
    let f = Fixture::new(&["camera_bob.lua"]);
    let player = f.ids["player"];
    f.world
        .borrow_mut()
        .set_dynamic_resource("voxel.capture_movement", json!(true));
    f.world
        .borrow_mut()
        .get_mut::<PhysicsBody>(player)
        .unwrap()
        .grounded = true;
    f.lua
        .dispatch(
            "input.key",
            &json!({"code":"ShiftLeft","pressed":true,"is_repeat":false,"gameplay":true}),
        )
        .unwrap();
    f
}
fn movement_tick(f: &Fixture, dt: f32, distance: f32) {
    f.lua
        .dispatch(
            "physics.motion_resolved",
            &json!({"entity":f.ids["player"].to_string(),"moved":[0.0,0.0,-distance]}),
        )
        .unwrap();
    f.lua.run_systems(dt.into()).unwrap();
}
fn blur(f: &Fixture) -> f64 {
    f.world
        .borrow()
        .get_dynamic_resource("camera.peripheral_blur")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0)
}
fn run_for(f: &Fixture, ticks: usize, distance: f32) {
    for _ in 0..ticks {
        movement_tick(f, 1.0 / 60.0, distance);
    }
}

#[test]
fn sprint_blur_follows_resolved_movement_and_turns_off_for_aim_crouch_air_and_reset() {
    let f = sprint_fixture();
    let player = f.ids["player"];
    let run_distance = 6.4 / 60.0;
    run_for(&f, 120, run_distance);
    assert!(
        blur(&f) > 0.40,
        "actual sprint should produce blur: {}",
        blur(&f)
    );
    run_for(&f, 90, 0.0);
    assert_eq!(blur(&f), 0.0, "blocked movement must settle");
    run_for(&f, 120, run_distance);
    f.lua
        .dispatch("weapon.aim", &json!({"pressed":true}))
        .unwrap();
    run_for(&f, 90, 4.0 / 60.0);
    assert_eq!(blur(&f), 0.0, "scripted ADS blocks sprint blur");
    f.lua
        .dispatch("weapon.aim", &json!({"pressed":false}))
        .unwrap();
    run_for(&f, 120, run_distance);
    assert!(blur(&f) > 0.40);
    f.world
        .borrow_mut()
        .write_component_field(player, "PlayerPosture", "crouched", json!(true))
        .unwrap();
    run_for(&f, 90, run_distance);
    assert_eq!(blur(&f), 0.0, "crouching blocks sprint blur");
    f.world
        .borrow_mut()
        .write_component_field(player, "PlayerPosture", "crouched", json!(false))
        .unwrap();
    run_for(&f, 120, run_distance);
    f.world
        .borrow_mut()
        .get_mut::<PhysicsBody>(player)
        .unwrap()
        .grounded = false;
    run_for(&f, 90, run_distance);
    assert_eq!(
        blur(&f),
        0.0,
        "airborne travel does not keep sprint blur alive"
    );
    f.world
        .borrow_mut()
        .get_mut::<PhysicsBody>(player)
        .unwrap()
        .grounded = true;
    f.world
        .borrow_mut()
        .get_mut::<PlayerControls>(player)
        .unwrap()
        .bob_enabled = false;
    run_for(&f, 120, run_distance);
    assert!(
        blur(&f) > 0.40,
        "blur remains separately configurable from bob"
    );
    assert_eq!(f.camera().view_offset, [0.0; 3]);
    f.world
        .borrow_mut()
        .write_component_field(player, "SprintVisuals", "peripheral_enabled", json!(false))
        .unwrap();
    run_for(&f, 90, run_distance);
    assert_eq!(
        blur(&f),
        0.0,
        "disabling the effect fades it out during a sprint"
    );
    f.world
        .borrow_mut()
        .write_component_field(player, "SprintVisuals", "peripheral_enabled", json!(true))
        .unwrap();
    run_for(&f, 120, run_distance);
    assert!(blur(&f) > 0.40);
    f.lua.dispatch("input.reset", &json!({})).unwrap();
    assert_eq!(blur(&f), 0.0);
}

#[test]
fn batched_collision_responses_keep_sprint_effect_stable() {
    let continuous = sprint_fixture();
    let batched = sprint_fixture();
    let distance = 6.4 / 60.0;
    let mut values = Vec::new();
    for tick in 0..240 {
        movement_tick(&continuous, 1.0 / 60.0, distance);
        movement_tick(
            &batched,
            1.0 / 60.0,
            if tick % 6 == 5 { distance * 6.0 } else { 0.0 },
        );
        if tick >= 180 {
            values.push(blur(&batched));
        }
    }
    let minimum = values.iter().copied().fold(f64::INFINITY, f64::min);
    let maximum = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    assert!(
        minimum > 0.25 && maximum - minimum < 0.12,
        "batched range: {minimum}..{maximum}"
    );
    assert!(
        (mean - blur(&continuous)).abs() < 0.12,
        "batched={mean}, continuous={}",
        blur(&continuous)
    );
}

#[test]
fn partial_sprint_configuration_is_normalized_and_toggleable() {
    let f = Fixture::new(&[]);
    f.lua.execute("local p=ecs.get_resource('scene.entities').player; ecs.set(p,'SprintVisuals',{peripheral_enabled=false})").unwrap();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    f.lua
        .execute(&std::fs::read_to_string(root.join("assets/scripts/camera_bob.lua")).unwrap())
        .unwrap();
    f.lua.execute("local p=ecs.get_resource('scene.entities').player; local v=ecs.get(p,'SprintVisuals'); assert(not v.peripheral_enabled and v.peripheral_strength == 0.45); engine.set_export(p,'sprint_blur_enabled',true)").unwrap();
}

#[test]
fn brief_voxel_ground_gaps_do_not_cut_running_effects_but_jumps_do() {
    let f = sprint_fixture();
    let player = f.ids["player"];
    let mut samples = Vec::new();
    for tick in 0..240 {
        // Brief gaps simulate the ground flag changing between voxel replies.
        f.world
            .borrow_mut()
            .get_mut::<PhysicsBody>(player)
            .unwrap()
            .grounded = tick % 4 == 0;
        movement_tick(&f, 1.0 / 60.0, 6.4 / 60.0);
        if tick > 180 {
            samples.push(blur(&f));
        }
    }
    let min = samples.into_iter().fold(f64::INFINITY, f64::min);
    assert!(
        min > 0.35,
        "temporary ground gaps must not flicker the sprint effect: {min}"
    );
    {
        let mut world = f.world.borrow_mut();
        let body = world.get_mut::<PhysicsBody>(player).unwrap();
        body.grounded = false;
        body.velocity[1] = 4.0;
    }
    run_for(&f, 90, 6.4 / 60.0);
    assert_eq!(blur(&f), 0.0, "jumping should suppress sprint blur");
}

fn gait_fixture() -> Fixture {
    let f = Fixture::new(&["player_controls.lua", "camera_bob.lua"]);
    f.world.borrow_mut().get_mut::<PhysicsBody>(f.ids["player"]).unwrap().gravity_enabled = false;
    f
}
fn press(f: &Fixture, code: &str, pressed: bool) {
    f.lua
        .dispatch("input.key", &json!({"code": code, "pressed": pressed, "is_repeat": false, "gameplay": true}))
        .unwrap();
}
fn settle(f: &Fixture, ticks: usize) {
    for _ in 0..ticks {
        f.lua.run_systems(1.0 / 60.0).unwrap();
    }
}
/// Holds `keys` for `ticks`, then releases them and lets the view settle. Returns the ground distance covered,
/// the footsteps emitted and the summed squared view bob.
fn travel(f: &Fixture, keys: &[&str], ticks: usize) -> (f32, usize, f32) {
    f.world.borrow_mut().drain_events("player.step");
    for key in keys {
        press(f, key, true);
    }
    let start = f.camera().position;
    let mut energy = 0.0;
    for _ in 0..ticks {
        f.lua.run_systems(1.0 / 60.0).unwrap();
        energy += f.camera().view_offset.iter().map(|x| x * x).sum::<f32>();
    }
    let end = f.camera().position;
    for key in keys {
        press(f, key, false);
    }
    let steps = f.world.borrow_mut().drain_events("player.step").len();
    settle(f, 120);
    (((end[0] - start[0]).powi(2) + (end[2] - start[2]).powi(2)).sqrt(), steps, energy)
}

/// Every gait has its own speed and each direction within it differs: W is fastest, strafing slower and S
/// slowest; Shift runs, Ctrl walks cautiously.
#[test]
fn gaits_and_directions_set_their_own_speeds() {
    let f = gait_fixture();
    let (walk_w, _, _) = travel(&f, &["KeyW"], 60);
    let (walk_d, _, _) = travel(&f, &["KeyD"], 60);
    let (walk_s, _, _) = travel(&f, &["KeyS"], 60);
    assert!((walk_w - 4.0).abs() < 0.05, "walking forward at move_speed: {walk_w}");
    assert!((walk_d / walk_w - 0.85).abs() < 0.01, "strafing walk: {walk_d}");
    assert!((walk_s / walk_w - 0.7).abs() < 0.01, "backing up: {walk_s}");
    let sprint = f.world.borrow().get::<PlayerControls>(f.ids["player"]).unwrap().sprint_multiplier;
    let (run_w, _, _) = travel(&f, &["ShiftLeft", "KeyW"], 60);
    let (run_d, _, _) = travel(&f, &["ShiftLeft", "KeyD"], 60);
    let (run_s, _, _) = travel(&f, &["ShiftLeft", "KeyS"], 60);
    assert!((run_w / walk_w - sprint).abs() < 0.02, "running forward at the sprint multiplier: {run_w}");
    assert!(run_w > run_d && run_d > run_s, "run W > strafe > S: {run_w} {run_d} {run_s}");
    assert!((run_s / run_w - 0.55).abs() < 0.01);
    let (cautious_w, _, _) = travel(&f, &["ControlLeft", "KeyW"], 60);
    assert!((cautious_w / walk_w - 0.5).abs() < 0.01, "a cautious walk is half speed: {cautious_w}");
    let (careful_run, _, _) = travel(&f, &["ControlLeft", "ShiftLeft", "KeyW"], 60);
    assert!((careful_run - cautious_w).abs() < 0.01, "Ctrl overrides Shift");
}

/// Footsteps are simulated per gait cadence and the bob is phased to them; the bob grows from a cautious walk to
/// a walk to a run.
#[test]
fn steps_tick_at_each_gaits_cadence_and_the_bob_grows_with_speed() {
    let f = gait_fixture();
    let (_, walk_steps, walk_bob) = travel(&f, &["KeyW"], 120);
    let (_, run_steps, run_bob) = travel(&f, &["ShiftLeft", "KeyW"], 120);
    let (_, cautious_steps, cautious_bob) = travel(&f, &["ControlLeft", "KeyW"], 120);
    let (_, back_steps, _) = travel(&f, &["KeyS"], 120);
    // Two seconds each: 2.0, 2.8 and 1.3 steps per second at each gait's forward speed.
    assert!((3..=4).contains(&walk_steps), "{walk_steps}");
    assert!((5..=6).contains(&run_steps), "{run_steps}");
    assert!((2..=3).contains(&cautious_steps), "{cautious_steps}");
    assert!(back_steps < walk_steps, "backing up takes slower steps: {back_steps}");
    assert!(run_bob > walk_bob && walk_bob > cautious_bob * 2.0, "{run_bob} {walk_bob} {cautious_bob}");
    let step = {
        press(&f, "KeyW", true);
        settle(&f, 40);
        press(&f, "KeyW", false);
        f.world.borrow_mut().drain_events("player.step").pop().unwrap()
    };
    assert_eq!(step["gait"], "walk");
    assert!(step["foot"] == "left" || step["foot"] == "right");
}

/// Breathing: running builds exertion and a faster, deeper breath; standing still settles it; the settings
/// toggle turns it off.
#[test]
fn breathing_builds_while_running_settles_at_rest_and_can_be_disabled() {
    let f = gait_fixture();
    f.lua.dispatch("settings.view_request", &json!({"breathing_enabled": true})).unwrap();
    let exertion = |f: &Fixture| f.world.borrow().get_dynamic_resource("player.exertion").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let sway = |f: &Fixture, ticks: usize| {
        let mut peak: f32 = 0.0;
        for _ in 0..ticks {
            f.lua.run_systems(1.0 / 60.0).unwrap();
            peak = peak.max(f.camera().view_offset[1].abs());
        }
        peak
    };
    let resting = sway(&f, 300);
    assert!(resting > 0.0, "a calm breath still moves the view slightly");
    press(&f, "ShiftLeft", true);
    press(&f, "KeyW", true);
    settle(&f, 480);
    assert!(exertion(&f) > 0.9, "eight seconds of running: {}", exertion(&f));
    press(&f, "ShiftLeft", false);
    press(&f, "KeyW", false);
    settle(&f, 30);
    let winded = sway(&f, 180);
    assert!(winded > resting * 3.0, "out of breath after the run: {winded} vs {resting}");
    settle(&f, 1800);
    assert!(exertion(&f) < 0.01, "breathing settles at rest: {}", exertion(&f));
    let recovered = sway(&f, 300);
    assert!(recovered < winded * 0.4, "the view steadies again: {recovered}");

    f.world.borrow_mut().drain_events("player.controls_changed");
    f.lua.dispatch("settings.view_request", &json!({"breathing_enabled": false})).unwrap();
    assert!(!f.world.borrow().get::<PlayerControls>(f.ids["player"]).unwrap().breathing_enabled);
    assert_eq!(f.world.borrow_mut().drain_events("player.controls_changed").len(), 1, "the toggle is saved");
    settle(&f, 120);
    assert_eq!(f.camera().view_offset, [0.0; 3], "no breathing sway when disabled");
}
