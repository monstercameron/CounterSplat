//! Compare dynamic component reads and writes before and after change tracking.
use countersplat_ecs::{Value, World};
use serde_json::json;
use std::{hint::black_box, time::Instant};

fn main() {
    let mut world = World::new();
    let entity = world.spawn();
    world
        .set_dynamic(
            entity,
            "Settings",
            json!({"speed": 4, "enabled": true,
        "bindings": {"forward": "KeyW", "backward": "KeyS"}}),
        )
        .unwrap();
    let mut samples = Vec::new();
    for mode in ["get", "set", "field", "if_changed"] {
        samples.clear();
        let token = world
            .read_component_if_changed(entity, "Settings", None)
            .unwrap()
            .unwrap()
            .revision;
        for _ in 0..9 {
            let start = Instant::now();
            for i in 0..250_000 {
                match mode {
                    "get" => {
                        black_box(world.get_dynamic(black_box(entity), "Settings"));
                    }
                    "set" => {
                        world
                            .set_dynamic(
                                black_box(entity),
                                "Settings",
                                black_box(json!({"speed": i, "enabled": true,
                            "bindings": {"forward": "KeyW", "backward": "KeyS"}})),
                            )
                            .unwrap();
                    }
                    "field" => {
                        world
                            .write_component_field(
                                black_box(entity),
                                "Settings",
                                "speed",
                                black_box(Value::from(i)),
                            )
                            .unwrap();
                    }
                    _ => {
                        black_box(
                            world
                                .read_component_if_changed(
                                    black_box(entity),
                                    "Settings",
                                    black_box(token.as_deref()),
                                )
                                .unwrap(),
                        );
                    }
                }
            }
            samples.push(start.elapsed().as_nanos() as f64 / 250_000.0);
        }
        samples.sort_by(f64::total_cmp);
        println!("{mode}: {:.1} ns/op", samples[4]);
    }
}
