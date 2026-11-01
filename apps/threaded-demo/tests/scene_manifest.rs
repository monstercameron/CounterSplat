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
fn gameplay_scene_authors_ownership_without_moving_ui_panels() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let manifest =
        SceneManifest::from_json(&std::fs::read(root.join("assets/scenes/gameplay.json")).unwrap())
            .unwrap();
    let mut world = World::new();
    countersplat_ui_ecs::register(&mut world).unwrap();
    countersplat_scene_ecs::register(&mut world).unwrap();
    let ids = countersplat_scene_ecs::instantiate_layers(
        &mut world,
        &manifest,
        &["ui", "player", "level", "scene"],
    )
    .unwrap();
    for (child, parent) in [
        ("camera_controller", "player"),
        ("first_person", "player"),
        ("skybox", "scene_visuals"),
        ("image_correction", "scene_visuals"),
        ("editor_ui", "game_ui"),
        ("hud_ui", "game_ui"),
        ("player_ui", "game_ui"),
        ("scene_ui", "game_ui"),
        ("ecs_panel", "editor_ui"),
        ("entity_inspector", "editor_ui"),
        ("stats_panel", "editor_ui"),
        ("camera_panel", "player_ui"),
        ("controls_panel", "player_ui"),
        ("level_panel", "scene_ui"),
        ("sky_panel", "scene_ui"),
        ("image_panel", "scene_ui"),
        ("scene_panel", "scene_ui"),
        ("weapon_hud", "hud_ui"),
        ("weapon_impact_notice", "hud_ui"),
        ("back_to_levels", "hud_ui"),
        ("ecs_list", "ecs_panel"),
        ("camera_fov", "camera_panel"),
    ] {
        assert_eq!(
            world.parent(ids[child]),
            Some(ids[parent]),
            "{child} must belong to {parent}"
        );
    }
    for root_id in ["player", "active_level", "scene_visuals", "game_ui"] {
        assert_eq!(world.parent(ids[root_id]), None);
    }

    // Non-UI grouping entities organize ownership but do not create a new
    // coordinate space or change panel visibility.
    let viewport = countersplat_ui::Rect::new(0.0, 0.0, 1800.0, 1200.0);
    let frame = countersplat_ui_ecs::extract(&world, viewport).unwrap();
    for id in [
        "scene_panel",
        "stats_panel",
        "sky_panel",
        "controls_panel",
        "level_panel",
        "ecs_panel",
        "camera_panel",
        "image_panel",
        "back_to_levels",
        "weapon_hud",
        "weapon_impact_notice",
    ] {
        let element = world.get::<countersplat_ui::UiElement>(ids[id]).unwrap();
        if !element.visible {
            continue;
        }
        let mut rect = element.rect;
        if let Some(panel) = &element.collapsible {
            if panel.collapsed {
                rect.height = panel.header_height;
            }
        }
        let mut expected = countersplat_ui::place(viewport, rect, element.anchor);
        expected.x += element.drag_offset[0];
        expected.y += element.drag_offset[1];
        let node = frame
            .nodes
            .iter()
            .find(|node| node.id == ids[id].to_string())
            .expect(id);
        assert_eq!(
            node.rect, expected,
            "{id} should retain its viewport placement"
        );
    }
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
