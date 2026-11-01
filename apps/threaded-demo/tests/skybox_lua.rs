//! Exercise validated skybox components through the real Lua ECS bridge.
use countersplat_ecs::{World, WorldOperation, WorldOutcome};
use countersplat_scripting_lua::{LuaRuntime, ScriptOptions, WorldBridge};
use std::{cell::RefCell, rc::Rc};

struct LocalBridge(Rc<RefCell<World>>);
impl WorldBridge for LocalBridge {
    fn request(&mut self, op: WorldOperation) -> Result<WorldOutcome, String> {
        self.0.borrow_mut().apply_operation(op)
    }
}

#[test]
fn lua_creates_updates_and_validates_a_scene_skybox() {
    let world = Rc::new(RefCell::new(World::new()));
    countersplat_scene_ecs::register(&mut world.borrow_mut()).unwrap();
    let lua = LuaRuntime::new(LocalBridge(world.clone()), ScriptOptions::default()).unwrap();
    lua.execute(
        r#"
        sky = ecs.spawn()
        ecs.set(sky, "Skybox", { asset = "sunset", exposure = 1.5, rotation_degrees = 45 })
        local value = ecs.get(sky, "Skybox")
        assert(value.asset == "sunset" and value.exposure == 1.5 and value.enabled)
        value.rotation_degrees = 90
        ecs.set(sky, "Skybox", value)
        value.exposure = -1
        assert(not pcall(function() ecs.set(sky, "Skybox", value) end))
        assert(ecs.get(sky, "Skybox").exposure == 1.5)
    "#,
    )
    .unwrap();
    let snapshot = countersplat_scene_ecs::extract(&world.borrow()).unwrap();
    assert_eq!(snapshot.skybox.asset, "sunset");
    assert_eq!(snapshot.skybox.rotation_degrees, 90.0);
    lua.execute("local value = ecs.get(sky, 'Skybox'); value.enabled = false; ecs.set(sky, 'Skybox', value)").unwrap();
    assert!(countersplat_scene_ecs::extract(&world.borrow()).is_none());
}
