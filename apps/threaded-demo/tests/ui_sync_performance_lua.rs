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
}
impl WorldBridge for CountingBridge {
    fn request(&mut self, op: WorldOperation) -> Result<WorldOutcome, String> {
        if matches!(&op, WorldOperation::ReadComponent { name, .. } if name == "UiElement") {
            self.ui_reads.set(self.ui_reads.get() + 1);
        }
        self.world.borrow_mut().apply_operation(op)
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
    let lua = LuaRuntime::new(
        CountingBridge {
            world: world.clone(),
            ui_reads: ui_reads.clone(),
        },
        ScriptOptions::default(),
    )
    .unwrap();
    for script in ["level_settings.lua", "voxel_settings.lua"] {
        lua.execute(&std::fs::read_to_string(root.join("assets/scripts").join(script)).unwrap())
            .unwrap();
    }

    ui_reads.set(0);
    for _ in 0..120 {
        lua.run_systems(1.0 / 60.0).unwrap();
    }
    assert_eq!(
        ui_reads.get(),
        0,
        "idle control synchronization should not copy UI components"
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
