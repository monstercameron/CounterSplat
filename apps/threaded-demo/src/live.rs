//! A real monotonic-time loop driving actual ECS, Lua and physics worker state.
//!
//! The host sleeps until the next simulation tick or software frame request. It deliberately
//! counts frame requests instead of pretending this headless demo presents GPU frames.

use super::{call_ecs, physics_world, EcsBridge, QUEUE_CAPACITY, TIMEOUT};
use countersplat_core::{rate::Rate, scheduler::Scheduler, time::FixedClock};
use countersplat_ecs::{Entity, World as EcsWorld, WorldOperation, WorldOutcome};
use countersplat_physics::world::{BodyHandle, BodyKind, BoxBody, World as PhysicsWorld};
use countersplat_runtime::native::Worker;
use countersplat_scripting_lua::{ScriptOptions, ScriptWorker};
use serde_json::json;
use std::{
    thread,
    time::{Duration, Instant},
};

const SCRIPT: &str = r#"
    actor = ecs.spawn()
    ecs.set(actor, "Position", { x = 0.0, y = 5.0, z = 0.0 })
    ecs.set_resource("script_ticks", 0)
    ecs.system("fixed_update", function(dt)
        ecs.set_resource("script_ticks", ecs.get_resource("script_ticks") + 1)
    end)
"#;

struct PhysicsState {
    world: PhysicsWorld,
    body: BodyHandle,
}

fn create_physics() -> Result<PhysicsState, String> {
    let mut world = physics_world()?;
    let body = world
        .add_box(BoxBody {
            kind: BodyKind::Dynamic,
            center: [0.0, 5.0, 0.0],
            half_extents: [0.5; 3],
            friction: 0.5,
            restitution: 0.0,
        })
        .map_err(str::to_owned)?;
    Ok(PhysicsState { world, body })
}

fn tick_physics(state: &mut PhysicsState, step_seconds: f32) -> Result<[f32; 3], String> {
    state.world.step(step_seconds).map_err(str::to_owned)?;
    state
        .world
        .position(state.body)
        .ok_or_else(|| "physics body disappeared".to_string())
}

#[derive(Debug)]
struct LiveReport {
    ticks: u128,
    script_ticks: u64,
    frame_requests: u64,
    missed_frames: u128,
    dropped: Duration,
    last_position: [f32; 3],
}

fn only_actor(
    ecs: &countersplat_runtime::native::Client<WorldOperation, WorldOutcome>,
) -> Result<Entity, String> {
    match call_ecs(
        ecs,
        WorldOperation::Query {
            components: vec!["Position".into()],
        },
    )? {
        WorldOutcome::Entities(entities) if entities.len() == 1 => Ok(entities[0]),
        other => Err(format!("expected one actor, got {other:?}")),
    }
}

fn run(duration: Duration) -> Result<LiveReport, String> {
    let mut ecs = Worker::spawn(
        "countersplat-simulation",
        QUEUE_CAPACITY,
        || Ok(EcsWorld::new()),
        |world, operation| world.apply_operation(operation),
    )
    .map_err(|error| error.to_string())?;
    let mut physics = Worker::spawn(
        "countersplat-physics",
        QUEUE_CAPACITY,
        create_physics,
        tick_physics,
    )
    .map_err(|error| error.to_string())?;
    ecs.wait_ready(TIMEOUT).map_err(|error| error.to_string())?;
    physics
        .wait_ready(TIMEOUT)
        .map_err(|error| error.to_string())?;
    let ecs_client = ecs.client();
    let physics_client = physics.client();
    let lua = ScriptWorker::spawn(
        EcsBridge {
            client: ecs_client.clone(),
            lua_thread: None,
        },
        ScriptOptions::default(),
    )?;
    lua.execute(SCRIPT)?
        .recv_timeout(TIMEOUT)
        .map_err(|error| error.to_string())??;
    let actor = only_actor(&ecs_client)?;

    let fixed = FixedClock::with_rate(Rate::new(60, 1).unwrap(), Duration::from_millis(250), 4)
        .map_err(str::to_owned)?;
    let mut scheduler = Scheduler::new(fixed, Rate::new(60, 1).unwrap());
    let start = Instant::now();
    let mut frames = 0u64;
    let mut missed = 0u128;
    let mut dropped = Duration::ZERO;
    let mut last_position = [0.0, 5.0, 0.0];
    let mut next_report = Duration::from_secs(1);
    let total_ticks = loop {
        let now = Instant::now()
            .checked_duration_since(start)
            .ok_or("monotonic clock moved backwards")?;
        let plan = scheduler.poll(now).map_err(|error| error.to_string())?;
        dropped = dropped.saturating_add(plan.simulation.dropped);
        for _ in 0..plan.simulation.ticks {
            last_position = physics_client
                .try_submit(scheduler.tick_seconds() as f32)
                .map_err(|error| error.to_string())?
                .wait_timeout(TIMEOUT)
                .map_err(|error| error.to_string())?;
            let outcome = call_ecs(
                &ecs_client,
                WorldOperation::WriteComponent {
                    entity: actor,
                    name: "Position".into(),
                    value: json!({"x": last_position[0], "y": last_position[1], "z": last_position[2]}),
                },
            )?;
            if !matches!(outcome, WorldOutcome::Applied) {
                return Err(format!("unexpected ECS reply: {outcome:?}"));
            }
            lua.run_system("fixed_update", scheduler.tick_seconds())?
                .recv_timeout(TIMEOUT)
                .map_err(|error| error.to_string())??;
        }
        if plan.frame.due {
            frames = frames.saturating_add(1);
        }
        missed = missed.saturating_add(plan.frame.missed);
        if now >= next_report {
            println!(
                "{:.2}s: {} simulation ticks, {} frame requests, {} missed, y={:.3}",
                now.as_secs_f64(),
                plan.total_ticks,
                frames,
                missed,
                last_position[1]
            );
            next_report = now.saturating_add(Duration::from_secs(1));
        }
        if now >= duration {
            break plan.total_ticks;
        }
        // Re-read the clock after work: slow workers may already have passed the next deadline.
        let after_work = Instant::now()
            .checked_duration_since(start)
            .ok_or("monotonic clock moved backwards")?;
        let wake = plan.next_wake.unwrap_or(duration).min(duration);
        if let Some(wait) = wake.checked_sub(after_work) {
            if !wait.is_zero() {
                thread::sleep(wait);
            }
        }
    };
    let script_ticks = match call_ecs(
        &ecs_client,
        WorldOperation::ReadResource {
            name: "script_ticks".into(),
        },
    )? {
        WorldOutcome::Value(Some(value)) => value
            .as_u64()
            .ok_or("script tick count was not an integer")?,
        other => return Err(format!("unexpected script tick count: {other:?}")),
    };
    // Lua may synchronously request ECS work, so stop it before the ECS owner.
    lua.shutdown()?;
    physics.shutdown().map_err(|error| error.to_string())?;
    ecs.shutdown().map_err(|error| error.to_string())?;
    Ok(LiveReport {
        ticks: total_ticks,
        script_ticks,
        frame_requests: frames,
        missed_frames: missed,
        dropped,
        last_position,
    })
}

pub(super) fn run_seconds(seconds: f64) -> Result<(), String> {
    let duration = Duration::try_from_secs_f64(seconds)
        .map_err(|_| "duration must be a positive finite number of seconds".to_string())?;
    if duration.is_zero() {
        return Err("duration must be greater than zero".into());
    }
    println!("Running the 60 Hz fixed-step clock and frame-request pacer on real monotonic time.");
    let report = run(duration)?;
    println!("Finished: {} ticks, {} Lua ticks, {} frame requests, {} missed, {:?} discarded; body y={:.3}",
        report.ticks, report.script_ticks, report.frame_requests, report.missed_frames, report.dropped, report.last_position[1]);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn monotonic_loop_drives_real_worker_state() {
        let report =
            run(Duration::from_millis(150)).expect("real-time worker loop should complete");
        assert!(report.ticks > 0);
        assert_eq!(report.ticks, u128::from(report.script_ticks));
        assert!(report.frame_requests > 0);
        assert!(report.last_position[1] < 5.0);
    }
}
