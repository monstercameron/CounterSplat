use countersplat_ecs::{World, WorldOperation, WorldOutcome};
use countersplat_scene::{SceneManifest, VoxelCollider, VoxelColliderState, VoxelStatus};
use countersplat_scripting_lua::{LuaRuntime, ScriptOptions, WorldBridge};
use countersplat_ui::{UiElement, Widget};
use serde_json::json;
use std::{cell::RefCell, path::PathBuf, rc::Rc};

struct LocalBridge(Rc<RefCell<World>>);
impl WorldBridge for LocalBridge {
    fn request(&mut self, op: WorldOperation) -> Result<WorldOutcome, String> {
        self.0.borrow_mut().apply_operation(op)
    }
}
#[test]
fn lua_voxel_controls_are_independent_and_follow_availability() {
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
    let entity = ids["active_level"];
    let lua = LuaRuntime::new(LocalBridge(world.clone()), ScriptOptions::default()).unwrap();
    lua.execute(&std::fs::read_to_string(root.join("assets/scripts/voxel_settings.lua")).unwrap())
        .unwrap();
    let click = |id: &str, value: bool| {
        lua.dispatch(
            "ui.action",
            &json!({"id":ids[id].to_string(),"kind":{"type":"bool_changed","value":value}}),
        )
        .unwrap()
    };
    click("voxel_visible", true);
    click("voxel_physics", false);
    click("voxel_enabled", false);
    let settings = world.borrow().get::<VoxelCollider>(entity).unwrap().clone();
    assert!(!settings.enabled && settings.visible && !settings.physics);
    assert!(
        !world
            .borrow()
            .get::<UiElement>(ids["voxel_visible"])
            .unwrap()
            .enabled
    );
    click("voxel_enabled", true);
    world
        .borrow_mut()
        .insert(
            entity,
            VoxelColliderState {
                status: VoxelStatus::Ready,
                resolution: Some(0.05),
                ..Default::default()
            },
        )
        .unwrap();
    lua.run_systems(0.016).unwrap();
    let element = world
        .borrow()
        .get::<UiElement>(ids["voxel_visible"])
        .unwrap()
        .clone();
    assert!(element.enabled && matches!(element.widget, Widget::Checkbox { checked: true, .. }));
    world
        .borrow_mut()
        .insert(
            entity,
            VoxelColliderState {
                status: VoxelStatus::Unavailable,
                ..Default::default()
            },
        )
        .unwrap();
    lua.run_systems(0.016).unwrap();
    assert!(
        !world
            .borrow()
            .get::<UiElement>(ids["voxel_enabled"])
            .unwrap()
            .enabled
    );
}
