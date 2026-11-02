//! Inspection presentation is ordinary Lua and ECS UI, driven by real scene data.
use countersplat_ecs::{Entity, World, WorldOperation, WorldOutcome};
use countersplat_scene::SceneManifest;
use countersplat_scripting_lua::{LuaRuntime, ScriptOptions, WorldBridge};
use countersplat_ui::{UiElement, Widget};
use serde_json::json;
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
            &["ui", "player"],
        )
        .unwrap();
        world.borrow_mut().set_dynamic_resource(
            "render.viewport",
            json!({"width":1050,"height":720,"scene_aspect":16.0/9.0,"scaling":"fit"}),
        );
        let lua = LuaRuntime::new(Bridge(world.clone()), ScriptOptions::default()).unwrap();
        lua.execute(
            &std::fs::read_to_string(root.join("assets/scripts/inspection_hud.lua")).unwrap(),
        )
        .unwrap();
        Self { world, lua, ids }
    }
    fn key(&self, gameplay: bool, repeat: bool) {
        self.lua
            .dispatch(
                "input.key",
                &json!({"code":"KeyQ","pressed":true,"is_repeat":repeat,"gameplay":gameplay}),
            )
            .unwrap();
    }
    fn tick(&self, dt: f64) {
        self.lua.run_systems(dt).unwrap();
    }
    fn visible(&self, id: &str) -> bool {
        self.world
            .borrow()
            .get::<UiElement>(self.ids[id])
            .unwrap()
            .visible
    }
    fn text(&self, id: &str) -> String {
        let w = self.world.borrow();
        match &w.get::<UiElement>(self.ids[id]).unwrap().widget {
            Widget::Text { text, .. } => text.clone(),
            _ => panic!("expected text"),
        }
    }
    fn leader(&self) -> Vec<[f32; 2]> {
        let w = self.world.borrow();
        match &w
            .get::<UiElement>(self.ids["inspect_leader"])
            .unwrap()
            .widget
        {
            Widget::Polyline { points, .. } => points.clone(),
            _ => panic!("expected line"),
        }
    }
    fn pose(&self, clip: &str, motion: f32) {
        self.world.borrow_mut().set_dynamic_resource("hud.weapon_pose",json!({"clip":clip,"index":1,"visible":true,"opacity":1,"vertical_offset":0.055,"motion_x":motion,"motion_y":0,"motion_roll":0}));
    }
}
#[test]
fn q_inspects_live_data_tracks_the_weapon_and_expires() {
    let f = Fixture::new();
    assert!(!f.visible("inspection_hud"));
    f.key(false, false);
    f.key(true, true);
    assert!(!f.visible("inspection_hud"));
    f.pose("lowReady", 0.0);
    f.key(true, false);
    f.tick(0.12);
    assert!(f.visible("inspection_hud"));
    assert!(f.visible("inspect_leader"));
    assert_eq!(f.text("inspect_rounds"), "30");
    assert_eq!(f.text("inspect_reserve"), "/ 90 reserve");
    assert_eq!(f.text("inspect_mode"), "Single");
    assert_eq!(f.text("inspect_health_value"), "100");
    assert_eq!(f.text("inspect_armor_value"), "100");
    let first = f.leader();
    f.pose("lowReady", 0.1);
    f.tick(0.05);
    assert!(f.leader()[0][0] > first[0][0]);
    {
        let mut w = f.world.borrow_mut();
        w.set_dynamic(
            f.ids["first_person"],
            "WeaponState",
            json!({"ammo":17,"reserve":60,"mode":"auto"}),
        )
        .unwrap();
        w.set_dynamic(
            f.ids["player"],
            "Health",
            json!({"current":73,"maximum":100}),
        )
        .unwrap();
        w.set_dynamic(
            f.ids["player"],
            "Armor",
            json!({"current":45,"maximum":100}),
        )
        .unwrap();
    }
    f.tick(0.05);
    assert_eq!(f.text("inspect_rounds"), "17");
    assert_eq!(f.text("inspect_mode"), "Auto");
    assert_eq!(f.text("inspect_health_value"), "73");
    assert_eq!(f.text("inspect_armor_value"), "45");
    f.pose("fireADS", 0.0);
    f.tick(0.05);
    assert!(!f.visible("inspect_leader"));
    for _ in 0..14 {
        f.tick(0.25);
    }
    assert!(!f.visible("inspection_hud"));
    f.key(true, false);
    assert!(f.visible("inspection_hud"));
    f.lua.dispatch("input.reset", &json!({})).unwrap();
    assert!(!f.visible("inspection_hud"));
}

#[test]
fn inspecting_does_not_move_the_player_and_emits_reusable_events() {
    let f = Fixture::new();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    f.lua
        .execute(&std::fs::read_to_string(root.join("assets/scripts/player_controls.lua")).unwrap())
        .unwrap();
    let player = f.ids["player"];
    let start = f
        .world
        .borrow()
        .get::<countersplat_scene::PlayerCamera>(player)
        .unwrap()
        .position;
    f.key(true, false);
    for _ in 0..5 {
        f.tick(0.05);
    }
    let end = f
        .world
        .borrow()
        .get::<countersplat_scene::PlayerCamera>(player)
        .unwrap()
        .position;
    assert_eq!(start, end, "inspection must not also move the player down");
    let events = f.world.borrow_mut().drain_events("engine.event");
    assert!(events.iter().any(|e| e["name"] == "hud.inspect_started"));
    assert!(events.iter().any(|e| e["name"] == "player.inspect"));
}

#[test]
fn control_rebinding_reserves_inspection_key_without_opening_the_hud() {
    let f = Fixture::new();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    f.lua
        .execute(&std::fs::read_to_string(root.join("assets/scripts/player_controls.lua")).unwrap())
        .unwrap();
    f.lua
        .dispatch(
            "ui.action",
            &json!({"id":f.ids["key_forward"].to_string(),"kind":{"type":"clicked"}}),
        )
        .unwrap();
    f.key(true, false);
    assert!(!f.visible("inspection_hud"));
    let w = f.world.borrow();
    assert_eq!(
        w.get::<countersplat_scene::PlayerControls>(f.ids["player"])
            .unwrap()
            .bindings
            .forward,
        "KeyW"
    );
    assert_eq!(
        w.get_dynamic_resource("input.binding_capture"),
        Some(&json!(true))
    );
    drop(w);
    f.lua.dispatch("input.key",&json!({"key":"ArrowUp","code":"ArrowUp","pressed":true,"is_repeat":false,"gameplay":true})).unwrap();
    assert_eq!(
        f.world
            .borrow()
            .get::<countersplat_scene::PlayerControls>(f.ids["player"])
            .unwrap()
            .bindings
            .forward,
        "ArrowUp"
    );
}

#[test]
fn inspection_callout_uses_the_actual_pixel_presentation_mapping() {
    let f = Fixture::new();
    f.world.borrow_mut().set_dynamic_resource(
        "render.viewport",
        json!({"width":1000,"height":600,
        "scene_aspect":16.0/9.0,"scaling":"pixel","uv_scale":[1.25,1.5],"uv_offset":[0.001,0.002]}),
    );
    f.pose("lowReady", 0.0);
    f.key(true, false);
    f.tick(0.12);
    let point = f.leader()[0];
    let expected_x = ((715.0_f32 / 1280.0 - 0.5 - 0.001) / 1.25 + 0.5) * 1000.0;
    let expected_y = ((635.0_f32 / 720.0 + 0.055 - 0.5 - 0.002) / 1.5 + 0.5) * 600.0;
    assert!((point[0] - expected_x).abs() < 0.01, "x = {}", point[0]);
    assert!((point[1] - expected_y).abs() < 0.01, "y = {}", point[1]);
}

#[test]
fn magazine_callout_follows_presented_frames_and_blends() {
    let f = Fixture::new();
    f.pose("lowReady", 0.0);
    f.key(true, false);
    f.tick(0.12);
    let pose = |clip: &str, index: i32, blend: f32, previous: serde_json::Value| {
        json!({
            "clip":clip,"index":index,"visible":true,"opacity":1.0,"vertical_offset":0.055,
            "motion_x":0.0,"motion_y":0.0,"motion_roll":0.0,"blend":blend,"previous":previous
        })
    };
    f.lua
        .dispatch(
            "hud.weapon_presentation",
            &pose("lowReady", 1, 1.0, json!(null)),
        )
        .unwrap();
    let low = f.leader()[0];
    let w = f.world.borrow();
    let text = &w.get::<UiElement>(f.ids["inspect_rounds"]).unwrap().rect;
    let card = &w.get::<UiElement>(f.ids["inspect_card"]).unwrap().rect;
    let points = f.leader();
    assert!(points[2][0] < points[1][0] && points[1][0] < points[0][0]);
    assert!(
        (points[2][1] - points[1][1]).abs() < 0.01,
        "the number has a horizontal underline"
    );
    assert!(
        text.x > card.x + card.width,
        "the ammo number is separate from the status panel"
    );
    drop(w);
    f.lua
        .dispatch(
            "hud.weapon_presentation",
            &pose("crouchEnterLow", 11, 1.0, json!(null)),
        )
        .unwrap();
    let crouch = f.leader()[0];
    assert!(
        crouch[1] < low[1] - 70.0,
        "the anchor follows the moving filmed magazine"
    );
    // A newer requested pose cannot override the compositor's actual frame.
    f.pose("lowReady", 0.1);
    f.tick(0.05);
    assert_eq!(f.leader()[0], crouch);
    f.lua
        .dispatch(
            "hud.weapon_presentation",
            &pose(
                "crouchEnterLow",
                11,
                0.5,
                json!({"clip":"lowReady","index":1}),
            ),
        )
        .unwrap();
    let blend = f.leader()[0];
    assert!(
        (blend[1] - (low[1] + crouch[1]) * 0.5).abs() < 0.05,
        "the marker follows a clip blend"
    );
    f.lua
        .dispatch(
            "hud.weapon_presentation",
            &pose("fireADS", 1, 1.0, json!(null)),
        )
        .unwrap();
    assert!(!f.visible("inspect_leader"));
    assert!(!f.visible("inspect_target_dot"));
    assert!(
        f.visible("inspect_rounds"),
        "ammo remains available when the magazine is hidden"
    );
}

#[test]
fn combat_dismisses_inspection_until_a_fresh_q_press() {
    for button in ["left", "right", "middle"] {
        let f = Fixture::new();
        f.key(true, false);
        assert!(f.visible("inspection_hud"));
        f.lua
            .dispatch(
                "input.mouse_button",
                &json!({"button":button,"pressed":true,"gameplay":false}),
            )
            .unwrap();
        assert!(f.visible("inspection_hud"), "UI clicks are not combat");
        f.lua
            .dispatch(
                "input.mouse_button",
                &json!({"button":button,"pressed":true,"gameplay":true}),
            )
            .unwrap();
        assert!(
            !f.visible("inspection_hud"),
            "{button} should dismiss inspection"
        );
        f.key(true, false);
        assert!(
            !f.visible("inspection_hud"),
            "Q cannot reopen while {button} is held"
        );
        f.lua
            .dispatch(
                "input.mouse_button",
                &json!({"button":button,"pressed":false,"gameplay":false}),
            )
            .unwrap();
        f.tick(0.1);
        assert!(
            !f.visible("inspection_hud"),
            "release must not reopen inspection"
        );
        f.key(true, false);
        assert!(f.visible("inspection_hud"));
    }
    for code in [
        "KeyR", "KeyV", "KeyF", "KeyG", "Digit1", "Digit2", "Digit3", "Digit4",
    ] {
        let f = Fixture::new();
        f.key(true, false);
        f.lua
            .dispatch(
                "input.key",
                &json!({"code":code,"pressed":true,"gameplay":true,"is_repeat":false}),
            )
            .unwrap();
        assert!(
            !f.visible("inspection_hud"),
            "{code} should dismiss inspection"
        );
        if code == "KeyR" {
            f.key(true, false);
            assert!(
                !f.visible("inspection_hud"),
                "Q cannot reopen while reload is held"
            );
            f.lua
                .dispatch(
                    "input.key",
                    &json!({"code":code,"pressed":false,"gameplay":true}),
                )
                .unwrap();
        }
        f.key(true, false);
        assert!(f.visible("inspection_hud"), "Q reopens after {code}");
    }
}

#[test]
fn scripted_player_combat_dismisses_inspection_but_other_entities_do_not() {
    let f = Fixture::new();
    f.key(true, false);
    f.lua
        .dispatch("weapon.shot", &json!({"entity":"someone-else"}))
        .unwrap();
    f.lua
        .dispatch("combat.damage", &json!({"target":"someone-else"}))
        .unwrap();
    assert!(f.visible("inspection_hud"));
    f.lua
        .dispatch(
            "combat.damage",
            &json!({"target":f.ids["player"].to_string()}),
        )
        .unwrap();
    assert!(!f.visible("inspection_hud"));
    f.key(true, false);
    f.lua
        .dispatch(
            "weapon.trigger",
            &json!({"entity":f.ids["first_person"].to_string(),"pressed":true}),
        )
        .unwrap();
    assert!(!f.visible("inspection_hud"));
    f.key(true, false);
    assert!(!f.visible("inspection_hud"));
    f.lua
        .dispatch(
            "weapon.trigger",
            &json!({"entity":f.ids["first_person"].to_string(),"pressed":false}),
        )
        .unwrap();
    f.key(true, false);
    assert!(f.visible("inspection_hud"));
    f.lua
        .dispatch(
            "engine.event",
            &json!({"name":"combat.action","payload":{"player":f.ids["player"].to_string()}}),
        )
        .unwrap();
    assert!(!f.visible("inspection_hud"));
    f.tick(0.2);
    assert!(!f.visible("inspection_hud"));
    f.key(true, false);
    assert!(f.visible("inspection_hud"));
}

#[test]
fn stats_and_equipment_use_live_or_explicitly_optional_data() {
    let f = Fixture::new();
    f.key(true, false);
    f.tick(0.1);
    assert!(f.visible("inspect_compass_ring"));
    {
        let w = f.world.borrow();
        let compass = w.get::<UiElement>(f.ids["inspect_compass_ring"]).unwrap();
        let speed = w.get::<UiElement>(f.ids["inspect_speed_track"]).unwrap();
        let slot = w.get::<UiElement>(f.ids["inspect_slot4_box"]).unwrap();
        let image_height = 1050.0 / (16.0 / 9.0);
        let top = (720.0 - image_height) / 2.0;
        let bottom = 720.0 - top;
        assert!(
            compass.rect.y >= top,
            "compass must avoid the top letterbox"
        );
        assert!(
            speed.rect.y + speed.rect.height <= bottom,
            "stats must avoid the bottom letterbox"
        );
        assert!(
            slot.rect.y + slot.rect.height <= bottom,
            "equipment must avoid the bottom letterbox"
        );
    }
    assert_eq!(f.text("inspect_health_value"), "100");
    assert_eq!(f.text("inspect_armor_value"), "100");
    assert!(
        !f.visible("inspect_stamina_icon"),
        "unavailable stamina must not be invented"
    );
    assert!(
        !f.visible("inspect_objective_title"),
        "unavailable objectives must not be invented"
    );
    assert_eq!(f.text("inspect_slot1_name"), "Rifle");
    assert_eq!(f.text("inspect_slot2_name"), "Empty");
    {
        let mut w = f.world.borrow_mut();
        w.set_dynamic(
            f.ids["player"],
            "Stamina",
            json!({"current":40,"maximum":80}),
        )
        .unwrap();
        w.set_dynamic(f.ids["player"],"Inventory",json!({"active_slot":2,"slots":[
            {"name":"Rifle","kind":"rifle"},{"name":"Sidearm","kind":"sidearm"},{"name":"Grenade","kind":"throwable","count":2}]})).unwrap();
        w.set_dynamic_resource(
            "scene.objective",
            json!({"title":"Find the exit","detail":"Reach the doorway"}),
        );
        let camera = w
            .get_mut::<countersplat_scene::PlayerCamera>(f.ids["player"])
            .unwrap();
        camera.position[0] += 0.3;
        camera.yaw = std::f32::consts::FRAC_PI_2;
    }
    f.tick(0.1);
    assert!(f.visible("inspect_stamina_icon"));
    assert_eq!(f.text("inspect_stamina_value"), "40");
    assert_eq!(f.text("inspect_slot2_name"), "Sidearm");
    assert_eq!(f.text("inspect_slot3_name"), "Grenade  2");
    assert_eq!(f.text("inspect_objective_title"), "Find the exit");
    assert!(f.text("inspect_speed_value").parse::<f32>().unwrap() > 0.0);
    let w = f.world.borrow();
    let mode = w.get::<UiElement>(f.ids["inspect_mode"]).unwrap();
    let selected = w.get::<UiElement>(f.ids["inspect_slot2_box"]).unwrap();
    assert!(
        mode.visible
            && mode.rect.y >= selected.rect.y
            && mode.rect.y < selected.rect.y + selected.rect.height
    );
    let track = w.get::<UiElement>(f.ids["inspect_stamina_track"]).unwrap();
    let fill = w.get::<UiElement>(f.ids["inspect_stamina_fill"]).unwrap();
    assert!((fill.rect.width / track.rect.width - 0.5).abs() < 0.001);
    let compass = w.get::<UiElement>(f.ids["inspect_compass_ring"]).unwrap();
    let north = w.get::<UiElement>(f.ids["inspect_compass_north"]).unwrap();
    assert!(
        north.rect.x < compass.rect.x + compass.rect.width * 0.2,
        "north moves left when looking east"
    );
}

#[test]
fn ammo_callout_is_contextual_for_fire_ads_and_inspection() {
    let f = Fixture::new();
    f.pose("lowReady", 0.0);
    f.tick(1.0 / 60.0);
    assert!(
        !f.visible("inspection_hud") && !f.visible("ammo_hud"),
        "ammo starts hidden at rest"
    );
    {
        let world = f.world.borrow();
        assert_eq!(
            world.parent(f.ids["inspect_rounds"]),
            Some(f.ids["ammo_hud"])
        );
        let frame = countersplat_ui_ecs::extract(
            &world,
            countersplat_ui::Rect::new(0.0, 0.0, 1050.0, 720.0),
        )
        .unwrap();
        assert!(
            !frame
                .nodes
                .iter()
                .any(|node| node.id == f.ids["inspect_rounds"].to_string()),
            "hidden ammo must not be rendered"
        );
    }
    f.lua
        .dispatch(
            "input.mouse_button",
            &json!({"button":"left","pressed":true,"gameplay":false}),
        )
        .unwrap();
    f.lua
        .dispatch("weapon.shot", &json!({"entity":"another-weapon"}))
        .unwrap();
    assert!(
        !f.visible("ammo_hud"),
        "UI clicks and another entity's shot cannot show ammo"
    );

    f.lua
        .dispatch(
            "input.mouse_button",
            &json!({"button":"right","pressed":true,"gameplay":true}),
        )
        .unwrap();
    f.tick(0.05);
    assert!(
        f.visible("ammo_hud") && f.visible("inspect_rounds"),
        "ADS shows the count"
    );
    assert_eq!(f.text("inspect_rounds"), "30");
    for _ in 0..8 {
        f.tick(0.25);
    }
    assert!(f.visible("ammo_hud"), "ADS stays visible without firing");
    f.lua
        .dispatch(
            "hud.weapon_presentation",
            &json!({"clip":"fireADS","index":1,"visible":true,"opacity":1.0}),
        )
        .unwrap();
    assert!(
        !f.visible("inspect_leader") && f.visible("inspect_rounds"),
        "ADS retains a fallback count when the magazine is obscured"
    );
    {
        let world = f.world.borrow();
        let ammo = world.get::<UiElement>(f.ids["inspect_rounds"]).unwrap();
        assert!(
            ammo.rect.y > 550.0 && ammo.style.font_size >= 20.0,
            "fallback count remains readable"
        );
    }
    f.lua
        .dispatch(
            "input.mouse_button",
            &json!({"button":"right","pressed":false,"gameplay":false}),
        )
        .unwrap();
    assert!(
        !f.visible("ammo_hud"),
        "leaving ADS hides ammo when no shot is recent"
    );
    f.lua
        .dispatch(
            "weapon.aim",
            &json!({"entity":f.ids["first_person"].to_string(),"pressed":true}),
        )
        .unwrap();
    assert!(f.visible("ammo_hud"), "scripted ADS has the same behavior");
    f.lua
        .dispatch(
            "weapon.aim",
            &json!({"entity":f.ids["first_person"].to_string(),"pressed":false}),
        )
        .unwrap();
    assert!(!f.visible("ammo_hud"));

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
    assert!(
        f.visible("ammo_hud"),
        "firing immediately shows the callout"
    );
    f.world
        .borrow_mut()
        .set_dynamic(f.ids["first_person"], "WeaponState", json!({"ammo":17}))
        .unwrap();
    f.tick(0.05);
    assert_eq!(f.text("inspect_rounds"), "17");
    for _ in 0..3 {
        f.tick(0.25);
    }
    f.lua.dispatch("weapon.shot", &json!({"entity":f.ids["first_person"].to_string(),"player":f.ids["player"].to_string()})).unwrap();
    for _ in 0..3 {
        f.tick(0.25);
    }
    assert!(f.visible("ammo_hud"), "another shot renews the hold time");
    for _ in 0..2 {
        f.tick(0.25);
    }
    assert!(!f.visible("ammo_hud"), "ammo hides after firing stops");

    f.key(true, false);
    assert!(
        f.visible("inspection_hud") && f.visible("ammo_hud"),
        "Q shows the ammo count"
    );
    f.lua
        .dispatch(
            "input.mouse_button",
            &json!({"button":"right","pressed":true,"gameplay":true}),
        )
        .unwrap();
    assert!(
        !f.visible("inspection_hud") && f.visible("ammo_hud"),
        "ADS dismisses Q stats but keeps ammo"
    );
    f.lua.dispatch("input.reset", &json!({})).unwrap();
    assert!(
        !f.visible("inspection_hud") && !f.visible("ammo_hud"),
        "input reset clears held visibility"
    );
}

#[test]
fn held_hip_and_ads_fire_keep_the_ammo_count_on_screen() {
    fn drawn(f: &Fixture, stage: &str) {
        let world = f.world.borrow();
        let root = world.get::<UiElement>(f.ids["ammo_hud"]).unwrap();
        let count = world.get::<UiElement>(f.ids["inspect_rounds"]).unwrap();
        assert!(
            root.visible && count.visible && count.style.text.0[3] > 0,
            "{stage}"
        );
        let frame = countersplat_ui_ecs::extract(
            &world,
            countersplat_ui::Rect::new(0.0, 0.0, 1050.0, 720.0),
        )
        .unwrap();
        assert!(
            frame
                .nodes
                .iter()
                .any(|node| node.id == f.ids["inspect_rounds"].to_string()),
            "the ammo count must actually be extracted for {stage}"
        );
    }
    fn mouse(f: &Fixture, button: &str, pressed: bool) {
        f.lua
            .dispatch(
                "input.mouse_button",
                &json!({
                    "button": button, "pressed": pressed, "gameplay": true
                }),
            )
            .unwrap();
    }

    let f = Fixture::new();
    f.pose("lowReady", 0.0);
    mouse(&f, "left", true);
    f.tick(0.05);
    drawn(&f, "hip fire starts");
    let (x, y) = {
        let world = f.world.borrow();
        let rect = &world
            .get::<UiElement>(f.ids["inspect_rounds"])
            .unwrap()
            .rect;
        (rect.x, rect.y)
    };
    f.lua
        .dispatch(
            "hud.weapon_presentation",
            &json!({
                "clip":"fireHip", "index":1, "visible":true, "opacity":1.0
            }),
        )
        .unwrap();
    {
        let world = f.world.borrow();
        let rect = &world
            .get::<UiElement>(f.ids["inspect_rounds"])
            .unwrap()
            .rect;
        assert!(
            (rect.x - x).abs() < 1.0 && (rect.y - y).abs() < 1.0,
            "the count should not jump away when the firing clip hides the magazine"
        );
    }
    for _ in 0..12 {
        f.tick(0.25);
        drawn(&f, "held hip fire");
    }
    mouse(&f, "left", false);
    for _ in 0..3 {
        f.tick(0.25);
    }
    drawn(&f, "briefly after releasing hip fire");
    for _ in 0..2 {
        f.tick(0.25);
    }
    assert!(!f.visible("ammo_hud"), "ammo hides at rest after hip fire");

    mouse(&f, "right", true);
    mouse(&f, "left", true);
    f.tick(0.05);
    f.lua
        .dispatch(
            "hud.weapon_presentation",
            &json!({
                "clip":"fireADS", "index":1, "visible":true, "opacity":1.0
            }),
        )
        .unwrap();
    for _ in 0..12 {
        f.tick(0.25);
        drawn(&f, "held ADS fire");
    }
    mouse(&f, "left", false);
    for _ in 0..6 {
        f.tick(0.25);
    }
    drawn(&f, "still aiming after firing");
    mouse(&f, "right", false);
    assert!(!f.visible("ammo_hud"), "leaving ADS at rest hides ammo");

    f.lua
        .dispatch(
            "weapon.trigger",
            &json!({
                "entity":f.ids["first_person"].to_string(), "pressed":true
            }),
        )
        .unwrap();
    for _ in 0..12 {
        f.tick(0.25);
        drawn(&f, "held scripted fire");
    }
    f.lua
        .dispatch(
            "weapon.trigger",
            &json!({
                "entity":f.ids["first_person"].to_string(), "pressed":false
            }),
        )
        .unwrap();
    for _ in 0..5 {
        f.tick(0.25);
    }
    assert!(
        !f.visible("ammo_hud"),
        "released scripted fire times out at rest"
    );
}

#[test]
fn real_weapon_and_hud_keep_ammo_visible_through_ads_fire() {
    let f = Fixture::new();
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
            &json!({
                "button":"right", "pressed":true, "gameplay":true
            }),
        )
        .unwrap();
    for _ in 0..30 {
        f.tick(0.05);
    }
    assert_eq!(
        f.world
            .borrow()
            .get_dynamic(f.ids["first_person"], "WeaponState")
            .unwrap()["state"],
        "ads",
        "the real controller must reach ADS before firing"
    );

    f.lua
        .dispatch(
            "input.mouse_button",
            &json!({
                "button":"left", "pressed":true, "gameplay":true
            }),
        )
        .unwrap();
    let mut ads_shots = 0;
    for _ in 0..60 {
        f.tick(0.05);
        let shots = f.world.borrow_mut().drain_events("weapon.shot");
        for shot in shots {
            assert_eq!(shot["stance"], "ads");
            ads_shots += 1;
            f.lua.dispatch("weapon.shot", &shot).unwrap();
            // The actual ADS firing clip has no visible magazine to track.
            f.lua
                .dispatch(
                    "hud.weapon_presentation",
                    &json!({
                        "clip":"fireADS", "index":1, "visible":true, "opacity":1.0
                    }),
                )
                .unwrap();
        }
        assert!(
            f.visible("ammo_hud") && f.visible("inspect_rounds"),
            "ADS firing must retain the count through every controller tick"
        );
    }
    assert_eq!(ads_shots, 1, "single-fire mode should emit an ADS shot");
    assert_eq!(
        f.text("inspect_rounds"),
        "29",
        "ADS fire updates the displayed count"
    );

    f.lua
        .dispatch(
            "input.mouse_button",
            &json!({
                "button":"left", "pressed":false, "gameplay":true
            }),
        )
        .unwrap();
    for _ in 0..30 {
        f.tick(0.05);
    }
    assert!(
        f.visible("ammo_hud"),
        "ADS still shows the count after the shot"
    );
    f.lua
        .dispatch(
            "input.mouse_button",
            &json!({
                "button":"right", "pressed":false, "gameplay":true
            }),
        )
        .unwrap();
    f.tick(0.05);
    assert!(
        !f.visible("ammo_hud"),
        "the count hides when ADS ends at rest"
    );
}

#[test]
fn real_weapon_fire_updates_ammo_then_returns_to_hidden_idle() {
    let f = Fixture::new();
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    f.lua
        .execute(
            &std::fs::read_to_string(root.join("assets/scripts/first_person_controller.lua"))
                .unwrap(),
        )
        .unwrap();
    f.tick(0.05);
    assert!(!f.visible("ammo_hud"));
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
    for _ in 0..8 {
        f.tick(0.05);
        let shots = f.world.borrow_mut().drain_events("weapon.shot");
        for shot in shots {
            f.lua.dispatch("weapon.shot", &shot).unwrap();
        }
    }
    assert!(f.visible("ammo_hud"));
    assert_eq!(
        f.text("inspect_rounds"),
        "29",
        "the count follows the actual weapon state"
    );
    for _ in 0..7 {
        f.tick(0.25);
    }
    assert!(!f.visible("ammo_hud"));
}

#[test]
fn q_map_requests_a_floor_slice_and_tracks_player_on_the_result() {
    let f = Fixture::new();
    assert!(f
        .world
        .borrow_mut()
        .drain_events("hud.map_request")
        .is_empty());
    f.key(true, false);
    let request = f
        .world
        .borrow_mut()
        .drain_events("hud.map_request")
        .pop()
        .unwrap();
    assert!((request["height"].as_f64().unwrap() - 0.9144).abs() < 0.0001);
    assert_eq!(request["radius"], 12.0);
    assert!(!f.visible("inspect_minimap"));
    assert_eq!(f.text("inspect_map_status"), "MAPPING...");
    let position = request["position"].as_array().unwrap();
    f.lua
        .dispatch(
            "hud.map_result",
            &json!({"status":"ready", "center":[position[0],position[2]], "radius":12.0}),
        )
        .unwrap();
    f.tick(0.12);
    assert!(f.visible("inspect_minimap"));
    assert!(!f.visible("inspect_map_status"));
    let original = f
        .world
        .borrow()
        .get::<UiElement>(f.ids["inspect_compass_arrow"])
        .unwrap()
        .rect
        .x;
    let north_tip = {
        let world = f.world.borrow();
        match &world
            .get::<UiElement>(f.ids["inspect_compass_arrow"])
            .unwrap()
            .widget
        {
            Widget::Polyline { points, .. } => points[1],
            _ => panic!("expected player marker"),
        }
    };
    {
        let mut world = f.world.borrow_mut();
        let camera = world
            .get_mut::<countersplat_scene::PlayerCamera>(f.ids["player"])
            .unwrap();
        camera.position[0] += 2.0;
        camera.yaw = std::f32::consts::FRAC_PI_2;
    }
    f.tick(0.25);
    assert!(
        f.world
            .borrow()
            .get::<UiElement>(f.ids["inspect_compass_arrow"])
            .unwrap()
            .rect
            .x
            > original
    );
    let east_tip = {
        let world = f.world.borrow();
        match &world
            .get::<UiElement>(f.ids["inspect_compass_arrow"])
            .unwrap()
            .widget
        {
            Widget::Polyline { points, .. } => points[1],
            _ => panic!("expected player marker"),
        }
    };
    assert!(
        east_tip[0] > north_tip[0] + 5.0,
        "the marker turns east with the camera"
    );
    let next = f.world.borrow_mut().drain_events("hud.map_request");
    assert_eq!(next.len(), 1, "moving requests a fresh map slice");
    f.lua.dispatch("input.reset", &json!({})).unwrap();
    assert!(!f.visible("inspection_hud"));
    f.world.borrow_mut().drain_events("hud.map_request");
    f.tick(0.25);
    assert!(
        f.world
            .borrow_mut()
            .drain_events("hud.map_request")
            .is_empty(),
        "hidden Q mode does not generate maps"
    );
}

#[test]
fn missing_voxels_leave_an_honest_map_status() {
    let f = Fixture::new();
    f.key(true, false);
    f.lua
        .dispatch("hud.map_result", &json!({"status":"unavailable"}))
        .unwrap();
    assert!(!f.visible("inspect_minimap"));
    assert_eq!(f.text("inspect_map_status"), "MAP UNAVAILABLE");
}
