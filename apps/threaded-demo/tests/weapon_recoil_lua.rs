//! Exercise recoil, aim, and mouse compensation through the actual Lua ECS bridge.
mod common;
use countersplat_ecs::{Entity, World, WorldOperation, WorldOutcome};
use countersplat_scene::{PlayerCamera, PlayerControls, SceneManifest};
use countersplat_scripting_lua::{LuaRuntime, ScriptOptions, WorldBridge};
use serde_json::{json, Value};
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
            &["ui", "player"],
        )
        .unwrap();
        {
            let mut w = world.borrow_mut();
            let camera = w.get_mut::<PlayerCamera>(ids["player"]).unwrap();
            camera.yaw = 0.0;
            camera.pitch = 0.0;
            // Isolate actual camera recoil from footage motion and angular sway.
            for (component, field, value) in [
                ("WeaponAnimation", "idle_enabled", json!(false)),
                ("WeaponAnimation", "movement_strength", json!(0.0)),
                ("WeaponAnimation", "recoil_strength", json!(0.0)),
                ("AimStability", "idle_sway_deg", json!(0.0)),
                ("AimStability", "heat_sway_deg", json!(0.0)),
            ] {
                w.write_component_field(ids["first_person"], component, field, value)
                    .unwrap();
            }
        }
        let lua = LuaRuntime::new(Bridge(world.clone()), ScriptOptions::default()).unwrap();
        for script in [
            "neural_camera.lua",
            "aim_stability.lua",
            "first_person_controller.lua",
        ] {
            lua.execute(
                &std::fs::read_to_string(root.join("assets/scripts").join(script)).unwrap(),
            )
            .unwrap();
        }
        Self { world, lua, ids }
    }
    fn tick(&self, count: usize, rate: f64) {
        for _ in 0..count {
            self.lua.run_systems(1.0 / rate).unwrap();
        }
    }
    fn dispatch(&self, name: &str, data: Value) {
        self.lua.dispatch(name, &data).unwrap();
    }
    fn mouse(&self, button: &str, pressed: bool) {
        self.dispatch(
            "input.mouse_button",
            json!({"button":button,"pressed":pressed,"gameplay":true}),
        );
    }
    fn aim(&self) {
        self.mouse("right", true);
        self.tick(60, 60.0);
    }
    fn fire_single(&self) {
        self.mouse("left", true);
        self.mouse("left", false);
    }
    fn camera(&self) -> PlayerCamera {
        self.world
            .borrow()
            .get::<PlayerCamera>(self.ids["player"])
            .unwrap()
            .clone()
    }
    fn set_recoil(&self, field: &str, value: Value) {
        self.world
            .borrow_mut()
            .write_component_field(self.ids["first_person"], "WeaponRecoil", field, value)
            .unwrap();
    }
    fn shots(&self) -> Vec<Value> {
        self.world.borrow_mut().drain_events("weapon.shot")
    }
}

#[test]
fn shot_rises_smoothly_stays_raised_and_accepts_mouse_compensation() {
    let f = Fixture::new();
    f.aim();
    let initial = f.camera();
    f.fire_single();
    let first = f.shots().pop().expect("shot should fire");
    assert_eq!(
        f.camera().pitch,
        initial.pitch,
        "the shot must snapshot aim before its own kick"
    );

    f.tick(1, 60.0);
    let early = f.camera().pitch;
    f.tick(60, 60.0);
    let settled = f.camera().pitch;
    assert!(
        early > initial.pitch && settled > early,
        "camera rise must be smooth: {early}, {settled}"
    );
    assert!(
        settled.to_degrees() > 0.4,
        "one ADS shot should have a felt rise: {settled}"
    );
    f.tick(180, 60.0);
    assert!(
        (f.camera().pitch - settled).abs() < 0.00001,
        "aim must not automatically return"
    );

    // The next shot uses the raised aim rather than the original aim.
    f.fire_single();
    let next = f.shots().pop().unwrap();
    assert!(
        next["direction"][1].as_f64().unwrap() > first["direction"][1].as_f64().unwrap() + 0.004
    );
    f.tick(2, 60.0);
    let before_correction = f.camera().pitch;
    let sensitivity = f
        .world
        .borrow()
        .get::<PlayerControls>(f.ids["player"])
        .unwrap()
        .mouse_sensitivity;
    f.dispatch("input.mouse_delta", json!({"dx":0.0,"dy":3.0}));
    let corrected = f.camera().pitch;
    assert!((corrected - (before_correction - 3.0 * sensitivity)).abs() < 0.00001);
    f.tick(120, 60.0);
    let after = f.camera().pitch;
    assert!(
        after < settled * 2.0 - 0.003,
        "pending recoil must preserve mouse correction: {after}"
    );
}

#[test]
fn automatic_fire_accumulates_and_does_not_recentre() {
    let f = Fixture::new();
    f.aim();
    f.dispatch("weapon.mode", json!({}));
    f.dispatch("weapon.mode", json!({}));
    f.mouse("left", true);
    f.tick(60, 60.0);
    f.mouse("left", false);
    let shots = f.shots();
    assert!(shots.len() >= 9, "automatic shots: {}", shots.len());
    let at_release = f.camera().pitch;
    f.tick(120, 60.0);
    let settled = f.camera().pitch;
    assert!(
        settled.to_degrees() > 4.0,
        "a long burst should visibly climb: {settled}"
    );
    assert!(
        settled >= at_release,
        "remaining recoil may rise but never pull aim down"
    );
    f.tick(180, 60.0);
    assert!((f.camera().pitch - settled).abs() < 0.00001);
}

#[test]
fn recoil_settings_are_live_and_can_disable_queued_motion() {
    let f = Fixture::new();
    f.aim();
    f.fire_single();
    f.set_recoil("enabled", json!(false));
    f.tick(60, 60.0);
    assert_eq!(f.camera().pitch, 0.0, "disabling must discard queued kick");
    f.set_recoil("enabled", json!(true));
    f.set_recoil("vertical_degrees", json!(1.0));
    f.set_recoil("horizontal_degrees", json!(0.0));
    f.set_recoil("ads_multiplier", json!(1.0));
    f.tick(1, 60.0);
    f.fire_single();
    f.tick(60, 60.0);
    assert!(f.camera().pitch.to_degrees() > 0.95);
    assert!(f.camera().yaw.abs() < 0.00001);
}

#[test]
fn crouching_reduces_real_camera_recoil() {
    let standing = Fixture::new();
    standing.aim();
    standing.fire_single();
    standing.tick(60, 60.0);
    let upright = standing.camera().pitch;

    let crouched = Fixture::new();
    crouched.aim();
    crouched.dispatch("weapon.posture", json!({"crouch":1.0,"moving":false}));
    crouched.tick(120, 60.0);
    crouched.fire_single();
    crouched.tick(60, 60.0);
    let low = crouched.camera().pitch;
    assert!(
        low > 0.0 && low < upright * 0.8,
        "standing={upright}, crouched={low}"
    );
}

#[test]
fn recoil_respects_pitch_limit_and_is_stable_across_tick_rates() {
    let mut rises = Vec::new();
    for rate in [30_usize, 60, 144] {
        let f = Fixture::new();
        f.aim();
        f.fire_single();
        f.tick(rate * 2, rate as f64);
        rises.push(f.camera().pitch);
    }
    assert!(
        rises.iter().copied().fold(f32::NEG_INFINITY, f32::max)
            - rises.iter().copied().fold(f32::INFINITY, f32::min)
            < 0.00002,
        "{rises:?}"
    );

    let f = Fixture::new();
    f.aim();
    f.world
        .borrow_mut()
        .get_mut::<PlayerCamera>(f.ids["player"])
        .unwrap()
        .pitch = 79.99_f32.to_radians();
    f.fire_single();
    f.tick(60, 60.0);
    assert!(f.camera().pitch.to_degrees() <= 80.0001);

    // A scene can start above the controller's configured limit. Recoil must
    // not yank the view downward when it discovers that pre-existing pose.
    let f = Fixture::new();
    f.aim();
    f.world
        .borrow_mut()
        .get_mut::<PlayerCamera>(f.ids["player"])
        .unwrap()
        .pitch = 85.0_f32.to_radians();
    f.fire_single();
    f.tick(60, 60.0);
    assert!((f.camera().pitch.to_degrees() - 85.0).abs() < 0.001);
}

#[test]
fn ads_shots_follow_the_actual_presented_recoil_frame_and_motion() {
    for (aspect, blend) in [(16.0_f64 / 9.0, 1.0_f64), (4.0 / 3.0, 0.25)] {
        let f = Fixture::new();
        f.world
            .borrow_mut()
            .set_dynamic_resource("render.scene_aspect", json!(aspect));
        f.aim();
        // The displayed frame can lag the Lua animation request while images
        // decode. Use the actual frame, including its transform and blend.
        f.dispatch(
            "first_person.presentation",
            json!({
                "visible":true, "clip":"fireADS", "index":4,
                "motion_x":0.012, "motion_y":-0.018, "motion_roll":0.09,
                "vertical_offset":0.055, "blend":blend,
                "previous":{"clip":"raising", "index":33}
            }),
        );
        f.fire_single();
        let shot = f.shots().pop().expect("ADS shot");
        let dir = shot["direction"].as_array().unwrap();
        let dx = dir[0].as_f64().unwrap();
        let dy = dir[1].as_f64().unwrap();
        let dz = dir[2].as_f64().unwrap();
        let camera = f.camera();
        let tangent = ((camera.view_fov_degrees.unwrap_or(camera.fov_degrees) as f64).to_radians()
            * 0.5)
            .tan();
        let projected_x = 0.5 + dx / (-dz) / (2.0 * aspect * tangent);
        let projected_y = 0.5 - dy / (-dz) / (2.0 * tangent);

        let u = (640.0 + (638.0 - 640.0) * blend) / 1280.0;
        let v = (361.0 + (305.0 - 361.0) * blend) / 720.0;
        let x = u - 0.5;
        let y = v + 0.055 - 1.0;
        let c = 0.09_f64.cos();
        let r = 0.09_f64.sin();
        let frame_x = c * x - r * y + 0.5 + 0.012;
        let frame_y = r * x + c * y + 1.0 - 0.018;
        let source = 16.0 / 9.0;
        let (expected_x, expected_y) = if aspect > source {
            (0.5 + (frame_x - 0.5) * source / aspect, frame_y)
        } else {
            (frame_x, 1.0 + (frame_y - 1.0) * aspect / source)
        };
        assert!(
            (projected_x - expected_x).abs() < 1e-6,
            "x: {projected_x} vs {expected_x}"
        );
        assert!(
            (projected_y - expected_y).abs() < 1e-6,
            "y: {projected_y} vs {expected_y}"
        );
    }
}

#[test]
fn cleared_presentation_uses_the_scene_pose_instead_of_a_stale_frame() {
    let f = Fixture::new();
    f.aim();
    f.dispatch(
        "first_person.presentation",
        json!({
            "visible":true,"clip":"fireADS","index":4,"motion_x":0.1,"motion_y":-0.1
        }),
    );
    f.dispatch("first_person.presentation", json!({"visible":false}));
    f.fire_single();
    let shot = f.shots().pop().unwrap();
    // The default ADS landmark has no horizontal offset in an uncropped view.
    assert!(shot["direction"][0].as_f64().unwrap().abs() < 0.0001);
}
