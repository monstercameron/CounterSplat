//! Exercise the real player Lua system against validated ECS camera and UI components.
use countersplat_ecs::{World, WorldOperation, WorldOutcome};
use countersplat_scene::{PlayerCamera, PlayerControls, PlayerRig, SceneManifest};
use countersplat_scripting_lua::{LuaRuntime, ScriptOptions, WorldBridge};
use countersplat_ui::{UiEvent, UiEventKind};
use serde_json::json;
use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
    rc::Rc,
};

struct LocalBridge(Rc<RefCell<World>>);
impl WorldBridge for LocalBridge {
    fn request(&mut self, op: WorldOperation) -> Result<WorldOutcome, String> {
        self.0.borrow_mut().apply_operation(op)
    }
}

// Exercise the browser's resource handoff without placing spawn decisions in
// the host. The ECS component still validates every script write.
fn spawn_fixture(
    source: &str,
    candidate: Option<serde_json::Value>,
    supplied_spawn: Option<PlayerCamera>,
) -> (
    Rc<RefCell<World>>,
    LuaRuntime,
    countersplat_ecs::Entity,
    PlayerCamera,
) {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let manifest =
        SceneManifest::from_json(&std::fs::read(root.join("assets/scenes/gameplay.json")).unwrap())
            .unwrap();
    let world = Rc::new(RefCell::new(World::new()));
    countersplat_ui_ecs::register(&mut world.borrow_mut()).unwrap();
    countersplat_scene_ecs::register(&mut world.borrow_mut()).unwrap();
    let ids = countersplat_scene_ecs::instantiate_layers(
        &mut world.borrow_mut(),
        &manifest,
        &["ui", "player", "level"],
    )
    .unwrap();
    let player = ids["player"];
    let authored = PlayerCamera::from_view([1.0, 2.0, 3.0], [1.0, 2.0, 2.0], 70.0).unwrap();
    {
        let mut w = world.borrow_mut();
        *w.get_mut::<PlayerCamera>(player).unwrap() = authored.clone();
        let mut preview = w.get_dynamic_resource("preview").unwrap().clone();
        preview["player_spawn"] = json!(source);
        w.set_dynamic_resource("preview", preview);
        w.set_dynamic_resource("player.default_controls", json!(PlayerControls::default()));
        if let Some(candidate) = candidate {
            w.set_dynamic_resource("player.level_view", candidate);
        }
        if let Some(spawn) = supplied_spawn {
            w.set_dynamic_resource("player.spawn_camera", json!(spawn));
        }
    }
    let lua = LuaRuntime::new(LocalBridge(world.clone()), ScriptOptions::default()).unwrap();
    (world, lua, player, authored)
}

fn load_player_controller(lua: &LuaRuntime) -> Result<(), String> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    lua.execute(&std::fs::read_to_string(root.join("assets/scripts/player_controls.lua")).unwrap())
        .map_err(|error| error.to_string())
}

#[test]
fn lua_selects_level_or_scene_spawn_and_sets_reset_pose() {
    let level = PlayerCamera::from_view([4.0, 5.0, 6.0], [4.0, 5.0, 5.0], 75.0).unwrap();
    for source in ["level", "scene"] {
        let (world, lua, player, authored) =
            spawn_fixture(source, Some(json!({"camera": level})), None);
        load_player_controller(&lua).unwrap();
        let expected = if source == "level" { &level } else { &authored };
        let w = world.borrow();
        assert_eq!(w.get::<PlayerCamera>(player).unwrap(), expected);
        assert_eq!(
            w.get_dynamic_resource("player.spawn_camera").unwrap(),
            &json!(expected)
        );
    }
}

#[test]
fn lua_validates_only_the_selected_spawn_and_supports_hosts_without_a_level_hint() {
    let invalid = Some(json!({"error": "bad camera"}));
    let (world, lua, player, authored) = spawn_fixture("scene", invalid.clone(), None);
    load_player_controller(&lua).unwrap();
    assert_eq!(
        world.borrow().get::<PlayerCamera>(player).unwrap(),
        &authored
    );
    assert_eq!(
        world
            .borrow()
            .get_dynamic_resource("player.spawn_camera")
            .unwrap(),
        &json!(authored)
    );

    let (_, lua, _, _) = spawn_fixture("level", invalid, None);
    assert!(load_player_controller(&lua)
        .unwrap_err()
        .contains("bad camera"));
    let (_, lua, _, _) = spawn_fixture("unknown", None, None);
    assert!(load_player_controller(&lua)
        .unwrap_err()
        .contains("preview.player_spawn"));

    let (world, lua, _, authored) = spawn_fixture("level", None, None);
    load_player_controller(&lua).unwrap();
    assert_eq!(
        world
            .borrow()
            .get_dynamic_resource("player.spawn_camera")
            .unwrap(),
        &json!(authored)
    );

    let supplied = PlayerCamera::default();
    let (world, lua, _, _) = spawn_fixture("level", None, Some(supplied.clone()));
    load_player_controller(&lua).unwrap();
    assert_eq!(
        world
            .borrow()
            .get_dynamic_resource("player.spawn_camera")
            .unwrap(),
        &json!(supplied)
    );
}

#[test]
fn lua_player_moves_looks_rebinds_and_resets_through_ecs() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let manifest =
        SceneManifest::from_json(&std::fs::read(root.join("assets/scenes/gameplay.json")).unwrap())
            .unwrap();
    let world = Rc::new(RefCell::new(World::new()));
    countersplat_ui_ecs::register(&mut world.borrow_mut()).unwrap();
    countersplat_scene_ecs::register(&mut world.borrow_mut()).unwrap();
    let ids = countersplat_scene_ecs::instantiate_layers(
        &mut world.borrow_mut(),
        &manifest,
        &["ui", "player"],
    )
    .unwrap();
    let player = ids["player"];
    world.borrow_mut().set_dynamic_resource(
        "player.default_controls",
        serde_json::to_value(PlayerControls::default()).unwrap(),
    );
    world.borrow_mut().set_dynamic_resource(
        "player.spawn_camera",
        serde_json::to_value(PlayerCamera::default()).unwrap(),
    );
    let lua = LuaRuntime::new(LocalBridge(world.clone()), ScriptOptions::default()).unwrap();
    lua.execute(
        &std::fs::read_to_string(root.join("assets/scripts/gameplay_navigation.lua")).unwrap(),
    )
    .unwrap();
    lua.execute(&std::fs::read_to_string(root.join("assets/scripts/player_controls.lua")).unwrap())
        .unwrap();

    lua.dispatch(
        "input.key",
        &json!({"code":"KeyW","pressed":true,"is_repeat":false,"gameplay":true}),
    )
    .unwrap();
    lua.run_systems(1.0).unwrap();
    assert!((world.borrow().get::<PlayerCamera>(player).unwrap().position[2] + 4.0).abs() < 0.001);
    lua.dispatch("input.reset", &json!({})).unwrap();
    let after_move = world.borrow().get::<PlayerCamera>(player).unwrap().position;
    lua.run_systems(1.0).unwrap();
    assert_eq!(
        world.borrow().get::<PlayerCamera>(player).unwrap().position,
        after_move
    );

    let sensitivity = world
        .borrow()
        .get::<PlayerControls>(player)
        .unwrap()
        .mouse_sensitivity;
    lua.dispatch("input.mouse_delta", &json!({"dx":100.0,"dy":50.0}))
        .unwrap();
    let camera = world.borrow().get::<PlayerCamera>(player).unwrap().clone();
    assert!(
        (camera.yaw - 100.0 * sensitivity).abs() < 0.001
            && (camera.pitch + 50.0 * sensitivity).abs() < 0.001
    );
    world
        .borrow_mut()
        .get_mut::<countersplat_ui::UiElement>(ids["controls_panel"])
        .unwrap()
        .collapsible
        .as_mut()
        .unwrap()
        .collapsed = false;
    let ui_event = |id: countersplat_ecs::Entity, kind| UiEvent {
        id: id.to_string(),
        kind,
    };
    let events = [
        ui_event(ids["controls_invert"], UiEventKind::BoolChanged(true)),
        ui_event(ids["controls_speed"], UiEventKind::NumberChanged(8.0)),
        ui_event(ids["controls_fov"], UiEventKind::NumberChanged(90.0)),
        ui_event(ids["key_forward"], UiEventKind::Clicked),
    ];
    assert_eq!(
        countersplat_ui_ecs::apply_events(&mut world.borrow_mut(), events).len(),
        4
    );
    let actions = world.borrow_mut().drain_events("ui.action");
    for event in actions {
        lua.dispatch("ui.action", &event).unwrap();
    }
    // Escape cancels a pending binding before the navigation handler can open the menu.
    lua.dispatch("input.key", &json!({"key":"Escape", "code":"Escape", "pressed":true, "is_repeat":false, "gameplay":true})).unwrap();
    assert!(world.borrow_mut().drain_events("engine.scene").is_empty());
    lua.dispatch(
        "ui.action",
        &json!({"id":ids["key_forward"].to_string(),"kind":{"type":"clicked"}}),
    )
    .unwrap();
    lua.dispatch("input.key", &json!({"key":"ArrowUp", "code":"ArrowUp", "pressed":true, "is_repeat":false, "gameplay":true})).unwrap();
    let controls = world
        .borrow()
        .get::<PlayerControls>(player)
        .unwrap()
        .clone();
    assert!(
        controls.invert_y
            && controls.move_speed == 8.0
            && controls.fov_degrees == Some(90.0)
            && controls.bindings.forward == "ArrowUp"
    );
    lua.dispatch("input.mouse_delta", &json!({"dx":0.0,"dy":50.0}))
        .unwrap();
    assert!(world.borrow().get::<PlayerCamera>(player).unwrap().pitch > camera.pitch);
    lua.dispatch(
        "input.key",
        &json!({"code":"ArrowUp","pressed":true,"is_repeat":false,"gameplay":true}),
    )
    .unwrap();
    let before = world.borrow().get::<PlayerCamera>(player).unwrap().position;
    lua.run_systems(0.5).unwrap();
    let after = world.borrow().get::<PlayerCamera>(player).unwrap().position;
    let distance = ((0..3).map(|i| (after[i] - before[i]).powi(2)).sum::<f32>()).sqrt();
    assert!((distance - 4.0).abs() < 0.001);
    lua.dispatch(
        "ui.action",
        &json!({"id":ids["reset"].to_string(),"kind":{"type":"clicked"}}),
    )
    .unwrap();
    assert_eq!(
        *world.borrow().get::<PlayerCamera>(player).unwrap(),
        PlayerCamera {
            fov_degrees: 90.0,
            ..PlayerCamera::default()
        }
    );
}

#[test]
fn jump_uses_the_bound_key_and_emits_collision_checked_movement() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let manifest =
        SceneManifest::from_json(&std::fs::read(root.join("assets/scenes/gameplay.json")).unwrap())
            .unwrap();
    let world = Rc::new(RefCell::new(World::new()));
    countersplat_ui_ecs::register(&mut world.borrow_mut()).unwrap();
    countersplat_scene_ecs::register(&mut world.borrow_mut()).unwrap();
    let ids = countersplat_scene_ecs::instantiate_layers(
        &mut world.borrow_mut(),
        &manifest,
        &["ui", "player"],
    )
    .unwrap();
    let player = ids["player"];
    // This test exercises the optional free-fly jump mode.
    world
        .borrow_mut()
        .get_mut::<countersplat_scene::PhysicsBody>(player)
        .unwrap()
        .gravity_enabled = false;
    world.borrow_mut().set_dynamic_resource(
        "player.default_controls",
        serde_json::to_value(PlayerControls::default()).unwrap(),
    );
    world.borrow_mut().set_dynamic_resource(
        "player.spawn_camera",
        serde_json::to_value(PlayerCamera::default()).unwrap(),
    );
    world
        .borrow_mut()
        .set_dynamic_resource("voxel.capture_movement", json!(true));
    let lua = LuaRuntime::new(LocalBridge(world.clone()), ScriptOptions::default()).unwrap();
    lua.execute(&std::fs::read_to_string(root.join("assets/scripts/player_controls.lua")).unwrap())
        .unwrap();

    let key = |code: &str, pressed: bool, repeat: bool| {
        json!({
            "code":code,"pressed":pressed,"is_repeat":repeat,"gameplay":true
        })
    };
    lua.dispatch("input.key", &key("Space", true, false))
        .unwrap();
    lua.run_systems(0.1).unwrap();
    let first = world.borrow().get::<PlayerCamera>(player).unwrap().position[1];
    assert!(
        first > 0.3 && first < 0.5,
        "the player should rise: {first}"
    );
    let moves = world.borrow_mut().drain_events("physics.move");
    assert!(
        moves[0]["delta"][1].as_f64().unwrap() > 0.3,
        "jump must go through the collision host"
    );
    // Releasing and pressing again in midair must not restart the arc.
    lua.dispatch("input.key", &key("Space", false, false))
        .unwrap();
    lua.dispatch("input.key", &key("Space", true, false))
        .unwrap();
    for _ in 0..6 {
        lua.run_systems(0.1).unwrap();
    }
    let landed = world.borrow().get::<PlayerCamera>(player).unwrap().position[1];
    assert!(
        landed.abs() < 0.001,
        "jump must return to its starting height: {landed}"
    );
    lua.run_systems(0.1).unwrap();
    assert!(
        world.borrow().get::<PlayerCamera>(player).unwrap().position[1].abs() < 0.001,
        "holding Space must not auto-repeat"
    );
    lua.dispatch("input.key", &key("Space", false, false))
        .unwrap();
    lua.dispatch("input.key", &key("Space", true, false))
        .unwrap();
    lua.run_systems(0.1).unwrap();
    assert!(world.borrow().get::<PlayerCamera>(player).unwrap().position[1] > 0.3);

    // Resolve the same Lua intents against a voxel floor and ceiling, as the browser worker does.
    lua.dispatch(
        "ui.action",
        &json!({"id":ids["reset"].to_string(),"kind":{"type":"clicked"}}),
    )
    .unwrap();
    world.borrow_mut().drain_events("physics.move");
    world
        .borrow_mut()
        .get_mut::<PlayerCamera>(player)
        .unwrap()
        .position = [1.5, 1.2, 1.5];
    let metadata = json!({"version":"1.1","gridBounds":{"min":[0,0,0],"max":[4,4,4]},
        "voxelResolution":1.0,"leafSize":4,"treeDepth":0,"nodeCount":1,"leafDataCount":2});
    let mut mask = 0u64;
    for z in 0..4 {
        for x in 0..4 {
            for y in [0, 2] {
                mask |= 1 << (x + y * 4 + z * 16);
            }
        }
    }
    let binary: Vec<u8> = [0u32, mask as u32, (mask >> 32) as u32]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect();
    let collider = countersplat_physics::voxel_collision::VoxelCollider::from_json_and_bin(
        metadata.to_string().as_bytes(),
        &binary,
    )
    .unwrap();
    lua.dispatch("input.key", &key("Space", false, false))
        .unwrap();
    lua.dispatch("input.key", &key("Space", true, false))
        .unwrap();
    let mut peak = 1.2f32;
    for _ in 0..45 {
        let start = world.borrow().get::<PlayerCamera>(player).unwrap().position;
        lua.run_systems(1.0 / 60.0).unwrap();
        let intents = world.borrow_mut().drain_events("physics.move");
        let mut position = start;
        for intent in intents {
            let delta = [0, 1, 2].map(|i| intent["delta"][i].as_f64().unwrap() as f32);
            position = collider.move_sphere(position, delta, 0.2).unwrap();
        }
        world
            .borrow_mut()
            .get_mut::<PlayerCamera>(player)
            .unwrap()
            .position = position;
        peak = peak.max(position[1]);
        assert!(
            position[1] >= 1.199 && position[1] <= 1.801,
            "floor and ceiling must contain the jump: {position:?}"
        );
    }
    assert!(peak > 1.7, "jump should reach the ceiling: {peak}");
    let landed = world.borrow().get::<PlayerCamera>(player).unwrap().position[1];
    assert!(
        (landed - 1.2).abs() < 0.01,
        "jump should return to the voxel floor: {landed}"
    );
}

#[test]
fn lua_rig_aligns_spawn_and_keeps_scale_motion_and_collider_consistent() {
    use countersplat_scene::{PhysicsBody, PlayerRig, VoxelColliderState, VoxelStatus};
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let manifest =
        SceneManifest::from_json(&std::fs::read(root.join("assets/scenes/gameplay.json")).unwrap())
            .unwrap();
    let world = Rc::new(RefCell::new(World::new()));
    countersplat_ui_ecs::register(&mut world.borrow_mut()).unwrap();
    countersplat_scene_ecs::register(&mut world.borrow_mut()).unwrap();
    let ids = countersplat_scene_ecs::instantiate_layers(
        &mut world.borrow_mut(),
        &manifest,
        &["ui", "player", "level"],
    )
    .unwrap();
    let player = ids["player"];
    {
        let mut w = world.borrow_mut();
        let state = w
            .get_mut::<VoxelColliderState>(ids["active_level"])
            .unwrap();
        state.status = VoxelStatus::Ready;
        state.resolution = Some(0.05);
    }
    world.borrow_mut().set_dynamic_resource(
        "player.default_controls",
        serde_json::to_value(PlayerControls::default()).unwrap(),
    );
    world.borrow_mut().set_dynamic_resource(
        "player.spawn_camera",
        serde_json::to_value(PlayerCamera::default()).unwrap(),
    );
    world
        .borrow_mut()
        .get_mut::<countersplat_ui::UiElement>(ids["controls_panel"])
        .unwrap()
        .collapsible
        .as_mut()
        .unwrap()
        .collapsed = false;
    let lua = LuaRuntime::new(LocalBridge(world.clone()), ScriptOptions::default()).unwrap();
    lua.execute(&std::fs::read_to_string(root.join("assets/scripts/player_controls.lua")).unwrap())
        .unwrap();
    lua.dispatch("physics.ready", &json!({"resolution":0.05}))
        .unwrap();
    let probe = world
        .borrow_mut()
        .drain_events("physics.ground_request")
        .pop()
        .unwrap();
    assert_eq!(probe["entity"], player.to_string());
    assert_eq!(probe["max_drop"], 4.0);
    // A missing floor leaves the source camera untouched.
    let original_eye = world.borrow().get::<PlayerCamera>(player).unwrap().position[1];
    lua.dispatch(
        "physics.ground_result",
        &json!({"entity":player.to_string(),"token":probe["token"],"height":null}),
    )
    .unwrap();
    assert_eq!(
        world.borrow().get::<PlayerCamera>(player).unwrap().position[1],
        original_eye
    );
    lua.dispatch("physics.ready", &json!({"resolution":0.05}))
        .unwrap();
    let probe = world
        .borrow_mut()
        .drain_events("physics.ground_request")
        .pop()
        .unwrap();
    lua.dispatch(
        "physics.ground_result",
        &json!({"entity":player.to_string(),"token":probe["token"],"height":-1.1}),
    )
    .unwrap();
    let eye = world.borrow().get::<PlayerCamera>(player).unwrap().position[1];
    assert!(
        (eye - 0.6).abs() < 0.0001,
        "eye should be 1.7 units above floor: {eye}"
    );
    let spawn = world
        .borrow()
        .get_dynamic_resource("player.spawn_camera")
        .unwrap()["position"][1]
        .as_f64()
        .unwrap();
    assert!(
        (spawn - 0.6).abs() < 0.0001,
        "reset should use aligned height"
    );
    // A result arriving after movement must not snap the player, even if the pose is restored.
    lua.dispatch("physics.ready", &json!({"resolution":0.05}))
        .unwrap();
    let stale = world
        .borrow_mut()
        .drain_events("physics.ground_request")
        .pop()
        .unwrap();
    lua.execute("local p=ecs.get_resource('scene.entities').player; local before=ecs.get(p,'PlayerCamera'); engine.physics.move(p,{1,0,0}); ecs.set(p,'PlayerCamera',before)").unwrap();
    lua.dispatch(
        "physics.ground_result",
        &json!({"entity":player.to_string(),"token":stale["token"],"height":-0.5}),
    )
    .unwrap();
    assert!((world.borrow().get::<PlayerCamera>(player).unwrap().position[1] - 0.6).abs() < 0.0001);
    // A scale adjustment preserves feet while scaling the collider and movement.
    lua.dispatch("ui.action", &json!({"id":ids["controls_player_scale"].to_string(),"kind":{"type":"number_changed","value":1.5}})).unwrap();
    {
        let w = world.borrow();
        let rig = w.get::<PlayerRig>(player).unwrap();
        let body = w.get::<PhysicsBody>(player).unwrap();
        let camera = w.get::<PlayerCamera>(player).unwrap();
        assert!((rig.units_per_meter - 1.5).abs() < 0.0001);
        assert!((body.radius - 0.375).abs() < 0.0001);
        assert!((body.capsule_half_height - 1.0125).abs() < 0.0001);
        assert!(
            (body.radius + body.capsule_half_height - body.collision_offset_y - 2.55).abs()
                < 0.0001
        );
        assert!((camera.position[1] - 1.45).abs() < 0.0001);
        assert!((body.gravity - 27.0).abs() < 0.0001);
        assert!(
            (body.collision_offset_y + body.capsule_half_height + body.radius - 0.225).abs()
                < 0.0001
        );
    }
    // Changing head clearance expands the capsule upward without moving the feet or eyes.
    lua.execute("local p=ecs.get_resource('scene.entities').player; local r=ecs.get(p,'PlayerRig'); r.head_clearance_m=0.25; ecs.set(p,'PlayerRig',r)").unwrap();
    lua.run_systems(0.01).unwrap();
    {
        let w = world.borrow();
        let body = w.get::<PhysicsBody>(player).unwrap();
        let camera = w.get::<PlayerCamera>(player).unwrap();
        assert!(
            (body.radius + body.capsule_half_height - body.collision_offset_y - 2.55).abs()
                < 0.0001
        );
        assert!(
            (body.collision_offset_y + body.capsule_half_height + body.radius - 0.375).abs()
                < 0.0001
        );
        assert!((camera.position[1] - 1.45).abs() < 0.0001);
    }
    lua.dispatch(
        "input.key",
        &json!({"code":"KeyW","pressed":true,"is_repeat":false,"gameplay":true}),
    )
    .unwrap();
    let before = world.borrow().get::<PlayerCamera>(player).unwrap().position;
    lua.run_systems(0.1).unwrap();
    let after = world.borrow().get::<PlayerCamera>(player).unwrap().position;
    let distance = ((after[0] - before[0]).powi(2) + (after[2] - before[2]).powi(2)).sqrt();
    assert!(
        (distance - 0.6).abs() < 0.0001,
        "movement should scale with the rig: {distance}"
    );
    // Return to the spawn first, so the scene-mode assertion cannot pass merely
    // because the player moved away from it.
    lua.execute("local p=ecs.get_resource('scene.entities').player; ecs.set(p,'PlayerCamera',ecs.get_resource('player.spawn_camera'))").unwrap();
    world.borrow_mut().drain_events("physics.ground_request");
    lua.dispatch("physics.ready", &json!({"resolution":0.05}))
        .unwrap();
    assert_eq!(
        world
            .borrow_mut()
            .drain_events("physics.ground_request")
            .len(),
        1
    );
    // Explicit scene spawns are authoritative and must not request auto alignment.
    let mut preview = world
        .borrow()
        .get_dynamic_resource("preview")
        .unwrap()
        .clone();
    preview["player_spawn"] = json!("scene");
    world.borrow_mut().set_dynamic_resource("preview", preview);
    world.borrow_mut().drain_events("physics.ground_request");
    lua.dispatch("physics.ready", &json!({"resolution":0.05}))
        .unwrap();
    assert!(world
        .borrow_mut()
        .drain_events("physics.ground_request")
        .is_empty());
}

#[test]
fn player_waits_for_ground_before_gravity_and_falls_back_if_collision_is_missing() {
    use countersplat_scene::{PhysicsBody, VoxelColliderState, VoxelStatus};
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let manifest =
        SceneManifest::from_json(&std::fs::read(root.join("assets/scenes/gameplay.json")).unwrap())
            .unwrap();
    let world = Rc::new(RefCell::new(World::new()));
    countersplat_ui_ecs::register(&mut world.borrow_mut()).unwrap();
    countersplat_scene_ecs::register(&mut world.borrow_mut()).unwrap();
    let ids = countersplat_scene_ecs::instantiate_layers(
        &mut world.borrow_mut(),
        &manifest,
        &["ui", "player", "level"],
    )
    .unwrap();
    let player = ids["player"];
    world.borrow_mut().set_dynamic_resource(
        "player.default_controls",
        serde_json::to_value(PlayerControls::default()).unwrap(),
    );
    world.borrow_mut().set_dynamic_resource(
        "player.spawn_camera",
        serde_json::to_value(PlayerCamera::default()).unwrap(),
    );
    let lua = LuaRuntime::new(LocalBridge(world.clone()), ScriptOptions::default()).unwrap();
    for name in ["player_controls.lua", "physics_system.lua"] {
        lua.execute(&std::fs::read_to_string(root.join("assets/scripts").join(name)).unwrap())
            .unwrap();
    }
    lua.dispatch(
        "input.key",
        &json!({"code":"KeyW","pressed":true,"is_repeat":false,"gameplay":true}),
    )
    .unwrap();
    for _ in 0..10 {
        lua.run_systems(0.1).unwrap();
    }
    assert_eq!(
        world.borrow().get::<PlayerCamera>(player).unwrap().position,
        [0.0; 3]
    );
    assert_eq!(
        world.borrow().get::<PhysicsBody>(player).unwrap().velocity,
        [0.0; 3]
    );

    {
        let mut w = world.borrow_mut();
        let state = w
            .get_mut::<VoxelColliderState>(ids["active_level"])
            .unwrap();
        state.status = VoxelStatus::Ready;
        state.resolution = Some(0.05);
    }
    lua.dispatch("physics.ready", &json!({"resolution":0.05}))
        .unwrap();
    let probe = world
        .borrow_mut()
        .drain_events("physics.ground_request")
        .pop()
        .unwrap();
    lua.run_systems(0.1).unwrap();
    assert_eq!(
        world.borrow().get::<PlayerCamera>(player).unwrap().position,
        [0.0; 3],
        "movement and gravity wait for the asynchronous floor query"
    );
    lua.dispatch(
        "physics.ground_result",
        &json!({
            "entity":player.to_string(), "token":probe["token"], "height":-1.7
        }),
    )
    .unwrap();
    lua.run_systems(0.1).unwrap();
    assert!(world.borrow().get::<PlayerCamera>(player).unwrap().position[2] < -0.3);
    assert!(world.borrow().get::<PhysicsBody>(player).unwrap().velocity[1] < 0.0);

    {
        let mut w = world.borrow_mut();
        let state = w
            .get_mut::<VoxelColliderState>(ids["active_level"])
            .unwrap();
        state.status = VoxelStatus::Unavailable;
        state.resolution = None;
        w.get_mut::<PhysicsBody>(player).unwrap().velocity = [0.0; 3];
    }
    let before = world.borrow().get::<PlayerCamera>(player).unwrap().position;
    lua.run_systems(0.1).unwrap();
    let after = world.borrow().get::<PlayerCamera>(player).unwrap().position;
    assert!(
        after[2] < before[2],
        "an unavailable collider permits free-fly navigation"
    );
    assert_eq!(
        after[1], before[1],
        "an unavailable collider must not drop the player"
    );
}

// Observe only script writes to UI components; native edits below simulate other
// systems or inspector changes without counting them as Lua synchronization.
struct CountingUiBridge(Rc<RefCell<World>>, Rc<Cell<usize>>);
impl WorldBridge for CountingUiBridge {
    fn request(&mut self, op: WorldOperation) -> Result<WorldOutcome, String> {
        if matches!(&op, WorldOperation::WriteComponent { name, .. } if name == "UiElement") {
            self.1.set(self.1.get() + 1);
        }
        self.0.borrow_mut().apply_operation(op)
    }
}

#[test]
fn controls_panel_skips_unchanged_writes_and_tracks_external_edits() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let manifest =
        SceneManifest::from_json(&std::fs::read(root.join("assets/scenes/gameplay.json")).unwrap())
            .unwrap();
    let world = Rc::new(RefCell::new(World::new()));
    countersplat_ui_ecs::register(&mut world.borrow_mut()).unwrap();
    countersplat_scene_ecs::register(&mut world.borrow_mut()).unwrap();
    let ids = countersplat_scene_ecs::instantiate_layers(
        &mut world.borrow_mut(),
        &manifest,
        &["ui", "player", "level"],
    )
    .unwrap();
    let player = ids["player"];
    {
        let mut w = world.borrow_mut();
        w.set_dynamic_resource("player.default_controls", json!(PlayerControls::default()));
        w.set_dynamic_resource("player.spawn_camera", json!(PlayerCamera::default()));
        w.get_mut::<countersplat_ui::UiElement>(ids["controls_panel"])
            .unwrap()
            .collapsible
            .as_mut()
            .unwrap()
            .collapsed = false;
    }
    let writes = Rc::new(Cell::new(0));
    let lua = LuaRuntime::new(
        CountingUiBridge(world.clone(), writes.clone()),
        ScriptOptions::default(),
    )
    .unwrap();
    load_player_controller(&lua).unwrap();
    writes.set(0);
    lua.run_systems(0.25).unwrap();
    assert_eq!(
        writes.get(),
        0,
        "an unchanged panel should not rewrite its widgets"
    );

    {
        let mut w = world.borrow_mut();
        let controls = w.get_mut::<PlayerControls>(player).unwrap();
        controls.move_speed = 7.5;
        controls.invert_y = true;
        controls.bindings.forward = "ArrowUp".into();
        w.get_mut::<PlayerRig>(player).unwrap().eye_height_m = 1.8;
        w.get_mut::<PlayerCamera>(player).unwrap().fov_degrees = 81.0;
    }
    lua.run_systems(0.25).unwrap();
    let widget = |name: &str| {
        world
            .borrow()
            .read_component_value(ids[name], "UiElement")
            .unwrap()
            .unwrap()["widget"]
            .clone()
    };
    assert_eq!(widget("controls_speed")["value"], json!(7.5));
    assert_eq!(widget("controls_invert")["checked"], json!(true));
    assert_eq!(widget("key_forward")["label"], json!("ArrowUp"));
    assert!((widget("controls_eye_height")["value"].as_f64().unwrap() - 1.8).abs() < 0.00001);
    assert_eq!(widget("controls_fov")["value"], json!(81.0));
    assert_eq!(writes.get(), 5, "only the five changed widgets need writes");

    writes.set(0);
    lua.run_systems(0.25).unwrap();
    assert_eq!(writes.get(), 0);
    if let countersplat_ui::Widget::Slider { value, .. } = &mut world
        .borrow_mut()
        .get_mut::<countersplat_ui::UiElement>(ids["controls_speed"])
        .unwrap()
        .widget
    {
        *value = 1.0;
    } else {
        panic!("speed control is not a slider");
    }
    lua.run_systems(0.25).unwrap();
    assert_eq!(widget("controls_speed")["value"], json!(7.5));
    assert_eq!(writes.get(), 1, "an externally edited widget is restored");
}
