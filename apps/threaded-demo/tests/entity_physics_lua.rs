//! Exercise the real inspector, physics and export APIs through ECS and Lua.
use countersplat_ecs::{World, WorldOperation, WorldOutcome};
use countersplat_scene::{PhysicsBody, PlayerCamera, PlayerControls, SceneManifest, Transform};
use countersplat_scripting_lua::{LuaRuntime, ScriptOptions, WorldBridge};
use countersplat_ui::{UiElement, UiEvent, UiEventKind};
use serde_json::json;
use std::{cell::RefCell, path::PathBuf, rc::Rc};

struct LocalBridge(Rc<RefCell<World>>);
impl WorldBridge for LocalBridge {
    fn request(&mut self, op: WorldOperation) -> Result<WorldOutcome, String> {
        self.0.borrow_mut().apply_operation(op)
    }
}

#[test]
fn inspector_edits_independent_bodies_and_lua_exports_drive_real_components() {
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
        &["ui", "player", "level", "scene"],
    )
    .unwrap();
    let player = ids["player"];
    let level = ids["active_level"];
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
    for script in [
        "player_controls.lua",
        "physics_system.lua",
        "entity_inspector.lua",
    ] {
        lua.execute(&std::fs::read_to_string(root.join("assets/scripts").join(script)).unwrap())
            .unwrap();
    }
    let action = |id: countersplat_ecs::Entity, kind: UiEventKind| {
        let accepted = countersplat_ui_ecs::apply_events(
            &mut world.borrow_mut(),
            [UiEvent {
                id: id.to_string(),
                kind,
            }],
        );
        assert_eq!(accepted.len(), 1);
        let events = world.borrow_mut().drain_events("ui.action");
        for event in events {
            lua.dispatch("ui.action", &event).unwrap();
        }
    };
    assert!(
        world
            .borrow()
            .get::<PhysicsBody>(player)
            .unwrap()
            .collision_enabled
    );
    action(ids["inspector_collision"], UiEventKind::BoolChanged(false));
    assert!(
        !world
            .borrow()
            .get::<PhysicsBody>(player)
            .unwrap()
            .collision_enabled
    );
    lua.execute(
        "local p=ecs.get_resource('scene.entities').player; engine.physics.move(p,{1,0,0})",
    )
    .unwrap();
    assert!(world.borrow_mut().drain_events("physics.move").is_empty());
    action(ids["inspector_collision"], UiEventKind::BoolChanged(true));
    lua.execute(
        "local p=ecs.get_resource('scene.entities').player; engine.physics.move(p,{1,0,0})",
    )
    .unwrap();
    assert_eq!(
        world.borrow_mut().drain_events("physics.move")[0]["entity"],
        player.to_string()
    );
    action(ids["inspector_export_1"], UiEventKind::NumberChanged(7.0));
    assert_eq!(
        world
            .borrow()
            .get::<PlayerControls>(player)
            .unwrap()
            .move_speed,
        7.0
    );
    assert_eq!(
        world
            .borrow_mut()
            .drain_events("player.controls_changed")
            .len(),
        1
    );
    // Export a shared engine resource and verify type/range checks preserve its old value.
    lua.execute(
        r#"
        local p=ecs.get_resource('scene.entities').player
        ecs.set_resource('test.tuning', {strength=2, active=true})
        engine.export(p,'strength',{resource='test.tuning',field='strength',min=0,max=10})
        engine.set_export(p,'strength',6)
        engine.export(p,'active',{resource='test.tuning',field='active',label='Active'})
        assert(not pcall(function() engine.set_export(p,'strength',100) end))
        assert(not pcall(function() engine.set_export(p,'strength',false) end))
        assert(ecs.get_resource('test.tuning').strength == 6)
    "#,
    )
    .unwrap();
    lua.run_systems(0.25).unwrap();
    action(ids["inspector_export_next"], UiEventKind::Clicked);
    action(ids["inspector_export_4"], UiEventKind::BoolChanged(false));
    assert_eq!(
        world.borrow().get_dynamic_resource("test.tuning").unwrap()["active"],
        false
    );
    // Editing the player collider keeps the capsule, eye and floor in agreement.
    let before_eye = world.borrow().get::<PlayerCamera>(player).unwrap().position[1];
    action(ids["inspector_radius"], UiEventKind::NumberChanged(0.3));
    {
        let w = world.borrow();
        let body = w.get::<PhysicsBody>(player).unwrap();
        assert!((body.radius - 0.3).abs() < 0.0001);
        assert!(
            (body.radius + body.capsule_half_height - body.collision_offset_y - 1.7).abs() < 0.0001
        );
    }
    action(ids["inspector_offset"], UiEventKind::NumberChanged(-0.825));
    {
        let w = world.borrow();
        let body = w.get::<PhysicsBody>(player).unwrap();
        let rig = w.get::<countersplat_scene::PlayerRig>(player).unwrap();
        let eye = w.get::<PlayerCamera>(player).unwrap().position[1];
        assert!((rig.eye_height_m - 1.8).abs() < 0.0001);
        assert!(
            (body.radius + body.capsule_half_height - body.collision_offset_y - 1.8).abs() < 0.0001
        );
        assert!((eye - before_eye - 0.1).abs() < 0.0001);
    }
    // Select a different entity, attach a body and enable gravity without changing the player.
    action(ids["inspector_next"], UiEventKind::Clicked);
    action(ids["inspector_attach"], UiEventKind::Clicked);
    assert!(world.borrow().has::<Transform>(level));
    action(ids["inspector_gravity"], UiEventKind::BoolChanged(true));
    assert!(
        world
            .borrow()
            .get::<PhysicsBody>(level)
            .unwrap()
            .gravity_enabled
    );
    assert!(
        world
            .borrow()
            .get::<PhysicsBody>(player)
            .unwrap()
            .gravity_enabled
    );
    lua.run_systems(0.1).unwrap();
    assert!(world.borrow().get::<Transform>(level).unwrap().position[1] < 0.0);
    let events = world.borrow_mut().drain_events("physics.move");
    assert!(events
        .iter()
        .any(|event| event["entity"] == level.to_string()));
    action(ids["inspector_enabled"], UiEventKind::BoolChanged(false));
    let before = world.borrow().get::<Transform>(level).unwrap().position;
    lua.run_systems(0.1).unwrap();
    assert_eq!(
        world.borrow().get::<Transform>(level).unwrap().position,
        before
    );
    action(ids["inspector_next"], UiEventKind::Clicked);
    action(ids["inspector_attach"], UiEventKind::Clicked);
    assert!(world.borrow().has::<PhysicsBody>(ids["skybox"]));
    // Invalid settings must never replace the existing body.
    let old = world.borrow().get::<PhysicsBody>(player).unwrap().clone();
    let mut invalid = serde_json::to_value(&old).unwrap();
    invalid["radius"] = json!(-1);
    assert!(world
        .borrow_mut()
        .write_component_value(player, "PhysicsBody", invalid)
        .is_err());
    assert_eq!(world.borrow().get::<PhysicsBody>(player).unwrap(), &old);
}

#[test]
fn grounded_jump_applies_player_velocity_to_the_physics_system() {
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
    world
        .borrow_mut()
        .set_dynamic_resource("voxel.capture_movement", json!(true));
    {
        let mut w = world.borrow_mut();
        let body = w.get_mut::<PhysicsBody>(player).unwrap();
        body.gravity_enabled = true;
        body.grounded = true;
    }
    let lua = LuaRuntime::new(LocalBridge(world.clone()), ScriptOptions::default()).unwrap();
    lua.execute(&std::fs::read_to_string(root.join("assets/scripts/player_controls.lua")).unwrap())
        .unwrap();
    lua.execute(&std::fs::read_to_string(root.join("assets/scripts/physics_system.lua")).unwrap())
        .unwrap();
    lua.dispatch(
        "input.key",
        &json!({"code":"Space","pressed":true,"is_repeat":false,"gameplay":true}),
    )
    .unwrap();
    lua.run_systems(0.1).unwrap();
    assert!(world.borrow().get::<PlayerCamera>(player).unwrap().position[1] > 0.3);
    assert!(!world.borrow().get::<PhysicsBody>(player).unwrap().grounded);
    assert_eq!(world.borrow_mut().drain_events("physics.move").len(), 1);
}

#[test]
fn lua_owns_collision_contact_and_error_response() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let manifest =
        SceneManifest::from_json(&std::fs::read(root.join("assets/scenes/gameplay.json")).unwrap())
            .unwrap();
    let world = Rc::new(RefCell::new(World::new()));
    countersplat_ui_ecs::register(&mut world.borrow_mut()).unwrap();
    countersplat_scene_ecs::register(&mut world.borrow_mut()).unwrap();
    let ids =
        countersplat_scene_ecs::instantiate_layers(&mut world.borrow_mut(), &manifest, &["player"])
            .unwrap();
    let player = ids["player"];
    {
        let mut w = world.borrow_mut();
        let body = w.get_mut::<PhysicsBody>(player).unwrap();
        body.velocity = [1.0, -2.0, 0.0];
    }
    let lua = LuaRuntime::new(LocalBridge(world.clone()), ScriptOptions::default()).unwrap();
    lua.execute(&std::fs::read_to_string(root.join("assets/scripts/physics_system.lua")).unwrap())
        .unwrap();
    lua.dispatch(
        "physics.motion_resolved",
        &json!({"entity":player.to_string(),"attempted":[1.0,-0.5,0.0],"moved":[0.3,0.0,0.0]}),
    )
    .unwrap();
    {
        let w = world.borrow();
        let body = w.get::<PhysicsBody>(player).unwrap();
        assert_eq!(body.velocity, [0.0; 3]);
        assert!(body.grounded);
    }
    assert_eq!(
        world.borrow_mut().drain_events("physics.collision").len(),
        1
    );
    lua.dispatch(
        "physics.motion_error",
        &json!({"entity":player.to_string(),"message":"query failed"}),
    )
    .unwrap();
    assert!(!world.borrow().get::<PhysicsBody>(player).unwrap().grounded);
    assert_eq!(world.borrow_mut().drain_events("physics.error").len(), 1);
}
