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

fn measure(world: &mut World, request: &str, iterations: usize) -> f64 {
    for _ in 0..200 {
        black_box(world.apply_script_json(black_box(request)).unwrap());
    }
    let mut samples = Vec::new();
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

fn measure_three(world: &mut World, requests: &[String]) -> f64 {
    for _ in 0..200 {
        for request in requests {
            black_box(world.apply_script_json(request).unwrap());
        }
    }
    let mut samples = Vec::new();
    for _ in 0..7 {
        let start = Instant::now();
        for _ in 0..12_000 {
            for request in requests {
                black_box(world.apply_script_json(black_box(request)).unwrap());
            }
        }
        samples.push(start.elapsed().as_nanos() as f64 / 12_000.0);
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
    world
        .set_dynamic(entity, "State", json!({"x":1,"enabled":true}))
        .unwrap();
    world.set_dynamic_resource("state", json!({"x":1,"enabled":true}));
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
        (
            "set dynamic",
            json!({"op":"set", "id":entity.to_string(), "name":"State", "value":{"x":2,"enabled":true}}),
        ),
        (
            "set resource",
            json!({"op":"set_resource", "name":"state", "value":{"x":2,"enabled":true}}),
        ),
        ("drain empty", json!({"op":"drain", "name":"input.key"})),
        (
            "drain next empty",
            json!({"op":"drain_next", "names":[
            "input.key", "input.mouse_delta", "input.mouse_button", "input.reset",
            "ui.action", "editor.entity_selected", "physics.ready", "physics.ground_result",
            "physics.clearance_result", "physics.trace_result", "physics.motion_resolved",
            "physics.motion_error", "physics.collision", "physics.error", "weapon.trigger",
            "weapon.aim", "weapon.reload", "weapon.equip", "weapon.mode", "weapon.hit",
            "weapon.die", "weapon.posture"
        ], "start":0}),
        ),
    ] {
        println!(
            "{name:>20}: {:.0} ns/op",
            measure(&mut world, &request.to_string(), 12_000)
        );
    }
    world
        .set_dynamic(entity, "Posture", json!({"amount":0.0,"moving":false}))
        .unwrap();
    world
        .set_dynamic(entity, "Controls", json!({"enabled":true,"speed":4.0}))
        .unwrap();
    let requests = ["Posture", "Controls", "Body"]
        .map(|name| json!({"op":"get", "id":entity.to_string(), "name":name}).to_string());
    let batch =
        json!({"op":"get_many", "id":entity.to_string(), "names":["Posture","Controls","Body"]})
            .to_string();
    println!(
        "three separate component reads: {:.0} ns",
        measure_three(&mut world, &requests)
    );
    println!(
        "three batched component reads: {:.0} ns",
        measure(&mut world, &batch, 12_000)
    );
    for (name, names) in [
        ("large + typed", vec!["Inventory", "Body"]),
        ("missing + typed", vec!["Missing", "Body", "AlsoMissing"]),
    ] {
        let batch = json!({"op":"get_many", "id":entity.to_string(), "names":names}).to_string();
        println!(
            "{name:>20} batched: {:.0} ns",
            measure(&mut world, &batch, 12_000)
        );
    }
    for (size, iterations) in [(200, 1000), (2_000, 100)] {
        let mut world = World::new();
        let mut parent = None;
        for index in 0..size {
            let entity = world.spawn();
            if index % 10 == 0 {
                parent = Some(entity);
            } else {
                world.set_parent(entity, parent).unwrap();
            }
        }
        println!(
            "{size:>4} entity hierarchy: {:.0} ns/op",
            measure(&mut world, r#"{"op":"hierarchy"}"#, iterations)
        );
    }
}
