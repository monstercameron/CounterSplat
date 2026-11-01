//! Run with cargo run --release -p countersplat-ui-ecs --example layout_bench.
use countersplat_ecs::World;
use countersplat_ui::{Rect, TextAlign, UiElement, Widget};
use std::{hint::black_box, time::Instant};

fn measure(world: &World, iterations: usize) -> f64 {
    let viewport = Rect::new(0.0, 0.0, 1920.0, 1080.0);
    for _ in 0..20 {
        black_box(countersplat_ui_ecs::extract(world, viewport).unwrap());
    }
    let mut samples = Vec::new();
    for _ in 0..7 {
        let start = Instant::now();
        for _ in 0..iterations {
            black_box(countersplat_ui_ecs::extract(world, viewport).unwrap());
        }
        samples.push(start.elapsed().as_nanos() as f64 / iterations as f64);
    }
    samples.sort_by(f64::total_cmp);
    samples[3]
}

fn main() {
    for (size, iterations, padding) in [(200, 500, 0), (2_000, 50, 0), (200, 500, 10_000)] {
        let mut world = World::new();
        for _ in 0..padding {
            world.spawn();
        }
        let mut parent = None;
        for index in 0..size {
            let entity = world.spawn();
            let mut element = UiElement {
                rect: Rect::new(
                    (index % 15) as f32 * 3.0,
                    (index % 12) as f32 * 2.0,
                    80.0,
                    24.0,
                ),
                clip_children: false,
                ..UiElement::default()
            };
            if index % 4 != 0 {
                element.widget = Widget::Text {
                    text: format!("Item {index}"),
                    align: TextAlign::default(),
                };
            }
            world.insert(entity, element).unwrap();
            if index % 10 == 0 {
                parent = Some(entity);
            } else {
                world.set_parent(entity, parent).unwrap();
            }
        }
        println!(
            "{size} UI entities, {padding} unrelated slots: {:.0} ns/extract",
            measure(&world, iterations)
        );
    }
}
