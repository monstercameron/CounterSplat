//! Load the actual scene JSON, its UI layer, and the declared Lua script through real adapters.
use countersplat_ecs::{World, WorldOperation, WorldOutcome};
use countersplat_scene::SceneManifest;
use countersplat_scripting_lua::{LuaRuntime, ScriptOptions, WorldBridge};
use std::{cell::RefCell, path::PathBuf, rc::Rc};

struct LocalBridge(Rc<RefCell<World>>);
impl WorldBridge for LocalBridge {
    fn request(&mut self, op: WorldOperation) -> Result<WorldOutcome, String> {
        self.0.borrow_mut().apply_operation(op)
    }
}

#[test]
fn project_manifest_instantiates_ui_and_drives_lua_on_the_scene_world() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let manifest =
        SceneManifest::from_json(&std::fs::read(root.join("assets/scenes/gameplay.json")).unwrap())
            .unwrap();
    let mut ui_world = World::new();
    countersplat_ui_ecs::register(&mut ui_world).unwrap();
    let ui = countersplat_scene_ecs::instantiate_layer(&mut ui_world, &manifest, "ui").unwrap();
    assert!(ui.len() > 20);
    assert!(ui_world.has::<countersplat_ui::FrameCounter>(ui["counter"]));
    let frame = countersplat_ui_ecs::extract(
        &ui_world,
        countersplat_ui::Rect::new(0.0, 0.0, 960.0, 540.0),
    )
    .unwrap();
    assert!(!frame.nodes.is_empty());

    let world = Rc::new(RefCell::new(World::new()));
    countersplat_scene_ecs::register(&mut world.borrow_mut()).unwrap();
    let ids =
        countersplat_scene_ecs::instantiate_layer(&mut world.borrow_mut(), &manifest, "scene")
            .unwrap();
    let script = &manifest.resources.scripts["rotate-sky"];
    assert_eq!(script.language, "lua");
    assert!(!script.autoload);
    let source = std::fs::read_to_string(root.join("assets/scenes").join(&script.uri)).unwrap();
    let lua = LuaRuntime::new(LocalBridge(world.clone()), ScriptOptions::default()).unwrap();
    lua.execute(&source).unwrap();
    lua.run_system("skybox.rotate", 0.5).unwrap();
    let world = world.borrow();
    assert_eq!(
        world
            .get::<countersplat_scene::Skybox>(ids["skybox"])
            .unwrap()
            .rotation_degrees,
        1.0
    );
}

#[test]
fn selector_manifest_instantiates_clickable_scene_controls() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let manifest = SceneManifest::from_json(
        &std::fs::read(root.join("assets/scenes/level-selector.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        manifest.resources.assets["catalog"].uri,
        "/assets/levels/catalog.json"
    );
    assert_eq!(
        manifest.resources.assets["gameplay_scene"].uri,
        "/assets/scenes/gameplay.json"
    );
    let mut world = World::new();
    countersplat_ui_ecs::register(&mut world).unwrap();
    countersplat_scene_ecs::register(&mut world).unwrap();
    let ids = countersplat_scene_ecs::instantiate_layer(&mut world, &manifest, "ui").unwrap();
    for id in ["previous", "next", "launch"] {
        let element = world.get::<countersplat_ui::UiElement>(ids[id]).unwrap();
        assert!(matches!(
            element.widget,
            countersplat_ui::Widget::Button { .. }
        ));
    }
    let loader = world
        .get::<countersplat_scene::AssetLoader>(ids["loader_overlay"])
        .unwrap();
    assert_eq!(loader.status, countersplat_scene::AssetLoadingStatus::Idle);
    assert!(
        !world
            .get::<countersplat_ui::UiElement>(ids["loader_overlay"])
            .unwrap()
            .visible
    );
    assert!(matches!(
        world
            .get::<countersplat_ui::UiElement>(ids["loader_progress"])
            .unwrap()
            .widget,
        countersplat_ui::Widget::Progress { .. }
    ));
    let frame =
        countersplat_ui_ecs::extract(&world, countersplat_ui::Rect::new(0.0, 0.0, 960.0, 540.0))
            .unwrap();
    assert!(!frame.nodes.is_empty());
}
