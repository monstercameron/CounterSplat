//! Developer overlay visibility is managed by Lua and independent of the normal HUD.
use countersplat_ecs::{World, WorldOperation, WorldOutcome};
use countersplat_scene::SceneManifest;
use countersplat_scripting_lua::{LuaRuntime, ScriptOptions, WorldBridge};
use countersplat_ui::UiElement;
use serde_json::json;
use std::{cell::RefCell, path::PathBuf, rc::Rc};
struct Bridge(Rc<RefCell<World>>);
impl WorldBridge for Bridge {
    fn request(&mut self, op: WorldOperation) -> Result<WorldOutcome, String> {
        self.0.borrow_mut().apply_operation(op)
    }
}
#[test]
fn tools_toggle_independently_of_the_normal_performance_preference() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let scene =
        SceneManifest::from_json(&std::fs::read(root.join("assets/scenes/gameplay.json")).unwrap())
            .unwrap();
    let world = Rc::new(RefCell::new(World::new()));
    countersplat_ui_ecs::register(&mut world.borrow_mut()).unwrap();
    let ids =
        countersplat_scene_ecs::instantiate_layer(&mut world.borrow_mut(), &scene, "ui").unwrap();
    let lua = LuaRuntime::new(Bridge(world.clone()), ScriptOptions::default()).unwrap();
    lua.execute(&std::fs::read_to_string(root.join("assets/scripts/editor_overlay.lua")).unwrap())
        .unwrap();
    let visible = |name: &str| world.borrow().get::<UiElement>(ids[name]).unwrap().visible;
    let key = |gameplay: bool, repeat: bool| {
        lua.dispatch(
            "input.key",
            &json!({"code":"Backquote","pressed":true,"is_repeat":repeat,"gameplay":gameplay}),
        )
        .unwrap()
    };
    assert!(visible("stats_panel"));
    assert!(!visible("ecs_panel") && !visible("camera_panel") && !visible("scene_panel"));
    key(false, false);
    key(true, true);
    assert!(!visible("ecs_panel"));
    key(true, false);
    assert!(visible("ecs_panel") && visible("stats_panel"));
    world
        .borrow_mut()
        .get_mut::<UiElement>(ids["entity_inspector"])
        .unwrap()
        .visible = true;
    key(true, false);
    assert!(!visible("entity_inspector") && visible("stats_panel"));
    lua.dispatch(
        "hud.preferences_changed",
        &json!({"show_performance":false}),
    )
    .unwrap();
    assert!(!visible("stats_panel"));
    key(true, false);
    assert!(visible("entity_inspector") && visible("stats_panel"));
    key(true, false);
    assert!(!visible("stats_panel") && !visible("ecs_panel"));
    world
        .borrow_mut()
        .set_dynamic_resource("input.binding_capture", json!(true));
    key(true, false);
    assert!(!visible("ecs_panel"));
}
