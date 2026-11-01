//! Exercise the real Lua zoom script with the scene's camera and ECS controls.
use countersplat_ecs::{Entity, World, WorldOperation, WorldOutcome};
use countersplat_scene::{CameraLens, CameraZoomLens, PlayerCamera, PlayerControls, SceneManifest};
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
fn magnification(camera: &PlayerCamera) -> f32 {
    (camera.fov_degrees.to_radians() / 2.0).tan()
        / (camera.render_fov_degrees().to_radians() / 2.0).tan()
}
#[test]
fn right_hold_smoothly_reaches_optical_five_x_and_restores_the_base_fov() {
    let f = Fixture::new(&["camera_zoom.lua"]);
    f.world
        .borrow_mut()
        .get_mut::<PlayerCamera>(f.ids["player"])
        .unwrap()
        .fov_degrees = 75.0;
    f.lua
        .dispatch(
            "input.mouse_button",
            &json!({"button":"right", "pressed":true, "gameplay":false}),
        )
        .unwrap();
    f.lua.run_systems(0.05).unwrap();
    assert_eq!(
        f.camera().view_fov_degrees,
        None,
        "UI clicks must not start zoom"
    );
    f.lua
        .dispatch(
            "input.mouse_button",
            &json!({"button":"left", "pressed":true, "gameplay":true}),
        )
        .unwrap();
    f.lua.run_systems(0.05).unwrap();
    assert_eq!(f.camera().view_fov_degrees, None);
    f.lua
        .dispatch(
            "input.mouse_button",
            &json!({"button":"right", "pressed":true, "gameplay":true}),
        )
        .unwrap();
    f.lua.run_systems(0.02).unwrap();
    let first = magnification(&f.camera());
    assert!(first > 1.0 && first < 5.0, "zoom must ease in: {first}");
    f.lua.run_systems(0.08).unwrap();
    let second = magnification(&f.camera());
    assert!(
        second > first && second < 5.0,
        "zoom must move smoothly toward its target"
    );
    for _ in 0..100 {
        f.lua.run_systems(1.0 / 60.0).unwrap();
    }
    let zoomed = f.camera();
    assert!((magnification(&zoomed) - 5.0).abs() < 0.001, "{zoomed:?}");
    assert_eq!(zoomed.fov_degrees, 75.0);
    assert_eq!(zoomed.position, [0.0; 3]);
    // Changing the base FOV while zoomed preserves the requested optical factor.
    f.world
        .borrow_mut()
        .get_mut::<PlayerCamera>(f.ids["player"])
        .unwrap()
        .fov_degrees = 90.0;
    f.lua.run_systems(1.0 / 60.0).unwrap();
    assert!((magnification(&f.camera()) - 5.0).abs() < 0.001);
    f.lua
        .dispatch(
            "input.mouse_button",
            &json!({"button":"right", "pressed":false, "gameplay":false}),
        )
        .unwrap();
    f.lua.run_systems(0.02).unwrap();
    let returning = magnification(&f.camera());
    assert!(
        returning > 1.0 && returning < 5.0,
        "zoom must ease out: {returning}"
    );
    for _ in 0..100 {
        f.lua.run_systems(1.0 / 60.0).unwrap();
    }
    assert_eq!(f.camera().view_fov_degrees, None);
    assert_eq!(f.camera().render_fov_degrees(), 90.0);
    // Focus loss or pause releases the held button, even when ticks are paused.
    f.lua
        .dispatch(
            "input.mouse_button",
            &json!({"button":"right", "pressed":true, "gameplay":true}),
        )
        .unwrap();
    f.lua.run_systems(0.05).unwrap();
    f.lua.dispatch("input.reset", &json!({})).unwrap();
    assert_eq!(f.camera().view_fov_degrees, None);
    f.lua.run_systems(0.1).unwrap();
    assert_eq!(f.camera().view_fov_degrees, None);
}
#[test]
fn zoom_transition_is_independent_of_tick_rate() {
    let slower = Fixture::new(&["camera_zoom.lua"]);
    let faster = Fixture::new(&["camera_zoom.lua"]);
    let press = json!({"button":"right", "pressed":true, "gameplay":true});
    slower.lua.dispatch("input.mouse_button", &press).unwrap();
    faster.lua.dispatch("input.mouse_button", &press).unwrap();
    for _ in 0..6 {
        slower.lua.run_systems(1.0 / 30.0).unwrap();
    }
    for _ in 0..24 {
        faster.lua.run_systems(1.0 / 120.0).unwrap();
    }
    assert!((magnification(&slower.camera()) - magnification(&faster.camera())).abs() < 0.0001);
}
#[test]
fn zoom_settings_are_exposed_in_the_player_inspector() {
    let f = Fixture::new(&[
        "player_controls.lua",
        "camera_bob.lua",
        "camera_zoom.lua",
        "entity_inspector.lua",
    ]);
    f.action("inspector_export_next", UiEventKind::Clicked);
    f.action("inspector_export_next", UiEventKind::Clicked);
    f.action("inspector_export_4", UiEventKind::NumberChanged(7.0));
    f.action("inspector_export_next", UiEventKind::Clicked);
    f.action("inspector_export_1", UiEventKind::NumberChanged(18.0));
    let controls = f
        .world
        .borrow()
        .get::<PlayerControls>(f.ids["player"])
        .unwrap()
        .clone();
    assert!((controls.zoom_factor - 7.0).abs() < 0.001);
    assert!((controls.zoom_response - 18.0).abs() < 0.001);
    assert_eq!(
        f.world
            .borrow_mut()
            .drain_events("player.controls_changed")
            .len(),
        2
    );
    f.lua.execute("local p=ecs.get_resource('scene.entities').player; assert(not pcall(function() engine.set_export(p,'zoom_factor',11) end))").unwrap();
}

#[test]
fn zoom_blends_the_rendered_lens_without_overwriting_either_profile() {
    let f = Fixture::new(&["camera_zoom.lua"]);
    {
        let mut world = f.world.borrow_mut();
        let standard = world.get_mut::<CameraLens>(f.ids["player"]).unwrap();
        standard.aperture_f_number = 4.0;
        standard.iso = 100.0;
        let zoom = &mut world.get_mut::<CameraZoomLens>(f.ids["player"]).unwrap().0;
        zoom.aperture_f_number = 8.0;
        zoom.iso = 400.0;
        zoom.depth_of_field = true;
    }
    f.lua.run_systems(1.0 / 60.0).unwrap();
    let render = || {
        f.world
            .borrow()
            .get_dynamic_resource("camera.render_lens")
            .unwrap()
            .clone()
    };
    assert_eq!(render()["aperture_f_number"], 4.0);
    f.lua
        .dispatch(
            "input.mouse_button",
            &json!({"button":"right","pressed":true,"gameplay":true}),
        )
        .unwrap();
    f.lua.run_systems(0.02).unwrap();
    let middle = render();
    assert!(middle["aperture_f_number"].as_f64().unwrap() > 4.0);
    assert!(middle["aperture_f_number"].as_f64().unwrap() < 8.0);
    for _ in 0..100 {
        f.lua.run_systems(1.0 / 60.0).unwrap();
    }
    assert_eq!(render()["aperture_f_number"], 8.0);
    assert_eq!(render()["iso"], 400.0);
    assert_eq!(render()["depth_of_field"], true);
    assert_eq!(
        f.world
            .borrow()
            .get::<CameraLens>(f.ids["player"])
            .unwrap()
            .aperture_f_number,
        4.0
    );
    assert_eq!(
        f.world
            .borrow()
            .get::<CameraZoomLens>(f.ids["player"])
            .unwrap()
            .0
            .aperture_f_number,
        8.0
    );
    f.lua.dispatch("input.reset", &json!({})).unwrap();
    assert_eq!(render()["aperture_f_number"], 4.0);
    assert_eq!(render()["depth_of_field"], false);
}

#[test]
fn lens_cache_invalidates_after_removal_and_reinsertion() {
    let f = Fixture::new(&["camera_zoom.lua"]);
    let player = f.ids["player"];
    f.lua.run_systems(1.0 / 60.0).unwrap();
    assert!(f
        .world
        .borrow()
        .get_dynamic_resource("camera.render_lens")
        .is_some());
    f.world
        .borrow_mut()
        .write_component_field(player, "CameraLens", "aperture_f_number", json!(6.0))
        .unwrap();
    f.lua.run_systems(1.0 / 60.0).unwrap();
    assert_eq!(
        f.world
            .borrow()
            .get_dynamic_resource("camera.render_lens")
            .unwrap()["aperture_f_number"],
        6.0
    );
    let original = f.world.borrow_mut().remove::<CameraLens>(player).unwrap();
    f.lua.run_systems(1.0 / 60.0).unwrap();
    assert!(f
        .world
        .borrow()
        .get_dynamic_resource("camera.render_lens")
        .is_none());
    let mut replacement = original;
    replacement.aperture_f_number = 11.0;
    f.world.borrow_mut().insert(player, replacement).unwrap();
    f.lua.run_systems(1.0 / 60.0).unwrap();
    assert_eq!(
        f.world
            .borrow()
            .get_dynamic_resource("camera.render_lens")
            .unwrap()["aperture_f_number"],
        11.0
    );
}

#[test]
fn cached_controls_follow_validated_edits_and_component_lifecycle() {
    let f = Fixture::new(&["camera_zoom.lua"]);
    let player = f.ids["player"];
    // Populate the script's reader, then change its data through normal ECS APIs.
    f.lua.run_systems(1.0 / 60.0).unwrap();
    f.lua
        .execute(
            r#"
        local p = ecs.get_resource("scene.entities").player
        local read = engine.component_reader(p, "PlayerControls")
        local current = read()
        local edit = ecs.get(p, "PlayerControls")
        edit.zoom_factor = -5
        assert(not pcall(function() ecs.set(p, "PlayerControls", edit) end))
        assert(read().zoom_factor == current.zoom_factor)
        ecs.set_field(p, "PlayerControls", "zoom_factor", 7)
        assert(read().zoom_factor == 7)
        local removed = ecs.remove(p, "PlayerControls")
        assert(read() == nil)
        removed.zoom_factor = 3
        ecs.set(p, "PlayerControls", removed)
        assert(read().zoom_factor == 3)
    "#,
        )
        .unwrap();
    f.lua
        .dispatch(
            "input.mouse_button",
            &json!({
                "button":"right", "pressed":true, "gameplay":true
            }),
        )
        .unwrap();
    for _ in 0..100 {
        f.lua.run_systems(1.0 / 60.0).unwrap();
    }
    assert!((magnification(&f.camera()) - 3.0).abs() < 0.001);
    // Native mutable access must invalidate the same reader as script edits.
    f.world
        .borrow_mut()
        .get_mut::<PlayerControls>(player)
        .unwrap()
        .zoom_factor = 6.0;
    for _ in 0..100 {
        f.lua.run_systems(1.0 / 60.0).unwrap();
    }
    assert!((magnification(&f.camera()) - 6.0).abs() < 0.001);
}
