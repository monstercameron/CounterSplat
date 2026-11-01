//! Exercise the actual scene Lua and ECS bridge, including asynchronous traces.
use countersplat_ecs::{Entity, World, WorldOperation, WorldOutcome};
use countersplat_scene::{SceneManifest, Transform, VoxelColliderState, VoxelStatus};
use countersplat_scripting_lua::{LuaRuntime, ScriptOptions, WorldBridge};
use serde_json::{json, Value};
use std::{cell::RefCell, path::PathBuf, rc::Rc};

struct Bridge(Rc<RefCell<World>>);
impl WorldBridge for Bridge {
    fn request(&mut self, op: WorldOperation) -> Result<WorldOutcome, String> {
        self.0.borrow_mut().apply_operation(op)
    }
}
struct Fixture {
    world: Rc<RefCell<World>>,
    lua: LuaRuntime,
    weapon: Entity,
    player: Entity,
}
impl Fixture {
    fn new(worker: bool) -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let manifest = SceneManifest::from_json(
            &std::fs::read(root.join("assets/scenes/gameplay.json")).unwrap(),
        )
        .unwrap();
        let world = Rc::new(RefCell::new(World::new()));
        countersplat_ui_ecs::register(&mut world.borrow_mut()).unwrap();
        countersplat_scene_ecs::register(&mut world.borrow_mut()).unwrap();
        let ids = countersplat_scene_ecs::instantiate_layers(
            &mut world.borrow_mut(),
            &manifest,
            &["ui", "player", "level"],
        )
        .unwrap();
        let (weapon, player, level) = (ids["first_person"], ids["player"], ids["active_level"]);
        world.borrow_mut().set_dynamic(weapon,"Ballistics",json!({"speed_mps":10.0,"gravity_mps2":0.0,"max_range_m":100.0,"max_lifetime_s":3.0,"damage":35.0,"show_tracers":true})).unwrap();
        *world
            .borrow_mut()
            .get_mut::<VoxelColliderState>(level)
            .unwrap() = VoxelColliderState {
            status: if worker {
                VoxelStatus::Ready
            } else {
                VoxelStatus::Unavailable
            },
            resolution: Some(1.0),
            ..Default::default()
        };
        world
            .borrow_mut()
            .set_dynamic_resource("voxel.capture_movement", json!(worker));
        let lua = LuaRuntime::new(Bridge(world.clone()), ScriptOptions::default()).unwrap();
        lua.execute(&std::fs::read_to_string(root.join("assets/scripts/ballistics.lua")).unwrap())
            .unwrap();
        Self {
            world,
            lua,
            weapon,
            player,
        }
    }
    fn shot(&self) {
        self.lua.dispatch("weapon.shot",&json!({"entity":self.weapon.to_string(),"player":self.player.to_string(),"shot":1,"origin":[0,0,0],"direction":[0,0,-1]})).unwrap();
    }
    fn tick(&self, dt: f64) {
        self.lua.run_systems(dt).unwrap();
    }
    fn events(&self, name: &str) -> Vec<Value> {
        self.world.borrow_mut().drain_events(name)
    }
    fn target(&self, z: f32) -> Entity {
        let mut w = self.world.borrow_mut();
        let id = w.spawn();
        w.insert(
            id,
            Transform {
                position: [0.0, 0.0, z],
            },
        )
        .unwrap();
        w.set_dynamic(
            id,
            "Hitbox",
            json!({"enabled":true,"radius":1.0,"offset":[0,0,0]}),
        )
        .unwrap();
        w.set_dynamic(id, "Health", json!({"current":100.0}))
            .unwrap();
        id
    }
}

#[test]
fn shots_travel_before_hitting_an_entity_and_apply_damage_once() {
    let f = Fixture::new(false);
    let target = f.target(-10.0);
    f.shot();
    for _ in 0..3 {
        f.tick(0.25);
    }
    assert!(f.events("projectile.impact").is_empty());
    assert_eq!(
        f.world.borrow().get_dynamic(target, "Health").unwrap()["current"],
        100.0
    );
    f.tick(0.25);
    let hits = f.events("projectile.impact");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0]["kind"], "entity");
    assert_eq!(hits[0]["target"], target.to_string());
    assert!((hits[0]["travel_seconds"].as_f64().unwrap() - 0.9).abs() < 1e-6);
    assert_eq!(
        f.world.borrow().get_dynamic(target, "Health").unwrap()["current"],
        65.0
    );
    assert_eq!(f.events("combat.damage").len(), 1);
    f.tick(0.25);
    assert!(f.events("projectile.impact").is_empty());
}

#[test]
fn a_closer_voxel_blocks_an_entity_and_stale_results_do_nothing() {
    let f = Fixture::new(true);
    let target = f.target(-3.0);
    f.shot();
    f.tick(0.25);
    let request = f.events("physics.trace_request").pop().unwrap();
    let entity = request["entity"].as_str().unwrap();
    let token = request["token"].as_str().unwrap();
    f.lua
        .dispatch(
            "physics.trace_result",
            &json!({"entity":entity,"token":"stale","status":"miss"}),
        )
        .unwrap();
    assert!(f.events("projectile.impact").is_empty());
    f.lua.dispatch("physics.trace_result",&json!({"entity":entity,"token":token,"status":"hit","fraction":0.4,"position":[0,0,-1],"normal":[0,0,1]})).unwrap();
    let hits = f.events("projectile.impact");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0]["kind"], "level");
    assert!((hits[0]["travel_seconds"].as_f64().unwrap() - 0.1).abs() < 1e-6);
    assert_eq!(
        f.world.borrow().get_dynamic(target, "Health").unwrap()["current"],
        100.0
    );
    f.lua.dispatch("physics.trace_result",&json!({"entity":entity,"token":token,"status":"hit","fraction":0.4,"position":[0,0,-1],"normal":[0,0,1]})).unwrap();
    assert!(f.events("projectile.impact").is_empty());
}

#[test]
fn delayed_collision_results_do_not_skip_the_unchecked_path() {
    let f = Fixture::new(true);
    f.shot();
    f.tick(0.1);
    let first = f.events("physics.trace_request").pop().unwrap();
    f.tick(0.2);
    f.tick(0.2);
    assert!(f.events("physics.trace_request").is_empty());
    f.lua
        .dispatch(
            "physics.trace_result",
            &json!({"entity":first["entity"],"token":first["token"],"status":"miss"}),
        )
        .unwrap();
    f.tick(0.1);
    let second = f.events("physics.trace_request").pop().unwrap();
    assert!((second["start"][2].as_f64().unwrap() + 1.0).abs() < 1e-5);
    assert!((second["finish"][2].as_f64().unwrap() + 6.0).abs() < 1e-5);
    f.lua
        .dispatch(
            "physics.trace_result",
            &json!({"entity":second["entity"],"token":second["token"],"status":"error"}),
        )
        .unwrap();
    assert_eq!(
        f.events("projectile.expired")[0]["reason"],
        "collision_error"
    );
    assert!(f.events("projectile.impact").is_empty());
}

#[test]
fn travel_time_is_independent_of_simulation_tick_rate() {
    for rate in [30, 60, 144] {
        let f = Fixture::new(false);
        f.target(-10.0);
        f.shot();
        let mut hit = None;
        for _ in 0..rate {
            f.tick(1.0 / rate as f64);
            if let Some(event) = f.events("projectile.impact").pop() {
                hit = Some(event);
                break;
            }
        }
        let seconds = hit.unwrap()["travel_seconds"].as_f64().unwrap();
        assert!((seconds - 0.9).abs() < 0.0001, "{rate} Hz: {seconds}");
    }
}

#[test]
fn real_voxel_tree_and_lua_report_the_same_timed_impact() {
    use countersplat_physics::voxel_collision::VoxelCollider;
    let metadata = json!({"version":"1.1","gridBounds":{"min":[-2,-2,-12],"max":[2,2,-8]},
        "voxelResolution":1.0,"leafSize":4,"treeDepth":0,"nodeCount":1,"leafDataCount":2});
    let mut mask = 0u64;
    for x in 0..4 {
        for y in 0..4 {
            mask |= 1 << (x + y * 4 + 2 * 16);
        }
    }
    let bytes: Vec<u8> = [0u32, mask as u32, (mask >> 32) as u32]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect();
    let world = VoxelCollider::from_json_and_bin(metadata.to_string().as_bytes(), &bytes).unwrap();
    let f = Fixture::new(true);
    f.shot();
    for _ in 0..30 {
        f.tick(0.05);
        for request in f.events("physics.trace_request") {
            let point = |name: &str| [0, 1, 2].map(|i| request[name][i].as_f64().unwrap() as f32);
            let hit = world
                .trace_segment(point("start"), point("finish"))
                .unwrap();
            let result = match hit {
                Some(hit) => {
                    json!({"entity":request["entity"],"token":request["token"],"status":"hit",
                    "fraction":hit.fraction,"position":hit.position,"normal":hit.normal})
                }
                None => {
                    json!({"entity":request["entity"],"token":request["token"],"status":"miss"})
                }
            };
            f.lua.dispatch("physics.trace_result", &result).unwrap();
        }
        if let Some(hit) = f.events("projectile.impact").pop() {
            assert_eq!(hit["kind"], "level");
            assert!((hit["position"][2].as_f64().unwrap() + 9.0).abs() < 1e-5);
            assert!((hit["travel_seconds"].as_f64().unwrap() - 0.9).abs() < 1e-5);
            return;
        }
    }
    panic!("the projectile must hit the voxel wall");
}

#[test]
fn unloaded_or_failed_collision_cannot_silently_skip_geometry() {
    let f = Fixture::new(true);
    f.lua.execute("local id=ecs.get_resource('scene.entities').active_level; local s=ecs.get(id,'VoxelColliderState'); s.status='loading'; ecs.set(id,'VoxelColliderState',s)").unwrap();
    f.shot();
    for _ in 0..40 {
        f.tick(0.25);
    }
    assert!(f.events("physics.trace_request").is_empty());
    assert_eq!(
        f.events("projectile.expired")[0]["reason"],
        "collision_unavailable"
    );
    assert!(f.events("projectile.impact").is_empty());
}

#[test]
fn real_weapon_shot_snapshots_the_view_before_later_camera_motion() {
    use countersplat_scene::PlayerCamera;
    let f = Fixture::new(false);
    {
        let mut world = f.world.borrow_mut();
        let camera = world.get_mut::<PlayerCamera>(f.player).unwrap();
        camera.position = [2.0, 3.0, 4.0];
        camera.view_offset = [0.1, 0.2, 0.3];
        camera.view_posture_offset_y = 0.4;
        camera.yaw = 0.0;
        camera.pitch = 0.0;
        camera.view_bob_pitch = 0.1;
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    f.lua
        .execute(
            &std::fs::read_to_string(root.join("assets/scripts/first_person_controller.lua"))
                .unwrap(),
        )
        .unwrap();
    f.lua
        .dispatch(
            "input.mouse_button",
            &json!({"button":"left","pressed":true,"gameplay":true}),
        )
        .unwrap();
    f.lua
        .dispatch(
            "input.mouse_button",
            &json!({"button":"left","pressed":false,"gameplay":true}),
        )
        .unwrap();
    for _ in 0..30 {
        f.tick(1.0 / 60.0);
    }
    let shot = f.events("weapon.shot").pop().expect("weapon should fire");
    assert!((shot["origin"][0].as_f64().unwrap() - 2.1).abs() < 1e-5);
    assert!((shot["origin"][1].as_f64().unwrap() - 3.6).abs() < 1e-5);
    let length = (0..3)
        .map(|i| shot["direction"][i].as_f64().unwrap().powi(2))
        .sum::<f64>()
        .sqrt();
    assert!(
        (length - 1.0).abs() < 1e-6,
        "shot direction must be normalized"
    );
    f.world
        .borrow_mut()
        .get_mut::<PlayerCamera>(f.player)
        .unwrap()
        .yaw = 1.0;
    f.lua.dispatch("weapon.shot", &shot).unwrap();
    let launch = f
        .world
        .borrow()
        .get_dynamic_resource("combat.last_shot")
        .unwrap()
        .clone();
    for i in 0..3 {
        let expected = shot["direction"][i].as_f64().unwrap() * 10.0;
        assert!(
            (launch["velocity"][i].as_f64().unwrap() - expected).abs() < 1e-5,
            "projectile velocity must use the captured shot direction"
        );
    }
}

#[test]
fn gravity_uses_analytic_motion_at_different_tick_rates() {
    for rate in [30, 60, 144] {
        let f = Fixture::new(false);
        let mut config = f
            .world
            .borrow()
            .get_dynamic(f.weapon, "Ballistics")
            .unwrap()
            .clone();
        config["gravity_mps2"] = json!(9.81);
        f.world
            .borrow_mut()
            .set_dynamic(f.weapon, "Ballistics", config)
            .unwrap();
        f.shot();
        for _ in 0..rate {
            f.tick(1.0 / rate as f64);
        }
        let world = f.world.borrow();
        let projectile = world.query_dynamic(&["Projectile"])[0];
        let position = world.get::<Transform>(projectile).unwrap().position;
        assert!(
            (position[1] + 4.905).abs() < 0.0005,
            "{rate} Hz: {position:?}"
        );
        assert!(
            (position[2] + 10.0).abs() < 0.0005,
            "{rate} Hz: {position:?}"
        );
    }
}

#[test]
fn weapon_shot_tracks_the_visible_sight_through_the_render_aspect() {
    use countersplat_scene::PlayerCamera;
    let f = Fixture::new(false);
    {
        let mut w = f.world.borrow_mut();
        let camera = w.get_mut::<PlayerCamera>(f.player).unwrap();
        camera.yaw = 0.0;
        camera.pitch = 0.0;
        camera.roll = 0.0;
        camera.view_bob_pitch = 0.0;
        camera.view_bob_roll = 0.0;
        camera.view_turn_roll = 0.0;
        camera.fov_degrees = 60.0;
        camera.view_fov_degrees = None;
        w.set_dynamic_resource("render.scene_aspect", json!(4.0 / 3.0));
        let mut animation = w.get_dynamic(f.weapon, "WeaponAnimation").unwrap().clone();
        animation["idle_enabled"] = json!(false);
        animation["recoil_strength"] = json!(0.0);
        w.set_dynamic(f.weapon, "WeaponAnimation", animation)
            .unwrap();
    }
    let view = f
        .world
        .borrow()
        .get_dynamic(f.weapon, "FirstPersonView")
        .unwrap()
        .clone();
    let vertical = view["vertical_offset"].as_f64().unwrap();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    f.lua
        .execute(
            &std::fs::read_to_string(root.join("assets/scripts/first_person_controller.lua"))
                .unwrap(),
        )
        .unwrap();
    f.lua
        .dispatch("weapon.aim", &json!({"pressed":true}))
        .unwrap();
    for _ in 0..30 {
        f.tick(1.0 / 60.0);
    }
    f.lua
        .dispatch("weapon.trigger", &json!({"pressed":true}))
        .unwrap();
    f.tick(1.0 / 60.0);
    let shot = f.events("weapon.shot").pop().expect("ADS shot");
    // Invert the bottom-anchored 16:9 source mapping for a 4:3 scene.
    let scene_y = 1.0 + (361.0 / 720.0 + vertical - 1.0) * 0.75;
    let sy = (0.5 - scene_y) * 2.0 * (30.0_f64.to_radians()).tan();
    let expected_y = sy / (1.0 + sy * sy).sqrt();
    assert!((shot["direction"][0].as_f64().unwrap()).abs() < 1e-7);
    assert!((shot["direction"][1].as_f64().unwrap() - expected_y).abs() < 1e-6);
}

#[test]
fn a_hidden_weapon_fires_through_the_camera_center() {
    use countersplat_scene::PlayerCamera;
    let f = Fixture::new(false);
    {
        let mut world = f.world.borrow_mut();
        let camera = world.get_mut::<PlayerCamera>(f.player).unwrap();
        camera.yaw = 0.0;
        camera.pitch = 0.0;
        camera.view_bob_pitch = 0.0;
        let mut view = world
            .get_dynamic(f.weapon, "FirstPersonView")
            .unwrap()
            .clone();
        view["enabled"] = json!(false);
        world
            .set_dynamic(f.weapon, "FirstPersonView", view)
            .unwrap();
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    f.lua
        .execute(
            &std::fs::read_to_string(root.join("assets/scripts/first_person_controller.lua"))
                .unwrap(),
        )
        .unwrap();
    f.lua
        .dispatch("weapon.trigger", &json!({"pressed":true}))
        .unwrap();
    f.lua
        .dispatch("weapon.trigger", &json!({"pressed":false}))
        .unwrap();
    for _ in 0..30 {
        f.tick(1.0 / 60.0);
    }
    let shot = f.events("weapon.shot").pop().expect("weapon should fire");
    assert!(shot["direction"][0].as_f64().unwrap().abs() < 1e-7);
    assert!(shot["direction"][1].as_f64().unwrap().abs() < 1e-7);
    assert!((shot["direction"][2].as_f64().unwrap() + 1.0).abs() < 1e-7);
}
