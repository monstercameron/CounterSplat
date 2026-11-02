//! Real camera input and the Lua-owned weapon blur through the ECS bridge.
use countersplat_ecs::{Entity, World, WorldOperation, WorldOutcome};
use countersplat_scene::{
    CameraController, CameraLens, PlayerCamera, PlayerControls, SceneManifest,
};
use countersplat_scripting_lua::{LuaRuntime, ScriptOptions, WorldBridge};
use serde_json::json;
use std::{cell::RefCell, collections::BTreeMap, path::PathBuf, rc::Rc};

struct Bridge(Rc<RefCell<World>>);
impl WorldBridge for Bridge {
    fn request(&mut self, op: WorldOperation) -> Result<WorldOutcome, String> {
        self.0.borrow_mut().apply_operation(op)
    }
}
struct Fixture {
    world: Rc<RefCell<World>>,
    lua: LuaRuntime,
    ids: BTreeMap<String, Entity>,
}
impl Fixture {
    fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let manifest = SceneManifest::from_json(
            &std::fs::read(root.join("assets/scenes/gameplay.json")).unwrap(),
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
        let lua = LuaRuntime::new(Bridge(world.clone()), ScriptOptions::default()).unwrap();
        for script in [
            "player_controls.lua",
            "neural_camera.lua",
            "weapon_motion_blur.lua",
        ] {
            lua.execute(
                &std::fs::read_to_string(root.join("assets/scripts").join(script)).unwrap(),
            )
            .unwrap();
        }
        Self { world, lua, ids }
    }
    fn turn(&self, dx: f64, dy: f64, dt: f64) {
        self.lua
            .dispatch("input.mouse_delta", &json!({"dx":dx,"dy":dy}))
            .unwrap();
        self.lua.run_systems(dt).unwrap();
    }
    fn blur(&self) -> [f64; 2] {
        let world = self.world.borrow();
        let value = world
            .get_dynamic_resource("render.weapon_motion_blur")
            .unwrap();
        [value["x"].as_f64().unwrap(), value["y"].as_f64().unwrap()]
    }
    fn tick(&self, n: usize) {
        for _ in 0..n {
            self.lua.run_systems(1.0 / 60.0).unwrap();
        }
    }
    fn run_turn(&self, dx: f64, dy: f64, n: usize) {
        for _ in 0..n {
            self.turn(dx, dy, 1.0 / 60.0);
        }
    }
}
#[test]
fn real_camera_turn_drives_directional_bounded_blur_and_settles() {
    let f = Fixture::new();
    let player = f.ids["player"];
    let initial = f
        .world
        .borrow()
        .get::<PlayerCamera>(player)
        .unwrap()
        .clone();
    f.run_turn(8.0, 0.0, 30);
    let horizontal = f.blur();
    assert!(
        horizontal[0] > 0.002 && horizontal[0] < 0.012 && horizontal[1].abs() < 0.00001,
        "horizontal: {horizontal:?}"
    );
    let camera = f
        .world
        .borrow()
        .get::<PlayerCamera>(player)
        .unwrap()
        .clone();
    assert!(
        camera.yaw > initial.yaw && camera.position == initial.position,
        "blur must not alter camera motion"
    );
    f.tick(120);
    assert_eq!(f.blur(), [0.0, 0.0], "stopping the camera settles the blur");
    f.run_turn(0.0, 8.0, 15);
    let vertical = f.blur();
    assert!(
        vertical[1] > 0.002 && vertical[0].abs() < 0.00001,
        "vertical: {vertical:?}"
    );
    f.lua.dispatch("input.reset", &json!({})).unwrap();
    assert_eq!(f.blur(), [0.0, 0.0]);
}
#[test]
fn ads_and_shutter_settings_tune_blur_without_changing_camera_aim() {
    let f = Fixture::new();
    f.run_turn(6.0, 0.0, 24);
    let hip = f.blur()[0];
    f.lua.dispatch("input.reset", &json!({})).unwrap();
    f.lua
        .dispatch("weapon.aim", &json!({"pressed":true}))
        .unwrap();
    f.run_turn(6.0, 0.0, 24);
    let ads = f.blur()[0];
    assert!(
        (ads / hip - 0.5).abs() < 0.03,
        "ADS should reduce blur: hip={hip} ads={ads}"
    );
    f.lua.dispatch("input.reset", &json!({})).unwrap();
    f.world.borrow_mut().set_dynamic_resource(
        "camera.render_lens",
        json!({"motion_blur":true,"shutter_angle_degrees":90,"shutter_fps":24}),
    );
    f.run_turn(6.0, 0.0, 24);
    let short = f.blur()[0];
    assert!(
        (short / hip - 0.5).abs() < 0.03,
        "active render-lens shutter should scale blur: {short} {hip}"
    );
}
#[test]
fn disable_lens_or_component_and_external_camera_edits_do_not_cause_blur() {
    let f = Fixture::new();
    let player = f.ids["player"];
    let weapon = f.ids["first_person"];
    f.run_turn(6.0, 0.0, 24);
    assert!(f.blur()[0] > 0.0);
    f.world
        .borrow_mut()
        .get_mut::<CameraLens>(player)
        .unwrap()
        .motion_blur = false;
    f.tick(1);
    assert_eq!(f.blur(), [0.0, 0.0]);
    f.world
        .borrow_mut()
        .get_mut::<CameraLens>(player)
        .unwrap()
        .motion_blur = true;
    f.run_turn(6.0, 0.0, 24);
    assert!(f.blur()[0] > 0.0);
    f.world
        .borrow_mut()
        .write_component_field(weapon, "WeaponMotionBlur", "enabled", json!(false))
        .unwrap();
    f.tick(1);
    assert_eq!(f.blur(), [0.0, 0.0]);
    f.world
        .borrow_mut()
        .write_component_field(weapon, "WeaponMotionBlur", "enabled", json!(true))
        .unwrap();
    f.world
        .borrow_mut()
        .get_mut::<PlayerCamera>(player)
        .unwrap()
        .yaw = 2.0;
    f.tick(1);
    assert_eq!(
        f.blur(),
        [0.0, 0.0],
        "scripted view changes are not mouse swings"
    );
    f.world
        .borrow_mut()
        .get_mut::<PlayerControls>(player)
        .unwrap()
        .mouse_look = false;
    f.run_turn(8.0, 0.0, 12);
    assert_eq!(f.blur(), [0.0, 0.0]);
}
#[test]
fn wraparound_and_tick_rate_keep_blur_finite_and_consistent() {
    let f = Fixture::new();
    f.world
        .borrow_mut()
        .get_mut::<PlayerCamera>(f.ids["player"])
        .unwrap()
        .yaw = std::f32::consts::PI - 0.01;
    f.tick(1);
    f.run_turn(8.0, 0.0, 8);
    assert!(
        f.blur()[0] > 0.0 && f.blur()[0] < 0.0121,
        "yaw wrap must take the short path: {:?}",
        f.blur()
    );
    let mut values = Vec::new();
    for hz in [30usize, 60, 120] {
        let v = Fixture::new();
        for _ in 0..hz {
            v.turn(480.0 / hz as f64, 0.0, 1.0 / hz as f64);
        }
        values.push(v.blur()[0]);
    }
    assert!(
        (values[0] - values[1]).abs() < 0.0002 && (values[1] - values[2]).abs() < 0.0002,
        "tick rates: {values:?}"
    );
}

#[test]
fn look_pressed_against_pitch_limit_does_not_create_false_blur() {
    let f = Fixture::new();
    f.world
        .borrow_mut()
        .get_mut::<PlayerCamera>(f.ids["player"])
        .unwrap()
        .pitch = -80.0_f32.to_radians();
    f.tick(1);
    f.run_turn(0.0, 8.0, 30);
    assert!(
        f.blur()[1].abs() < 0.00001,
        "the camera is not rotating beyond its pitch limit: {:?}",
        f.blur()
    );
}
