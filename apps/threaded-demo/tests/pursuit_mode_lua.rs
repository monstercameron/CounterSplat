use countersplat_ecs::{Entity, World, WorldOperation, WorldOutcome};
use countersplat_scene::{
    PhysicsBody, PlayerCamera, SceneManifest, Transform, VoxelColliderState, VoxelStatus,
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
    fn new(collision: bool) -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut manifest = SceneManifest::from_json(
            &std::fs::read(root.join("assets/scenes/gameplay.json")).unwrap(),
        )
        .unwrap();
        // Damage and health rules are tested against the authored 35-damage ballistics, not the shipped weapon
        // profile.
        for entity in &mut manifest.entities {
            entity.components.remove("WeaponProfile");
        }
        let world = Rc::new(RefCell::new(World::new()));
        countersplat_ui_ecs::register(&mut world.borrow_mut()).unwrap();
        countersplat_scene_ecs::register(&mut world.borrow_mut()).unwrap();
        let ids = countersplat_scene_ecs::instantiate_layers(
            &mut world.borrow_mut(),
            &manifest,
            &["ui", "player", "level", "scene"],
        )
        .unwrap();
        {
            let mut w = world.borrow_mut();
            w.set_dynamic_resource("scene.level", json!({"id":"58d333ae"}));
            w.set_dynamic_resource("voxel.capture_movement", json!(collision));
            w.set_dynamic_resource(
                "render.viewport",
                json!({"width":1050,"height":720,"scene_aspect":1050.0/720.0}),
            );
            w.get_mut::<VoxelColliderState>(ids["active_level"])
                .unwrap()
                .status = if collision {
                VoxelStatus::Ready
            } else {
                VoxelStatus::Unavailable
            };
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
            "pursuit_mode.lua",
            "ballistics.lua",
        ] {
            lua.execute(
                &std::fs::read_to_string(root.join("assets/scripts").join(script)).unwrap(),
            )
            .unwrap();
        }
        Self { world, lua, ids }
    }
    fn events(&self, name: &str) -> Vec<Value> {
        self.world.borrow_mut().drain_events(name)
    }
    fn dispatch_events(&self, name: &str) {
        for event in self.events(name) {
            self.lua.dispatch(name, &event).unwrap();
        }
    }
    fn start(&self) {
        self.lua
            .dispatch(
                "game.mode_request",
                &json!({"mode":"pursuit","action":"start"}),
            )
            .unwrap();
        self.dispatch_events("combat.reset");
        self.dispatch_events("weapon.reset");
    }
    fn step(&self, dt: f64) {
        self.lua.run_systems(dt).unwrap();
        for name in [
            "weapon.shot",
            "projectile.impact",
            "combat.damage",
            "combat.death",
            "weapon.die",
        ] {
            self.dispatch_events(name);
        }
    }
    fn state(&self) -> Value {
        self.world
            .borrow()
            .get_dynamic_resource("game.mode_state")
            .unwrap()
            .clone()
    }
    fn targets(&self) -> Vec<Entity> {
        let w = self.world.borrow();
        w.entities()
            .into_iter()
            .filter(|id| w.get_dynamic(*id, "PursuitTarget").is_some())
            .collect()
    }
    fn shoot(&self, shot: u32) {
        self.lua.dispatch("weapon.shot",&json!({"entity":self.ids["first_person"].to_string(),
            "player":self.ids["player"].to_string(),"shot":shot,"origin":[0,1.68,0],"direction":[0,0,-1]})).unwrap();
        self.step(0.05);
    }
}

#[test]
fn targets_spawn_chase_take_zone_damage_and_die_from_projectiles() {
    let f = Fixture::new(false);
    f.start();
    f.step(0.05);
    let targets = f.targets();
    assert_eq!(targets.len(), 1);
    let target = targets[0];
    let initial_z = f.world.borrow().get::<Transform>(target).unwrap().position[2];
    let initial = f
        .events("hitbox.visuals")
        .into_iter()
        .rev()
        .find(|v| v["layer"] == "pursuit")
        .unwrap();
    assert_eq!(initial["boxes"].as_array().unwrap().len(), 9);
    assert_eq!(f.world.borrow().parent(target), Some(f.ids["pursuit_mode"]));
    f.shoot(1);
    {
        let w = f.world.borrow();
        assert_eq!(w.get_dynamic(target, "Health").unwrap()["current"], 65.0);
        assert_eq!(
            w.get_dynamic(target, "PursuitTarget").unwrap()["zone_damage"]["head"],
            35.0
        );
        assert!(w.get::<Transform>(target).unwrap().position[2] > initial_z);
    }
    f.step(0.05);
    let red = f
        .events("hitbox.visuals")
        .into_iter()
        .rev()
        .find(|v| v["layer"] == "pursuit")
        .unwrap();
    let head = red["boxes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|line| line["zone"] == "head")
        .unwrap();
    assert!(head["color"][1].as_f64().unwrap() < 0.5);
    let chest = red["boxes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|line| line["zone"] == "chest")
        .unwrap();
    assert!((chest["color"][1].as_f64().unwrap() - 0.77).abs() < 0.001);
    assert_eq!(f.state()["shots"], 1);
    assert_eq!(f.state()["hits"], 1);
    f.shoot(2);
    f.shoot(3);
    assert!(f.targets().is_empty(), "a killed target is removed");
    assert_eq!(f.state()["kills"], 1);
    assert_eq!(f.state()["shots"], 3);
    assert_eq!(f.state()["hits"], 3);
    for _ in 0..10 {
        f.step(0.05);
    }
    assert_eq!(f.targets().len(), 1, "a killed target is replaced");
    for _ in 0..180 {
        f.step(0.05);
    }
    assert!(f.targets().len() <= 4, "configured target cap is respected");
}

#[test]
fn reset_clears_status_and_targets_and_restores_player() {
    let f = Fixture::new(false);
    f.start();
    f.step(0.05);
    let target = f.targets()[0];
    {
        let mut w = f.world.borrow_mut();
        w.set_dynamic(
            f.ids["player"],
            "Health",
            json!({"current":1,"maximum":100}),
        )
        .unwrap();
        w.set_dynamic(f.ids["player"], "Armor", json!({"current":1,"maximum":100}))
            .unwrap();
    }
    f.lua
        .dispatch(
            "game.mode_request",
            &json!({"mode":"pursuit","action":"reset"}),
        )
        .unwrap();
    f.dispatch_events("combat.reset");
    f.dispatch_events("weapon.reset");
    assert!(f.targets().is_empty());
    assert!(!f.world.borrow().is_alive(target));
    assert_eq!(f.state()["status"], "ready");
    assert_eq!(f.state()["kills"], 0);
    assert_eq!(
        f.world
            .borrow()
            .get_dynamic(f.ids["player"], "Health")
            .unwrap()["current"],
        100
    );
    assert_eq!(
        f.world
            .borrow()
            .get_dynamic(f.ids["player"], "Armor")
            .unwrap()["current"],
        100
    );
    f.start();
    f.step(0.05);
    assert_eq!(f.targets().len(), 1);
    assert_eq!(f.state()["spawned"], 1);
}

#[test]
fn spawn_probes_ground_and_clearance_and_ignores_stale_replies() {
    let f = Fixture::new(true);
    f.start();
    f.step(0.05);
    let t = f.targets()[0];
    assert_eq!(
        f.world.borrow().get_dynamic(t, "Hitbox").unwrap()["enabled"],
        false
    );
    let ground = f
        .events("physics.ground_request")
        .into_iter()
        .find(|e| e["entity"] == t.to_string())
        .unwrap();
    f.lua
        .dispatch(
            "physics.ground_result",
            &json!({"entity":t.to_string(),"token":"old","height":7.0}),
        )
        .unwrap();
    assert_ne!(
        f.world.borrow().get::<Transform>(t).unwrap().position[1],
        7.0
    );
    f.lua
        .dispatch(
            "physics.ground_result",
            &json!({"entity":t.to_string(),"token":ground["token"],"height":0.0}),
        )
        .unwrap();
    let clear = f
        .events("physics.clearance_request")
        .into_iter()
        .find(|e| e["entity"] == t.to_string())
        .unwrap();
    f.lua
        .dispatch(
            "physics.clearance_result",
            &json!({"entity":t.to_string(),"token":clear["token"],"clear":true}),
        )
        .unwrap();
    assert_eq!(
        f.world.borrow().get_dynamic(t, "Hitbox").unwrap()["enabled"],
        true
    );
    assert_eq!(f.state()["alive"], 1);
    f.lua
        .dispatch(
            "game.mode_request",
            &json!({"mode":"pursuit","action":"reset"}),
        )
        .unwrap();
    f.lua
        .dispatch(
            "physics.clearance_result",
            &json!({"entity":t.to_string(),"token":clear["token"],"clear":true}),
        )
        .unwrap();
    assert!(f.targets().is_empty());
}

#[test]
fn target_can_defeat_player_and_reset_revives_them() {
    let f = Fixture::new(false);
    {
        let mut w = f.world.borrow_mut();
        let mut cfg = w
            .get_dynamic(f.ids["pursuit_mode"], "PursuitConfig")
            .unwrap()
            .clone();
        cfg["attack_damage"] = json!(100);
        cfg["attack_interval_s"] = json!(0.2);
        w.set_dynamic(f.ids["pursuit_mode"], "PursuitConfig", cfg)
            .unwrap();
    }
    f.start();
    f.step(0.05);
    {
        let target = f.targets()[0];
        let mut w = f.world.borrow_mut();
        w.get_mut::<Transform>(target).unwrap().position = [0.0, 0.0, -0.5];
        w.set_dynamic(f.ids["player"], "Armor", json!({"current":0,"maximum":100}))
            .unwrap();
    }
    f.step(0.1);
    assert_eq!(f.state()["status"], "defeated");
    assert_eq!(
        f.world
            .borrow()
            .get_dynamic(f.ids["player"], "Health")
            .unwrap()["current"],
        0.0
    );
    f.lua
        .dispatch(
            "game.mode_request",
            &json!({"mode":"pursuit","action":"start"}),
        )
        .unwrap();
    f.dispatch_events("combat.reset");
    f.dispatch_events("weapon.reset");
    assert_eq!(
        f.world
            .borrow()
            .get_dynamic(f.ids["player"], "Health")
            .unwrap()["current"],
        100.0
    );
    assert_eq!(f.state()["status"], "running");
}

#[test]
fn attacks_respect_voxel_line_of_sight() {
    let f = Fixture::new(true);
    f.start();
    f.step(0.05);
    let t = f.targets()[0];
    let ground = f
        .events("physics.ground_request")
        .into_iter()
        .find(|e| e["entity"] == t.to_string())
        .unwrap();
    f.lua
        .dispatch(
            "physics.ground_result",
            &json!({"entity":t.to_string(),"token":ground["token"],"height":0.0}),
        )
        .unwrap();
    let clear = f
        .events("physics.clearance_request")
        .into_iter()
        .find(|e| e["entity"] == t.to_string())
        .unwrap();
    f.lua
        .dispatch(
            "physics.clearance_result",
            &json!({"entity":t.to_string(),"token":clear["token"],"clear":true}),
        )
        .unwrap();
    f.world
        .borrow_mut()
        .get_mut::<Transform>(t)
        .unwrap()
        .position = [0.0, 0.0, -0.5];
    f.step(0.05);
    let trace = f
        .events("physics.trace_request")
        .into_iter()
        .find(|e| e["entity"] == t.to_string())
        .unwrap();
    f.lua
        .dispatch(
            "physics.trace_result",
            &json!({"entity":t.to_string(),"token":trace["token"],"status":"hit"}),
        )
        .unwrap();
    assert_eq!(
        f.world
            .borrow()
            .get_dynamic(f.ids["player"], "Health")
            .unwrap()["current"],
        100.0
    );
    for _ in 0..30 {
        f.step(0.05);
    }
    let trace = f
        .events("physics.trace_request")
        .into_iter()
        .find(|e| e["entity"] == t.to_string())
        .unwrap();
    f.lua
        .dispatch(
            "physics.trace_result",
            &json!({"entity":t.to_string(),"token":trace["token"],"status":"miss"}),
        )
        .unwrap();
    assert!(
        f.world
            .borrow()
            .get_dynamic(f.ids["player"], "Health")
            .unwrap()["current"]
            .as_f64()
            .unwrap()
            < 100.0
    );
}
