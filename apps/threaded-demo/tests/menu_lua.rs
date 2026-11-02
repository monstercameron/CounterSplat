//! Real scene manifests and Lua scripts communicate exclusively through ECS events.
use countersplat_ecs::{Entity, World, WorldOperation, WorldOutcome};
use countersplat_scene::SceneManifest;
use countersplat_scripting_lua::{LuaRuntime, ScriptOptions, WorldBridge};
use countersplat_ui::{UiEvent, UiEventKind};
use serde_json::{json, Value};
use std::{cell::RefCell, collections::BTreeMap, path::PathBuf, rc::Rc};

struct LocalBridge(Rc<RefCell<World>>);
impl WorldBridge for LocalBridge {
    fn request(&mut self, op: WorldOperation) -> Result<WorldOutcome, String> {
        self.0.borrow_mut().apply_operation(op)
    }
}
fn scene(name: &str, script: &str) -> (Rc<RefCell<World>>, LuaRuntime, BTreeMap<String, Entity>) {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let path = root.join("assets/scenes").join(name);
    let manifest = SceneManifest::from_json(&std::fs::read(&path).unwrap()).unwrap();
    let world = Rc::new(RefCell::new(World::new()));
    countersplat_ui_ecs::register(&mut world.borrow_mut()).unwrap();
    let ids = countersplat_scene_ecs::instantiate_layer(&mut world.borrow_mut(), &manifest, "ui")
        .unwrap();
    let definition = &manifest.resources.scripts[script];
    let source = std::fs::read_to_string(path.parent().unwrap().join(&definition.uri)).unwrap();
    let lua = LuaRuntime::new(LocalBridge(world.clone()), ScriptOptions::default()).unwrap();
    lua.execute(&source).unwrap();
    (world, lua, ids)
}
fn key(world: &Rc<RefCell<World>>, pressed: bool, repeat: bool) {
    world.borrow_mut().emit_event(
        "input.key",
        json!({"key":"Escape", "pressed":pressed, "is_repeat":repeat}),
    );
}
fn click(world: &Rc<RefCell<World>>, id: Entity) {
    let event = UiEvent {
        id: id.to_string(),
        kind: UiEventKind::Clicked,
    };
    assert_eq!(
        countersplat_ui_ecs::apply_events(&mut world.borrow_mut(), [event]).len(),
        1
    );
}
fn open_page(
    world: &Rc<RefCell<World>>,
    lua: &LuaRuntime,
    ids: &BTreeMap<String, Entity>,
    page: &str,
) {
    click(world, ids[page]);
    lua.run_systems(0.0).unwrap();
}
fn commands(world: &Rc<RefCell<World>>, lua: &LuaRuntime) -> Vec<Value> {
    lua.run_systems(1.0 / 60.0).unwrap();
    world.borrow_mut().drain_events("engine.scene")
}
fn gameplay_commands(world: &Rc<RefCell<World>>, lua: &LuaRuntime) -> Vec<Value> {
    for channel in ["input.key", "ui.action"] {
        let events = world.borrow_mut().drain_events(channel);
        for event in events {
            lua.dispatch(channel, &event).unwrap();
        }
    }
    commands(world, lua)
}
#[test]
fn gameplay_escape_opens_pause_menu() {
    let (world, lua, _ids) = scene("gameplay.json", "gameplay-navigation");
    key(&world, false, false);
    key(&world, true, true);
    assert!(gameplay_commands(&world, &lua).is_empty());
    key(&world, true, false);
    assert_eq!(
        gameplay_commands(&world, &lua),
        vec![json!({"action":"push", "scene":"pause_menu"})]
    );
    assert!(gameplay_commands(&world, &lua).is_empty());
}
#[test]
fn menu_escape_resume_and_choose_level_use_scene_api() {
    let (world, lua, ids) = scene("menu.json", "menu-navigation");
    key(&world, true, false);
    assert_eq!(commands(&world, &lua), vec![json!({"action":"pop"})]);
    click(&world, ids["resume"]);
    assert_eq!(commands(&world, &lua), vec![json!({"action":"pop"})]);
    click(&world, ids["levels"]);
    assert_eq!(
        commands(&world, &lua),
        vec![json!({"action":"replace", "scene":"level_selector"})]
    );
}

#[test]
fn pause_menu_requests_display_and_hud_changes() {
    let (world, lua, ids) = scene("menu.json", "menu-navigation");
    lua.dispatch("menu.settings", &json!({"display":{"mode":"fixed","width":1280,"height":720,"scaling":"fill"},"show_performance":false})).unwrap();
    {
        let w = world.borrow();
        assert!(matches!(
            w.get::<countersplat_ui::UiElement>(ids["show_performance"])
                .unwrap()
                .widget,
            countersplat_ui::Widget::Checkbox { checked: false, .. }
        ));
    }
    open_page(&world, &lua, &ids, "nav_display");
    click(&world, ids["resolution_next"]);
    lua.run_systems(0.0).unwrap();
    assert_eq!(
        world.borrow_mut().drain_events("settings.display_request"),
        vec![json!({"mode":"fixed","width":1920,"height":1080,"scaling":"fill"})]
    );
    lua.dispatch("menu.settings", &json!({"display":{"mode":"fixed","width":1920,"height":1080,"scaling":"fill"},"show_performance":false})).unwrap();
    click(&world, ids["scaling_toggle"]);
    lua.run_systems(0.0).unwrap();
    assert_eq!(
        world.borrow_mut().drain_events("settings.display_request")[0]["scaling"],
        "fit"
    );
    open_page(&world, &lua, &ids, "nav_settings");
    let event = UiEvent {
        id: ids["show_performance"].to_string(),
        kind: UiEventKind::BoolChanged(true),
    };
    assert_eq!(
        countersplat_ui_ecs::apply_events(&mut world.borrow_mut(), [event]).len(),
        1
    );
    lua.run_systems(0.0).unwrap();
    assert_eq!(
        world.borrow_mut().drain_events("settings.hud_request"),
        vec![json!({"show_performance":true})]
    );
    open_page(&world, &lua, &ids, "nav_display");
    for (id, text) in [("resolution_width", "1600"), ("resolution_height", "1200")] {
        let event = UiEvent {
            id: ids[id].to_string(),
            kind: UiEventKind::TextChanged(text.into()),
        };
        assert_eq!(
            countersplat_ui_ecs::apply_events(&mut world.borrow_mut(), [event]).len(),
            1
        );
    }
    click(&world, ids["resolution_apply"]);
    lua.run_systems(0.0).unwrap();
    assert_eq!(
        world.borrow_mut().drain_events("settings.display_request"),
        vec![json!({"mode":"fixed","width":1600,"height":1200,"scaling":"fill"})]
    );
}

#[test]
fn pause_menu_scales_with_small_viewports() {
    let (world, lua, ids) = scene("menu.json", "menu-navigation");
    lua.dispatch("menu.viewport", &json!({"width":320,"height":260}))
        .unwrap();
    let w = world.borrow();
    let panel = w.get::<countersplat_ui::UiElement>(ids["panel"]).unwrap();
    let button = w.get::<countersplat_ui::UiElement>(ids["resume"]).unwrap();
    assert!(panel.rect.width <= 296.0 && panel.rect.height <= 236.0);
    assert!(button.rect.x + button.rect.width <= panel.rect.width);
    assert!(button.rect.y + button.rect.height <= panel.rect.height);
    drop(w);
    lua.dispatch("menu.viewport", &json!({"width":1050,"height":720}))
        .unwrap();
    assert_eq!(
        world
            .borrow()
            .get::<countersplat_ui::UiElement>(ids["panel"])
            .unwrap()
            .rect
            .width,
        1026.0
    );
}

#[test]
fn ratio_buttons_and_pixel_perfect_scaling_emit_separate_requests() {
    let (world, lua, ids) = scene("menu.json", "menu-navigation");
    lua.dispatch("menu.settings",&json!({"display":{"mode":"fixed","width":1280,"height":720,"scaling":"fit","aspect":"16:9"},
      "presets":[{"id":"640x360","label":"640 × 360","width":640,"height":360},{"id":"1280x720","label":"1280 × 720","width":1280,"height":720},
      {"id":"1920x1080","label":"1920 × 1080","width":1920,"height":1080}]})).unwrap();
    open_page(&world, &lua, &ids, "nav_display");
    for (id, aspect) in [
        ("aspect_16_9", "16:9"),
        ("aspect_16_10", "16:10"),
        ("aspect_21_9", "21:9"),
        ("aspect_custom", "custom"),
    ] {
        click(&world, ids[id]);
        lua.run_systems(0.0).unwrap();
        assert_eq!(
            world.borrow_mut().drain_events("settings.aspect_request"),
            vec![json!({"aspect":aspect})]
        );
    }
    click(&world, ids["resolution_previous"]);
    lua.run_systems(0.0).unwrap();
    assert_eq!(
        world.borrow_mut().drain_events("settings.display_request"),
        vec![json!({"mode":"fixed","width":640,"height":360,"scaling":"fit","aspect":"16:9"})]
    );
    click(&world, ids["scaling_toggle"]);
    lua.run_systems(0.0).unwrap();
    assert_eq!(
        world.borrow_mut().drain_events("settings.display_request")[0]["scaling"],
        "pixel"
    );
    click(&world, ids["resolution_native"]);
    lua.run_systems(0.0).unwrap();
    assert_eq!(
        world.borrow_mut().drain_events("settings.display_request")[0]["mode"],
        "viewport"
    );
    lua.dispatch("menu.settings",&json!({"display":{"mode":"fixed","width":1280,"height":720,"scaling":"pixel","aspect":"16:9"},"error":"Pixel perfect: 1× cropped"})).unwrap();
    let w = world.borrow();
    let element = w
        .get::<countersplat_ui::UiElement>(ids["feedback"])
        .unwrap();
    assert!(
        matches!(&element.widget,countersplat_ui::Widget::Text{text,..} if text.contains("cropped"))
    );
}

#[test]
fn resolution_steps_are_bounded_and_neighbor_native_size() {
    let (world, lua, ids) = scene("menu.json", "menu-navigation");
    let presets = json!([
        {"id":"640x360","label":"640 × 360","width":640,"height":360},
        {"id":"1280x720","label":"1280 × 720","width":1280,"height":720},
        {"id":"1920x1080","label":"1920 × 1080","width":1920,"height":1080}
    ]);
    lua.dispatch("menu.settings", &json!({"display":{"mode":"viewport","width":1280,"height":720,
        "actual_width":1600,"actual_height":900,"scaling":"fit","aspect":"16:9"},"presets":presets})).unwrap();
    open_page(&world, &lua, &ids, "nav_display");
    for (id, width, height) in [
        ("resolution_previous", 1280, 720),
        ("resolution_next", 1920, 1080),
    ] {
        click(&world, ids[id]);
        lua.run_systems(0.0).unwrap();
        assert_eq!(
            world.borrow_mut().drain_events("settings.display_request"),
            vec![
                json!({"mode":"fixed","width":width,"height":height,"scaling":"fit","aspect":"16:9"})
            ]
        );
    }
    for (id, width, height) in [
        ("resolution_previous", 640, 360),
        ("resolution_next", 1920, 1080),
    ] {
        lua.dispatch(
            "menu.settings",
            &json!({"display":{"mode":"fixed","width":width,"height":height,
            "scaling":"fit","aspect":"16:9"}}),
        )
        .unwrap();
        click(&world, ids[id]);
        lua.run_systems(0.0).unwrap();
        assert!(world
            .borrow_mut()
            .drain_events("settings.display_request")
            .is_empty());
    }
}

#[test]
fn custom_resolution_steps_keep_the_selected_reference_ratio() {
    let (world, lua, ids) = scene("menu.json", "menu-navigation");
    lua.dispatch(
        "menu.settings",
        &json!({"display":{"mode":"fixed","width":1269,"height":600,
            "actual_width":1269,"actual_height":600,"scaling":"fit","aspect":"custom",
            "ratio_width":1280,"ratio_height":605},"presets":[
            {"id":"1142x540","label":"1142 × 540","width":1142,"height":540},
            {"id":"1269x600","label":"1269 × 600","width":1269,"height":600}
        ]}),
    )
    .unwrap();
    open_page(&world, &lua, &ids, "nav_display");
    let label = world
        .borrow()
        .get::<countersplat_ui::UiElement>(ids["aspect_custom"])
        .unwrap()
        .widget
        .clone();
    assert!(
        matches!(label, countersplat_ui::Widget::Button { label } if label == "Custom · 256:121")
    );
    click(&world, ids["resolution_previous"]);
    lua.run_systems(0.0).unwrap();
    assert_eq!(
        world.borrow_mut().drain_events("settings.display_request"),
        vec![
            json!({"mode":"fixed","width":1142,"height":540,"scaling":"fit","aspect":"custom",
            "ratio_width":1280,"ratio_height":605})
        ]
    );
    click(&world, ids["resolution_apply"]);
    lua.run_systems(0.0).unwrap();
    assert_eq!(
        world.borrow_mut().drain_events("settings.display_request")[0]["reset_ratio"],
        true
    );
}

#[test]
fn pause_overview_uses_live_scene_values_and_navigation_opens_pages() {
    let (world, lua, ids) = scene("menu.json", "menu-navigation");
    lua.dispatch("menu.settings",&json!({"display":{"mode":"fixed","width":1280,"height":720,"scaling":"fit","aspect":"16:9"},
      "game":{"name":"The Barn","format":"single_sog","resident_splats":2477739,"chunks_loaded":1,"playtime_seconds":754},
      "controls":{"bindings":{"forward":"ArrowUp","crouch":"ControlLeft"},"move_speed":5.0,"mouse_sensitivity":0.004,"invert_y":true},
      "graphics":{"lod":null,"lod_count":4,"draw_distance":200,"sky_enabled":true},"audio":{"gain":0.5,"muted":false}})).unwrap();
    let text = |id: &str| {
        let w = world.borrow();
        match &w.get::<countersplat_ui::UiElement>(ids[id]).unwrap().widget {
            countersplat_ui::Widget::Text { text, .. } => text.clone(),
            _ => panic!("text expected"),
        }
    };
    assert_eq!(text("status_level_name"), "The Barn");
    assert_eq!(text("status_value_splats"), "2,477,739");
    assert_eq!(text("status_value_chunks"), "1");
    assert_eq!(text("status_value_playtime"), "12:34");
    assert!(text("overview_key_1").contains("↑"));
    assert_eq!(text("overview_display_value_sky"), "On");
    for (nav, group) in [
        ("nav_settings", "page_settings"),
        ("nav_display", "page_display"),
        ("nav_controls", "page_controls"),
        ("nav_graphics", "page_graphics"),
        ("nav_audio", "page_audio"),
    ] {
        open_page(&world, &lua, &ids, nav);
        assert!(
            world
                .borrow()
                .get::<countersplat_ui::UiElement>(ids[group])
                .unwrap()
                .visible
        );
        assert!(
            !world
                .borrow()
                .get::<countersplat_ui::UiElement>(ids["page_overview"])
                .unwrap()
                .visible
        );
    }
    open_page(&world, &lua, &ids, "back_overview");
    assert!(
        world
            .borrow()
            .get::<countersplat_ui::UiElement>(ids["page_overview"])
            .unwrap()
            .visible
    );
}

#[test]
fn pause_controls_emit_typed_gameplay_setting_requests() {
    let (world, lua, ids) = scene("menu.json", "menu-navigation");
    for (page, id, kind, expected) in [
        (
            "nav_controls",
            "controls_speed",
            UiEventKind::NumberChanged(6.0),
            json!({"target":"controls_speed","kind":"number_changed","value":6.0}),
        ),
        (
            "nav_graphics",
            "graphics_sky",
            UiEventKind::BoolChanged(false),
            json!({"target":"sky_enabled","kind":"bool_changed","value":false}),
        ),
        (
            "nav_audio",
            "audio_gain",
            UiEventKind::NumberChanged(0.4),
            json!({"target":"audio_gain","kind":"number_changed","value":0.4}),
        ),
    ] {
        open_page(&world, &lua, &ids, page);
        let event = UiEvent {
            id: ids[id].to_string(),
            kind,
        };
        assert_eq!(
            countersplat_ui_ecs::apply_events(&mut world.borrow_mut(), [event]).len(),
            1
        );
        lua.run_systems(0.0).unwrap();
        let events = world.borrow_mut().drain_events("settings.gameplay_request");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["target"], expected["target"]);
        assert_eq!(events[0]["kind"], expected["kind"]);
        assert!(
            (events[0]["value"]
                .as_f64()
                .unwrap_or(if events[0]["value"] == true { 1.0 } else { 0.0 })
                - expected["value"]
                    .as_f64()
                    .unwrap_or(if expected["value"] == true { 1.0 } else { 0.0 }))
            .abs()
                < 0.0001
        );
    }
}

#[test]
fn return_to_menu_opens_the_available_top_level_scene() {
    let (world, lua, ids) = scene("menu.json", "menu-navigation");
    click(&world, ids["return_menu"]);
    assert_eq!(
        commands(&world, &lua),
        vec![json!({"action":"replace", "scene":"level_selector"})]
    );
}
