//! Real scene manifests and Lua scripts communicate exclusively through ECS events.
use countersplat_ecs::{Entity, World, WorldOperation, WorldOutcome};
use countersplat_scene::SceneManifest;
use countersplat_scripting_lua::{LuaRuntime, ScriptOptions, WorldBridge};
use countersplat_ui::{UiEvent, UiEventKind};
use serde_json::{json, Value};
use std::{cell::RefCell, collections::BTreeMap, path::PathBuf, rc::Rc};

struct LocalBridge(Rc<RefCell<World>>);
impl WorldBridge for LocalBridge {
    fn request(&mut self, op: WorldOperation) -> Result<WorldOutcome, String> {
        self.0.borrow_mut().apply_operation(op)
    }
}
fn scene(name: &str, script: &str) -> (Rc<RefCell<World>>, LuaRuntime, BTreeMap<String, Entity>) {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let path = root.join("assets/scenes").join(name);
    let manifest = SceneManifest::from_json(&std::fs::read(&path).unwrap()).unwrap();
    let world = Rc::new(RefCell::new(World::new()));
    countersplat_ui_ecs::register(&mut world.borrow_mut()).unwrap();
    let ids = countersplat_scene_ecs::instantiate_layer(&mut world.borrow_mut(), &manifest, "ui")
        .unwrap();
    let definition = &manifest.resources.scripts[script];
    let source = std::fs::read_to_string(path.parent().unwrap().join(&definition.uri)).unwrap();
    let lua = LuaRuntime::new(LocalBridge(world.clone()), ScriptOptions::default()).unwrap();
    lua.execute(&source).unwrap();
    (world, lua, ids)
}
fn key(world: &Rc<RefCell<World>>, pressed: bool, repeat: bool) {
    world.borrow_mut().emit_event(
        "input.key",
        json!({"key":"Escape", "pressed":pressed, "is_repeat":repeat}),
    );
}
fn click(world: &Rc<RefCell<World>>, id: Entity) {
    let event = UiEvent {
        id: id.to_string(),
        kind: UiEventKind::Clicked,
    };
    assert_eq!(
        countersplat_ui_ecs::apply_events(&mut world.borrow_mut(), [event]).len(),
        1
    );
}
fn commands(world: &Rc<RefCell<World>>, lua: &LuaRuntime) -> Vec<Value> {
    lua.run_systems(1.0 / 60.0).unwrap();
    world.borrow_mut().drain_events("engine.scene")
}
fn gameplay_commands(world: &Rc<RefCell<World>>, lua: &LuaRuntime) -> Vec<Value> {
    for channel in ["input.key", "ui.action"] {
        let events = world.borrow_mut().drain_events(channel);
        for event in events {
            lua.dispatch(channel, &event).unwrap();
        }
    }
    commands(world, lua)
}
#[test]
fn gameplay_escape_and_levels_button_request_scene_changes() {
    let (world, lua, ids) = scene("gameplay.json", "gameplay-navigation");
    key(&world, false, false);
    key(&world, true, true);
    assert!(gameplay_commands(&world, &lua).is_empty());
    key(&world, true, false);
    assert_eq!(
        gameplay_commands(&world, &lua),
        vec![json!({"action":"push", "scene":"pause_menu"})]
    );
    assert!(gameplay_commands(&world, &lua).is_empty());
    click(&world, ids["back_to_levels"]);
    assert_eq!(
        gameplay_commands(&world, &lua),
        vec![json!({"action":"replace", "scene":"level_selector"})]
    );
}
#[test]
fn menu_escape_resume_and_choose_level_use_scene_api() {
    let (world, lua, ids) = scene("menu.json", "menu-navigation");
    key(&world, true, false);
    assert_eq!(commands(&world, &lua), vec![json!({"action":"pop"})]);
    click(&world, ids["resume"]);
    assert_eq!(commands(&world, &lua), vec![json!({"action":"pop"})]);
    click(&world, ids["levels"]);
    assert_eq!(
        commands(&world, &lua),
        vec![json!({"action":"replace", "scene":"level_selector"})]
    );
}
