//! Run real player and posture Lua against scene ECS components.
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
            &std::fs::read(root.join("assets/scenes/gameplay.json")).unwrap(),
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
    f.key("ControlLeft", true);
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
    f.key("ControlLeft", false);
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
    f.key("ControlRight", true);
    f.tick(20);
    assert_eq!(f.posture()["crouched"], true);
    f.key("ControlRight", false);
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
    f.key("ControlRight", true);
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
    f.key("ControlRight", true);
    f.tick(15);
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
    f.key("KeyC", true);
    assert_eq!(
        f.world
            .borrow()
            .get::<PlayerControls>(f.player)
            .unwrap()
            .bindings
            .crouch,
        "KeyC"
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
    f.key("ControlLeft", true);
    f.tick(20);
    assert!((f.feet() - 1.0).abs() < 0.0001);
    f.key("ControlLeft", false);
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
    f.key("ControlLeft", true);
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
    f.key("ControlLeft", true);
    f.tick(20);
    f.key("ControlLeft", false);
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
