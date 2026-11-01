//! Run with cargo run --release -p countersplat-render --example stream_selection_bench.
//! Measures CPU-side selection only, without network, GPU uploads, or rendering.
use countersplat_render::{
    camera,
    lod::{Leaf, LodMeta, Range},
    stream::{DrawRegion, LevelStream, StreamSettings},
};
use std::{hint::black_box, time::Instant};

fn chunk_meta() -> Vec<u8> {
    let book = (0..256)
        .map(|i| format!("{}", i as f32 * 0.01))
        .collect::<Vec<_>>()
        .join(",");
    format!(r#"{{"version":2,"count":1,"means":{{"mins":[-1,-1,-1],"maxs":[1,1,1]}},"scales":{{"codebook":[{book}]}},"sh0":{{"codebook":[{book}]}},"shN":{{"count":64,"codebook":[{book}]}}}}"#).into_bytes()
}

fn grid(side: usize) -> LodMeta {
    let leaves = (0..side * side)
        .map(|i| {
            let x = (i % side) as f32 * 8.0 - side as f32 * 4.0;
            let z = (i / side) as f32 * 8.0 - side as f32 * 4.0;
            Leaf {
                min: [x, -1.0, z],
                max: [x + 7.0, 5.0, z + 7.0],
                lods: (0..3)
                    .map(|lod| {
                        Some(Range {
                            file: lod,
                            offset: i as u32 * 100,
                            count: [100, 40, 10][lod],
                        })
                    })
                    .collect(),
            }
        })
        .collect();
    LodMeta {
        counts: vec![
            (side * side * 100) as u64,
            (side * side * 40) as u64,
            (side * side * 10) as u64,
        ],
        filenames: vec![
            "0/meta.json".into(),
            "1/meta.json".into(),
            "2/meta.json".into(),
        ],
        leaves,
    }
}

fn main() {
    let fixture = LodMeta::parse(include_bytes!("../tests/fixtures/lod-meta.json")).unwrap();
    for (name, meta) in [
        ("fixture", fixture),
        ("grid_1024", grid(32)),
        ("grid_10000", grid(100)),
    ] {
        for (region_name, draw_region) in [
            ("radial", DrawRegion::Radial),
            (
                "forward",
                DrawRegion::ForwardEllipse {
                    rear_fraction: 0.4,
                    side_fraction: 0.7,
                },
            ),
        ] {
            let settings = StreamSettings {
                draw_region,
                max_distance_m: 100.0,
                near_lod0_m: 8.0,
                center_lod0_m: 40.0,
                offscreen_keep_m: 30.0,
                ..StreamSettings::default()
            };
            let metas = vec![Some(chunk_meta()); meta.filenames.len()];
            let projection = camera::perspective(60.0_f32.to_radians(), 16.0 / 9.0, 0.05, 2000.0);
            let views: Vec<_> = (0..240)
                .map(|frame| {
                    let angle = frame as f32 * 0.012;
                    let eye = [frame as f32 * 0.06 - 7.0, 1.7, 0.0];
                    let forward = [angle.cos(), 0.0, angle.sin()];
                    let target = [eye[0] + forward[0], eye[1], eye[2] + forward[2]];
                    let view = camera::look_at(eye, target, [0.0, 1.0, 0.0]);
                    (eye, forward, camera::mul(&projection, &view))
                })
                .collect();
            let mut samples = Vec::new();
            let mut fingerprint = 0;
            let iterations = 1200;
            for _ in 0..5 {
                let mut stream = LevelStream::new(meta.clone(), &metas, settings).unwrap();
                for frame in 0..120 {
                    let (eye, forward, clip) = views[frame % views.len()];
                    stream.select(eye, forward, Some(&clip), frame as f32 / 60.0);
                }
                let start = Instant::now();
                for frame in 120..(120 + iterations) {
                    let (eye, forward, clip) = views[frame % views.len()];
                    stream.select(
                        black_box(eye),
                        black_box(forward),
                        Some(black_box(&clip)),
                        frame as f32 / 60.0,
                    );
                }
                samples.push(start.elapsed().as_nanos() as f64 / iterations as f64);
                let stats = stream.stats();
                fingerprint = stats.wanted_splats ^ ((stats.leaves_hidden as u64) << 32);
                black_box(&stream);
            }
            samples.sort_by(f64::total_cmp);
            println!("{name:>10} {region_name:>7}: {:>9.0} ns/select; leaves={}; fingerprint={fingerprint}", samples[2], meta.leaves.len());
        }
    }
}
