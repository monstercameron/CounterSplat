//! Run with cargo run --release -p countersplat-ui --example frame_history_bench.
use countersplat_ui::FrameHistory;
use std::{
    hint::black_box,
    time::{Duration, Instant},
};

fn main() {
    let mut history = FrameHistory::new();
    for index in 0..=240 {
        history.record(Duration::from_millis(index * 250));
    }
    let now = Duration::from_secs(60);
    let iterations = 20_000;
    let mut samples = Vec::new();
    for _ in 0..7 {
        let start = Instant::now();
        for _ in 0..iterations {
            black_box(history.points(black_box(now)));
        }
        samples.push(start.elapsed().as_nanos() as f64 / iterations as f64);
    }
    samples.sort_by(f64::total_cmp);
    println!(
        "240 frame-history buckets, fresh buffer: {:.0} ns/points",
        samples[3]
    );
    let mut buffer = Vec::new();
    let mut samples = Vec::new();
    for _ in 0..7 {
        let start = Instant::now();
        for _ in 0..iterations {
            history.write_points(black_box(now), &mut buffer);
            black_box(&buffer);
        }
        samples.push(start.elapsed().as_nanos() as f64 / iterations as f64);
    }
    samples.sort_by(f64::total_cmp);
    println!(
        "240 frame-history buckets, reused buffer: {:.0} ns/points",
        samples[3]
    );
}
