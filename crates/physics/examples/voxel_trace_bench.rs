//! Compare voxel ray tracing on a synthetic scene or an optional local level.
//! Run `cargo run --release -p countersplat-physics --example voxel_trace_bench`.
//! For a local level, append `-- path/to/level origin_x origin_y origin_z`.
mod support;
use countersplat_physics::voxel_collision::VoxelCollider;
use std::{hint::black_box, path::Path, time::Instant};

fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let (world, origin) = if args.is_empty() {
        (support::synthetic(), [1.0, 1.0, 1.4])
    } else {
        assert_eq!(
            args.len(),
            4,
            "expected level directory and ray origin x y z"
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
        ("short", [0.02, 0.0, 0.0], 0.0),
        ("near", [0.4, 0.0, 0.2], 0.0),
        ("long", [5.0, 0.05, 2.0], 0.0),
        ("open air", [5.0, 0.1, 2.0], 1.5),
    ];
    const COUNT: usize = 1000;
    for (name, delta, up) in cases {
        let rays: Vec<_> = (0..COUNT)
            .map(|i| {
                let start = [
                    origin[0] + (i % 25) as f32 * 0.012 - 0.15,
                    origin[1] + up + (i / 25 % 5) as f32 * 0.02,
                    origin[2] + (i / 125) as f32 * 0.04 - 0.14,
                ];
                (start, [0, 1, 2].map(|axis| start[axis] + delta[axis]))
            })
            .collect();
        let mut samples = Vec::new();
        let mut hash = 0xcbf29ce484222325_u64;
        for run in 0..8 {
            let begin = Instant::now();
            for &(start, end) in &rays {
                let hit = black_box(
                    world
                        .trace_segment(black_box(start), black_box(end))
                        .unwrap(),
                );
                if run == 0 {
                    let values = match hit {
                        None => [0_u32; 8],
                        Some(hit) => [
                            1,
                            hit.fraction.to_bits(),
                            hit.position[0].to_bits(),
                            hit.position[1].to_bits(),
                            hit.position[2].to_bits(),
                            hit.normal[0].to_bits(),
                            hit.normal[1].to_bits(),
                            hit.normal[2].to_bits(),
                        ],
                    };
                    for value in values {
                        hash = (hash ^ u64::from(value)).wrapping_mul(0x100000001b3);
                    }
                }
            }
            if run > 0 {
                samples.push(begin.elapsed().as_secs_f64() * 1_000_000.0 / COUNT as f64);
            }
        }
        samples.sort_by(f64::total_cmp);
        println!(
            "{name}: {:.2} us/ray (median of 7 x {COUNT}), checksum {hash:016x}",
            samples[3]
        );
    }
}
