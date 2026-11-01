//! Exercise the real player Lua system against validated ECS camera and UI components.
use countersplat_ecs::{World, WorldOperation, WorldOutcome};
use countersplat_scene::{PlayerCamera, PlayerControls, SceneManifest};
use countersplat_scripting_lua::{LuaRuntime, ScriptOptions, WorldBridge};
use countersplat_ui::{UiEvent, UiEventKind};
use serde_json::json;
use std::{cell::RefCell, path::PathBuf, rc::Rc};

struct LocalBridge(Rc<RefCell<World>>);
impl WorldBridge for LocalBridge {
    fn request(&mut self, op: WorldOperation) -> Result<WorldOutcome, String> {
        self.0.borrow_mut().apply_operation(op)
    }
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

    lua.dispatch("input.mouse_delta", &json!({"dx":100.0,"dy":50.0}))
        .unwrap();
    let camera = world.borrow().get::<PlayerCamera>(player).unwrap().clone();
    assert!((camera.yaw - 0.3).abs() < 0.001 && (camera.pitch + 0.15).abs() < 0.001);
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
