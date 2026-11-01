//! Run with cargo run --release -p countersplat-scene-ecs --example lens_revision_bench.
use countersplat_ecs::World;
use countersplat_scene::CameraLens;
use serde_json::json;
use std::{hint::black_box, time::Instant};

fn measure(world: &mut World, request: &str) -> f64 {
    for _ in 0..200 {
        black_box(world.apply_script_json(request).unwrap());
    }
    let mut samples = Vec::new();
    for _ in 0..9 {
        let start = Instant::now();
        for _ in 0..30_000 {
            black_box(world.apply_script_json(black_box(request)).unwrap());
        }
        samples.push(start.elapsed().as_nanos() as f64 / 30_000.0);
    }
    samples.sort_by(f64::total_cmp);
    samples[4]
}
fn main() {
    let mut world = World::new();
    countersplat_scene_ecs::register(&mut world).unwrap();
    let entity = world.spawn();
    world.insert(entity, CameraLens::default()).unwrap();
    let revision = world
        .read_component_if_changed(entity, "CameraLens", None)
        .unwrap()
        .unwrap()
        .revision
        .unwrap();
    let full = json!({"op":"get","id":entity.to_string(),"name":"CameraLens"}).to_string();
    let unchanged=json!({"op":"get_if_changed","id":entity.to_string(),"name":"CameraLens","revision":revision}).to_string();
    println!(
        "lens read: full {:.0} ns, unchanged {:.0} ns",
        measure(&mut world, &full),
        measure(&mut world, &unchanged)
    );
}
