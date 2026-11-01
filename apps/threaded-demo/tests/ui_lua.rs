//! Exercise UI components and events through the real Lua ECS bridge.
use countersplat_ecs::{World, WorldOperation, WorldOutcome};
use countersplat_scripting_lua::{LuaRuntime, ScriptOptions, WorldBridge};
use countersplat_ui::{Rect, UiEvent, UiEventKind, Widget};
use std::{cell::RefCell, rc::Rc};

struct LocalBridge(Rc<RefCell<World>>);
impl WorldBridge for LocalBridge {
    fn request(&mut self, op: WorldOperation) -> Result<WorldOutcome, String> {
        self.0.borrow_mut().apply_operation(op)
    }
}

#[test]
fn lua_creates_and_updates_ui_and_receives_control_events() {
    let world = Rc::new(RefCell::new(World::new()));
    countersplat_ui_ecs::register(&mut world.borrow_mut()).unwrap();
    let lua = LuaRuntime::new(LocalBridge(world.clone()), ScriptOptions::default()).unwrap();
    lua.execute(
        r#"
        toggle = ecs.spawn()
        ecs.set(toggle, "UiElement", {
            rect = { x = 10, y = 20, width = 160, height = 30 },
            widget = { type = "checkbox", label = "Music", checked = true }
        })
        local element = ecs.get(toggle, "UiElement")
        assert(element.widget.checked and element.widget.label == "Music")
        element.rect.width = -1
        assert(not pcall(function() ecs.set(toggle, "UiElement", element) end))
        assert(ecs.get(toggle, "UiElement").rect.width == 160)
    "#,
    )
    .unwrap();
    let frame =
        countersplat_ui_ecs::extract(&world.borrow(), Rect::new(0.0, 0.0, 400.0, 300.0)).unwrap();
    assert_eq!(frame.nodes.len(), 1);
    assert!(matches!(
        frame.nodes[0].element.widget,
        Widget::Checkbox { checked: true, .. }
    ));
    let event = UiEvent {
        id: frame.nodes[0].id.clone(),
        kind: UiEventKind::BoolChanged(false),
    };
    assert_eq!(
        countersplat_ui_ecs::apply_events(&mut world.borrow_mut(), [event]),
        vec![UiEvent {
            id: frame.nodes[0].id.clone(),
            kind: UiEventKind::BoolChanged(false),
        }]
    );
    lua.execute(
        r#"
        assert(ecs.get(toggle, "UiElement").widget.checked == false)
        local events = ecs.drain("ui.action")
        assert(#events == 1 and events[1].id == toggle)
        assert(events[1].kind.type == "bool_changed" and events[1].kind.value == false)
    "#,
    )
    .unwrap();
    lua.execute(include_str!("../examples/fps_counter.lua"))
        .unwrap();
    // Running the example before the first timing window must handle JSON null safely.
    lua.run_system("ui.frame_counter", 1.0 / 60.0).unwrap();
    lua.execute(
        r#"
        counter = ui_frame_counter
        assert(ecs.get(counter, "FrameCounter").show_fps)
        assert(ecs.get(counter, "FrameCounter").measurement == ecs.null)
        local config = ecs.get(counter, "FrameCounter")
        config.precision = 2
        ecs.set(counter, "FrameCounter", config)
        config.precision = 9
        assert(not pcall(function() ecs.set(counter, "FrameCounter", config) end))
        assert(ecs.get(counter, "FrameCounter").precision == 2)
    "#,
    )
    .unwrap();
    countersplat_ui_ecs::update_frame_counters(
        &mut world.borrow_mut(),
        Some(countersplat_ui::FrameMeasurement {
            fps: 50.0,
            frame_time_ms: 20.0,
            intervals: 25,
        }),
    )
    .unwrap();
    lua.run_system("ui.frame_counter", 1.0 / 60.0).unwrap();
    lua.execute(
        r#"
        assert(ecs.get(counter, "UiElement").style.text[2] == 245)
        local config = ecs.get(counter, "FrameCounter")
        assert(config.measurement.fps == 50 and config.measurement.frame_time_ms == 20)
        -- Scripts can customize the built-in counter or use its readings in their own UI.
        config.show_frame_time = false
        ecs.set(counter, "FrameCounter", config)
        local custom = ecs.get(toggle, "UiElement")
        custom.widget.label = string.format("FPS: %.0f", config.measurement.fps)
        ecs.set(toggle, "UiElement", custom)
    "#,
    )
    .unwrap();
    let snapshot =
        countersplat_ui_ecs::extract(&world.borrow(), Rect::new(0.0, 0.0, 400.0, 300.0)).unwrap();
    assert!(snapshot.nodes.iter().any(
        |node| matches!(&node.element.widget, Widget::Text { text, .. } if text == "50.00 FPS")
    ));
    assert!(snapshot.nodes.iter().any(
        |node| matches!(&node.element.widget, Widget::Checkbox { label, .. } if label == "FPS: 50")
    ));
    lua.execute(
        r#"
        local graph = ecs.spawn()
        ecs.set(graph, "UiElement", {
            rect = { x = 10, y = 40, width = 200, height = 60 },
            widget = { type = "line_graph", points = {{0, 16}, {60, 32}},
                       x_min = 0, x_max = 60, y_min = 0, y_max = 50 }
        })
        assert(ecs.get(graph, "UiElement").widget.points[2][2] == 32)
        local invalid = ecs.get(graph, "UiElement")
        invalid.widget.y_max = 0
        assert(not pcall(function() ecs.set(graph, "UiElement", invalid) end))
    "#,
    )
    .unwrap();
}

#[test]
fn lua_can_configure_and_control_collapsible_panels() {
    let world = Rc::new(RefCell::new(World::new()));
    countersplat_ui_ecs::register(&mut world.borrow_mut()).unwrap();
    let lua = LuaRuntime::new(LocalBridge(world.clone()), ScriptOptions::default()).unwrap();
    lua.execute(
        r#"
        panel = ecs.spawn()
        ecs.set(panel, "UiElement", {
            rect = { x = 10, y = 10, width = 200, height = 100 },
            widget = { type = "panel" },
            collapsible = { title = "Script panel", header_height = 28, collapsed = false }
        })
        child = ecs.spawn()
        ecs.set(child, "UiElement", {
            rect = { x = 0, y = 0, width = 80, height = 20 },
            widget = { type = "button", label = "Action" }
        })
        ecs.set_parent(child, panel)
        local state = ecs.get(panel, "UiElement")
        state.collapsible.collapsed = true
        ecs.set(panel, "UiElement", state)
        state.collapsible.header_height = 100
        assert(not pcall(function() ecs.set(panel, "UiElement", state) end))
        assert(ecs.get(panel, "UiElement").collapsible.header_height == 28)
    "#,
    )
    .unwrap();
    let frame =
        countersplat_ui_ecs::extract(&world.borrow(), Rect::new(0.0, 0.0, 400.0, 300.0)).unwrap();
    assert_eq!(frame.nodes.len(), 1);
    assert_eq!(frame.nodes[0].rect.height, 28.0);
    let event = UiEvent {
        id: frame.nodes[0].id.clone(),
        kind: UiEventKind::CollapsedChanged(false),
    };
    assert_eq!(
        countersplat_ui_ecs::apply_events(&mut world.borrow_mut(), [event.clone()]),
        vec![event]
    );
    lua.execute(r#"
        assert(not ecs.get(panel, "UiElement").collapsible.collapsed)
        local actions = ecs.drain("ui.action")
        assert(#actions == 1 and actions[1].kind.type == "collapsed_changed" and actions[1].kind.value == false)
    "#).unwrap();
    assert_eq!(
        countersplat_ui_ecs::extract(&world.borrow(), Rect::new(0.0, 0.0, 400.0, 300.0))
            .unwrap()
            .nodes
            .len(),
        2
    );
}
