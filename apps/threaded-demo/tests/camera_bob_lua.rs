//! Exercise the actual Lua view-bob script against the scene ECS and inspector.
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
            &std::fs::read(root.join("assets/scenes/gameplay.json")).unwrap(),
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
    for _ in 0..20 {
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
