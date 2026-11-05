//! Exercise the actual scene Lua and ECS bridge, including asynchronous traces.
mod common;
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
            &common::gameplay_with_ready_weapon(&root),
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
        lua.execute(&std::fs::read_to_string(root.join("assets/scripts/hitboxes.lua")).unwrap())
            .unwrap();
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
    for render_fov in [60.0_f32, 0.2_f32] {
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
            camera.view_fov_degrees = Some(render_fov);
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
        let sy = (0.5 - scene_y) * 2.0 * ((render_fov as f64 / 2.0).to_radians()).tan();
        let expected_y = sy / (1.0 + sy * sy).sqrt();
        assert!((shot["direction"][0].as_f64().unwrap()).abs() < 1e-7);
        assert!(
            (shot["direction"][1].as_f64().unwrap() - expected_y).abs() < 1e-7,
            "render FOV: {render_fov}"
        );
    }
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
#[test]
fn human_zones_trace_and_report_through_real_projectiles() {
    let f = Fixture::new(false);
    let target = f.target(-10.0);
    let config = f
        .world
        .borrow()
        .get_dynamic(f.player, "Hitbox")
        .unwrap()
        .clone();
    f.world
        .borrow_mut()
        .set_dynamic(target, "Hitbox", config)
        .unwrap();
    f.world
        .borrow_mut()
        .set_dynamic_resource("test.target", json!(target.to_string()));
    f.lua
        .execute(
            r#"
      local target=ecs.get_resource('test.target')
      local h=ecs.get(target,'Hitbox')
      for _,case in ipairs({
        {'head',0,1.68},{'neck',0,1.51},{'chest',0,1.29},
        {'abdomen',0,.99},{'pelvis',0,.82},{'left_arm',-.28,1.17},
        {'right_arm',.28,1.17},{'left_leg',-.10,.41},{'right_leg',.10,.41}
      }) do
        local hit=engine.hitbox.trace_segment(target,h,{case[2],case[3],-8},{case[2],case[3],-12})
        assert(hit and hit.zone==case[1], case[1] .. ': ' .. tostring(hit and hit.zone))
        assert(hit.fraction>=0 and hit.fraction<=1)
      end
      assert(not engine.hitbox.trace_segment(target,h,{2,1,-8},{2,1,-12}))
      assert(not engine.hitbox.trace_segment(target,h,{0,1,-8},{0,1,-8}))
      local inside=engine.hitbox.trace_segment(target,h,{0,1.68,-10},{0,1.68,-12})
      assert(inside and inside.zone=='head' and inside.fraction==0)
      h.yaw=math.pi/2
      local rotated=engine.hitbox.trace_segment(target,h,{-2,1.68,-10},{2,1.68,-10})
      assert(rotated and rotated.zone=='head' and rotated.normal[1]<-.99)
      h.enabled=false
      assert(not engine.hitbox.trace_segment(target,h,{0,1.68,-8},{0,1.68,-12}))
    "#,
        )
        .unwrap();
    f.lua.dispatch("weapon.shot", &json!({"entity":f.weapon.to_string(),"player":f.player.to_string(),"shot":7,"origin":[0,1.68,0],"direction":[0,0,-1]})).unwrap();
    for _ in 0..4 {
        f.tick(0.25);
    }
    let impact = f.events("projectile.impact").pop().unwrap();
    assert_eq!(impact["zone"], "head");
    assert_eq!(f.events("combat.damage")[0]["zone"], "head");
}

#[test]
fn player_hitbox_follows_rig_scale_and_crouched_feet() {
    let f = Fixture::new(false);
    f.lua.execute(r#"
      local id=ecs.get_resource('scene.entities').player
      local camera=ecs.get(id,'PlayerCamera'); camera.position={0,3.4,0}; camera.yaw=0; ecs.set(id,'PlayerCamera',camera)
      local rig=ecs.get(id,'PlayerRig'); rig.units_per_meter=2; rig.eye_height_m=1.7; ecs.set(id,'PlayerRig',rig)
      local body=ecs.get(id,'PhysicsBody'); body.radius=.5; body.capsule_half_height=1.35; body.collision_offset_y=-1.55; ecs.set(id,'PhysicsBody',body)
      local h=ecs.get(id,'Hitbox')
      local hit=engine.hitbox.trace_segment(id,h,{0,3.36,2},{0,3.36,-2})
      assert(hit and hit.zone=='head')
      -- Crouching keeps the feet fixed but halves the standing height.
      body.capsule_half_height=.5; body.collision_offset_y=-.7; ecs.set(id,'PhysicsBody',body)
      camera.position[2]=1.7; ecs.set(id,'PlayerCamera',camera)
      hit=engine.hitbox.trace_segment(id,h,{0,1.68,2},{0,1.68,-2})
      assert(hit and hit.zone=='head')
      assert(not engine.hitbox.trace_segment(id,h,{0,3.36,2},{0,3.36,-2}))
    "#).unwrap();
}

#[test]
fn armor_absorbs_part_of_damage_then_depletes() {
    let f = Fixture::new(false);
    let target = f.target(-3.0);
    f.world
        .borrow_mut()
        .set_dynamic(
            target,
            "Armor",
            json!({"current":10.0,"maximum":100.0,"absorption":0.5}),
        )
        .unwrap();
    f.shot();
    f.tick(0.25);
    let w = f.world.borrow();
    assert_eq!(w.get_dynamic(target, "Armor").unwrap()["current"], 0.0);
    assert_eq!(w.get_dynamic(target, "Health").unwrap()["current"], 75.0);
    drop(w);
    let damage = f.events("combat.damage");
    assert_eq!(damage[0]["armor_absorbed"], 10.0);
    assert_eq!(damage[0]["health_damage"], 25.0);
}

#[test]
fn impact_diagnostics_only_appear_with_the_ecs_tools() {
    use countersplat_ui::UiElement;
    let f = Fixture::new(false);
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    f.lua
        .execute(&std::fs::read_to_string(root.join("assets/scripts/editor_overlay.lua")).unwrap())
        .unwrap();
    let notice: Entity = f
        .world
        .borrow()
        .get_dynamic_resource("scene.entities")
        .unwrap()["weapon_impact_notice"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let visible = || f.world.borrow().get::<UiElement>(notice).unwrap().visible;
    let toggle = || {
        f.lua
            .dispatch(
                "input.key",
                &json!({"code":"Backquote","pressed":true,"is_repeat":false,"gameplay":true}),
            )
            .unwrap();
        for event in f.events("engine.event") {
            f.lua.dispatch("engine.event", &event).unwrap();
        }
    };
    f.target(-3.0);
    f.shot();
    f.tick(0.25);
    assert!(
        !visible(),
        "an impact must not leak debug output into the regular HUD"
    );
    assert_eq!(f.events("projectile.impact").len(), 1);
    toggle();
    assert!(
        visible(),
        "a still-current diagnostic is visible with ECS tools open"
    );
    toggle();
    assert!(
        !visible(),
        "closing ECS tools hides an active notice immediately"
    );
    toggle();
    assert!(visible());
    for _ in 0..5 {
        f.tick(0.25);
    }
    assert!(!visible(), "the notice still expires normally");
    f.shot();
    f.tick(0.25);
    assert!(
        visible(),
        "new impacts are displayed while ECS tools are open"
    );
}

#[test]
fn combat_reset_cancels_in_flight_projectiles_and_stale_worker_replies() {
    let f = Fixture::new(true);
    f.shot();
    f.lua.run_systems(0.1).unwrap();
    let trace = f
        .events("physics.trace_request")
        .pop()
        .expect("pending trace");
    let projectile = trace["entity"].as_str().unwrap().to_owned();
    f.lua
        .dispatch("combat.reset", &json!({"owner":f.player.to_string()}))
        .unwrap();
    f.lua
        .dispatch(
            "physics.trace_result",
            &json!({"entity":projectile,"token":trace["token"],"status":"miss"}),
        )
        .unwrap();
    f.lua.run_systems(0.1).unwrap();
    assert!(f.events("projectile.impact").is_empty());
    assert!(f.events("physics.trace_request").is_empty());
}

/// A shotgun-style shot (weapon profile `projectiles`, `spread_deg`, `projectile_mass_kg`) fires one projectile per
/// pellet, spread evenly inside the cone, each carrying its own mass.
#[test]
fn a_shot_fires_every_pellet_inside_its_spread_cone() {
    let f = Fixture::new(false);
    f.world.borrow_mut().set_dynamic(f.weapon, "Ballistics", json!({"speed_mps": 404.0, "gravity_mps2": 0.0,
        "max_range_m": 100.0, "max_lifetime_s": 3.0, "damage": 6.4, "show_tracers": true,
        "projectiles": 9, "spread_deg": 3.0, "projectile_mass_kg": 0.003486})).unwrap();
    f.shot();
    let spawned = f.events("projectile.spawned");
    assert_eq!(spawned.len(), 9, "{spawned:?}");
    let mut directions: Vec<[f64; 3]> = Vec::new();
    for event in &spawned {
        let v: Vec<f64> = (0..3).map(|i| event["velocity"][i].as_f64().unwrap()).collect();
        let speed = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        assert!((speed - 404.0).abs() < 1e-6);
        let d = [v[0] / speed, v[1] / speed, v[2] / speed];
        let off_axis = (-d[2]).clamp(-1.0, 1.0).acos().to_degrees();
        assert!(off_axis <= 1.5 + 1e-9, "pellet {off_axis} degrees off axis");
        assert!(directions.iter().all(|o| (0..3).map(|i| (o[i] - d[i]).abs()).sum::<f64>() > 1e-4), "pellets differ");
        directions.push(d);
        let projectile = f.world.borrow().get_dynamic(event["entity"].as_str().unwrap().parse().unwrap(), "Projectile")
            .unwrap().clone();
        assert_eq!(projectile["mass_kg"], 0.003486);
    }
}
