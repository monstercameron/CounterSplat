//! Run with cargo run --release -p countersplat-ecs --example query_bench.
use countersplat_ecs::World;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{hint::black_box, time::Instant};

#[derive(Serialize, Deserialize)]
struct Position {
    x: f32,
}

#[derive(Serialize, Deserialize)]
struct SparseMarker;

fn measure(world: &World, names: &[&str], iterations: usize) -> f64 {
    for _ in 0..20 {
        black_box(world.query_components(black_box(names)).unwrap());
    }
    let mut samples = Vec::new();
    for _ in 0..7 {
        let start = Instant::now();
        for _ in 0..iterations {
            black_box(world.query_components(black_box(names)).unwrap());
        }
        samples.push(start.elapsed().as_nanos() as f64 / iterations as f64);
    }
    samples.sort_by(f64::total_cmp);
    samples[3]
}

fn measure_join(world: &World, iterations: usize) -> f64 {
    for _ in 0..20 {
        black_box(world.query2::<Position, SparseMarker>());
    }
    let mut samples = Vec::new();
    for _ in 0..7 {
        let start = Instant::now();
        for _ in 0..iterations {
            black_box(world.query2::<Position, SparseMarker>());
        }
        samples.push(start.elapsed().as_nanos() as f64 / iterations as f64);
    }
    samples.sort_by(f64::total_cmp);
    samples[3]
}

fn main() {
    for (size, iterations) in [(200, 3000), (10_000, 120)] {
        let mut world = World::new();
        world
            .register_serde_component::<Position>("Position")
            .unwrap();
        world
            .register_serde_component::<SparseMarker>("SparseMarker")
            .unwrap();
        for index in 0..size {
            let entity = world.spawn();
            world.insert(entity, Position { x: index as f32 }).unwrap();
            world.set_dynamic(entity, "Dense", json!(true)).unwrap();
            if index % 100 == 0 {
                world.set_dynamic(entity, "Rare", json!(true)).unwrap();
                world.insert(entity, SparseMarker).unwrap();
            }
        }
        println!(
            "{size:>5} entities native sparse join: {:.0} ns/query",
            measure_join(&world, iterations)
        );
        for names in [
            &["Missing"][..],
            &["Rare"][..],
            &["Dense"][..],
            &["Position", "Rare"][..],
            &["Position"][..],
            &["SparseMarker"][..],
            &["Position", "SparseMarker"][..],
            &["Dense", "SparseMarker"][..],
        ] {
            println!(
                "{size:>5} entities {names:?}: {:.0} ns/query",
                measure(&world, names, iterations)
            );
        }
    }
}
