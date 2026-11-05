//! Run real player and posture Lua against scene ECS components.
mod common;
use countersplat_ecs::{Entity, World, WorldOperation, WorldOutcome};
use countersplat_scene::{
    PhysicsBody, PlayerCamera, PlayerControls, SceneManifest, VoxelColliderState, VoxelStatus,
};
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
    player: Entity,
    crouch_button: Entity,
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
            &["ui", "player", "level"],
        )
        .unwrap();
        {
            let mut w = world.borrow_mut();
            let state = w
                .get_mut::<VoxelColliderState>(ids["active_level"])
                .unwrap();
            state.status = VoxelStatus::Ready;
            state.resolution = Some(0.05);
            w.get_mut::<PhysicsBody>(ids["player"])
                .unwrap()
                .gravity_enabled = false;
            w.set_dynamic_resource("voxel.capture_movement", json!(true));
            w.set_dynamic_resource(
                "player.default_controls",
                serde_json::to_value(PlayerControls::default()).unwrap(),
            );
            w.set_dynamic_resource(
                "player.spawn_camera",
                serde_json::to_value(PlayerCamera::default()).unwrap(),
            );
        }
        let lua = LuaRuntime::new(Bridge(world.clone()), ScriptOptions::default()).unwrap();
        for path in [
            "player_posture.lua",
            "player_controls.lua",
            "physics_system.lua",
        ] {
            lua.execute(&std::fs::read_to_string(root.join("assets/scripts").join(path)).unwrap())
                .unwrap();
        }
        Self {
            world,
            lua,
            player: ids["player"],
            crouch_button: ids["key_crouch"],
        }
    }
    fn tick(&self, n: usize) {
        for _ in 0..n {
            self.lua.run_systems(1.0 / 60.0).unwrap();
        }
    }
    fn key(&self, code: &str, pressed: bool) {
        self.lua.dispatch("input.key",&json!({"code":code,"key":code,"pressed":pressed,"is_repeat":false,"gameplay":true})).unwrap();
    }
    fn camera(&self) -> PlayerCamera {
        self.world
            .borrow()
            .get::<PlayerCamera>(self.player)
            .unwrap()
            .clone()
    }
    fn sprint_multiplier(&self) -> f32 {
        self.world
            .borrow()
            .get::<PlayerControls>(self.player)
            .unwrap()
            .sprint_multiplier
    }
    fn body(&self) -> PhysicsBody {
        self.world
            .borrow()
            .get::<PhysicsBody>(self.player)
            .unwrap()
            .clone()
    }
    fn posture(&self) -> Value {
        self.world
            .borrow()
            .get_dynamic(self.player, "PlayerPosture")
            .unwrap()
            .clone()
    }
    fn feet(&self) -> f32 {
        let c = self.camera();
        let b = self.body();
        c.position[1] + b.collision_offset_y - b.radius - b.capsule_half_height
    }
    fn next_probe(&self) -> Value {
        self.world
            .borrow_mut()
            .drain_events("physics.clearance_request")
            .pop()
            .expect("stand clearance request")
    }
    fn result(&self, probe: &Value, clear: bool) {
        self.lua
            .dispatch(
                "physics.clearance_result",
                &json!({"entity":self.player.to_string(),"token":probe["token"],"clear":clear}),
            )
            .unwrap();
    }
}

#[test]
fn ctrl_crouches_the_capsule_preserves_feet_and_checks_the_ceiling() {
    let f = Fixture::new();
    let feet = f.feet();
    let standing = f.camera();
    f.key("KeyC", true);
    f.tick(1);
    assert_eq!(f.posture()["crouched"], true);
    assert!((f.feet() - feet).abs() < 0.0001);
    assert!(f.camera().position[1] < standing.position[1] - 0.6);
    assert!(
        f.camera().render_position()[1] > f.camera().position[1] + 0.5,
        "eye must not snap down"
    );
    f.tick(20);
    assert!((f.camera().view_posture_offset_y).abs() < 0.001);
    assert!((f.posture()["amount"].as_f64().unwrap() - 1.0).abs() < 1e-6);
    let crouched_eye = f.camera().render_position()[1];
    f.key("KeyC", false);
    f.tick(1);
    let first = f.next_probe();
    assert_eq!(
        f.world
            .borrow()
            .get_dynamic_resource("player.stand_probe_pending"),
        Some(&json!(true))
    );
    f.key("KeyW", true);
    let before = f.camera().position;
    f.tick(2);
    assert_eq!(
        f.camera().position,
        before,
        "movement waits for an outstanding stand check"
    );
    f.result(&first, false);
    f.tick(1);
    assert_eq!(f.posture()["crouched"], true);
    assert_eq!(f.posture()["blocked"], true);
    f.key("KeyW", false);
    let mut next = None;
    for _ in 0..30 {
        f.tick(1);
        let p = f
            .world
            .borrow_mut()
            .drain_events("physics.clearance_request");
        if let Some(probe) = p.last() {
            next = Some(probe.clone());
            break;
        }
    }
    let next = next.expect("blocked player retries the clearance check");
    f.result(&next, true);
    assert_eq!(f.posture()["crouched"], false);
    assert!((f.feet() - feet).abs() < 0.0001);
    assert!(
        (f.camera().render_position()[1] - crouched_eye).abs() < 0.0001,
        "standing starts without a visible eye jump"
    );
    f.tick(20);
    assert!((f.camera().view_posture_offset_y).abs() < 0.001);
    assert!((f.camera().position[1] - standing.position[1]).abs() < 0.001);
}

#[test]
fn stale_results_and_repressed_ctrl_cannot_expand_the_body() {
    let f = Fixture::new();
    f.key("KeyC", true);
    f.tick(20);
    assert_eq!(f.posture()["crouched"], true);
    f.key("KeyC", false);
    f.tick(1);
    let stale = f.next_probe();
    f.world
        .borrow_mut()
        .get_mut::<PlayerCamera>(f.player)
        .unwrap()
        .position[0] += 0.2;
    f.result(&stale, true);
    assert_eq!(f.posture()["crouched"], true);
    f.tick(1);
    let next = f.next_probe();
    f.key("KeyC", true);
    f.result(&next, true);
    assert_eq!(f.posture()["crouched"], true);
    f.lua.dispatch("input.reset", &json!({})).unwrap();
    f.tick(1);
    let fresh = f.next_probe();
    f.result(&fresh, true);
    assert_eq!(f.posture()["crouched"], false);
}

#[test]
fn shift_runs_and_crouch_overrides_sprint_with_rebindable_ctrl() {
    let f = Fixture::new();
    f.key("KeyW", true);
    f.tick(6);
    let walk = -f.camera().position[2];
    f.world
        .borrow_mut()
        .get_mut::<PlayerCamera>(f.player)
        .unwrap()
        .position[2] = 0.0;
    f.key("ShiftRight", true);
    f.tick(6);
    let run = -f.camera().position[2];
    assert!(
        (run / walk - f.sprint_multiplier()).abs() < 0.01,
        "either Shift should run"
    );
    // Crouching mid-sprint slides first (crouching_mid_sprint_slides_then_slows_to_a_crouch); once the slide
    // has run out, crouch speed takes priority over Shift.
    f.key("KeyC", true);
    f.tick(60);
    f.world
        .borrow_mut()
        .get_mut::<PlayerCamera>(f.player)
        .unwrap()
        .position[2] = 0.0;
    f.tick(6);
    let crouch = -f.camera().position[2];
    assert!(
        (crouch / walk - 0.45).abs() < 0.01,
        "crouch takes priority over Shift"
    );
    f.lua
        .dispatch(
            "ui.action",
            &json!({"id":f.crouch_button.to_string(),"kind":{"type":"clicked"}}),
        )
        .unwrap();
    f.key("KeyX", true);
    assert_eq!(
        f.world
            .borrow()
            .get::<PlayerControls>(f.player)
            .unwrap()
            .bindings
            .crouch,
        "KeyX"
    );
}

#[test]
fn scripted_stand_probe_rejects_a_real_voxel_ceiling_then_allows_an_opening() {
    use countersplat_physics::voxel_collision::VoxelCollider;
    use countersplat_scene::PlayerRig;
    let f = Fixture::new();
    f.world
        .borrow_mut()
        .get_mut::<PlayerRig>(f.player)
        .unwrap()
        .eye_height_m = 2.0;
    f.tick(1);
    f.world
        .borrow_mut()
        .get_mut::<PlayerCamera>(f.player)
        .unwrap()
        .position = [1.5, 3.0, 1.5];
    let metadata = json!({"version":"1.1","gridBounds":{"min":[0,0,0],"max":[4,4,4]},
        "voxelResolution":1.0,"leafSize":4,"treeDepth":0,"nodeCount":1,"leafDataCount":2});
    let mut mask = 0u64;
    for z in 0..4 {
        for x in 0..4 {
            mask |= 1 << (x + z * 16);
        }
    }
    mask |= 1 << (1 + 3 * 4 + 1 * 16); // Low ceiling directly above the crouched player.
    let binary: Vec<u8> = [0u32, mask as u32, (mask >> 32) as u32]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect();
    let voxels =
        VoxelCollider::from_json_and_bin(metadata.to_string().as_bytes(), &binary).unwrap();
    let clear = |probe: &Value| {
        let center = [0, 1, 2].map(|i| probe["center"][i].as_f64().unwrap() as f32);
        voxels
            .capsule_clear(
                center,
                probe["radius"].as_f64().unwrap() as f32,
                probe["half_height"].as_f64().unwrap() as f32,
            )
            .unwrap()
    };
    f.key("KeyC", true);
    f.tick(20);
    assert!((f.feet() - 1.0).abs() < 0.0001);
    f.key("KeyC", false);
    f.tick(1);
    let blocked = f.next_probe();
    assert!(!clear(&blocked));
    f.result(&blocked, false);
    assert_eq!(f.posture()["blocked"], true);
    f.world
        .borrow_mut()
        .get_mut::<PlayerCamera>(f.player)
        .unwrap()
        .position[0] = 0.5;
    let mut opening = None;
    for _ in 0..30 {
        f.tick(1);
        let probes = f
            .world
            .borrow_mut()
            .drain_events("physics.clearance_request");
        if let Some(probe) = probes.last() {
            opening = Some(probe.clone());
            break;
        }
    }
    let opening = opening.expect("clearance was retried after moving");
    assert!(clear(&opening));
    f.result(&opening, true);
    assert_eq!(f.posture()["crouched"], false);
    assert!((f.feet() - 1.0).abs() < 0.0001);
}

#[test]
fn player_posture_drives_the_first_person_crouch_footage() {
    let f = Fixture::new();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    f.lua
        .execute(
            &std::fs::read_to_string(root.join("assets/scripts/first_person_controller.lua"))
                .unwrap(),
        )
        .unwrap();
    f.key("KeyC", true);
    for _ in 0..25 {
        f.tick(1);
        for event in f.world.borrow_mut().drain_events("weapon.posture") {
            f.lua.dispatch("weapon.posture", &event).unwrap();
        }
    }
    f.tick(1);
    let frames = f.world.borrow_mut().drain_events("first_person.frame");
    assert_eq!(frames.last().unwrap()["clip"], "crouchIdleLow");
    f.key("KeyW", true);
    f.tick(1);
    for event in f.world.borrow_mut().drain_events("weapon.posture") {
        f.lua.dispatch("weapon.posture", &event).unwrap();
    }
    f.tick(1);
    let frames = f.world.borrow_mut().drain_events("first_person.frame");
    assert_eq!(frames.last().unwrap()["clip"], "crouchWalkLow");
}

#[test]
fn input_reset_cancels_pending_clearance_without_accepting_old_results() {
    let f = Fixture::new();
    f.key("KeyC", true);
    f.tick(20);
    f.key("KeyC", false);
    f.tick(1);
    let old = f.next_probe();
    f.lua.dispatch("input.reset", &json!({})).unwrap();
    assert_eq!(
        f.world
            .borrow()
            .get_dynamic_resource("player.stand_probe_pending"),
        Some(&json!(false))
    );
    f.result(&old, true);
    assert_eq!(f.posture()["crouched"], true);
    f.tick(1);
    let fresh = f.next_probe();
    f.result(&fresh, true);
    assert_eq!(f.posture()["crouched"], false);
}
#[test]
fn aiming_blocks_sprint_speed_and_releasing_aim_restores_it() {
    let f = Fixture::new();
    f.key("KeyW", true);
    f.tick(6);
    let walk = -f.camera().position[2];
    let reset_position = || {
        f.world
            .borrow_mut()
            .get_mut::<PlayerCamera>(f.player)
            .unwrap()
            .position[2] = 0.0;
    };
    reset_position();
    f.key("ShiftRight", true);
    f.tick(6);
    let sprint = -f.camera().position[2];
    assert!((sprint / walk - f.sprint_multiplier()).abs() < 0.01);

    f.lua
        .dispatch(
            "input.mouse_button",
            &json!({"button":"right","pressed":true,"gameplay":true}),
        )
        .unwrap();
    reset_position();
    f.tick(6);
    assert!(
        (-f.camera().position[2] / walk - 1.0).abs() < 0.01,
        "ADS must walk despite Shift"
    );
    f.lua
        .dispatch(
            "input.mouse_button",
            &json!({"button":"right","pressed":false,"gameplay":false}),
        )
        .unwrap();
    reset_position();
    f.tick(6);
    assert!(
        (-f.camera().position[2] / walk - f.sprint_multiplier()).abs() < 0.01,
        "releasing ADS restores held sprint"
    );

    f.lua
        .dispatch("weapon.aim", &json!({"pressed":true}))
        .unwrap();
    reset_position();
    f.tick(6);
    assert!(
        (-f.camera().position[2] / walk - 1.0).abs() < 0.01,
        "scripted ADS also blocks sprint"
    );
    f.lua.dispatch("input.reset", &json!({})).unwrap();
    f.key("KeyW", true);
    f.key("ShiftRight", true);
    reset_position();
    f.tick(6);
    assert!(
        (-f.camera().position[2] / walk - f.sprint_multiplier()).abs() < 0.01,
        "reset clears the aim lock"
    );
}

/// C while sprinting slides: faster than the sprint at first, slowing by friction to crouch speed, with the body
/// crouched; `player.slide` events mark its start and end.
#[test]
fn crouching_mid_sprint_slides_then_slows_to_a_crouch() {
    let f = Fixture::new();
    let z = |f: &Fixture| -f.camera().position[2];
    f.key("KeyW", true);
    f.key("ShiftLeft", true);
    f.tick(30);
    let start = z(&f);
    f.tick(6);
    let sprint = z(&f) - start;
    assert!(f.world.borrow_mut().drain_events("player.slide").is_empty(), "sprinting alone does not slide");

    f.key("KeyC", true);
    let start = z(&f);
    f.tick(6);
    let slide = z(&f) - start;
    let events = f.world.borrow_mut().drain_events("player.slide");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["active"], true);
    assert!(slide > sprint * 1.1, "the slide starts faster than the sprint: {slide} vs {sprint}");
    assert_eq!(f.posture()["crouched"], true, "the body crouches into the slide");

    f.tick(60);
    let events = f.world.borrow_mut().drain_events("player.slide");
    assert_eq!(events.len(), 1, "the slide ends after slide_seconds");
    assert_eq!(events[0]["active"], false);
    let start = z(&f);
    f.tick(6);
    let crouch_walk = z(&f) - start;
    assert!(crouch_walk < sprint * 0.5, "it slows to crouch speed: {crouch_walk} vs {sprint}");
}

/// Releasing C does not end a slide: the body stays crouched while it runs. Releasing W cancels it, and the
/// player then stands (C is up) once the head clearance is confirmed.
#[test]
fn releasing_c_keeps_sliding_and_releasing_w_cancels_the_slide() {
    let f = Fixture::new();
    let z = |f: &Fixture| -f.camera().position[2];
    f.key("KeyW", true);
    f.key("ShiftLeft", true);
    f.tick(30);
    f.key("KeyC", true);
    f.tick(2);
    f.key("KeyC", false);
    f.tick(10);
    let started = f.world.borrow_mut().drain_events("player.slide");
    assert_eq!(started.len(), 1, "{started:?}");
    assert_eq!(f.posture()["crouched"], true, "still crouched with C released mid-slide");
    let before = z(&f);
    f.tick(6);
    assert!(z(&f) - before > 0.5, "still sliding at speed");

    f.key("KeyW", false);
    f.tick(1);
    let ended = f.world.borrow_mut().drain_events("player.slide");
    assert_eq!(ended.len(), 1);
    assert_eq!(ended[0]["active"], false, "releasing W cancels the slide");
    let stopped = z(&f);
    f.tick(10);
    assert!((z(&f) - stopped).abs() < 1e-6, "no more travel once cancelled");
    f.tick(1);
    let probe = f.next_probe();
    f.result(&probe, true);
    f.tick(20);
    assert_eq!(f.posture()["crouched"], false, "with C up the player stands after the slide");
}

/// With W held the slide runs its full length even after C is released, then the player stands.
#[test]
fn a_slide_runs_to_the_end_with_c_released_while_w_is_held() {
    let f = Fixture::new();
    f.key("KeyW", true);
    f.key("ShiftLeft", true);
    f.tick(30);
    f.key("KeyC", true);
    f.tick(2);
    f.key("KeyC", false);
    f.tick(40);
    assert!(f.world.borrow_mut().drain_events("player.slide").iter().all(|e| e["active"] == true));
    assert_eq!(f.posture()["crouched"], true, "crouched for the whole slide");
    f.tick(20);
    let ended = f.world.borrow_mut().drain_events("player.slide");
    assert_eq!(ended.len(), 1, "the slide finishes after slide_seconds");
    assert_eq!(ended[0]["active"], false);
    f.tick(1);
    let probe = f.next_probe();
    f.result(&probe, true);
    f.tick(20);
    assert_eq!(f.posture()["crouched"], false);
}

/// The slide bursts from sprint speed to its peak within slide_accel_seconds, then glides down steadily to crouch
/// speed by slide_seconds, without a sudden stop.
#[test]
fn a_slide_bursts_to_its_peak_then_glides_down_to_crouch_speed() {
    let f = Fixture::new();
    let z = |f: &Fixture| -f.camera().position[2];
    f.key("KeyW", true);
    f.key("ShiftLeft", true);
    f.tick(30);
    let before = z(&f);
    f.tick(1);
    let sprint = (z(&f) - before) * 60.0;
    f.key("KeyC", true);
    let mut speeds = Vec::new();
    for _ in 0..58 {
        let at = z(&f);
        f.tick(1);
        speeds.push((z(&f) - at) * 60.0);
    }
    let (peak_tick, peak) = speeds.iter().copied().enumerate().fold((0, 0.0f32), |a, (i, v)| if v > a.1 { (i, v) } else { a });
    assert!(peak_tick <= 10, "the peak comes within about 0.15 s: tick {peak_tick}");
    assert!((peak / sprint - 1.5).abs() < 0.05, "peak 1.5x sprint: {peak} vs {sprint}");
    assert!(speeds[..peak_tick].windows(2).all(|w| w[1] >= w[0] - 1e-4), "accelerates up to the peak: {speeds:?}");
    assert!(speeds[peak_tick..].windows(2).all(|w| w[1] <= w[0] + 1e-4), "then only slows: {speeds:?}");
    let last = *speeds.last().unwrap();
    assert!(last < sprint * 0.35, "near crouch speed by the end: {last}");
    let biggest_drop = speeds.windows(2).map(|w| w[0] - w[1]).fold(0.0f32, f32::max);
    assert!(biggest_drop < peak * 0.06, "no sudden stop: largest per-tick drop {biggest_drop}");
}
