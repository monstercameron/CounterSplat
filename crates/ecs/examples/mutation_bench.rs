//! Run with cargo run --release -p countersplat-ecs --example mutation_bench.
use countersplat_ecs::World;
use std::{hint::black_box, time::Instant};

#[derive(Clone, Copy)]
struct Counter(u64);

fn main() {
    let mut world = World::new();
    let entity = world.spawn();
    world.insert(entity, Counter(0)).unwrap();
    for mode in [
        "borrow_mut",
        "insert",
        "borrow_mut_tracked",
        "insert_tracked",
    ] {
        if mode.ends_with("_tracked") {
            world.track_component_changes::<Counter>();
        }
        let mut samples = Vec::new();
        for _ in 0..9 {
            let start = Instant::now();
            for i in 0..500_000 {
                if mode.starts_with("borrow_mut") {
                    let value = black_box(&mut world)
                        .get_mut::<Counter>(black_box(entity))
                        .unwrap();
                    value.0 = black_box(value.0.wrapping_add(1));
                } else {
                    black_box(&mut world)
                        .insert(black_box(entity), Counter(black_box(i)))
                        .unwrap();
                }
            }
            samples.push(start.elapsed().as_nanos() as f64 / 500_000.0);
        }
        samples.sort_by(f64::total_cmp);
        println!("{mode}: {:.1} ns/op", samples[4]);
    }
}
