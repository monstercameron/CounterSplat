//! The dojo targets, their visible geometry, and real weapon shots share Lua/ECS data.
mod common;
use countersplat_ecs::{Entity, World, WorldOperation, WorldOutcome};
use countersplat_scene::{
    PhysicsBody, PlayerCamera, SceneManifest, Transform, VoxelColliderState, VoxelStatus,
};
use countersplat_scripting_lua::{LuaRuntime, ScriptOptions, WorldBridge};
use countersplat_ui::{UiElement, Widget};
use serde_json::{json, Value};
use std::{cell::RefCell, collections::BTreeMap, path::PathBuf, rc::Rc};

struct Bridge(Rc<RefCell<World>>);
impl WorldBridge for Bridge {
    fn request(&mut self, op: WorldOperation) -> Result<WorldOutcome, String> {
        self.0.borrow_mut().apply_operation(op)
    }
}
struct Fixture {
    world: Rc<RefCell<World>>,
    lua: LuaRuntime,
    ids: BTreeMap<String, Entity>,
}
impl Fixture {
    fn new(level: &str) -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let scene = SceneManifest::from_json(
            &common::gameplay_with_ready_weapon(&root),
        )
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
        {
            let mut w = world.borrow_mut();
            w.set_dynamic_resource("scene.level", json!({"id":level}));
            w.set_dynamic_resource("voxel.capture_movement", json!(false));
            w.get_mut::<VoxelColliderState>(ids["active_level"])
                .unwrap()
                .status = VoxelStatus::Unavailable;
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
        for script in [
            "hitboxes.lua",
            "first_person_controller.lua",
            "training_target.lua",
            "ballistics.lua",
        ] {
            lua.execute(
                &std::fs::read_to_string(root.join("assets/scripts").join(script)).unwrap(),
            )
            .unwrap();
        }
        Self { world, lua, ids }
    }
    fn tick(&self, dt: f64) {
        self.lua.run_systems(dt).unwrap();
    }
    fn events(&self, name: &str) -> Vec<Value> {
        self.world.borrow_mut().drain_events(name)
    }
    fn input(&self, button: &str, pressed: bool) {
        self.lua
            .dispatch(
                "input.mouse_button",
                &json!({
                    "button":button, "pressed":pressed, "gameplay":true
                }),
            )
            .unwrap();
    }
    fn step_and_collect(&self, impacts: &mut Vec<Value>) {
        self.tick(0.05);
        for shot in self.events("weapon.shot") {
            self.lua.dispatch("weapon.shot", &shot).unwrap();
        }
        for impact in self.events("projectile.impact") {
            self.lua.dispatch("projectile.impact", &impact).unwrap();
            impacts.push(impact);
        }
    }
}

#[test]
fn dojo_spawns_three_targets_with_shared_world_space_hitboxes() {
    let f = Fixture::new("58d333ae");
    f.tick(0.25);
    let snapshots = f.events("hitbox.visuals");
    assert_eq!(
        snapshots.last().unwrap()["segments"]
            .as_array()
            .unwrap()
            .len(),
        3 * 9 * 12
    );
    let w = f.world.borrow();
    for (name, x, z) in [
        ("training_target_left", -1.6, -4.0),
        ("training_target_center", 0.0, -5.5),
        ("training_target_right", 1.6, -7.0),
    ] {
        let id = f.ids[name];
        let pos = w.get::<Transform>(id).unwrap().position;
        assert!((pos[0] - x).abs() < 1e-4 && pos[1].abs() < 1e-4 && (pos[2] - z).abs() < 1e-4);
        let h = w.get_dynamic(id, "Hitbox").unwrap();
        assert_eq!(h["enabled"], true);
        assert_eq!(h["zones"].as_array().unwrap().len(), 9);
        assert_eq!(w.parent(id), Some(f.ids["training_range"]));
    }
    drop(w);
    f.lua.execute(r#"
        local ids=ecs.get_resource('scene.entities')
        local id=ids.training_target_center
        local box=ecs.get(id,'Hitbox')
        local edges=engine.hitbox.visual_segments(id,box)
        assert(#edges==108 and edges[1].zone=='head')
        local p=ecs.get(id,'Transform').position
        assert(math.abs(edges[1].start[1]-(p[1]-.12))<.0001)
        assert(math.abs(edges[1].start[2]-(p[2]+1.56))<.0001)
        local hit=engine.hitbox.trace_segment(id,box,{p[1],p[2]+1.68,p[3]+1},{p[1],p[2]+1.68,p[3]-1})
        assert(hit and hit.zone=='head')
        box.yaw=math.pi/2
        box.units_per_meter=2
        edges=engine.hitbox.visual_segments(id,box,2)
        assert(#edges==24)
        local boxes=engine.hitbox.visual_boxes(id,box,2)
        assert(#boxes==2 and boxes[1].zone=='head')
        assert(math.abs(boxes[1].center[1]-p[1])<.0001 and math.abs(boxes[1].center[2]-(p[2]+3.36))<.0001)
        assert(math.abs(boxes[1].half_extents[1]-.24)<.0001 and math.abs(boxes[1].half_extents[3]-.22)<.0001)
        assert(math.abs(boxes[1].rotation[1])<.0001 and math.abs(boxes[1].rotation[2]-1)<.0001)
        assert(math.abs(edges[1].start[1]-(p[1]+.22))<.0001)
        assert(math.abs(edges[1].start[2]-(p[2]+3.12))<.0001)
        assert(math.abs(edges[1].start[3]-(p[3]-.24))<.0001)
        hit=engine.hitbox.trace_segment(id,box,{p[1]+2,p[2]+3.36,p[3]},{p[1]-2,p[2]+3.36,p[3]})
        assert(hit and hit.zone=='head')
        box.enabled=false
        assert(#engine.hitbox.visual_segments(id,box)==0)
        assert(not engine.hitbox.trace_segment(id,box,{p[1]+2,p[2]+3.36,p[3]},{p[1]-2,p[2]+3.36,p[3]}))
    "#).unwrap();
}

#[test]
fn range_is_level_scoped_and_each_target_can_be_hidden_or_disabled() {
    let other = Fixture::new("9f0f903a");
    other.tick(0.25);
    assert!(other.events("hitbox.visuals").last().unwrap()["segments"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(
        other
            .world
            .borrow()
            .get_dynamic(other.ids["training_target_center"], "Hitbox")
            .unwrap()["enabled"],
        false
    );

    let f = Fixture::new("58d333ae");
    f.tick(0.25);
    f.events("hitbox.visuals");
    let id = f.ids["training_target_left"];
    let mut cfg = f
        .world
        .borrow()
        .get_dynamic(id, "TrainingTarget")
        .unwrap()
        .clone();
    cfg["visible"] = json!(false);
    f.world
        .borrow_mut()
        .set_dynamic(id, "TrainingTarget", cfg.clone())
        .unwrap();
    f.tick(0.05);
    assert_eq!(
        f.events("hitbox.visuals").last().unwrap()["segments"]
            .as_array()
            .unwrap()
            .len(),
        2 * 9 * 12
    );
    assert_eq!(
        f.world.borrow().get_dynamic(id, "Hitbox").unwrap()["enabled"],
        true,
        "hiding geometry must not disable hit detection"
    );
    cfg["enabled"] = json!(false);
    f.world
        .borrow_mut()
        .set_dynamic(id, "TrainingTarget", cfg)
        .unwrap();
    f.tick(0.05);
    assert_eq!(
        f.world.borrow().get_dynamic(id, "Hitbox").unwrap()["enabled"],
        false
    );
}

#[test]
fn hip_and_ads_shots_hit_the_range_through_the_real_lua_controllers() {
    let f = Fixture::new("58d333ae");
    f.tick(0.25);
    f.events("hitbox.visuals");
    let mut impacts = Vec::new();
    f.input("left", true);
    f.input("left", false);
    for _ in 0..35 {
        f.step_and_collect(&mut impacts);
    }
    assert_eq!(impacts.len(), 1, "the hip shot must reach a target");
    assert_eq!(
        impacts[0]["target"],
        f.ids["training_target_center"].to_string()
    );
    assert_eq!(impacts[0]["stance"], "hip");
    assert!(impacts[0]["zone"]
        .as_str()
        .is_some_and(|zone| !zone.is_empty()));
    assert!(impacts[0]["travel_seconds"].as_f64().unwrap() > 0.0);
    assert!(
        f.world
            .borrow()
            .get_dynamic(f.ids["training_target_center"], "Health")
            .unwrap()["current"]
            .as_f64()
            .unwrap()
            < 10000.0
    );

    f.input("right", true);
    for _ in 0..30 {
        f.step_and_collect(&mut impacts);
    }
    f.input("left", true);
    for _ in 0..20 {
        f.step_and_collect(&mut impacts);
    }
    f.input("left", false);
    assert_eq!(impacts.len(), 2, "the ADS shot must reach the same target");
    assert_eq!(
        impacts[1]["target"],
        f.ids["training_target_center"].to_string()
    );
    assert_eq!(impacts[1]["stance"], "ads");
    let world = f.world.borrow();
    let notice = world
        .get::<UiElement>(f.ids["training_hit_notice"])
        .unwrap();
    assert!(notice.visible);
    match &notice.widget {
        Widget::Text { text, .. } => {
            assert!(text.contains("ADS") && text.contains(" m") && text.contains(" ms"))
        }
        _ => panic!("expected text feedback"),
    }
}

#[test]
fn reset_repositions_targets_and_restores_health() {
    let f = Fixture::new("58d333ae");
    f.tick(0.25);
    {
        let mut w = f.world.borrow_mut();
        w.get_mut::<PlayerCamera>(f.ids["player"]).unwrap().position = [2.0, 1.7, 3.0];
        w.set_dynamic(
            f.ids["training_target_center"],
            "Health",
            json!({"current":1.0,"maximum":10000.0}),
        )
        .unwrap();
    }
    f.lua
        .dispatch(
            "input.key",
            &json!({"code":"KeyT","pressed":true,"gameplay":true,"is_repeat":false}),
        )
        .unwrap();
    let w = f.world.borrow();
    let position = w
        .get::<Transform>(f.ids["training_target_center"])
        .unwrap()
        .position;
    assert!((position[0] - 2.0).abs() < 1e-4 && (position[2] + 2.5).abs() < 1e-4);
    assert_eq!(
        w.get_dynamic(f.ids["training_target_center"], "Health")
            .unwrap()["current"],
        10000.0
    );
}

#[test]
fn floor_probe_aligns_target_and_rejects_stale_results() {
    let f = Fixture::new("58d333ae");
    f.world
        .borrow_mut()
        .set_dynamic_resource("voxel.capture_movement", json!(true));
    f.tick(0.25);
    let requests = f.events("physics.ground_request");
    assert_eq!(requests.len(), 3);
    let id = f.ids["training_target_center"];
    let request = requests
        .iter()
        .find(|request| request["entity"] == id.to_string())
        .unwrap();
    let token = request["token"].as_str().unwrap();
    f.lua
        .dispatch(
            "physics.ground_result",
            &json!({
                "entity":id.to_string(), "token":"stale", "height":2.0
            }),
        )
        .unwrap();
    assert!(f.world.borrow().get::<Transform>(id).unwrap().position[1].abs() < 1e-4);
    f.lua
        .dispatch(
            "physics.ground_result",
            &json!({
                "entity":id.to_string(), "token":token, "height":0.4
            }),
        )
        .unwrap();
    assert!((f.world.borrow().get::<Transform>(id).unwrap().position[1] - 0.4).abs() < 1e-4);

    f.lua
        .dispatch(
            "input.key",
            &json!({"code":"KeyT","pressed":true,"gameplay":true}),
        )
        .unwrap();
    let request = f
        .events("physics.ground_request")
        .into_iter()
        .find(|request| request["entity"] == id.to_string())
        .unwrap();
    f.world
        .borrow_mut()
        .get_mut::<Transform>(id)
        .unwrap()
        .position[1] = 3.0;
    f.lua
        .dispatch(
            "physics.ground_result",
            &json!({
                "entity":id.to_string(), "token":request["token"], "height":0.8
            }),
        )
        .unwrap();
    assert!(
        (f.world.borrow().get::<Transform>(id).unwrap().position[1] - 3.0).abs() < 1e-4,
        "a late result must not move an edited target"
    );
}

#[test]
fn late_level_collision_probes_already_spawned_targets() {
    let f = Fixture::new("58d333ae");
    f.tick(0.25);
    assert!(f.events("physics.ground_request").is_empty());
    f.world
        .borrow_mut()
        .set_dynamic_resource("voxel.capture_movement", json!(true));
    f.tick(0.05);
    assert_eq!(f.events("physics.ground_request").len(), 3);
    f.tick(0.05);
    assert!(
        f.events("physics.ground_request").is_empty(),
        "the probes are not repeated every tick"
    );
}

#[test]
fn unchanged_targets_do_not_republish_but_transform_edits_refresh_geometry() {
    let f = Fixture::new("58d333ae");
    f.tick(0.25);
    let initial = f
        .events("hitbox.visuals")
        .pop()
        .expect("initial target geometry");
    // Each initial placement changes the shared Hitbox store revision;
    // allow the other target readers to observe the final placement once.
    f.tick(1.0 / 60.0);
    f.events("hitbox.visuals");
    for _ in 0..30 {
        f.tick(1.0 / 60.0);
    }
    assert!(
        f.events("hitbox.visuals").is_empty(),
        "unchanged targets must not rebuild or republish their geometry every tick"
    );
    let target = f.ids["training_target_left"];
    f.world
        .borrow_mut()
        .get_mut::<Transform>(target)
        .unwrap()
        .position[0] += 1.0;
    f.tick(1.0 / 60.0);
    let updated = f
        .events("hitbox.visuals")
        .pop()
        .expect("moving a target must refresh its geometry");
    assert_ne!(
        initial["segments"], updated["segments"],
        "updated geometry must reflect the moved target"
    );
    f.tick(1.0 / 60.0);
    assert!(
        f.events("hitbox.visuals").is_empty(),
        "geometry should settle again after the edit"
    );
    let mut hitbox = f
        .world
        .borrow()
        .get_dynamic(target, "Hitbox")
        .unwrap()
        .clone();
    hitbox["zones"][0]["half_extents"][0] = json!(0.3);
    f.world
        .borrow_mut()
        .set_dynamic(target, "Hitbox", hitbox)
        .unwrap();
    f.tick(1.0 / 60.0);
    let changed = f
        .events("hitbox.visuals")
        .pop()
        .expect("editing a hitbox must refresh geometry");
    assert_ne!(updated["segments"], changed["segments"]);
}
