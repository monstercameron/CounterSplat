//! Voxel query cost with warm and cold caches on a real level, for octree traversal changes.
//!
//! ```text
//! cargo run --release -p countersplat-physics --example voxel_cache_bench -- assets/levels/<id>
//! ```
//!
//! Queries are spread over the level (positions on a fixed pseudo-random sequence, each standing on the ground the
//! level's own probe finds) so one batch touches many octree paths. "warm" repeats the batch with its nodes cached;
//! "cold" streams 64 MiB through the caches before each batch, as the renderer and scripts do between a frame's
//! collision queries. Each workload prints a checksum of its results, which must not change with a traversal change.
use countersplat_physics::voxel_collision::VoxelCollider;
use std::{hint::black_box, path::Path, time::Instant};

fn main() {
    let dir = std::env::args().nth(1).expect("level directory");
    let dir = Path::new(&dir);
    let world = VoxelCollider::from_json_and_bin(
        &std::fs::read(dir.join("scene.voxel.json")).unwrap(),
        &std::fs::read(dir.join("scene.voxel.bin")).unwrap(),
    )
    .unwrap();
    let json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("scene.voxel.json")).unwrap()).unwrap();
    let bound = |key: &str| -> [f32; 3] {
        let v = &json["gridBounds"][key];
        [0, 1, 2].map(|i| v[i].as_f64().unwrap() as f32)
    };
    let (min, max) = (bound("min"), bound("max"));
    let mut state = 0x9e37_79b9u32;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        (state >> 8) as f32 / 16_777_216.0
    };
    // Standing positions: a random x/z, dropped from the top of the grid until the ground probe finds a floor.
    let mut feet = Vec::new();
    while feet.len() < 256 {
        let x = min[0] + (max[0] - min[0]) * next();
        let z = min[2] + (max[2] - min[2]) * next();
        let mut y = max[1] - 0.5;
        while y > min[1] + 0.5 {
            if let Ok(Some(ground)) = world.ground_height([x, y, z], 0.2, 2.0) {
                feet.push([x, ground, z]);
                break;
            }
            y -= 1.9;
        }
    }
    let directions: Vec<[f32; 3]> = (0..feet.len())
        .map(|_| {
            let yaw = next() * std::f32::consts::TAU;
            let pitch = (next() - 0.5) * 0.6;
            [yaw.cos() * pitch.cos(), pitch.sin(), yaw.sin() * pitch.cos()]
        })
        .collect();
    let flush = vec![1u8; 64 << 20];
    let mut sink = 0u64;
    let mut cold = || {
        // Read and write every cache line of a buffer larger than the last-level cache.
        let mut sum = 0u64;
        for chunk in flush.chunks(64) {
            sum = sum.wrapping_add(u64::from(black_box(chunk[0])));
        }
        sink = sink.wrapping_add(sum);
    };
    fn mix(hash: &mut u64, value: u32) {
        *hash = (*hash ^ u64::from(value)).wrapping_mul(0x100000001b3);
    }
    type Workload<'a> = Box<dyn Fn(&mut u64) + 'a>;
    let workloads: Vec<(&str, Workload)> = vec![
        (
            "hitscan 25 m",
            Box::new(|hash| {
                for (f, d) in feet.iter().zip(&directions) {
                    let eye = [f[0], f[1] + 1.6, f[2]];
                    let end = [0, 1, 2].map(|i| eye[i] + d[i] * 25.0);
                    if let Some(hit) = world.trace_segment(eye, end).unwrap() {
                        mix(hash, hit.fraction.to_bits());
                    } else {
                        mix(hash, 0);
                    }
                }
            }),
        ),
        (
            "ground probe",
            Box::new(|hash| {
                for f in &feet {
                    let probe = world.ground_height([f[0], f[1] + 1.6, f[2]], 0.2, 2.0).unwrap();
                    mix(hash, probe.map_or(0, f32::to_bits));
                }
            }),
        ),
        (
            "capsule walk",
            Box::new(|hash| {
                for (f, d) in feet.iter().zip(&directions) {
                    let centre = [f[0], f[1] + 0.25 + 0.675 + 0.01, f[2]];
                    let moved = world.move_capsule(centre, [d[0] * 0.06, -0.005, d[2] * 0.06], 0.25, 0.675).unwrap();
                    moved.iter().for_each(|v| mix(hash, v.to_bits()));
                }
            }),
        ),
        (
            "minimap slice",
            Box::new(|hash| {
                for f in feet.iter().step_by(16) {
                    let pixels = world.horizontal_slice([f[0], f[2]], f[1] + 1.0, 12.0, 0.4, 128).unwrap();
                    pixels.iter().for_each(|&p| mix(hash, u32::from(p)));
                }
            }),
        ),
    ];
    for (name, run) in &workloads {
        let mut hash = 0xcbf29ce484222325u64;
        run(&mut hash);
        let mut timed = |flush_first: bool| {
            let mut samples = Vec::new();
            for _ in 0..9 {
                if flush_first {
                    cold();
                }
                let begin = Instant::now();
                let mut ignored = 0u64;
                run(&mut ignored);
                samples.push(begin.elapsed().as_secs_f64() * 1e6);
                black_box(ignored);
            }
            samples.sort_by(f64::total_cmp);
            samples[4]
        };
        let warm = timed(false);
        let cold_us = timed(true);
        println!("{name:<14} warm {warm:>9.1} us  cold {cold_us:>9.1} us  checksum {hash:016x}");
    }
    black_box(sink);
}
