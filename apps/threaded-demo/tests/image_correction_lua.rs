//! Exercise image-correction UI events through the Lua and ECS bridges.
use countersplat_ecs::{World, WorldOperation, WorldOutcome};
use countersplat_scene::{ImageCorrection, SceneManifest};
use countersplat_scripting_lua::{LuaRuntime, ScriptOptions, WorldBridge};
use countersplat_ui::{UiElement, UiEvent, UiEventKind};
use std::{cell::RefCell, path::PathBuf, rc::Rc};

struct LocalBridge(Rc<RefCell<World>>);
impl WorldBridge for LocalBridge {
    fn request(&mut self, op: WorldOperation) -> Result<WorldOutcome, String> {
        self.0.borrow_mut().apply_operation(op)
    }
}

#[test]
fn ui_and_exports_change_the_scene_grade_and_reset_to_scene_defaults() {
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
    let entity = ids["image_correction"];
    let defaults = world
        .borrow()
        .get::<ImageCorrection>(entity)
        .unwrap()
        .clone();
    world.borrow_mut().set_dynamic_resource(
        "image.default_correction",
        serde_json::to_value(&defaults).unwrap(),
    );
    world
        .borrow_mut()
        .get_mut::<UiElement>(ids["image_panel"])
        .unwrap()
        .collapsible
        .as_mut()
        .unwrap()
        .collapsed = false;
    let lua = LuaRuntime::new(LocalBridge(world.clone()), ScriptOptions::default()).unwrap();
    lua.execute(
        &std::fs::read_to_string(root.join("assets/scripts/image_correction.lua")).unwrap(),
    )
    .unwrap();
    let action = |name: &str, kind: UiEventKind| {
        let accepted = countersplat_ui_ecs::apply_events(
            &mut world.borrow_mut(),
            [UiEvent {
                id: ids[name].to_string(),
                kind,
            }],
        );
        assert_eq!(accepted.len(), 1, "{name} should accept input");
        let events = world.borrow_mut().drain_events("ui.action");
        for event in events {
            lua.dispatch("ui.action", &event).unwrap();
        }
    };
    action("image_exposure", UiEventKind::NumberChanged(1.25));
    action("image_bloom", UiEventKind::NumberChanged(0.7));
    action("image_sharpness", UiEventKind::NumberChanged(1.25));
    action("image_enabled", UiEventKind::BoolChanged(false));
    let grade = world
        .borrow()
        .get::<ImageCorrection>(entity)
        .unwrap()
        .clone();
    assert_eq!(grade.exposure_ev, 1.25);
    assert_eq!(grade.bloom_intensity, 0.7);
    assert_eq!(grade.sharpness, 1.25);
    assert!(!grade.enabled);
    assert!(!world
        .borrow_mut()
        .drain_events("image.correction_changed")
        .is_empty());
    action("image_reset", UiEventKind::Clicked);
    assert_eq!(
        *world.borrow().get::<ImageCorrection>(entity).unwrap(),
        defaults
    );
}
