//! Run with cargo run --release -p countersplat-scene --example lens_decode_bench.
use countersplat_scene::CameraLens;
use serde::Deserialize;
use std::{hint::black_box, time::Instant};

fn main() {
    let lens = serde_json::to_value(CameraLens::default()).unwrap();
    let mut owned = Vec::new();
    let mut borrowed = Vec::new();
    for round in 0..9 {
        for use_borrow in if round % 2 == 0 {
            [false, true]
        } else {
            [true, false]
        } {
            let start = Instant::now();
            for _ in 0..100_000 {
                let decoded = if use_borrow {
                    CameraLens::deserialize(black_box(&lens)).unwrap()
                } else {
                    serde_json::from_value::<CameraLens>(black_box(lens.clone())).unwrap()
                };
                black_box(decoded);
            }
            let time = start.elapsed().as_nanos() as f64 / 100_000.0;
            if use_borrow {
                borrowed.push(time)
            } else {
                owned.push(time)
            }
        }
    }
    owned.sort_by(f64::total_cmp);
    borrowed.sort_by(f64::total_cmp);
    println!(
        "lens decode: cloned {:.0} ns, borrowed {:.0} ns",
        owned[4], borrowed[4]
    );
}
