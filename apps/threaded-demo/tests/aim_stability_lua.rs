//! Real Lua weapon stability and hostile-projectile event integration.
mod common;
use countersplat_ecs::{Entity, World, WorldOperation, WorldOutcome};
use countersplat_scene::{
    PhysicsBody, PlayerCamera, PlayerControls, PlayerRig, SceneManifest, VoxelColliderState,
    VoxelStatus,
};
use countersplat_scripting_lua::{LuaRuntime, ScriptOptions, WorldBridge};
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
    fn new() -> Self {
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
            w.set_dynamic_resource("voxel.capture_movement", json!(false));
            w.set_dynamic_resource("render.scene_aspect", json!(4.0 / 3.0));
            w.get_mut::<VoxelColliderState>(ids["active_level"])
                .unwrap()
                .status = VoxelStatus::Unavailable;
            let camera = w.get_mut::<PlayerCamera>(ids["player"]).unwrap();
            camera.position = [0.0, 1.7, 0.0];
            camera.yaw = 0.0;
            camera.pitch = 0.0;
            camera.roll = 0.0;
            camera.view_fov_degrees = Some(60.0);
            let body = w.get_mut::<PhysicsBody>(ids["player"]).unwrap();
            body.radius = 0.25;
            body.capsule_half_height = 0.6;
            body.collision_offset_y = -0.85;
            body.grounded = true;
            let mut animation = w
                .get_dynamic(ids["first_person"], "WeaponAnimation")
                .unwrap()
                .clone();
            animation["idle_enabled"] = json!(false);
            animation["movement_strength"] = json!(0.0);
            animation["recoil_strength"] = json!(0.0);
            w.set_dynamic(ids["first_person"], "WeaponAnimation", animation)
                .unwrap();
        }
        let lua = LuaRuntime::new(Bridge(world.clone()), ScriptOptions::default()).unwrap();
        for script in [
            "hitboxes.lua",
            "aim_stability.lua",
            "first_person_controller.lua",
            "ballistics.lua",
        ] {
            lua.execute(
                &std::fs::read_to_string(root.join("assets/scripts").join(script)).unwrap(),
            )
            .unwrap();
        }
        Self { world, lua, ids }
    }
    fn tick(&self, n: usize) {
        for _ in 0..n {
            self.lua.run_systems(1.0 / 60.0).unwrap();
        }
    }
    fn events(&self, name: &str) -> Vec<Value> {
        self.world.borrow_mut().drain_events(name)
    }
    fn dispatch(&self, name: &str, value: Value) {
        self.lua.dispatch(name, &value).unwrap();
    }
    fn mouse(&self, button: &str, pressed: bool) {
        self.dispatch(
            "input.mouse_button",
            json!({"button":button,"pressed":pressed,"gameplay":true}),
        );
    }
    fn key(&self, code: &str, pressed: bool) {
        self.dispatch(
            "input.key",
            json!({"code":code,"pressed":pressed,"gameplay":true,"is_repeat":false}),
        );
    }
    fn state(&self) -> Value {
        self.world
            .borrow()
            .get_dynamic(self.ids["first_person"], "AimStabilityState")
            .unwrap()
            .clone()
    }
    fn hostile_shot(&self, x: f64, z: f64, speed: f64) {
        let (attacker, weapon) = {
            let mut w = self.world.borrow_mut();
            let attacker = w.spawn();
            let weapon = w.spawn();
            w.set_dynamic(
                weapon,
                "Ballistics",
                json!({
                    "speed_mps":speed,"gravity_mps2":0.0,"max_range_m":100.0,
                    "max_lifetime_s":3.0,"damage":35.0,"show_tracers":false
                }),
            )
            .unwrap();
            (attacker, weapon)
        };
        self.dispatch(
            "weapon.shot",
            json!({
                "entity":weapon.to_string(),"player":attacker.to_string(),"shot":5,
                "origin":[x,1.25,z],"direction":[0.0,0.0,-1.0]
            }),
        );
    }
}

#[test]
fn walking_crouching_and_blocked_motion_change_stability() {
    let f = Fixture::new();
    f.mouse("right", true);
    f.tick(120);
    let idle = f.state()["sway_degrees"].as_f64().unwrap();
    f.world
        .borrow_mut()
        .set_dynamic_resource("voxel.capture_movement", json!(true));
    let distance = {
        let w = f.world.borrow();
        w.get::<PlayerControls>(f.ids["player"]).unwrap().move_speed
            * w.get::<PlayerRig>(f.ids["player"]).unwrap().units_per_meter
            / 60.0
    };
    f.key("KeyW", true);
    for _ in 0..120 {
        f.dispatch(
            "physics.motion_resolved",
            json!({"entity":f.ids["player"].to_string(),"moved":[0.0,0.0,-distance]}),
        );
        f.tick(1);
    }
    let walking = f.state();
    assert!(
        walking["movement"].as_f64().unwrap() > 0.8,
        "walking state: {walking}"
    );
    assert!(walking["sway_degrees"].as_f64().unwrap() > idle * 1.6);
    f.dispatch("weapon.posture", json!({"crouch":1.0,"moving":true}));
    for _ in 0..120 {
        f.dispatch(
            "physics.motion_resolved",
            json!({"entity":f.ids["player"].to_string(),"moved":[0.0,0.0,-distance]}),
        );
        f.tick(1);
    }
    let crouched = f.state();
    assert!(crouched["crouch"].as_f64().unwrap() > 0.95);
    assert!(
        crouched["sway_degrees"].as_f64().unwrap()
            < walking["sway_degrees"].as_f64().unwrap() * 0.75
    );
    f.tick(150);
    assert!(
        f.state()["movement"].as_f64().unwrap() < 0.01,
        "holding a movement key against a wall must settle stability"
    );
}

#[test]
fn burst_and_full_auto_build_distinct_instability_and_recover() {
    let f = Fixture::new();
    f.mouse("right", true);
    f.tick(60);
    f.dispatch("weapon.mode", json!({}));
    f.mouse("left", true);
    f.mouse("left", false);
    f.tick(35);
    let burst = f.events("weapon.shot");
    assert_eq!(burst.len(), 3);
    let burst_heat = f.state()["heat"].as_f64().unwrap();
    assert!(burst_heat > 0.05 && burst_heat < 0.5);
    f.tick(240);
    assert!(f.state()["heat"].as_f64().unwrap() < 0.02);
    f.dispatch("weapon.mode", json!({}));
    f.mouse("left", true);
    f.tick(70);
    f.mouse("left", false);
    let auto = f.events("weapon.shot");
    assert!(auto.len() >= 8);
    assert!(f.state()["heat"].as_f64().unwrap() > burst_heat + 0.3);
    let xs: Vec<f64> = auto
        .iter()
        .map(|s| s["direction"][0].as_f64().unwrap())
        .collect();
    let spread = xs.iter().copied().fold(f64::NEG_INFINITY, f64::max)
        - xs.iter().copied().fold(f64::INFINITY, f64::min);
    assert!(
        spread > 0.0001,
        "automatic sway must affect real shot directions"
    );
    f.tick(300);
    assert!(f.state()["heat"].as_f64().unwrap() < 0.02);
}

#[test]
fn hostile_pressure_and_damage_increase_sway_and_then_recover() {
    let f = Fixture::new();
    f.tick(120);
    let idle = f.state()["sway_degrees"].as_f64().unwrap();
    f.dispatch(
        "combat.near_miss",
        json!({"target":"other","intensity":1.0}),
    );
    f.dispatch("weapon.hit", json!({"entity":"other-weapon","near":false}));
    f.dispatch("combat.near_miss",json!({"target":f.ids["player"].to_string(),"owner":f.ids["player"].to_string(),"intensity":1.0}));
    f.tick(12);
    assert_eq!(f.state()["under_fire"], 0.0);
    f.dispatch(
        "combat.near_miss",
        json!({"target":f.ids["player"].to_string(),"owner":"hostile","intensity":0.8}),
    );
    f.tick(18);
    let pressure = f.state();
    assert!(pressure["under_fire"].as_f64().unwrap() > 0.2);
    assert!(pressure["sway_degrees"].as_f64().unwrap() > idle + 0.1);
    f.dispatch(
        "combat.damage",
        json!({"target":f.ids["player"].to_string(),"amount":35.0}),
    );
    f.tick(6);
    assert!(f.state()["under_fire"].as_f64().unwrap() > pressure["under_fire"].as_f64().unwrap());
    f.tick(600);
    assert!(f.state()["under_fire"].as_f64().unwrap() < 0.01);
}

#[test]
fn stability_offsets_are_the_same_sight_used_to_launch_an_ads_shot() {
    for fov in [60.0_f32, 25.0, 0.2] {
        let f = Fixture::new();
        f.world
            .borrow_mut()
            .get_mut::<PlayerCamera>(f.ids["player"])
            .unwrap()
            .view_fov_degrees = Some(fov);
        f.mouse("right", true);
        f.tick(90);
        f.dispatch(
            "combat.suppression",
            json!({"target":f.ids["player"].to_string(),"owner":"hostile","intensity":1.0}),
        );
        f.tick(12);
        let frame = f.events("first_person.frame").pop().unwrap();
        f.mouse("left", true);
        let shot = f
            .events("weapon.shot")
            .pop()
            .expect("ADS shot should be immediate");
        let mx = frame["motion_x"].as_f64().unwrap();
        let my = frame["motion_y"].as_f64().unwrap();
        let roll = frame["motion_roll"].as_f64().unwrap();
        let y = 361.0 / 720.0 + 0.055 - 1.0;
        let frame_x = -roll.sin() * y + 0.5 + mx;
        let frame_y = roll.cos() * y + 1.0 + my;
        let screen_y = 1.0 + (frame_y - 1.0) * 0.75;
        let tangent = ((fov as f64 / 2.0).to_radians()).tan();
        let sx = (frame_x - 0.5) * 2.0 * (4.0 / 3.0) * tangent;
        let sy = (0.5 - screen_y) * 2.0 * tangent;
        let length = (1.0 + sx * sx + sy * sy).sqrt();
        for (i, expected) in [sx / length, sy / length, -1.0 / length].iter().enumerate() {
            assert!(
                (shot["direction"][i].as_f64().unwrap() - expected).abs() < 0.00005,
                "shot direction must follow the rendered sight"
            );
        }
    }
}

#[test]
fn hostile_near_miss_emits_once_and_drives_the_player_response() {
    let f = Fixture::new();
    f.hostile_shot(0.8, 3.0, 10.0);
    let mut near = Vec::new();
    for _ in 0..60 {
        f.tick(1);
        near.extend(f.events("combat.near_miss"));
    }
    assert_eq!(near.len(), 1);
    assert_eq!(near[0]["target"], f.ids["player"].to_string());
    assert!(near[0]["intensity"].as_f64().unwrap() > 0.0);
    f.dispatch("combat.near_miss", near.remove(0));
    f.tick(12);
    assert!(f.state()["under_fire"].as_f64().unwrap() > 0.0);
}

#[test]
fn walls_and_own_shots_do_not_generate_near_misses() {
    let f = Fixture::new();
    f.world
        .borrow_mut()
        .set_dynamic_resource("voxel.capture_movement", json!(true));
    f.world
        .borrow_mut()
        .get_mut::<VoxelColliderState>(f.ids["active_level"])
        .unwrap()
        .status = VoxelStatus::Ready;
    f.hostile_shot(0.8, 3.0, 100.0);
    f.tick(1);
    let request = f.events("physics.trace_request").pop().unwrap();
    f.dispatch(
        "physics.trace_result",
        json!({
            "entity":request["entity"],"token":request["token"],"status":"hit",
            "fraction":0.1,"position":[0.8,1.25,2.833333333],"normal":[0,0,1]
        }),
    );
    assert!(f.events("combat.near_miss").is_empty());

    let g = Fixture::new();
    g.dispatch(
        "weapon.shot",
        json!({
            "entity":g.ids["first_person"].to_string(),"player":g.ids["player"].to_string(),
            "shot":7,"origin":[0.8,1.25,3.0],"direction":[0.0,0.0,-1.0]
        }),
    );
    g.tick(60);
    assert!(g.events("combat.near_miss").is_empty());
}

#[test]
fn a_real_hostile_hit_drives_damage_flinch_without_a_duplicate_near_miss() {
    let f = Fixture::new();
    f.hostile_shot(0.0, 3.0, 10.0);
    let mut damage = Vec::new();
    let mut near = Vec::new();
    for _ in 0..60 {
        f.tick(1);
        damage.extend(f.events("combat.damage"));
        near.extend(f.events("combat.near_miss"));
    }
    assert_eq!(damage.len(), 1);
    assert_eq!(damage[0]["target"], f.ids["player"].to_string());
    assert!(
        near.is_empty(),
        "a direct hit should not also report a near miss"
    );
    f.dispatch("combat.damage", damage.remove(0));
    f.tick(12);
    assert!(f.state()["under_fire"].as_f64().unwrap() > 0.0);
}

#[test]
fn stability_can_be_disabled_without_disabling_reusable_near_miss_events() {
    let f = Fixture::new();
    let id = f.ids["first_person"];
    let mut cfg = f
        .world
        .borrow()
        .get_dynamic(id, "AimStability")
        .unwrap()
        .clone();
    cfg["enabled"] = json!(false);
    f.world
        .borrow_mut()
        .set_dynamic(id, "AimStability", cfg.clone())
        .unwrap();
    f.tick(12);
    assert_eq!(f.state()["sway_degrees"], 0.0);
    f.hostile_shot(0.8, 3.0, 10.0);
    let mut events = Vec::new();
    for _ in 0..60 {
        f.tick(1);
        events.extend(f.events("combat.near_miss"));
    }
    assert_eq!(
        events.len(),
        1,
        "near-miss events are independent of the weapon modifier"
    );
    f.dispatch("combat.near_miss", events.remove(0));
    f.tick(12);
    assert_eq!(f.state()["under_fire"], 0.0);
    cfg["enabled"] = json!(true);
    f.world
        .borrow_mut()
        .set_dynamic(id, "AimStability", cfg)
        .unwrap();
    f.tick(60);
    assert!(f.state()["sway_degrees"].as_f64().unwrap() > 0.0);
}

#[test]
fn stability_envelopes_are_consistent_across_tick_rates() {
    let f = Fixture::new();
    f.lua
        .execute(
            r#"
        local ids=ecs.get_resource('scene.entities')
        local function sample(rate)
            local m=engine.aim_stability.new(ids.first_person,ids.player)
            m:under_fire(.8,false)
            for i=1,rate*2 do m:update(1/rate,1,.6,true) end
            return m
        end
        local a,b=sample(30),sample(144)
        assert(math.abs(a.amplitude-b.amplitude)<.005)
        assert(math.abs(a.pressure-b.pressure)<.0001)
        assert(math.abs(a.movement-b.movement)<.0001)
    "#,
        )
        .unwrap();
}

#[test]
fn delayed_collision_uses_the_player_pose_at_segment_submission() {
    let f = Fixture::new();
    f.world
        .borrow_mut()
        .set_dynamic_resource("voxel.capture_movement", json!(true));
    f.world
        .borrow_mut()
        .get_mut::<VoxelColliderState>(f.ids["active_level"])
        .unwrap()
        .status = VoxelStatus::Ready;
    f.hostile_shot(0.8, 3.0, 100.0);
    f.lua.run_systems(0.1).unwrap();
    let request = f.events("physics.trace_request").pop().unwrap();
    f.world
        .borrow_mut()
        .get_mut::<PlayerCamera>(f.ids["player"])
        .unwrap()
        .position[0] = 100.0;
    f.dispatch(
        "physics.trace_result",
        json!({
            "entity":request["entity"],"token":request["token"],"status":"miss"
        }),
    );
    assert_eq!(
        f.events("combat.near_miss").len(),
        1,
        "a late worker result must use the pose at the checked segment"
    );
}

#[test]
fn stance_changes_sustained_fire_and_incoming_flinch() {
    let f = Fixture::new();
    f.lua
        .execute(
            r#"
        local ids=ecs.get_resource('scene.entities')
        local function sample(speed,crouch)
            local m=engine.aim_stability.new(ids.first_person,ids.player)
            for i=1,180 do m:update(1/60,speed,crouch,true) end
            m:under_fire(1,true)
            local kick=m.kick_y
            local recoil=0
            for i=1,5 do recoil=m:shot('auto'); m:update(.1,speed,crouch,true) end
            return m.amplitude,recoil,kick
        end
        local standing,sr,sk=sample(0,0)
        local walking,wr,wk=sample(1,0)
        local crouched,cr,ck=sample(0,1)
        assert(walking>standing and wr>sr and wk>sk)
        assert(crouched<standing and cr<sr and ck<sk)
    "#,
        )
        .unwrap();
}

#[test]
fn partial_components_receive_safe_defaults_without_reenabling_toggles() {
    let f = Fixture::new();
    {
        let mut w = f.world.borrow_mut();
        w.set_dynamic(
            f.ids["first_person"],
            "AimStability",
            json!({"enabled":false,"walk_multiplier":99.0}),
        )
        .unwrap();
        w.set_dynamic(
            f.ids["player"],
            "SuppressionReceiver",
            json!({"enabled":false}),
        )
        .unwrap();
    }
    f.lua
        .execute(
            r#"
        local ids=ecs.get_resource('scene.entities')
        local m=assert(engine.aim_stability.new(ids.first_person,ids.player))
        local c=ecs.get(ids.first_person,'AimStability')
        local r=ecs.get(ids.player,'SuppressionReceiver')
        assert(c.enabled==false and c.walk_multiplier==5 and c.auto_heat_per_shot==.18)
        assert(r.enabled==false and r.near_miss_radius_m==1.25)
        local x,y=m:update(.1,1,0,true)
        assert(x==0 and y==0)
    "#,
        )
        .unwrap();
}
