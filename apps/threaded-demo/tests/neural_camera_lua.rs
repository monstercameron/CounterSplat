//! Exercise the ported camera controller with the actual Lua/ECS player systems.
mod common;
use countersplat_ecs::{Entity, World, WorldOperation, WorldOutcome};
use countersplat_scene::{
    CameraController, PhysicsBody, PlayerCamera, PlayerControls, SceneManifest,
};
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
    fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let manifest = SceneManifest::from_json(
            &common::gameplay_with_ready_weapon(&root),
        )
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
        world.borrow_mut().set_dynamic_resource(
            "player.default_controls",
            serde_json::to_value(PlayerControls::default()).unwrap(),
        );
        world.borrow_mut().set_dynamic_resource(
            "player.spawn_camera",
            serde_json::to_value(PlayerCamera::default()).unwrap(),
        );
        world.borrow_mut().set_dynamic_resource(
            "camera.default_controller",
            serde_json::to_value(CameraController::default()).unwrap(),
        );
        world
            .borrow_mut()
            .get_mut::<UiElement>(ids["camera_panel"])
            .unwrap()
            .collapsible
            .as_mut()
            .unwrap()
            .collapsed = false;
        let lua = LuaRuntime::new(LocalBridge(world.clone()), ScriptOptions::default()).unwrap();
        for name in [
            "player_controls.lua",
            "neural_camera.lua",
            "camera_bob.lua",
            "weapon_optics.lua",
            "camera_zoom.lua",
        ] {
            lua.execute(&std::fs::read_to_string(root.join("assets/scripts").join(name)).unwrap())
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
    fn action(&self, id: &str, kind: UiEventKind) {
        let events = countersplat_ui_ecs::apply_events(
            &mut self.world.borrow_mut(),
            [UiEvent {
                id: self.ids[id].to_string(),
                kind,
            }],
        );
        assert_eq!(events.len(), 1, "{id} should accept input");
        let events = self.world.borrow_mut().drain_events("ui.action");
        for event in events {
            self.lua.dispatch("ui.action", &event).unwrap();
        }
    }
}

#[test]
fn direct_look_turn_tilt_and_fallback_preserve_aim_and_position() {
    let f = Fixture::new();
    let player = f.ids["player"];
    let owner = f.ids["camera_controller"];
    let sensitivity = f
        .world
        .borrow()
        .get::<PlayerControls>(player)
        .unwrap()
        .mouse_sensitivity;
    f.lua
        .dispatch("input.mouse_delta", &json!({"dx":100.0,"dy":50.0}))
        .unwrap();
    let looked = f.camera();
    assert!(
        (looked.yaw - 100.0 * sensitivity).abs() < 0.0001,
        "look must run once: {looked:?}"
    );
    assert!((looked.pitch + 50.0 * sensitivity).abs() < 0.0001);
    let position = looked.position;
    let aim = looked.direction();
    f.lua.run_systems(1.0 / 60.0).unwrap();
    let tilted = f.camera();
    assert!(
        tilted.view_turn_roll.abs() > 0.00001,
        "mouse turn should tilt the rendered view"
    );
    assert_eq!(tilted.position, position);
    assert_eq!(tilted.direction(), aim, "tilt must not alter gameplay aim");
    assert_eq!(tilted.roll, 0.0, "authored camera roll is preserved");
    for _ in 0..120 {
        f.lua.run_systems(1.0 / 60.0).unwrap();
    }
    assert_eq!(f.camera().view_turn_roll, 0.0, "turn tilt should settle");
    f.lua
        .dispatch("input.mouse_delta", &json!({"dx":0.0,"dy":2000.0}))
        .unwrap();
    assert!((f.camera().pitch.to_degrees() + 80.0).abs() < 0.001);
    f.world
        .borrow_mut()
        .get_mut::<CameraController>(owner)
        .unwrap()
        .enabled = false;
    let before = f.camera().yaw;
    f.lua
        .dispatch("input.mouse_delta", &json!({"dx":25.0,"dy":0.0}))
        .unwrap();
    assert!(
        (f.camera().yaw - before - 25.0 * sensitivity).abs() < 0.0001,
        "disabled controller falls back to ordinary player look"
    );
    f.lua.dispatch("input.reset", &json!({})).unwrap();
    assert_eq!(f.camera().view_turn_roll, 0.0);
    assert!(f.world.borrow_mut().despawn(owner));
    f.lua.run_systems(1.0 / 60.0).unwrap();
    let before = f.camera().yaw;
    f.lua
        .dispatch("input.mouse_delta", &json!({"dx":25.0,"dy":0.0}))
        .unwrap();
    assert!(
        (f.camera().yaw - before - 25.0 * sensitivity).abs() < 0.0001,
        "removing the controller retains basic look"
    );
}

#[test]
fn aiming_reduces_turn_tilt_without_changing_the_mouse_look() {
    let f = Fixture::new();
    for _ in 0..30 {
        f.lua
            .dispatch("input.mouse_delta", &json!({"dx":5.0,"dy":0.0}))
            .unwrap();
        f.lua.run_systems(1.0 / 60.0).unwrap();
    }
    let normal = f.camera().view_turn_roll.abs();
    assert!(normal > 0.001);
    f.lua.dispatch("input.reset", &json!({})).unwrap();
    f.lua
        .dispatch(
            "input.mouse_button",
            &json!({"button":"right","pressed":true,"gameplay":true}),
        )
        .unwrap();
    for _ in 0..30 {
        f.lua
            .dispatch("input.mouse_delta", &json!({"dx":5.0,"dy":0.0}))
            .unwrap();
        f.lua.run_systems(1.0 / 60.0).unwrap();
    }
    assert!(f.camera().view_turn_roll.abs() < normal * 0.3);
}

#[test]
fn resolved_travel_drives_gait_and_aim_damps_it_without_moving_the_collider() {
    let f = Fixture::new();
    let player = f.ids["player"];
    f.world
        .borrow_mut()
        .set_dynamic_resource("voxel.capture_movement", json!(true));
    f.world
        .borrow_mut()
        .get_mut::<PhysicsBody>(player)
        .unwrap()
        .grounded = true;
    let step = |f: &Fixture| {
        f.lua.dispatch("physics.motion_resolved", &json!({"entity":player.to_string(), "attempted":[0.0,0.0,-0.0667], "moved":[0.0,0.0,-0.0667]})).unwrap();
        f.lua.run_systems(1.0 / 60.0).unwrap();
    };
    for _ in 0..12 {
        step(&f);
    }
    let walking = f.camera();
    assert_eq!(walking.position, [0.0; 3]);
    assert!(walking.view_offset.iter().any(|v| v.abs() > 0.001));
    assert!(walking.view_bob_pitch.abs() + walking.view_bob_roll.abs() > 0.0001);
    f.lua.dispatch("input.reset", &json!({})).unwrap();
    f.lua
        .dispatch(
            "input.mouse_button",
            &json!({"button":"right","pressed":true,"gameplay":true}),
        )
        .unwrap();
    for _ in 0..12 {
        step(&f);
    }
    let aiming = f.camera();
    let norm = |v: [f32; 3]| v.iter().map(|n| n * n).sum::<f32>().sqrt();
    assert!(
        norm(aiming.view_offset) < norm(walking.view_offset) * 0.35,
        "aim should reduce gait: {walking:?} {aiming:?}"
    );
    assert_eq!(aiming.position, [0.0; 3]);
    assert!(
        aiming.render_fov_degrees() < aiming.fov_degrees,
        "existing optical zoom still applies"
    );
    f.lua.dispatch("input.reset", &json!({})).unwrap();
    assert_eq!(f.camera().view_offset, [0.0; 3]);
    assert_eq!(f.camera().view_bob_pitch, 0.0);
    assert_eq!(f.camera().view_bob_roll, 0.0);
}

#[test]
fn controls_are_validated_saved_and_reset_from_the_scene() {
    let f = Fixture::new();
    let owner = f.ids["camera_controller"];
    f.action("camera_turn_strength_ui", UiEventKind::NumberChanged(2.0));
    f.action("camera_turn_enabled_ui", UiEventKind::BoolChanged(false));
    let c = f
        .world
        .borrow()
        .get::<CameraController>(owner)
        .unwrap()
        .clone();
    assert_eq!(c.turn_tilt_strength, 2.0);
    assert!(!c.turn_tilt_enabled);
    assert_eq!(
        f.world
            .borrow_mut()
            .drain_events("camera.controller_changed")
            .len(),
        2
    );
    let mut invalid = serde_json::to_value(&c).unwrap();
    invalid["pitch_limit_degrees"] = json!(120);
    assert!(f
        .world
        .borrow_mut()
        .write_component_value(owner, "CameraController", invalid)
        .is_err());
    assert_eq!(*f.world.borrow().get::<CameraController>(owner).unwrap(), c);
    f.action("camera_controller_reset", UiEventKind::Clicked);
    assert_eq!(
        *f.world.borrow().get::<CameraController>(owner).unwrap(),
        CameraController::default()
    );
}
