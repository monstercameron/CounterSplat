//! Run with cargo run --locked --release -p countersplat-scripting-lua --example system_schedule_bench.
use countersplat_ecs::{WorldOperation, WorldOutcome};
use countersplat_scripting_lua::{LuaRuntime, ScriptOptions, WorldBridge};
use std::{hint::black_box, time::Instant};

struct Sink;
impl WorldBridge for Sink {
    fn request(&mut self, _: WorldOperation) -> Result<WorldOutcome, String> {
        Ok(WorldOutcome::Applied)
    }
}

fn main() {
    for count in [0, 1, 16, 100, 1000] {
        let runtime = LuaRuntime::new(
            Sink,
            ScriptOptions {
                instruction_limit: None,
                ..ScriptOptions::default()
            },
        )
        .unwrap();
        runtime
            .execute(&format!(
                r#"
            for i=1,{count} do ecs.system("system_"..i, function(dt) end) end
        "#
            ))
            .unwrap();
        let iterations = if count <= 16 {
            20_000
        } else if count <= 100 {
            5_000
        } else {
            500
        };
        for _ in 0..100 {
            runtime.run_systems(1.0 / 60.0).unwrap();
        }
        let mut samples = Vec::new();
        for _ in 0..9 {
            let start = Instant::now();
            for _ in 0..iterations {
                black_box(runtime.run_systems(1.0 / 60.0).unwrap());
            }
            samples.push(start.elapsed().as_nanos() as f64 / iterations as f64);
        }
        samples.sort_by(f64::total_cmp);
        println!("{count} systems: {:.0} ns/pass", samples[4]);
    }
}
