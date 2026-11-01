//! Compare capsule movement cost on a synthetic floor or an optional local level.
//! Run `cargo run --release -p countersplat-physics --example voxel_move_bench`.
//! For a local level, append `-- path/to/level center_x center_y center_z`.
use countersplat_physics::voxel_collision::VoxelCollider;
mod support;
use std::{hint::black_box, path::Path, time::Instant};

fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let (world, center) = if args.is_empty() {
        (support::synthetic(), [1.4, 1.025, 1.4])
    } else {
        assert_eq!(
            args.len(),
            4,
            "expected level directory and capsule center x y z"
        );
        let dir = Path::new(&args[0]);
        (
            VoxelCollider::from_json_and_bin(
                &std::fs::read(dir.join("scene.voxel.json")).unwrap(),
                &std::fs::read(dir.join("scene.voxel.bin")).unwrap(),
            )
            .unwrap(),
            [
                args[1].parse().unwrap(),
                args[2].parse().unwrap(),
                args[3].parse().unwrap(),
            ],
        )
    };
    let cases = [
        ("idle", [0.0, -0.005, 0.0], 0.0, 0.25, 0.675),
        ("walk", [0.04, -0.005, 0.02], 0.0, 0.25, 0.675),
        ("air", [0.04, 0.0, 0.02], 1.2, 0.25, 0.675),
        ("small sphere", [0.01, -0.005, 0.0], 0.0, 0.05, 0.0),
        ("crouched", [0.04, -0.005, 0.02], 0.0, 0.25, 0.35),
        ("wide body", [0.04, -0.005, 0.02], 0.0, 0.5, 0.9),
        ("long empty sweep", [6.0, 0.0, 0.0], 100.0, 0.25, 0.675),
    ];
    const COUNT: usize = 400;
    for (name, delta, up, radius, half_height) in cases {
        let positions: Vec<_> = (0..COUNT)
            .map(|i| {
                [
                    center[0] + (i % 20) as f32 * 0.015 - 0.15,
                    center[1] + up,
                    center[2] + (i / 20) as f32 * 0.015 - 0.15,
                ]
            })
            .collect();
        let mut samples = Vec::new();
        let mut hash = 0xcbf29ce484222325_u64;
        for run in 0..8 {
            let start = Instant::now();
            for &p in &positions {
                let result = black_box(
                    world
                        .move_capsule(black_box(p), black_box(delta), radius, half_height)
                        .unwrap(),
                );
                if run == 0 {
                    for v in result {
                        hash = (hash ^ v.to_bits() as u64).wrapping_mul(0x100000001b3);
                    }
                }
            }
            if run > 0 {
                samples.push(start.elapsed().as_secs_f64() * 1_000_000.0 / COUNT as f64);
            }
        }
        samples.sort_by(f64::total_cmp);
        println!(
            "{name}: {:.2} us/move (median of 7 x {COUNT}), checksum {hash:016x}",
            samples[3]
        );
    }
}
