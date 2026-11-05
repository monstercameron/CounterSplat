//! Prints floor height, free space and the line of fire from the eye at points placed relative to a camera pose, to
//! choose where a prop can stand and still be shot from there.
//!
//! ```text
//! cargo run --release -p countersplat-physics --example floor_probe -- assets/levels/<id> eye_x eye_y eye_z yaw [forward:lateral ...]
//! ```
use countersplat_physics::voxel_collision::VoxelCollider;
use std::path::Path;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = Path::new(&args[0]);
    let world = VoxelCollider::from_json_and_bin(
        &std::fs::read(dir.join("scene.voxel.json")).unwrap(),
        &std::fs::read(dir.join("scene.voxel.bin")).unwrap(),
    )
    .unwrap();
    let n = |i: usize| args[i].parse::<f32>().unwrap();
    let (eye, yaw) = ([n(1), n(2), n(3)], n(4));
    let (s, c) = yaw.sin_cos();
    println!("spawn floor {:?}", world.ground_height(eye, 0.1, 4.0));
    for spec in &args[5..] {
        let (forward, lateral) = spec.split_once(':').unwrap();
        let (forward, lateral): (f32, f32) = (forward.parse().unwrap(), lateral.parse().unwrap());
        let x = eye[0] + s * forward + c * lateral;
        let z = eye[2] - c * forward + s * lateral;
        let floor = world.ground_height([x, eye[1], z], 0.1, 4.0).unwrap();
        // Free space for a 1 m tall, 0.6 m wide body standing on that floor, and along the wall's length.
        let clear = floor.map(|f| {
            (-3..=3)
                .map(|k| {
                    let along = k as f32 * 0.35;
                    world.capsule_clear([x + c * along, f + 0.6, z + s * along], 0.25, 0.3).unwrap()
                })
                .collect::<Vec<_>>()
        });
        // Line of fire from the eye to points along the top of a 1 m wall facing the eye (where the cans stand).
        let sight = floor.map(|f| {
            let (dx, dz) = (eye[0] - x, eye[2] - z);
            let d = (dx * dx + dz * dz).sqrt().max(1e-6);
            let (ax, az) = (-dz / d, dx / d);
            (-2..=2)
                .map(|k| {
                    let along = k as f32 * 0.3;
                    let target = [x + ax * along, f + 1.06, z + az * along];
                    let hit = world.trace_segment(eye, target).unwrap();
                    hit.map_or(true, |_| false)
                })
                .collect::<Vec<_>>()
        });
        println!(
            "forward {forward:>4} lateral {lateral:>5}: world ({x:7.3}, {z:7.3}) floor {floor:?} clear along wall {clear:?} clear sight {sight:?}"
        );
    }
}
