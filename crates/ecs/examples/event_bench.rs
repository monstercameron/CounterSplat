//! Run with cargo run --release -p countersplat-ecs --example event_bench.
use countersplat_ecs::{Value, World};
use std::{hint::black_box, time::Instant};

fn measure(burst: usize, iterations: usize) -> f64 {
    let mut world = World::new();
    let mut samples = Vec::new();
    for _ in 0..7 {
        let start = Instant::now();
        for _ in 0..iterations {
            for i in 0..burst {
                world.emit_event("weapon.shot", Value::from(i as u64));
            }
            black_box(world.drain_events("weapon.shot"));
        }
        samples.push(start.elapsed().as_nanos() as f64 / iterations as f64);
    }
    samples.sort_by(f64::total_cmp);
    samples[3]
}

fn main() {
    for (burst, iterations) in [(1, 50_000), (32, 10_000), (256, 1_000)] {
        println!(
            "{burst:>3} events per drain: {:.0} ns/burst",
            measure(burst, iterations)
        );
    }
}
