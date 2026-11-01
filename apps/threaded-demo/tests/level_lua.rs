//! Exercise the real level settings script against validated ECS components and UI events.
use countersplat_ecs::{World, WorldOperation, WorldOutcome};
use countersplat_scene::{LevelAssets, LevelFormat, LevelSettings, SceneManifest};
use countersplat_scripting_lua::{LuaRuntime, ScriptOptions, WorldBridge};
use countersplat_ui::{UiElement, UiEvent, UiEventKind, Widget};
use serde_json::json;
use std::{cell::RefCell, path::PathBuf, rc::Rc};

struct LocalBridge(Rc<RefCell<World>>);
impl WorldBridge for LocalBridge {
    fn request(&mut self, op: WorldOperation) -> Result<WorldOutcome, String> {
        self.0.borrow_mut().apply_operation(op)
    }
}

#[test]
fn lua_level_panel_controls_manual_and_automatic_lod() {
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
    let level = ids["active_level"];
    world
        .borrow_mut()
        .insert(
            level,
            LevelAssets {
                level_id: "1234abcd".into(),
                name: "Example level".into(),
                format: Some(LevelFormat::StreamedSog),
                lod_count: 3,
                ..Default::default()
            },
        )
        .unwrap();
    let lua = LuaRuntime::new(LocalBridge(world.clone()), ScriptOptions::default()).unwrap();
    lua.execute(&std::fs::read_to_string(root.join("assets/scripts/level_settings.lua")).unwrap())
        .unwrap();
    let event = UiEvent {
        id: ids["level_lod"].to_string(),
        kind: UiEventKind::NumberChanged(1.7),
    };
    assert_eq!(
        countersplat_ui_ecs::apply_events(&mut world.borrow_mut(), [event]).len(),
        1
    );
    let actions = world.borrow_mut().drain_events("ui.action");
    for action in actions {
        lua.dispatch("ui.action", &action).unwrap();
    }
    assert_eq!(
        world.borrow().get::<LevelSettings>(level).unwrap().lod,
        Some(2)
    );
    lua.dispatch(
        "ui.action",
        &json!({"id":ids["level_draw_distance"].to_string(),"kind":{"type":"number_changed","value":80.0}}),
    ).unwrap();
    {
        let world = world.borrow();
        let settings = world.get::<LevelSettings>(level).unwrap();
        assert_eq!(settings.draw_distance, 80.0);
        assert_eq!(settings.lod, Some(2));
    }
    let checkbox = world
        .borrow()
        .get::<UiElement>(ids["level_auto"])
        .unwrap()
        .clone();
    assert!(matches!(
        checkbox.widget,
        Widget::Checkbox { checked: false, .. }
    ));
    lua.dispatch(
        "ui.action",
        &json!({"id":ids["level_auto"].to_string(),"kind":{"type":"bool_changed","value":true}}),
    )
    .unwrap();
    assert_eq!(
        world.borrow().get::<LevelSettings>(level).unwrap().lod,
        None
    );
    lua.dispatch(
        "ui.action",
        &json!({"id":ids["level_auto"].to_string(),"kind":{"type":"bool_changed","value":false}}),
    )
    .unwrap();
    assert_eq!(
        world.borrow().get::<LevelSettings>(level).unwrap().lod,
        Some(2)
    );
    world
        .borrow_mut()
        .write_component_value(level, "LevelSettings", json!({"lod":1}))
        .unwrap();
    lua.run_systems(0.016).unwrap();
    let slider = world
        .borrow()
        .get::<UiElement>(ids["level_lod"])
        .unwrap()
        .clone();
    assert!(matches!(
        slider.widget,
        Widget::Slider {
            value: 1.0,
            max: 2.0,
            ..
        }
    ));
}
