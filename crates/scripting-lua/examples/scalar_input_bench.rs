//! Run with cargo run --locked --release -p countersplat-scripting-lua --example scalar_input_bench.
use countersplat_ecs::{WorldOperation, WorldOutcome};
use countersplat_scripting_lua::{LuaRuntime, ScriptOptions, WorldBridge};
use std::{hint::black_box, time::Instant};

struct Sink;
impl WorldBridge for Sink {
    fn request(&mut self, operation: WorldOperation) -> Result<WorldOutcome, String> {
        black_box(operation);
        Ok(WorldOutcome::Applied)
    }
}

fn main() {
    let runtime = LuaRuntime::new(
        Sink,
        ScriptOptions {
            instruction_limit: None,
            ..ScriptOptions::default()
        },
    )
    .unwrap();
    runtime
        .execute(
            r#"
        local array = ecs.array(); array[1], array[2], array[3] = 1.25, 2.5, -3.75
        local payload = {position=array, enabled=true}
        ecs.system("boolean", function() for i=1,1000 do ecs.set_resource("test", false) end end)
        ecs.system("integer", function() for i=1,1000 do ecs.set_resource("test", 42) end end)
        ecs.system("number", function() for i=1,1000 do ecs.set_resource("test", 1.25) end end)
        ecs.system("string", function() for i=1,1000 do ecs.set_resource("test", "value") end end)
        ecs.system("vector", function() for i=1,1000 do ecs.set_resource("test", array) end end)
        ecs.system("object", function() for i=1,1000 do ecs.set_resource("test", payload) end end)
    "#,
        )
        .unwrap();
    for name in ["boolean", "integer", "number", "string", "vector", "object"] {
        for _ in 0..3 {
            runtime.run_system(name, 1.0 / 60.0).unwrap();
        }
        let mut samples = Vec::new();
        for _ in 0..11 {
            let start = Instant::now();
            runtime.run_system(name, 1.0 / 60.0).unwrap();
            samples.push(start.elapsed().as_nanos() as f64 / 1_000.0);
        }
        samples.sort_by(f64::total_cmp);
        println!("{name}: {:.0} ns/write", samples[5]);
    }
}
