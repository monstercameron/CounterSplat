//! Run with cargo run --release -p countersplat-ecs --example script_bridge_bench.
use countersplat_ecs::World;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{hint::black_box, time::Instant};

#[derive(Serialize, Deserialize)]
struct Body {
    position: [f32; 3],
    velocity: [f32; 3],
    mass: f32,
    radius: f32,
    height: f32,
    grounded: bool,
    collision: bool,
}

fn measure(world: &mut World, request: &str) -> f64 {
    for _ in 0..200 {
        black_box(world.apply_script_json(black_box(request)).unwrap());
    }
    let mut samples = Vec::new();
    let iterations = 12_000;
    for _ in 0..7 {
        let start = Instant::now();
        for _ in 0..iterations {
            black_box(world.apply_script_json(black_box(request)).unwrap());
        }
        samples.push(start.elapsed().as_nanos() as f64 / iterations as f64);
    }
    samples.sort_by(f64::total_cmp);
    samples[3]
}

fn main() {
    let mut world = World::new();
    world.register_serde_component::<Body>("Body").unwrap();
    let entity = world.spawn();
    world
        .insert(
            entity,
            Body {
                position: [1.0, 2.0, 3.0],
                velocity: [0.0, -0.1, 0.0],
                mass: 80.0,
                radius: 0.35,
                height: 1.8,
                grounded: true,
                collision: true,
            },
        )
        .unwrap();
    world.set_dynamic(entity, "Inventory", json!({
        "items": (0..80).map(|id| json!({"id":id, "name":format!("item-{id}"), "count":10})).collect::<Vec<_>>()
    })).unwrap();
    world.set_dynamic_resource("scene.entities", json!({"player":entity.to_string()}));
    for (name, request) in [
        (
            "get typed",
            json!({"op":"get", "id":entity.to_string(), "name":"Body"}),
        ),
        (
            "has typed",
            json!({"op":"has", "id":entity.to_string(), "name":"Body"}),
        ),
        (
            "get large dynamic",
            json!({"op":"get", "id":entity.to_string(), "name":"Inventory"}),
        ),
        (
            "has large dynamic",
            json!({"op":"has", "id":entity.to_string(), "name":"Inventory"}),
        ),
        (
            "get resource field",
            json!({"op":"get_resource_field", "name":"scene.entities", "field":"player"}),
        ),
        ("drain empty", json!({"op":"drain", "name":"input.key"})),
    ] {
        println!(
            "{name:>20}: {:.0} ns/op",
            measure(&mut world, &request.to_string())
        );
    }
}
