//! Exercise the browser's scene-layout persistence with the real ECS components.
use countersplat_ecs::World;
use countersplat_ui::{Rect, ResizeLimits, UiElement};
use countersplat_ui_ecs::UiLayout;
use std::collections::BTreeMap;

#[test]
fn resized_layout_round_trips_and_legacy_offsets_still_restore() {
    let mut world = World::new();
    let id = world.spawn();
    world
        .insert(
            id,
            UiElement {
                rect: Rect::new(10.0, 20.0, 260.0, 430.0),
                resize: Some(ResizeLimits {
                    min: [250.0, 200.0],
                    max: [700.0, 1000.0],
                }),
                ..Default::default()
            },
        )
        .unwrap();
    let mut layout = UiLayout::new(&world, BTreeMap::from([("ecs_panel".into(), id)]));
    {
        let panel = world.get_mut::<UiElement>(id).unwrap();
        panel.drag_offset = [12.0, 15.0];
        panel.rect.width = 500.0;
        panel.rect.height = 700.0;
    }
    layout.changed();
    assert!(layout.take_changed());
    let saved = layout.save(&world).unwrap();
    layout.reset(&mut world);
    assert!(layout.take_changed());
    assert_eq!(world.get::<UiElement>(id).unwrap().rect.width, 260.0);
    layout.restore(&mut world, &saved).unwrap();
    let panel = world.get::<UiElement>(id).unwrap();
    assert_eq!(panel.drag_offset, [12.0, 15.0]);
    assert_eq!([panel.rect.width, panel.rect.height], [500.0, 700.0]);

    layout
        .restore(&mut world, r#"{"ecs_panel":[7.0,8.0]}"#)
        .unwrap();
    let panel = world.get::<UiElement>(id).unwrap();
    assert_eq!(panel.drag_offset, [7.0, 8.0]);
    assert_eq!([panel.rect.width, panel.rect.height], [260.0, 430.0]);

    // Out-of-range saved dimensions cannot make the component invalid.
    layout
        .restore(
            &mut world,
            r#"{"version":2,"offsets":{},"sizes":{"ecs_panel":[1,999999]}}"#,
        )
        .unwrap();
    assert!(world.get::<UiElement>(id).unwrap().validate().is_ok());
    assert_eq!(world.get::<UiElement>(id).unwrap().rect.width, 260.0);
}
