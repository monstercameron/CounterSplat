//! Run with cargo run --release -p countersplat-scene-ecs --example field_write_bench.
use countersplat_ecs::World;
use countersplat_scene::{PhysicsBody, PlayerCamera};
use serde_json::{json, Value};
use std::{hint::black_box, time::Instant};

fn measure(world: &mut World, request: &str, iterations: usize) -> f64 {
    for _ in 0..200 {
        black_box(world.apply_script_json(request).unwrap());
    }
    let mut samples = Vec::new();
    for _ in 0..9 {
        let start = Instant::now();
        for _ in 0..iterations {
            black_box(world.apply_script_json(black_box(request)).unwrap());
        }
        samples.push(start.elapsed().as_nanos() as f64 / iterations as f64);
    }
    samples.sort_by(f64::total_cmp);
    samples[4]
}

fn main() {
    let mut world = World::new();
    countersplat_scene_ecs::register(&mut world).unwrap();
    let entity = world.spawn();
    world.insert(entity, PhysicsBody::default()).unwrap();
    world.insert(entity, PlayerCamera::default()).unwrap();
    for (name, field, value) in [
        ("PhysicsBody", "velocity", json!([1.0, -2.0, 3.0])),
        ("PlayerCamera", "position", json!([1.0, 2.0, 3.0])),
    ] {
        let mut component: Value = world.read_component_value(entity, name).unwrap().unwrap();
        component[field] = value.clone();
        let full = json!({"op":"set", "id":entity.to_string(), "name":name, "value":component})
            .to_string();
        let partial = json!({"op":"set_field", "id":entity.to_string(), "name":name, "field":field, "value":value}).to_string();
        println!(
            "{name}.{field}: full {:.0} ns, field {:.0} ns",
            measure(&mut world, &full, 20_000),
            measure(&mut world, &partial, 20_000)
        );
    }
    for (name, fields) in [
        (
            "PhysicsBody",
            json!({"velocity":[0.0,0.0,0.0],"grounded":true}),
        ),
        (
            "PlayerCamera",
            json!({"view_offset":[0.1,0.2,0.3],"view_bob_pitch":0.01,"view_bob_roll":0.02}),
        ),
    ] {
        let mut component = world.read_component_value(entity, name).unwrap().unwrap();
        component
            .as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        let full = json!({"op":"set", "id":entity.to_string(), "name":name, "value":component})
            .to_string();
        let partial =
            json!({"op":"set_fields", "id":entity.to_string(), "name":name, "fields":fields})
                .to_string();
        println!(
            "{name} multiple fields: full {:.0} ns, fields {:.0} ns",
            measure(&mut world, &full, 20_000),
            measure(&mut world, &partial, 20_000)
        );
    }
}
