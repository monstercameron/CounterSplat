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
fn selecting_scrolling_and_removing_entities_updates_the_right_inspector() {
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
    let action_by_id = |id: countersplat_ecs::Entity, kind: UiEventKind| {
        let events = countersplat_ui_ecs::apply_events(
            &mut world.borrow_mut(),
            [UiEvent {
                id: id.to_string(),
                kind,
            }],
        );
        assert_eq!(events.len(), 1, "{id} should accept a click");
        let events = world.borrow_mut().drain_events("ui.action");
        for event in events {
            lua.dispatch("ui.action", &event).unwrap();
        }
    };
    let action = |name: &str, kind: UiEventKind| action_by_id(ids[name], kind);
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
    assert_eq!(label("ecs_row_2"), "Camera controller");
    assert_eq!(label("ecs_row_3"), "First person");
    assert_eq!(label("ecs_row_4"), "Active level");
    click("ecs_row_4");
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
    action("ecs_list", UiEventKind::ScrollChanged(52.0));
    let image_row = (1..=12)
        .find(|index| label(&format!("ecs_row_{index}")) == "Image correction")
        .expect("image correction is visible near the top");
    click(&format!("ecs_row_{image_row}"));
    assert_eq!(label("inspector_entity"), "Image correction");
    assert!(label("inspector_components_1").contains("ImageCorrection"));
    let sky_row = (1..=12)
        .find(|index| label(&format!("ecs_row_{index}")) == "Skybox")
        .expect("skybox is visible near the top");
    click(&format!("ecs_row_{sky_row}"));
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
    action("ecs_list", UiEventKind::ScrollChanged(0.0));
    click("ecs_row_1");
    assert!(
        world
            .borrow()
            .get::<UiElement>(ids["entity_inspector"])
            .unwrap()
            .visible
    );
    assert_eq!(label("inspector_entity"), "Player");
    // Expand the authored editor group so the tree is long enough to scroll.
    let editor_row = (1..=12)
        .find(|index| label(&format!("ecs_row_{index}")) == "Editor UI")
        .unwrap();
    let toggle_for_row = |index: usize| {
        let world = world.borrow();
        let row = world
            .get::<UiElement>(ids[&format!("ecs_row_{index}")])
            .unwrap();
        world.children(ids["ecs_list"]).into_iter().find(|id| {
            world.get::<UiElement>(*id).is_some_and(|ui| ui.visible && ui.z == 103 && ui.rect.y == row.rect.y
                && matches!(&ui.widget, Widget::Button { label } if label == "+" || label == "-"))
        }).expect("tree branch has a toggle")
    };
    action_by_id(toggle_for_row(editor_row), UiEventKind::Clicked);
    // A partial-row scroll recycles the list without changing selection.
    action("ecs_list", UiEventKind::ScrollChanged(131.0));
    assert_eq!(label("ecs_row_1"), "Game UI");
    assert_eq!(label("inspector_entity"), "Player");
    {
        let world = world.borrow();
        let row = world.get::<UiElement>(ids["ecs_row_1"]).unwrap();
        assert_eq!(row.rect.y, 124.0);
        let frame = countersplat_ui_ecs::extract(
            &world,
            countersplat_ui::Rect::new(0.0, 0.0, 2000.0, 1200.0),
        )
        .unwrap();
        let node = frame
            .nodes
            .iter()
            .find(|node| node.id == ids["ecs_row_1"].to_string())
            .unwrap();
        assert_eq!(node.clip.height, 20.0, "partially visible rows are clipped");
    }
    click("ecs_row_2");
    assert_eq!(label("inspector_entity"), "Editor UI");
    action("ecs_list", UiEventKind::ScrollChanged(0.0));
    assert_eq!(label("ecs_row_1"), "Player");
    let max_offset = {
        let world = world.borrow();
        let element = world.get::<UiElement>(ids["ecs_list"]).unwrap();
        let Widget::ScrollArea { content_height, .. } = element.widget else {
            panic!("scroll area")
        };
        content_height - element.rect.height
    };
    assert!(max_offset > 0.0);
    action("ecs_list", UiEventKind::ScrollChanged(max_offset));
    let last_row = {
        let world = world.borrow();
        let frame = countersplat_ui_ecs::extract(
            &world,
            countersplat_ui::Rect::new(0.0, 0.0, 2000.0, 1200.0),
        )
        .unwrap();
        (1..=12)
            .rev()
            .find(|i| {
                frame
                    .nodes
                    .iter()
                    .any(|node| node.id == ids[&format!("ecs_row_{i}")].to_string())
            })
            .unwrap()
    };
    let last_label = label(&format!("ecs_row_{last_row}"));
    click(&format!("ecs_row_{last_row}"));
    assert_eq!(
        label("inspector_entity"),
        last_label,
        "entities at the end remain selectable"
    );
    action("ecs_list", UiEventKind::ScrollChanged(0.0));
    let player_ui_row = (1..=12)
        .find(|index| label(&format!("ecs_row_{index}")) == "Player UI")
        .expect("the player UI group is visible");
    action_by_id(toggle_for_row(player_ui_row), UiEventKind::Clicked);
    action("ecs_list", UiEventKind::ScrollChanged(62.0));
    let camera_panel_row = (1..=12)
        .find(|index| label(&format!("ecs_row_{index}")) == "Camera panel")
        .expect("the player UI group reveals its camera panel");
    click(&format!("ecs_row_{camera_panel_row}"));
    assert_eq!(label("inspector_entity"), "Camera panel");
    action("inspector_export_1", UiEventKind::BoolChanged(false));
    assert!(
        !world
            .borrow()
            .get::<UiElement>(ids["camera_panel"])
            .unwrap()
            .visible
    );
    action("inspector_export_1", UiEventKind::BoolChanged(true));
    assert!(
        world
            .borrow()
            .get::<UiElement>(ids["camera_panel"])
            .unwrap()
            .visible
    );

    action("ecs_list", UiEventKind::ScrollChanged(0.0));
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
        Some(&json!(ids["camera_controller"].to_string()))
    );
}

#[test]
fn tree_edits_real_ownership_and_resize_recycles_enough_rows() {
    use countersplat_ecs::Entity;
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
    let (parent, child, grandchild, other) = {
        let mut w = world.borrow_mut();
        let parent = w.spawn();
        let child = w.spawn();
        let grandchild = w.spawn();
        let other = w.spawn();
        w.set_parent(child, Some(parent)).unwrap();
        w.set_parent(grandchild, Some(child)).unwrap();
        (parent, child, grandchild, other)
    };
    let lua = LuaRuntime::new(LocalBridge(world.clone()), ScriptOptions::default()).unwrap();
    lua.execute(
        &std::fs::read_to_string(root.join("assets/scripts/entity_inspector.lua")).unwrap(),
    )
    .unwrap();

    let send = |id: Entity, kind: UiEventKind| {
        let accepted = countersplat_ui_ecs::apply_events(
            &mut world.borrow_mut(),
            [UiEvent {
                id: id.to_string(),
                kind,
            }],
        );
        assert_eq!(accepted.len(), 1, "event for {id} must be accepted");
        let events = world.borrow_mut().drain_events("ui.action");
        for event in events {
            lua.dispatch("ui.action", &event).unwrap();
        }
    };
    let row = |entity: Entity| -> Option<Entity> {
        let w = world.borrow();
        let expected = format!("Entity {entity}");
        w.children(ids["ecs_list"]).into_iter().find(|id| {
            w.get::<UiElement>(*id).is_some_and(|ui| {
                ui.visible && matches!(&ui.widget, Widget::Button { label } if label == &expected)
            })
        })
    };
    let toggle =
        |entity: Entity| -> Entity {
            let id = row(entity).expect("entity row is visible");
            let w = world.borrow();
            let y = w.get::<UiElement>(id).unwrap().rect.y;
            w.children(ids["ecs_list"]).into_iter().find(|id| {
            w.get::<UiElement>(*id).is_some_and(|ui| ui.visible && ui.z == 103 && ui.rect.y == y
                && matches!(&ui.widget, Widget::Button { label } if label == "+" || label == "-"))
        }).expect("expand control is visible")
        };

    assert!(row(parent).is_some());
    assert!(row(child).is_none(), "children start collapsed");
    send(toggle(parent), UiEventKind::Clicked);
    assert!(row(child).is_some());
    assert!(row(grandchild).is_none());
    let parent_x = world
        .borrow()
        .get::<UiElement>(row(parent).unwrap())
        .unwrap()
        .rect
        .x;
    let child_x = world
        .borrow()
        .get::<UiElement>(row(child).unwrap())
        .unwrap()
        .rect
        .x;
    assert!(child_x > parent_x, "children are indented");
    send(toggle(child), UiEventKind::Clicked);
    assert!(row(grandchild).is_some());
    send(row(grandchild).unwrap(), UiEventKind::Clicked);
    assert_eq!(
        world
            .borrow()
            .get_dynamic_resource("editor.selected_entity"),
        Some(&json!(grandchild.to_string()))
    );
    send(toggle(parent), UiEventKind::Clicked);
    assert!(row(child).is_none() && row(grandchild).is_none());
    assert_eq!(
        world
            .borrow()
            .get_dynamic_resource("editor.selected_entity"),
        Some(&json!(grandchild.to_string())),
        "collapsing does not clear selection"
    );
    send(toggle(parent), UiEventKind::Clicked);

    send(row(child).unwrap(), UiEventKind::Clicked);
    send(ids["ecs_parent"], UiEventKind::Clicked);
    send(row(other).unwrap(), UiEventKind::Clicked);
    assert_eq!(world.borrow().parent(child), Some(other));
    assert!(
        row(child).is_some(),
        "new parent is expanded to reveal the child"
    );

    send(row(other).unwrap(), UiEventKind::Clicked);
    send(ids["ecs_parent"], UiEventKind::Clicked);
    send(row(child).unwrap(), UiEventKind::Clicked);
    assert_eq!(
        world.borrow().parent(other),
        None,
        "a cycle must be rejected"
    );
    let notice = match &world
        .borrow()
        .get::<UiElement>(ids["ecs_count"])
        .unwrap()
        .widget
    {
        Widget::Text { text, .. } => text.clone(),
        _ => unreachable!(),
    };
    assert!(notice.contains("cycle"));
    send(ids["ecs_parent"], UiEventKind::Clicked); // Cancel the failed parent choice.
    send(row(child).unwrap(), UiEventKind::Clicked);
    send(ids["ecs_root"], UiEventKind::Clicked);
    assert_eq!(world.borrow().parent(child), None);

    send(ids["ecs_panel"], UiEventKind::ResizedTo([500.0, 700.0]));
    {
        let w = world.borrow();
        let area = w.get::<UiElement>(ids["ecs_list"]).unwrap();
        assert_eq!((area.rect.width, area.rect.height), (476.0, 590.0));
        let row_count = w
            .children(ids["ecs_list"])
            .iter()
            .filter(|id| w.get::<UiElement>(**id).is_some_and(|ui| ui.z == 102))
            .count();
        assert!(
            row_count >= 21,
            "large viewport needs enough recycled rows: {row_count}"
        );
    }
    let invalid = UiEvent {
        id: ids["ecs_panel"].to_string(),
        kind: UiEventKind::ResizedTo([f32::NAN, 400.0]),
    };
    assert!(countersplat_ui_ecs::apply_events(&mut world.borrow_mut(), [invalid]).is_empty());
    send(ids["ecs_panel"], UiEventKind::ResizedTo([250.0, 200.0]));
    let w = world.borrow();
    let area = w.get::<UiElement>(ids["ecs_list"]).unwrap();
    assert_eq!((area.rect.width, area.rect.height), (226.0, 90.0));
    assert!(
        area.validate().is_ok(),
        "shrinking preserves a valid scroll offset"
    );
}

#[test]
fn hidden_entity_changes_update_the_count_without_rewriting_visible_rows() {
    use std::cell::Cell;
    struct CountingBridge {
        world: Rc<RefCell<World>>,
        writes: Rc<Cell<usize>>,
    }
    impl WorldBridge for CountingBridge {
        fn request(&mut self, op: WorldOperation) -> Result<WorldOutcome, String> {
            if matches!(&op, WorldOperation::WriteComponent { name, .. } if name == "UiElement") {
                self.writes.set(self.writes.get() + 1);
            }
            self.world.borrow_mut().apply_operation(op)
        }
    }

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
    let writes = Rc::new(Cell::new(0));
    let lua = LuaRuntime::new(
        CountingBridge {
            world: world.clone(),
            writes: writes.clone(),
        },
        ScriptOptions::default(),
    )
    .unwrap();
    lua.execute(
        &std::fs::read_to_string(root.join("assets/scripts/entity_inspector.lua")).unwrap(),
    )
    .unwrap();
    let count = |world: &World| match &world.get::<UiElement>(ids["ecs_count"]).unwrap().widget {
        Widget::Text { text, .. } => text.clone(),
        _ => unreachable!(),
    };
    let before = count(&world.borrow());
    writes.set(0);
    {
        let mut w = world.borrow_mut();
        let hidden = w.spawn();
        w.set_parent(hidden, Some(ids["hud_ui"])).unwrap();
    }
    lua.run_systems(0.25).unwrap();
    let after = count(&world.borrow());
    assert_ne!(
        after, before,
        "hidden entities still contribute to the count"
    );
    assert_eq!(writes.get(), 1, "only the count label needs an ECS write");
}
