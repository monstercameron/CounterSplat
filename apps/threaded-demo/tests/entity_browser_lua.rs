//! Exercise entity-list clicks through the real ECS, UI adapter, and Lua inspector.
use countersplat_ecs::{World, WorldOperation, WorldOutcome};
use countersplat_scene::{LevelSettings, PhysicsBody, SceneManifest, Skybox, VoxelCollider};
use countersplat_scripting_lua::{LuaRuntime, ScriptOptions, WorldBridge};
use countersplat_ui::{Anchor, UiElement, UiEvent, UiEventKind, Widget};
use serde_json::json;
use std::{cell::RefCell, path::PathBuf, rc::Rc};

struct LocalBridge(Rc<RefCell<World>>);
impl WorldBridge for LocalBridge {
    fn request(&mut self, op: WorldOperation) -> Result<WorldOutcome, String> {
        self.0.borrow_mut().apply_operation(op)
    }
}

#[test]
fn selecting_paging_and_removing_entities_updates_the_right_inspector() {
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
        &["ui", "player", "level", "scene"],
    )
    .unwrap();
    let lua = LuaRuntime::new(LocalBridge(world.clone()), ScriptOptions::default()).unwrap();
    for script in ["inspector_tuning.lua", "entity_inspector.lua"] {
        lua.execute(&std::fs::read_to_string(root.join("assets/scripts").join(script)).unwrap())
            .unwrap();
    }
    let action = |name: &str, kind: UiEventKind| {
        let events = countersplat_ui_ecs::apply_events(
            &mut world.borrow_mut(),
            [UiEvent {
                id: ids[name].to_string(),
                kind,
            }],
        );
        assert_eq!(events.len(), 1, "{name} should accept a click");
        let events = world.borrow_mut().drain_events("ui.action");
        for event in events {
            lua.dispatch("ui.action", &event).unwrap();
        }
    };
    let click = |name: &str| action(name, UiEventKind::Clicked);
    let label = |name: &str| {
        let world = world.borrow();
        match &world.get::<UiElement>(ids[name]).unwrap().widget {
            Widget::Button { label } => label.clone(),
            Widget::Text { text, .. } => text.clone(),
            _ => panic!("{name} must contain a label"),
        }
    };
    assert!(
        !world
            .borrow()
            .get::<UiElement>(ids["entity_inspector"])
            .unwrap()
            .visible
    );
    assert_eq!(label("ecs_row_1"), "Player");
    assert_eq!(label("ecs_row_2"), "Active level");
    click("ecs_row_2");
    {
        let world = world.borrow();
        let inspector = world.get::<UiElement>(ids["entity_inspector"]).unwrap();
        assert!(inspector.visible);
        assert_eq!(inspector.anchor, Anchor::TopRight);
        assert!(!inspector.collapsible.as_ref().unwrap().collapsed);
        assert_eq!(
            world.get_dynamic_resource("editor.selected_entity"),
            Some(&json!(ids["active_level"].to_string()))
        );
    }
    assert_eq!(label("inspector_entity"), "Active level");
    assert!(label("inspector_components_1").contains("LevelAssets"));
    action("inspector_export_1", UiEventKind::NumberChanged(250.0));
    action("inspector_export_2", UiEventKind::BoolChanged(false));
    assert_eq!(
        world
            .borrow()
            .get::<LevelSettings>(ids["active_level"])
            .unwrap()
            .draw_distance,
        250.0
    );
    assert!(
        !world
            .borrow()
            .get::<VoxelCollider>(ids["active_level"])
            .unwrap()
            .enabled
    );
    click("inspector_attach");
    assert!(world.borrow().has::<PhysicsBody>(ids["active_level"]));
    click("ecs_row_3");
    assert_eq!(label("inspector_entity"), "Skybox");
    action("inspector_export_1", UiEventKind::BoolChanged(false));
    action("inspector_export_2", UiEventKind::NumberChanged(2.0));
    assert!(!world.borrow().get::<Skybox>(ids["skybox"]).unwrap().enabled);
    assert_eq!(
        world
            .borrow()
            .get::<Skybox>(ids["skybox"])
            .unwrap()
            .exposure,
        2.0
    );
    click("inspector_close");
    assert!(
        !world
            .borrow()
            .get::<UiElement>(ids["entity_inspector"])
            .unwrap()
            .visible
    );
    click("ecs_row_1");
    assert!(
        world
            .borrow()
            .get::<UiElement>(ids["entity_inspector"])
            .unwrap()
            .visible
    );
    assert_eq!(label("inspector_entity"), "Player");
    click("ecs_page_next");
    assert!(label("ecs_page_label").starts_with("2 / "));
    click("ecs_page_prev");
    assert!(label("ecs_page_label").starts_with("1 / "));
    click("ecs_row_4");
    assert_eq!(label("inspector_entity"), "Animate");
    action("inspector_export_1", UiEventKind::BoolChanged(false));
    assert!(
        !world
            .borrow()
            .get::<UiElement>(ids["animate"])
            .unwrap()
            .visible
    );
    action("inspector_export_1", UiEventKind::BoolChanged(true));
    assert!(
        world
            .borrow()
            .get::<UiElement>(ids["animate"])
            .unwrap()
            .visible
    );

    let dynamic = world.borrow_mut().spawn();
    world
        .borrow_mut()
        .set_dynamic(dynamic, "Example", json!({"enabled":true}))
        .unwrap();
    lua.run_systems(0.25).unwrap();
    let dynamic_row = (1..=10)
        .find(|index| label(&format!("ecs_row_{index}")).starts_with("Entity "))
        .expect("a newly spawned entity appears in the browser");
    click(&format!("ecs_row_{dynamic_row}"));
    assert_eq!(
        world
            .borrow()
            .get_dynamic_resource("editor.selected_entity"),
        Some(&json!(dynamic.to_string()))
    );
    assert!(label("inspector_components_1").contains("Example"));
    assert!(world.borrow_mut().despawn(dynamic));
    lua.run_systems(0.25).unwrap();
    assert_eq!(label("inspector_entity"), "Player");
    assert_eq!(
        world
            .borrow()
            .get_dynamic_resource("editor.selected_entity"),
        Some(&json!(ids["player"].to_string()))
    );
}

#[test]
fn inspector_still_runs_in_scenes_without_the_new_entity_browser() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut manifest =
        SceneManifest::from_json(&std::fs::read(root.join("assets/scenes/gameplay.json")).unwrap())
            .unwrap();
    manifest.entities.retain(|entity| {
        !entity.id.starts_with("ecs_")
            && !entity.id.starts_with("inspector_components_")
            && entity.id != "inspector_close"
    });
    let world = Rc::new(RefCell::new(World::new()));
    countersplat_ui_ecs::register(&mut world.borrow_mut()).unwrap();
    countersplat_scene_ecs::register(&mut world.borrow_mut()).unwrap();
    let ids = countersplat_scene_ecs::instantiate_layers(
        &mut world.borrow_mut(),
        &manifest,
        &["ui", "player", "level", "scene"],
    )
    .unwrap();
    {
        let mut world = world.borrow_mut();
        let inspector = world.get_mut::<UiElement>(ids["entity_inspector"]).unwrap();
        inspector.visible = true;
        inspector.collapsible.as_mut().unwrap().collapsed = false;
    }
    let lua = LuaRuntime::new(LocalBridge(world.clone()), ScriptOptions::default()).unwrap();
    lua.execute(
        &std::fs::read_to_string(root.join("assets/scripts/entity_inspector.lua")).unwrap(),
    )
    .unwrap();
    lua.run_systems(0.25).unwrap();
    lua.dispatch(
        "ui.action",
        &json!({"id":ids["inspector_next"].to_string(),"kind":{"type":"clicked"}}),
    )
    .unwrap();
    assert_eq!(
        world
            .borrow()
            .get_dynamic_resource("editor.selected_entity"),
        Some(&json!(ids["active_level"].to_string()))
    );
}
