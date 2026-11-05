//! Exercise the live Lua camera panel against validated ECS components and UI events.
mod common;
use countersplat_ecs::{World, WorldOperation, WorldOutcome};
use countersplat_scene::{CameraLens, CameraZoomLens, PlayerCamera, PlayerControls, SceneManifest};
use countersplat_scripting_lua::{LuaRuntime, ScriptOptions, WorldBridge};
use countersplat_ui::{UiElement, UiEvent, UiEventKind, Widget};
use std::{cell::RefCell, path::PathBuf, rc::Rc};

struct LocalBridge(Rc<RefCell<World>>);
impl WorldBridge for LocalBridge {
    fn request(&mut self, op: WorldOperation) -> Result<WorldOutcome, String> {
        self.0.borrow_mut().apply_operation(op)
    }
}

#[test]
fn controls_link_shutter_time_and_angle_and_edit_real_camera_components() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let manifest =
        SceneManifest::from_json(&common::gameplay_with_ready_weapon(&root))
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
    world.borrow_mut().set_dynamic_resource(
        "camera.default_lens",
        serde_json::to_value(CameraLens::default()).unwrap(),
    );
    world.borrow_mut().set_dynamic_resource(
        "camera.default_zoom_lens",
        serde_json::to_value(CameraZoomLens::default()).unwrap(),
    );
    world.borrow_mut().set_dynamic_resource(
        "player.default_controls",
        serde_json::to_value(PlayerControls::default()).unwrap(),
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
    lua.execute(&std::fs::read_to_string(root.join("assets/scripts/weapon_optics.lua")).unwrap())
        .unwrap();
    lua.execute(&std::fs::read_to_string(root.join("assets/scripts/camera_lens.lua")).unwrap())
        .unwrap();
    let action = |name: &str, kind: UiEventKind| {
        let accepted = countersplat_ui_ecs::apply_events(
            &mut world.borrow_mut(),
            [UiEvent {
                id: ids[name].to_string(),
                kind,
            }],
        );
        assert_eq!(accepted.len(), 1, "{name} should accept the input");
        let events = world.borrow_mut().drain_events("ui.action");
        for event in events {
            lua.dispatch("ui.action", &event).unwrap();
        }
    };
    let lens = || world.borrow().get::<CameraLens>(player).unwrap().clone();
    let camera = || world.borrow().get::<PlayerCamera>(player).unwrap().clone();
    action("camera_shutter_fps", UiEventKind::NumberChanged(60.0));
    action("camera_shutter_angle", UiEventKind::NumberChanged(90.0));
    assert!((lens().shutter_seconds() - 1.0 / 240.0).abs() < 1e-6);
    let time_ms = match world
        .borrow()
        .get::<UiElement>(ids["camera_shutter_ms"])
        .unwrap()
        .widget
    {
        Widget::Slider { value, .. } => value,
        _ => panic!("shutter time slider missing"),
    };
    assert!((time_ms - 1000.0 / 240.0).abs() < 0.001);
    action(
        "camera_shutter_ms",
        UiEventKind::NumberChanged(1000.0 / 120.0),
    );
    assert!((lens().shutter_angle_degrees - 180.0).abs() < 0.01);
    action("camera_shutter_fps", UiEventKind::NumberChanged(30.0));
    assert!((lens().shutter_seconds() - 1.0 / 60.0).abs() < 1e-6);
    let before_iris = lens();
    action("camera_aperture", UiEventKind::NumberChanged(2.8));
    let expected_ratio = (before_iris.aperture_f_number / 2.8).powi(2);
    assert!(
        (lens().exposure_multiplier() / before_iris.exposure_multiplier() - expected_ratio).abs()
            < 1e-4
    );
    action("camera_dof", UiEventKind::BoolChanged(false));
    assert!(!lens().depth_of_field);
    action("camera_dof", UiEventKind::BoolChanged(true));
    action("camera_motion_blur", UiEventKind::BoolChanged(false));
    assert!(!lens().motion_blur);
    action("camera_motion_blur", UiEventKind::BoolChanged(true));
    assert!(lens().depth_of_field && lens().motion_blur);
    action("camera_roll", UiEventKind::NumberChanged(20.0));
    action("camera_yaw", UiEventKind::NumberChanged(-120.0));
    action("camera_pitch", UiEventKind::NumberChanged(45.0));
    assert!((camera().roll.to_degrees() - 20.0).abs() < 0.001);
    assert!((camera().yaw.to_degrees() + 120.0).abs() < 0.001);
    assert!((camera().pitch.to_degrees() - 45.0).abs() < 0.001);
    action("camera_fov", UiEventKind::NumberChanged(90.0));
    assert_eq!(camera().fov_degrees, 90.0);
    assert_eq!(
        world
            .borrow()
            .get::<PlayerControls>(player)
            .unwrap()
            .fov_degrees,
        Some(90.0)
    );
    assert!(!world
        .borrow_mut()
        .drain_events("camera.lens_changed")
        .is_empty());
    action("camera_reset", UiEventKind::Clicked);
    assert_eq!(lens(), CameraLens::default());

    action("camera_aperture", UiEventKind::NumberChanged(2.8));
    action("camera_lens_zoom", UiEventKind::BoolChanged(true));
    assert!(
        !world
            .borrow()
            .get::<UiElement>(ids["camera_fov"])
            .unwrap()
            .visible
    );
    assert!(
        !world
            .borrow()
            .get::<UiElement>(ids["camera_zoom_factor"])
            .unwrap()
            .visible
    );
    assert!(
        world
            .borrow()
            .get::<UiElement>(ids["camera_zoom_info"])
            .unwrap()
            .visible
    );
    action("camera_optic", UiEventKind::Clicked); // red dot
    action("camera_optic", UiEventKind::Clicked); // scope
    assert!(
        world
            .borrow()
            .get::<UiElement>(ids["camera_zoom_factor"])
            .unwrap()
            .visible
    );
    action("camera_aperture", UiEventKind::NumberChanged(8.0));
    action("camera_iso", UiEventKind::NumberChanged(400.0));
    action("camera_zoom_factor", UiEventKind::NumberChanged(3.0));
    assert!((lens().aperture_f_number - 2.8).abs() < 1e-5);
    let zoom = world
        .borrow()
        .get::<CameraZoomLens>(player)
        .unwrap()
        .0
        .clone();
    assert_eq!(zoom.aperture_f_number, 8.0);
    assert_eq!(zoom.iso, 400.0);
    assert_eq!(
        world
            .borrow()
            .get_dynamic(ids["first_person"], "WeaponOptic")
            .unwrap()["scope_magnification"],
        3.0
    );
    assert!(!world
        .borrow_mut()
        .drain_events("camera.zoom_lens_changed")
        .is_empty());
    action("camera_reset", UiEventKind::Clicked);
    assert_eq!(
        world.borrow().get::<CameraZoomLens>(player).unwrap().0,
        CameraLens::default()
    );
    assert!(
        (lens().aperture_f_number - 2.8).abs() < 1e-5,
        "resetting zoom must preserve the standard lens"
    );
    assert_eq!(
        world
            .borrow()
            .get_dynamic(ids["first_person"], "WeaponOptic")
            .unwrap()["scope_magnification"],
        3.0,
        "resetting a camera lens must not change the equipped scope"
    );
    action("camera_lens_zoom", UiEventKind::BoolChanged(false));
    assert!(
        world
            .borrow()
            .get::<UiElement>(ids["camera_fov"])
            .unwrap()
            .visible
    );
}
