//! Compare the native ECS worker's synchronous read round trips with one batched read.
//! Run with `cargo run --release -p countersplat-threaded-demo --example worker_latency`.
use countersplat_ecs::{Entity, World, WorldOperation, WorldOutcome};
use countersplat_runtime::native::Worker;
use serde_json::{json, Value};
use std::{
    hint::black_box,
    sync::mpsc,
    time::{Duration, Instant},
};

const NAMES: [&str; 4] = ["Transform", "Velocity", "Controls", "Body"];
const WARMUP: usize = 200;
const SAMPLES: usize = 4000;
const TIMEOUT: Duration = Duration::from_secs(10);

fn populate(world: &mut World) -> Entity {
    let entity = world.spawn();
    for (name, value) in [
        ("Transform", json!({"position":[1.0,2.0,3.0],"yaw":0.5})),
        ("Velocity", json!({"linear":[0.1,0.0,-0.1]})),
        ("Controls", json!({"enabled":true,"speed":4.0})),
        ("Body", json!({"grounded":true,"radius":0.3})),
    ] {
        world.set_dynamic(entity, name, value).unwrap();
    }
    entity
}

fn read(entity: Entity, name: &str) -> WorldOperation {
    WorldOperation::ReadComponent {
        entity,
        name: name.to_owned(),
    }
}
fn batch(entity: Entity) -> WorldOperation {
    WorldOperation::ReadComponents {
        entity,
        names: NAMES.iter().map(|name| (*name).to_owned()).collect(),
    }
}
fn inspect(outcome: WorldOutcome) -> Value {
    match outcome {
        WorldOutcome::Value(Some(value)) => value,
        other => panic!("unexpected read: {other:?}"),
    }
}
fn stats(samples: &mut [f64]) -> (f64, f64, f64) {
    samples.sort_by(f64::total_cmp);
    let percentile = |p: f64| samples[((samples.len() - 1) as f64 * p).ceil() as usize];
    (percentile(0.50), percentile(0.95), percentile(0.99))
}
fn main() {
    let (entity_tx, entity_rx) = mpsc::sync_channel(1);
    let mut worker = Worker::spawn(
        "ecs-latency-benchmark",
        16,
        move || {
            let mut world = World::new();
            entity_tx
                .send(populate(&mut world))
                .map_err(|e| e.to_string())?;
            Ok(world)
        },
        |world: &mut World, operation| world.apply_operation(operation),
    )
    .unwrap();
    worker.wait_ready(TIMEOUT).unwrap();
    let entity = entity_rx.recv_timeout(TIMEOUT).unwrap();
    let client = worker.client();
    let call = |operation| {
        client
            .try_submit(operation)
            .unwrap()
            .wait_timeout(TIMEOUT)
            .unwrap()
    };
    let batch_value = inspect(call(batch(entity)));
    for name in NAMES {
        assert_eq!(inspect(call(read(entity, name))), batch_value[name]);
    }

    let mut local = World::new();
    let local_entity = populate(&mut local);
    let mut samples: [Vec<f64>; 4] = std::array::from_fn(|_| Vec::with_capacity(SAMPLES));
    for round in 0..(WARMUP + SAMPLES) {
        // Alternate worker requests independently of the local controls so
        // neither case systematically runs after a longer idle period.
        let order = if round % 2 == 0 {
            [0, 1, 2, 3]
        } else {
            [1, 0, 3, 2]
        };
        for mode in order {
            let start = Instant::now();
            match mode {
                0 => {
                    for name in NAMES {
                        black_box(call(read(entity, name)));
                    }
                }
                1 => {
                    black_box(call(batch(entity)));
                }
                2 => {
                    for name in NAMES {
                        black_box(local.apply_operation(read(local_entity, name)).unwrap());
                    }
                }
                _ => {
                    black_box(local.apply_operation(batch(local_entity)).unwrap());
                }
            }
            if round >= WARMUP {
                samples[mode].push(start.elapsed().as_secs_f64() * 1_000_000.0);
            }
        }
    }
    for (label, sample) in [
        "worker: four replies",
        "worker: one batched reply",
        "local: four reads",
        "local: one batched read",
    ]
    .into_iter()
    .zip(&mut samples)
    {
        let (p50, p95, p99) = stats(sample);
        println!(
            "{label}: p50 {p50:.2} us, p95 {p95:.2} us, p99 {p99:.2} us ({} samples)",
            sample.len()
        );
    }
    worker.shutdown().unwrap();
}
