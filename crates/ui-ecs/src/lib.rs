//! ECS integration for CounterSplat's renderer-independent UI data.
//!
//! UI elements are ordinary ECS components. [`extract`] produces an owned, screen-space snapshot;
//! [`apply_events`] validates returned control events on the world owner and emits script-readable
//! `ui.action` events. No GPU or scripting implementation is needed in the ECS crate.

use countersplat_ecs::{ComponentAdapter, Entity, Value, World};
mod layout;
pub use layout::UiLayout;

use countersplat_ui::{
    place, FrameCounter, FrameMeasurement, Rect, ResolvedNode, UiElement, UiEvent, UiEventKind,
    UiFrame, Widget,
};
use std::collections::BTreeMap;

/// Name exposed through `WorldOperation` and Lua's generic `ecs.get`/`ecs.set` API.
pub const COMPONENT_NAME: &str = "UiElement";
/// Script-visible frame counter configuration and last measurement.
pub const FRAME_COUNTER_NAME: &str = "FrameCounter";
/// ECS event queue containing accepted UI interactions as JSON values.
pub const EVENT_CHANNEL: &str = "ui.action";

struct Adapter;
impl ComponentAdapter for Adapter {
    fn read(&self, world: &World, entity: Entity) -> Result<Option<Value>, String> {
        world
            .get::<UiElement>(entity)
            .map(serde_json::to_value)
            .transpose()
            .map_err(|e| e.to_string())
    }
    fn write(&self, world: &mut World, entity: Entity, value: Value) -> Result<(), String> {
        let element: UiElement = serde_json::from_value(value).map_err(|e| e.to_string())?;
        element.validate().map_err(str::to_owned)?;
        world.insert(entity, element)
    }
    fn remove(&self, world: &mut World, entity: Entity) -> Result<Option<Value>, String> {
        let old = self.read(world, entity)?;
        world.remove::<UiElement>(entity);
        Ok(old)
    }
    fn contains(&self, world: &World, entity: Entity) -> bool {
        world.has::<UiElement>(entity)
    }
    fn indexed_entities<'a>(&self, world: &'a World) -> Option<countersplat_ecs::EntityIndex<'a>> {
        Some(world.iter_with::<UiElement>())
    }
}

struct FrameCounterAdapter;
impl ComponentAdapter for FrameCounterAdapter {
    fn read(&self, world: &World, entity: Entity) -> Result<Option<Value>, String> {
        world
            .get::<FrameCounter>(entity)
            .map(serde_json::to_value)
            .transpose()
            .map_err(|e| e.to_string())
    }
    fn write(&self, world: &mut World, entity: Entity, value: Value) -> Result<(), String> {
        let counter: FrameCounter = serde_json::from_value(value).map_err(|e| e.to_string())?;
        counter.validate().map_err(str::to_owned)?;
        world.insert(entity, counter)
    }
    fn remove(&self, world: &mut World, entity: Entity) -> Result<Option<Value>, String> {
        let old = self.read(world, entity)?;
        world.remove::<FrameCounter>(entity);
        Ok(old)
    }
    fn contains(&self, world: &World, entity: Entity) -> bool {
        world.has::<FrameCounter>(entity)
    }
    fn indexed_entities<'a>(&self, world: &'a World) -> Option<countersplat_ecs::EntityIndex<'a>> {
        Some(world.iter_with::<FrameCounter>())
    }
}

/// Register validated UI and frame-counter adapters for the generic ECS scripting bridge.
pub fn register(world: &mut World) -> Result<(), String> {
    world.register_component_adapter(COMPONENT_NAME, Adapter)?;
    world.register_component_adapter(FRAME_COUNTER_NAME, FrameCounterAdapter)
}

/// Publish a measurement to every frame counter. Call this only on the world owner.
/// Passing `None` clears stale values after a pause or clock reset.
pub fn update_frame_counters(
    world: &mut World,
    measurement: Option<FrameMeasurement>,
) -> Result<(), &'static str> {
    if let Some(value) = measurement {
        value.validate()?;
    }
    let entities: Vec<_> = world
        .query::<FrameCounter>()
        .into_iter()
        .map(|(entity, _)| entity)
        .collect();
    for entity in entities {
        if let Some(counter) = world.get_mut::<FrameCounter>(entity) {
            counter.measurement = measurement;
        }
    }
    Ok(())
}

/// Spawn a validated UI entity and optionally parent it to another entity.
pub fn spawn(
    world: &mut World,
    parent: Option<Entity>,
    element: UiElement,
) -> Result<Entity, String> {
    element.validate().map_err(str::to_owned)?;
    if parent.is_some_and(|parent| !world.is_alive(parent)) {
        return Err("UI parent is stale or belongs to another world".into());
    }
    let entity = world.spawn();
    if let Err(error) = world
        .insert(entity, element)
        .and_then(|()| world.set_parent(entity, parent))
    {
        world.despawn(entity);
        return Err(error);
    }
    Ok(entity)
}

#[derive(Clone, Copy)]
struct Layout {
    child_rect: Rect,
    child_clip: Option<Rect>,
    children_visible: bool,
    enabled: bool,
    scroll_parent: Option<Entity>,
}

/// Extract visible, clipped, z-ordered nodes from the ECS into an owned render snapshot.
///
/// An immediate parent carrying `UiElement` establishes local coordinates, inherited visibility,
/// enabled state, and optional clipping. Collapsible panels provide a header and hide their entire
/// descendant tree while collapsed. A non-UI parent makes the element a viewport root. Invalid
/// components inserted through a direct Rust `World::insert` are skipped along with their descendants.
/// The ECS hierarchy prevents cycles; iterative layout avoids recursion for deeply nested UI trees.
/// Z values are global, with stable ID order for ties.
pub fn extract(world: &World, viewport: Rect) -> Result<UiFrame, &'static str> {
    if !viewport.valid() {
        return Err("UI viewport must be finite and nonempty");
    }
    let nodes: BTreeMap<Entity, &UiElement> = world.query::<UiElement>().into_iter().collect();
    // A scroll panel must keep its full viewport reachable when it fits in its
    // parent; moving only its header on screen can strand the last list rows.
    let scroll_hosts: std::collections::BTreeSet<Entity> = nodes
        .iter()
        .filter(|(_, item)| matches!(item.widget, Widget::ScrollArea { .. }))
        .filter_map(|(entity, _)| world.parent(*entity))
        .collect();
    let mut layouts: BTreeMap<Entity, Layout> = BTreeMap::new();
    let mut frame = UiFrame::default();
    for &entity in nodes.keys() {
        if layouts.contains_key(&entity) {
            continue;
        }
        let mut path = Vec::new();
        let mut cursor = Some(entity);
        while let Some(current) = cursor {
            if layouts.contains_key(&current) {
                break;
            }
            path.push(current);
            cursor = world
                .parent(current)
                .filter(|parent| nodes.contains_key(parent));
        }
        for current in path.into_iter().rev() {
            let element = nodes[&current];
            let parent = world.parent(current).and_then(|p| layouts.get(&p).copied());
            let parent_rect = parent.map_or(viewport, |p| p.child_rect);
            let inherited_clip = parent.map_or(Some(viewport), |p| p.child_clip);
            let collapsed = element
                .collapsible
                .as_ref()
                .is_some_and(|panel| panel.collapsed);
            let mut local_rect = element.rect;
            if let Some(panel) = &element.collapsible {
                if collapsed {
                    local_rect.height = panel.header_height;
                }
            }
            let mut rect = place(parent_rect, local_rect, element.anchor);
            rect.x += element.drag_offset[0];
            rect.y += element.drag_offset[1];
            let unclamped_position = [rect.x, rect.y];
            // Preserve a reachable part of a moved element after its viewport or parent resizes.
            if element.drag_offset != [0.0, 0.0] {
                if let Some(bounds) = inherited_clip {
                    let visible_width = rect.width.min(bounds.width).min(32.0);
                    let handle_height = element
                        .collapsible
                        .as_ref()
                        .map_or(rect.height.min(24.0), |panel| panel.header_height);
                    let visible_height = handle_height.min(bounds.height).min(24.0);
                    if visible_width > 0.0 && visible_height > 0.0 && rect.valid() && bounds.valid()
                    {
                        rect.x = rect.x.clamp(
                            bounds.x - rect.width + visible_width,
                            bounds.x + bounds.width - visible_width,
                        );
                        rect.y = rect
                            .y
                            .clamp(bounds.y, bounds.y + bounds.height - visible_height);
                    }
                }
            }
            if !collapsed && element.collapsible.is_some() && scroll_hosts.contains(&current) {
                if let Some(bounds) = inherited_clip {
                    if rect.valid() && bounds.valid() && rect.height <= bounds.height {
                        rect.y = rect
                            .y
                            .clamp(bounds.y, bounds.y + bounds.height - rect.height);
                    }
                }
            }
            let visible = element.visible
                && element.validate().is_ok()
                && rect.valid()
                && parent.is_none_or(|p| p.children_visible);
            let enabled = element.enabled && parent.is_none_or(|p| p.enabled);
            let own_clip = inherited_clip.and_then(|clip| clip.intersection(rect));
            let child_rect = match (&element.widget, &element.collapsible) {
                (
                    Widget::ScrollArea {
                        offset,
                        content_height,
                    },
                    _,
                ) => Rect::new(
                    rect.x,
                    rect.y - offset,
                    rect.width,
                    content_height.max(rect.height),
                ),
                (_, Some(panel)) if !collapsed => Rect::new(
                    rect.x,
                    rect.y + panel.header_height,
                    rect.width,
                    rect.height - panel.header_height,
                ),
                _ => rect,
            };
            // The content moves, but the scrolling viewport and its clip stay fixed.
            let clip_rect = if matches!(element.widget, Widget::ScrollArea { .. }) {
                rect
            } else {
                child_rect
            };
            let child_clip =
                if element.clip_children || matches!(element.widget, Widget::ScrollArea { .. }) {
                    inherited_clip.and_then(|clip| clip.intersection(clip_rect))
                } else {
                    inherited_clip
                };
            let scroll_parent = if matches!(element.widget, Widget::ScrollArea { .. }) {
                Some(current)
            } else {
                parent.and_then(|p| p.scroll_parent)
            };
            layouts.insert(
                current,
                Layout {
                    child_rect,
                    child_clip,
                    children_visible: visible && !collapsed,
                    enabled,
                    scroll_parent,
                },
            );
            if let (true, Some(clip)) = (visible, own_clip) {
                let mut displayed = element.clone();
                if let (Widget::Text { text, .. }, Some(counter)) =
                    (&mut displayed.widget, world.get::<FrameCounter>(current))
                {
                    if counter.validate().is_ok() {
                        *text = counter.label();
                    }
                }
                frame.nodes.push(ResolvedNode {
                    id: current.to_string(),
                    rect,
                    clip,
                    scroll_parent: scroll_parent.map(|id| id.to_string()),
                    drag_bounds: inherited_clip.expect("visible nodes have an inherited clip"),
                    drag_correction: [
                        rect.x - unclamped_position[0],
                        rect.y - unclamped_position[1],
                    ],
                    z: element.z,
                    enabled,
                    element: displayed,
                });
            }
        }
    }
    frame
        .nodes
        .sort_by(|a, b| a.z.cmp(&b.z).then_with(|| a.id.cmp(&b.id)));
    Ok(frame)
}

fn can_interact(world: &World, entity: Entity) -> bool {
    let mut cursor = Some(entity);
    let mut is_target = true;
    while let Some(current) = cursor {
        let Some(element) = world.get::<UiElement>(current) else {
            break;
        };
        if !element.visible
            || !element.enabled
            || element.validate().is_err()
            || (!is_target
                && element
                    .collapsible
                    .as_ref()
                    .is_some_and(|panel| panel.collapsed))
        {
            return false;
        }
        is_target = false;
        cursor = world.parent(current);
    }
    true
}

/// Apply renderer events on the world owner; stale, foreign, disabled and mismatched events are ignored.
///
/// Accepted events are returned and copied to the single-consumer `ui.action` ECS event queue. A host
/// may forward the returned owned events over its own transport instead of sharing `World` across threads.
pub fn apply_events(world: &mut World, events: impl IntoIterator<Item = UiEvent>) -> Vec<UiEvent> {
    let mut accepted = Vec::new();
    for event in events {
        let Ok(entity) = event.id.parse::<Entity>() else {
            continue;
        };
        if !world.is_alive(entity) || !can_interact(world, entity) {
            continue;
        }
        let Some(element) = world.get_mut::<UiElement>(entity) else {
            continue;
        };
        let applied = if let UiEventKind::MovedBy(delta) = &event.kind {
            let next = [
                element.drag_offset[0] + delta[0],
                element.drag_offset[1] + delta[1],
            ];
            if element.draggable
                && *delta != [0.0, 0.0]
                && delta
                    .iter()
                    .chain(next.iter())
                    .all(|v| v.is_finite() && v.abs() <= 1_000_000.0)
            {
                element.drag_offset = next;
                true
            } else {
                false
            }
        } else if let UiEventKind::ResizedTo(size) = &event.kind {
            if let Some(limits) = &element.resize {
                if !element
                    .collapsible
                    .as_ref()
                    .is_some_and(|panel| panel.collapsed)
                    && (0..2).all(|axis| {
                        size[axis].is_finite()
                            && size[axis] >= limits.min[axis]
                            && size[axis] <= limits.max[axis]
                    })
                    && *size != [element.rect.width, element.rect.height]
                {
                    element.rect.width = size[0];
                    element.rect.height = size[1];
                    true
                } else {
                    false
                }
            } else {
                false
            }
        } else {
            match (&mut element.widget, &event.kind) {
                (Widget::Panel, UiEventKind::CollapsedChanged(next)) => {
                    match &mut element.collapsible {
                        Some(panel) if panel.collapsed != *next => {
                            panel.collapsed = *next;
                            true
                        }
                        _ => false,
                    }
                }
                (
                    Widget::ScrollArea {
                        offset,
                        content_height,
                    },
                    UiEventKind::ScrollChanged(next),
                ) if next.is_finite()
                    && *next >= 0.0
                    && *next <= (*content_height - element.rect.height).max(0.0)
                    && *offset != *next =>
                {
                    *offset = *next;
                    true
                }
                (Widget::Button { .. }, UiEventKind::Clicked) => true,
                (Widget::Checkbox { checked, .. }, UiEventKind::BoolChanged(value))
                    if *checked != *value =>
                {
                    *checked = *value;
                    true
                }
                (
                    Widget::Slider {
                        value, min, max, ..
                    },
                    UiEventKind::NumberChanged(next),
                ) if next.is_finite() && *next >= *min && *next <= *max && *value != *next => {
                    *value = *next;
                    true
                }
                (Widget::TextInput { text, max_chars }, UiEventKind::TextChanged(next))
                    if next.len() <= 16_384
                        && next.chars().count() <= *max_chars
                        && text != next =>
                {
                    *text = next.clone();
                    true
                }
                _ => false,
            }
        };
        if applied {
            if let Ok(value) = serde_json::to_value(&event) {
                world.emit_event(EVENT_CHANNEL, value);
            }
            accepted.push(event);
        }
    }
    accepted
}

#[cfg(test)]
mod tests {
    #[test]
    fn slider_logarithmic_mode_is_optional_and_round_trips() {
        let old: Widget = serde_json::from_str(
            r#"{"type":"slider","label":"Focus","value":5.0,"min":0.1,"max":1000.0}"#,
        )
        .unwrap();
        assert!(matches!(
            old,
            Widget::Slider {
                logarithmic: false,
                ..
            }
        ));
        let updated: Widget = serde_json::from_str(r#"{"type":"slider","label":"Focus","value":5.0,"min":0.1,"max":1000.0,"logarithmic":true}"#).unwrap();
        assert!(matches!(
            updated,
            Widget::Slider {
                logarithmic: true,
                ..
            }
        ));
        assert!(serde_json::to_string(&updated)
            .unwrap()
            .contains("\"logarithmic\":true"));
    }

    use super::*;
    use countersplat_ui::{Anchor, UiElement};
    use serde_json::json;

    #[test]
    fn hierarchy_resolves_anchors_clipping_visibility_and_order() {
        let mut world = World::new();
        let root = spawn(
            &mut world,
            None,
            UiElement {
                rect: Rect::new(10.0, 20.0, 100.0, 80.0),
                ..Default::default()
            },
        )
        .unwrap();
        let child = spawn(
            &mut world,
            Some(root),
            UiElement {
                rect: Rect::new(5.0, 6.0, 40.0, 20.0),
                anchor: Anchor::BottomRight,
                z: 2,
                widget: Widget::Button { label: "Go".into() },
                ..Default::default()
            },
        )
        .unwrap();
        let frame = extract(&world, Rect::new(0.0, 0.0, 300.0, 200.0)).unwrap();
        assert_eq!(frame.nodes.len(), 2);
        assert_eq!(frame.nodes[1].id, child.to_string());
        assert_eq!(frame.nodes[1].rect, Rect::new(65.0, 74.0, 40.0, 20.0));
        world.get_mut::<UiElement>(root).unwrap().visible = false;
        assert!(extract(&world, Rect::new(0.0, 0.0, 300.0, 200.0))
            .unwrap()
            .nodes
            .is_empty());
    }

    #[test]
    fn scroll_area_clips_children_and_rejects_invalid_offsets() {
        let mut world = World::new();
        let area = spawn(
            &mut world,
            None,
            UiElement {
                rect: Rect::new(10.0, 20.0, 120.0, 100.0),
                widget: Widget::ScrollArea {
                    offset: 50.0,
                    content_height: 300.0,
                },
                ..Default::default()
            },
        )
        .unwrap();
        let child = spawn(
            &mut world,
            Some(area),
            UiElement {
                rect: Rect::new(5.0, 120.0, 100.0, 40.0),
                widget: Widget::Button {
                    label: "Child".into(),
                },
                ..Default::default()
            },
        )
        .unwrap();
        let viewport = Rect::new(0.0, 0.0, 400.0, 300.0);
        let frame = extract(&world, viewport).unwrap();
        let node = frame
            .nodes
            .iter()
            .find(|node| node.id == child.to_string())
            .unwrap();
        assert_eq!(node.rect, Rect::new(15.0, 90.0, 100.0, 40.0));
        assert_eq!(node.clip, Rect::new(15.0, 90.0, 100.0, 30.0));
        assert_eq!(
            node.scroll_parent.as_deref(),
            Some(area.to_string().as_str())
        );
        let event = |offset| UiEvent {
            id: area.to_string(),
            kind: UiEventKind::ScrollChanged(offset),
        };
        assert!(apply_events(&mut world, [event(-1.0), event(201.0), event(f32::NAN)]).is_empty());
        assert_eq!(apply_events(&mut world, [event(150.0)]).len(), 1);
        let frame = extract(&world, viewport).unwrap();
        let node = frame
            .nodes
            .iter()
            .find(|node| node.id == child.to_string())
            .unwrap();
        assert_eq!(node.clip, Rect::new(15.0, 20.0, 100.0, 10.0));
    }

    #[test]
    fn a_moved_scroll_panel_is_kept_fully_visible_when_it_fits() {
        use countersplat_ui::CollapsiblePanel;
        let mut world = World::new();
        let panel = spawn(
            &mut world,
            None,
            UiElement {
                rect: Rect::new(10.0, 150.0, 120.0, 100.0),
                drag_offset: [0.0, 40.0],
                collapsible: Some(CollapsiblePanel {
                    title: "Entities".into(),
                    header_height: 24.0,
                    collapsed: false,
                }),
                ..Default::default()
            },
        )
        .unwrap();
        spawn(
            &mut world,
            Some(panel),
            UiElement {
                rect: Rect::new(0.0, 0.0, 120.0, 76.0),
                widget: Widget::ScrollArea {
                    offset: 0.0,
                    content_height: 300.0,
                },
                ..Default::default()
            },
        )
        .unwrap();
        let frame = extract(&world, Rect::new(0.0, 0.0, 300.0, 200.0)).unwrap();
        let node = frame
            .nodes
            .iter()
            .find(|node| node.id == panel.to_string())
            .unwrap();
        assert_eq!(node.rect.y, 100.0);
        assert_eq!(node.rect.y + node.rect.height, 200.0);
        assert_eq!(node.drag_correction, [0.0, -90.0]);
        assert_eq!(
            world.get::<UiElement>(panel).unwrap().drag_offset,
            [0.0, 40.0]
        );
    }

    #[test]
    fn adapter_rejects_invalid_script_writes_without_overwriting_state() {
        let mut world = World::new();
        register(&mut world).unwrap();
        let entity = spawn(&mut world, None, UiElement::default()).unwrap();
        let old = world
            .read_component_value(entity, COMPONENT_NAME)
            .unwrap()
            .unwrap();
        let mut invalid = old.clone();
        invalid["rect"]["width"] = json!(-1);
        assert!(world
            .write_component_value(entity, COMPONENT_NAME, invalid)
            .is_err());
        assert_eq!(
            world
                .read_component_value(entity, COMPONENT_NAME)
                .unwrap()
                .unwrap(),
            old
        );
    }

    #[test]
    fn events_update_only_live_enabled_controls_and_reach_ecs_queue() {
        let mut world = World::new();
        let root = spawn(&mut world, None, UiElement::default()).unwrap();
        let toggle = spawn(
            &mut world,
            Some(root),
            UiElement {
                widget: Widget::Checkbox {
                    label: "Animate".into(),
                    checked: true,
                },
                ..Default::default()
            },
        )
        .unwrap();
        let event = UiEvent {
            id: toggle.to_string(),
            kind: UiEventKind::BoolChanged(false),
        };
        assert_eq!(
            apply_events(&mut world, [event.clone()]),
            vec![event.clone()]
        );
        assert!(matches!(
            world.get::<UiElement>(toggle).unwrap().widget,
            Widget::Checkbox { checked: false, .. }
        ));
        assert_eq!(
            world.drain_events(EVENT_CHANNEL),
            vec![serde_json::to_value(&event).unwrap()]
        );
        world.get_mut::<UiElement>(root).unwrap().enabled = false;
        assert!(apply_events(
            &mut world,
            [UiEvent {
                id: toggle.to_string(),
                kind: UiEventKind::BoolChanged(true)
            }]
        )
        .is_empty());
        world.despawn(toggle);
        assert!(apply_events(&mut world, [event]).is_empty());
    }
    #[test]
    fn collapse_preserves_children_and_blocks_stale_input() {
        use countersplat_ui::CollapsiblePanel;
        let mut world = World::new();
        let root = spawn(
            &mut world,
            None,
            UiElement {
                rect: Rect::new(10.0, 20.0, 120.0, 100.0),
                clip_children: false,
                collapsible: Some(CollapsiblePanel {
                    title: "Controls".into(),
                    header_height: 28.0,
                    collapsed: false,
                }),
                ..Default::default()
            },
        )
        .unwrap();
        let child = spawn(
            &mut world,
            Some(root),
            UiElement {
                rect: Rect::new(5.0, 7.0, 80.0, 20.0),
                widget: Widget::Checkbox {
                    label: "Sound".into(),
                    checked: true,
                },
                ..Default::default()
            },
        )
        .unwrap();
        let viewport = Rect::new(0.0, 0.0, 300.0, 300.0);
        let initial = extract(&world, viewport).unwrap();
        assert_eq!(initial.nodes.len(), 2);
        assert_eq!(
            initial
                .nodes
                .iter()
                .find(|n| n.id == child.to_string())
                .unwrap()
                .rect,
            Rect::new(15.0, 55.0, 80.0, 20.0)
        );
        let collapse = UiEvent {
            id: root.to_string(),
            kind: UiEventKind::CollapsedChanged(true),
        };
        let stale_child = UiEvent {
            id: child.to_string(),
            kind: UiEventKind::BoolChanged(false),
        };
        assert_eq!(
            apply_events(&mut world, [collapse.clone(), stale_child.clone()]),
            vec![collapse]
        );
        let hidden = extract(&world, viewport).unwrap();
        assert_eq!(hidden.nodes.len(), 1);
        assert_eq!(hidden.nodes[0].rect, Rect::new(10.0, 20.0, 120.0, 28.0));
        assert_eq!(world.get::<UiElement>(root).unwrap().rect.height, 100.0);
        assert!(matches!(
            world.get::<UiElement>(child).unwrap().widget,
            Widget::Checkbox { checked: true, .. }
        ));
        assert!(apply_events(&mut world, [stale_child.clone()]).is_empty());
        let expand = UiEvent {
            id: root.to_string(),
            kind: UiEventKind::CollapsedChanged(false),
        };
        assert_eq!(apply_events(&mut world, [expand.clone()]), vec![expand]);
        assert_eq!(extract(&world, viewport).unwrap().nodes.len(), 2);
        assert_eq!(
            apply_events(&mut world, [stale_child.clone()]),
            vec![stale_child]
        );
        assert!(matches!(
            world.get::<UiElement>(child).unwrap().widget,
            Widget::Checkbox { checked: false, .. }
        ));
    }

    #[test]
    fn nested_collapse_and_bottom_anchor_preserve_independent_state() {
        use countersplat_ui::CollapsiblePanel;
        let mut world = World::new();
        let root = spawn(
            &mut world,
            None,
            UiElement {
                rect: Rect::new(10.0, 10.0, 120.0, 100.0),
                anchor: Anchor::BottomRight,
                collapsible: Some(CollapsiblePanel {
                    title: "Outer".into(),
                    header_height: 25.0,
                    collapsed: false,
                }),
                ..Default::default()
            },
        )
        .unwrap();
        let inner = spawn(
            &mut world,
            Some(root),
            UiElement {
                rect: Rect::new(0.0, 0.0, 100.0, 60.0),
                collapsible: Some(CollapsiblePanel {
                    title: "Inner".into(),
                    header_height: 20.0,
                    collapsed: true,
                }),
                ..Default::default()
            },
        )
        .unwrap();
        let child = spawn(
            &mut world,
            Some(inner),
            UiElement {
                widget: Widget::Button {
                    label: "Hidden".into(),
                },
                ..Default::default()
            },
        )
        .unwrap();
        let viewport = Rect::new(0.0, 0.0, 300.0, 200.0);
        assert_eq!(extract(&world, viewport).unwrap().nodes.len(), 2);
        let child_click = UiEvent {
            id: child.to_string(),
            kind: UiEventKind::Clicked,
        };
        assert!(apply_events(&mut world, [child_click]).is_empty());
        assert_eq!(
            apply_events(
                &mut world,
                [UiEvent {
                    id: root.to_string(),
                    kind: UiEventKind::CollapsedChanged(true)
                }]
            )
            .len(),
            1
        );
        let frame = extract(&world, viewport).unwrap();
        assert_eq!(frame.nodes.len(), 1);
        assert_eq!(frame.nodes[0].rect, Rect::new(170.0, 165.0, 120.0, 25.0));
        assert!(apply_events(
            &mut world,
            [UiEvent {
                id: inner.to_string(),
                kind: UiEventKind::CollapsedChanged(false)
            }]
        )
        .is_empty());
        assert_eq!(
            apply_events(
                &mut world,
                [UiEvent {
                    id: root.to_string(),
                    kind: UiEventKind::CollapsedChanged(false)
                }]
            )
            .len(),
            1
        );
        assert!(
            world
                .get::<UiElement>(inner)
                .unwrap()
                .collapsible
                .as_ref()
                .unwrap()
                .collapsed
        );
        assert_eq!(extract(&world, viewport).unwrap().nodes.len(), 2);
        world.get_mut::<UiElement>(root).unwrap().enabled = false;
        assert!(apply_events(
            &mut world,
            [UiEvent {
                id: root.to_string(),
                kind: UiEventKind::CollapsedChanged(true)
            }]
        )
        .is_empty());
    }

    #[test]
    fn resized_viewport_keeps_moved_elements_reachable_and_reconciles_their_position() {
        let mut world = World::new();
        let entity = spawn(
            &mut world,
            None,
            UiElement {
                rect: Rect::new(10.0, 10.0, 100.0, 80.0),
                drag_offset: [250.0, 130.0],
                ..Default::default()
            },
        )
        .unwrap();
        let viewport = Rect::new(0.0, 0.0, 180.0, 100.0);
        let frame = extract(&world, viewport).unwrap();
        let node = &frame.nodes[0];
        assert_eq!((node.rect.x, node.rect.y), (148.0, 76.0));
        assert_eq!(node.drag_correction, [-112.0, -64.0]);
        assert_eq!((node.clip.width, node.clip.height), (32.0, 24.0));

        // The renderer includes the correction in the first movement after a resize.
        let event = UiEvent {
            id: entity.to_string(),
            kind: UiEventKind::MovedBy([
                node.drag_correction[0] - 10.0,
                node.drag_correction[1] - 5.0,
            ]),
        };
        assert_eq!(apply_events(&mut world, [event]).len(), 1);
        let next = extract(&world, viewport).unwrap();
        assert_eq!((next.nodes[0].rect.x, next.nodes[0].rect.y), (138.0, 71.0));
        assert_eq!(next.nodes[0].drag_correction, [0.0, 0.0]);
    }

    #[test]
    fn dragging_updates_screen_space_offsets_for_anchored_parents_and_children() {
        let mut world = World::new();
        let parent = spawn(
            &mut world,
            None,
            UiElement {
                rect: Rect::new(10.0, 10.0, 100.0, 80.0),
                anchor: Anchor::TopRight,
                ..Default::default()
            },
        )
        .unwrap();
        let child = spawn(
            &mut world,
            Some(parent),
            UiElement {
                rect: Rect::new(5.0, 6.0, 20.0, 20.0),
                ..Default::default()
            },
        )
        .unwrap();
        let viewport = Rect::new(0.0, 0.0, 400.0, 300.0);
        let before = extract(&world, viewport).unwrap();
        let find = |frame: &UiFrame, id: Entity| {
            frame
                .nodes
                .iter()
                .find(|node| node.id == id.to_string())
                .unwrap()
                .rect
        };
        let parent_before = find(&before, parent);
        let child_before = find(&before, child);
        let event = UiEvent {
            id: parent.to_string(),
            kind: UiEventKind::MovedBy([35.0, 22.0]),
        };
        assert_eq!(apply_events(&mut world, [event.clone()]), vec![event]);
        let after = extract(&world, viewport).unwrap();
        let parent_after = find(&after, parent);
        let child_after = find(&after, child);
        assert_eq!(
            (
                parent_after.x - parent_before.x,
                parent_after.y - parent_before.y
            ),
            (35.0, 22.0)
        );
        assert_eq!(
            (
                child_after.x - child_before.x,
                child_after.y - child_before.y
            ),
            (35.0, 22.0)
        );
        assert_eq!(world.get::<UiElement>(parent).unwrap().rect.x, 10.0);
        // Layout can be recomputed without losing the separately stored drag offset.
        world.get_mut::<UiElement>(parent).unwrap().rect.x = 20.0;
        assert_eq!(
            world.get::<UiElement>(parent).unwrap().drag_offset,
            [35.0, 22.0]
        );
        let bad = UiEvent {
            id: parent.to_string(),
            kind: UiEventKind::MovedBy([f32::NAN, 1.0]),
        };
        assert!(apply_events(&mut world, [bad]).is_empty());
        world.get_mut::<UiElement>(parent).unwrap().draggable = false;
        let locked = UiEvent {
            id: parent.to_string(),
            kind: UiEventKind::MovedBy([5.0, 5.0]),
        };
        assert!(apply_events(&mut world, [locked]).is_empty());
        assert_eq!(
            world.get::<UiElement>(parent).unwrap().drag_offset,
            [35.0, 22.0]
        );
    }
}
