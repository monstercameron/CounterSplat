//! The can range end to end: the real Lua scripts place a wall and cans and turn a real weapon shot into an impulse,
//! and the real Rust prop simulation stands the cans on the wall and knocks the shot one off.
mod common;
use countersplat_ecs::{Entity, World, WorldOperation, WorldOutcome};
use countersplat_physics::prop_runtime::{Ecs, PropRuntime};
use countersplat_scene::{PhysicsBody, PlayerCamera, SceneManifest, VoxelColliderState, VoxelStatus};
use countersplat_scripting_lua::{LuaRuntime, ScriptOptions, WorldBridge};
use serde_json::{json, Value};
use std::{cell::RefCell, collections::BTreeMap, path::PathBuf, rc::Rc};

struct Bridge(Rc<RefCell<World>>);
impl WorldBridge for Bridge {
    fn request(&mut self, op: WorldOperation) -> Result<WorldOutcome, String> {
        self.0.borrow_mut().apply_operation(op)
    }
}

/// The ECS as the browser host hands it to the prop runtime: JSON operations in, the reply's value out.
struct HostEcs(Rc<RefCell<World>>);
impl Ecs for HostEcs {
    fn ecs(&mut self, operation: Value) -> Result<Value, String> {
        let reply = self.0.borrow_mut().apply_script_json(&operation.to_string())?;
        countersplat_scene::voxel_host::reply_value(&reply)
    }
}

struct Fixture {
    world: Rc<RefCell<World>>,
    lua: LuaRuntime,
    ids: BTreeMap<String, Entity>,
    props: PropRuntime,
}

impl Fixture {
    fn new(level: &str) -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let scene = SceneManifest::from_json(&common::gameplay_with_ready_weapon(&root)).unwrap();
        let world = Rc::new(RefCell::new(World::new()));
        countersplat_ui_ecs::register(&mut world.borrow_mut()).unwrap();
        countersplat_scene_ecs::register(&mut world.borrow_mut()).unwrap();
        let ids =
            countersplat_scene_ecs::instantiate_layers(&mut world.borrow_mut(), &scene, &["ui", "player", "level"]).unwrap();
        {
            let mut w = world.borrow_mut();
            w.set_dynamic_resource("scene.level", json!({"id": level}));
            w.set_dynamic_resource("voxel.capture_movement", json!(false));
            w.get_mut::<VoxelColliderState>(ids["active_level"]).unwrap().status = VoxelStatus::Unavailable;
            let camera = w.get_mut::<PlayerCamera>(ids["player"]).unwrap();
            camera.position = [0.0, 1.7, 0.0];
            camera.yaw = 0.0;
            camera.pitch = 0.0;
            let body = w.get_mut::<PhysicsBody>(ids["player"]).unwrap();
            body.radius = 0.3;
            body.capsule_half_height = 0.55;
            body.collision_offset_y = -0.85;
        }
        let lua = LuaRuntime::new(Bridge(world.clone()), ScriptOptions::default()).unwrap();
        for script in ["hitboxes.lua", "first_person_controller.lua", "can_range.lua", "ballistics.lua"] {
            lua.execute(&std::fs::read_to_string(root.join("assets/scripts").join(script)).unwrap()).unwrap();
        }
        Self { world, lua, ids, props: PropRuntime::default() }
    }

    /// One 50 ms frame: the props step first (as the browser's Viewer does), then the scripts, with the events the
    /// browser routes between ticks delivered back to Lua. Returns the frame's impacts.
    fn frame(&mut self) -> Vec<Value> {
        self.props.tick(&mut HostEcs(self.world.clone()), 0.05).unwrap();
        self.lua.run_systems(0.05).unwrap();
        let mut impacts = Vec::new();
        // Impacts before spawns: the browser host may deliver a same-tick spawn and impact in this order.
        for channel in ["weapon.shot", "projectile.impact", "projectile.spawned", "projectile.expired"] {
            let events = self.world.borrow_mut().drain_events(channel);
            for event in events {
                self.lua.dispatch(channel, &event).unwrap();
                if channel == "projectile.impact" {
                    impacts.push(event);
                }
            }
        }
        impacts
    }

    fn position(&self, id: &str) -> [f64; 3] {
        let transform: Value = serde_json::from_str(
            &self.world.borrow_mut().apply_script_json(&json!({"op": "get", "id": id, "name": "Transform"}).to_string()).unwrap(),
        )
        .unwrap();
        let p = &transform["value"]["position"];
        [p[0].as_f64().unwrap(), p[1].as_f64().unwrap(), p[2].as_f64().unwrap()]
    }

    fn layout(&self) -> Value {
        let text = self.world.borrow().script_resource_value_json("props.layout").unwrap().expect("props.layout");
        serde_json::from_str(&text).unwrap()
    }
}

#[test]
fn shooting_a_can_knocks_it_off_the_wall() {
    let mut f = Fixture::new("58d333ae");
    // Without level collision the range is placed on the player's feet after a short wait.
    for _ in 0..70 {
        f.frame();
    }
    let layout = f.layout();
    let bodies = layout["bodies"].as_array().unwrap();
    assert_eq!(bodies.len(), 12, "floor, wall and ten cans");
    let cans: Vec<String> = bodies[2..].iter().map(|b| b["entity"].as_str().unwrap().to_owned()).collect();
    assert!(bodies[2..].iter().all(|b| b["dynamic"] == true && b["shape"] == "cylinder"));
    let wall = bodies[1]["position"].clone();
    // 4.5 m ahead (-z at yaw 0) and 2.2 m to the right, where the spawn has a clear line of fire past the barn's braces.
    assert!((wall[0].as_f64().unwrap() - 2.2).abs() < 1e-4 && (wall[2].as_f64().unwrap() + 4.5).abs() < 1e-4, "{wall}");

    // The cans settle upright on top of the 1 m wall (feet at y 0).
    for _ in 0..40 {
        f.frame();
    }
    let rest: Vec<[f64; 3]> = cans.iter().map(|id| f.position(id)).collect();
    for (id, p) in cans.iter().zip(&rest) {
        assert!((p[1] - 1.061).abs() < 0.01, "can {id} stands on the wall: {p:?}");
    }

    // Aim the camera at the fourth can and fire one hip shot.
    let target = &rest[3];
    {
        let mut w = f.world.borrow_mut();
        let camera = w.get_mut::<PlayerCamera>(f.ids["player"]).unwrap();
        let (dx, dy, dz) = (target[0], target[1] - 1.7, target[2]);
        camera.yaw = dx.atan2(-dz) as f32;
        camera.pitch = dy.atan2((dx * dx + dz * dz).sqrt()) as f32;
    }
    f.frame();
    f.lua.dispatch("input.mouse_button", &json!({"button": "left", "pressed": true, "gameplay": true})).unwrap();
    f.lua.dispatch("input.mouse_button", &json!({"button": "left", "pressed": false, "gameplay": true})).unwrap();
    let mut impacts = Vec::new();
    for _ in 0..80 {
        impacts.extend(f.frame());
    }
    assert_eq!(impacts.len(), 1, "one shot, one impact: {impacts:?}");
    assert_eq!(impacts[0]["target"], cans[3], "the shot hits the aimed can");
    assert!(impacts[0].get("damage").is_none() || impacts[0]["target"] != f.ids["player"].to_string());

    let after: Vec<[f64; 3]> = cans.iter().map(|id| f.position(id)).collect();
    assert!(after[3][1] < 0.5, "the shot can is knocked off the wall: {:?}", after[3]);
    let moved = ((after[3][0] - rest[3][0]).powi(2) + (after[3][2] - rest[3][2]).powi(2)).sqrt();
    assert!(moved > 0.15, "and pushed away from the shooter: moved {moved}");
    for (i, (a, r)) in after.iter().zip(&rest).enumerate().filter(|(i, _)| *i != 3) {
        assert!(a[1] > 1.0, "can {i} stays on the wall: {a:?} (was {r:?})");
    }

    // T stands every can back up where it was; the wall stays put although the camera has turned to aim.
    f.lua.dispatch("input.key", &json!({"code": "KeyT", "pressed": true, "is_repeat": false, "gameplay": true})).unwrap();
    for _ in 0..40 {
        f.frame();
    }
    assert_eq!(f.layout()["version"], 1, "a reset keeps the layout");
    assert_eq!(f.layout()["bodies"][1]["position"], wall);
    for (id, r) in cans.iter().zip(&rest) {
        let p = f.position(id);
        assert!((0..3).all(|k| (p[k] - r[k]).abs() < 0.01), "can {id} stands again where it was: {p:?} (was {r:?})");
    }
}

#[test]
fn the_range_only_appears_in_its_level() {
    let mut f = Fixture::new("942c5f7e");
    for _ in 0..80 {
        f.frame();
    }
    assert!(f.world.borrow().script_resource_value_json("props.layout").unwrap().is_none());
}
