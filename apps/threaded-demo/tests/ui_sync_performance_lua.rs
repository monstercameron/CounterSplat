//! Verify idle UI bindings avoid cross-boundary reads without losing live updates.
use countersplat_ecs::{World, WorldOperation, WorldOutcome};
use countersplat_scene::{LevelSettings, SceneManifest, VoxelColliderState, VoxelStatus};
use countersplat_scripting_lua::{LuaRuntime, ScriptOptions, WorldBridge};
use countersplat_ui::{UiElement, Widget};
use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
    rc::Rc,
};

struct CountingBridge {
    world: Rc<RefCell<World>>,
    ui_reads: Rc<Cell<usize>>,
    level_snapshots: Rc<Cell<usize>>,
}
impl WorldBridge for CountingBridge {
    fn request(&mut self, op: WorldOperation) -> Result<WorldOutcome, String> {
        if matches!(&op, WorldOperation::ReadComponent { name, .. } if name == "UiElement") {
            self.ui_reads.set(self.ui_reads.get() + 1);
        }
        let level_read = matches!(&op,
            WorldOperation::ReadComponent { name, .. } |
            WorldOperation::ReadComponentIfChanged { name, .. } if name == "LevelSettings");
        let result = self.world.borrow_mut().apply_operation(op);
        if level_read && matches!(&result, Ok(WorldOutcome::Value(Some(_)))) {
            self.level_snapshots.set(self.level_snapshots.get() + 1);
        }
        result
    }
}

#[test]
fn idle_level_and_voxel_controls_only_read_ui_when_data_changes() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let scene =
        SceneManifest::from_json(&std::fs::read(root.join("assets/scenes/gameplay.json")).unwrap())
            .unwrap();
    let world = Rc::new(RefCell::new(World::new()));
    countersplat_ui_ecs::register(&mut world.borrow_mut()).unwrap();
    countersplat_scene_ecs::register(&mut world.borrow_mut()).unwrap();
    let ids = countersplat_scene_ecs::instantiate_layers(
        &mut world.borrow_mut(),
        &scene,
        &["ui", "player", "level"],
    )
    .unwrap();
    let ui_reads = Rc::new(Cell::new(0));
    let level_snapshots = Rc::new(Cell::new(0));
    let lua = LuaRuntime::new(
        CountingBridge {
            world: world.clone(),
            ui_reads: ui_reads.clone(),
            level_snapshots: level_snapshots.clone(),
        },
        ScriptOptions::default(),
    )
    .unwrap();
    for script in ["level_settings.lua", "voxel_settings.lua"] {
        lua.execute(&std::fs::read_to_string(root.join("assets/scripts").join(script)).unwrap())
            .unwrap();
    }

    ui_reads.set(0);
    level_snapshots.set(0);
    for _ in 0..120 {
        lua.run_systems(1.0 / 60.0).unwrap();
    }
    assert_eq!(
        ui_reads.get(),
        0,
        "idle control synchronization should not copy UI components"
    );
    assert_eq!(
        level_snapshots.get(),
        0,
        "unchanged settings should not be copied"
    );
    lua.dispatch(
        "ui.action",
        &serde_json::json!({
            "id": ids["controls_speed"].to_string(),
            "kind": {"type": "number_changed", "value": 5.0}
        }),
    )
    .unwrap();
    assert_eq!(
        level_snapshots.get(),
        0,
        "other panels must not read level settings"
    );

    {
        let mut w = world.borrow_mut();
        w.get_mut::<LevelSettings>(ids["active_level"])
            .unwrap()
            .draw_distance = 345.0;
        let state = w
            .get_mut::<VoxelColliderState>(ids["active_level"])
            .unwrap();
        state.status = VoxelStatus::Ready;
        state.resolution = Some(0.05);
    }
    lua.run_systems(1.0 / 60.0).unwrap();
    assert!(
        ui_reads.get() > 0,
        "external changes must refresh controls promptly"
    );
    assert_eq!(
        level_snapshots.get(),
        1,
        "one changed settings snapshot should be read"
    );
    let w = world.borrow();
    assert!(
        matches!(w.get::<UiElement>(ids["level_draw_distance"]).unwrap().widget,
        Widget::Slider { value, .. } if value == 345.0)
    );
    assert!(
        matches!(&w.get::<UiElement>(ids["voxel_status"]).unwrap().widget,
        Widget::Text { text, .. } if text == "Voxel size: 0.050")
    );
}

struct HierarchyCountingBridge {
    world: Rc<RefCell<World>>,
    reads: Rc<Cell<usize>>,
}
impl WorldBridge for HierarchyCountingBridge {
    fn request(&mut self, op: WorldOperation) -> Result<WorldOutcome, String> {
        if matches!(&op, WorldOperation::Hierarchy) {
            self.reads.set(self.reads.get() + 1);
        }
        self.world.borrow_mut().apply_operation(op)
    }
}

#[test]
fn inspector_reuses_tree_until_structure_or_component_membership_changes() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let scene =
        SceneManifest::from_json(&std::fs::read(root.join("assets/scenes/gameplay.json")).unwrap())
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
    let reads = Rc::new(Cell::new(0));
    let lua = LuaRuntime::new(
        HierarchyCountingBridge {
            world: world.clone(),
            reads: reads.clone(),
        },
        ScriptOptions::default(),
    )
    .unwrap();
    for script in ["inspector_tuning.lua", "entity_inspector.lua"] {
        lua.execute(&std::fs::read_to_string(root.join("assets/scripts").join(script)).unwrap())
            .unwrap();
    }
    reads.set(0);
    for _ in 0..120 {
        lua.run_systems(1.0 / 60.0).unwrap();
    }
    assert_eq!(reads.get(), 0, "unchanged trees should not be copied");

    let entity = world.borrow_mut().spawn();
    for _ in 0..20 {
        lua.run_systems(1.0 / 60.0).unwrap();
    }
    assert_eq!(reads.get(), 1, "spawning must refresh the tree");
    world
        .borrow_mut()
        .insert(entity, UiElement::default())
        .unwrap();
    for _ in 0..20 {
        lua.run_systems(1.0 / 60.0).unwrap();
    }
    assert_eq!(
        reads.get(),
        2,
        "component membership must refresh tree classification"
    );
    world
        .borrow_mut()
        .set_parent(entity, Some(ids["player"]))
        .unwrap();
    for _ in 0..20 {
        lua.run_systems(1.0 / 60.0).unwrap();
    }
    assert_eq!(reads.get(), 3, "reparenting must refresh the tree");
    world.borrow_mut().despawn(entity);
    for _ in 0..20 {
        lua.run_systems(1.0 / 60.0).unwrap();
    }
    assert_eq!(reads.get(), 4, "despawning must refresh the tree");
}
