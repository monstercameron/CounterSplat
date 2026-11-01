//! Compare capsule movement cost on a synthetic floor or an optional local level.
//! Run `cargo run --release -p countersplat-physics --example voxel_move_bench`.
//! For a local level, append `-- path/to/level center_x center_y center_z`.
use countersplat_physics::voxel_collision::VoxelCollider;
use serde_json::json;
use std::{hint::black_box, path::Path, time::Instant};

fn synthetic() -> VoxelCollider {
    const DEPTH: u32 = 4;
    fn node(
        index: usize,
        depth: u32,
        origin: [u32; 3],
        nodes: &mut Vec<u32>,
        leaves: &mut Vec<u32>,
    ) {
        if depth == DEPTH {
            let mut mask = 0_u64;
            for z in 0..4 {
                for y in 0..4 {
                    for x in 0..4 {
                        let [gx, gy, gz] = [origin[0] + x, origin[1] + y, origin[2] + z];
                        let floor = gy < 2 && !(gx >= 20 && gx < 26 && gz >= 16 && gz < 22);
                        let wall = gx >= 30 && gx < 32 && gy < 40 && gz >= 8 && gz < 54;
                        let box_shape = (45..49).contains(&gx)
                            && (2..14).contains(&gy)
                            && (25..35).contains(&gz);
                        if floor || wall || box_shape {
                            mask |= 1_u64 << (x + 4 * y + 16 * z);
                        }
                    }
                }
            }
            nodes[index] = (leaves.len() / 2) as u32;
            leaves.extend([mask as u32, (mask >> 32) as u32]);
        } else {
            let first = nodes.len();
            nodes.resize(first + 8, 0);
            nodes[index] = 0xff00_0000 | first as u32;
            let child_span = 4 << (DEPTH - depth - 1);
            for octant in 0..8 {
                let child =
                    [0, 1, 2].map(|axis| origin[axis] + ((octant >> axis) & 1) as u32 * child_span);
                node(first + octant, depth + 1, child, nodes, leaves);
            }
        }
    }
    let mut nodes = vec![0];
    let mut leaves = Vec::new();
    node(0, 0, [0; 3], &mut nodes, &mut leaves);
    let metadata = json!({"version":"1.1", "gridBounds":{"min":[0,0,0],"max":[3.2,3.2,3.2]},
        "voxelResolution":0.05, "leafSize":4, "treeDepth":DEPTH,
        "nodeCount":nodes.len(), "leafDataCount":leaves.len()});
    let bytes: Vec<_> = nodes
        .into_iter()
        .chain(leaves)
        .flat_map(u32::to_le_bytes)
        .collect();
    VoxelCollider::from_json_and_bin(metadata.to_string().as_bytes(), &bytes).unwrap()
}

fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let (world, center) = if args.is_empty() {
        (synthetic(), [1.4, 1.025, 1.4])
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
        ("idle", [0.0, -0.005, 0.0], 0.0),
        ("walk", [0.04, -0.005, 0.02], 0.0),
        ("air", [0.04, 0.0, 0.02], 1.2),
    ];
    const COUNT: usize = 400;
    for (name, delta, up) in cases {
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
                        .move_capsule(black_box(p), black_box(delta), 0.25, 0.675)
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
